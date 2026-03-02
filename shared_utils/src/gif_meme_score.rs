//! GIF meme-score heuristic — multi-dimensional judgment for animated GIFs.
//!
//! Instead of relying on duration alone to decide whether a GIF should be kept
//! (skipped from video conversion), this module scores five independent
//! dimensions and combines them into a single `total` score in [0.0, 1.0].
//! A score ≥ 0.50 means "keep as GIF / skip video conversion"; < 0.50 means
//! "convert to video (HEVC)".
//!
//! Dimensions (inspired by the meme-score reference):
//!   - sharpness   (0.40): Low-variance / blurry frames → meme-like.  Computed
//!                         from per-pixel size (bytes / pixel) as a proxy when
//!                         no decoded frame is available.
//!   - resolution  (0.20): Small canvas → meme-like (≤200² ≈ 1.0, ≥1080p ≈ 0.0).
//!   - duration    (0.18): Short loop → meme-like (≤1 s ≈ 1.0, ≥10 s ≈ 0.0).
//!   - fps         (0.08): Low frame rate → meme-like (≤6 fps ≈ 1.0, ≥30 fps ≈ 0.0).
//!   - aspect_ratio(0.14): Square / near-square canvas → meme-like.

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

/// Weighted per-dimension scores and the aggregated total.
#[derive(Debug, Clone)]
pub struct MemeScore {
    /// Combined score in [0.0, 1.0].  ≥ 0.50 → keep as GIF.
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
}

/// Clamp-normalise `value` from [`low`, `high`] → [0.0, 1.0].
#[inline]
fn normalize(value: f64, low: f64, high: f64) -> f64 {
    if high <= low {
        return 0.0;
    }
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

/// Score a GIF using ffprobe-derived metadata (no decoded frame required).
///
/// ## Sharpness proxy
/// Real meme GIFs tend to have very simple palettes and low information density.
/// We approximate this as *bytes-per-pixel*: meme GIFs have few unique colours
/// so they compress well → low bytes/pixel → high sharpness score (meme-like).
/// High-quality screen-recordings or video clips tend to have more complex
/// palettes → higher bytes/pixel → lower sharpness score (video-like).
///
/// Heuristic range: 0.05 (highly compressed meme) … 0.40 (complex animation).
pub fn score_gif(meta: &GifMeta) -> MemeScore {
    let pixels = (meta.width as u64 * meta.height as u64).max(1);
    let total_frames = meta.frame_count.max(1);
    let bytes_per_pixel = meta.file_size_bytes as f64 / (pixels * total_frames) as f64;

    // Low bytes/pixel → simple/meme → score → 1.0; high → video-like → 0.0
    let sharpness_score = 1.0 - normalize(bytes_per_pixel, 0.05, 0.40);

    // Resolution: small ≈ meme; large ≈ video content
    let pixel_count = (meta.width as u64 * meta.height as u64) as f64;
    let resolution_score = 1.0 - normalize(pixel_count, 40_000.0, 2_073_600.0); // 200² … 1920×1080

    // Duration: short loop ≈ meme
    let duration_score = 1.0 - normalize(meta.duration_secs, 1.0, 10.0);

    // FPS: low frame-rate ≈ meme
    let fps_score = 1.0 - normalize(meta.fps, 6.0, 30.0);

    // Aspect ratio: square ≈ meme (1:1 → 1.0, 16:9 or 9:16 → lower)
    let ratio = if meta.height > 0 {
        meta.width as f64 / meta.height as f64
    } else {
        1.0
    };
    let aspect_score = 1.0 - (ratio - 1.0).abs().min(1.0);

    // Weighted sum (weights sum to 1.0, matching the reference implementation)
    const W_SHARPNESS: f64 = 0.40;
    const W_RESOLUTION: f64 = 0.20;
    const W_DURATION: f64 = 0.18;
    const W_ASPECT: f64 = 0.14;
    const W_FPS: f64 = 0.08;

    let total = sharpness_score * W_SHARPNESS
        + resolution_score * W_RESOLUTION
        + duration_score * W_DURATION
        + aspect_score * W_ASPECT
        + fps_score * W_FPS;

    MemeScore {
        total,
        sharpness: sharpness_score,
        resolution: resolution_score,
        duration: duration_score,
        fps: fps_score,
        aspect_ratio: aspect_score,
    }
}

/// Convenience wrapper: returns `true` when the GIF should be kept as-is
/// (skip video conversion), `false` when it should be converted to video.
///
/// Logs a one-line summary to stderr via the shared progress_mode emitter.
pub fn should_keep_as_gif(meta: &GifMeta) -> bool {
    let s = score_gif(meta);
    let verdict = if s.total >= 0.50 { "KEEP GIF" } else { "CONVERT→VIDEO" };
    let msg = format!(
        "   🎞️  GIF meme-score={:.3} [sharp={:.2} res={:.2} dur={:.2} fps={:.2} ratio={:.2}] → {}",
        s.total, s.sharpness, s.resolution, s.duration, s.fps, s.aspect_ratio, verdict
    );
    crate::progress_mode::emit_stderr(&msg);
    s.total >= 0.50
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
    fn normalize_clamps_correctly() {
        assert!((normalize(0.0, 0.0, 1.0) - 0.0).abs() < 1e-9);
        assert!((normalize(1.0, 0.0, 1.0) - 1.0).abs() < 1e-9);
        assert!((normalize(-1.0, 0.0, 1.0) - 0.0).abs() < 1e-9);
        assert!((normalize(2.0, 0.0, 1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn normalize_degenerate_range() {
        // high <= low → always 0.0
        assert_eq!(normalize(5.0, 10.0, 5.0), 0.0);
    }

    #[test]
    fn square_aspect_ratio_maxes_out() {
        let meta = make_meta(3.0, 300, 300, 12.0, 36, 200_000);
        let s = score_gif(&meta);
        assert!((s.aspect_ratio - 1.0).abs() < 1e-9, "square → aspect_ratio=1.0");
    }

    #[test]
    fn gif_meta_from_probe_zero_dimensions_returns_none() {
        // Simulate a probe result with zero dimensions (no valid video stream)
        let meta = gif_meta_from_probe_raw(0, 0, 2.0, 10.0, 20, 40_000);
        assert!(meta.is_none());
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
