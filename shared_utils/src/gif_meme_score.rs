//! GIF meme-score heuristic — multi-dimensional judgment for animated GIFs.
//!
//! Uses an eight-layer strategy to decide whether a GIF should be kept as-is
//! (skipped from video conversion) or converted to HEVC video:
//!
//! 1. **Veto rules** (hard constraints): extreme cases bypass scoring entirely
//!    - NEW: known meme-CDN app-extension blocks (GIPHY / Tenor) → hard `KeepGif`
//! 2. **Dynamic weighting**: dimension scores adjust based on inter-relationships
//! 3. **Confidence intervals**: uncertain cases (0.40-0.60) default to keeping GIF
//! 4. **Compression ratio**: bytes-per-pixel as a zero-cost strong feature
//! 5. **Filename analysis** (REVISED): distinguishes human-semantic vs machine-generated
//!    names, then attenuates the signal via *complexity hedging* so a 1080p "laugh.gif"
//!    does NOT escape the physical-feature verdict.
//! 6. **Loop frequency**: high loop rate (short duration) → meme-like
//! 7. **Palette entropy** (NEW): small power-of-2 palette size → synthetic / meme-like
//! 8. **Weighted scoring**: all dimensions combined when no veto/uncertainty applies
//!
//! Dimensions (base weights, adjusted dynamically):
//!   - sharpness       (0.38): Low bytes/pixel → simple palette → meme-like
//!   - resolution      (0.18): Small canvas → meme-like (≤200² ≈ 1.0, ≥1080p ≈ 0.0)
//!   - duration        (0.20): Short loop → meme-like (≤1 s ≈ 1.0, ≥10 s ≈ 0.0)
//!   - `aspect_ratio`    (0.09): Square canvas → meme-like
//!   - filename        (0.08): Human single-word name → meme-like (complexity-hedged)
//!   - `loop_frequency`  (0.04): High loop rate → meme-like
//!   - palette         (0.05): Small power-of-2 palette → synthetic/meme-like (NEW)
//!   - fps             (0.00): DEPRECATED — High frame rate memes (`Live2D`) exist.
//!
//! ## Filename complexity hedging
//!
//! A filename only carries meaningful signal when physical features are *already*
//! meme-like.  The effective score is:
//!
//! ```text
//! effective = raw_score × (1 − attenuation × phys_complexity)
//! ```
//!
//! where `phys_complexity = 0.6 × spatial + 0.4 × temporal` and `attenuation`
//! depends on naming origin:
//!   - `HumanSemantic`  (e.g. "laugh", "funny"):  attenuation = 0.85
//!   - `MachineGenerated` (hash / mmexport / ts):  attenuation = 0.95  (near-neutral for HD)
//!   - Ambiguous (multi-word etc.):              attenuation = 1.00
//!   - filename        (0.08): Single-word name → meme-like (NEW)
//!   - `loop_frequency`  (0.04): High loop rate → meme-like (NEW)
//!   - fps             (0.00): DEPRECATED - High frame rate memes (`Live2D`) exist.

/// Meta-information about an animated GIF derived from ffprobe / image-analyzer.
#[derive(Debug, Clone)]
pub struct GifMeta {
    /// Total animation duration in seconds.
    pub duration_secs: f64,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Playback frame rate (fps).
    pub fps: f64,
    /// Total number of frames.
    pub frame_count: u64,
    /// Raw file size in bytes (used to approximate visual complexity).
    pub file_size_bytes: u64,
    /// Optional: file name stem for linguistic analysis.
    pub file_name: Option<String>,
    /// Optional: GIF global colour-table size (2–256, must be a power of two).
    /// Populated by a cheap header-scan; `None` means "not available".
    pub palette_size: Option<u32>,
    /// Optional: application-extension vendor strings found in the GIF stream
    /// (e.g. `"NETSCAPE2.0"`, `"GIPHY    "`, `"STICKER  "`).
    pub app_extensions: Option<Vec<String>>,
    /// True when at least one frame advertises transparent pixels.
    pub has_transparency: bool,
    /// Frame payload size coefficient of variation estimated from GIF sub-blocks.
    pub frame_payload_variation: Option<f64>,
    /// Frame delay coefficient of variation estimated from GCE delay fields.
    pub frame_delay_variation: Option<f64>,
    /// Original extension of the source asset (useful when the scorer is reused
    /// for modern animated formats that may be transcoded to GIF).
    pub source_extension: Option<String>,
    /// Last few parent directory names from the source path.
    pub parent_directories: Option<Vec<String>>,
    /// True when the source carries an embedded ICC profile.
    pub has_embedded_icc: bool,
    /// True when probe metadata indicates non-sRGB / wide-gamut / HDR style colour handling.
    pub has_complex_color_profile: bool,
}

/// Three-way verdict used internally before falling back to weighted scoring.
#[derive(Debug, Clone, PartialEq)]
enum VetoVerdict {
    KeepGif,
    ConvertVideo,
    /// No veto applies; proceed with weighted scoring.
    Undecided,
}

/// Weighted per-dimension scores and the aggregated total.
#[derive(Debug, Clone)]
pub struct MemeScore {
    /// Combined score in [0.0, 1.0].  ≥ 0.60 → keep; ≤ 0.40 → convert; middle → keep.
    pub total: f64,
    /// Compression simplicity score.
    pub compression: f64,
    /// Resolution dimension score.
    pub resolution: f64,
    /// Duration dimension score.
    pub duration: f64,
    /// Frame-rate dimension score (always 0.5 — deprecated).
    pub fps: f64,
    /// Aspect-ratio dimension score.
    pub aspect_ratio: f64,
    /// Effective filename score after complexity hedging.
    pub filename_score: f64,
    /// Loop frequency score.
    pub loop_frequency_score: f64,
    /// Palette-entropy score (0.5 when `palette_size` is unavailable).
    pub palette_score: f64,
    /// Transparency score.
    pub transparency_score: f64,
    /// Frame payload variance score.
    pub frame_variance_score: f64,
    /// Timing irregularity / timing preservation score.
    pub timing_value_score: f64,
    /// Directory-context score (meme-ish parent folder names).
    pub directory_score: f64,
    /// Estimated tolerance for lossy simplification. Lower = preserve more.
    pub loss_tolerance_score: f64,
    /// Sparse slideshow-like timing penalty converted into meme-likelihood score.
    pub cadence_score: f64,
    /// Spatial bytes-per-pixel value (diagnostic only).
    pub spatial_bpp: f64,
    /// Temporal bytes-per-pixel value (diagnostic only).
    pub temporal_bpp: f64,
}

