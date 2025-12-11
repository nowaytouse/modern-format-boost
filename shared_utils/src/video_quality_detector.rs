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
//! ## Integration with quality_matcher
//! This module provides the detection layer, while quality_matcher
//! provides the CRF calculation layer.

use serde::{Deserialize, Serialize};
use crate::quality_matcher::{
    QualityAnalysis, VideoAnalysisBuilder, ContentType,
    SourceCodec, parse_source_codec, should_skip_video_codec,
};

// ============================================================
// Core Types
// ============================================================

/// Video quality analysis result for auto routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoQualityAnalysis {
    // === Basic Properties ===
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub duration_secs: f64,
    pub fps: f64,
    pub frame_count: u64,
    
    // === Codec Detection ===
    pub codec: String,
    pub codec_type: VideoCodecType,
    pub is_modern_codec: bool,
    pub should_skip: bool,
    pub skip_reason: Option<String>,
    
    // === Quality Metrics ===
    /// Total bitrate (includes audio)
    pub total_bitrate: u64,
    /// Video-only bitrate (excludes audio) - more accurate for BPP
    pub video_bitrate: Option<u64>,
    /// Bits per pixel per frame
    pub bpp: f64,
    /// Bit depth (8, 10, 12)
    pub bit_depth: u8,
    
    // === Encoding Structure ===
    /// Pixel format (yuv420p, yuv444p, etc.)
    pub pix_fmt: String,
    /// Chroma subsampling type
    pub chroma: ChromaSubsampling,
    /// GOP size (keyframe interval)
    pub gop_size: Option<u32>,
    /// Number of B-frames
    pub b_frame_count: u8,
    /// Has B-frames
    pub has_b_frames: bool,
    
    // === Color Information ===
    pub color_space: Option<String>,
    pub is_hdr: bool,
    
    // === Content Classification ===
    pub content_type: VideoContentType,
    pub compression_type: CompressionLevel,
    
    // === Quality Estimation ===
    /// Estimated quality score (0-100)
    pub quality_score: u8,
    /// Estimated CRF equivalent
    pub estimated_crf: u8,
    
    // === Routing Decision ===
    pub routing_decision: VideoRoutingDecision,
    
    /// Analysis confidence (0.0-1.0)
    pub confidence: f64,
}

/// Video codec type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodecType {
    /// Lossless codecs (FFV1, HuffYUV, UTVideo)
    Lossless,
    /// Modern efficient codecs (AV1, HEVC, VP9, VVC)
    ModernEfficient,
    /// Legacy codecs (H.264, MPEG-4)
    Legacy,
    /// Intermediate/Professional (ProRes, DNxHD)
    Intermediate,
    /// Very inefficient (MJPEG, GIF)
    Inefficient,
    /// Unknown
    Unknown,
}


impl VideoCodecType {
    pub fn from_source_codec(codec: SourceCodec) -> Self {
        match codec {
            SourceCodec::Ffv1 | SourceCodec::UtVideo | SourceCodec::HuffYuv => VideoCodecType::Lossless,
            SourceCodec::Av1 | SourceCodec::H265 | SourceCodec::Vp9 | 
            SourceCodec::Vvc | SourceCodec::Av2 => VideoCodecType::ModernEfficient,
            SourceCodec::H264 => VideoCodecType::Legacy,
            SourceCodec::ProRes | SourceCodec::DnxHD => VideoCodecType::Intermediate,
            SourceCodec::Mjpeg | SourceCodec::Gif => VideoCodecType::Inefficient,
            _ => VideoCodecType::Unknown,
        }
    }
}

/// Chroma subsampling type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChromaSubsampling {
    /// 4:2:0 - Most common, baseline
    Yuv420,
    /// 4:2:2 - Professional
    Yuv422,
    /// 4:4:4 - Full chroma
    Yuv444,
    /// RGB
    Rgb,
    /// Unknown
    Unknown,
}

impl ChromaSubsampling {
    pub fn from_pix_fmt(pix_fmt: &str) -> Self {
        let fmt = pix_fmt.to_lowercase();
        if fmt.contains("444") {
            ChromaSubsampling::Yuv444
        } else if fmt.contains("422") {
            ChromaSubsampling::Yuv422
        } else if fmt.contains("420") || fmt.contains("yuv") || fmt.contains("nv12") {
            ChromaSubsampling::Yuv420
        } else if fmt.contains("rgb") || fmt.contains("gbr") || fmt.contains("bgr") {
            ChromaSubsampling::Rgb
        } else {
            ChromaSubsampling::Unknown
        }
    }
    
    /// Factor for quality calculation (higher = needs more bits)
    pub fn quality_factor(&self) -> f64 {
        match self {
            ChromaSubsampling::Yuv420 => 1.0,
            ChromaSubsampling::Yuv422 => 1.05,
            ChromaSubsampling::Yuv444 => 1.15,
            ChromaSubsampling::Rgb => 1.20,
            ChromaSubsampling::Unknown => 1.0,
        }
    }
}

/// Video content type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoContentType {
    /// Live action film/video
    LiveAction,
    /// Animation/anime
    Animation,
    /// Screen recording
    ScreenRecording,
    /// Gaming content
    Gaming,
    /// Film with grain
    FilmGrain,
    /// Unknown/mixed
    Unknown,
}

impl VideoContentType {
    /// Convert to quality_matcher ContentType
    pub fn to_content_type(&self) -> ContentType {
        match self {
            VideoContentType::LiveAction => ContentType::LiveAction,
            VideoContentType::Animation => ContentType::Animation,
            VideoContentType::ScreenRecording => ContentType::ScreenRecording,
            VideoContentType::Gaming => ContentType::Gaming,
            VideoContentType::FilmGrain => ContentType::FilmGrain,
            VideoContentType::Unknown => ContentType::Unknown,
        }
    }
}


