//! Video Detection API Module (Shared)
//!
//! Pure analysis layer - detects video properties using ffprobe.
//! Determines codec type, compression level, and archival suitability.
//!
//! Migrated from `vid_hevc/vid_av1` `detection_api.rs` to eliminate
//! duplication.

use crate::ffprobe::{FFprobeError, probe_video};
use crate::media_index_types::MediaIndexRow;
use crate::media_precision::{BitDepthMetadata, MediaPrecision};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

const WEBP_HEADER_READ_CAP: usize = 1024 * 1024;
const GIF_HEADER_READ_CAP: usize = 1024 * 1024;
const PNG_HEADER_READ_CAP: usize = 1024 * 1024;

fn header_label_matches_true_format(
    label: &str,
    format: crate::image::format_detect::FormatKind,
) -> bool {
    matches!(
        (label, format),
        ("webp", crate::image::format_detect::FormatKind::WebP)
            | ("gif", crate::image::format_detect::FormatKind::Gif)
            | ("png" | "apng", crate::image::format_detect::FormatKind::Png)
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VideoPrecisionMetadata {
    pub original_crf: Option<f32>,
    pub original_preset: Option<String>,
    pub original_encoder: Option<String>,
    pub original_max_b_frames: Option<u8>,
    /// True when the detected bit depth came from `pix_fmt` inference rather
    /// than an explicit ffprobe sample-depth field.
    pub bit_depth_inferred_from_pix_fmt: bool,
    pub is_lossless_deterministic: bool,
    /// 🚀 Hint: The last successful CRF value found during exploration (stored
    /// in cache)
    pub last_best_crf: Option<f32>,
    /// 🚀 Hint: The last kept best-effort CRF value when exploration produced a
    /// usable output but did not fully satisfy the quality target.
    pub last_best_effort_crf: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoRecommendation {
    pub current_codec: String,
    pub recommended_codec: String,
    pub reason: String,
    pub is_archival_upgrade: bool,
    pub command_hint: String,
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
    VP8,
    MPEG4,
    MPEG2,
    MPEG1,
    VC1,
    Theora,
    Dirac,
    MagicYUV,
    Lagarith,
    QTRLE,
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
            "magicyuv" => Self::MagicYUV,
            "lagarith" => Self::Lagarith,
            "qtrle" => Self::QTRLE,
            "vp8" | "libvpx" => Self::VP8,
            "mpeg4" | "xvid" | "divx" => Self::MPEG4,
            "mpeg2video" | "mpeg2" => Self::MPEG2,
            "mpeg1video" | "mpeg1" => Self::MPEG1,
            "vc1" | "wmv3" => Self::VC1,
            "theora" | "libtheora" => Self::Theora,
            "dirac" => Self::Dirac,
            _ => Self::Unknown(codec_name.to_string()),
        }
    }

    #[must_use]
    pub const fn is_lossless(&self) -> bool {
        matches!(
            self,
            Self::FFV1
                | Self::Uncompressed
                | Self::HuffYUV
                | Self::UTVideo
                | Self::MagicYUV
                | Self::Lagarith
                | Self::QTRLE
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
            Self::VP8 => "VP8",
            Self::MPEG4 => "MPEG-4",
            Self::MPEG2 => "MPEG-2",
            Self::MPEG1 => "MPEG-1",
            Self::VC1 => "VC-1",
            Self::Theora => "Theora",
            Self::Dirac => "Dirac",
            Self::MagicYUV => "MagicYUV",
            Self::Lagarith => "Lagarith",
            Self::QTRLE => "QuickTime Animation (RLE)",
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

    #[must_use]
    pub fn yuv_output_colorspace(&self) -> Option<&str> {
        match self {
            Self::BT709 => Some(crate::constants::CS_BT709),
            Self::BT2020 => Some(crate::constants::CS_BT2020),
            Self::Unknown(s) if !s.is_empty() && s != crate::constants::STR_UNKNOWN => {
                Some(s.as_str())
            }
            Self::SRGB | Self::AdobeRGB | Self::Unknown(_) => None,
        }
    }

    #[must_use]
    pub const fn quality_matcher_color_profile(&self) -> Option<(&'static str, bool)> {
        match self {
            Self::BT709 => Some((crate::constants::CS_BT709, false)),
            Self::BT2020 => Some((crate::constants::CS_BT2020, true)),
            Self::SRGB => Some((crate::constants::CS_SRGB, false)),
            Self::AdobeRGB => Some((crate::constants::CS_ADOBE_RGB, false)),
            Self::Unknown(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct VideoStreamFlags {
    pub has_audio: bool,
    pub has_subtitles: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoContentFlags {
    pub archival_candidate: bool,
    pub has_b_frames: bool,
    pub is_variable_frame_rate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoHdrFlags {
    pub is_dolby_vision: bool,
    pub is_hdr10_plus: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoFlags {
    #[serde(flatten)]
    pub streams: VideoStreamFlags,
    #[serde(flatten)]
    pub content: VideoContentFlags,
    #[serde(flatten)]
    pub hdr: VideoHdrFlags,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Detection {
    pub file_path: String,
    pub format: String,
    pub codec: DetectedCodec,
    pub codec_long: String,
    pub compression: CompressionType,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_count: Option<u64>,
    pub fps: Option<f64>,
    pub duration_secs: Option<f64>,
    pub bit_depth: Option<u8>,
    pub pix_fmt: String,
    pub color_space: ColorSpace,
    pub bitrate: Option<u64>,
    pub audio_duration_secs: Option<f64>,
    #[serde(flatten)]
    pub flags: VideoFlags,
    pub audio_codec: Option<String>,
    pub file_size: u64,
    pub quality_score: u8,
    pub profile: Option<String>,
    pub max_b_frames: Option<u8>,
    pub encoder_params: Option<String>,
    pub video_bitrate: Option<u64>,
    pub bits_per_pixel: f64,
    /// `color_primaries` from ffprobe (e.g. "bt2020", "bt709")
    pub color_primaries: Option<String>,
    /// `color_transfer` (TRC) from ffprobe (e.g. "smpte2084", "arib-std-b67",
    /// "bt709")
    pub color_transfer: Option<String>,
    /// HDR10 mastering display metadata in ffmpeg format
    pub mastering_display: Option<String>,
    /// HDR10 content light level: "MaxCLL,MaxFALL"
    pub max_cll: Option<String>,
    pub dv_profile: Option<u8>,
    /// Dolby Vision BL signal compatibility ID (used to determine cross-compat)
    pub dv_bl_signal_compatibility_id: Option<u8>,
    /// Codec name of the first subtitle stream
    pub subtitle_codec: Option<String>,
    /// Number of audio channels (e.g. 2 for stereo, 6 for 5.1, 8 for 7.1/Atmos)
    pub audio_channels: Option<u32>,
    /// Precise metadata from encoder tags
    pub precision: VideoPrecisionMetadata,
    /// Raw tags from format section
    pub tags: HashMap<String, String>,
    /// 🛠️ New Dimension: Processing history for cache invalidation logic
    pub history: crate::types::ProcessHistory,
    /// 🔬 New Dimension: Visual perception data (Auxiliary analysis)
    pub perception: crate::types::Visual,
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

impl Detection {
    #[must_use]
    pub fn color_assessment(&self) -> crate::ffprobe_json::ColorInfoAssessment {
        crate::ffprobe_json::ColorInfoAssessment::from_probe_fields(
            Some(self.color_space.as_str()),
            self.color_transfer.as_deref(),
            self.color_primaries.as_deref(),
            BitDepthMetadata::new(
                self.bit_depth,
                self.precision.bit_depth_inferred_from_pix_fmt,
            ),
            crate::ffprobe_json::ColorProbeFlags {
                has_mastering_display: self.mastering_display.is_some(),
                has_max_cll: self.max_cll.is_some(),
                is_dolby_vision: self.flags.hdr.is_dolby_vision,
                is_hdr10_plus: self.flags.hdr.is_hdr10_plus,
                is_float: crate::ffprobe_json::pix_fmt_indicates_float(Some(&self.pix_fmt)),
            },
        )
    }

    /// Returns true when the content is any form of HDR (PQ, HLG, DV, HDR10,
    /// HDR10+)
    #[must_use]
    pub fn is_hdr(&self) -> bool {
        self.color_assessment().has_hdr_signaling()
    }

    /// Returns true for high-bitrate archival-grade content
    #[must_use]
    pub fn is_high_fidelity(&self) -> bool {
        self.has_confirmed_high_bit_depth()
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

impl MediaPrecision for Detection {
    fn bit_depth_metadata(&self) -> BitDepthMetadata {
        BitDepthMetadata::new(
            self.bit_depth,
            self.precision.bit_depth_inferred_from_pix_fmt,
        )
    }

    fn has_hdr_signaling(&self) -> bool {
        self.color_assessment().has_hdr_signaling()
    }
}

#[must_use]
pub fn determine_compression_type(
    codec: &DetectedCodec,
    bitrate: Option<u64>,
    width: u32,
    height: u32,
    fps: Option<f64>,
    precision: &VideoPrecisionMetadata,
) -> CompressionType {
    if codec.is_lossless() || precision.is_lossless_deterministic {
        return CompressionType::Lossless;
    }

    // HEVC/AV1 Lossless often uses specific profiles or encoder params
    if let Some(ref settings) = precision.original_encoder
        && (settings.contains("lossless=1") || settings.contains("qp=0"))
    {
        return CompressionType::Lossless;
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
    if let Some(fps_val) = fps {
        let pixels_per_second = f64::from(width) * f64::from(height) * fps_val;
        if pixels_per_second > 0.0_f64
            && let Some(bitrate_val) = bitrate
        {
            let bits_per_pixel =
                (crate::numeric_cast::u64_to_f64(bitrate_val) * 8.0_f64) / pixels_per_second;
            if bits_per_pixel > crate::constants::BPP_THRESHOLD_VISUALLY_LOSSLESS {
                return CompressionType::VisuallyLossless;
            } else if bits_per_pixel > crate::constants::BPP_THRESHOLD_HIGH_QUALITY {
                return CompressionType::HighQuality;
            } else if bits_per_pixel > crate::constants::BPP_THRESHOLD_STANDARD {
                return CompressionType::Standard;
            }
        }
    }
    CompressionType::LowQuality
}

#[must_use]
pub fn calculate_quality_score(
    compression: &CompressionType,
    bit_depth: Option<u8>,
    bit_depth_inferred_from_pix_fmt: bool,
    _bitrate: Option<u64>,
    width: u32,
    height: u32,
) -> u8 {
    let base_score: u8 = match compression {
        CompressionType::Lossless => crate::constants::VIDEO_QUALITY_SCORE_LOSSLESS,
        CompressionType::VisuallyLossless => {
            crate::constants::VIDEO_QUALITY_SCORE_VISUALLY_LOSSLESS
        }
        CompressionType::HighQuality => crate::constants::VIDEO_QUALITY_SCORE_HIGH,
        CompressionType::Standard => crate::constants::VIDEO_QUALITY_SCORE_STANDARD,
        CompressionType::LowQuality => crate::constants::VIDEO_QUALITY_SCORE_LOW,
    };
    let depth_bonus = if BitDepthMetadata::new(bit_depth, bit_depth_inferred_from_pix_fmt)
        .has_confirmed_high_bit_depth()
    {
        crate::constants::HDR_QUALITY_BONUS
    } else {
        0
    };
    let res_bonus =
        if width >= crate::constants::WIDTH_UHD_4K || height >= crate::constants::HEIGHT_UHD_4K {
            crate::constants::VIDEO_QUALITY_RESOLUTION_BONUS_UHD
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
/// Returns an error if the file cannot be read, ffprobe fails, or cache access
/// errors.
pub fn detect_video_with_cache(
    path: &Path,
    cache: Option<&crate::analysis_cache::AnalysisCache>,
) -> std::result::Result<Detection, FFprobeError> {
    let should_refresh_cached_result = |cached: &Detection| -> bool {
        if cached.frame_count.is_some_and(|fc| fc > 1)
            && cached.width.is_some_and(|w| w > 0)
            && cached.height.is_some_and(|h| h > 0)
        {
            return false;
        }

        // Root fix: invalidate stale negative cache entries produced by older
        // ffprobe-only logic for animation-capable image formats.
        let format = match crate::image_detection::detect_format_from_bytes(path) {
            Ok(format) => format,
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "video_cache_refresh_format_detect_failed",
                    path,
                    format!(
                        "cache refresh refused stale-metadata guess after format detection error: \
                         {err}"
                    ),
                );
                return true;
            }
        };
        if !matches!(
            format,
            crate::image_detection::DetectedFormat::GIF
                | crate::image_detection::DetectedFormat::WebP
                | crate::image_detection::DetectedFormat::PNG
                | crate::image_detection::DetectedFormat::AVIF
                | crate::image_detection::DetectedFormat::JXL
                | crate::image_detection::DetectedFormat::HEIC
                | crate::image_detection::DetectedFormat::HEIF
        ) {
            return false;
        }
        let (is_animated, native_frames, _) =
            match crate::image_detection::detect_animation(path, &format) {
                Ok(value) => value,
                Err(err) => {
                    crate::media_conversion_gate::probe_layer_audit(
                        "video_cache_refresh_animation_detect_failed",
                        path,
                        format!(
                            "cache refresh refused stale-metadata guess after animation detection \
                             error: {err}"
                        ),
                    );
                    return true;
                }
            };
        is_animated && native_frames.is_some_and(|nf| nf > 1)
    };

    if let Some(cache) = cache {
        match cache.get_video_analysis(path) {
            Ok(Some(mut cached)) => {
                if should_refresh_cached_result(&cached) {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_DETECTION,
                        &format!(
                            "Invalidating stale cached animation-capable frame metadata and \
                             re-running detection (path={})",
                            path.display()
                        )
                    );
                } else {
                    let mut cache_repair_failed = false;
                    if cached_detection_needs_bitstream_repair(&cached, path) {
                        let prior_fc = cached.frame_count;
                        match repair_animated_container_detection_from_bitstream_header(
                            path,
                            &mut cached,
                        ) {
                            Ok(()) => {
                                crate::media_conversion_gate::probe_layer_audit(
                                    "video_cache_bitstream_repair",
                                    path,
                                    format!(
                                        "cached detection repaired from bitstream (frame_count {} → {})",
                                        crate::media_conversion_gate::delivery_frame_count_label_u64(
                                            prior_fc,
                                            &format!("cache repair before {}", path.display()),
                                        ),
                                        crate::media_conversion_gate::delivery_frame_count_label_u64(
                                            cached.frame_count,
                                            &format!("cache repair after {}", path.display()),
                                        ),
                                    ),
                                );
                                if let Err(err) = cache.store_video_analysis(path, &cached) {
                                    crate::media_conversion_gate::video_cache_store_failed_audit(
                                        path,
                                        "cache-repair-persist",
                                        err,
                                    );
                                }
                            }
                            Err(err) => {
                                cache_repair_failed = true;
                                crate::media_conversion_gate::probe_layer_audit(
                                    "video_cache_bitstream_repair_failed",
                                    path,
                                    format!(
                                        "cached detection bitstream repair failed; re-running \
                                         detection: {err}"
                                    ),
                                );
                            }
                        }
                    }
                    if cache_repair_failed || should_refresh_cached_result(&cached) {
                        crate::media_conversion_gate::probe_layer_audit(
                            "video_cache_repair_incomplete",
                            path,
                            "bitstream repair on cache hit did not yield multi-frame + valid \
                             canvas; re-running detection",
                        );
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_DETECTION,
                            &format!(
                                "Cache bitstream repair incomplete; re-running detection (path={})",
                                path.display()
                            )
                        );
                    } else {
                        if std::env::var(crate::constants::ENV_DEBUG).is_ok() {
                            crate::progress_mode::emit_stderr(&format!(
                                "🔍 [Video Cache] Hit: {}",
                                path.display()
                            ));
                        }
                        cached.file_path = path.display().to_string();
                        apply_video_quality_db_fusion(&mut cached, path);
                        return Ok(cached);
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                crate::media_conversion_gate::video_cache_load_failed_audit(path, err);
            }
        }
    }

    let mut result = detect_video_impl(path)?;

    if let Some(cache) = cache
        && let Err(err) = cache.store_video_analysis(path, &result)
    {
        crate::media_conversion_gate::video_cache_store_failed_audit(path, "detect-store", err);
    }

    apply_video_quality_db_fusion(&mut result, path);
    Ok(result)
}

/// True when cached detection may be stale ffprobe-only metadata for an
/// animation-capable file (M128).
fn cached_detection_needs_bitstream_repair(cached: &Detection, path: &Path) -> bool {
    let canvas_ok = cached.width.is_some_and(|w| w > 0) && cached.height.is_some_and(|h| h > 0);
    let frames_ok = cached.frame_count.is_some_and(|fc| fc > 1);
    if canvas_ok && frames_ok {
        return false;
    }
    let format = match crate::image_detection::detect_format_from_bytes(path) {
        Ok(format) => format,
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "video_cache_bitstream_format_detect_failed",
                path,
                format!("cache bitstream repair format detection failed: {err}"),
            );
            return !canvas_ok || !frames_ok;
        }
    };
    if !matches!(
        format,
        crate::image_detection::DetectedFormat::GIF
            | crate::image_detection::DetectedFormat::WebP
            | crate::image_detection::DetectedFormat::PNG
            | crate::image_detection::DetectedFormat::AVIF
            | crate::image_detection::DetectedFormat::JXL
    ) {
        return false;
    }
    if format == crate::image_detection::DetectedFormat::PNG {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "video_cache_bitstream_apng_read_failed",
                    path,
                    format!("cache bitstream APNG read failed: {err}"),
                );
                return !canvas_ok || !frames_ok;
            }
        };
        let (is_animated, _) = crate::image_detection::parse_apng_frames(&data);
        return is_animated && (!canvas_ok || !frames_ok);
    }
    let (is_animated, native_frames, _) =
        match crate::image_detection::detect_animation(path, &format) {
            Ok(value) => value,
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "video_cache_bitstream_animation_detect_failed",
                    path,
                    format!("cache bitstream animation detection failed: {err}"),
                );
                return !canvas_ok || !frames_ok;
            }
        };
    is_animated && (native_frames.is_some_and(|nf| nf > 1) || !canvas_ok || !frames_ok)
}

/// Promote structurally animated containers when ffprobe leaves `frame_count`
/// empty (e.g. WebP 0×0).
///
/// Returns `true` when `detection.frame_count` was raised above 1 so vid routes
/// to animated encode.
pub fn promote_animated_container_for_vid(
    path: &Path,
    detection: &mut Detection,
) -> std::result::Result<bool, crate::ffprobe::FFprobeError> {
    let prior_frame_count = detection.frame_count;
    let repair_result = repair_animated_container_detection_from_bitstream_header(path, detection);
    repair_result?;
    if detection.frame_count.is_some_and(|fc| fc > 1) && prior_frame_count.is_none_or(|fc| fc <= 1)
    {
        return Ok(true);
    }
    if detection.frame_count.is_some_and(|fc| fc > 1) {
        return Ok(false);
    }
    let promoted = match crate::image_detection::detect_format_from_bytes(path) {
        Ok(format) => match crate::image_detection::detect_animation(path, &format) {
            Ok((true, frames, fps)) => {
                let n = u64::from(
                    crate::media_conversion_gate::probe_animated_promoted_frame_count_or_min_two(
                        frames, path,
                    ),
                );
                detection.frame_count = Some(n);
                if detection
                    .duration_secs
                    .is_none_or(|d| !d.is_finite() || d <= 0.0)
                    && let (Some(fc), Some(fps)) = (frames, fps)
                    && fc > 1
                    && fps.is_finite()
                    && fps > 0.0
                {
                    detection.duration_secs = Some(f64::from(fc) / f64::from(fps));
                }
                true
            }
            Ok((false, _, _)) => try_promote_animated_webp_from_header(path, detection)?,
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "animated_container_promote_animation_detect_failed",
                    path,
                    format!("animated-container promotion animation detection failed: {err}"),
                );
                return Err(crate::ffprobe::FFprobeError::ParseError(format!(
                    "animated-container promotion animation detection failed for {}: {err}",
                    path.display()
                )));
            }
        },
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_container_promote_format_detect_failed",
                path,
                format!("animated-container promotion format detection failed: {err}"),
            );
            return Err(crate::ffprobe::FFprobeError::ParseError(format!(
                "animated-container promotion format detection failed for {}: {err}",
                path.display()
            )));
        }
    };
    if promoted {
        backfill_detection_canvas_from_bitstream_header(path, detection);
        crate::media_conversion_gate::probe_detection_recovery_audit(
            "animated_container_ffprobe_recovery",
            format!(
                "{}: structurally animated container promoted for vid (frame_count={})",
                path.display(),
                crate::media_conversion_gate::delivery_frame_count_label_u64(
                    detection.frame_count,
                    &format!("animated promote {}", path.display()),
                ),
            ),
        );
    }
    Ok(promoted)
}

fn read_container_header_prefix(
    path: &Path,
    ext: &str,
    cap: usize,
) -> std::result::Result<Option<Vec<u8>>, crate::ffprobe::FFprobeError> {
    match crate::image::format_detect::detect_true_format(path) {
        Ok(format) if header_label_matches_true_format(ext, format) => {}
        Ok(_) => return Ok(None),
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_header_format_detect_failed",
                path,
                format!("{ext} header recovery true-format detection failed: {err}"),
            );
            return Err(crate::ffprobe::FFprobeError::ParseError(format!(
                "{ext} header recovery true-format detection failed for {}: {err}",
                path.display()
            )));
        }
    }
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_header_metadata_failed",
                path,
                format!("{ext} header recovery metadata read failed: {err}"),
            );
            return Err(err.into());
        }
    };
    let read_len_u64 = meta.len().min(crate::numeric_cast::usize_to_u64(cap));
    let Some(read_len) =
        crate::numeric_cast::u64_to_usize_strict(read_len_u64, "animated_header_read_len")
    else {
        crate::media_conversion_gate::probe_layer_audit(
            "animated_header_length_cast_failed",
            path,
            format!("{ext} header recovery length {read_len_u64} did not fit usize"),
        );
        return Err(crate::ffprobe::FFprobeError::ParseError(format!(
            "{ext} header recovery length {read_len_u64} did not fit usize for {}",
            path.display()
        )));
    };
    if read_len < 16 {
        return Err(crate::ffprobe::FFprobeError::ParseError(format!(
            "{ext} header recovery truncated for {}: {read_len} bytes",
            path.display()
        )));
    }
    let mut buf = vec![0u8; read_len];
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_header_open_failed",
                path,
                format!("{ext} header recovery file open failed: {err}"),
            );
            return Err(err.into());
        }
    };
    if let Err(err) = file.read_exact(&mut buf) {
        crate::media_conversion_gate::probe_layer_audit(
            "animated_header_read_failed",
            path,
            format!("{ext} header recovery read failed: {err}"),
        );
        return Err(err.into());
    }
    Ok(Some(buf))
}

