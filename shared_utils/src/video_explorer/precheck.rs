//! Video precheck and processing recommendation

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

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
        } else if fps <= FPS_RANGE_NORMAL.1 {
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
            FpsCategory::Normal => "normal range (1-240 fps)",
            FpsCategory::Extended => "extended range (240-2000 fps)",
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
    "huffyuv",
];

const OPTIMAL_CODECS: &[&str] = &[
    "hevc",
    "h265",
    "x265",
    "hvc1",
    "av1",
    "av01",
    "libaom-av1",
];

const FPS_RANGE_NORMAL: (f64, f64) = (1.0, 240.0);
const FPS_RANGE_EXTENDED: (f64, f64) = (240.0, 2000.0);
const FPS_RANGE_EXTREME: (f64, f64) = (2000.0, 10000.0);
const FPS_THRESHOLD_INVALID: f64 = 10000.0;

fn get_codec_info(input: &Path) -> Result<String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            "--",
        ])
        .arg(crate::safe_path_arg(input).as_ref())
        .output()
        .context("ffprobe failed to get codec")?;

    if !output.status.success() {
        bail!("ffprobe failed to get codec");
    }

    let codec = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();

    if codec.is_empty() {
        bail!("Could not detect video codec");
    }

    Ok(codec)
}

fn get_bitrate(input: &Path) -> Result<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=bit_rate",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            "--",
        ])
        .arg(crate::safe_path_arg(input).as_ref())
        .output()
        .context("ffprobe failed to get bitrate")?;

    if output.status.success() {
        let bitrate_str = String::from_utf8_lossy(&output.stdout);
        if let Ok(bitrate_bps) = bitrate_str.trim().parse::<f64>() {
            return Ok(bitrate_bps / 1000.0);
        }
    }

    Ok(0.0)
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

    eprintln!("   ⚠️ DURATION: stream.duration unavailable, trying format.duration...");
    let format_duration: Option<f64> = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .filter(|&d: &f64| d > 0.0 && !d.is_nan());

    if let Some(duration) = format_duration {
        eprintln!(
            "   ✅ DURATION RECOVERED via format.duration: {:.2}s",
            duration
        );
        return Ok((duration, fps, frame_count, "format.duration"));
    }

    eprintln!("   ⚠️ DURATION: format.duration failed, trying frame_count/fps...");
    if frame_count > 0 && fps > 0.0 && !fps.is_nan() {
        let duration = frame_count as f64 / fps;
        if duration > 0.0 {
            eprintln!(
                "   ✅ DURATION RECOVERED via frame_count/fps: {:.2}s ({} frames / {:.2} fps)",
                duration, frame_count, fps
            );
            return Ok((duration, fps, frame_count, "frame_count/fps"));
        }
    }

    eprintln!("   ⚠️ DURATION: frame_count/fps failed, trying ImageMagick (WebP/GIF)...");
    if let Some((duration_secs, frames)) = crate::image_analyzer::get_animation_duration_and_frames_imagemagick(input) {
        if duration_secs > 0.0 && frames > 0 {
            let inferred_fps = frames as f64 / duration_secs;
            eprintln!(
                "   ✅ DURATION RECOVERED via ImageMagick: {:.2}s ({} frames, {:.2} fps)",
                duration_secs, frames, inferred_fps
            );
            return Ok((duration_secs, inferred_fps, frames, "imagemagick"));
        }
    }

    eprintln!("   ❌ DURATION DETECTION FAILED - Cannot determine video duration");
    eprintln!("   File: {}", input.display());
    bail!("Failed to detect video duration - all methods failed")
}

