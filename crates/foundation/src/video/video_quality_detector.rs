//! 🔬 Video Quality Detector - Precision-Validated Video Analysis for Auto Routing
//!
//! This module provides unified video quality detection for:
//! - Auto format routing decisions (AV1/HEVC/FFV1)
//! - Quality matching (CRF calculation)
//! - Codec skip decisions
//!
//! ## Quality Manifesto Compliance
//! - NO silent fallback - errors fail loudly
//! - NO hardcoded defaults - all from actual ffprobe analysis
//! - Base decisions on actual content detection, not format names
//!
//! ## Integration with `quality_matcher`
//! This module provides the detection layer, while `quality_matcher`
//! provides the CRF calculation layer.

use crate::quality_matcher::{
    ContentType, QualityAnalysis, SourceCodec, VideoAnalysisBuilder, parse_source_codec,
    should_skip_video_codec,
};
use crate::video_detection::Detection;
use crate::{BitDepthMetadata, MediaPrecision};
use crate::{ffprobe_json::ColorInfoAssessment, ffprobe_json::pix_fmt_indicates_float};
use rug::Rational;
use serde::{Deserialize, Serialize};

use std::path::Path;

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
    pub bit_depth_inferred_from_pix_fmt: bool,

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

impl MediaPrecision for VideoQualityAnalysis {
    fn bit_depth_metadata(&self) -> BitDepthMetadata {
        BitDepthMetadata::new(self.bit_depth, self.bit_depth_inferred_from_pix_fmt)
    }

