//! Video precheck and processing recommendation

use crate::FfprobeBuilder;
use crate::builder_base::ToolBuilder;
use crate::quality_matcher::parse_source_codec;
use crate::unified_error::UnifiedError;
use anyhow::{Context, Result, bail};
use rug::Rational;
use std::path::Path;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compressibility {
    VeryHigh,
    High,
    Medium,
    Low,
    VeryLow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessingRecommendation {
    StronglyRecommended { codec: String, reason: String },
    Recommended { reason: String },
    Optional { reason: String },
    NotRecommended { codec: String, reason: String },
    CannotProcess { reason: String },
}

#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
    pub duration: f64,
    pub fps: f64,
    pub file_size: u64,
    pub bitrate_kbps: f64,
    pub bpp: f64,
    pub codec: String,
    pub compressibility: Compressibility,
    pub recommendation: ProcessingRecommendation,
    pub color_space: Option<String>,
    pub pix_fmt: Option<String>,
    pub bit_depth: Option<u8>,
    pub fps_category: FpsCategory,
    pub is_hdr: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpsCategory {
    Normal,
    Extended,
    Extreme,
    Invalid,
}

impl FpsCategory {
    #[must_use]
    pub fn from_fps(fps: f64) -> Self {
        if fps <= 0.0 || fps > FPS_THRESHOLD_INVALID {
            Self::Invalid
        } else if fps < FPS_RANGE_NORMAL.1 {
            Self::Normal
        } else if fps <= FPS_RANGE_EXTENDED.1 {
            Self::Extended
        } else if fps <= FPS_RANGE_EXTREME.1 {
            Self::Extreme
        } else {
            Self::Invalid
        }
    }

    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::Normal => "normal range (1–239 fps)",
            Self::Extended => "extended range (240–2000 fps)",
            Self::Extreme => "extreme range (2000-10000 fps)",
            Self::Invalid => "invalid (>10000 fps, possible metadata error)",
        }
    }

    #[must_use]
    pub const fn is_valid(&self) -> bool {
        !matches!(self, Self::Invalid)
    }
}

const LEGACY_CODECS_STRONGLY_RECOMMENDED: &[&str] = &[
    "theora",
    "rv30",
    "rv40",
    "realvideo",
    "vp6",
    "vp7",
    "wmv1",
    "wmv2",
    "wmv3",
    "msmpeg4v1",
    "msmpeg4v2",
    "msmpeg4v3",
    "cinepak",
    "indeo",
    "iv31",
    "iv32",
    "iv41",
    "iv50",
    "svq1",
    "svq3",
    "flv1",
    "msvideo1",
    "msrle",
    "8bps",
    "qtrle",
    "rpza",
    "mjpeg",
    "mjpegb",
    // huffyuv omitted: lossless codec; video_quality_detector routes to FFV1, not "strongly upgrade to lossy"
];

const OPTIMAL_CODECS: &[&str] = &["hevc", "h265", "x265", "hvc1", "av1", "av01", "libaom-av1"];

const FPS_RANGE_NORMAL: (f64, f64) = crate::constants::PRECHECK_FPS_RANGE_NORMAL;
const FPS_RANGE_EXTENDED: (f64, f64) = crate::constants::PRECHECK_FPS_RANGE_EXTENDED;
const FPS_RANGE_EXTREME: (f64, f64) = crate::constants::PRECHECK_FPS_RANGE_EXTREME;
const FPS_THRESHOLD_INVALID: f64 = crate::constants::PRECHECK_FPS_THRESHOLD_INVALID;

