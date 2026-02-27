//! Quality Matcher Module
//!
//! Unified quality matching algorithm for all modern_format_boost tools.
//! Calculates optimal encoding parameters (CRF/distance) based on input quality analysis.
//!
//! ## Supported Encoders
//! - **AV1 (SVT-AV1)**: CRF range 0-63, default 23
//! - **HEVC (x265)**: CRF range 0-51, default 23
//! - **JXL (cjxl)**: Distance range 0.0-15.0, 0.0 = lossless
//!
//! ## Quality Matching Philosophy
//!
//! The goal is to match the **perceived quality** of the input, not the bitrate.
//! Different codecs have different efficiency, so we normalize using:
//!
//! 1. **Bits per pixel (bpp)** - Primary quality indicator
//! 2. **Codec efficiency factor** - H.264 baseline (1.0), HEVC ~0.65, AV1 ~0.50 (empirical
//!    relative efficiency; see `SourceCodec::efficiency_factor()` and literature on
//!    codec bitrate comparisons, e.g. Netflix VMAF/codec studies).
//! 3. **Content complexity** - Resolution, B-frames, color depth
//!
//! ## Extreme BPP (defensive design)
//! For extreme inputs (very low or very high effective BPP):
//! - Reject `effective_bpp <= 0` or non-finite (NaN/Inf) with an error.
//! - Clamp `effective_bpp` to `[SAFE_BPP_MIN, SAFE_BPP_MAX]` (1e-6..50) before the CRF formula
//!   so `log2(effective_bpp * 100)` is always finite and in a reasonable range.
//! - Final CRF is always clamped to encoder range (AV1 [15, 40], HEVC [0, 35]) as the last line
//!   of defense against content-type adjustment and bias pushing out of range.
//!
//! ## 🔥 Quality Manifesto (质量宣言)
//!
//! - **No silent fallback**: If quality analysis fails, report error loudly
//! - **No hardcoded defaults**: All parameters derived from actual content analysis
//! - **Conservative on uncertainty**: When in doubt, prefer higher quality (lower CRF)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderType {
    Av1,
    Hevc,
    Jxl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceCodec {
    H264,
    H265,
    Vvc,
    Vp8,
    Vp9,
    Av1,
    Av2,

    Mpeg4,
    Mpeg2,
    Mpeg1,
    Wmv,
    Theora,
    RealVideo,
    FlashVideo,

    ProRes,
    DnxHD,
    Mjpeg,

    Ffv1,
    UtVideo,
    HuffYuv,
    RawVideo,
    Lagarith,
    MagicYuv,

    Gif,
    Apng,
    WebpAnimated,

    Jpeg,
    JpegXl,
    Png,
    WebpStatic,
    Avif,
    Heic,
    Bmp,
    Tiff,

    #[default]
    Unknown,
}

impl SourceCodec {
    /// Relative encoding efficiency vs. H.264 (1.0). Lower value = more efficient at same quality.
    /// H.265/HEVC ≈ 0.65 and AV1 ≈ 0.50 are empirical from bitrate comparison studies; no single
    /// canonical reference—values tuned for CRF mapping consistency across codecs.
    pub fn efficiency_factor(&self) -> f64 {
        match self {
            SourceCodec::H264 => 1.0,
            SourceCodec::H265 => 0.65,
            SourceCodec::Vp8 => 0.85,
            SourceCodec::Vp9 => 0.70,
            SourceCodec::Av1 => 0.50,
            SourceCodec::Vvc => 0.35,
            SourceCodec::Av2 => 0.35,

            SourceCodec::Mpeg4 => 1.3,
            SourceCodec::Mpeg2 => 1.8,
            SourceCodec::Mpeg1 => 2.5,
            SourceCodec::Wmv => 1.1,
            SourceCodec::Theora => 1.2,
            SourceCodec::RealVideo => 2.0,
            SourceCodec::FlashVideo => 1.5,

            SourceCodec::ProRes => 1.8,
            SourceCodec::DnxHD => 1.8,
            SourceCodec::Mjpeg => 2.5,

            SourceCodec::Ffv1 => 1.0,
            SourceCodec::UtVideo => 1.0,
            SourceCodec::HuffYuv => 1.0,
            SourceCodec::RawVideo => 1.0,
            SourceCodec::Lagarith => 1.0,
            SourceCodec::MagicYuv => 1.0,

            SourceCodec::Gif => 3.0,
            SourceCodec::Apng => 1.8,
            SourceCodec::WebpAnimated => 0.9,

            SourceCodec::Jpeg => 1.0,
            SourceCodec::JpegXl => 0.6,
            SourceCodec::Png => 1.5,
            SourceCodec::WebpStatic => 0.75,
            SourceCodec::Avif => 0.55,
            SourceCodec::Heic => 0.65,
            SourceCodec::Bmp => 3.0,
            SourceCodec::Tiff => 1.2,

            SourceCodec::Unknown => 1.0,
        }
    }

    pub fn is_lossless(&self) -> bool {
        matches!(
            self,
            SourceCodec::Ffv1
                | SourceCodec::UtVideo
                | SourceCodec::HuffYuv
                | SourceCodec::RawVideo
                | SourceCodec::Lagarith
                | SourceCodec::MagicYuv
                | SourceCodec::Png
                | SourceCodec::Apng
                | SourceCodec::Bmp
        )
    }

    pub fn is_modern(&self) -> bool {
        matches!(
            self,
            SourceCodec::H265
                | SourceCodec::Av1
                | SourceCodec::Vp9
                | SourceCodec::Vvc
                | SourceCodec::Av2
                | SourceCodec::JpegXl
                | SourceCodec::Avif
                | SourceCodec::Heic
        )
    }

