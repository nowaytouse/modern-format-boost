//! `FFprobe` wrapper module
//!
//! Shared `FFprobe` functionality for video analysis.
//! Used by the `vid` pipeline.

use crate::builder_base::ToolBuilder;
use crate::media_precision::{BitDepthMetadata, MediaPrecision};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

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
    pub duration: Option<f64>,
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
    /// HDR10 mastering display metadata (e.g.
    /// "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,500)"
    /// )
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
    pub const fn has_explicit_hdr_metadata(&self) -> bool {
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
    /// `None` when ffprobe does not report it (common for image containers,
    /// e.g. WebP).
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
    /// True when `bit_depth` was inferred from `pix_fmt` because ffprobe did
    /// not expose explicit sample-depth fields.
    pub bit_depth_inferred_from_pix_fmt: bool,
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
    /// Stream index of the selected video stream (for multi-stream files like
    /// animated AVIF)
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

/// Result of the strict IMG admission probe for a video container that carries
/// exactly one still frame and no other media streams.
#[derive(Debug)]
pub enum SingleFrameVideoStillProbe {
    Eligible(Box<FFprobeResult>),
    Ineligible(String),
}

impl FFprobeResult {
    #[must_use]
    pub const fn has_b_frames(&self) -> bool {
        matches!(self.max_b_frames, Some(b) if b > 0)
    }

    #[must_use]
    pub fn color_assessment(&self) -> crate::ffprobe_json::ColorInfoAssessment {
        crate::ffprobe_json::ColorInfoAssessment::from_probe_fields(
            self.color_space.as_deref(),
            self.color_transfer.as_deref(),
            self.color_primaries.as_deref(),
            BitDepthMetadata::new(self.bit_depth, self.bit_depth_inferred_from_pix_fmt),
            crate::ffprobe_json::ColorProbeFlags {
                has_mastering_display: self.hdr.mastering_display.is_some(),
                has_max_cll: self.hdr.max_cll.is_some(),
                is_dolby_vision: self.hdr.is_dolby_vision(),
                is_hdr10_plus: self.hdr.hdr10_plus,
                is_float: crate::ffprobe_json::pix_fmt_indicates_float(Some(&self.pix_fmt)),
            },
        )
    }

    #[must_use]
    pub fn is_hdr(&self) -> bool {
        self.color_assessment().has_hdr_signaling()
    }
}

impl MediaPrecision for FFprobeResult {
    fn bit_depth_metadata(&self) -> BitDepthMetadata {
        BitDepthMetadata::new(self.bit_depth, self.bit_depth_inferred_from_pix_fmt)
    }

    fn has_hdr_signaling(&self) -> bool {
        self.color_assessment().has_hdr_signaling()
    }
}

#[must_use]
pub fn is_ffprobe_available() -> bool {
    crate::ffmpeg_builder::FfprobeBuilder::check_available()
}

fn ffprobe_timeout() -> Duration {
    let default_secs = crate::constants::FFPROBE_TIMEOUT_SECS;
    match std::env::var(crate::constants::ENV_MFB_FFPROBE_TIMEOUT_SECS) {
        Ok(raw) => {
            let trimmed = raw.trim();
            match trimmed.parse::<u64>() {
                Ok(secs) if (5..=600).contains(&secs) => return Duration::from_secs(secs),
                Ok(_) | Err(_) => {}
            }
            crate::log_warn!(
                crate::infra::static_logs::messages::LABEL_PROBE,
                &format!(
                    "ffprobe timeout env {}={raw:?} invalid; using {}s",
                    crate::constants::ENV_MFB_FFPROBE_TIMEOUT_SECS,
                    default_secs
                )
            );
            Duration::from_secs(default_secs)
        }
        Err(_) => Duration::from_secs(default_secs),
    }
}

fn run_ffprobe_command(
    command: &mut Command,
    path: &Path,
    query: &'static str,
) -> Result<crate::process_runner::ProcessOutput, FFprobeError> {
    let context = format!("ffprobe {query} for {}", path.display());
    crate::process_runner::ManagedProcess::spawn(command)
        .map_err(|e| {
            FFprobeError::ExecutionFailed(contextual_ffprobe_error(path, query, &e.to_string()))
        })?
        .wait_liveness_timeout(
            ffprobe_timeout(),
            crate::process_runner::video_process_hard_timeout(),
            &context,
        )
        .map_err(|e| {
            FFprobeError::ExecutionFailed(contextual_ffprobe_error(path, query, &e.to_string()))
        })
}

fn contextual_ffprobe_error(path: &Path, query: &'static str, detail: &str) -> String {
    format!("{query} query for {}: {detail}", path.display())
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
    /// `None` when ffprobe format section omits `bit_rate` (e.g. image
    /// containers).
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
    bit_depth_inferred_from_pix_fmt: bool,
    profile: Option<String>,
    level: Option<String>,
    max_b_frames: Option<u8>,
    encoder_settings: Option<String>,
    video_bit_rate: Option<u64>,
    refs: Option<u32>,
    is_variable_frame_rate: bool,
}

fn parse_stream_bit_depth(video_stream: &serde_json::Value, pix_fmt: &str) -> (Option<u8>, bool) {
    let explicit =
        crate::media_conversion_gate::probe_ffprobe_bit_depth_string_fields(video_stream)
            .and_then(|value| crate::numeric_cast::parse_strict::<u8>(value, "stream_bit_depth"));

    if explicit.is_some() {
        return (explicit, false);
    }

    if let Some(inferred) = detect_bit_depth(pix_fmt) {
        return (Some(inferred), true);
    }

    (None, false)
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
        return Err(FFprobeError::ExecutionFailed(
            crate::infra::static_logs::messages::MSG_PROBE_FILE_NOT_FOUND
                .replace("{}", &path.display().to_string()),
        ));
    }

    if !path.is_file() {
        return Err(FFprobeError::ExecutionFailed(
            crate::infra::static_logs::messages::MSG_PROBE_NOT_A_FILE
                .replace("{}", &path.display().to_string()),
        ));
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
    let mut command = crate::ffmpeg_builder::FfprobeBuilder::new();
    command
        .input(path)
        .loglevel("error")
        .print_format("json")
        .show_format()
        .show_streams()
        .show_frames()
        // CONTRACT: `FFPROBE_FRAME_SHOW_ENTRIES` must include `side_data_list` (HDR10+).
        .show_entries(crate::constants::FFPROBE_FRAME_SHOW_ENTRIES)
        .read_intervals("%+#300");
    let mut process = command.build();
    let output = run_ffprobe_command(&mut process, path, "json")?;

    if !output.status.success() {
        let stderr = output.stderr.trim();
        let error_msg = if stderr.is_empty() {
            format!(
                "Probe Audit: ffprobe analysis cycle failed for {}: exit {:?}",
                path.display(),
                output.status.code()
            )
        } else {
            format!(
                "Probe Audit: ffprobe execution failed for {}: {}",
                path.display(),
                stderr
            )
        };
        return Err(FFprobeError::ExecutionFailed(error_msg));
    }

    serde_json::from_str(&output.stdout).map_err(|e| FFprobeError::ParseError(e.to_string()))
}

fn exact_single_frame_video_duration(json: &serde_json::Value) -> Result<f64, String> {
    let streams = json
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "ffprobe did not return a stream inventory".to_string())?;
    if streams.len() != 1 {
        return Err(format!(
            "container has {} streams; IMG requires exactly one video stream and no audio, subtitle, data, or attachment streams",
            streams.len()
        ));
    }

    let stream = &streams[0];
    if stream.get("codec_type").and_then(serde_json::Value::as_str) != Some("video") {
        return Err("the only stream is not a video stream".to_string());
    }
    let width = stream
        .get("width")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "video stream width is unavailable; refusing to guess".to_string())?;
    let height = stream
        .get("height")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "video stream height is unavailable; refusing to guess".to_string())?;
    if width == 0 || height == 0 {
        return Err("video stream has no valid image canvas".to_string());
    }

    let decoded_frames = parse_u64_string_field(&stream["nb_read_frames"])
        .ok_or_else(|| "decoded frame count is unavailable; refusing to guess".to_string())?;
    if decoded_frames != 1 {
        return Err(format!(
            "decoded frame count is {decoded_frames}; IMG requires exactly one frame"
        ));
    }
    if let Some(declared_frames) = parse_u64_string_field(&stream["nb_frames"])
        && declared_frames != 1
    {
        return Err(format!(
            "declared frame count is {declared_frames}; IMG requires exactly one frame"
        ));
    }

    for section in ["chapters", "programs"] {
        if json
            .get(section)
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| !entries.is_empty())
        {
            return Err(format!(
                "container has {section}; JXL cannot preserve that timeline structure"
            ));
        }
    }

    let format_duration = json
        .get("format")
        .and_then(|format| parse_f64_string_field(&format["duration"]));
    let duration = match format_duration {
        Some(duration) => Some(duration),
        None => parse_f64_string_field(&stream["duration"]),
    }
    .ok_or_else(|| "duration is unavailable; refusing to infer a still image".to_string())?;
    if !duration.is_finite() || duration < 0.0 {
        return Err(format!("duration {duration} is invalid"));
    }
    if duration > crate::constants::MICRO_CLIP_CEILING_SECS {
        return Err(format!(
            "duration {duration:.3}s exceeds the IMG still-container ceiling of {:.3}s",
            crate::constants::MICRO_CLIP_CEILING_SECS
        ));
    }
    Ok(duration)
}

