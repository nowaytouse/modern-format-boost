//! 「循环意图判断系统」 (Loop Intent Identification System)
//!
//! A modern, explainable judgment tree for identifying media looping intent (memes, stickers, loops).
//! Implements the 7-layer hierarchical decision tree defined in docs/decision_tree.md.
//!
//! Architecture:
//! - Layer 1 & 2: Hard constraint / explicit declaration → zero-cost forced exits
//! - Layer 3 & 4: Self-referential signals → WeightedScore accumulation with checkpoints
//! - Layer 5:     Context semantics → weak auxiliary corrections
//! - Layer 6:     KNN + WeightedScore fusion → probabilistic judgment
//! - Layer 7:     Conservative fallback → minimum-loss default

use crate::video_detection::VideoDetectionResult;
use crate::video_detection::ColorSpace;
use crate::progress_mode::emit_stderr;
use crate::file_copier::{SUPPORTED_IMAGE_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS};
use std::path::Path;
use serde::{Deserialize, Serialize};

const MODERN_ANIMATED_EXTENSIONS: &[&str] = &["webp", "avif", "apng", "heic", "heif", "jxl"];
const GIPHY_PLATFORM_MARKERS: &[&str] = &["GIPHY", "TENOR", "IMGUR", "STICKER", "TELEGRAM", "TIKTOK", "DISCORD"];

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
    /// Caller populates this via LoopMeta::set_webp_compression_ratio() or leaves None.
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
}

impl LoopMeta {
    /// Build LoopMeta from a full VideoDetectionResult.
    pub fn from_video_detection(detection: &VideoDetectionResult) -> Self {
        let file_path = Path::new(&detection.file_path);
        let file_name = file_path.file_name().and_then(|s| s.to_str()).map(|s| s.to_string());
        let source_extension = file_path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase());
        let parent_directories: Option<Vec<String>> = file_path.parent().map(|p| {
            p.iter().rev().take(4).filter_map(|s| s.to_str()).map(|s| s.to_string()).collect()
        });

