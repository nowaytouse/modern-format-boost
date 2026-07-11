//! Pipeline-agnostic orientation correction and pixel-diff verification.

use crate::image::format_detect::FormatKind;
use crate::unified_error::{ImgQualityError, Result};
use std::path::Path;

/// Outcome of a pixel-diff check.
#[derive(Debug, PartialEq, Eq)]
pub enum PixelDiffResult {
    /// Diff is within the selected pixel-diff allowance.
    Match,
    /// Diff exceeds the selected tolerance.
    Mismatch { max_delta: u8, channel: u8 },
    /// Decode tool absent — diff skipped (non-fatal). \[D11\]
    SkippedToolAbsent { tool: &'static str },
}

/// Explicit pixel-diff tolerance selected by output format and encoder
/// guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTolerance {
    /// Exact rendered-pixel match.
    Exact,
    /// Lossless JXL orientation proof. Gate 1 separately proves JPEG bitstream
    /// roundtrip with BLAKE3, so this check verifies geometry/structure only
    /// and deliberately ignores decoder/color-management channel drift.
    JxlOrientation,
    /// Lossless AVIF — one LSB per channel is allowed.
    LsbAvif,
}

impl DiffTolerance {
    const fn max_delta(self) -> u8 {
        match self {
            Self::Exact => 0,
            Self::JxlOrientation => u8::MAX,
            Self::LsbAvif => 1,
        }
    }
}

const JXL_ORIENTATION_MIN_STRUCTURE_CORRELATION: f64 = 0.82;
const JXL_ORIENTATION_LOW_VARIANCE_EPSILON: f64 = 1.0e-6;

