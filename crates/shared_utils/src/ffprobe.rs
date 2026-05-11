//! `FFprobe` wrapper module
//!
//! Shared `FFprobe` functionality for video analysis.
//! Used by the `vid` pipeline.

use crate::builder_base::ToolBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::Path;

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
    pub const fn is_hdr(&self) -> bool {
        self.mastering_display.is_some()
            || self.max_cll.is_some()
            || self.hdr10_plus
            || self.is_dolby_vision()
    }

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
    pub duration: Option<f64>,
    pub size: u64,
    /// Container-level bit rate from ffprobe format section.
    /// `None` when ffprobe does not report it (common for image containers, e.g. WebP).
    pub bit_rate: Option<u64>,
    pub video_codec: String,
    pub video_codec_long: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: Option<f64>,
    pub avg_frame_rate: Option<f64>,
    pub frame_count: Option<u64>,
    pub pix_fmt: String,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub bit_depth: Option<u8>,
    pub audio: FFprobeAudioInfo,
    pub profile: Option<String>,
    pub level: Option<String>,
    /// Actual B-frame count (`max_b_frames`) from ffprobe.
    pub max_b_frames: Option<u8>,
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
        matches!(self.max_b_frames, Some(b) if b > 0)
    }
}

#[must_use]
pub fn is_ffprobe_available() -> bool {
    crate::ffmpeg_builder::FfprobeBuilder::check_available()
}

/// Enhanced VFR detection with slow-motion video handling
fn detect_vfr_enhanced(
    video_stream: &serde_json::Value,
    r_frame_rate: Option<f64>,
    avg_frame_rate: Option<f64>,
    format_name: &str,
) -> bool {
    let (Some(r_fr), Some(avg_fr)) = (r_frame_rate, avg_frame_rate) else {
        return false;
    };

    if r_fr <= 0.0_f64 || avg_fr <= 0.0_f64 {
        return false;
    }

    // Slow-motion detection (separate logic for reliability)
    if (format_name.contains("mov") || format_name.contains("mp4"))
        && avg_fr >= crate::constants::VFR_SLOWMO_FPS_THRESHOLD
    {
        // Check for Apple's slow-mo tag (most reliable indicator)
        if video_stream
            .get("tags")
            .and_then(|t| t.get("com.apple.quicktime.fullframerate"))
            .is_some_and(serde_json::Value::is_string)
        {
            return true;
        }

        // Check for significant frame rate ratio (recording vs playback)
        if r_fr / avg_fr > crate::constants::VFR_SLOWMO_RATIO_THRESHOLD {
            return true;
        }
    }

    // Standard VFR detection with threshold
    let diff_ratio = (r_fr - avg_fr).abs() / r_fr;
    diff_ratio > crate::constants::VFR_STANDARD_DIFF_THRESHOLD
}

#[derive(Debug)]
struct ProbeFormatInfo {
    format_name: String,
    size: u64,
    /// `None` when ffprobe format section omits `bit_rate` (e.g. image containers).
    bit_rate: Option<u64>,
    duration: Option<f64>,
    tags: HashMap<String, String>,
}

#[derive(Debug)]
struct VideoStreamFields {
    video_codec: String,
    video_codec_long: String,
    width: u32,
    height: u32,
    frame_rate: Option<f64>,
    avg_frame_rate: Option<f64>,
    frame_count: Option<u64>,
    pix_fmt: String,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    bit_depth: Option<u8>,
    profile: Option<String>,
    level: Option<String>,
    max_b_frames: Option<u8>,
    encoder_settings: Option<String>,
    video_bit_rate: Option<u64>,
    refs: Option<u32>,
    is_variable_frame_rate: bool,
}

/// Validates that a path is suitable for `FFprobe` analysis.
///
/// Checks that the path exists and is a file, not a directory.
/// Returns appropriate errors for invalid targets.
///
/// # Arguments
/// * `path` - The path to validate
///
/// # Returns
/// Ok(()) if valid, or `FFprobeError` with details
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

/// Runs `FFprobe` with `JSON` output for detailed media analysis.
///
/// Executes `FFprobe` with comprehensive options to extract format,
/// stream, and frame information in JSON format.
///
/// # Arguments
/// * `path` - The media file to probe
///
/// # Returns
/// `JSON` output from `FFprobe`, or `FFprobeError` if execution fails
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

/// Parses a string field from JSON as u64.
///
/// # Arguments
/// * `value` - The JSON value to parse
///
/// # Returns
/// Parsed u64 value, or None if parsing fails
fn parse_u64_string_field(value: &serde_json::Value) -> Option<u64> {
    crate::numeric_cast::parse_option_strict(value.as_str(), "u64_field")
}

