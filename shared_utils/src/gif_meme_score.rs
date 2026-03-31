//! GIF meme-score heuristic — multi-dimensional judgment for animated GIFs.
//!
//! Uses an eight-layer strategy to decide whether a GIF should be kept as-is
//! (skipped from video conversion) or converted to HEVC video:
//!
//! //! 1. **Veto rules** (hard constraints): extreme cases bypass scoring entirely
//!    - NEW: known meme-CDN app-extension blocks (GIPHY / Tenor) → hard `KeepGif`
//!    - NEW: modern high-res stickers (short, loopable) → hard `KeepGif`
//! 2. **Dynamic weighting**: dimension scores adjust based on inter-relationships
//! 3. **Confidence intervals**: uncertain cases (0.40-0.60) default to keeping GIF
//! 4. **Compression ratio**: bytes-per-pixel as a zero-cost strong feature
//! 5. **Filename analysis**: distinguishes human-semantic vs machine-generated
//! 6. **Loop frequency**: high loop rate (short duration) → meme-like
//! 7. **Palette entropy**: small power-of-2 palette size → synthetic / meme-like
//! 8. **Content Intensity**: frame payload variation as visual complexity proxy
//! 9. **Weighted scoring**: all dimensions combined when no veto applies
//!
//! Dimensions (base weights, adjusted dynamically):
//!   - sharpness       (0.18): Low bytes/pixel → simple palette
//!   - resolution      (0.12): Canvas size impact (attenuated)
//!   - duration        (0.28): Short loop → meme-like (≤1.5s ≈ 1.0, ≥15s ≈ 0.0)
//!   - loop_frequency  (0.15): High loop rate → meme-like
//!   - content_intensity (0.10): Frame variance proxy
//!   - transparency    (0.08): Alpha usage
//!   - aspect_ratio    (0.06): Square canvas → meme-like
//!   - palette         (0.03): Small palette → synthetic
//!   - filename        (0.00): DEPRECATED — filenames too noisy for HD
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
    /// Optional: parent directory names from the source path.
    pub parent_directories: Option<Vec<String>>,
    /// True when the source carries an embedded ICC profile.
    pub has_embedded_icc: bool,
    /// True when probe metadata indicates non-sRGB / wide-gamut / HDR style colour handling.
    pub has_complex_color_profile: bool,
    /// Optional: Loop count from NETSCAPE2.0 extension (0 = infinite).
    pub loop_count: Option<u16>,
    /// True when the source (video) contains an audio stream.
    /// Silent videos are a strong signal for GIF-origin stickers.
    pub has_audio: bool,
    /// 🎞️ Frame types (I, P, B, etc.) from the video sequence.
    pub frame_types: Vec<char>,
    /// 🎞️ PTS deltas (frame intervals) from the video sequence.
    pub pts_deltas: Vec<f64>,
    /// 🎞️ Motion vector magnitudes (if available).
    pub mv_magnitudes: Vec<f64>,
    /// 🎞️ Optional: Palette depth score [0, 1].
    pub palette_depth: Option<f64>,
    /// 🎞️ Optional: Motion Gini coefficient [0, 1].
    pub motion_gini: Option<f64>,
    /// 🎞️ Optional: Block variance skewness [0, 1].
    pub block_skew: Option<f64>,
    /// 🎞️ Optional: Temporal flatness [0, 1].
    pub temporal_flatness: Option<f64>,
    /// 🎞️ Captured packet sizes for bitrate inequality analysis.
    pub pkt_sizes: Vec<u64>,
}

impl GifMeta {
    /// Factory: Build assessment metadata from a video detection result.
    /// This enables 'Meme Scoring' for MP4/MOV/MKV inputs.
    pub fn from_video(detection: &crate::video_detection::VideoDetectionResult) -> Self {
        let file_name = std::path::Path::new(&detection.file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());

        let loop_count = detection.loop_count; // Respect actual metadata if present (rare for MP4)
        let has_complex_color_profile = detection.is_dolby_vision
            || detection.is_hdr10_plus
            || detection.mastering_display.is_some()
            || detection.max_cll.is_some()
            || matches!(detection.color_space, crate::video_detection::ColorSpace::BT2020)
            || detection
                .color_transfer
                .as_deref()
                .is_some_and(|s| !matches!(s, "bt709" | "iec61966-2-1" | "srgb" | "unknown"))
            || detection
                .color_primaries
                .as_deref()
                .is_some_and(|s| !matches!(s, "bt709" | "smpte170m" | "unknown"));

        // Calculate coefficients of variation for variation fields
        let frame_payload_variation = if detection.pkt_sizes.is_empty() {
            None
        } else {
            let n = detection.pkt_sizes.len() as f64;
            let mean = detection.pkt_sizes.iter().map(|&s| s as f64).sum::<f64>() / n;
            if mean > 0.0 {
                let variance = detection
                    .pkt_sizes
                    .iter()
                    .map(|&s| (s as f64 - mean).powi(2))
                    .sum::<f64>()
                    / n;
                Some(variance.sqrt() / mean)
            } else {
                Some(0.0)
            }
        };

        let frame_delay_variation = if detection.pts_deltas.is_empty() {
            None
        } else {
            let n = detection.pts_deltas.len() as f64;
            let mean = detection.pts_deltas.iter().sum::<f64>() / n;
            if mean > 0.0 {
                let variance = detection
                    .pts_deltas
                    .iter()
                    .map(|&d| (d - mean).powi(2))
                    .sum::<f64>()
                    / n;
                Some(variance.sqrt() / mean)
            } else {
                Some(0.0)
            }
        };

        Self {
            duration_secs: detection.duration_secs,
            has_audio: detection.has_audio,
            width: detection.width,
            height: detection.height,
            fps: detection.fps,
            frame_count: detection.frame_count,
            file_size_bytes: detection.file_size,
            file_name,
            palette_size: None, // Video doesn't have a fixed GIF palette
            app_extensions: Some(Vec::new()),
            has_transparency: false, // Standard MP4 doesn't support alpha easily
            frame_payload_variation,
            frame_delay_variation,
            source_extension: std::path::Path::new(&detection.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase()),
            parent_directories: None, // Caller can populate if needed
            has_embedded_icc: false,
            has_complex_color_profile,
            loop_count,
            frame_types: detection.frame_types.clone(),
            pts_deltas: detection.pts_deltas.clone(),
            mv_magnitudes: detection.mv_magnitudes.clone(),
            palette_depth: None,
            motion_gini: None, // Will be computed if needed
            block_skew: None,
            temporal_flatness: None,
            pkt_sizes: detection.pkt_sizes.clone(),
        }
    }

    /// Returns true if the media satisfies the v5.3 rhythmic sticker criteria.
    pub fn is_rhythmic_sticker(&self) -> bool {
        let verdict = apply_veto(self, 0.0, 0.0); // BPP unused for rhythmic check
        verdict == VetoVerdict::KeepGif
    }
}

/// Composite GIF/Meme score using a multi-stage cascaded detection pipeline (v6.1).
/// Returns a score in [0.0, 1.0].
pub fn composite_gif_score(meta: &GifMeta) -> f64 {
    let pixel_count = (f64::from(meta.width) * f64::from(meta.height)).max(1.0);
    let total_frames = meta.frame_count.max(1);
    let temporal_bpp = meta.file_size_bytes as f64 / (pixel_count * total_frames as f64);
    let spatial_bpp = meta.file_size_bytes as f64 / pixel_count;

    let (bpp_low, bpp_high) = temporal_bpp_thresholds(meta.fps);
    let complexity_norm = normalize(temporal_bpp, bpp_low, bpp_high);
    let weights = compute_weights(complexity_norm);

    // ── Phase 1: Metadata signals ──────────────────────────────
    let no_audio = if meta.has_audio { 0.0 } else { 1.0 };
    let fps_anomaly = fps_anomaly_score(meta.fps);
    let iframe_density = iframe_density_score(&meta.frame_types);
    let compression = compression_efficiency_score(
        meta.file_size_bytes,
        meta.width,
        meta.height,
        meta.fps,
        meta.duration_secs,
    );
    let bit_gini = bitrate_gini_score(&meta.pkt_sizes);

    // ── Phase 2: Timing & Cadence ──────────────────────────
    let interval_cv = interval_consistency_score(&meta.pts_deltas);
    let loop_freq = score_loop_affinity(meta);
    let cadence = score_sparse_cadence(meta.duration_secs, meta.frame_count);
    let loop_sim = meta
        .palette_size
        .map_or(0.5, |s| if s <= 64 { 0.8 } else { 0.2 });

    // ── Phase 3: Deep Signals ──────────────────────
    let p_depth = meta.palette_depth.unwrap_or(0.5);
    let m_gini = meta.motion_gini.unwrap_or(0.5);
    let b_skew = meta.block_skew.unwrap_or(0.5);
    let t_flat = meta.temporal_flatness.unwrap_or(0.5);
    let palette = score_palette(meta.palette_size).unwrap_or(0.5);
    let pixel_art = score_pixel_art(meta.palette_size, spatial_bpp, meta.frame_payload_variation);

    // ── Phase 4: Core Physical Signals ───────────────
    let dynamic_threshold = dynamic_duration_threshold(meta.width, meta.height, meta.fps).max(0.35);
    let duration_ratio = meta.duration_secs / dynamic_threshold;
    let duration = 1.0 - normalize(duration_ratio, 0.35, 1.9);
    let resolution = 1.0 - normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P);
    let transparency = score_transparency(meta.has_transparency);
    let premium = score_premium_all(meta);

    // ── Phase 5: Linguistic & Contextual ─────────────
    let fa = analyze_filename(meta.file_name.as_deref());
    let spatial_norm = normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P);
    let p_complexity = phys_complexity(meta, spatial_norm);
    let attenuation = match fa.kind {
        FilenameKind::HumanSemantic => 0.85,
        FilenameKind::MachineGenerated => 0.95,
        FilenameKind::Ambiguous => 1.00,
    };
    let filename = fa.raw * (1.0 - attenuation * p_complexity);
    let directory = score_directory_context(meta.parent_directories.as_deref());

    let total = weights.get("no_audio").unwrap_or(&0.0) * no_audio
        + weights.get("fps_anomaly").unwrap_or(&0.0) * fps_anomaly
        + weights.get("iframe_dens").unwrap_or(&0.0) * iframe_density
        + weights.get("compression").unwrap_or(&0.0) * compression
        + weights.get("bit_gini").unwrap_or(&0.0) * bit_gini
        + weights.get("cadence").unwrap_or(&0.0) * cadence
        + weights.get("interval_cv").unwrap_or(&0.0) * interval_cv
        + weights.get("loop_sim").unwrap_or(&0.0) * loop_sim
        + weights.get("p_depth").unwrap_or(&0.0) * p_depth
        + weights.get("m_gini").unwrap_or(&0.0) * m_gini
        + weights.get("b_skew").unwrap_or(&0.0) * b_skew
        + weights.get("temporal_flat").unwrap_or(&0.0) * t_flat
        + weights.get("pixel_art").unwrap_or(&0.0) * pixel_art
        + weights.get("duration").unwrap_or(&0.0) * duration
        + weights.get("loop_freq").unwrap_or(&0.0) * loop_freq
        + weights.get("resolution").unwrap_or(&0.0) * resolution
        + weights.get("transparency").unwrap_or(&0.0) * transparency
        + weights.get("premium").unwrap_or(&0.0) * premium
        + weights.get("palette").unwrap_or(&0.0) * palette
        + weights.get("filename").unwrap_or(&0.0) * filename
        + weights.get("directory").unwrap_or(&0.0) * directory
        + weights.get("knn").unwrap_or(&0.0) * 0.5;

    total.clamp(0.0, 1.0)
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
    /// Pixel art classification score.
    pub pixel_art_score: f64,
}

