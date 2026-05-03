//! Video Detection API Module (Shared)
//!
//! Pure analysis layer - detects video properties using ffprobe.
//! Determines codec type, compression level, and archival suitability.
//!
//! Migrated from `vid_hevc/vid_av1` `detection_api.rs` to eliminate duplication.

use crate::ffprobe::{probe_video, FFprobeError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VideoPrecisionMetadata {
    pub original_crf: Option<f32>,
    pub original_preset: Option<String>,
    pub original_encoder: Option<String>,
    pub original_max_b_frames: Option<u8>,
    pub is_lossless_deterministic: bool,
    /// 🚀 Hint: The last successful CRF value found during exploration (stored in cache)
    pub last_best_crf: Option<f32>,
    /// 🚀 Hint: The last kept best-effort CRF value when exploration produced a usable
    /// output but did not fully satisfy the quality target.
    pub last_best_effort_crf: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectedCodec {
    Unknown(String),
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
}

impl Default for DetectedCodec {
    fn default() -> Self {
        Self::Unknown(String::new())
    }
}

impl DetectedCodec {
    #[must_use]
    pub fn from_ffprobe(codec_name: &str) -> Self {
        match codec_name.to_lowercase().as_str() {
            "ffv1" => Self::FFV1,
            "h264" | "avc" | "libx264" => Self::H264,
            "hevc" | "h265" | "libx265" => Self::H265,
            "vp9" | "libvpx-vp9" => Self::VP9,
            "av1" | "libaom-av1" | "libsvtav1" => Self::AV1,
            "av2" => Self::AV2,
            "vvc" | "h266" => Self::VVC,
            "prores" | "prores_ks" => Self::ProRes,
            "dnxhd" | "dnxhr" => Self::DNxHD,
            "mjpeg" | "mjpegb" => Self::MJPEG,
            "rawvideo" => Self::Uncompressed,
            "huffyuv" | "ffvhuff" => Self::HuffYUV,
            "utvideo" => Self::UTVideo,
            "vc1" | "wmv3" => Self::Unknown("VC-1".to_string()),
            "dirac" => Self::Unknown("Dirac".to_string()),
            "theora" => Self::Unknown("Theora".to_string()),
            "vp8" | "libvpx" => Self::Unknown("VP8".to_string()),
            _ => Self::Unknown(codec_name.to_string()),
        }
    }

    #[must_use]
    pub const fn is_lossless(&self) -> bool {
        matches!(
            self,
            Self::FFV1 | Self::Uncompressed | Self::HuffYUV | Self::UTVideo
        )
    }

    #[must_use]
    pub const fn can_be_lossless(&self) -> bool {
        matches!(
            self,
            Self::FFV1
                | Self::Uncompressed
                | Self::HuffYUV
                | Self::UTVideo
                | Self::ProRes
                | Self::DNxHD
        )
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::FFV1 => "FFV1",
            Self::H264 => "H.264",
            Self::H265 => "H.265",
            Self::VP9 => "VP9",
            Self::AV1 => "AV1",
            Self::AV2 => "AV2",
            Self::VVC => "H.266/VVC",
            Self::ProRes => "ProRes",
            Self::DNxHD => "DNxHD/DNxHR",
            Self::MJPEG => "MJPEG",
            Self::Uncompressed => "Uncompressed",
            Self::HuffYUV => "HuffYUV",
            Self::UTVideo => "UTVideo",
            Self::Unknown(s) => s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompressionType {
    Lossless,
    VisuallyLossless,
    HighQuality,
    #[default]
    Standard,
    LowQuality,
}

impl CompressionType {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Lossless => "Lossless",
            Self::VisuallyLossless => "Visually Lossless",
            Self::HighQuality => "High Quality",
            Self::Standard => "Standard Quality",
            Self::LowQuality => "Low Quality",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorSpace {
    Unknown(String),
    BT709,
    BT2020,
    SRGB,
    AdobeRGB,
}

impl Default for ColorSpace {
    fn default() -> Self {
        Self::Unknown(String::new())
    }
}

impl ColorSpace {
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bt709" => Self::BT709,
            "bt2020" | "bt2020nc" | "bt2020ncl" => Self::BT2020,
            "srgb" | "iec61966-2-1" => Self::SRGB,
            "adobergb" => Self::AdobeRGB,
            _ => Self::Unknown(s.to_string()),
        }
    }

    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::BT709 => "bt709",
            Self::BT2020 => "bt2020",
            Self::SRGB => "srgb",
            Self::AdobeRGB => "adobergb",
            Self::Unknown(s) => s.as_str(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
// Rationale: This struct serves as a comprehensive configuration or state container where individual boolean flags are the most idiomatic and explicit way to represent discrete options.
#[allow(clippy::struct_excessive_bools)]
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
    /// `color_primaries` from ffprobe (e.g. "bt2020", "bt709")
    pub color_primaries: Option<String>,
    /// `color_transfer` (TRC) from ffprobe (e.g. "smpte2084", "arib-std-b67", "bt709")
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
    /// Optional: Loop count from metadata (0 = infinite).
    pub loop_count: Option<u16>,
    /// 🎞️ Frame types (I, P, B) for the initial sample.
    pub frame_types: Vec<char>,
    /// 🎞️ PTS deltas (frame intervals) for the initial sample.
    pub pts_deltas: Vec<f64>,
    /// 🎞️ Motion vector magnitudes (if available).
    pub mv_magnitudes: Vec<f64>,
    /// 🎞️ Packet sizes (in bytes) for bitrate analysis.
    pub pkt_sizes: Vec<u64>,
    /// 📺 Whether the video is physically interlaced (penetration detection).
    pub is_interlaced: Option<bool>,
}

impl VideoDetectionResult {
    /// Returns true when the content is any form of HDR (PQ, HLG, DV, HDR10, HDR10+)
    #[must_use]
    pub fn is_hdr(&self) -> bool {
        self.is_dolby_vision
            || self.is_hdr10_plus
            || self.mastering_display.is_some()
            || self.max_cll.is_some()
            || matches!(
                self.color_transfer.as_deref(),
                Some("smpte2084" | "arib-std-b67")
            )
    }

    /// Returns true for high-bitrate archival-grade content
    #[must_use]
    pub const fn is_high_fidelity(&self) -> bool {
        self.bit_depth >= 10
            && matches!(
                self.compression,
                CompressionType::Lossless | CompressionType::VisuallyLossless
            )
    }

    /// High-precision VFR detection including slow-motion recording analysis
    #[must_use]
    pub fn is_apple_slow_mo(&self) -> bool {
        self.tags.contains_key("com.apple.quicktime.fullframerate")
    }
}

#[must_use]
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
        if crf <= crate::constants::CRF_THRESHOLD_VISUALLY_LOSSLESS {
            return CompressionType::VisuallyLossless;
        } else if crf <= crate::constants::CRF_THRESHOLD_HIGH_QUALITY {
            return CompressionType::HighQuality;
        } else if crf <= crate::constants::CRF_THRESHOLD_STANDARD {
            return CompressionType::Standard;
        }
        return CompressionType::LowQuality;
    }

    if matches!(codec, DetectedCodec::ProRes | DetectedCodec::DNxHD) {
        return CompressionType::VisuallyLossless;
    }

    // BPP (Bits Per Pixel) thresholding for generic streams
    let pixels_per_second = f64::from(width) * f64::from(height) * fps;
    if pixels_per_second > 0.0 {
        let bits_per_pixel = (crate::numeric_cast::u64_to_f64(bitrate) * 8.0) / pixels_per_second;
        if bits_per_pixel > crate::constants::BPP_THRESHOLD_VISUALLY_LOSSLESS {
            return CompressionType::VisuallyLossless;
        } else if bits_per_pixel > crate::constants::BPP_THRESHOLD_HIGH_QUALITY {
            return CompressionType::HighQuality;
        } else if bits_per_pixel > crate::constants::BPP_THRESHOLD_STANDARD {
            return CompressionType::Standard;
        }
    }
    CompressionType::LowQuality
}

#[must_use]
pub fn calculate_quality_score(
    compression: &CompressionType,
    bit_depth: u8,
    _bitrate: u64,
    width: u32,
    height: u32,
) -> u8 {
    let base_score: u8 = match compression {
        CompressionType::Lossless => 100,
        CompressionType::VisuallyLossless => 95,
        CompressionType::HighQuality => 80,
        CompressionType::Standard => 60,
        CompressionType::LowQuality => 40,
    };
    let depth_bonus = if bit_depth
        >= crate::numeric_cast::u32_to_u8_sat(crate::constants::HDR_BIT_DEPTH_THRESHOLD)
    {
        crate::numeric_cast::u32_to_u8_sat(crate::constants::HDR_QUALITY_BONUS)
    } else {
        0
    };
    let res_bonus =
        if width >= crate::constants::WIDTH_UHD_4K || height >= crate::constants::HEIGHT_UHD_4K {
            3
        } else {
            0
        };
    base_score
        .saturating_add(depth_bonus)
        .saturating_add(res_bonus)
        .min(100)
}

/// Analyzes a video file with optional `SQLite` caching.
///
/// # Errors
/// Returns an error if the file cannot be read, ffprobe fails, or cache access errors.
pub fn detect_video_with_cache(
    path: &Path,
    cache: Option<&crate::analysis_cache::AnalysisCache>,
) -> Result<VideoDetectionResult, FFprobeError> {
    let should_refresh_cached_result = |cached: &VideoDetectionResult| -> bool {
        if cached.frame_count > 1 {
            return false;
        }

        // Root fix: invalidate stale WebP cache entries produced by old ffprobe-only logic.
        // Some animated WebP files were previously cached as single-frame static.
        let Ok(format) = crate::image_detection::detect_format_from_bytes(path) else {
            return false;
        };
        if !matches!(format, crate::image_detection::DetectedFormat::WebP) {
            return false;
        }
        let Ok((is_animated, native_frames, _)) =
            crate::image_detection::detect_animation(path, &format)
        else {
            return false;
        };
        is_animated && native_frames > 1
    };

    if let Some(cache) = cache {
        match cache.get_video_analysis(path) {
            Ok(Some(mut cached)) => {
                if should_refresh_cached_result(&cached) {
                    tracing::warn!(
                        path = %path.display(),
                        cached_frames = cached.frame_count,
                        "Invalidating stale cached WebP frame metadata and re-running detection"
                    );
                } else {
                    if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                        eprintln!("🔍 [Video Cache] Hit: {}", path.display());
                    }
                    cached.file_path = path.display().to_string();
                    return Ok(cached);
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "Failed to load cached video analysis"
                );
            }
        }
    }

    let result = detect_video(path)?;

    if let Some(cache) = cache {
        if let Err(err) = cache.store_video_analysis(path, &result) {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "Failed to store video analysis in cache"
            );
        }
    }

    Ok(result)
}