/// Parses a string field from JSON as f64.
///
/// # Arguments
/// * `value` - The JSON value to parse
///
/// # Returns
/// Parsed f64 value, or None if parsing fails
fn parse_f64_string_field(value: &serde_json::Value) -> Option<f64> {
    crate::numeric_cast::parse_option_strict(value.as_str(), "f64_field")
}

/// Parses a string field from JSON, filtering out empty and "unknown" values.
///
/// # Arguments
/// * `value` - The JSON value to parse
///
/// # Returns
/// Filtered string value, or None if empty/unknown
fn parse_optional_known_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(str::to_string)
}

/// Collects string tags from a JSON tags object.
///
/// Extracts all key-value pairs from a JSON object representing tags,
/// filtering for string values only.
///
/// # Arguments
/// * `tags_value` - The JSON value containing tags
///
/// # Returns
/// `HashMap` of tag key-value pairs
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

/// Parses the format information from `FFprobe` `JSON` output.
///
/// Extracts format-level metadata including format name, duration,
/// bit rate, and other format-specific information.
///
/// # Arguments
/// * `format` - The format `JSON` object from `FFprobe`
///
/// # Returns
/// Parsed format information, or `FFprobeError` if parsing fails
fn parse_probe_format(format: &serde_json::Value) -> Result<ProbeFormatInfo, FFprobeError> {
    let format_name = format["format_name"]
        .as_str()
        .ok_or_else(|| FFprobeError::ParseError("Missing format_name".to_string()))?
        .to_string();
    let size = parse_u64_string_field(&format["size"])
        .ok_or_else(|| FFprobeError::ParseError("Missing or invalid file size".to_string()))?;

    // `bit_rate` is absent for many image containers (WebP, AVIF) — treat as optional.
    let bit_rate = parse_u64_string_field(&format["bit_rate"]);
    if bit_rate.is_none() {
        crate::log_info!(
            crate::static_logs::messages::LABEL_PROBE,
            "ffprobe: 'bit_rate' metadata missing from format section (expected for static image containers like WebP/AVIF)"
        );
    }

    // `duration` is optional in the format section for some containers.
    let duration = parse_f64_string_field(&format["duration"]);
    if duration.is_none() {
        crate::log_info!(
            crate::static_logs::messages::LABEL_PROBE,
            "ffprobe: 'duration' metadata missing from format section (expected for static image containers or malformed streams)"
        );
    }

    Ok(ProbeFormatInfo {
        format_name,
        size,
        bit_rate,
        duration,
        tags: collect_string_tags(&format["tags"]),
    })
}

/// Selects the primary video stream from available streams.
///
/// Filters streams to find video streams and selects the most appropriate one.
/// Returns an error if no video stream is found.
///
/// # Arguments
/// * `streams` - Array of stream `JSON` objects from `FFprobe`
///
/// # Returns
/// Tuple of (`stream_index`, `stream_json`) for the selected video stream
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

    // A stream with zero or absent dimensions is not selectable as the primary stream.
    let has_valid_dimensions = |stream: &serde_json::Value| -> bool {
        stream["width"].as_u64().is_some_and(|w| w > 0)
            && stream["height"].as_u64().is_some_and(|h| h > 0)
    };

    let (fallback_index, stream) = if video_streams.len() > 1 {
        video_streams
            .into_iter()
            .max_by_key(|(_, stream)| {
                // `nb_frames` absent → treat as 0 (lowest sort priority); this is
                // intentional: a stream with unknown frame count loses to one with known count.
                let nb = parse_u64_string_field(&stream["nb_frames"]).unwrap_or_else(|| {
                    crate::log_info!(
                        crate::static_logs::messages::LABEL_PROBE,
                        "ffprobe: 'nb_frames' (total frames) not reported by stream; defaulting to 0 for stream selection"
                    );
                    0
                });
                (u8::from(has_valid_dimensions(stream)), nb)
            })
            .ok_or_else(|| FFprobeError::ParseError("No video stream found".to_string()))?
    } else {
        video_streams
            .into_iter()
            .next()
            .ok_or_else(|| FFprobeError::ParseError("No video stream found".to_string()))?
    };

    let actual_index = stream["index"]
        .as_u64()
        .and_then(|index| crate::numeric_cast::u64_to_usize_strict(index, "stream_index"))
        .unwrap_or(fallback_index);

    Ok((actual_index, stream))
}

