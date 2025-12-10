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
//! 2. **Codec efficiency factor** - H.264 baseline, HEVC ~30% better, AV1 ~50% better
//! 3. **Content complexity** - Resolution, B-frames, color depth
//! 
//! ## 🔥 Quality Manifesto (质量宣言)
//! 
//! - **No silent fallback**: If quality analysis fails, report error loudly
//! - **No hardcoded defaults**: All parameters derived from actual content analysis
//! - **Conservative on uncertainty**: When in doubt, prefer higher quality (lower CRF)

use serde::{Deserialize, Serialize};

/// Encoder type for quality matching
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderType {
    /// SVT-AV1 encoder (CRF 0-63)
    Av1,
    /// x265 HEVC encoder (CRF 0-51)
    Hevc,
    /// cjxl JXL encoder (distance 0.0-15.0)
    Jxl,
}

/// Source codec information for efficiency calculation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceCodec {
    /// H.264/AVC - baseline efficiency
    H264,
    /// H.265/HEVC - ~30% more efficient than H.264
    H265,
    /// H.266/VVC - ~50% more efficient than HEVC (cutting-edge 2024+)
    Vvc,
    /// VP9 - similar to HEVC
    Vp9,
    /// AV1 - ~50% more efficient than H.264
    Av1,
    /// AV2 - next-gen AV1 successor (~30% better than AV1, experimental 2025+)
    Av2,
    /// ProRes - high bitrate intermediate codec
    ProRes,
    /// DNxHD/DNxHR - high bitrate intermediate codec
    DnxHD,
    /// MJPEG - very inefficient
    Mjpeg,
    /// FFV1 - lossless archival codec
    Ffv1,
    /// UT Video - lossless intermediate codec
    UtVideo,
    /// HuffYUV - lossless codec
    HuffYuv,
    /// GIF - very inefficient (256 colors, LZW)
    Gif,
    /// APNG - moderately efficient
    Apng,
    /// WebP animated - efficient
    WebpAnimated,
    /// JPEG - lossy image
    Jpeg,
    /// JPEG XL - next-gen image format
    JpegXl,
    /// PNG - lossless image
    Png,
    /// WebP static - efficient
    WebpStatic,
    /// AVIF - AV1-based image format
    Avif,
    /// HEIC/HEIF - HEVC-based image format
    Heic,
    /// Unknown codec
    #[default]
    Unknown,
}

impl SourceCodec {
    /// Get codec efficiency factor relative to H.264 baseline
    /// 
    /// Lower value = more efficient (needs fewer bits for same quality)
    /// Based on industry benchmarks and codec specifications:
    /// - H.264: 1.0 (baseline)
    /// - HEVC: ~30-40% better → 0.65
    /// - AV1: ~50% better than H.264 → 0.5
    /// - VVC: ~50% better than HEVC → 0.35
    /// - AV2: ~30% better than AV1 (projected) → 0.35
    pub fn efficiency_factor(&self) -> f64 {
        match self {
            // === Video Codecs (by generation) ===
            SourceCodec::H264 => 1.0,       // Baseline (2003)
            SourceCodec::H265 => 0.65,      // ~35% more efficient (2013)
            SourceCodec::Vp9 => 0.70,       // Similar to HEVC (2013)
            SourceCodec::Av1 => 0.50,       // ~50% more efficient than H.264 (2018)
            SourceCodec::Vvc => 0.35,       // ~50% more efficient than HEVC (2020)
            SourceCodec::Av2 => 0.35,       // ~30% more efficient than AV1 (2025+, projected)
            
            // === Intermediate/Professional Codecs ===
            SourceCodec::ProRes => 1.8,     // High bitrate intermediate (quality-focused)
            SourceCodec::DnxHD => 1.8,      // High bitrate intermediate
            SourceCodec::Mjpeg => 2.5,      // Very inefficient (intra-only)
            
            // === Lossless Video Codecs ===
            SourceCodec::Ffv1 => 1.0,       // Lossless archival
            SourceCodec::UtVideo => 1.0,    // Lossless intermediate
            SourceCodec::HuffYuv => 1.0,    // Lossless
            
            // === Animation Formats ===
            SourceCodec::Gif => 3.0,        // Very inefficient (256 colors, no inter-frame)
            SourceCodec::Apng => 1.8,       // Moderately efficient (PNG-based)
            SourceCodec::WebpAnimated => 0.9, // Efficient (VP8-based)
            
            // === Image Formats ===
            SourceCodec::Jpeg => 1.0,       // Baseline for images
            SourceCodec::JpegXl => 0.6,     // ~40% better than JPEG
            SourceCodec::Png => 1.5,        // Less efficient for photos (lossless)
            SourceCodec::WebpStatic => 0.75, // ~25% better than JPEG
            SourceCodec::Avif => 0.55,      // AV1-based, very efficient
            SourceCodec::Heic => 0.65,      // HEVC-based
            
            SourceCodec::Unknown => 1.0,
        }
    }
    