pub fn get_video_info(input: &Path) -> Result<VideoInfo> {
    let file_size = std::fs::metadata(input)
        .context("Failed to read file metadata")?
        .len();

    let codec = get_codec_info(input)?;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            "--",
        ])
        .arg(crate::safe_path_arg(input).as_ref())
        .output()
        .context("ffprobe failed")?;

    if !output.status.success() {
        bail!("ffprobe failed to get video info");
    }

    let info_str = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = info_str.trim().split(',').collect();

    if parts.len() < 2 {
        bail!("ffprobe output format abnormal: {}", info_str);
    }

    let width: u32 = parts
        .first()
        .and_then(|s| s.parse().ok())
        .context("Failed to parse video width")?;
    let height: u32 = parts
        .get(1)
        .and_then(|s| s.parse().ok())
        .context("Failed to parse video height")?;

    let (duration, fps, frame_count_raw, _method) = detect_duration_comprehensive(input)?;

    let frame_count = if frame_count_raw == 0 && duration > 0.0 {
        (duration * fps) as u64
    } else {
        frame_count_raw.max(1)
    };

    let bitrate_kbps = get_bitrate(input).unwrap_or_else(|_| {
        if duration > 0.0 {
            (file_size as f64 * 8.0) / (duration * 1000.0)
        } else {
            0.0
        }
    });

    let total_pixels = width as u64 * height as u64 * frame_count;
    let bpp = if total_pixels > 0 {
        (file_size as f64 * 8.0) / total_pixels as f64
    } else {
        0.5
    };

    use crate::quality_matcher::parse_source_codec;
    let source_codec_enum = parse_source_codec(&codec);

    let compressibility = if source_codec_enum.is_modern() {
        Compressibility::VeryLow
    } else if codec.to_lowercase().contains("theora")
        || codec.to_lowercase().contains("rv")
        || codec.to_lowercase().contains("real")
        || codec.to_lowercase().contains("mjpeg")
        || codec.to_lowercase().contains("cinepak")
        || codec.to_lowercase().contains("indeo")
        || codec.to_lowercase().contains("gif")
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

    let recommendation = evaluate_processing_recommendation(
        &codec,
        width,
        height,
        duration,
        fps,
        bitrate_kbps,
        bpp,
    );

    let (color_space, pix_fmt, bit_depth) = extract_color_info(input);

    let fps_category = FpsCategory::from_fps(fps);

    let is_hdr = color_space
        .as_ref()
        .map(|cs| cs.contains("bt2020") || cs.contains("2020"))
        .unwrap_or(false)
        || bit_depth.map(|bd| bd >= 10).unwrap_or(false)
        || pix_fmt
            .as_ref()
            .map(|pf| pf.contains("10le") || pf.contains("10be") || pf.contains("p10"))
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

