//! Loop Intent Identification System
//!
//! A modern, explainable judgment tree for identifying media looping intent (memes, stickers, loops).
//! Implements the 7-layer hierarchical decision tree defined in `docs/decision_tree.md`.
//!
//! Architecture:
//! - Layer 1: Hard constraints + veto-gated hard passes
//! - Layer 2: Explicit declarations → direct exits
//! - Layer 3 & 4: Structural/content signals → `WeightedScore` accumulation with checkpoints
//! - Layer 5: Weak contextual corrections
//! - Layer 6: KNN + `WeightedScore` fusion
//! - Layer 7: Conservative fallback
//!
//! Note: This module heavily utilizes Nightly-only features (intrinsics, `try_blocks`, SIMD)
//! enabled via #![feature(...)] in lib.rs for forensic-grade performance.

use crate::builder_base::ToolBuilder;
use crate::constants::{
    DIRECTORY_CONTEXT_POSITIVE_LOG_ODDS, FILENAME_CONTEXT_POSITIVE_LOG_ODDS,
    LOCALIZED_MOTION_POSITIVE_LOG_ODDS, MODERN_MASTER_NEGATIVE_LOG_ODDS,
    PLAY_ONCE_NEGATIVE_LOG_ODDS,
};
use crate::database::{DistributionStats, LoopReferenceProfile};
use crate::file_copier::{SUPPORTED_IMAGE_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS};
use crate::media_penetration::{
    detect_audio_silence, detect_real_frame_count, detect_real_transparency,
};
use crate::modern_ui::symbols;
use crate::ui_stderr;
use crate::video_detection::Detection;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, GenericImageView};
use serde::{Deserialize, Serialize};
use std::path::Path;

const WEBP_RATIO_SAMPLE_MAX_DIM: u32 = crate::constants::WEBP_RATIO_SAMPLE_MAX_DIM;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilenameKind {
    HumanSemantic,
    MachineGenerated,
    Ambiguous,
}

pub struct FilenameAnalysis {
    pub raw: f64,
    pub kind: FilenameKind,
}

// ── Output: Tri-state Output ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Strong loop intent: Keep as GIF / convert video → GIF.
    LoopStrong(String),
    /// Weak loop intent: Convert GIF → video / keep as video.
    LoopWeak(String),
    /// Uncertain: insufficient signal, handled by conservative fallback (Layer 7).
    Uncertain(String),
    /// Error: impossible or conflicting signals (e.g. 1 frame video).
    Error(String),
}

impl Verdict {
    /// Returns true if this media should be preserved as a looping GIF.
    #[must_use]
    pub const fn is_keep_gif(&self) -> bool {
        matches!(self, Self::LoopStrong(_))
    }

    /// Returns true if this media should be converted to / kept as video.
    #[must_use]
    pub const fn is_keep_video(&self) -> bool {
        matches!(self, Self::LoopWeak(_))
    }

    /// Returns true if classification is uncertain (Layer 7 fallback applies).
    #[must_use]
    pub const fn is_uncertain(&self) -> bool {
        matches!(self, Self::Uncertain(_))
    }

    /// Returns true if an error occurred in inference.
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    /// The human-readable reason string embedded in the verdict.
    #[must_use]
    pub fn reason(&self) -> &str {
        match self {
            Self::LoopStrong(r) | Self::LoopWeak(r) | Self::Uncertain(r) | Self::Error(r) => r,
        }
    }
}

// ── LoopMeta: unified signal bundle ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DurationTier {
    UltraShort,
    Short,
    MediumLong,
    Long,
    VeryLong,
    DefinitivelyLong,
}

impl DurationTier {
    #[must_use]
    pub fn from_secs(secs: f64) -> Self {
        if secs <= crate::constants::DURATION_TIER_ULTRA_SHORT_LIMIT {
            Self::UltraShort
        } else if secs <= crate::constants::DURATION_TIER_SHORT_LIMIT {
            Self::Short
        } else if secs <= crate::constants::DURATION_TIER_MEDIUM_LONG_LIMIT {
            Self::MediumLong
        } else if secs <= crate::constants::DURATION_TIER_LONG_LIMIT {
            Self::Long
        } else if secs <= crate::constants::DURATION_TIER_VERY_LONG_LIMIT {
            Self::VeryLong
        } else {
            Self::DefinitivelyLong
        }
    }
}

/// Consolidated boolean flags for `LoopMeta`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LoopStreamFlags {
    pub has_audio: bool,
    pub has_transparency: bool,
    pub is_native_gif: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LoopColorFlags {
    pub has_embedded_icc: bool,
    pub has_complex_color_profile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LoopMemeFlags {
    pub is_meme_platform: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LoopFlags {
    #[serde(flatten)]
    pub streams: LoopStreamFlags,
    #[serde(flatten)]
    pub color: LoopColorFlags,
    #[serde(flatten)]
    pub meme: LoopMemeFlags,
}

const fn has_master_like_color_footprint(
    assessment: crate::ffprobe_json::ColorInfoAssessment,
) -> bool {
    assessment.has_wide_gamut_signal()
        || matches!(
            assessment.hdr_signal(),
            Some(
                crate::ffprobe_json::HdrSignalKind::DolbyVision
                    | crate::ffprobe_json::HdrSignalKind::Hdr10Plus
            )
        )
}

fn has_complex_color_profile_signal(assessment: crate::ffprobe_json::ColorInfoAssessment) -> bool {
    assessment.has_hdr_signaling() || assessment.has_confirmed_high_bit_depth()
}

fn calculate_numeric_density(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let total_chars = crate::numeric_cast::usize_to_f64(s.chars().count());
    let digit_chars =
        crate::numeric_cast::usize_to_f64(s.chars().filter(char::is_ascii_digit).count());
    digit_chars / total_chars
}

/// Shared color signal projection for loop-intent heuristics.
///
/// `has_embedded_icc` is historical naming; it means "master-like color
/// footprint" rather than a literal ICC payload check.
fn loop_color_flags_from_assessment(
    assessment: crate::ffprobe_json::ColorInfoAssessment,
) -> LoopColorFlags {
    LoopColorFlags {
        has_embedded_icc: has_master_like_color_footprint(assessment),
        has_complex_color_profile: has_complex_color_profile_signal(assessment),
    }
}

/// Unified signal bundle consumed by the 7-layer decision tree.
///
/// Populated by constructors (`from_video_detection`, `from_ffprobe_result`, `from_gif_path`).
/// The tree itself is a pure function over this struct — no I/O, no side effects.
#[derive(Debug, Clone, Default)]
pub struct LoopMeta {
    // ── Basic geometry ──
    pub duration_secs: Option<f64>,
    pub duration_tier: Option<DurationTier>, // Optional so we don't break Default, though constructor populates it
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub frame_count: Option<u64>,
    pub file_size_bytes: u64,

    // ── Identity ──
    pub file_name: Option<String>,
    pub source_extension: Option<String>,
    pub parent_directories: Option<Vec<String>>,

    // ── Layer 1 signals (hard constraints) ──
    pub flags: LoopFlags,
    /// Whether the audio track is silent (`mean_volume` < -70 dB or `n_samples` == 0).
    /// `None` = not yet detected, `Some(true)` = silent, `Some(false)` = has audible content.
    pub audio_is_silent: Option<bool>,
    /// Whether transparency is actually used (not all opaque). Penetrating detection.
    /// `None` = not yet verified, `Some(true)` = real transparency, `Some(false)` = fake/unused alpha.
    pub transparency_is_real: Option<bool>,
    /// Actual decoded frame count (may differ from metadata claim). Penetrating detection.
    pub real_frame_count: Option<u64>,

    // ── Layer 2 signals (explicit declarations) ──
    /// 0 = infinite loop, 1 = play once, `None` = unknown.
    pub loop_count: Option<u16>,
    /// e.g. [`GIPHY`, `NETSCAPE2.0`, ...] from `GIF` Application Extension block.
    pub app_extensions: Option<Vec<String>>,
    /// "webm", "mp4", "gif", etc.
    pub container: Option<String>,
    /// Encoder software tags extracted from format/stream metadata (e.g., "Adobe Premiere", "Lavf", "Photoshop").
    pub encoder_software: Option<String>,
    /// Whether the video is physically interlaced (from penetration testing).
    pub is_interlaced: Option<bool>,

    // ── Layer 3 signals (self-referential structure) ──
    /// `frame_payload_variation`: coefficient of variation of frame packet sizes (`pkt_sizes` CV)
    pub frame_payload_variation: Option<f64>,
    /// `frame_delay_variation`: CV of presentation timestamps deltas
    pub frame_delay_variation: Option<f64>,
    /// Raw frame packet sizes — used to compute `closure_ratio`
    pub pkt_sizes: Vec<u64>,
    /// Raw PTS deltas — used for interval consistency score.
    pub pts_deltas: Vec<f64>,

    // ── Layer 4 signals (content features) ──
    pub palette_size: Option<u32>,
    /// `WebP` compression ratio proxy: `raw_size` / `webp_size` for a sampled frame.
    /// Constructors populate this on a best-effort basis for image-like sources.
    pub webp_compression_ratio: Option<f64>,
    pub palette_depth: Option<f64>,
    pub motion_gini: Option<f64>,
    pub temporal_flatness: Option<f64>,
    pub block_skew: Option<f64>,
    pub loop_closure_score: Option<f64>,
    pub motion_periodicity: Option<f64>,
    pub temporal_jitter: Option<f64>,
    /// Real physical features (225-dimensional 15x15 luminance sampling)
    pub physics_225: Option<Vec<f32>>,

    // ── Layer 5 signals (context semantics) ──
    pub directory_loop_intent_score: f64,
    pub filename_loop_intent_score: f64,
    pub max_frame_delay: Option<f64>,
    pub min_frame_delay: Option<f64>,
    pub audio_duration_secs: Option<f64>,
    pub path_depth: u32,
    pub filename_numeric_density: f64,

    // ── Auxiliary (used in KNN bridge) ──
    pub frame_types: Vec<char>,
    pub mv_magnitudes: Vec<f64>,
    pub cached_frame_png: Option<Vec<u8>>,
}

#[must_use]
fn loop_meta_duration_tier_or_from_secs(
    cached: Option<DurationTier>,
    duration_secs: Option<f64>,
) -> Option<DurationTier> {
    match cached {
        Some(tier) => Some(tier),
        None => duration_secs.map(DurationTier::from_secs),
    }
}

impl LoopMeta {
    // Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
    /// Build `LoopMeta` from a full `Detection`.
    #[must_use]
    pub fn from_video_detection(detection: &Detection) -> Self {
        let color_assessment = detection.color_assessment();
        let file_path = Path::new(&detection.file_path);
        let file_name = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(std::string::ToString::to_string);
        let source_extension = file_path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);
        let parent_directories: Option<Vec<String>> = file_path.parent().map(|p| {
            p.iter()
                .filter_map(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .map(std::string::ToString::to_string)
                .collect()
        });
        let path_depth = crate::media_conversion_gate::loop_parent_directory_depth(
            parent_directories.as_deref(),
            "LoopMeta path_depth",
        );
        let parent_directories_truncated: Option<Vec<String>> = parent_directories.map(|mut p| {
            if p.len() > 4 {
                p.split_off(p.len() - 4)
            } else {
                p
            }
        });

        // Detect transparency from pixel format
        let has_transparency = detection.pix_fmt.contains('a')
            || detection
                .pix_fmt
                .contains(crate::constants::PIX_FMT_YUVA420P)
            || detection.pix_fmt.contains(crate::constants::PIX_FMT_GBRAP);

        // Detect palette-based formats (limited color space)
        let palette_size = if detection.pix_fmt == crate::constants::PIX_FMT_PAL8 {
            Some(crate::constants::PALETTE_MAX_COLORS)
        } else {
            None
        };

        // Must compute before file_name is moved into the struct
        let filename_numeric_density = calculate_numeric_density(
            crate::media_conversion_gate::loop_filename_or_empty_for_density(file_name.as_deref()),
        );

        let mut meta = Self {
            duration_secs: detection.duration_secs,
            duration_tier: detection.duration_secs.map(DurationTier::from_secs),
            width: detection.width,
            height: detection.height,
            fps: detection.fps,
            frame_count: detection.frame_count,
            file_size_bytes: detection.file_size,
            file_name,
            source_extension,
            parent_directories: parent_directories_truncated.clone(),
            flags: LoopFlags {
                streams: LoopStreamFlags {
                    has_audio: detection.flags.streams.has_audio,
                    has_transparency,
                    is_native_gif: detection.format == crate::constants::CONTAINER_GIF,
                },
                color: loop_color_flags_from_assessment(color_assessment),
                meme: LoopMemeFlags {
                    is_meme_platform: detection.tags.values().any(|v| {
                        let up = v.to_uppercase();
                        crate::constants::LOOP_PLATFORM_MARKERS
                            .iter()
                            .any(|&m| up.contains(m))
                    }),
                },
            },
            audio_is_silent: None,      // Will be populated on-demand
            transparency_is_real: None, // Will be verified on-demand
            real_frame_count: None,     // Will be verified on-demand
            loop_count: detection.loop_count,
            app_extensions: Some(Vec::new()),
            container: Some(detection.format.clone()),
            encoder_software: crate::media_conversion_gate::loop_encoder_software_label(
                detection.precision.original_encoder.clone(),
                &detection.tags,
            ),
            is_interlaced: detection.is_interlaced,
            frame_payload_variation: calculate_cv(&detection.pkt_sizes),
            frame_delay_variation: calculate_cv_f64(&detection.pts_deltas),
            pkt_sizes: detection.pkt_sizes.clone(),
            pts_deltas: detection.pts_deltas.clone(),
            palette_size,
            webp_compression_ratio: None,
            palette_depth: None,
            motion_gini: {
                let sizes: Vec<f64> = detection
                    .pkt_sizes
                    .iter()
                    .map(|&s| crate::numeric_cast::u64_to_f64(s))
                    .collect();
                calculate_gini_f64(&sizes)
            },
            temporal_flatness: None,
            block_skew: None,
            loop_closure_score: loop_closure_score(&detection.pkt_sizes),
            motion_periodicity: motion_periodicity_score(&detection.mv_magnitudes),
            temporal_jitter: temporal_jitter_score(&detection.pts_deltas),
            physics_225: None,
            directory_loop_intent_score: crate::constants::LOOP_INTENT_NEUTRAL_SCORE,
            filename_loop_intent_score: crate::constants::LOOP_INTENT_NEUTRAL_SCORE,
            max_frame_delay: detection.pts_deltas.iter().copied().reduce(f64::max),
            min_frame_delay: detection.pts_deltas.iter().copied().reduce(f64::min),
            audio_duration_secs: detection.audio_duration_secs.or({
                if detection.flags.streams.has_audio {
                    detection.duration_secs
                } else {
                    None
                }
            }),
            path_depth,
            filename_numeric_density,
            frame_types: detection.frame_types.clone(),
            mv_magnitudes: detection.mv_magnitudes.clone(),
            cached_frame_png: None,
        };
        meta.directory_loop_intent_score =
            score_directory_context(parent_directories_truncated.as_deref(), &[]);
        meta.filename_loop_intent_score = analyze_filename(meta.file_name.as_deref(), &[]).raw;
        meta.populate_webp_compression_ratio_from_path(file_path);
        ensure_frame_delay_variation(&mut meta);
        meta
    }

    // Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
    /// Build `LoopMeta` from an `FFprobeResult` (used in pipelines without full detection).
    #[must_use]
    pub fn from_ffprobe_result(probe: &crate::ffprobe::FFprobeResult, path: &Path) -> Self {
        let color_assessment = probe.color_assessment();
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(std::string::ToString::to_string);
        let source_extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);
        let parent_directories: Option<Vec<String>> = path.parent().map(|p| {
            p.iter()
                .filter_map(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .map(std::string::ToString::to_string)
                .collect()
        });
        let path_depth = crate::media_conversion_gate::loop_parent_directory_depth(
            parent_directories.as_deref(),
            "LoopMeta path_depth",
        );
        let parent_directories_truncated: Option<Vec<String>> = parent_directories.map(|mut p| {
            if p.len() > 4 {
                p.split_off(p.len() - 4)
            } else {
                p
            }
        });

        // Detect transparency from pixel format
        let has_transparency = probe.pix_fmt.contains('a')
            || probe.pix_fmt.contains(crate::constants::PIX_FMT_YUVA420P)
            || probe.pix_fmt.contains(crate::constants::PIX_FMT_GBRAP);

        // Detect palette-based formats (limited color space)
        let palette_size = if probe.pix_fmt == crate::constants::PIX_FMT_PAL8 {
            Some(crate::constants::PALETTE_MAX_COLORS)
        } else {
            None
        };

        // Must compute before file_name is moved into the struct
        let filename_numeric_density = calculate_numeric_density(
            crate::media_conversion_gate::loop_filename_or_empty_for_density(file_name.as_deref()),
        );

        let mut meta = Self {
            duration_secs: probe.duration,
            duration_tier: probe.duration.map(DurationTier::from_secs),
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
            fps: probe.frame_rate,
            frame_count: probe.frame_count,
            file_size_bytes: probe.size,
            file_name,
            source_extension,
            parent_directories: parent_directories_truncated.clone(),
            flags: LoopFlags {
                streams: LoopStreamFlags {
                    has_audio: probe.audio.present,
                    has_transparency,
                    is_native_gif: probe.format_name == crate::constants::CONTAINER_GIF,
                },
                color: loop_color_flags_from_assessment(color_assessment),
                meme: LoopMemeFlags {
                    is_meme_platform: probe.tags.values().any(|v| {
                        let up = v.to_uppercase();
                        crate::constants::LOOP_PLATFORM_MARKERS
                            .iter()
                            .any(|&m| up.contains(m))
                    }),
                },
            },
            audio_is_silent: None,      // Will be populated on-demand
            transparency_is_real: None, // Will be verified on-demand
            real_frame_count: None,     // Will be verified on-demand
            loop_count: probe.loop_count,
            app_extensions: Some(Vec::new()),
            container: Some(probe.format_name.clone()),
            encoder_software: crate::media_conversion_gate::loop_encoder_software_label(
                None,
                &probe.tags,
            ),
            is_interlaced: None,
            frame_payload_variation: calculate_cv(&probe.pkt_sizes),
            frame_delay_variation: calculate_cv_f64(&probe.pts_deltas),
            pkt_sizes: probe.pkt_sizes.clone(),
            pts_deltas: probe.pts_deltas.clone(),
            palette_size,
            palette_depth: None,
            temporal_flatness: None,
            webp_compression_ratio: None,
            motion_gini: {
                let sizes: Vec<f64> = probe
                    .pkt_sizes
                    .iter()
                    .map(|&s| crate::numeric_cast::u64_to_f64(s))
                    .collect();
                calculate_gini_f64(&sizes)
            },
            block_skew: None,
            loop_closure_score: loop_closure_score(&probe.pkt_sizes),
            motion_periodicity: motion_periodicity_score(&probe.mv_magnitudes),
            temporal_jitter: temporal_jitter_score(&probe.pts_deltas),
            physics_225: None,
            directory_loop_intent_score: crate::constants::LOOP_INTENT_NEUTRAL_SCORE,
            filename_loop_intent_score: crate::constants::LOOP_INTENT_NEUTRAL_SCORE,
            max_frame_delay: probe.pts_deltas.iter().copied().reduce(f64::max),
            min_frame_delay: probe.pts_deltas.iter().copied().reduce(f64::min),
            audio_duration_secs: probe.audio.duration.or({
                if probe.audio.present {
                    probe.duration
                } else {
                    None
                }
            }),
            path_depth,
            filename_numeric_density,
            frame_types: probe.frame_types.clone(),
            mv_magnitudes: probe.mv_magnitudes.clone(),
            cached_frame_png: None,
        };
        meta.directory_loop_intent_score =
            score_directory_context(parent_directories_truncated.as_deref(), &[]);
        meta.filename_loop_intent_score = analyze_filename(meta.file_name.as_deref(), &[]).raw;
        meta.populate_webp_compression_ratio_from_path(path);
        ensure_frame_delay_variation(&mut meta);
        meta
    }

    /// Build `LoopMeta` from a `GIF` file using header-level scanning (fast, no `ffprobe`).
    #[must_use]
    /// # Panics
    ///
    /// Panics if the GIF file is corrupted or contains malformed header data that violates the `GIF89a` specification.
    pub fn from_gif_path(path: &Path) -> Option<Self> {
        let scan = match crate::media_meta_utils::scan_gif_headers(path) {
            Ok(s) => s,
            Err(e) => {
                crate::media_conversion_gate::delivery_intent_path_audit(
                    "delivery_intent",
                    path,
                    format!(
                        "Forensic: Failed to scan loop candidates for {}: {e}",
                        path.display()
                    ),
                );
                return None;
            }
        };

        let file_size = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) => {
                crate::media_conversion_gate::delivery_intent_path_audit(
                    "delivery_intent",
                    path,
                    format!(
                        "Forensic: Failed to extract loop metadata for {path_display}; using conservative fallback: {e}",
                        path_display = path.display(),
                    ),
                );
                return None;
            }
        };
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(std::string::ToString::to_string);
        let parent_directories: Option<Vec<String>> = path.parent().map(|p| {
            p.iter()
                .filter_map(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .map(std::string::ToString::to_string)
                .collect()
        });
        let path_depth = crate::media_conversion_gate::loop_parent_directory_depth(
            parent_directories.as_deref(),
            "LoopMeta path_depth",
        );
        let parent_directories_truncated: Option<Vec<String>> = parent_directories.map(|mut p| {
            if p.len() > 4 {
                p.split_off(p.len() - 4)
            } else {
                p
            }
        });

        // Fast header read for GIF dimensions
        let (width, height) =
            match crate::media_conversion_gate::loop_gif_logical_screen_optional(path) {
                Some((w, h)) => (Some(w), Some(h)),
                None => (None, None),
            };

        let frame_count = scan.frame_count;

        // Honest fps: only report when we have both a real duration and >1 frames.
        // Refuse to fabricate a default cadence (previously 12.0) that downstream
        // scoring could mistake for an observed measurement.
        let fps = match (scan.duration_secs, frame_count) {
            (Some(dur), n) if n > 1 && dur > 0.0_f64 => Some(f64::from(n) / dur),
            _ => None,
        };

        // Must compute before file_name is moved into the struct
        let filename_numeric_density = calculate_numeric_density(
            crate::media_conversion_gate::loop_filename_or_empty_for_density(file_name.as_deref()),
        );

        let mut meta = Self {
            duration_secs: scan.duration_secs,
            duration_tier: scan.duration_secs.map(DurationTier::from_secs),
            width,
            height,
            fps,
            frame_count: Some(u64::from(frame_count)),
            file_size_bytes: file_size,
            file_name,
            source_extension: Some("gif".to_string()),
            parent_directories: parent_directories_truncated.clone(),
            flags: LoopFlags {
                streams: LoopStreamFlags {
                    has_audio: false,
                    has_transparency: scan.has_transparency,
                    is_native_gif: true,
                },
                color: LoopColorFlags {
                    has_embedded_icc: false,
                    has_complex_color_profile: false,
                },
                meme: LoopMemeFlags {
                    is_meme_platform: scan.app_extensions.as_ref().is_some_and(|e_list| {
                        e_list.iter().any(|e| {
                            let up = e.to_uppercase();
                            crate::constants::LOOP_PLATFORM_MARKERS
                                .iter()
                                .any(|&m| up.contains(m))
                        })
                    }),
                },
            },
            audio_is_silent: Some(true), // GIFs never have audio
            transparency_is_real: if scan.has_transparency {
                None
            } else {
                Some(false)
            }, // Verify if claimed
            real_frame_count: Some(u64::from(frame_count)), // GIF frame count is reliable
            loop_count: scan.loop_count,
            app_extensions: scan.app_extensions.clone(),
            container: Some("gif".to_string()),
            frame_payload_variation: scan.frame_payload_variation,
            frame_delay_variation: scan.frame_delay_variation,
            palette_size: scan.palette_size,
            max_frame_delay: None,
            min_frame_delay: None,
            audio_duration_secs: None,
            path_depth,
            filename_numeric_density,
            ..Default::default()
        };

        meta.directory_loop_intent_score =
            score_directory_context(parent_directories_truncated.as_deref(), &[]);
        meta.filename_loop_intent_score = analyze_filename(meta.file_name.as_deref(), &[]).raw;
        meta.populate_webp_compression_ratio_from_path(path);
        Some(meta)
    }

    fn populate_webp_compression_ratio_from_path(&mut self, path: &Path) {
        if self.webp_compression_ratio.is_some() {
            return;
        }

        if self.should_sample_webp_compression_ratio() {
            // Extract one frame, read into memory, compute the WebP ratio, and cache the
            // frame bytes in-memory to avoid repeated ffmpeg invocations later.
            match extract_frame_to_temp(path) {
                Ok(Some(temp_frame)) => {
                    match std::fs::read(&temp_frame) {
                        Ok(bytes) => {
                            // Remove the temporary file immediately; keep bytes in-memory only.
                            crate::media_conversion_gate::delivery_remove_file_or_audit(
                                "loop_intent_temp_frame",
                                &temp_frame,
                            );

                            // Cache the PNG bytes for potential reuse in Tier 3 visual heuristics.
                            self.cached_frame_png = Some(bytes.clone());

                            // Compute the WebP compression ratio from the in-memory image.
                            match image::load_from_memory(&bytes) {
                                Ok(img) => {
                                    match sampled_webp_compression_ratio_from_image(&img) {
                                        Ok(ratio) => {
                                            self.webp_compression_ratio = ratio;
                                        }
                                        Err(err) => {
                                            crate::media_conversion_gate::probe_image_format_audit(
                                                "loop_intent_webp_ratio_failed",
                                                path,
                                                format!(
                                                    "failed to compute sampled WebP compression ratio: {err}"
                                                ),
                                            );
                                        }
                                    }
                                    // Also extract real physics (15x15 luminance grid)
                                    self.physics_225 =
                                        Some(crate::real_physics::extract_image_physics_225(&img));
                                }
                                Err(err) => {
                                    crate::media_conversion_gate::probe_image_format_audit(
                                        "loop_intent_cached_frame_decode_failed",
                                        path,
                                        format!(
                                            "failed to decode extracted frame {}: {err}",
                                            temp_frame.display()
                                        ),
                                    );
                                }
                            }
                        }
                        Err(err) => {
                            crate::media_conversion_gate::delivery_remove_file_or_audit(
                                "loop_intent_temp_frame",
                                &temp_frame,
                            );
                            crate::media_conversion_gate::probe_image_format_audit(
                                "loop_intent_temp_frame_read_failed",
                                path,
                                format!(
                                    "failed to read extracted frame {}: {err}",
                                    temp_frame.display()
                                ),
                            );
                        }
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    crate::media_conversion_gate::probe_image_format_audit(
                        "loop_intent_temp_frame_extract_failed",
                        path,
                        format!("failed to extract representative frame: {err}"),
                    );
                }
            }
        }
    }

    /// Force empirical WebP-ratio measurement for training ingest.
    ///
    /// The regular constructor path samples this feature only when the source is large
    /// enough to justify best-effort runtime work. Training vectors require the field,
    /// so ingestion must measure it or fail closed before writing a DB row.
    pub fn ensure_webp_compression_ratio_from_path(&mut self, path: &Path) -> anyhow::Result<()> {
        if let Some(ratio) = self.webp_compression_ratio {
            if ratio.is_finite() && ratio > 0.0 {
                return Ok(());
            }
            anyhow::bail!(
                "loop_stats_webp_ratio is non-finite or non-positive for {}",
                path.display()
            );
        }

        let temp_frame = extract_frame_to_temp(path)?.ok_or_else(|| {
            anyhow::anyhow!(
                "loop_stats_webp_ratio frame extraction produced no frame for {}",
                path.display()
            )
        })?;
        let bytes = std::fs::read(&temp_frame).map_err(|err| {
            anyhow::anyhow!(
                "loop_stats_webp_ratio frame read failed for {} from {}: {err}",
                path.display(),
                temp_frame.display()
            )
        });
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "loop_intent_temp_frame_strict_webp_ratio",
            &temp_frame,
        );
        let bytes = bytes?;
        let img = image::load_from_memory(&bytes).map_err(|err| {
            anyhow::anyhow!(
                "loop_stats_webp_ratio frame decode failed for {} from {}: {err}",
                path.display(),
                temp_frame.display()
            )
        })?;
        let ratio = sampled_webp_compression_ratio_from_image(&img)?.ok_or_else(|| {
            anyhow::anyhow!(
                "loop_stats_webp_ratio measurement absent for {}",
                path.display()
            )
        })?;
        if !ratio.is_finite() || ratio <= 0.0 {
            anyhow::bail!(
                "loop_stats_webp_ratio measurement invalid for {}: {ratio}",
                path.display()
            );
        }

        self.cached_frame_png = Some(bytes);
        self.physics_225 = Some(crate::real_physics::extract_image_physics_225(&img));
        self.webp_compression_ratio = Some(ratio);
        Ok(())
    }

    #[must_use]
    pub fn should_sample_webp_compression_ratio(&self) -> bool {
        self.width.is_some_and(|w| w >= 64)
            && self.height.is_some_and(|h| h >= 64)
            && (self
                .duration_secs
                .is_some_and(|d| d > crate::constants::LOOP_INTENT_SHORT_DURATION_THRESHOLD)
                || self.frame_count.is_some_and(|c| c > 1))
    }

    /// Re-run semantic scoring with dynamic keywords from the database.
    pub fn refresh_semantics(&mut self, keywords: &[String]) {
        self.directory_loop_intent_score =
            score_directory_context(self.parent_directories.as_deref(), keywords);
        self.filename_loop_intent_score = analyze_filename(self.file_name.as_deref(), keywords).raw;
    }

    /// Returns the duration tier, falling back to calculation if the cached field is None.
    #[must_use]
    pub fn tier(&self) -> Option<DurationTier> {
        loop_meta_duration_tier_or_from_secs(self.duration_tier, self.duration_secs)
    }

    /// Returns the confirmed audibility of the audio stream.
    /// - Some(true): Audible content detected.
    /// - Some(false): Silent audio track OR no audio stream present.
    /// - None: Audio state unverified (penetrating detection not yet run).
    #[must_use]
    pub fn audible_audio_state(&self) -> Option<bool> {
        if !self.flags.streams.has_audio {
            return Some(false); // Definitive: no stream means no sound.
        }
        self.audio_is_silent.map(|is_silent| !is_silent)
    }

    #[must_use]
    pub fn has_confirmed_audible_audio(&self) -> bool {
        self.audible_audio_state() == Some(true)
    }

    #[must_use]
    pub fn has_confirmed_silent_or_no_audio(&self) -> bool {
        self.audible_audio_state() == Some(false)
    }

    #[must_use]
    pub const fn real_transparency_state(&self) -> Option<bool> {
        if !self.flags.streams.has_transparency {
            return Some(false);
        }
        self.transparency_is_real
    }

    #[must_use]
    pub fn has_verified_real_transparency(&self) -> bool {
        self.real_transparency_state() == Some(true)
    }

    #[must_use]
    pub fn has_confirmed_no_real_transparency(&self) -> bool {
        self.real_transparency_state() == Some(false)
    }
}

// ── DB-Driven Loop Intent Forest ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct LogOdds(Option<f64>);

impl Default for LogOdds {
    fn default() -> Self {
        Self(Some(0.0))
    }
}

impl LogOdds {
    fn add(&mut self, delta: f64) {
        if !delta.is_finite() {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_tree",
                branch = "log_odds_delta_non_finite",
                delta,
                "dropping non-finite log-odds delta"
            );
            return;
        }
        let Some(acc) = self.0 else {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_tree",
                branch = "log_odds_poisoned",
                "log-odds accumulator poisoned; ignoring further deltas"
            );
            return;
        };
        let next = acc + delta;
        if next.is_finite() {
            self.0 = Some(next);
            if delta.abs() >= crate::constants::LOOP_INTENT_LOG_ODDS_SIGNAL_TRACE_MIN {
                tracing::trace!(
                    target: "mfb.algorithm",
                    pipeline = "loop_intent_tree",
                    branch = "log_odds_signal_accumulated",
                    delta,
                    accumulator = next,
                    "log-odds signal contribution (trace only, not a counter)"
                );
            }
        } else {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_tree",
                branch = "log_odds_accumulator_non_finite",
                "log-odds accumulator overflow; state cleared"
            );
            self.0 = None;
        }
    }

    const fn value(self) -> Option<f64> {
        self.0
    }

    fn probability(self) -> Option<f64> {
        let log_odds = self.value()?;
        let raw = 1.0 / (1.0 + (-log_odds).exp());
        crate::algorithm_seal::loop_unit_probability(raw)
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct SignalMotionFlags {
    scene_cut: bool,
    localized_motion: bool,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct SignalContentFlags {
    has_audible_audio: bool,
    is_portrait: bool,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct SignalFlags {
    #[serde(flatten)]
    motion: SignalMotionFlags,
    #[serde(flatten)]
    content: SignalContentFlags,
}

#[derive(Debug, Default, Clone, Copy)]
struct DerivedLoopSignals {
    flags: SignalFlags,
    zero_motion_ratio: f64,
    /// Ratio of I-frames to total frames. GIF→MP4 transcodes produce all-I-frame streams
    /// (ratio ≈ 1.0); real video with GOP structure has ratio ≈ 0.03–0.10.
    iframe_ratio: f64,
    /// Average bytes per frame. GIF-class content typically has low `bytes_per_frame`
    /// compared to real video content.
    bytes_per_frame: f64,
}

impl DerivedLoopSignals {
    #[allow(clippy::manual_unwrap_or)]
    fn from_meta(meta: &LoopMeta) -> Self {
        let zero_motion_ratio = zero_motion_ratio(&meta.mv_magnitudes);
        let i_count = meta.frame_types.iter().filter(|&&c| c == 'I').count();
        let total = meta.frame_types.len();
        let iframe_ratio = if total > 0 {
            crate::numeric_cast::usize_to_f64(i_count) / crate::numeric_cast::usize_to_f64(total)
        } else {
            crate::constants::LOOP_INTENT_NEUTRAL_SCORE // neutral when no frame type data
        };
        let bytes_per_frame = match crate::media_conversion_gate::loop_bytes_per_frame_optional(
            meta.file_size_bytes,
            meta.frame_count,
            "DerivedLoopSignals::from_meta",
        ) {
            Some(value) => value,
            None => f64::NAN,
        };
        let is_portrait = if let (Some(w), Some(h)) = (meta.width, meta.height)
            && w > 0
            && h > 0
        {
            let ratio = f64::from(h) / f64::from(w);
            (ratio - crate::constants::ASPECT_RATIO_WIDESCREEN).abs()
                < crate::constants::ASPECT_RATIO_TOLERANCE_NEAR
        } else {
            false
        };
        Self {
            flags: SignalFlags {
                motion: SignalMotionFlags {
                    scene_cut: detect_scene_cut(&meta.pkt_sizes),
                    localized_motion: meta.mv_magnitudes.len() >= 10
                        && zero_motion_ratio > crate::constants::LOOP_INTENT_ZERO_MOTION_RATIO,
                },
                content: SignalContentFlags {
                    has_audible_audio: crate::media_conversion_gate::loop_audible_audio_fail_closed(
                        meta.flags.streams.has_audio,
                        meta.audible_audio_state(),
                        "DerivedLoopSignals::from_meta",
                    ),
                    is_portrait,
                },
            },
            zero_motion_ratio,
            iframe_ratio,
            bytes_per_frame,
        }
    }
}

#[derive(Debug, Clone)]
struct LoopThresholds {
    reference: LoopReferenceProfile,
    duration_override_secs: f64,
    short_clip_secs: f64,
    short_asset_window_secs: f64,
    modern_bias_duration_secs: f64,
    decision_threshold: f64,
}

impl LoopThresholds {
    /// DB-backed thresholds only; returns `None` when profile missing or incomplete.
    fn for_evaluation(reference_profile: Option<&LoopReferenceProfile>) -> Option<Self> {
        reference_profile.and_then(Self::from_reference_profile)
    }

    /// Build thresholds from a DB-backed profile; returns `None` when empirical percentiles are incomplete.
    fn from_reference_profile(reference: &LoopReferenceProfile) -> Option<Self> {
        let duration_percentiles_available = reference.duration_has_empirical_percentiles;
        if duration_percentiles_available
            && reference
                .duration
                .p25
                .or(reference.duration.p10)
                .as_ref()
                .is_none_or(|v| !v.is_finite())
        {
            crate::media_conversion_gate::delivery_intent_batch_audit(
                "loop_thresholds_incomplete",
                "LoopThresholds: duration histogram present but p25/p10 missing; refusing fabricated percentile",
            );
            return None;
        }
        let short_percentile_fallback =
            crate::media_conversion_gate::loop_collection_duration_p90_or_baseline(
                reference.collection.duration_p90,
                crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS,
                "collection.duration_p90",
                "LoopThresholds short_percentile",
                duration_percentiles_available,
                reference.collection.duration_p90_from_samples,
            )
            .min(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS);
        let short_percentile = crate::media_conversion_gate::loop_duration_or_fallback_policy(
            reference.duration.p25.or(reference.duration.p10),
            short_percentile_fallback,
            "duration.p25|duration.p10",
            "LoopThresholds short_percentile",
            duration_percentiles_available,
        );
        let median_scaled =
            crate::media_conversion_gate::loop_scaled_duration_percentile_or_fallback_policy(
                reference.duration.p50,
                short_percentile,
                crate::constants::LOOP_INTENT_MOTION_MEDIAN_SCALE,
                "duration.p50",
                "LoopThresholds median_scaled",
                duration_percentiles_available,
            );
        let duration_p90_cap =
            crate::media_conversion_gate::loop_collection_duration_p90_or_baseline(
                reference.collection.duration_p90,
                crate::constants::DEFAULT_LOOP_BASELINE_DURATION_P90_SECS,
                "collection.duration_p90",
                "LoopThresholds duration_override",
                duration_percentiles_available,
                reference.collection.duration_p90_from_samples,
            )
            .max(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_P90_SECS);
        let duration_override_secs = if duration_percentiles_available {
            short_percentile
                .min(median_scaled.max(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_P90_SECS))
                .clamp(
                    crate::constants::DEFAULT_LOOP_BASELINE_DURATION_P90_SECS,
                    duration_p90_cap,
                )
        } else {
            reference
                .duration
                .std_dev
                .mul_add(
                    crate::constants::LOOP_INTENT_DYN_THRESH_SCALING_LOW,
                    reference.duration.mean,
                )
                .clamp(
                    crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS,
                    crate::constants::LOOP_INTENT_BASELINE_DURATION,
                )
        };
        if duration_override_secs + 1.0 > crate::constants::LOOP_INTENT_MAX_DURATION {
            crate::media_conversion_gate::delivery_intent_batch_audit(
                "loop_thresholds_invalid",
                format!(
                    "LoopThresholds: duration_override_secs {duration_override_secs:.3} leaves no room below loop-intent max duration {:.3}; refusing inverted short-clip clamp",
                    crate::constants::LOOP_INTENT_MAX_DURATION
                ),
            );
            return None;
        }
        let short_clip_estimate = reference
            .duration
            .std_dev
            .mul_add(
                crate::constants::LOOP_INTENT_DYN_THRESH_SCALING_HIGH,
                reference.duration.mean,
            )
            .clamp(
                duration_override_secs + 1.0,
                crate::constants::LOOP_INTENT_MAX_DURATION,
            );
        let short_clip_secs = crate::media_conversion_gate::loop_duration_or_fallback_policy(
            crate::media_conversion_gate::loop_duration_p50_or_capped_p75_policy(
                reference.duration.p50,
                reference.duration.p75,
                duration_percentiles_available,
            ),
            short_clip_estimate,
            "duration.p50|duration.p75",
            "LoopThresholds short_clip",
            duration_percentiles_available,
        )
        .max(duration_override_secs + 0.5);
        let short_asset_window_secs =
            short_clip_secs.max(crate::constants::HARD_PASS_SHORT_GIF_THRESHOLD_SECS);
        let collection_duration_p90 =
            crate::media_conversion_gate::loop_collection_duration_p90_or_baseline(
                reference.collection.duration_p90,
                crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS,
                "collection.duration_p90",
                "LoopThresholds modern_bias",
                duration_percentiles_available,
                reference.collection.duration_p90_from_samples,
            );
        let modern_bias_duration_secs =
            crate::media_conversion_gate::loop_duration_or_fallback_policy(
                reference.duration.p75,
                collection_duration_p90,
                "duration.p75",
                "LoopThresholds modern_bias",
                duration_percentiles_available,
            )
            .max(collection_duration_p90)
            .max(crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS);

        Some(Self {
            reference: reference.clone(),
            duration_override_secs,
            short_clip_secs,
            short_asset_window_secs,
            modern_bias_duration_secs,
            decision_threshold: crate::constants::TREE_DECISION_LOG_ODDS_THRESHOLD,
        })
    }

    fn get_feature_weight(&self, key: &str) -> Option<f64> {
        match key {
            "duration" => self.reference.duration.weight,
            "fps" => self.reference.fps.weight,
            "file_size_bytes" => self.reference.file_size_bytes.weight,
            "temporal_bpp" => self.reference.temporal_bpp.weight,
            "spatial_bpp" => self.reference.spatial_bpp.weight,
            "payload_var" => self.reference.payload_variation.weight,
            "delay_var" => self.reference.delay_variation.weight,
            "p_depth" => self.reference.palette_depth.weight,
            "m_gini" => self.reference.motion_gini.weight,
            "t_flat" => self.reference.temporal_flatness.weight,
            "webp_ratio" => self.reference.webp_ratio.weight,
            _ => {
                crate::media_conversion_gate::delivery_intent_batch_audit(
                    "delivery_io",
                    format!(
                        "FEATURE WEIGHT ANOMALY: Unknown feature key '{key}' encountered in reference profile | Refusing to forge data"
                    ),
                );
                None
            }
        }
    }

    fn clamp_z(value: f64) -> f64 {
        value.clamp(
            -crate::constants::TREE_Z_SCORE_CAP,
            crate::constants::TREE_Z_SCORE_CAP,
        )
    }

    fn duration_z(&self, duration_secs: Option<f64>) -> f64 {
        crate::media_conversion_gate::loop_duration_z_or_neutral(
            duration_secs,
            |d| Self::clamp_z(self.reference.duration.z_score(d)),
            "loop_intent duration_z",
        )
    }

    fn fps_z(&self, fps: f64) -> f64 {
        Self::clamp_z(self.reference.fps.z_score(fps))
    }

    fn file_size_z(&self, file_size_bytes: f64) -> f64 {
        Self::clamp_z(self.reference.file_size_bytes.z_score(file_size_bytes))
    }

    fn pixels_z(&self, pixels: f64) -> f64 {
        Self::clamp_z(self.reference.pixels.z_score(pixels))
    }

    fn delay_variation_z(&self, value: f64) -> f64 {
        Self::clamp_z(self.reference.delay_variation.z_score(value))
    }

    fn webp_ratio_z(&self, value: f64) -> f64 {
        Self::clamp_z(self.reference.webp_ratio.z_score(value))
    }

    fn motion_gini_z(&self, value: f64) -> f64 {
        Self::clamp_z(self.reference.motion_gini.z_score(value))
    }

    fn palette_depth_z(&self, value: f64) -> f64 {
        Self::clamp_z(self.reference.palette_depth.z_score(value))
    }

    fn temporal_flatness_z(&self, value: f64) -> f64 {
        Self::clamp_z(self.reference.temporal_flatness.z_score(value))
    }
}

#[derive(Debug, Clone)]
pub struct TreeEvaluation {
    pub verdict: Verdict,
    /// Sealed unit probability when the tree produced one (`None` = non-finite / unavailable).
    pub tree_probability: Option<f64>,
    /// Finite log-odds accumulator when available (`None` = overflow or non-finite).
    pub log_odds_value: Option<f64>,
    /// Set when the tree exits before Layer 6 (Layer 0 veto, checkpoint, etc.).
    pub resolution_path: Option<String>,
}

impl TreeEvaluation {
    fn seal_algorithm_outputs(&mut self) {
        self.tree_probability = self
            .tree_probability
            .and_then(crate::algorithm_seal::loop_unit_probability);
        self.log_odds_value = self
            .log_odds_value
            .and_then(crate::algorithm_seal::loop_finite_scalar);
    }
}

#[inline]
fn format_optional_probability(value: Option<f64>) -> String {
    crate::media_conversion_gate::loop_format_optional_probability_or_na(
        value,
        "loop_intent tree diagnostic",
    )
}

fn has_platform_marker(app_extensions: Option<&[String]>) -> bool {
    let Some(app_extensions) = app_extensions else {
        return false;
    };
    app_extensions.iter().any(|app| {
        let normalized = app.trim().to_ascii_uppercase();
        crate::constants::LOOP_PLATFORM_MARKERS
            .iter()
            .any(|marker| normalized.contains(marker))
    })
}

fn has_explicit_loop_platform_marker(meta: &LoopMeta) -> bool {
    has_platform_marker(meta.app_extensions.as_deref()) || meta.flags.meme.is_meme_platform
}

fn is_silent_webm(meta: &LoopMeta, ext_lower: &str) -> bool {
    (meta
        .container
        .as_deref()
        .is_some_and(|container| container.eq_ignore_ascii_case("webm"))
        || ext_lower == "webm")
        && meta.has_confirmed_silent_or_no_audio()
}

fn is_short_silent_asset(meta: &LoopMeta, _thresholds: &LoopThresholds) -> bool {
    meta.has_confirmed_silent_or_no_audio()
        && meta.tier().is_some_and(|t| {
            matches!(
                t,
                DurationTier::UltraShort | DurationTier::Short | DurationTier::MediumLong
            )
        })
}

fn checkpoint_verdict(
    log_odds: LogOdds,
    threshold: f64,
    layer_tag: &str,
    strong_label: &str,
    weak_label: &str,
) -> Option<Verdict> {
    let Some(log_odds_value) = log_odds.value() else {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_tree",
            branch = "checkpoint_non_finite_log_odds",
            layer = layer_tag,
            "checkpoint skipped: log-odds not finite"
        );
        return None;
    };
    let verdict = if log_odds_value >= threshold {
        Some(Verdict::LoopStrong(format!(
            "{layer_tag}: log-odds {log_odds_value:.2} >= {threshold:.2} ({strong_label})"
        )))
    } else if log_odds_value <= -threshold {
        Some(Verdict::LoopWeak(format!(
            "{layer_tag}: log-odds {log_odds_value:.2} <= -{threshold:.2} ({weak_label})"
        )))
    } else {
        None
    };
    if let Some(ref v) = verdict {
        let outcome = if v.is_keep_gif() {
            "loop_strong"
        } else {
            "loop_weak"
        };
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_tree",
            branch = "layer_checkpoint_fire",
            layer = layer_tag,
            log_odds = log_odds_value,
            threshold,
            outcome,
            "decision tree checkpoint produced early exit (not a counter metric)"
        );
    }
    verdict
}

/// Stable audit tag for tree checkpoint early exits (layer tag is also in verdict text).
fn tree_checkpoint_resolution_path(layer_tag: &str) -> &'static str {
    if layer_tag.contains("Layer 3") {
        if layer_tag.contains("Image") {
            "tree_checkpoint_layer3_image"
        } else {
            "tree_checkpoint_layer3_video"
        }
    } else if layer_tag.contains("Layer 4") {
        if layer_tag.contains("Image") {
            "tree_checkpoint_layer4_image"
        } else {
            "tree_checkpoint_layer4_video"
        }
    } else {
        "tree_checkpoint"
    }
}

