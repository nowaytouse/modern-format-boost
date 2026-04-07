//! Loop Intent Identification System
//!
//! A modern, explainable judgment tree for identifying media looping intent (memes, stickers, loops).
//! Implements the 7-layer hierarchical decision tree defined in docs/decision_tree.md.
//!
//! Architecture:
//! - Layer 1: Hard constraints + veto-gated hard passes
//! - Layer 2: Explicit declarations → direct exits
//! - Layer 3 & 4: Structural/content signals → WeightedScore accumulation with checkpoints
//! - Layer 5: Weak contextual corrections
//! - Layer 6: KNN + WeightedScore fusion
//! - Layer 7: Conservative fallback

use crate::constants::{
    DIRECTORY_CONTEXT_POSITIVE_LOG_ODDS, FILENAME_CONTEXT_POSITIVE_LOG_ODDS,
    LOCALIZED_MOTION_POSITIVE_LOG_ODDS, MODERN_MASTER_NEGATIVE_LOG_ODDS,
    PLAY_ONCE_NEGATIVE_LOG_ODDS,
};
use crate::database::LoopReferenceProfile;
use crate::file_copier::{SUPPORTED_IMAGE_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS};
use crate::progress_mode::emit_stderr;
use crate::video_detection::ColorSpace;
use crate::video_detection::VideoDetectionResult;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, GenericImageView};
use serde::{Deserialize, Serialize};
use std::path::Path;

const WEBP_RATIO_SAMPLE_MAX_DIM: u32 = 256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
pub enum LoopIntentVerdict {
    /// Strong loop intent: Keep as GIF / convert video → GIF.
    LoopStrong(String),
    /// Weak loop intent: Convert GIF → video / keep as video.
    LoopWeak(String),
    /// Uncertain: insufficient signal, handled by conservative fallback (Layer 7).
    Uncertain(String),
}

impl LoopIntentVerdict {
    /// Returns true if this media should be preserved as a looping GIF.
    #[must_use]
    pub fn is_keep_gif(&self) -> bool {
        matches!(self, Self::LoopStrong(_))
    }

    /// Returns true if this media should be converted to / kept as video.
    #[must_use]
    pub fn is_keep_video(&self) -> bool {
        matches!(self, Self::LoopWeak(_))
    }

    /// Returns true if classification is uncertain (Layer 7 fallback applies).
    #[must_use]
    pub fn is_uncertain(&self) -> bool {
        matches!(self, Self::Uncertain(_))
    }

    /// The human-readable reason string embedded in the verdict.
    #[must_use]
    pub fn reason(&self) -> &str {
        match self {
            Self::LoopStrong(r) | Self::LoopWeak(r) | Self::Uncertain(r) => r,
        }
    }
}

// ── LoopMeta: unified signal bundle ──────────────────────────────────────────

/// Unified signal bundle consumed by the 7-layer decision tree.
///
/// Populated by constructors (`from_video_detection`, `from_ffprobe_result`, `from_gif_path`).
/// The tree itself is a pure function over this struct — no I/O, no side effects.
#[derive(Debug, Clone, Default)]
pub struct LoopMeta {
    // ── Basic geometry ──
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frame_count: u64,
    pub file_size_bytes: u64,

    // ── Identity ──
    pub file_name: Option<String>,
    pub source_extension: Option<String>,
    pub parent_directories: Option<Vec<String>>,

    // ── Layer 1 signals (hard constraints) ──
    pub has_audio: bool,
    pub has_transparency: bool,
    /// Whether the source is natively a GIF container.
    pub is_native_gif: bool,

    // ── Layer 2 signals (explicit declarations) ──
    /// 0 = infinite loop, 1 = play once, `None` = unknown.
    pub loop_count: Option<u16>,
    /// e.g. [`GIPHY`, `NETSCAPE2.0`, ...] from `GIF` Application Extension block.
    pub app_extensions: Option<Vec<String>>,
    /// "webm", "mp4", "gif", etc.
    pub container: Option<String>,

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

    // ── Layer 5 signals (context semantics) ──
    pub directory_loop_intent_score: f64,
    pub filename_loop_intent_score: f64,

    // ── Color Profile signals ──
    pub has_embedded_icc: bool,
    pub has_complex_color_profile: bool,

    // ── Auxiliary (used in KNN bridge) ──
    pub frame_types: Vec<char>,
    pub mv_magnitudes: Vec<f64>,
    pub cached_frame_png: Option<Vec<u8>>,
    pub is_meme_platform: bool,
}

impl LoopMeta {
    /// Build `LoopMeta` from a full `VideoDetectionResult`.
    pub fn from_video_detection(detection: &VideoDetectionResult) -> Self {
        let file_path = Path::new(&detection.file_path);
        let file_name = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        let source_extension = file_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());
        let parent_directories: Option<Vec<String>> = file_path.parent().map(|p| {
            p.iter()
                .rev()
                .take(4)
                .filter_map(|s| s.to_str())
                .map(|s| s.to_string())
                .collect()
        });

        // Detect transparency from pixel format
        let has_transparency = detection.pix_fmt.contains("a")
            || detection.pix_fmt.contains("yuva")
            || detection.pix_fmt.contains("gbrap");

        // Detect palette-based formats (limited color space)
        let palette_size = if detection.pix_fmt == "pal8" {
            Some(256)
        } else {
            None
        };

        let mut meta = Self {
            duration_secs: detection.duration_secs,
            width: detection.width,
            height: detection.height,
            fps: detection.fps,
            frame_count: detection.frame_count,
            file_size_bytes: detection.file_size,
            file_name,
            source_extension,
            parent_directories: parent_directories.clone(),
            has_audio: detection.has_audio,
            has_transparency,
            is_native_gif: detection.format == "gif",
            loop_count: detection.loop_count,
            app_extensions: Some(Vec::new()),
            container: Some(detection.format.clone()),
            has_embedded_icc: false,
            has_complex_color_profile: matches!(
                detection.color_space,
                ColorSpace::BT2020 | ColorSpace::AdobeRGB
            ) || detection.is_dolby_vision
                || detection.is_hdr10_plus,
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
                    .map(|&s| f64::from(u32::try_from(s).unwrap_or(u32::MAX)))
                    .collect();
                calculate_gini_f64(&sizes)
            },
            temporal_flatness: None,
            block_skew: None,
            loop_closure_score: loop_closure_score(&detection.pkt_sizes),
            motion_periodicity: motion_periodicity_score(&detection.mv_magnitudes),
            temporal_jitter: temporal_jitter_score(&detection.pts_deltas),
            directory_loop_intent_score: 0.5,
            filename_loop_intent_score: 0.5,
            frame_types: detection.frame_types.clone(),
            mv_magnitudes: detection.mv_magnitudes.clone(),
            cached_frame_png: None,
            is_meme_platform: {
                detection.tags.values().any(|v| {
                    let up = v.to_uppercase();
                    crate::constants::LOOP_PLATFORM_MARKERS
                        .iter()
                        .any(|&m| up.contains(m))
                })
            },
        };
        meta.directory_loop_intent_score =
            score_directory_context(parent_directories.as_deref(), &[]);
        meta.filename_loop_intent_score = analyze_filename(meta.file_name.as_deref(), &[]).raw;
        meta.populate_webp_compression_ratio_from_path(file_path);
        meta
    }

    /// Build `LoopMeta` from an `FFprobeResult` (used in pipelines without full detection).
    pub fn from_ffprobe_result(probe: &crate::ffprobe::FFprobeResult, path: &Path) -> Self {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        let source_extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());
        let parent_directories: Option<Vec<String>> = path.parent().map(|p| {
            p.iter()
                .rev()
                .take(4)
                .filter_map(|s| s.to_str())
                .map(|s| s.to_string())
                .collect()
        });

        // Detect transparency from pixel format
        let has_transparency = probe.pix_fmt.contains("a")
            || probe.pix_fmt.contains("yuva")
            || probe.pix_fmt.contains("gbrap");

        // Detect palette-based formats (limited color space)
        let palette_size = if probe.pix_fmt == "pal8" {
            Some(256)
        } else {
            None
        };

        let mut meta = Self {
            duration_secs: probe.duration,
            width: probe.width,
            height: probe.height,
            fps: probe.frame_rate,
            frame_count: probe.frame_count,
            file_size_bytes: probe.size,
            file_name,
            source_extension,
            parent_directories: parent_directories.clone(),
            has_audio: probe.has_audio,
            has_transparency,
            is_native_gif: probe.format_name == "gif",
            loop_count: probe.loop_count,
            app_extensions: Some(Vec::new()),
            container: Some(probe.format_name.clone()),
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
                    .map(|&s| f64::from(u32::try_from(s).unwrap_or(u32::MAX)))
                    .collect();
                calculate_gini_f64(&sizes)
            },
            block_skew: None,
            loop_closure_score: loop_closure_score(&probe.pkt_sizes),
            motion_periodicity: motion_periodicity_score(&probe.mv_magnitudes),
            temporal_jitter: temporal_jitter_score(&probe.pts_deltas),
            directory_loop_intent_score: 0.5,
            filename_loop_intent_score: 0.5,
            frame_types: probe.frame_types.clone(),
            mv_magnitudes: probe.mv_magnitudes.clone(),
            has_embedded_icc: false,
            has_complex_color_profile: false,
            cached_frame_png: None,
            is_meme_platform: false,
        };
        meta.is_meme_platform = meta
            .app_extensions
            .as_ref()
            .is_some_and(|e_list: &Vec<String>| {
                e_list.iter().any(|e: &String| {
                    let up = e.to_uppercase();
                    crate::constants::LOOP_PLATFORM_MARKERS
                        .iter()
                        .any(|&m| up.contains(m))
                })
            });
        meta.directory_loop_intent_score =
            score_directory_context(parent_directories.as_deref(), &[]);
        meta.filename_loop_intent_score = analyze_filename(meta.file_name.as_deref(), &[]).raw;
        meta.populate_webp_compression_ratio_from_path(path);
        meta
    }

    /// Build `LoopMeta` from a `GIF` file using header-level scanning (fast, no `ffprobe`).
    pub fn from_gif_path(path: &Path) -> Option<Self> {
        let (pal, exts, has_transparency, variation, delay_variation, loops, total_dur) =
            crate::media_meta_utils::scan_gif_headers(path).ok()?;

        let file_size = std::fs::metadata(path).ok()?.len();
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        let parent_directories: Option<Vec<String>> = path.parent().map(|p| {
            p.iter()
                .rev()
                .take(4)
                .filter_map(|s| s.to_str())
                .map(|s| s.to_string())
                .collect()
        });

        // Fast header read for GIF dimensions
        let (width, height) = if let Ok(mut f) = std::fs::File::open(path) {
            use std::io::Read;
            let mut head = [0u8; 10];
            if f.read_exact(&mut head).is_ok() {
                (
                    u32::from(u16::from_le_bytes([head[6], head[7]])),
                    u32::from(u16::from_le_bytes([head[8], head[9]])),
                )
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        };

        let frame_count = if let Some(dur) = total_dur {
            if dur > 0.0 {
                if let Some(_v) = delay_variation {
                    crate::numeric_cast::f64_to_u32_sat((dur * 10.0_f64).ceil()).min(10000_u32)
                } else {
                    1_u32
                }
            } else {
                1_u32
            }
        } else {
            1_u32
        };

        let fps = if let Some(dur) = total_dur {
            if frame_count > 1 && dur > 0.0 {
                f64::from(frame_count) / dur
            } else {
                12.0
            }
        } else {
            12.0
        };

        let mut meta = Self {
            duration_secs: total_dur.unwrap_or(0.0),
            width,
            height,
            fps,
            frame_count: u64::from(frame_count),
            file_size_bytes: file_size,
            file_name,
            source_extension: Some("gif".to_string()),
            parent_directories: parent_directories.clone(),
            has_audio: false,
            has_transparency,
            loop_count: loops,
            app_extensions: exts.clone(),
            container: Some("gif".to_string()),
            is_native_gif: true,
            frame_payload_variation: variation,
            frame_delay_variation: delay_variation,
            palette_size: pal,
            is_meme_platform: exts.as_ref().is_some_and(|e_list: &Vec<String>| {
                e_list.iter().any(|e: &String| {
                    let up = e.to_uppercase();
                    crate::constants::LOOP_PLATFORM_MARKERS
                        .iter()
                        .any(|&m| up.contains(m))
                })
            }),
            ..Default::default()
        };

        meta.directory_loop_intent_score =
            score_directory_context(parent_directories.as_deref(), &[]);
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
            if let Some(temp_frame) = extract_frame_to_temp(path) {
                if let Ok(bytes) = std::fs::read(&temp_frame) {
                    // Remove the temporary file immediately; keep bytes in-memory only.
                    let _ = std::fs::remove_file(&temp_frame);

                    // Cache the PNG bytes for potential reuse in Tier 3 visual heuristics.
                    self.cached_frame_png = Some(bytes.clone());

                    // Compute the WebP compression ratio from the in-memory image.
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        self.webp_compression_ratio =
                            sampled_webp_compression_ratio_from_image(&img);
                    }
                }
            }
        }
    }

    pub fn should_sample_webp_compression_ratio(&self) -> bool {
        self.width >= 64 && self.height >= 64 && self.duration_secs > 0.05
    }

    /// Re-run semantic scoring with dynamic keywords from the database.
    pub fn refresh_semantics(&mut self, keywords: &[String]) {
        self.directory_loop_intent_score =
            score_directory_context(self.parent_directories.as_deref(), keywords);
        self.filename_loop_intent_score = analyze_filename(self.file_name.as_deref(), keywords).raw;
    }
}