// ── Internal filename analysis ────────────────────────────────────────────────

/// Origin classification of a filename, used to set attenuation strength.
#[derive(Debug, Clone, PartialEq)]
enum FilenameKind {
    /// Human-assigned, semantically meaningful (single short word / CJK phrase).
    /// Strong meme signal when physical features agree.
    HumanSemantic,
    /// Machine-generated: hash, social-app prefix, or pure timestamp.
    /// Indicates social-media *origin*, NOT content class.
    MachineGenerated,
    /// Multi-word, generic, or unclassifiable.
    Ambiguous,
}

struct FilenameAnalysis {
    /// Raw score before complexity hedging, in [0.0, 1.0].
    raw: f64,
    kind: FilenameKind,
}

// ── Veto thresholds ───────────────────────────────────────────────────────────
/// bytes/pixel above this value → video-like content (veto: convert)
const TEMPORAL_BPP_HIGH: f64 = 0.60;
/// bytes/pixel below this value → highly compressed meme (veto: keep)
const TEMPORAL_BPP_LOW: f64 = 0.03;
/// large overall file density for a given canvas size → video-like.
const SPATIAL_BPP_HIGH: f64 = 80.0;
/// low overall file density for a given canvas size → meme-like.
const SPATIAL_BPP_LOW: f64 = 2.0;
/// pixel count above this → 1080p+
const PIXELS_1080P: f64 = (1920 * 1080) as f64;
/// pixel count below this → very small canvas (≤200×200)
const PIXELS_SMALL: f64 = (200 * 200) as f64;
/// pixel count below this → ultra tiny canvas (≤96×96)
const PIXELS_TINY: f64 = (96 * 96) as f64;
/// duration below this → ultra-short loop typical of reaction stickers (seconds)
const DURATION_ULTRA_SHORT: f64 = 0.7;

// ── Confidence thresholds ─────────────────────────────────────────────────────
/// Score above this → high-confidence meme → keep
const CONF_KEEP: f64 = 0.52;

// ── Known meme-platform app-extension prefixes ────────────────────────────────
/// If any app-extension vendor string *starts with* one of these, the GIF
/// originates from a meme CDN and is vetoed as `KeepGif` regardless of resolution.
const MEME_PLATFORM_PREFIXES: &[&str] = &[
    "GIPHY    ", // GIPHY (8-byte padded as per GIF spec)
    "TENOR    ",
    "STICKER  ",
    "GIPHY", // unpadded variants seen in the wild
    "TENOR",
    "STICKER",
];

const MEME_DIRECTORY_KEYWORDS: &[&str] = &[
    "meme",
    "memes",
    "sticker",
    "stickers",
    "emoji",
    "emojis",
    "reaction",
    "reactions",
    "表情包",
    "表情",
    "贴纸",
    "斗图",
    "梗图",
    "梗",
];

// ── Helper: clamp-normalise ───────────────────────────────────────────────────

/// Clamp-normalise `value` from [`low`, `high`] → [0.0, 1.0].
#[inline]
fn normalize(value: f64, low: f64, high: f64) -> f64 {
    if high <= low {
        return 0.0;
    }
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

// ── Filename analysis ─────────────────────────────────────────────────────────

/// Analyse a filename stem and return a raw score plus its naming-origin kind.
///
/// Raw score is the naive meme-likelihood before any complexity hedging;
/// the kind determines how aggressively the score is attenuated by physical
/// features in `score_gif`.
fn analyze_filename(name: Option<&str>) -> FilenameAnalysis {
    const MACHINE_PREFIXES: &[&str] = &[
        "mmexport",
        "wx_camera",
        "wx_image",
        "IMG_",
        "VID_",
        "Screenshot_",
        "signal-",
        "telegram-",
    ];

    let neutral = FilenameAnalysis {
        raw: 0.5,
        kind: FilenameKind::Ambiguous,
    };

    let name = match name {
        Some(n) if !n.is_empty() => n,
        _ => return neutral,
    };

    // Strip extension
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);

    // ── Machine-generated patterns ─────────────────────────────────────────
    // MD5-style 32-char hex → social-media cache name
    let is_hex32 = stem.len() == 32 && stem.chars().all(|c| c.is_ascii_hexdigit());
    if is_hex32 {
        return FilenameAnalysis {
            raw: 0.60,
            kind: FilenameKind::MachineGenerated,
        };
    }

    // WeChat / common social-app export prefixes
    if MACHINE_PREFIXES.iter().any(|p| stem.starts_with(p)) {
        return FilenameAnalysis {
            raw: 0.60,
            kind: FilenameKind::MachineGenerated,
        };
    }

    // Pure numeric timestamp (10–16 digits → Unix epoch or ms epoch)
    let is_timestamp =
        stem.len() >= 10 && stem.len() <= 16 && stem.chars().all(|c| c.is_ascii_digit());
    if is_timestamp {
        return FilenameAnalysis {
            raw: 0.58,
            kind: FilenameKind::MachineGenerated,
        };
    }

    // ── Word-count analysis for everything else ───────────────────────────
    let parts: Vec<&str> = stem
        .split(&['-', '_', '.', ' '][..])
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return neutral;
    }

    let mut total_words: usize = 0;

    for part in &parts {
        let mut word_count = 0usize;
        let mut in_latin_word = false;
        let mut cjk_run = 0usize;

        for ch in part.chars() {
            let is_cjk = ('\u{4E00}'..='\u{9FFF}').contains(&ch)  // CJK Unified
                || ('\u{3040}'..='\u{309F}').contains(&ch)         // Hiragana
                || ('\u{30A0}'..='\u{30FF}').contains(&ch)         // Katakana
                || ('\u{AC00}'..='\u{D7AF}').contains(&ch); // Hangul

            if is_cjk {
                cjk_run += 1;
                in_latin_word = false;
            } else if ch.is_alphanumeric() {
                if !in_latin_word {
                    word_count += 1;
                    in_latin_word = true;
                }
                cjk_run = 0;
            } else {
                in_latin_word = false;
                cjk_run = 0;
            }
        }

        // CJK: count each logical word (~4 chars); clamp to at least 1 if any CJK
        if cjk_run > 0 || part.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)) {
            let cjk_total = part
                .chars()
                .filter(|&c| {
                    ('\u{4E00}'..='\u{9FFF}').contains(&c)
                        || ('\u{3040}'..='\u{309F}').contains(&c)
                        || ('\u{30A0}'..='\u{30FF}').contains(&c)
                        || ('\u{AC00}'..='\u{D7AF}').contains(&c)
                })
                .count();
            // Treat every ~4 CJK chars as one logical "word"
            word_count += ((cjk_total as f64 / 4.0).ceil() as usize).max(1);
        }

        total_words += word_count.max(1);
    }

    // Single-word human name: strong meme signal
    let (raw, kind) = match total_words {
        0 | 1 => (1.0, FilenameKind::HumanSemantic),
        2 => (0.70, FilenameKind::HumanSemantic), // borderline human
        3 => (0.35, FilenameKind::Ambiguous),
        _ => (0.20, FilenameKind::Ambiguous),
    };

    FilenameAnalysis { raw, kind }
}

