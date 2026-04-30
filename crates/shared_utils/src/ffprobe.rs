//! `FFprobe` wrapper module
//!
//! Shared `FFprobe` functionality for video analysis.
//! Used by the `vid` pipeline.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use tracing::{warn, debug};

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
            Self::ToolNotFound(s) => write!(f, "Tool not found: {s}"),
            Self::ExecutionFailed(s) => write!(f, "FFprobe failed: {s}"),
            Self::ParseError(s) => write!(f, "Parse error: {s}"),
            Self::IoError(e) => write!(f, "IO error: {e}"),
        }
    }
}

impl std::error::Error for FFprobeError {}

impl From<io::Error> for FFprobeError {
    fn from(e: io::Error) -> Self {
        Self::IoError(e)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FFprobeAudioInfo {
    pub present: bool,
    pub codec: Option<String>,
    pub bit_rate: Option<u64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FFprobeSubtitleInfo {
    pub present: bool,
    pub codec: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DolbyVisionMetadata {
    pub profile: Option<u8>,
    pub bl_signal_compatibility_id: Option<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FFprobeHdrInfo {
    /// HDR10 mastering display metadata (e.g. "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,500)")
    pub mastering_display: Option<String>,
    /// HDR10 content light level metadata (e.g. "MaxCLL=1000,MaxFALL=400")
    pub max_cll: Option<String>,
    /// Dolby Vision metadata when detected in side data
    pub dolby_vision: Option<DolbyVisionMetadata>,
    /// True when content uses HDR10+ dynamic metadata (SMPTE ST 2094-40)
    pub hdr10_plus: bool,
}

impl FFprobeHdrInfo {
    #[must_use]
    pub const fn is_dolby_vision(&self) -> bool {
        self.dolby_vision.is_some()
    }

    #[must_use]
    pub fn dv_profile(&self) -> Option<u8> {
        self.dolby_vision
            .and_then(|dolby_vision| dolby_vision.profile)
    }

    #[must_use]
    pub fn dv_bl_signal_compatibility_id(&self) -> Option<u8> {
        self.dolby_vision
            .and_then(|dolby_vision| dolby_vision.bl_signal_compatibility_id)
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
    pub avg_frame_rate: f64,
    pub frame_count: u64,
    pub pix_fmt: String,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub bit_depth: u8,
    pub audio: FFprobeAudioInfo,
    pub profile: Option<String>,
    pub level: Option<String>,
    /// Actual B-frame count (`max_b_frames`) from ffprobe.
    pub max_b_frames: u8,
    /// Raw encoder settings string (e.g. from x264-params or x265-params tags).
    pub encoder_settings: Option<String>,
    pub video_bit_rate: Option<u64>,
    pub refs: Option<u32>,
    pub hdr: FFprobeHdrInfo,
    pub subtitles: FFprobeSubtitleInfo,
    /// Variable frame rate detected (`r_frame_rate` != `avg_frame_rate`)
    pub is_variable_frame_rate: bool,
    /// Stream index of the selected video stream (for multi-stream files like animated AVIF)
    pub stream_index: usize,
    /// Format tags (e.g. encoder, `creation_time`) from the format section
    pub tags: HashMap<String, String>,
    /// Optional: Loop count from metadata (0 = infinite)
    pub loop_count: Option<u16>,
    /// Frame types (I, P, B) for the initial sample.
    pub frame_types: Vec<char>,
    /// PTS deltas (frame intervals) for the initial sample.
    pub pts_deltas: Vec<f64>,
    /// Motion vector magnitudes (if available).
    pub mv_magnitudes: Vec<f64>,
    /// Captured packet sizes for bitrate inequality analysis.
    pub pkt_sizes: Vec<u64>,
}

impl FFprobeResult {
    #[must_use]
    pub const fn has_b_frames(&self) -> bool {
        self.max_b_frames > 0
    }
}

#[must_use]
pub fn is_ffprobe_available() -> bool {
    crate::ffmpeg_builder::FfprobeBuilder::check_available()
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


#[derive(Debug)]
struct ProbeFormatInfo {
    format_name: String,
    size: u64,
    bit_rate: u64,
    duration: f64,
    tags: HashMap<String, String>,
}

#[derive(Debug)]
struct VideoStreamFields {
    video_codec: String,
    video_codec_long: String,
    width: u32,
    height: u32,
    frame_rate: f64,
    avg_frame_rate: f64,
    frame_count: u64,
    pix_fmt: String,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    bit_depth: u8,
    profile: Option<String>,
    level: Option<String>,
    max_b_frames: u8,
    encoder_settings: Option<String>,
    video_bit_rate: Option<u64>,
    refs: Option<u32>,
    is_variable_frame_rate: bool,
}

fn validate_probe_target(path: &Path) -> Result<(), FFprobeError> {
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

    Ok(())
}

fn run_ffprobe_json(path: &Path) -> Result<serde_json::Value, FFprobeError> {
    let output = crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(path)
        .loglevel("error")
        .print_format("json")
        .show_format()
        .show_streams()
        .show_frames()
        .show_entries("frame=pict_type,pkt_pts_time,pkt_size")
        .read_intervals("%+#300")
        .build()
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
    serde_json::from_str(&json_str).map_err(|e| FFprobeError::ParseError(e.to_string()))
}

fn parse_u64_string_field(value: &serde_json::Value) -> Option<u64> {
    value.as_str().and_then(|s| s.parse::<u64>().ok())
}

fn parse_f64_string_field(value: &serde_json::Value) -> Option<f64> {
    value.as_str().and_then(|s| s.parse::<f64>().ok())
}

fn parse_optional_known_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(str::to_string)
}

fn collect_string_tags(tags_value: &serde_json::Value) -> HashMap<String, String> {
    let mut tags = HashMap::new();
    if let Some(tags_obj) = tags_value.as_object() {
        for (key, value) in tags_obj {
            if let Some(string_value) = value.as_str() {
                tags.insert(key.clone(), string_value.to_string());
            }
        }
    }
    tags
}

fn parse_probe_format(format: &serde_json::Value) -> Result<ProbeFormatInfo, FFprobeError> {
    let format_name = format["format_name"]
        .as_str()
        .ok_or_else(|| FFprobeError::ParseError("Missing format_name".to_string()))?
        .to_string();
    let size = parse_u64_string_field(&format["size"])
        .ok_or_else(|| FFprobeError::ParseError("Missing or invalid file size".to_string()))?;

    Ok(ProbeFormatInfo {
        format_name,
        size,
        bit_rate: parse_u64_string_field(&format["bit_rate"]).unwrap_or(0),
        duration: parse_f64_string_field(&format["duration"]).unwrap_or(0.0),
        tags: collect_string_tags(&format["tags"]),
    })
}

fn select_video_stream<'a>(
    streams: &'a [serde_json::Value],
) -> Result<(usize, &'a serde_json::Value), FFprobeError> {
    let video_streams: Vec<(usize, &'a serde_json::Value)> = streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| stream["codec_type"].as_str() == Some("video"))
        .collect();

    if video_streams.is_empty() {
        return Err(FFprobeError::ParseError(
            "No video stream found".to_string(),
        ));
    }

    let (fallback_index, stream) = if video_streams.len() > 1 {
        video_streams
            .into_iter()
            .max_by_key(|(_, stream)| parse_u64_string_field(&stream["nb_frames"]).unwrap_or(0))
            .ok_or_else(|| FFprobeError::ParseError("No video stream found".to_string()))?
    } else {
        video_streams[0]
    };

    let actual_index = stream["index"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .unwrap_or(fallback_index);

    Ok((actual_index, stream))
}

fn resolve_probe_duration(
    format_duration: f64,
    video_stream: &serde_json::Value,
) -> Result<f64, FFprobeError> {
    let duration = if format_duration > 0.0 {
        format_duration
    } else {
        parse_f64_string_field(&video_stream["duration"]).unwrap_or(0.0)
    };

    // Allow 0.0 duration for formats like headless GIFs where duration is not globally specified
    Ok(duration)
}

/// Fallback for dimension parsing using `ImageMagick`'s identify tool.
fn get_dimensions_via_identify(path: &Path) -> Option<(u32, u32)> {
    if !crate::image_builders::IdentifyBuilder::check_available() {
        return None;
    }

    let output = crate::image_builders::IdentifyBuilder::new()
        .input(path)
        .format("%w %h")
        .build()
        .output()
        .ok()?;

    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout);
        // identify might output multiple lines for animations; take the first frame's dimensions.
        let first_line = s.lines().next()?;
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() >= 2 {
            let w = parts[0].parse().ok()?;
            let h = parts[1].parse().ok()?;
            return Some((w, h));
        }
    }
    None
}

fn parse_required_u32_field(
    video_stream: &serde_json::Value,
    field_name: &str,
) -> Result<u32, FFprobeError> {
    let raw_value = video_stream[field_name]
        .as_u64()
        .ok_or_else(|| FFprobeError::ParseError(format!("Missing {field_name}")))?;

    u32::try_from(raw_value)
        .map_err(|_| FFprobeError::ParseError(format!("Invalid {field_name}: {raw_value}")))
}

fn parse_video_stream_fields(
    video_stream: &serde_json::Value,
    format_name: &str,
    duration: f64,
    path: &Path,
) -> Result<VideoStreamFields, FFprobeError> {
    let video_codec = video_stream["codec_name"]
        .as_str()
        .ok_or_else(|| FFprobeError::ParseError("Missing video codec name".to_string()))?
        .to_string();
    let video_codec_long = video_stream["codec_long_name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut width = parse_required_u32_field(video_stream, "width")?;
    let mut height = parse_required_u32_field(video_stream, "height")?;
    if width == 0 || height == 0 {
        // 🛡️ WebP Fallback: Some WebP files (especially animated ones) return 0x0 from ffprobe metadata.
        // Use ImageMagick's identify as a trusted fallback.
        if format_name.contains("webp") {
            if let Some((w, h)) = get_dimensions_via_identify(path) {
                debug!(path = %path.display(), "ffprobe reported 0x0, fell back to identify: {w}x{h}");
                width = w;
                height = h;
            }
        }

        if width == 0 || height == 0 {
            return Err(FFprobeError::ParseError(format!(
                "Invalid dimensions: {width}x{height}"
            )));
        }
    }

    let frame_rate = parse_frame_rate(video_stream["r_frame_rate"].as_str().unwrap_or("0/1"))
        .map_err(|e| FFprobeError::ParseError(format!("Invalid r_frame_rate: {e}")))?;
    let avg_frame_rate = parse_frame_rate(video_stream["avg_frame_rate"].as_str().unwrap_or("0/1"))
        .map_err(|e| FFprobeError::ParseError(format!("Invalid avg_frame_rate: {e}")))?;
    let is_variable_frame_rate =
        detect_vfr_enhanced(video_stream, frame_rate, avg_frame_rate, format_name);
    let frame_count = parse_u64_string_field(&video_stream["nb_frames"])
        .unwrap_or_else(|| crate::numeric_cast::f64_to_u64_sat(duration * frame_rate));

    let pix_fmt = video_stream["pix_fmt"]
        .as_str()
        .ok_or_else(|| FFprobeError::ParseError("Missing pixel format".to_string()))?
        .to_string();
    let max_b_frames =
        u8::try_from(video_stream["has_b_frames"].as_i64().unwrap_or(0)).unwrap_or(0);

    Ok(VideoStreamFields {
        video_codec,
        video_codec_long,
        width,
        height,
        frame_rate,
        avg_frame_rate,
        frame_count,
        pix_fmt: pix_fmt.clone(),
        color_space: parse_optional_known_string(&video_stream["color_space"]),
        color_transfer: parse_optional_known_string(&video_stream["color_transfer"]),
        color_primaries: parse_optional_known_string(&video_stream["color_primaries"]),
        bit_depth: detect_bit_depth(&pix_fmt),
        profile: video_stream["profile"].as_str().map(str::to_string),
        level: video_stream["level"]
            .as_u64()
            .map(|level| format!("{:.1}", level as f64 / 10.0)),
        max_b_frames,
        encoder_settings: video_stream["tags"]["x265-params"]
            .as_str()
            .or_else(|| video_stream["tags"]["x264-params"].as_str())
            .or_else(|| video_stream["tags"]["encoder_settings"].as_str())
            .map(str::to_string),
        video_bit_rate: parse_u64_string_field(&video_stream["bit_rate"]),
        refs: video_stream["refs"]
            .as_u64()
            .and_then(|refs| u32::try_from(refs).ok()),
        is_variable_frame_rate,
    })
}

fn extract_audio_stream_fields(streams: &[serde_json::Value]) -> FFprobeAudioInfo {
    let Some(audio_stream) = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("audio"))
    else {
        return FFprobeAudioInfo::default();
    };

    FFprobeAudioInfo {
        present: true,
        codec: audio_stream["codec_name"].as_str().map(str::to_string),
        bit_rate: parse_u64_string_field(&audio_stream["bit_rate"]),
        sample_rate: parse_u64_string_field(&audio_stream["sample_rate"])
            .and_then(|sample_rate| u32::try_from(sample_rate).ok()),
        channels: audio_stream["channels"]
            .as_u64()
            .and_then(|channels| u32::try_from(channels).ok()),
    }
}

fn extract_subtitle_stream_fields(streams: &[serde_json::Value]) -> FFprobeSubtitleInfo {
    let Some(subtitle_stream) = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("subtitle"))
    else {
        return FFprobeSubtitleInfo::default();
    };

    FFprobeSubtitleInfo {
        present: true,
        codec: subtitle_stream["codec_name"].as_str().map(str::to_string),
    }
}

/// Probe video file using ffprobe.
///
/// # Errors
/// Returns `FFprobeError` if `ffprobe` is not found, execution fails, or parsing results fails.
///
/// # Panics
/// Panics if no video streams are found.
pub fn probe_video(path: &Path) -> Result<FFprobeResult, FFprobeError> {
    if !is_ffprobe_available() {
        return Err(FFprobeError::ToolNotFound(
            "ffprobe not found. Install with: brew install ffmpeg".to_string(),
        ));
    }

    validate_probe_target(path)?;
    let json = run_ffprobe_json(path)?;
    let ProbeFormatInfo {
        format_name,
        size,
        bit_rate,
        duration: format_duration,
        tags,
    } = parse_probe_format(&json["format"])?;
    let streams = json["streams"]
        .as_array()
        .ok_or_else(|| FFprobeError::ParseError("No streams found".to_string()))?;
    let (stream_index, video_stream) = select_video_stream(streams)?;
    let duration = resolve_probe_duration(format_duration, video_stream)?;
    let video = parse_video_stream_fields(video_stream, &format_name, duration, path)?;
    let hdr = extract_hdr_side_data(&json);
    let audio = extract_audio_stream_fields(streams);
    let subtitles = extract_subtitle_stream_fields(streams);

    let mut result = FFprobeResult {
        format_name,
        duration,
        size,
        bit_rate,
        video_codec: video.video_codec,
        video_codec_long: video.video_codec_long,
        width: video.width,
        height: video.height,
        frame_rate: video.frame_rate,
        avg_frame_rate: video.avg_frame_rate,
        frame_count: video.frame_count,
        pix_fmt: video.pix_fmt,
        color_space: video.color_space,
        color_transfer: video.color_transfer,
        color_primaries: video.color_primaries,
        bit_depth: video.bit_depth,
        audio,
        profile: video.profile,
        level: video.level,
        max_b_frames: video.max_b_frames,
        encoder_settings: video.encoder_settings,
        video_bit_rate: video.video_bit_rate,
        refs: video.refs,
        hdr,
        subtitles,
        is_variable_frame_rate: video.is_variable_frame_rate,
        stream_index,
        tags,
        loop_count: extract_loop_count(&json["format"]),
        frame_types: extract_frame_types(&json),
        pts_deltas: extract_pts_deltas(&json),
        pkt_sizes: extract_pkt_sizes(&json),
        mv_magnitudes: Vec::new(),
    };

    // ── Penetrating Content Verification ──
    // Verify critical metadata by decoding actual content
    if result.audio.present {
        if let crate::media_penetration::PenetrationResult::Verified(is_silent) =
            crate::media_penetration::detect_audio_silence(path)
        {
            if is_silent {
                result.audio.present = false;
            }
        }
    }

    if result.frame_count <= 1 || result.frame_count > 50000 {
        if let crate::media_penetration::PenetrationResult::Verified(real_count) =
            crate::media_penetration::detect_real_frame_count(path, result.frame_count)
        {
            if real_count != result.frame_count {
                result.frame_count = real_count;
            }
        }
    }

    Ok(result)
}

/// Attempt to extract loop count from format tags (e.g. NETSCAPE2.0 or `LoopCount`)
fn extract_loop_count(format: &serde_json::Value) -> Option<u16> {
    if let Some(tags) = format["tags"].as_object() {
        if let Some(val) = tags.get("loop_count").or_else(|| tags.get("loop")) {
            if let Some(s) = val.as_str() {
                return s.parse::<u16>().ok();
            }
        }
    }
    None
}

fn extract_frame_types(json: &serde_json::Value) -> Vec<char> {
    let mut types = Vec::new();
    if let Some(frames) = json["frames"].as_array() {
        for frame in frames {
            if let Some(pict_type) = frame["pict_type"].as_str() {
                if let Some(first_char) = pict_type.chars().next() {
                    types.push(first_char);
                }
            }
        }
    }
    types
}

fn extract_pts_deltas(json: &serde_json::Value) -> Vec<f64> {
    let mut deltas = Vec::new();
    let mut last_pts: Option<f64> = None;
    if let Some(frames) = json["frames"].as_array() {
        for frame in frames {
            if let Some(pts_str) = frame["pkt_pts_time"].as_str() {
                if let Ok(pts) = pts_str.parse::<f64>() {
                    if let Some(last) = last_pts {
                        deltas.push((pts - last).abs());
                    }
                    last_pts = Some(pts);
                }
            }
        }
    }
    deltas
}

fn extract_pkt_sizes(json: &serde_json::Value) -> Vec<u64> {
    let mut sizes = Vec::new();
    if let Some(frames) = json["frames"].as_array() {
        for frame in frames {
            if let Some(size_str) = frame["pkt_size"].as_str() {
                if let Ok(size) = size_str.parse::<u64>() {
                    sizes.push(size);
                }
            }
        }
    }
    sizes
}

/// Recursively scan all `side_data` arrays in a ffprobe JSON value to detect:
/// - Dolby Vision RPU (`side_data_type` contains "Dolby Vision")
/// - HDR10+ dynamic metadata (SMPTE ST 2094-40)
/// - Mastering display colour volume (HDR10 static metadata)
/// - Content light level (`MaxCLL` / `MaxFALL`)
///
/// Returns parsed HDR side data including DV profile information.
fn extract_hdr_side_data(json: &serde_json::Value) -> FFprobeHdrInfo {
    let mut result = FFprobeHdrInfo::default();

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

    // All side data arrays from streams and frames are scanned.
    // If not found, they remain false (Default).

    for sd in &side_data_entries {
        let sd_type = sd["side_data_type"].as_str().unwrap_or("").to_lowercase();

        if sd_type.contains("dolby vision") || sd_type.contains("dovi") {
            let dolby_vision = result.dolby_vision.get_or_insert_default();

            // Parse DOVI configuration record fields
            if let Some(profile) = sd["dv_profile"].as_u64() {
                dolby_vision.profile = Some(u8::try_from(profile).unwrap_or(0));
            }
            if let Some(compat_id) = sd["dv_bl_signal_compatibility_id"].as_u64() {
                dolby_vision.bl_signal_compatibility_id =
                    Some(u8::try_from(compat_id).unwrap_or(0));
            }
        }

        if sd_type.contains("hdr dynamic")
            || sd_type.contains("st2094")
            || sd_type.contains("hdr10+")
        {
            result.hdr10_plus = true;
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
        if d == 0.0 {
            return None;
        }
        // Normalise to denominator 50000
        let val = crate::numeric_cast::f64_to_u64_sat((n / d) * 50000.0);
        Some(val)
    } else {
        // plain float
        let v: f64 = s.trim().parse().ok()?;
        // Already normalised value (some ffprobe versions give 0.265 style)
        if v <= 1.0 {
            let val = crate::numeric_cast::f64_to_u64_sat(v * 50000.0);
            Some(val)
        } else {
            // raw integer-style already in 50k units
            let val = crate::numeric_cast::f64_to_u64_sat(v);
            Some(val)
        }
    }
}

/// Convert a rational luminance string to 10000-unit integer (cd/m² × 10000).
fn parse_luminance_to_10k(s: &str) -> Option<u64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if d == 0.0 {
            return None;
        }
        let val = crate::numeric_cast::f64_to_u64_sat((n / d) * 10000.0);
        Some(val)
    } else {
        let v: f64 = s.trim().parse().ok()?;
        if v <= 10000.0 {
            let val = crate::numeric_cast::f64_to_u64_sat(v * 10000.0);
            Some(val)
        } else {
            let val = crate::numeric_cast::f64_to_u64_sat(v);
            Some(val)
        }
    }
}

/// Build the ffmpeg `-master_display` string from a `mastering_display` `side_data` object.
/// Format: "G(gx,gy)B(bx,by)R(rx,ry)WP(wx,wy)L(lmax,lmin)"
fn build_mastering_display_string(sd: &serde_json::Value) -> Option<String> {
    let get_coord = |field: &str| -> Option<u64> {
        sd[field]
            .as_str()
            .and_then(parse_rational_to_50k)
            .or_else(|| {
                sd[field]
                    .as_f64()
                    .map(|v| crate::numeric_cast::f64_to_u64_sat(v * 50000.0))
            })
    };
    let get_lum = |field: &str| -> Option<u64> {
        sd[field]
            .as_str()
            .and_then(parse_luminance_to_10k)
            .or_else(|| {
                sd[field]
                    .as_f64()
                    .map(|v| crate::numeric_cast::f64_to_u64_sat(v * 10000.0))
            })
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
    Some(format!("{max_content},{max_average}"))
}

pub fn get_duration(path: &Path) -> Option<f64> {
    let output = match crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(path)
        .loglevel("quiet")
        .show_entries("format=duration")
        .print_format("default=noprint_wrappers=1:nokey=1")
        .build()
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "Failed to launch ffprobe duration query"
            );
            return None;
        }
    };

    if !output.status.success() {
        warn!(
            path = %path.display(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "ffprobe duration query failed"
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    match trimmed.parse::<f64>() {
        Ok(duration) => Some(duration),
        Err(err) => {
            warn!(
                path = %path.display(),
                output = %trimmed,
                error = %err,
                "Failed to parse ffprobe duration output"
            );
            None
        }
    }
}

pub fn get_frame_count(path: &Path) -> Option<u64> {
    let output = match crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(path)
        .loglevel("quiet")
        .count_frames()
        .select_stream(crate::ffmpeg_builder::StreamType::Video, 0)
        .show_entries("stream=nb_read_frames")
        .print_format("default=noprint_wrappers=1:nokey=1")
        .build()
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "Failed to launch ffprobe frame-count query"
            );
            return None;
        }
    };

    if !output.status.success() {
        warn!(
            path = %path.display(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "ffprobe frame-count query failed"
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    match trimmed.parse::<u64>() {
        Ok(frame_count) => Some(frame_count),
        Err(err) => {
            warn!(
                path = %path.display(),
                output = %trimmed,
                error = %err,
                "Failed to parse ffprobe frame-count output"
            );
            None
        }
    }
}

/// Parse frame rate string (e.g. "30/1" or "29.97").
///
/// # Errors
/// Returns `FFprobeError` if parsing fails.
pub fn parse_frame_rate(s: &str) -> Result<f64, FFprobeError> {
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let num = parts[0]
                .parse::<f64>()
                .map_err(|e| FFprobeError::ParseError(format!("Invalid numerator: {e}")))?;
            let den = parts[1]
                .parse::<f64>()
                .map_err(|e| FFprobeError::ParseError(format!("Invalid denominator: {e}")))?;

            if den == 0.0 {
                return Err(FFprobeError::ParseError(
                    "Frame rate denominator cannot be zero".to_string(),
                ));
            }

            let rate = num / den;
            if rate >= 0.0 {
                return Ok(rate);
            }
        }
    }
    match s.parse::<f64>() {
        Ok(v) if v > 0.0 => Ok(v),
        _ => Err(FFprobeError::ParseError(format!(
            "Could not parse frame rate: '{s}'"
        ))),
    }
}

#[must_use]
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
                "parse_frame_rate({input:?}): expected {expected}, got {result}"
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
        ];