    fn has_hdr_signaling(&self) -> bool {
        self.flags.features.is_hdr
    }
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
            SourceCodec::Vp8
            | SourceCodec::Mpeg4
            | SourceCodec::Mpeg2
            | SourceCodec::Mpeg1
            | SourceCodec::Wmv
            | SourceCodec::Theora
            | SourceCodec::RealVideo
            | SourceCodec::FlashVideo
            | SourceCodec::RawVideo
            | SourceCodec::Lagarith
            | SourceCodec::MagicYuv
            | SourceCodec::Apng
            | SourceCodec::WebpAnimated
            | SourceCodec::Jpeg
            | SourceCodec::JpegXl
            | SourceCodec::Png
            | SourceCodec::WebpStatic
            | SourceCodec::Avif
            | SourceCodec::Heic
            | SourceCodec::Bmp
            | SourceCodec::Tiff
            | SourceCodec::Unknown => Self::Unknown,
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
#[allow(clippy::struct_excessive_bools)]
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
    pub bit_depth_inferred_from_pix_fmt: bool,
    pub max_b_frames: Option<u8>,
    pub encoder_params: Option<&'a str>,
    pub gop_size: Option<u32>,
    pub color_space: Option<&'a str>,
    pub color_transfer: Option<&'a str>,
    pub color_primaries: Option<&'a str>,
    pub has_mastering_display: bool,
    pub has_max_cll: bool,
    pub is_dolby_vision: bool,
    pub is_hdr10_plus: bool,
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
            VideoCodecType::Lossless
            | VideoCodecType::Legacy
            | VideoCodecType::Intermediate
            | VideoCodecType::Unknown => 1.0_f64,
        };

        let Some(bpp_r) = f64_to_rational_strict(bpp, "bpp") else {
            crate::media_conversion_gate::probe_quality_batch_audit(
                "video_quality_bpp_rational_failed",
                crate::infra::static_logs::messages::MSG_VQD_BPP_ANOMALY,
            );
            return Self::Unknown;
        };
        let Some(efficiency_r) = f64_to_rational_strict(efficiency, "efficiency") else {
            crate::media_conversion_gate::probe_quality_batch_audit(
                "video_quality_efficiency_rational_failed",
                crate::infra::static_logs::messages::MSG_VQD_EFFICIENCY_ANOMALY,
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
        bit_depth_inferred_from_pix_fmt,
        max_b_frames,
        encoder_params,
        gop_size,
        color_space,
        color_transfer,
        color_primaries,
        has_mastering_display,
        has_max_cll,
        is_dolby_vision,
        is_hdr10_plus,
        file_size,
        frame_count: input_frame_count,
    } = input;

    let w = width
        .ok_or_else(|| crate::infra::static_logs::messages::MSG_VQD_MISSING_WIDTH.to_string())?;
    let h = height
        .ok_or_else(|| crate::infra::static_logs::messages::MSG_VQD_MISSING_HEIGHT.to_string())?;
    if w == 0 || h == 0 {
        return Err(crate::infra::static_logs::messages::MSG_VQD_INVALID_DIM.to_string());
    }
    let fps_val =
        fps.ok_or_else(|| crate::infra::static_logs::messages::MSG_VQD_MISSING_FPS.to_string())?;
    if fps_val <= 0.0_f64 {
        return Err(crate::infra::static_logs::messages::MSG_VQD_INVALID_FPS.to_string());
    }
    let dur = duration_secs
        .ok_or_else(|| crate::infra::static_logs::messages::MSG_VQD_MISSING_DUR.to_string())?;
    if dur <= 0.0_f64 {
        return Err(crate::infra::static_logs::messages::MSG_VQD_INVALID_DUR.to_string());
    }

    let source_codec = parse_source_codec(codec);
    let codec_type = VideoCodecType::from_source_codec(source_codec);
    let is_modern = source_codec.is_modern();

    let skip_decision = should_skip_video_codec(codec);

    let effective_bitrate = match video_bitrate {
        Some(v) => Some(v),
        None => {
            match total_bitrate {
                Some(tb) if tb > 0 => Some(tb),
                _ => {
                    // Calculate from file size: (file_size * 8) / duration
                    let bits = crate::numeric_cast::u64_to_f64(file_size) * 8.0_f64;
                    crate::numeric_cast::f64_to_u64_strict(bits / dur, "derived_bitrate")
                }
            }
        }
    }
    .ok_or_else(|| crate::infra::static_logs::messages::MSG_VQD_MISSING_BITRATE.to_string())?;
    let bpp = {
        let pixels_per_second = Rational::from(w) * Rational::from(h) * {
            let Some(fps_r) = crate::numeric_cast::f64_to_rational_strict(fps_val, "fps") else {
                return Err(
                    crate::infra::static_logs::messages::MSG_VQD_INVALID_FPS_NUM.to_string()
                );
            };
            fps_r
        };
        if pixels_per_second > 0 {
            let bits_per_second = Rational::from_f64(crate::numeric_cast::u64_to_f64(
                effective_bitrate,
            ))
            .ok_or_else(|| {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "video_quality_bitrate_overflow",
                    format!("bitrate overflow in BPP calculation: {effective_bitrate}"),
                );
                crate::media_conversion_gate::ui_quality_user_error(
                    "Bitrate overflow: cannot convert to Rational",
                )
            })?;
            (bits_per_second / pixels_per_second).to_f64()
        } else {
            0.0_f64
        }
    };

    let chroma = ChromaSubsampling::from_pix_fmt(pix_fmt);
    let color_assessment = ColorInfoAssessment::from_probe_fields(
        color_space,
        color_transfer,
        color_primaries,
        BitDepthMetadata::new(bit_depth, bit_depth_inferred_from_pix_fmt),
        crate::ffprobe_json::ColorProbeFlags {
            has_mastering_display,
            has_max_cll,
            is_dolby_vision,
            is_hdr10_plus,
            is_float: pix_fmt_indicates_float(Some(pix_fmt)),
        },
    );
    let is_hdr = color_assessment.has_hdr_signaling();

    let has_b_frames = max_b_frames.is_some_and(|b| b > 0);
    let b_frame_count = max_b_frames;

    let content_type = estimate_content_type(bpp, codec_type, w, h, fps_val);

    let compression_type = CompressionLevel::from_bpp(bpp, codec_type);

    let quality_score = calculate_quality_score(
        bpp,
        codec_type,
        bit_depth,
        bit_depth_inferred_from_pix_fmt,
        compression_type,
    )?;

    // Prioritize precise CRF/QP from encoder tags over BPP heuristic
    let estimated_crf = match encoder_params {
        None => estimate_crf_from_bpp(bpp, codec_type),
        Some(params) => {
            let parsed_crf = extract_crf_from_params(params)?;
            crate::media_conversion_gate::probe_video_crf_from_params_or_estimate(
                parsed_crf,
                estimate_crf_from_bpp(bpp, codec_type),
            )
        }
    };

    let confidence = calculate_video_confidence(
        video_bitrate.is_some(),
        gop_size.is_some(),
        dur,
        input_frame_count,
    );

    finalize_video_quality_analysis(VideoQualityAnalysis {
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
        bit_depth_inferred_from_pix_fmt,
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

/// Terminal contract for [`VideoQualityAnalysis`] before routing / CRF prediction.
fn finalize_video_quality_analysis(
    mut analysis: VideoQualityAnalysis,
) -> Result<VideoQualityAnalysis, String> {
    let bpp = crate::algorithm_seal::seal_non_negative_finite(analysis.bpp).ok_or_else(|| {
        format!(
            "Video quality analysis rejected: non-finite BPP ({})",
            analysis.bpp
        )
    })?;
    let confidence = crate::algorithm_seal::quality_unit_probability(analysis.confidence)
        .ok_or_else(|| {
            format!(
                "Video quality analysis rejected: non-finite confidence ({})",
                analysis.confidence
            )
        })?;
    analysis.bpp = bpp;
    analysis.confidence = confidence;
    analysis.quality_score = crate::algorithm_seal::seal_u8_quality_display(analysis.quality_score);
    analysis.estimated_crf = crate::algorithm_seal::seal_u8_crf_setpoint(analysis.estimated_crf);
    Ok(analysis)
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
    let mut analysis = analyze_video_quality(VideoQualityInput {
        codec: detection.codec.as_str(),
        width: detection.width,
        height: detection.height,
        fps: detection.fps,
        duration_secs: detection.duration_secs,
        total_bitrate: detection.bitrate,
        video_bitrate: detection.video_bitrate,
        pix_fmt: &detection.pix_fmt,
        bit_depth: detection.bit_depth,
        bit_depth_inferred_from_pix_fmt: detection.precision.bit_depth_inferred_from_pix_fmt,
        max_b_frames: detection.max_b_frames,
        encoder_params: detection.encoder_params.as_deref(),
        gop_size: None,
        color_space: Some(detection.color_space.as_str()),
        color_transfer: detection.color_transfer.as_deref(),
        color_primaries: detection.color_primaries.as_deref(),
        has_mastering_display: detection.mastering_display.is_some(),
        has_max_cll: detection.max_cll.is_some(),
        is_dolby_vision: detection.flags.hdr.is_dolby_vision,
        is_hdr10_plus: detection.flags.hdr.is_hdr10_plus,
        file_size: detection.file_size,
        frame_count: detection.frame_count,
    })?;

    if !matches!(
        detection.compression,
        crate::video_detection::CompressionType::Lossless
            | crate::video_detection::CompressionType::VisuallyLossless
    ) {
        let path = Path::new(&detection.file_path);
        if crate::algorithm_runtime::quality_db_lookup_enabled("video_quality_detector")
            && let Some(prediction) =
                crate::scenario_quality_lookup::lookup_media_quality_by_path(path)
            && let Some(fused) =
                crate::image_quality_db::fuse_quality_regression_prediction_if_enabled(
                    "video_quality_detector",
                    Some(analysis.quality_score),
                    prediction,
                )
        {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "video_quality_detector",
                branch = "quality_regression_fusion_applied",
                heuristic = analysis.quality_score,
                fused,
                "video quality analysis fused with scenario DB prediction"
            );
            analysis.quality_score = crate::algorithm_seal::seal_u8_quality_display(fused);
        }
    }

    Ok(analysis)
}

fn extract_crf_from_params(params: &str) -> Result<Option<u8>, String> {
    let lower = params.to_lowercase();

    // Look for various ways CRF/QP might be specified
    for keyword in ["crf=", "qp=", "cqp=", "crf ", "qp "] {
        if let Some(pos) = lower.find(keyword) {
            let start = pos + keyword.len();
            let rest = &lower[start..];
            // Take characters while they are part of a float
            let end = crate::media_conversion_gate::explore_metric_numeric_end(rest, false);
            let val_str = rest[..end].trim();
            if val_str.is_empty() {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "video_quality_crf_params_malformed",
                    format!("CRF/QP token after {keyword:?} is empty in encoder params"),
                );
                return Err(format!(
                    "Malformed CRF/QP token after {keyword:?} in encoder params"
                ));
            }
            let val = val_str.parse::<f64>().map_err(|err| {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "video_quality_crf_params_malformed",
                    format!("Failed to parse CRF/QP token {val_str:?}: {err}"),
                );
                format!("Failed to parse CRF/QP token {val_str:?}: {err}")
            })?;
            return match crate::numeric_cast::f64_to_u8_strict(val.round(), "crf_from_params") {
                None => {
                    crate::media_conversion_gate::probe_quality_batch_audit(
                        "video_quality_crf_params_invalid",
                        crate::infra::static_logs::messages::MSG_VQD_CRF_PARAMS_AUDIT
                            .replace("{}", &val.to_string()),
                    );
                    Err(format!("Invalid CRF/QP value {val} in encoder params"))
                }
                Some(crf) => Ok(Some(crf)),
            };
        }
    }
    Ok(None)
}