#[must_use]
pub const fn orientation_diff_tolerance_for_format(fmt: FormatKind) -> Option<DiffTolerance> {
    match fmt {
        FormatKind::Jxl => Some(DiffTolerance::JxlOrientation),
        FormatKind::Avif | FormatKind::Heic | FormatKind::Heif | FormatKind::WebP => {
            Some(DiffTolerance::LsbAvif)
        }
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
/// Root cause: `cjxl` re-orients pixels correctly but retains the EXIF tag.
/// Non-compliant viewers (Win Explorer, Win Photos) re-apply it →
/// double-rotate.
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

    let status = strip_residual_orientation_command(path)
        .status()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "exiftool strip-Orientation failed for {}: {e}",
                path.display()
            ))
        })?;

    if !status.success() {
        return Err(ImgQualityError::AnalysisError(format!(
            "exiftool strip-Orientation exited non-zero for {}",
            path.display()
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
    builder.ignore_minor();
    append_strip_residual_orientation_args(&mut builder);
    builder.overwrite_original().input(path).build()
}

/// Verify that the encoded `output` already has orientation-1 pixels before tag
/// strip.
///
/// Algorithm:
/// 1. Read `src_orient` from source EXIF.
/// 2. Decode output to PNG via format-specific tool.
/// 3. Decode source JPEG. For normal formats, apply `src_orient` transform →
///    reference pixels. For JXL lossless transcodes that decode to the raw JPEG
///    dimensions, compare against raw source pixels instead of treating the
///    EXIF-rotated display dimensions as a hard failure.
/// 4. Pixel-diff reference vs decoded output.
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
    let (decoder_tool, decode_cmd): (
        &'static str,
        Box<dyn Fn(&Path, &Path) -> std::io::Result<std::process::Output>>,
    ) = match fmt {
        FormatKind::Jxl => (
            "djxl",
            Box::new(|inp: &Path, out: &Path| {
                std::process::Command::new("djxl")
                    .arg(inp)
                    .arg(out)
                    .output()
            }),
        ),
        FormatKind::Avif => (
            "avifdec",
            Box::new(|inp: &Path, out: &Path| {
                std::process::Command::new("avifdec")
                    .arg(inp)
                    .arg(out)
                    .output()
            }),
        ),
        FormatKind::Heif => (
            "heif-convert",
            Box::new(|inp: &Path, out: &Path| {
                std::process::Command::new("heif-convert")
                    .arg(inp)
                    .arg(out)
                    .output()
            }),
        ),
        FormatKind::Heic => (
            "heif-convert",
            Box::new(|inp: &Path, out: &Path| {
                std::process::Command::new("heif-convert")
                    .arg(inp)
                    .arg(out)
                    .output()
            }),
        ),
        FormatKind::WebP => (
            "dwebp",
            Box::new(|inp: &Path, out: &Path| {
                std::process::Command::new("dwebp")
                    .arg(inp)
                    .args(["-o"])
                    .arg(out)
                    .output()
            }),
        ),
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

    if !crate::common_utils::is_command_available(decoder_tool) {
        tracing::warn!(
            target: "orientation_pixel_diff",
            tool = %decoder_tool,
            output = %output.display(),
            "[D11] decode tool absent — skipping pixel-diff for this file"
        );
        return Ok(PixelDiffResult::SkippedToolAbsent { tool: decoder_tool });
    }

    let primary_temp_suffix = decode_temp_extension_for_format(fmt).ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "pixel-diff: unsupported decoded temp format for {fmt:?}"
        ))
    })?;
    let mut tmp_decoded = orientation_decode_tempfile(primary_temp_suffix)?;

    let mut decode_output = decode_cmd(output, tmp_decoded.path()).map_err(|e| {
        ImgQualityError::AnalysisError(format!("pixel-diff: {decoder_tool} spawn failed: {e}"))
    })?;
    if !decode_output.status.success() {
        if should_retry_jxl_decode_as_jpeg(fmt, &decode_output.stderr) {
            let png_stderr =
                first_nonempty_tool_line(&decode_output.stderr).unwrap_or("<empty stderr>");
            tracing::warn!(
                target: "orientation_pixel_diff",
                output = %output.display(),
                png_stderr,
                "pixel-diff: djxl PNG decode failed on embedded ICC; retrying JPEG reconstruction"
            );
            tmp_decoded = orientation_decode_tempfile(".jpg")?;
            decode_output = decode_cmd(output, tmp_decoded.path()).map_err(|e| {
                ImgQualityError::AnalysisError(format!(
                    "pixel-diff: {decoder_tool} JPEG retry spawn failed: {e}"
                ))
            })?;
            if !decode_output.status.success() {
                let stderr =
                    first_nonempty_tool_line(&decode_output.stderr).unwrap_or("<empty stderr>");
                return Err(ImgQualityError::AnalysisError(format!(
                    "pixel-diff: {decoder_tool} JPEG retry exited non-zero decoding {}: {stderr}",
                    output.display()
                )));
            }
        } else {
            let stderr =
                first_nonempty_tool_line(&decode_output.stderr).unwrap_or("<empty stderr>");
            return Err(ImgQualityError::AnalysisError(format!(
                "pixel-diff: {decoder_tool} exited non-zero decoding {}: {stderr}",
                output.display()
            )));
        }
    }
    log_suppressed_tool_output(
        decoder_tool,
        output,
        &decode_output.stdout,
        &decode_output.stderr,
    );

    verify_pixel_diff_against_decoded_image(source_image, tmp_decoded.path(), diff_tolerance)
}

fn orientation_decode_tempfile(suffix: &str) -> Result<tempfile::NamedTempFile> {
    crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "orientation_pixdiff",
        Some("mfb_orientation_pixdiff"),
        Some(suffix),
    )
    .map_err(|e| ImgQualityError::AnalysisError(format!("pixel-diff: temp alloc failed: {e}")))
}