        for (input, expected) in cases {
            assert_eq!(
                detect_bit_depth(input),
                *expected,
                "detect_bit_depth({input:?})"
            );
        }
    }

    #[test]
    fn test_extract_frame_types() {
        let json = serde_json::json!({
            "frames": [
                {"pict_type": "I"},
                {"pict_type": "P"},
                {"pict_type": "B"},
                {"pict_type": "I"}
            ]
        });
        let types = extract_frame_types(&json);
        assert_eq!(types, vec!['I', 'P', 'B', 'I']);
    }

    #[test]
    fn test_extract_pts_deltas() {
        let json = serde_json::json!({
            "frames": [
                {"pkt_pts_time": "0.0"},
                {"pkt_pts_time": "0.04"},
                {"pkt_pts_time": "0.08"},
                {"pkt_pts_time": "0.12"}
            ]
        });
        let deltas = extract_pts_deltas(&json);
        assert_eq!(deltas.len(), 3);
        for delta in &deltas {
            assert!((delta - 0.04).abs() < 0.001);
        }
    }

    #[test]
    fn test_extract_pkt_sizes() {
        let json = serde_json::json!({
            "frames": [
                {"pkt_size": "1024"},
                {"pkt_size": "2048"},
                {"pkt_size": "512"}
            ]
        });
        let sizes = extract_pkt_sizes(&json);
        assert_eq!(sizes, vec![1024, 2048, 512]);
    }

    #[test]
    fn test_extract_loop_count() {
        let json_with_loop = serde_json::json!({
            "format": {
                "tags": {
                    "loop_count": "5"
                }
            }
        });
        let loop_count = extract_loop_count(&json_with_loop["format"]);
        assert_eq!(loop_count, Some(5));

        let json_no_loop = serde_json::json!({
            "format": {
                "tags": {}
            }
        });
        let loop_count = extract_loop_count(&json_no_loop["format"]);
        assert_eq!(loop_count, None);
    }
}