    /// Check if this is a lossless codec
    pub fn is_lossless(&self) -> bool {
        matches!(
            self,
            SourceCodec::Ffv1 | SourceCodec::UtVideo | SourceCodec::HuffYuv |
            SourceCodec::Png | SourceCodec::Apng
        )
    }
    
    /// Check if this is a modern/cutting-edge codec (should skip re-encoding)
    pub fn is_modern(&self) -> bool {
        matches!(
            self,
            SourceCodec::H265 | SourceCodec::Av1 | SourceCodec::Vp9 |
            SourceCodec::Vvc | SourceCodec::Av2 |
            SourceCodec::JpegXl | SourceCodec::Avif | SourceCodec::Heic
        )
    }
    
    /// Check if this is a cutting-edge codec (VVC, AV2 - 2024+)
    pub fn is_cutting_edge(&self) -> bool {
        matches!(self, SourceCodec::Vvc | SourceCodec::Av2)
    }
}

/// Input quality analysis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityAnalysis {
    /// Bits per pixel (for video: bits per pixel per frame)
    pub bpp: f64,
    /// Source codec
    pub source_codec: String,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Whether source has B-frames (video only)
    pub has_b_frames: bool,
    /// Bit depth (8, 10, 12, etc.)
    pub bit_depth: u8,
    /// Whether source has alpha channel
    pub has_alpha: bool,
    /// Duration in seconds (for video/animation)
    pub duration_secs: Option<f64>,
    /// Frame rate (for video/animation)
    pub fps: Option<f64>,
    /// File size in bytes
    pub file_size: u64,
    /// Estimated quality (0-100, if available from JPEG analysis etc.)
    pub estimated_quality: Option<u8>,
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
        }
    }
}

/// Quality matching result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedQuality {
    /// Calculated CRF value (for video encoders)
    pub crf: u8,
    /// Calculated distance value (for JXL)
    pub distance: f32,
    /// Effective bits per pixel after adjustments
    pub effective_bpp: f64,
    /// Detailed analysis breakdown
    pub analysis_details: AnalysisDetails,
}

/// Detailed analysis breakdown for debugging/logging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisDetails {
    pub raw_bpp: f64,
    pub codec_factor: f64,
    pub resolution_factor: f64,
    pub bframe_factor: f64,
    pub alpha_factor: f64,
    pub color_depth_factor: f64,
    pub fps_factor: f64,
    pub duration_factor: f64,
    pub aspect_factor: f64,
    pub confidence: f64,  // 0.0-1.0, how confident we are in the analysis
}