/// Format [`VideoQualityAnalysis`] as multi-line media info. **Log file only** — does not write to
/// terminal. Call when a log file is configured (e.g. alongside SSIM/quality runs).
pub fn log_media_info_for_quality(analysis: &VideoQualityAnalysis, input_path: &Path) {
    if !crate::progress_mode::has_log_file() {
        return;
    }
    tracing::debug!(
        "[Media info] {} | codec={} type={:?} modern={} size={}x{} fps={} duration={}s frames={:?} bitrate={} video_bitrate={:?} bpp={:.4} bit_depth={} pix_fmt={} chroma={:?} has_b_frames={} content_type={:?} compression={:?} quality_score={} estimated_crf={} HDR={}",
        input_path.display(),
        analysis.codec,
        analysis.codec_type,
        analysis.flags.decision.is_modern_codec,
        crate::media_conversion_gate::ui_optional_u32_or_na(analysis.width, "video_quality_width"),
        crate::media_conversion_gate::ui_optional_u32_or_na(
            analysis.height,
            "video_quality_height"
        ),
        crate::media_conversion_gate::ui_f64_or_na(analysis.fps, "video_quality_fps", 2),
        crate::media_conversion_gate::ui_f64_or_na(
            analysis.duration_secs,
            "video_quality_duration_secs",
            2,
        ),
        analysis.frame_count,
        crate::media_conversion_gate::ui_optional_u64_or_na(
            analysis.total_bitrate,
            "video_quality_total_bitrate",
        ),
        analysis.video_bitrate,
        analysis.bpp,
        analysis.format_bit_depth_label(),
        analysis.pix_fmt,
        analysis.chroma,
        analysis.flags.features.has_b_frames,
        analysis.content_type,
        analysis.compression_type,
        analysis.quality_score,
        analysis.estimated_crf,
        analysis.flags.features.is_hdr
    );
}