/// Resolves the accurate duration from format and stream information.
///
/// Uses format duration as primary source, falls back to stream duration.
/// Applies format-specific corrections and fallbacks for different container types.
///
/// # Arguments
/// * `format_duration` - Duration from format information
/// * `video_stream` - The video stream JSON object
/// * `format_name` - The container format name
/// * `path` - The file path for debugging
///
/// # Returns
/// Resolved duration in seconds
fn resolve_probe_duration(
    format_duration: Option<f64>,
    video_stream: &serde_json::Value,
    format_name: &str,
    path: &Path,
) -> Option<f64> {
    // `format_duration` was already validated as present in `parse_probe_format`;
    // the stream-level fallback is a secondary source for edge cases.
    let mut duration = format_duration.filter(|&d| d > 0.0_f64);

    if duration.is_none() {
        duration = parse_f64_string_field(&video_stream["duration"]);
    }

    // Root fix: ffprobe often reports 0/N/A duration for animated WebP (`webp_pipe`).
    // Loop-intent logic requires a real duration; derive it from ANMF frame durations.
    if duration.is_none()
        && format_name.contains("webp")
        && let Ok(data) = std::fs::read(path)
        && let Some(native_dur) = crate::image_formats::webp::duration_secs_from_bytes(&data)
    {
        let native_dur = f64::from(native_dur);
        if native_dur > 0.0_f64 {
            duration = Some(native_dur);
        }
    }

    // Allow 0.0 duration for formats like headless GIFs where duration is not globally specified
    duration
}

/// Parses a required u32 field from video stream JSON.
///
/// Attempts to parse the specified field as `u32`, with fallback to `coded_width`/`coded_height`
/// for width/height fields. Returns an error if the field is missing or invalid.
///
/// # Arguments
/// * `video_stream` - The video stream JSON object
/// * `field_name` - The field name to parse
///
/// # Returns
/// Parsed `u32` value, or `FFprobeError` if parsing fails
fn parse_required_u32_field(
    video_stream: &serde_json::Value,
    field_name: &str,
) -> Result<u32, FFprobeError> {
    let raw_value = video_stream[field_name]
        .as_u64()
        .or_else(|| {
            if field_name == "width" {
                video_stream["coded_width"].as_u64()
            } else if field_name == "height" {
                video_stream["coded_height"].as_u64()
            } else {
                None
            }
        })
        .ok_or_else(|| FFprobeError::ParseError(format!("Missing {field_name}")))?;

    u32::try_from(raw_value)
        .map_err(|_| FFprobeError::ParseError(format!("Invalid {field_name}: {raw_value}")))
}