fn gif_logical_screen_from_prefix(buf: &[u8]) -> Option<(u32, u32)> {
    if buf.len() < 10 {
        return None;
    }
    let magic = &buf[0..6];
    if magic != b"GIF87a" && magic != b"GIF89a" {
        return None;
    }
    let width = u32::from(u16::from_le_bytes([buf[6], buf[7]]));
    let height = u32::from(u16::from_le_bytes([buf[8], buf[9]]));
    (width > 0 && height > 0).then_some((width, height))
}

fn png_ihdr_dimensions_from_bytes(data: &[u8]) -> Option<(u32, u32)> {
    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if data.len() < 24 || data.get(0..8) != Some(&PNG_SIG) || data.get(12..16) != Some(b"IHDR") {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    (width > 0 && height > 0).then_some((width, height))
}

fn read_png_header_prefix(
    path: &Path,
) -> std::result::Result<Option<Vec<u8>>, crate::ffprobe::FFprobeError> {
    Ok(
        crate::media_conversion_gate::probe_header_bytes_png_or_apng(
            read_container_header_prefix(path, "png", PNG_HEADER_READ_CAP)?,
            read_container_header_prefix(path, "apng", PNG_HEADER_READ_CAP)?,
        ),
    )
}

fn animated_header_file_size(
    path: &Path,
    label: &'static str,
) -> std::result::Result<u64, crate::ffprobe::FFprobeError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_header_size_metadata_failed",
                path,
                format!("{label} header recovery file-size metadata failed: {err}"),
            );
            Err(err.into())
        }
    }
}

