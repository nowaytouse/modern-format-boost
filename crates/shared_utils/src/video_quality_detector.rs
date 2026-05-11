//! 🔬 Video Quality Detector - Precision-Validated Video Analysis for Auto Routing
//!
//! This module provides unified video quality detection for:
//! - Auto format routing decisions (AV1/HEVC/FFV1)
//! - Quality matching (CRF calculation)
//! - Codec skip decisions
//!
//! ## 🔥 Quality Manifesto Compliance
//! - NO silent fallback - errors fail loudly
//! - NO hardcoded defaults - all from actual ffprobe analysis
//! - Base decisions on actual content detection, not format names
//!
//! ## Integration with `quality_matcher`
//! This module provides the detection layer, while `quality_matcher`
//! provides the CRF calculation layer.

use crate::progress_mode::write_to_log_at_level;
use crate::quality_matcher::{
    ContentType, QualityAnalysis, SourceCodec, VideoAnalysisBuilder, parse_source_codec,
    should_skip_video_codec,
};
use crate::video_detection::Detection;
use rug::Rational;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::Level;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DecisionFlags {
    pub is_modern_codec: bool,
    pub should_skip: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeatureFlags {
    pub has_b_frames: bool,
    pub is_hdr: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QualityFlags {
    #[serde(flatten)]
    pub decision: DecisionFlags,
    #[serde(flatten)]
    pub features: FeatureFlags,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoQualityAnalysis {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub file_size: u64,
    pub duration_secs: Option<f64>,
    pub fps: Option<f64>,
    pub frame_count: Option<u64>,

    pub codec: String,
    pub codec_type: VideoCodecType,
    #[serde(flatten)]
    pub flags: QualityFlags,
    pub skip_reason: Option<String>,

    pub total_bitrate: Option<u64>,
    pub video_bitrate: Option<u64>,
    pub bpp: f64,
    pub bit_depth: Option<u8>,

    pub pix_fmt: String,
    pub chroma: ChromaSubsampling,
    pub gop_size: Option<u32>,
    /// Actual B-frame count (`max_b_frames`) from ffprobe.
    pub b_frame_count: Option<u8>,

    pub color_space: Option<String>,

    pub content_type: VideoContentType,
    pub compression_type: CompressionLevel,

    pub quality_score: u8,
    pub estimated_crf: u8,

    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodecType {
    Lossless,
    ModernEfficient,
    Legacy,
    Intermediate,
    Inefficient,
    Unknown,
}

impl VideoCodecType {
    #[must_use]
    pub const fn from_source_codec(codec: SourceCodec) -> Self {
        match codec {
            SourceCodec::Ffv1 | SourceCodec::UtVideo | SourceCodec::HuffYuv => Self::Lossless,
            SourceCodec::Av1
            | SourceCodec::H265
            | SourceCodec::Vp9
            | SourceCodec::Vvc
            | SourceCodec::Av2 => Self::ModernEfficient,
            SourceCodec::H264 => Self::Legacy,
            SourceCodec::ProRes | SourceCodec::DnxHD => Self::Intermediate,
            SourceCodec::Mjpeg | SourceCodec::Gif => Self::Inefficient,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChromaSubsampling {
    Yuv420,
    Yuv422,
    Yuv444,
    Rgb,
    Unknown,
}

impl ChromaSubsampling {
    #[must_use]
    pub fn from_pix_fmt(pix_fmt: &str) -> Self {
        let fmt = pix_fmt.to_lowercase();
        if fmt.contains("444") {
            Self::Yuv444
        } else if fmt.contains("422") || fmt.contains("411") {
            Self::Yuv422
        } else if fmt.contains("420")
            || fmt.contains("nv12")
            || fmt.starts_with("yuv")
            || fmt.contains("410")
        {
            Self::Yuv420
        } else if fmt.contains("rgb") || fmt.contains("gbr") || fmt.contains("bgr") {
            Self::Rgb
        } else {
            Self::Unknown
        }
    }

    #[must_use]
    pub const fn quality_factor(&self) -> f64 {
        match self {
            Self::Yuv420 | Self::Unknown => crate::constants::CHROMA_FACTOR_YUV420,
            Self::Yuv422 => crate::constants::CHROMA_FACTOR_YUV422,
            Self::Yuv444 => crate::constants::CHROMA_FACTOR_YUV444,
            Self::Rgb => crate::constants::CHROMA_FACTOR_RGB,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoContentType {
    LiveAction,
    Animation,
    ScreenRecording,
    Gaming,
    FilmGrain,
    Unknown,
}

impl VideoContentType {
    #[must_use]
    pub const fn to_content_type(&self) -> ContentType {
        match self {
            Self::LiveAction => ContentType::LiveAction,
            Self::Animation => ContentType::Animation,
            Self::ScreenRecording => ContentType::ScreenRecording,
            Self::Gaming => ContentType::Gaming,
            Self::FilmGrain => ContentType::FilmGrain,
            Self::Unknown => ContentType::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionLevel {
    Lossless,
    VisuallyLossless,
    HighQuality,
    Standard,
    LowQuality,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct VideoQualityInput<'a> {
    pub codec: &'a str,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub duration_secs: Option<f64>,
    pub total_bitrate: Option<u64>,
    pub video_bitrate: Option<u64>,
    pub pix_fmt: &'a str,
    pub bit_depth: Option<u8>,
    pub max_b_frames: Option<u8>,
    pub encoder_params: Option<&'a str>,
    pub gop_size: Option<u32>,
    pub color_space: Option<&'a str>,
    pub file_size: u64,
    pub frame_count: Option<u64>,
}

impl CompressionLevel {
    #[must_use]
    pub fn from_bpp(bpp: f64, codec_type: VideoCodecType) -> Self {
        use crate::numeric_cast::f64_to_rational_strict;
        if bpp <= 0.0_f64 {
            return Self::LowQuality;
        }

        if codec_type == VideoCodecType::Lossless {
            return Self::Lossless;
        }
        if codec_type == VideoCodecType::Intermediate {
            return Self::VisuallyLossless;
        }

        let efficiency = match codec_type {
            VideoCodecType::ModernEfficient => crate::constants::BPP_FACTOR_MODERN,
            VideoCodecType::Inefficient => crate::constants::BPP_FACTOR_INEFFICIENT,
            _ => 1.0_f64,
        };

        let Some(bpp_r) = f64_to_rational_strict(bpp, "bpp") else {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_ANOMALY,
                "BPP NaN/Inf! Refusing to forge data. Information invalidated."
            );
            return Self::Unknown;
        };
        let Some(efficiency_r) = f64_to_rational_strict(efficiency, "efficiency") else {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_ANOMALY,
                "Efficiency NaN/Inf! Refusing to forge data. Information invalidated."
            );
            return Self::Unknown;
        };
        let adjusted_bpp = (bpp_r / efficiency_r).to_f64();

        if adjusted_bpp > crate::constants::BPP_THRESHOLD_VERY_HIGH {
            Self::VisuallyLossless
        } else if adjusted_bpp > crate::constants::BPP_THRESHOLD_HIGH {
            Self::HighQuality
        } else if adjusted_bpp > crate::constants::BPP_THRESHOLD_MEDIUM {
            Self::Standard
        } else {
            Self::LowQuality
        }
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
/// Analyze video quality (codec type, bpp, content type, compression level, etc.).
///
/// # Errors
/// Returns an error if video quality analysis fails due to invalid parameters.
pub fn analyze_video_quality(input: VideoQualityInput<'_>) -> Result<VideoQualityAnalysis, String> {
    let VideoQualityInput {
        codec,
        width,
        height,
        fps,
        duration_secs,
        total_bitrate,
        video_bitrate,
        pix_fmt,
        bit_depth,
        max_b_frames,
        encoder_params,
        gop_size,
        color_space,
        file_size,
        frame_count: input_frame_count,
    } = input;

    let w = width.ok_or_else(|| "❌ Missing width metadata".to_string())?;
    let h = height.ok_or_else(|| "❌ Missing height metadata".to_string())?;
    if w == 0 || h == 0 {
        return Err("❌ Invalid dimensions: width or height is 0".to_string());
    }
    let fps_val = fps.ok_or_else(|| "❌ Missing frame rate metadata".to_string())?;
    if fps_val <= 0.0_f64 {
        return Err("❌ Invalid frame rate: fps must be > 0".to_string());
    }
    let dur = duration_secs.ok_or_else(|| "❌ Missing duration metadata".to_string())?;
    if dur <= 0.0_f64 {
        return Err("❌ Invalid duration: must be > 0".to_string());
    }

    let source_codec = parse_source_codec(codec);
    let codec_type = VideoCodecType::from_source_codec(source_codec);
    let is_modern = source_codec.is_modern();

    let skip_decision = should_skip_video_codec(codec);

    let effective_bitrate = video_bitrate
        .or_else(|| {
            if let Some(tb) = total_bitrate
                && tb > 0
            {
                Some(tb)
            } else {
                // Calculate from file size: (file_size * 8) / duration
                let bits = crate::numeric_cast::u64_to_f64(file_size) * 8.0_f64;
                Some(crate::numeric_cast::f64_to_u64_sat(bits / dur))
            }
        })
        .ok_or_else(|| {
            "❌ Missing bitrate: cannot calculate BPP for quality assessment".to_string()
        })?;
    let bpp = {
        let pixels_per_second = Rational::from(w) * Rational::from(h) * {
            let Some(fps_r) = crate::numeric_cast::f64_to_rational_strict(fps_val, "fps") else {
                return Err("❌ Invalid frame rate: fps is NaN/Inf".to_string());
            };
            fps_r
        };
        if pixels_per_second > 0 {
            // effective_bitrate is u64; Rational::from requires i64 or smaller.
            // Saturate to i64::MAX for astronomically large bitrates (>9 Pbps).
            let bits_per_second = Rational::from(
                i64::try_from(effective_bitrate).unwrap_or_else(|_| {
                    tracing::warn!(
                        effective_bitrate,
                        "analyze_video_quality: effective_bitrate exceeds i64::MAX; saturating for BPP calc"
                    );
                    i64::MAX
                }),
            );
            (bits_per_second / pixels_per_second).to_f64()
        } else {
            0.0_f64
        }
    };

    let chroma = ChromaSubsampling::from_pix_fmt(pix_fmt);
    let is_hdr = color_space.is_some_and(|cs| {
        let cs_lower = cs.to_lowercase();
        cs_lower.contains("bt2020") || cs_lower.contains("2020")
    });

    let has_b_frames = max_b_frames.is_some_and(|b| b > 0);
    let b_frame_count = max_b_frames;

    let content_type = estimate_content_type(bpp, codec_type, w, h, fps_val);

    let compression_type = CompressionLevel::from_bpp(bpp, codec_type);

    let quality_score = calculate_quality_score(bpp, codec_type, bit_depth, compression_type);

    // Prioritize precise CRF/QP from encoder tags over BPP heuristic
    let estimated_crf = encoder_params.map_or_else(
        || estimate_crf_from_bpp(bpp, codec_type),
        |params| {
            extract_crf_from_params(params).unwrap_or_else(|| {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_PHASE_3,
                    "Video Analysis: Metadata search for CRF failed; falling back to BPP heuristic"
                );
                estimate_crf_from_bpp(bpp, codec_type)
            })
        },
    );

    let confidence = calculate_video_confidence(
        video_bitrate.is_some(),
        gop_size.is_some(),
        dur,
        input_frame_count,
    );

    Ok(VideoQualityAnalysis {
        width: Some(w),
        height: Some(h),
        file_size,
        duration_secs: Some(dur),
        fps: Some(fps_val),
        frame_count: input_frame_count,
        codec: codec.to_string(),
        codec_type,
        flags: QualityFlags {
            decision: DecisionFlags {
                is_modern_codec: is_modern,
                should_skip: skip_decision.should_skip,
            },
            features: FeatureFlags {
                has_b_frames,
                is_hdr,
            },
        },
        skip_reason: if skip_decision.should_skip {
            Some(skip_decision.reason)
        } else {
            None
        },
        total_bitrate: Some(effective_bitrate),
        video_bitrate,
        bpp,
        bit_depth,
        pix_fmt: pix_fmt.to_string(),
        chroma,
        gop_size,
        b_frame_count,
        color_space: color_space.map(std::string::ToString::to_string),
        content_type,
        compression_type,
        quality_score,
        estimated_crf,
        confidence,
    })
}

/// Build [`VideoQualityAnalysis`] from [`Detection`] for logging/display.
///
/// Use when you already have detection (e.g. before SSIM exploration) and want media info for log file only.
/// Analyze video quality based on a previous detection result.
///
/// # Errors
/// Returns an error message if analysis fails.
///
/// # Panics
/// Panics if the `duration_secs` is missing from the detection result,
/// although this is guarded by a check at the start of the function.
pub fn analyze_video_quality_from_detection(
    detection: &Detection,
) -> Result<VideoQualityAnalysis, String> {
    analyze_video_quality(VideoQualityInput {
        codec: detection.codec.as_str(),
        width: detection.width,
        height: detection.height,
        fps: detection.fps,
        duration_secs: detection.duration_secs,
        total_bitrate: detection.bitrate,
        video_bitrate: detection.video_bitrate,
        pix_fmt: &detection.pix_fmt,
        bit_depth: detection.bit_depth,
        max_b_frames: detection.max_b_frames,
        encoder_params: detection.encoder_params.as_deref(),
        gop_size: None,
        color_space: Some(detection.color_space.as_str()),
        file_size: detection.file_size,
        frame_count: detection.frame_count,
    })
}

fn extract_crf_from_params(params: &str) -> Option<u8> {
    let lower = params.to_lowercase();

    // Look for various ways CRF/QP might be specified
    for keyword in ["crf=", "qp=", "cqp=", "crf ", "qp "] {
        if let Some(pos) = lower.find(keyword) {
            let start = pos + keyword.len();
            let rest = &lower[start..];
            // Take characters while they are part of a float
            let end = rest
                .find(|c: char| !c.is_numeric() && c != '.')
                .unwrap_or(rest.len());
            let val_str = rest[..end].trim();
            if let Ok(val) = val_str.parse::<f64>() {
                return crate::numeric_cast::f64_to_u8_strict(val.round(), "crf_from_params")
                    .map_or_else(
                        || {
                            tracing::warn!("CRF value {} out of valid range, using default", val);
                            None
                        },
                        Some,
                    );
            }
        }
    }
    None
}

/// Format [`VideoQualityAnalysis`] as multi-line media info. **Log file only** — does not write to
/// terminal. Call when a log file is configured (e.g. alongside SSIM/quality runs).
pub fn log_media_info_for_quality(analysis: &VideoQualityAnalysis, input_path: &Path) {
    if !crate::progress_mode::has_log_file() {
        return;
    }
    write_to_log_at_level(
        Level::DEBUG,
        &format!("[Media info] {}", input_path.display()),
    );
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  codec={} type={:?} modern={}",
            analysis.codec, analysis.codec_type, analysis.flags.decision.is_modern_codec
        ),
    );
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  size={}x{} fps={} duration={}s frames={:?}",
            analysis
                .width
                .map_or_else(|| "N/A".to_string(), |v| v.to_string()),
            analysis
                .height
                .map_or_else(|| "N/A".to_string(), |v| v.to_string()),
            analysis
                .fps
                .map_or_else(|| "N/A".to_string(), |f| format!("{f:.2}")),
            analysis
                .duration_secs
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.2}")),
            analysis.frame_count
        ),
    );
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  bitrate={} video_bitrate={:?} bpp={:.4} bit_depth={}",
            analysis
                .total_bitrate
                .map_or_else(|| "N/A".to_string(), |v| v.to_string()),
            analysis.video_bitrate,
            analysis.bpp,
            analysis
                .bit_depth
                .map_or_else(|| "N/A".to_string(), |v| v.to_string())
        ),
    );
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  pix_fmt={} chroma={:?} has_b_frames={}",
            analysis.pix_fmt, analysis.chroma, analysis.flags.features.has_b_frames
        ),
    );
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  content_type={:?} compression={:?} quality_score={} estimated_crf={}",
            analysis.content_type,
            analysis.compression_type,
            analysis.quality_score,
            analysis.estimated_crf
        ),
    );
    if analysis.flags.features.is_hdr {
        write_to_log_at_level(Level::DEBUG, "  HDR: true");
    }
    write_to_log_at_level(Level::DEBUG, "");
}

/// Converts to `QualityAnalysis`.
///
/// # Panics
/// Panics if `width` or `height` is missing, which `analyze_video_quality` guarantees are present.
#[must_use]
pub fn to_quality_analysis(analysis: &VideoQualityAnalysis) -> QualityAnalysis {
    let gop_size = analysis.gop_size.or_else(|| {
        analysis.fps.and_then(|f| {
            crate::numeric_cast::f64_to_u32_strict(
                (f * crate::constants::GOP_CALC_FPS_MULTIPLIER)
                    .round()
                    .clamp(
                        crate::constants::GOP_CALC_MIN_LIMIT,
                        crate::constants::GOP_CALC_MAX_LIMIT,
                    ),
                "gop_calc",
            )
        })
    });
    let color_fallback = if analysis
        .height
        .is_some_and(|h| h <= crate::constants::RES_SD_HEIGHT_THRESHOLD)
    {
        "bt601"
    } else {
        "bt709"
    };
    if analysis.color_space.is_none() {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_COLOR_SPACE,
            &format!("Missing color space; defaulting to {color_fallback} based on resolution")
        );
    }
    VideoAnalysisBuilder::new()
        .basic(
            &analysis.codec,
            analysis
                .width
                .expect("analyze_video_quality guarantees width is Some"),
            analysis
                .height
                .expect("analyze_video_quality guarantees height is Some"),
            analysis.fps,
            analysis.duration_secs,
        )
        .file_size(analysis.file_size)
        .video_bitrate(
            analysis
                .video_bitrate
                .or(analysis.total_bitrate)
                .expect("analyze_video_quality guarantees total_bitrate is Some"),
        )
        .gop(gop_size, analysis.b_frame_count)
        .pix_fmt(&analysis.pix_fmt)
        .color(
            analysis.color_space.as_deref().unwrap_or(color_fallback),
            analysis.flags.features.is_hdr,
        )
        .content_type(analysis.content_type.to_content_type())
        .bit_depth(analysis.bit_depth)
        .build()
}