/// Single ffprobe run for precheck: stream (codec, size, duration, fps, `bit_rate`, color) + format.duration.
fn run_precheck_ffprobe(input: &Path) -> Result<serde_json::Value> {
    let output = FfprobeBuilder::new()
        .input(input)
        .arg("-v")
        .arg("error")
        .select_stream(crate::ffmpeg_builder::StreamType::Video, 0)
        .arg("-count_frames")
        .arg("-show_entries")
        .arg("stream=codec_name,width,height,r_frame_rate,avg_frame_rate,duration,nb_frames,nb_read_frames,bit_rate,color_space,color_transfer,pix_fmt,bits_per_raw_sample")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("json")
        .build()
        .output()
        .context("ffprobe failed")?;

    if !output.status.success() {
        bail!("ffprobe failed");
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("ffprobe JSON parse failed")?;
    Ok(json)
}

/// Parse rational "num/den" or plain number from JSON.
fn parse_rational_fps(value: &serde_json::Value) -> Option<f64> {
    value.as_str().and_then(|s| {
        let parts: Vec<&str> = s.split('/').collect();
        match parts[..] {
            [num_str, den_str] => {
                let num: f64 = crate::numeric_cast::parse_strict(num_str, "fps_num")?;
                let den: f64 = crate::numeric_cast::parse_strict(den_str, "fps_den")?;
                if den > 0.0_f64 { Some(num / den) } else { None }
            }
            _ => crate::numeric_cast::parse_strict(s, "fps_plain"),
        }
    })
}

/// Prefer `avg_frame_rate` (actual frames per second); fallback to `r_frame_rate`.
/// `r_frame_rate` can be the `time_base` reciprocal (e.g. 90000) rather than real FPS — callers
/// should use `fps_sanitise_for_validation` when fps may be `time_base`.
fn parse_fps_from_stream(stream: &serde_json::Value) -> Option<f64> {
    let avg = parse_rational_fps(&stream["avg_frame_rate"])
        .filter(|&v| v > 0.0_f64 && v.is_finite() && v <= FPS_THRESHOLD_INVALID);
    let r_fps =
        parse_rational_fps(&stream["r_frame_rate"]).filter(|&v| v > 0.0_f64 && v.is_finite());
    avg.or(r_fps)
}

/// If fps looks like `time_base` (e.g. 90000) rather than real FPS, derive from `frame_count/duration`.
fn fps_sanitise_for_validation(fps: f64, duration: f64, frame_count: u64) -> f64 {
    if fps > FPS_THRESHOLD_INVALID
        && frame_count > 0
        && duration >= crate::constants::DURATION_MIN_VALID
    {
        let inferred = crate::numeric_cast::u64_to_f64(frame_count) / duration;
        if inferred > 0.0_f64 && inferred <= FPS_THRESHOLD_INVALID {
            return inferred;
        }
    }
    fps
}

fn parse_duration_from_precheck_json(
    json: &serde_json::Value,
    fps: f64,
    mut frame_count: u64,
    input: &Path,
) -> Result<(f64, f64, u64)> {
    let stream = json["streams"].get(0);

    // If frame_count is 0, try to get nb_read_frames (for formats like APNG that need -count_frames)
    if frame_count == 0
        && let Some(nb_read_frames) = crate::numeric_cast::parse_option_strict(
            stream.and_then(|s| s["nb_read_frames"].as_str()),
            "nb_read_frames",
        )
    {
        frame_count = nb_read_frames;
        info!(
            nb_read_frames = frame_count,
            "Using nb_read_frames for frame count"
        );
    }

    let stream_duration: Option<f64> = crate::numeric_cast::parse_option_strict(
        stream.and_then(|s| s["duration"].as_str()),
        "stream_duration",
    )
    .filter(|&d: &f64| d > 0.0_f64 && !d.is_nan());

    if let Some(duration) = stream_duration {
        return Ok((duration, fps, frame_count));
    }

    warn!("DURATION: stream.duration unavailable, trying format.duration");
    let format_duration: Option<f64> = json
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| crate::numeric_cast::parse_strict(s, "numeric_field"))
        .filter(|&d: &f64| d > 0.0_f64 && !d.is_nan());

    if let Some(duration) = format_duration {
        info!(duration_secs = %duration, "DURATION RECOVERED via format.duration");
        return Ok((duration, fps, frame_count));
    }

    warn!("DURATION: format.duration failed, trying frame_count/fps");
    if frame_count > 0 && fps > 0.0_f64 && !fps.is_nan() {
        let duration = crate::numeric_cast::u64_to_f64(frame_count) / fps;
        if duration > 0.0_f64 {
            info!(duration_secs = %duration, frames = frame_count, fps = %fps, "DURATION RECOVERED via frame_count/fps");
            return Ok((duration, fps, frame_count));
        }
    }

    warn!("DURATION: frame_count/fps failed, trying ImageMagick (animated image fallback)");
    if let Some((duration_secs, frames)) =
        crate::image_analyzer::get_animation_duration_and_frames_imagemagick(input)
        && duration_secs > 0.0_f64
        && frames > 0
    {
        let inferred_fps = crate::numeric_cast::u64_to_f64(frames) / duration_secs;
        info!(duration_secs = %duration_secs, frames, fps = %inferred_fps, "DURATION RECOVERED via ImageMagick");
        return Ok((duration_secs, inferred_fps, frames));
    }

    error!(file = %input.display(), "DURATION DETECTION FAILED - Cannot determine video duration");
    Err(UnifiedError::ResultAnomaly(
        "Failed to detect video duration - all methods failed".to_string(),
    )
    .into())
}

