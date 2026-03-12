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
    pub color_primaries: Option<String>,
    pub bit_depth: u8,
    pub has_audio: bool,
    pub audio_codec: Option<String>,
    pub audio_bit_rate: Option<u64>,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub profile: Option<String>,
    pub level: Option<String>,
    /// Actual B-frame count (max_b_frames) from ffprobe.
    pub max_b_frames: u8,
    pub has_b_frames: bool,
    /// Raw encoder settings string (e.g. from x264-params or x265-params tags).
    pub encoder_settings: Option<String>,
    pub video_bit_rate: Option<u64>,
    pub refs: Option<u32>,
    /// HDR10 mastering display metadata (e.g. "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,500)")
    pub mastering_display: Option<String>,
    /// HDR10 content light level metadata (e.g. "MaxCLL=1000,MaxFALL=400")
    pub max_cll: Option<String>,
    /// True when content uses Dolby Vision (side data detected)
    pub is_dolby_vision: bool,
    /// Dolby Vision profile number (5, 7, 8, etc.) — None if not DV
    pub dv_profile: Option<u8>,
    /// Dolby Vision BL signal compatibility ID (used to determine cross-compat)
    pub dv_bl_signal_compatibility_id: Option<u8>,
    /// True when content uses HDR10+ dynamic metadata (SMPTE ST 2094-40)
    pub is_hdr10_plus: bool,
    /// True when at least one subtitle stream is present
    pub has_subtitles: bool,
    /// Codec name of the first subtitle stream (e.g. "srt", "ass", "mov_text", "hdmv_pga_subtitle")
    pub subtitle_codec: Option<String>,
    /// Variable frame rate detected (r_frame_rate != avg_frame_rate)
    pub is_variable_frame_rate: bool,
    /// Stream index of the selected video stream (for multi-stream files like animated AVIF)
    pub stream_index: usize,
    /// Format tags (e.g. encoder, creation_time) from the format section
    pub tags: std::collections::HashMap<String, String>,
}

pub fn is_ffprobe_available() -> bool {
    Command::new("ffprobe").arg("-version").output().is_ok()
}