// ── Internal filename analysis ────────────────────────────────────────────────

/// Origin classification of a filename, used to set attenuation strength.
#[derive(Debug, Clone, PartialEq)]
pub enum FilenameKind {
    /// Human-assigned, semantically meaningful (single short word / CJK phrase).
    /// Strong meme signal when physical features agree.
    HumanSemantic,
    /// Machine-generated: hash, social-app prefix, or pure timestamp.
    /// Indicates social-media *origin*, NOT content class.
    MachineGenerated,
    /// Multi-word, generic, or unclassifiable.
    Ambiguous,
}

pub struct FilenameAnalysis {
    /// Raw score before complexity hedging, in [0.0, 1.0].
    pub raw: f64,
    pub kind: FilenameKind,
}

// ── Veto thresholds ───────────────────────────────────────────────────────────
/// reference area for 720p scaling (pixels)
const REFERENCE_AREA: f64 = 720.0 * 720.0;
/// pixel count above this → 1080p+
const PIXELS_1080P: f64 = (1920 * 1080) as f64;
/// pixel count below this → very small canvas (≤200×200)
const PIXELS_SMALL: f64 = (200 * 200) as f64;
/// Absolute byte thresholds to hard-keep or hard-convert GIFs regardless of
/// dynamic heuristics. These provide a safety net for tiny personal stickers
/// and for extremely large GIFs that should never be stored as GIF due to bloat.
const ABSOLUTE_KEEP_UNDER_BYTES: u64 = 102_400; // 100 KiB
const ABSOLUTE_CONVERT_OVER_BYTES: u64 = 50_000_000; // 50 MB

/// Returns the dynamic duration threshold T_max(N, fps).
fn dynamic_duration_threshold(width: u32, height: u32, fps: f64) -> f64 {
    const T_REF: f64 = 4.25;
    const N_REF: f64 = 518_400.0; // 720×720
    const FPS_REF: f64 = 12.0;
    const GAMMA: f64 = 0.30;

    let n = (f64::from(width) * f64::from(height)).max(1.0);
    let fps_clamped = fps.clamp(3.0, 60.0);
    let fps_factor = (FPS_REF / fps_clamped).powf(GAMMA);

    // clamp fps_factor to [0.65, 1.60] — avoid over-penalizing extreme frame rates
    (T_REF * (N_REF / n).sqrt() * fps_factor.clamp(0.65, 1.60)).max(0.3)
}

/// Returns the file size floor: GIFs smaller than this are likely kept.
fn keep_floor(width: u32, height: u32) -> u64 {
    let n = (f64::from(width) * f64::from(height)).max(1.0);
    (n * 0.5).clamp(32_768.0, 131_072.0) as u64
}

/// Returns the file size ceiling: GIFs larger than this are likely converted.
fn convert_ceiling(width: u32, height: u32) -> u64 {
    let n = u64::from(width) * u64::from(height);
    n * 8 // 8 bytes/pixel: theoretical lower bound for natural content GIFs
}

/// Returns the FPS-aware temporal BPP thresholds.
fn temporal_bpp_thresholds(fps: f64) -> (f64, f64) {
    const BPP_LOW_REF: f64 = 0.03;
    const BPP_HIGH_REF: f64 = 0.60;
    const FPS_REF: f64 = 12.0;
    const EXPONENT: f64 = 0.40;

    let factor = (FPS_REF / fps.clamp(3.0, 60.0)).powf(EXPONENT);
    let factor = factor.clamp(0.5, 2.0);
    (BPP_LOW_REF * factor, BPP_HIGH_REF * factor)
}

// ── Mathematical Signal Functions (v6.0) ──────────────────────────────────────

/// Sigmoid function for smooth threshold mapping.
fn sigmoid(x: f64, center: f64, steepness: f64) -> f64 {
    1.0 / (1.0 + (-steepness * (x - center)).exp())
}

/// Calculates I-frame density score.
/// High density (> 0.1) is a strong signal for non-continuous GIF sources.
fn iframe_density_score(frame_types: &[char]) -> f64 {
    if frame_types.is_empty() {
        return 0.5;
    }
    let total = frame_types.len() as f64;
    let i_count = frame_types.iter().filter(|&&t| t == 'I').count() as f64;
    let density = i_count / total;

    // Standard video I-frame density is usually 0.01~0.04.
    // GIF-to-MP4 often has very high density due to scene change detection triggers.
    sigmoid(density, 0.08, 15.0)
}

/// Information Efficiency Score: Theoretical Max Bits / Actual Bits.
/// GIF content is extremely compressible in H.264/HEVC.
fn compression_efficiency_score(
    file_bytes: u64,
    width: u32,
    height: u32,
    fps: f64,
    duration: f64,
) -> f64 {
    if duration <= 0.1 || width == 0 || height == 0 || fps <= 0.1 {
        return 0.5;
    }

    // Theoretical max (uncompressed 24-bit raw)
    let theoretical_max = f64::from(width) * f64::from(height) * fps * duration * 24.0;
    let actual_bits = file_bytes as f64 * 8.0;

    if theoretical_max <= 0.0 {
        return 0.5;
    }
    let ratio = actual_bits / theoretical_max;

    // GIF-like content usually has a ratio < 0.001.
    // Score ~1.0 for extremely high compression, ~0.0 for low compression.
    1.0 - (ratio * 100.0).min(1.0)
}

/// Calculates the Gini coefficient of a distribution.
/// Measures the inequality (concentration) of values.
fn gini_coefficient(values: &[f64]) -> f64 {
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

/// Bitrate inequality score using Gini coefficient of packet sizes.
/// GIF-to-video conversions usually have extreme bitrate spikes (huge I-frames, tiny or zero-size P-frames).
fn bitrate_gini_score(pkt_sizes: &[u64]) -> f64 {
    if pkt_sizes.len() < 5 {
        return 0.5;
    }
    let values: Vec<f64> = pkt_sizes.iter().map(|&s| s as f64).collect();
    let gini = gini_coefficient(&values);

    // Typical video Gini is 0.3-0.5.
    // Typical Sticker-MP4 Gini is 0.7-0.95.
    sigmoid(gini, 0.65, 12.0)
}

/// Motion complexity score using Gini coefficient of motion vector magnitudes.
/// GIF content has highly concentrated/sparse motion.
fn motion_gini_score(mv_magnitudes: &[f64]) -> f64 {
    if mv_magnitudes.is_empty() {
        return 0.5;
    }
    let gini = gini_coefficient(mv_magnitudes);
    // Low Gini (uniform motion) -> Video
    // High Gini (sparse/irregular motion) -> GIF
    sigmoid(gini, 0.5, 8.0)
}

/// Temporal flatness: Coefficient of variation of frame-to-frame differences.
fn temporal_flatness_score(ydif_values: &[f64]) -> f64 {
    if ydif_values.is_empty() {
        return 0.5;
    }
    let n = ydif_values.len() as f64;
    let mean = ydif_values.iter().sum::<f64>() / n;
    if mean < 1e-6 {
        return 1.0;
    } // Perfect flatness

    let variance = ydif_values.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n;
    let std = variance.sqrt();

    // Low variation relative to mean -> Consistent (GIF-like)
    1.0 / (1.0 + std / (mean + 1e-6))
}

/// Palette Depth Score: 5-bit color quantization depth.
/// Real photos have complex gradients; GIFs have flat palette-limited colors.
fn palette_depth_score(quantized_unique_colors: usize) -> f64 {
    if quantized_unique_colors == 0 {
        return 0.5;
    }

    let count = quantized_unique_colors as f64;
    let max_possible = 32_f64.powi(3); // 5-bit per channel

    // Log-scale mapping for perceptual linearity.
    let score = 1.0 - (count.ln() / max_possible.ln()).min(1.0);
    score.clamp(0.0, 1.0)
}

/// Block Variance Skewness: Measures content complexity distribution.
/// High skewness (many flat blocks, few busy blocks) is typical for memes.
fn block_variance_skewness_score(block_variances: &[f64]) -> f64 {
    if block_variances.is_empty() {
        return 0.5;
    }
    let n = block_variances.len() as f64;
    let mean = block_variances.iter().sum::<f64>() / n;

    let variance = block_variances
        .iter()
        .map(|&v| (v - mean).powi(2))
        .sum::<f64>()
        / n;
    let std = variance.sqrt();

    if std < 1e-7 {
        return 1.0;
    }

    // Pearson's moment coefficient of skewness.
    let m3 = block_variances
        .iter()
        .map(|&v| (v - mean).powi(3))
        .sum::<f64>()
        / n;

    let skewness = m3 / std.powi(3);

    // Scale skewness into [0, 1]. High positive skewness -> GIF
    sigmoid(skewness, 2.0, 1.0)
}

/// Physical Complexity Proxy: content entropy proxy.
fn phys_complexity(meta: &GifMeta, spatial_bpp_norm: f64) -> f64 {
    let payload_var = meta.frame_payload_variation.unwrap_or(0.5);
    let delay_var = meta.frame_delay_variation.unwrap_or(0.5);

    // temporal_entropy_norm: weighted fusion of two variation types
    let temporal_entropy_norm = ((payload_var + 0.3 * delay_var) / 1.3).clamp(0.0, 1.0);

    0.58 * spatial_bpp_norm + 0.42 * temporal_entropy_norm
}

// ── Softmax Dynamic Weights ──────────────────────────────────────────────────

struct WeightConfig {
    name: &'static str,
    base: f64,
    affinity: f64, // > 0: high complexity more important; < 0: low complexity more important
}

fn compute_weights(complexity: f64) -> std::collections::HashMap<&'static str, f64> {
    // Log-linear softmax weights: Core signals carry ~90% weight,
    // Secondary Core signals carry ~10% weight, and Tie-breakers act as 0.001-level signals.
    let configs = [
        // --- Core Signals (Dominant) ---
        WeightConfig {
            name: "compression",
            base: 0.40,
            affinity: 1.2,
        },
        WeightConfig {
            name: "duration",
            base: 0.30,
            affinity: 0.6,
        },
        WeightConfig {
            name: "loop_freq",
            base: 0.30,
            affinity: -0.8,
        },
        // --- Secondary Core Signals ---
        WeightConfig {
            name: "fps_anomaly",
            base: 0.05,
            affinity: -0.8,
        },
        WeightConfig {
            name: "p_depth",
            base: 0.04,
            affinity: 0.2,
        },
        WeightConfig {
            name: "no_audio",
            base: 0.04,
            affinity: -0.5,
        },
        // --- Auxiliary Signals (Tie-breakers) ---
        WeightConfig {
            name: "resolution",
            base: 0.005,
            affinity: 0.8,
        },
        WeightConfig {
            name: "knn",
            base: 0.005,
            affinity: 0.0,
        },
        WeightConfig {
            name: "transparency",
            base: 0.005,
            affinity: -1.0,
        },
        WeightConfig {
            name: "premium",
            base: 0.005,
            affinity: -0.4,
        },
        WeightConfig {
            name: "palette",
            base: 0.005,
            affinity: -0.6,
        },
        WeightConfig {
            name: "cadence",
            base: 0.005,
            affinity: -0.2,
        },
        WeightConfig {
            name: "iframe_dens",
            base: 0.003,
            affinity: 0.4,
        },
        WeightConfig {
            name: "bit_gini",
            base: 0.001,
            affinity: 0.5,
        },
        WeightConfig {
            name: "interval_cv",
            base: 0.002,
            affinity: -0.2,
        },
        WeightConfig {
            name: "loop_sim",
            base: 0.002,
            affinity: -0.4,
        },
        WeightConfig {
            name: "m_gini",
            base: 0.001,
            affinity: 0.6,
        },
        WeightConfig {
            name: "b_skew",
            base: 0.001,
            affinity: 0.4,
        },
        WeightConfig {
            name: "filename",
            base: 0.005,
            affinity: -0.2,
        },
        WeightConfig {
            name: "directory",
            base: 0.005,
            affinity: -0.4,
        },
        WeightConfig {
            name: "pixel_art",
            base: 0.005,
            affinity: -0.5,
        },
        WeightConfig {
            name: "temporal_flat",
            base: 0.002,
            affinity: 0.3,
        },
    ];

    // log-linear softmax: w_i(C) = base_i * exp(affinity_i * (C - 0.5)) / Z
    let exponents: Vec<f64> = configs
        .iter()
        .map(|cfg| cfg.base * (cfg.affinity * (complexity - 0.5)).exp())
        .collect();

    let z: f64 = exponents.iter().sum();
    let mut weights = std::collections::HashMap::new();

    for (i, cfg) in configs.iter().enumerate() {
        weights.insert(cfg.name, exponents[i] / z.max(1e-9));
    }

    weights
}