fn animated_header_timing_data(
    path: &Path,
    label: &'static str,
) -> std::result::Result<Vec<u8>, crate::ffprobe::FFprobeError> {
    match std::fs::read(path) {
        Ok(data) => Ok(data),
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_header_timing_read_failed",
                path,
                format!("{label} header recovery timing read failed: {err}"),
            );
            Err(err.into())
        }
    }
}

/// Build an [`FFprobeResult`] from PNG `acTL` / `fcTL` when structurally APNG
/// (M125).
fn try_probe_from_animated_apng_header(
    path: &Path,
) -> std::result::Result<Option<crate::ffprobe::FFprobeResult>, crate::ffprobe::FFprobeError> {
    let Some(buf) = read_png_header_prefix(path)? else {
        return Ok(None);
    };
    let (is_animated, frame_count) = crate::image_detection::parse_apng_frames(&buf);
    if !is_animated || frame_count <= 1 {
        return Ok(None);
    }
    let Some((width, height)) = png_ihdr_dimensions_from_bytes(&buf) else {
        return Err(crate::ffprobe::FFprobeError::ParseError(format!(
            "APNG header preflight missing valid IHDR for {}",
            path.display()
        )));
    };
    let frame_count = u64::from(frame_count);

    let file_size = animated_header_file_size(path, "apng")?;
    let timing = crate::image_detection::apng_timing_stats_from_bytes(
        &animated_header_timing_data(path, "apng")?,
    );
    let duration = timing
        .as_ref()
        .map(|t| t.duration_secs)
        .filter(|d| d.is_finite() && *d > 0.0);
    let frame_rate = timing
        .as_ref()
        .map(|t| t.fps)
        .filter(|f| f.is_finite() && *f > 0.0);

    Ok(Some(crate::ffprobe::FFprobeResult {
        format_name: "apng".to_string(),
        duration,
        size: file_size,
        bit_rate: None,
        video_codec: "apng".to_string(),
        video_codec_long: "APNG (animated, header preflight)".to_string(),
        width,
        height,
        frame_rate,
        avg_frame_rate: frame_rate,
        frame_count: Some(frame_count),
        pix_fmt: "unknown".to_string(),
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        bit_depth: None,
        bit_depth_inferred_from_pix_fmt: false,
        audio: crate::ffprobe::FFprobeAudioInfo::default(),
        profile: None,
        level: None,
        max_b_frames: None,
        encoder_settings: None,
        video_bit_rate: None,
        refs: None,
        hdr: crate::ffprobe::FFprobeHdrInfo::default(),
        subtitles: crate::ffprobe::FFprobeSubtitleInfo::default(),
        is_variable_frame_rate: false,
        stream_index: 0,
        tags: HashMap::new(),
        loop_count: None,
        frame_types: Vec::new(),
        pts_deltas: Vec::new(),
        mv_magnitudes: Vec::new(),
        pkt_sizes: Vec::new(),
    }))
}

