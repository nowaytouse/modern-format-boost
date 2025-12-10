//! FFprobe wrapper module
//!
//! Shared FFprobe functionality for video analysis.
//! Used by vidquality and vidquality-hevc.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::io;

/// FFprobe error types
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

/// FFprobe analysis result
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
    // Enhanced fields for precise CRF matching
    pub profile: Option<String>,        // H.264 profile (Baseline/Main/High)
    pub level: Option<String>,          // H.264 level (3.1, 4.0, etc.)
    pub has_b_frames: bool,             // Whether B-frames are used
    pub video_bit_rate: Option<u64>,    // Video stream specific bitrate
    pub refs: Option<u32>,              // Reference frames
}

/// Check if ffprobe is available
pub fn is_ffprobe_available() -> bool {
    Command::new("ffprobe").arg("-version").output().is_ok()
}

/// Probe video file using ffprobe
pub fn probe_video(path: &Path) -> Result<FFprobeResult, FFprobeError> {
    // Check if ffprobe exists
    if !is_ffprobe_available() {
        return Err(FFprobeError::ToolNotFound("ffprobe not found. Install with: brew install ffmpeg".to_string()));
    }
    
    // 🔥 检查文件是否存在
    if !path.exists() {
        return Err(FFprobeError::ExecutionFailed(format!(
            "File not found: {}", path.display()
        )));
    }
    
    // 🔥 检查是否是文件（不是目录）
    if !path.is_file() {
        return Err(FFprobeError::ExecutionFailed(format!(
            "Not a file (is it a directory?): {}", path.display()
        )));
    }
    
    let path_str = path.to_str().ok_or_else(|| {
        FFprobeError::ExecutionFailed(format!(
            "Invalid path encoding: {}", path.display()
        ))
    })?;
    
    let output = Command::new("ffprobe")
        .args(&[
            "-v", "error",  // 🔥 改为 error 级别以获取错误信息
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            path_str,
        ])
        .output()?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error_msg = if stderr.trim().is_empty() {
            format!("ffprobe failed to analyze file: {} (exit code: {:?})", 
                path.display(), output.status.code())
        } else {
            format!("ffprobe error for '{}': {}", path.display(), stderr.trim())
        };
        return Err(FFprobeError::ExecutionFailed(error_msg));
    }
    
    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| FFprobeError::ParseError(e.to_string()))?;
    
    // Extract format info
    let format = &json["format"];
    let format_name = format["format_name"].as_str().unwrap_or("unknown").to_string();
    let duration = format["duration"].as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let size = format["size"].as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let bit_rate = format["bit_rate"].as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    
    // Find video stream
    let streams = json["streams"].as_array()
        .ok_or_else(|| FFprobeError::ParseError("No streams found".to_string()))?;
    
    let video_stream = streams.iter()
        .find(|s| s["codec_type"].as_str() == Some("video"))
        .ok_or_else(|| FFprobeError::ParseError("No video stream found".to_string()))?;
    
    let video_codec = video_stream["codec_name"].as_str().unwrap_or("unknown").to_string();
    let video_codec_long = video_stream["codec_long_name"].as_str().unwrap_or("").to_string();
    let width = video_stream["width"].as_u64().unwrap_or(0) as u32;
    let height = video_stream["height"].as_u64().unwrap_or(0) as u32;
    
    // Parse frame rate (e.g., "30/1" or "29.97")
    let frame_rate = parse_frame_rate(
        video_stream["r_frame_rate"].as_str().unwrap_or("0/1")
    );
    
    // Get frame count
    let frame_count = video_stream["nb_frames"].as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| (duration * frame_rate) as u64);
    
    let pix_fmt = video_stream["pix_fmt"].as_str().unwrap_or("unknown").to_string();
    let color_space = video_stream["color_space"].as_str().map(|s| s.to_string());
    let color_transfer = video_stream["color_transfer"].as_str().map(|s| s.to_string());
    
    // Determine bit depth from pixel format
    let bit_depth = detect_bit_depth(&pix_fmt);
    
    // Enhanced fields for precise CRF matching
    let profile = video_stream["profile"].as_str().map(|s| s.to_string());
    let level = video_stream["level"].as_u64().map(|l| format!("{:.1}", l as f64 / 10.0));
    let has_b_frames = video_stream["has_b_frames"].as_u64().unwrap_or(0) > 0;
    let video_bit_rate = video_stream["bit_rate"].as_str()
        .and_then(|s| s.parse::<u64>().ok());
    let refs = video_stream["refs"].as_u64().map(|r| r as u32);
    
    // Check for audio
    let has_audio = streams.iter().any(|s| s["codec_type"].as_str() == Some("audio"));
    let audio_codec = streams.iter()
        .find(|s| s["codec_type"].as_str() == Some("audio"))
        .and_then(|s| s["codec_name"].as_str())
        .map(|s| s.to_string());
    
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
        profile,
        level,
        has_b_frames,
        video_bit_rate,
        refs,
    })
}

/// Get animation/video duration in seconds
/// Works for both video files and animated images (GIF, WebP, APNG)
pub fn get_duration(path: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
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

/// Get frame count for video/animation
pub fn get_frame_count(path: &Path) -> Option<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-count_frames",
            "-select_streams", "v:0",
            "-show_entries", "stream=nb_read_frames",
            "-of", "default=noprint_wrappers=1:nokey=1",
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

/// Parse frame rate string (e.g., "30/1" or "29.97")
pub fn parse_frame_rate(s: &str) -> f64 {
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let num = parts[0].parse::<f64>().unwrap_or(0.0);
            let den = parts[1].parse::<f64>().unwrap_or(1.0);
            if den > 0.0 {
                return num / den;
            }
        }
    }
    s.parse::<f64>().unwrap_or(0.0)
}

/// Detect bit depth from pixel format
pub fn detect_bit_depth(pix_fmt: &str) -> u8 {
    if pix_fmt.contains("10le") || pix_fmt.contains("10be") || pix_fmt.contains("p010") {
        10
    } else if pix_fmt.contains("12le") || pix_fmt.contains("12be") {
        12
    } else if pix_fmt.contains("16le") || pix_fmt.contains("16be") {
        16
    } else {
        8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_frame_rate() {
        assert!((parse_frame_rate("30/1") - 30.0).abs() < 0.001);
        assert!((parse_frame_rate("30000/1001") - 29.97).abs() < 0.01);
        assert!((parse_frame_rate("24") - 24.0).abs() < 0.001);
    }
    
    #[test]
    fn test_detect_bit_depth() {
        assert_eq!(detect_bit_depth("yuv420p"), 8);
        assert_eq!(detect_bit_depth("yuv420p10le"), 10);
        assert_eq!(detect_bit_depth("yuv422p12le"), 12);
    }
}