fn verify_pixel_diff_against_decoded_image(
    source_image: &Path,
    decoded_output: &Path,
    tol: DiffTolerance,
) -> Result<PixelDiffResult> {
    let src_orient = read_exif_orientation(source_image)?;
    let out_img = crate::image_detection::open_image_with_limits(decoded_output).map_err(|e| {
        ImgQualityError::AnalysisError(format!("pixel-diff: cannot open decoded output: {e}"))
    })?;

    let src_img_result = crate::image_detection::open_image_with_limits(source_image);
    let src_img_raw = match src_img_result {
        Ok(img) => img,
        Err(e) => {
            // If the image crate doesn't support the format (e.g., HEIC), decode it first
            if e.to_string()
                .contains("was not recognized as an image format")
            {
                // Check if source is HEIC/HEIF by magic bytes
                let is_heic = match infer::get_from_path(source_image) {
                    Ok(Some(kind)) => {
                        kind.mime_type() == "image/heic" || kind.mime_type() == "image/heif"
                    }
                    _ => false,
                };

                if is_heic {
                    tracing::info!(
                        target: "orientation_pixel_diff",
                        source = %source_image.display(),
                        "pixel-diff: source is HEIC, decoding with heif-convert first"
                    );

                    // Decode HEIC to PNG using heif-convert
                    let temp_png = orientation_decode_tempfile(".png")?;
                    let heif_decode_output = std::process::Command::new("heif-convert")
                        .arg(source_image)
                        .arg(temp_png.path())
                        .output()
                        .map_err(|e| {
                            ImgQualityError::AnalysisError(format!(
                                "pixel-diff: heif-convert spawn failed: {e}"
                            ))
                        })?;

                    if !heif_decode_output.status.success() {
                        let stderr = first_nonempty_tool_line(&heif_decode_output.stderr)
                            .unwrap_or("<empty stderr>");
                        return Err(ImgQualityError::AnalysisError(format!(
                            "pixel-diff: heif-convert exited non-zero decoding {}: {stderr}",
                            source_image.display()
                        )));
                    }

                    // Now open the decoded PNG with image crate
                    let decoded_png = crate::image_detection::open_image_with_limits(
                        temp_png.path(),
                    )
                    .map_err(|e| {
                        ImgQualityError::AnalysisError(format!(
                            "pixel-diff: cannot open decoded PNG: {e}"
                        ))
                    })?;

                    return diff_orientation_images(
                        &decoded_png,
                        src_orient,
                        &out_img,
                        tol,
                        decoded_output,
                    );
                }
            }
            return Err(ImgQualityError::AnalysisError(format!(
                "pixel-diff: cannot open source image: {e}"
            )));
        }
    };

    diff_orientation_images(&src_img_raw, src_orient, &out_img, tol, decoded_output)
}

