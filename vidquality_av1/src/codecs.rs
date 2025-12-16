//! Codec information module
//!
//! Contains codec-specific information and characteristics

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