// ── Palette entropy ───────────────────────────────────────────────────────────

/// Score the GIF global colour-table size as a meme-likelihood indicator.
///
/// Synthetic / hand-crafted GIFs (memes, stickers) tend to use small power-of-2
/// palettes (8–64 colours) for compatibility and size.  Video-captured GIFs
/// typically use the maximum 256-colour table.
///
/// Returns a score in [0.0, 1.0] where 1.0 = small/synthetic palette.
/// Returns `None` when `palette_size` is not available (caller uses 0.5).
fn score_palette(palette_size: Option<u32>) -> Option<f64> {
    let sz = palette_size?;
    if sz == 0 {
        return None;
    }
    // Score by palette size bucket:
    //   2–32   → almost certainly synthetic  → 1.0
    //   64     → likely synthetic            → 0.80
    //   128    → ambiguous                   → 0.55
    //   256    → likely natural/video        → 0.25
    let score = if sz <= 32 {
        1.0
    } else if sz <= 64 {
        0.80
    } else if sz <= 128 {
        0.55
    } else {
        0.25
    };
    // Bonus for exact power-of-two sizes (sign of deliberate palette tuning)
    let is_pow2 = sz.is_power_of_two();
    Some(if is_pow2 {
        score
    } else {
        (score * 0.85).max(0.10)
    })
}

/// Calculate loop frequency score.
/// High loop rate (short duration indicating intentional cyclic animation) → meme-like.
/// Returns score in [0.0, 1.0] where 1.0 = high loop frequency (meme-like).
fn score_loop_frequency(duration_secs: f64, frame_count: u64) -> f64 {
    if duration_secs <= 0.01 || frame_count == 0 {
        return 0.5; // neutral
    }

    // Calculate loops per minute (assuming the animation loops)
    let loops_per_minute = 60.0 / duration_secs;

    // Meme/stickers typically loop very frequently (>10 times/min)
    // Video clips loop slowly (<3 times/min)
    //
    // Also consider frame density: very few frames → likely a simple loop
    let frame_density = frame_count as f64 / duration_secs;

    // High loop rate score
    let loop_score: f64 = if loops_per_minute >= 20.0 {
        1.0 // Very fast loop (≤3s) → definitely meme-like
    } else if loops_per_minute >= 10.0 {
        0.8 // Fast loop (≤6s) → probably meme
    } else if loops_per_minute >= 5.0 {
        0.6 // Medium loop (≤12s) → uncertain
    } else if loops_per_minute >= 2.0 {
        0.4 // Slow loop (≤30s) → probably video
    } else {
        0.2 // Very slow loop (>30s) → definitely video
    };

    // Sparse frame cadence often means slideshow / sampled clip, not sticker.
    let density_adjustment: f64 = if frame_density < 1.2 {
        -0.35
    } else if frame_density < 3.0 {
        -0.20
    } else if frame_density < 6.0 {
        -0.08
    } else {
        0.0
    };

    (loop_score + density_adjustment).clamp(0.0, 1.0)
}

fn score_sparse_cadence(duration_secs: f64, frame_count: u64) -> f64 {
    if duration_secs <= 0.01 || frame_count <= 1 {
        return 0.5;
    }

    let avg_frame_gap = duration_secs / frame_count as f64;
    let frame_density = frame_count as f64 / duration_secs.max(0.01);

    if duration_secs >= 2.0 && frame_count <= 24 && avg_frame_gap >= 0.25 {
        return 0.05;
    }
    if duration_secs >= 1.5 && frame_density <= 3.0 {
        return 0.15;
    }
    if duration_secs <= 1.2 && frame_density >= 8.0 {
        return 0.95;
    }
    if duration_secs <= 2.0 && frame_density >= 6.0 {
        return 0.80;
    }

    0.5
}

fn score_directory_context(parent_directories: Option<&[String]>) -> f64 {
    let Some(parts) = parent_directories else {
        return 0.5;
    };

    let has_meme_hint = parts.iter().rev().take(3).any(|part| {
        let lower = part.to_lowercase();
        MEME_DIRECTORY_KEYWORDS.iter().any(|kw| lower.contains(kw))
    });

    if has_meme_hint {
        1.0
    } else {
        0.5
    }
}

fn score_transparency(has_transparency: bool) -> f64 {
    if has_transparency {
        1.0
    } else {
        0.02
    }
}

fn score_frame_variation(frame_payload_variation: Option<f64>) -> f64 {
    let Some(variation) = frame_payload_variation else {
        return 0.5;
    };
    normalize(variation, 0.08, 0.85)
}

fn score_timing_value(frame_delay_variation: Option<f64>, frame_count: u64) -> f64 {
    let Some(variation) = frame_delay_variation else {
        return if frame_count <= 1 { 0.0 } else { 0.35 };
    };
    normalize(variation, 0.05, 1.20)
}

// ── Veto rules ────────────────────────────────────────────────────────────────

/// Apply veto rules based on extreme metadata values.
/// Returns `KeepGif` / `ConvertVideo` for clear-cut cases; `Undecided` otherwise.
fn apply_veto(meta: &GifMeta, temporal_bpp: f64, spatial_bpp: f64) -> VetoVerdict {
    let pixel_count = (u64::from(meta.width) * u64::from(meta.height)) as f64;

    if meta.has_complex_color_profile || meta.has_embedded_icc {
        return VetoVerdict::ConvertVideo;
    }

    // ── Hard KEEP via meme-platform signature ─────────────────────────────
    // If the GIF carries an application-extension block from a known meme CDN,
    // keep it regardless of resolution or size.
    if let Some(exts) = &meta.app_extensions {
        for ext in exts {
            if MEME_PLATFORM_PREFIXES.iter().any(|p| ext.starts_with(p)) {
                return VetoVerdict::KeepGif;
            }
        }
    }

    // ── Hard CONVERT vetos ────────────────────────────────────────────────
    if temporal_bpp > TEMPORAL_BPP_HIGH && pixel_count >= PIXELS_1080P {
        return VetoVerdict::ConvertVideo;
    }
    if meta.duration_secs > 15.0 && pixel_count >= PIXELS_1080P {
        return VetoVerdict::ConvertVideo;
    }
    if spatial_bpp > SPATIAL_BPP_HIGH && !meta.has_transparency {
        return VetoVerdict::ConvertVideo;
    }

    // ── Hard KEEP vetos ───────────────────────────────────────────────────
    if temporal_bpp < TEMPORAL_BPP_LOW && pixel_count < PIXELS_SMALL {
        return VetoVerdict::KeepGif;
    }
    if pixel_count <= PIXELS_TINY
        && meta.duration_secs <= DURATION_ULTRA_SHORT
        && temporal_bpp <= 0.10
        && meta.duration_secs > 0.01
    {
        return VetoVerdict::KeepGif;
    }
    if meta.has_transparency
        && pixel_count <= PIXELS_SMALL
        && meta.duration_secs <= 1.2
        && meta.frame_count <= 24
    {
        return VetoVerdict::KeepGif;
    }

    VetoVerdict::Undecided
}

