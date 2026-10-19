//! Pipeline-agnostic orientation correction and pixel-diff verification.

use crate::image::format_detect::FormatKind;
use crate::unified_error::{ImgQualityError, Result};
use std::path::Path;

/// Outcome of the selected decoded-image evidence contract.
#[derive(Debug, PartialEq, Eq)]
pub enum PixelDiffResult {
    /// The selected contract passed; JPEG geometry still requires separate byte proof.
    Match,
    /// Diff exceeds the selected tolerance.
    Mismatch { max_delta: u8, channel: u8 },
    /// Proof unavailable because its decoder is absent; callers must not accept it.
    SkippedToolAbsent { tool: &'static str },
}

/// Explicit pixel-diff tolerance selected by output format and encoder
/// guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTolerance {
    /// Exact rendered-pixel match.
    Exact,
    /// An orientation-only request cannot prove a pixel or lossy quality contract.
    JxlOrientation,
    /// JPEG reconstruction orientation/geometry proof; the caller must first
    /// prove byte-identical JPEG reconstruction from the same final output.
    JxlReconstructionOrientation,
    /// Exact rendered sample equivalence; not a source-deletion authorization.
    JxlPixelEquivalent,
    /// Lossless HEIC/HEIF/WebP — one LSB per channel is allowed.
    LsbAvif,
    /// Lossy AVIF (meme mode) — structure must still correlate with the source.
    LossyAvif,
}

impl DiffTolerance {
    const fn max_delta(self) -> u8 {
        match self {
            // Lossy AVIF is handled by its separate quality contract below.
            Self::Exact
            | Self::LossyAvif
            | Self::JxlOrientation
            | Self::JxlReconstructionOrientation
            | Self::JxlPixelEquivalent => 0,
            Self::LsbAvif => 1,
        }
    }
}

#[must_use]
pub const fn pixel_equivalence_diff_tolerance_for_format(fmt: FormatKind) -> Option<DiffTolerance> {
    match fmt {
        FormatKind::Jxl => Some(DiffTolerance::JxlPixelEquivalent),
        _ => orientation_diff_tolerance_for_format(fmt),
    }
}

const JXL_ORIENTATION_LOW_VARIANCE_EPSILON: f64 = 1.0e-6;
const LOSSY_AVIF_MIN_STRUCTURE_CORRELATION: f64 = 0.75;
const LOSSY_AVIF_FLAT_MAX_DELTA: u8 = 32;
const LOSSY_AVIF_MAX_MEAN_RGB_DELTA: f64 = 24.0;
const LOSSY_AVIF_MAX_SINGLE_CHANNEL_DELTA: u8 = 160;

#[must_use]
pub const fn orientation_diff_tolerance_for_format(fmt: FormatKind) -> Option<DiffTolerance> {
    match fmt {
        FormatKind::Jxl
        | FormatKind::Avif
        | FormatKind::Heic
        | FormatKind::Heif
        | FormatKind::WebP => Some(DiffTolerance::Exact),
        FormatKind::Jpeg
        | FormatKind::Png
        | FormatKind::Gif
        | FormatKind::Bmp
        | FormatKind::Tiff
        | FormatKind::Qoi
        | FormatKind::Jp2
        | FormatKind::Ico
        | FormatKind::Exr
        | FormatKind::Flif
        | FormatKind::Psd
        | FormatKind::Pnm
        | FormatKind::Dds
        | FormatKind::Mp4
        | FormatKind::Mov
        | FormatKind::Mkv
        | FormatKind::Webm
        | FormatKind::Unknown => None,
    }
}

/// Strip the residual `Orientation` EXIF tag from an output file (§Orientation,
/// all formats).
///
/// Pixel-encoded outputs may retain or regain the EXIF tag through metadata
/// copying. Non-compliant viewers can re-apply it and double-rotate. Do not use
/// this mutation on JPEG-reconstructible JXL: its original Exif is part of the
/// `jbrd` contract.
///
/// # Errors
/// Returns an error if exiftool is unavailable or exits non-zero.
pub fn strip_residual_orientation_tag(path: &Path) -> Result<()> {
    use crate::image_builders::ExiftoolBuilder;

    if !ExiftoolBuilder::check_available() {
        return Err(ImgQualityError::AnalysisError(
            "exiftool unavailable; cannot strip residual Orientation tag".to_string(),
        ));
    }

    let output = strip_residual_orientation_command(path)
        .output()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "exiftool strip-Orientation failed for {}: {e}",
                path.display()
            ))
        })?;

    if !output.status.success() {
        return Err(ImgQualityError::AnalysisError(format!(
            "exiftool strip-Orientation exited non-zero for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(())
}

pub fn append_strip_residual_orientation_args(builder: &mut crate::ExiftoolBuilder) {
    builder
        .arg("-Orientation=")
        .arg("-IFD0:Orientation=")
        .arg("-IFD1:Orientation=")
        .arg("-EXIF:Orientation=")
        .arg("-XMP:Orientation=")
        .arg("-all:Orientation=");
}

fn strip_residual_orientation_command(path: &Path) -> std::process::Command {
    use crate::ToolBuilder;
    use crate::image_builders::ExiftoolBuilder;

    let mut builder = ExiftoolBuilder::new();
    append_strip_residual_orientation_args(&mut builder);
    builder.overwrite_original().input(path).build()
}

/// Decode the output independently and verify the requested evidence contract.
///
/// Pixel paths compare samples in the source's explicit display domain. JPEG
/// reconstruction separately requires source and codestream orientation/raw
/// geometry agreement; byte reconstruction alone is not a rendering proof.
///
/// Decode tool absent → `PixelDiffResult::SkippedToolAbsent`. \[D11\]
///
/// # Errors
/// Returns an error on decode failure, dimension mismatch, or I/O failure.
pub fn verify_orientation_pixel_diff(
    source_image: &Path,
    output: &Path,
    fmt: FormatKind,
    diff_tolerance: DiffTolerance,
) -> Result<PixelDiffResult> {
    if diff_tolerance == DiffTolerance::JxlReconstructionOrientation && fmt != FormatKind::Jxl {
        return Err(ImgQualityError::AnalysisError(
            "pixel-diff: JPEG reconstruction orientation requires a JXL output".to_string(),
        ));
    }
    let decoder_tool = match fmt {
        FormatKind::Jxl => "djxl",
        FormatKind::Avif => "avifdec",
        FormatKind::Heif | FormatKind::Heic => "heif-convert",
        FormatKind::WebP => "dwebp",
        FormatKind::Jpeg
        | FormatKind::Png
        | FormatKind::Gif
        | FormatKind::Bmp
        | FormatKind::Tiff
        | FormatKind::Qoi
        | FormatKind::Jp2
        | FormatKind::Ico
        | FormatKind::Exr
        | FormatKind::Flif
        | FormatKind::Psd
        | FormatKind::Pnm
        | FormatKind::Dds
        | FormatKind::Mp4
        | FormatKind::Mov
        | FormatKind::Mkv
        | FormatKind::Webm
        | FormatKind::Unknown => {
            return Ok(PixelDiffResult::SkippedToolAbsent {
                tool: "unknown-format",
            });
        }
    };
    let Some(decoder_path) = crate::common_utils::resolve_tool_path(decoder_tool) else {
        tracing::warn!(
            target: "orientation_pixel_diff",
            tool = %decoder_tool,
            output = %output.display(),
            "[D11] decode tool absent — skipping pixel-diff for this file"
        );
        return Ok(PixelDiffResult::SkippedToolAbsent { tool: decoder_tool });
    };
    let float_source = if matches!(
        diff_tolerance,
        DiffTolerance::Exact | DiffTolerance::JxlOrientation | DiffTolerance::JxlPixelEquivalent
    ) && fmt == FormatKind::Jxl
    {
        match crate::image_detection::open_image_with_limits(source_image) {
            Ok(image) => matches!(
                image.color(),
                image::ColorType::Rgb32F | image::ColorType::Rgba32F
            ),
            Err(error) => {
                // Modern integer formats may require their official decoder.
                // The subsequent reference decode must still succeed; this is
                // only output representation selection, never a passed proof.
                tracing::debug!(
                    source = %source_image.display(),
                    %error,
                    "Native float probe unavailable; requiring authoritative reference decode"
                );
                false
            }
        }
    } else {
        false
    };
    let decode_cmd = |inp: &Path, out: &Path| {
        let mut command = std::process::Command::new(&decoder_path);
        command.arg(inp);
        if fmt == FormatKind::WebP {
            command.arg("-o");
        }
        command.arg(out).output()
    };

    let primary_temp_suffix = decode_temp_extension_for_format(fmt).ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "pixel-diff: unsupported decoded temp format for {fmt:?}"
        ))
    })?;
    let tmp_decoded = orientation_decode_tempfile(if float_source {
        ".pfm"
    } else {
        primary_temp_suffix
    })?;

    let decode_output = if fmt == FormatKind::Avif {
        crate::image_builders::avif_decode_preserving_samples_command(output, tmp_decoded.path())?
            .output()
    } else {
        decode_cmd(output, tmp_decoded.path())
    }
    .map_err(|e| {
        ImgQualityError::AnalysisError(format!("pixel-diff: {decoder_tool} spawn failed: {e}"))
    })?;
    if !decode_output.status.success() {
        let stderr = String::from_utf8_lossy(&decode_output.stderr);
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: {decoder_tool} exited non-zero decoding {} ({}): {}",
            output.display(),
            decode_output.status,
            stderr.trim(),
        )));
    }
    log_suppressed_tool_output(
        decoder_tool,
        output,
        &decode_output.stdout,
        &decode_output.stderr,
    );

    if diff_tolerance == DiffTolerance::JxlReconstructionOrientation {
        let source_orientation = read_exif_orientation(source_image)?;
        let source_dimensions = image::image_dimensions(source_image).map_err(|error| {
            ImgQualityError::AnalysisError(format!(
                "pixel-diff: cannot read source JPEG geometry: {error}"
            ))
        })?;
        let decoded_dimensions = image::image_dimensions(tmp_decoded.path()).map_err(|error| {
            ImgQualityError::AnalysisError(format!(
                "pixel-diff: cannot read independently decoded JXL geometry: {error}"
            ))
        })?;
        verify_reconstruction_geometry(
            source_orientation,
            source_dimensions,
            read_jxl_codestream_info(output)?,
            decoded_dimensions,
        )?;
        return Ok(PixelDiffResult::Match);
    }
    verify_pixel_diff_against_decoded_image(source_image, tmp_decoded.path(), diff_tolerance)
}

