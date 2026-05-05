//! HDR Utilities Module
//!
//! Provides utilities for HDR metadata handling:
//! - CICP (Coding-Independent Code Points) mapping for JXL encoding
//! - `FFmpeg` HDR parameter generation for video encoding
//! - Color space and transfer function conversions

use crate::ffprobe_json::ColorInfo;
use std::path::{Path, PathBuf};

/// Convert `ColorInfo` to CICP string for JXL encoding.
/// CICP format: --cicp=`<primaries>`-`<transfer>`-`<matrix>`
///
/// # CICP Code Points
/// - Primaries: 1=BT.709, 9=BT.2020, 12=P3-D65
/// - Transfer: 1=BT.709, 13=sRGB, 16=PQ (SMPTE 2084), 18=HLG (ARIB STD-B67)
/// - Matrix: 1=BT.709, 9=BT.2020 non-constant, 0=RGB/Identity
///
/// Returns None if no HDR metadata is present.
#[must_use]
pub fn color_info_to_cicp(info: &ColorInfo) -> Option<String> {
    // Map color primaries to CICP code
    let primaries = match info.color_primaries.as_deref() {
        Some("bt709") => 1_i32,
        Some("bt2020") => 9_i32,
        Some("smpte432" | "display-p3") => 12_i32, // DCI-P3 / Display P3
        _ => {
            // If no primaries but has HDR transfer, assume BT.2020
            if info.color_transfer.as_deref() == Some("smpte2084")
                || info.color_transfer.as_deref() == Some("arib-std-b67")
            {
                9_i32
            } else {
                return None;
            }
        }
    };

    // Map transfer function to CICP code
    let transfer = match info.color_transfer.as_deref() {
        Some("smpte2084") => 16_i32,    // PQ (HDR10)
        Some("arib-std-b67") => 18_i32, // HLG
        Some("bt709") => 1_i32,
        Some("srgb" | "iec61966-2-1") => 13_i32,
        _ => {
            // If no transfer but has wide-gamut primaries, assume PQ
            if primaries == 9_i32 {
                16_i32
            } else {
                return None;
            }
        }
    };

    // Map color space (matrix coefficients) to CICP code
    let matrix = match info.color_space.as_deref() {
        Some("bt2020nc" | "bt2020-ncl") => 9_i32,
        Some("bt709") => 1_i32,
        Some("rgb" | "gbr") => 0_i32, // Identity/RGB
        _ => {
            // Infer from primaries
            if primaries == 9_i32 {
                9_i32 // BT.2020
            } else {
                i32::from(primaries == 1_i32) // 1 for BT.709, 0 for RGB/Identity (P3)
            }
        }
    };

    Some(format!("{primaries}-{transfer}-{matrix}"))
}

/// Convert `ColorInfo` to `FFmpeg` color parameters for video encoding.
/// Returns a vector of `FFmpeg` arguments: ["-colorspace", "bt2020nc", "-`color_trc`", "smpte2084", ...]
#[must_use]
pub fn color_info_to_ffmpeg_args(info: &ColorInfo) -> Vec<String> {
    let mut args = Vec::new();

    if let Some(ref colorspace) = info.color_space {
        args.push("-colorspace".to_string());
        args.push(colorspace.clone());
    }

    if let Some(ref trc) = info.color_transfer {
        args.push("-color_trc".to_string());
        args.push(trc.clone());
    }

    if let Some(ref primaries) = info.color_primaries {
        args.push("-color_primaries".to_string());
        args.push(primaries.clone());
    }

    if let Some(ref range) = info.color_range {
        args.push("-color_range".to_string());
        args.push(range.clone());
    }

    args
}

/// Infers missing color information for modern or high-definition content.
/// BT.709 is the correct standard for HD/modern content, while legacy SD content may expect BT.601.
///
/// Skips inference when any HDR signal is already present so a modern HDR HEIC/AVIF/HEIF
/// with a partially-populated color tag (e.g. `color_space=bt2020nc` but no transfer)
/// is not silently downgraded to SDR/sRGB.
#[must_use]
pub fn infer_bt709_if_modern(mut info: ColorInfo, width: u32, height: u32, ext: &str) -> ColorInfo {
    let is_hd = width >= 1280 || height >= 720;
    let is_modern_format = matches!(
        ext.to_lowercase().as_str(),
        "avif" | "webp" | "jxl" | "heic" | "heif" | "apng"
    );

    // Never override explicit HDR tags with sRGB/BT.709 defaults.
    let looks_hdr = info.bit_depth.is_some_and(|d| d > 8)
        || info.mastering_display.is_some()
        || info.max_cll.is_some()
        || matches!(
            info.color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        )
        || matches!(info.color_primaries.as_deref(), Some("bt2020"))
        || matches!(
            info.color_space.as_deref(),
            Some("bt2020nc" | "bt2020c" | "bt2020_ncl" | "bt2020ncl")
        );
    if looks_hdr {
        return info;
    }

    if is_hd || is_modern_format {
        if info.color_space.is_none() {
            info.color_space = Some("bt709".to_string());
        }
        if info.color_transfer.is_none() {
            info.color_transfer = Some("iec61966-2-1".to_string()); // sRGB
        }
        if info.color_primaries.is_none() {
            info.color_primaries = Some("bt709".to_string());
        }
    }

    info
}