fn estimate_content_type(
    bpp: f64,
    codec_type: VideoCodecType,
    width: u32,
    height: u32,
    fps: f64,
) -> VideoContentType {
    let is_screen_res = (width == crate::constants::RES_FULL_HD_W
        && height == crate::constants::RES_FULL_HD_H)
        || (width == crate::constants::RES_QHD_W && height == crate::constants::RES_QHD_H)
        || (width == crate::constants::RES_4K_W && height == crate::constants::RES_4K_H);
    if is_screen_res && bpp < crate::constants::BPP_THRESHOLD_SCREEN_RECORDING {
        return VideoContentType::ScreenRecording;
    }

    if bpp < crate::constants::BPP_THRESHOLD_ANIMATION_HEURISTIC {
        return VideoContentType::Animation;
    }

    if codec_type == VideoCodecType::Intermediate
        && bpp > crate::constants::BPP_THRESHOLD_FILM_GRAIN
    {
        return VideoContentType::FilmGrain;
    }

    let is_1080_or_720 = (width == crate::constants::RES_FULL_HD_W
        && height == crate::constants::RES_FULL_HD_H)
        || (width == crate::constants::RES_HD_W && height == crate::constants::RES_HD_H);
    if fps >= crate::constants::FPS_THRESHOLD_GAMING
        && is_1080_or_720
        && (crate::constants::BPP_THRESHOLD_GAMING_LOW
            ..=crate::constants::BPP_THRESHOLD_GAMING_HIGH)
            .contains(&bpp)
    {
        return VideoContentType::Gaming;
    }

    if (crate::constants::BPP_THRESHOLD_LIVE_ACTION_LOW
        ..=crate::constants::BPP_THRESHOLD_LIVE_ACTION_HIGH)
        .contains(&bpp)
        && codec_type != VideoCodecType::Intermediate
    {
        return VideoContentType::LiveAction;
    }

    VideoContentType::Unknown
}