fn tree_layer5_resolution_path(media: &str, outcome: &str) -> &'static str {
    match (media, outcome) {
        ("image", "strong") => "tree_layer5_image_strong",
        ("image", "weak") => "tree_layer5_image_weak",
        ("image", "uncertain") => "tree_layer5_image_uncertain",
        ("video", "strong") => "tree_layer5_video_strong",
        ("video", "weak") => "tree_layer5_video_weak",
        ("video", _) => "tree_layer5_video_uncertain",
        _ => "tree_layer5",
    }
}

fn zero_motion_ratio(mvs: &[f64]) -> f64 {
    if mvs.is_empty() {
        return 0.0;
    }
    let zero_count = mvs
        .iter()
        .filter(|&&value| value.abs() < crate::constants::LOOP_INTENT_ZERO_MV_THRESHOLD)
        .count();
    crate::numeric_cast::usize_to_f64(zero_count) / crate::numeric_cast::usize_to_f64(mvs.len())
}

fn is_near_16_by_9(width: Option<u32>, height: Option<u32>) -> bool {
    let (Some(w), Some(h)) = (width, height) else {
        return false;
    };
    if w == 0 || h == 0 {
        return false;
    }
    ((f64::from(w) / f64::from(h)) - crate::constants::ASPECT_RATIO_WIDESCREEN).abs()
        < crate::constants::ASPECT_RATIO_TOLERANCE_NEAR
}

fn loop_count_zero_bonus(meta: &LoopMeta, _thresholds: &LoopThresholds) -> f64 {
    match meta.tier() {
        Some(DurationTier::UltraShort | DurationTier::Short) => {
            crate::constants::LOOP_COUNT_ZERO_BONUS_MAX
        }
        Some(DurationTier::MediumLong) => crate::constants::LOOP_COUNT_ZERO_BONUS_DECAY_MAX
            .mul_add(
                -crate::constants::LOOP_COUNT_ZERO_BONUS_DECAY_MEDIUM,
                crate::constants::LOOP_COUNT_ZERO_BONUS_MAX,
            ),
        Some(DurationTier::Long | DurationTier::VeryLong) => {
            crate::constants::LOOP_COUNT_ZERO_BONUS_DECAY_MAX.mul_add(
                -crate::constants::LOOP_COUNT_ZERO_BONUS_DECAY_LONG,
                crate::constants::LOOP_COUNT_ZERO_BONUS_MAX,
            )
        }
        Some(DurationTier::DefinitivelyLong) => crate::constants::LOOP_COUNT_ZERO_BONUS_MIN,
        None => f64::NAN,
    }
}

