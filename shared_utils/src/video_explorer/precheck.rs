//! Video precheck and processing recommendation

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Compressibility {
    VeryHigh,
    High,
    Medium,
    Low,
    VeryLow,
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FpsCategory {
    Normal,
    Extended,
    Extreme,
    Invalid,
}

impl FpsCategory {
    pub fn from_fps(fps: f64) -> Self {
        if fps <= 0.0 || fps > FPS_THRESHOLD_INVALID {
            FpsCategory::Invalid
        } else if fps < FPS_RANGE_NORMAL.1 {
            FpsCategory::Normal
        } else if fps <= FPS_RANGE_EXTENDED.1 {
            FpsCategory::Extended
        } else if fps <= FPS_RANGE_EXTREME.1 {
            FpsCategory::Extreme
        } else {
            FpsCategory::Invalid
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            FpsCategory::Normal => "normal range (1–239 fps)",
            FpsCategory::Extended => "extended range (240–2000 fps)",
            FpsCategory::Extreme => "extreme range (2000-10000 fps)",
            FpsCategory::Invalid => "invalid (>10000 fps, possible metadata error)",
        }
    }

    pub fn is_valid(&self) -> bool {
        !matches!(self, FpsCategory::Invalid)
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

const FPS_RANGE_NORMAL: (f64, f64) = (1.0, 240.0);
const FPS_RANGE_EXTENDED: (f64, f64) = (240.0, 2000.0);
const FPS_RANGE_EXTREME: (f64, f64) = (2000.0, 10000.0);
const FPS_THRESHOLD_INVALID: f64 = 10000.0;

/// Single ffprobe run for precheck: stream (codec, size, duration, fps, bit_rate, color) + format.duration.
fn run_precheck_ffprobe(input: &Path) -> Result<serde_json::Value> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height,r_frame_rate,duration,nb_frames,bit_rate,color_space,color_transfer,pix_fmt,bits_per_raw_sample",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
            "--",
        ])
        .arg(crate::safe_path_arg(input).as_ref())
        .output()
        .context("ffprobe failed")?;

    if !output.status.success() {
        bail!("ffprobe failed");
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("ffprobe JSON parse failed")?;
    Ok(json)
}

fn parse_fps_from_stream(stream: &serde_json::Value) -> f64 {
    stream["r_frame_rate"]
        .as_str()
        .and_then(|s| {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() == 2 {
                let num: f64 = parts[0].parse().ok()?;
                let den: f64 = parts[1].parse().ok()?;
                if den > 0.0 {
                    Some(num / den)
                } else {
                    None
                }
            } else {
                s.parse().ok()
            }
        })
        .unwrap_or(30.0)
}

fn parse_duration_from_precheck_json(
    json: &serde_json::Value,
    fps: f64,
    frame_count: u64,
    input: &Path,
) -> Result<(f64, f64, u64)> {
    let stream = json["streams"].get(0);
    let stream_duration: Option<f64> = stream
        .and_then(|s| s["duration"].as_str())
        .and_then(|s| s.parse().ok())
        .filter(|&d: &f64| d > 0.0 && !d.is_nan());

    if let Some(duration) = stream_duration {
        return Ok((duration, fps, frame_count));
    }

    warn!("DURATION: stream.duration unavailable, trying format.duration");
    let format_duration: Option<f64> = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .filter(|&d: &f64| d > 0.0 && !d.is_nan());

    if let Some(duration) = format_duration {
        info!(duration_secs = %duration, "DURATION RECOVERED via format.duration");
        return Ok((duration, fps, frame_count));
    }

    warn!("DURATION: format.duration failed, trying frame_count/fps");
    if frame_count > 0 && fps > 0.0 && !fps.is_nan() {
        let duration = frame_count as f64 / fps;
        if duration > 0.0 {
            info!(duration_secs = %duration, frames = frame_count, fps = %fps, "DURATION RECOVERED via frame_count/fps");
            return Ok((duration, fps, frame_count));
        }
    }

    warn!("DURATION: frame_count/fps failed, trying ImageMagick (animated image fallback)");
    if let Some((duration_secs, frames)) =
        crate::image_analyzer::get_animation_duration_and_frames_imagemagick(input)
    {
        if duration_secs > 0.0 && frames > 0 {
            let inferred_fps = frames as f64 / duration_secs;
            info!(duration_secs = %duration_secs, frames, fps = %inferred_fps, "DURATION RECOVERED via ImageMagick");
            return Ok((duration_secs, inferred_fps, frames));
        }
    }

    error!(file = %input.display(), "DURATION DETECTION FAILED - Cannot determine video duration");
    bail!("Failed to detect video duration - all methods failed")
}

/// P3: Compute only BPP from precheck JSON (one ffprobe, no full VideoInfo).
fn bpp_from_precheck_json(
    json: &serde_json::Value,
    file_size: u64,
    input: &Path,
) -> Result<f64> {
    let stream = json["streams"]
        .get(0)
        .context("No video stream in ffprobe output")?;
    let width: u32 = stream["width"]
        .as_u64()
        .and_then(|w| u32::try_from(w).ok())
        .context("Missing or invalid video width")?;
    let height: u32 = stream["height"]
        .as_u64()
        .and_then(|h| u32::try_from(h).ok())
        .context("Missing or invalid video height")?;
    let fps = parse_fps_from_stream(stream);
    let frame_count_raw: u64 = stream["nb_frames"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| stream["nb_frames"].as_u64())
        .unwrap_or(0);
    let (duration, fps, frame_count_raw) =
        parse_duration_from_precheck_json(json, fps, frame_count_raw, input)?;
    let frame_count = if frame_count_raw == 0 && duration > 0.0 {
        (duration * fps) as u64
    } else {
        frame_count_raw.max(1)
    };
    let video_bytes = stream["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&br| br > 0)
        .map(|br| (br as f64 * duration / 8.0) as u64)
        .unwrap_or(0);
    let bytes_for_bpp = if video_bytes > 0 {
        video_bytes
    } else {
        file_size
    };
    let total_pixels = width as u64 * height as u64 * frame_count;
    Ok(if total_pixels > 0 {
        (bytes_for_bpp as f64 * 8.0) / total_pixels as f64
    } else {
        0.5
    })
}

pub fn detect_duration_comprehensive(input: &Path) -> Result<(f64, f64, u64, &'static str)> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=r_frame_rate,duration,nb_frames",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
            "--",
        ])
        .arg(crate::safe_path_arg(input).as_ref())
        .output()
        .context("ffprobe failed")?;

    if !output.status.success() {
        bail!("ffprobe failed to get duration");
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&json_str).context("ffprobe JSON parse failed")?;

    let fps: f64 = json["streams"]
        .get(0)
        .and_then(|s| s["r_frame_rate"].as_str())
        .and_then(|s| {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() == 2 {
                let num: f64 = parts[0].parse().ok()?;
                let den: f64 = parts[1].parse().ok()?;
                if den > 0.0 {
                    Some(num / den)
                } else {
                    None
                }
            } else {
                s.parse().ok()
            }
        })
        .unwrap_or(30.0);

    let frame_count: u64 = json["streams"]
        .get(0)
        .and_then(|s| s["nb_frames"].as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let stream_duration: Option<f64> = json["streams"]
        .get(0)
        .and_then(|s| s["duration"].as_str())
        .and_then(|s| s.parse().ok())
        .filter(|&d: &f64| d > 0.0 && !d.is_nan());

    if let Some(duration) = stream_duration {
        return Ok((duration, fps, frame_count, "stream.duration"));
    }

    warn!("DURATION: stream.duration unavailable, trying format.duration");
    let format_duration: Option<f64> = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .filter(|&d: &f64| d > 0.0 && !d.is_nan());

    if let Some(duration) = format_duration {
        info!(duration_secs = %duration, "DURATION RECOVERED via format.duration");
        return Ok((duration, fps, frame_count, "format.duration"));
    }

    warn!("DURATION: format.duration failed, trying frame_count/fps");
    if frame_count > 0 && fps > 0.0 && !fps.is_nan() {
        let duration = frame_count as f64 / fps;
        if duration > 0.0 {
            info!(duration_secs = %duration, frames = frame_count, fps = %fps, "DURATION RECOVERED via frame_count/fps");
            return Ok((duration, fps, frame_count, "frame_count/fps"));
        }
    }

    warn!("DURATION: frame_count/fps failed, trying ImageMagick (animated image fallback)");
    if let Some((duration_secs, frames)) =
        crate::image_analyzer::get_animation_duration_and_frames_imagemagick(input)
    {
        if duration_secs > 0.0 && frames > 0 {
            let inferred_fps = frames as f64 / duration_secs;
            info!(duration_secs = %duration_secs, frames, fps = %inferred_fps, "DURATION RECOVERED via ImageMagick");
            return Ok((duration_secs, inferred_fps, frames, "imagemagick"));
        }
    }

    error!(file = %input.display(), "DURATION DETECTION FAILED - Cannot determine video duration");
    bail!("Failed to detect video duration - all methods failed")
}

pub fn get_video_info(input: &Path) -> Result<VideoInfo> {
    let file_size = std::fs::metadata(input)
        .context("Failed to read file metadata")?
        .len();

    let json = run_precheck_ffprobe(input)?;
    let stream = json["streams"]
        .get(0)
        .context("No video stream in ffprobe output")?;

    let codec = stream["codec_name"]
        .as_str()
        .unwrap_or("")
        .to_string()
        .to_lowercase();
    if codec.is_empty() {
        bail!("Could not detect video codec");
    }

    let width: u32 = stream["width"]
        .as_u64()
        .and_then(|w| u32::try_from(w).ok())
        .context("Missing or invalid video width")?;
    let height: u32 = stream["height"]
        .as_u64()
        .and_then(|h| u32::try_from(h).ok())
        .context("Missing or invalid video height")?;

    let fps = parse_fps_from_stream(stream);
    let frame_count_raw: u64 = stream["nb_frames"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| stream["nb_frames"].as_u64())
        .unwrap_or(0);

    let (duration, fps, frame_count_raw) =
        parse_duration_from_precheck_json(&json, fps, frame_count_raw, input)?;
    let frame_count = if frame_count_raw == 0 && duration > 0.0 {
        (duration * fps) as u64
    } else {
        frame_count_raw.max(1)
    };

    let bitrate_kbps = stream["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|bps| bps / 1000.0)
        .unwrap_or_else(|| {
            if duration > 0.0 {
                (file_size as f64 * 8.0) / (duration * 1000.0)
            } else {
                0.0
            }
        });

    let video_bytes = stream["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&br| br > 0)
        .map(|br| (br as f64 * duration / 8.0) as u64)
        .unwrap_or(0);
    let bytes_for_bpp = if video_bytes > 0 {
        video_bytes
    } else {
        file_size
    };
    let total_pixels = width as u64 * height as u64 * frame_count;
    let bpp = if total_pixels > 0 {
        (bytes_for_bpp as f64 * 8.0) / total_pixels as f64
    } else {
        0.5
    };

    use crate::quality_matcher::parse_source_codec;
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
        || bpp > 0.50
    {
        Compressibility::VeryHigh
    } else if bpp > 0.30 {
        Compressibility::High
    } else if bpp < 0.15 {
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
    let bit_depth = stream["bits_per_raw_sample"]
        .as_str()
        .and_then(|s| s.parse::<u8>().ok());

    let fps_category = FpsCategory::from_fps(fps);

    // HDR: require BT.2020 (or 2020) and PQ/HLG transfer; 10-bit alone is not HDR (ProRes/DPX SDR).
    let is_hdr = color_space
        .as_ref()
        .map(|cs| cs.contains("bt2020") || cs.contains("2020"))
        .unwrap_or(false)
        && color_transfer
            .as_ref()
            .map(|t| {
                let lower = t.to_lowercase();
                lower.contains("smpte2084")
                    || lower.contains("arib-std-b67")
                    || lower.contains("pq")
                    || lower.contains("hlg")
            })
            .unwrap_or(false);

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

/// Caller must pass lowercase codec (e.g. from get_video_info).
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
            reason: format!("Resolution too small {}x{} (< 16px)", width, height),
        };
    }
    if width > 16384 || height > 16384 {
        return ProcessingRecommendation::CannotProcess {
            reason: format!("Resolution too large {}x{} (> 16K)", width, height),
        };
    }

    if duration < 0.001 {
        return ProcessingRecommendation::CannotProcess {
            reason: format!(
                "Duration read as {:.3}s (possible metadata issue, will attempt conversion)",
                duration
            ),
        };
    }

    if fps <= 0.0 {
        return ProcessingRecommendation::CannotProcess {
            reason: format!("Invalid FPS ({:.2})", fps),
        };
    }
    if fps > FPS_THRESHOLD_INVALID {
        return ProcessingRecommendation::CannotProcess {
            reason: format!(
                "Abnormal FPS ({:.0} > {}, likely metadata error)",
                fps, FPS_THRESHOLD_INVALID
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
                "Detected {}, strongly recommended to upgrade to modern codec (expect 10-50x better compression)",
                codec_category
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

    use crate::quality_matcher::parse_source_codec;
    let source_codec = parse_source_codec(codec);
    let codec_efficiency = source_codec.efficiency_factor();

    let resolution_factor = (width * height) as f64 / (1920.0 * 1080.0);
    let fps_factor = fps / 30.0;

    let base_bitrate_1080p30_h264 = 2500.0;
    let expected_min_bitrate =
        base_bitrate_1080p30_h264 * resolution_factor * fps_factor * codec_efficiency;

    let bpp_threshold_very_low = 0.05 / codec_efficiency;
    let bpp_threshold_low = 0.10 / codec_efficiency;

    if bitrate_kbps > 0.0
        && bitrate_kbps < expected_min_bitrate * 0.5
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

    if bitrate_kbps > 0.0 && bitrate_kbps < expected_min_bitrate && bpp < bpp_threshold_low {
        return ProcessingRecommendation::Recommended {
            reason: format!(
                "File has some compression (bitrate: {:.0} kbps), but modern codecs can optimize further",
                bitrate_kbps
            ),
        };
    }

    ProcessingRecommendation::Recommended {
        reason: format!(
            "Standard codec ({}), suggest upgrading to HEVC/AV1 for better compression and quality",
            codec
        ),
    }
}

/// Returns bits-per-pixel from video stream (one ffprobe, minimal parse; P3 lightweight path).
pub fn calculate_bpp(input: &Path) -> Result<f64> {
    let file_size = std::fs::metadata(input).context("Failed to read file metadata")?.len();
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
    lines.push(format!("│ FPS: {:.2} {}", info.fps, info.fps_category.description()));
    lines.push(format!(
        "│ File Size: {:.2} MB",
        info.file_size as f64 / 1024.0 / 1024.0
    ));
    lines.push(format!("│ Bitrate: {:.0} kbps", info.bitrate_kbps));
    lines.push(format!("│ BPP: {:.4} bits/pixel", info.bpp));

    if info.color_space.is_some() || info.pix_fmt.is_some() || info.bit_depth.is_some() {
        lines.push("├─────────────────────────────────────────────────────".to_string());
        if let Some(ref cs) = info.color_space {
            let hdr_indicator = if info.is_hdr { " HDR" } else { "" };
            lines.push(format!("│ Color Space: {}{}", cs, hdr_indicator));
        }
        if let Some(ref pf) = info.pix_fmt {
            lines.push(format!("│ Pixel Format: {}", pf));
        }
        if let Some(bd) = info.bit_depth {
            lines.push(format!("│ Bit Depth: {}-bit", bd));
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
            lines.push(format!("│    → Source: {} (legacy/inefficient)", codec));
            lines.push(format!("│    → {}", reason));
        }
        ProcessingRecommendation::Recommended { reason } => {
            lines.push("│ ✅ RECOMMENDED: Convert to modern codec".to_string());
            lines.push(format!("│    → {}", reason));
        }
        ProcessingRecommendation::Optional { reason } => {
            lines.push("│ OPTIONAL: Marginal benefit expected".to_string());
            lines.push(format!("│    → {}", reason));
        }
        ProcessingRecommendation::NotRecommended { codec, reason } => {
            lines.push("│ ⚠️  NOT RECOMMENDED: Already optimal".to_string());
            lines.push(format!("│    → Codec: {}", codec));
            lines.push(format!("│    → {}", reason));
        }
        ProcessingRecommendation::CannotProcess { reason } => {
            lines.push("│ ❌ CANNOT PROCESS: File issue detected".to_string());
            lines.push(format!("│    → {}", reason));
        }
    }

    lines.push("└─────────────────────────────────────────────────────".to_string());
    for line in &lines {
        info!("{}", line);
    }
}

pub fn run_precheck(input: &Path) -> Result<VideoInfo> {
    let info = get_video_info(input)?;
    print_precheck_report(&info);

    match &info.recommendation {
        ProcessingRecommendation::CannotProcess { reason } => {
            warn!(reason = %reason, "PRECHECK: cannot process");
            bail!("Precheck cannot process this file: {}", reason);
        }

        ProcessingRecommendation::NotRecommended { codec, reason } => {
            warn!(codec = %codec, reason = %reason, "WARNING: already modern codec (continuing anyway)");
        }

        ProcessingRecommendation::StronglyRecommended { codec, reason } => {
            info!(codec = %codec, reason = %reason, "EXCELLENT TARGET: legacy codec, will benefit from modern encoding");
        }

        ProcessingRecommendation::Recommended { .. }
        | ProcessingRecommendation::Optional { .. } => {}
    }

    Ok(info)
}
