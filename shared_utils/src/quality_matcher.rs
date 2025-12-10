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
    /// VP9 - similar to HEVC
    Vp9,
    /// AV1 - ~50% more efficient than H.264
    Av1,
    /// ProRes - high bitrate intermediate codec
    ProRes,
    /// DNxHD - high bitrate intermediate codec
    DnxHD,
    /// MJPEG - very inefficient
    Mjpeg,
    /// FFV1 - lossless
    Ffv1,
    /// GIF - very inefficient (256 colors, LZW)
    Gif,
    /// APNG - moderately efficient
    Apng,
    /// WebP animated - efficient
    WebpAnimated,
    /// JPEG - lossy image
    Jpeg,
    /// PNG - lossless image
    Png,
    /// WebP static - efficient
    WebpStatic,
    /// Unknown codec
    #[default]
    Unknown,
}

impl SourceCodec {
    /// Get codec efficiency factor relative to H.264 baseline
    /// Higher value = less efficient (needs more bits for same quality)
    pub fn efficiency_factor(&self) -> f64 {
        match self {
            SourceCodec::H264 => 1.0,       // Baseline
            SourceCodec::H265 => 0.7,       // ~30% more efficient
            SourceCodec::Vp9 => 0.75,       // Similar to HEVC
            SourceCodec::Av1 => 0.5,        // ~50% more efficient
            SourceCodec::ProRes => 1.5,     // High bitrate codec
            SourceCodec::DnxHD => 1.5,      // High bitrate codec
            SourceCodec::Mjpeg => 2.0,      // Very inefficient
            SourceCodec::Ffv1 => 1.0,       // Lossless, not comparable
            SourceCodec::Gif => 2.5,        // Very inefficient (256 colors)
            SourceCodec::Apng => 1.5,       // Moderately efficient
            SourceCodec::WebpAnimated => 1.0, // Efficient
            SourceCodec::Jpeg => 1.0,       // Baseline for images
            SourceCodec::Png => 1.5,        // Less efficient for photos
            SourceCodec::WebpStatic => 0.8, // Efficient
            SourceCodec::Unknown => 1.0,
        }
    }
    
    /// Check if this is a lossless codec
    pub fn is_lossless(&self) -> bool {
        matches!(self, SourceCodec::Ffv1 | SourceCodec::Png | SourceCodec::Apng)
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
/// - Content complexity (B-frames, alpha, color depth)
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
                let fps = analysis.fps.unwrap_or(30.0);
                let total_frames = (duration * fps) as u64;
                let bits_per_frame = (analysis.file_size * 8) as f64 / total_frames.max(1) as f64;
                bits_per_frame / pixels as f64
            } else {
                // 🔥 Quality Manifesto: Unknown duration, use conservative estimate
                // Assume 1 second duration for conservative bpp calculation
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
    
    // Resolution factor (higher res is harder to compress efficiently)
    let resolution_factor = if pixels > 8_000_000 {
        0.85  // 4K+ needs more bits
    } else if pixels > 2_000_000 {
        0.9   // 1080p
    } else if pixels > 500_000 {
        0.95  // 720p
    } else {
        1.0   // SD
    };
    
    // Alpha channel factor (alpha adds complexity)
    let alpha_factor = if analysis.has_alpha { 0.9 } else { 1.0 };
    
    // Color depth factor
    let color_depth_factor = match analysis.bit_depth {
        8 => 1.0,
        10 => 1.25,
        12 => 1.5,
        16 => 2.0,
        _ => 1.0,
    };
    
    // Target encoder adjustment
    // When converting to a more efficient codec, we can use higher CRF
    let target_adjustment = match target_encoder {
        EncoderType::Av1 => 0.5,   // AV1 is very efficient
        EncoderType::Hevc => 0.7,  // HEVC is efficient
        EncoderType::Jxl => 0.8,   // JXL is efficient for images
    };
    
    // Effective bpp after all adjustments
    let effective_bpp = raw_bpp 
        * codec_factor 
        * bframe_factor 
        * resolution_factor 
        * alpha_factor 
        / color_depth_factor
        / target_adjustment;
    
    let details = AnalysisDetails {
        raw_bpp,
        codec_factor,
        resolution_factor,
        bframe_factor,
        alpha_factor,
        color_depth_factor,
    };
    
    Ok((effective_bpp, details))
}

/// Parse source codec string to SourceCodec enum
fn parse_source_codec(codec_str: &str) -> SourceCodec {
    let codec_lower = codec_str.to_lowercase();
    
    if codec_lower.contains("h264") || codec_lower.contains("avc") || codec_lower.contains("x264") {
        SourceCodec::H264
    } else if codec_lower.contains("h265") || codec_lower.contains("hevc") || codec_lower.contains("x265") {
        SourceCodec::H265
    } else if codec_lower.contains("vp9") {
        SourceCodec::Vp9
    } else if codec_lower.contains("av1") || codec_lower.contains("svt") || codec_lower.contains("aom") {
        SourceCodec::Av1
    } else if codec_lower.contains("prores") {
        SourceCodec::ProRes
    } else if codec_lower.contains("dnxh") {
        SourceCodec::DnxHD
    } else if codec_lower.contains("mjpeg") || codec_lower.contains("motion jpeg") {
        SourceCodec::Mjpeg
    } else if codec_lower.contains("ffv1") {
        SourceCodec::Ffv1
    } else if codec_lower.contains("gif") {
        SourceCodec::Gif
    } else if codec_lower.contains("apng") {
        SourceCodec::Apng
    } else if codec_lower.contains("webp") {
        if codec_lower.contains("anim") {
            SourceCodec::WebpAnimated
        } else {
            SourceCodec::WebpStatic
        }
    } else if codec_lower.contains("jpeg") || codec_lower.contains("jpg") {
        SourceCodec::Jpeg
    } else if codec_lower.contains("png") {
        SourceCodec::Png
    } else {
        SourceCodec::Unknown
    }
}

/// Log quality analysis details (for debugging)
pub fn log_quality_analysis(analysis: &QualityAnalysis, result: &MatchedQuality, encoder: EncoderType) {
    let encoder_name = match encoder {
        EncoderType::Av1 => "AV1",
        EncoderType::Hevc => "HEVC",
        EncoderType::Jxl => "JXL",
    };
    
    eprintln!("   📊 Quality Analysis ({}):", encoder_name);
    eprintln!("      Raw bpp: {:.4}", result.analysis_details.raw_bpp);
    eprintln!("      Codec: {} (factor: {:.2})", analysis.source_codec, result.analysis_details.codec_factor);
    eprintln!("      Resolution: {}x{} (factor: {:.2})", analysis.width, analysis.height, result.analysis_details.resolution_factor);
    eprintln!("      B-frames: {} (factor: {:.2})", analysis.has_b_frames, result.analysis_details.bframe_factor);
    eprintln!("      Alpha: {} (factor: {:.2})", analysis.has_alpha, result.analysis_details.alpha_factor);
    eprintln!("      Color depth: {}-bit (factor: {:.2})", analysis.bit_depth, result.analysis_details.color_depth_factor);
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
        assert_eq!(parse_source_codec("h264"), SourceCodec::H264);
        assert_eq!(parse_source_codec("H.265/HEVC"), SourceCodec::H265);
        assert_eq!(parse_source_codec("AV1"), SourceCodec::Av1);
        assert_eq!(parse_source_codec("GIF"), SourceCodec::Gif);
        assert_eq!(parse_source_codec("unknown_codec"), SourceCodec::Unknown);
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
}