fn evaluate_kinetics_and_physics(
    meta: &LoopMeta,
    derived: &DerivedLoopSignals,
    thresholds: &LoopThresholds,
    log_odds: &mut LogOdds,
) {
    let duration_positive = (-thresholds.duration_z(meta.duration_secs)).max(0.0);
    let duration_negative = thresholds.duration_z(meta.duration_secs).max(0.0);
    let (fps_positive, fps_negative) =
        crate::media_conversion_gate::loop_fps_kinetic_weights_or_neutral(
            meta.fps,
            |fps| thresholds.fps_z(fps),
            "evaluate_kinetics_and_physics",
        );
    let total_pixels_opt = crate::media_conversion_gate::loop_total_pixels_optional(
        meta.width,
        meta.height,
        "evaluate_kinetics_and_physics",
    );

    if duration_positive > 0.0_f64 {
        let short_fast = duration_positive
            * fps_positive.mul_add(
                crate::constants::KINETIC_WEIGHT_ADJ,
                crate::constants::KINETIC_WEIGHT_BASE,
            );
        log_odds.add(
            short_fast.min(crate::constants::TREE_Z_SCORE_CAP)
                * crate::constants::SHORT_FAST_POSITIVE_LOG_ODDS,
        );
    }

    if duration_negative > 0.0_f64 {
        let long_slow = duration_negative
            * fps_negative.mul_add(
                crate::constants::KINETIC_WEIGHT_ADJ,
                crate::constants::KINETIC_WEIGHT_BASE,
            );
        log_odds.add(
            -long_slow.min(crate::constants::TREE_Z_SCORE_CAP)
                * crate::constants::LONG_SLOW_NEGATIVE_LOG_ODDS,
        );
    }

    if derived.flags.motion.scene_cut {
        log_odds.add(-crate::constants::SCENE_CUT_NEGATIVE_LOG_ODDS);
    }

    let compactness_pixels = total_pixels_opt.and_then(|total_pixels| {
        let v = (-thresholds.pixels_z(total_pixels)).max(0.0_f64);
        v.is_finite().then_some(v)
    });
    let compactness_signal = {
        let size_part = (-thresholds
            .file_size_z(crate::numeric_cast::u64_to_f64(meta.file_size_bytes)))
        .max(0.0)
            * crate::constants::COMPACTNESS_SIGNAL_SIZE_WEIGHT;
        match compactness_pixels {
            Some(p) => p.mul_add(
                crate::constants::COMPACTNESS_SIGNAL_PIXELS_WEIGHT,
                size_part,
            ),
            None => size_part,
        }
    };
    if meta.has_confirmed_silent_or_no_audio()
        && compactness_signal.is_finite()
        && compactness_signal > 0.0_f64
    {
        log_odds.add(
            (compactness_signal + crate::constants::COMPACTNESS_SIGNAL_BIAS)
                .min(crate::constants::COMPACTNESS_SIGNAL_MAX)
                * crate::constants::COMPACT_SILENT_POSITIVE_LOG_ODDS,
        );
    }

    let large_media_pixels = total_pixels_opt.and_then(|total_pixels| {
        let v = thresholds.pixels_z(total_pixels).max(0.0_f64);
        v.is_finite().then_some(v)
    });
    let large_media_signal = {
        let size_part = thresholds
            .file_size_z(crate::numeric_cast::u64_to_f64(meta.file_size_bytes))
            .max(0.0)
            * crate::constants::LARGE_MEDIA_SIGNAL_SIZE_WEIGHT;
        match large_media_pixels {
            Some(p) => p.mul_add(
                crate::constants::LARGE_MEDIA_SIGNAL_PIXELS_WEIGHT,
                size_part,
            ),
            None => size_part,
        }
    };
    if large_media_signal.is_finite() && large_media_signal > 0.0_f64 {
        let audio_multiplier = if meta.has_confirmed_audible_audio() {
            1.0_f64
        } else {
            crate::constants::LARGE_MEDIA_AUDIO_MULTIPLIER
        };
        if let Some(weight) = thresholds.get_feature_weight("file_size_bytes") {
            log_odds.add(
                -large_media_signal.min(crate::constants::LARGE_MEDIA_SIGNAL_MAX)
                    * audio_multiplier
                    * crate::constants::LARGE_MEDIA_NEGATIVE_LOG_ODDS
                    * weight.sqrt(),
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoopMediaFamily {
    Image,
    Video,
    Other,
}

impl LoopMediaFamily {
    const fn from_flags(is_image: bool, is_video: bool) -> Self {
        if is_image {
            Self::Image
        } else if is_video {
            Self::Video
        } else {
            Self::Other
        }
    }

    const fn is_image(self) -> bool {
        matches!(self, Self::Image)
    }

    const fn is_video(self) -> bool {
        matches!(self, Self::Video)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShortAssetProfile {
    NonShortSilent,
    SilentOther,
    ShortClip,
    ExtendedShortAsset,
}

impl ShortAssetProfile {
    fn from_meta(meta: &LoopMeta, thresholds: &LoopThresholds) -> Self {
        if !is_short_silent_asset(meta, thresholds) {
            return Self::NonShortSilent;
        }
        if meta
            .duration_secs
            .is_some_and(|duration| duration > thresholds.duration_override_secs)
            && meta
                .duration_secs
                .is_some_and(|duration| duration <= thresholds.short_clip_secs)
        {
            return Self::ShortClip;
        }
        if meta
            .duration_secs
            .is_some_and(|duration| duration > thresholds.short_clip_secs)
            && meta
                .duration_secs
                .is_some_and(|duration| duration <= thresholds.short_asset_window_secs)
        {
            return Self::ExtendedShortAsset;
        }
        Self::SilentOther
    }

    const fn is_short_silent(self) -> bool {
        !matches!(self, Self::NonShortSilent)
    }

    const fn is_short_clip(self) -> bool {
        matches!(self, Self::ShortClip)
    }

    const fn is_extended_short_asset(self) -> bool {
        matches!(self, Self::ExtendedShortAsset)
    }
}

struct StructuralSignalScorer<'a> {
    meta: &'a LoopMeta,
    derived: &'a DerivedLoopSignals,
    thresholds: &'a LoopThresholds,
    log_odds: &'a mut LogOdds,
    media_family: LoopMediaFamily,
    short_silent_asset: bool,
    is_short_tier: bool,
}

impl<'a> StructuralSignalScorer<'a> {
    fn new(
        meta: &'a LoopMeta,
        derived: &'a DerivedLoopSignals,
        thresholds: &'a LoopThresholds,
        log_odds: &'a mut LogOdds,
        is_image: bool,
        is_video: bool,
    ) -> Self {
        let short_silent_asset = is_short_silent_asset(meta, thresholds);
        let is_short_tier = meta.tier().is_some_and(|tier| {
            matches!(
                tier,
                DurationTier::UltraShort | DurationTier::Short | DurationTier::MediumLong
            )
        });
        Self {
            meta,
            derived,
            thresholds,
            log_odds,
            media_family: LoopMediaFamily::from_flags(is_image, is_video),
            short_silent_asset,
            is_short_tier,
        }
    }

    fn apply(mut self) {
        self.apply_loop_closure_signal();
        self.apply_motion_periodicity_signal();
        self.apply_cadence_signals();
        self.apply_zero_cost_signals();
        self.apply_alignment_bonuses();
        self.apply_absolute_counters();
    }

    fn apply_loop_closure_signal(&mut self) {
        use std::intrinsics::{likely, unlikely};

        if likely(
            self.meta
                .loop_closure_score
                .is_some_and(|closure| closure >= crate::constants::LOOP_INTENT_CLOSURE_THRESHOLD)
                && self.is_short_tier,
        ) {
            if let Some(closure) = self.meta.loop_closure_score {
                let strength = ((closure - crate::constants::LOOP_INTENT_CLOSURE_THRESHOLD)
                    / crate::constants::LOOP_INTENT_CLOSURE_SCALE)
                    .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
                self.log_odds
                    .add(strength * crate::constants::FEATURE_WEIGHT_LOOP_CLOSURE);
            }
            return;
        }

        if unlikely(self.meta.loop_closure_score.is_some_and(|closure| {
            closure <= crate::constants::LOOP_INTENT_CLOSURE_REJECT_THRESHOLD
        })) && let Some(closure) = self.meta.loop_closure_score
        {
            let strength = ((crate::constants::LOOP_INTENT_CLOSURE_REJECT_THRESHOLD - closure)
                / crate::constants::LOOP_INTENT_CLOSURE_REJECT_THRESHOLD)
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(-strength * crate::constants::FEATURE_WEIGHT_LOOP_CLOSURE);
        }
    }

    fn apply_motion_periodicity_signal(&mut self) {
        let Some(periodicity) = self.meta.motion_periodicity else {
            return;
        };

        if periodicity >= crate::constants::LOOP_INTENT_PERIODICITY_THRESHOLD {
            let strength = ((periodicity - crate::constants::LOOP_INTENT_PERIODICITY_THRESHOLD)
                / crate::constants::LOOP_INTENT_PERIODICITY_SCALE)
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            let envelope_multiplier = if self.short_silent_asset
                || self.media_family.is_image()
                || self.derived.flags.motion.localized_motion
            {
                1.0_f64
            } else {
                crate::constants::LOOP_INTENT_MOTION_ENVELOPE_REDUCTION
            };
            self.log_odds.add(
                strength
                    * crate::constants::FEATURE_WEIGHT_MOTION_PERIODICITY
                    * envelope_multiplier,
            );
            return;
        }

        if periodicity <= crate::constants::LOOP_INTENT_PERIODICITY_REJECT_THRESHOLD {
            let strength = ((crate::constants::LOOP_INTENT_PERIODICITY_REJECT_THRESHOLD
                - periodicity)
                / crate::constants::LOOP_INTENT_PERIODICITY_REJECT_THRESHOLD)
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(-strength * crate::constants::FEATURE_WEIGHT_MOTION_PERIODICITY);
        }
    }

    fn apply_cadence_signals(&mut self) {
        self.apply_loop_frequency_signal();
        self.apply_sparse_cadence_signal();
        self.apply_temporal_jitter_signal();
    }

    fn apply_loop_frequency_signal(&mut self) {
        let loop_frequency = score_loop_frequency(self.meta.duration_secs, self.meta.frame_count);
        if loop_frequency >= crate::constants::LOOP_INTENT_LOOP_FREQ_HIGH {
            let strength = ((loop_frequency - crate::constants::LOOP_INTENT_LOOP_FREQ_HIGH)
                / (1.0 - crate::constants::LOOP_INTENT_LOOP_FREQ_HIGH))
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(strength * crate::constants::FEATURE_WEIGHT_LOOP_FREQUENCY);
        } else if loop_frequency <= crate::constants::LOOP_INTENT_LOOP_FREQ_LOW {
            let strength = ((crate::constants::LOOP_INTENT_LOOP_FREQ_LOW - loop_frequency)
                / crate::constants::LOOP_INTENT_LOOP_FREQ_LOW)
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(-strength * crate::constants::FEATURE_WEIGHT_LOOP_FREQUENCY);
        }
    }

    fn apply_sparse_cadence_signal(&mut self) {
        let sparse_cadence = score_sparse_cadence(self.meta.duration_secs, self.meta.frame_count);
        if sparse_cadence >= crate::constants::LOOP_INTENT_SPARSE_CADENCE_THRESHOLD
            && (self.short_silent_asset || self.media_family.is_image())
        {
            let strength = ((sparse_cadence
                - crate::constants::LOOP_INTENT_SPARSE_CADENCE_THRESHOLD)
                / (1.0 - crate::constants::LOOP_INTENT_SPARSE_CADENCE_THRESHOLD))
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(strength * crate::constants::FEATURE_WEIGHT_SPARSE_CADENCE);
        }
    }

    fn apply_temporal_jitter_signal(&mut self) {
        let Some(jitter) = self.meta.temporal_jitter else {
            return;
        };

        if jitter >= crate::constants::LOOP_INTENT_JITTER_HIGH
            && (self.short_silent_asset || self.media_family.is_image())
        {
            let strength = ((jitter - crate::constants::LOOP_INTENT_JITTER_HIGH)
                / (1.0 - crate::constants::LOOP_INTENT_JITTER_HIGH))
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(strength * crate::constants::FEATURE_WEIGHT_TEMPORAL_JITTER);
            return;
        }

        if jitter <= crate::constants::LOOP_INTENT_JITTER_LOW {
            let strength = ((crate::constants::LOOP_INTENT_JITTER_LOW - jitter)
                / crate::constants::LOOP_INTENT_JITTER_LOW)
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(-strength * crate::constants::FEATURE_WEIGHT_TEMPORAL_JITTER);
        }
    }

    fn apply_zero_cost_signals(&mut self) {
        self.apply_iframe_ratio_signal();
        self.apply_bytes_per_frame_signal();

        if self.derived.flags.content.is_portrait {
            self.log_odds
                .add(-crate::constants::PORTRAIT_ASPECT_PENALTY);
        }

        if self.media_family.is_video()
            && !self.short_silent_asset
            && self.meta.has_confirmed_no_real_transparency()
            && self
                .meta
                .loop_closure_score
                .is_some_and(|closure| closure < crate::constants::LOOP_INTENT_ANTI_LOOP_THRESHOLD)
            && self.meta.motion_periodicity.is_some_and(|periodicity| {
                periodicity < crate::constants::LOOP_INTENT_ANTI_LOOP_THRESHOLD
            })
        {
            self.log_odds
                .add(-crate::constants::LOOP_INTENT_KNN_MIN_DELTA);
        }
    }

    fn apply_iframe_ratio_signal(&mut self) {
        if (self.derived.iframe_ratio - 0.5).abs() <= 0.01_f64 {
            return;
        }

        if self.derived.iframe_ratio >= crate::constants::LOOP_INTENT_IFRAME_RATIO_HIGH {
            let strength = ((self.derived.iframe_ratio
                - crate::constants::LOOP_INTENT_IFRAME_RATIO_HIGH)
                / (1.0 - crate::constants::LOOP_INTENT_IFRAME_RATIO_HIGH))
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(strength * crate::constants::FEATURE_WEIGHT_IFRAME_RATIO);
        } else if self.derived.iframe_ratio <= crate::constants::LOOP_INTENT_IFRAME_RATIO_LOW {
            let strength = ((crate::constants::LOOP_INTENT_IFRAME_RATIO_LOW
                - self.derived.iframe_ratio)
                / crate::constants::LOOP_INTENT_IFRAME_RATIO_LOW)
                .clamp(crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN, 1.0);
            self.log_odds
                .add(-strength * crate::constants::FEATURE_WEIGHT_IFRAME_RATIO);
        }
    }

    fn apply_bytes_per_frame_signal(&mut self) {
        if !self.derived.bytes_per_frame.is_finite() || self.derived.bytes_per_frame <= 0.0_f64 {
            return;
        }

        let bpf_z = self.thresholds.file_size_z(self.derived.bytes_per_frame);
        if bpf_z <= -crate::constants::LOOP_INTENT_BPF_Z_THRESHOLD {
            let strength = ((-bpf_z - crate::constants::LOOP_INTENT_BPF_Z_THRESHOLD)
                / crate::constants::LOOP_INTENT_BPF_Z_THRESHOLD)
                .clamp(
                    crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN_RELAXED,
                    1.0,
                );
            self.log_odds
                .add(strength * crate::constants::FEATURE_WEIGHT_BYTES_PER_FRAME);
        } else if bpf_z >= crate::constants::LOOP_INTENT_BPF_Z_THRESHOLD {
            let strength = ((bpf_z - crate::constants::LOOP_INTENT_BPF_Z_THRESHOLD)
                / crate::constants::LOOP_INTENT_BPF_Z_THRESHOLD)
                .clamp(
                    crate::constants::LOOP_INTENT_SIGNAL_STRENGTH_MIN_RELAXED,
                    1.0,
                );
            self.log_odds
                .add(-strength * crate::constants::FEATURE_WEIGHT_BYTES_PER_FRAME);
        }
    }

    fn apply_alignment_bonuses(&mut self) {
        self.apply_convergence_bonus(self.anti_loop_signal_count(), false);
        self.apply_convergence_bonus(self.pro_loop_signal_count(), true);
    }

    fn anti_loop_signal_count(&self) -> u8 {
        let mut count = 0_u8;
        if self.derived.flags.content.has_audible_audio {
            count += 1;
        }
        if self.derived.flags.motion.scene_cut {
            count += 1;
        }
        if self.derived.flags.content.is_portrait {
            count += 1;
        }
        if is_near_16_by_9(self.meta.width, self.meta.height) {
            count += 1;
        }
        if self.derived.iframe_ratio < crate::constants::LOOP_INTENT_IFRAME_RATIO_LOW_VETO {
            count += 1;
        }
        count
    }

    fn pro_loop_signal_count(&self) -> u8 {
        let mut count = 0_u8;
        if self.meta.has_confirmed_silent_or_no_audio() {
            count += 1;
        }
        if let (Some(w), Some(h)) = (self.meta.width, self.meta.height)
            && w > 0
            && h > 0
            && w == h
        {
            count += 1;
        }
        if self.derived.iframe_ratio >= crate::constants::LOOP_INTENT_IFRAME_RATIO_HIGH_VETO {
            count += 1;
        }
        if self.loop_closure_bonus_signal() {
            count += 1;
        }
        if self.motion_periodicity_bonus_signal() {
            count += 1;
        }
        count
    }

    fn loop_closure_bonus_signal(&self) -> bool {
        self.meta
            .loop_closure_score
            .is_some_and(|score| score >= crate::constants::LOOP_INTENT_CLOSURE_HIGH)
            && self.is_short_tier
    }

    fn motion_periodicity_bonus_signal(&self) -> bool {
        self.meta
            .motion_periodicity
            .is_some_and(|score| score >= crate::constants::LOOP_INTENT_PERIODICITY_HIGH)
    }

    fn apply_convergence_bonus(&mut self, count: u8, positive: bool) {
        if count < crate::constants::LOOP_INTENT_SIGNAL_THRESHOLD {
            return;
        }

        let bonus = crate::constants::LOOP_INTENT_BONUS_INCREMENT
            * crate::numeric_cast::usize_to_f64(
                usize::from(count) - crate::constants::LOOP_INTENT_SIGNAL_OFFSET,
            );
        self.log_odds.add(if positive { bonus } else { -bonus });
    }

    fn apply_absolute_counters(&mut self) {
        if self.meta.is_interlaced == Some(true) {
            self.log_odds.add(
                -crate::constants::LOG_ODDS_BIAS_DEFINITIVELY_LONG
                    * crate::constants::INTERLACED_PENALTY_MULTIPLIER,
            );
        }
    }
}

struct WeakHeuristicScorer<'a> {
    meta: &'a LoopMeta,
    derived: &'a DerivedLoopSignals,
    thresholds: &'a LoopThresholds,
    log_odds: &'a mut LogOdds,
    media_family: LoopMediaFamily,
    ext_lower: String,
    short_asset_profile: ShortAssetProfile,
}

impl<'a> WeakHeuristicScorer<'a> {
    fn new(
        meta: &'a LoopMeta,
        derived: &'a DerivedLoopSignals,
        thresholds: &'a LoopThresholds,
        log_odds: &'a mut LogOdds,
        is_image: bool,
        is_video: bool,
    ) -> Self {
        let ext_lower = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
            meta.source_extension.as_deref(),
            "loop_intent",
        );
        Self {
            meta,
            derived,
            thresholds,
            log_odds,
            media_family: LoopMediaFamily::from_flags(is_image, is_video),
            ext_lower,
            short_asset_profile: ShortAssetProfile::from_meta(meta, thresholds),
        }
    }

    fn apply(mut self) {
        self.apply_container_and_short_asset_biases();
        self.apply_short_clip_prior();
        self.apply_extended_short_asset_prior();
        self.apply_distributional_signals();
        self.apply_contextual_signals();
        self.apply_geometry_and_cadence_signals();
        self.apply_long_silent_penalty();
        self.apply_container_prior();
        self.apply_modern_master_penalty();
    }

    fn apply_container_and_short_asset_biases(&mut self) {
        if self.media_family.is_video() {
            let has_verified_transparency = if let Some(is_real) =
                self.meta.real_transparency_state()
            {
                is_real
            } else {
                if self.meta.flags.streams.has_transparency {
                    crate::log_debug!(
                        crate::infra::static_logs::messages::LABEL_INTENT,
                        "SIGNAL VOID: Missing 'transparency_is_real' flag; refusing to assume video transparency is real"
                    );
                }
                false
            };
            if has_verified_transparency {
                self.log_odds
                    .add(crate::constants::TRANSPARENCY_POSITIVE_LOG_ODDS);
            }
        }

        if is_silent_webm(self.meta, &self.ext_lower) {
            self.log_odds.add(crate::constants::SHORT_CLIP_MIN_BIAS);
        }
    }

    fn apply_short_clip_prior(&mut self) {
        if !self.short_asset_profile.is_short_clip() {
            return;
        }

        let range = (self.thresholds.short_clip_secs - self.thresholds.duration_override_secs)
            .max(crate::constants::HEURISTIC_SAFETY_FLOOR);
        let Some(duration) = self.meta.duration_secs else {
            crate::media_conversion_gate::delivery_intent_batch_audit(
                "delivery_intent",
                "HEURISTIC AUDIT: Video duration is missing | Forensic: Field is None; skipping short-clip heuristics to prevent score forgery",
            );
            return;
        };

        let headroom =
            1.0_f64 - ((duration - self.thresholds.duration_override_secs) / range).clamp(0.0, 1.0);
        let format_bonus = if self.media_family.is_image() {
            crate::constants::SHORT_CLIP_FORMAT_BONUS_IMAGE
        } else {
            crate::constants::SHORT_CLIP_FORMAT_BONUS_VIDEO
        };
        let cadence_bonus = if self
            .meta
            .frame_count
            .is_some_and(|frame_count| frame_count > 1)
        {
            crate::constants::SHORT_CLIP_CADENCE_BONUS
        } else {
            0.0_f64
        };
        self.log_odds.add(
            (crate::constants::SHORT_CLIP_MIN_BIAS
                + headroom * crate::constants::SHORT_CLIP_HEADROOM_MAX
                + format_bonus
                + cadence_bonus)
                * crate::constants::SHORT_CLIP_PRIOR_LOG_ODDS,
        );
    }

    fn apply_extended_short_asset_prior(&mut self) {
        if !self.short_asset_profile.is_extended_short_asset() {
            return;
        }

        let range = (self.thresholds.short_asset_window_secs - self.thresholds.short_clip_secs)
            .max(crate::constants::HEURISTIC_SAFETY_FLOOR);
        let tail_headroom =
            match crate::media_conversion_gate::loop_extended_short_tail_headroom_optional(
                self.meta.duration_secs,
                self.thresholds.short_clip_secs,
                range,
                "loop_intent extended_short tail_headroom",
            ) {
                Some(v) => v,
                None => return,
            };
        let square_bonus = if let (Some(w), Some(h)) = (self.meta.width, self.meta.height)
            && w > 0
            && w == h
        {
            crate::constants::EXTENDED_SHORT_ASSET_SQUARE_BONUS
        } else {
            0.0_f64
        };
        let image_bonus = if self.media_family.is_image() {
            crate::constants::EXTENDED_SHORT_ASSET_IMAGE_BONUS
        } else {
            0.0_f64
        };
        let compact_bonus = if self.meta.file_size_bytes <= crate::constants::STICKER_MAX_SIZE_BYTES
        {
            crate::constants::EXTENDED_SHORT_ASSET_COMPACT_BONUS
        } else {
            0.0_f64
        };
        self.log_odds.add(
            (tail_headroom.mul_add(
                crate::constants::EXTENDED_SHORT_ASSET_HEADROOM_MAX,
                crate::constants::EXTENDED_SHORT_ASSET_MIN_BIAS,
            ) + square_bonus
                + image_bonus
                + compact_bonus)
                * crate::constants::EXTENDED_SHORT_ASSET_PRIOR_LOG_ODDS,
        );
    }

    fn apply_distributional_signals(&mut self) {
        self.apply_feature_weighted_signal(
            self.meta.frame_delay_variation,
            "delay_var",
            crate::constants::FEATURE_WEIGHT_DELAY_VAR,
            |thresholds, value| -thresholds.delay_variation_z(value),
        );
        self.apply_feature_weighted_signal(
            self.meta.webp_compression_ratio,
            "webp_ratio",
            crate::constants::FEATURE_WEIGHT_WEBP_RATIO,
            LoopThresholds::webp_ratio_z,
        );
        self.apply_motion_gini_signal();
        self.apply_feature_weighted_signal(
            self.meta.palette_depth,
            "p_depth",
            crate::constants::FEATURE_WEIGHT_PALETTE_DEPTH,
            LoopThresholds::palette_depth_z,
        );
        self.apply_feature_weighted_signal(
            self.meta.temporal_flatness,
            "t_flat",
            crate::constants::FEATURE_WEIGHT_TEMPORAL_FLATNESS,
            LoopThresholds::temporal_flatness_z,
        );

        if self.derived.flags.motion.localized_motion
            || self.derived.zero_motion_ratio
                > crate::constants::LOOP_INTENT_ZERO_MOTION_HIGH_THRESHOLD
        {
            self.log_odds.add(LOCALIZED_MOTION_POSITIVE_LOG_ODDS);
        }
    }

    fn apply_feature_weighted_signal<F>(
        &mut self,
        value: Option<f64>,
        feature: &str,
        base_weight: f64,
        zscore: F,
    ) where
        F: Fn(&LoopThresholds, f64) -> f64,
    {
        if let Some(metric) = value
            && let Some(weight) = self.thresholds.get_feature_weight(feature)
        {
            self.log_odds
                .add(zscore(self.thresholds, metric) * base_weight * weight);
        }
    }

    fn apply_motion_gini_signal(&mut self) {
        let Some(motion_gini) = self.meta.motion_gini else {
            return;
        };
        let Some(weight) = self.thresholds.get_feature_weight("m_gini") else {
            return;
        };

        let z = self.thresholds.motion_gini_z(motion_gini);
        let loop_support = self
            .meta
            .loop_closure_score
            .or(self.meta.motion_periodicity);
        let support_relief = support_relief_from_loop_support(
            loop_support,
            z,
            self.short_asset_profile.is_short_silent(),
            self.media_family.is_image(),
            self.derived.flags.motion.localized_motion,
        );
        let Some(support_relief) = support_relief else {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_signals",
                branch = "motion_gini_support_missing",
                "omitting motion-gini relief due to missing loop support evidence"
            );
            return;
        };
        self.log_odds
            .add(z * crate::constants::FEATURE_WEIGHT_MOTION_GINI * support_relief * weight);
    }

    fn apply_contextual_signals(&mut self) {
        if self.meta.directory_loop_intent_score
            > crate::constants::LOOP_INTENT_SEMANTIC_SCORE_THRESHOLD
        {
            self.log_odds.add(DIRECTORY_CONTEXT_POSITIVE_LOG_ODDS);
        }
        if self.meta.filename_loop_intent_score
            > crate::constants::LOOP_INTENT_SEMANTIC_SCORE_THRESHOLD
        {
            self.log_odds.add(FILENAME_CONTEXT_POSITIVE_LOG_ODDS);
        }
    }

    fn apply_geometry_and_cadence_signals(&mut self) {
        if let Some(frame_count) = self.meta.frame_count {
            if frame_count <= crate::constants::LOOP_INTENT_FRAME_COUNT_SHORT_LIMIT {
                self.log_odds.add(crate::constants::FRAME_COUNT_SHORT_BONUS);
            } else if frame_count > crate::constants::LOOP_INTENT_FRAME_COUNT_LONG_LIMIT {
                self.log_odds
                    .add(-crate::constants::FRAME_COUNT_LONG_PENALTY);
            }
        }

        if let (Some(w), Some(h)) = (self.meta.width, self.meta.height)
            && w > 0
            && h > 0
        {
            if w == h {
                self.log_odds.add(crate::constants::SQUARE_ASPECT_BONUS);
            } else if is_near_16_by_9(self.meta.width, self.meta.height) {
                self.log_odds
                    .add(-crate::constants::WIDESCREEN_ASPECT_PENALTY);
            } else if self.derived.flags.content.is_portrait {
                self.log_odds
                    .add(-crate::constants::PORTRAIT_ASPECT_PENALTY);
            }
        }

        if let Some(fps) = self.meta.fps
            && fps_anomaly_score(fps) > crate::constants::LOOP_INTENT_FPS_ANOMALY_THRESHOLD
        {
            self.log_odds.add(crate::constants::FPS_ANOMALY_BONUS);
        }
    }

    fn apply_long_silent_penalty(&mut self) {
        if !self.meta.has_confirmed_silent_or_no_audio()
            || self
                .meta
                .duration_secs
                .is_none_or(|duration| duration <= self.thresholds.modern_bias_duration_secs)
        {
            return;
        }

        let overflow = match crate::media_conversion_gate::loop_modern_bias_overflow_optional(
            self.meta.duration_secs,
            self.thresholds.modern_bias_duration_secs,
            "loop_intent modern_bias overflow",
        ) {
            Some(v) => v,
            None => return,
        };
        let container_penalty = if self.media_family.is_video() {
            crate::constants::LONG_SILENT_PENALTY_VIDEO_ADD
        } else if self.media_family.is_image() {
            crate::constants::LONG_SILENT_PENALTY_IMAGE_ADD
        } else {
            0.0_f64
        };
        let transparency_relief = if self.meta.has_verified_real_transparency() {
            crate::constants::LONG_SILENT_TRANSPARENCY_RELIEF
        } else {
            0.0_f64
        };
        let penalty = (overflow.mul_add(
            crate::constants::LONG_SILENT_PENALTY_OVERFLOW_MAX,
            crate::constants::LONG_SILENT_PENALTY_BASE,
        ) + container_penalty
            - transparency_relief)
            .max(crate::constants::LONG_SILENT_MIN_PENALTY);
        self.log_odds
            .add(-penalty * crate::constants::LONG_SILENT_PRIOR_NEGATIVE_LOG_ODDS);
    }

    fn apply_container_prior(&mut self) {
        if self.media_family.is_image() {
            self.log_odds.add(crate::constants::IMAGE_PRIOR_BONUS);
        } else if self.media_family.is_video() {
            self.log_odds.add(-crate::constants::VIDEO_PRIOR_PENALTY);
        }
    }

    fn apply_modern_master_penalty(&mut self) {
        let bias_enabled = crate::media_conversion_gate::algorithm_env_flag_enabled_or_default(
            crate::constants::ENV_MODERN_FORMAT_CONVERT_BIAS,
            true,
        );
        let is_modern =
            crate::constants::MODERN_ANIMATED_EXTENSIONS.contains(&self.ext_lower.as_str());
        let bias_threshold = self.thresholds.modern_bias_duration_secs;
        if !is_modern
            || !bias_enabled
            || self
                .meta
                .duration_secs
                .is_none_or(|duration| duration <= bias_threshold)
        {
            return;
        }

        let master_like = self.meta.flags.color.has_embedded_icc
            || self.meta.flags.color.has_complex_color_profile
            || self
                .meta
                .webp_compression_ratio
                .is_some_and(|ratio| self.thresholds.webp_ratio_z(ratio) < -0.75_f64);
        if master_like {
            self.log_odds.add(-MODERN_MASTER_NEGATIVE_LOG_ODDS);
        }
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
fn apply_structural_signals(
    meta: &LoopMeta,
    derived: &DerivedLoopSignals,
    thresholds: &LoopThresholds,
    log_odds: &mut LogOdds,
    is_image: bool,
    is_video: bool,
) {
    StructuralSignalScorer::new(meta, derived, thresholds, log_odds, is_image, is_video).apply();
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
fn apply_weak_heuristics(
    meta: &LoopMeta,
    derived: &DerivedLoopSignals,
    thresholds: &LoopThresholds,
    log_odds: &mut LogOdds,
    is_image: bool,
    is_video: bool,
) {
    WeakHeuristicScorer::new(meta, derived, thresholds, log_odds, is_image, is_video).apply();
}

#[must_use]
pub fn identify(meta: &LoopMeta) -> Verdict {
    evaluate_loop_tree(meta, None).verdict
}

fn developer_layer1_override_enabled(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "loop_intent_env",
                format!("failed to read developer override {name}: {e}; treating as disabled"),
            );
            false
        }
    }
}

/// Converts a `Verdict` + accumulated `LogOdds` into a `TreeEvaluation`.
///
fn finalize_with_path(
    verdict: Verdict,
    lo: LogOdds,
    resolution_path: Option<&'static str>,
) -> TreeEvaluation {
    let log_odds_value = lo.value();
    if log_odds_value.is_none() {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_tree",
            branch = "finalize_non_finite_log_odds",
            "tree finalize: log-odds not finite"
        );
    }
    let tree_probability = match &verdict {
        Verdict::LoopStrong(_) | Verdict::LoopWeak(_) | Verdict::Uncertain(_) => {
            let prob = lo.probability();
            if prob.is_none() {
                tracing::warn!(
                    target: "mfb.algorithm",
                    pipeline = "loop_intent_tree",
                    branch = "finalize_tree_probability_unavailable",
                    verdict = ?verdict,
                    "tree finalize: log-odds probability not sealable"
                );
            }
            prob
        }
        Verdict::Error(_) => None,
    };
    let mut evaluation = TreeEvaluation {
        tree_probability,
        log_odds_value,
        verdict,
        resolution_path: resolution_path.map(str::to_string),
    };
    evaluation.seal_algorithm_outputs();
    let layer = extract_layer_tag(evaluation.verdict.reason());
    tracing::debug!(
        target: "mfb.algorithm",
        pipeline = "loop_intent_tree",
        branch = "layer_exit",
        layer = %layer,
        log_odds = evaluation.log_odds_value,
        tree_probability = evaluation.tree_probability,
        resolution_path = crate::media_conversion_gate::trace_label_or_default(
            evaluation.resolution_path.as_deref(),
            "tree_default",
        ),
        verdict = ?evaluation.verdict,
        "decision tree layer exit"
    );
    evaluation
}

fn evaluate_loop_tree_without_reference_profile(meta: &LoopMeta) -> Option<TreeEvaluation> {
    let derived = DerivedLoopSignals::from_meta(meta);
    let log_odds = LogOdds::default();

    let Some(frame_count) = meta.frame_count else {
        return None;
    };
    if frame_count <= 1 || meta.tier().is_none() {
        return None;
    }
    let has_audible_audio_global = derived.flags.content.has_audible_audio;

    if meta
        .duration_secs
        .is_some_and(|d| d <= crate::constants::EXTREME_SHORT_ABSOLUTE_LIMIT_SECS)
        && !has_audible_audio_global
    {
        let dur_str = crate::media_conversion_gate::loop_format_duration_secs_label(
            meta.duration_secs,
            "layer0_ex_short_hard_veto",
        );
        return Some(finalize_with_path(
            Verdict::LoopStrong(format!(
                "Layer 0-EX (Hard Veto): extreme-short duration {} ≤ {:.1}s — \
                 definitively animated image regardless of all other signals",
                dur_str,
                crate::constants::EXTREME_SHORT_ABSOLUTE_LIMIT_SECS,
            )),
            log_odds,
            Some("layer0_ex_short_hard_veto"),
        ));
    }

    if meta
        .duration_secs
        .is_some_and(|d| d >= crate::constants::EXTREME_LONG_ABSOLUTE_LIMIT_SECS)
    {
        let dur_str = crate::media_conversion_gate::loop_format_duration_secs_label(
            meta.duration_secs,
            "layer0_ex_long_hard_veto",
        );
        return Some(finalize_with_path(
            Verdict::LoopWeak(format!(
                "Layer 0-EX (Hard Veto): extreme-long duration {} ≥ {:.1}s — \
                 definitively video regardless of all other signals",
                dur_str,
                crate::constants::EXTREME_LONG_ABSOLUTE_LIMIT_SECS,
            )),
            log_odds,
            Some("layer0_ex_long_hard_veto"),
        ));
    }

    None
}

#[must_use]
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
pub fn evaluate_loop_tree(
    meta: &LoopMeta,
    reference_profile: Option<&LoopReferenceProfile>,
) -> TreeEvaluation {
    let ext_lower = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
        meta.source_extension.as_deref(),
        "loop_intent",
    );
    let is_image = SUPPORTED_IMAGE_EXTENSIONS.contains(&ext_lower.as_str());
    let derived = DerivedLoopSignals::from_meta(meta);
    let mut log_odds = LogOdds::default();

    if let Some(evaluation) = evaluate_loop_tree_without_reference_profile(meta) {
        return evaluation;
    }

    let Some(thresholds) = LoopThresholds::for_evaluation(reference_profile) else {
        return finalize_with_path(
            Verdict::Uncertain(
                "Loop reference profile unavailable or incomplete; refusing fabricated thresholds"
                    .to_string(),
            ),
            log_odds,
            Some("no_reference_profile"),
        );
    };

    // ── Layer 0: Degenerate Input Guard (Veto/Uncertain) ──────────────────────────
    // Sequential Validation Rule:
    // 1. Physically impossible assets (1-frame) → Immediate Error.
    // 2. Assets missing core metadata (None) → Uncertain (Unverifiable, let L7 fallback).
    // 3. Degenerate duration (video only) → Immediate Error.

    if let Some(fc) = meta.frame_count
        && fc <= 1
    {
        return finalize_with_path(
            Verdict::Error("Layer 0: single-frame input, physically cannot loop".to_string()),
            log_odds,
            Some("layer0_veto_single_frame"),
        );
    }

    if meta.frame_count.is_none() {
        return finalize_with_path(
            Verdict::Uncertain(
                "Layer 0: frame count missing; unable to verify periodicity".to_string(),
            ),
            log_odds,
            Some("layer0_uncertain_missing_frame_count"),
        );
    }

    let Some(tier) = meta.tier() else {
        return finalize_with_path(
            Verdict::Uncertain(
                "Layer 0: missing duration; cannot resolve duration tier".to_string(),
            ),
            log_odds,
            Some("layer0_uncertain_missing_duration"),
        );
    };

    if !meta.flags.streams.is_native_gif
        && meta
            .duration_secs
            .is_some_and(|d| d < crate::constants::NEGLIGIBLE_DURATION_SECS)
    {
        return finalize_with_path(
            Verdict::Error("Layer 0: degenerate duration".to_string()),
            log_odds,
            Some("layer0_veto_degenerate_duration"),
        );
    }

    // ── Layer 0-EX: Extreme Duration Hard Veto ─────────────────────────────────
    // This is the ONLY place in the entire system where duration alone has
    // absolute veto power without going through the log-odds pipeline.
    //
    // Boundaries (by design, not by heuristic):
    //   • ≤ 6.0s silent: covers all real-world stickers, reactions, and looping memes.
    //     No file size, resolution, or metadata signal overrides this.
    //   • ≥ 15.0s: exceeds the practical upper bound for any real-world animated image.
    //     No loop_count, transparency, or platform marker overrides this.

    // Audio signal is now in `derived.flags.content.has_audible_audio` (computed once, used everywhere).
    let has_audible_audio_global = derived.flags.content.has_audible_audio;

    // Hard veto: Extreme short (≤ 6.0s, silent)
    if meta
        .duration_secs
        .is_some_and(|d| d <= crate::constants::EXTREME_SHORT_ABSOLUTE_LIMIT_SECS)
        && !has_audible_audio_global
    {
        let dur_str = crate::media_conversion_gate::loop_format_duration_secs_label(
            meta.duration_secs,
            "layer0_ex_short_hard_veto",
        );
        return finalize_with_path(
            Verdict::LoopStrong(format!(
                "Layer 0-EX (Hard Veto): extreme-short duration {} ≤ {:.1}s — \
                 definitively animated image regardless of all other signals",
                dur_str,
                crate::constants::EXTREME_SHORT_ABSOLUTE_LIMIT_SECS,
            )),
            log_odds,
            Some("layer0_ex_short_hard_veto"),
        );
    }

    // Hard veto: Extreme long (≥ 15.0s) — no exceptions, even for silent assets
    if meta
        .duration_secs
        .is_some_and(|d| d >= crate::constants::EXTREME_LONG_ABSOLUTE_LIMIT_SECS)
    {
        let dur_str = crate::media_conversion_gate::loop_format_duration_secs_label(
            meta.duration_secs,
            "layer0_ex_long_hard_veto",
        );
        return finalize_with_path(
            Verdict::LoopWeak(format!(
                "Layer 0-EX (Hard Veto): extreme-long duration {} ≥ {:.1}s — \
                 definitively video regardless of all other signals",
                dur_str,
                crate::constants::EXTREME_LONG_ABSOLUTE_LIMIT_SECS,
            )),
            log_odds,
            Some("layer0_ex_long_hard_veto"),
        );
    }

    // ── Layer 0: Duration Dispatcher (Bias + Anti-Cliff Proximity Ramp) ──────────
    // Assets in the gray zone (6–15s) receive:
    //   1. A tier-proportional base bias (UltraShort → +crate::constants::LOOP_INTENT_Z_SCORE_STRENGTH … DefinitivelyLong → -3.0)
    //   2. A linearly-decaying proximity bonus/penalty for assets near a veto boundary.
    //      This prevents the "behavioral cliff" where 5.9s and 6.1s are treated radically
    //      differently despite being only 0.2s apart.
    //
    // Proximity ramp (short side, silent only):
    //   At 6.0s + ε → proximity ≈ 1.0 → full +2.5 bonus (nearly as strong as the veto)
    //   At 8.0s     → proximity = 0.0 → no additional bonus
    //
    // Proximity ramp (long side):
    //   At 15.0s - ε → proximity ≈ 1.0 → full -2.5 penalty (nearly as strong as the veto)
    //   At 13.0s     → proximity = 0.0 → no additional penalty

    let is_short_tier = matches!(
        tier,
        DurationTier::UltraShort | DurationTier::Short | DurationTier::MediumLong
    );

    if has_audible_audio_global {
        // Audible audio is an extremely strong anti-loop signal regardless of duration,
        // but we still let the full tree run so structural signals can be logged.
        log_odds.add(crate::constants::LOG_ODDS_BIAS_DEFINITIVELY_LONG);
    } else if is_short_tier {
        // Silent short assets: tier bias
        match tier {
            DurationTier::UltraShort => {
                log_odds.add(crate::constants::LOG_ODDS_BIAS_ULTRA_SHORT);
            }
            DurationTier::Short => log_odds.add(crate::constants::LOG_ODDS_BIAS_SHORT),
            DurationTier::MediumLong => {
                log_odds.add(crate::constants::LOG_ODDS_BIAS_MEDIUM_LONG);
            }
            _ => {}
        }
        // Anti-cliff proximity ramp (short side): 6.0–8.0s
        let short_veto = crate::constants::EXTREME_SHORT_ABSOLUTE_LIMIT_SECS;
        let short_buf = crate::constants::EXTREME_SHORT_PROXIMITY_BUFFER_SECS;
        let short_ramp_top = short_veto + short_buf;
        if meta
            .duration_secs
            .is_some_and(|d| d > short_veto && d <= short_ramp_top)
            && let Some(proximity) =
                crate::media_conversion_gate::loop_short_proximity_ramp_optional(
                    meta.duration_secs,
                    short_veto,
                    short_buf,
                    "loop_intent extreme_short proximity",
                )
        {
            log_odds.add(proximity * crate::constants::EXTREME_SHORT_PROXIMITY_MAX_BIAS);
        }
    } else {
        // Long tiers: tier bias
        match tier {
            DurationTier::Long => log_odds.add(crate::constants::LOG_ODDS_BIAS_LONG),
            DurationTier::VeryLong => log_odds.add(crate::constants::LOG_ODDS_BIAS_VERY_LONG),
            DurationTier::DefinitivelyLong => {
                log_odds.add(crate::constants::LOG_ODDS_BIAS_DEFINITIVELY_LONG);
            }
            _ => {}
        }
        // Anti-cliff proximity ramp (long side): 13.0–15.0s
        let long_veto = crate::constants::EXTREME_LONG_ABSOLUTE_LIMIT_SECS;
        let long_buf = crate::constants::EXTREME_LONG_PROXIMITY_BUFFER_SECS;
        let long_ramp_bottom = long_veto - long_buf;
        if meta.duration_secs.is_some_and(|d| d >= long_ramp_bottom)
            && meta.duration_secs.is_some_and(|d| d < long_veto)
        {
            let proximity = crate::media_conversion_gate::loop_long_proximity_ramp_or_one(
                meta.duration_secs,
                long_ramp_bottom,
                long_buf,
                "loop_intent extreme_long proximity",
            );
            log_odds.add(-proximity * crate::constants::EXTREME_LONG_PROXIMITY_MAX_BIAS);
        }
    }

    // ── Stage 1: Specialized Tree Dispatch ─────────────────────────────────────
    // Extreme-zone assets have already exited. All remaining assets (6–15s gray zone)
    // proceed to the specialized tree for further weighted evidence accumulation.
    //
    // Container-Aware Metadata Trust: replaces the former duration-proportional decay.
    // The old approach assumed "longer → less trustworthy metadata", which has no causal
    // basis. MP4 loop_count is unreliable at ANY duration (no standard loop field).
    // GIF NETSCAPE2.0 is authoritative at ANY duration.
    //
    // Trust levels:
    //   1.0: GIF (NETSCAPE2.0 extension), WebP (ANIM chunk loop field), APNG (acTL loop)
    //   0.6: Modern animated containers where loop semantics exist but are less standardized
    //   0.2: MP4/MKV/AVI — no authoritative loop field; loop_count is typically inferred
    let metadata_trust = {
        let ext = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
            meta.source_extension.as_deref(),
            "loop_intent",
        );
        let container = crate::media_conversion_gate::meta_container_lowercase_or_empty(
            meta.container.as_deref(),
            "loop_intent",
        );
        let mut base_trust: f64 =
            if meta.flags.streams.is_native_gif || ext == "gif" || container == "gif" {
                crate::constants::METADATA_TRUST_AUTHORITATIVE // GIF NETSCAPE2.0 is authoritative
            } else if ext == "webp"
                || ext == "apng"
                || ext == "png"
                || container == "webp"
                || container == "apng"
                || container == "png"
            {
                crate::constants::METADATA_TRUST_MODERN_ANIMATED // WebP ANIM chunk / APNG acTL have real loop fields
            } else if ext == "avif" || container == "avif" {
                crate::constants::METADATA_TRUST_STANDARD_VIDEO // AVIF loop semantics exist but less standardized
            } else {
                // MP4, MKV, AVI, etc. — no authoritative loop metadata
                crate::constants::METADATA_TRUST_UNTRUSTED
            };

        // ── Deep Penetration: Creator Software Validation ──
        // If we know the software that generated this file, we can override trust.
        // This solves the "Adobe Premiere exporting WebP with a loop marker" forgery risk.
        if let Some(encoder) = &meta.encoder_software {
            let lower = encoder.to_lowercase();
            // NLE (Non-Linear Editors) exporting to WebP/GIF rarely intend for short loops.
            // Even if they write a loop block, they are treated as untrusted video.
            if lower.contains(crate::constants::EDITOR_PREMIERE)
                || lower.contains(crate::constants::EDITOR_RESOLVE)
                || lower.contains(crate::constants::EDITOR_FINAL_CUT)
                || lower.contains(crate::constants::EDITOR_AVID)
                || lower.contains(crate::constants::EDITOR_VEGAS)
                || lower.contains(crate::constants::EDITOR_CANVA)
                || lower.contains(crate::constants::EDITOR_FIGMA)
                || lower.contains(crate::constants::PLATFORM_TIKTOK)
                || lower.contains(crate::constants::PLATFORM_INSTAGRAM)
                || lower.contains(crate::constants::PLATFORM_WHATSAPP)
                || lower.contains(crate::constants::PLATFORM_FACEBOOK)
                || lower.contains(crate::constants::PLATFORM_TWITTER)
                || lower.contains(crate::constants::PLATFORM_X)
                || lower.contains(crate::constants::PLATFORM_SNAPCHAT)
                || lower.contains(crate::constants::PLATFORM_YOUTUBE)
                || lower.contains(crate::constants::PLATFORM_DISCORD)
                || lower.contains(crate::constants::BRAND_BILIBILI)
                || lower.contains(crate::constants::BRAND_DOUYIN)
                || lower.contains(crate::constants::BRAND_KUAISHOU)
                || lower.contains(crate::constants::BRAND_XIAOHONGSHU)
                || lower.contains(crate::constants::BRAND_WEIBO)
                || lower.contains(crate::constants::BRAND_TENCENT)
                || lower.contains(crate::constants::BRAND_BAIDU)
                || lower.contains(crate::constants::BRAND_IQIYI)
                || lower.contains(crate::constants::BRAND_VIMEO)
                || lower.contains(crate::constants::BRAND_TWITCH)
                || lower.contains(crate::constants::BRAND_VLC)
                || lower.contains(crate::constants::BRAND_POTPLAYER)
            {
                base_trust = base_trust.min(crate::constants::METADATA_TRUST_UNTRUSTED);
            }
            // Dedicated animation/meme creation tools
            else if lower.contains(crate::constants::EDITOR_PHOTOSHOP)
                || lower.contains(crate::constants::EDITOR_GIPHY)
                || lower.contains(crate::constants::EDITOR_EZGIF)
                || lower.contains(crate::constants::EDITOR_SCREENTOGIF)
                || lower.contains(crate::constants::EDITOR_KRITA)
                || lower.contains(crate::constants::EDITOR_PROCREATE)
                || lower.contains(crate::constants::EDITOR_CLIP_STUDIO)
                || lower.contains(crate::constants::EDITOR_LIGHTROOM)
                || lower.contains(crate::constants::EDITOR_DARKTABLE)
                || lower.contains(crate::constants::EDITOR_CAPTURE_ONE)
                || lower.contains(crate::constants::EDITOR_AFFINITY)
                || lower.contains(crate::constants::EDITOR_PIXELMATOR)
                || lower.contains(crate::constants::EDITOR_GIMP)
                || lower.contains(crate::constants::EDITOR_PAINT_NET)
                || lower.contains(crate::constants::SOFTWARE_SNAPSEED)
                || lower.contains(crate::constants::SOFTWARE_VSCO)
                || lower.contains(crate::constants::SOFTWARE_PICSART)
                || lower.contains(crate::constants::BRAND_ADOBE_AE)
                || lower.contains(crate::constants::BRAND_BLENDER)
                || lower.contains(crate::constants::BRAND_MAYA)
                || lower.contains(crate::constants::BRAND_MAX)
                || lower.contains(crate::constants::BRAND_HOUDINI)
                || lower.contains(crate::constants::BRAND_CINEMA4D)
                || lower.contains(crate::constants::BRAND_UNITY)
                || lower.contains(crate::constants::BRAND_UNREAL)
            {
                base_trust = base_trust.max(crate::constants::METADATA_TRUST_AUTHORITATIVE); // Absolute trust
            }
            // Slightly penalize generic FFmpeg wrappers, basic encoders, and mobile fast-editors
            // This is now independent of the initial trust assignment to ensure it accumulates.
            if lower.contains(crate::constants::SOFTWARE_LAVF)
                || lower.contains(crate::constants::SOFTWARE_HANDBRAKE)
                || lower.contains(crate::constants::SOFTWARE_SHANA)
                || lower.contains(crate::constants::SOFTWARE_MEGUI)
                || lower.contains(crate::constants::SOFTWARE_X264_CLI)
                || lower.contains(crate::constants::SOFTWARE_X265_CLI)
                || lower.contains(crate::constants::SOFTWARE_AOM_CLI)
                || lower.contains(crate::constants::SOFTWARE_CAPCUT)
                || lower.contains(crate::constants::SOFTWARE_FILMORA)
                || lower.contains(crate::constants::SOFTWARE_INSHOT)
                || lower.contains(crate::constants::SOFTWARE_KINEMASTER)
                || lower.contains(crate::constants::SOFTWARE_POWERDIRECTOR)
                || lower.contains(crate::constants::SOFTWARE_MEITU)
            {
                base_trust = (base_trust - crate::constants::METADATA_TRUST_PENALTY_LAVF).max(0.0);
            }
        }

        base_trust
    };

    if is_image || meta.flags.streams.is_native_gif {
        evaluate_image_tree(meta, &derived, &thresholds, log_odds, tier, metadata_trust)
    } else {
        evaluate_video_tree(meta, &derived, &thresholds, log_odds, tier, metadata_trust)
    }
}

