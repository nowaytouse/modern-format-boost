//! Codec Information Module
//!
//! Contains codec-specific information and characteristics.
//! Shared between vidquality and vidquality-hevc.

use serde::{Deserialize, Serialize};

/// Codec category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodecCategory {
    /// Designed for lossless archival
    Archival,
    /// Production intermediate codec
    Production,
    /// Delivery/streaming codec
    Delivery,
    /// Screen recording codec
    ScreenCapture,
    /// Unknown category
    Unknown,
}

/// Codec information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodecInfo {
    pub name: String,
    pub long_name: String,
    pub category: CodecCategory,
    pub is_lossless: bool,
    pub typical_extension: String,
}

/// Detected video codec enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectedCodec {
    // Lossless/Archival
    FFV1,
    ProRes,
    DNxHD,
    HuffYUV,
    UTVideo,
    RawVideo,
    
    // Modern Delivery (skip conversion)
    H265,
    AV1,
    AV2,
    VP9,
    VVC,
    
    // Legacy Delivery (convert)
    H264,
    VP8,
    MPEG4,
    MPEG2,
    MPEG1,
    WMV,
    
    // Animation
    GIF,
    APNG,
    WebPAnim,
    
    // Unknown
    Unknown(String),
}

impl DetectedCodec {
    /// Create from ffprobe codec name
    pub fn from_ffprobe(codec_name: &str) -> Self {
        match codec_name.to_lowercase().as_str() {
            "ffv1" => DetectedCodec::FFV1,
            "prores" | "prores_ks" => DetectedCodec::ProRes,
            "dnxhd" | "dnxhr" => DetectedCodec::DNxHD,
            "huffyuv" | "ffvhuff" => DetectedCodec::HuffYUV,
            "utvideo" => DetectedCodec::UTVideo,
            "rawvideo" => DetectedCodec::RawVideo,
            "hevc" | "h265" | "libx265" => DetectedCodec::H265,
            "av1" | "libaom-av1" | "libsvtav1" => DetectedCodec::AV1,
            "vp9" | "libvpx-vp9" => DetectedCodec::VP9,
            "vvc" | "h266" | "libvvenc" => DetectedCodec::VVC,
            "h264" | "avc" | "libx264" => DetectedCodec::H264,
            "vp8" | "libvpx" => DetectedCodec::VP8,
            "mpeg4" | "xvid" | "divx" => DetectedCodec::MPEG4,
            "mpeg2video" => DetectedCodec::MPEG2,
            "mpeg1video" => DetectedCodec::MPEG1,
            "wmv1" | "wmv2" | "wmv3" | "vc1" => DetectedCodec::WMV,
            "gif" => DetectedCodec::GIF,
            "apng" => DetectedCodec::APNG,
            "webp" => DetectedCodec::WebPAnim,
            _ => DetectedCodec::Unknown(codec_name.to_string()),
        }
    }
    
    /// Get human-readable codec name
    pub fn as_str(&self) -> &str {
        match self {
            DetectedCodec::FFV1 => "FFV1",
            DetectedCodec::ProRes => "ProRes",
            DetectedCodec::DNxHD => "DNxHD",
            DetectedCodec::HuffYUV => "HuffYUV",
            DetectedCodec::UTVideo => "UT Video",
            DetectedCodec::RawVideo => "Raw Video",
            DetectedCodec::H265 => "H.265/HEVC",
            DetectedCodec::AV1 => "AV1",
            DetectedCodec::AV2 => "AV2",
            DetectedCodec::VP9 => "VP9",
            DetectedCodec::VVC => "H.266/VVC",
            DetectedCodec::H264 => "H.264/AVC",
            DetectedCodec::VP8 => "VP8",
            DetectedCodec::MPEG4 => "MPEG-4",
            DetectedCodec::MPEG2 => "MPEG-2",
            DetectedCodec::MPEG1 => "MPEG-1",
            DetectedCodec::WMV => "WMV",
            DetectedCodec::GIF => "GIF",
            DetectedCodec::APNG => "APNG",
            DetectedCodec::WebPAnim => "WebP (Animated)",
            DetectedCodec::Unknown(s) => s,
        }
    }
    
