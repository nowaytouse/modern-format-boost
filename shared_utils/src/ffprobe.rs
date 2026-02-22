//! FFprobe wrapper module
//!
//! Shared FFprobe functionality for video analysis.
//! Used by vidquality and vid-hevc.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub enum FFprobeError {
    ToolNotFound(String),
    ExecutionFailed(String),
    ParseError(String),
    IoError(io::Error),
}

impl std::fmt::Display for FFprobeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FFprobeError::ToolNotFound(s) => write!(f, "Tool not found: {}", s),
            FFprobeError::ExecutionFailed(s) => write!(f, "FFprobe failed: {}", s),
            FFprobeError::ParseError(s) => write!(f, "Parse error: {}", s),
            FFprobeError::IoError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for FFprobeError {}

impl From<io::Error> for FFprobeError {
    fn from(e: io::Error) -> Self {
        FFprobeError::IoError(e)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FFprobeResult {
    pub format_name: String,
    pub duration: f64,
    pub size: u64,
    pub bit_rate: u64,
    pub video_codec: String,
    pub video_codec_long: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub frame_count: u64,
    pub pix_fmt: String,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub bit_depth: u8,
    pub has_audio: bool,
    pub audio_codec: Option<String>,
    pub audio_bit_rate: Option<u64>,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub profile: Option<String>,
    pub level: Option<String>,
    pub has_b_frames: bool,
    pub video_bit_rate: Option<u64>,
    pub refs: Option<u32>,
}

pub fn is_ffprobe_available() -> bool {
    Command::new("ffprobe").arg("-version").output().is_ok()
}

pub fn probe_video(path: &Path) -> Result<FFprobeResult, FFprobeError> {
    if !is_ffprobe_available() {
        return Err(FFprobeError::ToolNotFound(
            "ffprobe not found. Install with: brew install ffmpeg".to_string(),
        ));
    }

    if !path.exists() {
        return Err(FFprobeError::ExecutionFailed(format!(
            "File not found: {}",
            path.display()
        )));
    }

    if !path.is_file() {
        return Err(FFprobeError::ExecutionFailed(format!(
            "Not a file (is it a directory?): {}",
            path.display()
        )));
    }

    let path_str = path.to_str().ok_or_else(|| {
        FFprobeError::ExecutionFailed(format!("Invalid path encoding: {}", path.display()))
    })?;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "--",
            path_str,
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error_msg = if stderr.trim().is_empty() {
            format!(
                "ffprobe failed to analyze file: {} (exit code: {:?})",
                path.display(),
                output.status.code()
            )
        } else {
            format!("ffprobe error for '{}': {}", path.display(), stderr.trim())
        };
        return Err(FFprobeError::ExecutionFailed(error_msg));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| FFprobeError::ParseError(e.to_string()))?;

    let format = &json["format"];
    let format_name = format["format_name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let duration = format["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let size = format["size"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let bit_rate = format["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let streams = json["streams"]
        .as_array()
        .ok_or_else(|| FFprobeError::ParseError("No streams found".to_string()))?;

    let video_stream = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("video"))
        .ok_or_else(|| FFprobeError::ParseError("No video stream found".to_string()))?;

    let video_codec = video_stream["codec_name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let video_codec_long = video_stream["codec_long_name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let width = video_stream["width"].as_u64().unwrap_or(0) as u32;
    let height = video_stream["height"].as_u64().unwrap_or(0) as u32;

    let frame_rate = parse_frame_rate(video_stream["r_frame_rate"].as_str().unwrap_or("0/1"));

    let frame_count = video_stream["nb_frames"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or((duration * frame_rate) as u64);

    let pix_fmt = video_stream["pix_fmt"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let color_space = video_stream["color_space"].as_str().map(|s| s.to_string());
    let color_transfer = video_stream["color_transfer"]
        .as_str()
        .map(|s| s.to_string());

    let bit_depth = detect_bit_depth(&pix_fmt);

    let profile = video_stream["profile"].as_str().map(|s| s.to_string());
    let level = video_stream["level"]
        .as_u64()
        .map(|l| format!("{:.1}", l as f64 / 10.0));
    let has_b_frames = video_stream["has_b_frames"].as_u64().unwrap_or(0) > 0;
    let video_bit_rate = video_stream["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok());
    let refs = video_stream["refs"].as_u64().map(|r| r as u32);

    let audio_stream = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("audio"));
    let has_audio = audio_stream.is_some();
    let audio_codec = audio_stream
        .and_then(|s| s["codec_name"].as_str())
        .map(|s| s.to_string());
    let audio_bit_rate = audio_stream
        .and_then(|s| s["bit_rate"].as_str())
        .and_then(|s| s.parse::<u64>().ok());
    let audio_sample_rate = audio_stream
        .and_then(|s| s["sample_rate"].as_str())
        .and_then(|s| s.parse::<u32>().ok());
    let audio_channels = audio_stream
        .and_then(|s| s["channels"].as_u64())
        .map(|c| c as u32);

    Ok(FFprobeResult {
        format_name,
        duration,
        size,
        bit_rate,
        video_codec,
        video_codec_long,
        width,
        height,
        frame_rate,
        frame_count,
        pix_fmt,
        color_space,
        color_transfer,
        bit_depth,
        has_audio,
        audio_codec,
        audio_bit_rate,
        audio_sample_rate,
        audio_channels,
        profile,
        level,
        has_b_frames,
        video_bit_rate,
        refs,
    })
}

pub fn get_duration(path: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            "--",
            path.to_str()?,
        ])
        .output()
        .ok()?;

    if output.status.success() {
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<f64>()
            .ok()
    } else {
        None
    }
}

pub fn get_frame_count(path: &Path) -> Option<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            "--",
            path.to_str()?,
        ])
        .output()
        .ok()?;

    if output.status.success() {
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok()
    } else {
        None
    }
}

const FALLBACK_FRAME_RATE: f64 = 24.0;

pub fn parse_frame_rate(s: &str) -> f64 {
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let num = parts[0].parse::<f64>().unwrap_or(0.0);
            let den = parts[1].parse::<f64>().unwrap_or(0.0);
            if den > 0.0 {
                let rate = num / den;
                if rate > 0.0 {
                    return rate;
                }
            }
        }
    }
    match s.parse::<f64>() {
        Ok(v) if v > 0.0 => v,
        _ => {
            if !s.is_empty() && s != "0" && s != "0/1" {
                eprintln!("⚠️ [ffprobe] Failed to parse frame rate '{}', using fallback {}fps", s, FALLBACK_FRAME_RATE);
            }
            FALLBACK_FRAME_RATE
        }
    }
}