#[derive(Debug, Clone, Copy)]
struct JxlCodestreamInfo {
    orientation: u8,
    dimensions: (u32, u32),
}

fn verify_reconstruction_geometry(
    source_orientation: u8,
    source_dimensions: (u32, u32),
    codestream: JxlCodestreamInfo,
    decoded_dimensions: (u32, u32),
) -> Result<()> {
    let display_dimensions = if orientation_swaps_dimensions(source_orientation) {
        (source_dimensions.1, source_dimensions.0)
    } else {
        source_dimensions
    };
    if source_dimensions.0 == 0
        || source_dimensions.1 == 0
        || !(1..=8).contains(&source_orientation)
        || source_orientation != codestream.orientation
        || source_dimensions != codestream.dimensions
        || display_dimensions != decoded_dimensions
    {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: JPEG/JXL orientation or geometry mismatch: source_orientation={source_orientation} \
             codestream_orientation={} source_raw={source_dimensions:?} codestream_raw={:?} \
             expected_display={display_dimensions:?} decoded_display={decoded_dimensions:?}",
            codestream.orientation, codestream.dimensions
        )));
    }
    Ok(())
}

fn read_jxl_codestream_info(path: &Path) -> Result<JxlCodestreamInfo> {
    use crate::builder_base::ToolBuilder;

    if !crate::tool_builders::JxlinfoBuilder::check_available() {
        return Err(ImgQualityError::AnalysisError(
            "pixel-diff: jxlinfo unavailable; cannot prove JXL reconstruction orientation".into(),
        ));
    }
    let mut command = crate::tool_builders::JxlinfoBuilder::new()
        .input(path)
        .build();
    command.arg("-v");
    let output = crate::process_runner::ManagedProcess::spawn_captured(&mut command)?
        .wait_timeout(
            std::time::Duration::from_secs(30),
            "JXL codestream orientation",
        )?;
    if !output.status.success() {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: jxlinfo failed for {}: {}",
            path.display(),
            output.stderr.trim()
        )));
    }
    parse_jxlinfo_info(&output.stdout)
}

fn parse_jxlinfo_info(stdout: &str) -> Result<JxlCodestreamInfo> {
    let invalid = || {
        ImgQualityError::AnalysisError(
            "pixel-diff: missing, duplicate or invalid JXL raw codestream dimensions".into(),
        )
    };
    let mut headers = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("JPEG XL image, "));
    let dimensions = headers.next().ok_or_else(invalid)?;
    if headers.next().is_some() {
        return Err(invalid());
    }
    let dimensions = dimensions.split_once(", ").ok_or_else(invalid)?.0;
    let (width, height) = dimensions.split_once('x').ok_or_else(invalid)?;
    let width = width.parse::<u32>().map_err(|_| invalid())?;
    let height = height.parse::<u32>().map_err(|_| invalid())?;
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    Ok(JxlCodestreamInfo {
        orientation: parse_jxlinfo_orientation(stdout)?,
        dimensions: (width, height),
    })
}

fn parse_jxlinfo_orientation(stdout: &str) -> Result<u8> {
    let invalid = || {
        ImgQualityError::AnalysisError(
            "pixel-diff: missing, duplicate or invalid JXL codestream orientation".into(),
        )
    };
    let mut lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("Orientation: "));
    let value = lines.next().ok_or_else(invalid)?;
    if lines.next().is_some() {
        return Err(invalid());
    }
    let (number, label) = value.split_once(' ').ok_or_else(invalid)?;
    let orientation = number.parse::<u8>().map_err(|_| invalid())?;
    if !(1..=8).contains(&orientation) || !label.starts_with('(') || !label.ends_with(')') {
        return Err(invalid());
    }
    Ok(orientation)
}