/// Prove that a video container is semantically a still image before IMG is
/// allowed to decode it. Unknown frame counts or extra streams fail closed.
///
/// # Errors
/// Returns `FFprobeError` when the exact inventory cannot be obtained.
pub fn probe_single_frame_video_still(
    path: &Path,
) -> Result<SingleFrameVideoStillProbe, FFprobeError> {
    if !is_ffprobe_available() {
        return Err(FFprobeError::ToolNotFound(
            crate::infra::static_logs::messages::MSG_PROBE_TOOL_MISSING.to_string(),
        ));
    }
    validate_probe_target(path)?;

    let mut command = crate::ffmpeg_builder::FfprobeBuilder::new();
    command
        .input(path)
        .loglevel("error")
        .print_format("json")
        .show_format()
        .show_streams()
        .count_frames()
        .arg("-show_chapters")
        .arg("-show_programs");
    let mut process = command.build();
    let output = run_ffprobe_command(&mut process, path, "exact single-frame inventory")?;
    if !output.status.success() {
        return Err(FFprobeError::ExecutionFailed(contextual_ffprobe_error(
            path,
            "exact single-frame inventory",
            output.stderr.trim(),
        )));
    }
    let json: serde_json::Value = serde_json::from_str(&output.stdout)
        .map_err(|error| FFprobeError::ParseError(error.to_string()))?;
    let duration = match exact_single_frame_video_duration(&json) {
        Ok(duration) => duration,
        Err(reason) => return Ok(SingleFrameVideoStillProbe::Ineligible(reason)),
    };

    let mut probe = probe_video(path)?;
    probe.duration = Some(duration);
    probe.frame_count = Some(1);
    Ok(SingleFrameVideoStillProbe::Eligible(Box::new(probe)))
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

/// `nb_frames` for each video stream (empty when ffprobe is unavailable or
/// probe fails).
#[must_use]
pub fn video_stream_frame_counts(path: &Path) -> Vec<u64> {
    if !is_ffprobe_available() {
        return Vec::new();
    }
    let Ok(json) = run_ffprobe_json(path) else {
        return Vec::new();
    };
    let Some(streams) = json.get("streams").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    streams
        .iter()
        .filter(|stream| stream["codec_type"].as_str() == Some("video"))
        .filter_map(|stream| parse_u64_string_field(&stream["nb_frames"]))
        .collect()
}

/// Returns true when `counts` look like a cover stream (≤1 frame) plus a longer
/// stream.
#[must_use]
pub(crate) fn isobmff_cover_stream_ambiguous_from_counts(counts: &[u64]) -> bool {
    if counts.len() <= 1 {
        return false;
    }
    let has_multi = counts.iter().any(|&c| c > 1);
    let has_single = counts.iter().any(|&c| c <= 1);
    has_multi && has_single
}

/// Multiple video streams where one looks like a single-frame cover/thumbnail
/// and another is longer.
#[must_use]
pub fn isobmff_cover_stream_ambiguous(path: &Path) -> bool {
    isobmff_cover_stream_ambiguous_from_counts(&video_stream_frame_counts(path))
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

/// Still-image and pipe muxers often omit `format.duration`; absence is not a
/// probe defect.
fn format_duration_absent_is_expected(format_name: &str, path: &Path) -> bool {
    let name = format_name.to_ascii_lowercase();
    if name.contains("heif")
        || name.contains("heic")
        || name.contains("avif")
        || name.contains("webp")
        || name.contains("image2")
        || name.contains("png_pipe")
        || name.contains("gif")
        || name.contains("bmp")
        || name.contains("tiff")
        || name.contains("jpeg_pipe")
        || name.contains("mjpeg")
        || name.contains("jxl")
        || name.contains("jpegxl")
        || name.contains("jpeg_xl")
    {
        return true;
    }
    let ext = crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path);
    crate::constants::IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Parses the format information from `FFprobe` `JSON` output.
///
/// Extracts format-level metadata including format name, duration,
/// bit rate, and other format-specific information.
///
/// # Arguments
/// * `format` - The format `JSON` object from `FFprobe`
/// * `path` - Source path (extension used when muxer name is generic)
///
/// # Returns
/// Parsed format information, or `FFprobeError` if parsing fails
fn parse_probe_format(
    format: &serde_json::Value,
    path: &Path,
) -> Result<ProbeFormatInfo, FFprobeError> {
    let format_name = format["format_name"]
        .as_str()
        .ok_or_else(|| FFprobeError::ParseError("Missing format_name".to_string()))?
        .to_string();
    let size = parse_u64_string_field(&format["size"])
        .ok_or_else(|| FFprobeError::ParseError("Missing or invalid file size".to_string()))?;

    // `bit_rate` is absent for many image containers (WebP, AVIF) — treat as
    // optional.
    let bit_rate = parse_u64_string_field(&format["bit_rate"]);
    if bit_rate.is_none() {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PROBE,
            crate::infra::static_logs::messages::MSG_PROBE_BITRATE_MISSING
        );
    }

    // `duration` is optional in the format section for still-image and pipe muxers.
    let duration = parse_f64_string_field(&format["duration"]);
    if duration.is_none() && !format_duration_absent_is_expected(&format_name, path) {
        crate::media_conversion_gate::probe_format_duration_missing_audit();
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
    path: Option<&Path>,
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

    // A stream with zero or absent dimensions is not selectable as the primary
    // stream.
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
                let nb = crate::media_conversion_gate::probe_ffprobe_stream_nb_frames_sort_or_zero(
                    parse_u64_string_field(&stream["nb_frames"]),
                );
                (u8::from(has_valid_dimensions(stream)), nb)
            })
            .ok_or_else(|| FFprobeError::ParseError("No video stream found".to_string()))?
    } else {
        video_streams
            .into_iter()
            .next()
            .ok_or_else(|| FFprobeError::ParseError("No video stream found".to_string()))?
    };

    let actual_index = crate::media_conversion_gate::probe_stream_index_or_fallback(
        stream["index"]
            .as_u64()
            .and_then(|index| crate::numeric_cast::u64_to_usize_strict(index, "stream_index")),
        fallback_index,
        path,
    );

    Ok((actual_index, stream))
}