/// P3: Compute only BPP from precheck JSON (one ffprobe, no full `VideoInfo`).
fn bpp_from_precheck_json(json: &serde_json::Value, file_size: u64, input: &Path) -> Result<f64> {
    let stream = json
        .get("streams")
        .and_then(|s| s.get(0))
        .context("No video stream in ffprobe output")?;
    let width: u32 = stream["width"]
        .as_u64()
        .and_then(|w| crate::numeric_cast::u64_to_u32_strict(w, "dimension"))
        .context("Missing or invalid video width")?;
    let height: u32 = stream["height"]
        .as_u64()
        .and_then(|h| crate::numeric_cast::u64_to_u32_strict(h, "dimension"))
        .context("Missing or invalid video height")?;
    let fps =
        parse_fps_from_stream(stream).context("Could not determine FPS for BPP calculation")?;
    // `nb_frames` may be absent or non-numeric for image containers (e.g. WebP, APNG).
    // 0 is the correct initial value here — downstream `parse_duration_from_precheck_json`
    // re-derives frame_count from duration*fps when nb_frames == 0.
    let frame_count_raw: u64 = crate::numeric_cast::parse_option_strict(
        stream["nb_frames"].as_str(),
        "nb_frames_str",
    )
    .or_else(|| stream["nb_frames"].as_u64())
    .unwrap_or_else(|| {
        tracing::warn!(
            path = %input.display(),
            "ffprobe bpp_from_precheck_json: nb_frames absent or non-numeric; using 0 (will re-derive from duration)"
        );
        0
    });
    let (duration, fps, frame_count_raw) =
        parse_duration_from_precheck_json(json, fps, frame_count_raw, input)?;
    let fps = fps_sanitise_for_validation(fps, duration, frame_count_raw);
    let frame_count = if frame_count_raw == 0 && duration > 0.0_f64 {
        crate::numeric_cast::f64_to_u64_sat(duration * fps)
    } else {
        frame_count_raw.max(1)
    };
    let video_bytes =
        crate::numeric_cast::parse_option_strict(stream["bit_rate"].as_str(), "bit_rate")
            .filter(|&br| br > 0)
            .map_or(0, |br| {
                crate::numeric_cast::f64_to_u64_sat(
                    crate::numeric_cast::u64_to_f64(br) * duration / 8.0,
                )
            });
    let bytes_for_bpp = if video_bytes > 0 {
        video_bytes
    } else {
        file_size
    };
    // Calculate total pixels with high precision when available
    #[cfg(feature = "high-precision")]
    {
        use rug::Integer;

        let total_pixels_u64 = u64::from(width) * u64::from(height) * frame_count;
        if total_pixels_u64 > 0 {
            let total_pixels_int = Integer::from(total_pixels_u64);
            let bits_int = Integer::from(bytes_for_bpp) * Integer::from(8);
            let bpp = Rational::from(bits_int) / Rational::from(total_pixels_int);
            Ok(bpp.to_f64())
        } else {
            bail!("Total pixels is 0, cannot calculate BPP")
        }
    }

    #[cfg(not(feature = "high-precision"))]
    {
        let total_pixels = u64::from(width) * u64::from(height) * frame_count;
        if total_pixels > 0 {
            let bpp = (Rational::from(bytes_for_bpp) * Rational::from(8))
                / Rational::from(total_pixels.max(1));
            Ok(bpp.to_f64())
        } else {
            bail!("Total pixels is 0, cannot calculate BPP")
        }
    }
}