    pub fn is_cutting_edge(&self) -> bool {
        matches!(self, SourceCodec::Vvc | SourceCodec::Av2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MatchMode {
    #[default]
    Quality,
    Size,
    Speed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QualityBias {
    Conservative,
    #[default]
    Balanced,
    Aggressive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityAnalysis {
    pub bpp: f64,
    pub source_codec: String,
    pub width: u32,
    pub height: u32,
    pub has_b_frames: bool,
    pub bit_depth: u8,
    pub has_alpha: bool,
    pub duration_secs: Option<f64>,
    pub fps: Option<f64>,
    pub file_size: u64,
    pub estimated_quality: Option<u8>,

    pub video_bitrate: Option<u64>,

    pub gop_size: Option<u32>,

    pub b_frame_count: Option<u8>,

    pub pix_fmt: Option<String>,

    pub color_space: Option<String>,

    pub is_hdr: Option<bool>,

    pub content_type: Option<ContentType>,

    pub spatial_complexity: Option<f64>,

    pub temporal_complexity: Option<f64>,

    pub has_film_grain: Option<bool>,

    pub encoder_preset: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ContentType {
    LiveAction,
    Animation,
    ScreenRecording,
    Gaming,
    FilmGrain,
    #[default]
    Unknown,
}

impl ContentType {
    pub fn crf_adjustment(&self) -> i8 {
        match self {
            ContentType::Animation => 4,
            ContentType::ScreenRecording => 5,
            ContentType::LiveAction => 0,
            ContentType::Gaming => -1,
            ContentType::FilmGrain => -3,
            ContentType::Unknown => 0,
        }
    }
}

impl Default for QualityAnalysis {
    fn default() -> Self {
        Self {
            bpp: 0.0,
            source_codec: String::new(),
            width: 0,
            height: 0,
            has_b_frames: false,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: None,
            fps: None,
            file_size: 0,
            estimated_quality: None,
            video_bitrate: None,
            gop_size: None,
            b_frame_count: None,
            pix_fmt: None,
            color_space: None,
            is_hdr: None,
            content_type: None,
            spatial_complexity: None,
            temporal_complexity: None,
            has_film_grain: None,
            encoder_preset: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedQuality {
    pub crf: f32,
    pub distance: f32,
    pub effective_bpp: f64,
    pub analysis_details: AnalysisDetails,
}

impl MatchedQuality {
    #[inline]
    pub fn crf_hevc_typed(&self) -> Option<crate::types::Crf<crate::types::HevcEncoder>> {
        crate::types::Crf::<crate::types::HevcEncoder>::new(self.crf).ok()
    }

    #[inline]
    pub fn crf_av1_typed(&self) -> Option<crate::types::Crf<crate::types::Av1Encoder>> {
        crate::types::Crf::<crate::types::Av1Encoder>::new(self.crf).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisDetails {
    pub raw_bpp: f64,
    pub codec_factor: f64,
    pub resolution_factor: f64,
    pub alpha_factor: f64,
    pub color_depth_factor: f64,

    pub gop_factor: f64,
    pub chroma_factor: f64,
    pub hdr_factor: f64,
    pub content_type_adjustment: i8,

    pub aspect_factor: f64,
    pub complexity_factor: f64,
    pub grain_factor: f64,

    #[serde(default = "default_one")]
    pub bframe_factor: f64,
    #[serde(default = "default_one")]
    pub fps_factor: f64,
    #[serde(default = "default_one")]
    pub duration_factor: f64,

    pub confidence: f64,
    pub match_mode: MatchMode,
    pub quality_bias: QualityBias,
}

fn default_one() -> f64 {
    1.0
}

impl Default for AnalysisDetails {
    fn default() -> Self {
        Self {
            raw_bpp: 0.0,
            codec_factor: 1.0,
            resolution_factor: 1.0,
            alpha_factor: 1.0,
            color_depth_factor: 1.0,
            gop_factor: 1.0,
            chroma_factor: 1.0,
            hdr_factor: 1.0,
            content_type_adjustment: 0,
            aspect_factor: 1.0,
            complexity_factor: 1.0,
            grain_factor: 1.0,
            bframe_factor: 1.0,
            fps_factor: 1.0,
            duration_factor: 1.0,
            confidence: 0.0,
            match_mode: MatchMode::Quality,
            quality_bias: QualityBias::Balanced,
        }
    }
}

/// Safe BPP range for CRF formula: avoids log2(0), NaN, and overflow. Final CRF is still clamped to [15, 40] (AV1) or [0, 35] (HEVC).
const SAFE_BPP_MIN: f64 = 1e-6;
const SAFE_BPP_MAX: f64 = 50.0;

/// AV1 CRF output range; final clamp is the last line of defense for extreme BPP or content/bias adjustments.
const AV1_CRF_CLAMP_MIN: f32 = 15.0;
const AV1_CRF_CLAMP_MAX: f32 = 40.0;

/// HEVC CRF output range (x265 0–51, we use 0–35 for quality matching).
const HEVC_CRF_CLAMP_MIN: f32 = 0.0;
const HEVC_CRF_CLAMP_MAX: f32 = 35.0;

pub fn calculate_av1_crf(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    calculate_av1_crf_with_options(analysis, MatchMode::Quality, QualityBias::Balanced)
}

pub fn calculate_av1_crf_with_options(
    analysis: &QualityAnalysis,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<MatchedQuality, String> {
    let (mut effective_bpp, details) =
        calculate_effective_bpp_with_options(analysis, EncoderType::Av1, mode, bias)?;

    if effective_bpp <= 0.0 {
        return Err(format!(
            "❌ Cannot calculate AV1 CRF: effective_bpp is {} (must be > 0)\n\
             💡 Possible causes:\n\
             - File size is 0 or unknown\n\
             - video_bitrate not provided\n\
             - Duration/fps detection failed\n\
             - Invalid dimensions\n\
             💡 Confidence: {:.0}%",
            effective_bpp,
            details.confidence * 100.0
        ));
    }
    if !effective_bpp.is_finite() {
        return Err(format!(
            "❌ Cannot calculate AV1 CRF: effective_bpp is non-finite (NaN/Inf)\n\
             💡 Confidence: {:.0}%",
            details.confidence * 100.0
        ));
    }
    // Defensive clamp so formula inputs are always in a safe range; final CRF clamp [15, 40] remains the safeguard.
    effective_bpp = effective_bpp.clamp(SAFE_BPP_MIN, SAFE_BPP_MAX);

    let crf_float = if effective_bpp < 0.03 {
        35.0_f64.min(50.0 - 6.0 * (effective_bpp * 100.0).max(0.001).log2())
    } else if effective_bpp > 2.0 {
        18.0_f64.max(50.0 - 6.0 * (effective_bpp * 100.0).log2())
    } else {
        50.0 - 6.0 * (effective_bpp * 100.0).log2()
    };

    let crf_with_content = crf_float + details.content_type_adjustment as f64;

    let crf_with_bias = match bias {
        QualityBias::Conservative => crf_with_content - 2.0,
        QualityBias::Balanced => crf_with_content,
        QualityBias::Aggressive => crf_with_content + 2.0,
    };

    let crf_rounded = (crf_with_bias * 2.0).round() / 2.0;
    // Last line of defense: guarantee CRF in valid range regardless of extreme BPP or content/bias.
    let crf = (crf_rounded as f32).clamp(AV1_CRF_CLAMP_MIN, AV1_CRF_CLAMP_MAX);

    Ok(MatchedQuality {
        crf,
        distance: 0.0,
        effective_bpp,
        analysis_details: details,
    })
}

pub fn calculate_hevc_crf(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    calculate_hevc_crf_with_options(analysis, MatchMode::Quality, QualityBias::Balanced)
}

pub fn calculate_hevc_crf_with_options(
    analysis: &QualityAnalysis,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<MatchedQuality, String> {
    let (mut effective_bpp, details) =
        calculate_effective_bpp_with_options(analysis, EncoderType::Hevc, mode, bias)?;

    if effective_bpp <= 0.0 {
        return Err(format!(
            "❌ Cannot calculate HEVC CRF: effective_bpp is {} (must be > 0)\n\
             💡 Possible causes:\n\
             - File size is 0 or unknown\n\
             - video_bitrate not provided\n\
             - Duration/fps detection failed\n\
             - Invalid dimensions\n\
             💡 Confidence: {:.0}%",
            effective_bpp,
            details.confidence * 100.0
        ));
    }
    if !effective_bpp.is_finite() {
        return Err(format!(
            "❌ Cannot calculate HEVC CRF: effective_bpp is non-finite (NaN/Inf)\n\
             💡 Confidence: {:.0}%",
            details.confidence * 100.0
        ));
    }
    effective_bpp = effective_bpp.clamp(SAFE_BPP_MIN, SAFE_BPP_MAX);

    let crf_float = if effective_bpp < 0.02 {
        35.0_f64.min(46.0 - 5.0 * (effective_bpp * 100.0).max(0.001).log2())
    } else if effective_bpp > 2.0 {
        15.0_f64.max(46.0 - 5.0 * (effective_bpp * 100.0).log2())
    } else {
        46.0 - 5.0 * (effective_bpp * 100.0).log2()
    };

    let crf_with_content = crf_float + details.content_type_adjustment as f64;

    let crf_with_bias = match bias {
        QualityBias::Conservative => crf_with_content - 2.0,
        QualityBias::Balanced => crf_with_content,
        QualityBias::Aggressive => crf_with_content + 2.0,
    };

    let crf_rounded = (crf_with_bias * 2.0).round() / 2.0;
    let crf = (crf_rounded as f32).clamp(HEVC_CRF_CLAMP_MIN, HEVC_CRF_CLAMP_MAX);

    Ok(MatchedQuality {
        crf,
        distance: 0.0,
        effective_bpp,
        analysis_details: details,
    })
}

pub fn calculate_jxl_distance(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    calculate_jxl_distance_with_options(analysis, MatchMode::Quality, QualityBias::Balanced)
}

pub fn calculate_jxl_distance_with_options(
    analysis: &QualityAnalysis,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<MatchedQuality, String> {
    if let Some(quality) = analysis.estimated_quality {
        let base_distance = (100.0 - quality as f32) / 10.0;

        let biased_distance = match bias {
            QualityBias::Conservative => base_distance - 0.2,
            QualityBias::Balanced => base_distance,
            QualityBias::Aggressive => base_distance + 0.3,
        };

        let clamped = biased_distance.clamp(0.0, 5.0);

        return Ok(MatchedQuality {
            crf: 0.0,
            distance: clamped,
            effective_bpp: analysis.bpp,
            analysis_details: AnalysisDetails {
                confidence: 0.9,
                match_mode: mode,
                quality_bias: bias,
                ..Default::default()
            },
        });
    }

    let (effective_bpp, details) =
        calculate_effective_bpp_with_options(analysis, EncoderType::Jxl, mode, bias)?;

    if effective_bpp <= 0.0 {
        return Err(format!(
            "❌ Cannot calculate JXL distance: effective_bpp is {} (must be > 0)\n\
             💡 Possible causes:\n\
             - File size is 0 or unknown\n\
             - Invalid dimensions\n\
             💡 For JPEG sources, ensure JPEG quality analysis is available\n\
             💡 Confidence: {:.0}%",
            effective_bpp,
            details.confidence * 100.0
        ));
    }

    let estimated_quality = 70.0 + 15.0 * (effective_bpp * 5.0).max(0.001).log2();

    let clamped_quality = estimated_quality.clamp(50.0, 100.0);
    let base_distance = ((100.0 - clamped_quality) / 10.0) as f32;

    let content_adj = details.content_type_adjustment as f32 * 0.1;
    let distance_with_content = base_distance - content_adj;

    let distance_with_bias = match bias {
        QualityBias::Conservative => distance_with_content - 0.2,
        QualityBias::Balanced => distance_with_content,
        QualityBias::Aggressive => distance_with_content + 0.3,
    };

    let clamped_distance = distance_with_bias.clamp(0.0, 5.0);

    Ok(MatchedQuality {
        crf: 0.0,
        distance: clamped_distance,
        effective_bpp,
        analysis_details: details,
    })
}

fn calculate_effective_bpp_with_options(
    analysis: &QualityAnalysis,
    target_encoder: EncoderType,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<(f64, AnalysisDetails), String> {
    if analysis.width == 0 || analysis.height == 0 {
        return Err("❌ Invalid dimensions: width or height is 0".to_string());
    }

    let pixels = (analysis.width as u64) * (analysis.height as u64);

    let raw_bpp = calculate_raw_bpp(analysis, pixels)?;

    let source_codec = parse_source_codec(&analysis.source_codec);
    let codec_factor = calculate_codec_efficiency(source_codec, analysis.encoder_preset.as_deref());

    let gop_factor = calculate_gop_factor(
        analysis.gop_size,
        analysis
            .b_frame_count
            .unwrap_or(if analysis.has_b_frames { 2 } else { 0 }),
    );

    let chroma_factor = calculate_chroma_factor(analysis.pix_fmt.as_deref());

    let hdr_factor = calculate_hdr_factor(analysis.is_hdr, analysis.color_space.as_deref());

    let content_type_adjustment = analysis
        .content_type
        .unwrap_or(ContentType::Unknown)
        .crf_adjustment();

    let resolution_factor = calculate_resolution_factor(pixels);

    let alpha_factor = if analysis.has_alpha { 0.9 } else { 1.0 };

    let color_depth_factor = calculate_color_depth_factor(analysis.bit_depth, source_codec);

    let aspect_factor = calculate_aspect_factor(analysis.width, analysis.height);

    let complexity_factor = calculate_complexity_factor(
        analysis.spatial_complexity,
        analysis.temporal_complexity,
        raw_bpp,
        pixels,
    );

    let grain_factor = if analysis.has_film_grain == Some(true) {
        1.20
    } else {
        1.0
    };

    let target_adjustment = match target_encoder {
        EncoderType::Av1 => 0.5,
        EncoderType::Hevc => 0.7,
        EncoderType::Jxl => 0.8,
    };

    let mode_adjustment = match mode {
        MatchMode::Quality => 1.0,
        MatchMode::Size => 0.8,
        MatchMode::Speed => 0.9,
    };

    let effective_bpp = raw_bpp
        * gop_factor
        * chroma_factor
        * hdr_factor
        * aspect_factor
        * complexity_factor
        * grain_factor
        * mode_adjustment
        * resolution_factor
        * alpha_factor
        / codec_factor
        / color_depth_factor
        / target_adjustment;

    let confidence = calculate_confidence_v3(analysis);

    let details = AnalysisDetails {
        raw_bpp,
        codec_factor,
        resolution_factor,
        alpha_factor,
        color_depth_factor,
        gop_factor,
        chroma_factor,
        hdr_factor,
        content_type_adjustment,
        aspect_factor,
        complexity_factor,
        grain_factor,
        bframe_factor: gop_factor,
        fps_factor: 1.0,
        duration_factor: 1.0,
        confidence,
        match_mode: mode,
        quality_bias: bias,
    };

    Ok((effective_bpp, details))
}

fn calculate_raw_bpp(analysis: &QualityAnalysis, pixels: u64) -> Result<f64, String> {
    if analysis.bpp > 0.0 {
        return Ok(analysis.bpp);
    }

    if let Some(video_bitrate) = analysis.video_bitrate {
        if video_bitrate > 0 {
            if let Some(fps) = analysis.fps {
                if fps > 0.0 {
                    let bits_per_frame = video_bitrate as f64 / fps;
                    return Ok(bits_per_frame / pixels as f64);
                }
            }
        }
    }

    if analysis.file_size > 0 {
        if let Some(duration) = analysis.duration_secs {
            if duration > 0.0 {
                let fps = analysis.fps.unwrap_or_else(|| {
                    let codec = parse_source_codec(&analysis.source_codec);
                    match codec {
                        SourceCodec::Gif => 10.0,
                        SourceCodec::Apng => 15.0,
                        SourceCodec::WebpAnimated => 20.0,
                        _ => 24.0,
                    }
                });
                let total_frames = (duration * fps) as u64;
                let bits_per_frame = (analysis.file_size * 8) as f64 / total_frames.max(1) as f64;
                return Ok(bits_per_frame / pixels as f64);
            }
        }
        return Ok(analysis.file_size as f64 / pixels as f64);
    }

    Err("❌ Cannot calculate bpp: no video_bitrate, file_size, or bpp provided".to_string())
}

fn calculate_gop_factor(gop_size: Option<u32>, b_frames: u8) -> f64 {
    let gop_base = match gop_size {
        Some(1) => 0.70,
        Some(2..=10) => 0.85,
        Some(11..=50) => 1.0,
        Some(51..=150) => 1.15,
        Some(151..=300) => 1.20,
        Some(_) => 1.25,
        None => 1.0,
    };

    let b_pyramid_bonus = match b_frames {
        0 => 1.0,
        1 => 1.05,
        2 => 1.08,
        _ => 1.12,
    };

    gop_base * b_pyramid_bonus
}

fn calculate_chroma_factor(pix_fmt: Option<&str>) -> f64 {
    match pix_fmt {
        Some(fmt) => {
            let fmt_lower = fmt.to_lowercase();
            if fmt_lower.contains("444") {
                1.15
            } else if fmt_lower.contains("422") {
                1.05
            } else if fmt_lower.contains("rgb") || fmt_lower.contains("gbr") {
                1.20
            } else {
                1.0
            }
        }
        None => 1.0,
    }
}

fn calculate_hdr_factor(is_hdr: Option<bool>, color_space: Option<&str>) -> f64 {
    if is_hdr == Some(true) {
        return 1.25;
    }

    if let Some(cs) = color_space {
        let cs_lower = cs.to_lowercase();
        if cs_lower.contains("bt2020") || cs_lower.contains("2020") {
            return 1.15;
        }
    }

    1.0
}

fn calculate_codec_efficiency(codec: SourceCodec, preset: Option<&str>) -> f64 {
    let base_efficiency = codec.efficiency_factor();

    if let Some(p) = preset {
        let p_lower = p.to_lowercase();

        if p_lower.contains("placebo") || p_lower.contains("veryslow") {
            return base_efficiency * 0.85;
        } else if p_lower.contains("slow") {
            return base_efficiency * 0.90;
        } else if p_lower.contains("fast") || p_lower.contains("veryfast") {
            return base_efficiency * 1.15;
        } else if p_lower.contains("ultrafast") {
            return base_efficiency * 1.30;
        }

        if let Ok(preset_num) = p.parse::<u8>() {
            return match preset_num {
                0..=2 => base_efficiency * 0.80,
                3..=4 => base_efficiency * 0.90,
                5..=6 => base_efficiency * 1.0,
                7..=8 => base_efficiency * 1.10,
                9..=10 => base_efficiency * 1.20,
                _ => base_efficiency * 1.30,
            };
        }
    }

    base_efficiency
}

fn calculate_resolution_factor(pixels: u64) -> f64 {
    let megapixels = pixels as f64 / 1_000_000.0;
    if megapixels > 8.0 {
        0.80 + 0.05 * (8.0 / megapixels).min(1.0)
    } else if megapixels > 2.0 {
        0.85 + 0.05 * ((8.0 - megapixels) / 6.0)
    } else if megapixels > 0.5 {
        0.90 + 0.05 * ((2.0 - megapixels) / 1.5)
    } else {
        0.95 + 0.05 * ((0.5 - megapixels) / 0.5).min(1.0)
    }
}

fn calculate_color_depth_factor(bit_depth: u8, codec: SourceCodec) -> f64 {
    match bit_depth {
        1..=8 => {
            if codec == SourceCodec::Gif {
                1.3
            } else {
                1.0
            }
        }
        10 => 1.25,
        12 => 1.5,
        16 => 2.0,
        _ => 1.0,
    }
}

fn calculate_aspect_factor(width: u32, height: u32) -> f64 {
    let aspect_ratio = width as f64 / height.max(1) as f64;
    if aspect_ratio > 2.5 {
        1.08
    } else if aspect_ratio > 2.0 {
        1.04
    } else if aspect_ratio < 0.5 {
        1.08
    } else {
        1.0
    }
}

fn calculate_complexity_factor(si: Option<f64>, ti: Option<f64>, raw_bpp: f64, pixels: u64) -> f64 {
    if let (Some(spatial), Some(temporal)) = (si, ti) {
        let si_ratio = spatial / 50.0;
        let ti_ratio = temporal / 20.0;

        let spatial_factor = if si_ratio > 1.3 {
            1.15
        } else if si_ratio < 0.7 {
            0.90
        } else {
            1.0
        };

        let temporal_factor = if ti_ratio > 1.5 {
            1.10
        } else if ti_ratio < 0.5 {
            0.95
        } else {
            1.0
        };

        return spatial_factor * temporal_factor;
    }

    let expected_bpp = if pixels > 8_000_000 {
        0.15
    } else if pixels > 2_000_000 {
        0.20
    } else if pixels > 500_000 {
        0.30
    } else {
        0.50
    };

    let ratio = raw_bpp / expected_bpp;
    if ratio > 2.0 {
        1.15
    } else if ratio > 1.0 {
        1.0 + 0.15 * ((ratio - 1.0) / 1.0)
    } else if ratio > 0.5 {
        1.0
    } else {
        0.95
    }
}

fn calculate_confidence_v3(analysis: &QualityAnalysis) -> f64 {
    let mut score: f64 = 0.0;
    let mut max_score: f64 = 0.0;

    max_score += 25.0;
    if analysis.width > 0 && analysis.height > 0 {
        score += 25.0;
    }

    max_score += 20.0;
    if analysis.file_size > 0 || analysis.video_bitrate.is_some() {
        score += 20.0;
    }

    max_score += 10.0;
    if analysis.bpp > 0.0 {
        score += 10.0;
    }

    max_score += 8.0;
    let codec = parse_source_codec(&analysis.source_codec);
    if codec != SourceCodec::Unknown {
        score += 8.0;
    }

    max_score += 5.0;
    if analysis.video_bitrate.is_some() {
        score += 5.0;
    }

    max_score += 4.0;
    if analysis.gop_size.is_some() {
        score += 4.0;
    }

    max_score += 3.0;
    if analysis.b_frame_count.is_some() {
        score += 3.0;
    }

    max_score += 3.0;
    if analysis.pix_fmt.is_some() {
        score += 3.0;
    }

    max_score += 3.0;
    if analysis.is_hdr.is_some() || analysis.color_space.is_some() {
        score += 3.0;
    }

    max_score += 2.0;
    if analysis.content_type.is_some() {
        score += 2.0;
    }

    max_score += 3.0;
    if analysis.spatial_complexity.is_some() && analysis.temporal_complexity.is_some() {
        score += 3.0;
    }

    max_score += 4.0;
    if analysis.duration_secs.is_some() {
        score += 4.0;
    }

    max_score += 4.0;
    if analysis.fps.is_some() {
        score += 4.0;
    }

    max_score += 3.0;
    if analysis.estimated_quality.is_some() {
        score += 3.0;
    }

    max_score += 3.0;
    if analysis.bit_depth > 0 {
        score += 3.0;
    }

    if let (Some(fps), Some(duration)) = (analysis.fps, analysis.duration_secs) {
        if fps > 0.0 && duration > 0.0 {
            if (1.0..=240.0).contains(&fps) {
                score += 2.0;
                max_score += 2.0;
            }
        }
    }

    if let Some(video_bitrate) = analysis.video_bitrate {
        let pixels = (analysis.width as u64) * (analysis.height as u64);
        if pixels > 0 && video_bitrate > 0 {
            let bpp_estimate =
                video_bitrate as f64 / (pixels as f64 * analysis.fps.unwrap_or(24.0));
            if (0.01..=5.0).contains(&bpp_estimate) {
                score += 2.0;
                max_score += 2.0;
            }
        }
    }

    (score / max_score).clamp(0.0, 1.0)
}

pub fn parse_source_codec(codec_str: &str) -> SourceCodec {
    let codec_lower = codec_str.to_lowercase();

    if codec_lower.contains("vvc") || codec_lower.contains("h266") || codec_lower.contains("h.266")
    {
        return SourceCodec::Vvc;
    }
    if codec_lower.contains("av2") || codec_lower.contains("avm") {
        return SourceCodec::Av2;
    }

    if codec_lower.contains("av1")
        || codec_lower.contains("svt")
        || codec_lower.contains("aom")
        || codec_lower.contains("libaom")
    {
        return SourceCodec::Av1;
    }
    if codec_lower.contains("h265")
        || codec_lower.contains("hevc")
        || codec_lower.contains("x265")
        || codec_lower.contains("h.265")
    {
        return SourceCodec::H265;
    }
    if codec_lower.contains("vp9") {
        return SourceCodec::Vp9;
    }
    if codec_lower.contains("vp8") || codec_lower == "libvpx" {
        return SourceCodec::Vp8;
    }
    if codec_lower.contains("h264")
        || codec_lower.contains("avc")
        || codec_lower.contains("x264")
        || codec_lower.contains("h.264")
    {
        return SourceCodec::H264;
    }

    if codec_lower.contains("mpeg4")
        || codec_lower.contains("xvid")
        || codec_lower.contains("divx")
        || codec_lower.contains("mp4v")
    {
        return SourceCodec::Mpeg4;
    }
    if codec_lower.contains("mpeg2") || codec_lower == "mpeg2video" {
        return SourceCodec::Mpeg2;
    }
    if codec_lower.contains("mpeg1") || codec_lower == "mpeg1video" {
        return SourceCodec::Mpeg1;
    }
    if codec_lower.contains("wmv") || codec_lower.contains("vc1") || codec_lower.contains("vc-1") {
        return SourceCodec::Wmv;
    }
    if codec_lower.contains("theora") {
        return SourceCodec::Theora;
    }
    if codec_lower.contains("rv10")
        || codec_lower.contains("rv20")
        || codec_lower.contains("rv30")
        || codec_lower.contains("rv40")
        || codec_lower.contains("realvideo")
    {
        return SourceCodec::RealVideo;
    }
    if codec_lower.contains("flv") || codec_lower.contains("vp6") || codec_lower.contains("flashsv")
    {
        return SourceCodec::FlashVideo;
    }

    if codec_lower.contains("prores") {
        return SourceCodec::ProRes;
    }
    if codec_lower.contains("dnxh") || codec_lower.contains("dnxhr") {
        return SourceCodec::DnxHD;
    }
    if codec_lower.contains("mjpeg") || codec_lower.contains("motion jpeg") {
        return SourceCodec::Mjpeg;
    }

    if codec_lower.contains("ffv1") {
        return SourceCodec::Ffv1;
    }
    if codec_lower.contains("utvideo") || codec_lower.contains("ut video") {
        return SourceCodec::UtVideo;
    }
    if codec_lower.contains("huffyuv") || codec_lower.contains("ffvhuff") {
        return SourceCodec::HuffYuv;
    }
    if codec_lower.contains("rawvideo") || codec_lower == "raw" {
        return SourceCodec::RawVideo;
    }
    if codec_lower.contains("lagarith") {
        return SourceCodec::Lagarith;
    }
    if codec_lower.contains("magicyuv") {
        return SourceCodec::MagicYuv;
    }

    if codec_lower.contains("gif") {
        return SourceCodec::Gif;
    }
    if codec_lower.contains("apng") {
        return SourceCodec::Apng;
    }

    if codec_lower.contains("jxl")
        || codec_lower.contains("jpeg xl")
        || codec_lower.contains("jpegxl")
    {
        return SourceCodec::JpegXl;
    }
    if codec_lower.contains("avif") {
        return SourceCodec::Avif;
    }
    if codec_lower.contains("heic") || codec_lower.contains("heif") {
        return SourceCodec::Heic;
    }
    if codec_lower.contains("webp") {
        if codec_lower.contains("anim") {
            return SourceCodec::WebpAnimated;
        } else {
            return SourceCodec::WebpStatic;
        }
    }

    if codec_lower.contains("jpeg") || codec_lower.contains("jpg") {
        return SourceCodec::Jpeg;
    }
    if codec_lower.contains("png") {
        return SourceCodec::Png;
    }
    if codec_lower.contains("bmp") || codec_lower.contains("bitmap") {
        return SourceCodec::Bmp;
    }
    if codec_lower.contains("tiff") || codec_lower.contains("tif") {
        return SourceCodec::Tiff;
    }

    SourceCodec::Unknown
}

pub fn log_quality_analysis(
    analysis: &QualityAnalysis,
    result: &MatchedQuality,
    encoder: EncoderType,
) {
    if !crate::progress_mode::is_verbose_mode() {
        return;
    }
    let encoder_name = match encoder {
        EncoderType::Av1 => "AV1",
        EncoderType::Hevc => "HEVC",
        EncoderType::Jxl => "JXL",
    };

    let d = &result.analysis_details;
    let codec = parse_source_codec(&analysis.source_codec);

    eprintln!("   Quality Analysis v3.0 ({}):", encoder_name);
    eprintln!(
        "      Mode: {:?} | Bias: {:?}",
        d.match_mode, d.quality_bias
    );
    eprintln!("      Confidence: {:.0}%", d.confidence * 100.0);
    eprintln!();

    eprintln!("      Source:");
    eprintln!(
        "         Codec: {} ({:?}, efficiency: {:.2})",
        analysis.source_codec, codec, d.codec_factor
    );
    if codec.is_cutting_edge() {
        eprintln!("         CUTTING-EDGE codec (VVC/AV2) - SKIP RECOMMENDED");
    } else if codec.is_modern() {
        eprintln!("         ⚠️  Modern codec - consider skipping re-encode");
    }
    eprintln!(
        "         Resolution: {}x{} (factor: {:.2})",
        analysis.width, analysis.height, d.resolution_factor
    );
    eprintln!(
        "         Bit depth: {}-bit (factor: {:.2})",
        analysis.bit_depth, d.color_depth_factor
    );
    eprintln!();

    eprintln!("      High Priority Factors:");
    eprintln!("         Raw BPP: {:.4}", d.raw_bpp);
    if let Some(vbr) = analysis.video_bitrate {
        eprintln!(
            "         Video bitrate: {} kbps (audio excluded)",
            vbr / 1000
        );
    }
    eprintln!("         GOP factor: {:.2}", d.gop_factor);
    if let Some(gop) = analysis.gop_size {
        eprintln!(
            "            └─ GOP size: {}, B-frames: {}",
            gop,
            analysis.b_frame_count.unwrap_or(0)
        );
    }
    eprintln!("         Chroma factor: {:.2}", d.chroma_factor);
    if let Some(ref pf) = analysis.pix_fmt {
        eprintln!("            └─ Pixel format: {}", pf);
    }
    eprintln!("         HDR factor: {:.2}", d.hdr_factor);
    if analysis.is_hdr == Some(true) {
        eprintln!("            └─ HDR content detected");
    }
    if d.content_type_adjustment != 0 {
        eprintln!(
            "         Content type adjustment: {:+} CRF",
            d.content_type_adjustment
        );
        if let Some(ct) = analysis.content_type {
            eprintln!("            └─ Type: {:?}", ct);
        }
    }
    eprintln!();

    eprintln!("      Medium Priority Factors:");
    eprintln!("         Aspect factor: {:.2}", d.aspect_factor);
    eprintln!("         Complexity factor: {:.2}", d.complexity_factor);
    if analysis.spatial_complexity.is_some() || analysis.temporal_complexity.is_some() {
        eprintln!(
            "            └─ SI: {:.1}, TI: {:.1}",
            analysis.spatial_complexity.unwrap_or(0.0),
            analysis.temporal_complexity.unwrap_or(0.0)
        );
    }
    eprintln!("         Grain factor: {:.2}", d.grain_factor);
    eprintln!("         Alpha factor: {:.2}", d.alpha_factor);
    eprintln!();

    eprintln!("      Result:");
    eprintln!("         Effective BPP: {:.4}", result.effective_bpp);
    if let Some(fps) = analysis.fps {
        eprintln!("         FPS: {:.2}", fps);
    }
    if let Some(duration) = analysis.duration_secs {
        eprintln!("         Duration: {:.1}s", duration);
    }

    match encoder {
        EncoderType::Av1 | EncoderType::Hevc => {
            eprintln!("         ✅ Calculated CRF: {}", result.crf);
        }
        EncoderType::Jxl => {
            eprintln!("         ✅ Calculated distance: {:.2}", result.distance);
        }
    }
}

pub fn from_video_detection(
    file_path: &str,
    codec: &str,
    width: u32,
    height: u32,
    bitrate: u64,
    fps: f64,
    duration_secs: f64,
    has_b_frames: bool,
    bit_depth: u8,
    file_size: u64,
) -> QualityAnalysis {
    let pixels_per_frame = (width as f64) * (height as f64);
    let pixels_per_second = pixels_per_frame * fps;

    let bpp = if pixels_per_second > 0.0 && bitrate > 0 {
        (bitrate as f64) / pixels_per_second
    } else {
        if pixels_per_second <= 0.0 {
            eprintln!(
                "   ⚠️  Warning: pixels_per_second is {} for {}",
                pixels_per_second, file_path
            );
        }
        if bitrate == 0 {
            eprintln!("   ⚠️  Warning: bitrate is 0 for {}", file_path);
        }
        0.0
    };

    QualityAnalysis {
        bpp,
        source_codec: codec.to_string(),
        width,
        height,
        has_b_frames,
        bit_depth,
        has_alpha: false,
        duration_secs: Some(duration_secs),
        fps: Some(fps),
        file_size,
        estimated_quality: None,
        ..Default::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct VideoAnalysisBuilder {
    analysis: QualityAnalysis,
}

impl VideoAnalysisBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn basic(
        mut self,
        codec: &str,
        width: u32,
        height: u32,
        fps: f64,
        duration_secs: f64,
    ) -> Self {
        self.analysis.source_codec = codec.to_string();
        self.analysis.width = width;
        self.analysis.height = height;
        self.analysis.fps = Some(fps);
        self.analysis.duration_secs = Some(duration_secs);
        self
    }

    pub fn file_size(mut self, size: u64) -> Self {
        self.analysis.file_size = size;
        self
    }

    pub fn video_bitrate(mut self, bitrate: u64) -> Self {
        self.analysis.video_bitrate = Some(bitrate);
        if let (Some(fps), w, h) = (self.analysis.fps, self.analysis.width, self.analysis.height) {
            if fps > 0.0 && w > 0 && h > 0 {
                let pixels = (w as f64) * (h as f64);
                self.analysis.bpp = (bitrate as f64 / fps) / pixels;
            }
        }
        self
    }

    pub fn gop(mut self, gop_size: u32, b_frames: u8) -> Self {
        self.analysis.gop_size = Some(gop_size);
        self.analysis.b_frame_count = Some(b_frames);
        self.analysis.has_b_frames = b_frames > 0;
        self
    }

    pub fn pix_fmt(mut self, fmt: &str) -> Self {
        self.analysis.pix_fmt = Some(fmt.to_string());
        self
    }

    pub fn color(mut self, color_space: &str, is_hdr: bool) -> Self {
        self.analysis.color_space = Some(color_space.to_string());
        self.analysis.is_hdr = Some(is_hdr);
        self
    }

    pub fn content_type(mut self, ct: ContentType) -> Self {
        self.analysis.content_type = Some(ct);
        self
    }

    pub fn bit_depth(mut self, depth: u8) -> Self {
        self.analysis.bit_depth = depth;
        self
    }

    pub fn complexity(mut self, spatial: f64, temporal: f64) -> Self {
        self.analysis.spatial_complexity = Some(spatial);
        self.analysis.temporal_complexity = Some(temporal);
        self
    }

    pub fn film_grain(mut self, has_grain: bool) -> Self {
        self.analysis.has_film_grain = Some(has_grain);
        self
    }

    pub fn preset(mut self, preset: &str) -> Self {
        self.analysis.encoder_preset = Some(preset.to_string());
        self
    }

    pub fn build(self) -> QualityAnalysis {
        self.analysis
    }
}

#[derive(Debug, Clone)]
pub struct SkipDecision {
    pub should_skip: bool,
    pub reason: String,
    pub codec: SourceCodec,
}

pub fn should_skip_video_codec(codec_str: &str) -> SkipDecision {
    let codec = parse_source_codec(codec_str);

    // Normal mode: skip all modern codecs (HEVC, AV1, VP9, VVC, AV2) — already modern, no need to process.
    // Only when Apple-compat flag is on do we convert AV1/VP9/VVC/AV2 via should_skip_video_codec_apple_compat (skip HEVC only).
    let should_skip = matches!(
        codec,
        SourceCodec::H265 | SourceCodec::Av1 | SourceCodec::Vp9 | SourceCodec::Vvc | SourceCodec::Av2
    );

    let reason = if should_skip {
        let codec_name = match codec {
            SourceCodec::H265 => "H.265/HEVC",
            SourceCodec::Av1 => "AV1",
            SourceCodec::Vp9 => "VP9",
            SourceCodec::Vvc => "H.266/VVC (cutting-edge)",
            SourceCodec::Av2 => "AV2 (cutting-edge)",
            _ => "modern codec",
        };
        format!(
            "Source is {} - skipping (modern format; use Apple-compat mode to convert to HEVC)",
            codec_name
        )
    } else {
        String::new()
    };

    SkipDecision {
        should_skip,
        reason,
        codec,
    }
}

pub fn should_skip_video_codec_apple_compat(codec_str: &str) -> SkipDecision {
    let codec = parse_source_codec(codec_str);

    let should_skip = matches!(codec, SourceCodec::H265);

    let reason = if should_skip {
        "Source is H.265/HEVC - already Apple compatible, skipping".to_string()
    } else {
        String::new()
    };

    SkipDecision {
        should_skip,
        reason,
        codec,
    }
}

/// True when the source codec is one that Apple devices do not support (or support poorly).
pub fn is_apple_incompatible_video_codec(codec_str: &str) -> bool {
    should_keep_best_effort_output_on_failure(codec_str)
}

/// True only when we may keep best-effort HEVC/AV1 output on compression/quality failure.
/// - Apple-incompatible (AV1, VP9, VVC, AV2): user still gets an importable file.
/// - ProRes/DNxHD are NOT included: they must pass strict size-shrink + SSIM; failure must not
///   keep output when size got bigger — decision is strictly by SSIM and size balance, never allow larger output.
pub fn should_keep_best_effort_output_on_failure(codec_str: &str) -> bool {
    let codec = parse_source_codec(codec_str);
    matches!(
        codec,
        SourceCodec::Av1 | SourceCodec::Vp9 | SourceCodec::Vvc | SourceCodec::Av2
    )
}

/// Single predicate for keeping Apple-compat fallback HEVC output (explore failure or require_compression path).
/// Returns true only when: Apple compat is on, source is not GIF, source codec is Apple-incompatible (AV1/VP9/VVC/AV2),
/// and either the video stream was compressed or (allow_size_tolerance && video_compression_ratio < 1.01).
/// Use this in both "explore failed" and "require_compression" branches so behavior stays consistent.
pub fn should_keep_apple_fallback_hevc_output(
    codec_str: &str,
    video_stream_compressed: bool,
    video_compression_ratio: f64,
    allow_size_tolerance: bool,
    apple_compat: bool,
    source_is_gif: bool,
) -> bool {
    if !apple_compat || source_is_gif || !is_apple_incompatible_video_codec(codec_str) {
        return false;
    }
    video_stream_compressed || (allow_size_tolerance && video_compression_ratio < 1.01)
}

pub fn should_skip_image_format(format_str: &str, is_lossless: bool) -> SkipDecision {
    let codec = parse_source_codec(format_str);

    let is_modern_lossy = !is_lossless
        && matches!(
            codec,
            SourceCodec::WebpStatic | SourceCodec::Avif | SourceCodec::Heic | SourceCodec::JpegXl
        );

    let is_jxl = matches!(codec, SourceCodec::JpegXl);

    let should_skip = is_modern_lossy || is_jxl;

    let reason = if should_skip {
        let codec_name = match codec {
            SourceCodec::WebpStatic => "lossy WebP",
            SourceCodec::Avif => "lossy AVIF",
            SourceCodec::Heic => "lossy HEIC/HEIF",
            SourceCodec::JpegXl => "JPEG XL (already optimal)",
            _ => "modern lossy format",
        };
        format!(
            "Source is {} - skipping to avoid generational loss",
            codec_name
        )
    } else {
        String::new()
    };

    SkipDecision {
        should_skip,
        reason,
        codec,
    }
}

pub fn from_image_analysis(
    format: &str,
    width: u32,
    height: u32,
    bit_depth: u8,
    has_alpha: bool,
    file_size: u64,
    duration_secs: Option<f64>,
    fps: Option<f64>,
    estimated_quality: Option<u8>,
) -> QualityAnalysis {
    let pixels = (width as u64) * (height as u64);

    let bpp = if let (Some(duration), Some(frame_rate)) = (duration_secs, fps) {
        if duration > 0.0 && frame_rate > 0.0 {
            let total_frames = (duration * frame_rate) as u64;
            let bits_per_frame = (file_size * 8) as f64 / total_frames.max(1) as f64;
            bits_per_frame / pixels as f64
        } else {
            file_size as f64 / pixels as f64
        }
    } else {
        file_size as f64 / pixels as f64
    };

    QualityAnalysis {
        bpp,
        source_codec: format.to_string(),
        width,
        height,
        has_b_frames: false,
        bit_depth,
        has_alpha,
        duration_secs,
        fps,
        file_size,
        estimated_quality,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_av1_crf_calculation() {
        let analysis = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: true,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: Some(60.0),
            fps: Some(30.0),
            file_size: 100_000_000,
            estimated_quality: None,
            ..Default::default()
        };

        let result = calculate_av1_crf(&analysis).unwrap();
        assert!(result.crf >= 15.0 && result.crf <= 40.0);
        assert!(result.analysis_details.confidence > 0.5);
    }

    #[test]
    fn test_hevc_crf_calculation() {
        let analysis = QualityAnalysis {
            bpp: 0.5,
            source_codec: "gif".to_string(),
            width: 640,
            height: 480,
            has_b_frames: false,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: Some(5.0),
            fps: Some(10.0),
            file_size: 5_000_000,
            estimated_quality: None,
            ..Default::default()
        };

        let result = calculate_hevc_crf(&analysis).unwrap();
        assert!(result.crf <= 35.0);
    }

    #[test]
    fn test_jxl_distance_with_quality() {
        let analysis = QualityAnalysis {
            bpp: 0.0,
            source_codec: "jpeg".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: false,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: None,
            fps: None,
            file_size: 500_000,
            estimated_quality: Some(85),
            ..Default::default()
        };

        let result = calculate_jxl_distance(&analysis).unwrap();
        assert!((result.distance - 1.5).abs() < 0.2);
    }

    #[test]
    fn test_gop_factor() {
        assert!(calculate_gop_factor(Some(1), 0) < 0.8);
        assert!(calculate_gop_factor(Some(250), 3) > 1.3);
        assert!((calculate_gop_factor(Some(30), 2) - 1.08).abs() < 0.1);
    }

    #[test]
    fn test_chroma_factor() {
        assert!((calculate_chroma_factor(Some("yuv420p")) - 1.0).abs() < 0.01);
        assert!(calculate_chroma_factor(Some("yuv444p")) > 1.1);
        assert!(calculate_chroma_factor(Some("rgb24")) > 1.1);
    }

    #[test]
    fn test_hdr_factor() {
        assert!((calculate_hdr_factor(None, Some("bt709")) - 1.0).abs() < 0.01);
        assert!(calculate_hdr_factor(Some(true), None) > 1.2);
        assert!(calculate_hdr_factor(None, Some("bt2020nc")) > 1.1);
    }

    #[test]
    fn test_quality_bias() {
        let analysis = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            file_size: 100_000_000,
            fps: Some(30.0),
            duration_secs: Some(60.0),
            ..Default::default()
        };

        let conservative = calculate_av1_crf_with_options(
            &analysis,
            MatchMode::Quality,
            QualityBias::Conservative,
        )
        .unwrap();
        let balanced =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Balanced)
                .unwrap();
        let aggressive =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Aggressive)
                .unwrap();

        assert!(conservative.crf <= balanced.crf);
        assert!(aggressive.crf >= balanced.crf);
    }

    #[test]
    fn test_parse_source_codec() {
        assert_eq!(parse_source_codec("h264"), SourceCodec::H264);
        assert_eq!(parse_source_codec("H.265/HEVC"), SourceCodec::H265);
        assert_eq!(parse_source_codec("AV1"), SourceCodec::Av1);
        assert_eq!(parse_source_codec("GIF"), SourceCodec::Gif);

        assert_eq!(parse_source_codec("VVC"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("H.266"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("h266"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("AV2"), SourceCodec::Av2);
        assert_eq!(parse_source_codec("avm"), SourceCodec::Av2);

        assert_eq!(parse_source_codec("JPEG XL"), SourceCodec::JpegXl);
        assert_eq!(parse_source_codec("jxl"), SourceCodec::JpegXl);
        assert_eq!(parse_source_codec("AVIF"), SourceCodec::Avif);
        assert_eq!(parse_source_codec("HEIC"), SourceCodec::Heic);

        assert_eq!(parse_source_codec("FFV1"), SourceCodec::Ffv1);
        assert_eq!(parse_source_codec("UTVideo"), SourceCodec::UtVideo);
        assert_eq!(parse_source_codec("HuffYUV"), SourceCodec::HuffYuv);

        assert_eq!(parse_source_codec("unknown_codec"), SourceCodec::Unknown);
    }

    #[test]
    fn test_codec_properties() {
        assert!(SourceCodec::H265.is_modern());
        assert!(SourceCodec::Av1.is_modern());
        assert!(SourceCodec::Vvc.is_modern());
        assert!(SourceCodec::Av2.is_modern());
        assert!(!SourceCodec::H264.is_modern());

        assert!(SourceCodec::Vvc.is_cutting_edge());
        assert!(SourceCodec::Av2.is_cutting_edge());
        assert!(!SourceCodec::Av1.is_cutting_edge());

        assert!(SourceCodec::Ffv1.is_lossless());
        assert!(SourceCodec::Png.is_lossless());
        assert!(!SourceCodec::H264.is_lossless());
    }

    #[test]
    fn test_codec_efficiency_ordering() {
        assert!(SourceCodec::Av1.efficiency_factor() < SourceCodec::H265.efficiency_factor());
        assert!(SourceCodec::H265.efficiency_factor() < SourceCodec::H264.efficiency_factor());
        assert!(SourceCodec::Vvc.efficiency_factor() < SourceCodec::Av1.efficiency_factor());
        assert!(SourceCodec::Av2.efficiency_factor() <= SourceCodec::Vvc.efficiency_factor());

        assert!(SourceCodec::Gif.efficiency_factor() > 2.0);
    }

    #[test]
    fn test_confidence_calculation() {
        let complete = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: true,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: Some(60.0),
            fps: Some(30.0),
            file_size: 100_000_000,
            estimated_quality: Some(85),
            video_bitrate: Some(5_000_000),
            gop_size: Some(60),
            b_frame_count: Some(3),
            pix_fmt: Some("yuv420p".to_string()),
            ..Default::default()
        };
        let result = calculate_av1_crf(&complete).unwrap();
        assert!(result.analysis_details.confidence > 0.8);

        let minimal = QualityAnalysis {
            bpp: 0.0,
            source_codec: "unknown".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: false,
            bit_depth: 0,
            has_alpha: false,
            duration_secs: None,
            fps: None,
            file_size: 100_000_000,
            estimated_quality: None,
            ..Default::default()
        };
        let result = calculate_av1_crf(&minimal).unwrap();
        assert!(result.analysis_details.confidence < 0.7);
    }

    #[test]
    fn test_should_skip_video_codec() {
        assert!(should_skip_video_codec("hevc").should_skip);
        assert!(should_skip_video_codec("h265").should_skip);
        assert!(should_skip_video_codec("av1").should_skip);
        assert!(should_skip_video_codec("vp9").should_skip);
        assert!(should_skip_video_codec("vvc").should_skip);
        assert!(should_skip_video_codec("h266").should_skip);
        assert!(should_skip_video_codec("av2").should_skip);

        assert!(!should_skip_video_codec("h264").should_skip);
        assert!(!should_skip_video_codec("mpeg4").should_skip);
        assert!(!should_skip_video_codec("prores").should_skip);
        assert!(!should_skip_video_codec("ffv1").should_skip);
    }

    #[test]
    fn test_should_skip_image_format() {
        assert!(should_skip_image_format("webp", false).should_skip);
        assert!(should_skip_image_format("avif", false).should_skip);
        assert!(should_skip_image_format("heic", false).should_skip);

        assert!(should_skip_image_format("jxl", true).should_skip);
        assert!(should_skip_image_format("jxl", false).should_skip);

        assert!(!should_skip_image_format("webp", true).should_skip);
        assert!(!should_skip_image_format("avif", true).should_skip);

        assert!(!should_skip_image_format("jpeg", false).should_skip);
        assert!(!should_skip_image_format("png", true).should_skip);
        assert!(!should_skip_image_format("gif", true).should_skip);
    }

    #[test]
    fn test_precision_1080p_h264_8mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .bit_depth(8)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        eprintln!("1080p H.264 8Mbps test:");
        eprintln!("  raw_bpp: {:.4}", result.analysis_details.raw_bpp);
        eprintln!("  effective_bpp: {:.4}", result.effective_bpp);
        eprintln!(
            "  codec_factor: {:.2}",
            result.analysis_details.codec_factor
        );
        eprintln!("  gop_factor: {:.2}", result.analysis_details.gop_factor);
        eprintln!("  CRF: {}", result.crf);

        assert!(
            result.crf >= 18.0 && result.crf <= 32.0,
            "1080p H.264 8Mbps: expected CRF 18-32, got {}",
            result.crf
        );

        assert!(
            result.effective_bpp > 0.05 && result.effective_bpp < 2.0,
            "Effective BPP out of range: {}",
            result.effective_bpp
        );
    }

    #[test]
    fn test_precision_4k_h264_20mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(20_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .bit_depth(8)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 22.0 && result.crf <= 32.0,
            "4K H.264 20Mbps: expected CRF 22-32, got {}",
            result.crf
        );
    }

    #[test]
    fn test_precision_animation_content() {
        let base = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 24.0, 60.0)
            .video_bitrate(5_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .build();

        let animation = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 24.0, 60.0)
            .video_bitrate(5_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .content_type(ContentType::Animation)
            .build();

        let base_result = calculate_av1_crf(&base).unwrap();
        let anim_result = calculate_av1_crf(&animation).unwrap();

        let crf_diff = anim_result.crf as i32 - base_result.crf as i32;
        assert!(
            (2..=6).contains(&crf_diff),
            "Animation CRF adjustment: expected +2 to +6, got {:+}",
            crf_diff
        );
    }

    #[test]
    fn test_precision_film_grain_content() {
        let base = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 24.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .build();

        let grain = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 24.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .content_type(ContentType::FilmGrain)
            .film_grain(true)
            .build();

        let base_result = calculate_av1_crf(&base).unwrap();
        let grain_result = calculate_av1_crf(&grain).unwrap();

        assert!(
            grain_result.crf <= base_result.crf,
            "Film grain CRF should be <= baseline: grain={}, base={}",
            grain_result.crf,
            base_result.crf
        );

        assert!(
            grain_result.analysis_details.grain_factor > 1.1,
            "Grain factor should be > 1.1: {}",
            grain_result.analysis_details.grain_factor
        );
    }

    #[test]
    fn test_precision_hdr_content() {
        let sdr = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p10le")
            .color("bt709", false)
            .bit_depth(10)
            .build();

        let hdr = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p10le")
            .color("bt2020nc", true)
            .bit_depth(10)
            .build();

        let sdr_result = calculate_av1_crf(&sdr).unwrap();
        let hdr_result = calculate_av1_crf(&hdr).unwrap();

        assert!(
            hdr_result.crf <= sdr_result.crf,
            "HDR should have CRF <= SDR: HDR={}, SDR={}",
            hdr_result.crf,
            sdr_result.crf
        );
    }

    #[test]
    fn test_precision_chroma_subsampling() {
        let yuv420 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let yuv444 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv444p")
            .build();

        let yuv420_result = calculate_av1_crf(&yuv420).unwrap();
        let yuv444_result = calculate_av1_crf(&yuv444).unwrap();

        assert!(
            yuv444_result.crf <= yuv420_result.crf,
            "YUV444 should have CRF <= YUV420: 444={}, 420={}",
            yuv444_result.crf,
            yuv420_result.crf
        );
    }

    #[test]
    fn test_precision_gop_structure() {
        let all_intra = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(20_000_000)
            .gop(1, 0)
            .pix_fmt("yuv420p")
            .build();

        let long_gop = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(250, 3)
            .pix_fmt("yuv420p")
            .build();

        let intra_result = calculate_av1_crf(&all_intra).unwrap();
        let gop_result = calculate_av1_crf(&long_gop).unwrap();

        assert!(
            intra_result.analysis_details.gop_factor < 0.8,
            "All-intra GOP factor should be < 0.8: {}",
            intra_result.analysis_details.gop_factor
        );
        assert!(
            gop_result.analysis_details.gop_factor > 1.2,
            "Long GOP factor should be > 1.2: {}",
            gop_result.analysis_details.gop_factor
        );
    }

    #[test]
    fn test_precision_screen_recording() {
        let screen = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(2_000_000)
            .gop(60, 0)
            .pix_fmt("yuv420p")
            .content_type(ContentType::ScreenRecording)
            .build();

        let result = calculate_av1_crf(&screen).unwrap();

        assert!(
            result.crf >= 25.0,
            "Screen recording should allow CRF >= 25, got {}",
            result.crf
        );

        assert!(
            result.analysis_details.content_type_adjustment > 0,
            "Screen recording should have positive CRF adjustment"
        );
    }

    #[test]
    fn test_precision_ultrawide_aspect() {
        let standard = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let ultrawide = VideoAnalysisBuilder::new()
            .basic("h264", 2560, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let _standard_result = calculate_av1_crf(&standard).unwrap();
        let ultrawide_result = calculate_av1_crf(&ultrawide).unwrap();

        assert!(
            ultrawide_result.analysis_details.aspect_factor > 1.0,
            "Ultra-wide should have aspect factor > 1.0: {}",
            ultrawide_result.analysis_details.aspect_factor
        );
    }

    #[test]
    fn test_precision_codec_efficiency() {
        let h264_source = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let hevc_source = VideoAnalysisBuilder::new()
            .basic("hevc", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let h264_result = calculate_av1_crf(&h264_source).unwrap();
        let hevc_result = calculate_av1_crf(&hevc_source).unwrap();

        assert!(
            hevc_result.analysis_details.codec_factor < h264_result.analysis_details.codec_factor,
            "HEVC should have lower codec factor: HEVC={}, H264={}",
            hevc_result.analysis_details.codec_factor,
            h264_result.analysis_details.codec_factor
        );
    }

    #[test]
    fn test_precision_boundary_low_bpp() {
        let low_bpp = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(500_000)
            .gop(60, 0)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&low_bpp).unwrap();

        assert!(
            result.crf <= 40.0,
            "Ultra-low BPP should cap CRF at 40, got {}",
            result.crf
        );
        assert!(
            result.crf >= 28.0,
            "Ultra-low BPP should have CRF >= 28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_precision_boundary_high_bpp() {
        let high_bpp = VideoAnalysisBuilder::new()
            .basic("prores", 1920, 1080, 30.0, 60.0)
            .video_bitrate(150_000_000)
            .gop(1, 0)
            .pix_fmt("yuv422p10le")
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&high_bpp).unwrap();

        assert!(
            result.crf >= 15.0,
            "Ultra-high BPP should floor CRF at 15, got {}",
            result.crf
        );
        assert!(
            result.crf <= 25.0,
            "ProRes source should produce CRF <= 25, got {}",
            result.crf
        );
    }

    #[test]
    fn test_precision_jxl_jpeg_q85() {
        let jpeg = QualityAnalysis {
            source_codec: "jpeg".to_string(),
            width: 1920,
            height: 1080,
            file_size: 500_000,
            estimated_quality: Some(85),
            ..Default::default()
        };

        let result = calculate_jxl_distance(&jpeg).unwrap();

        assert!(
            (result.distance - 1.5).abs() < 0.3,
            "JPEG Q85 should produce distance ~1.5, got {}",
            result.distance
        );
    }

    #[test]
    fn test_precision_jxl_jpeg_q95() {
        let jpeg = QualityAnalysis {
            source_codec: "jpeg".to_string(),
            width: 1920,
            height: 1080,
            file_size: 1_000_000,
            estimated_quality: Some(95),
            ..Default::default()
        };

        let result = calculate_jxl_distance(&jpeg).unwrap();

        assert!(
            (result.distance - 0.5).abs() < 0.3,
            "JPEG Q95 should produce distance ~0.5, got {}",
            result.distance
        );
    }

    #[test]
    fn test_precision_hevc_gif_source() {
        let gif = QualityAnalysis {
            bpp: 0.5,
            source_codec: "gif".to_string(),
            width: 640,
            height: 480,
            bit_depth: 8,
            duration_secs: Some(5.0),
            fps: Some(10.0),
            file_size: 5_000_000,
            ..Default::default()
        };

        let result = calculate_hevc_crf(&gif).unwrap();

        assert!(
            result.crf >= 20.0 && result.crf <= 32.0,
            "GIF to HEVC should produce CRF 20-32, got {}",
            result.crf
        );

        assert!(
            result.analysis_details.codec_factor > 2.0,
            "GIF codec factor should be > 2.0: {}",
            result.analysis_details.codec_factor
        );
    }

    #[test]
    fn test_precision_consistency() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let result1 = calculate_av1_crf(&analysis).unwrap();
        let result2 = calculate_av1_crf(&analysis).unwrap();

        assert_eq!(
            result1.crf, result2.crf,
            "Same input should produce same CRF"
        );
        assert!(
            (result1.effective_bpp - result2.effective_bpp).abs() < 0.0001,
            "Same input should produce same effective BPP"
        );
    }

    #[test]
    fn test_precision_mode_comparison() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let quality =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Balanced)
                .unwrap();
        let size =
            calculate_av1_crf_with_options(&analysis, MatchMode::Size, QualityBias::Balanced)
                .unwrap();

        assert!(
            size.crf >= quality.crf,
            "Size mode should have CRF >= Quality mode: Size={}, Quality={}",
            size.crf,
            quality.crf
        );
    }

    #[test]
    fn test_strict_1080p_5mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 120.0)
            .video_bitrate(5_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 23.0 && result.crf <= 27.0,
            "STRICT: 1080p 5Mbps expected CRF 23-27, got {}",
            result.crf
        );
    }

    #[test]
    fn test_strict_720p_2mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1280, 720, 30.0, 60.0)
            .video_bitrate(2_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 25.0 && result.crf <= 29.0,
            "STRICT: 720p 2Mbps expected CRF 25-29, got {}",
            result.crf
        );
    }

    #[test]
    fn test_strict_4k_15mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 24.0 && result.crf <= 28.0,
            "STRICT: 4K 15Mbps expected CRF 24-28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_extremely_low_bitrate() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(500_000)
            .gop(60, 0)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 30.0 && result.crf <= 40.0,
            "EDGE: Extremely low bitrate should cap CRF 30-40, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_extremely_high_bitrate() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("prores", 1920, 1080, 30.0, 60.0)
            .video_bitrate(100_000_000)
            .gop(1, 0)
            .pix_fmt("yuv422p10le")
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 15.0 && result.crf <= 22.0,
            "EDGE: Extremely high bitrate should floor CRF 15-22, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_small_resolution() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 320, 240, 15.0, 30.0)
            .video_bitrate(500_000)
            .gop(30, 1)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 15.0 && result.crf <= 25.0,
            "EDGE: Small resolution high-bpp should produce CRF 15-25, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_8k_resolution() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 7680, 4320, 30.0, 60.0)
            .video_bitrate(50_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p10le")
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 28.0 && result.crf <= 38.0,
            "EDGE: 8K low-bpp should produce CRF 28-38, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_high_framerate() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 120.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(120, 3)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 18.0 && result.crf <= 28.0,
            "EDGE: 120fps should produce CRF 18-28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_short_gop() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(10_000_000)
            .gop(2, 0)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.analysis_details.gop_factor < 0.9,
            "EDGE: Short GOP factor should be < 0.9, got {}",
            result.analysis_details.gop_factor
        );
    }

    #[test]
    fn test_edge_max_bframes() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(250, 8)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.analysis_details.gop_factor > 1.3,
            "EDGE: Max B-frames GOP factor should be > 1.3, got {}",
            result.analysis_details.gop_factor
        );
    }

    #[test]
    fn test_edge_10bit_hdr() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(20_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p10le")
            .color("bt2020nc", true)
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.analysis_details.hdr_factor > 1.1,
            "EDGE: HDR factor should be > 1.1, got {}",
            result.analysis_details.hdr_factor
        );

        assert!(
            result.crf >= 20.0 && result.crf <= 28.0,
            "EDGE: 10-bit HDR should produce CRF 20-28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_rgb_format() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(60, 2)
            .pix_fmt("rgb24")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.analysis_details.chroma_factor > 1.1,
            "EDGE: RGB chroma factor should be > 1.1, got {}",
            result.analysis_details.chroma_factor
        );
    }

    #[test]
    fn test_edge_vertical_video() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1080, 1920, 30.0, 60.0)
            .video_bitrate(5_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 20.0 && result.crf <= 30.0,
            "EDGE: Vertical video should produce CRF 20-30, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_ultrawide_cinema() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 2560, 1080, 24.0, 120.0)
            .video_bitrate(8_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 20.0 && result.crf <= 28.0,
            "EDGE: Ultra-wide cinema should produce CRF 20-28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_lossless_source() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("ffv1", 1920, 1080, 30.0, 60.0)
            .video_bitrate(200_000_000)
            .gop(1, 0)
            .pix_fmt("yuv444p10le")
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap();

        assert!(
            result.crf >= 15.0 && result.crf <= 25.0,
            "EDGE: Lossless source should produce CRF 15-25, got {}",
            result.crf
        );
    }

    #[test]
    fn test_factor_gop_isolation() {
        let short_gop = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(10, 1)
            .pix_fmt("yuv420p")
            .build();

        let long_gop = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(250, 3)
            .pix_fmt("yuv420p")
            .build();

        let short_result = calculate_av1_crf(&short_gop).unwrap();
        let long_result = calculate_av1_crf(&long_gop).unwrap();

        assert!(
            long_result.analysis_details.gop_factor > short_result.analysis_details.gop_factor,
            "Long GOP factor ({}) should be > short GOP factor ({})",
            long_result.analysis_details.gop_factor,
            short_result.analysis_details.gop_factor
        );
    }

    #[test]
    fn test_factor_chroma_isolation() {
        let yuv420 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let yuv444 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv444p")
            .build();

        let yuv420_result = calculate_av1_crf(&yuv420).unwrap();
        let yuv444_result = calculate_av1_crf(&yuv444).unwrap();

        assert!(
            yuv444_result.analysis_details.chroma_factor
                > yuv420_result.analysis_details.chroma_factor,
            "YUV444 chroma factor ({}) should be > YUV420 ({})",
            yuv444_result.analysis_details.chroma_factor,
            yuv420_result.analysis_details.chroma_factor
        );
    }

    #[test]
    fn test_factor_hdr_isolation() {
        let sdr = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .build();

        let hdr = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .color("bt2020nc", true)
            .build();

        let sdr_result = calculate_av1_crf(&sdr).unwrap();
        let hdr_result = calculate_av1_crf(&hdr).unwrap();

        assert!(
            hdr_result.analysis_details.hdr_factor > sdr_result.analysis_details.hdr_factor,
            "HDR factor ({}) should be > SDR ({})",
            hdr_result.analysis_details.hdr_factor,
            sdr_result.analysis_details.hdr_factor
        );
    }

    #[test]
    fn test_factor_content_type_isolation() {
        let live_action = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .content_type(ContentType::LiveAction)
            .build();

        let animation = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .content_type(ContentType::Animation)
            .build();

        let live_result = calculate_av1_crf(&live_action).unwrap();
        let anim_result = calculate_av1_crf(&animation).unwrap();

        assert!(
            anim_result.analysis_details.content_type_adjustment
                > live_result.analysis_details.content_type_adjustment,
            "Animation adjustment ({}) should be > LiveAction ({})",
            anim_result.analysis_details.content_type_adjustment,
            live_result.analysis_details.content_type_adjustment
        );

        assert!(
            anim_result.crf > live_result.crf,
            "Animation CRF ({}) should be > LiveAction ({})",
            anim_result.crf,
            live_result.crf
        );
    }

    #[test]
    fn test_factor_bias_isolation() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let conservative = calculate_av1_crf_with_options(
            &analysis,
            MatchMode::Quality,
            QualityBias::Conservative,
        )
        .unwrap();
        let balanced =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Balanced)
                .unwrap();
        let aggressive =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Aggressive)
                .unwrap();

        assert!(
            conservative.crf < balanced.crf,
            "Conservative CRF ({}) should be < Balanced ({})",
            conservative.crf,
            balanced.crf
        );
        assert!(
            balanced.crf < aggressive.crf,
            "Balanced CRF ({}) should be < Aggressive ({})",
            balanced.crf,
            aggressive.crf
        );

        assert!(
            (balanced.crf - conservative.crf - 2.0).abs() < 0.1,
            "Conservative should be exactly 2 less than Balanced"
        );
        assert!(
            (aggressive.crf - balanced.crf - 2.0).abs() < 0.1,
            "Aggressive should be exactly 2 more than Balanced"
        );
    }
}