pub fn detect_bit_depth(pix_fmt: &str) -> u8 {

    if pix_fmt.contains("16le") || pix_fmt.contains("16be") ||
       pix_fmt.contains("48le") || pix_fmt.contains("48be") ||
       pix_fmt.contains("64le") || pix_fmt.contains("64be")
    {
        return 16;
    }

    if pix_fmt.contains("12le") || pix_fmt.contains("12be") {
        return 12;
    }

    if pix_fmt.contains("10le")
        || pix_fmt.contains("10be")
        || pix_fmt.contains("p010")
        || pix_fmt.contains("p210")
        || pix_fmt.contains("p410")
    {
        return 10;
    }

    8
}


#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn test_parse_frame_rate() {
        let cases: &[(&str, f64, f64)] = &[
            ("30/1", 30.0, 0.001),
            ("24/1", 24.0, 0.001),
            ("60/1", 60.0, 0.001),
            ("25/1", 25.0, 0.001),
            ("30000/1001", 30000.0 / 1001.0, 0.0001),
            ("24000/1001", 24000.0 / 1001.0, 0.0001),
            ("60000/1001", 60000.0 / 1001.0, 0.0001),
            ("24", 24.0, 0.001),
            ("29.97", 29.97, 0.01),
            ("59.94", 59.94, 0.01),
            ("120/1", 120.0, 0.001),
            ("240/1", 240.0, 0.001),
            ("144/1", 144.0, 0.001),
        ];

        for (input, expected, tolerance) in cases {
            let result = parse_frame_rate(input);
            assert!(
                (result - expected).abs() < *tolerance,
                "parse_frame_rate({:?}): expected {}, got {}",
                input, expected, result
            );
        }
    }

    #[test]
    fn test_parse_frame_rate_edge_cases() {
        assert_eq!(parse_frame_rate("30/0"), FALLBACK_FRAME_RATE);
        assert_eq!(parse_frame_rate("invalid"), FALLBACK_FRAME_RATE);
        assert_eq!(parse_frame_rate(""), FALLBACK_FRAME_RATE);
        assert_eq!(parse_frame_rate("30/1/extra"), FALLBACK_FRAME_RATE);
    }


    #[test]
    fn test_detect_bit_depth() {
        let cases: &[(&str, u8)] = &[
            ("yuv420p", 8), ("yuv422p", 8), ("yuv444p", 8),
            ("rgb24", 8), ("bgr24", 8), ("nv12", 8), ("yuvj420p", 8),
            ("yuv420p10le", 10), ("yuv420p10be", 10), ("yuv422p10le", 10),
            ("yuv444p10le", 10), ("p010le", 10), ("p010", 10),
            ("yuv420p12le", 12), ("yuv420p12be", 12),
            ("yuv422p12le", 12), ("yuv444p12le", 12),
            ("yuv420p16le", 16), ("yuv420p16be", 16), ("rgb48le", 16),
            ("unknown", 8), ("", 8), ("custom_format", 8),
        ];

        for (fmt, expected) in cases {
            assert_eq!(
                detect_bit_depth(fmt), *expected,
                "detect_bit_depth({:?}) mismatch", fmt
            );
        }
    }
}