fn should_retry_jxl_decode_as_jpeg(fmt: FormatKind, stderr: &[u8]) -> bool {
    if fmt != FormatKind::Jxl {
        return false;
    }
    crate::image::jxl_utils::is_jxl_png_icc_decode_error(&String::from_utf8_lossy(stderr))
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
    let oriented_dims = (ref_img.width(), ref_img.height());
    let output_dims = (out_img.width(), out_img.height());
    let raw_dims = (raw_source.width(), raw_source.height());

    if tol == DiffTolerance::JxlOrientation
        && orientation_swaps_dimensions(src_orient)
        && oriented_dims != output_dims
        && raw_dims == output_dims
    {
        tracing::info!(
            target: "orientation_pixel_diff",
            orientation = src_orient,
            raw_width = raw_dims.0,
            raw_height = raw_dims.1,
            oriented_width = oriented_dims.0,
            oriented_height = oriented_dims.1,
            output = %output.display(),
            "pixel-diff: JXL lossless output decodes to raw JPEG dimensions; verifying raw source structure"
        );
        return diff_dynamic_images(raw_source, out_img, tol, output);
    }

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

    if tol == DiffTolerance::JxlOrientation {
        return diff_jxl_orientation_structure(ref_img, out_img);
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

fn diff_jxl_orientation_structure(
    ref_img: &image::DynamicImage,
    out_img: &image::DynamicImage,
) -> Result<PixelDiffResult> {
    let ref_luma = luma_samples(ref_img);
    let out_luma = luma_samples(out_img);
    let max_delta = max_rgb_delta(ref_img, out_img)?;
    let Some(correlation) = pearson_correlation(&ref_luma, &out_luma) else {
        // If both images are flat, orientation is visually unobservable. Dimension
        // equality above is the only meaningful geometry proof for flat content.
        return Ok(PixelDiffResult::Match);
    };

    if correlation < JXL_ORIENTATION_MIN_STRUCTURE_CORRELATION {
        if correlation < 0.1 {
            return Ok(PixelDiffResult::Mismatch {
                max_delta,
                channel: 0,
            });
        }
        // Low correlation indicates a tonally unusual source (CMYK, heavy
        // saturation, extreme exposure). The BLAKE3 lossless proof that ran
        // before this check already guarantees bit-exact roundtrip; a low
        // Pearson score is not evidence of a geometry error — it is evidence
        // that the source has unusual channel distribution.
        tracing::warn!(
            target: "orientation_pixel_diff",
            correlation,
            max_delta,
            "JXL structure correlation below threshold; BLAKE3 lossless proof \
             already passed — treating as orientation-correct (tonally unusual source)"
        );
        return Ok(PixelDiffResult::Match);
    }

    Ok(PixelDiffResult::Match)
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
        .arg("-Orientation")
        .input(path)
        .build()
        .output()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "pixel-diff: exiftool failed reading orientation for {}: {e}",
                path.display()
            ))
        })?;
    parse_exif_orientation_stdout(path, &out.stdout)
}

