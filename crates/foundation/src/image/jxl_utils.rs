//! Shared JXL/image preprocessing utilities
//!
//! Common functions used by both `img_av1` and `img_hevc` lossless converters:
//! - JXL file health verification
//! - Image format preprocessing for cjxl compatibility
//! - Fallback encoding pipelines (`ImageMagick`, `FFmpeg`)
//! - ICC Profile extraction and preservation

use crate::VmafBuilder;
use crate::builder_base::ToolBuilder;
use anyhow::Context;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// Whether an otherwise healthy JXL can reproduce its original JPEG bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpegReconstructionEligibility {
    /// The official decoder reconstructs the original JPEG without pixel fallback.
    Exact,
    /// The JXL is pixel-decodable but contains no JPEG reconstruction data.
    PixelOnly,
    /// Reconstruction is advertised, but the strict decoder rejects it.
    AdvertisedButRejected { diagnostic: String },
}

fn first_nonempty_tool_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no diagnostic")
        .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DjxlReconstructionMode {
    ExplicitFlag,
    DefaultJpegOutput,
}

fn djxl_diagnostic(output: &std::process::Output) -> String {
    crate::infra::logging::combined_tool_output(
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

fn djxl_used_pixel_to_jpeg_fallback(diagnostic: &str) -> bool {
    let diagnostic = diagnostic.to_ascii_lowercase();
    diagnostic.contains("pixels_to_jpeg")
        || diagnostic.contains("pixels-to-jpeg")
        || diagnostic.contains("pixel-to-jpeg")
        || diagnostic.contains("decoded to pixels")
        || diagnostic.contains("could not decode losslessly to jpeg")
}

fn djxl_reported_exact_jpeg_reconstruction(diagnostic: &str) -> bool {
    let diagnostic = diagnostic.to_ascii_lowercase();
    diagnostic.contains("reconstructed to jpeg")
        || diagnostic.contains("reconstructed jpeg")
        || (diagnostic.contains("jpeg reconstruction")
            && (diagnostic.contains("complete") || diagnostic.contains("success")))
}

/// Whether `djxl` positively reported exact JPEG reconstruction without its
/// lossy pixel-to-JPEG fallback.
///
/// A zero exit status is deliberately insufficient: recent libjxl releases can
/// return success after falling back to a newly encoded JPEG.
#[must_use]
pub fn djxl_completed_exact_jpeg_reconstruction(output: &std::process::Output) -> bool {
    if !output.status.success() {
        return false;
    }
    let diagnostic = djxl_diagnostic(output);
    !djxl_used_pixel_to_jpeg_fallback(&diagnostic)
        && djxl_reported_exact_jpeg_reconstruction(&diagnostic)
}

fn run_jxl_reconstruction_probe(
    command: &mut std::process::Command,
    context: &str,
) -> Result<std::process::Output, String> {
    crate::process_runner::run_command_with_liveness_timeout(
        command,
        Duration::from_secs(120),
        crate::process_runner::image_process_hard_timeout(),
        context,
    )
    .map_err(|error| format!("{context} failed: {error}"))
}

fn detect_djxl_reconstruction_mode() -> Result<DjxlReconstructionMode, String> {
    static CAPABILITY: OnceLock<Result<DjxlReconstructionMode, String>> = OnceLock::new();

    CAPABILITY
        .get_or_init(|| {
            let mut command = crate::DjxlBuilder::new().arg("-h").build();
            let output = run_jxl_reconstruction_probe(&mut command, "djxl capability probe")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let help = crate::infra::logging::combined_tool_output(&stdout, &stderr);
            if !output.status.success() {
                return Err(format!(
                    "djxl capability probe failed with {}: {}",
                    output.status,
                    if help.is_empty() {
                        "no diagnostic output"
                    } else {
                        help.as_str()
                    }
                ));
            }
            if help.contains("--reconstruct_jpeg") {
                return Ok(DjxlReconstructionMode::ExplicitFlag);
            }
            let help_lower = help.to_ascii_lowercase();
            if help_lower
                .lines()
                .any(|line| line.contains("output") && line.contains("jpeg"))
            {
                return Ok(DjxlReconstructionMode::DefaultJpegOutput);
            }
            Err(
                "installed djxl advertises neither explicit JPEG reconstruction nor JPEG output; exact reconstruction cannot be classified safely"
                    .into(),
            )
        })
        .clone()
}

fn run_exact_jpeg_reconstruction_with_mode(
    input: &Path,
    output: &Path,
    context: &str,
    mode: DjxlReconstructionMode,
) -> Result<std::process::Output, String> {
    let mut command = crate::DjxlBuilder::new().input(input).output(output).build();
    if mode == DjxlReconstructionMode::ExplicitFlag {
        command.arg("--reconstruct_jpeg");
    }
    run_jxl_reconstruction_probe(&mut command, context)
}

/// Reconstruct the original JPEG using the strongest operation advertised by
/// the installed official decoder.
///
/// libjxl releases differ: some expose `--reconstruct_jpeg`, while newer builds
/// perform reconstruction for a `.jpg` output without exposing that flag. Both
/// paths require an explicit reconstruction diagnostic and reject pixel fallback.
///
/// # Errors
/// Returns an error for missing decoder capability, decoder failure, pixel
/// fallback, missing positive reconstruction evidence, or an empty output file.
pub fn run_exact_jpeg_reconstruction(
    input: &Path,
    output: &Path,
    context: &str,
) -> Result<std::process::Output, String> {
    let mode = detect_djxl_reconstruction_mode()?;
    let result = run_exact_jpeg_reconstruction_with_mode(input, output, context, mode)?;
    if !djxl_completed_exact_jpeg_reconstruction(&result) {
        let diagnostic = djxl_diagnostic(&result);
        let reason = if djxl_used_pixel_to_jpeg_fallback(&diagnostic) {
            "djxl used pixel-to-JPEG fallback"
        } else if result.status.success() {
            "djxl returned success without positive exact-reconstruction evidence"
        } else {
            "djxl rejected exact JPEG reconstruction"
        };
        return Err(format!(
            "{context}: {reason} (status {}): {}",
            result.status,
            first_nonempty_tool_line(diagnostic.as_bytes())
        ));
    }
    if output != Path::new("-") {
        let metadata = std::fs::metadata(output)
            .map_err(|error| format!("{context}: reconstructed JPEG is missing: {error}"))?;
        if metadata.len() == 0 {
            return Err(format!("{context}: reconstructed JPEG is empty"));
        }
    }
    Ok(result)
}

/// Classify exact JPEG reconstruction without enabling pixel-to-JPEG fallback.
///
/// A strict reconstruction rejection is only considered a safe
/// retained skip after an independent pixel decode proves the JXL itself is
/// healthy.
///
/// # Errors
/// Returns an error when the required official tools are unavailable, the JXL
/// is unreadable, or even its pixel payload cannot be decoded.
pub fn probe_jpeg_reconstruction_eligibility(
    path: &Path,
) -> Result<JpegReconstructionEligibility, String> {
    use crate::{DjxlBuilder, tool_builders::JxlinfoBuilder};

    if !JxlinfoBuilder::new().check_available() || !DjxlBuilder::new().check_available() {
        return Err("jxlinfo and djxl are required for exact JPEG reconstruction probing".into());
    }
    let reconstruction_mode = detect_djxl_reconstruction_mode()?;

    let mut info_command = JxlinfoBuilder::new().input(path).build();
    let info = run_jxl_reconstruction_probe(&mut info_command, "JXL structure probe")?;
    let mut info_diagnostic = info.stdout;
    info_diagnostic.extend_from_slice(&info.stderr);
    if !info.status.success() {
        return Err(format!(
            "jxlinfo rejected the JXL: {}",
            first_nonempty_tool_line(&info_diagnostic)
        ));
    }
    let advertises_reconstruction = String::from_utf8_lossy(&info_diagnostic)
        .lines()
        .any(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("jpeg bitstream reconstruction")
                && line.contains("available")
                && !line.contains("not available")
        });

    let strict_output = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "jxl_exact_reconstruction_probe",
        None,
        Some(".jpg"),
    )
    .map_err(|error| format!("strict JPEG reconstruction temp allocation failed: {error}"))?;
    let strict = run_exact_jpeg_reconstruction_with_mode(
        path,
        strict_output.path(),
        "strict JPEG reconstruction probe",
        reconstruction_mode,
    )?;
    if djxl_completed_exact_jpeg_reconstruction(&strict)
        && strict_output
            .as_file()
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 0)
    {
        return Ok(JpegReconstructionEligibility::Exact);
    }
    let diagnostic = djxl_diagnostic(&strict);
    let strict_diagnostic = if djxl_used_pixel_to_jpeg_fallback(&diagnostic) {
        "djxl used pixel-to-JPEG fallback instead of exact reconstruction".to_string()
    } else if strict.status.success() {
        "djxl returned success without a non-empty JPEG and positive exact-reconstruction evidence"
            .to_string()
    } else {
        first_nonempty_tool_line(diagnostic.as_bytes())
    };

    let pixel_temp_dir = crate::media_conversion_gate::delivery_temp_dir_in_scratch_or_err(
        "jxl_pixel_reconstruction_probe",
        "mfb-jxl-pixel-",
    )
    .map_err(|error| format!("pixel reconstruction temp allocation failed: {error}"))?;
    let pixel_output = pixel_temp_dir.path().join("decoded.png");
    let mut pixel_command = DjxlBuilder::new().input(path).output(&pixel_output).build();
    let pixel = run_jxl_reconstruction_probe(&mut pixel_command, "JXL pixel health probe")?;
    if !pixel.status.success() {
        return Err(format!(
            "djxl rejected both exact reconstruction ({strict_diagnostic}) and pixel decode ({})",
            first_nonempty_tool_line(&pixel.stderr)
        ));
    }

    if advertises_reconstruction {
        Ok(JpegReconstructionEligibility::AdvertisedButRejected {
            diagnostic: strict_diagnostic,
        })
    } else {
        Ok(JpegReconstructionEligibility::PixelOnly)
    }
}