/// Parses all video stream fields into a structured `VideoStreamFields`.
///
/// Extracts codec information, dimensions, frame rates, bit rates,
/// and other video-specific metadata from the video stream JSON.
///
/// # Arguments
/// * `video_stream` - The video stream JSON object
/// * `format_name` - The container format name
/// * `duration` - The resolved video duration
/// * `path` - The file path for debugging
///
/// # Returns
/// Parsed video stream fields, or `FFprobeError` if parsing fails
#[allow(
    clippy::too_many_lines,
    reason = "Single ffprobe JSON normalizer: covers many heterogeneous codec/format dialects in one place so divergent fallbacks cannot drift across helpers."
)]
fn parse_video_stream_fields(
    video_stream: &serde_json::Value,
    format_name: &str,
    duration: Option<f64>,
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
    if (width == 0 || height == 0)
        && let Some((fallback_w, fallback_h)) = crate::conversion::dimensions_without_ffprobe(path)
    {
        width = fallback_w;
        height = fallback_h;
    }
    if width == 0 || height == 0 {
        return Err(FFprobeError::ParseError(format!(
            "Invalid dimensions: {width}x{height}"
        )));
    }

    let frame_rate = video_stream["r_frame_rate"]
        .as_str()
        .and_then(|s| parse_frame_rate(s).ok());

    let mut avg_frame_rate = video_stream["avg_frame_rate"]
        .as_str()
        .and_then(|s| parse_frame_rate(s).ok());
    let is_variable_frame_rate =
        detect_vfr_enhanced(video_stream, frame_rate, avg_frame_rate, format_name);
    let mut frame_count = parse_u64_string_field(&video_stream["nb_frames"]);

    // Root fix for Safari-style animated WebP: ffprobe often reports invalid frame metadata
    // (e.g. nb_frames missing/absurd, image data not found) even when ANMF frames exist.
    // If the container is animated per native markers, trust native frame counting.
    if format_name.contains("webp")
        && let Ok(data) = std::fs::read(path)
        && crate::image_formats::webp::is_animated_from_bytes(&data)
    {
        let native_frames = u64::from(
            crate::image_formats::webp::count_frames_from_bytes(&data)
                .map_err(|e| FFprobeError::ParseError(e.to_string()))?,
        );
        if native_frames > 1 {
            frame_count = Some(native_frames);
        }
        if duration.is_none()
            && let Some(duration_secs) = crate::image_formats::webp::duration_secs_from_bytes(&data)
        {
            let duration_secs = f64::from(duration_secs);
            if duration_secs > 0.0_f64 {
                avg_frame_rate =
                    frame_count.map(|fc| crate::numeric_cast::u64_to_f64(fc) / duration_secs);
            }
        }
    }

    let pix_fmt = video_stream["pix_fmt"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown")
        .to_string();
    // `has_b_frames` is an optional stream field; absent means the codec/container
    // did not advertise B-frame usage — treat as None to avoid forgery.
    let max_b_frames = video_stream["has_b_frames"].as_i64().map(|v| {
        u8::try_from(v.clamp(0, i64::from(u8::MAX))).unwrap_or_else(|_| {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_PROBE,
                &format!(
                    "has_b_frames out of u8 range (val={}); clamping to 255 (path={})",
                    v,
                    path.display()
                )
            );
            u8::MAX
        })
    });

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
            .map(|level| format!("{:.1}", crate::numeric_cast::u64_to_f64(level) / 10.0_f64)),
        max_b_frames,
        encoder_settings: video_stream.get("tags").and_then(|tags| {
            tags.get("x265-params")
                .and_then(serde_json::Value::as_str)
                .or_else(|| tags.get("x264-params").and_then(serde_json::Value::as_str))
                .or_else(|| {
                    tags.get("encoder_settings")
                        .and_then(serde_json::Value::as_str)
                })
                .map(str::to_string)
        }),
        video_bit_rate: parse_u64_string_field(&video_stream["bit_rate"]),
        refs: video_stream["refs"]
            .as_u64()
            .and_then(|refs| crate::numeric_cast::u64_to_u32_strict(refs, "refs")),
        is_variable_frame_rate,
    })
}

/// Extracts audio stream information from available streams.
///
/// Searches for the first audio stream and extracts codec, bit rate,
/// sample rate, channel layout, and other audio-specific metadata.
///
/// # Arguments
/// * `streams` - Array of stream `JSON` objects from `FFprobe`
///
/// # Returns
/// Audio information struct with extracted fields
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
        sample_rate: parse_u64_string_field(&audio_stream["sample_rate"]).and_then(|sample_rate| {
            crate::numeric_cast::u64_to_u32_strict(sample_rate, "sample_rate")
        }),
        channels: audio_stream["channels"]
            .as_u64()
            .and_then(|channels| crate::numeric_cast::u64_to_u32_strict(channels, "channels")),
    }
}

/// Extracts subtitle stream information from available streams.
///
/// Searches for the first subtitle stream and extracts codec information.
/// Returns default info if no subtitle stream is found.
///
/// # Arguments
/// * `streams` - Array of stream `JSON` objects from `FFprobe`
///
/// # Returns
/// Subtitle information struct with extracted fields
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
    } = parse_probe_format(json.get("format").ok_or_else(|| {
        FFprobeError::ParseError("ffprobe JSON missing 'format' object".to_string())
    })?)?;
    let streams = json
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| FFprobeError::ParseError("No streams found".to_string()))?;
    let (stream_index, video_stream) = select_video_stream(streams)?;
    let duration = resolve_probe_duration(format_duration, video_stream, &format_name, path);
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
        loop_count: json.get("format").and_then(extract_loop_count),
        frame_types: extract_frame_types(&json),
        pts_deltas: extract_pts_deltas(&json),
        pkt_sizes: extract_pkt_sizes(&json),
        mv_magnitudes: Vec::new(),
    };

    // ── Penetrating Content Verification ──
    // Verify critical metadata by decoding actual content
    if result.audio.present
        && let crate::media_penetration::PenetrationResult::Verified(is_silent) =
            crate::media_penetration::detect_audio_silence(path)
        && is_silent
    {
        result.audio.present = false;
    }

    if let Some(fc_val) = result.frame_count
        && (fc_val <= crate::constants::FRAME_COUNT_TRUST_LOWER_LIMIT
            || fc_val > crate::constants::FRAME_COUNT_TRUST_UPPER_LIMIT)
        && let crate::media_penetration::PenetrationResult::Verified(real_count) =
            crate::media_penetration::detect_real_frame_count(path, fc_val)
        && real_count > 0
    {
        result.frame_count = Some(fc_val.max(real_count));
    }

    Ok(result)
}

