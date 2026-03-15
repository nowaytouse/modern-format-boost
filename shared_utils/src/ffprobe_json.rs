//! 🔥 v6.5: FFprobe JSON 解析模块
//! 使用 serde_json 替代手动字符串解析

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use tracing::warn;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeSideData {
    pub side_data_type: Option<String>,
    // mastering display fields
    pub green_x: Option<serde_json::Value>,
    pub green_y: Option<serde_json::Value>,
    pub blue_x: Option<serde_json::Value>,
    pub blue_y: Option<serde_json::Value>,
    pub red_x: Option<serde_json::Value>,
    pub red_y: Option<serde_json::Value>,
    pub white_point_x: Option<serde_json::Value>,
    pub white_point_y: Option<serde_json::Value>,
    pub max_luminance: Option<serde_json::Value>,
    pub min_luminance: Option<serde_json::Value>,
    // CLL fields
    pub max_content: Option<u64>,
    pub max_average: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeStream {
    #[serde(default)]
    pub color_space: Option<String>,
    #[serde(default)]
    pub color_transfer: Option<String>,
    #[serde(default)]
    pub color_primaries: Option<String>,
    #[serde(default)]
    pub pix_fmt: Option<String>,
    #[serde(default)]
    pub bits_per_raw_sample: Option<String>,
    #[serde(default)]
    pub side_data_list: Vec<FfprobeSideData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeFrame {
    #[serde(default)]
    pub side_data_list: Vec<FfprobeSideData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeOutput {
    #[serde(default)]
    pub streams: Vec<FfprobeStream>,
    #[serde(default)]
    pub frames: Vec<FfprobeFrame>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColorInfo {
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub pix_fmt: Option<String>,
    pub bit_depth: Option<u8>,
    /// HDR10 mastering display string (ffmpeg format)
    pub mastering_display: Option<String>,
    /// HDR10 CLL: "MaxCLL,MaxFALL"
    pub max_cll: Option<String>,
    pub is_dolby_vision: bool,
    pub is_hdr10_plus: bool,
}

impl ColorInfo {
    /// Returns true when the content is any form of HDR (PQ, HLG, DV, HDR10, HDR10+)
    pub fn is_hdr(&self) -> bool {
        self.is_dolby_vision
            || self.is_hdr10_plus
            || self.mastering_display.is_some()
            || self.max_cll.is_some()
            || matches!(
                self.color_transfer.as_deref(),
                Some("smpte2084") | Some("arib-std-b67")
            )
    }
}

fn rational_to_50k(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::String(s) => {
            if let Some((n, d)) = s.split_once('/') {
                let n: f64 = n.trim().parse().ok()?;
                let d: f64 = d.trim().parse().ok()?;
                if d == 0.0 { return None; }
                Some(((n / d) * 50000.0).round() as u64)
            } else {
                let f: f64 = s.trim().parse().ok()?;
                if f <= 1.0 { Some((f * 50000.0).round() as u64) } else { Some(f.round() as u64) }
            }
        }
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if f <= 1.0 { Some((f * 50000.0).round() as u64) } else { Some(f.round() as u64) }
        }
        _ => None,
    }
}

fn rational_to_10k(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::String(s) => {
            if let Some((n, d)) = s.split_once('/') {
                let n: f64 = n.trim().parse().ok()?;
                let d: f64 = d.trim().parse().ok()?;
                if d == 0.0 { return None; }
                Some(((n / d) * 10000.0).round() as u64)
            } else {
                let f: f64 = s.trim().parse().ok()?;
                if f <= 10000.0 { Some((f * 10000.0).round() as u64) } else { Some(f.round() as u64) }
            }
        }
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if f <= 10000.0 { Some((f * 10000.0).round() as u64) } else { Some(f.round() as u64) }
        }
        _ => None,
    }
}