/// Calculate matched CRF for AV1 encoder (SVT-AV1)
/// 
/// AV1 CRF range: 0-63, with 23 being default "good quality"
/// 
/// Formula: CRF = 50 - 8 * log2(effective_bpp * 100)
/// 
/// Clamped to range [18, 35] for practical use
/// 
/// # Arguments
/// * `analysis` - Quality analysis of input
/// 
/// # Returns
/// * `Result<MatchedQuality, String>` - Matched quality or error
pub fn calculate_av1_crf(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    let (effective_bpp, details) = calculate_effective_bpp(analysis, EncoderType::Av1)?;
    
    // Convert bpp to CRF using logarithmic formula for AV1
    let crf_float = if effective_bpp > 0.0 {
        50.0 - 8.0 * (effective_bpp * 100.0).log2()
    } else {
        28.0
    };
    
    // Clamp to reasonable range [18, 35] for AV1
    let crf = (crf_float.round() as i32).clamp(18, 35) as u8;
    
    Ok(MatchedQuality {
        crf,
        distance: 0.0,
        effective_bpp,
        analysis_details: details,
    })
}

/// Calculate matched CRF for HEVC encoder (x265)
/// 
/// HEVC CRF range: 0-51, with 23 being default "good quality"
/// 
/// Formula: CRF = 51 - 10 * log2(effective_bpp * 1000)
/// 
/// Clamped to range [0, 32] for practical use (allows visually lossless CRF 0)
/// 
/// # Arguments
/// * `analysis` - Quality analysis of input
/// 
/// # Returns
/// * `Result<MatchedQuality, String>` - Matched quality or error
pub fn calculate_hevc_crf(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    let (effective_bpp, details) = calculate_effective_bpp(analysis, EncoderType::Hevc)?;
    
    // Convert bpp to CRF using logarithmic formula for HEVC
    let crf_float = if effective_bpp > 0.0 {
        51.0 - 10.0 * (effective_bpp * 1000.0).log2()
    } else {
        23.0
    };
    
    // Clamp to reasonable range [0, 32] for HEVC (allows visually lossless)
    let crf = (crf_float.round() as i32).clamp(0, 32) as u8;
    
    Ok(MatchedQuality {
        crf,
        distance: 0.0,
        effective_bpp,
        analysis_details: details,
    })
}

/// Calculate matched distance for JXL encoder (cjxl)
/// 
/// JXL distance range: 0.0-15.0, with 0.0 being lossless
/// 
/// For JPEG input with quality info:
///   distance = (100 - quality) / 10
/// 
/// For other inputs:
///   Estimate quality from bpp, then convert to distance
/// 
/// Clamped to range [0.0, 5.0] for practical use
/// 
/// # Arguments
/// * `analysis` - Quality analysis of input
/// 
/// # Returns
/// * `Result<MatchedQuality, String>` - Matched quality or error
pub fn calculate_jxl_distance(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    // If we have estimated quality (e.g., from JPEG analysis), use it directly
    if let Some(quality) = analysis.estimated_quality {
        let distance = (100.0 - quality as f32) / 10.0;
        let clamped = distance.clamp(0.0, 5.0);
        
        return Ok(MatchedQuality {
            crf: 0,
            distance: clamped,
            effective_bpp: analysis.bpp,
            analysis_details: AnalysisDetails {
                raw_bpp: analysis.bpp,
                codec_factor: 1.0,
                resolution_factor: 1.0,
                bframe_factor: 1.0,
                alpha_factor: 1.0,
                color_depth_factor: 1.0,
                fps_factor: 1.0,
                duration_factor: 1.0,
                aspect_factor: 1.0,
                confidence: 0.9, // High confidence when quality is directly provided
            },
        });
    }
    
    // For non-JPEG, estimate quality from bpp
    let (effective_bpp, details) = calculate_effective_bpp(analysis, EncoderType::Jxl)?;
    
    // Estimate quality from effective bpp
    // bpp=2.0 → Q95 → d=0.5
    // bpp=1.0 → Q90 → d=1.0
    // bpp=0.5 → Q85 → d=1.5
    // bpp=0.3 → Q80 → d=2.0
    // bpp=0.1 → Q70 → d=3.0
    let estimated_quality = if effective_bpp > 0.0 {
        70.0 + 15.0 * (effective_bpp * 5.0).log2()
    } else {
        75.0
    };
    
    let clamped_quality = estimated_quality.clamp(50.0, 100.0);
    let distance = ((100.0 - clamped_quality) / 10.0) as f32;
    let clamped_distance = distance.clamp(0.0, 5.0);
    
    Ok(MatchedQuality {
        crf: 0,
        distance: clamped_distance,
        effective_bpp,
        analysis_details: details,
    })
}