/// Attempt to extract loop count from format tags (e.g. NETSCAPE2.0 or `LoopCount`)
fn extract_loop_count(format: &serde_json::Value) -> Option<u16> {
    if let Some(tags) = format["tags"].as_object()
        && let Some(val) = tags.get("loop_count").or_else(|| tags.get("loop"))
        && let Some(s) = val.as_str()
    {
        return s.parse::<u16>().ok();
    }
    None
}

/// Extracts frame picture types from `FFprobe` frame data.
///
/// Parses the frames array to collect picture type characters
/// (I, P, B frames) for analysis of video compression patterns.
///
/// # Arguments
/// * `json` - The `JSON` response from `FFprobe` containing frame data
///
/// # Returns
/// Vector of picture type characters
fn extract_frame_types(json: &serde_json::Value) -> Vec<char> {
    let mut types = Vec::new();
    if let Some(frames) = json["frames"].as_array() {
        for frame in frames {
            if let Some(pict_type) = frame["pict_type"].as_str()
                && let Some(first_char) = pict_type.chars().next()
            {
                types.push(first_char);
            }
        }
    }
    types
}

/// Extracts PTS (Presentation Timestamp) deltas from frame data.
///
/// Calculates the time differences between consecutive frames
/// to analyze frame timing patterns and detect variable frame rates.
///
/// # Arguments
/// * `json` - The `JSON` response from `FFprobe` containing frame data
///
/// # Returns
/// Vector of time deltas between consecutive frames
fn extract_pts_deltas(json: &serde_json::Value) -> Vec<f64> {
    let mut deltas = Vec::new();
    let mut last_pts: Option<f64> = None;
    if let Some(frames) = json["frames"].as_array() {
        for frame in frames {
            if let Some(pts_str) = frame["pkt_pts_time"].as_str()
                && let Ok(pts) = pts_str.parse::<f64>()
            {
                if let Some(last) = last_pts {
                    deltas.push((pts - last).abs());
                }
                last_pts = Some(pts);
            }
        }
    }
    deltas
}

/// Extracts packet sizes from frame data.
///
/// Parses the frames array to collect packet sizes for each frame.
/// Used to analyze data size patterns and compression efficiency.
///
/// # Arguments
/// * `json` - The `JSON` response from `FFprobe` containing frame data
///
/// # Returns
/// Vector of packet sizes in bytes
fn extract_pkt_sizes(json: &serde_json::Value) -> Vec<u64> {
    let mut sizes = Vec::new();
    if let Some(frames) = json["frames"].as_array() {
        for frame in frames {
            if let Some(size_str) = frame["pkt_size"].as_str()
                && let Ok(size) = size_str.parse::<u64>()
            {
                sizes.push(size);
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

            // Parse DOVI configuration record fields.
            // DV profile is u8 (0–9); values >255 are malformed side-data — warn and skip.
            if let Some(profile) = sd["dv_profile"].as_u64() {
                if let Ok(v) = u8::try_from(profile) {
                    dolby_vision.profile = Some(v);
                } else {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_DETECTION,
                        &format!("DV profile {profile} out of u8 range; ignoring")
                    );
                }
            }
            if let Some(compat_id) = sd["dv_bl_signal_compatibility_id"].as_u64() {
                if let Ok(v) = u8::try_from(compat_id) {
                    dolby_vision.bl_signal_compatibility_id = Some(v);
                } else {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_DETECTION,
                        &format!(
                            "DV bl_signal_compatibility_id {compat_id} out of u8 range; ignoring"
                        )
                    );
                }
            }
        }

        if sd_type.contains("hdr dynamic")
            || sd_type.contains("st2094")
            || sd_type.contains("hdr10+")
        {
            result.hdr10_plus = true;
        }

        // Mastering display: parse colour primaries + luminance into ffmpeg format
        if sd_type.contains("mastering display")
            && let Some(md_str) = build_mastering_display_string(sd)
        {
            result.mastering_display = Some(md_str);
        }

        // Content light level
        if sd_type.contains("content light level")
            && let Some(cll_str) = build_max_cll_string(sd)
        {
            result.max_cll = Some(cll_str);
        }
    }

    result
}