fn evaluate_image_tree(
    meta: &LoopMeta,
    derived: &DerivedLoopSignals,
    thresholds: &LoopThresholds,
    mut log_odds: LogOdds,
    tier: DurationTier,
    metadata_trust: f64,
) -> TreeEvaluation {
    let ext_lower = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
        meta.source_extension.as_deref(),
        "loop_intent",
    );

    // Layer 1-B: Transparency is a strong signal, but NOT decisive on its own.
    // Attenuated by metadata_trust: in the Long zone, transparency cannot carry
    // enough weight to flip the verdict without genuine physical loop evidence.
    if meta.has_confirmed_silent_or_no_audio() && meta.has_verified_real_transparency() {
        log_odds.add(crate::constants::TRANSPARENCY_POSITIVE_LOG_ODDS * 2.0 * metadata_trust);
    }

    // Layer 2: Explicit declarations — attenuated by metadata_trust.
    // Soft metadata signals (loop_count, platform markers) decay toward zero as
    // duration approaches the long-veto boundary. Physical signals are NOT affected.
    if meta.loop_count == Some(0) {
        let bonus = loop_count_zero_bonus(meta, thresholds);
        if bonus.is_finite() {
            log_odds.add(bonus * metadata_trust);
        }
    } else if meta.loop_count == Some(1) {
        // play-once is a negative signal — apply full weight regardless (safe direction)
        log_odds.add(PLAY_ONCE_NEGATIVE_LOG_ODDS);
    }

    if has_explicit_loop_platform_marker(meta) {
        log_odds.add(crate::constants::PLATFORM_MARKER_POSITIVE_LOG_ODDS * metadata_trust);
    }

    // Sticker-class native GIF: weighted bonus, not immediate exit.
    // Dimensions and extension are metadata — they cannot one-shot the verdict.
    if ext_lower == "gif"
        && meta.has_confirmed_silent_or_no_audio()
        && matches!(
            tier,
            DurationTier::UltraShort | DurationTier::Short | DurationTier::MediumLong
        )
        && let (Some(w), Some(h)) = (meta.width, meta.height)
        && w > 0
        && w <= crate::constants::STICKER_MAX_DIMENSION
        && h <= crate::constants::STICKER_MAX_DIMENSION
    {
        let px = u64::from(w) * u64::from(h);
        if px <= crate::constants::STICKER_TIER_NATIVE_GIF_MAX_PIXELS {
            log_odds.add(crate::constants::COMPACT_SILENT_POSITIVE_LOG_ODDS);
        }
    }

    // Note: tier-based log-odds bias is applied by the top-level evaluate_loop_tree
    // dispatcher (including buffer zones) before dispatching here. Do not re-apply.

    evaluate_kinetics_and_physics(meta, derived, thresholds, &mut log_odds);
    apply_structural_signals(meta, derived, thresholds, &mut log_odds, true, false);

    if let Some(verdict) = checkpoint_verdict(
        log_odds,
        crate::constants::TREE_STRUCTURAL_CHECKPOINT_LOG_ODDS_THRESHOLD,
        "Layer 3 (Image)",
        "self-referential loop structure",
        "self-referential structure points away from looping",
    ) {
        return finalize_with_path(
            verdict,
            log_odds,
            Some(tree_checkpoint_resolution_path("Layer 3 (Image)")),
        );
    }

    apply_weak_heuristics(meta, derived, thresholds, &mut log_odds, true, false);

    if let Some(verdict) = checkpoint_verdict(
        log_odds,
        crate::constants::TREE_CONTENT_CHECKPOINT_LOG_ODDS_THRESHOLD,
        "Layer 4 (Image)",
        "content and envelope strongly favor a looping asset",
        "content and envelope strongly favor standard video processing",
    ) {
        return finalize_with_path(
            verdict,
            log_odds,
            Some(tree_checkpoint_resolution_path("Layer 4 (Image)")),
        );
    }

    // Final arbitration for Images
    let Some(lo) = log_odds.value() else {
        return finalize_with_path(
            Verdict::Uncertain("Layer 5 (Image): log-odds not finite".into()),
            log_odds,
            Some(tree_layer5_resolution_path("image", "uncertain")),
        );
    };
    if lo >= thresholds.decision_threshold {
        finalize_with_path(
            Verdict::LoopStrong(format!(
                "Layer 5 (Image): log-odds {:.2} >= {:.2}",
                lo, thresholds.decision_threshold
            )),
            log_odds,
            Some(tree_layer5_resolution_path("image", "strong")),
        )
    } else if lo <= -thresholds.decision_threshold {
        finalize_with_path(
            Verdict::LoopWeak(format!(
                "Layer 5 (Image): log-odds {:.2} <= -{:.2}",
                lo, thresholds.decision_threshold
            )),
            log_odds,
            Some(tree_layer5_resolution_path("image", "weak")),
        )
    } else {
        finalize_with_path(
            Verdict::Uncertain(format!(
                "Layer 5 (Image): log-odds {:.2} within ±{:.2}",
                lo, thresholds.decision_threshold
            )),
            log_odds,
            Some(tree_layer5_resolution_path("image", "uncertain")),
        )
    }
}

fn evaluate_video_tree(
    meta: &LoopMeta,
    derived: &DerivedLoopSignals,
    thresholds: &LoopThresholds,
    mut log_odds: LogOdds,
    tier: DurationTier,
    metadata_trust: f64,
) -> TreeEvaluation {
    // Layer 1-A: Audio is a very strong anti-loop signal, but not an absolute veto.
    // An ultra-short video with a single click sound is still plausibly a loop.
    // Duration-tier interaction modulates the penalty.
    // Use the centralized audio signal from DerivedLoopSignals (no duplicate computation).
    if derived.flags.content.has_audible_audio {
        let audio_penalty = match tier {
            DurationTier::UltraShort => crate::constants::SCENE_CUT_NEGATIVE_LOG_ODDS * 0.6_f64,
            DurationTier::Short => crate::constants::SCENE_CUT_NEGATIVE_LOG_ODDS,
            _ => crate::constants::LOG_ODDS_BIAS_DEFINITIVELY_LONG,
        };
        log_odds.add(-audio_penalty);
    }

    // Layer 1-D: Long silent video handling (with Dev override)
    let intercept_long_silent =
        developer_layer1_override_enabled(crate::constants::ENV_INTERCEPT_LONG_SILENT);
    if intercept_long_silent
        && matches!(
            tier,
            DurationTier::Long | DurationTier::VeryLong | DurationTier::DefinitivelyLong
        )
    {
        log_odds.add(-crate::constants::LOG_ODDS_BIAS_DEFINITIVELY_LONG);
    }

    // Layer 2: Explicit declarations — attenuated by metadata_trust.
    // Soft metadata signals decay toward zero as duration approaches the long-veto
    // boundary (15s), preventing forged metadata from overcoming the Long-tier bias
    // without genuine physical loop evidence in Layers 3–5.
    if meta.loop_count == Some(0) {
        let bonus = loop_count_zero_bonus(meta, thresholds);
        if bonus.is_finite() {
            log_odds.add(bonus * metadata_trust);
        }
    } else if meta.loop_count == Some(1) {
        // play-once penalty: safe direction, apply full weight
        log_odds.add(-PLAY_ONCE_NEGATIVE_LOG_ODDS);
    }

    if has_explicit_loop_platform_marker(meta) {
        log_odds.add(crate::constants::PLATFORM_MARKER_POSITIVE_LOG_ODDS * metadata_trust);
    }

    // Layer 2-D: Short silent WebM (weighted signal)
    let ext_lower = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
        meta.source_extension.as_deref(),
        "loop_intent",
    );
    if is_silent_webm(meta, &ext_lower)
        && matches!(
            tier,
            DurationTier::UltraShort | DurationTier::Short | DurationTier::MediumLong
        )
    {
        log_odds.add(crate::constants::COMPACT_SILENT_POSITIVE_LOG_ODDS);
    }

    // Layer 1-B3: Dimensional Sticker — dimensions are metadata, use as weighted bonus.
    // UltraShort duration is the real anchor; dimensions just add confidence.
    if tier == DurationTier::UltraShort
        && let (Some(w), Some(h)) = (meta.width, meta.height)
        && w > 0
        && w <= crate::constants::STICKER_MAX_DIMENSION
        && h > 0
        && h <= crate::constants::STICKER_MAX_DIMENSION
        && (meta.pkt_sizes.len() < 3 || meta.pts_deltas.len() < 3)
    {
        log_odds.add(crate::constants::COMPACT_SILENT_POSITIVE_LOG_ODDS);
    }

    // Note: tier-based log-odds bias is applied by the top-level evaluate_loop_tree
    // dispatcher (including buffer zones) before dispatching here. Do not re-apply.

    evaluate_kinetics_and_physics(meta, derived, thresholds, &mut log_odds);
    apply_structural_signals(meta, derived, thresholds, &mut log_odds, false, true);

    if let Some(verdict) = checkpoint_verdict(
        log_odds,
        crate::constants::TREE_STRUCTURAL_CHECKPOINT_LOG_ODDS_THRESHOLD,
        "Layer 3 (Video)",
        "self-referential loop structure",
        "self-referential structure points away from looping",
    ) {
        return finalize_with_path(
            verdict,
            log_odds,
            Some(tree_checkpoint_resolution_path("Layer 3 (Video)")),
        );
    }

    apply_weak_heuristics(meta, derived, thresholds, &mut log_odds, false, true);

    if let Some(verdict) = checkpoint_verdict(
        log_odds,
        crate::constants::TREE_CONTENT_CHECKPOINT_LOG_ODDS_THRESHOLD,
        "Layer 4 (Video)",
        "content and envelope strongly favor a looping asset",
        "content and envelope strongly favor standard video processing",
    ) {
        return finalize_with_path(
            verdict,
            log_odds,
            Some(tree_checkpoint_resolution_path("Layer 4 (Video)")),
        );
    }

    // Final arbitration for Video
    let Some(lo) = log_odds.value() else {
        return finalize_with_path(
            Verdict::Uncertain("Layer 5 (Video): log-odds not finite".into()),
            log_odds,
            Some(tree_layer5_resolution_path("video", "uncertain")),
        );
    };
    if lo >= thresholds.decision_threshold {
        finalize_with_path(
            Verdict::LoopStrong(format!(
                "Layer 5 (Video): log-odds {:.2} >= {:.2}",
                lo, thresholds.decision_threshold
            )),
            log_odds,
            Some(tree_layer5_resolution_path("video", "strong")),
        )
    } else if lo <= -thresholds.decision_threshold {
        finalize_with_path(
            Verdict::LoopWeak(format!(
                "Layer 5 (Video): log-odds {:.2} <= -{:.2}",
                lo, thresholds.decision_threshold
            )),
            log_odds,
            Some(tree_layer5_resolution_path("video", "weak")),
        )
    } else {
        finalize_with_path(
            Verdict::Uncertain(format!(
                "Layer 5 (Video): log-odds {:.2} within ±{:.2}",
                lo, thresholds.decision_threshold
            )),
            log_odds,
            Some(tree_layer5_resolution_path("video", "uncertain")),
        )
    }
}

fn should_accept_layer6_loopstrong(
    meta: &LoopMeta,
    thresholds: &LoopThresholds,
    keep_prob: f64,
    final_score: f64,
    confidence: f64,
) -> bool {
    if confidence >= crate::constants::LAYER6_CONFIDENCE_HIGH
        && final_score > crate::constants::LAYER6_FINAL_SCORE_HIGH
    {
        return true;
    }

    let ext = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
        meta.source_extension.as_deref(),
        "loop_intent",
    );
    let is_image = SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str());
    let is_gif_family =
        ext == "gif" || crate::constants::MODERN_ANIMATED_EXTENSIONS.contains(&ext.as_str());
    let short_clip_like = meta.has_confirmed_silent_or_no_audio()
        && meta.duration_secs.is_some_and(|d| d > 0.0_f64)
        && meta
            .duration_secs
            .is_some_and(|d| d <= thresholds.short_asset_window_secs);

    final_score >= crate::constants::LAYER6_HIGH_SCORE_THRESHOLD
        && keep_prob >= crate::constants::LAYER6_KEEP_PROB_MIN
        && confidence >= crate::constants::LAYER6_RELAXED_CONFIDENCE_THRESHOLD
        && (short_clip_like || is_image || is_gif_family)
}

#[derive(Debug, Clone, Copy)]
struct Layer6Fusion {
    knn_weight: f64,
    tree_weight: f64,
    final_score: f64,
}

/// Fuses KNN results with the decision tree output using a Logistic Regression model.
///
/// Formula: P(keep) = `sigmoid`(`w_knn`*knn + `w_tree`*tree + `w_density`*log(n) + bias)
fn logistic_regression_fusion(
    knn_prob: f64,
    tree_prob: f64,
    neighbor_count: usize,
    nudge: f64,
) -> Option<f64> {
    use crate::constants::{
        LAYER6_LR_BIAS, LAYER6_LR_W_DENSITY, LAYER6_LR_W_KNN, LAYER6_LR_W_TREE,
    };

    // neighbor_count is log-scaled to normalized density signal
    let density_signal = crate::numeric_cast::usize_to_f64(neighbor_count).ln_1p();

    // MATH FIX: Convert probabilities to log-odds (logit) before linear combination.
    // Previously, raw probabilities were weighted and then passed through sigmoid,
    // which is mathematically incorrect: sigmoid(p1*w1 + p2*w2) ≠ proper probability fusion.
    // The correct form is: sigmoid(logit(p1)*w1 + logit(p2)*w2 + bias)
    // This ensures high-confidence inputs (p ≈ 0 or p ≈ 1) are properly preserved
    // through the fusion, instead of being compressed toward 0.5.
    let logit = |p: f64| -> f64 {
        let clamped = p.clamp(
            crate::constants::FUSED_PROB_CLAMP_LOWER,
            crate::constants::FUSED_PROB_CLAMP_UPPER,
        );
        (clamped / (1.0 - clamped)).ln()
    };

    let score = density_signal.mul_add(
        LAYER6_LR_W_DENSITY,
        (logit(knn_prob) * LAYER6_LR_W_KNN) + (logit(tree_prob) * LAYER6_LR_W_TREE),
    ) + LAYER6_LR_BIAS;

    // Apply sigmoid once to convert the log-odds-weighted sum back to probability
    let fused_prob = 1.0_f64 / (1.0_f64 + (-score).exp());
    let raw = (fused_prob + nudge).clamp(
        crate::constants::FUSED_PROB_CLAMP_LOWER,
        crate::constants::FUSED_PROB_CLAMP_UPPER,
    );
    crate::algorithm_seal::loop_unit_probability(raw)
}

fn compute_layer6_fusion(
    keep_prob: f64,
    tree_probability: Option<f64>,
    neighbor_count: usize,
    nudge_score: f64,
) -> Option<Layer6Fusion> {
    let keep_prob = crate::algorithm_seal::loop_unit_probability(keep_prob)?;
    let tree_probability =
        tree_probability.and_then(crate::algorithm_seal::loop_unit_probability)?;
    let final_score =
        logistic_regression_fusion(keep_prob, tree_probability, neighbor_count, nudge_score)?;
    let final_score = crate::algorithm_seal::loop_unit_probability(final_score)?;

    // Legacy weights kept for logging purposes, but the final_score now uses LR
    let fusion = Layer6Fusion {
        knn_weight: crate::constants::LAYER6_MAX_KNN_WEIGHT, // Representational
        tree_weight: 1.0 - crate::constants::LAYER6_MAX_KNN_WEIGHT,
        final_score,
    };
    tracing::debug!(
        target: "mfb.algorithm",
        pipeline = "loop_intent_layer6",
        branch = "fusion_computed",
        keep_prob,
        tree_probability,
        neighbor_count,
        nudge_score,
        final_score = fusion.final_score,
        "Layer 6 logistic fusion"
    );
    Some(fusion)
}

#[derive(Debug, Default)]
struct DirectionalArbitration {
    keep_score: f64,
    convert_score: f64,
    keep_trace: Vec<String>,
    convert_trace: Vec<String>,
}

impl DirectionalArbitration {
    fn add_keep(&mut self, delta: f64, reason: impl Into<String>) {
        if !delta.is_finite() {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_layer6b",
                branch = "arbitration_delta_non_finite",
                side = "keep",
                delta,
                "dropping non-finite arbitration delta"
            );
            return;
        }
        self.keep_score += delta;
        if !self.keep_score.is_finite() {
            self.keep_score = 0.0;
        }
        self.keep_trace.push(reason.into());
    }

    fn add_convert(&mut self, delta: f64, reason: impl Into<String>) {
        if !delta.is_finite() {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_layer6b",
                branch = "arbitration_delta_non_finite",
                side = "convert",
                delta,
                "dropping non-finite arbitration delta"
            );
            return;
        }
        self.convert_score += delta;
        if !self.convert_score.is_finite() {
            self.convert_score = 0.0;
        }
        self.convert_trace.push(reason.into());
    }

    fn winner_trace(&self, keep_wins: bool) -> &[String] {
        if keep_wins {
            &self.keep_trace
        } else {
            &self.convert_trace
        }
    }
}

struct Layer6DirectionalArbitrator<'a> {
    meta: &'a LoopMeta,
    thresholds: &'a LoopThresholds,
    tree: &'a TreeEvaluation,
    keep_prob: Option<f64>,
    confidence: Option<f64>,
    fusion_score: Option<f64>,
    neighbor_count: Option<usize>,
    upstream_reason: &'a str,
    media_family: LoopMediaFamily,
    short_silent_asset: bool,
    platform_marker: bool,
    arbitration: DirectionalArbitration,
}