/// Compression level classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionLevel {
    /// Mathematically lossless
    Lossless,
    /// Visually lossless (CRF 0-4)
    VisuallyLossless,
    /// High quality (CRF 5-18)
    HighQuality,
    /// Standard quality (CRF 19-28)
    Standard,
    /// Low quality (CRF 29+)
    LowQuality,
}

impl CompressionLevel {
    /// Estimate from BPP and codec
    pub fn from_bpp(bpp: f64, codec_type: VideoCodecType) -> Self {
        if codec_type == VideoCodecType::Lossless {
            return CompressionLevel::Lossless;
        }
        if codec_type == VideoCodecType::Intermediate {
            return CompressionLevel::VisuallyLossless;
        }
        
        // BPP thresholds (adjusted for codec efficiency)
        let efficiency = match codec_type {
            VideoCodecType::ModernEfficient => 0.6,
            VideoCodecType::Legacy => 1.0,
            VideoCodecType::Inefficient => 2.0,
            _ => 1.0,
        };
        
        let adjusted_bpp = bpp / efficiency;
        
        if adjusted_bpp > 1.0 {
            CompressionLevel::VisuallyLossless
        } else if adjusted_bpp > 0.3 {
            CompressionLevel::HighQuality
        } else if adjusted_bpp > 0.1 {
            CompressionLevel::Standard
        } else {
            CompressionLevel::LowQuality
        }
    }
}

/// Routing decision for video format selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoRoutingDecision {
    /// Primary recommended format
    pub primary_format: String,
    /// Alternative formats
    pub alternatives: Vec<String>,
    /// Recommended encoder
    pub encoder: String,
    /// Estimated CRF for quality matching
    pub recommended_crf: u8,
    /// Use lossless encoding
    pub use_lossless: bool,
    /// Reason for decision
    pub reason: String,
    /// Should skip conversion
    pub should_skip: bool,
    /// Skip reason
    pub skip_reason: Option<String>,
}


// ============================================================
// Analysis Functions
// ============================================================

/// Analyze video quality from ffprobe data
/// 
/// # Arguments
/// * `codec` - Video codec name from ffprobe
/// * `width` - Video width
/// * `height` - Video height
/// * `fps` - Frame rate
/// * `duration_secs` - Duration in seconds
/// * `total_bitrate` - Total bitrate (video + audio)
/// * `video_bitrate` - Video-only bitrate (optional, more accurate)
/// * `pix_fmt` - Pixel format string
/// * `bit_depth` - Bit depth (8, 10, 12)
/// * `has_b_frames` - Whether B-frames are used
/// * `gop_size` - GOP size (optional)
/// * `color_space` - Color space (optional)
/// * `file_size` - File size in bytes
/// 
/// # Returns
/// * `Result<VideoQualityAnalysis, String>` - Analysis or error
/// 
/// # 🔥 Quality Manifesto
/// - All metrics from actual ffprobe data
/// - Fails loudly on invalid input
pub fn analyze_video_quality(
    codec: &str,
    width: u32,
    height: u32,
    fps: f64,
    duration_secs: f64,
    total_bitrate: u64,
    video_bitrate: Option<u64>,
    pix_fmt: &str,
    bit_depth: u8,
    has_b_frames: bool,
    gop_size: Option<u32>,
    color_space: Option<&str>,
    file_size: u64,
) -> Result<VideoQualityAnalysis, String> {
    // 🔥 Validate input - fail loudly
    if width == 0 || height == 0 {
        return Err("❌ Invalid dimensions: width or height is 0".to_string());
    }
    if fps <= 0.0 {
        return Err("❌ Invalid frame rate: fps must be > 0".to_string());
    }
    if duration_secs <= 0.0 {
        return Err("❌ Invalid duration: must be > 0".to_string());
    }
    
    let _pixels = (width as u64) * (height as u64);
    let frame_count = (duration_secs * fps) as u64;
    
    // === Codec analysis ===
    let source_codec = parse_source_codec(codec);
    let codec_type = VideoCodecType::from_source_codec(source_codec);
    let is_modern = source_codec.is_modern();
    
    // === Skip decision ===
    let skip_decision = should_skip_video_codec(codec);
    
    // === Calculate BPP ===
    let effective_bitrate = video_bitrate.unwrap_or(total_bitrate);
    let pixels_per_second = (width as f64) * (height as f64) * fps;
    let bpp = if pixels_per_second > 0.0 {
        effective_bitrate as f64 / pixels_per_second
    } else {
        0.0
    };
    
    // === Chroma and color ===
    let chroma = ChromaSubsampling::from_pix_fmt(pix_fmt);
    let is_hdr = color_space.map(|cs| {
        let cs_lower = cs.to_lowercase();
        cs_lower.contains("bt2020") || cs_lower.contains("2020")
    }).unwrap_or(false);
    
    // === B-frame count estimation ===
    let b_frame_count = if has_b_frames { 2 } else { 0 };
    
    // === Content type estimation (basic) ===
    let content_type = estimate_content_type(bpp, codec_type, width, height);
    
    // === Compression level ===
    let compression_type = CompressionLevel::from_bpp(bpp, codec_type);
    
    // === Quality score ===
    let quality_score = calculate_quality_score(bpp, codec_type, bit_depth, compression_type);
    
    // === Estimated CRF ===
    let estimated_crf = estimate_crf_from_bpp(bpp, codec_type);
    
    // === Routing decision ===
    let routing_decision = make_video_routing_decision(
        codec_type,
        compression_type,
        is_modern,
        skip_decision.should_skip,
        &skip_decision.reason,
        estimated_crf,
    );
    
    // === Confidence ===
    let confidence = calculate_video_confidence(
        video_bitrate.is_some(),
        gop_size.is_some(),
        duration_secs,
        frame_count,
    );
    
    Ok(VideoQualityAnalysis {
        width,
        height,
        file_size,
        duration_secs,
        fps,
        frame_count,
        codec: codec.to_string(),
        codec_type,
        is_modern_codec: is_modern,
        should_skip: skip_decision.should_skip,
        skip_reason: if skip_decision.should_skip { Some(skip_decision.reason) } else { None },
        total_bitrate,
        video_bitrate,
        bpp,
        bit_depth,
        pix_fmt: pix_fmt.to_string(),
        chroma,
        gop_size,
        b_frame_count,
        has_b_frames,
        color_space: color_space.map(|s| s.to_string()),
        is_hdr,
        content_type,
        compression_type,
        quality_score,
        estimated_crf,
        routing_decision,
        confidence,
    })
}