#[test]
fn test_apple_compat_skip_hevc_only() {
    let hevc = should_skip_video_codec_apple_compat("hevc");
    assert!(
        hevc.should_skip,
        "HEVC should be skipped in Apple compat mode"
    );
    assert!(
        hevc.reason.contains("Apple compatible"),
        "HEVC skip reason should mention Apple compatible"
    );

    let h265 = should_skip_video_codec_apple_compat("h265");
    assert!(
        h265.should_skip,
        "H.265 should be skipped in Apple compat mode"
    );
}

#[test]
fn test_apple_compat_convert_vp9() {
    let vp9 = should_skip_video_codec_apple_compat("vp9");
    assert!(
        !vp9.should_skip,
        "VP9 should NOT be skipped in Apple compat mode"
    );
    assert_eq!(vp9.codec, SourceCodec::Vp9);
}

#[test]
fn test_apple_compat_convert_av1() {
    let av1 = should_skip_video_codec_apple_compat("av1");
    assert!(
        !av1.should_skip,
        "AV1 should NOT be skipped in Apple compat mode"
    );
    assert_eq!(av1.codec, SourceCodec::Av1);
}

#[test]
fn test_apple_compat_convert_vvc() {
    let vvc = should_skip_video_codec_apple_compat("vvc");
    assert!(
        !vvc.should_skip,
        "VVC should NOT be skipped in Apple compat mode"
    );

    let h266 = should_skip_video_codec_apple_compat("h266");
    assert!(
        !h266.should_skip,
        "H.266 should NOT be skipped in Apple compat mode"
    );
}