/// Detect video duration comprehensively using multiple methods.
///
/// # Errors
/// Returns an error if duration detection fails.
pub fn detect_duration_comprehensive(input: &Path) -> Result<(f64, f64, u64, &'static str)> {
    let output = FfprobeBuilder::new()
        .input(input)
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=r_frame_rate,avg_frame_rate,duration,nb_frames")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("json")
        .build()
        .output()
        .context("ffprobe failed")?;

    if !output.status.success() {
        bail!("ffprobe failed to get duration");
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&json_str).context("ffprobe JSON parse failed")?;

    let stream = json
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .context("No video stream")?;
    let fps: f64 = parse_fps_from_stream(stream)
        .context("Could not determine FPS for duration calculation")?;

    let frame_count: u64 = json
        .get("streams")
        .and_then(|s| s.get(0))
        .and_then(|s| s.get("nb_frames"))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| crate::numeric_cast::parse_strict(s, "nb_frames"))
        .unwrap_or_else(|| {
            tracing::warn!(
                path = %input.display(),
                "detect_duration_comprehensive: nb_frames absent or non-numeric; using 0"
            );
            0
        });

    let stream_duration: Option<f64> = crate::numeric_cast::parse_option_strict(
        json.get("streams")
            .and_then(|s| s.get(0))
            .and_then(|s| s.get("duration"))
            .and_then(serde_json::Value::as_str),
        "stream_duration",
    )
    .filter(|&d: &f64| d > 0.0_f64 && !d.is_nan());

    if let Some(duration) = stream_duration {
        let fps = fps_sanitise_for_validation(fps, duration, frame_count);
        return Ok((duration, fps, frame_count, "stream.duration"));
    }

    warn!("DURATION: stream.duration unavailable, trying format.duration");
    let format_duration: Option<f64> = json
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| crate::numeric_cast::parse_strict(s, "numeric_field"))
        .filter(|&d: &f64| d > 0.0_f64 && !d.is_nan());

    if let Some(duration) = format_duration {
        info!(duration_secs = %duration, "DURATION RECOVERED via format.duration");
        let fps = fps_sanitise_for_validation(fps, duration, frame_count);
        return Ok((duration, fps, frame_count, "format.duration"));
    }

    warn!("DURATION: format.duration failed, trying frame_count/fps");
    if frame_count > 0 && fps > 0.0_f64 && !fps.is_nan() && fps <= FPS_THRESHOLD_INVALID {
        let duration = crate::numeric_cast::u64_to_f64(frame_count) / fps;
        if duration > 0.0_f64 {
            info!(duration_secs = %duration, frames = frame_count, fps = %fps, "DURATION RECOVERED via frame_count/fps");
            return Ok((duration, fps, frame_count, "frame_count/fps"));
        }
    }

    warn!("DURATION: frame_count/fps failed, trying ImageMagick (animated image fallback)");
    if let Some((duration_secs, frames)) =
        crate::image_analyzer::get_animation_duration_and_frames_imagemagick(input)
        && duration_secs > 0.0_f64
        && frames > 0
    {
        let inferred_fps = crate::numeric_cast::u64_to_f64(frames) / duration_secs;
        info!(duration_secs = %duration_secs, frames, fps = %inferred_fps, "DURATION RECOVERED via ImageMagick");
        return Ok((duration_secs, inferred_fps, frames, "imagemagick"));
    }

    error!(file = %input.display(), "DURATION DETECTION FAILED - Cannot determine video duration");
    Err(UnifiedError::ResultAnomaly(
        "Failed to detect video duration - all methods failed".to_string(),
    )
    .into())
}