/// Resolves the accurate duration from format and stream information.
///
/// Uses format duration as primary source, falls back to stream duration.
/// Applies format-specific corrections and fallbacks for different container
/// types.
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
) -> Result<Option<f64>, FFprobeError> {
    // `format_duration` was already validated as present in `parse_probe_format`;
    // the stream-level fallback is a secondary source for edge cases.
    let mut duration = format_duration.filter(|&d| d > 0.0_f64);

    if duration.is_none() {
        duration = parse_f64_string_field(&video_stream["duration"]);
    }

    // Root fix: ffprobe often reports 0/N/A duration for animated WebP
    // (`webp_pipe`). Loop-intent logic requires a real duration; derive it from
    // ANMF frame durations.
    if duration.is_none() && format_name.contains("webp") {
        let data = read_native_probe_bytes(path, "webp duration fallback")?;
        if let Some(native_dur) = crate::image_formats::webp::duration_secs_from_bytes(&data)
            .map_err(|error| FFprobeError::ParseError(error.to_string()))?
        {
            let native_dur = f64::from(native_dur);
            if native_dur > 0.0_f64 {
                duration = Some(native_dur);
            }
        }
    }

    if duration.is_none() && format_name.contains("gif") {
        let data = read_native_probe_bytes(path, "gif duration fallback")?;
        if let Some(stats) = crate::image_formats::gif::timing_stats_from_bytes(&data)
            .map_err(|err| FFprobeError::ParseError(err.to_string()))?
            && stats.duration_secs > 0.0_f64
        {
            duration = Some(stats.duration_secs);
        }
    }

    if duration.is_none() && format_name.contains("png") {
        let data = read_native_probe_bytes(path, "apng duration fallback")?;
        if let Some(stats) = crate::image_detection::apng_timing_stats_from_bytes(&data)
            && stats.duration_secs > 0.0_f64
        {
            duration = Some(stats.duration_secs);
        }
    }

    if duration.is_none() {
        duration = probe_duration_from_frame_count_and_fps(
            parse_u64_string_field(&video_stream["nb_frames"]),
            parse_optional_frame_rate_field(video_stream, "avg_frame_rate", path),
            parse_optional_frame_rate_field(video_stream, "r_frame_rate", path),
        );
    }

    // Allow 0.0 duration for formats like headless GIFs where duration is not
    // globally specified
    Ok(duration)
}

fn parse_optional_frame_rate_field(
    video_stream: &serde_json::Value,
    field_name: &str,
    path: &Path,
) -> Option<f64> {
    let Some(raw) = video_stream[field_name].as_str() else {
        return None;
    };
    match parse_frame_rate(raw) {
        Ok(value) => Some(value),
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "ffprobe_frame_rate_parse_failed",
                path,
                format!("{field_name}={raw:?} could not be parsed as frame rate: {err}"),
            );
            None
        }
    }
}

fn read_native_probe_bytes(path: &Path, context: &str) -> Result<Vec<u8>, FFprobeError> {
    std::fs::read(path).map_err(|err| {
        FFprobeError::IoError(io::Error::new(
            err.kind(),
            format!("{context} read failed for {}: {err}", path.display()),
        ))
    })
}

#[must_use]
fn probe_duration_from_frame_count_and_fps(
    nb_frames: Option<u64>,
    avg_frame_rate: Option<f64>,
    r_frame_rate: Option<f64>,
) -> Option<f64> {
    let frames = nb_frames.filter(|count| *count > 0)?;
    let fps = crate::media_conversion_gate::probe_ffprobe_fps_avg_or_r_frame_rate(
        avg_frame_rate,
        r_frame_rate,
    )?;
    let secs = crate::numeric_cast::u64_to_f64(frames) / fps;
    (secs.is_finite() && secs > 0.0_f64).then_some(secs)
}