#[test]
fn test_apple_compat_convert_av2() {
    let av2 = should_skip_video_codec_apple_compat("av2");
    assert!(
        !av2.should_skip,
        "AV2 should NOT be skipped in Apple compat mode"
    );
}

#[test]
fn test_apple_compat_legacy_codecs() {
    assert!(!should_skip_video_codec("h264").should_skip);
    assert!(!should_skip_video_codec_apple_compat("h264").should_skip);

    assert!(!should_skip_video_codec("mpeg4").should_skip);
    assert!(!should_skip_video_codec_apple_compat("mpeg4").should_skip);

    assert!(!should_skip_video_codec("prores").should_skip);
    assert!(!should_skip_video_codec_apple_compat("prores").should_skip);
}

#[test]
fn test_apple_compat_vs_normal_mode() {
    assert!(should_skip_video_codec("vp9").should_skip);
    assert!(!should_skip_video_codec_apple_compat("vp9").should_skip);

    assert!(should_skip_video_codec("av1").should_skip);
    assert!(!should_skip_video_codec_apple_compat("av1").should_skip);

    assert!(should_skip_video_codec("hevc").should_skip);
    assert!(should_skip_video_codec_apple_compat("hevc").should_skip);

    assert!(!should_skip_video_codec("h264").should_skip);
    assert!(!should_skip_video_codec_apple_compat("h264").should_skip);
}