/// Generate x265 HDR parameters for video encoding.
/// Returns a string suitable for x265 --hdr or --hdr10 options.
#[must_use]
pub fn color_info_to_x265_hdr_params(info: &ColorInfo) -> Option<String> {
    if !info.is_hdr() {
        return None;
    }

    let mut params = Vec::new();

    // Color primaries
    if let Some(ref primaries) = info.color_primaries {
        let code = match primaries.as_str() {
            "bt709" => "1",
            "smpte432" | "display-p3" => "12",
            _ => "9", // Default to BT.2020 for HDR
        };
        params.push(format!("colorprim={code}"));
    }

    // Transfer characteristics
    if let Some(ref trc) = info.color_transfer {
        let code = match trc.as_str() {
            "arib-std-b67" => "18",
            "bt709" => "1",
            _ => "16", // Default to PQ for HDR
        };
        params.push(format!("transfer={code}"));
    }

    // Color matrix
    if let Some(ref colorspace) = info.color_space {
        let code = match colorspace.as_str() {
            "bt709" => "1",
            _ => "9",
        };
        params.push(format!("colormatrix={code}"));
    }

    // Mastering display metadata
    if let Some(ref master) = info.mastering_display {
        params.push(format!("master-display={master}"));
    }

    // Content light level
    if let Some(ref cll) = info.max_cll {
        params.push(format!("max-cll={cll}"));
    }

    if params.is_empty() {
        None
    } else {
        Some(params.join(":"))
    }
}

/// Check if an image should use HDR decoding path (10-bit or higher).
#[must_use]
pub fn should_use_hdr_decode(info: &ColorInfo) -> bool {
    info.is_hdr() || info.bit_depth.is_some_and(|d| d > 8)
}

/// Get recommended pixel format for HDR content.
/// Returns "rgb48le" for 10-bit+ HDR, "rgb24" for SDR.
#[must_use]
pub fn get_hdr_pix_fmt(info: &ColorInfo) -> &'static str {
    if should_use_hdr_decode(info) {
        "rgb48le" // 16-bit RGB (3 channels × 16-bit)
    } else {
        "rgb24" // 8-bit RGB
    }
}

#[must_use]
pub fn is_dovi_tool_available() -> bool {
    crate::tool_builders::DoviBuilder::check_available()
}

#[must_use]
pub fn is_hdr10plus_tool_available() -> bool {
    crate::tool_builders::Hdr10PlusBuilder::check_available()
}

/// Extract raw HEVC Annex-B bitstream from a container using ffmpeg.
/// Returns the path to the raw `.hevc` file inside `temp_dir`.
///
/// # Errors
/// Returns an error if `ffmpeg` fails or the bitstream extraction fails.
pub fn extract_hevc_bitstream(input: &Path, temp_dir: &Path) -> Result<PathBuf, String> {
    let raw_hevc = temp_dir.join("raw.hevc");
    let status = crate::tool_builders::FfmpegBuilder::new()
        .overwrite()
        .input(input)
        .arg("-c:v")
        .arg("copy")
        .arg("-bsf:v")
        .arg("hevc_mp4toannexb")
        .arg("-an")
        .arg("-sn")
        .output(&raw_hevc)
        .build()
        .output()
        .map_err(|e| format!("failed to run ffmpeg for bitstream extraction: {e}"))?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(format!("ffmpeg bitstream extraction failed: {stderr}"));
    }
    Ok(raw_hevc)
}

