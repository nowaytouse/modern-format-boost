//! 「循环意图判断系统」 (Loop Intent Identification System)
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

use crate::file_copier::{SUPPORTED_IMAGE_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS};
use crate::progress_mode::emit_stderr;
use crate::video_detection::ColorSpace;
use crate::video_detection::VideoDetectionResult;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, GenericImageView};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::constants::MODERN_ANIMATED_EXTENSIONS;
const GIPHY_PLATFORM_MARKERS: &[&str] =
    &["GIPHY", "TENOR", "STICKER", "TELEGRAM", "TIKTOK", "DISCORD"];
const WEBP_RATIO_SAMPLE_MAX_DIM: u32 = 256;

// ── Output: 三态输出 ──────────────────────────────────────────────────────────

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
/// Populated by constructors (from_video_detection, from_ffprobe_result, from_gif_path).
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

    // ── Layer 2 signals (explicit declarations) ──
    /// 0 = infinite loop, 1 = play once, None = unknown
    pub loop_count: Option<u16>,
    /// e.g. ["GIPHY", "NETSCAPE2.0", ...] from GIF Application Extension block
    pub app_extensions: Option<Vec<String>>,
    /// "webm", "mp4", "gif", etc.
    pub container: Option<String>,

    // ── Layer 3 signals (self-referential structure) ──
    /// frame_payload_variation: coefficient of variation of frame packet sizes (pkt_sizes CV)
    pub frame_payload_variation: Option<f64>,
    /// frame_delay_variation: CV of presentation timestamps deltas
    pub frame_delay_variation: Option<f64>,
    /// Raw frame packet sizes — used to compute closure_ratio
    pub pkt_sizes: Vec<u64>,
    /// Raw PTS deltas — used for interval consistency score
    pub pts_deltas: Vec<f64>,

    // ── Layer 4 signals (content features) ──
    pub palette_size: Option<u32>,
    /// WebP compression ratio proxy: raw_size / webp_size for a sampled frame.
    /// Constructors populate this on a best-effort basis for image-like sources.
    pub webp_compression_ratio: Option<f64>,
    pub palette_depth: Option<f64>,
    pub motion_gini: Option<f64>,
    pub temporal_flatness: Option<f64>,
    pub block_skew: Option<f64>,

    // ── Layer 5 signals (context semantics) ──
    pub directory_meme_score: f64,
    pub filename_meme_score: f64,

    // ── Color Profile signals ──
    pub has_embedded_icc: bool,
    pub has_complex_color_profile: bool,

    // ── Auxiliary (used in KNN bridge) ──
    pub frame_types: Vec<char>,
    pub mv_magnitudes: Vec<f64>,
    pub cached_frame_png: Option<Vec<u8>>,
}

impl LoopMeta {
    /// Build LoopMeta from a full VideoDetectionResult.
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
            loop_count: detection.loop_count,
            app_extensions: Some(Vec::new()),
            container: None,
            has_embedded_icc: false, // Video containers rarely have ICC in this context
            has_complex_color_profile: matches!(
                detection.color_space,
                ColorSpace::BT2020 | ColorSpace::AdobeRGB
            ) || detection.is_dolby_vision
                || detection.is_hdr10_plus,
            frame_payload_variation: Some(calculate_cv(&detection.pkt_sizes)),
            frame_delay_variation: Some(calculate_cv_f64(&detection.pts_deltas)),
            pkt_sizes: detection.pkt_sizes.clone(),
            pts_deltas: detection.pts_deltas.clone(),
            palette_size,
            webp_compression_ratio: None,
            palette_depth: None,
            motion_gini: Some(calculate_gini_f64(&detection.mv_magnitudes)),
            temporal_flatness: None,
            block_skew: None,
            directory_meme_score: 0.5,
            filename_meme_score: 0.5,
            frame_types: detection.frame_types.clone(),
            mv_magnitudes: detection.mv_magnitudes.clone(),
            cached_frame_png: None,
        };
        meta.directory_meme_score = score_directory_context(parent_directories.as_deref());
        meta.filename_meme_score = analyze_filename(meta.file_name.as_deref());
        meta.populate_webp_compression_ratio_from_path(file_path);
        meta
    }

    /// Build LoopMeta from an FFprobeResult (used in pipelines without full detection).
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
            loop_count: probe.loop_count,
            app_extensions: Some(Vec::new()),
            container: None,
            frame_payload_variation: Some(calculate_cv(&probe.pkt_sizes)),
            frame_delay_variation: Some(calculate_cv_f64(&probe.pts_deltas)),
            pkt_sizes: probe.pkt_sizes.clone(),
            pts_deltas: probe.pts_deltas.clone(),
            palette_size,
            webp_compression_ratio: None,
            palette_depth: None,
            motion_gini: Some(calculate_gini_f64(&probe.mv_magnitudes)),
            temporal_flatness: None,
            block_skew: None,
            directory_meme_score: 0.5,
            filename_meme_score: 0.5,
            frame_types: probe.frame_types.clone(),
            mv_magnitudes: probe.mv_magnitudes.clone(),
            has_embedded_icc: false,
            has_complex_color_profile: false,
            cached_frame_png: None,
        };
        meta.directory_meme_score = score_directory_context(parent_directories.as_deref());
        meta.filename_meme_score = analyze_filename(meta.file_name.as_deref());
        meta.populate_webp_compression_ratio_from_path(path);
        meta
    }

    /// Build LoopMeta from a GIF file using header-level scanning (fast, no ffprobe).
    pub fn from_gif_path(path: &Path) -> Option<Self> {
        let (pal, exts, has_transparency, variation, delay_variation, loops, total_dur) =
            crate::useless::gif_meme_score::scan_gif_headers(path).ok()?;

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

        // Estimate frame_count from duration and frame_delay_variation.
        // For GIFs without delay info, use 1 frame as conservative default.
        // For animated GIFs with estimated duration, infer count from typical delay (~100ms).
        let frame_count: u32 = if let Some(dur) = total_dur {
            if dur > 0.01 {
                // If we have a realistic duration, estimate frames from delay variation.
                // delay_variation being Some indicates frames were scanned.
                if delay_variation.is_some() {
                    // Conservative: assume average 100ms delay per frame
                    ((dur * 10.0).ceil() as u32).min(10000)
                } else {
                    1
                }
            } else {
                1
            }
        } else {
            1
        };

        let mut meta = Self {
            duration_secs: total_dur.unwrap_or(0.0),
            width,
            height,
            fps: if frame_count > 1 && total_dur.is_some() && total_dur.unwrap() > 0.0 {
                frame_count as f64 / total_dur.unwrap()
            } else {
                12.0  // Conservative estimate for header-only path
            },
            frame_count: frame_count as u64,
            file_size_bytes: file_size,
            file_name,
            source_extension: Some("gif".to_string()),
            parent_directories,
            has_audio: false,
            has_transparency,
            loop_count: loops,
            app_extensions: exts,
            container: Some("gif".to_string()),
            frame_payload_variation: variation,
            frame_delay_variation: delay_variation,
            palette_size: pal,
            ..Default::default()
        };

        meta.directory_meme_score = score_directory_context(meta.parent_directories.as_deref());
        meta.filename_meme_score = analyze_filename(meta.file_name.as_deref());
        meta.populate_webp_compression_ratio_from_path(path);
        Some(meta)
    }

    fn populate_webp_compression_ratio_from_path(&mut self, path: &Path) {
        if self.webp_compression_ratio.is_some() {
            return;
        }

        if should_sample_webp_compression_ratio(self.source_extension.as_deref()) {
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
                        self.webp_compression_ratio = sampled_webp_compression_ratio_from_image(&img);
                    }
                }
            }
        }
    }

    /// Bridge: convert LoopMeta to the legacy GifMeta struct for KNN lookup.
    pub fn to_legacy_gif_meta(&self) -> crate::useless::gif_meme_score::GifMeta {
        crate::useless::gif_meme_score::GifMeta {
            duration_secs: self.duration_secs,
            width: self.width,
            height: self.height,
            fps: self.fps,
            frame_count: self.frame_count,
            file_size_bytes: self.file_size_bytes,
            file_name: self.file_name.clone(),
            palette_size: self.palette_size,
            app_extensions: self.app_extensions.clone(),
            has_transparency: self.has_transparency,
            frame_payload_variation: self.frame_payload_variation,
            frame_delay_variation: self.frame_delay_variation,
            source_extension: self.source_extension.clone(),
            parent_directories: self.parent_directories.clone(),
            has_embedded_icc: self.has_embedded_icc,
            has_complex_color_profile: self.has_complex_color_profile,
            loop_count: self.loop_count,
            has_audio: self.has_audio,
            frame_types: self.frame_types.clone(),
            pts_deltas: self.pts_deltas.clone(),
            mv_magnitudes: self.mv_magnitudes.clone(),
            palette_depth: self.palette_depth,
            motion_gini: self.motion_gini,
            block_skew: self.block_skew,
            temporal_flatness: self.temporal_flatness,
            pkt_sizes: self.pkt_sizes.clone(),
        }
    }
}