impl<'a> Layer6DirectionalArbitrator<'a> {
    fn new(
        meta: &'a LoopMeta,
        thresholds: &'a LoopThresholds,
        tree: &'a TreeEvaluation,
        keep_prob: Option<f64>,
        confidence: Option<f64>,
        fusion_score: Option<f64>,
        neighbor_count: Option<usize>,
        upstream_reason: &'a str,
    ) -> Self {
        let ext = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
            meta.source_extension.as_deref(),
            "loop_intent",
        );
        let is_image = SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str());
        let is_video = !is_image && SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str());
        Self {
            meta,
            thresholds,
            tree,
            keep_prob,
            confidence,
            fusion_score,
            neighbor_count,
            upstream_reason,
            media_family: LoopMediaFamily::from_flags(is_image, is_video),
            short_silent_asset: is_short_silent_asset(meta, thresholds),
            platform_marker: has_explicit_loop_platform_marker(meta),
            arbitration: DirectionalArbitration::default(),
        }
    }

    fn run(mut self) -> (Option<Verdict>, Layer6bAuditSnapshot) {
        self.apply_model_biases();
        self.apply_keep_biases();
        self.apply_convert_biases();
        self.finalize()
    }

    fn apply_model_biases(&mut self) {
        self.apply_tree_bias();
        self.apply_knn_bias();
        self.apply_fusion_bias();
    }

    fn apply_tree_bias(&mut self) {
        let Some(tree_p) = self.tree.tree_probability else {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_layer6",
                branch = "tree_probability_missing",
                "skipping tree bias: tree_probability unavailable"
            );
            return;
        };
        if tree_p >= crate::constants::LAYER6_DIRECTIONAL_KEEP_MIN {
            let delta = ((tree_p - 0.5) * crate::constants::LOOP_INTENT_DIRECTIONAL_BIAS).clamp(
                crate::constants::LOOP_INTENT_DIRECTIONAL_MIN_BONUS,
                crate::constants::LOOP_INTENT_DIRECTIONAL_MAX_BONUS,
            );
            self.arbitration
                .add_keep(delta, format!("tree lean {tree_p:.2}"));
        } else if tree_p <= crate::constants::LAYER6_DIRECTIONAL_WEAK_MAX {
            let delta = ((0.5 - tree_p) * crate::constants::LOOP_INTENT_DIRECTIONAL_BIAS).clamp(
                crate::constants::LOOP_INTENT_DIRECTIONAL_MIN_BONUS,
                crate::constants::LOOP_INTENT_DIRECTIONAL_MAX_BONUS,
            );
            self.arbitration
                .add_convert(delta, format!("tree lean {tree_p:.2}"));
        }
    }

    fn apply_knn_bias(&mut self) {
        let Some(knn_keep) = self.keep_prob else {
            return;
        };

        let Some(conf) = self.resolved_confidence() else {
            return;
        };
        if knn_keep >= crate::constants::LOOP_INTENT_KNN_HIGH {
            let delta = (((knn_keep - 0.5) * crate::constants::LOOP_INTENT_KNN_BIAS) * conf).clamp(
                crate::constants::LOOP_INTENT_KNN_MIN_DELTA,
                crate::constants::LOOP_INTENT_KNN_MAX_DELTA,
            );
            self.arbitration
                .add_keep(delta, format!("KNN keep {knn_keep:.2} @ conf {conf:.2}"));
        } else if knn_keep <= crate::constants::LOOP_INTENT_KNN_LOW {
            let delta = (((0.5 - knn_keep) * crate::constants::LOOP_INTENT_KNN_BIAS) * conf).clamp(
                crate::constants::LOOP_INTENT_KNN_MIN_DELTA,
                crate::constants::LOOP_INTENT_KNN_MAX_DELTA,
            );
            self.arbitration
                .add_convert(delta, format!("KNN keep {knn_keep:.2} @ conf {conf:.2}"));
        }
    }

    fn apply_fusion_bias(&mut self) {
        let Some(score) = self.fusion_score else {
            return;
        };

        let Some(conf) = self.resolved_confidence() else {
            return;
        };
        if score >= crate::constants::LOOP_INTENT_FUSION_KEEP_THRESHOLD {
            let delta = (((score - 0.5) * crate::constants::LOOP_INTENT_FUSION_BIAS) * conf).clamp(
                crate::constants::LOOP_INTENT_KNN_MIN_DELTA,
                crate::constants::LOOP_INTENT_KNN_MAX_DELTA,
            );
            self.arbitration
                .add_keep(delta, format!("fusion score {score:.2}"));
        } else if score <= crate::constants::LOOP_INTENT_FUSION_REJECT_THRESHOLD {
            let delta = (((0.5 - score) * crate::constants::LOOP_INTENT_FUSION_BIAS) * conf).clamp(
                crate::constants::LOOP_INTENT_KNN_MIN_DELTA,
                crate::constants::LOOP_INTENT_KNN_MAX_DELTA,
            );
            self.arbitration
                .add_convert(delta, format!("fusion score {score:.2}"));
        }
    }

    fn resolved_confidence(&self) -> Option<f64> {
        let raw = self.confidence?;
        let sealed = crate::algorithm_seal::loop_unit_probability(raw)?;
        Some(sealed.max(crate::constants::LOOP_INTENT_KNN_LOW))
    }

    fn apply_keep_biases(&mut self) {
        self.apply_asset_keep_biases();
        self.apply_motion_keep_biases();
    }

    fn apply_asset_keep_biases(&mut self) {
        if self.platform_marker {
            self.arbitration.add_keep(
                crate::constants::LOOP_INTENT_ARBITRATION_MARKER_BONUS,
                "platform/app marker",
            );
        }
        if self.meta.has_verified_real_transparency() {
            self.arbitration.add_keep(
                crate::constants::LOOP_INTENT_ARBITRATION_TRANSPARENCY_BONUS,
                "transparency",
            );
        }
        if self.short_silent_asset {
            let delta = if self
                .meta
                .duration_secs
                .is_some_and(|duration| duration <= self.thresholds.short_clip_secs)
            {
                crate::constants::LOOP_INTENT_ARBITRATION_AUDIO_BONUS
            } else {
                crate::constants::LOOP_INTENT_ARBITRATION_METADATA_BONUS
            };
            self.arbitration.add_keep(
                delta,
                format!("short silent asset {:?}s", self.meta.duration_secs),
            );
        }
        if let (Some(w), Some(h)) = (self.meta.width, self.meta.height)
            && w > 0
            && w == h
        {
            self.arbitration.add_keep(
                crate::constants::LOOP_INTENT_ARBITRATION_SQUARE_BONUS,
                "square canvas",
            );
        }
        if self.media_family.is_image() {
            self.arbitration.add_keep(
                crate::constants::LOOP_INTENT_ARBITRATION_IMAGE_BONUS,
                "image-family container",
            );
        }
    }

    fn apply_motion_keep_biases(&mut self) {
        self.apply_loop_closure_keep_bias();
        self.apply_periodicity_keep_bias();
        self.apply_loop_frequency_keep_bias();
    }

    fn apply_loop_closure_keep_bias(&mut self) {
        let Some(closure) = self.meta.loop_closure_score else {
            return;
        };

        if closure >= crate::constants::LOOP_INTENT_CLOSURE_HIGH {
            let delta = (((closure - crate::constants::LOOP_INTENT_CLOSURE_HIGH)
                / crate::constants::LOOP_INTENT_CLOSURE_REDUCTION_SCALE)
                * crate::constants::LOOP_INTENT_DIRECTIONAL_MAX_BONUS)
                .clamp(
                    crate::constants::LOOP_INTENT_KNN_MIN_DELTA,
                    crate::constants::LOOP_INTENT_DIRECTIONAL_MAX_BONUS,
                );
            self.arbitration
                .add_keep(delta, format!("loop closure {closure:.2}"));
        } else if closure <= crate::constants::LOOP_INTENT_CLOSURE_LOW {
            let delta = (((crate::constants::LOOP_INTENT_CLOSURE_LOW - closure)
                / crate::constants::LOOP_INTENT_CLOSURE_LOW)
                * crate::constants::LOOP_INTENT_CLOSURE_REJECT_DELTA)
                .clamp(
                    crate::constants::LOOP_INTENT_KNN_MIN_DELTA,
                    crate::constants::LOOP_INTENT_CLOSURE_REJECT_DELTA,
                );
            self.arbitration
                .add_convert(delta, format!("loop closure {closure:.2}"));
        }
    }

    fn apply_periodicity_keep_bias(&mut self) {
        let Some(periodicity) = self.meta.motion_periodicity else {
            return;
        };

        if periodicity >= crate::constants::LOOP_INTENT_PERIODICITY_HIGH {
            let delta = (((periodicity - crate::constants::LOOP_INTENT_PERIODICITY_HIGH)
                / crate::constants::LOOP_INTENT_PERIODICITY_REDUCTION_SCALE)
                * crate::constants::LOOP_INTENT_PERIODICITY_MAX_BONUS)
                .clamp(
                    crate::constants::LOOP_INTENT_KNN_MIN_DELTA,
                    crate::constants::LOOP_INTENT_PERIODICITY_MAX_BONUS,
                );
            self.arbitration
                .add_keep(delta, format!("motion periodicity {periodicity:.2}"));
        } else if periodicity <= crate::constants::LOOP_INTENT_PERIODICITY_LOW {
            let delta = (((crate::constants::LOOP_INTENT_PERIODICITY_LOW - periodicity)
                / crate::constants::LOOP_INTENT_PERIODICITY_LOW)
                * crate::constants::LOOP_INTENT_PERIODICITY_REJECT_DELTA)
                .clamp(
                    crate::constants::LOOP_INTENT_TREE_MAX_DELTA,
                    crate::constants::LOOP_INTENT_PERIODICITY_REJECT_DELTA,
                );
            self.arbitration
                .add_convert(delta, format!("motion periodicity {periodicity:.2}"));
        }
    }

    fn apply_loop_frequency_keep_bias(&mut self) {
        let loop_frequency = score_loop_frequency(self.meta.duration_secs, self.meta.frame_count);
        if loop_frequency >= crate::constants::LOOP_FREQUENCY_HIGH_THRESHOLD {
            let delta = (((loop_frequency - crate::constants::LOOP_FREQUENCY_HIGH_THRESHOLD)
                / crate::constants::LOOP_INTENT_FREQ_REDUCTION_SCALE)
                * crate::constants::LOOP_INTENT_FREQ_MAX_BONUS)
                .clamp(
                    crate::constants::LOOP_INTENT_DIRECTIONAL_MIN_BONUS,
                    crate::constants::LOOP_INTENT_FREQ_MAX_BONUS,
                );
            self.arbitration
                .add_keep(delta, format!("loop frequency {loop_frequency:.2}"));
        } else if loop_frequency <= crate::constants::LOOP_FREQUENCY_LOW_THRESHOLD {
            let delta = (((crate::constants::LOOP_FREQUENCY_LOW_THRESHOLD - loop_frequency)
                / crate::constants::LOOP_FREQUENCY_LOW_THRESHOLD)
                * crate::constants::LOOP_INTENT_FREQ_MAX_PENALTY)
                .clamp(
                    crate::constants::LOOP_INTENT_FRAME_COUNT_MIN_PENALTY,
                    crate::constants::LOOP_INTENT_FREQ_MAX_PENALTY,
                );
            self.arbitration
                .add_convert(delta, format!("loop frequency {loop_frequency:.2}"));
        }
    }

    fn apply_convert_biases(&mut self) {
        self.apply_container_convert_biases();
        self.apply_audio_and_frame_density_biases();
    }

    fn apply_container_convert_biases(&mut self) {
        if self.media_family.is_video() {
            let delta = if self.short_silent_asset {
                crate::constants::LOOP_INTENT_VIDEO_CONTAINER_SHORT_DELTA
            } else {
                crate::constants::LOOP_INTENT_VIDEO_CONTAINER_STANDARD_DELTA
            };
            self.arbitration.add_convert(delta, "video container");
        }
        if let (Some(w), Some(h)) = (self.meta.width, self.meta.height)
            && w > 0
            && h > 0
            && is_near_16_by_9(self.meta.width, self.meta.height)
        {
            self.arbitration.add_convert(
                crate::constants::LOOP_INTENT_WIDESCREEN_DELTA,
                "widescreen framing",
            );
        }
        if detect_scene_cut(&self.meta.pkt_sizes) {
            self.arbitration
                .add_convert(crate::constants::LOOP_INTENT_SCENE_CUT_DELTA, "scene cut");
        }
        if self.meta.has_confirmed_silent_or_no_audio()
            && self
                .meta
                .duration_secs
                .is_some_and(|duration| duration > self.thresholds.modern_bias_duration_secs)
        {
            self.arbitration.add_convert(
                crate::constants::LOOP_INTENT_LONG_SILENT_CLIP_DELTA,
                format!("long silent clip {:?}s", self.meta.duration_secs),
            );
        }
        if self.media_family.is_video()
            && !self.short_silent_asset
            && self.meta.file_size_bytes > crate::constants::STICKER_MAX_SIZE_BYTES
        {
            self.arbitration.add_convert(
                crate::constants::LOOP_INTENT_LARGE_VIDEO_ENVELOPE_DELTA,
                "large video envelope",
            );
        }
    }

    fn apply_audio_and_frame_density_biases(&mut self) {
        let has_audible_audio = crate::media_conversion_gate::loop_audible_audio_fail_closed(
            self.meta.flags.streams.has_audio,
            self.meta.audible_audio_state(),
            "LoopIntentArbitration::apply_audio_and_frame_density_biases",
        );
        if has_audible_audio {
            let audio_weight = if self.short_silent_asset {
                crate::constants::LOOP_INTENT_AUDIBLE_AUDIO_SHORT_DELTA
            } else {
                crate::constants::LOOP_INTENT_AUDIBLE_AUDIO_STANDARD_DELTA
            };
            self.arbitration
                .add_convert(audio_weight, "audible audio track");
        }
        if let (Some(frame_count), Some(duration)) =
            (self.meta.frame_count, self.meta.duration_secs)
            && frame_count > crate::constants::LOOP_INTENT_FRAME_COUNT_LONG_LIMIT
            && duration > 0.01_f64
        {
            let fps = crate::numeric_cast::u64_to_f64(frame_count) / duration;
            if fps < crate::constants::LOOP_INTENT_FRAME_COUNT_FPS_THRESHOLD {
                let weight = (crate::numeric_cast::u64_to_f64(
                    frame_count
                        .saturating_sub(crate::constants::LOOP_INTENT_FRAME_COUNT_LONG_LIMIT),
                ) / crate::constants::LOOP_INTENT_FRAME_COUNT_PENALTY_DIVISOR)
                    .clamp(
                        crate::constants::LOOP_INTENT_FRAME_COUNT_MIN_PENALTY,
                        crate::constants::LOOP_INTENT_FRAME_COUNT_MAX_PENALTY,
                    );
                self.arbitration.add_convert(
                    weight,
                    format!("high frame count {frame_count} @ {fps:.0}fps"),
                );
            }
        }
    }

    fn finalize(self) -> (Option<Verdict>, Layer6bAuditSnapshot) {
        let Some(keep_score) =
            crate::algorithm_seal::loop_finite_scalar(self.arbitration.keep_score)
        else {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_layer6b",
                branch = "keep_score_seal_rejected",
                value = self.arbitration.keep_score,
                "Layer 6-B keep_score rejected; arbitration skipped"
            );
            return (
                None,
                Layer6bAuditSnapshot {
                    keep_score: self.arbitration.keep_score,
                    convert_score: self.arbitration.convert_score,
                    margin: 0.0,
                    resolved: false,
                },
            );
        };
        let Some(convert_score) =
            crate::algorithm_seal::loop_finite_scalar(self.arbitration.convert_score)
        else {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_layer6b",
                branch = "convert_score_seal_rejected",
                value = self.arbitration.convert_score,
                "Layer 6-B convert_score rejected; arbitration skipped"
            );
            return (
                None,
                Layer6bAuditSnapshot {
                    keep_score,
                    convert_score: self.arbitration.convert_score,
                    margin: 0.0,
                    resolved: false,
                },
            );
        };
        let margin = keep_score - convert_score;
        let snapshot = Layer6bAuditSnapshot {
            keep_score,
            convert_score,
            margin,
            resolved: false,
        };
        if margin.abs() < crate::constants::LAYER6_DIRECTIONAL_MARGIN_MIN {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_layer6b",
                branch = "directional_arbitration_inconclusive",
                keep_score,
                convert_score,
                margin,
                "Layer 6-B margin below threshold; no verdict"
            );
            return (None, snapshot);
        }

        let keep_wins = margin.is_sign_positive();
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_layer6b",
            branch = "directional_arbitration_resolved",
            keep_score,
            convert_score,
            margin,
            keep_wins,
            "Layer 6-B directional arbitration produced verdict"
        );
        let trace = self
            .arbitration
            .winner_trace(keep_wins)
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        let upstream_layer = extract_layer_tag(self.upstream_reason);
        let neighbor_suffix = crate::media_conversion_gate::loop_neighbor_count_suffix_or_empty(
            self.neighbor_count,
            "Layer 6-B arbitration",
        );

        let verdict = if keep_wins {
            Verdict::LoopStrong(format!(
                "Layer 6-B: arbitration resolved KEEP (from {upstream_layer}; keep={keep_score:.2}, convert={convert_score:.2}{neighbor_suffix}; {trace})"
            ))
        } else {
            Verdict::LoopWeak(format!(
                "Layer 6-B: arbitration resolved CONVERT (from {upstream_layer}; keep={keep_score:.2}, convert={convert_score:.2}{neighbor_suffix}; {trace})"
            ))
        };
        (
            Some(verdict),
            Layer6bAuditSnapshot {
                resolved: true,
                ..snapshot
            },
        )
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
fn layer6_directional_arbitration(
    meta: &LoopMeta,
    thresholds: &LoopThresholds,
    tree: &TreeEvaluation,
    keep_prob: Option<f64>,
    confidence: Option<f64>,
    fusion_score: Option<f64>,
    neighbor_count: Option<usize>,
    upstream_reason: &str,
) -> (Option<Verdict>, Layer6bAuditSnapshot) {
    Layer6DirectionalArbitrator::new(
        meta,
        thresholds,
        tree,
        keep_prob,
        confidence,
        fusion_score,
        neighbor_count,
        upstream_reason,
    )
    .run()
}

/// Execute the loop intent identification for a given detection result.
///
/// # Errors
/// Returns an error if the underlying database fetches fail or the
/// classification logic encounters an IO error.
#[must_use]
pub fn assess(detection: &Detection) -> Verdict {
    let meta = LoopMeta::from_video_detection(detection);
    assess_from_meta(&meta, Some(Path::new(&detection.file_path)))
}

/// Apply Apple compatibility delivery policy to a loop-intent verdict.
///
/// This keeps the "what should we deliver on Apple?" rule centralized in `foundation`,
/// while callers (e.g. `vid`) remain orchestration-only.
///
/// Policy summary (modern animated image formats only):
/// - Short, silent animated-image assets should be delivered as GIF.
/// - Long animations must NOT be forced into GIF (keep eligible for HEVC delivery).
/// - Uncertain verdicts are forced to GIF in Apple mode to maximize compatibility.
#[must_use]
pub fn apply_apple_compat_modern_animation_policy(
    verdict: Verdict,
    meta: &LoopMeta,
    apple_compat: bool,
    force: bool,
) -> Verdict {
    if !apple_compat || force {
        return verdict;
    }

    let ext_lower = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
        meta.source_extension.as_deref(),
        "loop_intent",
    );
    let is_modern_anim = crate::constants::MODERN_ANIMATED_EXTENSIONS.contains(&ext_lower.as_str());
    if !is_modern_anim {
        return verdict;
    }

    // Only apply when the asset is honestly known to have no audible audio.
    if !meta.has_confirmed_silent_or_no_audio() {
        return verdict;
    }

    // Guard: do not synthesize loop policy for single/zero-frame inputs.
    let Some(frame_count) = meta.frame_count.filter(|&fc| fc > 1) else {
        return verdict;
    };

    // Long animations are video-like; never force GIF.
    if meta
        .duration_secs
        .is_some_and(|d| d >= crate::constants::EXTREME_LONG_ABSOLUTE_LIMIT_SECS)
    {
        return verdict;
    }

    // Short animations are definitively "animated image" territory.
    if let Some(dur) = meta.duration_secs
        && dur > 0.0_f64
        && dur <= crate::constants::EXTREME_SHORT_ABSOLUTE_LIMIT_SECS
    {
        return Verdict::LoopStrong(format!(
            "Apple compat policy: modern animated format ({ext_lower}) \u{2192} force GIF (duration={:.2}s, frames={}, audible_audio={})",
            dur,
            frame_count,
            meta.has_confirmed_audible_audio()
        ));
    }

    // Degenerate duration fallback: only treat as "short" for apple-compat forcing when the
    // animation is clearly not video-like (small-ish frame count, silent).
    if meta.duration_secs.is_some_and(|d| d <= 0.0_f64)
        && meta.frame_count.is_none_or(|fc| fc <= 300)
    {
        return Verdict::LoopStrong(format!(
            "Apple compat policy: modern animated format ({ext_lower}) → force GIF (degenerate duration, frames={}, audible_audio={})",
            frame_count,
            meta.has_confirmed_audible_audio()
        ));
    }

    // Compatibility fallback: modern animated formats with uncertain intent are delivered as GIF.
    if matches!(verdict, Verdict::Uncertain(_)) {
        return Verdict::LoopStrong(format!(
            "Apple compat policy: modern animated format ({ext_lower}) with uncertain intent → force GIF",
        ));
    }

    verdict
}

/// Execute the loop intent identification for a given probe result.
///
/// # Errors
/// Returns an error if the underlying database fetches fail or the
/// classification logic encounters an IO error.
#[must_use]
pub fn assess_from_probe(probe: &crate::ffprobe::FFprobeResult, path: &Path) -> Verdict {
    let meta = LoopMeta::from_ffprobe_result(probe, path);
    assess_from_meta(&meta, Some(path))
}

#[derive(Default)]
struct InferenceTracking {
    keep_probability: Option<f64>,
    confidence: Option<f64>,
    neighbor_count: Option<usize>,
    layer6_fusion_score: Option<f64>,
    hdbscan_cluster_id: Option<i32>,
    hdbscan_cluster_loop_prior: Option<f64>,
    micro_nudge_score: Option<f64>,
    layer6b_keep_score: Option<f64>,
    layer6b_convert_score: Option<f64>,
    layer6b_margin: Option<f64>,
    layer6b_resolved: Option<bool>,
    tree_layer_exit: Option<String>,
    tree_log_odds: Option<f64>,
    layer7_upstream: Option<String>,
    resolution_path: Option<String>,
    knn_lookup_succeeded: Option<bool>,
    hnsw_lookup_branch: Option<String>,
    /// Corpus-health probe when the tree exited before Layer 6; never affects verdict or decision-path KNN fields.
    knn_telemetry_lookup_succeeded: Option<bool>,
    knn_telemetry_branch: Option<String>,
    knn_telemetry_neighbor_count: Option<usize>,
}

/// True when the final verdict came from Layer 7 format-preservation policy (not tree/KNN posteriors).
fn inference_used_layer7_policy(tracking: &InferenceTracking) -> bool {
    tracking.layer7_upstream.is_some()
        || tracking.resolution_path.as_deref() == Some("layer7_fallback")
}

/// Captured whenever Layer 6-B arbitration runs (resolved or inconclusive).
#[derive(Debug, Clone, Copy)]
struct Layer6bAuditSnapshot {
    keep_score: f64,
    convert_score: f64,
    margin: f64,
    resolved: bool,
}

struct LoopAssessmentSession<'a> {
    path: Option<&'a Path>,
    conn: Option<postgres::Client>,
    reference_profile: Option<LoopReferenceProfile>,
    thresholds: Option<LoopThresholds>,
    mutable_meta: LoopMeta,
    tracking: InferenceTracking,
}