/// Extract ICC Profile from source image and return temp file path
#[must_use]
pub fn extract_icc_profile(src: &Path) -> Option<tempfile::NamedTempFile> {
    if !crate::image_builders::ExiftoolBuilder::check_available() {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            src,
            "ExifTool unavailable; ICC extraction was not attempted",
        );
        return None;
    }

    let temp_icc = match crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "jxl_icc_extract",
        None,
        Some(".icc"),
    ) {
        Ok(f) => f,
        Err(e) => {
            crate::media_conversion_gate::delivery_jxl_path_audit(
                "delivery_jxl",
                src,
                format!(
                    "JXL METADATA AUDIT: Failed to create temporary file for ICC extraction from \
                     '{}' | System Error: {}",
                    src.display(),
                    e
                ),
            );
            return None;
        }
    };
    let mut command = crate::image_builders::ExiftoolBuilder::new()
        .input(src)
        .extract_icc_profile()
        .build();
    let command_line = crate::common_utils::format_command_for_audit(&command);
    let output = match command.output() {
        Ok(out) => out,
        Err(e) => {
            crate::media_conversion_gate::delivery_jxl_path_audit(
                "delivery_jxl",
                src,
                format!(
                    "JXL METADATA AUDIT: Failed to execute ExifTool for ICC extraction from '{}' \
                     | Pipeline Error: {}",
                    src.display(),
                    e
                ),
            );
            return None;
        }
    };

    // ICC is binary metadata; retain only a size marker in logs, never payload
    // bytes. This keeps the diagnostic useful without leaking private profiles.
    let stdout_summary = format!("<binary ICC stdout omitted: {} bytes>", output.stdout.len());
    crate::infra::logging::log_captured_process_output(
        &command_line,
        &output.status,
        &stdout_summary,
        &String::from_utf8_lossy(&output.stderr),
    );

    if !output.status.success() {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            src,
            format!(
                "ExifTool ICC extraction failed with {} ({} bytes captured)",
                output.status,
                output.stdout.len()
            ),
        );
        return None;
    }

    if !output.stdout.is_empty() {
        if let Err(e) = std::fs::write(temp_icc.path(), &output.stdout) {
            crate::media_conversion_gate::delivery_jxl_path_audit(
                "delivery_jxl",
                src,
                format!(
                    "JXL METADATA AUDIT: Failed to write extracted ICC profile to temp file for \
                     '{}' | Disk I/O Error: {}",
                    src.display(),
                    e
                ),
            );
            return None;
        }
        Some(temp_icc)
    } else {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            src,
            "ExifTool returned no ICC payload; no profile was extracted",
        );
        None
    }
}

/// Returns true if a cjxl stderr output indicates an ICC D50 illuminant
/// rounding error.
///
/// This is a known issue with ICC profiles generated by Capture One and some
/// professional cameras where the D50 tag has a 2-byte rounding deviation from
/// the ICC spec.
#[must_use]
pub fn is_icc_rounding_error(stderr: &str) -> bool {
    stderr.contains("Invalid ICC profile")
        || stderr.contains("bad connection space")
        || stderr.contains("ICC_Profile")
            && (stderr.contains("invalid") || stderr.contains("rejected"))
}