fn evaluate_processing_recommendation(
    codec: &str,
    width: u32,
    height: u32,
    duration: f64,
    fps: f64,
    bitrate_kbps: f64,
    bpp: f64,
) -> ProcessingRecommendation {
    let codec_lower = codec.to_lowercase();


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
        .any(|&c| codec_lower.contains(c))
    {
        let codec_category = if codec_lower.contains("theora") {
            "Theora (Open Source, WebM predecessor)"
        } else if codec_lower.contains("rv") || codec_lower.contains("real") {
            "RealVideo (Legacy streaming standard)"
        } else if codec_lower.contains("vp6") || codec_lower.contains("vp7") {
            "VP6/VP7 (Flash Video era)"
        } else if codec_lower.contains("wmv") {
            "Windows Media Video"
        } else if codec_lower.contains("cinepak") {
            "Cinepak (CD-ROM era)"
        } else if codec_lower.contains("indeo") || codec_lower.contains("iv") {
            "Intel Indeo"
        } else if codec_lower.contains("svq") {
            "Sorenson Video (QuickTime)"
        } else if codec_lower.contains("flv") {
            "Flash Video H.263"
        } else if codec_lower.contains("mjpeg") {
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

    if OPTIMAL_CODECS.iter().any(|&c| codec_lower.contains(c)) {
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

fn extract_color_info(input: &Path) -> (Option<String>, Option<String>, Option<u8>) {
    let info = crate::ffprobe_json::extract_color_info(input);
    (info.color_space, info.pix_fmt, info.bit_depth)
}

pub fn calculate_bpp(input: &Path) -> Result<f64> {
    let info = get_video_info(input)?;
    Ok(info.bpp)
}

pub fn print_precheck_report(info: &VideoInfo) {
    if !crate::progress_mode::is_verbose_mode() {
        return;
    }
    eprintln!("┌─────────────────────────────────────────────────────");
    eprintln!("│ Precheck Report v5.75");
    eprintln!("├─────────────────────────────────────────────────────");
    eprintln!("│ Codec: {}", info.codec);
    eprintln!("│ Resolution: {}x{}", info.width, info.height);
    eprintln!(
        "│ Duration: {:.1}s ({} frames)",
        info.duration, info.frame_count
    );
    eprintln!(
        "│ FPS: {:.2} {}",
        info.fps,
        info.fps_category.description()
    );

    eprintln!(
        "│ File Size: {:.2} MB",
        info.file_size as f64 / 1024.0 / 1024.0
    );
    eprintln!("│ Bitrate: {:.0} kbps", info.bitrate_kbps);
    eprintln!("│ BPP: {:.4} bits/pixel", info.bpp);

    if info.color_space.is_some() || info.pix_fmt.is_some() || info.bit_depth.is_some() {
        eprintln!("├─────────────────────────────────────────────────────");
        if let Some(ref cs) = info.color_space {
            let hdr_indicator = if info.is_hdr { " HDR" } else { "" };
            eprintln!("│ Color Space: {}{}", cs, hdr_indicator);
        }
        if let Some(ref pf) = info.pix_fmt {
            eprintln!("│ Pixel Format: {}", pf);
        }
        if let Some(bd) = info.bit_depth {
            eprintln!("│ Bit Depth: {}-bit", bd);
        }
    }

    eprintln!("├─────────────────────────────────────────────────────");

    match info.compressibility {
        Compressibility::VeryHigh => {
            eprintln!("│ Compression Potential: VERY HIGH");
            eprintln!("│    → Ancient codec or extremely high BPP");
            eprintln!("│    → Expected 10-50x compression improvement!");
        }
        Compressibility::High => {
            eprintln!("│ ✅ Compression Potential: High");
            eprintln!("│    → Large compression space expected");
        }
        Compressibility::Medium => {
            eprintln!("│ Compression Potential: Medium");
            eprintln!("│    → Moderate compression potential");
        }
        Compressibility::Low => {
            eprintln!("│ ⚠️  Compression Potential: Low");
            eprintln!("│    → File already optimized");
        }
        Compressibility::VeryLow => {
            eprintln!("│ Compression Potential: VERY LOW");
            eprintln!("│    → Already using modern codec (HEVC/AV1)");
            eprintln!("│    → Re-encoding may cause quality loss");
        }
    }

    eprintln!("├─────────────────────────────────────────────────────");
    match &info.recommendation {
        ProcessingRecommendation::StronglyRecommended { codec, reason } => {
            eprintln!("│ STRONGLY RECOMMENDED: Upgrade to modern codec!");
            eprintln!("│    → Source: {} (legacy/inefficient)", codec);
            eprintln!("│    → {}", reason);
        }
        ProcessingRecommendation::Recommended { reason } => {
            eprintln!("│ ✅ RECOMMENDED: Convert to modern codec");
            eprintln!("│    → {}", reason);
        }
        ProcessingRecommendation::Optional { reason } => {
            eprintln!("│ OPTIONAL: Marginal benefit expected");
            eprintln!("│    → {}", reason);
        }
        ProcessingRecommendation::NotRecommended { codec, reason } => {
            eprintln!("│ ⚠️  NOT RECOMMENDED: Already optimal");
            eprintln!("│    → Codec: {}", codec);
            eprintln!("│    → {}", reason);
        }
        ProcessingRecommendation::CannotProcess { reason } => {
            eprintln!("│ ❌ CANNOT PROCESS: File issue detected");
            eprintln!("│    → {}", reason);
        }
    }

    eprintln!("└─────────────────────────────────────────────────────");
}

pub fn run_precheck(input: &Path) -> Result<VideoInfo> {
    let info = get_video_info(input)?;
    print_precheck_report(&info);

    match &info.recommendation {
        ProcessingRecommendation::CannotProcess { reason } => {
            eprintln!("⚠️  PRECHECK WARNING: {}", reason);
            eprintln!("    → Possible metadata issue, attempting conversion anyway...");
            eprintln!("    → If conversion fails, check source file integrity");
        }

        ProcessingRecommendation::NotRecommended { codec, reason } => {
            eprintln!("⚠️  WARNING: {} is already a modern codec", codec);
            eprintln!("    {}", reason);
            eprintln!("    (Continuing anyway, but quality loss may occur...)");
        }

        ProcessingRecommendation::StronglyRecommended { codec, reason } => {
            eprintln!("EXCELLENT TARGET: {} is a legacy codec!", codec);
            eprintln!("    {}", reason);
            eprintln!("    (This file will benefit greatly from modern encoding!)");
        }

        ProcessingRecommendation::Recommended { .. }
        | ProcessingRecommendation::Optional { .. } => {
        }
    }

    Ok(info)
}