    /// Check if this is a modern codec (should skip conversion)
    pub fn is_modern(&self) -> bool {
        matches!(self, 
            DetectedCodec::H265 | 
            DetectedCodec::AV1 | 
            DetectedCodec::AV2 |
            DetectedCodec::VP9 | 
            DetectedCodec::VVC
        )
    }
    
    /// Check if this is a lossless codec
    pub fn is_lossless(&self) -> bool {
        matches!(self,
            DetectedCodec::FFV1 |
            DetectedCodec::HuffYUV |
            DetectedCodec::UTVideo |
            DetectedCodec::RawVideo
        )
    }
    
    /// Check if this is a production codec
    pub fn is_production(&self) -> bool {
        matches!(self,
            DetectedCodec::ProRes |
            DetectedCodec::DNxHD
        )
    }
}

/// Get codec info by name
pub fn get_codec_info(codec_name: &str) -> CodecInfo {
    match codec_name.to_lowercase().as_str() {
        "ffv1" => CodecInfo {
            name: "FFV1".to_string(),
            long_name: "FF Video 1 (Lossless)".to_string(),
            category: CodecCategory::Archival,
            is_lossless: true,
            typical_extension: "mkv".to_string(),
        },
        "prores" | "prores_ks" => CodecInfo {
            name: "ProRes".to_string(),
            long_name: "Apple ProRes".to_string(),
            category: CodecCategory::Production,
            is_lossless: false,
            typical_extension: "mov".to_string(),
        },
        "dnxhd" | "dnxhr" => CodecInfo {
            name: "DNxHD".to_string(),
            long_name: "Avid DNxHD/DNxHR".to_string(),
            category: CodecCategory::Production,
            is_lossless: false,
            typical_extension: "mxf".to_string(),
        },
        "h264" | "avc" | "libx264" => CodecInfo {
            name: "H.264".to_string(),
            long_name: "H.264 / AVC".to_string(),
            category: CodecCategory::Delivery,
            is_lossless: false,
            typical_extension: "mp4".to_string(),
        },
        "hevc" | "h265" | "libx265" => CodecInfo {
            name: "H.265".to_string(),
            long_name: "H.265 / HEVC".to_string(),
            category: CodecCategory::Delivery,
            is_lossless: false,
            typical_extension: "mp4".to_string(),
        },
        "vp9" | "libvpx-vp9" => CodecInfo {
            name: "VP9".to_string(),
            long_name: "Google VP9".to_string(),
            category: CodecCategory::Delivery,
            is_lossless: false,
            typical_extension: "webm".to_string(),
        },
        "av1" | "libaom-av1" | "libsvtav1" => CodecInfo {
            name: "AV1".to_string(),
            long_name: "AOMedia Video 1".to_string(),
            category: CodecCategory::Delivery,
            is_lossless: false,
            typical_extension: "mp4".to_string(),
        },
        "vvc" | "h266" | "libvvenc" => CodecInfo {
            name: "H.266".to_string(),
            long_name: "H.266 / VVC".to_string(),
            category: CodecCategory::Delivery,
            is_lossless: false,
            typical_extension: "mp4".to_string(),
        },
        "rawvideo" => CodecInfo {
            name: "Raw".to_string(),
            long_name: "Uncompressed Video".to_string(),
            category: CodecCategory::Archival,
            is_lossless: true,
            typical_extension: "avi".to_string(),
        },
        "huffyuv" | "ffvhuff" => CodecInfo {
            name: "HuffYUV".to_string(),
            long_name: "Huffman YUV Lossless".to_string(),
            category: CodecCategory::Archival,
            is_lossless: true,
            typical_extension: "avi".to_string(),
        },
        "utvideo" => CodecInfo {
            name: "UT Video".to_string(),
            long_name: "Ut Video Lossless".to_string(),
            category: CodecCategory::Archival,
            is_lossless: true,
            typical_extension: "avi".to_string(),
        },
        _ => CodecInfo {
            name: "Unknown".to_string(),
            long_name: codec_name.to_string(),
            category: CodecCategory::Unknown,
            is_lossless: false,
            typical_extension: "mp4".to_string(),
        },
    }
}