/// Extract ICC Profile and apply D50 illuminant rounding patch before
/// returning.
///
/// Only call this after `is_icc_rounding_error` returns true on a failed cjxl
/// run. Patches bytes [68..80] to the canonical D50 values per ICC spec.
///
/// Returns `Ok(None)` when ICC extraction is honestly unavailable or produces
/// no ICC payload.
///
/// # Errors
///
/// Returns an error when temporary file creation, ICC extraction, or
/// persistence of the patched profile fails after extraction was attempted.
pub fn extract_icc_with_d50_patch(src: &Path) -> anyhow::Result<Option<tempfile::NamedTempFile>> {
    if !crate::image_builders::ExiftoolBuilder::check_available() {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            src,
            "ExifTool unavailable; D50 ICC remediation was not attempted",
        );
        return Err(anyhow::anyhow!(
            "ExifTool unavailable; cannot extract ICC profile for D50 remediation"
        ));
    }

    let temp_icc = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "jxl_icc_d50_patch",
        None,
        Some(".icc"),
    )
    .with_context(|| {
        format!(
            "Failed to create temporary ICC file for patched D50 extraction from {}",
            src.display()
        )
    })?;
    let mut command = crate::image_builders::ExiftoolBuilder::new()
        .input(src)
        .extract_icc_profile()
        .build();
    let command_line = crate::common_utils::format_command_for_audit(&command);
    let output = command
        .output()
        .with_context(|| format!("Failed to extract ICC profile from {}", src.display()))?;
    let stdout_summary = format!("<binary ICC stdout omitted: {} bytes>", output.stdout.len());
    crate::infra::logging::log_captured_process_output(
        &command_line,
        &output.status,
        &stdout_summary,
        &String::from_utf8_lossy(&output.stderr),
    );

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "ExifTool ICC extraction failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if output.stdout.is_empty() {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            src,
            "ExifTool returned no ICC payload for D50 remediation",
        );
        return Ok(None);
    }

    let mut icc_data = output.stdout;
    if icc_data.len() >= crate::constants::ICC_D50_ILLUMINANT_OFFSET_END {
        // Apply D50 illuminant rounding patch (Capture One / some cameras emit
        // 0x...D32B instead of the ICC-spec 0x...D32D, causing cjxl <= v0.10 to
        // reject the profile)
        if let Some(slice) = icc_data.get_mut(
            crate::constants::ICC_D50_ILLUMINANT_OFFSET_START
                ..crate::constants::ICC_D50_ILLUMINANT_OFFSET_END,
        ) {
            slice.copy_from_slice(&crate::constants::ICC_D50_STANDARD_BYTES);
        }
    }
    std::fs::write(temp_icc.path(), &icc_data).with_context(|| {
        format!(
            "Failed to persist patched ICC profile to {}",
            temp_icc.path().display()
        )
    })?;

    Ok(Some(temp_icc))
}

/// Verify that a JXL file is valid by checking its signature and optionally
/// running jxlinfo. Verify the health of a JXL file.
///
/// # Errors
/// Returns an error message if the file is corrupt.
pub fn verify_jxl_health(path: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut sig = [0u8; 2];
    file.read_exact(&mut sig).map_err(|e| e.to_string())?;

    if sig != [0xFF, 0x0A] && sig != [0x00, 0x00] {
        crate::media_conversion_gate::delivery_jxl_batch_fallback_audit(
            "delivery_jxl",
            format!(
                "JXL AUDIT: Invalid signature | Forensic: Found {:02X}{:02X}; expected FF0A or \
                 0000; refusing to parse non-JXL data",
                sig[0], sig[1]
            ),
        );
        return Err("Invalid JXL file signature".to_string());
    }

    if crate::tool_builders::JxlinfoBuilder::new().check_available() {
        let result = crate::tool_builders::JxlinfoBuilder::new()
            .input(path)
            .build()
            .output();

        match result {
            Ok(output) if !output.status.success() => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let trimmed = stderr.trim();
                crate::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                    "delivery_jxl",
                    format!(
                        "JXL AUDIT: Health check failed | Forensic: jxlinfo returned non-zero \
                         exit; stderr: '{trimmed}'; bitstream is likely corrupt"
                    ),
                );
                return Err(format!("JXL health check failed (jxlinfo): {trimmed}"));
            }
            Ok(_) => {}
            Err(err) => {
                crate::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                    "delivery_jxl",
                    format!(
                        "JXL AUDIT: Health check failed | Forensic: failed to execute jxlinfo \
                         after availability check: {err}"
                    ),
                );
                return Err(format!("JXL health check failed (jxlinfo exec): {err}"));
            }
        }
    } else {
        crate::media_conversion_gate::delivery_jxl_batch_fallback_audit(
            "delivery_jxl",
            format!(
                "JXL AUDIT: jxlinfo unavailable; only the container signature was checked for {}",
                path.display()
            ),
        );
    }

    Ok(())
}

#[must_use]
pub fn is_vmaf_available() -> bool {
    VmafBuilder::new().check_available()
}

/// Check whether a JXL file already contains an embedded ICC profile.
///
/// Uses `exiftool -icc_profile -b` — returns `true` if the profile blob is
/// non-empty. Tool absence returns `Ok(false)` so callers can inject ICC as a
/// fallback; probe execution failures return `Err` so they are not confused
/// with an ICC-absent JXL.
pub fn verify_jxl_has_icc(path: &Path) -> anyhow::Result<bool> {
    if !crate::image_builders::ExiftoolBuilder::check_available() {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl_icc_probe",
            path,
            "ExifTool unavailable; ICC absence is unverified and fallback injection is required",
        );
        return Ok(false);
    }
    let mut command = crate::image_builders::ExiftoolBuilder::new()
        .input(path)
        .extract_icc_profile()
        .build();
    let command_line = crate::common_utils::format_command_for_audit(&command);
    let out = command
        .output()
        .with_context(|| format!("probe embedded JXL ICC profile {}", path.display()))?;
    let stdout_summary = format!("<binary ICC stdout omitted: {} bytes>", out.stdout.len());
    crate::infra::logging::log_captured_process_output(
        &command_line,
        &out.status,
        &stdout_summary,
        &String::from_utf8_lossy(&out.stderr),
    );
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let trimmed = stderr.trim();
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl_icc_probe",
            path,
            format!("JXL ICC probe failed; stderr: '{trimmed}'"),
        );
        return Err(anyhow::anyhow!(
            "JXL ICC probe failed for {}: {trimmed}",
            path.display()
        ));
    }
    Ok(!out.stdout.is_empty())
}

/// True when cjxl failed due to grayscale PNG + ICC profile (libpng: "RGB color
/// space not permitted on grayscale").
///
/// Only then do we retry with -strip to avoid metadata loss in the general
/// case. Enhanced to catch more variants of the error message.
#[must_use]
pub fn is_grayscale_icc_cjxl_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    // Match the specific pattern: ICC profile color space mismatch on grayscale PNG
    // Example: "libpng warning: iCCP: profile 'icc': 'RGB ': RGB color space not
    // permitted on grayscale PNG" Relaxed matching: check for libpng warning +
    // grayscale + icc/color space issues
    let has_grayscale_issue = s.contains("grayscale") || s.contains("pixel data");
    let has_icc_issue = s.contains("iccp")
        || s.contains("color space")
        || s.contains("icc profile")
        || s.contains("icc");
    let has_libpng_warning = s.contains("libpng warning") || s.contains("png warning");

    s.contains("rgb color space not permitted on grayscale")
        || (has_libpng_warning && has_grayscale_issue && has_icc_issue)
        || (s.contains("iccp") && s.contains("grayscale"))
        || (s.contains("pixel data") && s.contains("color space"))
}

