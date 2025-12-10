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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detected_codec() {
        assert_eq!(DetectedCodec::from_ffprobe("h264").as_str(), "H.264/AVC");
        assert_eq!(DetectedCodec::from_ffprobe("hevc").as_str(), "H.265/HEVC");
        assert!(DetectedCodec::from_ffprobe("av1").is_modern());
        assert!(DetectedCodec::from_ffprobe("ffv1").is_lossless());
    }

    #[test]
    fn test_codec_info() {
        let info = get_codec_info("h264");
        assert_eq!(info.name, "H.264");
        assert_eq!(info.category, CodecCategory::Delivery);
    }
}
