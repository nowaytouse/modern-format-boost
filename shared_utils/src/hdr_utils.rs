//! HDR Utilities Module
//!
//! Provides utilities for HDR metadata handling:
//! - CICP (Coding-Independent Code Points) mapping for JXL encoding
//! - FFmpeg HDR parameter generation for video encoding
//! - Color space and transfer function conversions

use crate::ffprobe_json::ColorInfo;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Convert ColorInfo to CICP string for JXL encoding.
/// CICP format: --cicp=<primaries>-<transfer>-<matrix>
///
/// # CICP Code Points
/// - Primaries: 1=BT.709, 9=BT.2020, 12=P3-D65
/// - Transfer: 1=BT.709, 13=sRGB, 16=PQ (SMPTE 2084), 18=HLG (ARIB STD-B67)
/// - Matrix: 1=BT.709, 9=BT.2020 non-constant, 0=RGB/Identity
///
/// Returns None if no HDR metadata is present.
pub fn color_info_to_cicp(info: &ColorInfo) -> Option<String> {
    // Map color primaries to CICP code
    let primaries = match info.color_primaries.as_deref() {
        Some("bt709") => 1,
        Some("bt2020") => 9,
        Some("smpte432") | Some("display-p3") => 12, // DCI-P3 / Display P3
        _ => {
            // If no primaries but has HDR transfer, assume BT.2020
            if info.color_transfer.as_deref() == Some("smpte2084")
                || info.color_transfer.as_deref() == Some("arib-std-b67")
            {
                9
            } else {
                return None;
            }
        }
    };

    // Map transfer function to CICP code
    let transfer = match info.color_transfer.as_deref() {
        Some("smpte2084") => 16, // PQ (HDR10)
        Some("arib-std-b67") => 18, // HLG
        Some("bt709") => 1,
        Some("srgb") | Some("iec61966-2-1") => 13,
        _ => {
            // If no transfer but has wide-gamut primaries, assume PQ
            if primaries == 9 {
                16
            } else {
                return None;
            }
        }
    };

    // Map color space (matrix coefficients) to CICP code
    let matrix = match info.color_space.as_deref() {
        Some("bt2020nc") | Some("bt2020-ncl") => 9,
        Some("bt709") => 1,
        Some("rgb") | Some("gbr") => 0, // Identity/RGB
        _ => {
            // Infer from primaries
            if primaries == 9 {
                9 // BT.2020
            } else if primaries == 1 {
                1 // BT.709
            } else {
                0 // RGB/Identity for P3
            }
        }
    };

    Some(format!("{}-{}-{}", primaries, transfer, matrix))
}

/// Convert ColorInfo to FFmpeg color parameters for video encoding.
/// Returns a vector of FFmpeg arguments: ["-colorspace", "bt2020nc", "-color_trc", "smpte2084", ...]
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

    args
}

/// Generate x265 HDR parameters for video encoding.
/// Returns a string suitable for x265 --hdr or --hdr10 options.
pub fn color_info_to_x265_hdr_params(info: &ColorInfo) -> Option<String> {
    if !info.is_hdr() {
        return None;
    }

    let mut params = Vec::new();

    // Color primaries
    if let Some(ref primaries) = info.color_primaries {
        let code = match primaries.as_str() {
            "bt709" => "1",
            "bt2020" => "9",
            "smpte432" | "display-p3" => "12",
            _ => "9", // Default to BT.2020 for HDR
        };
        params.push(format!("colorprim={}", code));
    }

    // Transfer characteristics
    if let Some(ref trc) = info.color_transfer {
        let code = match trc.as_str() {
            "smpte2084" => "16",
            "arib-std-b67" => "18",
            "bt709" => "1",
            _ => "16", // Default to PQ for HDR
        };
        params.push(format!("transfer={}", code));
    }

    // Color matrix
    if let Some(ref colorspace) = info.color_space {
        let code = match colorspace.as_str() {
            "bt2020nc" | "bt2020-ncl" => "9",
            "bt709" => "1",
            _ => "9",
        };
        params.push(format!("colormatrix={}", code));
    }

    // Mastering display metadata
    if let Some(ref master) = info.mastering_display {
        params.push(format!("master-display={}", master));
    }

    // Content light level
    if let Some(ref cll) = info.max_cll {
        params.push(format!("max-cll={}", cll));
    }

    if params.is_empty() {
        None
    } else {
        Some(params.join(":"))
    }
}