// ── DB-Driven Loop Intent Forest ─────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy)]
struct LogOdds(f64);

impl LogOdds {
    fn add(&mut self, delta: f64) {
        self.0 += delta;
    }

    fn value(self) -> f64 {
        self.0
    }

    fn probability(self) -> f64 {
        1.0 / (1.0 + (-self.0).exp())
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct DerivedLoopSignals {
    scene_cut: bool,
    localized_motion: bool,
    zero_motion_ratio: f64,
}

impl DerivedLoopSignals {
    fn from_meta(meta: &LoopMeta) -> Self {
        let zero_motion_ratio = zero_motion_ratio(&meta.mv_magnitudes);
        Self {
            scene_cut: detect_scene_cut(&meta.pkt_sizes),
            localized_motion: meta.mv_magnitudes.len() >= 10 && zero_motion_ratio > 0.70,
            zero_motion_ratio,
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
    fn from_profile(reference: Option<&LoopReferenceProfile>) -> Self {
        let reference = reference.cloned().unwrap_or_default();
        let duration_percentiles_available = reference.duration.p25.is_some()
            || reference.duration.p10.is_some()
            || reference.duration.p50.is_some();
        let short_percentile = reference.duration.p25.or(reference.duration.p10).unwrap_or(
            reference
                .collection
                .duration_p90
                .min(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS),
        );
        let median_scaled = reference
            .duration
            .p50
            .map_or(short_percentile, |median| median * 0.60);
        let duration_override_secs = if duration_percentiles_available {
            short_percentile
                .min(median_scaled.max(0.35))
                .clamp(0.35, reference.collection.duration_p90.max(0.35))
        } else {
            (reference.duration.mean + reference.duration.std_dev * 0.25)
                .clamp(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS, 4.5)
        };
        let short_clip_secs = reference
            .duration
            .p50
            .or(reference.duration.p75.map(|value| value.min(8.0)))
            .unwrap_or(
                (reference.duration.mean + reference.duration.std_dev * 0.50)
                    .clamp(duration_override_secs + 1.0, 8.0),
            )
            .max(duration_override_secs + 0.5);
        let short_asset_window_secs =
            short_clip_secs.max(crate::constants::HARD_PASS_SHORT_GIF_THRESHOLD_SECS);
        let modern_bias_duration_secs = reference
            .duration
            .p75
            .unwrap_or(reference.collection.duration_p90)
            .max(reference.collection.duration_p90)
            .max(crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS);

        Self {
            reference,
            duration_override_secs,
            short_clip_secs,
            short_asset_window_secs,
            modern_bias_duration_secs,
            decision_threshold: crate::constants::TREE_DECISION_LOG_ODDS_THRESHOLD,
        }
    }

    fn get_feature_weight(&self, key: &str) -> f64 {
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
            _ => None,
        }
        .unwrap_or(1.0)
        .max(0.01) // Ensure it never fully zeros out a signal unless intentionally zero
    }

    fn clamp_z(value: f64) -> f64 {
        value.clamp(
            -crate::constants::TREE_Z_SCORE_CAP,
            crate::constants::TREE_Z_SCORE_CAP,
        )
    }

    fn duration_z(&self, duration_secs: f64) -> f64 {
        Self::clamp_z(self.reference.duration.z_score(duration_secs))
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
    pub verdict: LoopIntentVerdict,
    pub tree_probability: f64,
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

fn zero_motion_ratio(mvs: &[f64]) -> f64 {
    if mvs.is_empty() {
        return 0.0;
    }
    let zero_count = mvs.iter().filter(|&&value| value.abs() < 0.1).count();
    f64::from(u32::try_from(zero_count).unwrap_or(u32::MAX))
        / f64::from(u32::try_from(mvs.len()).unwrap_or(1))
}

fn is_near_16_by_9(width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    ((f64::from(width) / f64::from(height)) - crate::constants::ASPECT_RATIO_WIDESCREEN).abs()
        < crate::constants::ASPECT_RATIO_TOLERANCE_NEAR
}

fn loop_count_zero_bonus(duration_secs: f64, thresholds: &LoopThresholds) -> f64 {
    let short_window = (thresholds.duration_override_secs * 6.0).max(3.0);
    let long_window = (thresholds.modern_bias_duration_secs * 2.0).max(short_window + 1.0);

    if duration_secs <= short_window {
        crate::constants::LOOP_COUNT_ZERO_BONUS_MAX
    } else if duration_secs <= long_window {
        let decay = (duration_secs - short_window) / (long_window - short_window);
        crate::constants::LOOP_COUNT_ZERO_BONUS_MAX
            - (crate::constants::LOOP_COUNT_ZERO_BONUS_DECAY_MAX * decay)
    } else {
        crate::constants::LOOP_COUNT_ZERO_BONUS_MIN
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
    let fps_positive = thresholds.fps_z(meta.fps.max(1.0)).max(0.0);
    let fps_negative = (-thresholds.fps_z(meta.fps.max(1.0))).max(0.0);
    let total_pixels = f64::from(meta.width) * f64::from(meta.height);

    if duration_positive > 0.0 {
        let short_fast = duration_positive * (1.0 + fps_positive * 0.5);
        log_odds.add(
            short_fast.min(crate::constants::TREE_Z_SCORE_CAP)
                * crate::constants::SHORT_FAST_POSITIVE_LOG_ODDS,
        );
    }

    if duration_negative > 0.0 {
        let long_slow = duration_negative * (1.0 + fps_negative * 0.5);
        log_odds.add(
            -long_slow.min(crate::constants::TREE_Z_SCORE_CAP)
                * crate::constants::LONG_SLOW_NEGATIVE_LOG_ODDS,
        );
    }

    if derived.scene_cut {
        log_odds.add(-crate::constants::SCENE_CUT_NEGATIVE_LOG_ODDS);
    }

    let compactness_signal = (-thresholds.file_size_z(f64::from(
        u32::try_from(meta.file_size_bytes).unwrap_or(u32::MAX),
    )))
    .max(0.0)
        * crate::constants::COMPACTNESS_SIGNAL_SIZE_WEIGHT
        + (-thresholds.pixels_z(total_pixels)).max(0.0)
            * crate::constants::COMPACTNESS_SIGNAL_PIXELS_WEIGHT;
    if !meta.has_audio && compactness_signal > 0.0 {
        log_odds.add(
            (compactness_signal + crate::constants::COMPACTNESS_SIGNAL_BIAS)
                .min(crate::constants::COMPACTNESS_SIGNAL_MAX)
                * crate::constants::COMPACT_SILENT_POSITIVE_LOG_ODDS,
        );
    }

    let large_media_signal = thresholds
        .file_size_z(f64::from(
            u32::try_from(meta.file_size_bytes).unwrap_or(u32::MAX),
        ))
        .max(0.0)
        * crate::constants::LARGE_MEDIA_SIGNAL_SIZE_WEIGHT
        + thresholds.pixels_z(total_pixels).max(0.0)
            * crate::constants::LARGE_MEDIA_SIGNAL_PIXELS_WEIGHT;
    if large_media_signal > 0.0 {
        let audio_multiplier = if meta.has_audio {
            1.0
        } else {
            crate::constants::LARGE_MEDIA_AUDIO_MULTIPLIER
        };
        log_odds.add(
            -large_media_signal.min(crate::constants::LARGE_MEDIA_SIGNAL_MAX)
                * audio_multiplier
                * crate::constants::LARGE_MEDIA_NEGATIVE_LOG_ODDS
                * thresholds.get_feature_weight("file_size_bytes").sqrt(),
        );
    }
}

fn apply_weak_heuristics(
    meta: &LoopMeta,
    derived: &DerivedLoopSignals,
    thresholds: &LoopThresholds,
    log_odds: &mut LogOdds,
    is_image: bool,
    is_video: bool,
) {
    let ext_lower = meta
        .source_extension
        .as_deref()
        .unwrap_or("")
        .to_lowercase();
    let is_webm = meta
        .container
        .as_deref()
        .is_some_and(|container| container.eq_ignore_ascii_case("webm"))
        || ext_lower == "webm";
    let is_short_clip = !meta.has_audio
        && meta.duration_secs > thresholds.duration_override_secs
        && meta.duration_secs <= thresholds.short_clip_secs;
    let is_extended_short_asset = !meta.has_audio
        && meta.duration_secs > thresholds.short_clip_secs
        && meta.duration_secs <= thresholds.short_asset_window_secs;

    if has_platform_marker(meta.app_extensions.as_deref()) || meta.is_meme_platform {
        log_odds.add(crate::constants::PLATFORM_MARKER_POSITIVE_LOG_ODDS);
    }
    if is_webm && !meta.has_audio {
        log_odds.add(crate::constants::SHORT_CLIP_MIN_BIAS);
    }
    if meta.has_transparency {
        log_odds.add(crate::constants::TRANSPARENCY_POSITIVE_LOG_ODDS);
    }
    if is_short_clip {
        let range = (thresholds.short_clip_secs - thresholds.duration_override_secs).max(0.5);
        let headroom = 1.0
            - ((meta.duration_secs - thresholds.duration_override_secs) / range).clamp(0.0, 1.0);
        let format_bonus = if is_image {
            crate::constants::SHORT_CLIP_FORMAT_BONUS_IMAGE
        } else {
            crate::constants::SHORT_CLIP_FORMAT_BONUS_VIDEO
        };
        let cadence_bonus = if meta.frame_count > 1 {
            crate::constants::SHORT_CLIP_CADENCE_BONUS
        } else {
            0.0
        };
        log_odds.add(
            (crate::constants::SHORT_CLIP_MIN_BIAS
                + headroom * crate::constants::SHORT_CLIP_HEADROOM_MAX
                + format_bonus
                + cadence_bonus)
                * crate::constants::SHORT_CLIP_PRIOR_LOG_ODDS,
        );
    }
    if is_extended_short_asset {
        let range = (thresholds.short_asset_window_secs - thresholds.short_clip_secs).max(0.5);
        let tail_headroom =
            1.0 - ((meta.duration_secs - thresholds.short_clip_secs) / range).clamp(0.0, 1.0);
        let square_bonus = if meta.width > 0 && meta.width == meta.height {
            crate::constants::EXTENDED_SHORT_ASSET_SQUARE_BONUS
        } else {
            0.0
        };
        let image_bonus = if is_image {
            crate::constants::EXTENDED_SHORT_ASSET_IMAGE_BONUS
        } else {
            0.0
        };
        let compact_bonus = if meta.file_size_bytes <= crate::constants::STICKER_MAX_SIZE_BYTES {
            crate::constants::EXTENDED_SHORT_ASSET_COMPACT_BONUS
        } else {
            0.0
        };
        log_odds.add(
            (crate::constants::EXTENDED_SHORT_ASSET_MIN_BIAS
                + tail_headroom * crate::constants::EXTENDED_SHORT_ASSET_HEADROOM_MAX
                + square_bonus
                + image_bonus
                + compact_bonus)
                * crate::constants::EXTENDED_SHORT_ASSET_PRIOR_LOG_ODDS,
        );
    }
    if meta.loop_count == Some(1) {
        log_odds.add(-PLAY_ONCE_NEGATIVE_LOG_ODDS);
    } else if meta.loop_count == Some(0) {
        log_odds.add(loop_count_zero_bonus(meta.duration_secs, thresholds));
    }

    if let Some(delay_variation) = meta.frame_delay_variation {
        log_odds.add(
            -thresholds.delay_variation_z(delay_variation)
                * crate::constants::FEATURE_WEIGHT_DELAY_VAR
                * thresholds.get_feature_weight("delay_var"),
        );
    }
    if let Some(webp_ratio) = meta.webp_compression_ratio {
        log_odds.add(
            thresholds.webp_ratio_z(webp_ratio)
                * crate::constants::FEATURE_WEIGHT_WEBP_RATIO
                * thresholds.get_feature_weight("webp_ratio"),
        );
    }
    if let Some(motion_gini) = meta.motion_gini {
        log_odds.add(
            thresholds.motion_gini_z(motion_gini)
                * crate::constants::FEATURE_WEIGHT_MOTION_GINI
                * thresholds.get_feature_weight("m_gini"),
        );
    }
    if let Some(palette_depth) = meta.palette_depth {
        log_odds.add(
            thresholds.palette_depth_z(palette_depth)
                * crate::constants::FEATURE_WEIGHT_PALETTE_DEPTH
                * thresholds.get_feature_weight("p_depth"),
        );
    }
    if let Some(temporal_flatness) = meta.temporal_flatness {
        log_odds.add(
            thresholds.temporal_flatness_z(temporal_flatness)
                * crate::constants::FEATURE_WEIGHT_TEMPORAL_FLATNESS
                * thresholds.get_feature_weight("t_flat"),
        );
    }

    if derived.localized_motion || derived.zero_motion_ratio > 0.80 {
        log_odds.add(LOCALIZED_MOTION_POSITIVE_LOG_ODDS);
    }

    if meta.directory_loop_intent_score > 0.8 {
        log_odds.add(DIRECTORY_CONTEXT_POSITIVE_LOG_ODDS);
    }
    if meta.filename_loop_intent_score > 0.8 {
        log_odds.add(FILENAME_CONTEXT_POSITIVE_LOG_ODDS);
    }

    if meta.frame_count > 0 {
        if meta.frame_count <= 8 {
            log_odds.add(crate::constants::FRAME_COUNT_SHORT_BONUS);
        } else if meta.frame_count > 500 {
            log_odds.add(-crate::constants::FRAME_COUNT_LONG_PENALTY);
        }
    }

    if meta.width > 0 && meta.height > 0 {
        if meta.width == meta.height {
            log_odds.add(crate::constants::SQUARE_ASPECT_BONUS);
        } else if is_near_16_by_9(meta.width, meta.height) {
            log_odds.add(-crate::constants::WIDESCREEN_ASPECT_PENALTY);
        }
    }

    if fps_anomaly_score(meta.fps) > 0.6 {
        log_odds.add(crate::constants::FPS_ANOMALY_BONUS);
    }

    if !meta.has_audio && meta.duration_secs > thresholds.modern_bias_duration_secs {
        let overflow = ((meta.duration_secs - thresholds.modern_bias_duration_secs)
            / thresholds.modern_bias_duration_secs.max(1.0))
        .clamp(0.0, 1.0);
        let container_penalty = if is_video {
            crate::constants::LONG_SILENT_PENALTY_VIDEO_ADD
        } else if is_image {
            crate::constants::LONG_SILENT_PENALTY_IMAGE_ADD
        } else {
            0.0
        };
        let transparency_relief = if meta.has_transparency {
            crate::constants::LONG_SILENT_TRANSPARENCY_RELIEF
        } else {
            0.0
        };
        let penalty = (crate::constants::LONG_SILENT_PENALTY_BASE
            + overflow * crate::constants::LONG_SILENT_PENALTY_OVERFLOW_MAX
            + container_penalty
            - transparency_relief)
            .max(crate::constants::LONG_SILENT_MIN_PENALTY);
        log_odds.add(-penalty * crate::constants::LONG_SILENT_PRIOR_NEGATIVE_LOG_ODDS);
    }

    if is_image {
        log_odds.add(crate::constants::IMAGE_PRIOR_BONUS);
    } else if is_video {
        log_odds.add(-crate::constants::VIDEO_PRIOR_PENALTY);
    }

    let bias_enabled = std::env::var(crate::constants::ENV_MODERN_FORMAT_CONVERT_BIAS)
        .map_or(true, |value| value == "1");
    let is_modern = crate::constants::MODERN_ANIMATED_EXTENSIONS.contains(&ext_lower.as_str());
    if is_modern && bias_enabled && (meta.duration_secs > thresholds.modern_bias_duration_secs) {
        let master_like = meta.has_embedded_icc
            || meta.has_complex_color_profile
            || meta
                .webp_compression_ratio
                .is_some_and(|ratio| thresholds.webp_ratio_z(ratio) < -0.75);
        if master_like {
            log_odds.add(-MODERN_MASTER_NEGATIVE_LOG_ODDS);
        }
    }
}

#[must_use]
pub fn identify_loop_intent(meta: &LoopMeta) -> LoopIntentVerdict {
    evaluate_loop_tree(meta, None).verdict
}

fn developer_layer1_override_enabled(name: &str) -> bool {
    let val = std::env::var(name).ok();
    if let Some(value) = val {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    } else {
        // PATCH: Default disabled. All logic now relies on DB-tuned thresholds (Layer 1-B) and KNN (Layer 6).
        false
    }
}

pub fn evaluate_loop_tree(
    meta: &LoopMeta,
    reference_profile: Option<&LoopReferenceProfile>,
) -> TreeEvaluation {
    let ext_lower = meta
        .source_extension
        .as_deref()
        .unwrap_or("")
        .to_lowercase();
    let is_image = SUPPORTED_IMAGE_EXTENSIONS.contains(&ext_lower.as_str());
    let is_video = !is_image && SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str());
    let derived = DerivedLoopSignals::from_meta(meta);
    let thresholds = LoopThresholds::from_profile(reference_profile);
    let mut log_odds = LogOdds::default();

    let finalize = |verdict: LoopIntentVerdict, log_odds: LogOdds| TreeEvaluation {
        tree_probability: match verdict {
            LoopIntentVerdict::LoopStrong(_) => 1.0,
            LoopIntentVerdict::LoopWeak(_) => 0.0,
            LoopIntentVerdict::Uncertain(_) => log_odds.probability(),
        },
        verdict,
    };

    if meta.has_audio && is_video {
        return finalize(
            LoopIntentVerdict::LoopWeak(
                "Layer 1-A: audio track present in a video container".to_string(),
            ),
            log_odds,
        );
    }

    if meta.frame_count <= 1 {
        return finalize(
            LoopIntentVerdict::LoopWeak("Layer 1-A: single frame media (cannot loop)".to_string()),
            log_odds,
        );
    }

    if !meta.has_audio
        && meta.duration_secs > 0.0
        && meta.duration_secs <= thresholds.duration_override_secs
    {
        return finalize(
            LoopIntentVerdict::LoopStrong(format!(
                "Layer 1-B: duration override ({:.2}s <= {:.2}s DB short-duration threshold)",
                meta.duration_secs, thresholds.duration_override_secs
            )),
            log_odds,
        );
    }

    // Layer 1-B2: Sticker-class native GIF — strong loop/sticker prior from physical envelope.
    // Runs only after Layer 1-B (duration is above the DB short cutoff). Uncertain cases that
    // do not match this profile still defer to Layer 4 and Layer 6 KNN.
    if ext_lower == "gif"
        && !meta.has_audio
        && meta.frame_count > 1
        && meta.duration_secs > 0.0
        && meta.duration_secs <= f64::from(crate::constants::ANIMATION_CLIP_THRESHOLD_SECS)
        && meta.width > 0
        && meta.height > 0
        && meta.width <= crate::constants::STICKER_MAX_DIMENSION
        && meta.height <= crate::constants::STICKER_MAX_DIMENSION
    {
        let px = u64::from(meta.width) * u64::from(meta.height);
        if px <= crate::constants::STICKER_TIER_NATIVE_GIF_MAX_PIXELS {
            return finalize(
                LoopIntentVerdict::LoopStrong(format!(
                    "Layer 1-B2: sticker-class native GIF ({}x{}, {:.2}s, {} px; strong loop/sticker prior)",
                    meta.width, meta.height, meta.duration_secs, px
                )),
                log_odds,
            );
        }
    }

    let force_short_gifs =
        developer_layer1_override_enabled(crate::constants::ENV_FORCE_SHORT_GIFS);
    if force_short_gifs
        && !meta.has_audio
        && meta.duration_secs > 0.0
        && meta.duration_secs <= crate::constants::HARD_PASS_SHORT_GIF_THRESHOLD_SECS
    {
        return finalize(
            LoopIntentVerdict::LoopStrong(format!(
                "Layer 1-C (Dev): forceful short asset pass ({:.2}s <= {:.2}s manual threshold)",
                meta.duration_secs,
                crate::constants::HARD_PASS_SHORT_GIF_THRESHOLD_SECS
            )),
            log_odds,
        );
    }

    let intercept_long_silent =
        developer_layer1_override_enabled(crate::constants::ENV_INTERCEPT_LONG_SILENT);
    if intercept_long_silent
        && !meta.has_audio
        && meta.duration_secs > crate::constants::HARD_PASS_SHORT_GIF_THRESHOLD_SECS
    {
        return finalize(
            LoopIntentVerdict::LoopWeak(format!(
                "Layer 1-D (Dev): intercepting long silent asset (> {:.2}s) to video pathway",
                crate::constants::HARD_PASS_SHORT_GIF_THRESHOLD_SECS
            )),
            log_odds,
        );
    }

    evaluate_kinetics_and_physics(meta, &derived, &thresholds, &mut log_odds);
    apply_weak_heuristics(
        meta,
        &derived,
        &thresholds,
        &mut log_odds,
        is_image,
        is_video,
    );

    if log_odds.value() >= thresholds.decision_threshold {
        return finalize(
            LoopIntentVerdict::LoopStrong(format!(
                "Layer 4: log-odds {:.2} >= {:.2} (short/fast/abrupt profile)",
                log_odds.value(),
                thresholds.decision_threshold
            )),
            log_odds,
        );
    }

    if log_odds.value() <= -thresholds.decision_threshold {
        return finalize(
            LoopIntentVerdict::LoopWeak(format!(
                "Layer 4: log-odds {:.2} <= -{:.2} (long/slow/smooth profile)",
                log_odds.value(),
                thresholds.decision_threshold
            )),
            log_odds,
        );
    }

    finalize(
        LoopIntentVerdict::Uncertain(format!(
            "Layer 4: log-odds {:.2} within ±{:.2}; defer to KNN fusion",
            log_odds.value(),
            thresholds.decision_threshold
        )),
        log_odds,
    )
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

    let ext = meta
        .source_extension
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_image = SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str());
    let is_gif_family =
        ext == "gif" || crate::constants::MODERN_ANIMATED_EXTENSIONS.contains(&ext.as_str());
    let short_clip_like = !meta.has_audio
        && meta.duration_secs > 0.0
        && meta.duration_secs <= thresholds.short_asset_window_secs;

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
) -> f64 {
    use crate::constants::{
        LAYER6_LR_BIAS, LAYER6_LR_W_DENSITY, LAYER6_LR_W_KNN, LAYER6_LR_W_TREE,
    };

    // neighbor_count is log-scaled to normalized density signal
    let density_signal = f64::from(u32::try_from(neighbor_count).unwrap_or(u32::MAX)).ln_1p();

    let score = (knn_prob * LAYER6_LR_W_KNN)
        + (tree_prob * LAYER6_LR_W_TREE)
        + (density_signal * LAYER6_LR_W_DENSITY)
        + LAYER6_LR_BIAS;

    // Apply sigmoid and then the micro-nudge adjustment
    let fused_prob = 1.0 / (1.0 + (-score).exp());
    (fused_prob + nudge).clamp(0.01, 0.99)
}

fn compute_layer6_fusion(
    keep_prob: f64,
    tree_probability: f64,
    neighbor_count: usize,
    nudge_score: f64,
) -> Layer6Fusion {
    let final_score =
        logistic_regression_fusion(keep_prob, tree_probability, neighbor_count, nudge_score);

    // Legacy weights kept for logging purposes, but the final_score now uses LR
    Layer6Fusion {
        knn_weight: crate::constants::LAYER6_MAX_KNN_WEIGHT, // Representational
        tree_weight: 1.0 - crate::constants::LAYER6_MAX_KNN_WEIGHT,
        final_score,
    }
}

/// Execute the loop intent identification for a given detection result.
///
/// # Errors
/// Returns an error if the underlying database fetches fail or the
/// classification logic encounters an IO error.
pub fn assess_loop_intent(detection: &VideoDetectionResult) -> LoopIntentVerdict {
    let meta = LoopMeta::from_video_detection(detection);
    assess_loop_intent_from_meta(&meta, Some(Path::new(&detection.file_path)))
}

/// Execute the loop intent identification for a given probe result.
///
/// # Errors
/// Returns an error if the underlying database fetches fail or the
/// classification logic encounters an IO error.
pub fn assess_loop_intent_from_probe(
    probe: &crate::ffprobe::FFprobeResult,
    path: &Path,
) -> LoopIntentVerdict {
    let meta = LoopMeta::from_ffprobe_result(probe, path);
    assess_loop_intent_from_meta(&meta, Some(path))
}

/// Every invocation logs an inference record to the database (if connected) to
/// build the feedback loop described in Level 4 of the database utilization plan.
///
/// # Errors
/// Returns an error if the underlying database fetches fail or the
/// classification logic encounters an IO error during visual sampling.
pub fn assess_loop_intent_from_meta(meta: &LoopMeta, path: Option<&Path>) -> LoopIntentVerdict {
    use crate::database::{
        fetch_loop_reference_profile, log_inference_record, lookup_similar_samples, open_pg_client,
        LoopInferenceRecord,
    };

    let disable_db = developer_layer1_override_enabled(crate::constants::ENV_DISABLE_DB_FEEDBACK);
    let mut conn = if disable_db {
        None
    } else {
        open_pg_client().ok()
    };
    let is_legacy_mode = conn.is_none();

    let reference_profile = conn
        .as_mut()
        .and_then(|client| fetch_loop_reference_profile(client).ok());

    let thresholds = LoopThresholds::from_profile(reference_profile.as_ref());
    let keywords = reference_profile
        .as_ref()
        .map_or(&[][..], |profile| profile.top_keywords.as_slice());

    let mut mutable_meta = meta.clone();
    mutable_meta.refresh_semantics(keywords);

    // ── Layer 0: Legacy Fallback ──
    if is_legacy_mode {
        emit_stderr(
            "⚠️  Loop DB unavailable or disabled — running tree without KNN and refusing fabricated priors",
        );
        let tree_only = evaluate_loop_tree(&mutable_meta, None);
        match &tree_only.verdict {
            LoopIntentVerdict::LoopStrong(reason) | LoopIntentVerdict::LoopWeak(reason) => {
                emit_stderr(&format!("💡 Tree-only Result: {reason}"));
                return tree_only.verdict;
            }
            LoopIntentVerdict::Uncertain(reason) => {
                emit_stderr(&format!(
                    "⚠️  Tree-only result remained uncertain ({reason}) — using Layer 7 fallback"
                ));
                let fallback =
                    layer7_fallback(&mutable_meta, "Layer 0: DB unavailable / KNN disabled");
                emit_stderr(&format!("💡 Fallback Result: {}", fallback.reason()));
                return fallback;
            }
        }
    }

    let sample_match = lookup_similar_samples(meta, path);

    let tree = evaluate_loop_tree(&mutable_meta, reference_profile.as_ref());
    let tree_probability = tree.tree_probability;

    // ── Tracking variables for inference logging ──
    let mut knn_keep_probability: Option<f64> = None;
    let mut knn_confidence: Option<f64> = None;
    let mut knn_neighbor_count: Option<usize> = None;

    let verdict = match &tree.verdict {
        LoopIntentVerdict::LoopStrong(reason) | LoopIntentVerdict::LoopWeak(reason) => {
            if tree.verdict.is_keep_gif() {
                emit_stderr(&format!("✅ Tree Decisive: {reason}"));
            } else {
                emit_stderr(&format!("ℹ️  Tree Decisive: {reason}"));
            }
            tree.verdict.clone()
        }
        LoopIntentVerdict::Uncertain(reason) => {
            emit_stderr(&format!(
                "🔭 Tree uncertain ({reason}) [prob={tree_probability:.2}] — falling back to Layer 6 KNN..."
            ));

            if let Some(m) = sample_match {
                let Some(keep_prob) = m.keep_probability else {
                    emit_stderr(&format!(
                        "   ⚠️  KNN match missing keep-probability (conf={:.2}, n={}) — treating as unknown and deferring to Layer 7",
                        m.confidence, m.neighbor_count
                    ));
                    let final_v = layer7_fallback(meta, "Layer 6: KNN match missing probability");
                    if final_v.is_keep_gif() {
                        emit_stderr(&format!("✅ Fallback Result: {}", final_v.reason()));
                    } else {
                        emit_stderr(&format!("ℹ️  Fallback Result: {}", final_v.reason()));
                    }
                    return final_v;
                };
                let confidence = m.confidence;

                // Capture KNN data for inference log
                knn_keep_probability = m.keep_probability;
                knn_confidence = Some(confidence);
                knn_neighbor_count = Some(m.neighbor_count);

                let nudges = calculate_micro_nudges(meta);
                let fusion = compute_layer6_fusion(
                    keep_prob,
                    tree_probability,
                    m.neighbor_count,
                    nudges.score,
                );

                let mut final_score = fusion.final_score;

                if !nudges.trace.is_empty() {
                    emit_stderr(&format!(
                        "   ⚖️  Micro-Nudges ({:+.2}): {}",
                        nudges.score,
                        nudges.trace.join(" | ")
                    ));
                }

                if final_score > crate::constants::LAYER6_FUSION_SCORE_UNCERTAIN_LOW
                    && final_score < crate::constants::LAYER6_FUSION_SCORE_UNCERTAIN_HIGH
                    && confidence < crate::constants::LAYER6_CONFIDENCE_HIGH
                {
                    if let Some(p) = path {
                        emit_stderr(
                            "   🔍 Triggering high-cost visual heuristics (extreme uncertainty)...",
                        );
                        let mut tier3_nudge = AuxiliaryNudge::default();

                        let mut img_opt: Option<image::DynamicImage> = None;
                        if let Some(bytes) = meta.cached_frame_png.as_ref() {
                            if let Ok(img) = image::load_from_memory(bytes) {
                                img_opt = Some(img);
                            }
                        } else if let Some(temp_frame) = extract_frame_to_temp(p) {
                            if let Ok(bytes) = std::fs::read(&temp_frame) {
                                let _ = std::fs::remove_file(&temp_frame);
                                if let Ok(img) = image::load_from_memory(&bytes) {
                                    img_opt = Some(img);
                                }
                            }
                        }

                        if let Some(ref img) = img_opt {
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
                        }

                        if !tier3_nudge.trace.is_empty() {
                            emit_stderr(&format!(
                                "   📊 Tier 3 Visual ({:+.2}): {}",
                                tier3_nudge.score,
                                tier3_nudge.trace.join(" | ")
                            ));
                            final_score += tier3_nudge.score.clamp(
                                -crate::constants::AUXILIARY_NUDGE_CAP,
                                crate::constants::AUXILIARY_NUDGE_CAP,
                            );
                        }
                    }
                }

                if should_accept_layer6_loopstrong(
                    &mutable_meta,
                    &thresholds,
                    keep_prob,
                    final_score,
                    confidence,
                ) {
                    let v = LoopIntentVerdict::LoopStrong(format!(
                        "Layer 6: KNN+Nudges score={:.2} (knn={:.2}×{:.2}, tree={:.2}×{:.2}, nudge={:+.2}, conf={:.2}, n={})",
                        final_score,
                        keep_prob,
                        fusion.knn_weight,
                        tree_probability,
                        fusion.tree_weight,
                        nudges.score,
                        confidence,
                        m.neighbor_count
                    ));
                    emit_stderr(&format!("✅ KNN Fusion Success: {}", v.reason()));
                    v
                } else if confidence >= crate::constants::LAYER6_CONFIDENCE_HIGH
                    && final_score <= crate::constants::LAYER6_FUSION_SCORE_UNCERTAIN_LOW
                {
                    let v = LoopIntentVerdict::LoopWeak(format!(
                        "Layer 6: KNN+Nudges score={:.2} (knn={:.2}×{:.2}, tree={:.2}×{:.2}, nudge={:+.2}, conf={:.2}, n={})",
                        final_score,
                        keep_prob,
                        fusion.knn_weight,
                        tree_probability,
                        fusion.tree_weight,
                        nudges.score,
                        confidence,
                        m.neighbor_count
                    ));
                    emit_stderr(&format!("ℹ️  KNN Fusion Exit: {}", v.reason()));
                    v
                } else {
                    emit_stderr(&format!(
                        "   ℹ️  KNN data inconclusive (conf={confidence:.2}, score={final_score:.2}) — deferring to Layer 7"
                    ));
                    let final_v = layer7_fallback(meta, reason);
                    if final_v.is_keep_gif() {
                        emit_stderr(&format!("✅ Fallback Result: {}", final_v.reason()));
                    } else {
                        emit_stderr(&format!("ℹ️  Fallback Result: {}", final_v.reason()));
                    }
                    final_v
                }
            } else {
                emit_stderr(&format!(
                    "   ℹ️  KNN similarity match unavailable (tree_prob={tree_probability:.2}) — using Layer 7 fallback"
                ));
                let final_v = layer7_fallback(meta, reason);
                if final_v.is_keep_gif() {
                    emit_stderr(&format!("✅ Fallback Result: {}", final_v.reason()));
                } else {
                    emit_stderr(&format!("ℹ️  Fallback Result: {}", final_v.reason()));
                }
                final_v
            }
        }
    };

    // ── Inference Logging (Level 4 Feedback Loop) ──
    // Fire-and-forget: log the record if we have a DB connection, never block.
    if let Some(ref mut client) = conn {
        let (final_verdict_str, layer_exit) = match &verdict {
            LoopIntentVerdict::LoopStrong(r) => ("LoopStrong".to_string(), extract_layer_tag(r)),
            LoopIntentVerdict::LoopWeak(r) => ("LoopWeak".to_string(), extract_layer_tag(r)),
            LoopIntentVerdict::Uncertain(r) => ("Uncertain".to_string(), extract_layer_tag(r)),
        };

        let final_probability = match &verdict {
            LoopIntentVerdict::LoopStrong(_) => 1.0,
            LoopIntentVerdict::LoopWeak(_) => 0.0,
            LoopIntentVerdict::Uncertain(_) => tree_probability,
        };

        let record = LoopInferenceRecord {
            tree_probability,
            knn_keep_probability,
            knn_confidence,
            knn_neighbor_count,
            final_probability,
            final_verdict: final_verdict_str,
            decision_reason: verdict.reason().to_string(),
            layer_exit,
        };

        log_inference_record(client, meta, &record, path);
    }

    verdict
}

/// Layer 7: Conservative fallback with minimum-loss default.
fn layer7_fallback(meta: &LoopMeta, upstream_reason: &str) -> LoopIntentVerdict {
    let ext = meta
        .source_extension
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_gif = ext == "gif";
    let is_video = SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str());
    let is_modern_animated = crate::constants::MODERN_ANIMATED_EXTENSIONS.contains(&ext.as_str());

    let reason = format!("Layer 7: Fallback [{upstream_reason}]");

    if is_modern_animated {
        LoopIntentVerdict::LoopStrong(format!("{reason} → convert to GIF (modern animated)"))
    } else if is_gif {
        LoopIntentVerdict::LoopStrong(format!("{reason} → preserve GIF as-is (Layer 7 default)"))
    } else if is_video {
        let sticker_limit = match std::env::var(crate::constants::ENV_STICKER_LIMIT_SECS) {
            Ok(s) => match s.parse::<f64>() {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(env = crate::constants::ENV_STICKER_LIMIT_SECS, input = s, error = %e, "Invalid sticker limit env var; admitting unknown input via default value {}", crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS);
                    crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS
                }
            },
            Err(_) => crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS,
        };

        let is_sticker_safe_zone = !meta.has_audio
            && meta.duration_secs > 0.0
            && meta.duration_secs <= sticker_limit
            && meta.file_size_bytes > 0
            && meta.file_size_bytes <= crate::constants::STICKER_MAX_SIZE_BYTES;

        if is_sticker_safe_zone {
            LoopIntentVerdict::LoopStrong(format!(
                "{reason} → promotion [Sticker Safe Zone] (short/small/silent video)"
            ))
        } else {
            LoopIntentVerdict::LoopWeak(format!(
                "{reason} → standard video processing (no loop intent)"
            ))
        }
    } else {
        LoopIntentVerdict::LoopWeak(format!("{reason} → unknown format, standard processing"))
    }
}

/// Extract the layer tag (e.g. "Layer 1-A", "Layer 6", "Layer 7") from a verdict reason string.
fn extract_layer_tag(reason: &str) -> String {
    // Reason strings start with "Layer X..." — extract the prefix up to the first ':'
    if let Some(colon_pos) = reason.find(':') {
        reason[..colon_pos].trim().to_string()
    } else if reason.starts_with("Layer") {
        // Some reasons don't have a colon (e.g. Layer 7 fallback sub-reasons)
        reason.split_once('→').map_or_else(
            || reason.to_string(),
            |(prefix, _)| prefix.trim().to_string(),
        )
    } else {
        "Unknown".to_string()
    }
}

// ── Safety & Exploration Helpers ──────────────────────────────────────────────

/// Dynamic safety-guard for CRF 0.00 (lossless) exploration.
#[must_use]
pub fn is_lossless_exploration_safe(meta: &LoopMeta, path: Option<&Path>) -> bool {
    let sample_match = crate::database::lookup_similar_samples(meta, path);
    let (threshold, keep_prob_label) = match sample_match.as_ref().and_then(|m| m.keep_probability)
    {
        Some(keep_prob) => (
            lossless_duration_limit_for_keep_prob(keep_prob),
            format!("keep_prob={keep_prob:.2}"),
        ),
        None => {
            emit_stderr(
                "   ⚠️  Lossless-first safety: KNN evidence unavailable — using conservative high-value limit",
            );
            (
                crate::constants::HIGH_VALUE_LOSSLESS_DURATION_LIMIT,
                "keep_prob=unknown".to_string(),
            )
        }
    };
    let is_safe = meta.duration_secs < f64::from(threshold);

    if !is_safe {
        emit_stderr(&format!(
            "   ⚠️  Lossless-first (CRF 0.00) skip: duration {:.1}s exceeds limit {:.1}s ({})",
            meta.duration_secs, threshold, keep_prob_label
        ));
    }
    is_safe
}

#[must_use]
fn lossless_duration_limit_for_keep_prob(keep_prob: f64) -> f32 {
    use crate::constants::{HIGH_VALUE_LOSSLESS_DURATION_LIMIT, MEME_LOSSLESS_DURATION_LIMIT};
    if keep_prob <= 0.3 {
        HIGH_VALUE_LOSSLESS_DURATION_LIMIT
    } else if keep_prob >= 0.7 {
        MEME_LOSSLESS_DURATION_LIMIT
    } else {
        let t = (keep_prob - 0.3) / 0.4;
        let limit_meme = f64::from(MEME_LOSSLESS_DURATION_LIMIT);
        let limit_high = f64::from(HIGH_VALUE_LOSSLESS_DURATION_LIMIT);
        crate::numeric_cast::f64_to_f32_lossy(limit_high + (t * (limit_meme - limit_high)))
    }
}

// ── Signal Scorers ────────────────────────────────────────────────────────────

fn calculate_cv(values: &[u64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let n = f64::from(u32::try_from(values.len()).unwrap_or(1));
    let mean = values
        .iter()
        .map(|&v| f64::from(u32::try_from(v).unwrap_or(u32::MAX)))
        .sum::<f64>()
        / n;
    if mean <= 0.0 {
        return Some(0.0);
    }
    let var = values
        .iter()
        .map(|&v| (f64::from(u32::try_from(v).unwrap_or(u32::MAX)) - mean).powi(2))
        .sum::<f64>()
        / n;
    Some(var.sqrt() / mean)
}

fn calculate_cv_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let n = f64::from(u32::try_from(values.len()).unwrap_or(1));
    let mean = values.iter().sum::<f64>() / n;
    if mean <= 0.0 {
        return Some(0.0);
    }
    let var = values.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n;
    Some(var.sqrt() / mean)
}

fn calculate_gini_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = f64::from(u32::try_from(sorted.len()).unwrap_or(1));
    let sum: f64 = sorted.iter().sum();
    if sum.abs() < 1e-9 {
        return Some(0.0);
    }
    let weighted_sum: f64 = sorted
        .iter()
        .enumerate()
        .map(|(i, &v)| f64::from(u32::try_from(2 * (i + 1)).unwrap_or(1)) * v)
        .sum();
    Some((weighted_sum / (n * sum)) - (n + 1.0) / n)
}

fn fps_anomaly_score(fps: f64) -> f64 {
    // Returns high score when fps is far from standard rates → atypical → possible loop artifact
    let std_rates = [24.0, 25.0, 30.0, 60.0, 120.0];
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
        let json_str = include_str!("meme_keywords.json");
        let languages: HashMap<String, Vec<String>> =
            serde_json::from_str(json_str).unwrap_or_default();
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
pub fn score_directory_context(parts: Option<&[String]>, keywords: &[String]) -> f64 {
    let Some(parts) = parts else {
        return 0.5;
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
    0.5
}

/// Returns `0.5` (Ambiguous) if no name is provided.
///
/// # Errors
/// This function does not typically return `Result`, but uses `0.5` as a neutral score.
pub fn analyze_filename(name: Option<&str>, keywords: &[String]) -> FilenameAnalysis {
    let Some(name) = name else {
        return FilenameAnalysis {
            raw: 0.5,
            kind: FilenameKind::Ambiguous,
        };
    };
    let stem = name
        .rsplit_once('.')
        .map_or(name, |(s, _)| s)
        .to_lowercase();

    let global_keywords = get_meme_keywords();

    // 1. Dynamic Keyword Match from Database & JSON config
    if keywords.iter().any(|keyword| stem.contains(keyword))
        || global_keywords.iter().any(|keyword| stem.contains(keyword))
    {
        return FilenameAnalysis {
            raw: 0.85,
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
    if stem.chars().all(|c| c.is_alphanumeric()) && stem.len() >= 20 {
        return FilenameAnalysis {
            raw: 0.70,
            kind: FilenameKind::MachineGenerated,
        };
    }

    FilenameAnalysis {
        raw: 0.5,
        kind: FilenameKind::Ambiguous,
    }
}

pub fn score_loop_frequency(duration_secs: f64, frame_count: u64) -> f64 {
    if duration_secs <= 0.01 || frame_count == 0 {
        return 0.5;
    }
    let loops_per_minute = 60.0 / duration_secs;
    let frame_density = f64::from(u32::try_from(frame_count).unwrap_or(u32::MAX)) / duration_secs;

    let loop_score = if loops_per_minute >= 20.0 {
        1.0
    } else if loops_per_minute >= 10.0 {
        0.8
    } else if loops_per_minute >= 5.0 {
        0.6
    } else if loops_per_minute >= 2.0 {
        0.4
    } else {
        0.2
    };

    let density_adj = if frame_density < 1.2 {
        -0.35
    } else if frame_density < 3.0 {
        -0.20
    } else if frame_density < 6.0 {
        -0.08
    } else {
        0.0
    };

    let combined_score: f64 = loop_score + density_adj;
    combined_score.clamp(0.0_f64, 1.0_f64)
}

pub fn score_sparse_cadence(duration_secs: f64, frame_count: u64) -> f64 {
    if duration_secs <= 0.01 || frame_count <= 1 {
        return 0.5;
    }
    let frame_density =
        f64::from(u32::try_from(frame_count).unwrap_or(u32::MAX)) / duration_secs.max(0.01);
    let avg_gap = duration_secs / f64::from(u32::try_from(frame_count).unwrap_or(u32::MAX));

    if duration_secs <= 1.5 && frame_density >= 12.0 {
        return 0.98;
    }
    if duration_secs >= 1.5 && avg_gap >= 0.25 {
        return 0.92;
    }
    if duration_secs >= 4.0 && frame_count <= 12 && avg_gap >= 0.5 {
        return 0.95;
    }

    0.5
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
    if meta.width > 0 && meta.height > 0 {
        if meta.width == meta.height {
            nudge.apply(0.05, "1:1 aspect ratio");
        } else if ((f64::from(meta.width) / f64::from(meta.height)) - 1.777).abs() < 0.05 {
            nudge.apply(-0.05, "16:9 cinematic ratio");
        }
    }

    if (meta.width * meta.height) > (1920 * 1080) {
        nudge.apply(-0.08, "4K+ resolution");
    }

    // ── Tier 2: Low-Cost Bitstream ──
    if detect_scene_cut(&meta.pkt_sizes) {
        nudge.apply(-0.08, "Scene cut detected");
    }

    if detect_localized_motion(&meta.mv_magnitudes) {
        nudge.apply(0.05, "Localized motion");
    }

    // Clamp total nudge to [-0.15, +0.15]
    nudge.score = nudge.score.clamp(-0.15, 0.15);
    nudge
}

/// Detect hard scene cuts in packet size stream.
/// If any inner frame is 5x larger than the median inner packet size,
/// it's likely an `I-frame` scene cut.
fn detect_scene_cut(pkt_sizes: &[u64]) -> bool {
    if pkt_sizes.len() < 5 {
        return false;
    }
    let inner = &pkt_sizes[1..pkt_sizes.len() - 1];
    let mut baseline = inner.to_vec();
    baseline.sort_unstable();
    let median = f64::from(u32::try_from(baseline[baseline.len() / 2]).unwrap_or(u32::MAX));

    if median <= 0.0 {
        return false;
    }

    inner
        .iter()
        .any(|&size| (f64::from(u32::try_from(size).unwrap_or(u32::MAX))) > median * 5.0)
}

/// Detect localized motion (high concentration of motion in small area).
/// Returns true if motion vectors suggest synthetic/sticker content.
fn detect_localized_motion(mvs: &[f64]) -> bool {
    mvs.len() >= 10 && zero_motion_ratio(mvs) > 0.7
}

/// Extract first frame from video to temporary `PNG` for analysis.
fn extract_frame_to_temp(path: &Path) -> Option<std::path::PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Generate unique filename: timestamp + random seed
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let rand_seed = std::process::id() ^ (u32::try_from(timestamp).unwrap_or(u32::MAX));

    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("mfb_frame_{timestamp:x}_{rand_seed:x}.png"));

    let output = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .frames_v(1)
        .format("image2")
        .overwrite()
        .output(&temp_path)
        .build()
        .output()
        .ok()?;

    if output.status.success() && temp_path.exists() {
        Some(temp_path)
    } else {
        None
    }
}

fn detect_heavy_letterboxing_from_image(img: &image::DynamicImage) -> bool {
    let (_w, h) = img.dimensions();
    if h < 100 {
        return false;
    }
    let top_band = crate::numeric_cast::f64_to_u32_sat(f64::from(h) * 0.15);
    let bottom_start = h - top_band;
    let top_var = calculate_band_variance(img, 0, top_band);
    let bottom_var = calculate_band_variance(img, bottom_start, h);
    top_var < 100.0 && bottom_var < 100.0
}

/// Calculate pixel variance in a horizontal band.
fn calculate_band_variance(img: &image::DynamicImage, y_start: u32, y_end: u32) -> f64 {
    use image::GenericImageView;
    let (w, _) = img.dimensions();
    let mut values = Vec::new();

    for y in y_start..y_end.min(img.height()) {
        for x in 0..w.min(img.width()) {
            let pixel = img.get_pixel(x, y);
            let gray = f64::from(pixel[0]) * 0.299
                + f64::from(pixel[1]) * 0.587
                + f64::from(pixel[2]) * 0.114;
            values.push(gray);
        }
    }

    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / f64::from(u32::try_from(values.len()).unwrap_or(1));
    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
        / f64::from(u32::try_from(values.len()).unwrap_or(1))
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

            if (center - right).abs() > 80 || (center - bottom).abs() > 80 {
                edge_count += 1;
            }
        }
    }

    let edge_ratio = f64::from(u32::try_from(edge_count).unwrap_or(u32::MAX)) / total_pixels;
    edge_ratio > 0.15
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

fn sampled_webp_compression_ratio_from_image(img: &image::DynamicImage) -> Option<f64> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }

