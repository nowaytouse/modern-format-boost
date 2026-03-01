//! HDR Image Decoding Module
//!
//! Provides HDR-aware image decoding using FFmpeg to preserve high bit-depth content.
//! Standard Rust image decoders (image crate, libheif-rs) decode to 8-bit RGB,
//! losing HDR information. This module uses FFmpeg to decode HDR images to 16-bit PNG.

use crate::ffprobe_json::ColorInfo;
use crate::hdr_utils::{get_hdr_pix_fmt, should_use_hdr_decode};
use crate::img_errors::{ImgQualityError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Decode an HDR image to a high bit-depth PNG using FFmpeg.
/// Returns the path to the temporary 16-bit PNG file.
///
/// # Arguments
/// * `input` - Path to the HDR image (HEIC, AVIF, TIFF, etc.)
/// * `hdr_info` - HDR metadata from ffprobe
///
/// # Returns
/// * `Ok((PathBuf, NamedTempFile))` - Path to temporary 16-bit PNG file and its guard
/// * `Err(ImgQualityError)` - Decoding failed
///
/// # Note
/// The caller must keep the NamedTempFile guard alive to prevent automatic cleanup.
///
/// # Example
/// ```no_run
/// use shared_utils::{decode_hdr_image_to_png16, ColorInfo};
/// use std::path::Path;
///
/// let hdr_info = ColorInfo {
///     bit_depth: Some(10),
///     ..Default::default()
/// };
/// let (png16_path, _guard) = decode_hdr_image_to_png16(Path::new("input.heic"), &hdr_info)?;
/// // Use png16_path...
/// // _guard will clean up the temp file when dropped
/// # Ok::<(), shared_utils::img_errors::ImgQualityError>(())
/// ```
pub fn decode_hdr_image_to_png16(
    input: &Path,
    hdr_info: &ColorInfo,
) -> Result<(PathBuf, tempfile::NamedTempFile)> {
    if !should_use_hdr_decode(hdr_info) {
        return Err(ImgQualityError::ConversionError(
            "Not an HDR image, use standard decoding".to_string(),
        ));
    }

    // Create temporary PNG file
    let temp_png = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .map_err(|e| ImgQualityError::IoError(e))?;
    let temp_path = temp_png.path().to_path_buf();

    let pix_fmt = get_hdr_pix_fmt(hdr_info);

    // FFmpeg command: decode to 16-bit RGB PNG
    // -i input.heic -pix_fmt rgb48le -frames:v 1 output.png
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-pix_fmt")
        .arg(pix_fmt)
        .arg("-frames:v")
        .arg("1")
        .arg("-y") // Overwrite output
        .arg(crate::safe_path_arg(&temp_path).as_ref());

    let output = cmd
        .output()
        .map_err(|e| ImgQualityError::ConversionError(format!("FFmpeg spawn failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ImgQualityError::ConversionError(format!(
            "FFmpeg HDR decode failed: {}",
            stderr
        )));
    }

    // Verify output exists and has content
    if !temp_path.exists() {
        return Err(ImgQualityError::ConversionError(
            "FFmpeg did not create output file".to_string(),
        ));
    }

    let file_size = std::fs::metadata(&temp_path)
        .map_err(|e| ImgQualityError::IoError(e))?
        .len();
    if file_size == 0 {
        return Err(ImgQualityError::ConversionError(
            "FFmpeg created empty output file".to_string(),
        ));
    }

    Ok((temp_path, temp_png))
}

/// Check if an image should use HDR decoding path based on analysis.
/// Returns true if the image has HDR metadata or high bit depth.
pub fn needs_hdr_decode(hdr_info: Option<&ColorInfo>) -> bool {
    hdr_info.map_or(false, should_use_hdr_decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_hdr_decode() {
        let hdr_info = ColorInfo {
            bit_depth: Some(10),
            color_transfer: Some("smpte2084".to_string()),
            ..Default::default()
        };
        assert!(needs_hdr_decode(Some(&hdr_info)));

        let sdr_info = ColorInfo {
            bit_depth: Some(8),
            ..Default::default()
        };
        assert!(!needs_hdr_decode(Some(&sdr_info)));

        assert!(!needs_hdr_decode(None));
    }
}
