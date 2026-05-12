//! Video Detection API Module (Shared)
//!
//! Pure analysis layer - detects video properties using ffprobe.
//! Determines codec type, compression level, and archival suitability.
//!
//! Migrated from `vid_hevc/vid_av1` `detection_api.rs` to eliminate duplication.

use crate::ffprobe::{FFprobeError, probe_video};
use crate::media_index_types::MediaIndexRow;
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
    /// `color_transfer` (TRC) from ffprobe (e.g. "smpte2084", "arib-std-b67", "bt709")
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
    /// Returns true when the content is any form of HDR (PQ, HLG, DV, HDR10, HDR10+)
    #[must_use]
    pub fn is_hdr(&self) -> bool {
        self.flags.hdr.is_dolby_vision
            || self.flags.hdr.is_hdr10_plus
            || self.mastering_display.is_some()
            || self.max_cll.is_some()
            || matches!(
                self.color_transfer.as_deref(),
                Some(crate::constants::HDR_TRANSFER_PQ | crate::constants::HDR_TRANSFER_HLG)
            )
    }

    /// Returns true for high-bitrate archival-grade content
    #[must_use]
    pub fn is_high_fidelity(&self) -> bool {
        self.bit_depth.is_some_and(|d| d >= 10)
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
    let depth_bonus = if bit_depth.is_some_and(|d| d >= crate::constants::HDR_BIT_DEPTH_THRESHOLD) {
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
/// Returns an error if the file cannot be read, ffprobe fails, or cache access errors.
pub fn detect_video_with_cache(
    path: &Path,
    cache: Option<&crate::analysis_cache::AnalysisCache>,
) -> std::result::Result<Detection, FFprobeError> {
    let should_refresh_cached_result = |cached: &Detection| -> bool {
        if cached.frame_count.is_some_and(|fc| fc > 1) {
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
        is_animated && native_frames.is_some_and(|nf| nf > 1)
    };

    if let Some(cache) = cache {
        match cache.get_video_analysis(path) {
            Ok(Some(mut cached)) => {
                if should_refresh_cached_result(&cached) {
                    crate::log_info!(
                        crate::static_logs::messages::LABEL_DETECTION,
                        &format!(
                            "Invalidating stale cached WebP frame metadata and re-running detection (path={})",
                            path.display()
                        )
                    );
                } else {
                    if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                        crate::progress_mode::emit_stderr(&format!(
                            "🔍 [Video Cache] Hit: {}",
                            path.display()
                        ));
                    }
                    cached.file_path = path.display().to_string();
                    return Ok(cached);
                }
            }
            Ok(None) => {}
            Err(err) => {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_DETECTION,
                    &format!(
                        "Failed to load cached video analysis (path={}): {}",
                        path.display(),
                        err
                    )
                );
            }
        }
    }

    let result = detect_video(path)?;

    if let Some(cache) = cache
        && let Err(err) = cache.store_video_analysis(path, &result)
    {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_DETECTION,
            &format!(
                "Failed to store video analysis in cache (path={}): {}",
                path.display(),
                err
            )
        );
    }

    Ok(result)
}