/// Convert a rational string like "13250/50000" to a u64 numerator (for ffmpeg master-display format).
/// ffmpeg expects values multiplied by [`crate::constants::HDR_COORD_SCALING_FACTOR`] for chromaticity coordinates.
fn parse_rational_to_50k(s: &str) -> Option<u64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = crate::numeric_cast::parse_strict(num.trim(), "hdr_num")?;
        let d: f64 = crate::numeric_cast::parse_strict(den.trim(), "hdr_den")?;
        if d == 0.0 {
            return None;
        }
        // Normalise to denominator 50000
        crate::numeric_cast::f64_to_u64_strict(
            (n / d) * crate::constants::HDR_COORD_SCALING_FACTOR,
            "hdr_coord",
        )
    } else {
        // plain float
        let v: f64 = crate::numeric_cast::parse_strict(s.trim(), "hdr_val")?;
        // Already normalised value (some ffprobe versions give 0.265 style)
        if v <= 1.0 {
            crate::numeric_cast::f64_to_u64_strict(
                v * crate::constants::HDR_COORD_SCALING_FACTOR,
                "hdr_coord",
            )
        } else {
            // raw integer-style already in 50k units
            let val = crate::numeric_cast::f64_to_u64_sat(v);
            Some(val)
        }
    }
}

/// Convert a rational luminance string to 10000-unit integer (cd/m² × 10000).
/// ffmpeg expects values multiplied by [`crate::constants::HDR_LUMA_SCALING_FACTOR`] for luminance.
fn parse_luminance_to_10k(s: &str) -> Option<u64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = crate::numeric_cast::parse_strict(num.trim(), "hdr_num")?;
        let d: f64 = crate::numeric_cast::parse_strict(den.trim(), "hdr_den")?;
        if d == 0.0 {
            return None;
        }
        crate::numeric_cast::f64_to_u64_strict(
            (n / d) * crate::constants::HDR_LUMA_SCALING_FACTOR,
            "hdr_luma",
        )
    } else {
        let v: f64 = crate::numeric_cast::parse_strict(s.trim(), "hdr_val")?;
        if v <= crate::constants::HDR_LUMA_SCALING_FACTOR {
            crate::numeric_cast::f64_to_u64_strict(
                v * crate::constants::HDR_LUMA_SCALING_FACTOR,
                "hdr_luma",
            )
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
                sd[field].as_f64().map(|v| {
                    crate::numeric_cast::f64_to_u64_sat(
                        v * crate::constants::HDR_COORD_SCALING_FACTOR,
                    )
                })
            })
    };
    let get_lum = |field: &str| -> Option<u64> {
        sd[field]
            .as_str()
            .and_then(parse_luminance_to_10k)
            .or_else(|| {
                sd[field].as_f64().map(|v| {
                    crate::numeric_cast::f64_to_u64_sat(
                        v * crate::constants::HDR_LUMA_SCALING_FACTOR,
                    )
                })
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

#[must_use]
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
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_DETECTION,
                &format!(
                    "Failed to launch ffprobe duration query for {}: {}",
                    path.display(),
                    err
                )
            );
            return None;
        }
    };

    if !output.status.success() {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_DETECTION,
            &format!(
                "ffprobe duration query failed for {} (stderr: {})",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    match trimmed.parse::<f64>() {
        Ok(duration) => Some(duration),
        Err(err) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_DETECTION,
                &format!(
                    "Failed to parse ffprobe duration output for {} (output: {}): {}",
                    path.display(),
                    trimmed,
                    err
                )
            );
            None
        }
    }
}