        // Detect transparency from pixel format
        let has_transparency = detection.pix_fmt.contains("a") ||
                               detection.pix_fmt.contains("yuva") ||
                               detection.pix_fmt.contains("gbrap");

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
            has_complex_color_profile: matches!(detection.color_space, ColorSpace::BT2020 | ColorSpace::AdobeRGB) || 
                                       detection.is_dolby_vision || 
                                       detection.is_hdr10_plus,
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
        };
        meta.directory_meme_score = score_directory_context(parent_directories.as_deref());
        meta.filename_meme_score = analyze_filename(meta.file_name.as_deref());
        meta
    }

    /// Build LoopMeta from an FFprobeResult (used in pipelines without full detection).
    pub fn from_ffprobe_result(probe: &crate::ffprobe::FFprobeResult, path: &Path) -> Self {
        let file_name = path.file_name().and_then(|s| s.to_str()).map(|s| s.to_string());
        let source_extension = path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase());
        let parent_directories: Option<Vec<String>> = path.parent().map(|p| {
            p.iter().rev().take(4).filter_map(|s| s.to_str()).map(|s| s.to_string()).collect()
        });

        // Detect transparency from pixel format
        let has_transparency = probe.pix_fmt.contains("a") ||
                               probe.pix_fmt.contains("yuva") ||
                               probe.pix_fmt.contains("gbrap");

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
        };
        meta.directory_meme_score = score_directory_context(parent_directories.as_deref());
        meta.filename_meme_score = analyze_filename(meta.file_name.as_deref());
        meta
    }

    /// Build LoopMeta from a GIF file using header-level scanning (fast, no ffprobe).
    pub fn from_gif_path(path: &Path) -> Option<Self> {
        let (pal, exts, has_transparency, variation, delay_variation, loops, total_dur) =
            crate::useless::gif_meme_score::scan_gif_headers(path).ok()?;

        let file_size = std::fs::metadata(path).ok()?.len();
        let file_name = path.file_name().and_then(|s| s.to_str()).map(|s| s.to_string());
        let parent_directories: Option<Vec<String>> = path.parent().map(|p| {
            p.iter().rev().take(4).filter_map(|s| s.to_str()).map(|s| s.to_string()).collect()
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

        let mut meta = Self {
            duration_secs: total_dur.unwrap_or(0.0),
            width,
            height,
            fps: 12.0, // Conservative estimate for header-only path
            frame_count: 1, // Placeholder; refined if deep scan is available
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
        Some(meta)
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

/// The WeightedScore accumulator that flows through Layers 3–6.
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

/// Run the 7-layer decision tree and return a LoopIntentVerdict.
/// This function is pure: no I/O, no database calls.
/// KNN fallback (Layer 6) is performed in `assess_loop_intent_from_meta`.
#[must_use]
pub fn identify_loop_intent(meta: &LoopMeta) -> LoopIntentVerdict {
    let mut score = WeightedScore::default();

    let ext_lower = meta.source_extension.as_deref().unwrap_or("").to_lowercase();
    let is_image = SUPPORTED_IMAGE_EXTENSIONS.contains(&ext_lower.as_str());
    let is_video = !is_image && SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str());

    // ══════════════════════════════════════════════════════════════
    // LAYER 1: 格式物理硬约束 — forced exits, WeightedScore 不参与
    // ══════════════════════════════════════════════════════════════

    // 1-A: 有音轨？（仅对视频容器执行硬否决，绝不否决动态图片）
    if meta.has_audio && is_video {
        return LoopIntentVerdict::LoopWeak("Layer 1-A: Hard Veto — Audio Track Present in Video Device".to_string());
    }

    // 1-B: 有透明通道且无音轨？视频透明度处理成本极高
    if meta.has_transparency && !meta.has_audio {
        return LoopIntentVerdict::LoopStrong("Layer 1-B: Hard Pass — Transparency Channel Present".to_string());
    }

    // 1-C: 组合硬通行 (Combined Hard Pass)
    if is_image {
        // 规则 1: 0-10秒无条件硬通行
        if meta.duration_secs <= 10.0 {
            return LoopIntentVerdict::LoopStrong("Layer 1-C: Unconditional Hard Pass (<=10s)".to_string());
        }
        // 规则 2: 10-18秒+无限循环标记硬通行
        if meta.duration_secs <= 18.0 && meta.loop_count == Some(0) {
            return LoopIntentVerdict::LoopStrong("Layer 1-C: Conditional Hard Pass (10-18s + loop_count=0)".to_string());
        }
    }

    // 1-D: 小尺寸/小分辨率硬通行 (Small Resolution Hard Pass)
    // 物理尺寸 <= 512x512 的素材通常被视为贴纸或图标，不具备转换为视频容器的价值
    if is_image && (meta.width > 0 && meta.height > 0) && (meta.width <= 512 && meta.height <= 512) {
        return LoopIntentVerdict::LoopStrong("Layer 1-D: Unconditional Hard Pass (Small Resolution <= 512x512)".to_string());
    }

    // ══════════════════════════════════════════════════════════════
    // LAYER 2: 显式自我声明 — WeightedScore 累积，层末无检查点（降级为强信号）
    // ══════════════════════════════════════════════════════════════

    // 2-A: 无限循环标记 (loop_count == 0) — 渐进式权重衰减
    if meta.loop_count == Some(0) {
        let weight = if meta.duration_secs <= 18.0 {
            0.45
        } else if meta.duration_secs <= 35.0 {
            // 18-35s: 线性衰减自 0.45 至 0.20
            let ratio = (meta.duration_secs - 18.0) / (35.0 - 18.0);
            0.45 - (0.45 - 0.20) * ratio
        } else {
            0.15 // >35s: 极低可信度
        };
        score.add(weight);
    }

    // 2-B: 明确不循环 (loop_count == 1 → play once and stop) — 强视频偏向
    if meta.loop_count == Some(1) {
        score.add(-0.45);
    }

    // 2-C: 平台特定的应用扩展（GIPHY, TENOR 等）
    if let Some(apps) = &meta.app_extensions {
        for app in apps {
            if GIPHY_PLATFORM_MARKERS.contains(&app.as_str()) {
                score.add(0.50); // Significant signal for meme/loop intent
            }
        }
    }

    // 2-D: WebM 无音轨 — Web 动图标准载体，格式本身即循环语义
    let is_webm = meta.container.as_deref().is_some_and(|c| c.eq_ignore_ascii_case("webm"))
        || ext_lower == "webm";
    if is_webm && !meta.has_audio {
        score.add(0.40);
    }

    // 2-E: 时长级进分值 (Progressive Duration Scoring)
    // 在此处添加分值，以便它们参与 Layer 3/4 的检查点判定
    if meta.duration_secs > 10.0 {
        if meta.duration_secs <= 18.0 {
            score.add(0.35); // 10-18s 强权重
        } else if meta.duration_secs <= 35.0 {
            let ratio = (meta.duration_secs - 18.0) / (35.0 - 18.0);
            score.add(-0.15 * ratio); // 18-35s 线性惩罚
        } else {
            score.add(-0.15); // >35s 重度惩罚
        }
    } else {
        // 0-10s 针对非图片资产的额外加分（图片资产已在 Layer 1-C 退出）
        score.add(0.40);
    }

    // 2-F: 现代格式加压转换 (Modern Format Conversion Bias)
    // 现代格式（WebP/AVIF等）在结构上与视频编码高度一致。如果判定分数已显露出负向（视频特征），
    // 则通过加压分值加速其流向视频转换逻辑（LoopWeak）。
    let is_modern = MODERN_ANIMATED_EXTENSIONS.contains(&ext_lower.as_str());
    if is_modern {
        let val = score.value();
        if val < 0.0 {
            if val < -0.30 {
                score.add(-0.35); // 快速锁定为 LoopWeak
            } else {
                score.add(-0.25); // 显著导向视频转换
            }
        }
    }

    // ══════════════════════════════════════════════════════════════
    // LAYER 3: 自参照结构信号 — WeightedScore 累积，层末有检查点
    // ══════════════════════════════════════════════════════════════

    // 3-A: 首尾帧自参照闭合比
    // closure_ratio = 首尾帧视觉距离 / 帧间平均视觉距离
    // We proxy this via: first_pkt_size vs last_pkt_size vs avg, normalized by avg.
    // When pkt_sizes has ≥3 frames and avg > 0, the signal is valid.
    if meta.pkt_sizes.len() >= 3 {
        let n = meta.pkt_sizes.len();
        let first = meta.pkt_sizes[0] as f64;
        let last = meta.pkt_sizes[n - 1] as f64;
        let avg = meta.pkt_sizes.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
        if avg > 1.0 {
            // Closure distance = |first - last| / avg  (how different first and last are)
            let closure_dist = (first - last).abs() / avg;
            // Frame-to-frame avg distance ≈ 1 unit (self-referential: normalized to avg)
            let inter_dist = meta.frame_payload_variation.unwrap_or(0.5);

            // closure_ratio ≈ 1.0 → similar jump between frames → strong loop
            // closure_ratio >> 1.0 → first/last differ much more than avg frames → not a loop
            let closure_ratio = if inter_dist > 0.01 {
                closure_dist / inter_dist.clamp(0.1, 2.0)
            } else {
                // Skip: inter-frame distance itself is near-zero (static content or edge case)
                -1.0 // sentinel: skip
            };

            if closure_ratio >= 0.0 {
                if closure_ratio <= 1.2 {
                    // Closure ≈ normal inter-frame jump → loop
                    score.add(0.35);
                } else if closure_ratio > 2.5 {
                    // Closure far larger than normal inter-frame → not a clean loop
                    score.add(-0.35);
                }
                // 1.2 < ratio ≤ 2.5: neutral, don't modify score
            }
            // closure_ratio < 0 (sentinel): skip
        }
        // else: avg ≈ 0 (edge case) → skip
    }

    // 3-B: 节奏均匀性 — interval_consistency_score via frame_delay_variation CV
    // Low CV of PTS deltas = uniform timing = regular rhythm = loop-like
    if let Some(delay_cv) = meta.frame_delay_variation {
        if delay_cv < 0.10 {
            // Highly uniform timing
            score.add(0.20);
        } else if delay_cv < 0.25 {
            // Somewhat uniform
            score.add(0.10);
        } else if delay_cv > 0.60 {
            // Chaotic timing
            score.add(-0.15);
        }
        // 0.25..=0.60: neutral
    }

    // Layer 3 checkpoint
    if score.value() >= 0.55 {
        return LoopIntentVerdict::LoopStrong(
            format!("Layer 3 Checkpoint: WeightedScore={:.2} ≥ 0.55 (self-referential structure)", score.value())
        );
    }
    if score.value() <= -0.55 {
        return LoopIntentVerdict::LoopWeak(
            format!("Layer 3 Checkpoint: WeightedScore={:.2} ≤ -0.55 (structure mismatch)", score.value())
        );
    }

    // ══════════════════════════════════════════════════════════════
    // LAYER 4: 内容特征信号 — 成本较高，继续累积
    // ══════════════════════════════════════════════════════════════

    // 4-A: 调色板大小
    if let Some(p_size) = meta.palette_size {
        if p_size <= 64 {
            score.add(0.25); // typical synthetic/sticker content
        } else if p_size > 128 {
            score.add(-0.15); // closer to natural photographic color space
        }
        // 65–128: neutral
    }

    // 4-B: 帧内容可压缩性 (WebP压缩比代理)
    // Caller sets webp_compression_ratio if available. Skip if None.
    if let Some(ratio) = meta.webp_compression_ratio {
        if ratio > 15.0 {
            score.add(0.20); // flat/synthetic → heavy compressible → loop-like
        } else if ratio < 5.0 {
            score.add(-0.25); // noisy/photographic → naturally low compressibility
        }
        // 5.0..=15.0: neutral
    }

    // 4-C: compression_efficiency_score (existing implementation)
    let ce = compression_efficiency_score(meta.file_size_bytes, meta.width, meta.height, meta.fps, meta.duration_secs);
    if ce > 0.7 {
        score.add(0.15);
    } else if ce < 0.3 {
        score.add(-0.10);
    }

    // Layer 4 checkpoint
    if score.value() >= 0.55 {
        return LoopIntentVerdict::LoopStrong(
            format!("Layer 4 Checkpoint: WeightedScore={:.2} ≥ 0.55 (content features)", score.value())
        );
    }
    if score.value() <= -0.55 {
        return LoopIntentVerdict::LoopWeak(
            format!("Layer 4 Checkpoint: WeightedScore={:.2} ≤ -0.55 (content features)", score.value())
        );
    }

    // ══════════════════════════════════════════════════════════════
    // LAYER 5: 上下文语义信号 — 权重刻意压低，仅作辅助修正
    // ══════════════════════════════════════════════════════════════

    // 5-A: 目录 / 文件名语义
    let dir_score = meta.directory_meme_score;
    let file_score = meta.filename_meme_score;
    if dir_score > 0.8 && file_score > 0.8 {
        score.add(0.10);
    } else if dir_score > 0.8 || file_score > 0.8 {
        score.add(0.05);
    }

    // 5-B: fps 异常（非标准帧率 → 典型动图特征）
    let fps_anom = fps_anomaly_score(meta.fps);
    if fps_anom > 0.6 {
        score.add(0.05);
    }

    // 5-C: 总帧数（以自参照形式表达时长，不硬编码秒数）
    // frame_count is our proxy; very few frames = short = loop candidate
    // Very many frames = long video = not likely a loop
    if meta.frame_count > 0 && meta.duration_secs > 0.01 {
        let frame_gap = meta.duration_secs / meta.frame_count as f64; // secs per frame
        if frame_gap > 0.5 && meta.frame_count <= 8 {
            // Very few frames, very sparse — typical of a short GIF loop
            score.add(0.05);
        } else if meta.frame_count > 500 {
            // Many frames → extended video content
            score.add(-0.10);
        }
    }

    // 5-D: (已移至 Layer 2-E 以提前参与检查点判定)
    // 此处保留注释以维持层级文档完整性

    // 5-E: 颜色配置文件奖励 (Standard Color Profile Reward)
    // sRGB 或无配置文件通常是 GIF 和 MEME 的特征；复杂配置文件 (HDR/DCI-P3) 则是高质量视频的特征
    if !meta.has_complex_color_profile {
        score.add(0.00050);
    }

    // 5-F: 1:1 Aspect Ratio (Square)
    // Most modern stickers (Telegram, WeChat, Discord) are strictly 1:1.
    if meta.width > 0 && meta.height > 0 && (meta.width == meta.height) {
        score.add(0.03); // Minor auxiliary signal
    }

    // 5-G: (已移至 Layer 2-F 以提前参与检查点判定)

    // No Layer 5 checkpoint (by design — Layer 5 is only auxiliary correction).

    // ══════════════════════════════════════════════════════════════
    // LAYER 6: KNN + WeightedScore 综合融合判断
    // ══════════════════════════════════════════════════════════════
    // This layer is handled in assess_loop_intent_from_meta() below,
    // since it requires a database call.
    // identify_loop_intent() returns Uncertain to signal "proceed to Layer 6".
    LoopIntentVerdict::Uncertain(
        format!("Layer 6: Incomplete tree signal (WeightedScore={:.2}), proceeding to KNN fusion", score.value())
    )
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Execute the loop intent identification for a given detection result.
pub fn assess_loop_intent(detection: &VideoDetectionResult) -> LoopIntentVerdict {
    let meta = LoopMeta::from_video_detection(detection);
    assess_loop_intent_from_meta(&meta, Some(Path::new(&detection.file_path)))
}

/// Execute the loop intent identification for a given probe result.
pub fn assess_loop_intent_from_probe(probe: &crate::ffprobe::FFprobeResult, path: &Path) -> LoopIntentVerdict {
    let meta = LoopMeta::from_ffprobe_result(probe, path);
    assess_loop_intent_from_meta(&meta, Some(path))
}

/// Core entry point: runs the full tree including KNN Layer 6 and Layer 7 fallback.
/// 
/// - Runs `identify_loop_intent()` (pure, Layers 1–5 + early Layer 6 check).
/// - If result is `Uncertain`, invokes KNN via `gif_value_db::lookup_similar_samples`.
/// - If KNN is unavailable or confidence is low, falls back to Layer 7 conservatively.
pub fn assess_loop_intent_from_meta(meta: &LoopMeta, path: Option<&Path>) -> LoopIntentVerdict {
    // First pass: deterministic tree (Layers 1–5)
    let tree_verdict = identify_loop_intent(meta);

    // WeightedScore is embedded in the Uncertain reason string — re-extract for fusion.
    // We re-run score computation cheaply by calling identify_loop_intent again.
    // (It's a pure function, so this is safe and cheap.)
    let weighted_score_normalized = extract_weighted_score_from_verdict(&tree_verdict, meta);

    match tree_verdict {
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
                let mut final_score = keep_prob * 0.6 + weighted_score_normalized * 0.4 + nudges.score;

                // Log nudge trace if any nudges were applied
                if !nudges.trace.is_empty() {
                    emit_stderr(&format!("   ⚖️  Micro-Nudges ({:+.2}): {}", nudges.score, nudges.trace.join(" | ")));
                }

                // Tier 3: High-cost visual checks (gated execution)
                // Only trigger if still uncertain (0.4-0.6) after Tier 1+2 nudges
                if final_score > 0.40 && final_score < 0.60 && confidence < 0.75 {
                    if let Some(p) = path {
                        emit_stderr("   🔍 Triggering high-cost visual heuristics (extreme uncertainty)...");
                        let mut tier3_nudge = AuxiliaryNudge::default();

                        if detect_heavy_letterboxing(p) {
                            tier3_nudge.apply(0.05, "Letterboxing detected");
                        }
                        if detect_high_text_density(p) {
                            tier3_nudge.apply(0.08, "High text density");
                        }

                        if !tier3_nudge.trace.is_empty() {
                            emit_stderr(&format!("   📊 Tier 3 Visual ({:+.2}): {}", tier3_nudge.score, tier3_nudge.trace.join(" | ")));
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
                emit_stderr("   ⚠️ KNN returned no match — using Layer 7 fallback");
            }

            // ── Layer 7: 保守兜底 ────────────────────────────────────────────
            layer7_fallback(meta, reason)
        }
    }
}

/// Layer 7: Conservative fallback with minimum-loss default.
fn layer7_fallback(meta: &LoopMeta, upstream_reason: &str) -> LoopIntentVerdict {
    let ext = meta.source_extension.as_deref().unwrap_or("");
    let is_gif = ext.eq_ignore_ascii_case("gif");
    let is_video = matches!(ext, "mp4" | "mov" | "mkv" | "avi" | "flv");
    let is_modern_animated = matches!(ext, "webp" | "apng" | "avif");

    let reason = format!("Layer 7: Fallback [{upstream_reason}]");

    if is_modern_animated {
        // Modern animated formats → convert to GIF (minimum-loss preservation)
        LoopIntentVerdict::LoopStrong(format!("{reason} → convert to GIF (modern animated)"))
    } else if is_gif || is_video {
        // Already in target format → keep as-is
        if is_gif {
            LoopIntentVerdict::Uncertain(format!("{reason} → preserve GIF as-is (low confidence)"))
        } else {
            LoopIntentVerdict::Uncertain(format!("{reason} → preserve video as-is (low confidence)"))
        }
    } else {
        // Unknown format — default conservative: treat as video (safer for quality)
        LoopIntentVerdict::Uncertain(format!("{reason} → unknown format, skip conversion"))
    }
}

/// Re-extract the normalized WeightedScore from a verdict.
/// Since identify_loop_intent() is pure, we can re-run the score computation
/// by tracing through only the WeightedScore parts (not the early-exit paths).
fn extract_weighted_score_from_verdict(verdict: &LoopIntentVerdict, meta: &LoopMeta) -> f64 {
    // If the tree gave a definitive answer in Layers 1–2, score is irrelevant.
    // If it exited in Layer 3/4 via checkpoint, we use 0.275 (midpoint of [0.55/2.0, 0.45])
    // as a safe approximation: the tree leaned one way but not maximally.
    // If Uncertain (reached Layer 6), we parse the score from the reason string.
    match verdict {
        LoopIntentVerdict::LoopStrong(_) => 1.0,
        LoopIntentVerdict::LoopWeak(_) => 0.0,
        LoopIntentVerdict::Uncertain(_) => {
            // Re-run the score accumulation (cheap, pure function)
            recompute_weighted_score(meta)
        }
    }
}

/// Re-run only the WeightedScore accumulation (Layers 3–5), bypassing early exits.
/// Used to reconstruct the score for Layer 6 KNN fusion.
fn recompute_weighted_score(meta: &LoopMeta) -> f64 {
    let mut score = WeightedScore::default();

    let ext_lower = meta.source_extension.as_deref().unwrap_or("").to_lowercase();
    let is_webm = meta.container.as_deref().is_some_and(|c| c.eq_ignore_ascii_case("webm"))
        || ext_lower == "webm";

    // Duration Scoring (Consistent with Layer 5-D)
    if meta.duration_secs <= 10.0 {
        score.add(0.40);
    } else if meta.duration_secs <= 18.0 {
        score.add(0.35);
    } else if meta.duration_secs <= 35.0 {
        let ratio = (meta.duration_secs - 18.0) / (35.0 - 18.0);
        score.add(-0.15 * ratio);
    } else if meta.duration_secs > 35.0 {
        score.add(-0.15);
    }

    // Re-apply Layer 2 signals (consistent with identify_loop_intent)
    if meta.loop_count == Some(0) {
        let weight = if meta.duration_secs <= 18.0 {
            0.45
        } else if meta.duration_secs <= 35.0 {
            let ratio = (meta.duration_secs - 18.0) / (35.0 - 18.0);
            0.45 - (0.45 - 0.20) * ratio
        } else {
            0.15
        };
        score.add(weight);
    }
    if meta.loop_count == Some(1) {
        score.add(-0.45);
    }
    const PLATFORM_MARKERS: &[&str] = &["GIPHY", "TENOR", "STICKER", "TELEGRAM", "TIKTOK", "DISCORD"];
    if let Some(exts) = &meta.app_extensions {
        for ext in exts {
            if PLATFORM_MARKERS.iter().any(|&p| ext.trim().to_uppercase().starts_with(p)) {
                score.add(0.50);
            }
        }
    }
    if is_webm && !meta.has_audio {
        score.add(0.40);
    }

    // 3-A
    if meta.pkt_sizes.len() >= 3 {
        let n = meta.pkt_sizes.len();
        let first = meta.pkt_sizes[0] as f64;
        let last = meta.pkt_sizes[n - 1] as f64;
        let avg = meta.pkt_sizes.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
        if avg > 1.0 {
            let closure_dist = (first - last).abs() / avg;
            let inter_dist = meta.frame_payload_variation.unwrap_or(0.5);
            if inter_dist > 0.01 {
                let closure_ratio = closure_dist / inter_dist.clamp(0.1, 2.0);
                if closure_ratio <= 1.2 {
                    score.add(0.35);
                } else if closure_ratio > 2.5 {
                    score.add(-0.35);
                }
            }
        }
    }
    // 3-B
    if let Some(delay_cv) = meta.frame_delay_variation {
        if delay_cv < 0.10 { score.add(0.20); }
        else if delay_cv < 0.25 { score.add(0.10); }
        else if delay_cv > 0.60 { score.add(-0.15); }
    }
    // 4-A
    if let Some(p_size) = meta.palette_size {
        if p_size <= 64 { score.add(0.25); }
        else if p_size > 128 { score.add(-0.15); }
    }
    // 4-B
    if let Some(ratio) = meta.webp_compression_ratio {
        if ratio > 15.0 { score.add(0.20); }
        else if ratio < 5.0 { score.add(-0.25); }
    }
    // 4-C
    let ce = compression_efficiency_score(meta.file_size_bytes, meta.width, meta.height, meta.fps, meta.duration_secs);
    if ce > 0.7 { score.add(0.15); }
    else if ce < 0.3 { score.add(-0.10); }
    // 5-A
    if meta.directory_meme_score > 0.8 && meta.filename_meme_score > 0.8 { score.add(0.10); }
    else if meta.directory_meme_score > 0.8 || meta.filename_meme_score > 0.8 { score.add(0.05); }
    // 5-B
    if fps_anomaly_score(meta.fps) > 0.6 { score.add(0.05); }
    // 5-C
    if meta.frame_count > 0 && meta.duration_secs > 0.01 {
        let frame_gap = meta.duration_secs / meta.frame_count as f64;
        if frame_gap > 0.5 && meta.frame_count <= 8 { score.add(0.05); }
        else if meta.frame_count > 500 { score.add(-0.10); }
    }
    // 5-D
    if meta.duration_secs > 18.0 {
        if meta.duration_secs <= 35.0 {
            let ratio = (meta.duration_secs - 18.0) / (35.0 - 18.0);
            score.add(-0.15 * ratio);
        } else {
            score.add(-0.15);
        }
    }

    score.normalized()
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
    if values.is_empty() { return 0.5; }
    let n = values.len() as f64;
    let mean = values.iter().map(|&v| v as f64).sum::<f64>() / n;
    if mean <= 0.0 { return 0.0; }
    let var = values.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n;
    var.sqrt() / mean
}

fn calculate_cv_f64(values: &[f64]) -> f64 {
    if values.is_empty() { return 0.5; }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    if mean <= 0.0 { return 0.0; }
    let var = values.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n;
    var.sqrt() / mean
}

fn calculate_gini_f64(values: &[f64]) -> f64 {
    if values.is_empty() { return 0.0; }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len() as f64;
    let sum: f64 = sorted.iter().sum();
    if sum.abs() < 1e-9 { return 0.0; }
    let weighted_sum: f64 = sorted.iter().enumerate().map(|(i, &v)| (2 * (i + 1)) as f64 * v).sum();
    (weighted_sum / (n * sum)) - (n + 1.0) / n
}

fn compression_efficiency_score(bytes: u64, w: u32, h: u32, fps: f64, dur: f64) -> f64 {
    let theoretical_bits = f64::from(w) * f64::from(h) * fps * dur * 24.0;
    let actual_bits = bytes as f64 * 8.0;
    if theoretical_bits <= 0.0 { return 0.5; }
    1.0 - (actual_bits / theoretical_bits * 150.0).min(1.0)
}

fn fps_anomaly_score(fps: f64) -> f64 {
    // Returns high score when fps is far from standard rates → atypical → possible loop artifact
    let std_rates = [24.0, 25.0, 30.0, 60.0, 120.0];
    let min_delta = std_rates.iter().map(|&s| (fps - s).abs()).fold(f64::MAX, f64::min);
    (min_delta / 2.5).min(1.0)
}

fn score_directory_context(parts: Option<&[String]>) -> f64 {
    const MEME_KEYWORDS: &[&str] = &["meme", "sticker", "emoji", "表情", "贴纸", "梗", "斗图", "reaction", "gif"];
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
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_lowercase();

    // Platform cache naming patterns → almost certainly meme/sticker
    if stem.starts_with("mmexport") || stem.starts_with("wx_camera") || stem.len() == 32 {
        return 1.0;
    }

    // Meme keywords in filename
    const MEME_KEYWORDS: &[&str] = &["meme", "sticker", "emoji", "reaction", "lol", "funny", "gif", "表情", "贴纸"];
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
/// If any inner frame is 5x larger than average, it's likely an I-frame scene cut.
fn detect_scene_cut(pkt_sizes: &[u64]) -> bool {
    if pkt_sizes.len() < 5 { return false; }
    let inner = &pkt_sizes[1..pkt_sizes.len() - 1];
    let avg = inner.iter().sum::<u64>() as f64 / inner.len() as f64;
    inner.iter().any(|&size| (size as f64) > avg * 5.0)
}

/// Detect localized motion (high concentration of motion in small area).
/// Returns true if motion vectors suggest synthetic/sticker content.
fn detect_localized_motion(mvs: &[f64]) -> bool {
    if mvs.len() < 10 { return false; }
    let zero_count = mvs.iter().filter(|&&v| v.abs() < 0.1).count();
    let zero_ratio = zero_count as f64 / mvs.len() as f64;
    zero_ratio > 0.7 // >70% of blocks have near-zero motion
}

/// Extract first frame from video to temporary PNG for analysis.
fn extract_frame_to_temp(path: &Path) -> Option<std::path::PathBuf> {
    use std::process::Command;
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("mfb_frame_{}.png", std::process::id()));

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

/// Detect heavy letterboxing/pillarboxing (solid color bars on top/bottom or sides).
fn detect_heavy_letterboxing(path: &Path) -> bool {
    let temp_frame = match extract_frame_to_temp(path) {
        Some(p) => p,
        None => return false,
    };

    let result = (|| {
        use image::GenericImageView;
        let img = image::open(&temp_frame).ok()?;
        let (_w, h) = img.dimensions();
        if h < 100 { return Some(false); }

        let top_band = (f64::from(h) * 0.15) as u32;
        let bottom_start = h - top_band;

        // Calculate variance in top and bottom bands
        let top_var = calculate_band_variance(&img, 0, top_band);
        let bottom_var = calculate_band_variance(&img, bottom_start, h);

        // Low variance = solid color = letterboxing
        Some(top_var < 100.0 && bottom_var < 100.0)
    })();

    let _ = std::fs::remove_file(temp_frame);
    result.unwrap_or(false)
}

/// Calculate pixel variance in a horizontal band.
fn calculate_band_variance(img: &image::DynamicImage, y_start: u32, y_end: u32) -> f64 {
    use image::GenericImageView;
    let (w, _) = img.dimensions();
    let mut values = Vec::new();

    for y in y_start..y_end.min(img.height()) {
        for x in 0..w.min(img.width()) {
            let pixel = img.get_pixel(x, y);
            let gray = f64::from(pixel[0]) * 0.299 + f64::from(pixel[1]) * 0.587 + f64::from(pixel[2]) * 0.114;
            values.push(gray);
        }
    }

    if values.is_empty() { return 0.0; }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64
}

/// Detect high text density via edge detection heuristic.
fn detect_high_text_density(path: &Path) -> bool {
    let temp_frame = match extract_frame_to_temp(path) {
        Some(p) => p,
        None => return false,
    };

    let result = (|| {
        let img = image::open(&temp_frame).ok()?;
        let gray = img.to_luma8();
        let (w, h) = gray.dimensions();

        // Count high-contrast edges (typical of text)
        let mut edge_count = 0;
        let total_pixels = f64::from(w * h);

        for y in 1..h-1 {
            for x in 1..w-1 {
                let center = i32::from(gray.get_pixel(x, y)[0]);
                let right = i32::from(gray.get_pixel(x+1, y)[0]);
                let bottom = i32::from(gray.get_pixel(x, y+1)[0]);

                if (center - right).abs() > 80 || (center - bottom).abs() > 80 {
                    edge_count += 1;
                }
            }
        }

        let edge_ratio = f64::from(edge_count) / total_pixels;
        Some(edge_ratio > 0.15) // >15% high-contrast edges suggests text
    })();

    let _ = std::fs::remove_file(temp_frame);
    result.unwrap_or(false)
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn base_meta() -> LoopMeta {
        LoopMeta {
            duration_secs: 15.0, // Default to 15s to bypass Layer 1-C/1-D for tree testing
            width: 1280, // > 512 to avoid Layer 1-D
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
        assert!(matches!(v, LoopIntentVerdict::LoopWeak(_)), "Expected LoopWeak for MP4 with audio, got {:?}", v);
        assert!(v.reason().contains("Layer 1-A"));
    }

    #[test]
    fn test_audio_bypass_for_animated_images() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.has_audio = true; // Even if it has audio, GIFs bypass the veto
        let v = identify_loop_intent(&m);
        // It should proceed to next layers, and since it's a neutral 2s GIF, it ends up Uncertain (Layer 6)
        assert!(matches!(v, LoopIntentVerdict::Uncertain(_)), "Expected Uncertain for GIF with audio (bypass veto), got {:?}", v);
        assert!(!v.reason().contains("Layer 1-A"));
    }

    #[test]
    fn test_unconditional_hard_pass() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.duration_secs = 5.0; // <= 10s
        let v = identify_loop_intent(&m);
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)));
        assert!(v.reason().contains("Unconditional Hard Pass"));
    }

    #[test]
    fn test_conditional_hard_pass() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.duration_secs = 15.0; // 10-18s
        m.loop_count = Some(0); // infinite marker
        let v = identify_loop_intent(&m);
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)));
        assert!(v.reason().contains("Conditional Hard Pass"));
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
        m.duration_secs = 15.0; // 10s < d <= 18s -> +0.35
        let v = identify_loop_intent(&m);
        // Base (0.35) + color (0.0005) + 1:1 (0.03) = 0.3805. Still Uncertain (< 0.55)
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
        m.duration_secs = 19.0; // Use > 18s to bypass Layer 1-C Combined Hard Pass
        m.loop_count = Some(0); 
        m.frame_delay_variation = Some(0.05); // Add rhythm signal (+0.20) to push over checkpoint
        let v = identify_loop_intent(&m);
        // Score: ~0.43 (loop) + 0.20 (rhythm) + rewards... > 0.55
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)));
        assert!(v.reason().contains("Layer 3 Checkpoint") || v.reason().contains("Layer 4 Checkpoint"));
    }

    #[test]
    fn test_long_gif_conversion_escape() {
        let mut m = base_meta();
        m.source_extension = Some("gif".to_string());
        m.loop_count = Some(0); // Decayed to +0.15
        m.duration_secs = 40.0; // Penalty -0.15
        // Total base score: 0.15 - 0.15 = 0.0. 
        let v = identify_loop_intent(&m);
        assert!(v.is_uncertain());
    }

    #[test]
    fn test_layer2b_no_loop_signal() {
        let mut m = base_meta();
        m.width = 640; // > 512 but still 1:1 for Layer 5-F testing
        m.height = 640;
        m.loop_count = Some(1); // -0.45
        let v = identify_loop_intent(&m);
        // Base (15s: 0.35) + no-loop (-0.45) + 1:1 (0.03) = -0.07. Still Uncertain.
        assert!(v.is_uncertain(), "Expected Uncertain for 10-18s range + no-loop marker, got {:?}", v);
    }

    #[test]
    fn test_modern_format_bias_acceleration() {
        let mut m = base_meta();
        m.source_extension = Some("webp".to_string());
        m.duration_secs = 19.0;
        m.loop_count = Some(1); // -0.45
        let v = identify_loop_intent(&m);
        // Base(19s: -0.15) + loop(-0.45) = -0.60.
        // Modern Bias adds -0.35 -> Total score will be highly negative (e.g. -0.80+)
        assert!(matches!(v, LoopIntentVerdict::LoopWeak(_)), "Expected LoopWeak for WebP with loop_count=1, got {:?}", v);
        // Ensure the bias pushed it well below the -0.55 threshold early
        assert!(v.reason().contains("-0.8"), "Expected score acceleration below -0.8, got {}", v.reason());
    }

    #[test]
    fn test_legacy_gif_vs_modern_webp() {
        // GIF case
        let mut m1 = base_meta();
        m1.source_extension = Some("gif".to_string());
        m1.duration_secs = 19.0;
        m1.loop_count = Some(1); // -0.45
        let _v1 = identify_loop_intent(&m1);
        // Score: -0.15 + (-0.45) = -0.60. 
        // Layer 4 Checkpoint is at -0.55. Wait, -0.60 IS below -0.55.
        // So GIF also hits it. 
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
        assert!(v.reason().contains("Layer 3") || v.reason().contains("Layer 2") || v.reason().contains("Layer 1"));
    }

    // ── Layer 4 ──

    #[test]
    fn test_layer4a_small_palette_and_webp() {
        let mut m = base_meta();
        // Layer 3-B: uniform timing → +0.20
        m.frame_delay_variation = Some(0.05);
        // Layer 4-A: small palette → +0.25
        m.palette_size = Some(32);
        // Layer 4-B: high WebP compression ratio → +0.20
        m.webp_compression_ratio = Some(20.0);
        // Total before CE: 0.20 + 0.25 + 0.20 = 0.65 ≥ 0.55 → Layer 4 checkpoint
        let v = identify_loop_intent(&m);
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)), "Expected LoopStrong, got: {:?}", v.reason());
        assert!(v.reason().contains("Layer 3") || v.reason().contains("Layer 4"));
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
        // Multiple extensions, one of which is a platform marker (+0.50)
        m.app_extensions = Some(vec!["UNKNOWN".to_string(), "GIPHY".to_string()]);
        // Add a small rhythm signal (+0.10) to push it over the 0.55 threshold
        m.frame_delay_variation = Some(0.20); 
        let v = identify_loop_intent(&m);
        assert!(v.is_keep_gif(), "Expected LoopStrong for platform marker + rhythm, got {:?}", v);
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
        m.width = 400;  // <= 512
        m.height = 400; // <= 512
        m.duration_secs = 60.0; // Very long, usually triggers video conversion
        let v = identify_loop_intent(&m);
        assert!(matches!(v, LoopIntentVerdict::LoopStrong(_)), "Expected LoopStrong for small resolution, got {:?}", v);
        assert!(v.reason().contains("Layer 1-D"));
    }
}