fn orientation_decode_tempfile(suffix: &str) -> Result<tempfile::NamedTempFile> {
    crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "orientation_pixdiff",
        Some("mfb_orientation_pixdiff"),
        Some(suffix),
    )
    .map_err(|e| ImgQualityError::AnalysisError(format!("pixel-diff: temp alloc failed: {e}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfficialSourceDecoder {
    Avif,
    Heif,
    WebP,
    Jxl,
    LegacyRaster,
}

impl OfficialSourceDecoder {
    const fn tool(self) -> &'static str {
        match self {
            Self::Avif => "avifdec",
            Self::Heif => "heif-convert",
            Self::WebP => "dwebp",
            Self::Jxl => "djxl",
            Self::LegacyRaster => "magick",
        }
    }
}

const fn official_source_decoder(format: FormatKind) -> Option<OfficialSourceDecoder> {
    match format {
        FormatKind::Avif => Some(OfficialSourceDecoder::Avif),
        FormatKind::Heic | FormatKind::Heif => Some(OfficialSourceDecoder::Heif),
        FormatKind::WebP => Some(OfficialSourceDecoder::WebP),
        FormatKind::Jxl => Some(OfficialSourceDecoder::Jxl),
        // The conversion path already uses this adapter for cursor/icon and
        // JPEG 2000 sources unsupported by the Rust image decoder. Decode the
        // original independently here; decoder gaps must not bypass the audit.
        FormatKind::Ico | FormatKind::Jp2 => Some(OfficialSourceDecoder::LegacyRaster),
        _ => None,
    }
}

fn official_source_decode_command(
    decoder: OfficialSourceDecoder,
    executable: &Path,
    source: &Path,
    output: &Path,
) -> Result<std::process::Command> {
    let mut command = std::process::Command::new(executable);
    match decoder {
        OfficialSourceDecoder::Avif => {
            return crate::image_builders::avif_decode_preserving_samples_command(source, output);
        }
        OfficialSourceDecoder::Heif | OfficialSourceDecoder::Jxl => {
            command.arg(source).arg(output);
        }
        OfficialSourceDecoder::WebP => {
            command.arg(source).arg("-o").arg(output);
        }
        OfficialSourceDecoder::LegacyRaster => {
            command.arg(source).args(["-depth", "16"]).arg(output);
        }
    }
    Ok(command)
}

fn decode_source_with_official_tool(
    source_image: &Path,
    format: FormatKind,
) -> Result<image::DynamicImage> {
    let Some(decoder) = official_source_decoder(format) else {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: no official source decoder for {format:?}"
        )));
    };
    let tool = decoder.tool();
    let executable = crate::common_utils::resolve_tool_path(tool).ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "pixel-diff: official {tool} was not found or failed its runtime health check"
        ))
    })?;

    tracing::info!(
        target: "orientation_pixel_diff",
        source = %source_image.display(),
        ?format,
        tool,
        "pixel-diff: decoding unsupported source with official format decoder"
    );

    let decoded_file = orientation_decode_tempfile(".png")?;
    let mut command =
        official_source_decode_command(decoder, &executable, source_image, decoded_file.path())?;
    let output = crate::process_runner::ManagedProcess::spawn_captured(&mut command)
        .and_then(|process| {
            process.wait_liveness_timeout(
                std::time::Duration::from_secs(120),
                crate::process_runner::image_process_hard_timeout(),
                &format!("pixel-diff official {tool} source decode"),
            )
        })
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "pixel-diff: official {tool} source decode failed to run: {e}"
            ))
        })?;

    if !output.status.success() {
        let stderr = output
            .stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("<empty stderr>");
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: official {tool} exited non-zero decoding {}: {stderr}",
            source_image.display()
        )));
    }
    if !decoded_file.path().is_file() {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: official {tool} produced no source image for {}",
            source_image.display()
        )));
    }
    let decoded_size = std::fs::metadata(decoded_file.path())
        .map_err(|error| {
            ImgQualityError::AnalysisError(format!(
                "pixel-diff: cannot inspect official {tool} decoded source image for {}: {error}",
                source_image.display()
            ))
        })?
        .len();
    if decoded_size == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: official {tool} produced an empty source image for {}",
            source_image.display()
        )));
    }
    if decoded_file
        .path()
        .extension()
        .is_some_and(|extension| extension == "png")
        && !crate::image::png_validation::is_true_png(decoded_file.path())?
    {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: official {tool} output failed strict PNG validation for {}",
            source_image.display()
        )));
    }

    crate::image_detection::open_image_with_limits(decoded_file.path()).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "pixel-diff: cannot open official {tool} decoded source image: {e}"
        ))
    })
}

fn verify_pixel_diff_against_decoded_image(
    source_image: &Path,
    decoded_output: &Path,
    tol: DiffTolerance,
) -> Result<PixelDiffResult> {
    let out_img = if decoded_output
        .extension()
        .is_some_and(|extension| extension == "pfm")
    {
        read_float_pfm(decoded_output)
    } else {
        crate::image_detection::open_image_with_limits(decoded_output)
    }
    .map_err(|e| {
        ImgQualityError::AnalysisError(format!("pixel-diff: cannot open decoded output: {e}"))
    })?;

    let source_format = crate::image::format_detect::detect_true_format(source_image)?;
    let src_img_raw = if source_format == FormatKind::Avif {
        // Native AV1 decoders may expose only the primary sample-transform layer.
        decode_source_with_official_tool(source_image, source_format)?
    } else {
        match crate::image_detection::open_image_with_limits(source_image) {
            Ok(img) => img,
            Err(open_error) => decode_source_with_official_tool(source_image, source_format)
                .map_err(|decode_error| {
                    ImgQualityError::AnalysisError(format!(
                        "pixel-diff: source decode failed: native={open_error}; official={decode_error}"
                    ))
                })?,
        }
    };
    let reference_orientation = if source_format == FormatKind::Pnm {
        // A fully decoded PNM/PAM raster has no embedded orientation transform.
        1
    } else {
        read_exif_orientation(source_image)?
    };

    diff_orientation_images(
        &src_img_raw,
        reference_orientation,
        &out_img,
        tol,
        decoded_output,
    )
}

// PFM is the djxl float output without EXR's linear-light conversion. Reject
// unsupported scales/layouts rather than normalizing away float sample bits.
fn read_float_pfm(path: &Path) -> Result<image::DynamicImage> {
    let invalid =
        || ImgQualityError::AnalysisError("pixel-diff: invalid or unsupported float PFM".into());
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::MAX_IMAGE_DECODE_ALLOC_BYTES,
    )
    .map_err(|error| ImgQualityError::AnalysisError(error.to_string()))?;
    let bytes = std::fs::read(path)?;
    let mut parts = bytes.splitn(4, |byte| *byte == b'\n');
    let magic = parts.next().ok_or_else(invalid)?;
    let dimensions =
        std::str::from_utf8(parts.next().ok_or_else(invalid)?).map_err(|_| invalid())?;
    let mut dimensions = dimensions.split_whitespace().map(str::parse::<u32>);
    let (Some(Ok(width)), Some(Ok(height)), None) =
        (dimensions.next(), dimensions.next(), dimensions.next())
    else {
        return Err(invalid());
    };
    let scale = std::str::from_utf8(parts.next().ok_or_else(invalid)?)
        .map_err(|_| invalid())?
        .trim();
    let scale = scale.parse::<f32>().map_err(|_| invalid())?;
    if !matches!(magic, b"PF" | b"Pf")
        || scale.abs().to_bits() != 1.0_f32.to_bits()
        || width == 0
        || height == 0
    {
        return Err(invalid());
    }
    let channels = if magic == b"PF" { 3_u64 } else { 1 };
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(channels * 4))
        .ok_or_else(invalid)?;
    if u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(16))
        .is_none_or(|allocation| allocation > crate::constants::MAX_IMAGE_DECODE_ALLOC_BYTES)
    {
        return Err(invalid());
    }
    let payload = parts.next().ok_or_else(invalid)?;
    if expected != u64::try_from(payload.len()).map_err(|_| invalid())? {
        return Err(invalid());
    }
    // Exact payload length above proves there is no trailing partial sample.
    let samples = payload.as_chunks::<4>().0.iter().map(|bits| {
        if scale.is_sign_negative() {
            f32::from_le_bytes(*bits)
        } else {
            f32::from_be_bytes(*bits)
        }
    });
    let rgb = if channels == 3 {
        samples.collect::<Vec<_>>()
    } else {
        samples.flat_map(|value| [value; 3]).collect::<Vec<_>>()
    };
    let buffer = image::ImageBuffer::from_raw(width, height, rgb).ok_or_else(invalid)?;
    Ok(image::DynamicImage::ImageRgb32F(buffer).flipv())
}

fn first_nonempty_tool_line(bytes: &[u8]) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.lines().map(str::trim).find(|line| !line.is_empty()),
        Err(err) => {
            tracing::debug!(
                error = %err,
                "suppressed orientation tool output was not valid UTF-8"
            );
            None
        }
    }
}

fn log_suppressed_tool_output(tool: &str, output: &Path, stdout: &[u8], stderr: &[u8]) {
    let stdout_line = first_nonempty_tool_line(stdout);
    let stderr_line = first_nonempty_tool_line(stderr);
    if stdout_line.is_some() || stderr_line.is_some() {
        tracing::debug!(
            target: "orientation_pixel_diff",
            tool,
            output = %output.display(),
            stdout = stdout_line.unwrap_or(""),
            stderr = stderr_line.unwrap_or(""),
            "decoder output captured"
        );
    }
}

const fn decode_temp_extension_for_format(fmt: FormatKind) -> Option<&'static str> {
    match fmt {
        FormatKind::Jxl
        | FormatKind::Avif
        | FormatKind::Heic
        | FormatKind::Heif
        | FormatKind::WebP => Some(".png"),
        FormatKind::Jpeg
        | FormatKind::Png
        | FormatKind::Gif
        | FormatKind::Bmp
        | FormatKind::Tiff
        | FormatKind::Qoi
        | FormatKind::Jp2
        | FormatKind::Ico
        | FormatKind::Exr
        | FormatKind::Flif
        | FormatKind::Psd
        | FormatKind::Pnm
        | FormatKind::Dds
        | FormatKind::Mp4
        | FormatKind::Mov
        | FormatKind::Mkv
        | FormatKind::Webm
        | FormatKind::Unknown => None,
    }
}