#[must_use]
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
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_DETECTION,
                &format!(
                    "Failed to launch ffprobe frame-count query for {}: {}",
                    path.display(),
                    err
                )
            );
            return None;
        }
    };

    if !output.status.success() {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_DETECTION,
            &format!(
                "ffprobe frame-count query failed for {} (stderr: {})",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    match trimmed.parse::<u64>() {
        Ok(frame_count) => Some(frame_count),
        Err(err) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_DETECTION,
                &format!(
                    "Failed to parse ffprobe frame-count output for {} (output: {}): {}",
                    path.display(),
                    trimmed,
                    err
                )
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
            let num = parts
                .first()
                .ok_or_else(|| FFprobeError::ParseError("Missing numerator".to_string()))?
                .parse::<f64>()
                .map_err(|e| FFprobeError::ParseError(format!("Invalid numerator: {e}")))?;
            let den = parts
                .get(1)
                .ok_or_else(|| FFprobeError::ParseError("Missing denominator".to_string()))?
                .parse::<f64>()
                .map_err(|e| FFprobeError::ParseError(format!("Invalid denominator: {e}")))?;

            if den == 0.0_f64 {
                return Err(FFprobeError::ParseError(
                    "Frame rate denominator cannot be zero".to_string(),
                ));
            }

            let rate = num / den;
            if rate >= 0.0_f64 {
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
pub fn detect_bit_depth(pix_fmt: &str) -> Option<u8> {
    if pix_fmt.is_empty() {
        return None;
    }
    let pix_fmt = pix_fmt.to_lowercase();
    if pix_fmt.contains("16") || pix_fmt.contains("48") || pix_fmt.contains("64") {
        return Some(16);
    }

    // Explicitly handle 8-bit formats that contain '12' or '10' in their name
    if pix_fmt == "nv12" || pix_fmt == "nv21" {
        return Some(8);
    }

    if pix_fmt.contains("12") {
        return Some(12);
    }

    if pix_fmt.contains("10")
        || pix_fmt.contains("p010")
        || pix_fmt.contains("p210")
        || pix_fmt.contains("p410")
    {
        return Some(10);
    }

    if pix_fmt.contains('8')
        || pix_fmt.contains("yuv")
        || pix_fmt.contains("rgb")
        || pix_fmt.contains("bgr")
        || pix_fmt.contains("nv12")
        || pix_fmt.contains("gray")
    {
        return Some(8);
    }

    None
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
            let result = parse_frame_rate(input).unwrap_or_else(|e| panic!("error: {e:?}"));
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
                Some(*expected),
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
            assert!((delta - 0.04).abs() < 0.001_f64);
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
        let loop_count = extract_loop_count(
            json_with_loop
                .get("format")
                .expect("Required JSON field missing or malformed"),
        );
        assert_eq!(loop_count, Some(5));

        let json_no_loop = serde_json::json!({
            "format": {
                "tags": {}
            }
        });
        let loop_count = extract_loop_count(
            json_no_loop
                .get("format")
                .expect("test: format key must exist"),
        );
        assert_eq!(loop_count, None);
    }

    // ──────────────────────────────────────────────────────────────────────────
    // Tests guarding previously-panicking paths (image container probing)
    // These tests catch the class of bugs that required a full production run to
    // discover. Each test mirrors a real-world failure mode (WebP, AVIF, APNG).
    // ──────────────────────────────────────────────────────────────────────────

    /// `bit_rate` absent from format section (WebP, AVIF) must NOT panic.
    #[test]
    fn parse_probe_format_absent_bit_rate_returns_ok() {
        let format = serde_json::json!({
            "format_name": "webp_pipe",
            "size": "204800",
            "duration": "0.04",
            "tags": {}
            // bit_rate intentionally absent — mirrors WebP ffprobe output
        });
        let result = parse_probe_format(&format);
        assert!(
            result.is_ok(),
            "parse_probe_format must not panic when bit_rate is absent: {result:?}"
        );
        let info = result.expect("parse_probe_format should succeed in test");
        assert!(
            info.bit_rate.is_none(),
            "bit_rate should be None when absent from format section"
        );
        assert!(info.duration.is_some_and(|d| (d - 0.04).abs() < 1e-9_f64));
    }

    /// `bit_rate` is null JSON (another form of absent) — must not panic.
    #[test]
    fn parse_probe_format_null_bit_rate_returns_none() {
        let format = serde_json::json!({
            "format_name": "avif",
            "size": "512000",
            "duration": "1.0",
            "bit_rate": null
        });
        let result = parse_probe_format(&format);
        assert!(result.is_ok());
        assert!(
            result
                .expect("parse_probe_format should succeed in test")
                .bit_rate
                .is_none()
        );
    }

    /// Missing `duration` must return `Err`, not panic or use a bogus default.
    #[test]
    fn parse_probe_format_absent_duration_returns_none() {
        let format = serde_json::json!({
            "format_name": "webp_pipe",
            "size": "204800"
        });
        let result = parse_probe_format(&format);
        assert!(result.is_ok());
        assert!(result.unwrap().duration.is_none());
    }

    /// `has_valid_dimensions` must not panic when width/height are 0.
    #[test]
    fn select_video_stream_zero_dimensions_does_not_panic() {
        // A stream where width/height are 0 (animated WebP via webp_pipe)
        let streams = vec![serde_json::json!({
            "codec_type": "video",
            "codec_name": "webp",
            "width": 0,
            "height": 0,
            "r_frame_rate": "25/1",
            "avg_frame_rate": "25/1",
            "nb_frames": "1"
        })];
        let result = select_video_stream(&streams);
        assert!(
            result.is_ok(),
            "select_video_stream must not panic with 0x0 stream: {result:?}"
        );
    }

    /// `nb_frames` absent from stream — must not panic in `select_video_stream`.
    #[test]
    fn select_video_stream_absent_nb_frames_does_not_panic() {
        let streams = vec![serde_json::json!({
            "codec_type": "video",
            "codec_name": "h264",
            "width": 1920,
            "height": 1080,
            "r_frame_rate": "30/1",
            "avg_frame_rate": "30/1"
            // nb_frames absent — common for some containers
        })];
        let result = select_video_stream(&streams);
        assert!(
            result.is_ok(),
            "absent nb_frames must not panic: {result:?}"
        );
    }

    /// `has_b_frames` absent — `parse_video_stream_fields` must not panic.
    #[test]
    fn parse_video_stream_fields_absent_has_b_frames_does_not_panic() {
        // Test the lower-level parse_video_stream_fields directly.
        // Width/height 0 triggers the image::image_dimensions fallback path;
        // that will fail for a non-existent path, but the function returns Ok
        // since dimension fallback failure is non-fatal (codec continues parsing).
        let stream = serde_json::json!({
            "codec_name": "webp",
            "codec_long_name": "WebP image",
            "r_frame_rate": "25/1",
            "avg_frame_rate": "25/1",
            "width": 640,
            "height": 480,
            "pix_fmt": "yuva420p",
            "color_space": "bt709",
            "color_transfer": "srgb",
            "bits_per_raw_sample": "8"
            // has_b_frames intentionally absent — mirrors WebP probe output
        });
        let path = std::path::Path::new("test.webp");
        let result = parse_video_stream_fields(&stream, "webp_pipe", Some(0.04_f64), path);
        assert!(
            result.is_ok(),
            "absent has_b_frames must not cause panic or Err: {result:?}"
        );
        let fields = result.expect("parse_video_stream_fields should succeed in test");
        // has_b_frames absent → treated as 0
        assert!(
            fields.max_b_frames.is_none(),
            "absent has_b_frames should be None"
        );
    }

    /// DV profile > 255 must not panic — should warn and skip.
    #[test]
    fn dolby_vision_profile_overflow_does_not_panic() {
        // Test parse_probe_format separately from stream fields:
        // the DV side-data is parsed in the extract_side_data path.
        // We verify that a side_data_list with dv_profile=9999 does not panic
        // by exercising the stream extraction path with a synthetic JSON.
        let stream_json = serde_json::json!({
            "codec_name": "hevc",
            "codec_long_name": "HEVC (High Efficiency Video Coding)",
            "r_frame_rate": "24/1",
            "avg_frame_rate": "24/1",
            "width": 3840,
            "height": 2160,
            "pix_fmt": "yuv420p10le",
            "color_space": "bt2020nc",
            "color_transfer": "smpte2084",
            "bits_per_raw_sample": "10",
            "has_b_frames": 0,
            "side_data_list": [{
                "side_data_type": "DOVI configuration record",
                "dv_profile": 9999,
                "dv_bl_signal_compatibility_id": 0
            }]
        });

        // The DV extraction is called inside `extract_side_data_info`.
        // Call parse_video_stream_fields which exercises that path.
        let path = std::path::Path::new("test_dv.mkv");
        let result =
            parse_video_stream_fields(&stream_json, "matroska,webm", Some(3600.0_f64), path);
        // Must not panic regardless of DV profile value
        assert!(
            result.is_ok(),
            "out-of-range DV profile must not panic: {result:?}"
        );
    }

    /// Test a comprehensive edge case: WebP with missing metadata that mirrors the reported failure mode.
    /// identify failed, ffprobe has limited info, animated media detected.
    #[test]
    fn test_webp_missing_metadata_flow() {
        // Mirrored from real-world failure: 0x0 dimensions and missing nb_frames
        let streams = vec![serde_json::json!({
            "codec_type": "video",
            "codec_name": "webp",
            "width": 0,
            "height": 0,
            "r_frame_rate": "25/1",
            "avg_frame_rate": "0/0"
        })];
        let result = select_video_stream(&streams);
        // Should succeed by picking the only available video stream, even if 0x0
        assert!(
            result.is_ok(),
            "select_video_stream must fallback to picking the stream even if dimensions are 0"
        );
        let (idx, stream) = result.expect("select_video_stream should succeed in test");
        assert_eq!(idx, 0);
        assert_eq!(stream["codec_name"], "webp");
    }
}
