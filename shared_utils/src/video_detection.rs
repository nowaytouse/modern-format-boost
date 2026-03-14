//! Video Detection API Module (Shared)
//!
//! Pure analysis layer - detects video properties using ffprobe.
//! Determines codec type, compression level, and archival suitability.
//!
//! Migrated from vid_hevc/vid_av1 detection_api.rs to eliminate duplication.

use crate::ffprobe::{probe_video, FFprobeError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoPrecisionMetadata {
    pub original_crf: Option<f32>,
    pub original_preset: Option<String>,
    pub original_encoder: Option<String>,
    pub original_max_b_frames: Option<u8>,
    pub is_lossless_deterministic: bool,
    /// 🚀 Hint: The last successful CRF value found during exploration (stored in cache)
    pub last_best_crf: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectedCodec {
    FFV1,
    H264,
    H265,
    VP9,
    AV1,
    AV2,
    VVC,
    ProRes,
    DNxHD,
    MJPEG,
    Uncompressed,
    HuffYUV,
    UTVideo,
    Unknown(String),
}

impl DetectedCodec {
    pub fn from_ffprobe(codec_name: &str) -> Self {
        match codec_name.to_lowercase().as_str() {
            "ffv1" => DetectedCodec::FFV1,
            "h264" | "avc" | "libx264" => DetectedCodec::H264,
            "hevc" | "h265" | "libx265" => DetectedCodec::H265,
            "vp9" | "libvpx-vp9" => DetectedCodec::VP9,
            "av1" | "libaom-av1" | "libsvtav1" => DetectedCodec::AV1,
            "av2" => DetectedCodec::AV2,
            "vvc" | "h266" => DetectedCodec::VVC,
            "prores" | "prores_ks" => DetectedCodec::ProRes,
            "dnxhd" | "dnxhr" => DetectedCodec::DNxHD,
            "mjpeg" | "mjpegb" => DetectedCodec::MJPEG,
            "rawvideo" => DetectedCodec::Uncompressed,
            "huffyuv" | "ffvhuff" => DetectedCodec::HuffYUV,
            "utvideo" => DetectedCodec::UTVideo,
            "vc1" | "wmv3" => DetectedCodec::Unknown("VC-1".to_string()),
            "dirac" => DetectedCodec::Unknown("Dirac".to_string()),
            "theora" => DetectedCodec::Unknown("Theora".to_string()),
            "vp8" | "libvpx" => DetectedCodec::Unknown("VP8".to_string()),
            _ => DetectedCodec::Unknown(codec_name.to_string()),
        }
    }

    pub fn is_lossless(&self) -> bool {
        matches!(
            self,
            DetectedCodec::FFV1
                | DetectedCodec::Uncompressed
                | DetectedCodec::HuffYUV
                | DetectedCodec::UTVideo
        )
    }

    pub fn can_be_lossless(&self) -> bool {
        matches!(
            self,
            DetectedCodec::FFV1
                | DetectedCodec::Uncompressed
                | DetectedCodec::HuffYUV
                | DetectedCodec::UTVideo
                | DetectedCodec::ProRes
                | DetectedCodec::DNxHD
        )
    }

    pub fn as_str(&self) -> &str {
        match self {
            DetectedCodec::FFV1 => "FFV1",
            DetectedCodec::H264 => "H.264",
            DetectedCodec::H265 => "H.265",
            DetectedCodec::VP9 => "VP9",
            DetectedCodec::AV1 => "AV1",
            DetectedCodec::AV2 => "AV2",
            DetectedCodec::VVC => "H.266/VVC",
            DetectedCodec::ProRes => "ProRes",
            DetectedCodec::DNxHD => "DNxHD/DNxHR",
            DetectedCodec::MJPEG => "MJPEG",
            DetectedCodec::Uncompressed => "Uncompressed",
            DetectedCodec::HuffYUV => "HuffYUV",
            DetectedCodec::UTVideo => "UTVideo",
            DetectedCodec::Unknown(s) => s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionType {
    Lossless,
    VisuallyLossless,
    HighQuality,
    Standard,
    LowQuality,
}

impl CompressionType {
    pub fn as_str(&self) -> &str {
        match self {
            CompressionType::Lossless => "Lossless",
            CompressionType::VisuallyLossless => "Visually Lossless",
            CompressionType::HighQuality => "High Quality",
            CompressionType::Standard => "Standard Quality",
            CompressionType::LowQuality => "Low Quality",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorSpace {
    BT709,
    BT2020,
    SRGB,
    AdobeRGB,
    Unknown(String),
}

impl ColorSpace {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bt709" => ColorSpace::BT709,
            "bt2020" | "bt2020nc" | "bt2020ncl" => ColorSpace::BT2020,
            "srgb" | "iec61966-2-1" => ColorSpace::SRGB,
            "adobergb" => ColorSpace::AdobeRGB,
            _ => ColorSpace::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            ColorSpace::BT709 => "bt709",
            ColorSpace::BT2020 => "bt2020",
            ColorSpace::SRGB => "srgb",
            ColorSpace::AdobeRGB => "adobergb",
            ColorSpace::Unknown(s) => s.as_str(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoDetectionResult {
    pub file_path: String,
    pub format: String,
    pub codec: DetectedCodec,
    pub codec_long: String,
    pub compression: CompressionType,
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
    pub fps: f64,
    pub duration_secs: f64,
    pub bit_depth: u8,
    pub pix_fmt: String,
    pub color_space: ColorSpace,
    pub bitrate: u64,
    pub has_audio: bool,
    pub audio_codec: Option<String>,
    pub file_size: u64,
    pub quality_score: u8,
    pub archival_candidate: bool,
    pub profile: Option<String>,
    pub max_b_frames: u8,
    pub has_b_frames: bool,
    pub encoder_params: Option<String>,
    pub video_bitrate: Option<u64>,
    pub bits_per_pixel: f64,
    /// color_primaries from ffprobe (e.g. "bt2020", "bt709")
    pub color_primaries: Option<String>,
    /// color_transfer (TRC) from ffprobe (e.g. "smpte2084", "arib-std-b67", "bt709")
    pub color_transfer: Option<String>,
    /// HDR10 mastering display metadata in ffmpeg format
    pub mastering_display: Option<String>,
    /// HDR10 content light level: "MaxCLL,MaxFALL"
    pub max_cll: Option<String>,
    /// Dolby Vision detected in stream side data
    pub is_dolby_vision: bool,
    /// Dolby Vision profile number (5, 7, 8, etc.) — None if not DV
    pub dv_profile: Option<u8>,
    /// Dolby Vision BL signal compatibility ID (used to determine cross-compat)
    pub dv_bl_signal_compatibility_id: Option<u8>,
    /// HDR10+ (SMPTE ST 2094-40) detected in stream side data
    pub is_hdr10_plus: bool,
    /// True when at least one subtitle stream is present
    pub has_subtitles: bool,
    /// Codec name of the first subtitle stream
    pub subtitle_codec: Option<String>,
    /// Number of audio channels (e.g. 2 for stereo, 6 for 5.1, 8 for 7.1/Atmos)
    pub audio_channels: Option<u32>,
    /// Variable frame rate (VFR) detected - common in iPhone slow-motion videos
    pub is_variable_frame_rate: bool,
    /// Precise metadata from encoder tags
    pub precision: VideoPrecisionMetadata,
    /// Raw tags from format section
    pub tags: HashMap<String, String>,
    /// 🛠️ New Dimension: Processing history for cache invalidation logic
    pub history: crate::types::ProcessHistory,
    /// 🔬 New Dimension: Visual perception data (Auxiliary analysis)
    pub perception: crate::types::VisualPerception,
}

impl VideoDetectionResult {
    /// Returns true when the content is any form of HDR (PQ, HLG, DV, HDR10, HDR10+)
    pub fn is_hdr(&self) -> bool {
        self.is_dolby_vision
            || self.is_hdr10_plus
            || self.mastering_display.is_some()
            || self.max_cll.is_some()
            || matches!(
                self.color_transfer.as_deref(),
                Some("smpte2084") | Some("arib-std-b67")
            )
    }

    /// Returns true for high-bitrate archival-grade content
    pub fn is_high_fidelity(&self) -> bool {
        self.bit_depth >= 10 && matches!(self.compression, CompressionType::Lossless | CompressionType::VisuallyLossless)
    }

    /// High-precision VFR detection including slow-motion recording analysis
    pub fn is_apple_slow_mo(&self) -> bool {
        self.tags.contains_key("com.apple.quicktime.fullframerate")
    }
}

pub fn determine_compression_type(
    codec: &DetectedCodec,
    bitrate: u64,
    width: u32,
    height: u32,
    fps: f64,
    precision: &VideoPrecisionMetadata,
) -> CompressionType {
    if codec.is_lossless() || precision.is_lossless_deterministic {
        return CompressionType::Lossless;
    }
    
    // HEVC/AV1 Lossless often uses specific profiles or encoder params
    if let Some(ref settings) = precision.original_encoder {
        if settings.contains("lossless=1") || settings.contains("qp=0") {
            return CompressionType::Lossless;
        }
    }

    // Use original CRF if available
    if let Some(crf) = precision.original_crf {
        if crf <= 15.0 {
            return CompressionType::VisuallyLossless;
        } else if crf <= 23.0 {
            return CompressionType::HighQuality;
        } else if crf <= 30.0 {
            return CompressionType::Standard;
        } else {
            return CompressionType::LowQuality;
        }
    }

    if matches!(codec, DetectedCodec::ProRes | DetectedCodec::DNxHD) {
        return CompressionType::VisuallyLossless;
    }

    // BPP (Bits Per Pixel) thresholding for generic streams
    let pixels_per_second = (width as f64) * (height as f64) * fps;
    if pixels_per_second > 0.0 {
        let bits_per_pixel = (bitrate as f64 * 8.0) / pixels_per_second;
        if bits_per_pixel > 2.0 {
            return CompressionType::VisuallyLossless;
        } else if bits_per_pixel > 0.5 {
            return CompressionType::HighQuality;
        } else if bits_per_pixel > 0.1 {
            return CompressionType::Standard;
        }
    }
    CompressionType::LowQuality
}

pub fn calculate_quality_score(
    compression: &CompressionType,
    bit_depth: u8,
    _bitrate: u64,
    width: u32,
    height: u32,
) -> u8 {
    let base_score = match compression {
        CompressionType::Lossless => 100,
        CompressionType::VisuallyLossless => 95,
        CompressionType::HighQuality => 80,
        CompressionType::Standard => 60,
        CompressionType::LowQuality => 40,
    };
    let depth_bonus = if bit_depth >= 10 { 5 } else { 0 };
    let res_bonus = if width >= 3840 || height >= 2160 {
        3
    } else {
        0
    };
    (base_score + depth_bonus + res_bonus).min(100)
}

/// Analyzes a video file with optional SQLite caching.
pub fn detect_video_with_cache(
    path: &Path,
    cache: Option<&crate::analysis_cache::AnalysisCache>,
) -> Result<VideoDetectionResult, FFprobeError> {
    if let Some(cache) = cache {
        if let Ok(Some(cached)) = cache.get_video_analysis(path) {
            if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                eprintln!("🔍 [Video Cache] Hit: {}", path.display());
            }
            return Ok(cached);
        }
    }

    let result = detect_video(path)?;

    if let Some(cache) = cache {
        let _ = cache.store_video_analysis(path, &result);
    }

    Ok(result)
}

pub fn detect_video(path: &Path) -> Result<VideoDetectionResult, FFprobeError> {
    let probe = probe_video(path)?;

    let codec = DetectedCodec::from_ffprobe(&probe.video_codec);

    let pixels_per_second = (probe.width as f64) * (probe.height as f64) * probe.frame_rate;
    let bits_per_pixel = if pixels_per_second > 0.0 {
        (probe.bit_rate as f64) / pixels_per_second
    } else {
        0.0
    };

    let precision = extract_video_precision(&probe.tags, probe.encoder_settings.as_deref(), probe.max_b_frames);

    let compression = determine_compression_type(
        &codec,
        probe.bit_rate,
        probe.width,
        probe.height,
        probe.frame_rate,
        &precision,
    );

    let color_space = probe
        .color_space
        .as_ref()
        .map(|s| ColorSpace::parse(s))
        .unwrap_or(ColorSpace::Unknown("unknown".to_string()));

    let quality_score = calculate_quality_score(
        &compression,
        probe.bit_depth,
        probe.bit_rate,
        probe.width,
        probe.height,
    );

    let archival_candidate = matches!(
        compression,
        CompressionType::Lossless | CompressionType::VisuallyLossless
    ) || codec.can_be_lossless();

    Ok(VideoDetectionResult {
        file_path: path.display().to_string(),
        format: probe.format_name,
        codec,
        codec_long: probe.video_codec_long,
        compression,
        width: probe.width,
        height: probe.height,
        frame_count: probe.frame_count,
        fps: probe.frame_rate,
        duration_secs: probe.duration,
        bit_depth: probe.bit_depth,
        pix_fmt: probe.pix_fmt,
        color_space,
        bitrate: probe.bit_rate,
        has_audio: probe.has_audio,
        audio_codec: probe.audio_codec,
        file_size: probe.size,
        quality_score,
        archival_candidate,
        profile: probe.profile,
        max_b_frames: probe.max_b_frames,
        has_b_frames: probe.has_b_frames,
        encoder_params: probe.encoder_settings.clone(),
        video_bitrate: probe.video_bit_rate,
        bits_per_pixel,
        color_primaries: probe.color_primaries,
        color_transfer: probe.color_transfer,
        mastering_display: probe.mastering_display,
        max_cll: probe.max_cll,
        is_dolby_vision: probe.is_dolby_vision,
        dv_profile: probe.dv_profile,
        dv_bl_signal_compatibility_id: probe.dv_bl_signal_compatibility_id,
        is_hdr10_plus: probe.is_hdr10_plus,
        has_subtitles: probe.has_subtitles,
        subtitle_codec: probe.subtitle_codec,
        audio_channels: probe.audio_channels,
        is_variable_frame_rate: probe.is_variable_frame_rate,
        precision,
        tags: probe.tags,
        history: crate::common_utils::get_current_history(),
        perception: Default::default(),
    })
}

fn extract_video_precision(tags: &HashMap<String, String>, encoder_settings: Option<&str>, max_b_frames: u8) -> VideoPrecisionMetadata {
    let mut precision = VideoPrecisionMetadata::default();
    precision.original_max_b_frames = Some(max_b_frames);
    
    if let Some(encoder) = tags.get("encoder") {
        precision.original_encoder = Some(encoder.clone());
    }

    // Prioritize explicit encoder_settings (x264-params/x265-params) over generic tags
    let search_string = if let Some(settings) = encoder_settings {
        settings.to_string()
    } else if let Some(comment) = tags.get("comment") {
        comment.clone()
    } else {
        String::new()
    };

    if !search_string.is_empty() {
        let lower = search_string.to_lowercase();
        
        // Extract CRF
        if let Some(crf_pos) = lower.find("crf=") {
            let sub = &lower[crf_pos + 4..];
            let val: String = sub.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Ok(v) = val.parse::<f32>() {
                precision.original_crf = Some(v);
            }
        } else if let Some(qp_pos) = lower.find("qp=") {
            let sub = &lower[qp_pos + 4..];
            let val: String = sub.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Ok(v) = val.parse::<f32>() {
                precision.original_crf = Some(v);
            }
        }

        // Extract Preset
        if let Some(preset_pos) = lower.find("preset=") {
            let sub = &lower[preset_pos + 7..];
            let val: String = sub.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if !val.is_empty() {
                precision.original_preset = Some(val);
            }
        }
    }
    
    precision
}