fn diff_orientation_images(
    raw_source: &image::DynamicImage,
    src_orient: u8,
    out_img: &image::DynamicImage,
    tol: DiffTolerance,
    output: &Path,
) -> Result<PixelDiffResult> {
    let ref_img = apply_orientation_transform(raw_source.clone(), src_orient);
    diff_dynamic_images(&ref_img, out_img, tol, output)
}

const fn orientation_swaps_dimensions(orientation: u8) -> bool {
    matches!(orientation, 5..=8)
}

fn diff_dynamic_images(
    ref_img: &image::DynamicImage,
    out_img: &image::DynamicImage,
    tol: DiffTolerance,
    output: &Path,
) -> Result<PixelDiffResult> {
    let (rw, rh) = (ref_img.width(), ref_img.height());
    let (ow, oh) = (out_img.width(), out_img.height());
    if rw != ow || rh != oh {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: dimension mismatch ref={rw}x{rh} out={ow}x{oh} for {}",
            output.display()
        )));
    }

    if matches!(
        tol,
        DiffTolerance::JxlOrientation | DiffTolerance::JxlReconstructionOrientation
    ) {
        return Err(ImgQualityError::AnalysisError(
            "pixel-diff: orientation-only comparison is not an explicit sample or lossy quality proof".into(),
        ));
    }
    if tol == DiffTolerance::LossyAvif {
        return diff_lossy_avif_structure(ref_img, out_img);
    }
    if matches!(
        tol,
        DiffTolerance::Exact | DiffTolerance::JxlPixelEquivalent
    ) {
        return diff_exact_samples(ref_img, out_img);
    }

    let ref_bytes = ref_img.to_rgb8();
    let out_bytes = out_img.to_rgb8();
    let tolerance = tol.max_delta();
    let mut max_delta = 0;
    let mut max_channel = 0;

    for (rp, op) in ref_bytes.pixels().iter().zip(out_bytes.pixels()) {
        for (channel, (rc, oc)) in rp.0.iter().zip(op.0.iter()).enumerate() {
            let diff = rc.abs_diff(*oc);
            if diff > max_delta {
                max_delta = diff;
                max_channel = crate::numeric_cast::usize_to_u8_sat(channel);
            }
        }
    }

    if max_delta > tolerance {
        return Ok(PixelDiffResult::Mismatch {
            max_delta,
            channel: max_channel,
        });
    }

    Ok(PixelDiffResult::Match)
}

fn diff_exact_samples(
    ref_img: &image::DynamicImage,
    out_img: &image::DynamicImage,
) -> Result<PixelDiffResult> {
    let is_float = |image: &image::DynamicImage| {
        matches!(
            image.color(),
            image::ColorType::Rgb32F | image::ColorType::Rgba32F
        )
    };
    if is_float(ref_img) || is_float(out_img) {
        if !is_float(ref_img) || !is_float(out_img) {
            return Err(ImgQualityError::AnalysisError(
                "pixel-diff: Exact comparison cannot quantize floating-point samples".to_string(),
            ));
        }
        for (index, (reference, output)) in ref_img
            .to_rgba32f()
            .as_raw()
            .iter()
            .zip(out_img.to_rgba32f().as_raw())
            .enumerate()
        {
            // Equality of floats alone would erase signed zero and NaN payloads.
            if reference.to_bits() != output.to_bits() {
                return Ok(PixelDiffResult::Mismatch {
                    max_delta: u8::MAX,
                    channel: crate::numeric_cast::usize_to_u8_sat(index % 4),
                });
            }
        }
        return Ok(PixelDiffResult::Match);
    }

    let ref_samples = ref_img.to_rgba16();
    let out_samples = out_img.to_rgba16();
    for (index, (reference, output)) in ref_samples
        .as_raw()
        .iter()
        .zip(out_samples.as_raw().iter())
        .enumerate()
    {
        let delta = reference.abs_diff(*output);
        if delta != 0 {
            return Ok(PixelDiffResult::Mismatch {
                max_delta: u8::try_from(delta).unwrap_or(u8::MAX),
                channel: crate::numeric_cast::usize_to_u8_sat(index % 4),
            });
        }
    }
    Ok(PixelDiffResult::Match)
}

fn diff_lossy_avif_structure(
    ref_img: &image::DynamicImage,
    out_img: &image::DynamicImage,
) -> Result<PixelDiffResult> {
    let ref_visible = composite_on_black(ref_img);
    let out_visible = composite_on_black(out_img);
    let max_delta = max_rgb_delta(&ref_visible, &out_visible)?;
    let mean_delta = mean_rgb_delta(&ref_visible, &out_visible)?;
    let correlation = pearson_correlation(&luma_samples(&ref_visible), &luma_samples(&out_visible));
    let structure_matches = correlation.is_some_and(|score| {
        score >= LOSSY_AVIF_MIN_STRUCTURE_CORRELATION
            && mean_delta <= LOSSY_AVIF_MAX_MEAN_RGB_DELTA
            && max_delta <= LOSSY_AVIF_MAX_SINGLE_CHANNEL_DELTA
    }) || (correlation.is_none() && max_delta <= LOSSY_AVIF_FLAT_MAX_DELTA);

    if structure_matches {
        Ok(PixelDiffResult::Match)
    } else {
        Ok(PixelDiffResult::Mismatch {
            max_delta,
            channel: 0,
        })
    }
}

fn mean_rgb_delta(ref_img: &image::DynamicImage, out_img: &image::DynamicImage) -> Result<f64> {
    let ref_bytes = ref_img.to_rgb8();
    let out_bytes = out_img.to_rgb8();
    let mut total = 0u64;
    let mut samples = 0u64;
    for (reference, output) in ref_bytes.pixels().iter().zip(out_bytes.pixels()) {
        for (left, right) in reference.0.iter().zip(output.0.iter()) {
            total = total
                .checked_add(u64::from(left.abs_diff(*right)))
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError(
                        "pixel-diff: mean RGB delta accumulation overflowed".to_string(),
                    )
                })?;
            samples = samples.checked_add(1).ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "pixel-diff: mean RGB sample count overflowed".to_string(),
                )
            })?;
        }
    }
    if samples == 0 {
        return Err(ImgQualityError::AnalysisError(
            "pixel-diff: cannot compute mean RGB delta for an empty image".to_string(),
        ));
    }
    Ok(crate::numeric_cast::u64_to_f64(total) / crate::numeric_cast::u64_to_f64(samples))
}

fn composite_on_black(img: &image::DynamicImage) -> image::DynamicImage {
    let rgba = img.to_rgba8();
    image::DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(
        rgba.width(),
        rgba.height(),
        |x, y| {
            let pixel = rgba.get_pixel(x, y).0;
            let alpha = u16::from(pixel[3]);
            image::Rgb([
                u8::try_from((u16::from(pixel[0]) * alpha) / 255).unwrap_or(u8::MAX),
                u8::try_from((u16::from(pixel[1]) * alpha) / 255).unwrap_or(u8::MAX),
                u8::try_from((u16::from(pixel[2]) * alpha) / 255).unwrap_or(u8::MAX),
            ])
        },
    ))
}

fn luma_samples(img: &image::DynamicImage) -> Vec<f64> {
    img.to_rgb8()
        .pixels()
        .iter()
        .map(|pixel| {
            0.114f64.mul_add(
                f64::from(pixel.0[2]),
                0.587f64.mul_add(f64::from(pixel.0[1]), 0.299 * f64::from(pixel.0[0])),
            )
        })
        .collect()
}