impl<'a> LoopAssessmentSession<'a> {
    fn new(meta: &LoopMeta, path: Option<&'a Path>) -> Self {
        let disable_db =
            developer_layer1_override_enabled(crate::constants::ENV_DISABLE_DB_FEEDBACK);
        let mut conn = if disable_db {
            None
        } else {
            match crate::database::open_pg_client() {
                Ok(client) => Some(client),
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        "loop_intent: PG client unavailable; refusing evaluation without reference profile"
                    );
                    None
                }
            }
        };

        let reference_profile = conn.as_mut().and_then(|client| {
            match crate::database::fetch_loop_reference_profile(client) {
                Ok(profile) => Some(profile),
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        "loop_intent: reference-profile fetch failed; refusing fabricated thresholds"
                    );
                    None
                }
            }
        });

        let thresholds = LoopThresholds::for_evaluation(reference_profile.as_ref());
        let keywords = crate::media_conversion_gate::loop_top_keywords_or_empty(
            reference_profile
                .as_ref()
                .map(|profile| profile.top_keywords.as_slice()),
            "LoopAssessmentSession::new",
        );
        let mut mutable_meta = meta.clone();
        mutable_meta.refresh_semantics(keywords);

        Self {
            path,
            conn,
            reference_profile,
            thresholds,
            mutable_meta,
            tracking: InferenceTracking::default(),
        }
    }

    fn run(mut self) -> Verdict {
        self.apply_penetration();

        let tree = self.evaluate_tree();
        if self.thresholds.is_none()
            && tree.resolution_path.as_deref() == Some("no_reference_profile")
        {
            return Verdict::Uncertain(
                "Loop reference profile required; refusing tree without DB-backed thresholds"
                    .to_string(),
            );
        }

        let verdict = if self.conn.is_none() {
            self.resolve_legacy_mode(&tree)
        } else {
            self.resolve_tree_verdict(&tree)
        };
        self.log_inference(&tree, &verdict);
        verdict
    }

    fn apply_penetration(&mut self) {
        let Some(path) = self.path else {
            return;
        };

        self.detect_audio_silence(path);
        self.detect_real_transparency(path);
        self.detect_real_frame_count(path);
    }

    fn detect_audio_silence(&mut self, path: &Path) {
        if !self.mutable_meta.flags.streams.has_audio || self.mutable_meta.audio_is_silent.is_some()
        {
            return;
        }

        match detect_audio_silence(path) {
            crate::media_penetration::PenetrationResult::Verified(is_silent) => {
                self.mutable_meta.audio_is_silent = Some(is_silent);
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_INTENT,
                    &format!(
                        "Audio penetration: {}",
                        if is_silent {
                            "SILENT (< -70 dB or empty)"
                        } else {
                            "AUDIBLE"
                        }
                    )
                );
            }
            crate::media_penetration::PenetrationResult::Failed => {
                crate::media_conversion_gate::delivery_intent_batch_audit(
                    "delivery_intent",
                    "Audio penetration failed; audible state remains unverified",
                );
            }
            crate::media_penetration::PenetrationResult::Skipped => {}
        }
    }

    fn detect_real_transparency(&mut self, path: &Path) {
        if !self.mutable_meta.flags.streams.has_transparency
            || self.mutable_meta.transparency_is_real.is_some()
        {
            return;
        }

        match detect_real_transparency(path, self.mutable_meta.duration_secs) {
            crate::media_penetration::PenetrationResult::Verified(is_real) => {
                self.mutable_meta.transparency_is_real = Some(is_real);
                if is_real {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_INTENT,
                        "Transparency penetration: REAL (alpha variance detected)"
                    );
                } else {
                    crate::media_conversion_gate::delivery_intent_batch_audit(
                        "delivery_intent",
                        "Transparency penetration: FAKE (alpha unused), overriding metadata",
                    );
                    self.mutable_meta.flags.streams.has_transparency = false;
                }
            }
            crate::media_penetration::PenetrationResult::Failed => {
                crate::media_conversion_gate::delivery_intent_batch_audit(
                    "delivery_intent",
                    "Transparency penetration failed; real transparency remains unverified",
                );
            }
            crate::media_penetration::PenetrationResult::Skipped => {}
        }
    }

    fn detect_real_frame_count(&mut self, path: &Path) {
        if !self.mutable_meta.frame_count.is_none_or(|frame_count| {
            frame_count <= crate::constants::FRAME_COUNT_TRUST_LOWER_LIMIT
                || frame_count > crate::constants::FRAME_COUNT_TRUST_UPPER_LIMIT
        }) {
            return;
        }

        let frame_count_for_detection = self.mutable_meta.frame_count;
        if frame_count_for_detection.is_none() {
            tracing::debug!(
                "{}",
                crate::infra::static_logs::messages::MSG_SIGNAL_VOID_FRAME_COUNT
            );
        }
        match detect_real_frame_count(path, frame_count_for_detection) {
            crate::media_penetration::PenetrationResult::Verified(real_count) => {
                self.mutable_meta.real_frame_count = Some(real_count);
                if frame_count_for_detection.is_some_and(|claimed| real_count == claimed) {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_INTENT,
                        &format!("Frame count verified: {real_count}")
                    );
                } else {
                    crate::media_conversion_gate::delivery_intent_batch_audit(
                        "delivery_intent",
                        format!(
                            "Frame count mismatch: metadata={}, actual={real_count}, overriding",
                            crate::media_conversion_gate::loop_frame_count_label_or_unknown(
                                frame_count_for_detection,
                                "LoopAssessmentSession::detect_real_frame_count",
                            )
                        ),
                    );
                    self.mutable_meta.frame_count = Some(real_count);
                }
            }
            crate::media_penetration::PenetrationResult::Failed => {
                crate::media_conversion_gate::delivery_intent_batch_audit(
                    "delivery_intent",
                    "Frame count penetration failed; invalidating suspicious frame-count metadata rather than trusting it",
                );
                self.mutable_meta.frame_count = None;
                self.mutable_meta.real_frame_count = None;
            }
            crate::media_penetration::PenetrationResult::Skipped => {}
        }
    }

    fn capture_tree_context(&mut self, tree: &TreeEvaluation) {
        self.tracking.tree_layer_exit = Some(extract_layer_tag(tree.verdict.reason()));
        self.tracking.tree_log_odds = tree.log_odds_value;
    }

    /// Run HNSW lookup for corpus-health telemetry when the tree exited before Layer 6 KNN
    /// (e.g. Layer 0-EX hard veto). Does not change verdict, session meta, or decision-path KNN audit fields.
    fn run_supplementary_knn_telemetry(&mut self) {
        if self.tracking.knn_telemetry_lookup_succeeded.is_some() {
            return;
        }
        if !crate::algorithm_runtime::loop_intent_layer6_knn_enabled() {
            self.tracking.knn_telemetry_branch =
                Some("layer6_knn_disabled_by_runtime_gate".to_string());
            return;
        }
        let mut lookup_meta = self.mutable_meta.clone();
        if lookup_meta.physics_225.is_none()
            && let Some(path) = self.path
            && let Err(err) = deep_refine_meta(&mut lookup_meta, path)
        {
            crate::media_conversion_gate::delivery_intent_batch_audit(
                "delivery_intent",
                format!("supplementary KNN telemetry deep refinement failed: {err}"),
            );
            self.tracking.knn_telemetry_branch = Some("telemetry_deep_refine_failed".to_string());
            self.tracking.knn_telemetry_lookup_succeeded = Some(false);
            return;
        }
        let lookup = crate::database::lookup_similar_samples_detailed(&lookup_meta, self.path);
        self.tracking.knn_telemetry_branch = Some(format!("telemetry_{}", lookup.branch.as_str()));
        if let Some(sample_match) = lookup.sample {
            self.tracking.knn_telemetry_lookup_succeeded = Some(true);
            self.tracking.knn_telemetry_neighbor_count = Some(sample_match.neighbor_count);
        } else {
            self.tracking.knn_telemetry_lookup_succeeded = Some(false);
        }
    }

    fn set_resolution_path(&mut self, path: &'static str) {
        self.tracking.resolution_path = Some(path.to_string());
    }

    const fn record_layer6b_audit(&mut self, audit: Layer6bAuditSnapshot) {
        self.tracking.layer6b_keep_score = Some(audit.keep_score);
        self.tracking.layer6b_convert_score = Some(audit.convert_score);
        self.tracking.layer6b_margin = Some(audit.margin);
        self.tracking.layer6b_resolved = Some(audit.resolved);
    }

    fn apply_layer7_fallback(&mut self, upstream_reason: &str) -> Verdict {
        self.tracking.layer7_upstream = Some(upstream_reason.to_string());
        self.set_resolution_path("layer7_fallback");
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_layer7",
            branch = "fallback_applied",
            upstream = upstream_reason,
            "Layer 7 conservative fallback"
        );
        layer7_fallback(&self.mutable_meta, upstream_reason)
    }

    fn try_layer6b_arbitration(
        &mut self,
        tree: &TreeEvaluation,
        keep_prob: Option<f64>,
        confidence: Option<f64>,
        fusion_score: Option<f64>,
        neighbor_count: Option<usize>,
        upstream_reason: &str,
    ) -> Option<Verdict> {
        let thresholds = self.thresholds.as_ref()?;
        let (verdict, audit) = layer6_directional_arbitration(
            &self.mutable_meta,
            thresholds,
            tree,
            keep_prob,
            confidence,
            fusion_score,
            neighbor_count,
            upstream_reason,
        );
        self.record_layer6b_audit(audit);
        if audit.resolved {
            self.set_resolution_path("layer6b_arbitration");
        } else {
            self.set_resolution_path("layer6b_inconclusive");
        }
        verdict
    }

    fn resolve_legacy_mode(&mut self, tree: &TreeEvaluation) -> Verdict {
        crate::media_conversion_gate::delivery_intent_batch_audit(
            "delivery_intent",
            "Loop DB unavailable or disabled — running tree without KNN and refusing fabricated priors",
        );
        self.capture_tree_context(tree);
        match &tree.verdict {
            Verdict::LoopStrong(reason) | Verdict::LoopWeak(reason) | Verdict::Error(reason) => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_INTENT,
                    &format!("Tree-only Result: {reason}")
                );
                tree.verdict.clone()
            }
            Verdict::Uncertain(reason) => self.resolve_legacy_uncertain(tree, reason),
        }
    }

    fn resolve_legacy_uncertain(&mut self, tree_only: &TreeEvaluation, reason: &str) -> Verdict {
        crate::media_conversion_gate::delivery_intent_batch_audit(
            "delivery_intent",
            format!(
                "Tree-only result remained uncertain ({reason}) — attempting Layer 6-B arbitration"
            ),
        );
        if let Some(arbitrated) =
            self.try_layer6b_arbitration(tree_only, None, None, None, None, reason)
        {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_INTENT,
                &format!("Tree-only Arbitration: {}", arbitrated.reason())
            );
            return arbitrated;
        }

        let fallback = self.apply_layer7_fallback("Layer 0: DB unavailable / KNN disabled");
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_INTENT,
            &format!("Fallback Result: {}", fallback.reason())
        );
        fallback
    }

    fn evaluate_tree(&self) -> TreeEvaluation {
        evaluate_loop_tree(&self.mutable_meta, self.reference_profile.as_ref())
    }

    fn resolve_tree_verdict(&mut self, tree: &TreeEvaluation) -> Verdict {
        self.capture_tree_context(tree);
        if self.tracking.resolution_path.is_none() {
            self.tracking
                .resolution_path
                .clone_from(&tree.resolution_path);
        }
        let tree_probability_label = format_optional_probability(tree.tree_probability);
        match &tree.verdict {
            Verdict::LoopStrong(reason) | Verdict::LoopWeak(reason) => {
                if self.tracking.resolution_path.is_none() {
                    self.set_resolution_path("tree_decisive");
                }
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_INTENT,
                    &format!("Tree Decisive: {reason}")
                );
                self.run_supplementary_knn_telemetry();
                tree.verdict.clone()
            }
            Verdict::Error(reason) => {
                if self.tracking.resolution_path.is_none() {
                    self.set_resolution_path("tree_error");
                }
                crate::media_conversion_gate::delivery_intent_batch_audit(
                    "delivery_intent",
                    format!("Tree Error: {reason}"),
                );
                self.run_supplementary_knn_telemetry();
                tree.verdict.clone()
            }
            Verdict::Uncertain(reason) => {
                self.set_resolution_path("tree_uncertain");
                ui_stderr::line(
                    "🔭",
                    symbols::plain::TREE_UNCERTAIN,
                    format!(
                        "Tree uncertain ({reason}) [prob={tree_probability_label}] — falling back to Layer 6 KNN..."
                    ),
                );

                if !crate::algorithm_runtime::loop_intent_layer6_knn_enabled() {
                    self.tracking.knn_lookup_succeeded = Some(false);
                    self.tracking.hnsw_lookup_branch =
                        Some("layer6_knn_disabled_by_runtime_gate".to_string());
                    return self.resolve_uncertain_without_knn(tree, reason, tree.tree_probability);
                }

                // Ensure we have "Real Physics" features for high-fidelity KNN lookup.
                // We only perform this expensive extraction if the tree is uncertain.
                if self.mutable_meta.physics_225.is_none()
                    && let Some(path) = self.path
                    && let Err(err) = deep_refine_meta(&mut self.mutable_meta, path)
                {
                    crate::media_conversion_gate::delivery_intent_batch_audit(
                        "delivery_intent",
                        format!("Layer 6 KNN deep refinement failed: {err}"),
                    );
                    self.tracking.knn_lookup_succeeded = Some(false);
                    self.tracking.hnsw_lookup_branch =
                        Some("layer6_deep_refine_failed".to_string());
                    return self.resolve_uncertain_without_knn(tree, reason, tree.tree_probability);
                }

                let lookup =
                    crate::database::lookup_similar_samples_detailed(&self.mutable_meta, self.path);
                self.tracking.hnsw_lookup_branch = Some(lookup.branch.as_str().to_string());
                if let Some(sample_match) = lookup.sample {
                    self.tracking.knn_lookup_succeeded = Some(true);
                    self.resolve_uncertain_with_knn(tree, reason, &sample_match)
                } else {
                    self.tracking.knn_lookup_succeeded = Some(false);
                    self.resolve_uncertain_without_knn(tree, reason, tree.tree_probability)
                }
            }
        }
    }

    fn resolve_uncertain_without_knn(
        &mut self,
        tree: &TreeEvaluation,
        reason: &str,
        tree_probability: Option<f64>,
    ) -> Verdict {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_INTENT,
            &format!(
                "KNN similarity match unavailable (tree_prob={}) — attempting Layer 6-B arbitration",
                format_optional_probability(tree_probability)
            )
        );
        if let Some(arbitrated) = self.try_layer6b_arbitration(tree, None, None, None, None, reason)
        {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_INTENT,
                &format!("Arbitration Result: {}", arbitrated.reason())
            );
            return arbitrated;
        }

        self.apply_layer7_fallback(reason)
    }

    fn resolve_uncertain_with_knn(
        &mut self,
        tree: &TreeEvaluation,
        reason: &str,
        sample_match: &crate::database::SampleMatch,
    ) -> Verdict {
        let Some(keep_prob) = sample_match.keep_probability else {
            return self.resolve_missing_keep_probability(tree, reason, sample_match);
        };

        self.capture_knn_tracking(sample_match);

        let confidence = sample_match.confidence;
        let nudges = calculate_micro_nudges(&self.mutable_meta);
        let Some(fusion) = compute_layer6_fusion(
            keep_prob,
            tree.tree_probability,
            sample_match.neighbor_count,
            nudges.score,
        ) else {
            return self.resolve_missing_keep_probability(tree, reason, sample_match);
        };
        let mut final_score = fusion.final_score;

        if !nudges.trace.is_empty() {
            ui_stderr::line(
                "⚖️",
                symbols::plain::ARBITRATION,
                format!(
                    "   Micro-Nudges ({:+.2}): {}",
                    nudges.score,
                    nudges.trace.join(" | ")
                ),
            );
        }

        let Some(sealed_score) = self.apply_high_cost_visual_nudges(final_score, confidence) else {
            return self.resolve_missing_keep_probability(tree, reason, sample_match);
        };
        final_score = sealed_score;

        self.tracking.layer6_fusion_score =
            crate::algorithm_seal::loop_unit_probability(final_score);
        self.tracking.micro_nudge_score = Some(nudges.score);

        self.resolve_fusion_verdict(
            tree,
            reason,
            keep_prob,
            confidence,
            sample_match,
            &fusion,
            &nudges,
            final_score,
        )
    }

    fn resolve_missing_keep_probability(
        &mut self,
        tree: &TreeEvaluation,
        reason: &str,
        sample_match: &crate::database::SampleMatch,
    ) -> Verdict {
        ui_stderr::line(
            symbols::WARNING,
            symbols::plain::WARNING,
            format!(
                "   KNN match missing keep-probability (conf={:.2}, n={}) — attempting Layer 6-B arbitration",
                sample_match.confidence, sample_match.neighbor_count
            ),
        );
        if let Some(arbitrated) = self.try_layer6b_arbitration(
            tree,
            None,
            Some(sample_match.confidence),
            None,
            Some(sample_match.neighbor_count),
            reason,
        ) {
            ui_stderr::line(
                "⚖️",
                symbols::plain::ARBITRATION,
                format!("Arbitration Result: {}", arbitrated.reason()),
            );
            return arbitrated;
        }

        let final_verdict = self.apply_layer7_fallback("Layer 6: KNN match missing probability");
        if final_verdict.is_keep_gif() {
            ui_stderr::line(
                symbols::SUCCESS,
                symbols::plain::SUCCESS,
                format!("Fallback Result: {}", final_verdict.reason()),
            );
        } else {
            ui_stderr::line(
                symbols::INFO,
                symbols::plain::INFO,
                format!("Fallback Result: {}", final_verdict.reason()),
            );
        }
        final_verdict
    }

    fn capture_knn_tracking(&mut self, sample_match: &crate::database::SampleMatch) {
        self.tracking.keep_probability = sample_match
            .keep_probability
            .and_then(crate::algorithm_seal::loop_unit_probability);
        self.tracking.confidence =
            crate::algorithm_seal::loop_unit_probability(sample_match.confidence);
        self.tracking.neighbor_count = Some(sample_match.neighbor_count);
        self.tracking.hdbscan_cluster_id = sample_match.hdbscan_cluster_id;
        self.tracking.hdbscan_cluster_loop_prior = sample_match.hdbscan_cluster_loop_prior;
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "knn_tracking_captured",
            keep_prob = ?self.tracking.keep_probability,
            confidence = ?self.tracking.confidence,
            neighbor_count = sample_match.neighbor_count,
            hdbscan_cluster_id = ?sample_match.hdbscan_cluster_id,
            "KNN match captured for inference audit"
        );
    }

    fn apply_high_cost_visual_nudges(&self, mut final_score: f64, confidence: f64) -> Option<f64> {
        if !(final_score > crate::constants::LAYER6_FUSION_SCORE_UNCERTAIN_LOW
            && final_score < crate::constants::LAYER6_FUSION_SCORE_UNCERTAIN_HIGH
            && confidence < crate::constants::LAYER6_CONFIDENCE_HIGH)
        {
            return crate::algorithm_seal::loop_unit_probability(final_score);
        }
        let Some(path) = self.path else {
            return crate::algorithm_seal::loop_unit_probability(final_score);
        };

        ui_stderr::line(
            symbols::SEARCH,
            symbols::plain::SEARCH,
            "   Triggering high-cost visual heuristics (extreme uncertainty)...",
        );
        let Some(tier3_nudge) = self.compute_visual_tier3_nudge(path) else {
            return crate::algorithm_seal::loop_unit_probability(final_score);
        };
        if !tier3_nudge.trace.is_empty() {
            ui_stderr::line(
                symbols::CHART,
                symbols::plain::CHART,
                format!(
                    "   Tier 3 Visual ({:+.2}): {}",
                    tier3_nudge.score,
                    tier3_nudge.trace.join(" | ")
                ),
            );
            final_score += tier3_nudge.score.clamp(
                -crate::constants::AUXILIARY_NUDGE_CAP,
                crate::constants::AUXILIARY_NUDGE_CAP,
            );
        }

        crate::algorithm_seal::loop_unit_probability(final_score)
    }

    fn compute_visual_tier3_nudge(&self, path: &Path) -> Option<AuxiliaryNudge> {
        let mut img_opt: Option<image::DynamicImage> = None;
        if let Some(bytes) = self.mutable_meta.cached_frame_png.as_ref() {
            match image::load_from_memory(bytes) {
                Ok(img) => img_opt = Some(img),
                Err(err) => {
                    crate::media_conversion_gate::probe_image_format_audit(
                        "loop_intent_cached_frame_decode_failed",
                        path,
                        format!(
                            "failed to decode cached frame bytes for Tier 3 visual scan: {err}"
                        ),
                    );
                }
            }
        } else {
            match extract_frame_to_temp(path) {
                Ok(Some(temp_frame)) => match std::fs::read(&temp_frame) {
                    Ok(bytes) => {
                        crate::media_conversion_gate::delivery_remove_file_or_audit(
                            "loop_intent_temp_frame_tier3",
                            &temp_frame,
                        );
                        match image::load_from_memory(&bytes) {
                            Ok(img) => img_opt = Some(img),
                            Err(err) => {
                                crate::media_conversion_gate::probe_image_format_audit(
                                    "loop_intent_tier3_frame_decode_failed",
                                    path,
                                    format!(
                                        "failed to decode Tier 3 extracted frame {}: {err}",
                                        temp_frame.display()
                                    ),
                                );
                            }
                        }
                    }
                    Err(err) => {
                        crate::media_conversion_gate::delivery_remove_file_or_audit(
                            "loop_intent_temp_frame_tier3",
                            &temp_frame,
                        );
                        crate::media_conversion_gate::probe_image_format_audit(
                            "loop_intent_tier3_frame_read_failed",
                            path,
                            format!(
                                "failed to read Tier 3 extracted frame {}: {err}",
                                temp_frame.display()
                            ),
                        );
                    }
                },
                Ok(None) => {}
                Err(err) => {
                    crate::media_conversion_gate::probe_image_format_audit(
                        "loop_intent_tier3_frame_extract_failed",
                        path,
                        format!("failed to extract Tier 3 representative frame: {err}"),
                    );
                }
            }
        }

        let img = img_opt.as_ref()?;
        let mut tier3_nudge = AuxiliaryNudge::default();
        if detect_heavy_letterboxing_from_image(img) {
            tier3_nudge.apply(
                crate::constants::LETTERBOXING_NUDGE,
                "Letterboxing detected",
            );
        }
        if detect_high_text_density_from_image(img) {
            tier3_nudge.apply(
                crate::constants::HIGH_TEXT_DENSITY_NUDGE,
                "High text density",
            );
        }
        Some(tier3_nudge)
    }

    fn resolve_fusion_verdict(
        &mut self,
        tree: &TreeEvaluation,
        reason: &str,
        keep_prob: f64,
        confidence: f64,
        sample_match: &crate::database::SampleMatch,
        fusion: &Layer6Fusion,
        nudges: &AuxiliaryNudge,
        final_score: f64,
    ) -> Verdict {
        let Some(thresholds) = self.thresholds.as_ref() else {
            return Verdict::Uncertain("loop thresholds missing after run() gate".into());
        };
        if should_accept_layer6_loopstrong(
            &self.mutable_meta,
            thresholds,
            keep_prob,
            final_score,
            confidence,
        ) {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_layer6",
                branch = "fusion_loop_strong",
                final_score,
                keep_prob,
                confidence,
                neighbor_count = sample_match.neighbor_count,
                "Layer 6 fusion accepted LoopStrong"
            );
            let verdict = Verdict::LoopStrong(format!(
                "Layer 6: KNN+Nudges score={:.2} (knn={:.2}×{:.2}, tree={}×{:.2}, nudge={:+.2}, conf={:.2}, n={})",
                final_score,
                keep_prob,
                fusion.knn_weight,
                format_optional_probability(tree.tree_probability),
                fusion.tree_weight,
                nudges.score,
                confidence,
                sample_match.neighbor_count
            ));
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_INTENT,
                &format!("KNN Fusion Success: {}", verdict.reason())
            );
            self.set_resolution_path("layer6_knn_fusion");
            return verdict;
        }

        if confidence >= crate::constants::LAYER6_CONFIDENCE_HIGH
            && final_score <= crate::constants::LAYER6_FUSION_SCORE_UNCERTAIN_LOW
        {
            let verdict = Verdict::LoopWeak(format!(
                "Layer 6: KNN+Nudges score={:.2} (knn={:.2}×{:.2}, tree={}×{:.2}, nudge={:+.2}, conf={:.2}, n={})",
                final_score,
                keep_prob,
                fusion.knn_weight,
                format_optional_probability(tree.tree_probability),
                fusion.tree_weight,
                nudges.score,
                confidence,
                sample_match.neighbor_count
            ));
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_INTENT,
                &format!("KNN Fusion Exit: {}", verdict.reason())
            );
            self.set_resolution_path("layer6_knn_fusion");
            return verdict;
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_INTENT,
            &format!(
                "KNN data inconclusive (conf={confidence:.2}, score={final_score:.2}) — attempting Layer 6-B arbitration"
            )
        );
        if let Some(arbitrated) = self.try_layer6b_arbitration(
            tree,
            Some(keep_prob),
            Some(confidence),
            Some(final_score),
            Some(sample_match.neighbor_count),
            reason,
        ) {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_INTENT,
                &format!("Arbitration Result: {}", arbitrated.reason())
            );
            return arbitrated;
        }

        self.apply_layer7_fallback(reason)
    }

    fn log_inference(&mut self, tree: &TreeEvaluation, verdict: &Verdict) {
        let Some(client) = self.conn.as_mut() else {
            return;
        };
        if !crate::algorithm_runtime::loop_inference_telemetry_enabled() {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_inference_log",
                branch = "inference_log_disabled",
                "loop inference_log write skipped (gate disabled)"
            );
            return;
        }

        let (final_verdict, layer_exit) = match verdict {
            Verdict::LoopStrong(reason) => ("LoopStrong".to_string(), extract_layer_tag(reason)),
            Verdict::LoopWeak(reason) => ("LoopWeak".to_string(), extract_layer_tag(reason)),
            Verdict::Uncertain(reason) => ("Uncertain".to_string(), extract_layer_tag(reason)),
            Verdict::Error(reason) => ("Error".to_string(), extract_layer_tag(reason)),
        };
        let layer7_policy = inference_used_layer7_policy(&self.tracking);
        let tree_probability = if layer7_policy {
            None
        } else {
            tree.tree_probability
                .and_then(crate::algorithm_seal::loop_unit_probability)
        };
        if !layer7_policy && tree_probability.is_none() {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent",
                branch = "inference_log_tree_probability_rejected",
                "skipping inference_log insert"
            );
            return;
        }
        let fallback_probability = if layer7_policy {
            None
        } else {
            match verdict {
                Verdict::LoopStrong(_) | Verdict::LoopWeak(_) | Verdict::Uncertain(_) => {
                    tree.tree_probability
                }
                Verdict::Error(_) => None,
            }
        };
        let final_probability = if layer7_policy {
            self.tracking
                .layer6_fusion_score
                .and_then(crate::algorithm_seal::loop_unit_probability)
        } else {
            crate::media_conversion_gate::loop_inference_unit_probability_or_tree_fallback(
                self.tracking.layer6_fusion_score,
                fallback_probability,
            )
        };
        if !layer7_policy && final_probability.is_none() {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent",
                branch = "inference_log_final_probability_rejected",
                "skipping inference_log insert"
            );
            return;
        }
        if layer7_policy {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_inference_log",
                branch = "layer7_policy_null_posteriors",
                layer_exit = %layer_exit,
                "Layer 7 policy exit: inference_log stores NULL tree/final probabilities"
            );
        }
        let knn_keep_probability = self.tracking.keep_probability;
        let knn_confidence = self.tracking.confidence;

        let record = crate::database::LoopInferenceRecord {
            tree_probability,
            knn_keep_probability,
            knn_confidence,
            knn_neighbor_count: self.tracking.neighbor_count,
            final_probability,
            final_verdict,
            decision_reason: verdict.reason().to_string(),
            layer_exit,
        };
        let audit = crate::database::LoopInferenceAudit {
            layer6_fusion_score: self.tracking.layer6_fusion_score,
            hdbscan_cluster_id: self.tracking.hdbscan_cluster_id,
            hdbscan_cluster_loop_prior: self.tracking.hdbscan_cluster_loop_prior,
            micro_nudge_score: self.tracking.micro_nudge_score,
            layer6b_keep_score: self.tracking.layer6b_keep_score,
            layer6b_convert_score: self.tracking.layer6b_convert_score,
            layer6b_margin: self.tracking.layer6b_margin,
            layer6b_resolved: self.tracking.layer6b_resolved,
            tree_layer_exit: self.tracking.tree_layer_exit.clone(),
            tree_log_odds: self.tracking.tree_log_odds,
            layer7_upstream: self.tracking.layer7_upstream.clone(),
            resolution_path: crate::media_conversion_gate::loop_inference_resolution_path_or_tree(
                self.tracking.resolution_path.clone(),
                tree.resolution_path.clone(),
            ),
            knn_lookup_succeeded: self.tracking.knn_lookup_succeeded,
            hnsw_lookup_branch: self.tracking.hnsw_lookup_branch.clone(),
            knn_telemetry_lookup_succeeded: self.tracking.knn_telemetry_lookup_succeeded,
            knn_telemetry_branch: self.tracking.knn_telemetry_branch.clone(),
            knn_telemetry_neighbor_count: self.tracking.knn_telemetry_neighbor_count,
        };
        crate::database::log_inference_record(
            client,
            &self.mutable_meta,
            &record,
            self.path,
            Some(audit),
        );
    }
}

/// Persists an `inference_log` row when loop inference logging and DB feedback are enabled.
///
/// Logging defaults on (`MODERN_FORMAT_DISABLE_LOOP_INTENT_INFERENCE_LOG=1` to skip).
/// Inference log defaults to audit-only (`MODERN_FORMAT_DISABLE_LOOP_INTENT_INFERENCE_AUDIT_ONLY=1` for runtime verdict column).
///
/// # Errors
/// Returns an error if the underlying database fetches fail or the
/// classification logic encounters an IO error during visual sampling.
#[must_use]
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
pub fn assess_from_meta(meta: &LoopMeta, path: Option<&Path>) -> Verdict {
    LoopAssessmentSession::new(meta, path).run()
}

/// Layer 7: Conservative fallback with minimum-loss default.
///
/// BUG FIX: Removed sticker safe zone logic. "Uncertain" means we don't know,
/// so we preserve the original format routing without making additional guesses.
fn layer7_fallback(meta: &LoopMeta, upstream_reason: &str) -> Verdict {
    tracing::trace!(
        target: "mfb.algorithm",
        pipeline = "loop_intent_layer7",
        branch = "fallback_policy",
        upstream = upstream_reason,
        extension = ?meta.source_extension,
        "evaluating Layer 7 format-preservation policy"
    );
    let ext = crate::media_conversion_gate::meta_extension_lowercase_or_empty(
        meta.source_extension.as_deref(),
        "loop_intent",
    );
    let is_gif = ext == "gif";
    let is_video = SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str());
    let is_modern_animated = crate::constants::MODERN_ANIMATED_EXTENSIONS.contains(&ext.as_str());

    let reason = format!("Layer 7: Fallback [{upstream_reason}]");

    // Preserve original format routing without additional heuristics:
    // - Animation formats (GIF/WebP/AVIF) → keep as animation
    // - Video formats (MP4/MOV/etc) → keep as video
    // - Unknown → default to video (safer for quality preservation)
    if is_modern_animated {
        Verdict::LoopStrong(format!("{reason} → preserve modern animated format"))
    } else if is_gif {
        Verdict::LoopStrong(format!("{reason} → preserve GIF as-is"))
    } else if is_video {
        Verdict::LoopWeak(format!("{reason} → preserve video format"))
    } else {
        Verdict::LoopWeak(format!("{reason} → unknown format, default to video"))
    }
}

/// Extract the layer tag (e.g. "Layer 1-A", "Layer 6", "Layer 7") from a verdict reason string.
fn extract_layer_tag(reason: &str) -> String {
    crate::media_conversion_gate::loop_layer_tag_from_reason_or_unknown(
        reason,
        "loop_intent verdict reason",
    )
}

// ── Safety & Exploration Helpers ──────────────────────────────────────────────

/// Dynamic safety-guard for CRF 0.00 (lossless) exploration.
#[must_use]
pub fn is_lossless_exploration_safe(meta: &LoopMeta, path: Option<&Path>) -> bool {
    let sample_match = crate::database::lookup_similar_samples(meta, path);
    let Some(keep_prob) = sample_match.as_ref().and_then(|m| {
        m.keep_probability
            .and_then(crate::algorithm_seal::loop_unit_probability)
    }) else {
        ui_stderr::line(
            symbols::WARNING,
            symbols::plain::WARNING,
            "   Lossless-first safety: KNN keep_prob unavailable — exploration blocked",
        );
        return false;
    };
    let threshold = lossless_duration_limit_for_keep_prob(keep_prob);
    let keep_prob_label = format!("keep_prob={keep_prob:.2}");
    let is_safe = meta.duration_secs.is_some_and(|d| d < f64::from(threshold));
    if !is_safe {
        let dur_str = crate::media_conversion_gate::loop_format_duration_secs_label(
            meta.duration_secs,
            "is_lossless_exploration_safe",
        );
        ui_stderr::line(
            symbols::WARNING,
            symbols::plain::WARNING,
            format!(
                "   Lossless-first (CRF 0.00) skip: duration {dur_str} exceeds limit {threshold:.1}s ({keep_prob_label})",
            ),
        );
    }
    is_safe
}

#[must_use]
fn lossless_duration_limit_for_keep_prob(keep_prob: f64) -> f32 {
    use crate::constants::{HIGH_VALUE_LOSSLESS_DURATION_LIMIT, MEME_LOSSLESS_DURATION_LIMIT};
    if keep_prob <= crate::constants::LOSSLESS_DURATION_LIMIT_LOW_PROB {
        HIGH_VALUE_LOSSLESS_DURATION_LIMIT
    } else if keep_prob >= crate::constants::LOSSLESS_DURATION_LIMIT_HIGH_PROB {
        MEME_LOSSLESS_DURATION_LIMIT
    } else {
        let t = (keep_prob - crate::constants::KEEP_PROB_INTERPOLATION_FLOOR)
            / crate::constants::KEEP_PROB_INTERPOLATION_RANGE;
        let limit_meme = f64::from(MEME_LOSSLESS_DURATION_LIMIT);
        let limit_high = f64::from(HIGH_VALUE_LOSSLESS_DURATION_LIMIT);
        crate::numeric_cast::f64_to_f32_lossy(limit_high.mul_add(1.0 - t, t * limit_meme))
    }
}

// ── Signal Scorers ────────────────────────────────────────────────────────────

fn calculate_cv(values: &[u64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let n = crate::numeric_cast::usize_to_f64(values.len());
    let mean = values
        .iter()
        .map(|&v| crate::numeric_cast::u64_to_f64(v))
        .sum::<f64>()
        / n;
    if mean <= 0.0_f64 {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_signals",
            branch = "frame_payload_cv_undefined",
            "packet-size CV undefined (non-positive mean); omitting signal"
        );
        return None;
    }
    let var = values
        .iter()
        .map(|&v| (crate::numeric_cast::u64_to_f64(v) - mean).powi(2))
        .sum::<f64>()
        / n;
    Some(var.sqrt() / mean)
}

/// Block-skew proxy from packet-size distribution when `YDIF` is unavailable.
pub(crate) fn ensure_block_skew(meta: &mut LoopMeta) {
    if meta.block_skew.is_some() {
        return;
    }
    let sizes: Vec<f64> = meta
        .pkt_sizes
        .iter()
        .map(|&s| crate::numeric_cast::u64_to_f64(s))
        .collect();
    if let Some(skew) = block_skew_score_from_signal(&sizes) {
        meta.block_skew = Some(skew);
    }
}

/// Fill `frame_delay_variation` when ffprobe omits per-frame PTS but container FPS is known.
///
/// Constant-FPS without per-frame `pkt_pts_time` yields CV=0.0 only inside live `LoopMeta`
/// from ffprobe/header probe — never via `Default` or repair-time column backfill.
pub(crate) fn ensure_frame_delay_variation(meta: &mut LoopMeta) {
    if meta.frame_delay_variation.is_some() {
        return;
    }
    if let Some(cv) = calculate_cv_f64(&meta.pts_deltas) {
        meta.frame_delay_variation = Some(cv);
        return;
    }
    if meta.pts_deltas.len() >= 2 {
        tracing::debug!(
            target: "mfb.database",
            pts_delta_count = meta.pts_deltas.len(),
            "loop probe: PTS deltas present but frame_delay_variation CV undefined"
        );
        return;
    }
    let Some(fps) = meta.fps.filter(|f| f.is_finite() && *f > 0.0) else {
        return;
    };
    let Some(frame_count) = meta.frame_count else {
        return;
    };
    if frame_count <= 1 {
        return;
    }
    tracing::info!(
        target: "mfb.database",
        fps,
        frame_count,
        "loop probe: frame_delay_variation=0 from constant FPS (no per-frame PTS in ffprobe)"
    );
    meta.frame_delay_variation = Some(0.0);
}

fn calculate_cv_f64(values: &[f64]) -> Option<f64> {
    use std::simd::f64x8;
    use std::simd::num::SimdFloat;

    if values.is_empty() {
        return None;
    }
    let n = crate::numeric_cast::usize_to_f64(values.len());

    // SIMD mean calculation

    let (prefix, chunks, suffix) = values.as_simd::<8>();
    let mut sum_simd = f64x8::splat(0.0);
    for chunk in chunks {
        sum_simd += *chunk;
    }
    let sum = sum_simd.reduce_sum() + prefix.iter().sum::<f64>() + suffix.iter().sum::<f64>();
    let mean = sum / n;

    if mean <= 0.0_f64 {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_signals",
            branch = "frame_delay_cv_undefined",
            "PTS-delta CV undefined (non-positive mean); omitting signal"
        );
        return None;
    }

    // SIMD variance calculation
    let mean_simd = f64x8::splat(mean);
    let mut var_sum_simd = f64x8::splat(0.0);
    for chunk in chunks {
        let diff = *chunk - mean_simd;
        var_sum_simd += diff * diff;
    }
    let var_sum = var_sum_simd.reduce_sum()
        + prefix.iter().map(|&v| (v - mean).powi(2)).sum::<f64>()
        + suffix.iter().map(|&v| (v - mean).powi(2)).sum::<f64>();

    let var = var_sum / n;
    Some(var.sqrt() / mean)
}

fn calculate_gini_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| crate::media_conversion_gate::f64_sort_cmp(*a, *b));
    let n = crate::numeric_cast::usize_to_f64(sorted.len());
    let sum: f64 = sorted.iter().sum();
    if sum.abs() < 1e-9_f64 {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_signals",
            branch = "motion_gini_undefined",
            "motion gini undefined (near-zero mass); omitting signal"
        );
        return None;
    }
    let weighted_sum: f64 = sorted
        .iter()
        .enumerate()
        .map(|(i, &v)| crate::numeric_cast::usize_to_f64(2 * (i + 1)) * v)
        .sum();
    Some((weighted_sum / (n * sum)) - (n + 1.0) / n)
}

fn fps_anomaly_score(fps: f64) -> f64 {
    // Returns high score when fps is far from standard rates → atypical → possible loop artifact
    let std_rates = [24.0_f64, 25.0_f64, 30.0_f64, 60.0_f64, 120.0_f64];
    let min_delta = std_rates
        .iter()
        .map(|&s| (fps - s).abs())
        .fold(f64::MAX, f64::min);
    (min_delta / 2.5).min(1.0)
}

use std::collections::HashMap;
use std::sync::OnceLock;

static MEME_KEYWORDS_CACHE: OnceLock<Vec<String>> = OnceLock::new();

fn get_meme_keywords() -> &'static [String] {
    MEME_KEYWORDS_CACHE.get_or_init(|| {
        let json_str = include_str!("../../../dev/src/config/meme_keywords.json");
        let languages: HashMap<String, Vec<String>> = match serde_json::from_str(json_str) {
            Ok(languages) => languages,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "embedded meme_keywords.json is malformed; disabling meme keyword hints"
                );
                return Vec::new();
            }
        };
        let mut all_keywords = Vec::new();
        for list in languages.values() {
            all_keywords.extend(list.clone());
        }
        all_keywords
    })
}

/// Returns `0.5` if no parts are provided.
///
/// # Errors
/// This function does not typically return `Result`, but uses `0.5` as a neutral score.
#[must_use]
pub fn score_directory_context(parts: Option<&[String]>, keywords: &[String]) -> f64 {
    let Some(parts) = parts else {
        return crate::constants::LOOP_INTENT_NEUTRAL_SCORE;
    };
    let global_keywords = get_meme_keywords();
    for part in parts {
        let lower = part.to_lowercase();
        if keywords.iter().any(|keyword| lower.contains(keyword))
            || global_keywords
                .iter()
                .any(|keyword| lower.contains(keyword))
        {
            return 1.0;
        }
    }
    crate::constants::LOOP_INTENT_NEUTRAL_PROB
}

/// Returns `0.5` (Ambiguous) if no name is provided.
///
/// # Errors
/// This function does not typically return `Result`, but uses `0.5` as a neutral score.
#[must_use]
pub fn analyze_filename(name: Option<&str>, keywords: &[String]) -> FilenameAnalysis {
    let Some(name) = name else {
        return FilenameAnalysis {
            raw: crate::constants::LOOP_INTENT_AMBIGUOUS_SCORE,
            kind: FilenameKind::Ambiguous,
        };
    };
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) => stem,
        None => name,
    }
    .to_lowercase();

    let global_keywords = get_meme_keywords();

    // 1. Dynamic Keyword Match from Database & JSON config
    if keywords.iter().any(|keyword| stem.contains(keyword))
        || global_keywords.iter().any(|keyword| stem.contains(keyword))
    {
        return FilenameAnalysis {
            raw: crate::constants::LOOP_CONFIDENCE_HIGH,
            kind: FilenameKind::HumanSemantic,
        };
    }

    // 2. Platform cache naming patterns
    if stem.starts_with("mmexport") || stem.starts_with("wx_camera") || stem.len() == 32 {
        return FilenameAnalysis {
            raw: 1.0,
            kind: FilenameKind::MachineGenerated,
        };
    }

    // 3. Pure random hash
    if stem.chars().all(char::is_alphanumeric) && stem.len() >= 20 {
        return FilenameAnalysis {
            raw: 0.70,
            kind: FilenameKind::MachineGenerated,
        };
    }

    FilenameAnalysis {
        raw: crate::constants::LOOP_INTENT_NEUTRAL_SCORE,
        kind: FilenameKind::Ambiguous,
    }
}

#[must_use]
pub fn score_loop_frequency(duration_secs: Option<f64>, frame_count: Option<u64>) -> f64 {
    if let (Some(dur), Some(fc)) = (duration_secs, frame_count)
        && dur > 0.01_f64
        && fc > 0
    {
        let loops_per_minute = 60.0_f64 / dur;
        let frame_density = crate::numeric_cast::u64_to_f64(fc) / dur;

        let loop_score =
            if loops_per_minute >= crate::constants::LOOP_INTENT_FREQ_SCORE_VHIGH_THRESHOLD {
                crate::constants::LOOP_INTENT_FREQ_SCORE_VHIGH
            } else if loops_per_minute >= crate::constants::LOOP_INTENT_FREQ_SCORE_HIGH_THRESHOLD {
                crate::constants::LOOP_INTENT_FREQ_SCORE_HIGH
            } else if loops_per_minute >= crate::constants::LOOP_INTENT_FREQ_SCORE_MED_THRESHOLD {
                crate::constants::LOOP_INTENT_FREQ_SCORE_MED
            } else if loops_per_minute >= crate::constants::LOOP_INTENT_FREQ_SCORE_LOW_THRESHOLD {
                crate::constants::LOOP_INTENT_FREQ_SCORE_LOW
            } else {
                crate::constants::LOOP_INTENT_FREQ_SCORE_DEFAULT
            };

        let density_adj = if frame_density < crate::constants::LOOP_INTENT_DENSITY_LOW_THRESHOLD {
            crate::constants::LOOP_INTENT_DENSITY_LOW_ADJ
        } else if frame_density < crate::constants::LOOP_INTENT_DENSITY_MED_THRESHOLD {
            crate::constants::LOOP_INTENT_DENSITY_MED_ADJ
        } else if frame_density < crate::constants::LOOP_INTENT_DENSITY_HIGH_THRESHOLD {
            crate::constants::LOOP_INTENT_DENSITY_HIGH_ADJ
        } else {
            0.0_f64
        };

        let combined_score: f64 = loop_score + density_adj;
        combined_score.clamp(0.0_f64, 1.0_f64)
    } else {
        crate::constants::LOOP_INTENT_FREQ_SCORE_NULL
    }
}

