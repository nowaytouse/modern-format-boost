//! Codec Information Module
//!
//! Contains codec-specific information and characteristics.
//! Shared between vidquality and vid-hevc.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodecCategory {
    Archival,
    Production,
    Delivery,
    ScreenCapture,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodecInfo {
    pub name: String,
    pub long_name: String,
    pub category: CodecCategory,
    pub is_lossless: bool,
    pub typical_extension: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectedCodec {
    FFV1,
    ProRes,
    DNxHD,
    HuffYUV,
    UTVideo,
    RawVideo,

    H265,
    AV1,
    AV2,
    VP9,
    VVC,

    H264,
    VP8,
    MPEG4,
    MPEG2,
    MPEG1,
    WMV,

    GIF,
    APNG,
    WebPAnim,

    Unknown(String),
}

impl DetectedCodec {
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

    pub fn is_modern(&self) -> bool {
        matches!(
            self,
            DetectedCodec::H265
                | DetectedCodec::AV1
                | DetectedCodec::AV2
                | DetectedCodec::VP9
                | DetectedCodec::VVC
        )
    }

    pub fn is_lossless(&self) -> bool {
        matches!(
            self,
            DetectedCodec::FFV1
                | DetectedCodec::HuffYUV
                | DetectedCodec::UTVideo
                | DetectedCodec::RawVideo
        )
    }

    pub fn is_production(&self) -> bool {
        matches!(self, DetectedCodec::ProRes | DetectedCodec::DNxHD)
    }
}

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
    fn test_detected_codec_from_ffprobe() {
        let cases: &[(&str, DetectedCodec)] = &[
            ("h264", DetectedCodec::H264),
            ("avc", DetectedCodec::H264),
            ("libx264", DetectedCodec::H264),
            ("H264", DetectedCodec::H264),
            ("hevc", DetectedCodec::H265),
            ("h265", DetectedCodec::H265),
            ("libx265", DetectedCodec::H265),
            ("HEVC", DetectedCodec::H265),
            ("av1", DetectedCodec::AV1),
            ("libaom-av1", DetectedCodec::AV1),
            ("libsvtav1", DetectedCodec::AV1),
            ("vp9", DetectedCodec::VP9),
            ("libvpx-vp9", DetectedCodec::VP9),
            ("vvc", DetectedCodec::VVC),
            ("h266", DetectedCodec::VVC),
            ("libvvenc", DetectedCodec::VVC),
            ("ffv1", DetectedCodec::FFV1),
            ("huffyuv", DetectedCodec::HuffYUV),
            ("ffvhuff", DetectedCodec::HuffYUV),
            ("utvideo", DetectedCodec::UTVideo),
            ("rawvideo", DetectedCodec::RawVideo),
            ("prores", DetectedCodec::ProRes),
            ("prores_ks", DetectedCodec::ProRes),
            ("dnxhd", DetectedCodec::DNxHD),
            ("dnxhr", DetectedCodec::DNxHD),
            ("vp8", DetectedCodec::VP8),
            ("libvpx", DetectedCodec::VP8),
            ("mpeg4", DetectedCodec::MPEG4),
            ("xvid", DetectedCodec::MPEG4),
            ("divx", DetectedCodec::MPEG4),
            ("mpeg2video", DetectedCodec::MPEG2),
            ("mpeg1video", DetectedCodec::MPEG1),
            ("wmv1", DetectedCodec::WMV),
            ("wmv2", DetectedCodec::WMV),
            ("wmv3", DetectedCodec::WMV),
            ("vc1", DetectedCodec::WMV),
            ("gif", DetectedCodec::GIF),
            ("apng", DetectedCodec::APNG),
            ("webp", DetectedCodec::WebPAnim),
        ];

        for (input, expected) in cases {
            assert_eq!(
                DetectedCodec::from_ffprobe(input),
                *expected,
                "from_ffprobe({:?}) mismatch",
                input
            );
        }
    }

    #[test]
    fn test_detected_codec_unknown() {
        let unknown = DetectedCodec::from_ffprobe("some_unknown_codec");
        assert!(matches!(unknown, DetectedCodec::Unknown(_)));
        if let DetectedCodec::Unknown(name) = unknown {
            assert_eq!(name, "some_unknown_codec");
        }
    }


    #[test]
    fn test_codec_properties() {
        let cases: &[(DetectedCodec, bool, bool, bool)] = &[
            (DetectedCodec::H265, true, false, false),
            (DetectedCodec::AV1, true, false, false),
            (DetectedCodec::AV2, true, false, false),
            (DetectedCodec::VP9, true, false, false),
            (DetectedCodec::VVC, true, false, false),
            (DetectedCodec::H264, false, false, false),
            (DetectedCodec::VP8, false, false, false),
            (DetectedCodec::MPEG4, false, false, false),
            (DetectedCodec::FFV1, false, true, false),
            (DetectedCodec::HuffYUV, false, true, false),
            (DetectedCodec::UTVideo, false, true, false),
            (DetectedCodec::RawVideo, false, true, false),
            (DetectedCodec::ProRes, false, false, true),
            (DetectedCodec::DNxHD, false, false, true),
        ];

        for (codec, modern, lossless, production) in cases {
            assert_eq!(codec.is_modern(), *modern, "{:?}.is_modern()", codec);
            assert_eq!(codec.is_lossless(), *lossless, "{:?}.is_lossless()", codec);
            assert_eq!(codec.is_production(), *production, "{:?}.is_production()", codec);
        }
    }


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


    #[test]
    fn test_strict_codec_names() {
        assert_eq!(DetectedCodec::H264.as_str(), "H.264/AVC");
        assert_eq!(DetectedCodec::H265.as_str(), "H.265/HEVC");
        assert_eq!(DetectedCodec::AV1.as_str(), "AV1");
        assert_eq!(DetectedCodec::VVC.as_str(), "H.266/VVC");
        assert_eq!(DetectedCodec::FFV1.as_str(), "FFV1");
        assert_eq!(DetectedCodec::ProRes.as_str(), "ProRes");
    }
}