/// Build an [`FFprobeResult`] from GIF logical screen + frame count when
/// structurally animated (M124).
fn try_probe_from_animated_gif_header(
    path: &Path,
) -> std::result::Result<Option<crate::ffprobe::FFprobeResult>, crate::ffprobe::FFprobeError> {
    let Some(buf) = read_container_header_prefix(path, "gif", GIF_HEADER_READ_CAP)? else {
        return Ok(None);
    };
    let Some((width, height)) = gif_logical_screen_from_prefix(&buf) else {
        return Err(crate::ffprobe::FFprobeError::ParseError(format!(
            "GIF header preflight missing logical screen for {}",
            path.display()
        )));
    };
    let frame_count_u32 = match crate::image_formats::gif::count_frames_from_bytes(&buf) {
        Ok(frame_count) => frame_count,
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_header_gif_frame_count_failed",
                path,
                format!("gif header recovery frame-count parse failed: {err}"),
            );
            return Err(crate::ffprobe::FFprobeError::ParseError(format!(
                "GIF header preflight frame-count parse failed for {}: {err}",
                path.display()
            )));
        }
    };
    if frame_count_u32 <= 1 {
        return Ok(None);
    }
    let frame_count = u64::from(frame_count_u32);

    let file_size = animated_header_file_size(path, "gif")?;
    let timing = crate::image_formats::gif::timing_stats_from_bytes(&animated_header_timing_data(
        path, "gif",
    )?)
    .map_err(|err| {
        crate::ffprobe::FFprobeError::ParseError(format!(
            "GIF header timing parse failed for {}: {err}",
            path.display()
        ))
    })?;
    let duration = timing
        .as_ref()
        .map(|t| t.duration_secs)
        .filter(|d| d.is_finite() && *d > 0.0);
    let frame_rate = timing
        .as_ref()
        .map(|t| t.fps)
        .filter(|f| f.is_finite() && *f > 0.0);

    Ok(Some(crate::ffprobe::FFprobeResult {
        format_name: "gif".to_string(),
        duration,
        size: file_size,
        bit_rate: None,
        video_codec: "gif".to_string(),
        video_codec_long: "GIF (animated, header preflight)".to_string(),
        width,
        height,
        frame_rate,
        avg_frame_rate: frame_rate,
        frame_count: Some(frame_count),
        pix_fmt: "unknown".to_string(),
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        bit_depth: None,
        bit_depth_inferred_from_pix_fmt: false,
        audio: crate::ffprobe::FFprobeAudioInfo::default(),
        profile: None,
        level: None,
        max_b_frames: None,
        encoder_settings: None,
        video_bit_rate: None,
        refs: None,
        hdr: crate::ffprobe::FFprobeHdrInfo::default(),
        subtitles: crate::ffprobe::FFprobeSubtitleInfo::default(),
        is_variable_frame_rate: false,
        stream_index: 0,
        tags: HashMap::new(),
        loop_count: None,
        frame_types: Vec::new(),
        pts_deltas: Vec::new(),
        mv_magnitudes: Vec::new(),
        pkt_sizes: Vec::new(),
    }))
}