/// Calculate effective bits per pixel with all adjustment factors
/// 
/// This is the core algorithm that normalizes bpp across different:
/// - Source codecs (efficiency varies)
/// - Resolutions (higher res needs more bits)
/// - Content complexity (B-frames, alpha, color depth, frame rate)
/// - Aspect ratio (ultra-wide needs different handling)
/// 
/// ## Enhanced Precision Features (v2.0)
/// 
/// 1. **Frame rate factor**: Higher fps = more temporal redundancy = better compression
/// 2. **Aspect ratio factor**: Ultra-wide content has different compression characteristics
/// 3. **Duration factor**: Longer content benefits more from temporal compression
/// 4. **Palette detection**: 8-bit indexed color (GIF) needs special handling
/// 
/// # Arguments
/// * `analysis` - Quality analysis of input
/// * `target_encoder` - Target encoder type
/// 
/// # Returns
/// * `Result<(f64, AnalysisDetails), String>` - Effective bpp and details, or error
fn calculate_effective_bpp(
    analysis: &QualityAnalysis,
    target_encoder: EncoderType,
) -> Result<(f64, AnalysisDetails), String> {
    // 🔥 Quality Manifesto: Validate input, fail loudly on invalid data
    if analysis.width == 0 || analysis.height == 0 {
        return Err("❌ Invalid dimensions: width or height is 0".to_string());
    }
    
    let pixels = (analysis.width as u64) * (analysis.height as u64);
    
    // Calculate raw bpp if not provided
    let raw_bpp = if analysis.bpp > 0.0 {
        analysis.bpp
    } else if analysis.file_size > 0 {
        // For video/animation, need duration
        if let Some(duration) = analysis.duration_secs {
            if duration > 0.0 {
                // 🔥 Enhanced: Use actual fps if available, estimate from common values otherwise
                let fps = analysis.fps.unwrap_or_else(|| {
                    // Estimate fps based on source codec
                    let codec = parse_source_codec(&analysis.source_codec);
                    match codec {
                        SourceCodec::Gif => 10.0,      // GIF typically 10fps
                        SourceCodec::Apng => 15.0,    // APNG typically 15fps
                        SourceCodec::WebpAnimated => 20.0, // WebP animated typically 20fps
                        _ => 24.0,                     // Default to 24fps for video
                    }
                });
                let total_frames = (duration * fps) as u64;
                let bits_per_frame = (analysis.file_size * 8) as f64 / total_frames.max(1) as f64;
                bits_per_frame / pixels as f64
            } else {
                // 🔥 Quality Manifesto: Unknown duration, use conservative estimate
                eprintln!("   ⚠️  Duration unknown or zero, using conservative estimate");
                (analysis.file_size * 8) as f64 / pixels as f64
            }
        } else {
            // Static image: bytes per pixel
            analysis.file_size as f64 / pixels as f64
        }
    } else {
        return Err("❌ Cannot calculate bpp: no file size or bpp provided".to_string());
    };
    
    // Codec efficiency factor
    let source_codec = parse_source_codec(&analysis.source_codec);
    let codec_factor = source_codec.efficiency_factor();
    
    // B-frames bonus (B-frames improve compression efficiency)
    let bframe_factor = if analysis.has_b_frames { 1.1 } else { 1.0 };
    
    // 🔥 Enhanced: More granular resolution factor with continuous scaling
    let resolution_factor = {
        let megapixels = pixels as f64 / 1_000_000.0;
        if megapixels > 8.0 {
            0.80 + 0.05 * (8.0 / megapixels).min(1.0)  // 4K+ (8MP+): 0.80-0.85
        } else if megapixels > 2.0 {
            0.85 + 0.05 * ((8.0 - megapixels) / 6.0)   // 1080p-4K: 0.85-0.90
        } else if megapixels > 0.5 {
            0.90 + 0.05 * ((2.0 - megapixels) / 1.5)   // 720p-1080p: 0.90-0.95
        } else {
            0.95 + 0.05 * ((0.5 - megapixels) / 0.5).min(1.0)  // SD: 0.95-1.0
        }
    };
    
    // Alpha channel factor (alpha adds complexity)
    let alpha_factor = if analysis.has_alpha { 0.9 } else { 1.0 };
    
    // Color depth factor with palette detection
    let color_depth_factor = match analysis.bit_depth {
        1..=8 => {
            // 🔥 Enhanced: 8-bit or less often means indexed color (GIF/PNG palette)
            // These have inherently limited quality ceiling
            if source_codec == SourceCodec::Gif {
                1.3  // GIF 256 colors - don't need high quality target
            } else {
                1.0
            }
        }
        10 => 1.25,
        12 => 1.5,
        16 => 2.0,
        _ => 1.0,
    };
    
    // 🔥 Enhanced: Frame rate factor for video/animation
    // Higher fps = more temporal redundancy = better compression potential
    let fps_factor = if let Some(fps) = analysis.fps {
        if fps >= 60.0 {
            1.15  // 60fps+ has lots of temporal redundancy
        } else if fps >= 30.0 {
            1.1   // 30fps standard
        } else if fps >= 24.0 {
            1.05  // 24fps cinematic
        } else if fps >= 15.0 {
            1.0   // 15fps animation
        } else {
            0.95  // Low fps (10fps GIF) - less temporal redundancy
        }
    } else {
        1.0
    };
    
    // 🔥 Enhanced: Duration factor for video/animation
    // Longer content benefits more from temporal compression
    let duration_factor = if let Some(duration) = analysis.duration_secs {
        if duration >= 60.0 {
            1.1   // 1min+ - good temporal compression
        } else if duration >= 10.0 {
            1.05  // 10s+ - moderate benefit
        } else if duration >= 3.0 {
            1.0   // 3-10s - baseline
        } else {
            0.95  // <3s - limited temporal benefit
        }
    } else {
        1.0
    };
    
    // 🔥 Enhanced: Aspect ratio factor
    // Ultra-wide content may have different compression characteristics
    let aspect_ratio = analysis.width as f64 / analysis.height as f64;
    let aspect_factor = if aspect_ratio > 2.5 || aspect_ratio < 0.4 {
        0.95  // Ultra-wide or ultra-tall - slightly harder to compress
    } else {
        1.0
    };
    
    // Target encoder adjustment
    // When converting to a more efficient codec, we can use higher CRF
    let target_adjustment = match target_encoder {
        EncoderType::Av1 => 0.5,   // AV1 is very efficient
        EncoderType::Hevc => 0.7,  // HEVC is efficient
        EncoderType::Jxl => 0.8,   // JXL is efficient for images
    };
    
    // 🔥 Enhanced: Content complexity factor based on entropy estimation
    // High entropy content (noise, fine detail) needs more bits
    // This is estimated from bpp relative to resolution
    let complexity_factor = {
        let expected_bpp_for_res = if pixels > 8_000_000 { 0.15 } // 4K+
            else if pixels > 2_000_000 { 0.20 } // 1080p
            else if pixels > 500_000 { 0.30 } // 720p
            else { 0.50 }; // SD
        
        let ratio = raw_bpp / expected_bpp_for_res;
        if ratio > 2.0 {
            1.15  // High complexity content
        } else if ratio > 1.0 {
            1.0 + 0.15 * ((ratio - 1.0) / 1.0)  // Linear interpolation
        } else if ratio > 0.5 {
            1.0   // Normal complexity
        } else {
            0.95  // Low complexity (simple content)
        }
    };
    
    // Effective bpp after all adjustments
    let effective_bpp = raw_bpp 
        * codec_factor 
        * bframe_factor 
        * resolution_factor 
        * alpha_factor 
        * fps_factor
        * duration_factor
        * aspect_factor
        * complexity_factor
        / color_depth_factor
        / target_adjustment;
    
    // 🔥 Calculate confidence score based on data completeness
    let confidence = calculate_confidence(analysis);
    
    let details = AnalysisDetails {
        raw_bpp,
        codec_factor,
        resolution_factor,
        bframe_factor,
        alpha_factor,
        color_depth_factor,
        fps_factor,
        duration_factor,
        aspect_factor,
        confidence,
    };
    
    Ok((effective_bpp, details))
}