/// Extract Dolby Vision RPU from a raw HEVC Annex-B bitstream using `dovi_tool`.
/// For Profile 7 sources, converts to Profile 8.1 (cross-compatible) automatically.
/// Returns the path to the `.bin` RPU file.
///
/// # Errors
/// Returns an error if `dovi_tool` fails or RPU extraction fails.
pub fn extract_dv_rpu(
    raw_hevc: &Path,
    temp_dir: &Path,
    dv_profile: Option<u8>,
) -> Result<PathBuf, String> {
    let rpu_path = temp_dir.join("rpu.bin");

    let output = crate::tool_builders::DoviBuilder::new()
        .mode("extract-rpu")
        .input(raw_hevc)
        .output(&rpu_path)
        .build()
        .output()
        .map_err(|e| format!("failed to run dovi_tool extract-rpu: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("dovi_tool extract-rpu failed: {stderr}"));
    }

    // Profile 7 → convert to 8.1 for x265 cross-compatibility
    if dv_profile == Some(7) {
        let converted_rpu = temp_dir.join("rpu_p81.bin");
        let conv_output = crate::tool_builders::DoviBuilder::new()
            .mode("convert")
            .arg("--discard")
            .input(&rpu_path)
            .output(&converted_rpu)
            .build()
            .output()
            .map_err(|e| format!("failed to run dovi_tool convert: {e}"))?;

        if !conv_output.status.success() {
            let stderr = String::from_utf8_lossy(&conv_output.stderr);
            return Err(format!(
                "dovi_tool convert (profile 7→8.1) failed: {stderr}"
            ));
        }
        return Ok(converted_rpu);
    }

    Ok(rpu_path)
}

/// Extract HDR10+ dynamic metadata from a raw HEVC Annex-B bitstream using `hdr10plus_tool`.
/// Returns the path to the `.json` metadata file.
///
/// # Errors
/// Returns an error if `hdr10plus_tool` fails or metadata extraction fails.
pub fn extract_hdr10plus_metadata(raw_hevc: &Path, temp_dir: &Path) -> Result<PathBuf, String> {
    let json_path = temp_dir.join("hdr10plus.json");

    let output = crate::tool_builders::Hdr10PlusBuilder::new()
        .mode("extract")
        .input(raw_hevc)
        .output(&json_path)
        .build()
        .output()
        .map_err(|e| format!("failed to run hdr10plus_tool extract: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_lower = stderr.to_lowercase();
        // Fallback for metadata with minor validation issues
        if stderr_lower.contains("error:") && stderr_lower.contains("invalid") {
            crate::log_eprintln!("⚠️  WRN  hdr10plus_tool exact extract validation failed, trying fallback with --skip-validation");

            let fb_output = crate::tool_builders::Hdr10PlusBuilder::new()
                .mode("extract")
                .skip_validation(true)
                .input(raw_hevc)
                .output(&json_path)
                .build()
                .output()
                .map_err(|e| format!("failed to run hdr10plus_tool extract (fallback): {e}"))?;
            if !fb_output.status.success() {
                let fb_stderr = String::from_utf8_lossy(&fb_output.stderr);
                return Err(format!(
                    "hdr10plus_tool extract fallback failed: {fb_stderr}"
                ));
            }
        } else {
            return Err(format!("hdr10plus_tool extract failed: {stderr}"));
        }
    }

    Ok(json_path)
}

/// Map DV profile + compatibility ID to the x265 `dolby-vision-profile` string.
/// Returns the numeric profile string that x265 expects (e.g. "8.1", "5.0").
#[must_use]
pub fn dv_x265_profile_string(dv_profile: Option<u8>, compat_id: Option<u8>) -> Option<String> {
    match dv_profile {
        Some(5) => Some("5.0".to_string()),
        Some(7) => {
            // Profile 7 gets converted to 8.1 by extract_dv_rpu
            Some("8.1".to_string())
        }
        Some(8) => {
            let sub = compat_id.unwrap_or(1);
            Some(format!("8.{sub}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cicp_hdr10() {
        let info = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("smpte2084".to_string()),
            color_space: Some("bt2020nc".to_string()),
            ..Default::default()
        };
        assert_eq!(color_info_to_cicp(&info), Some("9-16-9".to_string()));
    }

    #[test]
    fn test_cicp_hlg() {
        let info = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("arib-std-b67".to_string()),
            color_space: Some("bt2020nc".to_string()),
            ..Default::default()
        };
        assert_eq!(color_info_to_cicp(&info), Some("9-18-9".to_string()));
    }

    #[test]
    fn test_cicp_sdr() {
        let info = ColorInfo {
            color_primaries: Some("bt709".to_string()),
            color_transfer: Some("bt709".to_string()),
            color_space: Some("bt709".to_string()),
            ..Default::default()
        };
        assert_eq!(color_info_to_cicp(&info), Some("1-1-1".to_string()));
    }

    #[test]
    fn test_cicp_no_metadata() {
        let info = ColorInfo::default();
        assert_eq!(color_info_to_cicp(&info), None);
    }

    #[test]
    fn test_ffmpeg_args() {
        let info = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("smpte2084".to_string()),
            color_space: Some("bt2020nc".to_string()),
            ..Default::default()
        };
        let args = color_info_to_ffmpeg_args(&info);
        assert_eq!(
            args,
            vec![
                "-colorspace",
                "bt2020nc",
                "-color_trc",
                "smpte2084",
                "-color_primaries",
                "bt2020"
            ]
        );
    }

    #[test]
    fn test_should_use_hdr_decode() {
        let hdr_info = ColorInfo {
            color_transfer: Some("smpte2084".to_string()),
            bit_depth: Some(10),
            ..Default::default()
        };
        assert!(should_use_hdr_decode(&hdr_info));

        let sdr_info = ColorInfo {
            bit_depth: Some(8),
            ..Default::default()
        };
        assert!(!should_use_hdr_decode(&sdr_info));
    }

    #[test]
    fn test_get_hdr_pix_fmt() {
        let hdr_info = ColorInfo {
            bit_depth: Some(10),
            ..Default::default()
        };
        assert_eq!(get_hdr_pix_fmt(&hdr_info), "rgb48le");

        let sdr_info = ColorInfo {
            bit_depth: Some(8),
            ..Default::default()
        };
        assert_eq!(get_hdr_pix_fmt(&sdr_info), "rgb24");
    }
}
