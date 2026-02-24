//! 🔥 v6.5: FFprobe JSON 解析模块
//! 使用 serde_json 替代手动字符串解析

use serde::Deserialize;
use std::path::Path;
use std::process::Command;
use tracing::warn;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeStream {
    #[serde(default)]
    pub color_space: Option<String>,
    #[serde(default)]
    pub color_transfer: Option<String>,
    #[serde(default)]
    pub pix_fmt: Option<String>,
    #[serde(default)]
    pub bits_per_raw_sample: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeOutput {
    #[serde(default)]
    pub streams: Vec<FfprobeStream>,
}

#[derive(Debug, Clone, Default)]
pub struct ColorInfo {
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub pix_fmt: Option<String>,
    pub bit_depth: Option<u8>,
}

pub fn extract_color_info(input: &Path) -> ColorInfo {
    let input_str = input.to_string_lossy();

    let output = match Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-select_streams",
            "v:0",
            input_str.as_ref(),
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        Ok(_) => {
            warn!(input = %input_str, "FFPROBE FAILED: non-zero exit");
            return ColorInfo::default();
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

    let color_space = stream
        .color_space
        .clone()
        .filter(|s| !s.is_empty() && s != "unknown");

    let color_transfer = stream
        .color_transfer
        .clone()
        .filter(|s| !s.is_empty() && s != "unknown");

    ColorInfo {
        color_space,
        color_transfer,
        pix_fmt: stream.pix_fmt.clone(),
        bit_depth,
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