// ── Sticker Fingerprint (v5.5) ───────────────────────────────────────────────
const STANDARD_FPS: &[f64] = &[
    23.976, 24.0, 25.0, 29.97, 30.0, 47.952, 48.0, 50.0, 59.94, 60.0, 120.0,
];

/// Calculates how much an FPS value deviates from standard cinematic/video norms.
/// Returns score in [0.0, 1.0], where 1.0 = highly anomalous (likely GIF-origin).
fn fps_anomaly_score(fps: f64) -> f64 {
    if fps <= 0.01 {
        return 0.0;
    }

    let min_delta = STANDARD_FPS
        .iter()
        .map(|&s| (fps - s).abs() / s)
        .fold(f64::MAX, f64::min);

    // Threshold: > 2.5% deviation is considered a significant anomaly for stickers.
    (min_delta / 0.025).clamp(0.0, 1.0)
}

/// Infers whether a video asset was intended to be a looping sticker.
/// Used for containers (MP4/MOV) that lack explicit loop metadata.
fn infer_looping_intent(meta: &GifMeta) -> bool {
    // If it already has an infinite loop flag (from GIF), trust it.
    if meta.loop_count == Some(0) {
        return true;
    }

    // Weight-based evidence chain:
    // A: No Audio (Strong: 0.5)
    // B: FPS Anomaly (Medium: 0.3)
    // C: Short Duration (Support: 0.2)

    let audio_signal = if meta.has_audio { 0.0 } else { 0.5 };
    let fps_signal = fps_anomaly_score(meta.fps) * 0.3;
    let duration_signal = (1.0 - normalize(meta.duration_secs, 2.0, 15.0)) * 0.2;

    let total_intent = audio_signal + fps_signal + duration_signal;

    // threshold > 0.55 indicates high likelihood of sticker-intent
    total_intent > 0.55
}

/// Continuous estimate of "loop/sticker affinity" for both native GIFs and
/// silent short-form videos that likely originated as GIF/sticker assets.
///
/// Higher scores mean:
/// - shorter average frame interval
/// - stronger looping intent
/// - shorter duration relative to the canvas-scaled duration budget
/// - no audio
#[must_use]
pub fn score_loop_affinity(meta: &GifMeta) -> f64 {
    if meta.duration_secs <= 0.01 || meta.frame_count <= 1 {
        return 0.0;
    }

    let dynamic_threshold = dynamic_duration_threshold(meta.width, meta.height, meta.fps).max(0.35);
    let frame_density = meta.frame_count as f64 / meta.duration_secs.max(0.05);
    let avg_frame_gap = meta.duration_secs / meta.frame_count.max(1) as f64;
    let loop_rate = 60.0 / meta.duration_secs.max(0.05);
    let duration_ratio = meta.duration_secs / dynamic_threshold;

    let loop_intent = if meta.loop_count == Some(0) {
        1.0
    } else if infer_looping_intent(meta) {
        0.85
    } else {
        0.0
    };

    let audio_score = if meta.has_audio { 0.0 } else { 1.0 };
    let shortness_score = 1.0 - normalize(duration_ratio, 0.8, 2.8);
    let density_score = normalize(frame_density, 4.0, 30.0);
    let cadence_score = 1.0 - normalize(avg_frame_gap, 0.05, 0.35);
    let loop_rate_score = normalize(loop_rate, 3.0, 45.0);
    let fps_anomaly = fps_anomaly_score(meta.fps);

    (loop_intent * 0.26
        + audio_score * 0.22
        + shortness_score * 0.18
        + density_score * 0.14
        + cadence_score * 0.12
        + loop_rate_score * 0.05
        + fps_anomaly * 0.03)
        .clamp(0.0, 1.0)
}

fn interval_consistency_score(pts_deltas: &[f64]) -> f64 {
    if pts_deltas.is_empty() {
        return 0.5;
    }
    let n = pts_deltas.len() as f64;
    let mean = pts_deltas.iter().sum::<f64>() / n;
    if mean < 1e-6 {
        return 0.5;
    }

    let variance = pts_deltas.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n;
    let std = variance.sqrt();
    let cv = std / mean;
    sigmoid(cv, 0.15, 10.0)
}

/// Returns true when a non-GIF video behaves like a looping sticker / animated
/// GIF asset and should be evaluated by the GIF scorer.
#[must_use]
pub fn is_probably_gif_like_video(meta: &GifMeta) -> bool {
    if meta.source_extension.as_deref() == Some("gif") {
        return true;
    }

    // Core filter for non-GIF containers (MP4/MOV/etc.)
    // Must be silent and exhibit at least minimal meme-like features.
    !meta.has_audio && composite_gif_score(meta) >= 0.35
}

// ── Confidence thresholds ─────────────────────────────────────────────────────