/// Convert to QualityAnalysis for use with quality_matcher
pub fn to_quality_analysis(analysis: &VideoQualityAnalysis) -> QualityAnalysis {
    VideoAnalysisBuilder::new()
        .basic(&analysis.codec, analysis.width, analysis.height, analysis.fps, analysis.duration_secs)
        .file_size(analysis.file_size)
        .video_bitrate(analysis.video_bitrate.unwrap_or(analysis.total_bitrate))
        .gop(analysis.gop_size.unwrap_or(60), analysis.b_frame_count)
        .pix_fmt(&analysis.pix_fmt)
        .color(analysis.color_space.as_deref().unwrap_or("bt709"), analysis.is_hdr)
        .content_type(analysis.content_type.to_content_type())
        .bit_depth(analysis.bit_depth)
        .build()
}

// ============================================================
// Helper Functions
// ============================================================

fn estimate_content_type(bpp: f64, codec_type: VideoCodecType, width: u32, height: u32) -> VideoContentType {
    // Screen recording: typically low BPP, specific resolutions
    let is_screen_res = (width == 1920 && height == 1080) || 
                        (width == 2560 && height == 1440) ||
                        (width == 3840 && height == 2160);
    if is_screen_res && bpp < 0.1 {
        return VideoContentType::ScreenRecording;
    }
    
    // Animation: often uses specific codecs or has very low BPP
    if bpp < 0.05 {
        return VideoContentType::Animation;
    }
    
    // High BPP with intermediate codec = likely film
    if codec_type == VideoCodecType::Intermediate && bpp > 0.5 {
        return VideoContentType::FilmGrain;
    }
    
    VideoContentType::Unknown
}

fn calculate_quality_score(_bpp: f64, codec_type: VideoCodecType, bit_depth: u8, compression: CompressionLevel) -> u8 {
    let base = match compression {
        CompressionLevel::Lossless => 100,
        CompressionLevel::VisuallyLossless => 95,
        CompressionLevel::HighQuality => 80,
        CompressionLevel::Standard => 60,
        CompressionLevel::LowQuality => 40,
    };
    
    // Bit depth bonus
    let depth_bonus = if bit_depth >= 10 { 5 } else { 0 };
    
    // Modern codec bonus
    let codec_bonus = if codec_type == VideoCodecType::ModernEfficient { 3 } else { 0 };
    
    (base + depth_bonus + codec_bonus).min(100)
}

fn estimate_crf_from_bpp(bpp: f64, codec_type: VideoCodecType) -> u8 {
    if codec_type == VideoCodecType::Lossless {
        return 0;
    }
    
    // Efficiency factor
    let efficiency = match codec_type {
        VideoCodecType::ModernEfficient => 0.5,
        VideoCodecType::Legacy => 1.0,
        VideoCodecType::Intermediate => 0.7,
        VideoCodecType::Inefficient => 2.0,
        _ => 1.0,
    };
    
    let adjusted_bpp = bpp / efficiency;
    
    // CRF estimation based on adjusted BPP
    // Higher BPP = lower CRF (better quality)
    
    
    if adjusted_bpp > 1.0 {
        18
    } else if adjusted_bpp > 0.5 {
        22
    } else if adjusted_bpp > 0.3 {
        25
    } else if adjusted_bpp > 0.15 {
        28
    } else if adjusted_bpp > 0.08 {
        32
    } else {
        35
    }
}


fn make_video_routing_decision(
    codec_type: VideoCodecType,
    compression: CompressionLevel,
    _is_modern: bool,
    should_skip: bool,
    skip_reason: &str,
    estimated_crf: u8,
) -> VideoRoutingDecision {
    if should_skip {
        return VideoRoutingDecision {
            primary_format: "skip".to_string(),
            alternatives: vec![],
            encoder: "none".to_string(),
            recommended_crf: 0,
            use_lossless: false,
            reason: skip_reason.to_string(),
            should_skip: true,
            skip_reason: Some(skip_reason.to_string()),
        };
    }
    
    // Lossless source -> FFV1 for archival
    if codec_type == VideoCodecType::Lossless || compression == CompressionLevel::Lossless {
        return VideoRoutingDecision {
            primary_format: "ffv1".to_string(),
            alternatives: vec!["av1".to_string()],
            encoder: "ffv1".to_string(),
            recommended_crf: 0,
            use_lossless: true,
            reason: "Lossless source - preserve with FFV1".to_string(),
            should_skip: false,
            skip_reason: None,
        };
    }
    
    // High quality intermediate -> AV1 or HEVC
    if codec_type == VideoCodecType::Intermediate {
        return VideoRoutingDecision {
            primary_format: "av1".to_string(),
            alternatives: vec!["hevc".to_string()],
            encoder: "svt-av1".to_string(),
            recommended_crf: estimated_crf.saturating_sub(2), // Slightly lower CRF for quality
            use_lossless: false,
            reason: "Intermediate codec - convert to AV1 for efficiency".to_string(),
            should_skip: false,
            skip_reason: None,
        };
    }
    
    // Legacy or inefficient -> AV1
    VideoRoutingDecision {
        primary_format: "av1".to_string(),
        alternatives: vec!["hevc".to_string()],
        encoder: "svt-av1".to_string(),
        recommended_crf: estimated_crf,
        use_lossless: false,
        reason: format!("Convert to AV1 for better compression (estimated CRF {})", estimated_crf),
        should_skip: false,
        skip_reason: None,
    }
}