// ============================================================
// 🔬 PRECISION VALIDATION TESTS ("裁判" Tests)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // Codec Detection Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_detected_codec_h264_variants() {
        // All H.264 variants should be detected correctly
        assert_eq!(DetectedCodec::from_ffprobe("h264"), DetectedCodec::H264);
        assert_eq!(DetectedCodec::from_ffprobe("avc"), DetectedCodec::H264);
        assert_eq!(DetectedCodec::from_ffprobe("libx264"), DetectedCodec::H264);
        assert_eq!(DetectedCodec::from_ffprobe("H264"), DetectedCodec::H264); // case insensitive
    }
    
    #[test]
    fn test_detected_codec_hevc_variants() {
        // All HEVC variants should be detected correctly
        assert_eq!(DetectedCodec::from_ffprobe("hevc"), DetectedCodec::H265);
        assert_eq!(DetectedCodec::from_ffprobe("h265"), DetectedCodec::H265);
        assert_eq!(DetectedCodec::from_ffprobe("libx265"), DetectedCodec::H265);
        assert_eq!(DetectedCodec::from_ffprobe("HEVC"), DetectedCodec::H265); // case insensitive
    }
    
    #[test]
    fn test_detected_codec_av1_variants() {
        // All AV1 variants should be detected correctly
        assert_eq!(DetectedCodec::from_ffprobe("av1"), DetectedCodec::AV1);
        assert_eq!(DetectedCodec::from_ffprobe("libaom-av1"), DetectedCodec::AV1);
        assert_eq!(DetectedCodec::from_ffprobe("libsvtav1"), DetectedCodec::AV1);
    }
    
    #[test]
    fn test_detected_codec_vp9_variants() {
        assert_eq!(DetectedCodec::from_ffprobe("vp9"), DetectedCodec::VP9);
        assert_eq!(DetectedCodec::from_ffprobe("libvpx-vp9"), DetectedCodec::VP9);
    }
    
    #[test]
    fn test_detected_codec_vvc_variants() {
        assert_eq!(DetectedCodec::from_ffprobe("vvc"), DetectedCodec::VVC);
        assert_eq!(DetectedCodec::from_ffprobe("h266"), DetectedCodec::VVC);
        assert_eq!(DetectedCodec::from_ffprobe("libvvenc"), DetectedCodec::VVC);
    }
    
    #[test]
    fn test_detected_codec_lossless() {
        // All lossless codecs
        assert_eq!(DetectedCodec::from_ffprobe("ffv1"), DetectedCodec::FFV1);
        assert_eq!(DetectedCodec::from_ffprobe("huffyuv"), DetectedCodec::HuffYUV);
        assert_eq!(DetectedCodec::from_ffprobe("ffvhuff"), DetectedCodec::HuffYUV);
        assert_eq!(DetectedCodec::from_ffprobe("utvideo"), DetectedCodec::UTVideo);
        assert_eq!(DetectedCodec::from_ffprobe("rawvideo"), DetectedCodec::RawVideo);
    }
    
    #[test]
    fn test_detected_codec_production() {
        // Production codecs
        assert_eq!(DetectedCodec::from_ffprobe("prores"), DetectedCodec::ProRes);
        assert_eq!(DetectedCodec::from_ffprobe("prores_ks"), DetectedCodec::ProRes);
        assert_eq!(DetectedCodec::from_ffprobe("dnxhd"), DetectedCodec::DNxHD);
        assert_eq!(DetectedCodec::from_ffprobe("dnxhr"), DetectedCodec::DNxHD);
    }
    
    #[test]
    fn test_detected_codec_legacy() {
        // Legacy codecs
        assert_eq!(DetectedCodec::from_ffprobe("vp8"), DetectedCodec::VP8);
        assert_eq!(DetectedCodec::from_ffprobe("libvpx"), DetectedCodec::VP8);
        assert_eq!(DetectedCodec::from_ffprobe("mpeg4"), DetectedCodec::MPEG4);
        assert_eq!(DetectedCodec::from_ffprobe("xvid"), DetectedCodec::MPEG4);
        assert_eq!(DetectedCodec::from_ffprobe("divx"), DetectedCodec::MPEG4);
        assert_eq!(DetectedCodec::from_ffprobe("mpeg2video"), DetectedCodec::MPEG2);
        assert_eq!(DetectedCodec::from_ffprobe("mpeg1video"), DetectedCodec::MPEG1);
    }
    
    #[test]
    fn test_detected_codec_wmv() {
        assert_eq!(DetectedCodec::from_ffprobe("wmv1"), DetectedCodec::WMV);
        assert_eq!(DetectedCodec::from_ffprobe("wmv2"), DetectedCodec::WMV);
        assert_eq!(DetectedCodec::from_ffprobe("wmv3"), DetectedCodec::WMV);
        assert_eq!(DetectedCodec::from_ffprobe("vc1"), DetectedCodec::WMV);
    }
    
    #[test]
    fn test_detected_codec_animation() {
        assert_eq!(DetectedCodec::from_ffprobe("gif"), DetectedCodec::GIF);
        assert_eq!(DetectedCodec::from_ffprobe("apng"), DetectedCodec::APNG);
        assert_eq!(DetectedCodec::from_ffprobe("webp"), DetectedCodec::WebPAnim);
    }
    
    #[test]
    fn test_detected_codec_unknown() {
        let unknown = DetectedCodec::from_ffprobe("some_unknown_codec");
        assert!(matches!(unknown, DetectedCodec::Unknown(_)));
        if let DetectedCodec::Unknown(name) = unknown {
            assert_eq!(name, "some_unknown_codec");
        }
    }
    
    // ============================================================
    // Codec Properties Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_is_modern_codecs() {
        // Modern codecs should return true
        assert!(DetectedCodec::H265.is_modern(), "H265 should be modern");
        assert!(DetectedCodec::AV1.is_modern(), "AV1 should be modern");
        assert!(DetectedCodec::AV2.is_modern(), "AV2 should be modern");
        assert!(DetectedCodec::VP9.is_modern(), "VP9 should be modern");
        assert!(DetectedCodec::VVC.is_modern(), "VVC should be modern");
        
        // Non-modern codecs should return false
        assert!(!DetectedCodec::H264.is_modern(), "H264 should NOT be modern");
        assert!(!DetectedCodec::VP8.is_modern(), "VP8 should NOT be modern");
        assert!(!DetectedCodec::MPEG4.is_modern(), "MPEG4 should NOT be modern");
        assert!(!DetectedCodec::FFV1.is_modern(), "FFV1 should NOT be modern");
        assert!(!DetectedCodec::ProRes.is_modern(), "ProRes should NOT be modern");
    }
    
    #[test]
    fn test_is_lossless_codecs() {
        // Lossless codecs should return true
        assert!(DetectedCodec::FFV1.is_lossless(), "FFV1 should be lossless");
        assert!(DetectedCodec::HuffYUV.is_lossless(), "HuffYUV should be lossless");
        assert!(DetectedCodec::UTVideo.is_lossless(), "UTVideo should be lossless");
        assert!(DetectedCodec::RawVideo.is_lossless(), "RawVideo should be lossless");
        
        // Lossy codecs should return false
        assert!(!DetectedCodec::H264.is_lossless(), "H264 should NOT be lossless");
        assert!(!DetectedCodec::H265.is_lossless(), "H265 should NOT be lossless");
        assert!(!DetectedCodec::ProRes.is_lossless(), "ProRes should NOT be lossless");
    }
    
    #[test]
    fn test_is_production_codecs() {
        // Production codecs should return true
        assert!(DetectedCodec::ProRes.is_production(), "ProRes should be production");
        assert!(DetectedCodec::DNxHD.is_production(), "DNxHD should be production");
        
        // Non-production codecs should return false
        assert!(!DetectedCodec::H264.is_production(), "H264 should NOT be production");
        assert!(!DetectedCodec::FFV1.is_production(), "FFV1 should NOT be production");
    }
    
    // ============================================================
    // Codec Info Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_codec_info_h264() {
        let info = get_codec_info("h264");
        assert_eq!(info.name, "H.264");
        assert_eq!(info.category, CodecCategory::Delivery);
        assert!(!info.is_lossless);
        assert_eq!(info.typical_extension, "mp4");
    }
    
    #[test]
    fn test_codec_info_hevc() {
        let info = get_codec_info("hevc");
        assert_eq!(info.name, "H.265");
        assert_eq!(info.category, CodecCategory::Delivery);
        assert!(!info.is_lossless);
    }
    
    #[test]
    fn test_codec_info_av1() {
        let info = get_codec_info("av1");
        assert_eq!(info.name, "AV1");
        assert_eq!(info.category, CodecCategory::Delivery);
    }
    
    #[test]
    fn test_codec_info_ffv1() {
        let info = get_codec_info("ffv1");
        assert_eq!(info.name, "FFV1");
        assert_eq!(info.category, CodecCategory::Archival);
        assert!(info.is_lossless);
        assert_eq!(info.typical_extension, "mkv");
    }
    
    #[test]
    fn test_codec_info_prores() {
        let info = get_codec_info("prores");
        assert_eq!(info.name, "ProRes");
        assert_eq!(info.category, CodecCategory::Production);
        assert!(!info.is_lossless);
        assert_eq!(info.typical_extension, "mov");
    }
    
    #[test]
    fn test_codec_info_unknown() {
        let info = get_codec_info("unknown_codec");
        assert_eq!(info.name, "Unknown");
        assert_eq!(info.category, CodecCategory::Unknown);
    }
    
    // ============================================================
    // 🔬 Strict Consistency Tests (裁判机制)
    // ============================================================
    
    /// Strict test: Modern codec detection must be consistent with skip logic
    #[test]
    fn test_strict_modern_skip_consistency() {
        // All modern codecs should be skipped in conversion
        let modern_codecs = [
            DetectedCodec::H265,
            DetectedCodec::AV1,
            DetectedCodec::AV2,
            DetectedCodec::VP9,
            DetectedCodec::VVC,
        ];
        
        for codec in modern_codecs {
            assert!(codec.is_modern(),
                "STRICT: {:?} must be detected as modern", codec);
        }
    }
    
    /// Strict test: Lossless codec detection must be accurate
    #[test]
    fn test_strict_lossless_accuracy() {
        let lossless_codecs = [
            DetectedCodec::FFV1,
            DetectedCodec::HuffYUV,
            DetectedCodec::UTVideo,
            DetectedCodec::RawVideo,
        ];
        
        for codec in lossless_codecs {
            assert!(codec.is_lossless(),
                "STRICT: {:?} must be detected as lossless", codec);
        }
        
        // These should NOT be lossless
        let lossy_codecs = [
            DetectedCodec::H264,
            DetectedCodec::H265,
            DetectedCodec::AV1,
            DetectedCodec::ProRes,
            DetectedCodec::DNxHD,
        ];
        
        for codec in lossy_codecs {
            assert!(!codec.is_lossless(),
                "STRICT: {:?} must NOT be detected as lossless", codec);
        }
    }
    
    /// Strict test: Codec name display must be human-readable
    #[test]
    fn test_strict_codec_names() {
        // Names should be human-readable, not internal codec names
        assert_eq!(DetectedCodec::H264.as_str(), "H.264/AVC");
        assert_eq!(DetectedCodec::H265.as_str(), "H.265/HEVC");
        assert_eq!(DetectedCodec::AV1.as_str(), "AV1");
        assert_eq!(DetectedCodec::VVC.as_str(), "H.266/VVC");
        assert_eq!(DetectedCodec::FFV1.as_str(), "FFV1");
        assert_eq!(DetectedCodec::ProRes.as_str(), "ProRes");
    }
}