// ── Known meme-platform app-extension prefixes ────────────────────────────────
/// If any app-extension vendor string *starts with* one of these, the GIF
/// originates from a meme CDN and is vetoed as `KeepGif` regardless of resolution.
pub const MEME_PLATFORM_PREFIXES: &[&str] = &[
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
pub fn analyze_filename(name: Option<&str>) -> FilenameAnalysis {
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
pub fn score_palette(palette_size: Option<u32>) -> Option<f64> {
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
pub fn score_loop_frequency(duration_secs: f64, frame_count: u64) -> f64 {
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

pub fn score_sparse_cadence(duration_secs: f64, frame_count: u64) -> f64 {
    if duration_secs <= 0.01 || frame_count <= 1 {
        return 0.5;
    }

    let avg_frame_gap = duration_secs / frame_count as f64;
    let frame_density = frame_count as f64 / duration_secs.max(0.01);

    // Hard Threshold: If it's short and dense, it's likely a high-quality loop (Live2D)
    if duration_secs <= 1.5 && frame_density >= 12.0 {
        return 0.98;
    }
    if duration_secs <= 2.2 && frame_density >= 10.0 {
        return 0.90;
    }

    // NEW (v4.1) Logic Inversion: PPT-like sparse content is a strong MEME indicator
    if duration_secs >= 1.5 && avg_frame_gap >= 0.25 {
        return 0.92;
    }
    if duration_secs >= 1.2 && frame_density <= 4.0 {
        return 0.85;
    }

    if duration_secs >= 4.0 && frame_count <= 12 && avg_frame_gap >= 0.5 {
        return 0.95; // Extreme slideshow / reaction board
    }

    0.5
}

pub fn score_directory_context(parent_directories: Option<&[String]>) -> f64 {
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

pub fn score_transparency(has_transparency: bool) -> f64 {
    if has_transparency {
        1.0
    } else {
        0.02
    }
}

pub fn score_frame_variation(frame_payload_variation: Option<f64>) -> f64 {
    let Some(variation) = frame_payload_variation else {
        return 0.5;
    };
    normalize(variation, 0.08, 0.85)
}

pub fn score_timing_value(frame_delay_variation: Option<f64>, frame_count: u64) -> f64 {
    let Some(variation) = frame_delay_variation else {
        return if frame_count <= 1 { 0.0 } else { 0.35 };
    };
    normalize(variation, 0.05, 1.20)
}

/// Detects "Pixel Art" characteristics based on color count, spatial entropy, and consistency.
/// returns score in [0.0, 1.0] where 1.0 = likely pixel art (Keep GIF).
pub fn score_pixel_art(
    palette_size: Option<u32>,
    spatial_bpp: f64,
    frame_payload_variation: Option<f64>,
) -> f64 {
    // 1. Low color count is a primary indicator
    let color_score = match palette_size {
        Some(s) if s <= 32 => 1.0,
        Some(s) if s <= 64 => 0.9,
        Some(s) if s <= 128 => 0.6,
        Some(s) if s < 256 => 0.3,
        _ => 0.0,
    };

    // 2. Low spatial BPP relative to resolution indicates flat areas/efficient LZW compression
    // Pixel art is often extremely efficient, resulting in very low BPP.
    let entropy_score = 1.0 - normalize(spatial_bpp, 0.2, 3.5);

    // 3. Consistency (variation < 0.1) suggests a clean, synthetic source
    let var_score = match frame_payload_variation {
        Some(v) if v < 0.10 => 1.0,
        Some(v) if v < 0.20 => 0.7,
        _ => 0.1,
    };

    (color_score * 0.5 + entropy_score * 0.3 + var_score * 0.2).clamp(0.0, 1.0)
}

/// Detects "Premium/Cinematic" loop characteristics.
/// Returns score in [0.0, 1.0] where 1.0 = High-quality rhythmic loop (Protect from conversion).
pub fn score_premium_all(meta: &GifMeta) -> f64 {
    // 1. Visual Richness
    let has_max_colors = meta.palette_size == Some(256);
    let visual_score = if has_max_colors || meta.has_complex_color_profile || meta.has_embedded_icc
    {
        1.0
    } else {
        0.4
    };

    // 2. Rhythmic Consistency (Low variation is a sign of intentional loopcraft)
    let rhythm_score = match meta.frame_delay_variation {
        Some(v) if v < 0.05 => 1.0,
        Some(v) => 1.0 - normalize(v, 0.05, 0.40),
        None => 0.5,
    };

    // 3. Fluidity (High FPS is a hallmark of premium stickers/loops)
    let fluidity_score = normalize(meta.fps, 12.0, 30.0);

    // 4. Compact Durational Vibe
    let duration_vibe = 1.0 - normalize(meta.duration_secs, 2.0, 8.0);

    (visual_score * 0.4 + rhythm_score * 0.3 + fluidity_score * 0.2 + duration_vibe * 0.1)
        .clamp(0.0, 1.0)
}

// ── Veto rules ────────────────────────────────────────────────────────────────

/// Apply veto rules based on extreme metadata values.
/// Returns `KeepGif` / `ConvertVideo` for clear-cut cases; `Undecided` otherwise.
fn apply_veto(meta: &GifMeta, temporal_bpp: f64, spatial_bpp: f64) -> VetoVerdict {
    let pixel_count = (u64::from(meta.width) * u64::from(meta.height)) as f64;
    let dynamic_threshold = dynamic_duration_threshold(meta.width, meta.height, meta.fps).max(0.35);
    let duration_ratio = meta.duration_secs / dynamic_threshold;
    let loop_affinity = score_loop_affinity(meta);
    
    // Allow explicit operator overrides via environment variables. These are
    // intentionally conservative and opt-in only (no default behavior change).
    if let Some(ov) = check_meta_override(meta) {
        crate::progress_mode::emit_stderr(&format!(
            "🔒  GIF [{}] → {} (env override)",
            meta.file_name.as_deref().unwrap_or("?"),
            match ov { VetoVerdict::KeepGif => "KEEP GIF", VetoVerdict::ConvertVideo => "CONVERT→VIDEO", VetoVerdict::Undecided => "UNDECIDED" }
        ));
        return ov;
    }
    

    // CONDITION 2 & 4: HDR/Wide-gamut or Embedded ICC Profile (cannot be preserved in GIF)
    if meta.has_embedded_icc || meta.has_complex_color_profile {
        return VetoVerdict::ConvertVideo;
    }

    // Precompute rhythmic/infinite-loop intent & dynamic rhythmic threshold
    // (used by the size-ceiling logic and absolute guards).
    let is_infinite_loop = infer_looping_intent(meta);
    let avg_frame_delay = if meta.fps > 0.1 { 1.0 / meta.fps } else { 0.12 };
    let rhythmic_factor = (0.12 / avg_frame_delay).clamp(1.0, 4.0);
    let dynamic_rhythmic_threshold = 3.5 * rhythmic_factor;

    // ABSOLUTE GUARDS: small GIFs are always kept; extremely large GIFs
    // are converted unless they are clearly intentional infinite-loop/premium.
    if meta.file_size_bytes <= ABSOLUTE_KEEP_UNDER_BYTES {
        crate::progress_mode::emit_stderr(&format!(
            "🎯  GIF [{}] → KEEP GIF (absolute size <= {} bytes)",
            meta.file_name.as_deref().unwrap_or("?"),
            ABSOLUTE_KEEP_UNDER_BYTES
        ));
        return VetoVerdict::KeepGif;
    }

    if meta.file_size_bytes >= ABSOLUTE_CONVERT_OVER_BYTES {
        // Allow opt-in skip of the absolute convert ceiling via environment
        // variable for curated paths: `MFB_GIF_SKIP_CONVERT_CEILING`.
        if meta_matches_skip_convert_ceiling(meta) {
            crate::progress_mode::emit_stderr(&format!(
                "🔓  GIF [{}] → SKIP absolute-convert ceiling (whitelist)",
                meta.file_name.as_deref().unwrap_or("?"),
            ));
        } else if !(is_infinite_loop && meta.duration_secs < dynamic_rhythmic_threshold * 1.1)
            && !(score_premium_all(meta) > 0.90)
        {
            crate::progress_mode::emit_stderr(&format!(
                "🎯  GIF [{}] → CONVERT→VIDEO (absolute size >= {} bytes)",
                meta.file_name.as_deref().unwrap_or("?"),
                ABSOLUTE_CONVERT_OVER_BYTES
            ));
            return VetoVerdict::ConvertVideo;
        } else {
            crate::progress_mode::emit_stderr(&format!(
                "🔔  GIF [{}] → EXEMPT from absolute-convert due to strong loop/premium",
                meta.file_name.as_deref().unwrap_or("?")
            ));
        }
    }

    // High-density large-canvas clips retain too much photographic detail for GIF.
    let density_pressure =
        normalize(temporal_bpp, 0.10, 0.65) * 0.60 + normalize(spatial_bpp, 4.0, 70.0) * 0.40;
    let canvas_pressure = normalize(pixel_count, REFERENCE_AREA, PIXELS_1080P);

    let (bpp_low, bpp_high) = temporal_bpp_thresholds(meta.fps);
    if temporal_bpp >= bpp_high && canvas_pressure > 0.70 && loop_affinity < 0.85 {
        return VetoVerdict::ConvertVideo;
    }
    if density_pressure > 0.78 && canvas_pressure > 0.70 && loop_affinity < 0.55 {
        return VetoVerdict::ConvertVideo;
    }

    // Finite single-shot loops usually originate from ordinary video clips.
    if meta.loop_count == Some(1) && !meta.has_transparency {
        return VetoVerdict::ConvertVideo;
    }

    // Large / long assets with weak loop affinity should not stay in GIF space.
    if duration_ratio > 1.10 && loop_affinity < 0.42 && !meta.has_transparency {
        return VetoVerdict::ConvertVideo;
    }

    // Rhythmic/infinite-loop precomputation moved earlier (absolute guards).

    // CONDITION 3: File size ceiling
    if meta.file_size_bytes > convert_ceiling(meta.width, meta.height) {
        // Allow exception for strong infinite-loop short assets (intentional stickers/loops)
        // and premium rhythmic loops; otherwise convert to avoid storing huge GIFs.
        if !(is_infinite_loop && meta.duration_secs < dynamic_rhythmic_threshold * 1.1)
            && !(score_premium_all(meta) > 0.85)
        {
            return VetoVerdict::ConvertVideo;
        }
    }

    // ── 1. RHYTHMIC STICKER PROTECTION (v5.3) ──
    // Formula: Rhythmic Threshold = 3.5s * (0.12s / avg_frame_delay)
    // The faster it looks (shorter delay), the more duration headroom it gets.
    // (dynamic_rhythmic_threshold already computed above)

    if is_infinite_loop && meta.duration_secs < dynamic_rhythmic_threshold {
        return VetoVerdict::KeepGif;
    }

    // ── 2. STRONG KEEP CONDITIONS (Meme-like) ──

    // CONDITION 1: File size floor OR reaction-sticker duration (< 3.5s)
    if meta.file_size_bytes <= keep_floor(meta.width, meta.height) || meta.duration_secs < 3.5 {
        return VetoVerdict::KeepGif;
    }

    // CONDITION: Duration ceiling (> dynamic threshold * 1.25)
    let mut leniency = if meta.source_extension.as_deref() == Some("gif") {
        1.5
    } else {
        1.0
    };
    if score_premium_all(meta) > 0.85 {
        leniency *= 1.4;
    }

    // Looping assets with strong affinity get more duration headroom
    if loop_affinity > 0.85 {
        leniency *= 1.2;
    }

    if meta.duration_secs > dynamic_threshold * 1.25 * leniency {
        return VetoVerdict::ConvertVideo;
    }

    // ... rest of conditions ...
    if meta.has_transparency {
        return VetoVerdict::KeepGif;
    }
    if score_directory_context(meta.parent_directories.as_deref()) > 0.8 {
        return VetoVerdict::KeepGif;
    }
    if score_pixel_art(meta.palette_size, spatial_bpp, meta.frame_payload_variation) > 0.85 {
        return VetoVerdict::KeepGif;
    }

    let is_loop_intent = infer_looping_intent(meta);
    let variation = meta.frame_payload_variation.unwrap_or(0.0);
    if is_loop_intent && meta.duration_secs < dynamic_threshold && variation < 0.15 {
        return VetoVerdict::KeepGif;
    }

    if let Some(exts) = &meta.app_extensions {
        for ext in exts {
            if MEME_PLATFORM_PREFIXES.iter().any(|p| ext.starts_with(p)) {
                return VetoVerdict::KeepGif;
            }
        }
    }

    if temporal_bpp < bpp_low
        && ((u64::from(meta.width) * u64::from(meta.height)) as f64) < PIXELS_SMALL
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
pub fn score_gif(meta: &GifMeta, knn_prob: Option<f64>) -> MemeScore {
    let pixels = (u64::from(meta.width) * u64::from(meta.height)).max(1);
    let total_frames = meta.frame_count.max(1);
    let spatial_bpp = meta.file_size_bytes as f64 / pixels as f64;
    let temporal_bpp = meta.file_size_bytes as f64 / (pixels * total_frames) as f64;

    // ── Dynamic thresholds & components ──────────────────────────────────────
    let (bpp_low, bpp_high) = temporal_bpp_thresholds(meta.fps);
    let complexity_norm = normalize(temporal_bpp, bpp_low, bpp_high);

    // Per-dimension scores
    let temporal_density_score = 1.0 - complexity_norm;
    let spatial_density_score = 1.0 - normalize(spatial_bpp, 2.0, 80.0);
    let compression_score = 0.7 * temporal_density_score + 0.3 * spatial_density_score;
    let pixel_count = pixels as f64;
    let resolution_score = 1.0 - normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P);

    let dynamic_threshold = dynamic_duration_threshold(meta.width, meta.height, meta.fps).max(0.35);
    let duration_ratio = meta.duration_secs / dynamic_threshold;
    let duration_score = 1.0 - normalize(duration_ratio, 0.35, 1.9);

    let loop_affinity = score_loop_affinity(meta);
    let loop_frequency_score = loop_affinity; // Use comprehensive affinity instead of raw frequency
    let cadence_score = score_sparse_cadence(meta.duration_secs, meta.frame_count);
    let palette_score = score_palette(meta.palette_size).unwrap_or(0.5);
    let transparency_score = score_transparency(meta.has_transparency);
    let frame_variance_score = score_frame_variation(meta.frame_payload_variation);
    let pixel_art_score =
        score_pixel_art(meta.palette_size, spatial_bpp, meta.frame_payload_variation);
    let premium_score = score_premium_all(meta);
    let timing_value_score = score_timing_value(meta.frame_delay_variation, meta.frame_count);
    let directory_score = score_directory_context(meta.parent_directories.as_deref());

    // ── Filename Analysis & PhysComplexity ───────────────────────────────
    let fa = analyze_filename(meta.file_name.as_deref());
    let spatial_norm = normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P);
    let p_complexity = phys_complexity(meta, spatial_norm);

    let attenuation = match fa.kind {
        FilenameKind::HumanSemantic => 0.85,
        FilenameKind::MachineGenerated => 0.95,
        FilenameKind::Ambiguous => 1.00,
    };
    let effective_filename_score = fa.raw * (1.0 - attenuation * p_complexity);

    // ── Softmax Weights ───────────────────────────────────────────────────
    let weights = compute_weights(complexity_norm);

    // Logic Hardening: Live2D high-fps stickers immune to res pressure if tiny
    let is_live2d_exception =
        meta.has_transparency && meta.duration_secs <= 1.5 && meta.fps >= 30.0;
    let res_pressure = if is_live2d_exception {
        normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P * 2.5) * 0.4
    } else {
        normalize(pixel_count, PIXELS_SMALL, PIXELS_1080P * 1.5)
    };
    let res_pressure = res_pressure * (1.0 - premium_score * 0.85);

    let sticker_dampener = 1.0 - 0.45 * res_pressure;

    let mut total: f64 = (loop_frequency_score * sticker_dampener)
        * weights.get("loop_freq").unwrap_or(&0.0)
        + compression_score * weights.get("compression").unwrap_or(&0.0)
        + resolution_score * weights.get("resolution").unwrap_or(&0.0)
        + (duration_score * sticker_dampener) * weights.get("duration").unwrap_or(&0.0)
        + effective_filename_score * weights.get("filename").unwrap_or(&0.0)
        + directory_score * weights.get("directory").unwrap_or(&0.0)
        + palette_score * weights.get("palette").unwrap_or(&0.0)
        + transparency_score * weights.get("transparency").unwrap_or(&0.0)
        + f64::midpoint(
            frame_variance_score,
            block_variance_skewness_score(&meta.mv_magnitudes),
        ) * weights.get("b_skew").unwrap_or(&0.01)
        + timing_value_score * weights.get("interval_cv").unwrap_or(&0.0)
        + premium_score * weights.get("premium").unwrap_or(&0.0)
        + cadence_score * weights.get("cadence").unwrap_or(&0.0)
        + knn_prob.unwrap_or(0.5) * weights.get("knn").unwrap_or(&0.0)
        + (if meta.has_audio { 0.0 } else { 1.0 }) * weights.get("no_audio").unwrap_or(&0.0)
        + fps_anomaly_score(meta.fps) * weights.get("fps_anomaly").unwrap_or(&0.0)
        + iframe_density_score(&meta.frame_types) * weights.get("iframe_dens").unwrap_or(&0.0)
        + bitrate_gini_score(&meta.pkt_sizes) * weights.get("bit_gini").unwrap_or(&0.0)
        + meta.palette_depth.unwrap_or(0.5) * weights.get("p_depth").unwrap_or(&0.0)
        + meta.motion_gini.unwrap_or(0.5) * weights.get("m_gini").unwrap_or(&0.0)
        + meta
            .palette_size
            .map_or(0.5, |s| if s <= 64 { 0.8 } else { 0.2 })
            * weights.get("loop_sim").unwrap_or(&0.0)
        + meta.temporal_flatness.unwrap_or(0.5) * weights.get("temporal_flat").unwrap_or(&0.0)
        + pixel_art_score * weights.get("pixel_art").unwrap_or(&0.0);

    // Content Proxy Exemption Boost
    let boost_multiplier = 1.0 - res_pressure * 0.7;
    if spatial_bpp < 3.0 {
        total += 0.22 * boost_multiplier;
    }

    total -= 0.18 * res_pressure;

    MemeScore {
        total: total.clamp(0.0, 1.0),
        compression: compression_score,
        resolution: resolution_score,
        duration: duration_score,
        fps: 0.5,
        aspect_ratio: 0.5, // deprecated
        filename_score: effective_filename_score,
        loop_frequency_score,
        palette_score,
        transparency_score,
        frame_variance_score,
        timing_value_score,
        directory_score,
        loss_tolerance_score: 0.5,
        cadence_score,
        spatial_bpp,
        temporal_bpp,
        pixel_art_score,
    }
}

/// Simplified caller for [`should_keep_as_gif_with_path`] with no path context.
#[must_use]
pub fn should_keep_as_gif(meta: &GifMeta) -> bool {
    should_keep_as_gif_with_path(meta, None)
}

fn resolved_duration_with_confidence(meta: &GifMeta) -> (f64, bool) {
    let duration_from_meta = meta.duration_secs;
    let duration_from_timing = meta.frame_count > 1 && meta.fps > 0.1;

    if duration_from_meta > 0.11 {
        return (duration_from_meta, true);
    }

    if duration_from_timing {
        return (meta.frame_count as f64 / meta.fps, true);
    }

    ((meta.frame_count.max(1) as f64) / 12.0, false)
}

#[must_use]
pub fn should_keep_as_gif_with_path(meta: &GifMeta, path: Option<&std::path::Path>) -> bool {
    // Clone early so we can populate header-derived fields when a path is available.
    let mut current_meta = meta.clone();

    // If a path is provided and it looks like a GIF, perform a lightweight
    // header-scan so veto rules see loop_count / transparency / palette
    // and frame-payload variation before making a decision.
    if let Some(p) = path {
        // Perform a best-effort header scan; log failures for easier debugging.
        match scan_gif_headers(p) {
            Ok((pal, exts, has_transparency, payload_var, delay_var, loop_count, total_dur)) => {
                current_meta.palette_size = pal;
                current_meta.app_extensions = exts;
                current_meta.has_transparency = has_transparency;
                current_meta.frame_payload_variation = payload_var;
                current_meta.frame_delay_variation = delay_var;
                current_meta.loop_count = loop_count;
                // If the header scan can produce a total duration, prefer it as a
                // higher-confidence signal than the probe fallback.
                if let Some(d) = total_dur {
                    current_meta.duration_secs = d;
                }
            }
            Err(e) => {
                crate::progress_mode::emit_stderr(&format!(
                    "🔍 GIF header scan failed for {}: {}",
                    p.display(), e
                ));
            }
        }

        // Populate conservative defaults for any GIF-specific fields that
        // remained missing after a best-effort header-scan.
        ensure_conservative_gif_defaults(&mut current_meta, Some(p));

        // Extra short-loop safety guard: when header-scan indicates an infinite
        // loop or extremely low frame-payload variation on a short asset,
        // prefer keeping as GIF (path-aware conservative default).
        let dyn_thr = dynamic_duration_threshold(current_meta.width, current_meta.height, current_meta.fps).max(0.35);
        let shortish = current_meta.duration_secs <= dyn_thr * 1.6;
        let low_variation = current_meta.frame_payload_variation.unwrap_or(1.0) < 0.18;
        let small_size_guard = current_meta.file_size_bytes <= convert_ceiling(current_meta.width, current_meta.height) / 2;

        if (current_meta.loop_count == Some(0) || low_variation) && shortish && small_size_guard {
            crate::progress_mode::emit_stderr(&format!(
                "🎯  GIF [{}] → KEEP GIF (short-loop header-scan guard)",
                current_meta.file_name.as_deref().unwrap_or("?"),
            ));
            return true;
        }
    }

    let is_path_aware_gif_candidate = path.is_some()
        && (current_meta.source_extension.as_deref() == Some("gif")
            || current_meta.palette_size.is_some()
            || current_meta.app_extensions.is_some());
    let (estimated_duration, duration_is_confident) = resolved_duration_with_confidence(&current_meta);

    // Promote a simple duration estimate into the working meta before veto
    // evaluation so missing/zero probe durations do not masquerade as ultra-
    // short loops and bypass the intended keep-vs-convert logic.
    if is_path_aware_gif_candidate && !duration_is_confident {
        current_meta.duration_secs = estimated_duration;
    }

    // ── Phase 0: Veto rules (Zero-cost) ──────────────────────────────────
    let pixels = (u64::from(current_meta.width) * u64::from(current_meta.height)).max(1);
    let total_frames = current_meta.frame_count.max(1);
    let spatial_bpp = current_meta.file_size_bytes as f64 / pixels as f64;
    let temporal_bpp = current_meta.file_size_bytes as f64 / (pixels * total_frames) as f64;

    match apply_veto(&current_meta, temporal_bpp, spatial_bpp) {
        VetoVerdict::KeepGif => {
            crate::progress_mode::emit_stderr(&format!(
                "🎞️  GIF [{}] → KEEP GIF (veto early-exit verdict)",
                current_meta.file_name.as_deref().unwrap_or("?"),
            ));
            return true;
        }
        VetoVerdict::ConvertVideo => {
            crate::progress_mode::emit_stderr(&format!(
                "🎞️  GIF [{}] → CONVERT→VIDEO (veto early-exit verdict)",
                current_meta.file_name.as_deref().unwrap_or("?"),
            ));
            return false;
        }
        VetoVerdict::Undecided => {}
    }

    // Baseline fallback: for path-aware GIF candidates that remain undecided
    // and still lack trustworthy timing, revert to a fixed duration cutoff.
    if is_path_aware_gif_candidate && !duration_is_confident {
        const HARDCODE_FALLBACK_DUR: f64 = 4.25;
        if estimated_duration <= HARDCODE_FALLBACK_DUR {
            crate::progress_mode::emit_stderr(&format!(
                "🔁  GIF [{}] → KEEP GIF (duration-fallback <= {:.2}s)",
                current_meta.file_name.as_deref().unwrap_or("?"),
                HARDCODE_FALLBACK_DUR
            ));
            return true;
        } else {
            crate::progress_mode::emit_stderr(&format!(
                "🔁  GIF [{}] → CONVERT→VIDEO (duration-fallback > {:.2}s)",
                current_meta.file_name.as_deref().unwrap_or("?"),
                HARDCODE_FALLBACK_DUR
            ));
            return false;
        }
    }

    // ── Phase 1 & 2: Low/Medium Cost Cascade ─────────────────────────────
    let mut score = composite_gif_score(&current_meta);

    // Conclusive early exits
    if score >= 0.92 {
        crate::progress_mode::emit_stderr(&format!(
            "🎞️  GIF [{}] → KEEP GIF (cascade early-exit score {:.2})",
            current_meta.file_name.as_deref().unwrap_or("?"),
            score
        ));
        return true;
    }
    if score < 0.12 && current_meta.width > 720 {
        crate::progress_mode::emit_stderr(&format!(
            "🎞️  GIF [{}] → CONVERT→VIDEO (cascade early-exit score {:.2})",
            current_meta.file_name.as_deref().unwrap_or("?"),
            score
        ));
        return false;
    }

    // Phase 3: Deep Refinement (only for uncertain cases)
    if let Some(path) = path {
        if deep_refine_meta(&mut current_meta, path).is_ok() {
            score = composite_gif_score(&current_meta);
            crate::progress_mode::emit_stderr(&format!(
                "🔭  GIF [{}] → Final score: {:.2} (Deep Signals Applied)",
                current_meta.file_name.as_deref().unwrap_or("?"),
                score
            ));
        }
    }

    let label = score >= 0.58;
    crate::progress_mode::emit_stderr(&format!(
        "🎞️  GIF [{}] → {} (weighted score {:.2})",
        current_meta.file_name.as_deref().unwrap_or("?"),
        if label { "KEEP GIF" } else { "CONVERT→VIDEO" },
        score
    ));
    label
}

/// Performs deep signal extraction (Palette, YDIF, Block Skew) using ffmpeg benchmarks.
pub fn deep_refine_meta(
    meta: &mut GifMeta,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Extract Temporal Flatness (YDIF) viaffprobe if possible, or ffmpeg signalstats
    // We'll use a fast ffmpeg pass for YDIF and sample frames
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-i",
            crate::path_safety::safe_path_arg(path).as_ref(),
            "-vf",
            "signalstats,metadata=print",
            "-f",
            "null",
            "-",
        ])
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

    // 2. Extract Palette Depth (Stage 2: Sample first frame)
    let thumb_output = std::process::Command::new("ffmpeg")
        .args([
            "-i",
            crate::path_safety::safe_path_arg(path).as_ref(),
            "-frames:v",
            "1",
            "-vf",
            "scale=64:64",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
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

/// Probe a path into [`GifMeta`]. Native GIFs get the cheap header scan
/// attached; video containers keep probe-derived metadata only.
#[must_use]
pub fn gif_candidate_meta_from_path(path: &std::path::Path) -> Option<GifMeta> {
    let file_size = std::fs::metadata(path).ok()?.len();
    let probe = crate::probe_video(path).ok()?;
    let mut meta = gif_meta_from_probe_with_path(&probe, file_size, path)?;

    // Try a lightweight header-scan regardless of extension detection. The
    // scanner returns neutral values for non-GIF files; on I/O errors we log
    // and continue with probe-only metadata.
    match scan_gif_headers(path) {
        Ok((pal, exts, has_transparency, variation, delay_variation, loops, total_dur)) => {
            meta.palette_size = pal;
            meta.app_extensions = exts;
            meta.has_transparency = has_transparency;
            meta.frame_payload_variation = variation;
            meta.frame_delay_variation = delay_variation;
            meta.loop_count = loops;
            if let Some(d) = total_dur {
                meta.duration_secs = d;
            }
        }
        Err(e) => {
            crate::progress_mode::emit_stderr(&format!(
                "🔍 GIF header scan failed for {}: {}",
                path.display(), e
            ));
        }
    }

    // Apply conservative defaults for any missing GIF-specific fields
    // when a filesystem path is available.
    ensure_conservative_gif_defaults(&mut meta, Some(path));

    Some(meta)
}

/// Returns `Some(keep)` when the path is either a native GIF or a short silent
/// video that statistically behaves like a GIF/sticker asset.
#[must_use]
pub fn should_keep_as_gif_candidate_path(path: &std::path::Path) -> Option<bool> {
    let meta = gif_candidate_meta_from_path(path)?;
    let is_candidate =
        meta.source_extension.as_deref() == Some("gif") || is_probably_gif_like_video(&meta);
    is_candidate.then(|| should_keep_as_gif_with_path(&meta, Some(path)))
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
        loop_count: probe.loop_count,
        has_audio: probe.has_audio,
        frame_types: probe.frame_types.clone(),
        pts_deltas: probe.pts_deltas.clone(),
        mv_magnitudes: probe.mv_magnitudes.clone(),
        palette_depth: None,
        motion_gini: Some(motion_gini_score(&probe.mv_magnitudes)),
        block_skew: None,
        temporal_flatness: None,
        pkt_sizes: probe.pkt_sizes.clone(),
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
        loop_count: probe.loop_count,
        has_audio: probe.has_audio,
        frame_types: probe.frame_types.clone(),
        pts_deltas: probe.pts_deltas.clone(),
        mv_magnitudes: probe.mv_magnitudes.clone(),
        palette_depth: None,
        motion_gini: Some(motion_gini_score(&probe.mv_magnitudes)),
        block_skew: None,
        temporal_flatness: None,
        pkt_sizes: probe.pkt_sizes.clone(),
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
    Option<u16>,
    Option<f64>, // total duration in seconds (derived from frame delays if present)
)> {
    let buf = std::fs::read(path)?;
    let n = buf.len();

    if n < 13 {
        return Ok((None, None, false, None, None, None, None));
    }

    // GIF87a / GIF89a magic check
    if &buf[0..6] != b"GIF87a" && &buf[0..6] != b"GIF89a" {
                return Ok((None, None, false, None, None, None, None));
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

    let mut app_extensions: Vec<String> = Vec::new();
    let mut has_transparency = false;
    let mut loop_count: Option<u16> = None;
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
                        if let Ok(vendor) = std::str::from_utf8(&buf[pos + 3..pos + 3 + block_size])
                        {
                            if !vendor.is_empty() {
                                app_extensions.push(vendor.to_owned());
                                // Check for NETSCAPE2.0 loop count
                                if vendor == "NETSCAPE2.0" {
                                    let sub_pos = pos + 3 + block_size;
                                    if sub_pos + 3 < buf.len() {
                                        let sub_size = buf[sub_pos];
                                        if sub_size >= 3 && buf[sub_pos + 1] == 0x01 {
                                            loop_count = Some(
                                                u16::from(buf[sub_pos + 2])
                                                    | (u16::from(buf[sub_pos + 3]) << 8),
                                            );
                                        }
                                    }
                                }
                            }
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
            None
        }
    } else {
        None
    };

    // total duration in seconds from frame delays (centiseconds -> seconds)
    let total_duration_secs = if !frame_delays_cs.is_empty() {
        Some(frame_delays_cs.iter().map(|&d| d as f64).sum::<f64>() / 100.0)
    } else {
        None
    };

    Ok((
        palette_size,
        app_extensions,
        has_transparency,
        frame_payload_variation,
        frame_delay_variation,
        loop_count,
        total_duration_secs,
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

/// Populate conservative defaults for missing GIF-specific fields when a
/// filesystem `path` is available. This intentionally only runs when a path
/// was provided (so probe-only callers are unaffected) and when the asset
/// looks GIF-like (either by extension or by header-derived hints).
fn ensure_conservative_gif_defaults(meta: &mut GifMeta, path: Option<&std::path::Path>) {
    let Some(_p) = path else { return; };

    // Apply only to assets that either declare `gif` extension or where a
    // header-scan populated GIF-specific signals.
    let is_gif_candidate = meta.source_extension.as_deref() == Some("gif")
        || meta.palette_size.is_some()
        || meta.app_extensions.is_some();

    if !is_gif_candidate {
        return;
    }

    if meta.palette_size.is_none() {
        meta.palette_size = Some(64);
    }
    if meta.frame_payload_variation.is_none() {
        // Conservative low-variation default to avoid false-converting small
        // cyclic GIFs when header scan lacked sub-block sampling.
        meta.frame_payload_variation = Some(0.18);
    }
    if meta.frame_delay_variation.is_none() {
        meta.frame_delay_variation = Some(0.12);
    }
    if meta.palette_depth.is_none() {
        meta.palette_depth = Some(0.5);
    }
    if meta.motion_gini.is_none() {
        meta.motion_gini = Some(0.5);
    }
}

/// Helper: match comma-separated environment patterns against `file_name` or
/// `parent_directories` present in `GifMeta`.
fn meta_matches_env_list(meta: &GifMeta, varname: &str) -> bool {
    let var = match std::env::var_os(varname) {
        Some(v) => v.to_string_lossy().to_string(),
        None => return false,
    };
    if var.trim().is_empty() {
        return false;
    }

    let patterns: Vec<&str> = var
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if patterns.is_empty() {
        return false;
    }

    if let Some(name) = &meta.file_name {
        let name_l = name.to_lowercase();
        for pat in &patterns {
            if name_l.contains(&pat.to_lowercase()) {
                return true;
            }
        }
    }

    if let Some(parents) = &meta.parent_directories {
        for p in parents {
            let p_l = p.to_lowercase();
            for pat in &patterns {
                if p_l.contains(&pat.to_lowercase()) {
                    return true;
                }
            }
        }
    }

    false
}

/// Check for explicit env-driven overrides. Use `MFB_GIF_FORCE_KEEP` and
/// `MFB_GIF_FORCE_CONVERT` to force decisions (comma-separated substrings).
fn check_meta_override(meta: &GifMeta) -> Option<VetoVerdict> {
    if meta_matches_env_list(meta, "MFB_GIF_FORCE_KEEP") {
        return Some(VetoVerdict::KeepGif);
    }
    if meta_matches_env_list(meta, "MFB_GIF_FORCE_CONVERT") {
        return Some(VetoVerdict::ConvertVideo);
    }
    None
}

/// If `MFB_GIF_SKIP_CONVERT_CEILING` matches the meta, skip the absolute
/// convert-over size ceiling for that asset.
fn meta_matches_skip_convert_ceiling(meta: &GifMeta) -> bool {
    meta_matches_env_list(meta, "MFB_GIF_SKIP_CONVERT_CEILING")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONF_KEEP: f64 = 0.58;

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
            loop_count: None,
            has_audio: false,
            frame_types: vec!['P'; frames as usize],
            pts_deltas: vec![duration / frames.max(1) as f64; frames as usize],
            mv_magnitudes: Vec::new(),
            palette_depth: None,
            motion_gini: None,
            block_skew: None,
            temporal_flatness: None,
            pkt_sizes: vec![size / frames.max(1); frames as usize],
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
        let mut m = make_meta(duration, w, h, fps, frames, size);
        m.file_name = Some(name.to_string());
        m
    }

    // ── score_gif tests ───────────────────────────────────────────────────────

    #[test]
    fn tiny_meme_scores_high() {
        // 200×200, 2s, 10fps, 20 frames, tiny file → should score ≥ 0.5
        let meta = make_meta(2.0, 200, 200, 10.0, 20, 40_000);
        let s = score_gif(&meta, None);
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
        let s = score_gif(&meta, None);
        assert!(
            s.total < 0.50,
            "expected video score < 0.5, got {:.3}",
            s.total
        );
    }

    #[test]
    fn score_gif_exposes_bytes_per_pixel() {
        let meta = make_meta(3.0, 300, 300, 12.0, 36, 270_000);
        let s = score_gif(&meta, None);
        assert!(
            s.temporal_bpp > 0.0 && s.spatial_bpp > 0.0,
            "bpp metrics should be positive"
        );
    }

    #[test]
    fn square_aspect_ratio_maxes_out() {
        let meta = make_meta(3.0, 300, 300, 12.0, 36, 200_000);
        let s = score_gif(&meta, None);
        assert!(
            (s.aspect_ratio - 0.5).abs() < 1e-9,
            "aspect_ratio is deprecated and returns 0.5"
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
        // Now explicitly kept via: (infinite && short && low_change)
        let meta = make_meta(5.0, 640, 480, 15.0, 75, 500_000);
        assert_eq!(apply_veto(&meta, 0.10, 1.7), VetoVerdict::KeepGif);
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
        let s = score_gif(&meta, None);
        // If score is below keep threshold, it should default to convert.
        if s.total < CONF_KEEP {
            assert!(
                !should_keep_as_gif(&meta),
                "below keep threshold must default to convert"
            );
        }
        // If it landed outside the zone, just verify no panic
    }

    // ── gif_meta_from_probe tests ─────────────────────────────────────────────

    // ── Edge case tests for score_gif ─────────────────────────────────────────

    #[test]
    fn score_gif_zero_duration() {
        // Edge case: zero duration should not panic
        let meta = make_meta(0.0, 100, 100, 10.0, 0, 1000);
        let s = score_gif(&meta, None);
        assert!(s.total.is_finite(), "score should be finite");
    }

    #[test]
    fn score_gif_extremely_high_fps() {
        // Edge case: 1000fps should not cause overflow
        let meta = make_meta(1.0, 200, 200, 1000.0, 1000, 500_000);
        let s = score_gif(&meta, None);
        assert!(s.total.is_finite(), "score should be finite for high fps");
    }

    #[test]
    fn score_gif_single_frame() {
        // Edge case: single frame GIF (effectively static image)
        let meta = make_meta(0.1, 300, 300, 10.0, 1, 50_000);
        let s = score_gif(&meta, None);
        // Just verify it runs and produces finite score
        assert!(s.total.is_finite(), "single frame should produce finite score");
    }

    #[test]
    fn score_gif_massive_file() {
        // Edge case: 100MB+ GIF
        let meta = make_meta(60.0, 4096, 4096, 30.0, 1800, 100_000_000);
        let s = score_gif(&meta, None);
        assert!(s.total < 0.5, "massive file should score low");
    }

    #[test]
    fn score_gif_degenerate_dimensions() {
        // Edge case: 1x1 pixel GIF
        let meta = make_meta(1.0, 1, 1, 10.0, 10, 100);
        let s = score_gif(&meta, None);
        assert!(s.total.is_finite(), "score should be finite for 1x1");
    }

    #[test]
    fn score_gif_extreme_aspect_ratio() {
        // Edge case: very wide or tall GIF
        let meta_wide = make_meta(1.0, 1000, 10, 10.0, 10, 10_000);
        let s_wide = score_gif(&meta_wide, None);
        assert!(s_wide.total.is_finite(), "wide aspect should be finite");

        let meta_tall = make_meta(1.0, 10, 1000, 10.0, 10, 10_000);
        let s_tall = score_gif(&meta_tall, None);
        assert!(s_tall.total.is_finite(), "tall aspect should be finite");
    }

    // ── Edge case tests for apply_veto ────────────────────────────────────────

    #[test]
    fn veto_zero_frame_count() {
        let meta = make_meta(1.0, 100, 100, 10.0, 0, 1000);
        let result = apply_veto(&meta, 0.1, 1.0);
        assert!(matches!(result, VetoVerdict::KeepGif | VetoVerdict::Undecided));
    }

    #[test]
    fn veto_extreme_bpp_values() {
        let meta = make_meta(1.0, 100, 100, 10.0, 10, 1000);
        
        // Very high bpp
        let result_high = apply_veto(&meta, 0.99, 1.0);
        assert!(result_high != VetoVerdict::Undecided);
        
        // Very low bpp
        let result_low = apply_veto(&meta, 0.001, 1.0);
        assert!(result_low != VetoVerdict::Undecided);
    }

    #[test]
    fn veto_boundary_conditions() {
        // Test exact boundary values - just verify no panic
        let _meta_1080p = make_meta(10.0, 1920, 1080, 30.0, 300, 5_000_000);
        
        // Just under 15s threshold
        let meta_14_9s = make_meta(14.9, 1920, 1080, 30.0, 447, 5_000_000);
        let _result_under = apply_veto(&meta_14_9s, 0.3, 3.0);
        
        // Just over 15s threshold
        let meta_15_1s = make_meta(15.1, 1920, 1080, 30.0, 453, 5_000_000);
        let _result_over = apply_veto(&meta_15_1s, 0.3, 3.0);
    }

    // ── Edge case tests for should_keep_as_gif ────────────────────────────────

    #[test]
    fn should_keep_edge_case_zero_size() {
        let meta = make_meta(1.0, 100, 100, 10.0, 10, 0);
        let result = should_keep_as_gif(&meta);
        // Should not panic, result depends on other factors
        assert!(result || !result); // Just verify it runs
    }

    #[test]
    fn should_keep_transparency_edge_cases() {
        // Very short transparent GIF (should always keep)
        let mut meta = make_meta(0.1, 100, 100, 30.0, 3, 5000);
        meta.has_transparency = true;
        assert!(should_keep_as_gif(&meta), "short transparent should keep");

        // Long transparent GIF (might convert)
        let mut meta_long = make_meta(30.0, 500, 500, 30.0, 900, 5_000_000);
        meta_long.has_transparency = true;
        // Long duration overrides transparency
        let s = score_gif(&meta_long, None);
        if s.total < CONF_KEEP {
            assert!(!should_keep_as_gif(&meta_long));
        }
    }

    #[test]
    fn should_keep_loop_count_edge_cases() {
        // Infinite loop (loop_count = 0)
        let mut meta_infinite = make_meta(2.0, 200, 200, 15.0, 30, 100_000);
        meta_infinite.loop_count = Some(0);
        assert!(should_keep_as_gif(&meta_infinite), "infinite loop should keep");

        // Single play (loop_count = 1)
        let mut meta_single = make_meta(2.0, 200, 200, 15.0, 30, 100_000);
        meta_single.loop_count = Some(1);
        // Single play might convert depending on other factors
        let _ = should_keep_as_gif(&meta_single); // Just verify no panic
    }

    // ── Rhythmic sticker detection tests ──────────────────────────────────────

    #[test]
    fn is_rhythmic_sticker_short_high_cadence() {
        // Short duration with moderate size → should be kept as GIF
        let meta = make_meta(0.5, 200, 200, 30.0, 15, 50_000);
        assert!(meta.is_rhythmic_sticker(), "short content should be kept");
    }

    #[test]
    fn is_rhythmic_sticker_long_duration() {
        // Long duration (>15s) with 1080p → convert veto
        let meta = make_meta(20.0, 1920, 1080, 30.0, 600, 5_000_000);
        assert!(!meta.is_rhythmic_sticker(), "long 1080p should convert");
    }

    #[test]
    fn is_rhythmic_sticker_low_fps() {
        // Low fps with small size → might still keep
        let meta = make_meta(1.0, 100, 100, 5.0, 5, 5_000);
        // Just verify it runs without panic
        let _ = meta.is_rhythmic_sticker();
    }

    #[test]
    fn is_rhythmic_sticker_boundary_behavior() {
        // Test that boundary values don't cause panic
        let meta_small = make_meta(1.0, 50, 50, 10.0, 10, 5_000);
        assert!(meta_small.is_rhythmic_sticker() || !meta_small.is_rhythmic_sticker());
        
        let meta_large = make_meta(30.0, 1920, 1080, 30.0, 900, 10_000_000);
        assert!(meta_large.is_rhythmic_sticker() || !meta_large.is_rhythmic_sticker());
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
            loop_count: None,
            has_audio: false,
            frame_types: vec!['P'; frames as usize],
            pts_deltas: vec![duration / frames.max(1) as f64; frames as usize],
            mv_magnitudes: Vec::new(),
            palette_depth: None,
            motion_gini: None,
            block_skew: None,
            temporal_flatness: None,
            pkt_sizes: vec![size / frames.max(1); frames as usize],
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
        meta.parent_directories = Some(vec!["sticker_pack_01".to_string()]);
        let s = score_gif(&meta, None);
        assert!(s.directory_score > 0.9);
    }

    #[test]
    fn sparse_slideshow_cadence_scores_high() {
        let meta = make_meta(6.0, 720, 1280, 1.0, 6, 500_000);
        let s = score_gif(&meta, None);
        assert!(s.cadence_score > 0.9);
    }

    #[test]
    fn irregular_timing_scores_high() {
        let mut meta = make_meta(3.0, 320, 320, 12.0, 12, 120_000);
        meta.frame_delay_variation = Some(0.9);
        let s = score_gif(&meta, None);
        assert!(s.timing_value_score > 0.7);
    }

    #[test]
    fn uniform_timing_scores_low() {
        let mut meta = make_meta(3.0, 320, 320, 12.0, 12, 120_000);
        meta.frame_delay_variation = Some(0.02);
        let s = score_gif(&meta, None);
        assert!(s.timing_value_score < 0.1);
    }

    // ── New dimension tests ───────────────────────────────────────────────────

    #[test]
    fn filename_single_word_scores_high() {
        let meta = make_meta_with_name(3.0, 300, 300, 12.0, 36, 200_000, "laugh");
        let s = score_gif(&meta, None);
        assert!(
            s.filename_score >= 0.8,
            "single word should score high: {:.2}",
            s.filename_score
        );
    }

    #[test]
    fn filename_multi_word_scores_low() {
        let meta = make_meta_with_name(3.0, 300, 300, 12.0, 36, 200_000, "my_vacation_video_2024");
        let s = score_gif(&meta, None);
        assert!(
            s.filename_score <= 0.6,
            "multi-word should score low: {:.2}",
            s.filename_score
        );
    }

    #[test]
    fn filename_chinese_single_char() {
        let meta = make_meta_with_name(3.0, 300, 300, 12.0, 36, 200_000, "笑");
        let s = score_gif(&meta, None);
        assert!(
            s.filename_score >= 0.8,
            "single CJK char should score high: {:.2}",
            s.filename_score
        );
    }

    #[test]
    fn loop_frequency_fast_loop_scores_high() {
        // 2s duration → 30 loops/min
        let meta = make_meta(2.0, 300, 300, 10.0, 20, 100_000);
        let s = score_gif(&meta, None);
        assert!(
            s.loop_frequency_score >= 0.7,
            "fast loop should score high: {:.2}",
            s.loop_frequency_score
        );
    }

    #[test]
    fn loop_frequency_slow_loop_scores_low() {
        // 40s duration → 1.5 loops/min
        let meta = make_meta(40.0, 1920, 1080, 30.0, 1200, 5_000_000);
        let s = score_gif(&meta, None);
        assert!(
            s.loop_frequency_score <= 0.6,
            "slow loop should score low: {:.2}",
            s.loop_frequency_score
        );
    }

    #[test]
    fn large_hq_sticker_modern_retention() {
        // 1024x1024, 1.2s, transparent, square.
        // Modern stickers should be kept as GIF despite high resolution.
        let mut meta = make_meta(1.2, 1024, 1024, 15.0, 18, 1_500_000);
        meta.has_transparency = true;
        assert!(
            should_keep_as_gif(&meta),
            "modern hq sticker should be kept as gif"
        );
    }

    #[test]
    fn high_fps_live2d_retention() {
        // 640x640, 0.8s, 60fps, short loop.
        // High FPS should not be the sole reason for video conversion.
        let mut meta = make_meta(0.8, 640, 640, 60.0, 48, 800_000);
        meta.has_transparency = true;
        assert!(
            should_keep_as_gif(&meta),
            "high-fps live2d sticker should be kept"
        );
    }

    #[test]
    fn medium_res_short_loop_veto_keep() {
        // 720p-class but only 2s fast loop.
        let meta = make_meta(2.0, 1280, 720, 15.0, 30, 2_000_000);
        assert!(
            should_keep_as_gif(&meta),
            "medium-res short loop should trigger veto keep"
        );
    }
    #[test]
    fn hq_large_sticker_should_be_kept() {
        // 1024x1024, 1.2s, 15fps, 18 frames, ~1.5MB file
        // High-quality stickers should be preserved.
        let meta = make_meta(1.2, 1024, 1024, 15.0, 18, 1_500_000);
        let s = score_gif(&meta, None);
        assert!(
            should_keep_as_gif(&meta),
            "High-quality 1024px sticker should be kept (score: {:.3})",
            s.total
        );
    }

    #[test]
    fn high_fps_sticker_should_be_kept() {
        // 512x512, 1.0s, 60fps, 60 frames (Live2D style)
        // High frame rate but short duration; should be kept as GIF.
        let meta = make_meta(1.0, 512, 512, 60.0, 60, 800_000);
        assert!(
            should_keep_as_gif(&meta),
            "High-FPS short sticker should be kept"
        );
    }

    #[test]
    fn very_short_loop_high_res_should_be_kept() {
        // 1280x720, 0.8s, 24fps, 19 frames, 1.2MB
        // Extremely short and highly cyclic.
        let meta = make_meta(0.8, 1280, 720, 24.0, 19, 1_200_000);
        assert!(
            should_keep_as_gif(&meta),
            "Very short high-res loop should be kept"
        );
    }

    #[test]
    fn gif_source_leniency_retention() {
        // 1600x900 (Large!), 3.5s (longer), but source EXT is GIF!
        // Should be kept via the new "Existing GIF Leniency" veto.
        let mut meta = make_meta(3.5, 1600, 900, 15.0, 52, 2_500_000);
        meta.source_extension = Some("gif".to_string());
        assert!(
            should_keep_as_gif(&meta),
            "Original GIF file with moderate length/res should be kept"
        );
    }

    #[test]
    fn ppt_slideshow_modern_boost() {
        // Modern format (MOV), 4s duration, only 8 frames (sparse)
        let mut meta = make_meta(4.0, 1280, 720, 2.0, 8, 800_000);
        meta.source_extension = Some("mov".to_string());
        let s_no_boost = score_gif(&meta, None);

        // Cadence score should be high
        assert!(s_no_boost.cadence_score > 0.9);

        // Total score should be boosted or high enough to keep
        assert!(
            s_no_boost.total >= 0.55,
            "PPT-like modern format should score high: {:.3}",
            s_no_boost.total
        );
    }

    #[test]
    fn knn_proximity_boosts_score() {
        let meta = make_meta(3.0, 320, 320, 12.0, 36, 120_000);
        let s_none = score_gif(&meta, None);
        let s_high = score_gif(&meta, Some(1.0));
        let s_low = score_gif(&meta, Some(0.0));

        assert!(s_high.total >= s_none.total);
        assert!(s_low.total <= s_none.total);
    }

    #[test]
    fn roi_floor_small_file_veto() {
        // 80KB file should be kept regardless of other signals
        let meta = make_meta(5.0, 1280, 720, 10.0, 50, 80_000);
        assert!(should_keep_as_gif(&meta), "Files < 100KB must be kept");
    }

    #[test]
    fn roi_floor_short_loop_veto() {
        // 3-frame animation should be kept
        let meta = make_meta(0.5, 640, 480, 6.0, 3, 200_000);
        assert!(
            should_keep_as_gif(&meta),
            "Animations with ≤ 3 frames must be kept"
        );
    }

    #[test]
    fn pixel_art_retention() {
        // 16 colors, very low spatial bpp
        let mut meta = make_meta(2.0, 320, 320, 10.0, 20, 30_000);
        meta.palette_size = Some(16);
        meta.frame_payload_variation = Some(0.05);
        let s = score_gif(&meta, None);
        assert!(
            s.total >= 0.60,
            "Pixel art should score high for retention: {:.3}",
            s.total
        );
    }

    #[test]
    fn transparency_compatibility_guard() {
        // 720p transparent should be kept per user ROI advice
        let mut meta = make_meta(2.0, 1280, 720, 15.0, 30, 1_500_000);
        meta.has_transparency = true;
        assert!(should_keep_as_gif(&meta), "Transparent 720p should be kept");
    }

    #[test]
    fn high_roi_photographic_conversion() {
        // 5MB, 256 colors, high motion, high BPP -> definitely convert
        let mut meta = make_meta(10.0, 1280, 720, 30.0, 300, 5_000_000);
        meta.palette_size = Some(256);
        meta.frame_payload_variation = Some(0.8);
        let s = score_gif(&meta, None);
        assert!(
            s.total < 0.65,
            "High ROI photographic content should score low enough (convert): {:.3}",
            s.total
        );
    }

    #[test]
    fn test_premium_high_res_loop_retention() {
        // 8.5MB, 1920x1080 (Huge!), 256 colors, 30fps, 3.0s, perfect rhythm.
        // This is a "Premium Loop" — should be kept as GIF despite massive size.
        let mut meta = make_meta(3.0, 1920, 1080, 30.0, 90, 8_500_000);
        meta.palette_size = Some(256);
        meta.frame_delay_variation = Some(0.01); // Extremely rhythmic
        meta.frame_payload_variation = Some(0.1); // Consistent frame weight

        let s = score_gif(&meta, None);
        // Premium vibe should be very high
        assert!(score_premium_all(&meta) > 0.90);

        // Total score should be > 0.52 (CONF_KEEP)
        assert!(
            should_keep_as_gif(&meta),
            "8.5MB Premium Loop must be kept (score: {:.3})",
            s.total
        );
    }

    #[test]
    fn test_dynamic_duration_threshold() {
        // 720p equivalent -> 4.25s
        let t_720 = dynamic_duration_threshold(720, 720, 12.0);
        assert!((t_720 - 4.25).abs() < 0.01);

        // 360p (half dimensions) -> double duration (8.5s)
        let t_360 = dynamic_duration_threshold(360, 360, 12.0);
        assert!((t_360 - 8.5).abs() < 0.01);

        // 1440p (double dimensions) -> half duration (2.125s)
        let t_1440 = dynamic_duration_threshold(1440, 1440, 12.0);
        assert!((t_1440 - 2.125).abs() < 0.01);
    }

    #[test]
    fn veto_keep_floor_and_short_frames() {
        // Size floor (<= 100KB)
        let meta_small = make_meta(10.0, 300, 300, 10.0, 100, 102_400);
        assert_eq!(apply_veto(&meta_small, 0.1, 1.1), VetoVerdict::KeepGif);

        // Few frames (<= 3)
        let meta_frames = make_meta(1.0, 300, 300, 3.0, 3, 500_000);
        assert_eq!(apply_veto(&meta_frames, 0.1, 1.1), VetoVerdict::KeepGif);
    }

    #[test]
    fn veto_convert_ceiling_and_color() {
        // Size ceiling (> 10MB) - use parameters that won't trigger exemption
        // Long duration to avoid "sticker" protection, normal fps to avoid "premium loop" signal
        let meta_huge = make_meta(20.0, 300, 300, 24.0, 480, 11_000_000);
        assert_eq!(apply_veto(&meta_huge, 0.1, 1.1), VetoVerdict::ConvertVideo);

        // ICC Profile
        let mut meta_icc = make_meta(1.0, 300, 300, 10.0, 10, 500_000);
        meta_icc.has_embedded_icc = true;
        assert_eq!(apply_veto(&meta_icc, 0.1, 1.1), VetoVerdict::ConvertVideo);
    }

    #[test]
    fn veto_loop_count_semantics() {
        // Infinite loop (None/0) short/low change -> Keep
        let mut meta_inf = make_meta(2.0, 720, 720, 10.0, 20, 500_000);
        meta_inf.loop_count = Some(0);
        meta_inf.frame_payload_variation = Some(0.05);
        assert_eq!(apply_veto(&meta_inf, 0.1, 1.1), VetoVerdict::KeepGif);

        // Finite loop (1) -> Convert
        let mut meta_fin = make_meta(2.0, 720, 720, 10.0, 20, 500_000);
        meta_fin.loop_count = Some(1);
        assert_eq!(apply_veto(&meta_fin, 0.1, 1.1), VetoVerdict::ConvertVideo);
    }

    #[test]
    fn resolution_duration_combo_veto() {
        // Moderate mid-size loop content is no longer forced to video by a
        // single width/duration threshold; it stays eligible for GIF retention.
        let meta_res = make_meta(2.1, 800, 600, 10.0, 21, 500_000);
        assert_eq!(apply_veto(&meta_res, 0.1, 1.1), VetoVerdict::KeepGif);
    }

    #[test]
    fn absolute_small_size_kept_via_veto() {
        let meta = make_meta(2.0, 320, 240, 12.0, 24, ABSOLUTE_KEEP_UNDER_BYTES);
        assert_eq!(apply_veto(&meta, 0.1, 1.0), VetoVerdict::KeepGif);
    }

    #[test]
    fn absolute_large_size_converted_via_veto() {
        // Use parameters that won't trigger exemption (long duration, normal fps)
        // to ensure the absolute size ceiling is enforced
        let meta = make_meta(15.0, 1024, 768, 30.0, 450, ABSOLUTE_CONVERT_OVER_BYTES + 1);
        assert_eq!(apply_veto(&meta, 0.5, 2.0), VetoVerdict::ConvertVideo);
    }

    #[test]
    fn ensure_defaults_only_with_path() {
        let mut meta = make_meta(1.0, 200, 200, 12.0, 20, 50_000);
        meta.source_extension = Some("gif".to_string());

        // No path -> defaults should NOT be populated
        ensure_conservative_gif_defaults(&mut meta, None);
        assert!(meta.frame_payload_variation.is_none());

        // With a path present -> conservative defaults applied
        ensure_conservative_gif_defaults(&mut meta, Some(std::path::Path::new("/tmp")));
        assert!(meta.frame_payload_variation.is_some());
    }

    #[test]
    fn duration_fallback_keeps_short_undecided_gif_candidates() {
        let mut meta = make_meta(0.0, 900, 900, 0.0, 48, 2_000_000);
        meta.source_extension = Some("gif".to_string());
        meta.file_name = Some("short-undecided.gif".to_string());
        meta.palette_size = Some(256);
        meta.app_extensions = Some(Vec::new());
        meta.frame_payload_variation = Some(0.8);
        meta.frame_delay_variation = Some(0.5);
        meta.loop_count = Some(2);

        assert!(should_keep_as_gif_with_path(
            &meta,
            Some(std::path::Path::new("/tmp/short-undecided.gif"))
        ));
    }

    #[test]
    fn duration_fallback_converts_long_undecided_gif_candidates() {
        let mut meta = make_meta(0.0, 900, 900, 0.0, 60, 2_000_000);
        meta.source_extension = Some("gif".to_string());
        meta.file_name = Some("long-undecided.gif".to_string());
        meta.palette_size = Some(256);
        meta.app_extensions = Some(Vec::new());
        meta.frame_payload_variation = Some(0.8);
        meta.frame_delay_variation = Some(0.5);
        meta.loop_count = Some(2);

        assert!(!should_keep_as_gif_with_path(
            &meta,
            Some(std::path::Path::new("/tmp/long-undecided.gif"))
        ));
    }

        #[test]
        fn env_override_force_keep_and_convert() {
            // Preserve previous env state
            let prev_keep = std::env::var_os("MFB_GIF_FORCE_KEEP");
            let prev_convert = std::env::var_os("MFB_GIF_FORCE_CONVERT");

            std::env::set_var("MFB_GIF_FORCE_KEEP", "force_keep_token");
            std::env::set_var("MFB_GIF_FORCE_CONVERT", "force_convert_token");

            let mut meta = make_meta(1.0, 100, 100, 10.0, 10, 1000);
            meta.file_name = Some("force_keep_token_example.gif".to_string());
            assert!(meta_matches_env_list(&meta, "MFB_GIF_FORCE_KEEP"));
            assert_eq!(check_meta_override(&meta), Some(VetoVerdict::KeepGif));

            // convert override should not shadow keep if patterns differ
            let mut meta2 = make_meta(1.0, 100, 100, 10.0, 10, 1000);
            meta2.file_name = Some("force_convert_token_example.gif".to_string());
            assert!(meta_matches_env_list(&meta2, "MFB_GIF_FORCE_CONVERT"));
            assert_eq!(check_meta_override(&meta2), Some(VetoVerdict::ConvertVideo));

            // restore
            if let Some(v) = prev_keep { std::env::set_var("MFB_GIF_FORCE_KEEP", v); } else { std::env::remove_var("MFB_GIF_FORCE_KEEP"); }
            if let Some(v) = prev_convert { std::env::set_var("MFB_GIF_FORCE_CONVERT", v); } else { std::env::remove_var("MFB_GIF_FORCE_CONVERT"); }
        }

        #[test]
        fn env_skip_convert_ceiling_whitelist() {
            let prev = std::env::var_os("MFB_GIF_SKIP_CONVERT_CEILING");
            std::env::set_var("MFB_GIF_SKIP_CONVERT_CEILING", "whitelist_token");

            let mut meta = make_meta(3.0, 1024, 768, 24.0, 72, ABSOLUTE_CONVERT_OVER_BYTES + 1);
            meta.file_name = Some("whitelist_token_large.gif".to_string());

            assert!(meta_matches_skip_convert_ceiling(&meta));

            if let Some(v) = prev { std::env::set_var("MFB_GIF_SKIP_CONVERT_CEILING", v); } else { std::env::remove_var("MFB_GIF_SKIP_CONVERT_CEILING"); }
        }
}