fn calculate_quality_score(
    bpp: f64,
    codec_type: VideoCodecType,
    bit_depth: Option<u8>,
    compression: CompressionLevel,
) -> u8 {
    let base = match compression {
        CompressionLevel::Lossless => crate::constants::QUALITY_SCORE_LOSSLESS,
        CompressionLevel::VisuallyLossless => crate::constants::QUALITY_SCORE_VISUALLY_LOSSLESS,
        CompressionLevel::HighQuality => crate::constants::QUALITY_SCORE_HIGH,
        CompressionLevel::Standard => crate::constants::QUALITY_SCORE_STANDARD,
        CompressionLevel::LowQuality => crate::constants::QUALITY_SCORE_LOW,
        CompressionLevel::Unknown => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_QUALITY,
                &format!(
                    "[ANOMALY] Unknown compression level; using neutral score ({})",
                    crate::constants::VIDEO_QUALITY_SCORE_NEUTRAL
                )
            );
            crate::constants::VIDEO_QUALITY_SCORE_NEUTRAL
        }
    };

    let depth_bonus = if bit_depth.is_some_and(|bd| bd >= crate::constants::HDR_BIT_DEPTH_THRESHOLD)
    {
        crate::constants::HDR_QUALITY_BONUS
    } else {
        0
    };

    let codec_bonus = if codec_type == VideoCodecType::ModernEfficient {
        crate::constants::QUALITY_SCORE_MODERN_CODEC_BONUS
    } else {
        0
    };

    let bpp_tweak = match compression {
        CompressionLevel::Standard => {
            let val = ((bpp - crate::constants::QUALITY_TWEAK_BPP_STANDARD_MIN)
                .clamp(0.0, crate::constants::QUALITY_TWEAK_BPP_RANGE)
                / crate::constants::QUALITY_TWEAK_BPP_RANGE
                * f64::from(crate::constants::QUALITY_TWEAK_MAX_BONUS))
            .round();
            let t = crate::numeric_cast::f64_to_u32_checked(val).unwrap_or_else(|| {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_ANOMALY,
                    &format!("Quality calculation anomaly: bpp_tweak NaN/Inf for bpp={bpp}")
                );
                0
            });
            // t is clamped to 0..=5, always fits u8.
            u8::try_from(t.clamp(0, crate::constants::QUALITY_TWEAK_STANDARD_MAX_TICK))
                .expect("Clamped value strictly bounded")
        }
        CompressionLevel::HighQuality => {
            let t = crate::numeric_cast::f64_to_u32_strict(
                ((bpp - crate::constants::QUALITY_TWEAK_BPP_HIGH_MIN)
                    .clamp(0.0, crate::constants::QUALITY_TWEAK_BPP_RANGE)
                    / crate::constants::QUALITY_TWEAK_BPP_RANGE
                    * crate::constants::QUALITY_TWEAK_HIGH_SCALE)
                    .round(),
                "bpp_tweak",
            )
            .unwrap_or_else(|| {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_ANOMALY,
                    &format!("Invalid BPP value {bpp} for tweak calculation")
                );
                1 // Default middle value
            });
            // t is clamped to 0..=3, always fits u8.
            u8::try_from(t.clamp(0, crate::constants::QUALITY_TWEAK_HIGH_MAX_TICK))
                .expect("Clamped value strictly bounded")
        }
        _ => 0,
    };

    (base + depth_bonus + codec_bonus + bpp_tweak).min(100)
}