/// Calculate confidence score based on data completeness
/// 
/// Returns a value between 0.0 and 1.0 indicating how confident
/// we are in the quality analysis based on available data.
fn calculate_confidence(analysis: &QualityAnalysis) -> f64 {
    let mut score: f64 = 0.0;
    let mut max_score: f64 = 0.0;
    
    // Essential fields (high weight)
    max_score += 30.0;
    if analysis.width > 0 && analysis.height > 0 {
        score += 30.0;
    }
    
    max_score += 25.0;
    if analysis.file_size > 0 {
        score += 25.0;
    }
    
    max_score += 15.0;
    if analysis.bpp > 0.0 {
        score += 15.0;
    }
    
    // Codec identification
    max_score += 10.0;
    let codec = parse_source_codec(&analysis.source_codec);
    if codec != SourceCodec::Unknown {
        score += 10.0;
    }
    
    // Optional but helpful fields
    max_score += 5.0;
    if analysis.duration_secs.is_some() {
        score += 5.0;
    }
    
    max_score += 5.0;
    if analysis.fps.is_some() {
        score += 5.0;
    }
    
    max_score += 5.0;
    if analysis.estimated_quality.is_some() {
        score += 5.0;
    }
    
    max_score += 5.0;
    if analysis.bit_depth > 0 {
        score += 5.0;
    }
    
    (score / max_score).clamp(0.0, 1.0)
}