/// Enhanced VFR detection with slow-motion video handling
fn detect_vfr_enhanced(
    video_stream: &serde_json::Value,
    r_frame_rate: f64,
    avg_frame_rate: f64,
    format_name: &str,
) -> bool {
    if r_frame_rate <= 0.0 || avg_frame_rate <= 0.0 {
        return false;
    }

    // Slow-motion detection (separate logic for reliability)
    if (format_name.contains("mov") || format_name.contains("mp4")) && avg_frame_rate >= 60.0 {
        // Check for Apple's slow-mo tag (most reliable indicator)
        if video_stream["tags"]["com.apple.quicktime.fullframerate"].is_string() {
            return true;
        }

        // Check for significant frame rate ratio (recording vs playback)
        if r_frame_rate / avg_frame_rate > 2.0 {
            return true;
        }
    }

    // Standard VFR detection with 2% threshold
    let diff_ratio = (r_frame_rate - avg_frame_rate).abs() / r_frame_rate;
    diff_ratio > 0.02
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

    let path_arg = crate::safe_path_arg(path);
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_frames",
            "-read_intervals",
            "%+#5",
            "--",
        ])
        .arg(path_arg.as_ref())
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
        .ok_or_else(|| FFprobeError::ParseError("Missing format_name".to_string()))?
        .to_string();

    let size = format["size"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| FFprobeError::ParseError("Missing or invalid file size".to_string()))?;

    let bit_rate = format["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0); // overall bitrate can be 0 if unknown, handled downstream

    let mut duration = format["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    
    let mut tags = std::collections::HashMap::new();
    if let Some(tags_obj) = format["tags"].as_object() {
        for (k, v) in tags_obj {
            if let Some(val) = v.as_str() {
                tags.insert(k.clone(), val.to_string());
            }
        }
    }

    let streams = json["streams"]
        .as_array()
        .ok_or_else(|| FFprobeError::ParseError("No streams found".to_string()))?;

    // For animated images (AVIF/HEIC) with multiple video streams, select the one with most frames
    // First stream is often a thumbnail/poster, second stream contains the actual animation
    let video_streams: Vec<(usize, &serde_json::Value)> = streams
        .iter()
        .enumerate()
        .filter(|(_, s)| s["codec_type"].as_str() == Some("video"))
        .collect();
    
    if video_streams.is_empty() {
        return Err(FFprobeError::ParseError("No video stream found".to_string()));
    }
    
    // Select stream with most frames (for animated images) or first stream (for regular videos)
    // Use the actual stream index from the JSON, not the enumerate index
    let (stream_index, video_stream) = if video_streams.len() > 1 {
        video_streams
            .iter()
            .max_by_key(|(_, s)| {
                s["nb_frames"]
                    .as_str()
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0)
            })
            .map(|(_, s)| {
                let actual_index = s["index"].as_u64().unwrap_or(0) as usize;
                (actual_index, *s)
            })
            .unwrap()
    } else {
        let actual_index = video_streams[0].1["index"].as_u64().unwrap_or(0) as usize;
        (actual_index, video_streams[0].1)
    };

    if duration <= 0.0 {
        if let Some(d) = video_stream["duration"].as_str().and_then(|s| s.parse::<f64>().ok()) {
            duration = d;
        }
    }
    if duration <= 0.0 {
        return Err(FFprobeError::ParseError("Missing duration (both format and video stream reported 0 or invalid duration)".to_string()));
    }

    let video_codec = video_stream["codec_name"]
        .as_str()
        .ok_or_else(|| FFprobeError::ParseError("Missing video codec name".to_string()))?
        .to_string();
    let video_codec_long = video_stream["codec_long_name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let width = video_stream["width"].as_u64().ok_or_else(|| FFprobeError::ParseError("Missing width".to_string()))? as u32;
    let height = video_stream["height"].as_u64().ok_or_else(|| FFprobeError::ParseError("Missing height".to_string()))? as u32;
    if width == 0 || height == 0 {
        return Err(FFprobeError::ParseError(format!("Invalid dimensions: {}x{}", width, height)));
    }

    let frame_rate = parse_frame_rate(video_stream["r_frame_rate"].as_str().unwrap_or("0/1"))
        .map_err(|e| FFprobeError::ParseError(format!("Invalid r_frame_rate: {}", e)))?;
    let avg_frame_rate = parse_frame_rate(video_stream["avg_frame_rate"].as_str().unwrap_or("0/1"))
        .map_err(|e| FFprobeError::ParseError(format!("Invalid avg_frame_rate: {}", e)))?;

    // Enhanced VFR detection with slow-motion handling
    let is_variable_frame_rate = detect_vfr_enhanced(
        video_stream,
        frame_rate,
        avg_frame_rate,
        &format_name,
    );

    let frame_count = video_stream["nb_frames"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or((duration * frame_rate) as u64);

    let pix_fmt = video_stream["pix_fmt"]
        .as_str()
        .ok_or_else(|| FFprobeError::ParseError("Missing pixel format".to_string()))?
        .to_string();
    let color_space = video_stream["color_space"].as_str().and_then(|s| {
        if s.is_empty() || s == "unknown" { None } else { Some(s.to_string()) }
    });
    let color_transfer = video_stream["color_transfer"].as_str().and_then(|s| {
        if s.is_empty() || s == "unknown" { None } else { Some(s.to_string()) }
    });
    let color_primaries = video_stream["color_primaries"].as_str().and_then(|s| {
        if s.is_empty() || s == "unknown" { None } else { Some(s.to_string()) }
    });

    // Parse HDR side data: Dolby Vision, HDR10+, mastering display, CLL
    // We scan all objects across streams and frames for side_data entries
    let hdr = extract_hdr_side_data(&json);

    let bit_depth = detect_bit_depth(&pix_fmt);

    let profile = video_stream["profile"].as_str().map(|s| s.to_string());
    let level = video_stream["level"]
        .as_u64()
        .map(|l| format!("{:.1}", l as f64 / 10.0));

    // Extract actual B-frame count (integer) instead of just a boolean
    let max_b_frames = video_stream["has_b_frames"].as_i64().unwrap_or(0) as u8;
    let has_b_frames = max_b_frames > 0;

    // Extract encoder settings from tags (x264-params, x265-params, etc.)
    let encoder_settings = video_stream["tags"]["x265-params"]
        .as_str()
        .or_else(|| video_stream["tags"]["x264-params"].as_str())
        .or_else(|| video_stream["tags"]["encoder_settings"].as_str())
        .map(|s| s.to_string());

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

    let subtitle_stream = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("subtitle"));
    let has_subtitles = subtitle_stream.is_some();
    let subtitle_codec = subtitle_stream
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
        color_primaries,
        bit_depth,
        has_audio,
        audio_codec,
        audio_bit_rate,
        audio_sample_rate,
        audio_channels,
        profile,
        level,
        max_b_frames,
        has_b_frames,
        encoder_settings,
        video_bit_rate,
        refs,
        mastering_display: hdr.mastering_display,
        max_cll: hdr.max_cll,
        is_dolby_vision: hdr.is_dolby_vision,
        dv_profile: hdr.dv_profile,
        dv_bl_signal_compatibility_id: hdr.dv_bl_signal_compatibility_id,
        is_hdr10_plus: hdr.is_hdr10_plus,
        has_subtitles,
        subtitle_codec,
        is_variable_frame_rate,
        stream_index,
        tags,
    })
}