fn pearson_correlation(left: &[f64], right: &[f64]) -> Option<f64> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let count = crate::numeric_cast::usize_to_f64(left.len());
    let left_mean = left.iter().sum::<f64>() / count;
    let right_mean = right.iter().sum::<f64>() / count;
    let mut numerator = 0.0;
    let mut left_sq = 0.0;
    let mut right_sq = 0.0;
    for (left_value, right_value) in left.iter().zip(right.iter()) {
        let left_centered = left_value - left_mean;
        let right_centered = right_value - right_mean;
        numerator = left_centered.mul_add(right_centered, numerator);
        left_sq = left_centered.mul_add(left_centered, left_sq);
        right_sq = right_centered.mul_add(right_centered, right_sq);
    }
    if left_sq <= JXL_ORIENTATION_LOW_VARIANCE_EPSILON
        || right_sq <= JXL_ORIENTATION_LOW_VARIANCE_EPSILON
    {
        return None;
    }
    Some(numerator / (left_sq.sqrt() * right_sq.sqrt()))
}

fn max_rgb_delta(ref_img: &image::DynamicImage, out_img: &image::DynamicImage) -> Result<u8> {
    let ref_bytes = ref_img.to_rgb8();
    let out_bytes = out_img.to_rgb8();
    if let Some(max_delta) = ref_bytes
        .pixels()
        .iter()
        .zip(out_bytes.pixels())
        .flat_map(|(rp, op)| {
            rp.0.iter()
                .zip(op.0.iter())
                .map(|(rc, oc)| rc.abs_diff(*oc))
        })
        .max()
    {
        Ok(max_delta)
    } else {
        Err(ImgQualityError::AnalysisError(
            "pixel-diff: no pixels available for orientation delta".to_string(),
        ))
    }
}

/// Read the EXIF Orientation value (1–8) from a file.
fn read_exif_orientation(path: &Path) -> Result<u8> {
    use crate::ToolBuilder;
    use crate::image_builders::ExiftoolBuilder;

    if !ExiftoolBuilder::check_available() {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: exiftool unavailable; cannot determine source orientation for {}",
            path.display()
        )));
    }
    let out = ExiftoolBuilder::new()
        .arg("-n")
        .arg("-s3")
        .arg("-a")
        .arg("-IFD0:Orientation")
        .input(path)
        .build()
        .output()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "pixel-diff: exiftool failed reading orientation for {}: {e}",
                path.display()
            ))
        })?;
    parse_exif_orientation_evidence(path, out.status.success(), &out.stdout, &out.stderr)
}

fn parse_exif_orientation_evidence(
    path: &Path,
    succeeded: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<u8> {
    if !succeeded {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: exiftool exited non-zero reading source orientation for {}: {}",
            path.display(),
            String::from_utf8_lossy(stderr).trim()
        )));
    }
    if stdout.iter().all(u8::is_ascii_whitespace)
        && stderr.iter().any(|byte| !byte.is_ascii_whitespace())
    {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: source orientation absent from incomplete metadata read for {}: {}",
            path.display(),
            String::from_utf8_lossy(stderr).trim()
        )));
    }
    parse_exif_orientation_stdout(path, stdout)
}

fn parse_exif_orientation_stdout(path: &Path, stdout: &[u8]) -> Result<u8> {
    let raw = std::str::from_utf8(stdout).map_err(|error| {
        ImgQualityError::AnalysisError(format!("pixel-diff: invalid orientation text: {error}"))
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(1);
    }
    let orientation = trimmed.parse::<u8>().map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "pixel-diff: invalid numeric orientation for {}: {trimmed:?} ({e})",
            path.display()
        ))
    })?;
    if !(1..=8).contains(&orientation) {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-diff: EXIF orientation out of range for {}: {orientation}",
            path.display()
        )));
    }
    Ok(orientation)
}