    // Only sample if image isn't too large to avoid performance hit
    let (target_w, target_h) = if w > WEBP_RATIO_SAMPLE_MAX_DIM || h > WEBP_RATIO_SAMPLE_MAX_DIM {
        let ratio = f64::from(w) / f64::from(h);
        if ratio > 1.0 {
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
        .ok()?;

    let webp_size = f64::from(u32::try_from(buffer.get_ref().len()).unwrap_or(u32::MAX));

    if webp_size <= 0.0 {
        return None;
    }
    Some(raw_size / webp_size)
}

/// Check if a file path should use the `GIF` fast-path (`from_gif_path`) instead of `ffprobe`.
#[must_use]
pub fn should_use_gif_fast_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase()),
        Some(ext) if ext == "gif"
    )
}

/// Performs deep signal extraction (Palette, `YDIF`, Block Skew) using `FFmpeg` benchmarks.
///
/// # Errors
/// Returns an error if the `FFmpeg` command fails or the output cannot be parsed.
pub fn deep_refine_meta(meta: &mut LoopMeta, path: &std::path::Path) -> anyhow::Result<()> {
    // 1. Extract Temporal Flatness (YDIF)
    let output = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .arg("-vf")
        .arg("signalstats,metadata=print")
        .format("null")
        .output_null()
        .build()
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut ydif_values = Vec::new();
    for line in stderr.lines() {
        if let Some(idx) = line.find("lavfi.signalstats.YDIF=") {
            if let Ok(val) = line[idx + 23..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .parse::<f64>()
            {
                ydif_values.push(val);
            }
        }
    }
    if !ydif_values.is_empty() {
        meta.temporal_flatness = Some(temporal_flatness_score(&ydif_values));
    }

    // 2. Extract Palette Depth
    let thumb_output = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .frames_v(1)
        .arg("-vf")
        .arg("scale=64:64")
        .format("rawvideo")
        .pix_fmt(crate::ffmpeg_builder::PixFmt::Rgb24)
        .output_null()
        .build()
        .output()?;

    if thumb_output.status.success() && thumb_output.stdout.len() >= 64 * 64 * 3 {
        let mut quantized = std::collections::HashSet::new();
        for chunk in thumb_output.stdout.chunks_exact(3) {
            let r = chunk[0] >> 3;
            let g = chunk[1] >> 3;
            let b = chunk[2] >> 3;
            quantized.insert((r, g, b));
        }
        meta.palette_depth = Some(palette_depth_score(quantized.len()));
    }
    Ok(())
}

fn temporal_flatness_score(ydif_values: &[f64]) -> f64 {
    if ydif_values.is_empty() {
        return 0.5;
    }
    let n = f64::from(u32::try_from(ydif_values.len()).unwrap_or(1));
    let mean = ydif_values.iter().sum::<f64>() / n;
    if mean < 1e-6 {
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
    let count = f64::from(u32::try_from(quantized_unique_colors).unwrap_or(u32::MAX));
    let max_possible = 32_f64.powi(3);
    let score = 1.0 - (count.ln() / max_possible.ln()).min(1.0);
    score.clamp(0.0, 1.0)
}

fn loop_closure_score(pkt_sizes: &[u64]) -> Option<f64> {
    if pkt_sizes.len() < 4 {
        return None;
    }

    let vals: Vec<f64> = pkt_sizes
        .iter()
        .map(|&v| f64::from(u32::try_from(v).unwrap_or(u32::MAX)))
        .collect();
    let n = vals.len();
    let mean = vals.iter().sum::<f64>() / f64::from(u32::try_from(n).unwrap_or(1));
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>()
        / f64::from(u32::try_from(n).unwrap_or(1));
    if variance < 1e-6 {
        // All frames identical — perfect loop structure
        return Some(1.0);
    }

    // Normalized autocorrelation at lag = half sequence length.
    // A looping sequence has high self-similarity between its first and second half.
    let lag = n / 2;
    let autocorr: f64 = (0..n - lag)
        .map(|i| (vals[i] - mean) * (vals[i + lag] - mean))
        .sum::<f64>()
        / (f64::from(u32::try_from(n - lag).unwrap_or(1)) * variance);

    // Map [-1, 1] → [0, 1]; high positive autocorrelation = strong loop closure
    Some(f64::midpoint(autocorr, 1.0).clamp(0.0, 1.0))
}

fn motion_periodicity_score(mv_magnitudes: &[f64]) -> Option<f64> {
    let n = mv_magnitudes.len();
    if n < 6 {
        return None;
    }

    let mean = mv_magnitudes.iter().sum::<f64>() / f64::from(u32::try_from(n).unwrap_or(1));
    let variance = mv_magnitudes
        .iter()
        .map(|&v| (v - mean).powi(2))
        .sum::<f64>()
        / f64::from(u32::try_from(n).unwrap_or(1));
    if variance < 1e-6 {
        return Some(1.0); // Perfectly static — synthetic/sticker content
    }

    // Average normalized autocorrelation over lags n/4, n/3, n/2.
    // A periodic (looping) sequence scores high across multiple lags.
    let lags = [n / 4, n / 3, n / 2];
    let autocorr_sum: f64 = lags
        .iter()
        .filter(|&&lag| lag > 0 && lag < n)
        .map(|&lag| {
            let r: f64 = (0..n - lag)
                .map(|i| (mv_magnitudes[i] - mean) * (mv_magnitudes[i + lag] - mean))
                .sum::<f64>()
                / (f64::from(u32::try_from(n - lag).unwrap_or(1)) * variance);
            r.clamp(-1.0, 1.0)
        })
        .sum();
    let valid_lags = lags.iter().filter(|&&lag| lag > 0 && lag < n).count();

    Some(
        f64::midpoint(
            autocorr_sum / f64::from(u32::try_from(valid_lags).unwrap_or(1)),
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

    let mean = pts_deltas.iter().sum::<f64>() / f64::from(u32::try_from(n).unwrap_or(1));
    let variance = pts_deltas.iter().map(|&v| (v - mean).powi(2)).sum::<f64>()
        / f64::from(u32::try_from(n).unwrap_or(1));
    if variance < 1e-12 {
        return Some(1.0); // Perfectly uniform frame timing
    }

    // Lag-1 autocorrelation: measures rhythmic regularity of frame intervals.
    // A looping animation has consistent, self-similar inter-frame timing.
    let lag1: f64 = (0..n - 1)
        .map(|i| (pts_deltas[i] - mean) * (pts_deltas[i + 1] - mean))
        .sum::<f64>()
        / (f64::from(u32::try_from(n - 1).unwrap_or(1)) * variance);

    Some(f64::midpoint(lag1.clamp(-1.0, 1.0), 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{DistributionStats, LoopReferenceProfile};

    fn distribution(
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

    fn base_profile() -> LoopReferenceProfile {
        let collection = crate::database::GlobalCollectionStats {
            duration_p90: 16.0,
            ..Default::default()
        };

        LoopReferenceProfile {
            duration: distribution(8.0, 4.0, 1.0, 2.5, 6.0, 12.0, 16.0),
            fps: distribution(12.0, 6.0, 4.0, 8.0, 12.0, 18.0, 24.0),
            frame_density: distribution(12.0, 6.0, 4.0, 8.0, 12.0, 18.0, 24.0),
            file_size_bytes: distribution(
                1_800_000.0,
                1_200_000.0,
                120_000.0,
                450_000.0,
                1_200_000.0,
                3_000_000.0,
                7_000_000.0,
            ),
            pixels: distribution(
                300_000.0,
                500_000.0,
                64_000.0,
                160_000.0,
                262_144.0,
                640_000.0,
                2_073_600.0,
            ),
            delay_variation: distribution(0.24, 0.12, 0.05, 0.12, 0.22, 0.32, 0.48),
            webp_ratio: distribution(11.0, 4.0, 4.0, 7.0, 10.0, 13.0, 16.0),
            motion_gini: distribution(0.55, 0.16, 0.25, 0.40, 0.55, 0.70, 0.84),
            palette_depth: distribution(0.55, 0.16, 0.25, 0.40, 0.55, 0.70, 0.84),
            temporal_flatness: distribution(0.55, 0.16, 0.25, 0.40, 0.55, 0.70, 0.84),
            collection,
            top_keywords: vec![
                "meme".to_string(),
                "reaction".to_string(),
                "sticker".to_string(),
            ],
            ..Default::default()
        }
    }

    fn base_meta() -> LoopMeta {
        LoopMeta {
            duration_secs: 8.0,
            width: 640,
            height: 640,
            fps: 12.0,
            frame_count: 96,
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

    fn verdict_with_profile(meta: &LoopMeta, profile: &LoopReferenceProfile) -> LoopIntentVerdict {
        // Hidden Layer 1 overrides are opt-in for tree tests; set to "0" to bypass global defaults.
        std::env::set_var(crate::constants::ENV_FORCE_SHORT_GIFS, "0");
        std::env::set_var(crate::constants::ENV_INTERCEPT_LONG_SILENT, "0");
        evaluate_loop_tree(meta, Some(profile)).verdict
    }

    #[test]
    fn duration_override_beats_large_resolution_and_size() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = 2.0;
        meta.width = 3840;
        meta.height = 2160;
        meta.fps = 60.0;
        meta.file_size_bytes = 30_000_000;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(matches!(verdict, LoopIntentVerdict::LoopStrong(_)));
        assert!(verdict.reason().contains("Layer 1-B"));
    }

    #[test]
    fn layer_1_b2_sticker_class_native_gif_above_short_duration_cutoff() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.is_native_gif = true;
        meta.has_audio = false;
        meta.duration_secs = 4.0;
        meta.width = 150;
        meta.height = 108;
        meta.frame_count = 40;
        meta.file_size_bytes = 24_000;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(matches!(verdict, LoopIntentVerdict::LoopStrong(_)));
        assert!(
            verdict.reason().contains("Layer 1-B2"),
            "expected Layer 1-B2 sticker-class GIF prior, got {}",
            verdict.reason()
        );
    }

    #[test]
    fn layer_1_b2_does_not_apply_to_large_pixel_gif() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.is_native_gif = true;
        meta.has_audio = false;
        meta.duration_secs = 4.0;
        meta.width = 500;
        meta.height = 500;
        meta.frame_count = 40;
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
        meta.duration_secs = 1.5;
        meta.has_audio = true;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(matches!(verdict, LoopIntentVerdict::LoopWeak(_)));
        assert!(verdict.reason().contains("Layer 1-A"));
    }

    #[test]
    fn short_fast_silent_media_scores_loopstrong() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = 4.0;
        meta.fps = 24.0;
        meta.frame_count = 96;
        meta.width = 320;
        meta.height = 320;
        meta.file_size_bytes = 240_000;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, LoopIntentVerdict::LoopStrong(_)),
            "expected loop-strong, got {verdict:?}"
        );
        assert!(verdict.reason().contains("Layer 4"));
    }

    #[test]
    fn long_slow_scene_cut_media_scores_loopweak() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.duration_secs = 28.0;
        meta.fps = 4.0;
        meta.frame_count = 112;
        meta.file_size_bytes = 12_000_000;
        meta.width = 1920;
        meta.height = 1080;
        meta.pkt_sizes = vec![120, 130, 1400, 150, 120, 125];
        meta.webp_compression_ratio = Some(3.0);
        meta.motion_gini = Some(0.18);
        meta.palette_depth = Some(0.20);
        meta.temporal_flatness = Some(0.18);

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, LoopIntentVerdict::LoopWeak(_)),
            "expected loop-weak, got {verdict:?}"
        );
        assert!(verdict.reason().contains("Layer 4"));
    }

    #[test]
    fn play_once_and_master_signals_push_borderline_media_weak() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.loop_count = Some(1);
        meta.duration_secs = 11.0;
        meta.fps = 8.0;
        meta.frame_count = 88;
        meta.file_size_bytes = 3_800_000;
        meta.webp_compression_ratio = Some(3.5);
        meta.has_complex_color_profile = true;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, LoopIntentVerdict::LoopWeak(_)),
            "expected loop-weak, got {verdict:?}"
        );
    }

    #[test]
    fn platform_transparency_and_looping_signals_push_borderline_media_strong() {
        let profile = base_profile();
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.duration_secs = 7.0;
        meta.fps = 10.0;
        meta.frame_count = 70;
        meta.file_size_bytes = 500_000;
        meta.has_transparency = true;
        meta.app_extensions = Some(vec!["GIPHY".to_string()]);
        meta.loop_count = Some(0);
        meta.motion_gini = Some(0.82);
        meta.palette_depth = Some(0.82);
        meta.temporal_flatness = Some(0.80);
        meta.directory_loop_intent_score = 1.0;
        meta.filename_loop_intent_score = 1.0;

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, LoopIntentVerdict::LoopStrong(_)),
            "expected loop-strong, got {verdict:?}"
        );
    }

    #[test]
    fn balanced_case_stays_uncertain() {
        let profile = base_profile();
        let meta = base_meta();

        let verdict = verdict_with_profile(&meta, &profile);
        assert!(
            matches!(verdict, LoopIntentVerdict::Uncertain(_)),
            "expected uncertain, got {verdict:?}"
        );
        assert!(verdict.reason().contains("defer to KNN"));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_score_directory_context() {
        let directory_score = score_directory_context(
            Some(&["Downloads".to_string(), "ReactionPacks".to_string()]),
            &[],
        );
        assert!((directory_score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_analyze_filename_with_keywords() {
        // Test Chinese keyword (from JSON)
        let analysis_zh = analyze_filename(Some("gif表情 (379).gif"), &[]);
        assert_eq!(analysis_zh.raw, 0.85);
        assert_eq!(analysis_zh.kind, FilenameKind::HumanSemantic);

        // Test English keyword (from JSON)
        let analysis_en = analyze_filename(Some("my_funny_meme.webp"), &[]);
        assert_eq!(analysis_en.raw, 0.85);

        // Test Korean keyword (from JSON)
        let analysis_ko = analyze_filename(Some("cute_sticker_움짤.avif"), &[]);
        assert_eq!(analysis_ko.raw, 0.85);

        // Test non-meme filename
        let analysis_none = analyze_filename(Some("vacation_photo.jpg"), &[]);
        assert_eq!(analysis_none.raw, 0.5);
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
        profile.collection.duration_p90 = 15.0;

        let thresholds = LoopThresholds::from_profile(Some(&profile));
        assert!(thresholds.duration_override_secs > 2.0);
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
    fn layer6_accepts_short_gif_when_score_is_high_and_confidence_is_near_threshold() {
        let profile = base_profile();
        let thresholds = LoopThresholds::from_profile(Some(&profile));
        let mut meta = base_meta();
        meta.source_extension = Some("gif".to_string());
        meta.container = Some("gif".to_string());
        meta.duration_secs = 5.0;
        meta.has_audio = false;

        assert!(should_accept_layer6_loopstrong(
            &meta,
            &thresholds,
            0.82,
            0.72,
            0.71,
        ));
    }

    #[test]
    fn layer6_relaxes_for_silent_clips_up_to_core_short_asset_window() {
        let profile = base_profile();
        let thresholds = LoopThresholds::from_profile(Some(&profile));
        let mut meta = base_meta();
        meta.source_extension = Some("mp4".to_string());
        meta.container = Some("mp4".to_string());
        meta.duration_secs = 9.5;
        meta.has_audio = false;

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
        let thresholds = LoopThresholds::from_profile(Some(&profile));
        let mut meta = base_meta();
        meta.duration_secs = 24.0;
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
        meta.duration_secs = 5.0;

        std::env::set_var(crate::constants::ENV_FORCE_SHORT_GIFS, "0");
        std::env::set_var(crate::constants::ENV_INTERCEPT_LONG_SILENT, "0");
        let default_verdict = evaluate_loop_tree(&meta, Some(&profile)).verdict;
        assert!(
            !default_verdict.reason().contains("Layer 1-C"),
            "default verdict should not use hidden Layer 1-C: {default_verdict:?}"
        );

        std::env::set_var(crate::constants::ENV_FORCE_SHORT_GIFS, "1");
        let dev_short_verdict = evaluate_loop_tree(&meta, Some(&profile)).verdict;
        std::env::remove_var(crate::constants::ENV_FORCE_SHORT_GIFS);
        assert!(
            dev_short_verdict.reason().contains("Layer 1-C"),
            "developer override should enable hidden Layer 1-C: {dev_short_verdict:?}"
        );

        meta.duration_secs = 18.0;
        std::env::set_var(crate::constants::ENV_INTERCEPT_LONG_SILENT, "0");
        let default_long_verdict = evaluate_loop_tree(&meta, Some(&profile)).verdict;
        assert!(
            !default_long_verdict.reason().contains("Layer 1-D"),
            "default verdict should not use hidden Layer 1-D: {default_long_verdict:?}"
        );

        std::env::set_var(crate::constants::ENV_INTERCEPT_LONG_SILENT, "1");
        let dev_long_verdict = evaluate_loop_tree(&meta, Some(&profile)).verdict;
        std::env::remove_var(crate::constants::ENV_INTERCEPT_LONG_SILENT);
        assert!(
            dev_long_verdict.reason().contains("Layer 1-D"),
            "developer override should enable hidden Layer 1-D: {dev_long_verdict:?}"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_analyze_filename_variants() {
        let analysis_en = analyze_filename(Some("my_funny_meme.webp"), &[]);
        assert_eq!(analysis_en.raw, 0.85);

        // Test Korean keyword (from JSON)
        let analysis_ko = analyze_filename(Some("cute_sticker_움짤.avif"), &[]);
        assert_eq!(analysis_ko.raw, 0.85);

        // Test non-meme filename
        let analysis_none = analyze_filename(Some("vacation_photo.jpg"), &[]);
        assert_eq!(analysis_none.raw, 0.5);
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
}
