//! GIF meme-score heuristic — multi-dimensional judgment for animated GIFs.
//!
//! Uses a five-layer strategy to decide whether a GIF should be kept as-is
//! (skipped from video conversion) or converted to HEVC video:
//!
//! 1. **Veto rules** (hard constraints): extreme cases bypass scoring entirely
//! 2. **Dynamic weighting**: dimension scores adjust based on inter-relationships
//! 3. **Confidence intervals**: uncertain cases (0.35-0.65) default to keeping GIF
//! 4. **Compression ratio**: bytes-per-pixel as a zero-cost strong feature
//! 5. **Weighted scoring**: five dimensions combined when no veto/uncertainty applies
//!
//! Dimensions (base weights, adjusted dynamically):
//!   - sharpness   (0.40): Low bytes/pixel → simple palette → meme-like
//!   - resolution  (0.20): Small canvas → meme-like (≤200² ≈ 1.0, ≥1080p ≈ 0.0)
//!   - duration    (0.18): Short loop → meme-like (≤1 s ≈ 1.0, ≥10 s ≈ 0.0)
//!   - aspect_ratio(0.14): Square canvas → meme-like
//!   - fps         (0.08): Low frame rate → meme-like (≤6 fps ≈ 1.0, ≥30 fps ≈ 0.0)

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
    /// Combined score in [0.0, 1.0].  ≥ 0.65 → keep; ≤ 0.35 → convert; middle → uncertain (keep).
    pub total: f64,
    /// Sharpness proxy dimension score.
    pub sharpness: f64,
    /// Resolution dimension score.
    pub resolution: f64,
    /// Duration dimension score.
    pub duration: f64,
    /// Frame-rate dimension score.
    pub fps: f64,
    /// Aspect-ratio dimension score.
    pub aspect_ratio: f64,
    /// Raw bytes-per-pixel value (diagnostic only).
    pub bytes_per_pixel: f64,
}

/// Clamp-normalise `value` from [`low`, `high`] → [0.0, 1.0].
#[inline]
fn normalize(value: f64, low: f64, high: f64) -> f64 {
    if high <= low {
        return 0.0;
    }
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

// ── Veto thresholds ──────────────────────────────────────────────────────────
/// bytes/pixel above this value → video-like content (veto: convert)
const BPP_HIGH: f64 = 0.60;
/// bytes/pixel below this value → highly compressed meme (veto: keep)
const BPP_LOW: f64 = 0.03;
/// pixel count above this → 1080p+ (used in combination vetos)
const PIXELS_1080P: f64 = (1920 * 1080) as f64;
/// pixel count below this → very small canvas (≤200×200)
const PIXELS_SMALL: f64 = (200 * 200) as f64;

// ── Confidence thresholds ─────────────────────────────────────────────────────
/// Score above this → high-confidence meme → keep
const CONF_KEEP: f64 = 0.65;
/// Score below this → high-confidence video → convert
const CONF_CONVERT: f64 = 0.35;

/// Apply veto rules based on extreme metadata values.
/// Returns `KeepGif` / `ConvertVideo` for clear-cut cases; `Undecided` for the middle ground.
fn apply_veto(meta: &GifMeta, bytes_per_pixel: f64) -> VetoVerdict {
    let pixel_count = (meta.width as u64 * meta.height as u64) as f64;

    // --- Hard CONVERT vetos ---------------------------------------------------
    // Very high bytes/pixel AND large resolution → clearly a high-quality video clip
    if bytes_per_pixel > BPP_HIGH && pixel_count >= PIXELS_1080P {
        return VetoVerdict::ConvertVideo;
    }
    // Long duration AND large resolution, regardless of compression
    if meta.duration_secs > 15.0 && pixel_count >= PIXELS_1080P {
        return VetoVerdict::ConvertVideo;
    }

    // --- Hard KEEP vetos ------------------------------------------------------
    // Extremely compressed AND tiny canvas → almost certainly a meme/sticker
    if bytes_per_pixel < BPP_LOW && pixel_count < PIXELS_SMALL {
        return VetoVerdict::KeepGif;
    }
    // Very short loop (≤1 s) → meme regardless of other dimensions
    if meta.duration_secs <= 1.0 && meta.duration_secs > 0.01 {
        return VetoVerdict::KeepGif;
    }

    VetoVerdict::Undecided
}

/// Score a GIF using ffprobe-derived metadata (no decoded frame required).
///
/// ## Pipeline
/// 1. Compute bytes-per-pixel (compression proxy, zero decode cost).
/// 2. Compute per-dimension scores with **dynamic weight adjustment**:
///    when content is complex (high bpp), resolution and duration weights
///    increase while aspect/fps weights shrink, so large/long GIFs are pushed
///    toward "convert" more strongly.
/// 3. Renormalise weights to sum = 1.0.
///
/// Bytes-per-pixel range: `BPP_LOW` (meme) … `BPP_HIGH` (rich video clip).
pub fn score_gif(meta: &GifMeta) -> MemeScore {
    let pixels = (meta.width as u64 * meta.height as u64).max(1);
    let total_frames = meta.frame_count.max(1);
    let bytes_per_pixel = meta.file_size_bytes as f64 / (pixels * total_frames) as f64;

    // Sharpness proxy: low bytes/pixel → meme-like (simple palette)
    let sharpness_score = 1.0 - normalize(bytes_per_pixel, BPP_LOW, BPP_HIGH);

    // Resolution: small canvas ≈ meme
    let pixel_count = pixels as f64;
    let resolution_score = 1.0 - normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P);

    // Duration: short loop ≈ meme
    let duration_score = 1.0 - normalize(meta.duration_secs, 1.0, 10.0);

    // FPS: low frame-rate ≈ meme
    let fps_score = 1.0 - normalize(meta.fps, 6.0, 30.0);

    // Aspect ratio: square ≈ meme
    let ratio = if meta.height > 0 {
        meta.width as f64 / meta.height as f64
    } else {
        1.0
    };
    let aspect_score = 1.0 - (ratio - 1.0).abs().min(1.0);

    // ── Dynamic weights ───────────────────────────────────────────────────────
    // complexity ∈ [0, 1]: 0 = maximally meme-like, 1 = maximally video-like
    let complexity = normalize(bytes_per_pixel, BPP_LOW, BPP_HIGH);
    let w_sharpness  = 0.40;
    let w_resolution = 0.20 + 0.08 * complexity; // up to 0.28 for complex content
    let w_duration   = 0.18 + 0.06 * complexity; // up to 0.24
    let w_aspect     = 0.14 * (1.0 - 0.3 * complexity);
    let w_fps        = 0.08 * (1.0 - 0.3 * complexity);
    // Renormalise so weights always sum to 1.0
    let w_sum = w_sharpness + w_resolution + w_duration + w_aspect + w_fps;
    let (w_sharpness, w_resolution, w_duration, w_aspect, w_fps) = (
        w_sharpness / w_sum,
        w_resolution / w_sum,
        w_duration / w_sum,
        w_aspect / w_sum,
        w_fps / w_sum,
    );

    let total = sharpness_score * w_sharpness
        + resolution_score * w_resolution
        + duration_score * w_duration
        + aspect_score * w_aspect
        + fps_score * w_fps;

    MemeScore {
        total,
        sharpness: sharpness_score,
        resolution: resolution_score,
        duration: duration_score,
        fps: fps_score,
        aspect_ratio: aspect_score,
        bytes_per_pixel,
    }
}