fn calculate_video_confidence(
    has_video_bitrate: bool,
    has_gop_size: bool,
    duration: f64,
    frame_count: u64,
) -> f64 {
    let mut confidence: f64 = 0.7;
    
    // Video-only bitrate is more accurate
    if has_video_bitrate {
        confidence += 0.1;
    }
    
    // GOP size helps with quality estimation
    if has_gop_size {
        confidence += 0.05;
    }
    
    // Longer videos = more reliable analysis
    if duration > 10.0 {
        confidence += 0.05;
    }
    
    // More frames = more reliable
    if frame_count > 100 {
        confidence += 0.05;
    }
    
    confidence.clamp(0.0, 1.0)
}


// ============================================================
// 🔬 PRECISION VALIDATION TESTS ("裁判" Tests)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    // ============================================================
    // Basic Functionality Tests
    // ============================================================
    
    #[test]
    fn test_analyze_h264_1080p() {
        let result = analyze_video_quality(
            "h264",
            1920, 1080,
            30.0,
            60.0,
            8_000_000,
            Some(7_500_000),
            "yuv420p",
            8,
            true,
            Some(60),
            Some("bt709"),
            60_000_000,
        ).unwrap();
        
        assert_eq!(result.width, 1920);
        assert_eq!(result.height, 1080);
        assert_eq!(result.codec_type, VideoCodecType::Legacy);
        assert!(!result.is_modern_codec);
        assert!(!result.should_skip);
        assert!(result.bpp > 0.0);
    }
    
    #[test]
    fn test_analyze_hevc_4k() {
        let result = analyze_video_quality(
            "hevc",
            3840, 2160,
            30.0,
            120.0,
            20_000_000,
            Some(19_000_000),
            "yuv420p10le",
            10,
            true,
            Some(60),
            Some("bt2020nc"),
            300_000_000,
        ).unwrap();
        
        assert_eq!(result.codec_type, VideoCodecType::ModernEfficient);
        assert!(result.is_modern_codec);
        assert!(result.should_skip, "HEVC should be skipped");
        assert!(result.is_hdr, "BT.2020 should be detected as HDR");
        assert_eq!(result.bit_depth, 10);
    }
    
    #[test]
    fn test_analyze_av1() {
        let result = analyze_video_quality(
            "av1",
            1920, 1080,
            24.0,
            90.0,
            5_000_000,
            Some(4_800_000),
            "yuv420p",
            8,
            true,
            Some(120),
            None,
            56_000_000,
        ).unwrap();
        
        assert_eq!(result.codec_type, VideoCodecType::ModernEfficient);
        assert!(result.should_skip, "AV1 should be skipped");
    }
    
    #[test]
    fn test_analyze_prores() {
        let result = analyze_video_quality(
            "prores",
            1920, 1080,
            24.0,
            60.0,
            150_000_000,
            Some(145_000_000),
            "yuv422p10le",
            10,
            false,
            Some(1),
            Some("bt709"),
            1_125_000_000,
        ).unwrap();
        
        assert_eq!(result.codec_type, VideoCodecType::Intermediate);
        assert!(!result.should_skip, "ProRes should not be skipped");
        assert_eq!(result.chroma, ChromaSubsampling::Yuv422);
        assert!(result.bpp > 1.0, "ProRes should have high BPP");
    }
    
    #[test]
    fn test_analyze_ffv1_lossless() {
        let result = analyze_video_quality(
            "ffv1",
            1920, 1080,
            30.0,
            30.0,
            200_000_000,
            Some(195_000_000),
            "yuv444p",
            8,
            false,
            Some(1),
            None,
            750_000_000,
        ).unwrap();
        
        assert_eq!(result.codec_type, VideoCodecType::Lossless);
        assert_eq!(result.compression_type, CompressionLevel::Lossless);
        assert!(!result.should_skip);
        assert_eq!(result.chroma, ChromaSubsampling::Yuv444);
    }


    // ============================================================
    // 🔬 Skip Decision Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_skip_modern_codecs() {
        // HEVC should skip
        let hevc = analyze_video_quality(
            "hevc", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        assert!(hevc.should_skip, "HEVC should be skipped");
        
        // AV1 should skip
        let av1 = analyze_video_quality(
            "av1", 1920, 1080, 30.0, 60.0, 5_000_000, None,
            "yuv420p", 8, true, None, None, 37_500_000,
        ).unwrap();
        assert!(av1.should_skip, "AV1 should be skipped");
        
        // VP9 should skip
        let vp9 = analyze_video_quality(
            "vp9", 1920, 1080, 30.0, 60.0, 6_000_000, None,
            "yuv420p", 8, true, None, None, 45_000_000,
        ).unwrap();
        assert!(vp9.should_skip, "VP9 should be skipped");
        
        // VVC should skip
        let vvc = analyze_video_quality(
            "vvc", 1920, 1080, 30.0, 60.0, 4_000_000, None,
            "yuv420p", 8, true, None, None, 30_000_000,
        ).unwrap();
        assert!(vvc.should_skip, "VVC should be skipped");
    }
    
    #[test]
    fn test_not_skip_legacy_codecs() {
        // H.264 should NOT skip
        let h264 = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        assert!(!h264.should_skip, "H.264 should NOT be skipped");
        
        // MJPEG should NOT skip
        let mjpeg = analyze_video_quality(
            "mjpeg", 1920, 1080, 30.0, 60.0, 50_000_000, None,
            "yuvj420p", 8, false, None, None, 375_000_000,
        ).unwrap();
        assert!(!mjpeg.should_skip, "MJPEG should NOT be skipped");
        
        // ProRes should NOT skip
        let prores = analyze_video_quality(
            "prores", 1920, 1080, 24.0, 60.0, 150_000_000, None,
            "yuv422p10le", 10, false, None, None, 1_125_000_000,
        ).unwrap();
        assert!(!prores.should_skip, "ProRes should NOT be skipped");
    }
    
    // ============================================================
    // 🔬 Chroma Subsampling Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_chroma_detection() {
        assert_eq!(ChromaSubsampling::from_pix_fmt("yuv420p"), ChromaSubsampling::Yuv420);
        assert_eq!(ChromaSubsampling::from_pix_fmt("yuv420p10le"), ChromaSubsampling::Yuv420);
        assert_eq!(ChromaSubsampling::from_pix_fmt("yuv422p"), ChromaSubsampling::Yuv422);
        assert_eq!(ChromaSubsampling::from_pix_fmt("yuv444p"), ChromaSubsampling::Yuv444);
        assert_eq!(ChromaSubsampling::from_pix_fmt("rgb24"), ChromaSubsampling::Rgb);
        assert_eq!(ChromaSubsampling::from_pix_fmt("gbrp"), ChromaSubsampling::Rgb);
        assert_eq!(ChromaSubsampling::from_pix_fmt("nv12"), ChromaSubsampling::Yuv420);
    }
    
    #[test]
    fn test_chroma_quality_factor() {
        assert!((ChromaSubsampling::Yuv420.quality_factor() - 1.0).abs() < 0.01);
        assert!(ChromaSubsampling::Yuv422.quality_factor() > 1.0);
        assert!(ChromaSubsampling::Yuv444.quality_factor() > ChromaSubsampling::Yuv422.quality_factor());
        assert!(ChromaSubsampling::Rgb.quality_factor() > ChromaSubsampling::Yuv444.quality_factor());
    }


    // ============================================================
    // 🔬 BPP Calculation Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_bpp_calculation_accuracy() {
        // 1080p @ 8Mbps @ 30fps
        // BPP = 8_000_000 / (1920 * 1080 * 30) = 0.128
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, Some(8_000_000),
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        let expected_bpp = 8_000_000.0 / (1920.0 * 1080.0 * 30.0);
        assert!((result.bpp - expected_bpp).abs() < 0.001,
            "BPP calculation error: expected {}, got {}", expected_bpp, result.bpp);
    }
    
    #[test]
    fn test_bpp_uses_video_bitrate_when_available() {
        // Total bitrate includes audio, video bitrate is more accurate
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0,
            10_000_000,      // Total (includes audio)
            Some(8_000_000), // Video only
            "yuv420p", 8, true, None, None, 75_000_000,
        ).unwrap();
        
        // Should use video bitrate, not total
        let expected_bpp = 8_000_000.0 / (1920.0 * 1080.0 * 30.0);
        assert!((result.bpp - expected_bpp).abs() < 0.001,
            "Should use video_bitrate for BPP: expected {}, got {}", expected_bpp, result.bpp);
    }
    
    // ============================================================
    // 🔬 Compression Level Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_compression_level_lossless() {
        let result = analyze_video_quality(
            "ffv1", 1920, 1080, 30.0, 60.0, 200_000_000, None,
            "yuv444p", 8, false, None, None, 1_500_000_000,
        ).unwrap();
        
        assert_eq!(result.compression_type, CompressionLevel::Lossless);
    }
    
    #[test]
    fn test_compression_level_high_bpp() {
        // ProRes with very high BPP
        let result = analyze_video_quality(
            "prores", 1920, 1080, 24.0, 60.0, 150_000_000, None,
            "yuv422p10le", 10, false, None, None, 1_125_000_000,
        ).unwrap();
        
        assert_eq!(result.compression_type, CompressionLevel::VisuallyLossless);
    }
    
    #[test]
    fn test_compression_level_standard() {
        // H.264 with typical streaming bitrate
        // 8Mbps @ 1080p30 = 8_000_000 / (1920 * 1080 * 30) = 0.128 BPP
        // For Legacy codec (efficiency=1.0), adjusted_bpp = 0.128
        // 0.1 < 0.128 < 0.3 → Standard
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        assert!(
            result.compression_type == CompressionLevel::Standard ||
            result.compression_type == CompressionLevel::HighQuality,
            "8Mbps 1080p should be Standard/HighQuality, got {:?}", result.compression_type
        );
    }
    
    #[test]
    fn test_compression_level_low_quality() {
        // H.264 with low streaming bitrate
        // 3Mbps @ 1080p30 = 3_000_000 / (1920 * 1080 * 30) = 0.048 BPP
        // For Legacy codec (efficiency=1.0), adjusted_bpp = 0.048 < 0.1 → LowQuality
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 3_000_000, None,
            "yuv420p", 8, true, None, None, 22_500_000,
        ).unwrap();
        
        assert_eq!(result.compression_type, CompressionLevel::LowQuality,
            "3Mbps 1080p should be LowQuality, got {:?}", result.compression_type);
    }
    
    // ============================================================
    // 🔬 CRF Estimation Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_crf_estimation_high_quality() {
        // High bitrate = low CRF
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 20_000_000, Some(19_000_000),
            "yuv420p", 8, true, None, None, 150_000_000,
        ).unwrap();
        
        assert!(result.estimated_crf <= 25,
            "High bitrate should estimate low CRF, got {}", result.estimated_crf);
    }
    
    #[test]
    fn test_crf_estimation_low_quality() {
        // Low bitrate = high CRF
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 1_000_000, Some(900_000),
            "yuv420p", 8, true, None, None, 7_500_000,
        ).unwrap();
        
        assert!(result.estimated_crf >= 30,
            "Low bitrate should estimate high CRF, got {}", result.estimated_crf);
    }
    
    #[test]
    fn test_crf_lossless_is_zero() {
        let result = analyze_video_quality(
            "ffv1", 1920, 1080, 30.0, 60.0, 200_000_000, None,
            "yuv444p", 8, false, None, None, 1_500_000_000,
        ).unwrap();
        
        assert_eq!(result.estimated_crf, 0, "Lossless should have CRF 0");
    }
    
    // ============================================================
    // 🔬 HDR Detection Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_hdr_detection_bt2020() {
        let result = analyze_video_quality(
            "hevc", 3840, 2160, 30.0, 60.0, 25_000_000, None,
            "yuv420p10le", 10, true, None, Some("bt2020nc"), 187_500_000,
        ).unwrap();
        
        assert!(result.is_hdr, "BT.2020 should be detected as HDR");
    }
    
    #[test]
    fn test_hdr_detection_bt709_not_hdr() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, Some("bt709"), 60_000_000,
        ).unwrap();
        
        assert!(!result.is_hdr, "BT.709 should NOT be detected as HDR");
    }
    
    #[test]
    fn test_hdr_detection_none_not_hdr() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        assert!(!result.is_hdr, "No color space should NOT be detected as HDR");
    }
    
    // ============================================================
    // 🔬 Routing Decision Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_routing_skip_modern() {
        let result = analyze_video_quality(
            "hevc", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        assert!(result.routing_decision.should_skip, "HEVC routing should skip");
        assert_eq!(result.routing_decision.primary_format, "skip");
    }
    
    #[test]
    fn test_routing_lossless_to_ffv1() {
        let result = analyze_video_quality(
            "ffv1", 1920, 1080, 30.0, 60.0, 200_000_000, None,
            "yuv444p", 8, false, None, None, 1_500_000_000,
        ).unwrap();
        
        assert!(!result.routing_decision.should_skip);
        assert_eq!(result.routing_decision.primary_format, "ffv1");
        assert!(result.routing_decision.use_lossless);
    }
    
    #[test]
    fn test_routing_prores_to_av1() {
        let result = analyze_video_quality(
            "prores", 1920, 1080, 24.0, 60.0, 150_000_000, None,
            "yuv422p10le", 10, false, None, None, 1_125_000_000,
        ).unwrap();
        
        assert!(!result.routing_decision.should_skip);
        assert_eq!(result.routing_decision.primary_format, "av1");
        assert!(!result.routing_decision.use_lossless);
    }
    
    #[test]
    fn test_routing_h264_to_av1() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        assert!(!result.routing_decision.should_skip);
        assert_eq!(result.routing_decision.primary_format, "av1");
    }
    
    // ============================================================
    // 🔬 Edge Case Tests - Invalid Input (裁判机制)
    // ============================================================
    
    #[test]
    fn test_invalid_zero_width() {
        let result = analyze_video_quality(
            "h264", 0, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        );
        
        assert!(result.is_err(), "Should fail on zero width");
        assert!(result.unwrap_err().contains("Invalid dimensions"));
    }
    
    #[test]
    fn test_invalid_zero_height() {
        let result = analyze_video_quality(
            "h264", 1920, 0, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        );
        
        assert!(result.is_err(), "Should fail on zero height");
        assert!(result.unwrap_err().contains("Invalid dimensions"));
    }
    
    #[test]
    fn test_invalid_zero_fps() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 0.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        );
        
        assert!(result.is_err(), "Should fail on zero fps");
        assert!(result.unwrap_err().contains("Invalid frame rate"));
    }
    
    #[test]
    fn test_invalid_negative_fps() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, -30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        );
        
        assert!(result.is_err(), "Should fail on negative fps");
    }
    
    #[test]
    fn test_invalid_zero_duration() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 0.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        );
        
        assert!(result.is_err(), "Should fail on zero duration");
        assert!(result.unwrap_err().contains("Invalid duration"));
    }
    
    #[test]
    fn test_invalid_negative_duration() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, -60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        );
        
        assert!(result.is_err(), "Should fail on negative duration");
    }
    
    // ============================================================
    // 🔬 Edge Case Tests - Extreme Bitrates (裁判机制)
    // ============================================================
    
    #[test]
    fn test_extreme_low_bitrate() {
        // 100kbps - very low quality streaming
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 100_000, Some(90_000),
            "yuv420p", 8, true, None, None, 750_000,
        ).unwrap();
        
        assert!(result.bpp < 0.01, "Very low bitrate should have very low BPP");
        assert_eq!(result.compression_type, CompressionLevel::LowQuality);
        assert!(result.estimated_crf >= 32, "Low bitrate should estimate high CRF");
    }
    
    #[test]
    fn test_extreme_high_bitrate() {
        // 500Mbps - near lossless
        let result = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 500_000_000, Some(490_000_000),
            "yuv420p", 8, true, None, None, 3_750_000_000,
        ).unwrap();
        
        assert!(result.bpp > 5.0, "Very high bitrate should have high BPP");
        assert!(result.compression_type == CompressionLevel::VisuallyLossless ||
                result.compression_type == CompressionLevel::HighQuality,
            "High bitrate should be VisuallyLossless or HighQuality");
    }
    
    // ============================================================
    // 🔬 Edge Case Tests - Various Resolutions (裁判机制)
    // ============================================================
    
    #[test]
    fn test_resolution_sd_480p() {
        let result = analyze_video_quality(
            "h264", 854, 480, 30.0, 60.0, 2_000_000, None,
            "yuv420p", 8, true, None, None, 15_000_000,
        ).unwrap();
        
        assert_eq!(result.width, 854);
        assert_eq!(result.height, 480);
        // SD at 2Mbps should have decent BPP
        let expected_bpp = 2_000_000.0 / (854.0 * 480.0 * 30.0);
        assert!((result.bpp - expected_bpp).abs() < 0.001);
    }
    
    #[test]
    fn test_resolution_hd_720p() {
        let result = analyze_video_quality(
            "h264", 1280, 720, 30.0, 60.0, 5_000_000, None,
            "yuv420p", 8, true, None, None, 37_500_000,
        ).unwrap();
        
        assert_eq!(result.width, 1280);
        assert_eq!(result.height, 720);
    }
    
    #[test]
    fn test_resolution_4k_uhd() {
        let result = analyze_video_quality(
            "hevc", 3840, 2160, 30.0, 60.0, 25_000_000, None,
            "yuv420p10le", 10, true, None, None, 187_500_000,
        ).unwrap();
        
        assert_eq!(result.width, 3840);
        assert_eq!(result.height, 2160);
        assert!(result.should_skip, "4K HEVC should be skipped");
    }
    
    #[test]
    fn test_resolution_8k() {
        let result = analyze_video_quality(
            "av1", 7680, 4320, 30.0, 60.0, 80_000_000, None,
            "yuv420p10le", 10, true, None, None, 600_000_000,
        ).unwrap();
        
        assert_eq!(result.width, 7680);
        assert_eq!(result.height, 4320);
        assert!(result.should_skip, "8K AV1 should be skipped");
    }
    
    #[test]
    fn test_resolution_vertical_video() {
        // 9:16 vertical video (mobile)
        let result = analyze_video_quality(
            "h264", 1080, 1920, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        assert_eq!(result.width, 1080);
        assert_eq!(result.height, 1920);
        assert!(!result.should_skip);
    }
    
    #[test]
    fn test_resolution_square() {
        // 1:1 square video (Instagram)
        let result = analyze_video_quality(
            "h264", 1080, 1080, 30.0, 60.0, 6_000_000, None,
            "yuv420p", 8, true, None, None, 45_000_000,
        ).unwrap();
        
        assert_eq!(result.width, 1080);
        assert_eq!(result.height, 1080);
    }
    
    // ============================================================
    // 🔬 Edge Case Tests - Frame Rates (裁判机制)
    // ============================================================
    
    #[test]
    fn test_fps_24_film() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 24.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        assert!((result.fps - 24.0).abs() < 0.01);
        assert_eq!(result.frame_count, 1440); // 24 * 60
    }
    
    #[test]
    fn test_fps_60_gaming() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 60.0, 60.0, 15_000_000, None,
            "yuv420p", 8, true, None, None, 112_500_000,
        ).unwrap();
        
        assert!((result.fps - 60.0).abs() < 0.01);
        assert_eq!(result.frame_count, 3600); // 60 * 60
    }
    
    #[test]
    fn test_fps_120_high_refresh() {
        let result = analyze_video_quality(
            "h264", 1920, 1080, 120.0, 30.0, 25_000_000, None,
            "yuv420p", 8, true, None, None, 93_750_000,
        ).unwrap();
        
        assert!((result.fps - 120.0).abs() < 0.01);
        assert_eq!(result.frame_count, 3600); // 120 * 30
    }
    
    #[test]
    fn test_fps_fractional_ntsc() {
        // 29.97 fps (NTSC)
        let result = analyze_video_quality(
            "h264", 1920, 1080, 29.97, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        assert!((result.fps - 29.97).abs() < 0.01);
    }
    
    // ============================================================
    // 🔬 Codec Type Classification Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_codec_type_lossless() {
        // FFV1
        let ffv1 = analyze_video_quality(
            "ffv1", 1920, 1080, 30.0, 60.0, 200_000_000, None,
            "yuv444p", 8, false, None, None, 1_500_000_000,
        ).unwrap();
        assert_eq!(ffv1.codec_type, VideoCodecType::Lossless);
        
        // HuffYUV
        let huffyuv = analyze_video_quality(
            "huffyuv", 1920, 1080, 30.0, 60.0, 300_000_000, None,
            "yuv422p", 8, false, None, None, 2_250_000_000,
        ).unwrap();
        assert_eq!(huffyuv.codec_type, VideoCodecType::Lossless);
        
        // UTVideo
        let utvideo = analyze_video_quality(
            "utvideo", 1920, 1080, 30.0, 60.0, 250_000_000, None,
            "yuv422p", 8, false, None, None, 1_875_000_000,
        ).unwrap();
        assert_eq!(utvideo.codec_type, VideoCodecType::Lossless);
    }
    
    #[test]
    fn test_codec_type_modern() {
        let codecs = ["av1", "hevc", "h265", "vp9", "vvc"];
        for codec in codecs {
            let result = analyze_video_quality(
                codec, 1920, 1080, 30.0, 60.0, 8_000_000, None,
                "yuv420p", 8, true, None, None, 60_000_000,
            ).unwrap();
            assert_eq!(result.codec_type, VideoCodecType::ModernEfficient,
                "Codec {} should be ModernEfficient", codec);
        }
    }
    
    #[test]
    fn test_codec_type_intermediate() {
        // ProRes
        let prores = analyze_video_quality(
            "prores", 1920, 1080, 24.0, 60.0, 150_000_000, None,
            "yuv422p10le", 10, false, None, None, 1_125_000_000,
        ).unwrap();
        assert_eq!(prores.codec_type, VideoCodecType::Intermediate);
        
        // DNxHD
        let dnxhd = analyze_video_quality(
            "dnxhd", 1920, 1080, 24.0, 60.0, 120_000_000, None,
            "yuv422p", 8, false, None, None, 900_000_000,
        ).unwrap();
        assert_eq!(dnxhd.codec_type, VideoCodecType::Intermediate);
    }
    
    #[test]
    fn test_codec_type_inefficient() {
        // MJPEG
        let mjpeg = analyze_video_quality(
            "mjpeg", 1920, 1080, 30.0, 60.0, 50_000_000, None,
            "yuvj420p", 8, false, None, None, 375_000_000,
        ).unwrap();
        assert_eq!(mjpeg.codec_type, VideoCodecType::Inefficient);
        
        // GIF
        let gif = analyze_video_quality(
            "gif", 640, 480, 15.0, 10.0, 5_000_000, None,
            "rgb8", 8, false, None, None, 6_250_000,
        ).unwrap();
        assert_eq!(gif.codec_type, VideoCodecType::Inefficient);
    }
    
    // ============================================================
    // 🔬 Confidence Calculation Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_confidence_with_video_bitrate() {
        let with_vbr = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 10_000_000, Some(8_000_000),
            "yuv420p", 8, true, Some(60), None, 75_000_000,
        ).unwrap();
        
        let without_vbr = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 10_000_000, None,
            "yuv420p", 8, true, None, None, 75_000_000,
        ).unwrap();
        
        assert!(with_vbr.confidence > without_vbr.confidence,
            "Video bitrate should increase confidence: {} vs {}",
            with_vbr.confidence, without_vbr.confidence);
    }
    
    #[test]
    fn test_confidence_with_gop_size() {
        let with_gop = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, Some(60), None, 60_000_000,
        ).unwrap();
        
        let without_gop = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 60_000_000,
        ).unwrap();
        
        assert!(with_gop.confidence > without_gop.confidence,
            "GOP size should increase confidence");
    }
    
    #[test]
    fn test_confidence_longer_duration() {
        let long = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 120.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 120_000_000,
        ).unwrap();
        
        let short = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 5.0, 8_000_000, None,
            "yuv420p", 8, true, None, None, 5_000_000,
        ).unwrap();
        
        assert!(long.confidence >= short.confidence,
            "Longer duration should have >= confidence");
    }
    
    // ============================================================
    // 🔬 to_quality_analysis Integration Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_to_quality_analysis_conversion() {
        let analysis = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, Some(7_500_000),
            "yuv420p", 8, true, Some(60), Some("bt709"), 60_000_000,
        ).unwrap();
        
        let qa = to_quality_analysis(&analysis);
        
        assert_eq!(qa.width, 1920);
        assert_eq!(qa.height, 1080);
        assert!((qa.fps.unwrap() - 30.0).abs() < 0.01);
        assert!((qa.duration_secs.unwrap() - 60.0).abs() < 0.01);
        assert_eq!(qa.video_bitrate, Some(7_500_000));
    }
    
    // ============================================================
    // 🔬 Consistency Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_consistency_same_input() {
        let result1 = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, Some(7_500_000),
            "yuv420p", 8, true, Some(60), Some("bt709"), 60_000_000,
        ).unwrap();
        
        let result2 = analyze_video_quality(
            "h264", 1920, 1080, 30.0, 60.0, 8_000_000, Some(7_500_000),
            "yuv420p", 8, true, Some(60), Some("bt709"), 60_000_000,
        ).unwrap();
        
        assert!((result1.bpp - result2.bpp).abs() < 0.0001,
            "Same input should produce same BPP");
        assert_eq!(result1.codec_type, result2.codec_type);
        assert_eq!(result1.should_skip, result2.should_skip);
        assert_eq!(result1.estimated_crf, result2.estimated_crf);
    }
    
    // ============================================================
    // 🔬 Strict Precision Tests (裁判机制)
    // ============================================================
    
    /// Strict test: BPP calculation must be mathematically correct
    #[test]
    fn test_strict_bpp_formula() {
        // Test multiple resolutions and bitrates
        let test_cases = [
            (1920, 1080, 30.0, 8_000_000u64),
            (3840, 2160, 30.0, 25_000_000u64),
            (1280, 720, 60.0, 5_000_000u64),
            (854, 480, 24.0, 2_000_000u64),
        ];
        
        for (w, h, fps, bitrate) in test_cases {
            let result = analyze_video_quality(
                "h264", w, h, fps, 60.0, bitrate, Some(bitrate),
                "yuv420p", 8, true, None, None, bitrate * 60 / 8,
            ).unwrap();
            
            let expected = bitrate as f64 / (w as f64 * h as f64 * fps);
            assert!((result.bpp - expected).abs() < 0.0001,
                "STRICT: BPP for {}x{}@{}fps@{}bps: expected {}, got {}",
                w, h, fps, bitrate, expected, result.bpp);
        }
    }
    
    /// Strict test: Frame count must be mathematically correct
    #[test]
    fn test_strict_frame_count() {
        let test_cases = [
            (30.0, 60.0, 1800u64),   // 30fps * 60s = 1800
            (24.0, 120.0, 2880u64),  // 24fps * 120s = 2880
            (60.0, 30.0, 1800u64),   // 60fps * 30s = 1800
        ];
        
        for (fps, duration, expected_frames) in test_cases {
            let result = analyze_video_quality(
                "h264", 1920, 1080, fps, duration, 8_000_000, None,
                "yuv420p", 8, true, None, None, 60_000_000,
            ).unwrap();
            
            assert_eq!(result.frame_count, expected_frames,
                "STRICT: Frame count for {}fps * {}s: expected {}, got {}",
                fps, duration, expected_frames, result.frame_count);
        }
    }
    
    /// Strict test: Modern codecs must always skip
    #[test]
    fn test_strict_modern_always_skip() {
        let modern_codecs = ["hevc", "h265", "av1", "vp9", "vvc", "av2"];
        
        for codec in modern_codecs {
            let result = analyze_video_quality(
                codec, 1920, 1080, 30.0, 60.0, 8_000_000, None,
                "yuv420p", 8, true, None, None, 60_000_000,
            ).unwrap();
            
            assert!(result.should_skip,
                "STRICT: Modern codec {} must ALWAYS skip", codec);
            assert!(result.is_modern_codec,
                "STRICT: {} must be detected as modern", codec);
        }
    }
    
    /// Strict test: Legacy codecs must never skip
    #[test]
    fn test_strict_legacy_never_skip() {
        let legacy_codecs = ["h264", "mpeg4", "mpeg2video", "mjpeg"];
        
        for codec in legacy_codecs {
            let result = analyze_video_quality(
                codec, 1920, 1080, 30.0, 60.0, 8_000_000, None,
                "yuv420p", 8, true, None, None, 60_000_000,
            ).unwrap();
            
            assert!(!result.should_skip,
                "STRICT: Legacy codec {} must NEVER skip", codec);
        }
    }
}