fn estimate_crf_from_bpp(bpp: f64, codec_type: VideoCodecType) -> u8 {
    if codec_type == VideoCodecType::Lossless {
        return 0;
    }

    let efficiency = match codec_type {
        VideoCodecType::ModernEfficient => crate::constants::MODERN_EFFICIENT_CODEC_FACTOR,
        VideoCodecType::Intermediate => crate::constants::INTERMEDIATE_CODEC_FACTOR,
        VideoCodecType::Inefficient => crate::constants::INEFFICIENT_CODEC_FACTOR,
        _ => 1.0_f64,
    };

    let Some(bpp_r) = crate::numeric_cast::f64_to_rational_strict(bpp, "bpp") else {
        return crate::constants::FALLBACK_CRF_BPP_HEURISTIC;
    };
    let Some(efficiency_r) = crate::numeric_cast::f64_to_rational_strict(efficiency, "efficiency")
    else {
        return crate::constants::FALLBACK_CRF_BPP_HEURISTIC;
    };
    let adjusted_bpp = (bpp_r / efficiency_r).to_f64();

    for &(threshold, crf) in crate::constants::DENSITY_TO_CRF_LUT {
        if adjusted_bpp > threshold {
            return crf;
        }
    }
    crate::log_anomaly!(
        crate::static_logs::messages::LABEL_PHASE_3,
        &format!(
            "Video Analysis: BPP-to-CRF LUT failed for adjusted_bpp={adjusted_bpp:.4}; using fallback CRF ({})",
            crate::constants::FALLBACK_CRF_VIDEO
        )
    );
    crate::numeric_cast::f64_to_u8_strict(
        f64::from(crate::constants::FALLBACK_CRF_VIDEO),
        "fallback",
    )
    .expect("fallback CRF constant invalid")
}