// ── 7-Layer Decision Tree ─────────────────────────────────────────────────────

/// The WeightedScore accumulator that flows through Layers 3–5.
/// Range: [-1.0, +1.0]. Positive = loop-strong bias, negative = video bias.
#[derive(Debug, Default, Clone, Copy)]
struct WeightedScore(f64);

impl WeightedScore {
    fn add(&mut self, delta: f64) {
        self.0 = (self.0 + delta).clamp(-1.0, 1.0);
    }

    fn value(self) -> f64 {
        self.0
    }

    /// Normalize to [0, 1] for fusion with KNN keep_probability.
    fn normalized(self) -> f64 {
        f64::midpoint(self.0, 1.0)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct DerivedLoopSignals {
    scene_cut: bool,
}

impl DerivedLoopSignals {
    fn from_meta(meta: &LoopMeta) -> Self {
        Self {
            scene_cut: detect_scene_cut(&meta.pkt_sizes),
            // localized_motion signal is extracted inline where needed (Layer 1-E)
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct HardPassVetoFlags {
    scene_cut: bool,
    low_compressibility: bool,
}

impl HardPassVetoFlags {
    fn from_meta(meta: &LoopMeta, derived: &DerivedLoopSignals) -> Self {
        Self {
            scene_cut: derived.scene_cut,
            low_compressibility: meta.webp_compression_ratio.is_some_and(|ratio| ratio < 5.0),
        }
    }

    fn any(self) -> bool {
        self.scene_cut || self.low_compressibility
    }
}

#[derive(Debug, Clone)]
struct TreeEvaluation {
    verdict: LoopIntentVerdict,
    weighted_score_normalized: f64,
}

fn has_platform_marker(app_extensions: Option<&[String]>) -> bool {
    let Some(app_extensions) = app_extensions else {
        return false;
    };
    app_extensions.iter().any(|app| {
        let normalized = app.trim().to_ascii_uppercase();
        GIPHY_PLATFORM_MARKERS
            .iter()
            .any(|marker| normalized.contains(marker))
    })
}

fn zero_motion_ratio(mvs: &[f64]) -> f64 {
    if mvs.is_empty() {
        return 0.0;
    }
    let zero_count = mvs.iter().filter(|&&v| v.abs() < 0.1).count();
    zero_count as f64 / mvs.len() as f64
}

fn is_near_16_by_9(width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    ((f64::from(width) / f64::from(height)) - (16.0 / 9.0)).abs() < 0.05
}

fn loop_count_zero_weight(duration_secs: f64) -> f64 {
    if duration_secs <= 18.0 {
        0.25
    } else if duration_secs <= 35.0 {
        let ratio = (duration_secs - 18.0) / (35.0 - 18.0);
        0.25 - (0.15 * ratio)
    } else {
        0.05
    }
}

fn apply_layer3_signals(meta: &LoopMeta, derived: &DerivedLoopSignals, score: &mut WeightedScore) {
    // 3-A: 首尾帧自参照闭合比
    if !derived.scene_cut && meta.pkt_sizes.len() >= 3 {
        let n = meta.pkt_sizes.len();
        let first = meta.pkt_sizes[0] as f64;
        let last = meta.pkt_sizes[n - 1] as f64;
        let avg = meta.pkt_sizes.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
        let inter_dist = meta.frame_payload_variation.unwrap_or(0.5);

        // When inter-frame variation itself is extreme, closure ratio stops being trustworthy.
        if avg > 1.0 && inter_dist > 0.01 && inter_dist <= 1.5 {
            let closure_dist = (first - last).abs() / avg;
            let closure_ratio = closure_dist / inter_dist.clamp(0.1, 2.0);

            if closure_ratio <= 1.2 {
                score.add(0.35);
            } else if closure_ratio > 2.5 {
                score.add(-0.35);
            }
        }
    }

    // 3-B: 节奏均匀性
    if let Some(delay_cv) = meta.frame_delay_variation {
        if delay_cv < 0.10 {
            score.add(0.20);
        } else if delay_cv < 0.25 {
            score.add(0.10);
        } else if delay_cv > 0.60 {
            score.add(-0.15);
        }
    }

    // 3-C: 场景切换检测
    if derived.scene_cut {
        score.add(-0.30);
    }

    // 3-D: 运动向量分布
    if let Some(motion_gini) = meta.motion_gini {
        if motion_gini >= 0.70 {
            score.add(0.15);
        } else if motion_gini <= 0.35 {
            score.add(-0.15);
        }
    }

    if meta.mv_magnitudes.len() >= 10 && zero_motion_ratio(&meta.mv_magnitudes) > 0.70 {
        score.add(0.10);
    }

    // 3-E: loop_count == 0 仅作衰减加权信号
    if meta.loop_count == Some(0) {
        score.add(loop_count_zero_weight(meta.duration_secs));
    }
}

fn apply_layer4_signals(meta: &LoopMeta, score: &mut WeightedScore) {
    // 4-A: 帧内容可压缩性 (WebP 压缩比)
    if let Some(ratio) = meta.webp_compression_ratio {
        if ratio > 15.0 {
            score.add(0.20);
        } else if ratio < 5.0 {
            score.add(-0.25);
        }
    }

    // 4-B: 调色板大小
    if let Some(p_size) = meta.palette_size {
        if p_size <= 64 {
            score.add(0.20);
        } else if p_size > 128 {
            score.add(-0.15);
        }
    }

    // 4-C: compression_efficiency_score
    let ce = compression_efficiency_score(
        meta.file_size_bytes,
        meta.width,
        meta.height,
        meta.fps,
        meta.duration_secs,
    );
    if ce > 0.7 {
        score.add(0.10);
    } else if ce < 0.3 {
        score.add(-0.10);
    }
}

fn apply_layer5_signals(meta: &LoopMeta, score: &mut WeightedScore) {
    // 5-A: 目录 / 文件名语义
    if meta.directory_meme_score > 0.8 && meta.filename_meme_score > 0.8 {
        score.add(0.08);
    } else if meta.directory_meme_score > 0.8 || meta.filename_meme_score > 0.8 {
        score.add(0.04);
    }

    // 5-B: fps 异常
    if fps_anomaly_score(meta.fps) > 0.6 {
        score.add(0.04);
    }

    // 5-C: 总帧数
    if meta.frame_count > 0 {
        if meta.frame_count <= 8 {
            score.add(0.04);
        } else if meta.frame_count > 500 {
            score.add(-0.08);
        }
    }

    // 5-D: 宽高比
    if meta.width > 0 && meta.height > 0 {
        if meta.width == meta.height {
            score.add(0.03);
        } else if is_near_16_by_9(meta.width, meta.height) {
            score.add(-0.04);
        }
    }
}

/// Run the 7-layer decision tree and return a LoopIntentVerdict.
/// This function is pure: no I/O, no database calls.
/// KNN fallback (Layer 6) is performed in `assess_loop_intent_from_meta`.
#[must_use]
pub fn identify_loop_intent(meta: &LoopMeta) -> LoopIntentVerdict {
    evaluate_loop_tree(meta).verdict
}

fn evaluate_loop_tree(meta: &LoopMeta) -> TreeEvaluation {
    let derived_signals = DerivedLoopSignals::from_meta(meta);
    let mut score = WeightedScore::default();

    let ext_lower = meta
        .source_extension
        .as_deref()
        .unwrap_or("")
        .to_lowercase();
    let is_image = SUPPORTED_IMAGE_EXTENSIONS.contains(&ext_lower.as_str());
    let is_video = !is_image && SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str());
    let hard_pass_vetoes = HardPassVetoFlags::from_meta(meta, &derived_signals);

    let finalize = |verdict: LoopIntentVerdict, score: WeightedScore| TreeEvaluation {
        weighted_score_normalized: match verdict {
            LoopIntentVerdict::LoopStrong(_) => 1.0,
            LoopIntentVerdict::LoopWeak(_) => 0.0,
            LoopIntentVerdict::Uncertain(_) => score.normalized(),
        },
        verdict,
    };

    // ══════════════════════════════════════════════════════════════
    // LAYER 1: 格式物理硬约束 — forced exits, WeightedScore 不参与
    // ══════════════════════════════════════════════════════════════

    // 1-A: 有音轨？（仅对视频容器执行硬否决，绝不否决动态图片）
    if meta.has_audio && is_video {
        return finalize(
            LoopIntentVerdict::LoopWeak(
                "Layer 1-A: Hard Veto — Audio Track Present in Video Device".to_string(),
            ),
            score,
        );
    }

    // 1-B: 有透明通道且无音轨？视频透明度处理成本极高
    if meta.has_transparency && !meta.has_audio {
        return finalize(
            LoopIntentVerdict::LoopStrong(
                "Layer 1-B: Hard Pass — Transparency Channel Present".to_string(),
            ),
            score,
        );
    }

    // 1-C: 极短内容硬通行（带否决条件）
    if is_image && meta.duration_secs <= 10.0 {
        if !hard_pass_vetoes.any() {
            return finalize(
                LoopIntentVerdict::LoopStrong(
                    "Layer 1-C: Veto-Clean Hard Pass (image <=10s)".to_string(),
                ),
                score,
            );
        }
    }

    // 1-D: 小尺寸/小分辨率硬通行（带否决条件）
    if is_image && (meta.width > 0 && meta.height > 0) && (meta.width <= 512 && meta.height <= 512)
    {
        if !hard_pass_vetoes.any() {
            return finalize(
                LoopIntentVerdict::LoopStrong(
                    "Layer 1-D: Veto-Clean Hard Pass (<=512x512)".to_string(),
                ),
                score,
            );
        }
    }

    // ══════════════════════════════════════════════════════════════
    // LAYER 2: 显式自我声明 — direct exits
    // ══════════════════════════════════════════════════════════════

    // 2-A: 平台来源标记
    if has_platform_marker(meta.app_extensions.as_deref()) {
        return finalize(
            LoopIntentVerdict::LoopStrong("Layer 2-A: Explicit Platform Loop Marker".to_string()),
            score,
        );
    }

    // 2-B: WebM 无音轨
    let is_webm = meta
        .container
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case("webm"))
        || ext_lower == "webm";
    if is_webm && !meta.has_audio {
        return finalize(
            LoopIntentVerdict::LoopStrong(
                "Layer 2-B: Explicit WebM Loop Carrier (no audio)".to_string(),
            ),
            score,
        );
    }

    // 2-C: 明确不循环声明
    if meta.loop_count == Some(1) {
        return finalize(
            LoopIntentVerdict::LoopWeak("Layer 2-C: Explicit Play-Once Declaration".to_string()),
            score,
        );
    }

    // ══════════════════════════════════════════════════════════════
    // LAYER 3: 自参照结构信号 — WeightedScore 累积，层末有检查点
    // ══════════════════════════════════════════════════════════════
    apply_layer3_signals(meta, &derived_signals, &mut score);

    // Layer 3 checkpoint
    if score.value() >= 0.55 {
        return finalize(
            LoopIntentVerdict::LoopStrong(format!(
                "Layer 3 Checkpoint: WeightedScore={:.2} ≥ 0.55 (self-referential structure)",
                score.value()
            )),
            score,
        );
    }
    if score.value() <= -0.55 {
        return finalize(
            LoopIntentVerdict::LoopWeak(format!(
                "Layer 3 Checkpoint: WeightedScore={:.2} ≤ -0.55 (structure mismatch)",
                score.value()
            )),
            score,
        );
    }

    // ══════════════════════════════════════════════════════════════
    // LAYER 4: 内容特征信号 — 成本较高，继续累积
    // ══════════════════════════════════════════════════════════════
    apply_layer4_signals(meta, &mut score);

    // Layer 4 checkpoint
    if score.value() >= 0.55 {
        return finalize(
            LoopIntentVerdict::LoopStrong(format!(
                "Layer 4 Checkpoint: WeightedScore={:.2} ≥ 0.55 (content features)",
                score.value()
            )),
            score,
        );
    }
    if score.value() <= -0.55 {
        return finalize(
            LoopIntentVerdict::LoopWeak(format!(
                "Layer 4 Checkpoint: WeightedScore={:.2} ≤ -0.55 (content features)",
                score.value()
            )),
            score,
        );
    }

    // ══════════════════════════════════════════════════════════════
    // LAYER 5: 上下文语义信号 — 权重刻意压低，仅作辅助修正
    // ══════════════════════════════════════════════════════════════
    apply_layer5_signals(meta, &mut score);

    // No Layer 5 checkpoint (by design — Layer 5 is only auxiliary correction).

    // ══════════════════════════════════════════════════════════════
    // LAYER 6: KNN + WeightedScore 综合融合判断
    // ══════════════════════════════════════════════════════════════
    finalize(
        LoopIntentVerdict::Uncertain(format!(
            "Layer 6: Incomplete tree signal (WeightedScore={:.2}), proceeding to KNN fusion",
            score.value()
        )),
        score,
    )
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Execute the loop intent identification for a given detection result.
pub fn assess_loop_intent(detection: &VideoDetectionResult) -> LoopIntentVerdict {
    let meta = LoopMeta::from_video_detection(detection);
    assess_loop_intent_from_meta(&meta, Some(Path::new(&detection.file_path)))
}

/// Execute the loop intent identification for a given probe result.
pub fn assess_loop_intent_from_probe(
    probe: &crate::ffprobe::FFprobeResult,
    path: &Path,
) -> LoopIntentVerdict {
    let meta = LoopMeta::from_ffprobe_result(probe, path);
    assess_loop_intent_from_meta(&meta, Some(path))
}

/// Core entry point: runs the full tree including KNN Layer 6 and Layer 7 fallback.
///
/// - Runs `identify_loop_intent()` (pure, Layers 1–5 + early Layer 6 check).
/// - If result is `Uncertain`, invokes KNN via `gif_value_db::lookup_similar_samples`.
/// - If KNN is unavailable or confidence is low, falls back to Layer 7 conservatively.
pub fn assess_loop_intent_from_meta(meta: &LoopMeta, path: Option<&Path>) -> LoopIntentVerdict {
    let tree = evaluate_loop_tree(meta);
    let weighted_score_normalized = tree.weighted_score_normalized;

    match tree.verdict {
        v @ (LoopIntentVerdict::LoopStrong(_) | LoopIntentVerdict::LoopWeak(_)) => {
            // Layers 1–5 gave a definitive answer — trust it.
            v
        }
        LoopIntentVerdict::Uncertain(ref reason) => {
            emit_stderr(&format!("🔭 Tree uncertain [{weighted_score_normalized:.2}] — falling back to Layer 6 KNN..."));

            // ── Layer 6: KNN + WeightedScore fusion ─────────────────────────
            let legacy_meta = meta.to_legacy_gif_meta();
            let sample_match = crate::gif_value_db::lookup_similar_samples(&legacy_meta, path);

            if let Some(m) = sample_match {
                let keep_prob = m.keep_probability.unwrap_or(0.5);
                let confidence = m.confidence;

                // Calculate Tier 1 & 2 micro-nudges
                let nudges = calculate_micro_nudges(meta);

                // Initial fusion: final_score = KNN * 0.6 + WeightedScore_norm * 0.4 + nudges
                let mut final_score =
                    keep_prob * 0.6 + weighted_score_normalized * 0.4 + nudges.score;

                // Log nudge trace if any nudges were applied
                if !nudges.trace.is_empty() {
                    emit_stderr(&format!(
                        "   ⚖️  Micro-Nudges ({:+.2}): {}",
                        nudges.score,
                        nudges.trace.join(" | ")
                    ));
                }

                // Tier 3: High-cost visual checks (gated execution)
                // Only trigger if still uncertain (0.4-0.6) after Tier 1+2 nudges
                if final_score > 0.40 && final_score < 0.60 && confidence < 0.75 {
                    if let Some(p) = path {
                        emit_stderr(
                            "   🔍 Triggering high-cost visual heuristics (extreme uncertainty)...",
                        );
                        let mut tier3_nudge = AuxiliaryNudge::default();

                        // Try to reuse an in-memory cached frame (if populated earlier).
                        let mut img_opt: Option<image::DynamicImage> = None;
                        if let Some(bytes) = meta.cached_frame_png.as_ref() {
                            if let Ok(img) = image::load_from_memory(bytes) {
                                img_opt = Some(img);
                            }
                        } else {
                            // Extract a single frame once and reuse it for both heuristics.
                            if let Some(temp_frame) = extract_frame_to_temp(p) {
                                if let Ok(bytes) = std::fs::read(&temp_frame) {
                                    let _ = std::fs::remove_file(&temp_frame);
                                    if let Ok(img) = image::load_from_memory(&bytes) {
                                        img_opt = Some(img);
                                    }
                                }
                            }
                        }

                        if let Some(ref img) = img_opt {
                            if detect_heavy_letterboxing_from_image(img) {
                                tier3_nudge.apply(0.05, "Letterboxing detected");
                            }
                            if detect_high_text_density_from_image(img) {
                                tier3_nudge.apply(0.08, "High text density");
                            }
                        }

                        if !tier3_nudge.trace.is_empty() {
                            emit_stderr(&format!(
                                "   📊 Tier 3 Visual ({:+.2}): {}",
                                tier3_nudge.score,
                                tier3_nudge.trace.join(" | ")
                            ));
                            final_score += tier3_nudge.score.clamp(-0.15, 0.15);
                        }
                    }
                }

                if confidence > 0.75 && final_score > 0.6 {
                    return LoopIntentVerdict::LoopStrong(format!(
                        "Layer 6: KNN+Nudges score={:.2} (knn={:.2}, tree={:.2}, nudge={:+.2}, conf={:.2})",
                        final_score, keep_prob, weighted_score_normalized, nudges.score, confidence
                    ));
                }
                if confidence > 0.75 && final_score <= 0.4 {
                    return LoopIntentVerdict::LoopWeak(format!(
                        "Layer 6: KNN+Nudges score={:.2} (knn={:.2}, tree={:.2}, nudge={:+.2}, conf={:.2})",
                        final_score, keep_prob, weighted_score_normalized, nudges.score, confidence
                    ));
                }
                // confidence ≤ 0.75 or 0.4 < final_score ≤ 0.6 → too uncertain → Layer 7
                emit_stderr(&format!(
                    "   ⚠️ KNN confidence={confidence:.2} final_score={final_score:.2} — insufficient for decision, using Layer 7 fallback"
                ));
            } else {
                // No KNN match available. If the tree's normalized weighted score is already strongly in favor
                // of loop intent, promote to LoopStrong rather than blindly falling back to Layer 7.
                if weighted_score_normalized > 0.75 {
                    emit_stderr(&format!(
                        "   ⚖️ Tree strong ({weighted_score_normalized:.2}) but no KNN match — promoting to LoopStrong"
                    ));
                    return LoopIntentVerdict::LoopStrong(format!(
                        "Layer 6: Tree-only promotion (score={:.2}) - no KNN match",
                        weighted_score_normalized
                    ));
                }

                emit_stderr("   ⚠️ KNN returned no match — using Layer 7 fallback");
            }

            // ── Layer 7: 保守兜底 ────────────────────────────────────────────
            layer7_fallback(meta, reason)
        }
    }
}

/// Layer 7: Conservative fallback with minimum-loss default.
fn layer7_fallback(meta: &LoopMeta, upstream_reason: &str) -> LoopIntentVerdict {
    let ext = meta
        .source_extension
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_gif = ext == "gif";
    let is_video = matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "avi" | "flv");
    let is_modern_animated = MODERN_ANIMATED_EXTENSIONS.contains(&ext.as_str());

    let reason = format!("Layer 7: Fallback [{upstream_reason}]");

    if is_modern_animated {
        // Modern animated formats → convert to GIF (minimum-loss preservation)
        LoopIntentVerdict::LoopStrong(format!("{reason} → convert to GIF (modern animated)"))
    } else if is_gif {
        // GIF files default to LoopStrong unless explicitly LoopWeak was determined upstream
        // (transparent, small, silent stickers are common → preserve as-is by default)
        LoopIntentVerdict::LoopStrong(format!("{reason} → preserve GIF as-is (Layer 7 default)"))
    } else if is_video {
        // Video files: preserve as-is (low confidence)
        LoopIntentVerdict::Uncertain(format!(
            "{reason} → preserve video as-is (low confidence)"
        ))
    } else {
        // Unknown format — default conservative: treat as video (safer for quality)
        LoopIntentVerdict::Uncertain(format!("{reason} → unknown format, skip conversion"))
    }
}

// ── Safety & Exploration Helpers ──────────────────────────────────────────────

/// Dynamic safety-guard for CRF 0.00 (lossless) exploration.
///
/// Uses the KNN dataset to classify media as "Meme" vs "High Value".
/// - High-value art: strict 30s lossless limit (bloat risk).
/// - Meme / heavily compressed: up to 120s lossless limit (CRF 0.00 is efficient).
#[must_use]
pub fn is_lossless_exploration_safe(meta: &LoopMeta, path: Option<&Path>) -> bool {
    let legacy_meta = meta.to_legacy_gif_meta();
    let sample_match = crate::gif_value_db::lookup_similar_samples(&legacy_meta, path);
    let keep_prob = sample_match
        .as_ref()
        .and_then(|m| m.keep_probability)
        .unwrap_or(0.5);

    let threshold = lossless_duration_limit_for_keep_prob(keep_prob);
    let is_safe = meta.duration_secs < f64::from(threshold);

    if !is_safe {
        emit_stderr(&format!(
            "   ⚠️  Lossless-first (CRF 0.00) skip: duration {:.1}s exceeds dynamic limit {:.1}s (keep_prob={:.2})",
            meta.duration_secs, threshold, keep_prob
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
        (limit_high + (t * (limit_meme - limit_high))) as f32
    }
}

// ── Signal Scorers ────────────────────────────────────────────────────────────

fn calculate_cv(values: &[u64]) -> f64 {
    if values.is_empty() {
        return 0.5;
    }
    let n = values.len() as f64;
    let mean = values.iter().map(|&v| v as f64).sum::<f64>() / n;
    if mean <= 0.0 {
        return 0.0;
    }
    let var = values
        .iter()
        .map(|&v| (v as f64 - mean).powi(2))
        .sum::<f64>()
        / n;
    var.sqrt() / mean
}

fn calculate_cv_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.5;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    if mean <= 0.0 {
        return 0.0;
    }
    let var = values.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n;
    var.sqrt() / mean
}

fn calculate_gini_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len() as f64;
    let sum: f64 = sorted.iter().sum();
    if sum.abs() < 1e-9 {
        return 0.0;
    }
    let weighted_sum: f64 = sorted
        .iter()
        .enumerate()
        .map(|(i, &v)| (2 * (i + 1)) as f64 * v)
        .sum();
    (weighted_sum / (n * sum)) - (n + 1.0) / n
}

fn compression_efficiency_score(bytes: u64, w: u32, h: u32, fps: f64, dur: f64) -> f64 {
    let theoretical_bits = f64::from(w) * f64::from(h) * fps * dur * 24.0;
    let actual_bits = bytes as f64 * 8.0;
    if theoretical_bits <= 0.0 {
        return 0.5;
    }
    1.0 - (actual_bits / theoretical_bits * 150.0).min(1.0)
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

fn score_directory_context(parts: Option<&[String]>) -> f64 {
    const MEME_KEYWORDS: &[&str] = &[
        "meme", "sticker", "emoji", "表情", "贴纸", "梗", "斗图", "reaction", "gif",
    ];
    let Some(parts) = parts else { return 0.5 };
    for part in parts {
        let l = part.to_lowercase();
        if MEME_KEYWORDS.iter().any(|&k| l.contains(k)) {
            return 1.0;
        }
    }
    0.5
}

fn analyze_filename(name: Option<&str>) -> f64 {
    let Some(name) = name else { return 0.5 };
    let stem = name
        .rsplit_once('.')
        .map_or(name, |(s, _)| s)
        .to_lowercase();

    // Platform cache naming patterns → almost certainly meme/sticker
    if stem.starts_with("mmexport") || stem.starts_with("wx_camera") || stem.len() == 32 {
        return 1.0;
    }

    // Meme keywords in filename
    const MEME_KEYWORDS: &[&str] = &[
        "meme", "sticker", "emoji", "reaction", "lol", "funny", "gif", "表情", "贴纸",
    ];
    let lower = stem.to_lowercase();
    if MEME_KEYWORDS.iter().any(|&k| lower.contains(k)) {
        return 0.85;
    }

    // Pure random hash → likely social media download
    if stem.chars().all(|c| c.is_alphanumeric()) && stem.len() >= 20 {
        return 0.70;
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
/// it's likely an I-frame scene cut.
fn detect_scene_cut(pkt_sizes: &[u64]) -> bool {
    if pkt_sizes.len() < 5 {
        return false;
    }
    let inner = &pkt_sizes[1..pkt_sizes.len() - 1];
    let mut baseline = inner.to_vec();
    baseline.sort_unstable();
    let median = baseline[baseline.len() / 2] as f64;

    if median <= 0.0 {
        return false;
    }

    inner.iter().any(|&size| (size as f64) > median * 5.0)
}

/// Detect localized motion (high concentration of motion in small area).
/// Returns true if motion vectors suggest synthetic/sticker content.
fn detect_localized_motion(mvs: &[f64]) -> bool {
    mvs.len() >= 10 && zero_motion_ratio(mvs) > 0.7
}

/// Extract first frame from video to temporary PNG for analysis.
fn extract_frame_to_temp(path: &Path) -> Option<std::path::PathBuf> {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};
    
    // Generate unique filename: timestamp + random seed
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let rand_seed = std::process::id() ^ (timestamp as u32);
    
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!(
        "mfb_frame_{:x}_{:x}.png",
        timestamp, 
        rand_seed
    ));

    let output = Command::new("ffmpeg")
        .args(["-i", path.to_str()?, "-vframes", "1", "-f", "image2", "-y"])
        .arg(&temp_path)
        .stderr(std::process::Stdio::null())
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
    let top_band = (f64::from(h) * 0.15) as u32;
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
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64
}


fn detect_high_text_density_from_image(img: &image::DynamicImage) -> bool {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    if w < 3 || h < 3 {
        return false;
    }

    let mut edge_count = 0usize;
    let total_pixels = (w as f64) * (h as f64);

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

    let edge_ratio = edge_count as f64 / total_pixels;
    edge_ratio > 0.15
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn base_meta() -> LoopMeta {
        LoopMeta {
            duration_secs: 15.0, // Default to 15s to bypass Layer 1-C/1-D for tree testing
            width: 1280,         // > 512 to avoid Layer 1-D
            height: 720,
            fps: 12.0,
            frame_count: 24,
            file_size_bytes: 5_000_000,
            source_extension: Some("gif".to_string()),
            // Neutral signals for most tests
            frame_payload_variation: Some(0.5),
            frame_delay_variation: Some(0.5),
            directory_meme_score: 0.5,
            filename_meme_score: 0.5,
            ..Default::default()
        }
    }

    // ── Layer 1 ──

    #[test]
    fn test_layer1a_video_audio_veto() {
        let mut m = base_meta();
        m.source_extension = Some("mp4".to_string());
        m.has_audio = true;
        let v = identify_loop_intent(&m);
        assert!(
            matches!(v, LoopIntentVerdict::LoopWeak(_)),
            "Expected LoopWeak for MP4 with audio, got {:?}",
            v
        );
        assert!(v.reason().contains("Layer 1-A"));
    }

    #[test]
    fn test_audio_bypass_for_animated_images() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.has_audio = true; // Even if it has audio, GIFs bypass the veto
        let v = identify_loop_intent(&m);
        // It should proceed to next layers, and since it's a neutral 2s GIF, it ends up Uncertain (Layer 6)
        assert!(
            matches!(v, LoopIntentVerdict::Uncertain(_)),
            "Expected Uncertain for GIF with audio (bypass veto), got {:?}",
            v
        );
        assert!(!v.reason().contains("Layer 1-A"));
    }

    #[test]
    fn test_unconditional_hard_pass() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.duration_secs = 5.0; // <= 10s
        let v = identify_loop_intent(&m);
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)));
        assert!(v.reason().contains("Layer 1-C"));
    }

    #[test]
    fn test_short_hard_pass_vetoed_by_natural_signals() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.duration_secs = 5.0;
        m.fps = 24.0;
        m.webp_compression_ratio = Some(4.0);
        m.pkt_sizes = vec![120, 150, 1800, 140, 130, 125];
        let v = identify_loop_intent(&m);
        assert!(
            matches!(v, LoopIntentVerdict::LoopWeak(_)),
            "Expected LoopWeak when short hard pass is vetoed, got {:?}",
            v
        );
        assert!(!v.reason().contains("Layer 1-C"));
    }

    #[test]
    fn test_loop_count_decay_long() {
        let mut m = base_meta();
        m.duration_secs = 40.0; // > 35s -> minimal +0.15
        m.loop_count = Some(0);
        let v = identify_loop_intent(&m);
        // Base(40s: -0.15) + loop(0.15) = 0.0 -> Uncertain
        assert!(v.is_uncertain());
    }

    #[test]
    fn test_duration_layer1c_bypass() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.duration_secs = 5.0; // <= 10s
                               // (Handled by test_unconditional_hard_pass)
    }

    #[test]
    fn test_duration_layer5d_10_18s_weight() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.duration_secs = 15.0;
        let v = identify_loop_intent(&m);
        assert!(v.is_uncertain());
    }

    #[test]
    fn test_layer1b_transparency_pass() {
        let mut m = base_meta();
        m.has_transparency = true;
        let v = identify_loop_intent(&m);
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)));
        assert!(v.reason().contains("Layer 1-B"));
    }

    // ── Layer 2 ──

    #[test]
    fn test_layer2a_infinite_loop_signal() {
        let mut m = base_meta();
        m.duration_secs = 19.0;
        m.loop_count = Some(0);
        m.frame_delay_variation = Some(0.05);
        m.frame_payload_variation = Some(0.05);
        m.pkt_sizes = vec![1000, 980, 975, 990, 985, 1005];
        let v = identify_loop_intent(&m);
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)));
        assert!(v.reason().contains("Layer 3 Checkpoint"));
    }

    #[test]
    fn test_long_gif_conversion_escape() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.loop_count = Some(0);
        m.duration_secs = 40.0;
        let v = identify_loop_intent(&m);
        assert!(v.is_uncertain());
    }

    #[test]
    fn test_layer2b_no_loop_signal() {
        let mut m = base_meta();
        m.width = 640;
        m.height = 640;
        m.loop_count = Some(1);
        let v = identify_loop_intent(&m);
        assert!(
            matches!(v, LoopIntentVerdict::LoopWeak(_)),
            "Expected direct LoopWeak for play-once declaration, got {:?}",
            v
        );
        assert!(v.reason().contains("Layer 2-C"));
    }

    #[test]
    fn test_long_gif_scene_cut_becomes_loopweak() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.duration_secs = 40.0;
        m.fps = 24.0;
        m.file_size_bytes = 20_000_000;
        m.loop_count = Some(0);
        m.motion_gini = Some(0.20);
        m.webp_compression_ratio = Some(4.0);
        m.pkt_sizes = vec![100, 110, 1300, 115, 120, 118];
        let v = identify_loop_intent(&m);
        assert!(
            matches!(v, LoopIntentVerdict::LoopWeak(_)),
            "Expected LoopWeak for long natural-capture GIF, got {:?}",
            v
        );
    }

    #[test]
    fn test_small_resolution_hard_pass_is_vetoed_by_natural_signals() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.width = 400;
        m.height = 300;
        m.duration_secs = 60.0;
        m.fps = 24.0;
        m.webp_compression_ratio = Some(4.0);
        m.pkt_sizes = vec![90, 100, 1500, 95, 100, 98];
        let v = identify_loop_intent(&m);
        assert!(
            matches!(v, LoopIntentVerdict::LoopWeak(_)),
            "Expected LoopWeak when small-resolution hard pass is vetoed, got {:?}",
            v
        );
        assert!(!v.reason().contains("Layer 1-D"));
    }

    // ── Layer 3 ──

    #[test]
    fn test_layer3_high_closure_exits() {
        // Very uniform frame delays (low CV) → rhythm score high enough to trigger checkpoint
        let mut m = base_meta();
        // pkt_sizes: first and last very similar → low closure_dist
        m.pkt_sizes = vec![1000, 950, 980, 970, 960, 1010];
        m.frame_delay_variation = Some(0.05); // very uniform timing → +0.20
        m.frame_payload_variation = Some(0.05); // uniform payload
        let v = identify_loop_intent(&m);
        // Should exit at Layer 3 checkpoint (score ≥ 0.55)
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)));
        assert!(
            v.reason().contains("Layer 3")
                || v.reason().contains("Layer 2")
                || v.reason().contains("Layer 1")
        );
    }

    // ── Layer 4 ──

    #[test]
    fn test_layer4a_small_palette_and_webp() {
        let mut m = base_meta();
        m.frame_delay_variation = Some(0.05);
        m.palette_size = Some(32);
        m.webp_compression_ratio = Some(20.0);
        m.file_size_bytes = 900_000;
        let v = identify_loop_intent(&m);
        assert!(
            matches!(v, LoopIntentVerdict::LoopStrong(_)),
            "Expected LoopStrong, got: {:?}",
            v.reason()
        );
        assert!(v.reason().contains("Layer 4"));
    }

    // ── Layer 6/7 fallback (Uncertain) ──

    #[test]
    fn test_layer6_low_confidence_fallback() {
        // All neutral — nothing triggers layers 1-5 strongly
        let m = base_meta();
        let v = identify_loop_intent(&m);
        // Should reach Uncertain (Layer 6 needed)
        assert!(matches!(v, LoopIntentVerdict::Uncertain(_)));
        assert!(v.reason().contains("Layer 6") || v.reason().contains("Layer 7"));
    }

    #[test]
    fn test_layer2c_platform_marker_multiple() {
        let mut m = base_meta();
        m.app_extensions = Some(vec!["UNKNOWN".to_string(), "GIPHY".to_string()]);
        let v = identify_loop_intent(&m);
        assert!(
            v.is_keep_gif(),
            "Expected LoopStrong for platform marker + rhythm, got {:?}",
            v
        );
        assert!(v.reason().contains("Layer 2-A"));
    }

    #[test]
    fn test_very_short_video_minimal_frames() {
        let mut m = base_meta();
        m.frame_count = 2;
        m.pkt_sizes = vec![500, 505]; // Only 2 frames, Layer 3-A should skip
        m.frame_delay_variation = None;
        let v = identify_loop_intent(&m);
        // Should be Uncertain since 2 frames isn't enough for structural signal
        assert!(matches!(v, LoopIntentVerdict::Uncertain(_)));
    }

    #[test]
    fn test_layer2d_webm_audio_veto_priority() {
        let mut m = base_meta();
        m.source_extension = Some("webm".to_string());
        m.has_audio = true; // Audio Veto (Layer 1) should override WebM Loop (Layer 2)
        let v = identify_loop_intent(&m);
        assert!(matches!(v, LoopIntentVerdict::LoopWeak(_)));
        assert!(v.reason().contains("Layer 1-A"));
    }

    #[test]
    fn test_weighted_score_normalize() {
        let mut s = WeightedScore::default();
        s.add(0.5);
        assert!((s.normalized() - 0.75).abs() < 1e-9); // (0.5+1.0)/2.0 = 0.75
    }

    #[test]
    fn test_small_resolution_hard_pass() {
        let mut m = base_meta();
        m.source_extension = Some("webp".to_string());
        m.width = 400; // <= 512
        m.height = 400; // <= 512
        m.duration_secs = 60.0; // Very long, usually triggers video conversion
        let v = identify_loop_intent(&m);
        assert!(
            matches!(v, LoopIntentVerdict::LoopStrong(_)),
            "Expected LoopStrong for small resolution, got {:?}",
            v
        );
        assert!(v.reason().contains("Layer 1-D"));
    }
}