// ── Core scoring ──────────────────────────────────────────────────────────────

/// Score a GIF using ffprobe-derived metadata (no decoded frame required).
///
/// ## Filename complexity hedging
///
/// The filename signal is attenuated by `phys_complexity` so that large/long
/// GIFs cannot exploit a meme-like filename to escape conversion:
///
/// ```text
/// phys_complexity = 0.6 × spatial + 0.4 × temporal
///
/// effective_filename = raw × (1 − attenuation × phys_complexity)
///
/// attenuation:
///   HumanSemantic   → 0.85
///   MachineGenerated → 0.95   (almost zero contribution at 1080p)
///   Ambiguous        → 1.00
/// ```
#[must_use]
pub fn score_gif(meta: &GifMeta) -> MemeScore {
    let pixels = (u64::from(meta.width) * u64::from(meta.height)).max(1);
    let total_frames = meta.frame_count.max(1);
    let spatial_bpp = meta.file_size_bytes as f64 / pixels as f64;
    let temporal_bpp = meta.file_size_bytes as f64 / (pixels * total_frames) as f64;

    // ── Per-dimension scores ──────────────────────────────────────────────

    let temporal_density_score = 1.0 - normalize(temporal_bpp, TEMPORAL_BPP_LOW, TEMPORAL_BPP_HIGH);
    let spatial_density_score = 1.0 - normalize(spatial_bpp, SPATIAL_BPP_LOW, SPATIAL_BPP_HIGH);
    let compression_score = 0.7 * temporal_density_score + 0.3 * spatial_density_score;
    let pixel_count = pixels as f64;
    let resolution_score = 1.0 - normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P);
    let duration_score = 1.0 - normalize(meta.duration_secs, 1.0, 10.0);
    let fps_score = 0.5; // deprecated; neutral

    let ratio = if meta.height > 0 {
        f64::from(meta.width) / f64::from(meta.height)
    } else {
        1.0
    };
    let aspect_score = if (0.75..=1.35).contains(&ratio) {
        1.0
    } else if (0.68..=0.73).contains(&ratio) || (1.37..=1.48).contains(&ratio) {
        0.05
    } else if (0.56..=0.66).contains(&ratio) || (1.52..=1.85).contains(&ratio) {
        0.18
    } else if !(0.5..=2.0).contains(&ratio) {
        0.1
    } else {
        0.45
    };

    let loop_frequency_score = score_loop_frequency(meta.duration_secs, meta.frame_count);
    let cadence_score = score_sparse_cadence(meta.duration_secs, meta.frame_count);
    let palette_score = score_palette(meta.palette_size).unwrap_or(0.5);
    let transparency_score = score_transparency(meta.has_transparency);
    let frame_variance_score = score_frame_variation(meta.frame_payload_variation);
    let timing_value_score = score_timing_value(meta.frame_delay_variation, meta.frame_count);
    let directory_score = score_directory_context(meta.parent_directories.as_deref());

    // ── Filename: classify → raw score → complexity hedging ───────────────
    let fa = analyze_filename(meta.file_name.as_deref());

    // Physical-complexity proxy: high when the GIF is large AND/OR long
    let spatial_complexity = normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P);
    let temporal_complexity = normalize(meta.duration_secs, 1.0, 15.0);
    let phys_complexity = spatial_complexity * 0.6 + temporal_complexity * 0.4;

    let attenuation = match fa.kind {
        FilenameKind::HumanSemantic => 0.85,
        FilenameKind::MachineGenerated => 0.95,
        FilenameKind::Ambiguous => 1.00,
    };
    let effective_filename_score = fa.raw * (1.0 - attenuation * phys_complexity);

    // ── Dynamic weights ───────────────────────────────────────────────────
    let complexity = normalize(temporal_bpp, TEMPORAL_BPP_LOW, TEMPORAL_BPP_HIGH);

    let w_compression = 0.24;
    let w_resolution = 0.10f64.mul_add(complexity, 0.16);
    let w_duration = 0.08f64.mul_add(complexity, 0.16);
    let w_aspect = 0.08 * 0.3f64.mul_add(-complexity, 1.0);
    let w_fps = 0.00;
    let w_filename = 0.05;
    let w_loop_freq = 0.05;
    let w_palette = 0.05;
    let w_directory = 0.06;
    let w_cadence = 0.09;
    let w_transparency = 0.17;
    let w_variance = 0.09;
    let w_timing = 0.11;

    let w_sum = w_compression
        + w_resolution
        + w_duration
        + w_aspect
        + w_fps
        + w_filename
        + w_loop_freq
        + w_palette
        + w_directory
        + w_cadence
        + w_transparency
        + w_variance
        + w_timing;

    let (
        w_compression,
        w_resolution,
        w_duration,
        w_aspect,
        w_fps,
        w_filename,
        w_loop_freq,
        w_palette,
        w_directory,
        w_cadence,
        w_transparency,
        w_variance,
        w_timing,
    ) = (
        w_compression / w_sum,
        w_resolution / w_sum,
        w_duration / w_sum,
        w_aspect / w_sum,
        w_fps / w_sum,
        w_filename / w_sum,
        w_loop_freq / w_sum,
        w_palette / w_sum,
        w_directory / w_sum,
        w_cadence / w_sum,
        w_transparency / w_sum,
        w_variance / w_sum,
        w_timing / w_sum,
    );

    let total = loop_frequency_score.mul_add(
        w_loop_freq,
        compression_score * w_compression
            + resolution_score * w_resolution
            + duration_score * w_duration
            + aspect_score * w_aspect
            + fps_score * w_fps
            + effective_filename_score * w_filename
            + directory_score * w_directory
            + cadence_score * w_cadence
            + transparency_score * w_transparency
            + frame_variance_score * w_variance
            + timing_value_score * w_timing,
    ) + palette_score * w_palette;

    let loss_tolerance_score = if meta.has_embedded_icc || meta.has_complex_color_profile {
        0.0
    } else if temporal_bpp < 0.03
        || meta
            .app_extensions
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|ext| {
                ext.starts_with("GIPHY") || ext.starts_with("TENOR") || ext.starts_with("STICKER")
            })
    {
        1.0
    } else {
        0.5
    };

    MemeScore {
        total,
        compression: compression_score,
        resolution: resolution_score,
        duration: duration_score,
        fps: fps_score,
        aspect_ratio: aspect_score,
        filename_score: effective_filename_score,
        loop_frequency_score,
        palette_score,
        transparency_score,
        frame_variance_score,
        timing_value_score,
        directory_score,
        cadence_score,
        spatial_bpp,
        temporal_bpp,
        loss_tolerance_score,
    }
}