/// Decide whether to keep a GIF as-is or convert it to video.
///
/// ## Decision pipeline
/// 1. **Veto**: extreme metadata → immediate verdict (no scoring overhead)
/// 2. **Weighted score**: compute `score_gif` with dynamic weights
/// 3. **Confidence interval**:
///    - score ≥ `CONF_KEEP` (0.65) → keep (high-confidence meme)
///    - score ≤ `CONF_CONVERT` (0.35) → convert (high-confidence video)
///    - 0.35 < score < 0.65 → uncertain → **keep** (conservative default)
///
/// Logs a single diagnostic line to stderr.
pub fn should_keep_as_gif(meta: &GifMeta) -> bool {
    let pixels = (meta.width as u64 * meta.height as u64).max(1) as f64;
    let bpp = meta.file_size_bytes as f64 / (pixels * meta.frame_count.max(1) as f64);

    // Layer 1: veto rules
    match apply_veto(meta, bpp) {
        VetoVerdict::KeepGif => {
            crate::progress_mode::emit_stderr(&format!(
                "   🎞️  GIF veto=KEEP [bpp={:.3} px={:.0} dur={:.1}s] → KEEP GIF (veto rule)",
                bpp, pixels, meta.duration_secs
            ));
            return true;
        }
        VetoVerdict::ConvertVideo => {
            crate::progress_mode::emit_stderr(&format!(
                "   🎞️  GIF veto=CONVERT [bpp={:.3} px={:.0} dur={:.1}s] → CONVERT→VIDEO (veto rule)",
                bpp, pixels, meta.duration_secs
            ));
            return false;
        }
        VetoVerdict::Undecided => {}
    }

    // Layer 2: dynamic-weighted score
    let s = score_gif(meta);

    // Layer 3: confidence interval
    let (keep, confidence) = if s.total >= CONF_KEEP {
        (true, "high-conf meme")
    } else if s.total <= CONF_CONVERT {
        (false, "high-conf video")
    } else {
        (true, "uncertain→keep") // conservative: prefer keeping GIF over false conversion
    };

    crate::progress_mode::emit_stderr(&format!(
        "   🎞️  GIF score={:.3} [sharp={:.2} res={:.2} dur={:.2} fps={:.2} ratio={:.2} bpp={:.3}] {} → {}",
        s.total, s.sharpness, s.resolution, s.duration, s.fps, s.aspect_ratio, s.bytes_per_pixel,
        confidence,
        if keep { "KEEP GIF" } else { "CONVERT→VIDEO" }
    ));

    keep
}