/// Build an [`FFprobeResult`] from RIFF/WebP headers when the file is
/// structurally animated.
///
/// Avoids primary ffprobe on containers that often report 0×0 / empty
/// `frame_count` (M123).
fn try_probe_from_animated_webp_header(
    path: &Path,
) -> std::result::Result<Option<crate::ffprobe::FFprobeResult>, crate::ffprobe::FFprobeError> {
    let Some(buf) = read_webp_header_prefix(path)? else {
        return Ok(None);
    };
    if !crate::image_formats::webp::is_animated_from_bytes(&buf) {
        return Ok(None);
    }
    let canvas_dimensions = crate::image_formats::webp::canvas_dimensions_from_path(path)
        .map_err(crate::ffprobe::FFprobeError::from)?;
    let Some((width, height)) =
        crate::media_conversion_gate::probe_webp_dimensions_from_bytes_or_path(
            crate::image_formats::webp::dimensions_from_bytes(&buf),
            canvas_dimensions,
        )
    else {
        return Err(crate::ffprobe::FFprobeError::ParseError(format!(
            "WebP header preflight missing canvas dimensions for {}",
            path.display()
        )));
    };
    if width == 0 || height == 0 {
        return Err(crate::ffprobe::FFprobeError::ParseError(format!(
            "WebP header preflight invalid zero canvas for {}",
            path.display()
        )));
    }
    let frame_count = u64::from(
        crate::media_conversion_gate::probe_webp_animated_frame_count_or_minimum(
            crate::image_formats::webp::count_frames_from_bytes(&buf),
            path,
        ),
    );

    let file_size = animated_header_file_size(path, "webp")?;
    let timing = crate::image_formats::webp::timing_stats_from_bytes(&animated_header_timing_data(
        path, "webp",
    )?)
    .map_err(|err| {
        crate::ffprobe::FFprobeError::ParseError(format!(
            "WebP header timing parse failed for {}: {err}",
            path.display()
        ))
    })?;
    let duration = timing
        .as_ref()
        .map(|t| t.duration_secs)
        .filter(|d| d.is_finite() && *d > 0.0);
    let frame_rate = timing
        .as_ref()
        .map(|t| t.fps)
        .filter(|f| f.is_finite() && *f > 0.0);

    Ok(Some(crate::ffprobe::FFprobeResult {
        format_name: "webp".to_string(),
        duration,
        size: file_size,
        bit_rate: None,
        video_codec: "webp".to_string(),
        video_codec_long: "WebP (animated, header preflight)".to_string(),
        width,
        height,
        frame_rate,
        avg_frame_rate: frame_rate,
        frame_count: Some(frame_count),
        pix_fmt: "unknown".to_string(),
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        bit_depth: None,
        bit_depth_inferred_from_pix_fmt: false,
        audio: crate::ffprobe::FFprobeAudioInfo::default(),
        profile: None,
        level: None,
        max_b_frames: None,
        encoder_settings: None,
        video_bit_rate: None,
        refs: None,
        hdr: crate::ffprobe::FFprobeHdrInfo::default(),
        subtitles: crate::ffprobe::FFprobeSubtitleInfo::default(),
        is_variable_frame_rate: false,
        stream_index: 0,
        tags: HashMap::new(),
        loop_count: None,
        frame_types: Vec::new(),
        pts_deltas: Vec::new(),
        mv_magnitudes: Vec::new(),
        pkt_sizes: Vec::new(),
    }))
}

fn read_webp_header_prefix(
    path: &Path,
) -> std::result::Result<Option<Vec<u8>>, crate::ffprobe::FFprobeError> {
    read_container_header_prefix(path, "webp", WEBP_HEADER_READ_CAP)
}

/// Backfill missing or 0×0 canvas from native headers (GIF/PNG/WebP/etc.) after
/// ffprobe (M127).
fn backfill_detection_canvas_from_bitstream_header(path: &Path, detection: &mut Detection) {
    let needs_width = detection.width.is_none_or(|v| v == 0);
    let needs_height = detection.height.is_none_or(|v| v == 0);
    if !needs_width && !needs_height {
        return;
    }
    let Some((w, h)) = (match crate::conversion::dimensions_from_header(path) {
        Ok(value) => value,
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "detection_canvas_header_recovery_failed",
                path,
                format!("bitstream header dimension recovery failed: {err}"),
            );
            return;
        }
    }) else {
        return;
    };
    crate::media_conversion_gate::probe_bitstream_dimension_recovery_audit(
        path,
        crate::media_conversion_gate::delivery_audit_optional_u32(detection.width),
        crate::media_conversion_gate::delivery_audit_optional_u32(detection.height),
        w,
        h,
    );
    if needs_width {
        detection.width = Some(w);
    }
    if needs_height {
        detection.height = Some(h);
    }
}

fn backfill_webp_canvas_from_header(path: &Path, detection: &mut Detection) {
    backfill_detection_canvas_from_bitstream_header(path, detection);
}

/// Trust native animated structure when ffprobe left `frame_count` empty or
/// single-frame (M127).
fn backfill_animated_frame_count_from_bitstream_header(
    path: &Path,
    detection: &mut Detection,
) -> std::result::Result<bool, crate::ffprobe::FFprobeError> {
    if detection.frame_count.is_some_and(|fc| fc > 1) {
        return Ok(false);
    }

    let true_format = match crate::image::format_detect::detect_true_format(path) {
        Ok(format) => format,
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_frame_repair_format_detect_failed",
                path,
                format!("frame-count repair true-format detection failed: {err}"),
            );
            return Err(crate::ffprobe::FFprobeError::ParseError(format!(
                "frame-count repair true-format detection failed for {}: {err}",
                path.display()
            )));
        }
    };

    if true_format == crate::image::format_detect::FormatKind::WebP {
        let Some(buf) = read_webp_header_prefix(path)? else {
            return Ok(false);
        };
        if !crate::image_formats::webp::is_animated_from_bytes(&buf) {
            return Ok(false);
        }
        let n = crate::media_conversion_gate::probe_webp_animated_frame_count_or_minimum(
            crate::image_formats::webp::count_frames_from_bytes(&buf),
            path,
        );
        detection.frame_count = Some(u64::from(n));
        return Ok(true);
    }

    if true_format == crate::image::format_detect::FormatKind::Gif {
        let Some(buf) = read_container_header_prefix(path, "gif", GIF_HEADER_READ_CAP)? else {
            return Ok(false);
        };
        let count = crate::image_formats::gif::count_frames_from_bytes(&buf).map_err(|err| {
            crate::media_conversion_gate::probe_layer_audit(
                "animated_frame_repair_gif_frame_count_failed",
                path,
                format!("frame-count repair GIF parse failed: {err}"),
            );
            crate::ffprobe::FFprobeError::ParseError(format!(
                "frame-count repair GIF parse failed for {}: {err}",
                path.display()
            ))
        })?;
        if count <= 1 {
            return Ok(false);
        }
        detection.frame_count = Some(u64::from(count));
        return Ok(true);
    }

    if true_format == crate::image::format_detect::FormatKind::Png {
        let Some(buf) = read_png_header_prefix(path)? else {
            return Ok(false);
        };
        let (is_animated, fc) = crate::image_detection::parse_apng_frames(&buf);
        if !is_animated || fc <= 1 {
            return Ok(false);
        }
        detection.frame_count = Some(u64::from(fc));
        return Ok(true);
    }

    Ok(false)
}