// ── WebP Sampling Implementation ───────────────────────────────────────────

fn should_sample_webp_compression_ratio(ext: Option<&str>) -> bool {
    // Only sample when the input is a static/raster image format where a single-frame
    // WebP proxy is representative. Unknown or video formats are not sampled.
    let Some(ext) = ext else { return false };
    let lower = ext.to_lowercase();
    matches!(lower.as_str(), "gif" | "png" | "bmp" | "tiff" | "tif" | "tga" | "jpg" | "jpeg")
}

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
                (f64::from(WEBP_RATIO_SAMPLE_MAX_DIM) / ratio) as u32,
            )
        } else {
            (
                (f64::from(WEBP_RATIO_SAMPLE_MAX_DIM) * ratio) as u32,
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

    let mut buffer = std::io::Cursor::new(Vec::new());
    let encoder = WebPEncoder::new_lossless(&mut buffer);
    encoder
        .encode(
            resized.as_bytes(),
            resized.width(),
            resized.height(),
            ExtendedColorType::Rgba8,
        )
        .ok()?;

    let webp_size = buffer.get_ref().len() as f64;
    let raw_size = (resized.width() * resized.height() * 4) as f64;

    if webp_size <= 0.0 {
        return None;
    }
    Some(raw_size / webp_size)
}

/// Check if a file path should use the GIF fast-path (from_gif_path) instead of ffprobe.
#[must_use]
pub fn should_use_gif_fast_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase()),
        Some(ext) if ext == "gif"
    )
}