/// Converts to `QualityAnalysis`.
///
/// # Panics
/// Panics if `width` or `height` is missing, which `analyze_video_quality` guarantees are present.
#[must_use]
pub fn to_quality_analysis(analysis: &VideoQualityAnalysis) -> QualityAnalysis {
    if analysis.color_space.is_none() {
        crate::media_conversion_gate::probe_quality_batch_audit(
            "video_quality_color_space_missing",
            "color space information missing; preserving unknown metadata",
        );
    }
    let mut builder = VideoAnalysisBuilder::new()
        .basic(
            &analysis.codec,
            match analysis.width {
                Some(w) => w,
                None => unreachable!(
                    "CRITICAL: width missing in VideoQualityAnalysis to_quality_analysis (codec={}, path={:?})",
                    analysis.codec, analysis.frame_count
                ),
            },
            match analysis.height {
                Some(h) => h,
                None => unreachable!(
                    "CRITICAL: height missing in VideoQualityAnalysis to_quality_analysis (codec={}, path={:?})",
                    analysis.codec, analysis.frame_count
                ),
            },
            analysis.fps,
            analysis.duration_secs,
        )
        .file_size(analysis.file_size)
        .video_bitrate(match analysis.video_bitrate.or(analysis.total_bitrate) {
            Some(v) => v,
            None => unreachable!(
                "CRITICAL: total_bitrate missing in VideoQualityAnalysis to_quality_analysis (codec={}, path={:?})",
                analysis.codec, analysis.frame_count
            ),
        })
        .gop(analysis.gop_size, analysis.b_frame_count)
        .pix_fmt(&analysis.pix_fmt);

    builder = if let Some(color_space) = analysis.color_space.as_deref() {
        builder.color(color_space, analysis.flags.features.is_hdr)
    } else {
        builder.hdr(analysis.flags.features.is_hdr)
    };

    builder
        .content_type(analysis.content_type.to_content_type())
        .bit_depth_with_source(analysis.bit_depth, analysis.bit_depth_inferred_from_pix_fmt)
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
    bit_depth_inferred_from_pix_fmt: bool,
    compression: CompressionLevel,
) -> Result<u8, String> {
    let base = match compression {
        CompressionLevel::Lossless => crate::constants::QUALITY_SCORE_LOSSLESS,
        CompressionLevel::VisuallyLossless => crate::constants::QUALITY_SCORE_VISUALLY_LOSSLESS,
        CompressionLevel::HighQuality => crate::constants::QUALITY_SCORE_HIGH,
        CompressionLevel::Standard => crate::constants::QUALITY_SCORE_STANDARD,
        CompressionLevel::LowQuality => crate::constants::QUALITY_SCORE_LOW,
        CompressionLevel::Unknown => {
            crate::media_conversion_gate::probe_quality_batch_audit(
                "video_quality_compression_unknown",
                format!(
                    "unknown compression strategy (score={})",
                    crate::constants::VIDEO_QUALITY_SCORE_NEUTRAL
                ),
            );
            crate::constants::VIDEO_QUALITY_SCORE_NEUTRAL
        }
    };

    let depth_bonus = if BitDepthMetadata::new(bit_depth, bit_depth_inferred_from_pix_fmt)
        .has_confirmed_high_bit_depth()
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
            let t = crate::numeric_cast::f64_to_u32_strict(val, "bpp_quality_tweak").ok_or_else(
                || {
                    crate::media_conversion_gate::probe_quality_batch_audit(
                        "video_quality_bpp_tweak_nan",
                        format!("bpp_tweak NaN/Inf for bpp={bpp}"),
                    );
                    crate::media_conversion_gate::ui_quality_user_error(
                        "Numerical anomaly in BPP quality tweak calculation",
                    )
                },
            )?;
            // t is clamped to 0..=5, always fits u8.
            match u8::try_from(t.clamp(0, crate::constants::QUALITY_TWEAK_STANDARD_MAX_TICK)) {
                Ok(v) => v,
                Err(_) => unreachable!(
                    "CRITICAL: Standard quality tweak {} failed u8 conversion after clamp",
                    t
                ),
            }
        }
        CompressionLevel::HighQuality => {
            let t = crate::numeric_cast::f64_to_u32_strict(
                ((bpp - crate::constants::QUALITY_TWEAK_BPP_HIGH_MIN)
                    .clamp(0.0, crate::constants::QUALITY_TWEAK_BPP_RANGE)
                    / crate::constants::QUALITY_TWEAK_BPP_RANGE
                    * crate::constants::QUALITY_TWEAK_HIGH_SCALE)
                    .round(),
                "bpp_tweak_high",
            )
            .ok_or_else(|| {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "video_quality_bpp_high_quality_invalid",
                    format!("invalid BPP value {bpp} for high-quality tweak calculation"),
                );
                crate::media_conversion_gate::ui_quality_user_error(
                    "Numerical anomaly in high-quality BPP tweak calculation",
                )
            })?;
            // t is clamped to 0..=3, always fits u8.
            match u8::try_from(t.clamp(0, crate::constants::QUALITY_TWEAK_HIGH_MAX_TICK)) {
                Ok(v) => v,
                Err(_) => unreachable!(
                    "CRITICAL: High quality tweak {} failed u8 conversion after clamp",
                    t
                ),
            }
        }
        CompressionLevel::Lossless
        | CompressionLevel::VisuallyLossless
        | CompressionLevel::LowQuality
        | CompressionLevel::Unknown => 0,
    };

    Ok((base + depth_bonus + codec_bonus + bpp_tweak).min(100))
}