/// Post-ffprobe repair for structurally animated image containers (M127).
fn repair_animated_container_detection_from_bitstream_header(
    path: &Path,
    detection: &mut Detection,
) -> std::result::Result<(), crate::ffprobe::FFprobeError> {
    backfill_detection_canvas_from_bitstream_header(path, detection);
    if backfill_animated_frame_count_from_bitstream_header(path, detection)? {
        crate::media_conversion_gate::probe_layer_audit(
            "animated_frame_count_bitstream_recovery",
            path,
            format!(
                "ffprobe under-reported frame_count; native structure → {}",
                crate::media_conversion_gate::delivery_frame_count_label_u64(
                    detection.frame_count,
                    &format!("bitstream frame recovery {}", path.display()),
                ),
            ),
        );
    }
    Ok(())
}

fn try_promote_animated_webp_from_header(
    path: &Path,
    detection: &mut Detection,
) -> std::result::Result<bool, crate::ffprobe::FFprobeError> {
    let Some(buf) = read_webp_header_prefix(path)? else {
        return Ok(false);
    };
    if !crate::image_formats::webp::is_animated_from_bytes(&buf) {
        return Ok(false);
    }
    let canvas_dimensions = crate::image_formats::webp::canvas_dimensions_from_path(path)
        .map_err(crate::ffprobe::FFprobeError::from)?;
    if crate::media_conversion_gate::probe_webp_dimensions_from_bytes_or_path(
        crate::image_formats::webp::dimensions_from_bytes(&buf),
        canvas_dimensions,
    )
    .is_none()
    {
        return Err(crate::ffprobe::FFprobeError::ParseError(format!(
            "animated WebP promotion missing dimensions for {}",
            path.display()
        )));
    }
    let frame_count = u64::from(
        crate::media_conversion_gate::probe_webp_animated_frame_count_or_minimum(
            crate::image_formats::webp::count_frames_from_bytes(&buf),
            path,
        ),
    );
    detection.frame_count = Some(frame_count);
    backfill_webp_canvas_from_header(path, detection);
    Ok(true)
}

/// Detect video properties using ffprobe and fuse scenario DB quality when
/// applicable.
///
/// # Errors
/// Returns `FFprobeError` if the file is invalid or ffprobe fails.
pub fn detect_video(path: &Path) -> std::result::Result<Detection, FFprobeError> {
    let mut result = detect_video_impl(path)?;
    apply_video_quality_db_fusion(&mut result, path);
    Ok(result)
}

fn apply_video_quality_db_fusion(detection: &mut Detection, path: &Path) {
    if !crate::algorithm_runtime::quality_db_lookup_enabled("video_detection") {
        return;
    }
    if matches!(
        detection.compression,
        CompressionType::Lossless | CompressionType::VisuallyLossless
    ) {
        return;
    }
    let Some(prediction) = crate::scenario_quality_lookup::lookup_media_quality_by_path(path)
    else {
        return;
    };
    if let Some(fused) = crate::image_quality_db::fuse_quality_regression_prediction_if_enabled(
        "video_detection",
        Some(detection.quality_score),
        prediction,
    ) {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "video_detection",
            branch = "quality_regression_fusion_applied",
            heuristic = detection.quality_score,
            fused,
            "video quality score fused with scenario DB prediction"
        );
        detection.quality_score = crate::algorithm_seal::seal_u8_quality_display(fused);
    }
}

fn try_animated_header_preflight(
    path: &Path,
) -> std::result::Result<
    Option<(&'static str, &'static str, crate::ffprobe::FFprobeResult)>,
    crate::ffprobe::FFprobeError,
> {
    if let Some(probe) = try_probe_from_animated_webp_header(path)? {
        return Ok(Some(("webp_animated_header_preflight", "WebP", probe)));
    }
    if let Some(probe) = try_probe_from_animated_gif_header(path)? {
        return Ok(Some(("gif_animated_header_preflight", "GIF", probe)));
    }
    Ok(try_probe_from_animated_apng_header(path)?
        .map(|probe| ("apng_header_preflight", "APNG", probe)))
}