fn calculate_video_confidence(
    has_video_bitrate: bool,
    has_gop_size: bool,
    duration: f64,
    frame_count: Option<u64>,
) -> f64 {
    let mut confidence: f64 = crate::constants::VIDEO_CONFIDENCE_BASE;

    if has_video_bitrate {
        confidence += crate::constants::VIDEO_CONFIDENCE_BITRATE_BONUS;
    }

    if has_gop_size {
        confidence += crate::constants::VIDEO_CONFIDENCE_GOP_BONUS;
    }

    if duration > crate::constants::VIDEO_CONFIDENCE_DURATION_THRESHOLD {
        confidence += crate::constants::VIDEO_CONFIDENCE_DURATION_BONUS;
    }

    if frame_count.is_some_and(|fc| fc > crate::constants::VIDEO_CONFIDENCE_FRAMES_THRESHOLD) {
        confidence += crate::constants::VIDEO_CONFIDENCE_FRAMES_BONUS;
    }

    confidence.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crf_estimation_boundaries() {
        // Test strict > logic for ModernEfficient (factor 0.5)
        // bpp 1.0 => adjusted_bpp 2.0. In original logic:
        // if bpp > 5.0 (14) else if bpp > 1.0 (18) else if bpp > 0.5 (22)
        // So 2.0 > 1.0 => 18.
        // Let's test the threshold 0.5 (factor 1.0 for simplicity)
        assert_eq!(estimate_crf_from_bpp(5.0, VideoCodecType::Unknown), 18); // 5.0 is not > 5.0, hits next
        assert_eq!(estimate_crf_from_bpp(5.0001, VideoCodecType::Unknown), 14);
        assert_eq!(estimate_crf_from_bpp(1.0, VideoCodecType::Unknown), 22); // 1.0 is not > 1.0, hits 0.5 threshold
        assert_eq!(estimate_crf_from_bpp(1.0001, VideoCodecType::Unknown), 18);
        assert_eq!(estimate_crf_from_bpp(0.08, VideoCodecType::Unknown), 35); // Original fallback
        assert_eq!(estimate_crf_from_bpp(0.0001, VideoCodecType::Unknown), 35);
    }

    #[test]
    fn test_analyze_h264_1080p() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: Some(7_500_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: Some(60),
            color_space: Some("bt709"),
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.width, Some(1920));
        assert_eq!(result.height, Some(1080));
        assert_eq!(result.codec_type, VideoCodecType::Legacy);
        assert!(!result.flags.decision.is_modern_codec);
        assert!(!result.flags.decision.should_skip);
        assert!(result.bpp > 0.0_f64);
    }

    #[test]
    fn test_analyze_hevc_4k() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "hevc",
            width: Some(3840),
            height: Some(2160),
            fps: Some(30.0),
            duration_secs: Some(120.0),
            total_bitrate: Some(20_000_000),
            video_bitrate: Some(19_000_000),
            pix_fmt: "yuv420p10le",
            bit_depth: Some(10),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: Some(60),
            color_space: Some("bt2020nc"),
            file_size: 300_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.codec_type, VideoCodecType::ModernEfficient);
        assert!(result.flags.decision.is_modern_codec);
        assert!(result.flags.decision.should_skip, "HEVC should be skipped");
        assert!(
            result.flags.features.is_hdr,
            "BT.2020 should be detected as HDR"
        );
        assert_eq!(result.bit_depth, Some(10));
    }

    #[test]
    fn test_analyze_av1() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "av1",
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            duration_secs: Some(90.0),
            total_bitrate: Some(5_000_000),
            video_bitrate: Some(4_800_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: Some(120),
            color_space: None,
            file_size: 56_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.codec_type, VideoCodecType::ModernEfficient);
        assert!(
            result.flags.decision.should_skip,
            "AV1 skipped in normal mode (use Apple-compat to convert)"
        );
    }

    #[test]
    fn test_analyze_prores() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "prores",
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(150_000_000),
            video_bitrate: Some(145_000_000),
            pix_fmt: "yuv422p10le",
            bit_depth: Some(10),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: Some(1),
            color_space: Some("bt709"),
            file_size: 1_125_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.codec_type, VideoCodecType::Intermediate);
        assert!(
            !result.flags.decision.should_skip,
            "ProRes should not be skipped"
        );
        assert_eq!(result.chroma, ChromaSubsampling::Yuv422);
        assert!(result.bpp > 1.0_f64, "ProRes should have high BPP");
    }

    #[test]
    fn test_analyze_ffv1_lossless() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "ffv1",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(30.0),
            total_bitrate: Some(200_000_000),
            video_bitrate: Some(195_000_000),
            pix_fmt: "yuv444p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: Some(1),
            color_space: None,
            file_size: 750_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.codec_type, VideoCodecType::Lossless);
        assert_eq!(result.compression_type, CompressionLevel::Lossless);
        assert!(!result.flags.decision.should_skip);
        assert_eq!(result.chroma, ChromaSubsampling::Yuv444);
    }

    #[test]
    fn test_skip_modern_codecs() {
        for codec in ["hevc", "av1", "vp9", "vvc"] {
            let result = analyze_video_quality(VideoQualityInput {
                codec,
                width: Some(1920),
                height: Some(1080),
                fps: Some(30.0),
                duration_secs: Some(60.0),
                total_bitrate: Some(8_000_000),
                video_bitrate: None,
                pix_fmt: "yuv420p",
                bit_depth: Some(8),
                max_b_frames: Some(2),
                encoder_params: None,
                gop_size: None,
                color_space: None,
                file_size: 60_000_000,
                frame_count: None,
            })
            .unwrap_or_else(|e| panic!("{e}"));
            assert!(
                result.flags.decision.should_skip,
                "{codec} skipped in normal mode (modern format)"
            );
        }
    }

    #[test]
    fn test_not_skip_legacy_codecs() {
        let h264 = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !h264.flags.decision.should_skip,
            "H.264 should NOT be skipped"
        );

        let mjpeg = analyze_video_quality(VideoQualityInput {
            codec: "mjpeg",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(50_000_000),
            video_bitrate: None,
            pix_fmt: "yuvj420p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 375_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !mjpeg.flags.decision.should_skip,
            "MJPEG should NOT be skipped"
        );

        let prores = analyze_video_quality(VideoQualityInput {
            codec: "prores",
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(150_000_000),
            video_bitrate: None,
            pix_fmt: "yuv422p10le",
            bit_depth: Some(10),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_125_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !prores.flags.decision.should_skip,
            "ProRes should NOT be skipped"
        );
    }

    #[test]
    fn test_chroma_detection() {
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("yuv420p"),
            ChromaSubsampling::Yuv420
        );
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("yuv420p10le"),
            ChromaSubsampling::Yuv420
        );
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("yuv422p"),
            ChromaSubsampling::Yuv422
        );
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("yuv411p"),
            ChromaSubsampling::Yuv422
        );
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("yuv410p"),
            ChromaSubsampling::Yuv420
        );
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("yuv444p"),
            ChromaSubsampling::Yuv444
        );
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("rgb24"),
            ChromaSubsampling::Rgb
        );
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("gbrp"),
            ChromaSubsampling::Rgb
        );
        assert_eq!(
            ChromaSubsampling::from_pix_fmt("nv12"),
            ChromaSubsampling::Yuv420
        );
    }

    #[test]
    fn test_chroma_quality_factor() {
        assert!((ChromaSubsampling::Yuv420.quality_factor() - 1.0).abs() < 0.01_f64);
        assert!(ChromaSubsampling::Yuv422.quality_factor() > 1.0_f64);
        assert!(
            ChromaSubsampling::Yuv444.quality_factor() > ChromaSubsampling::Yuv422.quality_factor()
        );
        assert!(
            ChromaSubsampling::Rgb.quality_factor() > ChromaSubsampling::Yuv444.quality_factor()
        );
    }

    #[test]
    fn test_bpp_calculation_accuracy() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: Some(8_000_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        let expected_bpp = 8_000_000.0_f64 / (1_920.0_f64 * 1_080.0_f64 * 30.0_f64);
        assert!(
            (result.bpp - expected_bpp).abs() < 0.001_f64,
            "BPP calculation error: expected {}, got {}",
            expected_bpp,
            result.bpp
        );
    }

    #[test]
    fn test_bpp_uses_video_bitrate_when_available() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(10_000_000),
            video_bitrate: Some(8_000_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 75_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        let expected_bpp = 8_000_000.0_f64 / (1_920.0_f64 * 1_080.0_f64 * 30.0_f64);
        assert!(
            (result.bpp - expected_bpp).abs() < 0.001_f64,
            "Should use video_bitrate for BPP: expected {}, got {}",
            expected_bpp,
            result.bpp
        );
    }

    #[test]
    fn test_compression_level_lossless() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "ffv1",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(200_000_000),
            video_bitrate: None,
            pix_fmt: "yuv444p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_500_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.compression_type, CompressionLevel::Lossless);
    }

    #[test]
    fn test_compression_level_high_bpp() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "prores",
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(150_000_000),
            video_bitrate: None,
            pix_fmt: "yuv422p10le",
            bit_depth: Some(10),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_125_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.compression_type, CompressionLevel::VisuallyLossless);
    }

    #[test]
    fn test_compression_level_standard() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.compression_type == CompressionLevel::Standard
                || result.compression_type == CompressionLevel::HighQuality,
            "8Mbps 1080p should be Standard/HighQuality, got {:?}",
            result.compression_type
        );
    }

    #[test]
    fn test_compression_level_low_quality() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(3_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 22_500_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(
            result.compression_type,
            CompressionLevel::LowQuality,
            "3Mbps 1080p should be LowQuality, got {:?}",
            result.compression_type
        );
    }

    #[test]
    fn test_crf_estimation_high_quality() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(20_000_000),
            video_bitrate: Some(19_000_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 150_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.estimated_crf <= 25,
            "High bitrate should estimate low CRF, got {}",
            result.estimated_crf
        );
    }

    #[test]
    fn test_crf_estimation_low_quality() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(1_000_000),
            video_bitrate: Some(900_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 7_500_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.estimated_crf >= 30,
            "Low bitrate should estimate high CRF, got {}",
            result.estimated_crf
        );
    }

    #[test]
    fn test_crf_lossless_is_zero() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "ffv1",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(200_000_000),
            video_bitrate: None,
            pix_fmt: "yuv444p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_500_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.estimated_crf, 0, "Lossless should have CRF 0");
    }

    #[test]
    fn test_hdr_detection_bt2020() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "hevc",
            width: Some(3840),
            height: Some(2160),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(25_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p10le",
            bit_depth: Some(10),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: Some("bt2020nc"),
            file_size: 187_500_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.flags.features.is_hdr,
            "BT.2020 should be detected as HDR"
        );
    }

    #[test]
    fn test_hdr_detection_bt709_not_hdr() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: Some("bt709"),
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            !result.flags.features.is_hdr,
            "BT.709 should NOT be detected as HDR"
        );
    }

    #[test]
    fn test_hdr_detection_none_not_hdr() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            !result.flags.features.is_hdr,
            "No color space should NOT be detected as HDR"
        );
    }

    #[test]
    fn test_skip_modern_codec() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "hevc",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            result.flags.decision.should_skip,
            "HEVC should be marked skip by should_skip_video_codec"
        );
    }

    #[test]
    fn test_lossless_source_not_skipped() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "ffv1",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(200_000_000),
            video_bitrate: None,
            pix_fmt: "yuv444p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_500_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(!result.flags.decision.should_skip);
    }

    #[test]
    fn test_prores_analysis() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "prores",
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(150_000_000),
            video_bitrate: None,
            pix_fmt: "yuv422p10le",
            bit_depth: Some(10),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_125_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(!result.flags.decision.should_skip);
        assert_eq!(result.codec_type, VideoCodecType::Intermediate);
    }

    #[test]
    fn test_h264_analysis() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(!result.flags.decision.should_skip);
    }

    #[test]
    fn test_invalid_zero_width() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(0),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        });

        assert!(result.is_err(), "Should fail on zero width");
        assert!(
            result
                .err()
                .unwrap_or_default()
                .contains("Invalid dimensions")
        );
    }

    #[test]
    fn test_invalid_zero_height() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(0),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        });

        assert!(result.is_err(), "Should fail on zero height");
        assert!(
            result
                .err()
                .unwrap_or_default()
                .contains("Invalid dimensions")
        );
    }

    #[test]
    fn test_invalid_zero_fps() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(0.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        });

        assert!(result.is_err(), "Should fail on zero fps");
        assert!(
            result
                .err()
                .unwrap_or_default()
                .contains("Invalid frame rate")
        );
    }

    #[test]
    fn test_invalid_negative_fps() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(-30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        });

        assert!(result.is_err(), "Should fail on negative fps");
    }

    #[test]
    fn test_invalid_zero_duration() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(0.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        });

        assert!(result.is_err(), "Should fail on zero duration");
        assert!(
            result
                .err()
                .unwrap_or_default()
                .contains("Invalid duration")
        );
    }

    #[test]
    fn test_invalid_negative_duration() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(-60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        });

        assert!(result.is_err(), "Should fail on negative duration");
    }

    #[test]
    fn test_extreme_low_bitrate() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(100_000),
            video_bitrate: Some(90_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 750_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.bpp < 0.01_f64,
            "Very low bitrate should have very low BPP"
        );
        assert_eq!(result.compression_type, CompressionLevel::LowQuality);
        assert!(
            result.estimated_crf >= 32,
            "Low bitrate should estimate high CRF"
        );
    }

    #[test]
    fn test_extreme_high_bitrate() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(500_000_000),
            video_bitrate: Some(490_000_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 3_750_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.bpp > 5.0_f64,
            "Very high bitrate should have high BPP"
        );
        assert!(
            result.compression_type == CompressionLevel::VisuallyLossless
                || result.compression_type == CompressionLevel::HighQuality,
            "High bitrate should be VisuallyLossless or HighQuality"
        );
    }

    #[test]
    fn test_resolution_sd_480p() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(854),
            height: Some(480),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(2_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 15_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.width, Some(854));
        assert_eq!(result.height, Some(480));
        let expected_bpp = 2_000_000.0_f64 / (854.0_f64 * 480.0_f64 * 30.0_f64);
        assert!((result.bpp - expected_bpp).abs() < 0.001_f64);
    }

    #[test]
    fn test_resolution_hd_720p() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(5_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 37_500_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.width, Some(1280));
        assert_eq!(result.height, Some(720));
    }

    #[test]
    fn test_resolution_4k_uhd() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "hevc",
            width: Some(3840),
            height: Some(2160),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(25_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p10le",
            bit_depth: Some(10),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 187_500_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.width, Some(3840));
        assert_eq!(result.height, Some(2160));
        assert!(
            result.flags.decision.should_skip,
            "4K HEVC should be skipped"
        );
    }

    #[test]
    fn test_resolution_8k() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "av1",
            width: Some(7680),
            height: Some(4320),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(80_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p10le",
            bit_depth: Some(10),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 600_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.width, Some(7680));
        assert_eq!(result.height, Some(4320));
        assert!(
            result.flags.decision.should_skip,
            "8K AV1 skipped in normal mode"
        );
    }

    #[test]
    fn test_resolution_vertical_video() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1080),
            height: Some(1920),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.width, Some(1080));
        assert_eq!(result.height, Some(1920));
        assert!(!result.flags.decision.should_skip);
    }

    #[test]
    fn test_resolution_square() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1080),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(6_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 45_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(result.width, Some(1080));
        assert_eq!(result.height, Some(1080));
    }

    #[test]
    fn test_fps_24_film() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: Some(1440),
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(result.fps.is_some_and(|f| (f - 24.0).abs() < 0.01_f64));
        assert_eq!(result.frame_count, Some(1440));
    }

    #[test]
    fn test_fps_60_gaming() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(60.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(15_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 112_500_000,
            frame_count: Some(3600),
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(result.fps.is_some_and(|f| (f - 60.0).abs() < 0.01_f64));
        assert_eq!(result.frame_count, Some(3600));
    }

    #[test]
    fn test_fps_120_high_refresh() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(120.0),
            duration_secs: Some(30.0),
            total_bitrate: Some(25_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 93_750_000,
            frame_count: Some(3600),
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(result.fps.is_some_and(|f| (f - 120.0).abs() < 0.01_f64));
        assert_eq!(result.frame_count, Some(3600));
    }

    #[test]
    fn test_fps_fractional_ntsc() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(29.97),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(result.fps.is_some_and(|f| (f - 29.97).abs() < 0.01_f64));
    }

    #[test]
    fn test_codec_type_lossless() {
        let ffv1 = analyze_video_quality(VideoQualityInput {
            codec: "ffv1",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(200_000_000),
            video_bitrate: None,
            pix_fmt: "yuv444p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_500_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ffv1.codec_type, VideoCodecType::Lossless);

        let huffyuv = analyze_video_quality(VideoQualityInput {
            codec: "huffyuv",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(300_000_000),
            video_bitrate: None,
            pix_fmt: "yuv422p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 2_250_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(huffyuv.codec_type, VideoCodecType::Lossless);

        let utvideo = analyze_video_quality(VideoQualityInput {
            codec: "utvideo",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(250_000_000),
            video_bitrate: None,
            pix_fmt: "yuv422p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_875_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(utvideo.codec_type, VideoCodecType::Lossless);
    }

    #[test]
    fn test_codec_type_modern() {
        let codecs = ["av1", "hevc", "h265", "vp9", "vvc"];
        for codec in codecs {
            let result = analyze_video_quality(VideoQualityInput {
                codec,
                width: Some(1920),
                height: Some(1080),
                fps: Some(30.0),
                duration_secs: Some(60.0),
                total_bitrate: Some(8_000_000),
                video_bitrate: None,
                pix_fmt: "yuv420p",
                bit_depth: Some(8),
                max_b_frames: Some(2),
                encoder_params: None,
                gop_size: None,
                color_space: None,
                file_size: 60_000_000,
                frame_count: None,
            })
            .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(
                result.codec_type,
                VideoCodecType::ModernEfficient,
                "Codec {codec} should be ModernEfficient"
            );
        }
    }

    #[test]
    fn test_codec_type_intermediate() {
        let prores = analyze_video_quality(VideoQualityInput {
            codec: "prores",
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(150_000_000),
            video_bitrate: None,
            pix_fmt: "yuv422p10le",
            bit_depth: Some(10),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 1_125_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(prores.codec_type, VideoCodecType::Intermediate);

        let dnxhd = analyze_video_quality(VideoQualityInput {
            codec: "dnxhd",
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(120_000_000),
            video_bitrate: None,
            pix_fmt: "yuv422p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 900_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(dnxhd.codec_type, VideoCodecType::Intermediate);
    }

    #[test]
    fn test_codec_type_inefficient() {
        let mjpeg = analyze_video_quality(VideoQualityInput {
            codec: "mjpeg",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(50_000_000),
            video_bitrate: None,
            pix_fmt: "yuvj420p",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 375_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(mjpeg.codec_type, VideoCodecType::Inefficient);

        let gif = analyze_video_quality(VideoQualityInput {
            codec: "gif",
            width: Some(640),
            height: Some(480),
            fps: Some(15.0),
            duration_secs: Some(10.0),
            total_bitrate: Some(5_000_000),
            video_bitrate: None,
            pix_fmt: "rgb8",
            bit_depth: Some(8),
            max_b_frames: Some(0),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 6_250_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(gif.codec_type, VideoCodecType::Inefficient);
    }

    #[test]
    fn test_confidence_with_video_bitrate() {
        let with_vbr = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(10_000_000),
            video_bitrate: Some(8_000_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: Some(60),
            color_space: None,
            file_size: 75_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        let without_vbr = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(10_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 75_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            with_vbr.confidence > without_vbr.confidence,
            "Video bitrate should increase confidence: {} vs {}",
            with_vbr.confidence,
            without_vbr.confidence
        );
    }

    #[test]
    fn test_confidence_with_gop_size() {
        let with_gop = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: Some(60),
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        let without_gop = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            with_gop.confidence > without_gop.confidence,
            "GOP size should increase confidence"
        );
    }

    #[test]
    fn test_confidence_longer_duration() {
        let long = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(120.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 120_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        let short = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(5.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: None,
            color_space: None,
            file_size: 5_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            long.confidence >= short.confidence,
            "Longer duration should have >= confidence"
        );
    }

    #[test]
    fn test_to_quality_analysis_conversion() {
        let analysis = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: Some(7_500_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: Some(60),
            color_space: Some("bt709"),
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        let qa = to_quality_analysis(&analysis);

        assert_eq!(qa.width, 1920);
        assert_eq!(qa.height, 1080);
        assert!((qa.fps.unwrap_or_else(|| panic!("missing fps")) - 30.0).abs() < 0.01_f64);
        assert!(
            (qa.duration_secs
                .unwrap_or_else(|| panic!("missing duration"))
                - 60.0)
                .abs()
                < 0.01_f64
        );
        assert_eq!(qa.video_bitrate, Some(7_500_000));
    }

    #[test]
    fn test_consistency_same_input() {
        let result1 = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: Some(7_500_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: Some(60),
            color_space: Some("bt709"),
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        let result2 = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(8_000_000),
            video_bitrate: Some(7_500_000),
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: None,
            gop_size: Some(60),
            color_space: Some("bt709"),
            file_size: 60_000_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            (result1.bpp - result2.bpp).abs() < 0.000_1_f64,
            "Same input should produce same BPP"
        );
        assert_eq!(result1.codec_type, result2.codec_type);
        assert_eq!(
            result1.flags.decision.should_skip,
            result2.flags.decision.should_skip
        );
        assert_eq!(result1.estimated_crf, result2.estimated_crf);
    }

    #[test]
    fn test_strict_bpp_formula() {
        let test_cases = [
            (1920, 1080, 30.0_f64, 8_000_000u64),
            (3840, 2160, 30.0_f64, 25_000_000u64),
            (1280, 720, 60.0_f64, 5_000_000u64),
            (854, 480, 24.0_f64, 2_000_000u64),
        ];

        for (w, h, fps, bitrate) in test_cases {
            let result = analyze_video_quality(VideoQualityInput {
                codec: "h264",
                width: Some(w),
                height: Some(h),
                fps: Some(fps),
                duration_secs: Some(60.0),
                total_bitrate: Some(bitrate),
                video_bitrate: Some(bitrate),
                pix_fmt: "yuv420p",
                bit_depth: Some(8),
                max_b_frames: Some(2),
                encoder_params: None,
                gop_size: None,
                color_space: None,
                file_size: bitrate * 60 / 8,
                frame_count: None,
            })
            .unwrap_or_else(|e| panic!("{e}"));

            let expected = f64::from(
                u32::try_from(bitrate)
                    .expect("Value overflowed or is missing, cannot process ratio"),
            ) / (f64::from(w) * f64::from(h) * fps);
            assert!(
                (result.bpp - expected).abs() < 0.000_1_f64,
                "STRICT: BPP for {}x{}@{}fps@{}bps: expected {}, got {}",
                w,
                h,
                fps,
                bitrate,
                expected,
                result.bpp
            );
        }
    }

    #[test]
    fn test_strict_frame_count() {
        let test_cases = [
            (30.0_f64, 60.0_f64, 1800u64),
            (24.0_f64, 120.0_f64, 2880u64),
            (60.0_f64, 30.0_f64, 1800u64),
        ];

        for (fps, duration, expected_frames) in test_cases {
            let result = analyze_video_quality(VideoQualityInput {
                codec: "hevc",
                width: Some(1920),
                height: Some(1080),
                fps: Some(fps),
                duration_secs: Some(duration),
                total_bitrate: Some(8_000_000),
                video_bitrate: None,
                pix_fmt: "yuv420p",
                bit_depth: Some(8),
                max_b_frames: Some(2),
                encoder_params: None,
                gop_size: None,
                color_space: None,
                file_size: 60_000_000,
                frame_count: Some(expected_frames),
            })
            .unwrap_or_else(|e| panic!("{e}"));

            assert_eq!(
                result.frame_count,
                Some(expected_frames),
                "STRICT: Frame count for {}fps * {}s: expected {:?}, got {:?}",
                fps,
                duration,
                expected_frames,
                result.frame_count
            );
        }
    }

    #[test]
    fn test_strict_modern_always_skip() {
        // Normal mode: all modern codecs skipped (use Apple-compat to convert AV1/VP9/VVC/AV2).
        let modern_skip = [
            ("hevc", true),
            ("h265", true),
            ("av1", true),
            ("vp9", true),
            ("vvc", true),
            ("av2", true),
        ];

        for (codec, expected_skip) in modern_skip {
            let result = analyze_video_quality(VideoQualityInput {
                codec,
                width: Some(1920),
                height: Some(1080),
                fps: Some(30.0),
                duration_secs: Some(60.0),
                total_bitrate: Some(8_000_000),
                video_bitrate: None,
                pix_fmt: "yuv420p",
                bit_depth: Some(8),
                max_b_frames: Some(2),
                encoder_params: None,
                gop_size: None,
                color_space: None,
                file_size: 60_000_000,
                frame_count: None,
            })
            .unwrap_or_else(|e| panic!("{e}"));

            assert_eq!(
                result.flags.decision.should_skip, expected_skip,
                "STRICT: {} expected skip={}, got {}",
                codec, expected_skip, result.flags.decision.should_skip
            );
            assert!(
                result.flags.decision.is_modern_codec,
                "STRICT: {codec} must be detected as modern"
            );
        }
    }

    #[test]
    /// Codecs that are Legacy (h264, mpeg4, mpeg2video) or Inefficient (mjpeg) must not be skipped.
    fn test_strict_legacy_never_skip() {
        let non_modern_codecs = ["h264", "mpeg4", "mpeg2video", "mjpeg"];

        for codec in non_modern_codecs {
            let result = analyze_video_quality(VideoQualityInput {
                codec,
                width: Some(1920),
                height: Some(1080),
                fps: Some(30.0),
                duration_secs: Some(60.0),
                total_bitrate: Some(8_000_000),
                video_bitrate: None,
                pix_fmt: "yuv420p",
                bit_depth: Some(8),
                max_b_frames: Some(2),
                encoder_params: None,
                gop_size: None,
                color_space: None,
                file_size: 60_000_000,
                frame_count: None,
            })
            .unwrap_or_else(|e| panic!("{e}"));

            assert!(
                !result.flags.decision.should_skip,
                "Non-modern codec {codec} (Legacy or Inefficient) must NEVER skip"
            );
        }
    }

    #[test]
    fn test_extract_crf_from_params() {
        assert_eq!(extract_crf_from_params("crf=23.5"), Some(24u8));
        assert_eq!(extract_crf_from_params("x265 [info]: CRF 18.0"), Some(18u8));
        assert_eq!(extract_crf_from_params("... crf=20 ..."), Some(20u8));
        assert_eq!(extract_crf_from_params("no params here"), None);
    }

    #[test]
    fn test_analyze_video_quality_with_deterministic_crf() {
        let result = analyze_video_quality(VideoQualityInput {
            codec: "h264",
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            total_bitrate: Some(1_000_000),
            video_bitrate: None,
            pix_fmt: "yuv420p",
            bit_depth: Some(8),
            max_b_frames: Some(2),
            encoder_params: Some("rc=crf / crf=15.0 / preset=medium"),
            gop_size: None,
            color_space: None,
            file_size: 7_500_000,
            frame_count: None,
        })
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(
            result.estimated_crf, 15,
            "Should use CRF from encoder_params"
        );
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_crf_monotonicity(bpp1 in 0.01_f64..10.0f64, bpp2 in 0.01_f64..10.0f64) {
            let crf1 = estimate_crf_from_bpp(bpp1, VideoCodecType::ModernEfficient);
            let crf2 = estimate_crf_from_bpp(bpp2, VideoCodecType::ModernEfficient);

            if bpp1 > bpp2 {
                prop_assert!(crf1 <= crf2, "Higher BPP must result in lower or equal CRF (bpp1={}, bpp2={}, crf1={}, crf2={})", bpp1, bpp2, crf1, crf2);
            } else if bpp1 < bpp2 {
                prop_assert!(crf1 >= crf2, "Lower BPP must result in higher or equal CRF (bpp1={}, bpp2={}, crf1={}, crf2={})", bpp1, bpp2, crf1, crf2);
            }
        }
    }
}