#[test]
fn test_apple_compat_codec_detection() {
    assert_eq!(
        should_skip_video_codec_apple_compat("vp9").codec,
        SourceCodec::Vp9
    );
    assert_eq!(
        should_skip_video_codec_apple_compat("av1").codec,
        SourceCodec::Av1
    );
    assert_eq!(
        should_skip_video_codec_apple_compat("hevc").codec,
        SourceCodec::H265
    );
    assert_eq!(
        should_skip_video_codec_apple_compat("vvc").codec,
        SourceCodec::Vvc
    );
    assert_eq!(
        should_skip_video_codec_apple_compat("h264").codec,
        SourceCodec::H264
    );
}

#[test]
fn test_apple_compat_case_insensitive() {
    assert!(should_skip_video_codec_apple_compat("HEVC").should_skip);
    assert!(should_skip_video_codec_apple_compat("Hevc").should_skip);
    assert!(should_skip_video_codec_apple_compat("hevc").should_skip);

    assert!(!should_skip_video_codec_apple_compat("VP9").should_skip);
    assert!(!should_skip_video_codec_apple_compat("Vp9").should_skip);
    assert!(!should_skip_video_codec_apple_compat("vp9").should_skip);
}

#[test]
fn test_is_apple_incompatible_video_codec() {
    assert!(is_apple_incompatible_video_codec("av1"));
    assert!(is_apple_incompatible_video_codec("vp9"));
    assert!(is_apple_incompatible_video_codec("vvc"));
    assert!(is_apple_incompatible_video_codec("h266"));
    assert!(is_apple_incompatible_video_codec("av2"));
    assert!(is_apple_incompatible_video_codec("AV1"));
    assert!(is_apple_incompatible_video_codec("libaom-av1"));

    assert!(!is_apple_incompatible_video_codec("hevc"));
    assert!(!is_apple_incompatible_video_codec("h265"));
    assert!(!is_apple_incompatible_video_codec("h264"));
    assert!(!is_apple_incompatible_video_codec("H.264"));
    assert!(!is_apple_incompatible_video_codec("prores"));
    assert!(!is_apple_incompatible_video_codec("dnxhd"));
    assert!(!is_apple_incompatible_video_codec("ffv1"));
}