fn parse_side_data_list(
    entries: &[FfprobeSideData],
    is_dolby_vision: &mut bool,
    is_hdr10_plus: &mut bool,
    mastering_display: &mut Option<String>,
    max_cll: &mut Option<String>,
) {
    for sd in entries {
        let sd_type = sd.side_data_type.as_deref().unwrap_or("").to_lowercase();

        if sd_type.contains("dolby vision") || sd_type.contains("dovi") {
            *is_dolby_vision = true;
        }
        if sd_type.contains("hdr dynamic") || sd_type.contains("st2094") || sd_type.contains("hdr10+") {
            *is_hdr10_plus = true;
        }
        if sd_type.contains("mastering display") && mastering_display.is_none() {
            if let (Some(gx), Some(gy), Some(bx), Some(by_), Some(rx), Some(ry), Some(wx), Some(wy), Some(lmax), Some(lmin)) = (
                sd.green_x.as_ref().and_then(rational_to_50k),
                sd.green_y.as_ref().and_then(rational_to_50k),
                sd.blue_x.as_ref().and_then(rational_to_50k),
                sd.blue_y.as_ref().and_then(rational_to_50k),
                sd.red_x.as_ref().and_then(rational_to_50k),
                sd.red_y.as_ref().and_then(rational_to_50k),
                sd.white_point_x.as_ref().and_then(rational_to_50k),
                sd.white_point_y.as_ref().and_then(rational_to_50k),
                sd.max_luminance.as_ref().and_then(rational_to_10k),
                sd.min_luminance.as_ref().and_then(rational_to_10k),
            ) {
                *mastering_display = Some(format!(
                    "G({gx},{gy})B({bx},{by_})R({rx},{ry})WP({wx},{wy})L({lmax},{lmin})"
                ));
            }
        }
        if sd_type.contains("content light level") && max_cll.is_none() {
            if let (Some(mc), Some(ma)) = (sd.max_content, sd.max_average) {
                *max_cll = Some(format!("{},{}", mc, ma));
            }
        }
    }
}