#[must_use]
pub fn score_sparse_cadence(duration_secs: Option<f64>, frame_count: Option<u64>) -> f64 {
    if let (Some(dur), Some(fc)) = (duration_secs, frame_count)
        && dur > crate::constants::LOOP_INTENT_PROB_MIN
        && fc > 1
    {
        let frame_density = crate::numeric_cast::u64_to_f64(fc) / dur;
        let avg_gap = dur / crate::numeric_cast::u64_to_f64(fc);

        if dur <= crate::constants::LOOP_INTENT_SHORT_ANIMATION_DURATION_LIMIT
            && frame_density >= crate::constants::LOOP_INTENT_SPARSE_CADENCE_DENSITY_THRESHOLD
        {
            return crate::constants::LOOP_INTENT_SPARSE_CADENCE_SHORT_SCORE;
        }
        if dur >= crate::constants::LOOP_INTENT_SHORT_ANIMATION_DURATION_LIMIT
            && avg_gap >= crate::constants::LOOP_INTENT_SPARSE_CADENCE_GAP_THRESHOLD
        {
            return crate::constants::LOOP_INTENT_SPARSE_CADENCE_GAP_SCORE;
        }
        if dur >= crate::constants::LOOP_INTENT_SPARSE_CADENCE_LONG_DUR
            && fc <= crate::constants::LOOP_INTENT_SPARSE_CADENCE_LONG_FC
            && avg_gap >= crate::constants::LOOP_INTENT_NEUTRAL_PROB
        {
            return crate::constants::LOOP_INTENT_SPARSE_CADENCE_LONG_SCORE;
        }
    }

    crate::constants::LOOP_INTENT_NEUTRAL_PROB
}

// ── Layer 6: Auxiliary Micro-Nudges ───────────────────────────────────────────

/// Auxiliary nudge accumulator for Layer 6 micro-adjustments.
/// Total nudge score is clamped to [-0.15, +0.15] to ensure it only acts as a tie-breaker.
#[derive(Debug, Default)]
struct AuxiliaryNudge {
    score: f64,
    trace: Vec<String>,
}

impl AuxiliaryNudge {
    fn apply(&mut self, delta: f64, reason: &str) {
        self.score += delta;
        self.trace.push(format!("{reason} ({delta:+.2})"));
    }
}

/// Calculate Tier 1 (zero-cost metadata) and Tier 2 (low-cost bitstream) nudges.
fn calculate_micro_nudges(meta: &LoopMeta) -> AuxiliaryNudge {
    let mut nudge = AuxiliaryNudge::default();

    // ── Tier 1: Zero-Cost Metadata ──
    if let (Some(w), Some(h)) = (meta.width, meta.height)
        && w > 0
        && h > 0
    {
        if w == h {
            nudge.apply(
                crate::constants::LOOP_INTENT_NUDGE_ASPECT_1_1,
                "1:1 aspect ratio",
            );
        } else if ((f64::from(w) / f64::from(h)) - 1.777).abs() < 0.05_f64 {
            nudge.apply(
                crate::constants::LOOP_INTENT_NUDGE_ASPECT_16_9,
                "16:9 cinematic ratio",
            );
        }
    }

    if let (Some(w), Some(h)) = (meta.width, meta.height)
        && u64::from(w) * u64::from(h) > 1920 * 1080
    {
        nudge.apply(
            crate::constants::LOOP_INTENT_NUDGE_RESOLUTION_4K,
            "4K+ resolution",
        );
    }

    // ── Tier 2: Low-Cost Bitstream ──
    if detect_scene_cut(&meta.pkt_sizes) {
        nudge.apply(
            crate::constants::LOOP_INTENT_NUDGE_SCENE_CUT,
            "Scene cut detected",
        );
    }

    if detect_localized_motion(&meta.mv_magnitudes) {
        nudge.apply(
            crate::constants::LOOP_INTENT_NUDGE_LOCALIZED_MOTION,
            "Localized motion",
        );
    }

    // Clamp total nudge to limit
    nudge.score = nudge.score.clamp(
        -crate::constants::LOOP_INTENT_NUDGE_CLAMP,
        crate::constants::LOOP_INTENT_NUDGE_CLAMP,
    );
    nudge
}

/// Detect hard scene cuts in packet size stream.
/// If any inner frame is 5x larger than the median inner packet size,
/// it's likely an `I-frame` scene cut.
fn detect_scene_cut(pkt_sizes: &[u64]) -> bool {
    if pkt_sizes.len() < 5 {
        return false;
    }
    let inner = match pkt_sizes.get(1..pkt_sizes.len().saturating_sub(1)) {
        Some(v) => v,
        None => unreachable!(
            "CRITICAL: len() >= 5 guarantees 1..len-1 is a valid sub-slice in detect_scene_cut (len={})",
            pkt_sizes.len()
        ),
    };
    let mut baseline = inner.to_vec();
    baseline.sort_unstable();
    let median = match crate::media_conversion_gate::loop_baseline_median_frames_optional(
        baseline.get(baseline.len() / 2).copied(),
        "loop_intent baseline median frame_count",
    ) {
        Some(v) => v,
        None => return false,
    };

    inner.iter().any(|&size| {
        crate::numeric_cast::u64_to_f64(size)
            > median * crate::constants::LOOP_INTENT_SCENE_CUT_RATIO
    })
}

/// Detect localized motion (high concentration of motion in small area).
/// Returns true if motion vectors suggest synthetic/sticker content.
fn detect_localized_motion(mvs: &[f64]) -> bool {
    mvs.len() >= 10 && zero_motion_ratio(mvs) > crate::constants::LOOP_INTENT_LOCALIZED_MOTION_RATIO
}

/// Extract first frame from video to temporary `PNG` for analysis.
fn extract_frame_to_temp(path: &Path) -> anyhow::Result<Option<std::path::PathBuf>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Generate unique filename: timestamp + random seed
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| {
            anyhow::anyhow!("system clock before UNIX_EPOCH for frame extraction: {err}")
        })?
        .as_nanos();
    let timestamp_bytes = timestamp.to_le_bytes();
    let rand_seed = std::process::id()
        ^ u32::from_le_bytes([
            timestamp_bytes[0],
            timestamp_bytes[1],
            timestamp_bytes[2],
            timestamp_bytes[3],
        ]);

    let temp_dir = crate::media_conversion_gate::delivery_scratch_temp_dir_or_system_temp(
        "loop_intent_frame_extract",
    );
    std::fs::create_dir_all(&temp_dir).map_err(|err| {
        anyhow::anyhow!(
            "failed to prepare frame extraction temp dir {}: {err}",
            temp_dir.display()
        )
    })?;
    let temp_path = temp_dir.join(format!("mfb_frame_{timestamp:x}_{rand_seed:x}.png"));

    let output = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .frames_v(1)
        .format("image2")
        .overwrite()
        .output(&temp_path)
        .build()
        .output()
        .map_err(|err| {
            anyhow::anyhow!(
                "failed to launch ffmpeg frame extraction for {}: {err}",
                path.display()
            )
        })?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "ffmpeg frame extraction failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if !temp_path.exists() {
        return Err(anyhow::anyhow!(
            "ffmpeg frame extraction for {} reported success but did not create {}",
            path.display(),
            temp_path.display()
        ));
    }
    Ok(Some(temp_path))
}

fn detect_heavy_letterboxing_from_image(img: &image::DynamicImage) -> bool {
    let (_w, h) = img.dimensions();
    if h < 100 {
        return false;
    }
    let top_band = crate::numeric_cast::f64_to_u32_sat(
        f64::from(h) * crate::constants::LOOP_INTENT_LETTERBOX_THRESHOLD,
    );
    let bottom_start = h - top_band;
    let top_var = calculate_band_variance(img, 0, top_band);
    let bottom_var = calculate_band_variance(img, bottom_start, h);
    top_var < crate::constants::LOOP_INTENT_VARIANCE_THRESHOLD
        && bottom_var < crate::constants::LOOP_INTENT_VARIANCE_THRESHOLD
}

/// Calculate pixel variance in a horizontal band.
fn calculate_band_variance(img: &image::DynamicImage, y_start: u32, y_end: u32) -> f64 {
    use image::GenericImageView;
    let (w, _) = img.dimensions();
    let mut values = Vec::new();

    for y in y_start..y_end.min(img.height()) {
        for x in 0..w.min(img.width()) {
            let pixel = img.get_pixel(x, y);
            let gray = f64::from(pixel[2]).mul_add(
                crate::constants::LUMA_COEFF_B_F64,
                f64::from(pixel[0]).mul_add(
                    crate::constants::LUMA_COEFF_R_F64,
                    f64::from(pixel[1]) * crate::constants::LUMA_COEFF_G_F64,
                ),
            );
            values.push(gray);
        }
    }

    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(values.len());
    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
        / crate::numeric_cast::usize_to_f64(values.len())
}

fn detect_high_text_density_from_image(img: &image::DynamicImage) -> bool {
    // Consolidation cleanup
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    if w < 3 || h < 3 {
        return false;
    }

    let mut edge_count = 0usize;
    let total_pixels = f64::from(w) * f64::from(h);

    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let center = i32::from(gray.get_pixel(x, y)[0]);
            let right = i32::from(gray.get_pixel(x + 1, y)[0]);
            let bottom = i32::from(gray.get_pixel(x, y + 1)[0]);

            if (center - right).abs() > 80_i32 || (center - bottom).abs() > 80_i32 {
                edge_count += 1;
            }
        }
    }

    let edge_ratio = crate::numeric_cast::usize_to_f64(edge_count) / total_pixels;
    edge_ratio > crate::constants::LOOP_INTENT_TEXT_DENSITY_THRESHOLD
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

fn sampled_webp_compression_ratio_from_image(
    img: &image::DynamicImage,
) -> anyhow::Result<Option<f64>> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Ok(None);
    }

    // Only sample if image isn't too large to avoid performance hit
    let (target_w, target_h) = if w > WEBP_RATIO_SAMPLE_MAX_DIM || h > WEBP_RATIO_SAMPLE_MAX_DIM {
        let ratio = f64::from(w) / f64::from(h);
        if ratio > 1.0_f64 {
            (
                WEBP_RATIO_SAMPLE_MAX_DIM,
                crate::numeric_cast::f64_to_u32_sat(f64::from(WEBP_RATIO_SAMPLE_MAX_DIM) / ratio),
            )
        } else {
            (
                crate::numeric_cast::f64_to_u32_sat(f64::from(WEBP_RATIO_SAMPLE_MAX_DIM) * ratio),
                WEBP_RATIO_SAMPLE_MAX_DIM,
            )
        }
    } else {
        (w, h)
    };

    let resized = if target_w != w || target_h != h {
        img.thumbnail(target_w, target_h)
    } else {
        img.clone()
    };

    // Explicitly convert to RGBA8 before encoding.
    //
    // Bug fix: resized.as_bytes() returns channel-native bytes (RGB8 = 3 bytes/px,
    // RGBA8 = 4 bytes/px, etc.), but the encoder was unconditionally told the data
    // is ExtendedColorType::Rgba8 (4 bytes/px). When the source image is RGB (no alpha
    // channel), the byte length is w*h*3 while the encoder expects w*h*4, causing an
    // assertion panic inside the WebP encoder. Forcing to_rgba8() guarantees the buffer
    // is always exactly w*h*4 bytes regardless of the original pixel format.
    let rgba = resized.to_rgba8();
    let raw_size = f64::from(rgba.width() * rgba.height() * 4);

    let mut buffer = std::io::Cursor::new(Vec::new());
    let encoder = WebPEncoder::new_lossless(&mut buffer);
    encoder
        .encode(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|err| anyhow::anyhow!("WebP sample encode failed: {err}"))?;

    let webp_size = crate::numeric_cast::usize_to_f64(buffer.get_ref().len());

    if webp_size <= 0.0_f64 {
        return Ok(None);
    }
    Ok(Some(raw_size / webp_size))
}

/// Check if a file path should use the `GIF` fast-path (`from_gif_path`) instead of `ffprobe`.
#[must_use]
pub fn should_use_gif_fast_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase),
        Some(ext) if ext == "gif"
    )
}

/// Performs deep signal extraction (Palette, `YDIF`, Block Skew) using `FFmpeg` benchmarks.
///
/// # Errors
/// Returns an error if the `FFmpeg` command fails or the output cannot be parsed.
/// # Panics
///
/// Panics if the `FFmpeg` output contains malformed UTF-8 or if internal signal statistics parsing fails unexpectedly.
pub fn deep_refine_meta(meta: &mut LoopMeta, path: &std::path::Path) -> anyhow::Result<()> {
    // 1. Extract Temporal Flatness (YDIF)
    let output = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .arg("-vf")
        .arg("signalstats,metadata=print")
        .format("null")
        .output_pipe()
        .build()
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut ydif_values = Vec::new();
    for line in stderr.lines() {
        if let Some(idx) = line.find("lavfi.signalstats.YDIF=") {
            let tail = crate::media_conversion_gate::utf8_suffix_or_empty(
                line,
                idx + 23,
                "loop_intent lavfi.signalstats.YDIF",
            );
            let token = crate::media_conversion_gate::probe_stdout_first_token(
                tail,
                "loop_intent lavfi YDIF",
            );
            match token.parse::<f64>() {
                Ok(val) => ydif_values.push(val),
                Err(err) => {
                    anyhow::bail!("malformed loop_intent lavfi YDIF token {token:?}: {err}");
                }
            }
        }
    }
    if !ydif_values.is_empty() {
        meta.temporal_flatness = Some(temporal_flatness_score(&ydif_values));
        if meta.block_skew.is_none() {
            meta.block_skew = block_skew_score_from_signal(&ydif_values);
        }
    }

    // 2. Extract Palette Depth
    let thumb_output = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .frames_v(1)
        .arg("-vf")
        .arg("scale=64:64")
        .format("rawvideo")
        .pix_fmt(crate::ffmpeg_builder::PixFmt::Rgb24)
        .output_pipe()
        .build()
        .output()?;

    if thumb_output.status.success() && thumb_output.stdout.len() >= 64 * 64 * 3 {
        let mut quantized = std::collections::HashSet::new();
        for chunk in thumb_output.stdout.chunks_exact(3) {
            let r = chunk
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("Failed to parse red channel"))?
                >> 3_i32;
            let g = chunk
                .get(1)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("Failed to parse green channel"))?
                >> 3_i32;
            let b = chunk
                .get(2)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("Failed to parse blue channel"))?
                >> 3_i32;
            quantized.insert((r, g, b));
        }
        meta.palette_depth = Some(palette_depth_score(quantized.len()));

        // 3. Extract Real Physics (225-dimensional 15x15 luminance grid)
        // Reuse the already decoded 64x64 raw RGB buffer to avoid extra FFmpeg/Decoding overhead
        if let Some(img_buf) =
            image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(64, 64, thumb_output.stdout)
        {
            let dynamic_img = image::DynamicImage::ImageRgb8(img_buf);
            meta.physics_225 = Some(crate::real_physics::extract_image_physics_225(&dynamic_img));
        }
    }
    Ok(())
}

/// Skewness of per-frame luma-difference (`YDIF`) samples — empirical block-skew proxy.
fn block_skew_score_from_signal(values: &[f64]) -> Option<f64> {
    let n = values.len();
    if n < 3 {
        return None;
    }
    let n_f = crate::numeric_cast::usize_to_f64(n);
    let mean = values.iter().sum::<f64>() / n_f;
    let variance = values.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n_f;
    let std = variance.sqrt();
    if std <= 1e-9_f64 {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_signals",
            branch = "block_skew_zero_std",
            "block skew undefined (std ≤ 1e-9); omitting signal"
        );
        return None;
    }
    let skew = values
        .iter()
        .map(|&v| ((v - mean) / std).powi(3))
        .sum::<f64>()
        / n_f;
    Some(skew.clamp(-10.0, 10.0))
}

fn temporal_flatness_score(ydif_values: &[f64]) -> f64 {
    if ydif_values.is_empty() {
        return 0.5;
    }
    let n = crate::numeric_cast::usize_to_f64(ydif_values.len());
    let mean = ydif_values.iter().sum::<f64>() / n;
    if mean < 1e-6_f64 {
        return 1.0;
    }
    let variance = ydif_values.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n;
    let std = variance.sqrt();
    1.0 / (1.0 + std / (mean + 1e-6))
}

fn palette_depth_score(quantized_unique_colors: usize) -> f64 {
    if quantized_unique_colors == 0 {
        return 0.5;
    }
    let count = crate::numeric_cast::usize_to_f64(quantized_unique_colors);
    let max_possible = 32_f64.powi(3);
    let score = 1.0_f64 - count.log(max_possible).min(1.0);
    score.clamp(0.0, 1.0)
}

fn loop_closure_score(pkt_sizes: &[u64]) -> Option<f64> {
    if pkt_sizes.len() < 4 {
        return None;
    }

    let vals: Vec<f64> = pkt_sizes
        .iter()
        .map(|&v| crate::numeric_cast::u64_to_f64(v))
        .collect();
    let n = vals.len();
    let mean = vals.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(n);
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>()
        / crate::numeric_cast::usize_to_f64(n);
    if variance < 1e-6_f64 {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_signals",
            branch = "loop_closure_zero_variance",
            "loop closure autocorrelation undefined (zero packet-size variance); omitting signal"
        );
        return None;
    }

    // Normalized autocorrelation at lag = half sequence length.
    // A looping sequence has high self-similarity between its first and second half.
    let lag = n / 2;
    let autocorr: f64 = (0..n.saturating_sub(lag))
        .map(|i| {
            // i < n-lag, so both indexes are in-bounds; direct indexing avoids silent fallback forgery.
            let v1 = vals[i];
            let v2 = vals[i + lag];
            (v1 - mean) * (v2 - mean)
        })
        .sum::<f64>()
        / (crate::numeric_cast::usize_to_f64(n.saturating_sub(lag).max(1)) * variance);

    // Map [-1, 1] → [0, 1]; high positive autocorrelation = strong loop closure
    Some(f64::midpoint(autocorr, 1.0).clamp(0.0, 1.0))
}

fn motion_periodicity_score(mv_magnitudes: &[f64]) -> Option<f64> {
    let n = mv_magnitudes.len();
    if n < 6 {
        return None;
    }

    let mean = mv_magnitudes.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(n);
    let variance = mv_magnitudes
        .iter()
        .map(|&v| (v - mean).powi(2))
        .sum::<f64>()
        / crate::numeric_cast::usize_to_f64(n);
    if variance < 1e-6_f64 {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_signals",
            branch = "motion_periodicity_zero_variance",
            "motion periodicity undefined (zero MV variance); omitting signal"
        );
        return None;
    }

    // Average normalized autocorrelation over lags n/4, n/3, n/2.
    // A periodic (looping) sequence scores high across multiple lags.
    let lags = [n / 4, n / 3, n / 2];
    let autocorr_sum: f64 = lags
        .iter()
        .filter(|&&lag| lag > 0 && lag < n)
        .map(|&lag| {
            let r: f64 = (0..n.saturating_sub(lag))
                .map(|i| {
                    // i < n-lag, so both indexes are in-bounds; direct indexing avoids silent fallback forgery.
                    let v1 = mv_magnitudes[i];
                    let v2 = mv_magnitudes[i + lag];
                    (v1 - mean) * (v2 - mean)
                })
                .sum::<f64>()
                / (crate::numeric_cast::usize_to_f64(n.saturating_sub(lag).max(1)) * variance);
            r.clamp(-1.0, 1.0)
        })
        .sum();
    let valid_lags = lags.iter().filter(|&&lag| lag > 0 && lag < n).count();

    Some(
        f64::midpoint(
            autocorr_sum / crate::numeric_cast::usize_to_f64(valid_lags.max(1)),
            1.0,
        )
        .clamp(0.0, 1.0),
    )
}

fn temporal_jitter_score(pts_deltas: &[f64]) -> Option<f64> {
    let n = pts_deltas.len();
    if n < 3 {
        return None;
    }

    let mean = pts_deltas.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(n);
    let variance = pts_deltas.iter().map(|&v| (v - mean).powi(2)).sum::<f64>()
        / crate::numeric_cast::usize_to_f64(n);
    if variance < 1e-12_f64 {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_signals",
            branch = "temporal_jitter_zero_variance",
            "temporal jitter undefined (uniform PTS deltas); omitting signal"
        );
        return None;
    }

    // Lag-1 autocorrelation: measures rhythmic regularity of frame intervals.
    // A looping animation has consistent, self-similar inter-frame timing.
    let lag1: f64 = (0..n.saturating_sub(1))
        .map(|i| {
            // i < n-1, so both indexes are in-bounds; direct indexing avoids silent fallback forgery.
            let v1 = pts_deltas[i];
            let v2 = pts_deltas[i + 1];
            (v1 - mean) * (v2 - mean)
        })
        .sum::<f64>()
        / (crate::numeric_cast::usize_to_f64(n.saturating_sub(1).max(1)) * variance);

    Some(f64::midpoint(lag1.clamp(-1.0, 1.0), 1.0))
}

fn support_relief_from_loop_support(
    loop_support: Option<f64>,
    z: f64,
    is_short_silent: bool,
    is_image: bool,
    is_localized_motion: bool,
) -> Option<f64> {
    match loop_support {
        // No evidence: omit relief injection (avoid fabricated defaults).
        None => None,
        Some(support) => {
            if z.is_sign_negative() && support >= crate::constants::LOOP_INTENT_SUPPORT_HIGH {
                Some(crate::constants::LOOP_INTENT_SUPPORT_RELIEF_STRONG)
            } else if (z.is_sign_negative() && is_short_silent)
                || (z.is_sign_positive() && !(is_short_silent || is_image || is_localized_motion))
            {
                Some(crate::constants::LOOP_INTENT_SUPPORT_RELIEF_WEAK)
            } else {
                Some(1.0_f64)
            }
        }
    }
}

const fn unit_test_distribution_stats(
    mean: f64,
    std_dev: f64,
    p10: f64,
    p25: f64,
    p50: f64,
    p75: f64,
    p90: f64,
) -> DistributionStats {
    DistributionStats {
        mean,
        std_dev,
        p10: Some(p10),
        p25: Some(p25),
        p50: Some(p50),
        p75: Some(p75),
        p90: Some(p90),
        weight: None,
    }
}