/// Parses a required u32 field from video stream JSON.
///
/// Attempts to parse the specified field as `u32`, with fallback to
/// `coded_width`/`coded_height` for width/height fields. Returns an error if
/// the field is missing or invalid.
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
    let raw_value =
        crate::media_conversion_gate::probe_ffprobe_stream_u64_field(video_stream, field_name)
            .ok_or_else(|| FFprobeError::ParseError(format!("Missing {field_name}")))?;

    u32::try_from(raw_value)
        .map_err(|e| FFprobeError::ParseError(format!("Invalid {field_name}: {raw_value} - {e}")))
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
    let video_codec_long = crate::media_conversion_gate::probe_ffprobe_optional_string(
        video_stream["codec_long_name"].as_str(),
        "codec_long_name",
        &format!("ffprobe video stream for {}", path.display()),
    );
    let mut width = parse_required_u32_field(video_stream, "width")?;
    let mut height = parse_required_u32_field(video_stream, "height")?;
    if width == 0 || height == 0 {
        let fallback =
            crate::media_conversion_gate::probe_ffprobe_zero_dimension_recovery(path, format_name);
        if let Some((fallback_w, fallback_h)) = fallback {
            crate::media_conversion_gate::probe_bitstream_dimension_recovery_audit(
                path, width, height, fallback_w, fallback_h,
            );
            width = fallback_w;
            height = fallback_h;
        }
    }
    if width == 0 || height == 0 {
        return Err(FFprobeError::ParseError(format!(
            "Invalid dimensions: {width}x{height}"
        )));
    }

    let frame_rate = parse_optional_frame_rate_field(video_stream, "r_frame_rate", path);

    let mut avg_frame_rate = parse_optional_frame_rate_field(video_stream, "avg_frame_rate", path);
    let is_variable_frame_rate =
        detect_vfr_enhanced(video_stream, frame_rate, avg_frame_rate, format_name);
    let mut frame_count = parse_u64_string_field(&video_stream["nb_frames"]);

    // Root fix for Safari-style animated WebP: ffprobe often reports invalid frame
    // metadata (e.g. nb_frames missing/absurd, image data not found) even when
    // ANMF frames exist. If the container is animated per native markers, trust
    // native frame counting.
    if format_name.contains("webp") {
        let data = read_native_probe_bytes(path, "webp native frame fallback")?;
        if crate::image_formats::webp::is_animated_from_bytes(&data) {
            let native_frames = u64::from(
                crate::image_formats::webp::count_frames_from_bytes(&data)
                    .map_err(|e| FFprobeError::ParseError(e.to_string()))?,
            );
            if native_frames > 1 {
                frame_count = Some(native_frames);
            }
            if duration.is_none()
                && let Some(duration_secs) =
                    crate::image_formats::webp::duration_secs_from_bytes(&data)
                        .map_err(|error| FFprobeError::ParseError(error.to_string()))?
            {
                let duration_secs = f64::from(duration_secs);
                if duration_secs > 0.0_f64 {
                    avg_frame_rate =
                        frame_count.map(|fc| crate::numeric_cast::u64_to_f64(fc) / duration_secs);
                }
            }
        }
    }

    // Root fix: ffprobe often under-reports `nb_frames` for animated GIF (M126).
    if format_name.contains("gif") || video_codec.eq_ignore_ascii_case("gif") {
        let data = read_native_probe_bytes(path, "gif native frame fallback")?;
        let count = crate::image_formats::gif::count_frames_from_bytes(&data)
            .map_err(|err| FFprobeError::ParseError(err.to_string()))?;
        let native_frames = u64::from(count);
        if native_frames > 1 {
            frame_count = Some(native_frames);
            if duration.is_none() {
                let stats = crate::image_formats::gif::timing_stats_from_bytes(&data)
                    .map_err(|err| FFprobeError::ParseError(err.to_string()))?;
                if let Some(stats) = stats
                    && stats.duration_secs > 0.0_f64
                {
                    avg_frame_rate =
                        Some(crate::numeric_cast::u64_to_f64(native_frames) / stats.duration_secs);
                }
            }
        }
    }

    // Root fix: APNG via `png_pipe` — use validated APNG structure when ffprobe
    // omits frames (M126).
    if format_name.contains("png")
        || format_name.contains("apng")
        || video_codec.eq_ignore_ascii_case("apng")
    {
        let data = read_native_probe_bytes(path, "apng native frame fallback")?;
        let info = crate::image::png_validation::parse_apng_animation(&data)
            .map_err(|error| FFprobeError::ParseError(error.to_string()))?;
        if let Some(info) = info.filter(|info| info.frame_count > 1) {
            let native_frames = u64::from(info.frame_count);
            frame_count = Some(native_frames);
            if duration.is_none() && info.duration_secs > 0.0_f64 {
                avg_frame_rate =
                    Some(crate::numeric_cast::u64_to_f64(native_frames) / info.duration_secs);
            }
        }
    }

    let pix_fmt = crate::media_conversion_gate::probe_pix_fmt_label(
        video_stream["pix_fmt"].as_str(),
        path,
        "extract_video_stream_fields",
    );
    // `has_b_frames` is an optional stream field; absent means the codec/container
    // did not advertise B-frame usage — treat as None to avoid forgery.
    let max_b_frames = video_stream["has_b_frames"]
        .as_i64()
        .map(|v| crate::media_conversion_gate::probe_b_frames_u8_or_max(v, path));

    let (stream_bit_depth, bit_depth_inferred_from_pix_fmt) =
        parse_stream_bit_depth(video_stream, &pix_fmt);

    Ok(VideoStreamFields {
        video_codec,
        video_codec_long,
        width,
        height,
        frame_rate,
        avg_frame_rate,
        frame_count,
        pix_fmt,
        color_space: parse_optional_known_string(&video_stream["color_space"]),
        color_transfer: parse_optional_known_string(&video_stream["color_transfer"]),
        color_primaries: parse_optional_known_string(&video_stream["color_primaries"]),
        bit_depth: stream_bit_depth,
        bit_depth_inferred_from_pix_fmt,
        profile: video_stream["profile"].as_str().map(str::to_string),
        level: video_stream["level"]
            .as_u64()
            .map(|level| format!("{:.1}", crate::numeric_cast::u64_to_f64(level) / 10.0_f64)),
        max_b_frames,
        encoder_settings: video_stream
            .get("tags")
            .and_then(crate::media_conversion_gate::probe_ffprobe_encoder_settings_from_tags),
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
        duration: parse_f64_string_field(&audio_stream["duration"]),
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
/// Returns `FFprobeError` if `ffprobe` is not found, execution fails, or
/// parsing results fails.
///
/// # Panics
/// Panics if no video streams are found.
pub fn probe_video(path: &Path) -> Result<FFprobeResult, FFprobeError> {
    if !is_ffprobe_available() {
        return Err(FFprobeError::ToolNotFound(
            crate::infra::static_logs::messages::MSG_PROBE_TOOL_MISSING.to_string(),
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
    } = parse_probe_format(
        json.get("format").ok_or_else(|| {
            FFprobeError::ParseError("ffprobe JSON missing 'format' object".to_string())
        })?,
        path,
    )?;
    let streams = json
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| FFprobeError::ParseError("No streams found".to_string()))?;
    let (stream_index, video_stream) = select_video_stream(streams, Some(path))?;
    let duration = resolve_probe_duration(format_duration, video_stream, &format_name, path)?;
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
        bit_depth_inferred_from_pix_fmt: video.bit_depth_inferred_from_pix_fmt,
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
        loop_count: json
            .get("format")
            .and_then(|format| extract_loop_count(path, format)),
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
            crate::media_penetration::detect_real_frame_count(path, Some(fc_val))
        && real_count > 0
    {
        result.frame_count = Some(fc_val.max(real_count));
    }

    Ok(result)
}

/// Attempt to extract loop count from format tags (e.g. NETSCAPE2.0 or
/// `LoopCount`)
fn extract_loop_count(path: &Path, format: &serde_json::Value) -> Option<u16> {
    if let Some(tags) = format["tags"].as_object()
        && let Some(val) = crate::media_conversion_gate::probe_ffprobe_format_loop_count_tag(tags)
        && let Some(s) = val.as_str()
    {
        return match s.parse::<u16>() {
            Ok(loop_count) => Some(loop_count),
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "ffprobe_loop_count_tag_parse_failed",
                    path,
                    format!("invalid LoopCount/NETSCAPE2.0 tag value {s:?}: {err}"),
                );
                None
            }
        };
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
fn frame_pts_seconds(frame: &serde_json::Value) -> Option<f64> {
    for key in [
        "pkt_pts_time",
        "best_effort_timestamp_time",
        "pkt_dts_time",
        "pts_time",
    ] {
        let Some(raw) = frame.get(key) else {
            continue;
        };
        let parsed = crate::media_conversion_gate::probe_ffprobe_json_value_as_f64(raw);
        if let Some(pts) = parsed.filter(|v| v.is_finite()) {
            return Some(pts);
        }
    }
    None
}

fn extract_pts_deltas(json: &serde_json::Value) -> Vec<f64> {
    let mut deltas = Vec::new();
    let mut last_pts: Option<f64> = None;
    if let Some(frames) = json["frames"].as_array() {
        for frame in frames {
            if let Some(pts) = frame_pts_seconds(frame) {
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
            if let Some(size_str) = frame["pkt_size"].as_str() {
                match size_str.parse::<u64>() {
                    Ok(size) => sizes.push(size),
                    Err(err) => {
                        crate::media_conversion_gate::probe_image_format_batch_audit(
                            "ffprobe_pkt_size_parse_failed",
                            format!("failed to parse ffprobe pkt_size {size_str:?}: {err}"),
                        );
                    }
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
pub(crate) fn extract_hdr_side_data(json: &serde_json::Value) -> FFprobeHdrInfo {
    let mut result = FFprobeHdrInfo::default();

    // Collect all side_data arrays from streams and frames
    let mut side_data_entries: Vec<&serde_json::Value> = Vec::new();

    // Accept either a full ffprobe document or a single stream/frame object.
    if let Some(sda) = json["side_data_list"].as_array() {
        side_data_entries.extend(sda.iter());
    }

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
        let sd_type =
            crate::media_conversion_gate::probe_side_data_type_label(sd["side_data_type"].as_str());

        if sd_type.contains("dolby vision") || sd_type.contains("dovi") {
            let dolby_vision = result.dolby_vision.get_or_insert_default();

            // Parse DOVI configuration record fields.
            // DV profile is u8 (0–9); values >255 are malformed side-data — warn and skip.
            if let Some(profile) = sd["dv_profile"].as_u64() {
                match u8::try_from(profile) {
                    Ok(v) => dolby_vision.profile = Some(v),
                    Err(_) => {
                        crate::media_conversion_gate::probe_hdr_metadata_u8_or_skip(
                            profile,
                            "ffprobe_dv_profile_out_of_range",
                            format!("DV profile {profile} out of u8 range; ignoring"),
                        );
                    }
                }
            }
            if let Some(compat_id) = sd["dv_bl_signal_compatibility_id"].as_u64() {
                match u8::try_from(compat_id) {
                    Ok(v) => dolby_vision.bl_signal_compatibility_id = Some(v),
                    Err(_) => {
                        crate::media_conversion_gate::probe_hdr_metadata_u8_or_skip(
                            compat_id,
                            "ffprobe_dv_compat_id_out_of_range",
                            format!(
                                "DV bl_signal_compatibility_id {compat_id} out of u8 range; \
                                 ignoring"
                            ),
                        );
                    }
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

/// Build the ffmpeg `-master_display` string from a `mastering_display`
/// `side_data` object. Format: "G(gx,gy)B(bx,by)R(rx,ry)WP(wx,wy)L(lmax,lmin)"
fn build_mastering_display_string(sd: &serde_json::Value) -> Option<String> {
    let gx =
        crate::media_conversion_gate::probe_ffprobe_hdr_side_data_chromaticity_u64(sd, "green_x")?;
    let gy =
        crate::media_conversion_gate::probe_ffprobe_hdr_side_data_chromaticity_u64(sd, "green_y")?;
    let bx =
        crate::media_conversion_gate::probe_ffprobe_hdr_side_data_chromaticity_u64(sd, "blue_x")?;
    let by_ =
        crate::media_conversion_gate::probe_ffprobe_hdr_side_data_chromaticity_u64(sd, "blue_y")?;
    let rx =
        crate::media_conversion_gate::probe_ffprobe_hdr_side_data_chromaticity_u64(sd, "red_x")?;
    let ry =
        crate::media_conversion_gate::probe_ffprobe_hdr_side_data_chromaticity_u64(sd, "red_y")?;
    let wx = crate::media_conversion_gate::probe_ffprobe_hdr_side_data_chromaticity_u64(
        sd,
        "white_point_x",
    )?;
    let wy = crate::media_conversion_gate::probe_ffprobe_hdr_side_data_chromaticity_u64(
        sd,
        "white_point_y",
    )?;
    let lmax = crate::media_conversion_gate::probe_ffprobe_hdr_side_data_luminance_u64(
        sd,
        "max_luminance",
    )?;
    let lmin = crate::media_conversion_gate::probe_ffprobe_hdr_side_data_luminance_u64(
        sd,
        "min_luminance",
    )?;

    Some(format!(
        "G({gx},{gy})B({bx},{by_})R({rx},{ry})WP({wx},{wy})L({lmax},{lmin})"
    ))
}

/// Build the ffmpeg `-cll` string: "MaxCLL,MaxFALL"
fn build_max_cll_string(sd: &serde_json::Value) -> Option<String> {
    let max_content = crate::media_conversion_gate::probe_ffprobe_cll_max_content_u64(sd)?;
    let max_average = crate::media_conversion_gate::probe_ffprobe_cll_max_average_u64(sd)?;
    Some(format!("{max_content},{max_average}"))
}

#[must_use]
pub fn get_duration(path: &Path) -> Option<f64> {
    let mut command = crate::ffmpeg_builder::FfprobeBuilder::new();
    command
        .input(path)
        .loglevel("error")
        .show_entries("format=duration")
        .print_format("default=noprint_wrappers=1:nokey=1");
    let mut process = command.build();
    let output = match run_ffprobe_command(&mut process, path, "duration") {
        Ok(output) => output,
        Err(err) => {
            crate::media_conversion_gate::probe_ffprobe_path_audit(
                "ffprobe_duration_query_failed",
                path,
                err.to_string(),
            );
            return None;
        }
    };

    if !output.status.success() {
        crate::media_conversion_gate::probe_ffprobe_path_audit(
            "ffprobe_duration_query_failed",
            path,
            format!(
                "{} | stderr={}",
                crate::infra::static_logs::messages::MSG_PROBE_DURATION_FAIL
                    .replace("{}", &path.display().to_string()),
                output.stderr.trim()
            ),
        );
        return None;
    }

    let trimmed = output.stdout.trim();
    if trimmed == "N/A" || trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<f64>() {
        Ok(duration) => Some(duration),
        Err(err) => {
            crate::media_conversion_gate::probe_ffprobe_path_audit(
                "ffprobe_duration_parse_failed",
                path,
                format!("failed to parse ffprobe duration output ({trimmed}): {err}"),
            );
            None
        }
    }
}

#[must_use]
pub fn get_frame_count(path: &Path) -> Option<u64> {
    let mut command = crate::ffmpeg_builder::FfprobeBuilder::new();
    command
        .input(path)
        .loglevel("error")
        .count_frames()
        .select_stream(crate::ffmpeg_builder::StreamType::Video, 0)
        .show_entries("stream=nb_read_frames")
        .print_format("default=noprint_wrappers=1:nokey=1");
    let mut process = command.build();
    let output = match run_ffprobe_command(&mut process, path, "frame_count") {
        Ok(output) => output,
        Err(err) => {
            crate::media_conversion_gate::probe_ffprobe_path_audit(
                "ffprobe_frame_count_query_failed",
                path,
                err.to_string(),
            );
            return None;
        }
    };

    if !output.status.success() {
        crate::media_conversion_gate::probe_ffprobe_path_audit(
            "ffprobe_frame_count_query_failed",
            path,
            format!(
                "ffprobe frame-count query failed (stderr: {})",
                output.stderr.trim()
            ),
        );
        return None;
    }

    // Multi-stream ISOBMFF (AVIF/HEIC cover + image) can emit one count per stream
    // line ("1\n1").
    let mut parsed: Vec<u64> = Vec::new();
    // Track whether any non-empty, non-N/A content appeared.
    let mut has_unexpected_content = false;
    for line in output.stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "N/A" {
            continue;
        }
        has_unexpected_content = true;
        match trimmed.parse::<u64>() {
            Ok(value) => parsed.push(value),
            Err(err) => {
                crate::media_conversion_gate::probe_ffprobe_path_audit(
                    "ffprobe_frame_count_line_parse_failed",
                    path,
                    format!("failed to parse ffprobe frame-count line {trimmed:?}: {err}"),
                );
            }
        }
    }
    if let Some(frame_count) = parsed.into_iter().max() {
        Some(frame_count)
    } else if has_unexpected_content {
        // Non-N/A content appeared but nothing parsed to a u64 — line-level
        // audit already fired above for each offending token. Log this fallback
        // path explicitly so it is never silent.
        tracing::debug!(
            target: "mfb.ffprobe",
            path = %path.display(),
            "ffprobe_frame_count_parse_failed: all non-N/A lines failed u64 parse; returning None"
        );
        None
    } else {
        // All non-empty lines were "N/A": ffprobe found the file but reports no
        // countable frames for the selected video stream. This is the expected
        // outcome for still images (JXL, AVIF, etc.) that carry no video
        // stream. Not an error — log explicitly at debug so the path is
        // visible, then return None.
        let trimmed = output.stdout.trim();
        if trimmed.is_empty() {
            // Truly empty stdout is unexpected even for still images — keep the
            // probe audit so it surfaces in strict-delivery logs.
            crate::media_conversion_gate::probe_ffprobe_path_audit(
                "ffprobe_frame_count_empty_output",
                path,
                "ffprobe frame-count returned empty stdout",
            );
        } else {
            // Pure N/A output — known still-image case. Explicit debug log;
            // never a probe_ffprobe_path_audit (which escalates to RARE ERROR).
            tracing::debug!(
                target: "mfb.ffprobe",
                path = %path.display(),
                "ffprobe_frame_count_no_video_stream: stdout={trimmed:?}; \
                 still image with no video stream — returning None"
            );
        }
        None
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

            if crate::numeric_cast::is_effectively_zero(
                den,
                crate::numeric_cast::FloatContext::FfmpegMeasurement,
            ) {
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
    if pix_fmt == "unknown" {
        return None;
    }
    if pix_fmt.contains("f32") {
        return Some(32);
    }
    if pix_fmt.contains("rgba64")
        || pix_fmt.contains("bgra64")
        || pix_fmt.contains("argb64")
        || pix_fmt.contains("abgr64")
        || pix_fmt.contains("rgb48")
        || pix_fmt.contains("bgr48")
        || pix_fmt.contains("gray16")
        || pix_fmt.contains("ya16")
        || pix_fmt.contains("gbrp16")
        || pix_fmt.contains("gbrap16")
        || pix_fmt.contains("p416")
        || pix_fmt.contains("p216")
        || pix_fmt.contains("p016")
    {
        return Some(16);
    }
    if pix_fmt.contains("p14") {
        return Some(14);
    }
    if pix_fmt.contains("p12") {
        return Some(12);
    }
    if pix_fmt.contains("p10")
        || pix_fmt.contains("gray10")
        || pix_fmt.contains("ya10")
        || pix_fmt.contains("gbrp10")
        || pix_fmt.contains("gbrap10")
    {
        return Some(10);
    }
    if pix_fmt.contains("p9")
        || pix_fmt.contains("gray9")
        || pix_fmt.contains("ya9")
        || pix_fmt.contains("gbrp9")
        || pix_fmt.contains("gbrap9")
    {
        return Some(9);
    }

    if matches!(
        pix_fmt.as_str(),
        "nv12"
            | "nv21"
            | "yuv420p"
            | "yuv422p"
            | "yuv444p"
            | "yuv440p"
            | "yuv411p"
            | "yuv410p"
            | "yuva420p"
            | "yuva422p"
            | "yuva444p"
            | "gbrp"
            | "gbrap"
            | "gray"
            | "gray8"
            | "pal8"
            | "rgb24"
            | "bgr24"
            | "rgb0"
            | "bgr0"
            | "0rgb"
            | "0bgr"
            | "rgba"
            | "bgra"
            | "argb"
            | "abgr"
    ) {
        return Some(8);
    }

    None
}

#[cfg(test)]
mod cover_stream_tests {
    use super::isobmff_cover_stream_ambiguous_from_counts;

    #[test]
    fn isobmff_cover_stream_ambiguous_detects_mixed_frame_streams() {
        assert!(isobmff_cover_stream_ambiguous_from_counts(&[1, 24]));
        assert!(isobmff_cover_stream_ambiguous_from_counts(&[0, 10]));
        assert!(!isobmff_cover_stream_ambiguous_from_counts(&[1]));
        assert!(!isobmff_cover_stream_ambiguous_from_counts(&[12, 24]));
    }
}

#[cfg(test)]
mod single_frame_video_still_tests {
    use super::exact_single_frame_video_duration;
    use serde_json::json;

    #[test]
    fn img_admission_requires_one_decoded_frame_and_no_extra_streams() {
        let eligible = json!({
            "format": { "duration": "0.040" },
            "streams": [{
                "codec_type": "video",
                "width": 640,
                "height": 480,
                "nb_frames": "1",
                "nb_read_frames": "1"
            }],
            "chapters": [],
            "programs": []
        });
        assert_eq!(exact_single_frame_video_duration(&eligible), Ok(0.04));

        let mut with_audio = eligible.clone();
        with_audio["streams"]
            .as_array_mut()
            .expect("stream array")
            .push(json!({ "codec_type": "audio" }));
        assert!(
            exact_single_frame_video_duration(&with_audio)
                .expect_err("audio stream must stay in VID")
                .contains("exactly one video stream")
        );

        let mut two_frames = eligible.clone();
        two_frames["streams"][0]["nb_read_frames"] = json!("2");
        assert!(
            exact_single_frame_video_duration(&two_frames)
                .expect_err("two frames must stay in VID")
                .contains("exactly one frame")
        );

        let mut unknown_frames = eligible;
        unknown_frames["streams"][0]["nb_read_frames"] = json!("N/A");
        assert!(
            exact_single_frame_video_duration(&unknown_frames)
                .expect_err("unknown frame count must fail closed")
                .contains("refusing to guess")
        );

        let stream_duration_only = json!({
            "format": {},
            "streams": [{
                "codec_type": "video",
                "width": 640,
                "height": 480,
                "duration": "0.040",
                "nb_frames": "1",
                "nb_read_frames": "1"
            }],
            "chapters": [],
            "programs": []
        });
        assert_eq!(
            exact_single_frame_video_duration(&stream_duration_only),
            Ok(0.04)
        );

        for missing_dimension in ["width", "height"] {
            let mut incomplete = stream_duration_only.clone();
            incomplete["streams"][0]
                .as_object_mut()
                .expect("video stream object")
                .remove(missing_dimension);
            assert!(
                exact_single_frame_video_duration(&incomplete)
                    .expect_err("missing canvas dimension must fail closed")
                    .contains("refusing to guess")
            );
        }
    }
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
    fn probe_duration_from_frame_count_and_fps_derives_seconds() {
        let secs =
            probe_duration_from_frame_count_and_fps(Some(48), Some(24.0), None).expect("duration");
        assert!((secs - 2.0).abs() < f64::EPSILON);
        assert!(probe_duration_from_frame_count_and_fps(None, Some(24.0), None).is_none());
        assert!(probe_duration_from_frame_count_and_fps(Some(10), None, None).is_none());
    }

    #[test]
    fn duration_probe_scopes_apng_fallback_to_png() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing_heic = dir.path().join("missing.heic");
        let stream = serde_json::json!({});

        let duration = resolve_probe_duration(None, &stream, "heic", &missing_heic)
            .expect("HEIC without duration must not enter APNG parsing");

        assert_eq!(duration, None);

        let apng = dir.path().join("timed.png");
        std::fs::write(
            &apng,
            crate::image_detection::synthetic_two_frame_apng_for_test(),
        )
        .expect("write synthetic APNG");
        let duration = resolve_probe_duration(None, &stream, "apng", &apng)
            .expect("APNG must retain native timing fallback")
            .expect("synthetic APNG duration");
        assert!((duration - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn test_detect_bit_depth() {
        let cases: &[(&str, Option<u8>)] = &[
            ("yuv420p", Some(8)),
            ("yuv422p", Some(8)),
            ("yuv444p", Some(8)),
            ("rgb24", Some(8)),
            ("bgr24", Some(8)),
            ("nv12", Some(8)),
            ("yuv420p10le", Some(10)),
            ("gbrp12le", Some(12)),
            ("yuv420p9le", Some(9)),
            ("gbrpf32le", Some(32)),
            ("monow", None),
            ("unknown", None),
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
    fn parse_video_stream_fields_prefers_explicit_stream_bit_depth() {
        let stream = serde_json::json!({
            "codec_name": "prores",
            "codec_long_name": "Apple ProRes",
            "r_frame_rate": "24/1",
            "avg_frame_rate": "24/1",
            "width": 1920,
            "height": 1080,
            "pix_fmt": "yuv420p",
            "bits_per_raw_sample": "12",
            "has_b_frames": 0
        });

        let fields = parse_video_stream_fields(
            &stream,
            "mov,mp4,m4a,3gp,3g2,mj2",
            Some(1.0),
            std::path::Path::new("test.mov"),
        )
        .expect("stream fields should parse");

        assert_eq!(fields.bit_depth, Some(12));
        assert!(!fields.bit_depth_inferred_from_pix_fmt);
    }

    #[test]
    fn parse_video_stream_fields_marks_pix_fmt_inferred_bit_depth() {
        let stream = serde_json::json!({
            "codec_name": "hevc",
            "codec_long_name": "H.265 / HEVC",
            "r_frame_rate": "24/1",
            "avg_frame_rate": "24/1",
            "width": 3840,
            "height": 2160,
            "pix_fmt": "yuv420p10le",
            "has_b_frames": 0
        });

        let fields = parse_video_stream_fields(
            &stream,
            "mov,mp4,m4a,3gp,3g2,mj2",
            Some(1.0),
            std::path::Path::new("test.hevc"),
        )
        .expect("stream fields should parse");

        assert_eq!(fields.bit_depth, Some(10));
        assert!(fields.bit_depth_inferred_from_pix_fmt);
    }

    #[test]
    fn ffprobe_result_tracks_confirmed_vs_effective_bit_depth() {
        let inferred = FFprobeResult {
            format_name: "mov".to_string(),
            duration: Some(1.0),
            size: 1,
            bit_rate: None,
            video_codec: "hevc".to_string(),
            video_codec_long: "H.265 / HEVC".to_string(),
            width: 3840,
            height: 2160,
            frame_rate: Some(24.0),
            avg_frame_rate: Some(24.0),
            frame_count: Some(24),
            pix_fmt: "yuv420p10le".to_string(),
            color_space: Some("bt709".to_string()),
            color_transfer: None,
            color_primaries: Some("bt709".to_string()),
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: true,
            audio: FFprobeAudioInfo::default(),
            profile: None,
            level: None,
            max_b_frames: Some(0),
            encoder_settings: None,
            video_bit_rate: None,
            refs: None,
            hdr: FFprobeHdrInfo::default(),
            subtitles: FFprobeSubtitleInfo::default(),
            is_variable_frame_rate: false,
            stream_index: 0,
            tags: std::collections::HashMap::new(),
            loop_count: None,
            frame_types: Vec::new(),
            pts_deltas: Vec::new(),
            mv_magnitudes: Vec::new(),
            pkt_sizes: Vec::new(),
        };

        assert_eq!(inferred.effective_bit_depth(), Some(10));
        assert_eq!(inferred.confirmed_bit_depth(), None);
        assert!(inferred.should_preserve_high_bit_depth());
        assert!(!inferred.is_hdr());
    }

    #[test]
    fn ffprobe_result_treats_pq_transfer_as_hdr_for_preservation() {
        let hdr_probe = FFprobeResult {
            format_name: "mov".to_string(),
            duration: Some(1.0),
            size: 1,
            bit_rate: None,
            video_codec: "hevc".to_string(),
            video_codec_long: "H.265 / HEVC".to_string(),
            width: 3840,
            height: 2160,
            frame_rate: Some(24.0),
            avg_frame_rate: Some(24.0),
            frame_count: Some(24),
            pix_fmt: "yuv420p".to_string(),
            color_space: Some("bt2020nc".to_string()),
            color_transfer: Some(crate::constants::HDR_TRANSFER_PQ.to_string()),
            color_primaries: Some("bt2020".to_string()),
            bit_depth: None,
            bit_depth_inferred_from_pix_fmt: false,
            audio: FFprobeAudioInfo::default(),
            profile: None,
            level: None,
            max_b_frames: Some(0),
            encoder_settings: None,
            video_bit_rate: None,
            refs: None,
            hdr: FFprobeHdrInfo::default(),
            subtitles: FFprobeSubtitleInfo::default(),
            is_variable_frame_rate: false,
            stream_index: 0,
            tags: std::collections::HashMap::new(),
            loop_count: None,
            frame_types: Vec::new(),
            pts_deltas: Vec::new(),
            mv_magnitudes: Vec::new(),
            pkt_sizes: Vec::new(),
        };

        assert!(hdr_probe.is_hdr());
        assert!(hdr_probe.should_preserve_high_bit_depth());
    }

    #[test]
    fn ffprobe_result_color_assessment_reuses_shared_signal_and_float_logic() {
        let probe = FFprobeResult {
            format_name: "mov".to_string(),
            duration: Some(1.0),
            size: 1,
            bit_rate: None,
            video_codec: "hevc".to_string(),
            video_codec_long: "H.265 / HEVC".to_string(),
            width: 3840,
            height: 2160,
            frame_rate: Some(24.0),
            avg_frame_rate: Some(24.0),
            frame_count: Some(24),
            pix_fmt: "gbrpf32le".to_string(),
            color_space: Some("bt2020nc".to_string()),
            color_transfer: Some(crate::constants::HDR_TRANSFER_HLG.to_string()),
            color_primaries: Some("bt2020".to_string()),
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: false,
            audio: FFprobeAudioInfo::default(),
            profile: None,
            level: None,
            max_b_frames: Some(0),
            encoder_settings: None,
            video_bit_rate: None,
            refs: None,
            hdr: FFprobeHdrInfo {
                hdr10_plus: true,
                ..Default::default()
            },
            subtitles: FFprobeSubtitleInfo::default(),
            is_variable_frame_rate: false,
            stream_index: 0,
            tags: std::collections::HashMap::new(),
            loop_count: None,
            frame_types: Vec::new(),
            pts_deltas: Vec::new(),
            mv_magnitudes: Vec::new(),
            pkt_sizes: Vec::new(),
        };

        let assessment = probe.color_assessment();
        assert_eq!(
            assessment.hdr_signal(),
            Some(crate::ffprobe_json::HdrSignalKind::Hdr10Plus)
        );
        assert!(assessment.is_float());
        assert!(assessment.has_wide_gamut_signal());
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
            std::path::Path::new("loop-count-fixture.mov"),
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
            std::path::Path::new("loop-count-fixture.mov"),
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
        let path = Path::new("/tmp/sample.webp");
        let result = parse_probe_format(&format, path);
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
        let path = Path::new("/tmp/sample.avif");
        let result = parse_probe_format(&format, path);
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
        let path = Path::new("/tmp/sample.webp");
        let result = parse_probe_format(&format, path);
        assert!(result.is_ok());
        assert!(result.unwrap().duration.is_none());
    }

    /// Generic muxer name + still-image extension must not require
    /// format.duration.
    #[test]
    fn parse_probe_format_still_image_extension_without_duration_is_ok() {
        let format = serde_json::json!({
            "format_name": "mov",
            "size": "204800"
        });
        let path = Path::new("/tmp/photo.jxl");
        let result = parse_probe_format(&format, path);
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
        let result = select_video_stream(&streams, None);
        assert!(
            result.is_ok(),
            "select_video_stream must not panic with 0x0 stream: {result:?}"
        );
    }

    /// `nb_frames` absent from stream — must not panic in
    /// `select_video_stream`.
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
        let result = select_video_stream(&streams, None);
        assert!(
            result.is_ok(),
            "absent nb_frames must not panic: {result:?}"
        );
    }

    /// `has_b_frames` absent — `parse_video_stream_fields` must not panic.
    #[test]
    fn parse_video_stream_fields_absent_has_b_frames_does_not_panic() {
        // Test the lower-level parse_video_stream_fields directly.
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
        let file = tempfile::NamedTempFile::new().expect("temp webp");
        std::fs::write(file.path(), b"RIFF\x00\x00\x00\x00WEBPVP8 ")
            .expect("write static webp-like header");
        let result = parse_video_stream_fields(&stream, "webp_pipe", Some(0.04_f64), file.path());
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

    #[test]
    fn extract_hdr_side_data_accepts_direct_stream_objects() {
        let stream_json = serde_json::json!({
            "side_data_list": [{
                "side_data_type": "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
            }]
        });

        let hdr = extract_hdr_side_data(&stream_json);
        assert!(hdr.hdr10_plus);
        assert!(hdr.has_explicit_hdr_metadata());
    }

    /// CONTRACT: frame `side_data_list` must be present in ffprobe JSON (see
    /// `run_ffprobe_json`). Generic unregistered SEI alone is not treated
    /// as HDR10+.
    #[test]
    fn contract_ffprobe_frame_show_entries_includes_side_data_list() {
        assert!(
            crate::constants::FFPROBE_FRAME_SHOW_ENTRIES.contains("side_data_list"),
            "CONTRACT: FFPROBE_FRAME_SHOW_ENTRIES must include side_data_list for HDR10+"
        );
    }

    #[test]
    fn contract_hdr10_plus_requires_typed_frame_side_data_not_generic_sei_only() {
        let generic_sei_only = serde_json::json!({
            "streams": [{
                "side_data_list": [{
                    "side_data_type": "Mastering display metadata"
                }]
            }],
            "frames": [{
                "side_data_list": [{
                    "side_data_type": "H.26[45] User Data Unregistered SEI message"
                }]
            }]
        });
        assert!(!extract_hdr_side_data(&generic_sei_only).hdr10_plus);

        let typed_hdr10_plus = serde_json::json!({
            "frames": [{
                "side_data_list": [{
                    "side_data_type": "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
                }]
            }]
        });
        assert!(extract_hdr_side_data(&typed_hdr10_plus).hdr10_plus);
    }

    #[test]
    fn parse_video_stream_fields_gif_native_frame_override_m126() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("anim.gif");
        let mut gif_data = Vec::new();
        {
            let mut encoder = ::gif::Encoder::new(&mut gif_data, 10, 8, &[0, 0, 0, 255, 255, 255])
                .expect("gif encoder");
            let f0 = [0u8];
            let f1 = [1u8];
            encoder
                .write_frame(&::gif::Frame {
                    delay: 10,
                    width: 1,
                    height: 1,
                    buffer: std::borrow::Cow::Borrowed(&f0),
                    ..Default::default()
                })
                .expect("frame 0");
            encoder
                .write_frame(&::gif::Frame {
                    delay: 20,
                    width: 1,
                    height: 1,
                    buffer: std::borrow::Cow::Borrowed(&f1),
                    ..Default::default()
                })
                .expect("frame 1");
        }
        std::fs::File::create(&path)
            .expect("create gif")
            .write_all(&gif_data)
            .expect("write gif");

        let stream = serde_json::json!({
            "codec_name": "gif",
            "width": 10,
            "height": 8,
            "nb_frames": "1"
        });
        let fields = super::parse_video_stream_fields(&stream, "gif", None, &path)
            .expect("parse gif stream");
        assert_eq!(fields.frame_count, Some(2));
    }

    #[test]
    fn parse_video_stream_fields_gif_native_read_errors_are_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing.gif");
        let stream = serde_json::json!({
            "codec_name": "gif",
            "width": 10,
            "height": 8,
            "nb_frames": "1"
        });

        let err = super::parse_video_stream_fields(&stream, "gif", None, &path)
            .expect_err("missing GIF native recovery path must be an error");

        assert!(err.to_string().contains("missing.gif"));
    }

    #[test]
    fn parse_video_stream_fields_apng_native_frame_override_m126() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("anim.png");
        let data = crate::image_detection::synthetic_two_frame_apng_for_test();
        std::fs::File::create(&path)
            .expect("create png")
            .write_all(&data)
            .expect("write png");

        let stream = serde_json::json!({
            "codec_name": "png",
            "width": 1,
            "height": 1,
            "nb_frames": "1"
        });
        let fields = super::parse_video_stream_fields(&stream, "png", None, &path)
            .expect("parse apng stream");
        assert_eq!(fields.frame_count, Some(2));
    }

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
        let result = select_video_stream(&streams, None);
        // Should succeed by picking the only available video stream, even if 0x0
        assert!(
            result.is_ok(),
            "select_video_stream must fallback to picking the stream even if dimensions are 0"
        );
        let (idx, stream) = result.expect("select_video_stream should succeed in test");
        assert_eq!(idx, 0);
        assert_eq!(stream["codec_name"], "webp");
    }

    #[test]
    fn parse_video_stream_fields_webp_zero_dims_recovers_from_bitstream() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("safari_anim.webp");
        let data = crate::image_formats::webp::synthetic_two_frame_animated_webp_for_test();
        std::fs::write(&path, &data).expect("write webp");

        let stream = serde_json::json!({
            "codec_name": "webp",
            "width": 0,
            "height": 0,
            "r_frame_rate": "25/1",
            "avg_frame_rate": "0/0"
        });
        let fields = super::parse_video_stream_fields(&stream, "webp_pipe", None, &path)
            .expect("animated WebP must recover canvas from RIFF when ffprobe reports 0x0");
        assert!(fields.width > 0 && fields.height > 0);
        assert_eq!(fields.frame_count, Some(2));
    }
}