#[test]
fn test_should_keep_best_effort_output_on_failure() {
    assert!(should_keep_best_effort_output_on_failure("av1"));
    assert!(should_keep_best_effort_output_on_failure("vp9"));
    assert!(should_keep_best_effort_output_on_failure("vvc"));
    assert!(should_keep_best_effort_output_on_failure("av2"));
    assert!(!should_keep_best_effort_output_on_failure("prores"));
    assert!(!should_keep_best_effort_output_on_failure("dnxhd"));
    assert!(!should_keep_best_effort_output_on_failure("h264"));
    assert!(!should_keep_best_effort_output_on_failure("hevc"));
}

#[test]
fn test_strict_apple_compat_routing() {
    let test_cases = [
        ("h264", false, false),
        ("mpeg4", false, false),
        ("prores", false, false),
        ("hevc", true, true),
        ("h265", true, true),
        ("vp9", true, false),
        ("av1", true, false),
        ("vvc", true, false),
        ("h266", true, false),
        ("av2", true, false),
    ];

    for (codec, expected_normal, expected_apple) in test_cases {
        let normal = should_skip_video_codec(codec);
        let apple = should_skip_video_codec_apple_compat(codec);

        assert_eq!(
            normal.should_skip, expected_normal,
            "STRICT: {} normal mode: expected skip={}, got skip={}",
            codec, expected_normal, normal.should_skip
        );

        assert_eq!(
            apple.should_skip, expected_apple,
            "STRICT: {} Apple compat mode: expected skip={}, got skip={}",
            codec, expected_apple, apple.should_skip
        );
    }
}