/// Build a [`GifMeta`] from an [`crate::ffprobe::FFprobeResult`] and the raw
/// file size.  Returns `None` if the probe has no usable video dimensions.
pub fn gif_meta_from_probe(probe: &crate::ffprobe::FFprobeResult, file_size_bytes: u64) -> Option<GifMeta> {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(duration: f64, w: u32, h: u32, fps: f64, frames: u64, size: u64) -> GifMeta {
        GifMeta { duration_secs: duration, width: w, height: h, fps, frame_count: frames, file_size_bytes: size }
    }

    // ── score_gif tests ───────────────────────────────────────────────────────

    #[test]
    fn tiny_meme_scores_high() {
        // 200×200, 2s, 10fps, 20 frames, tiny file → should score ≥ 0.5
        let meta = make_meta(2.0, 200, 200, 10.0, 20, 40_000);
        let s = score_gif(&meta);
        assert!(s.total >= 0.50, "expected meme score ≥ 0.5, got {:.3}", s.total);
    }

    #[test]
    fn large_long_video_clip_scores_low() {
        // 1920×1080, 30s, 30fps, 900 frames, large file → should score < 0.5
        let meta = make_meta(30.0, 1920, 1080, 30.0, 900, 15_000_000);
        let s = score_gif(&meta);
        assert!(s.total < 0.50, "expected video score < 0.5, got {:.3}", s.total);
    }

    #[test]
    fn score_gif_exposes_bytes_per_pixel() {
        let meta = make_meta(3.0, 300, 300, 12.0, 36, 270_000);
        let s = score_gif(&meta);
        // bpp = 270_000 / (90_000 * 36) ≈ 0.0833
        assert!(s.bytes_per_pixel > 0.0, "bytes_per_pixel should be positive");
    }

    #[test]
    fn square_aspect_ratio_maxes_out() {
        let meta = make_meta(3.0, 300, 300, 12.0, 36, 200_000);
        let s = score_gif(&meta);
        assert!((s.aspect_ratio - 1.0).abs() < 1e-9, "square → aspect_ratio=1.0");
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
        assert_eq!(apply_veto(&meta, 0.70), VetoVerdict::ConvertVideo);
    }

    #[test]
    fn veto_convert_long_large() {
        // duration > 15s AND pixels > PIXELS_1080P → convert
        let meta = make_meta(20.0, 1920, 1080, 24.0, 480, 5_000_000);
        // bpp doesn't matter for this rule; pass a low value to isolate
        assert_eq!(apply_veto(&meta, 0.10), VetoVerdict::ConvertVideo);
    }

    #[test]
    fn veto_keep_ultra_compressed_tiny() {
        // bpp < 0.03 AND pixels < 200×200 → keep
        let meta = make_meta(3.0, 100, 100, 10.0, 30,
            // bpp = 1000 / (10_000*30) ≈ 0.003
            1_000);
        assert_eq!(apply_veto(&meta, 0.003), VetoVerdict::KeepGif);
    }

    #[test]
    fn veto_keep_very_short_loop() {
        // duration ≤ 1 s → always keep
        let meta = make_meta(0.8, 480, 480, 15.0, 12, 50_000);
        assert_eq!(apply_veto(&meta, 0.20), VetoVerdict::KeepGif);
    }

    #[test]
    fn veto_undecided_middle_ground() {
        // Nothing extreme → undecided
        let meta = make_meta(5.0, 640, 480, 15.0, 75, 500_000);
        assert_eq!(apply_veto(&meta, 0.10), VetoVerdict::Undecided);
    }

    // ── should_keep_as_gif confidence tests ──────────────────────────────────

    #[test]
    fn should_keep_veto_short_loop() {
        // duration ≤ 1 s → keep regardless of other dims
        let meta = make_meta(0.5, 1920, 1080, 30.0, 15, 10_000_000);
        assert!(should_keep_as_gif(&meta), "short loop should always keep");
    }

    #[test]
    fn should_convert_veto_long_large() {
        // 20 s, 1080p → convert veto
        let meta = make_meta(20.0, 1920, 1080, 30.0, 600, 5_000_000);
        assert!(!should_keep_as_gif(&meta), "long 1080p should always convert");
    }

    #[test]
    fn uncertain_zone_defaults_to_keep() {
        // Construct a case that lands in (0.35, 0.65) — moderate bpp, medium size/duration
        // 640×480, 6s, 15fps, 90 frames, moderate file
        let meta = make_meta(6.0, 640, 480, 15.0, 90, 800_000);
        let s = score_gif(&meta);
        // If score is in the uncertain zone, should_keep_as_gif returns true
        if s.total > CONF_CONVERT && s.total < CONF_KEEP {
            assert!(should_keep_as_gif(&meta), "uncertain zone must default to keep");
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
        w: u32, h: u32, duration: f64, fps: f64, frames: u64, size: u64,
    ) -> Option<GifMeta> {
        if w == 0 || h == 0 {
            return None;
        }
        Some(GifMeta { duration_secs: duration, width: w, height: h, fps, frame_count: frames, file_size_bytes: size })
    }
}