/// Get comprehensive video information for a file.
///
/// # Errors
/// Returns an error if information gathering fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn get_video_info(input: &Path) -> Result<VideoInfo> {
    let file_size = crate::io_utils::metadata_with_retry(input)
        .context("Failed to read file metadata")?
        .len();

    let json = run_precheck_ffprobe(input)?;
    let stream = json
        .get("streams")
        .and_then(|s| s.get(0))
        .context("No video stream in ffprobe output")?;

    let codec = stream["codec_name"]
        .as_str()
        .unwrap_or("")
        .to_string()
        .to_lowercase();
    if codec.is_empty() {
        return Err(UnifiedError::ResultAnomaly("Could not detect video codec".to_string()).into());
    }

    let width: u32 = stream["width"]
        .as_u64()
        .and_then(|w| crate::numeric_cast::u64_to_u32_strict(w, "dimension"))
        .context("Missing or invalid video width")?;
    let height: u32 = stream["height"]
        .as_u64()
        .and_then(|h| crate::numeric_cast::u64_to_u32_strict(h, "dimension"))
        .context("Missing or invalid video height")?;

    // Fallback for formats where ffprobe returns 0x0 (e.g., animated WebP)
    let (width, height) = if width == 0 || height == 0 {
        crate::conversion::get_input_dimensions(input)
            .map_err(|e| anyhow::anyhow!("Failed to get dimensions via fallback: {e}"))?
    } else {
        (width, height)
    };

    let fps = parse_fps_from_stream(stream).context("Could not determine FPS for video info")?;
    // `nb_frames` may be absent for image containers; 0 triggers re-derivation from duration*fps.
    let frame_count_raw: u64 = crate::numeric_cast::parse_option_strict(
        stream["nb_frames"].as_str(),
        "nb_frames_str",
    )
    .or_else(|| stream["nb_frames"].as_u64())
        .unwrap_or_else(|| {
            tracing::warn!(
                path = %input.display(),
                "get_video_info: nb_frames absent or non-numeric; using 0 (will re-derive from duration)"
            );
            0
        });

    let (duration, fps, frame_count_raw) =
        parse_duration_from_precheck_json(&json, fps, frame_count_raw, input)?;
    let fps = fps_sanitise_for_validation(fps, duration, frame_count_raw);
    let frame_count = if frame_count_raw == 0 && duration > 0.0_f64 {
        crate::numeric_cast::f64_to_u64_sat(duration * fps)
    } else {
        frame_count_raw.max(1)
    };

    let bitrate_kbps =
        crate::numeric_cast::parse_option_strict::<f64>(stream["bit_rate"].as_str(), "bit_rate")
            .map_or_else(
                || {
                    if duration > 0.0_f64 {
                        (crate::numeric_cast::u64_to_f64(file_size) * 8.0_f64)
                            / (duration * 1_000.0_f64)
                    } else {
                        0.0_f64
                    }
                },
                |bps: f64| bps / 1_000.0_f64,
            );

    let video_bytes =
        crate::numeric_cast::parse_option_strict(stream["bit_rate"].as_str(), "bit_rate_u64")
            .filter(|&br| br > 0)
            .map_or(0, |br| {
                crate::numeric_cast::f64_to_u64_sat(
                    crate::numeric_cast::u64_to_f64(br) * duration / 8.0,
                )
            });
    let bytes_for_bpp = if video_bytes > 0 {
        video_bytes
    } else {
        file_size
    };

    // Calculate total pixels with high precision when available
    let bpp = if cfg!(feature = "high-precision") && !cfg!(feature = "ci-static-build") {
        #[cfg(feature = "high-precision")]
        {
            let total_pixels_u64 = u64::from(width) * u64::from(height) * frame_count;
            if total_pixels_u64 > 0 {
                let total_pixels_int = rug::Integer::from(total_pixels_u64);
                let bits_int = rug::Integer::from(bytes_for_bpp) * rug::Integer::from(8);
                let bpp_rational = Rational::from(bits_int) / Rational::from(total_pixels_int);
                bpp_rational.to_f64()
            } else {
                bail!("Total pixels is 0, cannot calculate BPP");
            }
        }
        #[cfg(not(feature = "high-precision"))]
        {
            let total_pixels = u64::from(width) * u64::from(height) * frame_count;
            if total_pixels > 0 {
                ((Rational::from(bytes_for_bpp) * Rational::from(8_i32))
                    / Rational::from(total_pixels.max(1)))
                .to_f64()
            } else {
                bail!("Total pixels is 0, cannot calculate BPP");
            }
        }
    } else {
        let total_pixels = u64::from(width) * u64::from(height) * frame_count;
        if total_pixels > 0 {
            ((Rational::from(bytes_for_bpp) * Rational::from(8_i32))
                / Rational::from(total_pixels.max(1)))
            .to_f64()
        } else {
            bail!("Total pixels is 0, cannot calculate BPP");
        }
    };

    // ... (rest of the code remains the same)
    let _recommendation =
        evaluate_processing_recommendation(&codec, width, height, duration, fps, bitrate_kbps, bpp);

    let source_codec_enum = parse_source_codec(&codec);

    let compressibility = if source_codec_enum.is_modern() {
        Compressibility::VeryLow
    } else if codec.contains("theora")
        || codec.contains("rv")
        || codec.contains("real")
        || codec.contains("mjpeg")
        || codec.contains("cinepak")
        || codec.contains("indeo")
        || codec.contains("gif")
        || bpp > 0.50_f64
    {
        Compressibility::VeryHigh
    } else if bpp > 0.30_f64 {
        Compressibility::High
    } else if bpp < 0.15_f64 {
        Compressibility::Low
    } else {
        Compressibility::Medium
    };

    let recommendation =
        evaluate_processing_recommendation(&codec, width, height, duration, fps, bitrate_kbps, bpp);

    let color_space = stream["color_space"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(String::from);
    let color_transfer = stream["color_transfer"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(String::from);
    let pix_fmt = stream["pix_fmt"].as_str().map(String::from);
    let bit_depth = crate::numeric_cast::parse_option_strict(
        stream["bits_per_raw_sample"].as_str(),
        "bits_per_raw_sample",
    );

    let fps_category = FpsCategory::from_fps(fps);

    // HDR: require BT.2020 (or 2020) and PQ/HLG transfer; 10-bit alone is not HDR (ProRes/DPX SDR).
    let is_hdr = color_space
        .as_ref()
        .is_some_and(|cs| cs.contains("bt2020") || cs.contains("2020"))
        && color_transfer.as_ref().is_some_and(|t| {
            let lower = t.to_lowercase();
            lower.contains("smpte2084")
                || lower.contains("arib-std-b67")
                || lower.contains("pq")
                || lower.contains("hlg")
        });

    Ok(VideoInfo {
        width,
        height,
        frame_count,
        duration,
        fps,
        file_size,
        bitrate_kbps,
        bpp,
        codec,
        compressibility,
        recommendation,
        color_space,
        pix_fmt,
        bit_depth,
        fps_category,
        is_hdr,
    })
}

/// Caller must pass lowercase codec (e.g. from `get_video_info`).
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn evaluate_processing_recommendation(
    codec: &str,
    width: u32,
    height: u32,
    duration: f64,
    fps: f64,
    bitrate_kbps: f64,
    bpp: f64,
) -> ProcessingRecommendation {
    if width < 16 || height < 16 {
        return ProcessingRecommendation::CannotProcess {
            reason: format!("Resolution too small {width}x{height} (< 16px)"),
        };
    }
    if width > 16384 || height > 16384 {
        return ProcessingRecommendation::CannotProcess {
            reason: format!("Resolution too large {width}x{height} (> 16K)"),
        };
    }

    if duration < crate::constants::DURATION_MIN_VALID {
        return ProcessingRecommendation::CannotProcess {
            reason: format!(
                "Duration read as {duration:.3}s (possible metadata issue, will attempt conversion)"
            ),
        };
    }

    if fps <= 0.0_f64 {
        return ProcessingRecommendation::CannotProcess {
            reason: format!("Invalid FPS ({fps:.2})"),
        };
    }
    if fps > FPS_THRESHOLD_INVALID {
        return ProcessingRecommendation::CannotProcess {
            reason: format!(
                "Abnormal FPS ({fps:.0} > {FPS_THRESHOLD_INVALID}, likely metadata error)"
            ),
        };
    }

    if LEGACY_CODECS_STRONGLY_RECOMMENDED
        .iter()
        .any(|&c| codec.contains(c))
    {
        let codec_category = if codec.contains("theora") {
            "Theora (Open Source, WebM predecessor)"
        } else if codec.contains("rv") || codec.contains("real") {
            "RealVideo (Legacy streaming standard)"
        } else if codec.contains("vp6") || codec.contains("vp7") {
            "VP6/VP7 (Flash Video era)"
        } else if codec.contains("wmv") {
            "Windows Media Video"
        } else if codec.contains("cinepak") {
            "Cinepak (CD-ROM era)"
        } else if codec.contains("indeo") || codec.contains("iv") {
            "Intel Indeo"
        } else if codec.contains("svq") {
            "Sorenson Video (QuickTime)"
        } else if codec.contains("flv") {
            "Flash Video H.263"
        } else if codec.contains("mjpeg") {
            "Motion JPEG (Inefficient intra-frame only)"
        } else {
            "Legacy codec"
        };

        return ProcessingRecommendation::StronglyRecommended {
            codec: codec.to_string(),
            reason: format!(
                "Detected {codec_category}, strongly recommended to upgrade to modern codec (expect 10-50x better compression)"
            ),
        };
    }

    if OPTIMAL_CODECS.iter().any(|&c| codec.contains(c)) {
        return ProcessingRecommendation::NotRecommended {
            codec: codec.to_string(),
            reason: "File already uses modern codec (HEVC/AV1), re-encoding may cause quality loss"
                .to_string(),
        };
    }

    let source_codec = parse_source_codec(codec);
    let codec_efficiency = source_codec.efficiency_factor();

    // Calculate resolution factor with high precision when available
    let resolution_factor = if cfg!(feature = "high-precision")
        && !cfg!(feature = "ci-static-build")
    {
        #[cfg(feature = "high-precision")]
        {
            let resolution_u64 = u64::from(width) * u64::from(height);
            let reference_u64 = 1920_u64 * 1080_u64;
            Rational::from((resolution_u64, reference_u64))
        }
        #[cfg(not(feature = "high-precision"))]
        {
            (Rational::from(width) * Rational::from(height)) / Rational::from(1_920_i32 * 1_080_i32)
        }
    } else {
        (Rational::from(width) * Rational::from(height)) / Rational::from(1_920_i32 * 1_080_i32)
    };
    let Some(fps_r) = crate::numeric_cast::f64_to_rational_strict(fps, "fps") else {
        return ProcessingRecommendation::CannotProcess {
            reason: "Invalid FPS (NaN/Inf)".to_string(),
        };
    };
    let fps_factor = fps_r / Rational::from(30_i32);

    let Some(codec_efficiency_r) =
        crate::numeric_cast::f64_to_rational_strict(codec_efficiency, "codec_efficiency")
    else {
        return ProcessingRecommendation::CannotProcess {
            reason: "Invalid codec efficiency (NaN/Inf)".to_string(),
        };
    };

    let base_bitrate_1080p30_h264 = 2_500.0_f64;
    let base_bitrate_r =
        crate::numeric_cast::f64_to_rational_strict(base_bitrate_1080p30_h264, "base_bitrate")
            .expect("Base bitrate constant is strictly finite");

    let expected_min_bitrate =
        (base_bitrate_r * resolution_factor * fps_factor * codec_efficiency_r).to_f64();

    let bpp_threshold_very_low = 0.05_f64 / codec_efficiency;
    let bpp_threshold_low = 0.10_f64 / codec_efficiency;

    if bitrate_kbps > 0.0_f64
        && bitrate_kbps < expected_min_bitrate * 0.5_f64
        && bpp < bpp_threshold_very_low
    {
        return ProcessingRecommendation::Optional {
            reason: format!(
                "File already highly compressed (bitrate: {:.0} kbps < {:.0} kbps, BPP: {:.4} < {:.4}), \
                            limited gain expected",
                bitrate_kbps,
                expected_min_bitrate * 0.5,
                bpp,
                bpp_threshold_very_low
            ),
        };
    }

    if bitrate_kbps > 0.0_f64 && bitrate_kbps < expected_min_bitrate && bpp < bpp_threshold_low {
        return ProcessingRecommendation::Recommended {
            reason: format!(
                "File has some compression (bitrate: {bitrate_kbps:.0} kbps), but modern codecs can optimize further"
            ),
        };
    }

    ProcessingRecommendation::Recommended {
        reason: format!(
            "Standard codec ({codec}), suggest upgrading to HEVC/AV1 for better compression and quality"
        ),
    }
}

/// Returns bits-per-pixel from video stream (one ffprobe, minimal parse; P3 lightweight path).
/// Calculate the bits per pixel (BPP) for a video file.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_bpp(input: &Path) -> Result<f64> {
    let file_size = std::fs::metadata(input)
        .context("Failed to read file metadata")?
        .len();
    let json = run_precheck_ffprobe(input)?;
    bpp_from_precheck_json(&json, file_size, input)
}