/// Recursively scan all `side_data` arrays in a ffprobe JSON value to detect:
/// - Dolby Vision RPU (side_data_type contains "Dolby Vision")
/// - HDR10+ dynamic metadata (SMPTE ST 2094-40)
/// - Mastering display colour volume (HDR10 static metadata)
/// - Content light level (MaxCLL / MaxFALL)
///
/// Parsed HDR side data from ffprobe JSON.
struct HdrSideData {
    is_dolby_vision: bool,
    is_hdr10_plus: bool,
    mastering_display: Option<String>,
    max_cll: Option<String>,
    dv_profile: Option<u8>,
    dv_bl_signal_compatibility_id: Option<u8>,
}

/// Returns parsed HDR side data including DV profile information.
fn extract_hdr_side_data(json: &serde_json::Value) -> HdrSideData {
    let mut result = HdrSideData {
        is_dolby_vision: false,
        is_hdr10_plus: false,
        mastering_display: None,
        max_cll: None,
        dv_profile: None,
        dv_bl_signal_compatibility_id: None,
    };

    // Collect all side_data arrays from streams and frames
    let mut side_data_entries: Vec<&serde_json::Value> = Vec::new();

    // From streams
    if let Some(streams) = json["streams"].as_array() {
        for stream in streams {
            if let Some(sda) = stream["side_data_list"].as_array() {
                side_data_entries.extend(sda.iter());
            }
        }
    }

    // From frames (we requested %+#5 — first 5 frames)
    if let Some(frames) = json["frames"].as_array() {
        for frame in frames {
            if let Some(sda) = frame["side_data_list"].as_array() {
                side_data_entries.extend(sda.iter());
            }
        }
    }

    for sd in &side_data_entries {
        let sd_type = sd["side_data_type"].as_str().unwrap_or("").to_lowercase();

        if sd_type.contains("dolby vision") || sd_type.contains("dovi") {
            result.is_dolby_vision = true;

            // Parse DOVI configuration record fields
            if let Some(profile) = sd["dv_profile"].as_u64() {
                result.dv_profile = Some(profile as u8);
            }
            if let Some(compat_id) = sd["dv_bl_signal_compatibility_id"].as_u64() {
                result.dv_bl_signal_compatibility_id = Some(compat_id as u8);
            }
        }

        if sd_type.contains("hdr dynamic") || sd_type.contains("st2094") || sd_type.contains("hdr10+") {
            result.is_hdr10_plus = true;
        }

        // Mastering display: parse colour primaries + luminance into ffmpeg format
        if sd_type.contains("mastering display") {
            if let Some(md_str) = build_mastering_display_string(sd) {
                result.mastering_display = Some(md_str);
            }
        }

        // Content light level
        if sd_type.contains("content light level") {
            if let Some(cll_str) = build_max_cll_string(sd) {
                result.max_cll = Some(cll_str);
            }
        }
    }

    result
}

/// Convert a rational string like "13250/50000" to a u64 numerator (for ffmpeg master-display format).
/// ffmpeg expects values multiplied by 50000 for chromaticity coordinates.
fn parse_rational_to_50k(s: &str) -> Option<u64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if d == 0.0 { return None; }
        // Normalise to denominator 50000
        Some(((n / d) * 50000.0).round() as u64)
    } else {
        // plain float
        let v: f64 = s.trim().parse().ok()?;
        // Already normalised value (some ffprobe versions give 0.265 style)
        if v <= 1.0 {
            Some((v * 50000.0).round() as u64)
        } else {
            // raw integer-style already in 50k units
            Some(v.round() as u64)
        }
    }
}

/// Convert a rational luminance string to 10000-unit integer (cd/m² × 10000).
fn parse_luminance_to_10k(s: &str) -> Option<u64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if d == 0.0 { return None; }
        Some(((n / d) * 10000.0).round() as u64)
    } else {
        let v: f64 = s.trim().parse().ok()?;
        if v <= 10000.0 {
            Some((v * 10000.0).round() as u64)
        } else {
            Some(v.round() as u64)
        }
    }
}