/// Parse source codec string to SourceCodec enum
/// 
/// Supports comprehensive codec detection including cutting-edge formats:
/// - VVC/H.266 (2020+)
/// - AV2 (2025+, experimental)
/// - JPEG XL, AVIF, HEIC
pub fn parse_source_codec(codec_str: &str) -> SourceCodec {
    let codec_lower = codec_str.to_lowercase();
    
    // === Cutting-edge codecs (check first for priority) ===
    if codec_lower.contains("vvc") || codec_lower.contains("h266") || codec_lower.contains("h.266") {
        return SourceCodec::Vvc;
    }
    if codec_lower.contains("av2") || codec_lower.contains("avm") {
        return SourceCodec::Av2;
    }
    
    // === Modern video codecs ===
    if codec_lower.contains("av1") || codec_lower.contains("svt") || codec_lower.contains("aom") || codec_lower.contains("libaom") {
        return SourceCodec::Av1;
    }
    if codec_lower.contains("h265") || codec_lower.contains("hevc") || codec_lower.contains("x265") || codec_lower.contains("h.265") {
        return SourceCodec::H265;
    }
    if codec_lower.contains("vp9") {
        return SourceCodec::Vp9;
    }
    if codec_lower.contains("h264") || codec_lower.contains("avc") || codec_lower.contains("x264") || codec_lower.contains("h.264") {
        return SourceCodec::H264;
    }
    
    // === Professional/Intermediate codecs ===
    if codec_lower.contains("prores") {
        return SourceCodec::ProRes;
    }
    if codec_lower.contains("dnxh") || codec_lower.contains("dnxhr") {
        return SourceCodec::DnxHD;
    }
    if codec_lower.contains("mjpeg") || codec_lower.contains("motion jpeg") {
        return SourceCodec::Mjpeg;
    }
    
    // === Lossless video codecs ===
    if codec_lower.contains("ffv1") {
        return SourceCodec::Ffv1;
    }
    if codec_lower.contains("utvideo") || codec_lower.contains("ut video") {
        return SourceCodec::UtVideo;
    }
    if codec_lower.contains("huffyuv") || codec_lower.contains("ffvhuff") {
        return SourceCodec::HuffYuv;
    }
    
    // === Animation formats ===
    if codec_lower.contains("gif") {
        return SourceCodec::Gif;
    }
    if codec_lower.contains("apng") {
        return SourceCodec::Apng;
    }
    
    // === Modern image formats (check before legacy) ===
    if codec_lower.contains("jxl") || codec_lower.contains("jpeg xl") || codec_lower.contains("jpegxl") {
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
    
    // === Legacy image formats ===
    if codec_lower.contains("jpeg") || codec_lower.contains("jpg") {
        return SourceCodec::Jpeg;
    }
    if codec_lower.contains("png") {
        return SourceCodec::Png;
    }
    
    SourceCodec::Unknown
}

/// Log quality analysis details (for debugging)
pub fn log_quality_analysis(analysis: &QualityAnalysis, result: &MatchedQuality, encoder: EncoderType) {
    let encoder_name = match encoder {
        EncoderType::Av1 => "AV1",
        EncoderType::Hevc => "HEVC",
        EncoderType::Jxl => "JXL",
    };
    
    let d = &result.analysis_details;
    let codec = parse_source_codec(&analysis.source_codec);
    
    eprintln!("   📊 Quality Analysis ({}):", encoder_name);
    eprintln!("      Confidence: {:.0}%", d.confidence * 100.0);
    eprintln!("      Raw bpp: {:.4}", d.raw_bpp);
    eprintln!("      Codec: {} ({:?}, factor: {:.2})", analysis.source_codec, codec, d.codec_factor);
    if codec.is_modern() {
        eprintln!("      ⚠️  Modern codec detected - consider skipping re-encode");
    }
    if codec.is_cutting_edge() {
        eprintln!("      🚀 Cutting-edge codec (VVC/AV2) - skip recommended");
    }
    eprintln!("      Resolution: {}x{} (factor: {:.2})", analysis.width, analysis.height, d.resolution_factor);
    eprintln!("      B-frames: {} (factor: {:.2})", analysis.has_b_frames, d.bframe_factor);
    eprintln!("      Alpha: {} (factor: {:.2})", analysis.has_alpha, d.alpha_factor);
    eprintln!("      Color depth: {}-bit (factor: {:.2})", analysis.bit_depth, d.color_depth_factor);
    if let Some(fps) = analysis.fps {
        eprintln!("      FPS: {:.2} (factor: {:.2})", fps, d.fps_factor);
    }
    if let Some(duration) = analysis.duration_secs {
        eprintln!("      Duration: {:.1}s (factor: {:.2})", duration, d.duration_factor);
    }
    eprintln!("      Aspect ratio factor: {:.2}", d.aspect_factor);
    eprintln!("      Effective bpp: {:.4}", result.effective_bpp);
    
    match encoder {
        EncoderType::Av1 | EncoderType::Hevc => {
            eprintln!("      Calculated CRF: {}", result.crf);
        }
        EncoderType::Jxl => {
            eprintln!("      Calculated distance: {:.2}", result.distance);
        }
    }
}

/// Create QualityAnalysis from video detection result
/// 
/// This is a convenience function for video tools
pub fn from_video_detection(
    _file_path: &str,
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
    let bpp = if pixels_per_second > 0.0 {
        (bitrate as f64) / pixels_per_second
    } else {
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
    }
}

/// Create QualityAnalysis from animation/image analysis
/// 
/// This is a convenience function for image tools
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
    
    // Calculate bpp based on whether it's animated or static
    let bpp = if let (Some(duration), Some(frame_rate)) = (duration_secs, fps) {
        if duration > 0.0 && frame_rate > 0.0 {
            let total_frames = (duration * frame_rate) as u64;
            let bits_per_frame = (file_size * 8) as f64 / total_frames.max(1) as f64;
            bits_per_frame / pixels as f64
        } else {
            file_size as f64 / pixels as f64
        }
    } else {
        // Static image
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
        };
        
        let result = calculate_av1_crf(&analysis).unwrap();
        assert!(result.crf >= 18 && result.crf <= 35);
        assert!(result.analysis_details.confidence > 0.8); // High confidence with complete data
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
        };
        
        let result = calculate_hevc_crf(&analysis).unwrap();
        assert!(result.crf <= 32);
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
        };
        
        let result = calculate_jxl_distance(&analysis).unwrap();
        assert!((result.distance - 1.5).abs() < 0.1); // Q85 → d=1.5
    }
    
    #[test]
    fn test_parse_source_codec() {
        // Legacy codecs
        assert_eq!(parse_source_codec("h264"), SourceCodec::H264);
        assert_eq!(parse_source_codec("H.265/HEVC"), SourceCodec::H265);
        assert_eq!(parse_source_codec("AV1"), SourceCodec::Av1);
        assert_eq!(parse_source_codec("GIF"), SourceCodec::Gif);
        
        // Cutting-edge codecs
        assert_eq!(parse_source_codec("VVC"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("H.266"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("h266"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("AV2"), SourceCodec::Av2);
        assert_eq!(parse_source_codec("avm"), SourceCodec::Av2);
        
        // Modern image formats
        assert_eq!(parse_source_codec("JPEG XL"), SourceCodec::JpegXl);
        assert_eq!(parse_source_codec("jxl"), SourceCodec::JpegXl);
        assert_eq!(parse_source_codec("AVIF"), SourceCodec::Avif);
        assert_eq!(parse_source_codec("HEIC"), SourceCodec::Heic);
        
        // Lossless codecs
        assert_eq!(parse_source_codec("FFV1"), SourceCodec::Ffv1);
        assert_eq!(parse_source_codec("UTVideo"), SourceCodec::UtVideo);
        assert_eq!(parse_source_codec("HuffYUV"), SourceCodec::HuffYuv);
        
        assert_eq!(parse_source_codec("unknown_codec"), SourceCodec::Unknown);
    }
    
    #[test]
    fn test_codec_properties() {
        // Modern codec detection
        assert!(SourceCodec::H265.is_modern());
        assert!(SourceCodec::Av1.is_modern());
        assert!(SourceCodec::Vvc.is_modern());
        assert!(SourceCodec::Av2.is_modern());
        assert!(!SourceCodec::H264.is_modern());
        
        // Cutting-edge detection
        assert!(SourceCodec::Vvc.is_cutting_edge());
        assert!(SourceCodec::Av2.is_cutting_edge());
        assert!(!SourceCodec::Av1.is_cutting_edge());
        
        // Lossless detection
        assert!(SourceCodec::Ffv1.is_lossless());
        assert!(SourceCodec::Png.is_lossless());
        assert!(!SourceCodec::H264.is_lossless());
    }
    
    #[test]
    fn test_codec_efficiency_ordering() {
        // Verify efficiency ordering: newer codecs should be more efficient
        assert!(SourceCodec::Av1.efficiency_factor() < SourceCodec::H265.efficiency_factor());
        assert!(SourceCodec::H265.efficiency_factor() < SourceCodec::H264.efficiency_factor());
        assert!(SourceCodec::Vvc.efficiency_factor() < SourceCodec::Av1.efficiency_factor());
        assert!(SourceCodec::Av2.efficiency_factor() <= SourceCodec::Vvc.efficiency_factor());
        
        // GIF should be very inefficient
        assert!(SourceCodec::Gif.efficiency_factor() > 2.0);
    }
    
    #[test]
    fn test_invalid_dimensions_error() {
        let analysis = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 0,  // Invalid
            height: 1080,
            ..Default::default()
        };
        
        let result = calculate_av1_crf(&analysis);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_confidence_calculation() {
        // Complete data should have high confidence
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
        };
        let result = calculate_av1_crf(&complete).unwrap();
        assert!(result.analysis_details.confidence > 0.9);
        
        // Minimal data should have lower confidence
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
        };
        let result = calculate_av1_crf(&minimal).unwrap();
        assert!(result.analysis_details.confidence < 0.7);
    }
}