/// Check if an image should use HDR decoding path (10-bit or higher).
pub fn should_use_hdr_decode(info: &ColorInfo) -> bool {
    info.is_hdr() || info.bit_depth.map_or(false, |d| d > 8)
}

/// Get recommended pixel format for HDR content.
/// Returns "rgb48le" for 10-bit+ HDR, "rgb24" for SDR.
pub fn get_hdr_pix_fmt(info: &ColorInfo) -> &'static str {
    if should_use_hdr_decode(info) {
        "rgb48le" // 16-bit RGB (3 channels × 16-bit)
    } else {
        "rgb24" // 8-bit RGB
    }
}

/// Check if `dovi_tool` binary is available on PATH.
pub fn is_dovi_tool_available() -> bool {
    Command::new("dovi_tool")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Extract raw HEVC Annex-B bitstream from a container using ffmpeg.
/// Returns the path to the raw `.hevc` file inside `temp_dir`.
pub fn extract_hevc_bitstream(input: &Path, temp_dir: &Path) -> Result<PathBuf, String> {
    let raw_hevc = temp_dir.join("raw.hevc");
    let status = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(input)
        .args([
            "-c:v", "copy",
            "-bsf:v", "hevc_mp4toannexb",
            "-an", "-sn",
        ])
        .arg(&raw_hevc)
        .output()
        .map_err(|e| format!("failed to run ffmpeg for bitstream extraction: {}", e))?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(format!("ffmpeg bitstream extraction failed: {}", stderr));
    }
    Ok(raw_hevc)
}

/// Extract Dolby Vision RPU from a raw HEVC Annex-B bitstream using `dovi_tool`.
/// For Profile 7 sources, converts to Profile 8.1 (cross-compatible) automatically.
/// Returns the path to the `.bin` RPU file.
pub fn extract_dv_rpu(
    raw_hevc: &Path,
    temp_dir: &Path,
    dv_profile: Option<u8>,
) -> Result<PathBuf, String> {
    let rpu_path = temp_dir.join("rpu.bin");

    let mut cmd = Command::new("dovi_tool");
    cmd.arg("extract-rpu")
        .arg("-i")
        .arg(raw_hevc)
        .arg("-o")
        .arg(&rpu_path);

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run dovi_tool extract-rpu: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("dovi_tool extract-rpu failed: {}", stderr));
    }

    // Profile 7 → convert to 8.1 for x265 cross-compatibility
    if dv_profile == Some(7) {
        let converted_rpu = temp_dir.join("rpu_p81.bin");
        let conv_output = Command::new("dovi_tool")
            .arg("convert")
            .arg("--discard")
            .arg("-i")
            .arg(&rpu_path)
            .arg("-o")
            .arg(&converted_rpu)
            .output()
            .map_err(|e| format!("failed to run dovi_tool convert: {}", e))?;

        if !conv_output.status.success() {
            let stderr = String::from_utf8_lossy(&conv_output.stderr);
            return Err(format!("dovi_tool convert (profile 7→8.1) failed: {}", stderr));
        }
        return Ok(converted_rpu);
    }

    Ok(rpu_path)
}

/// Map DV profile + compatibility ID to the x265 `dolby-vision-profile` string.
/// Returns the numeric profile string that x265 expects (e.g. "8.1", "5.0").
pub fn dv_x265_profile_string(dv_profile: Option<u8>, compat_id: Option<u8>) -> Option<String> {
    match dv_profile {
        Some(5) => Some("5.0".to_string()),
        Some(7) => {
            // Profile 7 gets converted to 8.1 by extract_dv_rpu
            Some("8.1".to_string())
        }
        Some(8) => {
            let sub = compat_id.unwrap_or(1);
            Some(format!("8.{}", sub))
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