/// Detect video properties using ffprobe.
///
/// # Errors
/// Returns `FFprobeError` if the file is invalid or ffprobe fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn detect_video(path: &Path) -> std::result::Result<Detection, FFprobeError> {
    let probe = match probe_video(path) {
        Ok(p) => p,
        Err(e) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_DETECTION,
                &format!(
                    "ffprobe failed to analyze file; attempting secondary recovery via direct bitstream analysis (file={}): {}",
                    path.display(),
                    e
                )
            );

            let (width, height, channel_type, depth) = crate::conversion::media_info_without_ffprobe(path)
                .ok_or_else(|| {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_DETECTION,
                        &format!("Secondary recovery failed: could not determine REAL media properties via bitstream fallback (file={})", path.display())
                    );
                    e // Return original ffprobe error (e) if fallback also fails
                })?;

            let file_size = std::fs::metadata(path).map(|m| m.len()).map_err(|io_err| {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_DETECTION,
                    &format!(
                        "Failed to read REAL file metadata during recovery (file={}): {}",
                        path.display(),
                        io_err
                    )
                );
                crate::ffprobe::FFprobeError::from(io_err)
            })?;

            crate::ffprobe::FFprobeResult {
                format_name: path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
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
                // Honest pix_fmt mapping: we use the REAL channel property string from identify.
                // If it contains 'a', the system's alpha-detection logic will find it.
                pix_fmt: channel_type,
                color_space: None,
                color_transfer: None,
                color_primaries: None,
                bit_depth: Some(depth),
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
            }
        }
    };

    let codec = DetectedCodec::from_ffprobe(&probe.video_codec);
    let has_b_frames = probe.has_b_frames();

    // `bit_rate` is absent for image containers probed via ffprobe (e.g. WebP).
    // Root fix: Derive bitrate from file size and duration if missing to ensure accurate BPP and compression detection.
    let format_bit_rate = probe.bit_rate.or_else(|| {
        if let Some(dur) = probe.duration
            && dur > 0.0
        {
            let bits = probe.size.saturating_mul(8);
            let derived =
                crate::numeric_cast::f64_to_u64_sat(crate::numeric_cast::u64_to_f64(bits) / dur);
            if derived > 0 {
                crate::log_info!(
                    crate::static_logs::messages::LABEL_DETECTION,
                    &format!(
                        "ffprobe: Derived bitrate {:.1} kbps from file size and duration",
                        crate::numeric_cast::u64_to_f64(derived) / 1000.0
                    )
                );
                Some(derived)
            } else {
                None
            }
        } else {
            None
        }
    });

    let bits_per_pixel = if let Some(bitrate_val) = format_bit_rate
        && let Some(fps) = probe.frame_rate
        && (f64::from(probe.width) * f64::from(probe.height) * fps) > 0.0_f64
    {
        crate::numeric_cast::u64_to_f64(bitrate_val)
            / (f64::from(probe.width) * f64::from(probe.height) * fps)
    } else {
        0.0_f64
    };

    let precision = extract_video_precision(
        &probe.tags,
        probe.encoder_settings.as_deref(),
        probe.max_b_frames,
    );

    let compression = determine_compression_type(
        &codec,
        format_bit_rate,
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

    if let Ok(format) = crate::image_detection::detect_format_from_bytes(path)
        && matches!(format, crate::image_detection::DetectedFormat::JXL)
        && let Ok((is_animated, native_frames, _)) = crate::image_detection::detect_animation(path, &format)
        && (!is_animated || native_frames.unwrap_or(1) <= 1)
    {
        crate::progress_mode::emit_stderr(&format!(
            "⚙️ [Detection] Forcing single-frame for static JXL to avoid vid routing: {}",
            path.display()
        ));
        result.frame_count = Some(1);
        result.duration_secs = None;
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
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
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
            "⚠️  [{}] Transparency penetration: FAKE alpha channel (unused)",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
        ));
    }

    if let Some(fc_val) = result.frame_count
        && (fc_val <= 1 || fc_val > crate::constants::FRAME_COUNT_TRUST_UPPER_LIMIT)
        && let crate::media_penetration::PenetrationResult::Verified(real_count) =
            crate::media_penetration::detect_real_frame_count(path, fc_val)
        && real_count != fc_val
    {
        crate::progress_mode::emit_stderr(&format!(
            "⚠️  [{}] Frame count mismatch: metadata={}, actual={}, correcting",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            fc_val,
            real_count
        ));
        result.frame_count = Some(real_count);
    }

    // Interlace detection is expensive, so we only run it for "gray zone" assets (4s to 18s)
    // where loop intent might be ambiguous, and only if it's not a native gif/webp.
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

    // Decision Logic: If it's a high-fidelity archival candidate but not yet in modern modern formats
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
            "Professional archival format detected; recommend AV1 for space efficiency with zero visual loss".to_string()
        } else {
            "High-bitrate H.264 detected; recommend AV1 for 50%+ size reduction".to_string()
        };
        command_hint = format!(
            "ffmpeg -i '{}' -c:v libsvtav1 -preset {} -crf {} output.mp4",
            features.file_path,
            crate::constants::FFMPEG_SVTAV1_DEFAULT_PRESET,
            crate::constants::AV1_CRF_DEFAULT_F64
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