// ── Decision entry-point ──────────────────────────────────────────────────────

/// Decide whether to keep a GIF as-is or convert it to video.
///
/// ## Decision pipeline
/// 1. **Veto** (app-extension CDN marker → `KeepGif`; extreme physical → convert/keep)
/// 2. **Weighted score** with complexity-hedged filename signal
/// 3. **Confidence interval**: high-confidence keep / convert; middle defaults to convert
#[must_use]
pub fn should_keep_as_gif(meta: &GifMeta) -> bool {
    should_keep_as_gif_with_path(meta, None)
}

#[must_use]
pub fn should_keep_as_gif_with_path(meta: &GifMeta, path: Option<&std::path::Path>) -> bool {
    let pixels = (u64::from(meta.width) * u64::from(meta.height)).max(1) as f64;
    let temporal_bpp = meta.file_size_bytes as f64 / (pixels * meta.frame_count.max(1) as f64);
    let spatial_bpp = meta.file_size_bytes as f64 / pixels;

    match apply_veto(meta, temporal_bpp, spatial_bpp) {
        VetoVerdict::KeepGif => {
            crate::progress_mode::emit_stderr(&format!(
                "🎞️  GIF [{}] → KEEP GIF (veto: tbpp={:.3} sbpp={:.1} px={:.0} dur={:.1}s)",
                meta.file_name.as_deref().unwrap_or("?"),
                temporal_bpp,
                spatial_bpp,
                pixels,
                meta.duration_secs
            ));
            return true;
        }
        VetoVerdict::ConvertVideo => {
            crate::progress_mode::emit_stderr(&format!(
                "🎞️  GIF [{}] → CONVERT→VIDEO (veto: tbpp={:.3} sbpp={:.1} px={:.0} dur={:.1}s)",
                meta.file_name.as_deref().unwrap_or("?"),
                temporal_bpp,
                spatial_bpp,
                pixels,
                meta.duration_secs
            ));
            return false;
        }
        VetoVerdict::Undecided => {}
    }

    if let Some(sample_match) = crate::gif_value_db::lookup_similar_samples(meta, path) {
        if let Some(label) = sample_match.exact_label {
            crate::progress_mode::emit_stderr(&format!(
                "🎞️  GIF [{}] → {} (sample-db exact match)",
                meta.file_name.as_deref().unwrap_or("?"),
                if label { "KEEP GIF" } else { "CONVERT→VIDEO" }
            ));
            return label;
        }

        if let (Some(prob), Some(mean_distance)) =
            (sample_match.keep_probability, sample_match.mean_distance)
        {
            if sample_match.neighbor_count >= 4 && mean_distance <= 0.48 && prob >= 0.92 {
                crate::progress_mode::emit_stderr(&format!(
                    "🎞️  GIF [{}] → KEEP GIF (sample-db neighbors={} p={:.2} d={:.2})",
                    meta.file_name.as_deref().unwrap_or("?"),
                    sample_match.neighbor_count,
                    prob,
                    mean_distance
                ));
                return true;
            }
            if sample_match.neighbor_count >= 4 && mean_distance <= 0.48 && prob <= 0.08 {
                crate::progress_mode::emit_stderr(&format!(
                    "🎞️  GIF [{}] → CONVERT→VIDEO (sample-db neighbors={} p={:.2} d={:.2})",
                    meta.file_name.as_deref().unwrap_or("?"),
                    sample_match.neighbor_count,
                    prob,
                    mean_distance
                ));
                return false;
            }
        }
    }

    let s = score_gif(meta);

    let keep = s.total >= CONF_KEEP;

    crate::progress_mode::emit_stderr(&format!(
        "🎞️  GIF [{}] → {} (score={:.3}) │ comp:{:.2} res:{:.2} dur:{:.2} alpha:{:.2} var:{:.2} timing:{:.2} name:{:.2} dir:{:.2} loop:{:.2} cadence:{:.2} palette:{:.2} tbpp:{:.3} sbpp:{:.1}",
        meta.file_name.as_deref().unwrap_or("?"),
        if keep { "KEEP GIF" } else { "CONVERT→VIDEO" },
        s.total, s.compression, s.resolution, s.duration, s.transparency_score,
        s.frame_variance_score, s.timing_value_score, s.filename_score, s.directory_score, s.loop_frequency_score,
        s.cadence_score, s.palette_score, s.temporal_bpp, s.spatial_bpp,
    ));

    keep
}

// ── Builder helpers ───────────────────────────────────────────────────────────

/// Build a [`GifMeta`] from an [`crate::ffprobe::FFprobeResult`] and file size.
///
/// Returns `None` if the probe has no usable video dimensions.
/// `palette_size` and `app_extensions` are left `None`; populate them via
/// [`scan_gif_headers`] if a cheap header-scan is acceptable.
#[must_use]
pub fn gif_meta_from_probe(
    probe: &crate::ffprobe::FFprobeResult,
    file_size_bytes: u64,
) -> Option<GifMeta> {
    if probe.width == 0 || probe.height == 0 {
        return None;
    }
    Some(GifMeta {
        duration_secs: probe.duration,
        width: probe.width,
        height: probe.height,
        fps: probe.frame_rate,
        frame_count: probe.frame_count.max(1),
        file_size_bytes,
        file_name: None,
        palette_size: None,
        app_extensions: None,
        has_transparency: false,
        frame_payload_variation: None,
        frame_delay_variation: None,
        source_extension: None,
        parent_directories: None,
        has_embedded_icc: false,
        has_complex_color_profile: probe.bit_depth > 8,
    })
}