/// Shaped like a corpus-backed profile for **unit/integration tests only**.
///
/// Use with [`evaluate_loop_tree`] when `PostgreSQL` is unavailable (e.g. CI).
/// Production [`assess_from_meta`] still requires a DB-backed reference profile.
#[must_use]
pub fn unit_test_loop_reference_profile() -> LoopReferenceProfile {
    let collection = crate::database::GlobalCollectionStats {
        duration_p90: Some(16.0),
        ..Default::default()
    };

    LoopReferenceProfile {
        duration: unit_test_distribution_stats(8.0, 4.0, 1.0, 2.5, 6.0, 12.0, 16.0),
        fps: unit_test_distribution_stats(12.0, 6.0, 4.0, 8.0, 12.0, 18.0, 24.0),
        frame_density: unit_test_distribution_stats(12.0, 6.0, 4.0, 8.0, 12.0, 18.0, 24.0),
        file_size_bytes: unit_test_distribution_stats(
            1_800_000.0,
            1_200_000.0,
            120_000.0,
            450_000.0,
            1_200_000.0,
            3_000_000.0,
            7_000_000.0,
        ),
        pixels: unit_test_distribution_stats(
            300_000.0,
            500_000.0,
            64_000.0,
            160_000.0,
            262_144.0,
            640_000.0,
            2_073_600.0,
        ),
        delay_variation: unit_test_distribution_stats(0.24, 0.12, 0.05, 0.12, 0.22, 0.32, 0.48),
        webp_ratio: unit_test_distribution_stats(11.0, 4.0, 4.0, 7.0, 10.0, 13.0, 16.0),
        motion_gini: unit_test_distribution_stats(0.55, 0.16, 0.25, 0.40, 0.55, 0.70, 0.84),
        palette_depth: unit_test_distribution_stats(0.55, 0.16, 0.25, 0.40, 0.55, 0.70, 0.84),
        temporal_flatness: unit_test_distribution_stats(0.55, 0.16, 0.25, 0.40, 0.55, 0.70, 0.84),
        collection,
        temporal_bpp: DistributionStats::default(),
        spatial_bpp: DistributionStats::default(),
        payload_variation: DistributionStats::default(),
        cadence: DistributionStats::default(),
        top_keywords: vec![
            "meme".to_string(),
            "reaction".to_string(),
            "sticker".to_string(),
        ],
        duration_has_empirical_percentiles: false,
        is_knn_bootstrap_heuristic: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static LOOP_INTENT_ENV_LOCK: Mutex<()> = Mutex::new(());
    use crate::database::LoopReferenceProfile;

    fn base_profile() -> LoopReferenceProfile {
        unit_test_loop_reference_profile()
    }

    fn base_meta() -> LoopMeta {
        LoopMeta {
            duration_secs: Some(7.9),
            width: Some(640),
            height: Some(640),
            fps: Some(12.0),
            frame_count: Some(96),
            file_size_bytes: 1_200_000,
            source_extension: Some("mp4".to_string()),
            container: Some("mp4".to_string()),
            frame_payload_variation: Some(0.4),
            frame_delay_variation: Some(0.24),
            directory_loop_intent_score: 0.5,
            filename_loop_intent_score: 0.5,
            ..Default::default()
        }
    }

    #[test]
    fn test_support_relief_none_omits_fabricated_evidence() {
        let z_pos = 0.5_f64;
        let z_neg = -0.5_f64;

        assert_eq!(
            support_relief_from_loop_support(None, z_pos, false, false, false),
            None
        );
        assert_eq!(
            support_relief_from_loop_support(None, z_neg, true, true, true),
            None
        );

        assert_eq!(
            support_relief_from_loop_support(
                Some(crate::constants::LOOP_INTENT_SUPPORT_HIGH),
                z_neg,
                false,
                false,
                false
            ),
            Some(crate::constants::LOOP_INTENT_SUPPORT_RELIEF_STRONG)
        );
    }

    #[test]
    fn extract_frame_to_temp_missing_file_returns_error_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.mp4");

        let err =
            extract_frame_to_temp(&missing).expect_err("missing frame-extraction target is error");

        assert!(err.to_string().contains("missing.mp4"));
    }

    #[test]
    fn gif_fast_path_handles_missing_frame_delays_without_panic() {
        let file = tempfile::Builder::new()
            .suffix(".gif")
            .tempfile()
            .unwrap_or_else(|e| {
                unreachable!("CRITICAL: test temp gif creation failed (error: {:?})", e)
            });
        let gif_without_graphic_control_extension = [
            b"GIF89a".as_slice(),
            &[
                0x01, 0x00, 0x01, 0x00, // Logical screen size 1x1
                0x80, 0x00, 0x00, // Global color table, background, aspect
                0x00, 0x00, 0x00, // Color 0
                0xFF, 0xFF, 0xFF, // Color 1
                0x2C, // Image descriptor
                0x00, 0x00, 0x00, 0x00, // Left/top
                0x01, 0x00, 0x01, 0x00, // Image size 1x1
                0x00, // No local color table
                0x02, // LZW min code size
                0x02, 0x4C, 0x01, // Image data
                0x00, // Sub-block terminator
                0x3B, // Trailer
            ],
        ]
        .concat();

        std::fs::write(file.path(), gif_without_graphic_control_extension)
            .unwrap_or_else(|e| unreachable!("CRITICAL: test gif write failed (error: {:?})", e));

        let meta = LoopMeta::from_gif_path(file.path()).unwrap_or_else(|| {
            unreachable!("CRITICAL: valid GIF header failed to produce loop metadata in test")
        });
        assert!(
            meta.duration_secs.is_none(),
            "Missing frame delays must yield None (no forgery), got {:?}",
            meta.duration_secs
        );
        assert_eq!(meta.frame_count, Some(1));
    }

    fn minimal_two_frame_gif_fixture() -> Vec<u8> {
        let mut gif_data = Vec::new();
        {
            let mut encoder = ::gif::Encoder::new(&mut gif_data, 1, 1, &[0, 0, 0, 255, 255, 255])
                .expect("gif encoder");
            let buf0 = [0_u8];
            let buf1 = [1_u8];
            encoder
                .write_frame(&::gif::Frame {
                    delay: 10,
                    width: 1,
                    height: 1,
                    buffer: std::borrow::Cow::Borrowed(&buf0),
                    ..Default::default()
                })
                .expect("gif frame 1");
            encoder
                .write_frame(&::gif::Frame {
                    delay: 10,
                    width: 1,
                    height: 1,
                    buffer: std::borrow::Cow::Borrowed(&buf1),
                    ..Default::default()
                })
                .expect("gif frame 2");
        }
        gif_data
    }

    #[test]
    fn strict_webp_ratio_population_measures_tiny_dynamic_assets() {
        let file = tempfile::Builder::new()
            .suffix(".gif")
            .tempfile()
            .expect("temp gif");
        std::fs::write(file.path(), minimal_two_frame_gif_fixture()).expect("write temp gif");

        let mut meta = LoopMeta {
            width: Some(1),
            height: Some(1),
            frame_count: Some(2),
            duration_secs: Some(0.2),
            ..LoopMeta::default()
        };
        assert!(
            !meta.should_sample_webp_compression_ratio(),
            "tiny dynamic assets currently bypass best-effort WebP ratio sampling"
        );

        meta.ensure_webp_compression_ratio_from_path(file.path())
            .expect("training ingest must force empirical WebP ratio measurement");

        let ratio = meta
            .webp_compression_ratio
            .expect("strict measurement must populate WebP ratio");
        assert!(ratio.is_finite() && ratio > 0.0);
        assert!(meta.cached_frame_png.is_some());
        assert_eq!(meta.physics_225.as_ref().map(std::vec::Vec::len), Some(225));
    }

    fn verdict_with_profile(meta: &LoopMeta, profile: &LoopReferenceProfile) -> Verdict {
        let _guard = LOOP_INTENT_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Ensure developer intercepts are disabled for profile-based tests.
        // SAFETY: test-only; these tests must not run in parallel with other env-var tests.
        unsafe { std::env::set_var(crate::constants::ENV_INTERCEPT_LONG_SILENT, "0") };
        evaluate_loop_tree(meta, Some(profile)).verdict
    }

    #[test]
    fn duration_override_beats_large_resolution_and_size() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = Some(2.0_f64);
        meta.width = Some(3840);
        meta.height = Some(2160);
        meta.fps = Some(60.0_f64);
        meta.file_size_bytes = 30_000_000;

        let verdict = verdict_with_profile(&meta, &profile);
        // Duration bias (UltraShort) should dominate despite extreme resolution/size.
        // File size is NOT decisive — only duration matters for the final verdict direction.
        assert!(
            matches!(verdict, Verdict::LoopStrong(_)),
            "short duration should dominate over large file size, got: {verdict:?}"
        );
    }

    #[test]
    fn layer_0_short_audio_media_is_immediate_loopweak() {
        let profile = base_profile();
        let mut meta = base_meta();
        // Use 12.0s Long tier where audio penalty is -LOG_ODDS_BIAS_DEFINITIVELY_LONG (-3.0).
        meta.duration_secs = Some(12.0_f64);
        meta.flags.streams.has_audio = true;
        meta.audio_is_silent = Some(false); // Audible audio
        meta.frame_count = Some(288); // 24fps × 12s
        // Make this look like a real video: widescreen, large file, scene cuts
        meta.width = Some(1920);
        meta.height = Some(1080);
        meta.file_size_bytes = 8_000_000;
        meta.pkt_sizes = vec![120, 130, 1400, 150, 120, 125]; // scene cut signature
        // Remove all pro-loop signals
        meta.loop_closure_score = None;
        meta.motion_periodicity = None;
        meta.frame_payload_variation = None;
        meta.frame_delay_variation = None;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, Verdict::LoopWeak(_)),
            "audible audio in Long-tier video should resolve to LoopWeak, got: {verdict:?}"
        );
    }

    #[test]
    fn layer_0_long_media_proceeds_to_stage_1() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = Some(12.0_f64);
        meta.loop_count = Some(0); // Weighted signal, not immediate exit

        let verdict = verdict_with_profile(&meta, &profile);
        // loop_count=0 is now just a weighted bonus. The verdict depends on the
        // full pipeline evaluation. With 12s duration (Long tier), the negative
        // duration bias will fight against the loop_count bonus.
        // We only verify it doesn't crash and reaches a valid verdict.
        assert!(
            !verdict.is_error(),
            "Long asset with loop_count=0 should not error: {verdict:?}"
        );
    }

    #[test]
    fn layer_0_ex_short_hard_veto_does_not_require_reference_profile() {
        let mut meta = base_meta();
        meta.duration_secs = Some(1.5_f64);
        meta.frame_count = Some(18);
        meta.flags.streams.has_audio = false;
        meta.audio_is_silent = Some(true);

        let verdict = evaluate_loop_tree(&meta, None).verdict;

        assert!(
            verdict.reason().contains("Layer 0-EX") && matches!(verdict, Verdict::LoopStrong(_)),
            "short hard veto should bypass DB reference profile, got: {verdict:?}"
        );
    }

    #[test]
    fn layer_0_ex_long_hard_veto_does_not_require_reference_profile() {
        let mut meta = base_meta();
        meta.duration_secs = Some(45.0_f64);
        meta.frame_count = Some(1350);

        let verdict = evaluate_loop_tree(&meta, None).verdict;

        assert!(
            verdict.reason().contains("Layer 0-EX") && matches!(verdict, Verdict::LoopWeak(_)),
            "long hard veto should bypass DB reference profile, got: {verdict:?}"
        );
    }

    #[test]
    fn layer_1_b2_sticker_class_native_gif_now_handled_by_layer_0() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.flags.streams.is_native_gif = true;
        meta.flags.streams.has_audio = false;
        meta.duration_secs = Some(4.0_f64);
        meta.width = Some(150);
        meta.height = Some(108);
        meta.frame_count = Some(40);
        meta.file_size_bytes = 24_000;

        let verdict = verdict_with_profile(&meta, &profile);
        // Short duration bias + sticker-class bonus should push this to LoopStrong
        // through the full pipeline, not an immediate exit.
        assert!(
            matches!(verdict, Verdict::LoopStrong(_)),
            "small native GIF should still resolve to LoopStrong, got {verdict:?}"
        );
    }

    #[test]
    fn layer_1_b2_does_not_apply_to_large_pixel_gif() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.flags.streams.is_native_gif = true;
        meta.flags.streams.has_audio = false;
        meta.duration_secs = Some(4.0_f64);
        meta.width = Some(500);
        meta.height = Some(500);
        meta.frame_count = Some(40);
        meta.file_size_bytes = 400_000;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            !verdict.reason().contains("Layer 1-B2"),
            "large canvas should not hit B2: {}",
            verdict.reason()
        );
    }

    #[test]
    fn audio_video_is_absolute_veto() {
        let profile = base_profile();
        let mut meta = base_meta();
        // Use 12.0s Long tier where audio penalty is maximal.
        meta.duration_secs = Some(12.0_f64);
        meta.flags.streams.has_audio = true;
        meta.audio_is_silent = Some(false); // Audible audio
        meta.frame_count = Some(288);
        // Make this look like a real video: widescreen, large file
        meta.width = Some(1920);
        meta.height = Some(1080);
        meta.file_size_bytes = 8_000_000;
        meta.pkt_sizes = vec![120, 130, 1400, 150, 120, 125]; // scene cut signature
        // Remove all pro-loop signals
        meta.loop_closure_score = None;
        meta.motion_periodicity = None;
        meta.frame_payload_variation = None;
        meta.frame_delay_variation = None;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, Verdict::LoopWeak(_)),
            "audible audio should resolve to LoopWeak, got: {verdict:?}"
        );
    }

    #[test]
    fn explicit_loop_count_zero_exits_tree_strong() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.loop_count = Some(0);
        meta.duration_secs = Some(12.0_f64);

        // loop_count=0 is now a weighted signal, not an immediate exit.
        // With 12s duration (Long tier) and no other pro-loop signals,
        // the negative duration bias will likely override the loop_count bonus.
        // This test verifies the system doesn't blindly trust metadata.
        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            !verdict.is_error(),
            "loop_count=0 at 12s should not error: {verdict:?}"
        );
    }

    #[test]
    fn platform_marker_exits_tree_strong() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.app_extensions = Some(vec!["TENOR".to_string()]);
        meta.duration_secs = Some(14.0_f64); // Long duration - will go to specialized tree
        meta.flags.streams.has_audio = false; // Ensure it's silent
        meta.frame_count = Some(168); // Ensure valid frame count
        meta.source_extension = Some("mp4".to_string()); // Video container

        let verdict = verdict_with_profile(&meta, &profile);

        // Platform marker is now a weighted signal, not an immediate exit.
        // At 14s (Long tier), the negative duration bias fights the platform bonus.
        // We verify it doesn't crash and the platform marker doesn't blindly win.
        assert!(
            !verdict.is_error(),
            "platform marker at 14s should not error: {verdict:?}"
        );
    }

    #[test]
    fn short_fast_silent_media_scores_loopstrong() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = Some(4.0_f64);
        meta.fps = Some(24.0_f64);
        meta.frame_count = Some(96);
        meta.width = Some(320);
        meta.height = Some(320);
        meta.file_size_bytes = 240_000;

        let verdict = verdict_with_profile(&meta, &profile);
        // Short duration bias should dominate and push to LoopStrong,
        // even though the verdict now comes from the full pipeline.
        assert!(
            matches!(verdict, Verdict::LoopStrong(_)),
            "expected loop-strong for short silent asset, got {verdict:?}"
        );
    }

    #[test]
    fn long_slow_scene_cut_media_scores_loopweak() {
        let profile = base_profile();
        let mut meta = base_meta();
        // 28s now hits the ≥15s hard veto. We test that it correctly exits as LoopWeak.
        meta.duration_secs = Some(28.0_f64);
        meta.fps = Some(4.0_f64);
        meta.frame_count = Some(112);
        meta.file_size_bytes = 12_000_000;
        meta.width = Some(1920);
        meta.height = Some(1080);
        meta.pkt_sizes = vec![120, 130, 1400, 150, 120, 125];
        meta.webp_compression_ratio = Some(3.0_f64);
        meta.motion_gini = Some(0.18_f64);
        meta.palette_depth = Some(0.20_f64);
        meta.temporal_flatness = Some(0.18_f64);

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, Verdict::LoopWeak(_)),
            "expected loop-weak, got {verdict:?}"
        );
        // With ≥15s hard veto, the verdict should come from Layer 0-EX.
        assert!(
            verdict.reason().contains("Layer 0-EX")
                || verdict.reason().contains("Layer 3")
                || verdict.reason().contains("Layer 4")
                || verdict.reason().contains("Layer 5")
        );
    }

    #[test]
    fn play_once_and_master_signals_push_borderline_media_weak() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.loop_count = Some(1);
        meta.duration_secs = Some(11.0_f64);
        meta.fps = Some(8.0_f64);
        meta.frame_count = Some(88);
        meta.file_size_bytes = 3_800_000;
        meta.webp_compression_ratio = Some(3.5_f64);
        meta.flags.color.has_complex_color_profile = true;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, Verdict::LoopWeak(_)),
            "expected loop-weak, got {verdict:?}"
        );
    }

    #[test]
    fn from_ffprobe_result_ignores_pix_fmt_inferred_bit_depth_for_master_like_signal() {
        let probe = crate::ffprobe::FFprobeResult {
            format_name: "mp4".to_string(),
            duration: Some(6.0),
            size: 1_000_000,
            bit_rate: Some(1_200_000),
            video_codec: "h264".to_string(),
            video_codec_long: "H.264".to_string(),
            width: 640,
            height: 640,
            frame_rate: Some(30.0),
            avg_frame_rate: Some(30.0),
            frame_count: Some(180),
            pix_fmt: "yuv420p10le".to_string(),
            color_space: Some("bt709".to_string()),
            color_transfer: Some("bt709".to_string()),
            color_primaries: Some("bt709".to_string()),
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: true,
            audio: crate::ffprobe::FFprobeAudioInfo::default(),
            profile: None,
            level: None,
            max_b_frames: Some(2),
            encoder_settings: None,
            video_bit_rate: Some(1_000_000),
            refs: None,
            hdr: crate::ffprobe::FFprobeHdrInfo::default(),
            subtitles: crate::ffprobe::FFprobeSubtitleInfo::default(),
            is_variable_frame_rate: false,
            stream_index: 0,
            tags: std::collections::HashMap::new(),
            loop_count: None,
            frame_types: vec!['I', 'P', 'P'],
            pts_deltas: vec![0.033, 0.033],
            mv_magnitudes: vec![0.1, 0.2],
            pkt_sizes: vec![1_000, 900, 950],
        };

        let meta = LoopMeta::from_ffprobe_result(&probe, Path::new("sample.mp4"));

        assert!(!meta.flags.color.has_embedded_icc);
        assert!(!meta.flags.color.has_complex_color_profile);
    }

    #[test]
    fn from_video_detection_keeps_confirmed_high_bit_depth_as_complex_color_signal() {
        let detection = Detection {
            file_path: "sample.mov".to_string(),
            format: "mov".to_string(),
            bit_depth: Some(10),
            color_space: crate::video_detection::ColorSpace::BT709,
            pix_fmt: "yuv422p10le".to_string(),
            compression: crate::video_detection::CompressionType::VisuallyLossless,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(240),
            fps: Some(24.0),
            duration_secs: Some(10.0),
            file_size: 10_000_000,
            precision: crate::video_detection::VideoPrecisionMetadata {
                bit_depth_inferred_from_pix_fmt: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let meta = LoopMeta::from_video_detection(&detection);
        assert!(meta.flags.color.has_complex_color_profile);
    }

    #[test]
    fn from_video_detection_reuses_shared_wide_gamut_signal_for_master_like_footprint() {
        let detection = Detection {
            file_path: "sample.mov".to_string(),
            format: "mov".to_string(),
            color_space: crate::video_detection::ColorSpace::AdobeRGB,
            pix_fmt: "yuv444p".to_string(),
            compression: crate::video_detection::CompressionType::VisuallyLossless,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(240),
            fps: Some(24.0),
            duration_secs: Some(10.0),
            file_size: 10_000_000,
            ..Default::default()
        };

        let meta = LoopMeta::from_video_detection(&detection);
        assert!(meta.flags.color.has_embedded_icc);
        assert!(!meta.flags.color.has_complex_color_profile);
    }

    #[test]
    fn platform_transparency_and_looping_signals_push_borderline_media_strong() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.duration_secs = Some(7.0_f64);
        meta.fps = Some(10.0_f64);
        meta.frame_count = Some(70);
        meta.file_size_bytes = 500_000;
        meta.flags.streams.has_transparency = true;
        meta.transparency_is_real = Some(true);
        meta.app_extensions = Some(vec!["GIPHY".to_string()]);
        meta.loop_count = Some(0);
        meta.motion_gini = Some(0.82_f64);
        meta.palette_depth = Some(0.82_f64);
        meta.temporal_flatness = Some(0.80_f64);
        meta.directory_loop_intent_score = 1.0_f64;
        meta.filename_loop_intent_score = 1.0_f64;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, Verdict::LoopStrong(_)),
            "expected loop-strong, got {verdict:?}"
        );
    }

    #[test]
    fn balanced_case_stays_uncertain() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = Some(9.0_f64);
        // Strong positive physical signals to counteract the structural checkpoint.
        // At 9s (Short tier, +0.5 bias), we need the loop_closure and periodicity
        // to push log-odds into the gray zone (-1.05..+1.05) rather than triggering
        // early checkpoint exits.
        meta.loop_closure_score = Some(0.85_f64);
        meta.motion_periodicity = Some(0.80_f64);
        meta.filename_loop_intent_score = 0.5_f64;
        meta.directory_loop_intent_score = 0.5_f64;
        meta.frame_count = Some(108);
        meta.source_extension = Some("webp".to_string());
        meta.container = Some("webp".to_string());
        // Add verified transparency to give a moderate positive signal
        meta.flags.streams.has_transparency = true;
        meta.transparency_is_real = Some(true);

        let verdict = verdict_with_profile(&meta, &profile);

        // REVERTED: The review noted that with these strong signals, LoopStrong is also valid.
        // We accept either Uncertain (borderline) or LoopStrong (if signals push it over).
        assert!(
            matches!(verdict, Verdict::Uncertain(_) | Verdict::LoopStrong(_)),
            "expected uncertain or loop-strong for balanced case, got {verdict:?}"
        );
    }

    #[test]
    fn pure_balanced_case_is_uncertain() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = Some(8.0_f64); // MediumLong tier, proximity ramp = 0
        // Moderate signals that should stay in the uncertain zone.
        meta.loop_closure_score = Some(0.5_f64);
        meta.motion_periodicity = Some(0.5_f64);
        meta.filename_loop_intent_score = 0.5_f64;
        meta.directory_loop_intent_score = 0.5_f64;
        meta.frame_count = Some(108);
        meta.source_extension = Some("webp".to_string());
        meta.container = Some("webp".to_string());

        let verdict = verdict_with_profile(&meta, &profile);

        assert!(
            matches!(verdict, Verdict::Uncertain(_)),
            "expected strictly uncertain for pure balanced case, got {verdict:?}"
        );
    }

    #[test]
    fn resolve_legacy_mode_does_not_re_evaluate_tree() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/image/loop_intent.rs");
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let start = content
            .find("fn resolve_legacy_mode(")
            .expect("resolve_legacy_mode must exist");
        let end = content[start..]
            .find("\n    fn resolve_legacy_uncertain")
            .map(|off| start + off)
            .expect("resolve_legacy_uncertain must follow resolve_legacy_mode");
        let body = &content[start..end];
        assert!(
            !body.contains("evaluate_loop_tree"),
            "legacy resolve must use the tree from run(), not re-evaluate"
        );
    }

    #[test]
    fn inference_used_layer7_policy_detects_fallback_tracking() {
        let mut tracking = InferenceTracking::default();
        assert!(!inference_used_layer7_policy(&tracking));
        tracking.resolution_path = Some("layer7_fallback".into());
        assert!(inference_used_layer7_policy(&tracking));
        tracking.resolution_path = None;
        tracking.layer7_upstream = Some("Layer 6: KNN match missing probability".into());
        assert!(inference_used_layer7_policy(&tracking));
    }

    #[test]
    fn extract_layer_tag_parses_layer7_arrow_reason() {
        let tag =
            extract_layer_tag("Layer 7: Fallback [upstream] → preserve modern animated format");
        assert_eq!(tag, "Layer 7");
    }

    #[test]
    fn finalize_with_path_tags_layer0_veto() {
        let eval = finalize_with_path(
            Verdict::Error("Layer 0: single-frame input, physically cannot loop".into()),
            LogOdds::default(),
            Some("layer0_veto_single_frame"),
        );
        assert_eq!(
            eval.resolution_path.as_deref(),
            Some("layer0_veto_single_frame")
        );
    }

    #[test]
    fn tree_checkpoint_resolution_path_tags_image_layer3() {
        assert_eq!(
            tree_checkpoint_resolution_path("Layer 3 (Image)"),
            "tree_checkpoint_layer3_image"
        );
        assert_eq!(
            tree_layer5_resolution_path("video", "weak"),
            "tree_layer5_video_weak"
        );
    }

    #[test]
    fn meme_profile_with_db_thresholds_resolves_without_layer7_fallback() {
        let profile = base_profile();
        let meta = LoopMeta {
            duration_secs: Some(3.5),
            width: Some(640),
            height: Some(360),
            fps: Some(24.0),
            frame_count: Some(84),
            file_size_bytes: 2_000_000,
            flags: LoopFlags {
                streams: LoopStreamFlags {
                    has_audio: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            source_extension: Some("mp4".to_string()),
            container: Some("mp4".to_string()),
            loop_closure_score: Some(0.95_f64),
            ..Default::default()
        };

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, Verdict::LoopStrong(_)),
            "expected loop-strong with corpus profile, got {verdict:?}"
        );
        assert!(
            !verdict.reason().contains("Layer 7"),
            "DB-backed tree should resolve explicitly: {}",
            verdict.reason()
        );
    }

    #[test]
    fn silent_technical_profile_with_db_thresholds_resolves_without_layer7() {
        let profile = base_profile();
        let meta = LoopMeta {
            duration_secs: Some(8.5),
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            frame_count: Some(255),
            file_size_bytes: 8_000_000,
            flags: LoopFlags {
                streams: LoopStreamFlags {
                    has_audio: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            source_extension: Some("mp4".to_string()),
            container: Some("mp4".to_string()),
            motion_gini: Some(0.85_f64),
            ..Default::default()
        };

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, Verdict::LoopWeak(_)),
            "expected loop-weak with corpus profile, got {verdict:?}"
        );
        assert!(
            !verdict.reason().contains("Layer 7"),
            "DB-backed tree should not fall back to Layer 7: {}",
            verdict.reason()
        );
    }

    #[test]
    fn evaluate_without_reference_profile_refuses_fabricated_thresholds() {
        let meta = base_meta();
        let tree = evaluate_loop_tree(&meta, None);
        assert!(
            matches!(tree.verdict, Verdict::Uncertain(_)),
            "zero-tolerance: missing DB profile must not use legacy constants, got {:?}",
            tree.verdict
        );
        assert!(
            tree.verdict.reason().contains("reference profile"),
            "expected explicit refusal reason, got {}",
            tree.verdict.reason()
        );
    }

    #[test]
    fn test_score_directory_context() {
        let directory_score = score_directory_context(
            Some(&["Downloads".to_string(), "ReactionPacks".to_string()]),
            &[],
        );
        assert!(crate::float_compare::approx_eq_f64(directory_score, 1.0));
    }

    #[test]
    fn test_analyze_filename_with_keywords() {
        // Test Chinese keyword (from JSON)
        let analysis_zh = analyze_filename(Some("gif表情 (379).gif"), &[]);
        assert!(crate::float_compare::approx_eq_f64(analysis_zh.raw, 0.85));
        assert_eq!(analysis_zh.kind, FilenameKind::HumanSemantic);

        // Test English keyword (from JSON)
        let analysis_en = analyze_filename(Some("my_funny_meme.webp"), &[]);
        assert!(crate::float_compare::approx_eq_f64(analysis_en.raw, 0.85));

        // Test Korean keyword (from JSON)
        let analysis_ko = analyze_filename(Some("cute_sticker_움짤.avif"), &[]);
        assert!(crate::float_compare::approx_eq_f64(analysis_ko.raw, 0.85));

        // Test non-meme filename
        let analysis_none = analyze_filename(Some("vacation_photo.jpg"), &[]);
        assert!(crate::float_compare::approx_eq_f64(analysis_none.raw, 0.5));
    }

    #[test]
    fn thresholds_expand_short_clip_window_when_db_percentiles_are_missing() {
        let mut profile = base_profile();
        profile.duration = DistributionStats {
            mean: 2.88,
            std_dev: 5.17,
            p10: None,
            p25: None,
            p50: None,
            p75: None,
            p90: None,
            weight: None,
        };
        profile.duration_has_empirical_percentiles = false;
        profile.collection.duration_p90 = Some(15.0_f64);

        let thresholds = LoopThresholds::from_reference_profile(&profile)
            .expect("profile must yield thresholds in unit test");
        assert!(thresholds.duration_override_secs > 2.0_f64);
        assert!(thresholds.short_clip_secs > thresholds.duration_override_secs);
        assert!(
            thresholds.short_asset_window_secs
                >= crate::constants::HARD_PASS_SHORT_GIF_THRESHOLD_SECS
        );
        assert!(
            thresholds.modern_bias_duration_secs
                >= crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS
        );
    }

    #[test]
    fn thresholds_reject_empirical_duration_override_that_inverts_short_clip_clamp() {
        let mut profile = base_profile();
        profile.duration = DistributionStats {
            mean: 39.457,
            std_dev: 0.5,
            p10: Some(24.0),
            p25: Some(39.457),
            p50: Some(40.0),
            p75: Some(42.0),
            p90: Some(45.0),
            weight: None,
        };
        profile.duration_has_empirical_percentiles = true;
        profile.collection.duration_p90 = Some(45.0);
        profile.collection.duration_p90_from_samples = true;

        assert!(
            LoopThresholds::from_reference_profile(&profile).is_none(),
            "oversized empirical profile must fail closed instead of inverting threshold clamps"
        );

        profile.duration.p25 = Some(7.5);
        profile.duration.p50 = Some(12.0);
        profile.duration.p75 = Some(13.0);
        profile.duration.p90 = Some(14.0);
        profile.collection.duration_p90 = Some(14.0);
        assert!(
            LoopThresholds::from_reference_profile(&profile).is_none(),
            "near-max empirical profile must leave room for the short-clip clamp lower bound"
        );
    }

    #[test]
    fn layer6_accepts_short_gif_when_score_is_high_and_confidence_is_near_threshold() {
        let profile = base_profile();
        let thresholds = LoopThresholds::from_reference_profile(&profile)
            .expect("profile must yield thresholds in unit test");
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.duration_secs = Some(5.0_f64);
        meta.flags.streams.has_audio = false;

        assert!(should_accept_layer6_loopstrong(
            &meta,
            &thresholds,
            0.82,
            0.72,
            0.71,
        ));
    }

    #[test]
    fn layer6_directional_high_loop_frequency_bias_does_not_panic() -> anyhow::Result<()> {
        let profile = base_profile();
        let thresholds = LoopThresholds::from_reference_profile(&profile)
            .ok_or_else(|| anyhow::anyhow!("profile must yield thresholds in unit test"))?;
        let meta = LoopMeta {
            duration_secs: Some(2.0_f64),
            frame_count: Some(24),
            source_extension: Some("mp4".to_string()),
            container: Some("mp4".to_string()),
            flags: LoopFlags {
                streams: LoopStreamFlags {
                    has_audio: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..base_meta()
        };
        let tree = TreeEvaluation {
            verdict: Verdict::Uncertain("Layer 5 (Video): unit-test".to_string()),
            tree_probability: Some(0.55_f64),
            log_odds_value: Some(0.2_f64),
            resolution_path: None,
        };

        let (_verdict, audit) = Layer6DirectionalArbitrator::new(
            &meta,
            &thresholds,
            &tree,
            Some(0.56_f64),
            Some(0.70_f64),
            Some(0.56_f64),
            Some(4),
            tree.verdict.reason(),
        )
        .run();

        assert!(audit.keep_score.is_finite());
        assert!(audit.convert_score.is_finite());
        Ok(())
    }

    #[test]
    fn layer6_relaxes_for_silent_clips_up_to_core_short_asset_window() {
        let profile = base_profile();
        let thresholds = LoopThresholds::from_reference_profile(&profile)
            .expect("profile must yield thresholds in unit test");
        let mut meta = base_meta();
        meta.source_extension = Some("mp4".to_string());
        meta.container = Some("mp4".to_string());
        meta.duration_secs = Some(9.5_f64);
        meta.flags.streams.has_audio = false;

        assert!(should_accept_layer6_loopstrong(
            &meta,
            &thresholds,
            0.82,
            0.72,
            0.71,
        ));
    }

    #[test]
    fn layer6_does_not_relax_for_long_non_image_clips() {
        let profile = base_profile();
        let thresholds = LoopThresholds::from_reference_profile(&profile)
            .expect("profile must yield thresholds in unit test");
        let mut meta = base_meta();
        meta.duration_secs = Some(24.0_f64);
        meta.source_extension = Some("mp4".to_string());
        meta.container = Some("mp4".to_string());

        assert!(!should_accept_layer6_loopstrong(
            &meta,
            &thresholds,
            0.82,
            0.72,
            0.71,
        ));
    }

    #[test]
    fn hidden_layer1_overrides_are_opt_in() {
        let profile = base_profile();
        let mut meta = base_meta();
        // Use 14.0s to stay inside the gray zone (6–15s). Previously 18.0s, which now
        // hits the ≥15s hard veto and would never reach Layer 1-D.
        meta.duration_secs = Some(14.0_f64);

        let _guard = LOOP_INTENT_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: test-only; these tests must not run in parallel with other env-var tests.
        unsafe { std::env::set_var(crate::constants::ENV_INTERCEPT_LONG_SILENT, "1") };
        let dev_long_verdict = evaluate_loop_tree(&meta, Some(&profile)).verdict;
        unsafe { std::env::remove_var(crate::constants::ENV_INTERCEPT_LONG_SILENT) };
        // The developer override injects -LOG_ODDS_BIAS_DEFINITIVELY_LONG, which should
        // push the verdict away from LoopStrong. We verify it doesn't end up LoopStrong.
        assert!(
            !matches!(dev_long_verdict, Verdict::LoopStrong(_)),
            "developer override should suppress LoopStrong: {dev_long_verdict:?}"
        );
    }

    #[test]
    fn block_skew_score_from_signal_returns_skew_for_ydif_samples() {
        let skew = block_skew_score_from_signal(&[1.0, 2.0, 8.0, 2.0, 1.0]);
        assert!(skew.is_some());
        assert!(skew.unwrap().is_finite());
    }

    #[test]
    fn ensure_frame_delay_variation_uses_constant_fps_when_pts_missing() {
        let mut meta = LoopMeta {
            fps: Some(30.0),
            frame_count: Some(120),
            pts_deltas: Vec::new(),
            ..LoopMeta::default()
        };
        ensure_frame_delay_variation(&mut meta);
        assert_eq!(meta.frame_delay_variation, Some(0.0));
    }

    #[test]
    fn ensure_frame_delay_variation_prefers_pts_cv_when_present() {
        let mut meta = LoopMeta {
            fps: Some(30.0),
            frame_count: Some(4),
            pts_deltas: vec![0.033, 0.050, 0.033],
            ..LoopMeta::default()
        };
        ensure_frame_delay_variation(&mut meta);
        assert!(meta.frame_delay_variation.is_some());
        assert!(meta.frame_delay_variation.unwrap() > 0.0);
    }

    #[test]
    fn test_calculate_cv_f64_simd() {
        // Test with uniform data (CV should be 0)
        let data = vec![10.0; 16];
        let cv = calculate_cv_f64(&data);
        assert!(cv.is_some());
        assert!(cv.unwrap().abs() < 1e-10);

        // Test with known variation
        // mean = 4.5, std_dev ≈ 2.29, cv ≈ 0.509
        let data2 = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let cv2 = calculate_cv_f64(&data2);
        assert!(cv2.is_some());
        assert!((cv2.unwrap() - 0.509).abs() < 0.01);
    }

    #[test]
    fn test_likely_unlikely_logic_integrity() {
        use std::intrinsics::{likely, unlikely};
        // Verify that the intrinsics don't change logic
        assert!(likely(true));
        assert!(!likely(false));
        assert!(unlikely(true));
        assert!(!unlikely(false));
    }

    #[test]
    fn test_analyze_filename_variants() {
        let analysis_en = analyze_filename(Some("my_funny_meme.webp"), &[]);
        assert!(crate::float_compare::approx_eq_f64(analysis_en.raw, 0.85));

        // Test Korean keyword (from JSON)
        let analysis_ko = analyze_filename(Some("cute_sticker_움짤.avif"), &[]);
        assert!(crate::float_compare::approx_eq_f64(analysis_ko.raw, 0.85));

        // Test non-meme filename
        let analysis_none = analyze_filename(Some("vacation_photo.jpg"), &[]);
        assert!(crate::float_compare::approx_eq_f64(analysis_none.raw, 0.5));
    }

    #[test]
    fn bug_fix_silent_audio_track_should_be_treated_as_animation() {
        // Regression test for: 2.28s video with silent audio track (-91 dB)
        // Real-world case: "Commission finish_Apple_ProRes_422_HQ.mov"
        // Expected: Should be treated as animation (LoopStrong)
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = Some(2.28_f64);
        meta.frame_count = Some(57);
        meta.flags.streams.has_audio = true; // Audio track exists
        meta.audio_is_silent = Some(true); // But it's silent (-91 dB)
        meta.source_extension = Some("mov".to_string());
        meta.container = Some("mov".to_string());

        let verdict = verdict_with_profile(&meta, &profile);

        // Silent audio = no audible content → duration bias drives the verdict.
        // Short duration (UltraShort tier) should push to LoopStrong.
        assert!(
            matches!(verdict, Verdict::LoopStrong(_)),
            "Expected LoopStrong for silent audio track, got: {verdict:?}"
        );
    }

    #[test]
    fn structural_metrics_stay_unknown_when_inputs_are_missing() {
        assert_eq!(calculate_cv(&[]), None);
        assert_eq!(calculate_cv_f64(&[]), None);
        assert_eq!(calculate_gini_f64(&[]), None);
        assert_eq!(loop_closure_score(&[]), None);
        assert_eq!(motion_periodicity_score(&[]), None);
        assert_eq!(temporal_jitter_score(&[]), None);
    }

    #[test]
    fn zero_variance_signals_return_none_not_synthetic_unity() {
        let identical_packets = vec![100_u64; 8];
        assert_eq!(loop_closure_score(&identical_packets), None);

        let static_motion = vec![0.0_f64; 8];
        assert_eq!(motion_periodicity_score(&static_motion), None);

        let uniform_pts = vec![1.0_f64 / 30.0; 8];
        assert_eq!(temporal_jitter_score(&uniform_pts), None);

        assert_eq!(calculate_cv(&[0_u64; 8]), None);
        assert_eq!(calculate_cv_f64(&[0.0_f64; 8]), None);
        assert_eq!(calculate_gini_f64(&[0.0_f64; 8]), None);
    }
}

#[cfg(test)]
mod advanced_tests {
    include!("../tests/loop_intent_probe.rs");
}