/// Build the ffmpeg `-master_display` string from a mastering_display side_data object.
/// Format: "G(gx,gy)B(bx,by)R(rx,ry)WP(wx,wy)L(lmax,lmin)"
fn build_mastering_display_string(sd: &serde_json::Value) -> Option<String> {
    let get_coord = |field: &str| -> Option<u64> {
        sd[field].as_str().and_then(parse_rational_to_50k)
            .or_else(|| sd[field].as_f64().map(|v| (v * 50000.0).round() as u64))
    };
    let get_lum = |field: &str| -> Option<u64> {
        sd[field].as_str().and_then(parse_luminance_to_10k)
            .or_else(|| sd[field].as_f64().map(|v| (v * 10000.0).round() as u64))
    };

    let gx = get_coord("green_x")?;
    let gy = get_coord("green_y")?;
    let bx = get_coord("blue_x")?;
    let by_ = get_coord("blue_y")?;
    let rx = get_coord("red_x")?;
    let ry = get_coord("red_y")?;
    let wx = get_coord("white_point_x")?;
    let wy = get_coord("white_point_y")?;
    let lmax = get_lum("max_luminance")?;
    let lmin = get_lum("min_luminance")?;

    Some(format!(
        "G({gx},{gy})B({bx},{by_})R({rx},{ry})WP({wx},{wy})L({lmax},{lmin})"
    ))
}

/// Build the ffmpeg `-cll` string: "MaxCLL,MaxFALL"
fn build_max_cll_string(sd: &serde_json::Value) -> Option<String> {
    let max_content = sd["max_content"]
        .as_u64()
        .or_else(|| sd["MaxCLL"].as_u64())?;
    let max_average = sd["max_average"]
        .as_u64()
        .or_else(|| sd["MaxFALL"].as_u64())?;
    Some(format!("{},{}", max_content, max_average))
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
        ])
        .arg(crate::safe_path_arg(path).as_ref())
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
        ])
        .arg(crate::safe_path_arg(path).as_ref())
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

pub fn parse_frame_rate(s: &str) -> Result<f64, FFprobeError> {
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let num = parts[0].parse::<f64>().map_err(|e| FFprobeError::ParseError(format!("Invalid numerator: {}", e)))?;
            let den = parts[1].parse::<f64>().map_err(|e| FFprobeError::ParseError(format!("Invalid denominator: {}", e)))?;
            if den > 0.0 {
                let rate = num / den;
                if rate > 0.0 {
                    return Ok(rate);
                }
            }
        }
    }
    match s.parse::<f64>() {
        Ok(v) if v > 0.0 => Ok(v),
        _ => Err(FFprobeError::ParseError(format!("Could not parse frame rate: '{}'", s))),
    }
}

pub fn detect_bit_depth(pix_fmt: &str) -> u8 {
    if pix_fmt.contains("16le")
        || pix_fmt.contains("16be")
        || pix_fmt.contains("48le")
        || pix_fmt.contains("48be")
        || pix_fmt.contains("64le")
        || pix_fmt.contains("64be")
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
            let result = parse_frame_rate(input).unwrap();
            assert!(
                (result - expected).abs() < *tolerance,
                "parse_frame_rate({:?}): expected {}, got {}",
                input,
                expected,
                result
            );
        }
    }

    #[test]
    fn test_parse_frame_rate_edge_cases() {
        assert!(parse_frame_rate("30/0").is_err());
        assert!(parse_frame_rate("invalid").is_err());
        assert!(parse_frame_rate("").is_err());
        assert!(parse_frame_rate("30/1/extra").is_err());
    }

    #[test]
    fn test_detect_bit_depth() {
        let cases: &[(&str, u8)] = &[
            ("yuv420p", 8),
            ("yuv422p", 8),
            ("yuv444p", 8),
            ("rgb24", 8),
            ("bgr24", 8),
            ("nv12", 8),
            ("yuvj420p", 8),
            ("yuv420p10le", 10),
            ("yuv420p10be", 10),
            ("yuv422p10le", 10),
            ("yuv444p10le", 10),
            ("p010le", 10),
            ("p010", 10),
            ("yuv420p12le", 12),
            ("yuv420p12be", 12),
            ("yuv422p12le", 12),
            ("yuv444p12le", 12),
            ("yuv420p16le", 16),
            ("yuv420p16be", 16),
            ("rgb48le", 16),
            ("unknown", 8),
            ("", 8),
            ("custom_format", 8),
        ];

        for (fmt, expected) in cases {
            assert_eq!(
                detect_bit_depth(fmt),
                *expected,
                "detect_bit_depth({:?}) mismatch",
                fmt
            );
        }
    }
}