/// Apply EXIF orientation transform to produce orientation-1 pixels.
fn apply_orientation_transform(img: image::DynamicImage, orient: u8) -> image::DynamicImage {
    match orient {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiffTolerance, PixelDiffResult, decode_temp_extension_for_format, diff_dynamic_images,
        diff_orientation_images, official_source_decoder, orientation_diff_tolerance_for_format,
        parse_exif_orientation_stdout, read_exif_orientation,
        verify_pixel_diff_against_decoded_image,
    };
    use crate::image::format_detect::FormatKind;
    use crate::unified_error::ImgQualityError;
    use image::{DynamicImage, ImageBuffer, Luma, LumaA, Rgb, Rgba};
    use tempfile::NamedTempFile;

    fn rgb_image(pixel: [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(1, 1, Rgb(pixel)))
    }

    fn pattern_byte(value: u32) -> u8 {
        u8::try_from(value % 256).expect("pattern byte is reduced modulo 256")
    }

    fn patterned_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([
                pattern_byte(x.saturating_mul(40).saturating_add(y.saturating_mul(7))),
                pattern_byte(x.saturating_mul(11).saturating_add(y.saturating_mul(31))),
                pattern_byte(x.saturating_mul(3).saturating_add(y.saturating_mul(19))),
            ])
        }))
    }

    #[test]
    fn exact_match_passes() {
        let ref_img = rgb_image([10, 20, 30]);
        let out_img = rgb_image([10, 20, 30]);

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::Exact,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(result, PixelDiffResult::Match);
    }

    #[test]
    fn jxl_pixel_diff_uses_png_temp_output_not_pnm() {
        assert_eq!(
            decode_temp_extension_for_format(FormatKind::Jxl),
            Some(".png")
        );
    }

    #[test]
    fn unsupported_modern_sources_use_official_decoders() {
        assert_eq!(
            official_source_decoder(FormatKind::Avif).map(super::OfficialSourceDecoder::tool),
            Some("avifdec")
        );
        assert_eq!(
            official_source_decoder(FormatKind::Heic).map(super::OfficialSourceDecoder::tool),
            Some("heif-convert")
        );
        assert_eq!(
            official_source_decoder(FormatKind::WebP).map(super::OfficialSourceDecoder::tool),
            Some("dwebp")
        );
        assert_eq!(
            official_source_decoder(FormatKind::Jxl).map(super::OfficialSourceDecoder::tool),
            Some("djxl")
        );
        assert_eq!(official_source_decoder(FormatKind::Png), None);
    }

    #[test]
    fn reconstruction_requires_source_and_codestream_orientation_agreement() {
        for dimensions in [(32, 32), (32, 48)] {
            for orientation in 1..=8 {
                let display = if super::orientation_swaps_dimensions(orientation) {
                    (dimensions.1, dimensions.0)
                } else {
                    dimensions
                };
                for actual in 1..=8 {
                    let result = super::verify_reconstruction_geometry(
                        orientation,
                        dimensions,
                        super::JxlCodestreamInfo {
                            orientation: actual,
                            dimensions,
                        },
                        display,
                    );
                    assert_eq!(result.is_ok(), orientation == actual);
                }
                assert!(
                    super::verify_reconstruction_geometry(
                        orientation,
                        dimensions,
                        super::JxlCodestreamInfo {
                            orientation,
                            dimensions: (1, 1)
                        },
                        display,
                    )
                    .is_err()
                );
                assert!(
                    super::verify_reconstruction_geometry(
                        orientation,
                        dimensions,
                        super::JxlCodestreamInfo {
                            orientation,
                            dimensions
                        },
                        (1, 1),
                    )
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn jpeg_reconstruction_does_not_hide_short_exif_orientation_loss() {
        use crate::ToolBuilder;

        const CONTRACT: &str = "JPEG reconstruction independent orientation evidence";
        for tool in ["cjxl", "djxl", "jxlinfo", "exiftool"] {
            if crate::common_utils::resolve_tool_path(tool).is_none() {
                crate::test_ci_contract::require_tool_on_path(tool, CONTRACT);
                eprintln!("{CONTRACT}: {tool} unavailable; local codec test not run");
                return;
            }
        }
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("plain.jpg");
        patterned_image(32, 48)
            .save_with_format(&plain, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg = std::fs::read(&plain).unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);

        for complete_ifd in [false, true] {
            // The short IFD retains its readable Orientation entry, but lacks
            // the next-IFD offset; no source repair is permitted by this test.
            let mut exif =
                b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0".to_vec();
            if complete_ifd {
                exif.extend([0; 4]);
            }
            let mut source_bytes = jpeg[..2].to_vec();
            source_bytes.extend([0xff, 0xe1]);
            source_bytes.extend(u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
            source_bytes.extend(exif);
            source_bytes.extend_from_slice(&jpeg[2..]);
            let source = root.path().join(format!("ifd-{complete_ifd}.jpg"));
            let output = source.with_extension("jxl");
            std::fs::write(&source, &source_bytes).unwrap();
            assert_eq!(read_exif_orientation(&source).unwrap(), 6);
            let encoded = crate::CjxlBuilder::new()
                .input(&source)
                .output(&output)
                .effort(1)
                .threads(1)
                .lossless_jpeg(true)
                .allow_jpeg_reconstruction(true)
                .build()
                .output()
                .unwrap();
            assert!(
                encoded.status.success(),
                "{}",
                String::from_utf8_lossy(&encoded.stderr)
            );
            crate::image::fast_img::verify_jxl_roundtrip_integrity(&source, &output).unwrap();
            let actual = super::read_jxl_codestream_info(&output).unwrap();
            let evidence = crate::image::fast_img::verify_final_delivery_integrity(
                &source,
                &output,
                FormatKind::Jxl,
            );
            if complete_ifd || actual.orientation == 6 {
                assert_eq!(actual.orientation, 6);
                evidence.expect("matching explicit source and codestream orientation");
            } else {
                let error =
                    evidence.expect_err("byte reconstruction cannot excuse lost orientation");
                assert!(
                    error
                        .to_string()
                        .contains("JPEG/JXL orientation or geometry mismatch"),
                    "{error}"
                );
            }
            assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        }
    }

    #[test]
    fn jxl_orientation_proof_uses_the_declared_pixel_domain_for_all_orientations() {
        use std::hash::{Hash, Hasher};

        let raw_source = DynamicImage::ImageRgb8(ImageBuffer::from_fn(32, 32, |x, y| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (x, y).hash(&mut hasher);
            let bytes = hasher.finish().to_le_bytes();
            Rgb([bytes[0], bytes[1], bytes[2]])
        }));
        for orientation in 1..=8 {
            let rendered = super::apply_orientation_transform(raw_source.clone(), orientation);
            for tolerance in [DiffTolerance::Exact, DiffTolerance::JxlPixelEquivalent] {
                let result = diff_orientation_images(
                    &raw_source,
                    orientation,
                    &rendered,
                    tolerance,
                    std::path::Path::new("rendered.png"),
                )
                .expect("declared orientation proof");
                assert_eq!(result, PixelDiffResult::Match, "orientation {orientation}");
                if orientation != 1 {
                    assert!(matches!(
                        diff_orientation_images(
                            &raw_source,
                            orientation,
                            &raw_source,
                            tolerance,
                            std::path::Path::new("wrong-domain.png"),
                        )
                        .expect("wrong-domain comparison"),
                        PixelDiffResult::Mismatch { .. }
                    ));
                }
            }
        }
    }

    #[test]
    fn jxl_reconstruction_requires_valid_raw_dimensions() {
        let valid =
            "JPEG XL image, 32x48, (possibly) lossless, 8-bit RGB\nOrientation: 6 (Rotate 90 CW)\n";
        let info = super::parse_jxlinfo_info(valid).unwrap();
        assert_eq!(info.dimensions, (32, 48));
        assert_eq!(info.orientation, 6);
        for header in [
            "",
            "JPEG XL image, 0x48, lossless",
            "JPEG XL image, 32xno, lossless",
            "JPEG XL image, 32x48x3, lossless",
            "JPEG XL animation, 32x48, lossless",
            "JPEG XL image, 32x48, lossless\nJPEG XL image, 32x48, lossless",
        ] {
            assert!(
                super::parse_jxlinfo_info(&format!("{header}\nOrientation: 6 (Rotate 90 CW)\n"))
                    .is_err()
            );
        }
        for dimensions in [(0, 0), (0, 48), (32, 0)] {
            assert!(
                super::verify_reconstruction_geometry(
                    1,
                    dimensions,
                    super::JxlCodestreamInfo {
                        orientation: 1,
                        dimensions
                    },
                    dimensions,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn jxl_codestream_orientation_requires_one_valid_official_field() {
        for orientation in 1..=8 {
            assert_eq!(
                super::parse_jxlinfo_orientation(&format!(
                    "JPEG XL image, 32x32\nOrientation: {orientation} (display geometry)\n"
                ))
                .expect("official orientation field"),
                orientation
            );
        }
        for text in [
            "",
            "Orientation: 0 (invalid)",
            "Orientation: 9 (invalid)",
            "Orientation: unknown",
            "Orientation: 4",
            "Orientation: 4 (flip)\nOrientation: 1 (normal)",
        ] {
            assert!(super::parse_jxlinfo_orientation(text).is_err());
        }
    }

    #[test]
    fn shared_orientation_policy_requires_exact_samples_by_default() {
        for format in [
            FormatKind::Jxl,
            FormatKind::Avif,
            FormatKind::Heic,
            FormatKind::Heif,
            FormatKind::WebP,
        ] {
            assert_eq!(
                orientation_diff_tolerance_for_format(format),
                Some(DiffTolerance::Exact)
            );
            assert!(matches!(
                super::pixel_equivalence_diff_tolerance_for_format(format),
                Some(DiffTolerance::Exact | DiffTolerance::JxlPixelEquivalent)
            ));
        }
    }

    #[test]
    fn strip_residual_orientation_tag_keeps_exiftool_diagnostics_visible() {
        let cmd = super::strip_residual_orientation_command(std::path::Path::new("/tmp/probe.JXL"));
        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|arg| arg == "-Orientation="),
            "strip_residual_orientation_tag must strip Orientation"
        );

        assert!(!args.iter().any(|arg| arg == "-m"));
        assert!(
            args.iter().any(|arg| arg == "-all:Orientation="),
            "strip_residual_orientation_tag must remove grouped Orientation instances"
        );
        for required in [
            "-IFD0:Orientation=",
            "-IFD1:Orientation=",
            "-EXIF:Orientation=",
            "-XMP:Orientation=",
        ] {
            assert!(
                args.iter().any(|arg| arg == required),
                "strip_residual_orientation_tag must remove {required}"
            );
        }
    }

    #[test]
    fn exact_delta_one_mismatches() {
        let ref_img = rgb_image([10, 20, 30]);
        let out_img = rgb_image([11, 20, 30]);

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::Exact,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(
            result,
            PixelDiffResult::Mismatch {
                max_delta: u8::MAX,
                channel: 0
            }
        );
    }

    #[test]
    fn exact_rejects_grayalpha_alpha_only_change() {
        let ref_img = DynamicImage::ImageLumaA8(ImageBuffer::from_pixel(1, 1, LumaA([42, 0])));
        let out_img = DynamicImage::ImageLumaA8(ImageBuffer::from_pixel(1, 1, LumaA([42, 1])));

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::Exact,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(
            result,
            PixelDiffResult::Mismatch {
                max_delta: u8::MAX,
                channel: 3
            }
        );
    }

    #[test]
    fn exact_rejects_rgba16_low_bit_change() {
        let ref_img = DynamicImage::ImageRgba16(ImageBuffer::from_pixel(
            1,
            1,
            Rgba([0x1200, 0x3400, 0x5600, u16::MAX]),
        ));
        let out_img = DynamicImage::ImageRgba16(ImageBuffer::from_pixel(
            1,
            1,
            Rgba([0x1201, 0x3400, 0x5600, u16::MAX]),
        ));

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::Exact,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(
            result,
            PixelDiffResult::Mismatch {
                max_delta: 1,
                channel: 0
            }
        );
    }

    #[test]
    fn orientation_transforms_preserve_integer_and_float_sample_bits() {
        let integer = DynamicImage::ImageRgb16(ImageBuffer::from_fn(2, 3, |x, y| {
            Rgb([0x1201 + u16::try_from(x + y).unwrap(), 0x3403, 0x5605])
        }));
        let float = DynamicImage::ImageRgb32F(ImageBuffer::from_fn(2, 3, |x, y| {
            Rgb([
                f32::from_bits(0x3e00_0001 + x + y),
                -0.0,
                f32::from_bits(0x7fc0_0001),
            ])
        }));
        for orientation in 1..=8 {
            for source in [&integer, &float] {
                let mut rendered = super::apply_orientation_transform(source.clone(), orientation);
                assert_eq!(source.color(), rendered.color());
                assert_eq!(
                    diff_orientation_images(
                        source,
                        orientation,
                        &rendered,
                        DiffTolerance::Exact,
                        std::path::Path::new("rendered.png"),
                    )
                    .unwrap(),
                    PixelDiffResult::Match
                );
                match &mut rendered {
                    DynamicImage::ImageRgb16(pixels) => pixels.get_pixel_mut(0, 0).0[0] ^= 1,
                    DynamicImage::ImageRgb32F(pixels) => {
                        let sample = &mut pixels.get_pixel_mut(0, 0).0[0];
                        *sample = f32::from_bits(sample.to_bits() ^ 1);
                    }
                    _ => panic!("orientation changed the source sample type"),
                }
                assert!(matches!(
                    diff_orientation_images(
                        source,
                        orientation,
                        &rendered,
                        DiffTolerance::Exact,
                        std::path::Path::new("mutated.png"),
                    )
                    .unwrap(),
                    PixelDiffResult::Mismatch { .. }
                ));
            }
        }
    }

    #[test]
    fn exact_accepts_equivalent_gray_and_rgb_8_to_16_samples() {
        let cases = [
            (
                DynamicImage::ImageLuma8(ImageBuffer::from_pixel(1, 1, Luma([42]))),
                DynamicImage::ImageLuma16(ImageBuffer::from_pixel(1, 1, Luma([0x2a2a]))),
            ),
            (
                DynamicImage::ImageRgb8(ImageBuffer::from_pixel(1, 1, Rgb([12, 34, 56]))),
                DynamicImage::ImageRgb16(ImageBuffer::from_pixel(
                    1,
                    1,
                    Rgb([0x0c0c, 0x2222, 0x3838]),
                )),
            ),
        ];

        for (ref_img, out_img) in cases {
            assert_eq!(
                diff_dynamic_images(
                    &ref_img,
                    &out_img,
                    DiffTolerance::Exact,
                    std::path::Path::new("out.jxl"),
                )
                .unwrap(),
                PixelDiffResult::Match
            );
        }
    }

    #[test]
    fn exact_preserves_float_bits_instead_of_quantizing_them() {
        let ref_img =
            DynamicImage::ImageRgb32F(ImageBuffer::from_pixel(1, 1, Rgb([0.5, 0.0, 1.0])));
        assert_eq!(
            diff_dynamic_images(
                &ref_img,
                &ref_img,
                DiffTolerance::Exact,
                std::path::Path::new("out.jxl")
            )
            .unwrap(),
            PixelDiffResult::Match
        );
        let negative_zero =
            DynamicImage::ImageRgb32F(ImageBuffer::from_pixel(1, 1, Rgb([0.5, -0.0, 1.0])));
        assert!(matches!(
            diff_dynamic_images(
                &ref_img,
                &negative_zero,
                DiffTolerance::Exact,
                std::path::Path::new("out.jxl")
            )
            .unwrap(),
            PixelDiffResult::Mismatch { channel: 1, .. }
        ));
        let out_img = DynamicImage::ImageRgb16(ref_img.to_rgb16());

        let error = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::Exact,
            std::path::Path::new("out.jxl"),
        )
        .expect_err("Exact must reject float samples rather than quantize them");

        assert!(
            error
                .to_string()
                .contains("cannot quantize floating-point samples")
        );
    }

    #[test]
    fn float_pfm_proof_preserves_bits_and_rejects_invalid_layouts() {
        let file = NamedTempFile::new().unwrap();
        for little in [false, true] {
            let mut bytes = format!("PF\n1 1\n{}1.0\n", if little { "-" } else { "" }).into_bytes();
            for bits in [0x8000_0000_u32, 0x7fc0_0001, 0x3e00_0001] {
                bytes.extend(if little {
                    bits.to_le_bytes()
                } else {
                    bits.to_be_bytes()
                });
            }
            std::fs::write(file.path(), &bytes).unwrap();
            let decoded = super::read_float_pfm(file.path()).unwrap().to_rgb32f();
            assert_eq!(
                decoded
                    .as_raw()
                    .iter()
                    .map(|sample| sample.to_bits())
                    .collect::<Vec<_>>(),
                [0x8000_0000, 0x7fc0_0001, 0x3e00_0001]
            );
            bytes.pop();
            std::fs::write(file.path(), bytes).unwrap();
            assert!(super::read_float_pfm(file.path()).is_err());
        }
        for invalid in [
            b"PF\n1 1\nNaN\n".as_slice(),
            b"PF\n1 1 1\n-1\n",
            b"PF\n4294967295 4294967295\n-1\n",
            b"PF\n1 1\n-2\n",
        ] {
            std::fs::write(file.path(), invalid).unwrap();
            assert!(super::read_float_pfm(file.path()).is_err());
        }
    }

    #[test]
    fn avif_delta_one_matches() {
        let ref_img = rgb_image([10, 20, 30]);
        let out_img = rgb_image([11, 20, 30]);

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::LsbAvif,
            std::path::Path::new("out.avif"),
        )
        .unwrap();

        assert_eq!(result, PixelDiffResult::Match);
    }

    #[test]
    fn orientation_only_cannot_authorize_an_image_contract() {
        let ref_img = rgb_image([10, 20, 30]);
        let out_img = rgb_image([210, 40, 90]);

        for candidate in [&ref_img, &out_img] {
            assert!(
                diff_dynamic_images(
                    &ref_img,
                    candidate,
                    DiffTolerance::JxlOrientation,
                    std::path::Path::new("out.jxl"),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn jxl_pixel_equivalence_rejects_flat_color_replacement() {
        let ref_img = rgb_image([10, 20, 30]);
        let out_img = rgb_image([210, 40, 90]);

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::JxlPixelEquivalent,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(
            result,
            PixelDiffResult::Mismatch {
                max_delta: u8::MAX,
                channel: 0
            }
        );
    }

    #[test]
    fn jxl_pixel_equivalence_rejects_one_lsb_decoder_drift() {
        let ref_img = rgb_image([10, 20, 30]);
        let out_img = rgb_image([11, 20, 30]);

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::JxlPixelEquivalent,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert!(matches!(result, PixelDiffResult::Mismatch { .. }));
    }

    #[test]
    fn jxl_pixel_equivalence_rejects_correlated_color_changes() {
        let ref_img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(3, 2, |x, y| {
            let base = crate::numeric_cast::u32_shifted_byte_to_u8(x * 70 + y * 20, 0);
            Rgb([base, base.saturating_add(10), base.saturating_add(20)])
        }));
        let out_img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(3, 2, |x, y| {
            let base = crate::numeric_cast::u32_shifted_byte_to_u8(x * 70 + y * 20, 0);
            Rgb([
                base.saturating_add(80),
                base.saturating_add(90),
                base.saturating_add(100),
            ])
        }));

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::JxlPixelEquivalent,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert!(matches!(result, PixelDiffResult::Mismatch { .. }));
    }

    #[test]
    fn jxl_orientation_rejects_structural_mismatch() {
        let ref_img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(3, 2, |x, y| {
            let base = crate::numeric_cast::u32_shifted_byte_to_u8(x * 70 + y * 20, 0);
            Rgb([base, base, base])
        }));
        let out_img = DynamicImage::ImageRgb8(image::imageops::rotate180(&ref_img.to_rgb8()));

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::JxlPixelEquivalent,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(
            result,
            PixelDiffResult::Mismatch {
                max_delta: u8::MAX,
                channel: 0
            }
        );
    }

    #[test]
    fn lossy_avif_accepts_compression_drift_when_structure_matches() {
        let ref_img = patterned_image(8, 8);
        let out_img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(8, 8, |x, y| {
            let source = ref_img.to_rgb8().get_pixel(x, y).0;
            Rgb([
                source[0].saturating_add(8),
                source[1].saturating_add(8),
                source[2].saturating_add(8),
            ])
        }));

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::LossyAvif,
            std::path::Path::new("out.avif"),
        )
        .unwrap();

        assert_eq!(result, PixelDiffResult::Match);
    }

    #[test]
    fn lossy_avif_rejects_severe_monotonic_color_cast() {
        let ref_img = patterned_image(8, 8);
        let out_img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(8, 8, |x, y| {
            let source = ref_img.to_rgb8().get_pixel(x, y).0;
            Rgb([
                source[0].saturating_add(96),
                source[1].saturating_add(96),
                source[2].saturating_add(96),
            ])
        }));

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::LossyAvif,
            std::path::Path::new("out.avif"),
        )
        .unwrap();

        assert!(matches!(result, PixelDiffResult::Mismatch { .. }));
    }

    #[test]
    fn lossy_avif_rejects_unrelated_same_size_image() {
        let ref_img = patterned_image(8, 8);
        let out_img = DynamicImage::ImageRgb8(image::imageops::rotate180(&ref_img.to_rgb8()));

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::LossyAvif,
            std::path::Path::new("out.avif"),
        )
        .unwrap();

        assert!(matches!(result, PixelDiffResult::Mismatch { .. }));
    }

    #[test]
    fn lossy_avif_ignores_invisible_rgb_drift_under_full_transparency() {
        let ref_img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([0, 0, 0, 0])));
        let out_img =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([128, 128, 128, 0])));

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::LossyAvif,
            std::path::Path::new("out.avif"),
        )
        .unwrap();

        assert_eq!(result, PixelDiffResult::Match);
    }

    #[test]
    fn avif_delta_two_mismatches() {
        let ref_img = rgb_image([10, 20, 30]);
        let out_img = rgb_image([12, 20, 30]);

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::LsbAvif,
            std::path::Path::new("out.avif"),
        )
        .unwrap();

        assert_eq!(
            result,
            PixelDiffResult::Mismatch {
                max_delta: 2,
                channel: 0
            }
        );
    }

    #[test]
    fn all_eight_orientation_transforms_are_defined() {
        let img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(2, 3, |x, y| {
            let pixel = match (x, y) {
                (0, 0) => [0, 0, 0],
                (1, 0) => [1, 0, 1],
                (0, 1) => [0, 1, 1],
                (1, 1) => [1, 1, 2],
                (0, 2) => [0, 2, 2],
                (1, 2) => [1, 2, 3],
                _ => [9, 9, 9],
            };
            Rgb(pixel)
        }));
        let expected = [
            (1, 2, 3),
            (2, 2, 3),
            (3, 2, 3),
            (4, 2, 3),
            (5, 3, 2),
            (6, 3, 2),
            (7, 3, 2),
            (8, 3, 2),
        ];

        for (orientation, width, height) in expected {
            let transformed = super::apply_orientation_transform(img.clone(), orientation);
            assert_eq!(
                (transformed.width(), transformed.height()),
                (width, height),
                "orientation {orientation} dimensions"
            );
        }
    }

    #[test]
    fn no_orientation_tag_defaults_to_one() {
        assert_eq!(
            parse_exif_orientation_stdout(std::path::Path::new("source.png"), b"\n").unwrap(),
            1
        );
    }

    #[test]
    fn failed_source_orientation_read_cannot_default_to_one() {
        let tmp = NamedTempFile::new().unwrap();
        assert!(read_exif_orientation(tmp.path()).is_err());
    }

    #[test]
    fn pnm_reference_requires_full_raster_decode_and_exact_samples() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("misleading.jpg");
        let output = root.path().join("decoded.png");
        let header = b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n";
        let pixels = [255, 0, 0, 0, 255, 0];
        let mut bytes = header.to_vec();
        bytes.extend_from_slice(&pixels);
        std::fs::write(&source, &bytes).unwrap();
        let image = ImageBuffer::<Rgb<u8>, _>::from_raw(2, 1, pixels.to_vec()).unwrap();
        image.save(&output).unwrap();
        assert_eq!(
            verify_pixel_diff_against_decoded_image(&source, &output, DiffTolerance::Exact)
                .unwrap(),
            PixelDiffResult::Match
        );
        let mut changed = image;
        changed.get_pixel_mut(1, 0).0[0] = 1;
        changed.save(&output).unwrap();
        assert!(matches!(
            verify_pixel_diff_against_decoded_image(&source, &output, DiffTolerance::Exact)
                .unwrap(),
            PixelDiffResult::Mismatch { .. }
        ));
        std::fs::write(&source, header).unwrap();
        assert!(
            verify_pixel_diff_against_decoded_image(&source, &output, DiffTolerance::Exact)
                .is_err()
        );
    }

    #[test]
    fn source_orientation_defaults_require_a_successful_complete_read() {
        let source = std::path::Path::new("source.jpg");
        for (succeeded, stdout, stderr) in [
            (false, b"".as_slice(), b"read failed".as_slice()),
            (false, b"6\n", b"read failed"),
            (true, b"\n", b"Warning: metadata truncated"),
        ] {
            assert!(
                super::parse_exif_orientation_evidence(source, succeeded, stdout, stderr).is_err()
            );
        }
        assert_eq!(
            super::parse_exif_orientation_evidence(source, true, b"\n", b"").unwrap(),
            1
        );
        assert_eq!(
            super::parse_exif_orientation_evidence(source, true, b"6\n", b"").unwrap(),
            6
        );
    }

    #[test]
    fn invalid_orientation_stdout_fails_closed() {
        let tmp = NamedTempFile::new().unwrap();

        assert!(parse_exif_orientation_stdout(tmp.path(), b"Rotate 90 CW\n").is_err());
    }

    #[test]
    fn invalid_or_conflicting_orientation_stdout_fails_closed() {
        let tmp = NamedTempFile::new().unwrap();

        for text in [
            b"0\n".as_slice(),
            b"9\n",
            b"6\n3\n",
            b"6\n6\n",
            b"6 extra\n",
            b"\xff",
        ] {
            assert!(parse_exif_orientation_stdout(tmp.path(), text).is_err());
        }
    }

    #[test]
    fn test_verify_pixel_diff_heic_magic_recognition() {
        use std::io::Write;
        let mut heic_stub = tempfile::Builder::new().suffix(".HEIC").tempfile().unwrap();
        heic_stub
            .write_all(&[
                0, 0, 0, 16, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c', 0, 0, 0, 0,
            ])
            .unwrap();

        let decoded_stub = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        // Save a valid 1x1 PNG to the decoded stub path
        let ref_img = image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            1,
            1,
            image::Rgb([10, 20, 30]),
        ));
        ref_img
            .save_with_format(decoded_stub.path(), image::ImageFormat::Png)
            .unwrap();

        let result = super::decode_source_with_official_tool(
            heic_stub.path(),
            crate::image::format_detect::detect_true_format(heic_stub.path()).unwrap(),
        );

        match result {
            Err(ImgQualityError::AnalysisError(msg)) => {
                assert!(
                    msg.contains("heif-convert"),
                    "Expected heif-convert execution attempt, got: {msg}"
                );
            }
            other => panic!("Expected AnalysisError from heif-convert, got {other:?}"),
        }
    }

    #[test]
    fn test_verify_pixel_diff_dimension_mismatch_returns_analysis_error() {
        let f1 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        // Save a 1x1 PNG to f1
        let img1 = image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            1,
            1,
            image::Rgb([10, 20, 30]),
        ));
        img1.save_with_format(f1.path(), image::ImageFormat::Png)
            .unwrap();

        let f2 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        // Save a 2x2 PNG to f2 (different dimensions!)
        let img2 = image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            2,
            2,
            image::Rgb([10, 20, 30]),
        ));
        img2.save_with_format(f2.path(), image::ImageFormat::Png)
            .unwrap();

        let result =
            verify_pixel_diff_against_decoded_image(f1.path(), f2.path(), DiffTolerance::Exact);

        match result {
            Err(ImgQualityError::AnalysisError(msg)) => {
                assert!(
                    msg.contains("dimension mismatch"),
                    "Expected dimension mismatch error, got: {msg}"
                );
            }
            other => panic!("Expected AnalysisError(dimension mismatch), got {other:?}"),
        }
    }
}