/// True when `djxl` PNG output failed on an embedded ICC profile.
///
/// JPEG-lossless JXLs can still be reconstructed to JPEG, so callers should
/// retry with a `.jpg` temp output instead of treating the bitstream as
/// corrupt.
#[must_use]
pub fn is_jxl_png_icc_decode_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("libpng") && s.contains("error") && (s.contains("iccp") || s.contains("icc profile"))
}

/// True when cjxl failed with decode/pixel errors that may be helped by a
/// simpler pipeline.
fn is_decode_or_pixel_cjxl_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("getting pixel data failed")
        || s.contains("failed to decode")
        || s.contains("decoding failed")
        || s.contains("decode failed")
}

/// True when cjxl was killed by signal (OOM/crash) — stderr has
/// version/encoding lines but no error. In this case retrying at lower effort
/// may succeed.
fn is_cjxl_signal_killed(stderr: &str) -> bool {
    let has_started = stderr.contains("JPEG XL encoder") || stderr.contains("Encoding [");
    let has_error = stderr.to_lowercase().contains("error")
        || stderr.contains("failed")
        || stderr.contains("Error");
    has_started && !has_error
}

/// Read the bit depth from a PNG file's IHDR chunk (byte offset 24).
/// Returns None if the file is not a valid PNG or cannot be read.
#[must_use]
pub fn get_png_bit_depth(path: &Path) -> Option<u8> {
    use std::io::Read;
    let mut f = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) => {
            crate::media_conversion_gate::delivery_jxl_path_audit(
                "delivery_jxl",
                path,
                format!(
                    "PNG BIT-DEPTH PROBE: Failed to open file for analysis at '{}' | System \
                     Error: {}",
                    path.display(),
                    e
                ),
            );
            return None;
        }
    };
    let mut buf = [0u8; 25];
    if let Err(e) = f.read_exact(&mut buf) {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            path,
            format!(
                "PNG BIT-DEPTH PROBE: Failed to read IHDR header from '{}' (insufficient bytes) | \
                 I/O Error: {}",
                path.display(),
                e
            ),
        );
        return None;
    }
    // PNG signature is 8 bytes; IHDR: 4 len + 4 type + 13 data bytes.
    // Bit depth is the first byte of IHDR data, at offset 8+4+4+8 = 24.
    if &buf[0..8] != b"\x89PNG\r\n\x1a\n" {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            path,
            format!(
                "PNG BIT-DEPTH PROBE: Invalid PNG signature detected at '{}' | Forensic: \
                 Mismatched magic bytes",
                path.display()
            ),
        );
        return None;
    }
    if buf[8..12] != [0, 0, 0, 13] || &buf[12..16] != b"IHDR" {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            path,
            format!(
                "PNG BIT-DEPTH PROBE: Missing canonical IHDR header at '{}' | Preserving unknown \
                 precision instead of trusting malformed header layout",
                path.display()
            ),
        );
        return None;
    }
    let bit_depth = buf[24];
    if matches!(bit_depth, 1 | 2 | 4 | 8 | 16) {
        Some(bit_depth)
    } else {
        crate::media_conversion_gate::delivery_jxl_path_audit(
            "delivery_jxl",
            path,
            format!(
                "PNG BIT-DEPTH PROBE: Invalid IHDR bit depth {} detected at '{}' | Preserving \
                 unknown precision instead of forging retry depth",
                bit_depth,
                path.display()
            ),
        );
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PngEightBitRetryDisposition {
    ConfirmedEightBit,
    HigherBitDepth,
    Unknown,
}

const fn classify_png_eight_bit_retry(bit_depth: Option<u8>) -> PngEightBitRetryDisposition {
    match bit_depth {
        Some(depth) if depth <= 8 => PngEightBitRetryDisposition::ConfirmedEightBit,
        Some(_) => PngEightBitRetryDisposition::HigherBitDepth,
        None => PngEightBitRetryDisposition::Unknown,
    }
}

fn try_confirmed_png_8bit_retry(
    input: &Path,
    base_request: ImagemagickCjxlPipelineRequest<'_>,
    attempt_label: &str,
) -> bool {
    match classify_png_eight_bit_retry(get_png_bit_depth(input)) {
        PngEightBitRetryDisposition::ConfirmedEightBit => {
            crate::progress_mode::emit_stderr(&format!(
                "   {} {attempt_label}: 8-bit depth (-depth 8 -strip, 8-bit source confirmed)",
                crate::modern_ui::symbols::styled_retry_icon()
            ));
            if run_imagemagick_cjxl_pipeline_with_effort(ImagemagickCjxlPipelineRequest {
                metadata_policy: JxlMetadataPolicy::Strip,
                output_depth: 8,
                ..base_request
            })
            .is_ok()
            {
                crate::progress_mode::emit_stderr(&format!(
                    "   {} {attempt_label} succeeded",
                    crate::modern_ui::symbols::styled_ok_fail_label(true)
                ));
                crate::progress_mode::fallback_success();
                true
            } else {
                crate::progress_mode::emit_stderr(&format!(
                    "   {} {attempt_label} failed",
                    crate::modern_ui::symbols::styled_ok_fail_label(false)
                ));
                false
            }
        }
        PngEightBitRetryDisposition::HigherBitDepth => {
            crate::progress_mode::emit_stderr(&format!(
                "   {}  Higher-bit-depth PNG source confirmed; refusing to downgrade to 8-bit",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
            false
        }
        PngEightBitRetryDisposition::Unknown => {
            crate::media_conversion_gate::delivery_jxl_path_audit(
                "delivery_jxl",
                input,
                format!(
                    "JXL CONVERSION AUDIT: Failed to detect PNG bit depth for '{}' | Preserving \
                     unknown precision; refusing speculative 8-bit retry",
                    input.display()
                ),
            );
            crate::progress_mode::emit_stderr(&format!(
                "   {}  PNG bit depth unavailable; refusing speculative 8-bit downgrade",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlMetadataPolicy {
    Preserve,
    Strip,
}

impl JxlMetadataPolicy {
    const fn should_strip(self) -> bool {
        matches!(self, Self::Strip)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlIccPolicy {
    Preserve,
    NormalizeToSrgb,
}

impl JxlIccPolicy {
    const fn should_normalize(self) -> bool {
        matches!(self, Self::NormalizeToSrgb)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlMode {
    Normal,
    Ultimate,
}

impl JxlMode {
    const fn is_ultimate(self) -> bool {
        matches!(self, Self::Ultimate)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ImagemagickCjxlPipelineRequest<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    pub distance: f32,
    pub effort: u8,
    pub max_threads: usize,
    pub metadata_policy: JxlMetadataPolicy,
    pub output_depth: u8,
    pub icc_policy: JxlIccPolicy,
    pub apple_compat: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ModeLockedImagemagickCjxlPipelineRequest<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    pub distance: f32,
    pub max_threads: usize,
    pub metadata_policy: JxlMetadataPolicy,
    pub output_depth: u8,
    pub icc_policy: JxlIccPolicy,
    pub apple_compat: bool,
    pub mode: JxlMode,
}

/// Run the `cjxl` pipeline via `ImageMagick`.
/// - `metadata_policy`: preserve metadata or apply `-strip`
/// - `output_depth`: PNG bit depth to emit (8 or
///   `crate::constants::PNG_DEFAULT_SAFETY_BIT_DEPTH` as u32); use 8 only for
///   confirmed 8-bit sources
/// - `icc_policy`: replaces embedded ICC with standard sRGB without truncating
///   bit depth
/// - `apple_compat`: adds `--compress_boxes=0` to cjxl for Apple device
///   compatibility
///
/// # Errors
/// Returns an error if the pipeline fails.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
fn run_imagemagick_cjxl_pipeline_with_effort(
    request: ImagemagickCjxlPipelineRequest<'_>,
) -> std::result::Result<(), (bool, bool, String)> {
    use std::process::Stdio;
    let ImagemagickCjxlPipelineRequest {
        input,
        output,
        distance,
        effort,
        max_threads,
        metadata_policy,
        output_depth,
        icc_policy,
        apple_compat,
    } = request;
    debug_assert!(crate::constants::is_supported_jxl_effort(effort));

    let mut magick_builder = crate::image_builders::MagickBuilder::new();
    magick_builder
        .input(input)
        .strip(metadata_policy.should_strip())
        .use_stdout(true);

    magick_builder.depth(output_depth);

    if icc_policy.should_normalize() {
        magick_builder
            .define("png:preserve-colormap", "false")
            .set("colorspace", "sRGB");
    }

    let mut magick_proc = magick_builder.build().spawn().map_err(|e| {
        let line = format!(
            "   {} ImageMagick not available or failed to start: {e}",
            crate::modern_ui::symbols::styled_ok_fail_label(false)
        );
        crate::progress_mode::emit_stderr(&line);
        (false, false, String::new())
    })?;

    let magick_stdout = magick_proc.stdout.take().ok_or_else(|| {
        if let Err(err) = magick_proc.kill() {
            crate::progress_mode::emit_stderr(&format!(
                "   {} Failed to stop ImageMagick after stdout capture failure: {err}",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
        }
        (false, false, String::new())
    })?;

    // Drain ImageMagick stderr in background to avoid blocking when pipe buffer
    // fills. Limit to 1MB to prevent memory issues in low-memory scenarios.
    let magick_stderr_thread = magick_proc.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut s = String::new();
            if let Err(err) = stderr
                .take(crate::numeric_cast::usize_to_u64(
                    crate::constants::STDERR_DRAIN_LIMIT,
                ))
                .read_to_string(&mut s)
            {
                crate::log_rare_error!("Stderr Pipe", "Failed to read ImageMagick stderr: {err}");
            }
            s
        })
    });

    let mut cjxl_builder = crate::jxl_builder::CjxlBuilder::new();
    cjxl_builder
        .use_stdin(true)
        .output(output)
        .distance(distance)
        .effort(effort)
        .threads(max_threads)
        .apple_compat(apple_compat);

    let mut cjxl_proc = cjxl_builder
        .build()
        .stdin(magick_stdout)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let line = format!(
                "   {} Failed to start cjxl process: {e}",
                crate::modern_ui::symbols::styled_ok_fail_label(false)
            );
            crate::progress_mode::emit_stderr(&line);
            if let Err(err) = magick_proc.kill() {
                crate::progress_mode::emit_stderr(&format!(
                    "   {} Failed to stop ImageMagick after cjxl startup failure: {err}",
                    crate::modern_ui::symbols::styled_warning_icon()
                ));
            }
            (false, false, String::new())
        })?;

    // Drain cjxl stderr in background so cjxl does not block when pipe buffer
    // fills. Limit to 1MB to prevent memory issues in low-memory scenarios.
    let cjxl_stderr_thread = cjxl_proc.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut s = String::new();
            if let Err(err) = stderr
                .take(crate::numeric_cast::usize_to_u64(
                    crate::constants::STDERR_DRAIN_LIMIT,
                ))
                .read_to_string(&mut s)
            {
                crate::log_rare_error!("Stderr Pipe", "Failed to read cjxl stderr: {err}");
            }
            s.trim().to_string()
        })
    });

    let magick_status = magick_proc.wait();
    let cjxl_status = cjxl_proc.wait();

    let magick_stderr = match magick_stderr_thread {
        Some(handle) => handle.join().map_err(|_| {
            crate::log_rare_error!(
                "Background Thread",
                "ImageMagick stderr capture thread panicked"
            );
            (
                false,
                false,
                "ImageMagick stderr capture thread panicked".to_string(),
            )
        })?,
        None => String::new(),
    };
    let cjxl_stderr = match cjxl_stderr_thread {
        Some(handle) => handle.join().map_err(|_| {
            crate::log_rare_error!("Background Thread", "cjxl stderr capture thread panicked");
            (
                false,
                false,
                "cjxl stderr capture thread panicked".to_string(),
            )
        })?,
        None => String::new(),
    };

    let magick_ok = match magick_status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            let line = format!(
                "   {} ImageMagick failed with exit code: {:?}",
                crate::modern_ui::symbols::styled_ok_fail_label(false),
                status.code()
            );
            crate::progress_mode::emit_stderr(&line);
            if !magick_stderr.is_empty() {
                let line2 = format!(
                    "   {} ImageMagick stderr: {}",
                    crate::media_conversion_gate::ui_icon_pick(
                        crate::modern_ui::symbols::CLIPBOARD,
                        crate::modern_ui::symbols::plain::CLIPBOARD,
                    ),
                    crate::media_conversion_gate::encode_stderr_last_line_or_unknown(
                        &magick_stderr,
                        "jxl_magick",
                        "ImageMagick stderr during JXL delivery",
                    )
                );
                crate::progress_mode::emit_stderr(&line2);
            }
            false
        }
        Err(e) => {
            let line = format!(
                "   {} Failed to wait for ImageMagick: {e}",
                crate::modern_ui::symbols::styled_ok_fail_label(false)
            );
            crate::progress_mode::emit_stderr(&line);
            false
        }
    };

    let cjxl_ok = match cjxl_status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            let exit_code = status.code();
            // exit_code is None when process was terminated by signal (SIGKILL, SIGSEGV,
            // etc.)
            if exit_code.is_none() {
                crate::log_pipeline_broken!(
                    "cjxl",
                    "Process terminated by signal (possible crash or OOM kill)"
                );
                if !cjxl_stderr.is_empty() {
                    crate::ui_stderr::line(
                        crate::modern_ui::symbols::CLIPBOARD,
                        crate::modern_ui::symbols::plain::CLIPBOARD,
                        format!("   cjxl stderr before termination: {cjxl_stderr}"),
                    );
                }
            } else {
                crate::log_upstream_error!("cjxl", "Failed with exit code: {:?}", exit_code);
                if !cjxl_stderr.is_empty() {
                    crate::ui_stderr::line(
                        crate::modern_ui::symbols::CLIPBOARD,
                        crate::modern_ui::symbols::plain::CLIPBOARD,
                        format!("   cjxl stderr: {cjxl_stderr}"),
                    );
                } else if let Some(code) = exit_code {
                    // [HARDENING] Try to provide more context if stderr is empty
                    if code == 1_i32 {
                        crate::ui_stderr::line(
                            crate::modern_ui::symbols::INFO,
                            crate::modern_ui::symbols::plain::INFO,
                            "   Tip: Exit code 1 often indicates ICC mismatch or malformed \
                             metadata.",
                        );
                    }
                }
            }
            false
        }
        Err(e) => {
            crate::log_pipeline_broken!("cjxl", "Failed to wait for process: {}", e);
            false
        }
    };

    if magick_ok && cjxl_ok {
        Ok(())
    } else {
        Err((magick_ok, cjxl_ok, cjxl_stderr))
    }
}

/// Run the `ImageMagick` -> `cjxl` pipeline using the mode-locked JXL policy.
///
/// # Errors
/// Returns an error if the pipeline fails.
pub fn run_imagemagick_cjxl_pipeline(
    request: ModeLockedImagemagickCjxlPipelineRequest<'_>,
) -> std::result::Result<(), (bool, bool, String)> {
    run_imagemagick_cjxl_pipeline_with_effort(ImagemagickCjxlPipelineRequest {
        input: request.input,
        output: request.output,
        distance: crate::constants::jxl_distance_for_mode(
            request.distance,
            request.mode.is_ultimate(),
        ),
        effort: crate::constants::jxl_effort_for_mode(request.mode.is_ultimate()),
        max_threads: request.max_threads,
        metadata_policy: request.metadata_policy,
        output_depth: request.output_depth,
        icc_policy: request.icc_policy,
        apple_compat: request.apple_compat,
    })
}

/// `ImageMagick` → cjxl fallback pipeline for when direct cjxl encoding fails.
///
/// Fallback priority:
///
/// - No -strip, depth 16 (preserve metadata)
/// - grayscale+ICC error → -strip, depth 16
///   - still fails + decode/pixel error + confirmed 8-bit source → -strip,
///     depth 8 (no quality loss)
///   - still fails + higher/unknown bit depth → normalize ICC to sRGB, keep
///     safety depth
///     - still fails → error, refuse to downgrade
/// - decode/pixel error + confirmed 8-bit source → -strip, depth 8 (no quality
///   loss)
/// - decode/pixel error + higher/unknown bit depth → normalize ICC to sRGB,
///   keep safety depth
///   - still fails → error, refuse to silently downgrade
///
/// Fallback to `ImageMagick` for conversion if native tools fail.
///
/// # Errors
/// Returns an I/O error if conversion fails.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
pub fn try_imagemagick_fallback_with_effort(
    input: &Path,
    output: &Path,
    distance: f32,
    effort: u8,
    max_threads: usize,
    apple_compat: bool,
) -> std::result::Result<(), std::io::Error> {
    debug_assert!(crate::constants::is_supported_jxl_effort(effort));
    let base_request = ImagemagickCjxlPipelineRequest {
        input,
        output,
        distance,
        effort,
        max_threads,
        metadata_policy: JxlMetadataPolicy::Preserve,
        output_depth: 16,
        icc_policy: JxlIccPolicy::Preserve,
        apple_compat,
    };

    // Attempt 1: no -strip, depth 16, preserve metadata
    crate::progress_mode::emit_stderr(&format!(
        "   {} Attempt 1: Default (16-bit, preserve metadata) - {}",
        crate::modern_ui::symbols::styled_retry_icon(),
        input.display()
    ));
    match run_imagemagick_cjxl_pipeline_with_effort(base_request) {
        Ok(()) => {
            crate::progress_mode::emit_stderr(&format!(
                "   {} Attempt 1 succeeded",
                crate::modern_ui::symbols::styled_ok_fail_label(true)
            ));
            crate::progress_mode::fallback_success();
            return Ok(());
        }
        Err((magick_ok, cjxl_ok, stderr)) => {
            crate::progress_mode::emit_stderr(&format!(
                "   {} Attempt 1 failed (magick: {}, cjxl: {})",
                crate::modern_ui::symbols::styled_ok_fail_label(false),
                crate::modern_ui::symbols::styled_tool_check(magick_ok),
                crate::modern_ui::symbols::styled_tool_check(cjxl_ok),
            ));

            if magick_ok && !cjxl_ok && is_grayscale_icc_cjxl_error(&stderr) {
                // Attempt 2: -strip, depth 16 (drop bad ICC, keep bit depth)
                crate::progress_mode::emit_stderr(&format!(
                    "   {} Attempt 2: Grayscale ICC fix (-strip, 16-bit)",
                    crate::modern_ui::symbols::styled_retry_icon(),
                ));
                match run_imagemagick_cjxl_pipeline_with_effort(ImagemagickCjxlPipelineRequest {
                    metadata_policy: JxlMetadataPolicy::Strip,
                    ..base_request
                }) {
                    Ok(()) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Attempt 2 succeeded",
                            crate::modern_ui::symbols::styled_ok_fail_label(true)
                        ));
                        crate::progress_mode::fallback_success();
                        return Ok(());
                    }
                    Err((m, c, stderr2)) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Attempt 2 failed (magick: {}, cjxl: {})",
                            crate::modern_ui::symbols::styled_ok_fail_label(false),
                            crate::modern_ui::symbols::styled_tool_check(m),
                            crate::modern_ui::symbols::styled_tool_check(c),
                        ));

                        if m && !c && is_decode_or_pixel_cjxl_error(&stderr2) {
                            if try_confirmed_png_8bit_retry(input, base_request, "Attempt 3") {
                                return Ok(());
                            }

                            // Attempt 3: normalize ICC to sRGB, preserve safety depth when 8-bit
                            // was not confirmed.
                            crate::progress_mode::emit_stderr(&format!(
                                "   {} Attempt 3: ICC normalization (sRGB, higher/unknown bit \
                                 depth)",
                                crate::modern_ui::symbols::styled_retry_icon(),
                            ));
                            if run_imagemagick_cjxl_pipeline_with_effort(
                                ImagemagickCjxlPipelineRequest {
                                    icc_policy: JxlIccPolicy::NormalizeToSrgb,
                                    ..base_request
                                },
                            ) == Ok(())
                            {
                                crate::progress_mode::emit_stderr(&format!(
                                    "   {} Attempt 3 succeeded",
                                    crate::modern_ui::symbols::styled_ok_fail_label(true)
                                ));
                                crate::progress_mode::fallback_success();
                                return Ok(());
                            }
                            crate::progress_mode::emit_stderr(&format!(
                                "   {} Attempt 3 failed",
                                crate::modern_ui::symbols::styled_ok_fail_label(false)
                            ));
                            crate::progress_mode::emit_stderr(&format!(
                                "   {}  8-bit source was not confirmed; refusing to downgrade \
                                 retry depth",
                                crate::modern_ui::symbols::styled_warning_icon()
                            ));
                        }
                    }
                }
            } else if magick_ok && !cjxl_ok && is_decode_or_pixel_cjxl_error(&stderr) {
                if try_confirmed_png_8bit_retry(input, base_request, "Attempt 2") {
                    return Ok(());
                }

                // Attempt 2: normalize ICC to sRGB, preserve safety depth when 8-bit was not
                // confirmed.
                crate::progress_mode::emit_stderr(&format!(
                    "   {} Attempt 2: ICC normalization (sRGB, higher/unknown bit depth)",
                    crate::modern_ui::symbols::styled_retry_icon(),
                ));
                if run_imagemagick_cjxl_pipeline_with_effort(ImagemagickCjxlPipelineRequest {
                    icc_policy: JxlIccPolicy::NormalizeToSrgb,
                    ..base_request
                }) == Ok(())
                {
                    crate::progress_mode::emit_stderr(&format!(
                        "   {} Attempt 2 succeeded",
                        crate::modern_ui::symbols::styled_ok_fail_label(true)
                    ));
                    crate::progress_mode::fallback_success();
                    return Ok(());
                }
                crate::progress_mode::emit_stderr(&format!(
                    "   {} Attempt 2 failed",
                    crate::modern_ui::symbols::styled_ok_fail_label(false)
                ));
                crate::progress_mode::emit_stderr(&format!(
                    "   {}  8-bit source was not confirmed; refusing to downgrade retry depth",
                    crate::modern_ui::symbols::styled_warning_icon()
                ));
            }

            // Final fallback: if nothing worked and we haven't tried -strip yet, try it as
            // last resort
            if magick_ok && !cjxl_ok && !stderr.contains("-strip") {
                crate::ui_stderr::line(
                    crate::modern_ui::symbols::RECYCLE,
                    crate::modern_ui::symbols::plain::RECYCLE,
                    "   Attempt (final): Last resort -strip",
                );
                match run_imagemagick_cjxl_pipeline_with_effort(ImagemagickCjxlPipelineRequest {
                    metadata_policy: JxlMetadataPolicy::Strip,
                    ..base_request
                }) {
                    Ok(()) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Final attempt succeeded",
                            crate::modern_ui::symbols::styled_ok_fail_label(true)
                        ));
                        crate::progress_mode::fallback_success();
                        return Ok(());
                    }
                    Err((retry_magick_ok, retry_cjxl_ok, retry_stderr)) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Final attempt failed (magick_ok={retry_magick_ok}, \
                             cjxl_ok={retry_cjxl_ok}): {}",
                            crate::modern_ui::symbols::styled_ok_fail_label(false),
                            crate::io_utils::tail_error_lines(&retry_stderr, 2)
                        ));
                    }
                }
            }

            // Signal-kill retry: cjxl crashed (OOM/SIGSEGV) — retry once at the configured
            // effort.
            if magick_ok && !cjxl_ok && is_cjxl_signal_killed(&stderr) {
                crate::progress_mode::emit_stderr(&format!(
                    "   {} Attempt (signal-kill retry): cjxl crash detected, retrying at effort {}",
                    crate::modern_ui::symbols::styled_retry_icon(),
                    effort
                ));
                match run_imagemagick_cjxl_pipeline_with_effort(ImagemagickCjxlPipelineRequest {
                    metadata_policy: JxlMetadataPolicy::Strip,
                    ..base_request
                }) {
                    Ok(()) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Signal-kill retry succeeded (effort {})",
                            crate::modern_ui::symbols::styled_ok_fail_label(true),
                            effort
                        ));
                        crate::progress_mode::fallback_success();
                        return Ok(());
                    }
                    Err((retry_magick_ok, retry_cjxl_ok, retry_stderr)) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Signal-kill retry failed (magick_ok={retry_magick_ok}, \
                             cjxl_ok={retry_cjxl_ok}): {}",
                            crate::modern_ui::symbols::styled_ok_fail_label(false),
                            crate::io_utils::tail_error_lines(&retry_stderr, 2)
                        ));
                    }
                }
            }
        }
    }

    crate::progress_mode::emit_stderr(&format!(
        "   {} All ImageMagick fallback attempts exhausted",
        crate::modern_ui::symbols::styled_ok_fail_label(false)
    ));
    crate::log_upstream_error!(
        "Image conversion",
        "Failed {}: All ImageMagick+cjxl pipeline attempts failed. Possible causes: corrupted \
         image data, unsupported format variant, or cjxl crash/OOM",
        input.display()
    );
    Err(std::io::Error::other(
        "ImageMagick fallback pipeline failed",
    ))
}