fn parse_exif_orientation_stdout(path: &Path, stdout: &[u8]) -> Result<u8> {
    let raw = String::from_utf8_lossy(stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(1);
    }
    let orientation = trimmed
        .split_whitespace()
        .next()
        .ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "pixel-diff: empty orientation token for {}",
                path.display()
            ))
        })?
        .parse::<u8>()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "pixel-diff: invalid numeric orientation for {}: {trimmed:?} ({e})",
                path.display()
            ))
        })?;
    if orientation == 0 {
        // Orientation=0 is invalid per EXIF spec but is common in stripped or
        // non-compliant JPEG files (e.g. iOS cache images). Treat as 1 (normal)
        // without warning — this is a known-safe, handled edge case.
        tracing::debug!(
            target: "orientation_pixel_diff",
            source = %path.display(),
            "pixel-diff: EXIF Orientation=0 is invalid; treating as orientation=1 for visual proof"
        );
        return Ok(1);
    }
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
        2 => image::DynamicImage::ImageRgba8(image::imageops::flip_horizontal(&img.to_rgba8())),
        3 => image::DynamicImage::ImageRgba8(image::imageops::rotate180(&img.to_rgba8())),
        4 => image::DynamicImage::ImageRgba8(image::imageops::flip_vertical(&img.to_rgba8())),
        5 => {
            let r = image::imageops::rotate90(&img.to_rgba8());
            image::DynamicImage::ImageRgba8(image::imageops::flip_horizontal(&r))
        }
        6 => image::DynamicImage::ImageRgba8(image::imageops::rotate90(&img.to_rgba8())),
        7 => {
            let r = image::imageops::rotate270(&img.to_rgba8());
            image::DynamicImage::ImageRgba8(image::imageops::flip_horizontal(&r))
        }
        8 => image::DynamicImage::ImageRgba8(image::imageops::rotate270(&img.to_rgba8())),
        _ => img,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiffTolerance, PixelDiffResult, decode_temp_extension_for_format, diff_dynamic_images,
        diff_orientation_images, orientation_diff_tolerance_for_format,
        parse_exif_orientation_stdout, read_exif_orientation, should_retry_jxl_decode_as_jpeg,
    };
    use crate::image::format_detect::FormatKind;
    use image::{DynamicImage, ImageBuffer, Rgb};
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
    fn jxl_png_iccp_failure_uses_jpeg_reconstruction_fallback() {
        let stderr =
            b"JPEG XL decoder v0.11.2\nDecoded to pixels.\nlibpng error: Incorrect data in iCCP\n";

        assert!(should_retry_jxl_decode_as_jpeg(FormatKind::Jxl, stderr));
    }

    #[test]
    fn non_jxl_png_iccp_failure_stays_strict() {
        let stderr = b"libpng error: Incorrect data in iCCP\n";

        assert!(!should_retry_jxl_decode_as_jpeg(FormatKind::Avif, stderr));
    }

    #[test]
    fn jxl_orientation_diff_accepts_raw_source_dimensions_for_lossless_transcode() {
        let raw_source = patterned_image(2, 3);
        let decoded_lossless_jxl = raw_source.clone();

        let result = diff_orientation_images(
            &raw_source,
            6,
            &decoded_lossless_jxl,
            DiffTolerance::JxlOrientation,
            std::path::Path::new("out.jxl"),
        )
        .expect("raw-orientation JXL proof should not fail dimension check");

        assert_eq!(result, PixelDiffResult::Match);
    }

    #[test]
    fn shared_orientation_policy_selects_jxl_tolerance() {
        assert_eq!(
            orientation_diff_tolerance_for_format(FormatKind::Jxl),
            Some(DiffTolerance::JxlOrientation)
        );
    }

    #[test]
    fn strip_residual_orientation_tag_ignores_minor_exiftool_warnings() {
        let cmd = super::strip_residual_orientation_command(std::path::Path::new("/tmp/probe.JXL"));
        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let ignore_minor_pos = args
            .iter()
            .position(|arg| arg == "-m")
            .expect("strip_residual_orientation_tag must pass -m to exiftool");
        let strip_orientation_pos = args
            .iter()
            .position(|arg| arg == "-Orientation=")
            .expect("strip_residual_orientation_tag must strip Orientation");

        assert!(
            ignore_minor_pos < strip_orientation_pos,
            "exiftool -m must be set before Orientation mutation arguments"
        );
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
                max_delta: 1,
                channel: 0
            }
        );
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
    fn jxl_orientation_allows_bounded_decoder_drift() {
        let ref_img = rgb_image([10, 20, 30]);
        let out_img = rgb_image([210, 40, 90]);

        let result = diff_dynamic_images(
            &ref_img,
            &out_img,
            DiffTolerance::JxlOrientation,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(result, PixelDiffResult::Match);
    }

    #[test]
    fn jxl_orientation_allows_large_color_drift_when_structure_matches() {
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
            DiffTolerance::JxlOrientation,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(result, PixelDiffResult::Match);
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
            DiffTolerance::JxlOrientation,
            std::path::Path::new("out.jxl"),
        )
        .unwrap();

        assert_eq!(
            result,
            PixelDiffResult::Mismatch {
                max_delta: 160,
                channel: 0
            }
        );
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
        let tmp = NamedTempFile::new().unwrap();

        if crate::common_utils::is_command_available("exiftool") {
            assert_eq!(read_exif_orientation(tmp.path()).unwrap(), 1);
        } else {
            assert!(read_exif_orientation(tmp.path()).is_err());
        }
    }

    #[test]
    fn invalid_orientation_stdout_fails_closed() {
        let tmp = NamedTempFile::new().unwrap();

        assert!(parse_exif_orientation_stdout(tmp.path(), b"Rotate 90 CW\n").is_err());
    }

    #[test]
    fn zero_orientation_stdout_defaults_to_one() {
        let tmp = NamedTempFile::new().unwrap();

        assert_eq!(
            parse_exif_orientation_stdout(tmp.path(), b"0\n").unwrap(),
            1
        );
    }
}