#[test]
fn test_apple_compat_hevc_crf_vp9_source() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("vp9", 1920, 1080, 30.0, 60.0)
        .bit_depth(8)
        .file_size(45_000_000)
        .video_bitrate(6_000_000)
        .pix_fmt("yuv420p")
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap();
    assert!(
        result.crf >= 18.0 && result.crf <= 28.0,
        "VP9→HEVC CRF should be 18-28, got {:.1}",
        result.crf
    );
}

#[test]
fn test_apple_compat_hevc_crf_av1_source() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("av1", 1920, 1080, 30.0, 60.0)
        .bit_depth(8)
        .file_size(30_000_000)
        .video_bitrate(4_000_000)
        .pix_fmt("yuv420p")
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap();
    assert!(
        result.crf >= 16.0 && result.crf <= 26.0,
        "AV1→HEVC CRF should be 16-26, got {:.1}",
        result.crf
    );
}

#[test]
fn test_apple_compat_hevc_crf_4k_hdr() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("av1", 3840, 2160, 60.0, 120.0)
        .bit_depth(10)
        .file_size(1_800_000_000)
        .video_bitrate(120_000_000)
        .pix_fmt("yuv420p10le")
        .color("bt2020nc", true)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap();
    assert!(
        result.crf >= 0.0 && result.crf <= 22.0,
        "4K HDR should get CRF <= 22, got {:.1}",
        result.crf
    );
    assert!(
        result.analysis_details.hdr_factor > 1.0,
        "HDR factor should increase effective BPP (>1.0), got {:.2}",
        result.analysis_details.hdr_factor
    );
}