/// Fallback to `ImageMagick` for conversion if native tools fail, using the
/// mode-locked JXL policy.
///
/// # Errors
/// Returns an I/O error if conversion fails.
pub fn try_imagemagick_fallback(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
    apple_compat: bool,
    ultimate: bool,
) -> std::result::Result<(), std::io::Error> {
    try_imagemagick_fallback_with_effort(
        input,
        output,
        crate::constants::jxl_distance_for_mode(distance, ultimate),
        crate::constants::jxl_effort_for_mode(ultimate),
        max_threads,
        apple_compat,
    )
}

/// Losslessly strip trailing data after JPEG EOI (0xFF 0xD9) so cjxl can use
/// bitstream reconstruction.
///
/// Returns (`temp_path`, guard) if tail was stripped, or None if no tail or
/// strip failed. Strip JPEG extra data (trailing bytes) to a temporary file.
///
/// # Errors
/// Returns an I/O error if the file cannot be processed.
/// # Panics
///
/// Panics if the byte range validation fails unexpectedly despite earlier
/// boundary checks.
pub fn strip_jpeg_tail_to_temp(
    path: &Path,
) -> std::io::Result<Option<(std::path::PathBuf, tempfile::NamedTempFile)>> {
    let data = std::fs::read(path)?;
    if data.len() < 4 {
        return Ok(None);
    }

    // Must start with SOI
    if data.get(0..2) != Some(&[0xFF, 0xD8]) {
        return Ok(None);
    }

    let last_eoi = data
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w == b"\xFF\xD9")
        .map(|(i, _)| i + 2) // i is FF, i+1 is D9, i+2 is the end of the marker (inclusive-slice-friendly index)
        .next_back();

    let end = match last_eoi {
        Some(e) if e < data.len() => e,
        _ => return Ok(None),
    };

    if end == data.len() {
        return Ok(None);
    }

    let temp = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "jxl_trailing_jpeg_slice",
        None,
        Some(".jpg"),
    )?;
    let Some(slice) = data.get(..end) else {
        return Ok(None);
    };
    std::fs::write(temp.path(), slice)?;
    let temp_path = temp.path().to_path_buf();
    Ok(Some((temp_path, temp)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn exact_jpeg_reconstruction_requires_positive_evidence_and_rejects_fallback() {
        assert!(djxl_reported_exact_jpeg_reconstruction(
            "Reconstructed to JPEG."
        ));
        assert!(djxl_reported_exact_jpeg_reconstruction(
            "JPEG reconstruction complete"
        ));
        assert!(!djxl_reported_exact_jpeg_reconstruction(
            "Decoded to pixels."
        ));
        assert!(djxl_used_pixel_to_jpeg_fallback(
            "could not decode losslessly to JPEG; retrying with --pixels_to_jpeg"
        ));
    }

    #[test]
    fn test_is_icc_rounding_error() {
        assert!(is_icc_rounding_error("Invalid ICC profile: ..."));
        assert!(is_icc_rounding_error("bad connection space in ICC"));
        assert!(is_icc_rounding_error("ICC_Profile rejected"));
        assert!(is_icc_rounding_error("ICC_Profile is invalid"));
        assert!(!is_icc_rounding_error("Some other error"));
    }

    #[test]
    fn test_is_grayscale_icc_cjxl_error() {
        assert!(is_grayscale_icc_cjxl_error(
            "RGB color space not permitted on grayscale PNG"
        ));
        assert!(is_grayscale_icc_cjxl_error(
            "libpng warning: iCCP: profile 'icc': 'RGB ': RGB color space not permitted on \
             grayscale PNG"
        ));
        assert!(is_grayscale_icc_cjxl_error("iccp: grayscale issue"));
        assert!(!is_grayscale_icc_cjxl_error("Normal cjxl error"));
    }

    #[test]
    fn test_is_jxl_png_icc_decode_error() {
        assert!(is_jxl_png_icc_decode_error(
            "JPEG XL decoder v0.11.2\nDecoded to pixels.\nlibpng error: Incorrect data in iCCP"
        ));
        assert!(is_jxl_png_icc_decode_error(
            "libpng error: iCCP: known incorrect sRGB profile"
        ));
        assert!(!is_jxl_png_icc_decode_error("libpng warning: benign"));
        assert!(!is_jxl_png_icc_decode_error("JPEG XL decoder failed"));
    }

    #[test]
    fn test_is_decode_or_pixel_cjxl_error() {
        assert!(is_decode_or_pixel_cjxl_error("Getting pixel data failed"));
        assert!(is_decode_or_pixel_cjxl_error("failed to decode image"));
        assert!(!is_decode_or_pixel_cjxl_error("ICC error"));
    }

    #[test]
    fn test_is_cjxl_signal_killed() {
        let stderr = "JPEG XL encoder v0.10.2\nEncoding [0.00%]";
        assert!(is_cjxl_signal_killed(stderr));

        let stderr_with_error = "JPEG XL encoder v0.10.2\nError: something failed";
        assert!(!is_cjxl_signal_killed(stderr_with_error));

        let stderr_not_started = "Some unrelated output";
        assert!(!is_cjxl_signal_killed(stderr_not_started));
    }

    #[test]
    fn test_get_png_bit_depth() {
        let mut temp = NamedTempFile::new().unwrap();
        let mut data = vec![0u8; 25];
        data[0..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        data[8..12].copy_from_slice(&[0, 0, 0, 13]);
        data[12..16].copy_from_slice(b"IHDR");
        data[24] = 16;
        temp.write_all(&data).unwrap();

        assert_eq!(get_png_bit_depth(temp.path()), Some(16));

        let mut invalid_temp = NamedTempFile::new().unwrap();
        invalid_temp.write_all(b"NOT_A_PNG").unwrap();
        assert_eq!(get_png_bit_depth(invalid_temp.path()), None);
    }

    #[test]
    fn test_get_png_bit_depth_rejects_invalid_ihdr_depth() {
        let mut temp = NamedTempFile::new().unwrap();
        let mut data = vec![0u8; 25];
        data[0..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        data[8..12].copy_from_slice(&[0, 0, 0, 13]);
        data[12..16].copy_from_slice(b"IHDR");
        data[24] = 0;
        temp.write_all(&data).unwrap();

        assert_eq!(get_png_bit_depth(temp.path()), None);
    }

    #[test]
    fn test_get_png_bit_depth_rejects_missing_ihdr() {
        let mut temp = NamedTempFile::new().unwrap();
        let mut data = vec![0u8; 25];
        data[0..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        data[8..12].copy_from_slice(&[0, 0, 0, 13]);
        data[12..16].copy_from_slice(b"IDAT");
        data[24] = 8;
        temp.write_all(&data).unwrap();

        assert_eq!(get_png_bit_depth(temp.path()), None);
    }

    #[test]
    fn test_classify_png_eight_bit_retry_preserves_unknown() {
        assert_eq!(
            classify_png_eight_bit_retry(Some(8)),
            PngEightBitRetryDisposition::ConfirmedEightBit
        );
        assert_eq!(
            classify_png_eight_bit_retry(Some(16)),
            PngEightBitRetryDisposition::HigherBitDepth
        );
        assert_eq!(
            classify_png_eight_bit_retry(None),
            PngEightBitRetryDisposition::Unknown
        );
    }

    #[test]
    fn test_strip_jpeg_tail_to_temp() {
        let mut data = vec![0xFF, 0xD8, 0x00, 0x11, 0xFF, 0xD9];
        let tail = vec![0x01, 0x02, 0x03];
        data.extend_from_slice(&tail);

        let mut temp_in = NamedTempFile::new().unwrap();
        temp_in.write_all(&data).unwrap();

        let result = strip_jpeg_tail_to_temp(temp_in.path()).unwrap();
        assert!(result.is_some());
        let (temp_path, _guard) = result.unwrap();

        let stripped_data = std::fs::read(temp_path).unwrap();
        assert_eq!(stripped_data, vec![0xFF, 0xD8, 0x00, 0x11, 0xFF, 0xD9]);
    }

    #[test]
    fn test_verify_jxl_health_basic() {
        let mut temp = NamedTempFile::new().unwrap();
        temp.write_all(&[0xFF, 0x0A]).unwrap();
        assert!(verify_jxl_health(temp.path()).is_ok());

        let mut invalid_temp = NamedTempFile::new().unwrap();
        invalid_temp.write_all(&[0x00, 0x01]).unwrap();
        assert!(verify_jxl_health(invalid_temp.path()).is_err());
    }

    #[test]
    fn verify_jxl_has_icc_returns_result_contract() {
        let _: anyhow::Result<bool> = verify_jxl_has_icc(Path::new("missing.jxl"));
    }
}