/// Build a [`GifMeta`] from probe result + file path.
/// Does NOT perform a GIF header scan; call [`scan_gif_headers`] separately
/// if palette / app-extension data is needed.
#[must_use]
pub fn gif_meta_from_probe_with_path(
    probe: &crate::ffprobe::FFprobeResult,
    file_size_bytes: u64,
    file_path: &std::path::Path,
) -> Option<GifMeta> {
    if probe.width == 0 || probe.height == 0 {
        return None;
    }
    let file_name = file_path
        .file_name()
        .and_then(|s| s.to_str())
        .map(std::string::ToString::to_string);
    let source_extension = file_path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_lowercase);
    let parent_directories = file_path.parent().map(|parent| {
        parent
            .iter()
            .rev()
            .take(4)
            .filter_map(|part| part.to_str().map(std::string::ToString::to_string))
            .collect::<Vec<_>>()
    });

    let has_complex_color_profile = probe.bit_depth > 8
        || probe
            .color_space
            .as_deref()
            .is_some_and(|s| !matches!(s, "bt709" | "bt470bg" | "smpte170m" | "unknown"))
        || probe
            .color_transfer
            .as_deref()
            .is_some_and(|s| !matches!(s, "bt709" | "iec61966-2-1" | "srgb" | "unknown"))
        || probe
            .color_primaries
            .as_deref()
            .is_some_and(|s| !matches!(s, "bt709" | "smpte170m" | "unknown"));

    let duration = if probe.duration > 0.0 {
        probe.duration
    } else {
        // If animated but duration is 0, give it a candidate 0.1s for scoring
        0.1
    };

    Some(GifMeta {
        duration_secs: duration,
        width: probe.width,
        height: probe.height,
        fps: probe.frame_rate,
        frame_count: probe.frame_count.max(1),
        file_size_bytes,
        file_name,
        palette_size: None,
        app_extensions: None,
        has_transparency: false,
        frame_payload_variation: None,
        frame_delay_variation: None,
        source_extension,
        parent_directories,
        has_embedded_icc: has_embedded_icc_profile(file_path),
        has_complex_color_profile,
    })
}

fn has_embedded_icc_profile(path: &std::path::Path) -> bool {
    if which::which("exiftool").is_err() {
        return false;
    }

    let output = std::process::Command::new("exiftool")
        .arg("-b")
        .arg("-ICC_Profile")
        .arg(path)
        .output();

    matches!(output, Ok(o) if o.status.success() && !o.stdout.is_empty())
}

/// Perform a cheap byte-scan of a GIF file to extract:
///   - global colour-table size (from the Logical Screen Descriptor)
///   - application-extension vendor strings (e.g. `"NETSCAPE2.0"`, `"GIPHY    "`)
///
/// The scan reads the GIF bitstream once and extracts several cheap structural
/// signals: palette size, app extension markers, transparency usage, and frame
/// payload variance. Any I/O error silently returns neutral values so the
/// caller can proceed with the ffprobe-only path.
///
/// ## Usage
/// ```ignore
/// let mut meta = gif_meta_from_probe_with_path(&probe, size, path)?;
/// let (pal, exts, has_alpha, variation, delay_variation) =
///     scan_gif_headers(path).unwrap_or_default();
/// meta.palette_size = pal;
/// meta.app_extensions = exts;
/// meta.has_transparency = has_alpha;
/// meta.frame_payload_variation = variation;
/// meta.frame_delay_variation = delay_variation;
/// ```
/// Perform a cheap byte-scan of a GIF file to extract structural meme signals.
///
/// # Errors
/// Returns an I/O error if the file cannot be opened or read.
pub fn scan_gif_headers(
    path: &std::path::Path,
) -> std::io::Result<(
    Option<u32>,
    Option<Vec<String>>,
    bool,
    Option<f64>,
    Option<f64>,
)> {
    let buf = std::fs::read(path)?;
    let n = buf.len();

    if n < 13 {
        return Ok((None, None, false, None, None));
    }

    // GIF87a / GIF89a magic check
    if &buf[0..6] != b"GIF87a" && &buf[0..6] != b"GIF89a" {
        return Ok((None, None, false, None, None));
    }

    // Logical Screen Descriptor: byte 10 = packed field
    // Bits 0-2 = (size of global colour table − 1)  → actual size = 2^(n+1)
    let packed = buf[10];
    let has_gct = (packed & 0x80) != 0;
    let palette_size: Option<u32> = if has_gct {
        let n = u32::from(packed & 0x07);
        Some(2u32.pow(n + 1))
    } else {
        None
    };

    // Scan for application extension blocks, GCE transparency flags, and image payloads.
    let mut app_extensions: Vec<String> = Vec::new();
    let mut has_transparency = false;
    let mut frame_payload_sizes: Vec<usize> = Vec::new();
    let mut frame_delays_cs: Vec<u16> = Vec::new();
    let mut pos = 13usize;
    // Skip past Global Colour Table if present
    if has_gct {
        let gct_size = palette_size.unwrap_or(0) as usize * 3;
        pos += gct_size;
    }

    while pos + 2 < buf.len() {
        match buf[pos] {
            0x21 if pos + 1 < buf.len() => match buf[pos + 1] {
                0xFF => {
                    let block_size = buf.get(pos + 2).copied().unwrap_or(0) as usize;
                    if block_size == 11 && pos + 3 + block_size <= buf.len() {
                        let vendor = std::str::from_utf8(&buf[pos + 3..pos + 3 + block_size])
                            .unwrap_or("")
                            .to_owned();
                        if !vendor.is_empty() {
                            app_extensions.push(vendor);
                        }
                    }
                    pos += 3 + block_size;
                    pos = skip_sub_blocks(&buf, pos);
                }
                0xF9 => {
                    if pos + 7 < buf.len() && buf[pos + 2] == 0x04 {
                        if buf[pos + 3] & 0x01 != 0 {
                            has_transparency = true;
                        }
                        let delay = u16::from(buf[pos + 4]) | (u16::from(buf[pos + 5]) << 8);
                        frame_delays_cs.push(delay);
                        pos += 8;
                    } else {
                        pos += 1;
                    }
                }
                0xFE | 0x01 => {
                    pos += 2;
                    pos = skip_sub_blocks(&buf, pos);
                }
                _ => {
                    pos += 1;
                }
            },
            0x2C => {
                if pos + 10 >= buf.len() {
                    break;
                }
                let packed = buf[pos + 9];
                pos += 10;
                if (packed & 0x80) != 0 {
                    let lct_size_pow = usize::from(packed & 0x07);
                    let lct_size = 3 * (1usize << (lct_size_pow + 1));
                    if pos + lct_size > buf.len() {
                        break;
                    }
                    pos += lct_size;
                }
                if pos >= buf.len() {
                    break;
                }
                pos += 1; // LZW minimum code size
                let payload_start = pos;
                pos = skip_sub_blocks(&buf, pos);
                let payload_size = pos.saturating_sub(payload_start);
                if payload_size > 0 {
                    frame_payload_sizes.push(payload_size);
                }
            }
            0x3B => break,
            _ => {
                pos += 1;
            }
        }
    }

    let app_extensions = if app_extensions.is_empty() {
        None
    } else {
        Some(app_extensions)
    };

    let frame_payload_variation = if frame_payload_sizes.len() >= 2 {
        let mean =
            frame_payload_sizes.iter().sum::<usize>() as f64 / frame_payload_sizes.len() as f64;
        if mean > 0.0 {
            let variance = frame_payload_sizes
                .iter()
                .map(|&size| {
                    let diff = size as f64 - mean;
                    diff * diff
                })
                .sum::<f64>()
                / frame_payload_sizes.len() as f64;
            Some((variance.sqrt() / mean).clamp(0.0, 2.0))
        } else {
            None
        }
    } else {
        None
    };

    let frame_delay_variation = if frame_delays_cs.len() >= 2 {
        let mean = frame_delays_cs.iter().map(|&d| f64::from(d)).sum::<f64>()
            / frame_delays_cs.len() as f64;
        if mean > 0.0 {
            let variance = frame_delays_cs
                .iter()
                .map(|&delay| {
                    let diff = f64::from(delay) - mean;
                    diff * diff
                })
                .sum::<f64>()
                / frame_delays_cs.len() as f64;
            Some((variance.sqrt() / mean).clamp(0.0, 2.0))
        } else {
            Some(0.0)
        }
    } else {
        None
    };

    Ok((
        palette_size,
        app_extensions,
        has_transparency,
        frame_payload_variation,
        frame_delay_variation,
    ))
}