fn estimate_crf_from_bpp(bpp: f64, codec_type: VideoCodecType) -> u8 {
    if codec_type == VideoCodecType::Lossless {
        return 0;
    }

    let efficiency = match codec_type {
        VideoCodecType::ModernEfficient => crate::constants::MODERN_EFFICIENT_CODEC_FACTOR,
        VideoCodecType::Intermediate => crate::constants::INTERMEDIATE_CODEC_FACTOR,
        VideoCodecType::Inefficient => crate::constants::INEFFICIENT_CODEC_FACTOR,
        VideoCodecType::Lossless | VideoCodecType::Legacy | VideoCodecType::Unknown => 1.0_f64,
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
    crate::media_conversion_gate::probe_quality_batch_audit(
        "video_quality_bpp_to_crf_lut_failed",
        format!(
            "BPP-to-CRF LUT failed for adjusted_bpp={adjusted_bpp:.4}; using fallback CRF {}",
            crate::constants::FALLBACK_CRF_VIDEO
        ),
    );
    match crate::numeric_cast::f64_to_u8_strict(
        f64::from(crate::constants::FALLBACK_CRF_VIDEO),
        "fallback",
    ) {
        Some(v) => v,
        None => unreachable!(
            "CRITICAL: fallback CRF constant {} invalid for u8 conversion",
            crate::constants::FALLBACK_CRF_VIDEO
        ),
    }
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
    include!("../tests/video_quality.rs");
}