pub fn print_precheck_report(info: &VideoInfo) {
    if !crate::progress_mode::is_verbose_mode() {
        return;
    }
    let mut lines = Vec::new();
    lines.push("┌─────────────────────────────────────────────────────".to_string());
    lines.push("│ Precheck Report v5.75".to_string());
    lines.push("├─────────────────────────────────────────────────────".to_string());
    lines.push(format!("│ Codec: {}", info.codec));
    lines.push(format!("│ Resolution: {}x{}", info.width, info.height));
    lines.push(format!(
        "│ Duration: {:.1}s ({} frames)",
        info.duration, info.frame_count
    ));
    lines.push(format!(
        "│ FPS: {:.2} {}",
        info.fps,
        info.fps_category.description()
    ));
    lines.push(format!(
        "│ File Size: {:.2} MB",
        crate::numeric_cast::u64_to_f64(info.file_size) / 1_024.0_f64 / 1_024.0_f64
    ));
    lines.push(format!("│ Bitrate: {:.0} kbps", info.bitrate_kbps));
    lines.push(format!("│ BPP: {:.4} bits/pixel", info.bpp));

    if info.color_space.is_some() || info.pix_fmt.is_some() || info.bit_depth.is_some() {
        lines.push("├─────────────────────────────────────────────────────".to_string());
        if let Some(ref cs) = info.color_space {
            let hdr_indicator = if info.is_hdr { " HDR" } else { "" };
            lines.push(format!("│ Color Space: {cs}{hdr_indicator}"));
        }
        if let Some(ref pf) = info.pix_fmt {
            lines.push(format!("│ Pixel Format: {pf}"));
        }
        if let Some(bd) = info.bit_depth {
            lines.push(format!("│ Bit Depth: {bd}-bit"));
        }
    }

    lines.push("├─────────────────────────────────────────────────────".to_string());
    match info.compressibility {
        Compressibility::VeryHigh => {
            lines.push("│ Compression Potential: VERY HIGH".to_string());
            lines.push("│    → Ancient codec or extremely high BPP".to_string());
            lines.push("│    → Expected 10-50x compression improvement!".to_string());
        }
        Compressibility::High => {
            lines.push("│ ✅ Compression Potential: High".to_string());
            lines.push("│    → Large compression space expected".to_string());
        }
        Compressibility::Medium => {
            lines.push("│ Compression Potential: Medium".to_string());
            lines.push("│    → Moderate compression potential".to_string());
        }
        Compressibility::Low => {
            lines.push("│ ⚠️  Compression Potential: Low".to_string());
            lines.push("│    → File already optimized".to_string());
        }
        Compressibility::VeryLow => {
            lines.push("│ Compression Potential: VERY LOW".to_string());
            lines.push("│    → Already using modern codec (HEVC/AV1)".to_string());
            lines.push("│    → Re-encoding may cause quality loss".to_string());
        }
    }

    lines.push("├─────────────────────────────────────────────────────".to_string());
    match &info.recommendation {
        ProcessingRecommendation::StronglyRecommended { codec, reason } => {
            lines.push("│ STRONGLY RECOMMENDED: Upgrade to modern codec!".to_string());
            lines.push(format!("│    → Source: {codec} (legacy/inefficient)"));
            lines.push(format!("│    → {reason}"));
        }
        ProcessingRecommendation::Recommended { reason } => {
            lines.push("│ ✅ RECOMMENDED: Convert to modern codec".to_string());
            lines.push(format!("│    → {reason}"));
        }
        ProcessingRecommendation::Optional { reason } => {
            lines.push("│ OPTIONAL: Marginal benefit expected".to_string());
            lines.push(format!("│    → {reason}"));
        }
        ProcessingRecommendation::NotRecommended { codec, reason } => {
            lines.push("│ ⚠️  NOT RECOMMENDED: Already optimal".to_string());
            lines.push(format!("│    → Codec: {codec}"));
            lines.push(format!("│    → {reason}"));
        }
        ProcessingRecommendation::CannotProcess { reason } => {
            lines.push("│ ❌ CANNOT PROCESS: File issue detected".to_string());
            lines.push(format!("│    → {reason}"));
        }
    }

    lines.push("└─────────────────────────────────────────────────────".to_string());
    for line in &lines {
        info!("{}", line);
    }
}

/// Run pre-exploration checks on a video file.
///
/// # Errors
/// Returns an error if precheck fails.
pub fn run_precheck(input: &Path) -> Result<VideoInfo> {
    let info = get_video_info(input)?;
    print_precheck_report(&info);

    match &info.recommendation {
        ProcessingRecommendation::CannotProcess { reason } => {
            warn!(reason = %reason, "PRECHECK: cannot process");
            bail!("Precheck cannot process this file: {reason}");
        }

        ProcessingRecommendation::NotRecommended { codec, reason } => {
            info!(codec = %codec, reason = %reason, "already modern codec (continuing anyway)");
        }

        ProcessingRecommendation::StronglyRecommended { codec, reason } => {
            info!(codec = %codec, reason = %reason, "EXCELLENT TARGET: legacy codec, will benefit from modern encoding");
        }

        ProcessingRecommendation::Recommended { .. }
        | ProcessingRecommendation::Optional { .. } => {}
    }

    Ok(info)
}