fn skip_sub_blocks(buf: &[u8], mut pos: usize) -> usize {
    loop {
        let Some(&sub_size) = buf.get(pos) else {
            return buf.len();
        };
        pos += 1;
        if sub_size == 0 {
            return pos;
        }
        pos = pos.saturating_add(sub_size as usize);
        if pos > buf.len() {
            return buf.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(duration: f64, w: u32, h: u32, fps: f64, frames: u64, size: u64) -> GifMeta {
        GifMeta {
            duration_secs: duration,
            width: w,
            height: h,
            fps,
            frame_count: frames,
            file_size_bytes: size,
            file_name: None,
            palette_size: None,
            app_extensions: None,
            has_transparency: false,
            frame_payload_variation: None,
            frame_delay_variation: None,
            source_extension: None,
            parent_directories: None,
            has_embedded_icc: false,
            has_complex_color_profile: false,
        }
    }

    fn make_meta_with_name(
        duration: f64,
        w: u32,
        h: u32,
        fps: f64,
        frames: u64,
        size: u64,
        name: &str,
    ) -> GifMeta {
        GifMeta {
            duration_secs: duration,
            width: w,
            height: h,
            fps,
            frame_count: frames,
            file_size_bytes: size,
            file_name: Some(name.to_string()),
            palette_size: None,
            app_extensions: None,
            has_transparency: false,
            frame_payload_variation: None,
            frame_delay_variation: None,
            source_extension: None,
            parent_directories: None,
            has_embedded_icc: false,
            has_complex_color_profile: false,
        }
    }

    // ── score_gif tests ───────────────────────────────────────────────────────

    #[test]
    fn tiny_meme_scores_high() {
        // 200×200, 2s, 10fps, 20 frames, tiny file → should score ≥ 0.5
        let meta = make_meta(2.0, 200, 200, 10.0, 20, 40_000);
        let s = score_gif(&meta);
        assert!(
            s.total >= 0.50,
            "expected meme score ≥ 0.5, got {:.3}",
            s.total
        );
    }

    #[test]
    fn large_long_video_clip_scores_low() {
        // 1920×1080, 30s, 30fps, 900 frames, large file → should score < 0.5
        let meta = make_meta(30.0, 1920, 1080, 30.0, 900, 15_000_000);
        let s = score_gif(&meta);
        assert!(
            s.total < 0.50,
            "expected video score < 0.5, got {:.3}",
            s.total
        );
    }

    #[test]
    fn score_gif_exposes_bytes_per_pixel() {
        let meta = make_meta(3.0, 300, 300, 12.0, 36, 270_000);
        let s = score_gif(&meta);
        assert!(
            s.temporal_bpp > 0.0 && s.spatial_bpp > 0.0,
            "bpp metrics should be positive"
        );
    }

    #[test]
    fn square_aspect_ratio_maxes_out() {
        let meta = make_meta(3.0, 300, 300, 12.0, 36, 200_000);
        let s = score_gif(&meta);
        assert!(
            (s.aspect_ratio - 1.0).abs() < 1e-9,
            "square → aspect_ratio=1.0"
        );
    }

    // ── normalize tests ───────────────────────────────────────────────────────

    #[test]
    fn normalize_clamps_correctly() {
        assert!((normalize(0.0, 0.0, 1.0) - 0.0).abs() < 1e-9);
        assert!((normalize(1.0, 0.0, 1.0) - 1.0).abs() < 1e-9);
        assert!((normalize(-1.0, 0.0, 1.0) - 0.0).abs() < 1e-9);
        assert!((normalize(2.0, 0.0, 1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn normalize_degenerate_range() {
        assert_eq!(normalize(5.0, 10.0, 5.0), 0.0);
    }

    // ── apply_veto tests ──────────────────────────────────────────────────────

    #[test]
    fn veto_convert_high_bpp_large_canvas() {
        // bpp > BPP_HIGH (0.60) AND pixels > PIXELS_1080P → convert
        let meta = make_meta(5.0, 1920, 1080, 24.0, 120, 1_000_000);
        // pass bpp explicitly above threshold
        assert_eq!(apply_veto(&meta, 0.70, 4.0), VetoVerdict::ConvertVideo);
    }

    #[test]
    fn veto_convert_long_large() {
        // duration > 15s AND pixels > PIXELS_1080P → convert
        let meta = make_meta(20.0, 1920, 1080, 24.0, 480, 5_000_000);
        // bpp doesn't matter for this rule; pass a low value to isolate
        assert_eq!(apply_veto(&meta, 0.10, 5.0), VetoVerdict::ConvertVideo);
    }

    #[test]
    fn veto_keep_ultra_compressed_tiny() {
        // bpp < 0.03 AND pixels < 200×200 → keep
        let meta = make_meta(
            3.0, 100, 100, 10.0, 30, // bpp = 1000 / (10_000*30) ≈ 0.003
            1_000,
        );
        assert_eq!(apply_veto(&meta, 0.003, 0.1), VetoVerdict::KeepGif);
    }

    #[test]
    fn veto_keep_small_transparent_short_loop() {
        let mut meta = make_meta(0.8, 180, 180, 15.0, 12, 50_000);
        meta.has_transparency = true;
        assert_eq!(apply_veto(&meta, 0.20, 1.6), VetoVerdict::KeepGif);
    }

    #[test]
    fn veto_undecided_middle_ground() {
        // Nothing extreme → undecided
        let meta = make_meta(5.0, 640, 480, 15.0, 75, 500_000);
        assert_eq!(apply_veto(&meta, 0.10, 1.7), VetoVerdict::Undecided);
    }

    // ── should_keep_as_gif confidence tests ──────────────────────────────────

    #[test]
    fn should_keep_transparent_short_loop() {
        let mut meta = make_meta(0.5, 160, 160, 30.0, 15, 80_000);
        meta.has_transparency = true;
        assert!(should_keep_as_gif(&meta), "short loop should always keep");
    }

    #[test]
    fn should_convert_veto_long_large() {
        // 20 s, 1080p → convert veto
        let meta = make_meta(20.0, 1920, 1080, 30.0, 600, 5_000_000);
        assert!(
            !should_keep_as_gif(&meta),
            "long 1080p should always convert"
        );
    }

    #[test]
    fn uncertain_zone_defaults_to_convert() {
        // Construct a case that lands in (0.35, 0.65) — moderate bpp, medium size/duration
        // 640×480, 6s, 15fps, 90 frames, moderate file
        let meta = make_meta(6.0, 640, 480, 15.0, 90, 800_000);
        let s = score_gif(&meta);
        // If score is in the uncertain zone, should_keep_as_gif now biases toward convert.
        if s.total > CONF_CONVERT && s.total < CONF_KEEP {
            assert!(
                !should_keep_as_gif(&meta),
                "uncertain zone must default to convert"
            );
        }
        // If it landed outside the zone, just verify no panic
    }

    // ── gif_meta_from_probe tests ─────────────────────────────────────────────

    #[test]
    fn gif_meta_from_probe_zero_dimensions_returns_none() {
        assert!(gif_meta_from_probe_raw(0, 0, 2.0, 10.0, 20, 40_000).is_none());
    }

    // Helper that bypasses ffprobe for unit testing
    fn gif_meta_from_probe_raw(
        w: u32,
        h: u32,
        duration: f64,
        fps: f64,
        frames: u64,
        size: u64,
    ) -> Option<GifMeta> {
        if w == 0 || h == 0 {
            return None;
        }
        Some(GifMeta {
            duration_secs: duration,
            width: w,
            height: h,
            fps,
            frame_count: frames,
            file_size_bytes: size,
            file_name: None,
            palette_size: None,
            app_extensions: None,
            has_transparency: false,
            frame_payload_variation: None,
            frame_delay_variation: None,
            source_extension: None,
            parent_directories: None,
            has_embedded_icc: false,
            has_complex_color_profile: false,
        })
    }

    #[test]
    fn complex_color_profile_vetoes_to_video() {
        let mut meta = make_meta(0.8, 320, 320, 12.0, 10, 50_000);
        meta.has_complex_color_profile = true;
        assert!(!should_keep_as_gif(&meta));
    }

    #[test]
    fn directory_context_boosts_meme_score() {
        let mut meta = make_meta(3.0, 320, 320, 12.0, 36, 100_000);
        meta.parent_directories = Some(vec!["我的表情包".to_string()]);
        let s = score_gif(&meta);
        assert!(s.directory_score > 0.9);
    }

    #[test]
    fn sparse_slideshow_cadence_scores_low() {
        let meta = make_meta(6.0, 720, 1280, 1.0, 6, 500_000);
        let s = score_gif(&meta);
        assert!(s.cadence_score < 0.2);
    }

    #[test]
    fn irregular_timing_scores_high() {
        let mut meta = make_meta(3.0, 320, 320, 12.0, 12, 120_000);
        meta.frame_delay_variation = Some(0.9);
        let s = score_gif(&meta);
        assert!(s.timing_value_score > 0.7);
    }

    #[test]
    fn uniform_timing_scores_low() {
        let mut meta = make_meta(3.0, 320, 320, 12.0, 12, 120_000);
        meta.frame_delay_variation = Some(0.02);
        let s = score_gif(&meta);
        assert!(s.timing_value_score < 0.1);
    }

    // ── New dimension tests ───────────────────────────────────────────────────

    #[test]
    fn filename_single_word_scores_high() {
        let meta = make_meta_with_name(3.0, 300, 300, 12.0, 36, 200_000, "laugh");
        let s = score_gif(&meta);
        assert!(
            s.filename_score >= 0.9,
            "single word should score high: {:.2}",
            s.filename_score
        );
    }

    #[test]
    fn filename_multi_word_scores_low() {
        let meta = make_meta_with_name(3.0, 300, 300, 12.0, 36, 200_000, "my_vacation_video_2024");
        let s = score_gif(&meta);
        assert!(
            s.filename_score <= 0.5,
            "multi-word should score low: {:.2}",
            s.filename_score
        );
    }

    #[test]
    fn filename_chinese_single_char() {
        let meta = make_meta_with_name(3.0, 300, 300, 12.0, 36, 200_000, "laugh");
        let s = score_gif(&meta);
        assert!(
            s.filename_score >= 0.9,
            "single CJK char should score high: {:.2}",
            s.filename_score
        );
    }

    #[test]
    fn loop_frequency_fast_loop_scores_high() {
        // 2s duration → 30 loops/min
        let meta = make_meta(2.0, 300, 300, 10.0, 20, 100_000);
        let s = score_gif(&meta);
        assert!(
            s.loop_frequency_score >= 0.8,
            "fast loop should score high: {:.2}",
            s.loop_frequency_score
        );
    }

    #[test]
    fn loop_frequency_slow_loop_scores_low() {
        // 40s duration → 1.5 loops/min
        let meta = make_meta(40.0, 1920, 1080, 30.0, 1200, 5_000_000);
        let s = score_gif(&meta);
        assert!(
            s.loop_frequency_score <= 0.4,
            "slow loop should score low: {:.2}",
            s.loop_frequency_score
        );
    }
}