/// Detect video properties using ffprobe.
///
/// # Errors
/// Returns `FFprobeError` if the file is invalid or ffprobe fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
pub fn detect_video(path: &Path) -> Result<VideoDetectionResult, FFprobeError> {
    let probe = probe_video(path)?;

    let codec = DetectedCodec::from_ffprobe(&probe.video_codec);
    let has_b_frames = probe.has_b_frames();

    let pixels_per_second = f64::from(probe.width) * f64::from(probe.height) * probe.frame_rate;
    let bits_per_pixel = if pixels_per_second > 0.0 {
        crate::numeric_cast::u64_to_f64(probe.bit_rate) / pixels_per_second
    } else {
        0.0
    };

    let precision = extract_video_precision(
        &probe.tags,
        probe.encoder_settings.as_deref(),
        probe.max_b_frames,
    );

    let compression = determine_compression_type(
        &codec,
        probe.bit_rate,
        probe.width,
        probe.height,
        probe.frame_rate,
        &precision,
    );

    let color_space = probe.color_space.as_ref().map_or_else(
        || ColorSpace::Unknown("unknown".to_string()),
        |s| ColorSpace::parse(s),
    );

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

    let mut result = VideoDetectionResult {
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
        has_audio: probe.audio.present,
        audio_codec: probe.audio.codec.clone(),
        file_size: probe.size,
        quality_score,
        archival_candidate,
        profile: probe.profile,
        max_b_frames: probe.max_b_frames,
        has_b_frames,
        encoder_params: probe.encoder_settings.clone(),
        video_bitrate: probe.video_bit_rate,
        bits_per_pixel,
        color_primaries: probe.color_primaries,
        color_transfer: probe.color_transfer,
        mastering_display: probe.hdr.mastering_display.clone(),
        max_cll: probe.hdr.max_cll.clone(),
        is_dolby_vision: probe.hdr.is_dolby_vision(),
        dv_profile: probe.hdr.dv_profile(),
        dv_bl_signal_compatibility_id: probe.hdr.dv_bl_signal_compatibility_id(),
        is_hdr10_plus: probe.hdr.hdr10_plus,
        has_subtitles: probe.subtitles.present,
        subtitle_codec: probe.subtitles.codec.clone(),
        audio_channels: probe.audio.channels,
        is_variable_frame_rate: probe.is_variable_frame_rate,
        precision,
        tags: probe.tags,
        history: crate::common_utils::get_current_history(),
        perception: crate::types::VisualPerception::default(),
        loop_count: probe.loop_count,
        frame_types: probe.frame_types,
        pts_deltas: probe.pts_deltas,
        mv_magnitudes: probe.mv_magnitudes,
        pkt_sizes: probe.pkt_sizes,
        is_interlaced: None,
    };

    // ── Penetrating Content Verification ──
    // Verify critical metadata claims by decoding actual content
    if result.has_audio {
        if let crate::media_penetration::PenetrationResult::Verified(is_silent) =
            crate::media_penetration::detect_audio_silence(path)
        {
            if is_silent {
                crate::progress_mode::emit_stderr(&format!(
                    "🔊 [{}] Audio penetration: SILENT track detected, treating as no audio",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
                result.has_audio = false;
            }
        }
    }

    let has_transparency = result.pix_fmt.contains('a')
        || result.pix_fmt.contains("yuva")
        || result.pix_fmt.contains("gbrap");
    if has_transparency {
        if let crate::media_penetration::PenetrationResult::Verified(is_real) =
            crate::media_penetration::detect_real_transparency(path, Some(result.duration_secs))
        {
            if !is_real {
                crate::progress_mode::emit_stderr(&format!(
                    "⚠️  [{}] Transparency penetration: FAKE alpha channel (unused)",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
            }
        }
    }

    if result.frame_count <= 1 || result.frame_count > 50000 {
        if let crate::media_penetration::PenetrationResult::Verified(real_count) =
            crate::media_penetration::detect_real_frame_count(path, result.frame_count)
        {
            if real_count != result.frame_count {
                crate::progress_mode::emit_stderr(&format!(
                    "⚠️  [{}] Frame count mismatch: metadata={}, actual={}, correcting",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                    result.frame_count,
                    real_count
                ));
                result.frame_count = real_count;
            }
        }
    }

    // Interlace detection is expensive, so we only run it for "gray zone" assets (4s to 18s)
    // where loop intent might be ambiguous, and only if it's not a native gif/webp.
    if result.duration_secs >= 4.0
        && result.duration_secs <= 18.0
        && result.format != "gif"
        && result.format != "webp"
    {
        if let crate::media_penetration::PenetrationResult::Verified(is_interlaced) =
            crate::media_penetration::detect_interlacing(path)
        {
            result.is_interlaced = Some(is_interlaced);
        }
    }

    Ok(result)
}

fn extract_video_precision(
    tags: &HashMap<String, String>,
    encoder_settings: Option<&str>,
    max_b_frames: u8,
) -> VideoPrecisionMetadata {
    let mut precision = VideoPrecisionMetadata {
        original_max_b_frames: Some(max_b_frames),
        original_encoder: tags.get("encoder").cloned(),
        ..Default::default()
    };

    // Prioritize explicit encoder_settings (x264-params/x265-params) over generic tags
    let search_string = encoder_settings.map_or_else(
        || tags.get("comment").cloned().unwrap_or_default(),
        std::string::ToString::to_string,
    );

    if !search_string.is_empty() {
        let lower = search_string.to_lowercase();

        // Extract CRF
        if let Some(crf_pos) = lower.find("crf=") {
            let sub = &lower[crf_pos + 4..];
            let val: String = sub
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = val.parse::<f32>() {
                precision.original_crf = Some(v);
            }
        } else if let Some(qp_pos) = lower.find("qp=") {
            let sub = &lower[qp_pos + 4..];
            let val: String = sub
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = val.parse::<f32>() {
                precision.original_crf = Some(v);
            }
        }

        // Extract Preset
        if let Some(preset_pos) = lower.find("preset=") {
            let sub = &lower[preset_pos + 7..];
            let val: String = sub
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            if !val.is_empty() {
                precision.original_preset = Some(val);
            }
        }
    }

    precision
}