/// Core ffprobe detection without scenario DB fusion (cache stores this layer).
fn detect_video_impl(path: &Path) -> std::result::Result<Detection, FFprobeError> {
    let preflight = try_animated_header_preflight(path)?;
    let probe = if let Some((branch, label, probe)) = preflight {
        crate::media_conversion_gate::probe_layer_audit(
            branch,
            path,
            format!(
                "structurally animated {label}: header probe ({}x{}, frames={}) — skipping \
                 primary ffprobe",
                probe.width,
                probe.height,
                crate::media_conversion_gate::delivery_frame_count_label_u64(
                    probe.frame_count,
                    &format!("{label} preflight {}", path.display()),
                ),
            ),
        );
        probe
    } else {
        match probe_video(path) {
            Ok(p) => p,
            Err(e) => recover_bitstream_on_probe_failure(path, e)?,
        }
    };

    let codec = DetectedCodec::from_ffprobe(&probe.video_codec);
    let has_b_frames = probe.has_b_frames();

    // `bit_rate` is absent for image containers probed via ffprobe (e.g. WebP).
    // Root fix: Derive bitrate from file size and duration if missing to ensure
    // accurate BPP and compression detection.
    let format_bit_rate = crate::media_conversion_gate::probe_ffprobe_bit_rate_or_derived_from_size(
        probe.bit_rate,
        probe.size,
        probe.duration,
    );
    if format_bit_rate.is_some()
        && probe.bit_rate.is_none()
        && let Some(derived) = format_bit_rate
    {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DETECTION,
            &format!(
                "ffprobe: Derived bitrate {:.1} kbps from file size and duration",
                crate::numeric_cast::u64_to_f64(derived) / 1000.0
            )
        );
    }

    let bits_per_pixel = if let Some(bitrate_val) = format_bit_rate
        && let Some(fps) = probe.frame_rate
        && (f64::from(probe.width) * f64::from(probe.height) * fps) > 0.0_f64
    {
        crate::numeric_cast::u64_to_f64(bitrate_val)
            / (f64::from(probe.width) * f64::from(probe.height) * fps)
    } else {
        0.0_f64
    };

    let mut precision = extract_video_precision(
        &probe.tags,
        probe.encoder_settings.as_deref(),
        probe.max_b_frames,
    );
    precision.bit_depth_inferred_from_pix_fmt = probe.bit_depth_inferred_from_pix_fmt;

    let compression = determine_compression_type(
        &codec,
        format_bit_rate,
        probe.width,
        probe.height,
        probe.frame_rate,
        &precision,
    );

    let color_space = match probe.color_space.as_ref() {
        None => ColorSpace::Unknown("unknown".to_string()),
        Some(s) => ColorSpace::parse(s),
    };

    let quality_score = calculate_quality_score(
        &compression,
        probe.bit_depth,
        probe.bit_depth_inferred_from_pix_fmt,
        format_bit_rate,
        probe.width,
        probe.height,
    );

    let archival_candidate = matches!(
        compression,
        CompressionType::Lossless | CompressionType::VisuallyLossless
    ) || codec.can_be_lossless();

    let mut result = Detection {
        file_path: path.display().to_string(),
        format: probe.format_name,
        codec,
        codec_long: probe.video_codec_long,
        compression,
        width: if probe.width > 0 {
            Some(probe.width)
        } else {
            None
        },
        height: if probe.height > 0 {
            Some(probe.height)
        } else {
            None
        },
        frame_count: probe.frame_count,
        fps: probe.frame_rate,
        duration_secs: probe.duration,
        bit_depth: probe.bit_depth,
        pix_fmt: probe.pix_fmt,
        color_space,
        bitrate: format_bit_rate,
        audio_duration_secs: probe.audio.duration,
        flags: VideoFlags {
            streams: VideoStreamFlags {
                has_audio: probe.audio.present,
                has_subtitles: probe.subtitles.present,
            },
            content: VideoContentFlags {
                archival_candidate,
                has_b_frames,
                is_variable_frame_rate: probe.is_variable_frame_rate,
            },
            hdr: VideoHdrFlags {
                is_dolby_vision: probe.hdr.is_dolby_vision(),
                is_hdr10_plus: probe.hdr.hdr10_plus,
            },
        },
        audio_codec: probe.audio.codec.clone(),
        file_size: probe.size,
        quality_score,
        profile: probe.profile,
        max_b_frames: probe.max_b_frames,
        encoder_params: probe.encoder_settings.clone(),
        video_bitrate: probe.video_bit_rate,
        bits_per_pixel,
        color_primaries: probe.color_primaries,
        color_transfer: probe.color_transfer,
        mastering_display: probe.hdr.mastering_display.clone(),
        max_cll: probe.hdr.max_cll.clone(),
        dv_profile: probe.hdr.dv_profile(),
        dv_bl_signal_compatibility_id: probe.hdr.dv_bl_signal_compatibility_id(),
        subtitle_codec: probe.subtitles.codec.clone(),
        audio_channels: probe.audio.channels,
        precision,
        tags: probe.tags,
        history: crate::common_utils::get_current_history(),
        perception: crate::types::Visual::default(),
        loop_count: probe.loop_count,
        frame_types: probe.frame_types,
        pts_deltas: probe.pts_deltas,
        mv_magnitudes: probe.mv_magnitudes,
        pkt_sizes: probe.pkt_sizes,
        is_interlaced: None,
    };

    repair_animated_container_detection_from_bitstream_header(path, &mut result)?;

    match crate::image_detection::detect_format_from_bytes(path) {
        Ok(format) if matches!(format, crate::image_detection::DetectedFormat::JXL) => {
            match crate::image_detection::detect_animation(path, &format) {
                Ok((is_animated, native_frames, _)) => match (is_animated, native_frames) {
                    (false, Some(1)) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "{} [Detection] Static JXL with demux frame_count=1; vid ignore path: \
                             {}",
                            crate::media_conversion_gate::ui_icon_pick("⚙️", "[GEAR]"),
                            path.display()
                        ));
                        result.frame_count = native_frames.map(u64::from);
                        result.duration_secs = None;
                    }
                    (false, None) => {
                        crate::media_conversion_gate::probe_layer_audit(
                            "jxl_static_no_frame_count",
                            path,
                            "animation probe: static without frame count; keeping ffprobe metadata",
                        );
                    }
                    (false, Some(other)) => {
                        crate::media_conversion_gate::probe_layer_audit(
                            "jxl_static_implausible_frames",
                            path,
                            format!(
                                "animation probe: static with implausible frame count {other}; \
                                 keeping ffprobe metadata"
                            ),
                        );
                    }
                    (true, Some(0 | 1)) => {
                        crate::media_conversion_gate::probe_layer_audit(
                            "jxl_animated_implausible_frames",
                            path,
                            format!(
                                "animation probe: animated with implausible frame count {}; \
                                 keeping ffprobe metadata",
                                crate::media_conversion_gate::delivery_frame_count_label_u64(
                                    native_frames.map(u64::from),
                                    &format!("jxl animation probe {}", path.display()),
                                ),
                            ),
                        );
                    }
                    (true, Some(_)) => {}
                    (true, None) => {
                        crate::media_conversion_gate::probe_layer_audit(
                            "jxl_animated_unknown_frames",
                            path,
                            "animation probe: animated without frame count; keeping ffprobe \
                             metadata",
                        );
                    }
                },
                Err(err) => {
                    crate::media_conversion_gate::probe_layer_audit(
                        "jxl_static_animation_detect_failed",
                        path,
                        format!("JXL static/animated reconciliation failed: {err}"),
                    );
                }
            }
        }
        Ok(_) => {}
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "jxl_static_format_detect_failed",
                path,
                format!("JXL static/animated format detection failed: {err}"),
            );
        }
    }

    // ── Penetrating Content Verification ──
    // Verify critical metadata claims by decoding actual content
    if result.flags.streams.has_audio
        && let crate::media_penetration::PenetrationResult::Verified(is_silent) =
            crate::media_penetration::detect_audio_silence(path)
        && is_silent
    {
        crate::progress_mode::emit_stderr(&format!(
            "🔊 [{}] Audio penetration: SILENT track detected, treating as no audio",
            crate::media_conversion_gate::path_file_name_for_log(path)
        ));
        result.flags.streams.has_audio = false;
    }

    let has_transparency = result.pix_fmt.contains('a')
        || result.pix_fmt.contains("yuva")
        || result.pix_fmt.contains("gbrap");
    if has_transparency
        && let crate::media_penetration::PenetrationResult::Verified(is_real) =
            crate::media_penetration::detect_real_transparency(path, result.duration_secs)
        && !is_real
    {
        crate::progress_mode::emit_stderr(&format!(
            "{} [{}] Transparency penetration: FAKE alpha channel (unused)",
            crate::modern_ui::symbols::styled_warning_icon(),
            crate::media_conversion_gate::path_file_name_for_log(path)
        ));
    }

    if let Some(fc_val) = result.frame_count
        && (fc_val <= 1 || fc_val > crate::constants::FRAME_COUNT_TRUST_UPPER_LIMIT)
        && let crate::media_penetration::PenetrationResult::Verified(real_count) =
            crate::media_penetration::detect_real_frame_count(path, Some(fc_val))
        && real_count != fc_val
    {
        crate::progress_mode::emit_stderr(&format!(
            "{} [{}] Frame count mismatch: metadata={}, actual={}, correcting",
            crate::media_conversion_gate::ui_icon_pick(
                crate::modern_ui::symbols::WARNING,
                crate::modern_ui::symbols::plain::WARNING,
            ),
            crate::media_conversion_gate::path_file_name_for_log(path),
            fc_val,
            real_count
        ));
        result.frame_count = Some(real_count);
    }

    // Interlace detection is expensive, so we only run it for "gray zone" assets
    // (4s to 18s) where loop intent might be ambiguous, and only if it's not a
    // native gif/webp.
    if result
        .duration_secs
        .is_some_and(|d| d >= crate::constants::INTERLACE_DETECTION_MIN_DURATION_SECS)
        && result
            .duration_secs
            .is_some_and(|d| d <= crate::constants::INTERLACE_DETECTION_MAX_DURATION_SECS)
        && result.format != "gif"
        && result.format != "webp"
        && let crate::media_penetration::PenetrationResult::Verified(is_interlaced) =
            crate::media_penetration::detect_interlacing(path)
    {
        result.is_interlaced = Some(is_interlaced);
    }

    Ok(result)
}