#[test]
fn test_apple_compat_codec_efficiency() {
    assert!(SourceCodec::Av1.efficiency_factor() < SourceCodec::Vp9.efficiency_factor());
    assert!(
        (SourceCodec::Vp9.efficiency_factor() - SourceCodec::H265.efficiency_factor()).abs() < 0.1
    );
    assert!(SourceCodec::Vvc.efficiency_factor() < SourceCodec::Av1.efficiency_factor());
}

#[test]
fn test_h264_to_hevc_crf_1080p_8mbps() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 1920, 1080, 30.0, 120.0)
        .bit_depth(8)
        .file_size(120_000_000)
        .video_bitrate(8_000_000)
        .pix_fmt("yuv420p")
        .gop(60, 2)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap();
    assert!(
        result.crf >= 18.0 && result.crf <= 26.0,
        "H.264 8Mbps 1080p→HEVC should get CRF 18-26, got {:.1}",
        result.crf
    );
    assert!(
        (result.analysis_details.codec_factor - 1.0).abs() < 0.2,
        "H.264 codec factor should be ~1.0"
    );
}

#[test]
fn test_h264_to_hevc_crf_720p_4mbps() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 1280, 720, 30.0, 60.0)
        .bit_depth(8)
        .file_size(30_000_000)
        .video_bitrate(4_000_000)
        .pix_fmt("yuv420p")
        .gop(30, 2)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap();
    assert!(
        result.crf >= 20.0 && result.crf <= 28.0,
        "H.264 4Mbps 720p→HEVC should get CRF 20-28, got {:.1}",
        result.crf
    );
}

#[test]
fn test_h264_to_hevc_crf_4k_20mbps() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 3840, 2160, 30.0, 180.0)
        .bit_depth(8)
        .file_size(450_000_000)
        .video_bitrate(20_000_000)
        .pix_fmt("yuv420p")
        .gop(60, 3)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap();
    assert!(
        result.crf >= 18.0 && result.crf <= 30.0,
        "H.264 20Mbps 4K→HEVC should get CRF 18-30, got {:.1}",
        result.crf
    );
}

#[test]
fn test_h264_to_hevc_crf_low_bitrate() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 854, 480, 24.0, 300.0)
        .bit_depth(8)
        .file_size(45_000_000)
        .video_bitrate(1_200_000)
        .pix_fmt("yuv420p")
        .gop(48, 1)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap();
    assert!(
        result.crf >= 24.0 && result.crf <= 32.0,
        "H.264 1.2Mbps 480p→HEVC should get CRF 24-32, got {:.1}",
        result.crf
    );
}

#[test]
fn test_h264_to_hevc_crf_bluray_quality() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 1920, 1080, 24.0, 7200.0)
        .bit_depth(8)
        .file_size(4_500_000_000)
        .video_bitrate(40_000_000)
        .pix_fmt("yuv420p")
        .gop(24, 3)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap();
    assert!(
        result.crf >= 0.0 && result.crf <= 22.0,
        "H.264 40Mbps Blu-ray→HEVC should get CRF 0-22, got {:.1}",
        result.crf
    );
}

#[test]
fn test_h264_vs_av1_efficiency_comparison() {
    let h264 = VideoAnalysisBuilder::new()
        .basic("h264", 1920, 1080, 30.0, 60.0)
        .bit_depth(8)
        .file_size(60_000_000)
        .video_bitrate(8_000_000)
        .pix_fmt("yuv420p")
        .build();

    let av1 = VideoAnalysisBuilder::new()
        .basic("av1", 1920, 1080, 30.0, 60.0)
        .bit_depth(8)
        .file_size(30_000_000)
        .video_bitrate(4_000_000)
        .pix_fmt("yuv420p")
        .build();

    let h264_result = calculate_hevc_crf(&h264).unwrap();
    let av1_result = calculate_hevc_crf(&av1).unwrap();

    let crf_diff = (h264_result.crf - av1_result.crf).abs();
    assert!(
        crf_diff <= 4.0,
        "H.264 vs AV1 CRF diff should be <=4, got {:.1} (H.264:{:.1}, AV1:{:.1})",
        crf_diff,
        h264_result.crf,
        av1_result.crf
    );
}

#[test]
fn test_h264_should_not_skip() {
    let decision = should_skip_video_codec("h264");
    assert!(!decision.should_skip, "H.264 should NOT be skipped");
    assert_eq!(decision.codec, SourceCodec::H264);

    let avc = should_skip_video_codec("avc");
    assert!(!avc.should_skip, "AVC should NOT be skipped");
}

#[test]
fn test_h264_apple_compat_should_not_skip() {
    let decision = should_skip_video_codec_apple_compat("h264");
    assert!(
        !decision.should_skip,
        "H.264 should NOT be skipped in Apple compat"
    );
    assert_eq!(decision.codec, SourceCodec::H264);
}