pub fn extract_color_info(input: &Path) -> ColorInfo {
    let input_str = input.to_string_lossy();

    let output = match Command::new("ffprobe")
        .args([
            "-v",
            "error",  // Use "error" instead of "quiet" to capture stderr for fallback detection
            "-print_format",
            "json",
            "-show_streams",
            "-show_frames",
            "-read_intervals",
            "%+#5",
            "-select_streams",
            "v:0",
            "--",
        ])
        .arg(crate::safe_path_arg(input).as_ref())
        .output()
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            // Check if failure is due to image2 demuxer pattern matching (e.g., filenames with [])
            let stderr = String::from_utf8_lossy(&o.stderr);
            if stderr.contains("Could find no file with path") && stderr.contains("and index in the range") {
                crate::log_rare_error!("FFprobe", "Image2 demuxer pattern matching failed for file: {} - Retrying with -pattern_type none", input_str);
                // Retry with -pattern_type none to disable sequence pattern matching
                match Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-pattern_type",
                        "none",
                        "-print_format",
                        "json",
                        "-show_streams",
                        "-show_frames",
                        "-read_intervals",
                        "%+#5",
                        "-select_streams",
                        "v:0",
                        "--",
                    ])
                    .arg(crate::safe_path_arg(input).as_ref())
                    .output()
                {
                    Ok(retry_o) if retry_o.status.success() => retry_o,
                    _ => {
                        crate::log_rare_error!("FFprobe", "Pattern_type fallback also failed for: {}", input_str);
                        return ColorInfo::default();
                    }
                }
            } else {
                warn!(input = %input_str, "FFPROBE FAILED: non-zero exit");
                return ColorInfo::default();
            }
        }
        Err(e) => {
            warn!(error = %e, input = %input_str, "FFPROBE ERROR");
            return ColorInfo::default();
        }
    };

    let json_str = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "FFPROBE UTF8 ERROR");
            return ColorInfo::default();
        }
    };

    let parsed: FfprobeOutput = match serde_json::from_str(&json_str) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "FFPROBE JSON PARSE ERROR");
            return ColorInfo::default();
        }
    };

    let stream = match parsed.streams.first() {
        Some(s) => s,
        None => return ColorInfo::default(),
    };

    let bit_depth = stream
        .bits_per_raw_sample
        .as_ref()
        .and_then(|s| s.parse::<u8>().ok());

    let color_space = stream.color_space.clone().filter(|s| !s.is_empty() && s != "unknown");
    let color_transfer = stream.color_transfer.clone().filter(|s| !s.is_empty() && s != "unknown");
    let color_primaries = stream.color_primaries.clone().filter(|s| !s.is_empty() && s != "unknown");

    let mut is_dolby_vision = false;
    let mut is_hdr10_plus = false;
    let mut mastering_display: Option<String> = None;
    let mut max_cll: Option<String> = None;

    parse_side_data_list(
        &stream.side_data_list,
        &mut is_dolby_vision,
        &mut is_hdr10_plus,
        &mut mastering_display,
        &mut max_cll,
    );

    for frame in &parsed.frames {
        parse_side_data_list(
            &frame.side_data_list,
            &mut is_dolby_vision,
            &mut is_hdr10_plus,
            &mut mastering_display,
            &mut max_cll,
        );
    }

    ColorInfo {
        color_space,
        color_transfer,
        color_primaries,
        pix_fmt: stream.pix_fmt.clone(),
        bit_depth,
        mastering_display,
        max_cll,
        is_dolby_vision,
        is_hdr10_plus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_json() {
        let json = r#"{"streams":[{"color_space":"bt709","pix_fmt":"yuv420p","bits_per_raw_sample":"8"}]}"#;
        let parsed: FfprobeOutput = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.streams.len(), 1);
        assert_eq!(parsed.streams[0].color_space, Some("bt709".to_string()));
        assert_eq!(parsed.streams[0].pix_fmt, Some("yuv420p".to_string()));
    }

    #[test]
    fn test_parse_empty_streams() {
        let json = r#"{"streams":[]}"#;
        let parsed: FfprobeOutput = serde_json::from_str(json).unwrap();
        assert!(parsed.streams.is_empty());
    }

    #[test]
    fn test_parse_missing_fields() {
        let json = r#"{"streams":[{"pix_fmt":"yuv420p10le"}]}"#;
        let parsed: FfprobeOutput = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.streams[0].color_space, None);
        assert_eq!(parsed.streams[0].pix_fmt, Some("yuv420p10le".to_string()));
    }

    #[test]
    fn test_is_hdr_pq() {
        let ci = ColorInfo {
            color_transfer: Some("smpte2084".to_string()),
            ..Default::default()
        };
        assert!(ci.is_hdr());
    }

    #[test]
    fn test_is_hdr_hlg() {
        let ci = ColorInfo {
            color_transfer: Some("arib-std-b67".to_string()),
            ..Default::default()
        };
        assert!(ci.is_hdr());
    }

    #[test]
    fn test_not_hdr_sdr() {
        let ci = ColorInfo {
            color_space: Some("bt709".to_string()),
            color_transfer: Some("bt709".to_string()),
            ..Default::default()
        };
        assert!(!ci.is_hdr());
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_json_parse_roundtrip(
            cs in "[a-z0-9]{1,10}",
            pf in "[a-z0-9]{1,15}",
            bd in 8u8..=16
        ) {
            let json = format!(
                r#"{{"streams":[{{"color_space":"{}","pix_fmt":"{}","bits_per_raw_sample":"{}"}}]}}"#,
                cs, pf, bd
            );
            let parsed: Result<FfprobeOutput, _> = serde_json::from_str(&json);
            prop_assert!(parsed.is_ok());
            let p = parsed.unwrap();
            prop_assert_eq!(p.streams[0].color_space.clone(), Some(cs));
            prop_assert_eq!(p.streams[0].pix_fmt.clone(), Some(pf));
        }

        #[test]
        fn prop_invalid_json_no_panic(s in ".*") {
            let _ = serde_json::from_str::<FfprobeOutput>(&s);
        }
    }
}