fn extract_video_precision(
    tags: &HashMap<String, String>,
    encoder_settings: Option<&str>,
    max_b_frames: Option<u8>,
) -> VideoPrecisionMetadata {
    let mut precision = VideoPrecisionMetadata {
        original_max_b_frames: max_b_frames,
        original_encoder: tags.get("encoder").cloned(),
        ..Default::default()
    };

    // Prioritize explicit encoder_settings (x264-params/x265-params) over generic
    // tags
    let search_string =
        crate::media_conversion_gate::probe_encoder_settings_search_string(encoder_settings, tags);

    if !search_string.is_empty() {
        let lower = search_string.to_lowercase();

        // Extract CRF
        if let Some(crf_pos) = lower.find("crf=") {
            let sub = &lower[crf_pos + 4..];
            let val: String = sub
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            match val.parse::<f32>() {
                Ok(v) => precision.original_crf = Some(v),
                Err(err) if !val.is_empty() => {
                    crate::media_conversion_gate::probe_layer_batch_audit(
                        "video_precision_crf_parse_failed",
                        format!("encoder_settings CRF parse failed for token {val:?}: {err}"),
                    );
                }
                Err(_) => {}
            }
        } else if let Some(qp_pos) = lower.find("qp=") {
            let sub = &lower[qp_pos + 4..];
            let val: String = sub
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            match val.parse::<f32>() {
                Ok(v) => precision.original_crf = Some(v),
                Err(err) if !val.is_empty() => {
                    crate::media_conversion_gate::probe_layer_batch_audit(
                        "video_precision_qp_parse_failed",
                        format!("encoder_settings QP parse failed for token {val:?}: {err}"),
                    );
                }
                Err(_) => {}
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

/// 🚀 New Entry Point: Subscribes to `MediaIndexRow` (Database-driven decision)
///
/// # Errors
/// Returns an error if the recommendation cannot be generated.
pub fn get_video_recommendation_from_row(
    row: &MediaIndexRow,
) -> std::result::Result<VideoRecommendation, serde_json::Error> {
    let features: Detection = serde_json::from_str(&row.raw_features_json)?;

    Ok(generate_video_recommendation(&features))
}

#[must_use]
pub fn generate_video_recommendation(features: &Detection) -> VideoRecommendation {
    let mut recommended_codec = features.codec.as_str().to_string();
    let mut reason = "Current codec is optimal or sufficient".to_string();
    let mut is_archival_upgrade = false;
    let mut command_hint = String::new();

    // Decision logic: if it's an archival candidate but not yet in a modern
    // delivery format.
    let is_old_lossless = matches!(
        features.codec,
        DetectedCodec::ProRes | DetectedCodec::DNxHD | DetectedCodec::MJPEG
    );
    let is_high_bitrate_h264 = features.codec == DetectedCodec::H264
        && features
            .bitrate
            .is_some_and(|b| b > crate::constants::VIDEO_RECOMMENDATION_HIGH_BITRATE_THRESHOLD);

    if is_old_lossless || is_high_bitrate_h264 {
        recommended_codec = "AV1 (SVT-AV1)".to_string();
        is_archival_upgrade = true;
        reason = if is_old_lossless {
            "Professional archival format detected; recommend AV1 for space efficiency with zero \
             visual loss"
                .to_string()
        } else {
            "High-bitrate H.264 detected; recommend AV1 for 50%+ size reduction".to_string()
        };
        command_hint = format!(
            "ffmpeg -i '{}' -c:v libsvtav1 -preset {} -crf {} output.mp4",
            features.file_path,
            crate::constants::VIDEO_RECOMMENDATION_AV1_PRESET_DEFAULT,
            crate::constants::VIDEO_RECOMMENDATION_AV1_CRF_DEFAULT
        );
    }

    VideoRecommendation {
        current_codec: features.codec.as_str().to_string(),
        recommended_codec,
        reason,
        is_archival_upgrade,
        command_hint,
    }
}

fn recover_bitstream_on_probe_failure(
    path: &Path,
    original_error: crate::ffprobe::FFprobeError,
) -> std::result::Result<crate::ffprobe::FFprobeResult, crate::ffprobe::FFprobeError> {
    crate::media_conversion_gate::probe_layer_audit(
        "ffprobe_primary_failed",
        path,
        format!("ffprobe failed; attempting bitstream recovery: {original_error}"),
    );

    let media_info = crate::media_conversion_gate::probe_bitstream_media_info_or_webp_canvas(
        path,
        crate::conversion::media_info_without_ffprobe(path),
    )
    .map_err(|err| {
        crate::media_conversion_gate::probe_layer_audit(
            "ffprobe_recovery_probe_failed",
            path,
            format!("bitstream fallback probe failed: {err}"),
        );
        crate::ffprobe::FFprobeError::ParseError(format!(
            "ffprobe failed ({original_error}) and bitstream fallback probe failed: {err}"
        ))
    })?;
    let media_info = media_info.ok_or_else(|| {
        crate::media_conversion_gate::probe_layer_audit(
            "ffprobe_recovery_failed",
            path,
            "bitstream fallback could not determine media properties",
        );
        original_error // Return original ffprobe error if fallback also fails
    })?;
    let width = media_info.width;
    let height = media_info.height;
    let channel_type =
        crate::media_conversion_gate::recovery_channel_type_label(media_info.channel_type, path);

    let file_size = std::fs::metadata(path).map(|m| m.len()).map_err(|io_err| {
        crate::media_conversion_gate::probe_layer_audit(
            "recovery_metadata_read_failed",
            path,
            format!("failed to read file metadata during recovery: {io_err}"),
        );
        crate::ffprobe::FFprobeError::from(io_err)
    })?;

    Ok(crate::ffprobe::FFprobeResult {
        format_name: crate::media_conversion_gate::recovery_format_name(path),
        duration: None,
        size: file_size,
        bit_rate: None,
        video_codec: "unknown".to_string(),
        video_codec_long: "Unknown Codec (Recovery Mode)".to_string(),
        width,
        height,
        frame_rate: None,
        avg_frame_rate: None,
        frame_count: None,
        // Honest pix_fmt mapping: use the measured channel string when available,
        // otherwise keep explicit unknown instead of forging a concrete layout.
        pix_fmt: channel_type,
        color_space: None,
        color_transfer: None,
        color_primaries: None,
        bit_depth: media_info.bit_depth,
        bit_depth_inferred_from_pix_fmt: false,
        audio: crate::ffprobe::FFprobeAudioInfo::default(),
        profile: None,
        level: None,
        max_b_frames: None,
        encoder_settings: None,
        video_bit_rate: None,
        refs: None,
        hdr: crate::ffprobe::FFprobeHdrInfo::default(),
        subtitles: crate::ffprobe::FFprobeSubtitleInfo::default(),
        is_variable_frame_rate: false,
        stream_index: 0,
        tags: std::collections::HashMap::new(),
        loop_count: None,
        frame_types: Vec::new(),
        pts_deltas: Vec::new(),
        mv_magnitudes: Vec::new(),
        pkt_sizes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    include!("../tests/video_detection.rs");
}
