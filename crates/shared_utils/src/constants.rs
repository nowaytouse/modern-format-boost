//! Global Constants for `modern_format_boost`
//!
//! This module centralizes core magic numbers, business rules, and
//! environment variable toggles to ensure consistency across the workspace.

// --- Size & Storage Defaults ---
/// Default size tolerance (1MB) = 1,048,576 bytes
pub const DEFAULT_SIZE_TOLERANCE_BYTES: u64 = 1_048_576;
/// Default size tolerance percentage (1%)
pub const DEFAULT_SIZE_TOLERANCE_RATIO: f64 = 0.01;
/// Minimum output size for images to be considered valid for deletion of original.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE: u64 = 1024;
/// Minimum output size for videos to be considered valid for deletion of original.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO: u64 = 4096;
// --- Unified Video Duration Thresholds ---
/// Animation and short clip threshold (30s).
pub const ANIMATION_CLIP_THRESHOLD_SECS: f32 = 30.0;
/// Maximum duration for CRF 0.00 lossless-first probing (Meme vs High Value).
pub const MEME_LOSSLESS_DURATION_LIMIT: f32 = 120.0;
pub const HIGH_VALUE_LOSSLESS_DURATION_LIMIT: f32 = 30.0;
/// Video length categories
pub const LONG_VIDEO_THRESHOLD_SECS: f32 = 300.0;
pub const VERY_LONG_VIDEO_THRESHOLD_SECS: f32 = 600.0;
pub const HEAVY_VIDEO_THRESHOLD_SECS: f32 = 1200.0;
pub const VMAF_SKIP_THRESHOLD_SECS: f32 = 1800.0;
pub const VMAF_SKIP_THRESHOLD_ULTIMATE_SECS: f32 = 3600.0;
/// When MS-SSIM / VMAF-style metrics switch from a single full pass to three-segment sampling.
/// Same band as [`crate::gpu_accel::GPU_SAMPLE_DURATION`] (60s).
pub const MS_SSIM_THREE_SEGMENT_MIN_DURATION_SECS: f64 = 60.0;
/// Animated image CPU CRF search: above this duration, exploration encodes use three-segment
/// timeline sampling. Uses [`ANIMATION_CLIP_THRESHOLD_SECS`] (short vs long animation split).
pub const ANIMATED_IMAGE_EXPLORATION_SAMPLING_MIN_DURATION_SECS: f32 =
    ANIMATION_CLIP_THRESHOLD_SECS;
/// Minimum duration (seconds) for converting animated images to HEVC video.
pub const ANIMATED_MIN_DURATION_FOR_VIDEO_SECS: f32 = 4.5;
/// Fraction of total duration per segment (start / mid / end) for animated exploration sampling.
pub const ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION: f64 = 0.15;
/// Ultimate mode: wider segments.
pub const ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE: f64 = 0.25;
// --- Loop Intent System (Tree & KNN) ---
/// Platform markers that indicate a strong likelihood of being a GIF/sticker.
pub const LOOP_PLATFORM_MARKERS: &[&str] =
    &["GIPHY", "TENOR", "STICKER", "TELEGRAM", "TIKTOK", "DISCORD"];
// 1. Dynamic Multipliers (Relative to KNN P90 Baseline)
/// Multiplier for the "Safe Zone" beyond the P90 baseline.
pub const LOOP_KNN_P90_SAFE_ZONE_MULTIPLIER: f64 = 7.5;
/// Multiplier for the "Video Bias" threshold beyond the P90 baseline.
pub const LOOP_KNN_P90_BIAS_THRESHOLD_MULTIPLIER: f64 = 1.5;
// 2. Physical "Bottom-line" Thresholds
/// Default baseline for "Sticker Safe Zone" (seconds) if KNN is unavailable.
pub const DEFAULT_LOOP_BASELINE_DURATION_SECS: f64 = 2.0;
/// Max dimension (w or h) typically used for stickers/emojis.
pub const STICKER_MAX_DIMENSION: u32 = 512;
/// "Bottom-line" size control: assets below this size are likely stickers.
pub const STICKER_MAX_SIZE_BYTES: u64 = 1_572_864; // 1.5 MB
/// Maximum duration (seconds) for the dimension-agnostic micro-clip GIF interception.
///
/// Silent videos at or below this duration are treated as animated images regardless
/// of resolution or file size — screen captures, UI demos, and motion graphics
/// typically fall into this window.
pub const MICRO_CLIP_CEILING_SECS: f64 = DURATION_TIER_ULTRA_SHORT_LIMIT;
// --- Tiered Duration Classification (Loop Intent) ---
pub const DURATION_TIER_ULTRA_SHORT_LIMIT: f64 = 2.0;
pub const DURATION_TIER_SHORT_LIMIT: f64 = 5.0;
pub const DURATION_TIER_MEDIUM_LONG_LIMIT: f64 = 8.0;
pub const DURATION_TIER_LONG_LIMIT: f64 = 15.0;
pub const DURATION_TIER_VERY_LONG_LIMIT: f64 = 18.0;
pub const LOG_ODDS_BIAS_ULTRA_SHORT: f64 = 1.5;
pub const LOG_ODDS_BIAS_SHORT: f64 = 0.5;
pub const LOG_ODDS_BIAS_MEDIUM_LONG: f64 = -0.25;
pub const LOG_ODDS_BIAS_LONG: f64 = -1.0;
pub const LOG_ODDS_BIAS_VERY_LONG: f64 = -2.0;
pub const LOG_ODDS_BIAS_DEFINITIVELY_LONG: f64 = -3.0;
// --- Extreme Duration Hard-Veto Boundaries ---
//
// These are the ONLY two conditions where duration alone has absolute veto power.
// All other thresholds (Short, MediumLong, etc.) only inject log-odds bias.
// Architecture rule: NO signal outside of these two zones can override the verdict
// by itself — it must still win through log-odds accumulation.
/// Assets at or below this duration (silent) are definitively animated images.
///
/// 6.0s — empirically covers virtually all real-world stickers, reactions, and memes
/// without misclassifying intentional short video clips.
pub const EXTREME_SHORT_ABSOLUTE_LIMIT_SECS: f64 = 6.0;
/// Assets at or above this duration are definitively video, regardless of any
/// metadata signal (`loop_count`, transparency, platform markers, etc.).
///
/// 15.0s — the practical upper bound for any real-world looping animated image.
pub const EXTREME_LONG_ABSOLUTE_LIMIT_SECS: f64 = 15.0;
// --- Proximity Ramp (Anti-Cliff Defense) ---
// The hard veto boundaries create a potential behavioral cliff:
//   5.9s → Hard Veto → LoopStrong (certain)
//   6.1s → Tier bias only → much weaker prior
// The proximity ramp smooths this discontinuity by injecting a linearly-decaying
// additional bias for assets just outside the veto zone:
//   At the veto edge (6.0s + ε): full MAX_BIAS applied
//   At the buffer boundary (8.0s): zero additional bias (only tier bias remains)
// Result: 5.9s and 6.1s have nearly identical effective priors.
/// Width (in seconds) of the anti-cliff proximity ramp above the short veto.
/// Covers 6.0–8.0s. Beyond this, only the standard tier bias applies.
pub const EXTREME_SHORT_PROXIMITY_BUFFER_SECS: f64 = 2.0;
/// Maximum additional log-odds bonus at the veto edge (decays linearly to 0 at
/// `EXTREME_SHORT_ABSOLUTE_LIMIT_SECS + EXTREME_SHORT_PROXIMITY_BUFFER_SECS`).
pub const EXTREME_SHORT_PROXIMITY_MAX_BIAS: f64 = 2.5;
/// Width (in seconds) of the anti-cliff proximity ramp below the long veto.
/// Covers 13.0–15.0s. Below this, only the standard tier bias applies.
pub const EXTREME_LONG_PROXIMITY_BUFFER_SECS: f64 = 2.0;
/// Maximum additional log-odds penalty at the veto edge (decays linearly to 0 at
/// `EXTREME_LONG_ABSOLUTE_LIMIT_SECS - EXTREME_LONG_PROXIMITY_BUFFER_SECS`).
pub const EXTREME_LONG_PROXIMITY_MAX_BIAS: f64 = 2.5;
/// Upper bound on `width * height` for **GIF** assets.
///
/// Refers to [`crate::loop_intent::evaluate_loop_tree`]: a silent, sticker-class canvas is treated as
/// a strong loop/sticker prior (not a `vid` strategy bypass). Larger canvases stay in Layer 4 / KNN.
pub const STICKER_TIER_NATIVE_GIF_MAX_PIXELS: u64 = 200_000;
// 3. Physical Intensity & Bitrate Analysis
/// Threshold for "Physical Intensity" (Pixels per second normalized).
pub const PHYSICAL_INTENSITY_PASS_STRENGTH: f64 = 1.5;
/// WebP compression ratio below which an asset is considered "High Quality Master".
pub const MODERN_FORMAT_HIGH_BITRATE_RATIO: f64 = 8.0;
/// FPS below which an animation is considered "PPT-like" slow-playback.
pub const MODERN_FORMAT_PPT_FPS_THRESHOLD: f64 = 5.0;
// 4. Modern Format Compression Ratio Thresholds (Layer 4-A)
/// High compression ratio (15.0) indicating simple graphics / memes.
pub const MODERN_FORMAT_LOW_BITRATE_RATIO: f64 = 15.0;
/// Low compression ratio (5.0) indicating extremely high quality / noise.
pub const MODERN_FORMAT_ULTRA_HIGH_BITRATE_RATIO: f64 = 5.0;
// 4. Fallback & Force Rules
/// Hidden Layer 1-C developer hard-pass threshold (seconds).
/// The core tree also uses this as the upper bound for its short-asset soft prior.
pub const HARD_PASS_SHORT_GIF_THRESHOLD_SECS: f64 = 10.0;
/// Hidden long-silent/video-bias threshold (seconds).
/// The core tree also uses this as the lower bound for its long-silent soft penalty.
pub const MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS: f64 = 15.0;
// 5. Environment Variable Names
/// Toggle for modern format conversion bias ("1" = on, "0" = off).
pub const ENV_MODERN_FORMAT_CONVERT_BIAS: &str = "MODERN_FORMAT_CONVERT_BIAS";
/// Hidden developer toggle for Layer 1-C short-asset hard-pass ("1" = enable, default off).
pub const ENV_FORCE_SHORT_GIFS: &str = "MODERN_FORMAT_FORCE_SHORT_GIFS";
/// Hidden developer toggle for Layer 1-D long-silent interceptor ("1" = enable, default off).
pub const ENV_INTERCEPT_LONG_SILENT: &str = "MODERN_FORMAT_INTERCEPT_LONG_SILENT";
/// Override for the sticker duration safe-limit (seconds).
pub const ENV_STICKER_LIMIT_SECS: &str = "MODERN_FORMAT_STICKER_LIMIT_SECS";
/// Bypass for the entire database-driven feedback loop (Dynamic weights, KNN, Logging).
pub const ENV_DISABLE_DB_FEEDBACK: &str = "MODERN_FORMAT_DISABLE_DB_FEEDBACK";
/// Independent kill-switch for the static image quality DB (does not affect GIF/Video KNN).
pub const ENV_DISABLE_IMAGE_QUALITY_DB: &str = "MODERN_FORMAT_DISABLE_IMAGE_QUALITY_DB";
/// Developer override to force KNN database lookup for static quality testing.
pub const ENV_FORCE_QUALITY_KNN: &str = "MODERN_FORMAT_FORCE_QUALITY_KNN";
// --- Database Maturity Thresholds ---
// KNN results are unreliable when training data is too sparse or non-diverse.
// These thresholds gate both the GIF/Video KNN and the static image quality KNN.
/// Minimum total labeled samples required for GIF/Video KNN to engage.
/// The V7 seed database provides 30 highly-curated boundary samples.
pub const MIN_GIF_SAMPLES_TOTAL: i64 = 30;
/// Minimum samples per class (high/video) for GIF/Video KNN.
/// The V7 seed database provides exactly 10 `LoopWeak` and 20 `LoopStrong`.
pub const MIN_GIF_SAMPLES_PER_CLASS: i64 = 10;
/// Minimum total labeled samples required for static image KNN to engage.
pub const MIN_QUALITY_SAMPLES_TOTAL: i64 = 30;
/// Minimum samples per class (high/low) for static image KNN.
pub const MIN_QUALITY_SAMPLES_PER_CLASS: i64 = 10;
// --- Formats & Extensions ---
/// Modern animated image/container extensions.
pub const MODERN_ANIMATED_EXTENSIONS: &[&str] = &["webp", "avif", "apng", "heic", "heif", "jxl"];
// --- FFmpeg & Encoder Metadata ---
// 1. Tags
/// Apple Compatibility Tag (Common for HEVC)
pub const FFMPEG_TAG_HVC1: &str = "hvc1";
/// Standard HEVC Tag (Broad compatibility)
pub const FFMPEG_TAG_HEV1: &str = "hev1";
/// Standard AV1 Tag
pub const FFMPEG_TAG_AV01: &str = "av01";
// 2. Presets (x26x / SVT-AV1)
pub const FFMPEG_PRESET_ULTRAFAST: &str = "ultrafast";
pub const FFMPEG_PRESET_MEDIUM: &str = "medium";
pub const FFMPEG_PRESET_SLOW: &str = "slow";
pub const FFMPEG_PRESET_SLOWER: &str = "slower";
pub const FFMPEG_PRESET_VERYSLOW: &str = "veryslow";
// 3. Encoder Names
pub const FFMPEG_ENCODER_X264: &str = "libx264";
pub const FFMPEG_ENCODER_X265: &str = "libx265";
pub const FFMPEG_ENCODER_SVTAV1: &str = "libsvtav1";
/// Above this source size, enable the low-memory x265 profile even when the codec is unknown.
pub const X265_LOW_MEMORY_SOURCE_SIZE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Low-memory x265 profile: serialize frame encoding to cap peak RAM.
pub const X265_LOW_MEMORY_FRAME_THREADS: usize = 1;
/// Low-memory x265 profile: keep lookahead worker fan-out minimal.
pub const X265_LOW_MEMORY_LOOKAHEAD_THREADS: usize = 1;
/// Low-memory x265 profile: avoid per-slice lookahead fan-out on huge masters.
pub const X265_LOW_MEMORY_LOOKAHEAD_SLICES: usize = 1;
/// Low-memory x265 profile: cap worker pools aggressively to keep RAM spikes in check.
pub const X265_LOW_MEMORY_MAX_POOLS: usize = 2;
/// Current HEVC preset policy (`medium`/`slow`/`slower`) must tolerate x265's `slower` preset.
///
/// That preset can use up to 8 consecutive B-frames, so `rc-lookahead` must stay strictly
/// above that count or x265 rejects the encode at startup.
pub const X265_ALLOWED_HEVC_MAX_CONSECUTIVE_BFRAMES: usize = 8;
/// Low-memory x265 profile: shorten the lookahead queue to reduce buffered frames, while
/// still satisfying x265's strict `rc-lookahead > bframes` requirement.
pub const X265_LOW_MEMORY_RC_LOOKAHEAD: usize = X265_ALLOWED_HEVC_MAX_CONSECUTIVE_BFRAMES + 1;
/// Moderate-memory x265 profile: cap worker pools but still leave room to scale on healthy systems.
pub const X265_MODERATE_MEMORY_MAX_POOLS: usize = 6;
/// Moderate-memory x265 profile: allow limited parallelism for systems with adequate RAM.
pub const X265_MODERATE_MEMORY_FRAME_THREADS: usize = 3;
/// Moderate-memory x265 profile: allow limited lookahead parallelism.
pub const X265_MODERATE_MEMORY_LOOKAHEAD_THREADS: usize = 3;
/// Moderate-memory x265 profile: moderate lookahead slice fan-out.
pub const X265_MODERATE_MEMORY_LOOKAHEAD_SLICES: usize = 3;
/// Moderate-memory x265 profile: moderate lookahead queue depth.
pub const X265_MODERATE_MEMORY_RC_LOOKAHEAD: usize = 20;
/// RAM threshold (MB) above which the Default (uncapped) profile is used.
pub const X265_DEFAULT_RAM_THRESHOLD_MB: u64 = 12 * 1024;
/// Relaxed RAM threshold (MB) that still permits the default profile when free-memory ratio is healthy.
pub const X265_RELAXED_DEFAULT_RAM_THRESHOLD_MB: u64 = 8 * 1024;
/// Minimum free-memory ratio required to stay on the default x265 profile below the hard 16 GB cutoff.
pub const X265_DEFAULT_RAM_RATIO_THRESHOLD: f64 = 0.25;
/// Minimum RAM (MB) required to avoid the aggressive low-memory profile.
pub const X265_MODERATE_RAM_THRESHOLD_MB: u64 = 4 * 1024;
/// Minimum free-memory ratio required to stay above the aggressive low-memory profile.
pub const X265_MODERATE_RAM_RATIO_THRESHOLD: f64 = 0.15;
// 4. Default Search Parameters
/// Starting CRF for quality-matched exploration.
pub const DEFAULT_CRF_EXPLORE_START: f32 = 18.0;
/// CRF adjustment step for iterative search.
pub const CRF_SEARCH_STEP: f32 = 1.0;
// --- Loop Intent Decision Tree Thresholds (Log-Odds) ---
pub const TREE_DECISION_LOG_ODDS_THRESHOLD: f64 = 0.95;
pub const TREE_STRUCTURAL_CHECKPOINT_LOG_ODDS_THRESHOLD: f64 = 0.55;
pub const TREE_CONTENT_CHECKPOINT_LOG_ODDS_THRESHOLD: f64 = 0.78;
pub const TREE_Z_SCORE_CAP: f64 = 2.5;
pub const SHORT_FAST_POSITIVE_LOG_ODDS: f64 = 0.72;
pub const LONG_SLOW_NEGATIVE_LOG_ODDS: f64 = 0.82;
pub const SCENE_CUT_NEGATIVE_LOG_ODDS: f64 = 1.35;
pub const COMPACT_SILENT_POSITIVE_LOG_ODDS: f64 = 0.62;
pub const LARGE_MEDIA_NEGATIVE_LOG_ODDS: f64 = 0.55;
pub const PLATFORM_MARKER_POSITIVE_LOG_ODDS: f64 = 0.52;
pub const PLAY_ONCE_NEGATIVE_LOG_ODDS: f64 = 0.92;
pub const TRANSPARENCY_POSITIVE_LOG_ODDS: f64 = 0.34;
pub const LOCALIZED_MOTION_POSITIVE_LOG_ODDS: f64 = 0.16;
pub const DIRECTORY_CONTEXT_POSITIVE_LOG_ODDS: f64 = 0.12;
pub const FILENAME_CONTEXT_POSITIVE_LOG_ODDS: f64 = 0.10;
pub const MODERN_MASTER_NEGATIVE_LOG_ODDS: f64 = 0.35;
pub const SHORT_CLIP_PRIOR_LOG_ODDS: f64 = 0.42;
pub const EXTENDED_SHORT_ASSET_PRIOR_LOG_ODDS: f64 = 0.20;
pub const LONG_SILENT_PRIOR_NEGATIVE_LOG_ODDS: f64 = 0.26;
/// Empirical: 16:9 aspect ratio target.
pub const ASPECT_RATIO_WIDESCREEN: f64 = 16.0 / 9.0;
/// Sampling dimension for WebP ratio estimation.
pub const WEBP_RATIO_SAMPLE_MAX_DIM: u32 = 256;
/// Empirical: tolerance for near-16:9 check.
pub const ASPECT_RATIO_TOLERANCE_NEAR: f64 = 0.05;
/// Layer 4 soft-priors and weights.
pub const LOOP_COUNT_ZERO_BONUS_MAX: f64 = 0.18;
pub const LOOP_COUNT_ZERO_BONUS_MIN: f64 = 0.06;
pub const LOOP_COUNT_ZERO_BONUS_DECAY_MAX: f64 = 0.12;
pub const LOOP_COUNT_ZERO_BONUS_DECAY_MEDIUM: f64 = 0.3;
pub const LOOP_COUNT_ZERO_BONUS_DECAY_LONG: f64 = 0.8;
pub const COMPACTNESS_SIGNAL_SIZE_WEIGHT: f64 = 0.70;
pub const COMPACTNESS_SIGNAL_PIXELS_WEIGHT: f64 = 0.45;
pub const COMPACTNESS_SIGNAL_BIAS: f64 = 0.20;
pub const COMPACTNESS_SIGNAL_MAX: f64 = 1.6;
pub const LARGE_MEDIA_SIGNAL_SIZE_WEIGHT: f64 = 0.75;
pub const LARGE_MEDIA_SIGNAL_PIXELS_WEIGHT: f64 = 0.35;
pub const LARGE_MEDIA_SIGNAL_MAX: f64 = 1.8;
pub const LARGE_MEDIA_AUDIO_MULTIPLIER: f64 = 0.65;
pub const SHORT_CLIP_FORMAT_BONUS_IMAGE: f64 = 0.10;
pub const SHORT_CLIP_FORMAT_BONUS_VIDEO: f64 = 0.04;
pub const SHORT_CLIP_CADENCE_BONUS: f64 = 0.06;
pub const SHORT_CLIP_MIN_BIAS: f64 = 0.18;
pub const SHORT_CLIP_HEADROOM_MAX: f64 = 0.26;
pub const EXTENDED_SHORT_ASSET_MIN_BIAS: f64 = 0.10;
pub const EXTENDED_SHORT_ASSET_HEADROOM_MAX: f64 = 0.10;
pub const EXTENDED_SHORT_ASSET_SQUARE_BONUS: f64 = 0.04;
pub const EXTENDED_SHORT_ASSET_IMAGE_BONUS: f64 = 0.05;
pub const EXTENDED_SHORT_ASSET_COMPACT_BONUS: f64 = 0.05;
pub const FEATURE_WEIGHT_DELAY_VAR: f64 = 0.18;
// Reduced from 0.34: loop_closure_score measures pkt_size autocorrelation (codec behavior),
// not visual loop closure. Abrupt memes have intentionally different first/last frames.
// Retained at low weight as a secondary correlation signal for short content only.
pub const FEATURE_WEIGHT_LOOP_CLOSURE: f64 = 0.12;
pub const FEATURE_WEIGHT_MOTION_PERIODICITY: f64 = 0.22;
pub const FEATURE_WEIGHT_LOOP_FREQUENCY: f64 = 0.16;
pub const FEATURE_WEIGHT_SPARSE_CADENCE: f64 = 0.12;
// Reduced from 0.10: temporal_jitter unfairly penalizes abrupt memes with intentional
// frame delay variation (dramatic pause before punchline).
pub const FEATURE_WEIGHT_TEMPORAL_JITTER: f64 = 0.06;
pub const FEATURE_WEIGHT_WEBP_RATIO: f64 = 0.16;
pub const FEATURE_WEIGHT_MOTION_GINI: f64 = 0.14;
pub const FEATURE_WEIGHT_PALETTE_DEPTH: f64 = 0.12;
pub const FEATURE_WEIGHT_TEMPORAL_FLATNESS: f64 = 0.10;
// New zero-cost signals from existing LoopMeta data:
// I-frame ratio: GIF→MP4 transcodes produce all-I-frame streams; real video has GOP structure.
pub const FEATURE_WEIGHT_IFRAME_RATIO: f64 = 0.30;
// Bytes per frame: GIF-class content has much lower bytes_per_frame than real video.
pub const FEATURE_WEIGHT_BYTES_PER_FRAME: f64 = 0.18;
pub const FRAME_COUNT_SHORT_BONUS: f64 = 0.05;
pub const FRAME_COUNT_LONG_PENALTY: f64 = 0.10;
pub const SQUARE_ASPECT_BONUS: f64 = 0.08;
pub const WIDESCREEN_ASPECT_PENALTY: f64 = 0.10;
// 9:16 portrait (TikTok/Reels/Shorts standard) is a strong video signal, symmetric with 16:9.
pub const PORTRAIT_ASPECT_PENALTY: f64 = 0.10;
pub const FPS_ANOMALY_BONUS: f64 = 0.04;
pub const LONG_SILENT_PENALTY_BASE: f64 = 0.22;
pub const LONG_SILENT_PENALTY_OVERFLOW_MAX: f64 = 0.18;
pub const LONG_SILENT_PENALTY_VIDEO_ADD: f64 = 0.18;
pub const LONG_SILENT_PENALTY_IMAGE_ADD: f64 = 0.08;
pub const LONG_SILENT_TRANSPARENCY_RELIEF: f64 = 0.06;
pub const LONG_SILENT_MIN_PENALTY: f64 = 0.08;
pub const IMAGE_PRIOR_BONUS: f64 = 0.04;
pub const VIDEO_PRIOR_PENALTY: f64 = 0.04;
/// Layer 6 KNN & Fusion thresholds.
pub const LAYER6_CONFIDENCE_HIGH: f64 = 0.75;
pub const LAYER6_FINAL_SCORE_HIGH: f64 = 0.60;
pub const LAYER6_KEEP_PROB_MIN: f64 = 0.70;
pub const LAYER6_FUSION_SCORE_UNCERTAIN_LOW: f64 = 0.40;
pub const LAYER6_FUSION_SCORE_UNCERTAIN_HIGH: f64 = 0.60;
pub const LAYER6_DIRECTIONAL_KEEP_MIN: f64 = 0.58;
pub const LAYER6_DIRECTIONAL_WEAK_MAX: f64 = 0.42;
pub const LAYER6_DIRECTIONAL_MARGIN_MIN: f64 = 0.15;
pub const LETTERBOXING_NUDGE: f64 = 0.05;
pub const HIGH_TEXT_DENSITY_NUDGE: f64 = 0.08;
pub const AUXILIARY_NUDGE_CAP: f64 = 0.15;
pub const LOSSLESS_DURATION_LIMIT_LOW_PROB: f64 = 0.3;
pub const LOSSLESS_DURATION_LIMIT_HIGH_PROB: f64 = 0.7;
pub const LAYER6_HIGH_SCORE_THRESHOLD: f64 = 0.70;
pub const LAYER6_RELAXED_CONFIDENCE_THRESHOLD: f64 = 0.68;
pub const LAYER6_MIN_KNN_WEIGHT: f64 = 0.25;
pub const LAYER6_MAX_KNN_WEIGHT: f64 = 0.60;
pub const LAYER6_KNN_COLD_START_NEIGHBORS: usize = 6;
pub const LAYER6_KNN_FULL_WEIGHT_NEIGHBORS: usize = 16;
// Weights recalibrated for logit-space inputs (see logistic_regression_fusion).
// Old weights (3.8, 2.5) were for raw probability inputs [0,1].
// With logit transform, inputs span [-4.6, +4.6], so weights are ~6x smaller.
// The KNN-to-tree ratio (≈60:40) is preserved.
pub const LAYER6_LR_W_KNN: f64 = 0.65;
pub const LAYER6_LR_W_TREE: f64 = 0.40;
pub const LAYER6_LR_W_DENSITY: f64 = 0.18;
// Bias recalibrated: at knn=0.5, tree=0.5 the logit inputs are both 0,
// so the score = 0 + 0 + density*0.18 + bias. With bias=0 and no density,
// the fusion output is exactly 0.5, which is the correct neutral point.
pub const LAYER6_LR_BIAS: f64 = 0.0;
// --- Image Quality & Complexity Thresholds ---
/// Empirical gradient threshold (25.0) for edge detection in luminance space.
pub const IMAGE_EDGE_DENSITY_THRESHOLD: f64 = 25.0;
/// Complexity weights must sum to 1.0. Rationale:
/// - Texture (0.35) and Edges (0.25) are primary visual markers.
pub const IMAGE_COMPLEXITY_WEIGHT_NOISE: f64 = 0.15;
pub const IMAGE_COMPLEXITY_WEIGHT_TEXTURE: f64 = 0.35;
pub const IMAGE_COMPLEXITY_WEIGHT_EDGE: f64 = 0.25;
pub const IMAGE_COMPLEXITY_WEIGHT_COLOR: f64 = 0.25;
pub const IMAGE_ALPHA_SAMPLING_STEP: usize = 16;
pub const IMAGE_CONTRAST_NORMALIZATION: f64 = 80.0;
// --- Video Quality & Compression Boundaries ---
/// Empirical: base confidence for video quality analysis.
pub const VIDEO_CONFIDENCE_BASE: f64 = 0.7;
pub const VIDEO_CONFIDENCE_BITRATE_BONUS: f64 = 0.1;
pub const VIDEO_CONFIDENCE_GOP_BONUS: f64 = 0.05;
pub const VIDEO_CONFIDENCE_DURATION_BONUS: f64 = 0.05;
pub const VIDEO_CONFIDENCE_FRAMES_BONUS: f64 = 0.05;
pub const VIDEO_CONFIDENCE_DURATION_THRESHOLD: f64 = 10.0;
pub const VIDEO_CONFIDENCE_FRAMES_THRESHOLD: u64 = 100;
pub const SSIM_DISPLAY_PRECISION: u32 = 4;
pub const DEFAULT_MIN_SSIM: f64 = 0.95;
pub const HIGH_QUALITY_MIN_SSIM: f64 = 0.98;
pub const ACCEPTABLE_MIN_SSIM: f64 = 0.90;
pub const MIN_ACCEPTABLE_SSIM: f64 = 0.85;
pub const PSNR_DISPLAY_PRECISION: u32 = 2;
pub const DEFAULT_MIN_PSNR: f64 = 35.0;
pub const HIGH_QUALITY_MIN_PSNR: f64 = 40.0;
pub const DEFAULT_MIN_MS_SSIM: f64 = 0.90;
pub const HIGH_QUALITY_MIN_MS_SSIM: f64 = 0.95;
pub const ACCEPTABLE_MIN_MS_SSIM: f64 = 0.85;
/// Empirical: codec efficiency factors for CRF estimation.
pub const MODERN_EFFICIENT_CODEC_FACTOR: f64 = 0.5;
pub const INTERMEDIATE_CODEC_FACTOR: f64 = 0.7;
pub const INEFFICIENT_CODEC_FACTOR: f64 = 2.0;
/// Empirical: BPP to CRF lookup table for initial estimation.
/// format: (`bpp_threshold`, `crf_value`)
pub const DENSITY_TO_CRF_LUT: &[(f64, u8)] = &[
    (5.0, 14),
    (1.0, 18),
    (0.5, 22),
    (0.3, 25),
    (0.15, 28),
    (0.08, 32),
];
/// CRF threshold for "Visually Lossless" classification.
pub const CRF_THRESHOLD_VISUALLY_LOSSLESS: f32 = 15.0;
/// CRF threshold for "High Quality" classification.
pub const CRF_THRESHOLD_HIGH_QUALITY: f32 = 23.0;
/// CRF threshold for "Standard Quality" classification.
pub const CRF_THRESHOLD_STANDARD: f32 = 30.0;
/// Bits Per Pixel (BPP) threshold for "Visually Lossless" classification.
pub const BPP_THRESHOLD_VISUALLY_LOSSLESS: f64 = 2.0;
/// Bits Per Pixel (BPP) threshold for "High Quality" classification.
pub const BPP_THRESHOLD_HIGH_QUALITY: f64 = 0.5;
/// Bits Per Pixel (BPP) threshold for "Standard Quality" classification.
pub const BPP_THRESHOLD_STANDARD: f64 = 0.1;
/// Resolution threshold for 4K UHD height.
pub const HEIGHT_UHD_4K: u32 = 2160;
/// Resolution threshold for 4K UHD width.
pub const WIDTH_UHD_4K: u32 = 3840;
// --- Image Detection & Content Analysis ---
/// Empirical: base confidence for image analysis.
pub const IMAGE_CONFIDENCE_BASE: f64 = 0.7;
pub const IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD: u64 = 1_000_000;
pub const IMAGE_CONFIDENCE_PIXELS_SMALL_THRESHOLD: u64 = 100_000;
pub const IMAGE_CONFIDENCE_PIXELS_LARGE_BONUS: f64 = 0.1;
pub const IMAGE_CONFIDENCE_SIZE_MIN: u64 = 10_000;
pub const IMAGE_CONFIDENCE_SIZE_MAX: u64 = 100_000_000;
pub const IMAGE_CONFIDENCE_INCREMENT: f64 = 0.05;
/// Rec. 601 Luma coefficients.
pub const LUMA_COEFF_R: i32 = 299;
pub const LUMA_COEFF_G: i32 = 587;
pub const LUMA_COEFF_B: i32 = 114;
pub const LUMA_DIVISOR: i32 = 1000;
/// Detection and normalization parameters.
pub const IMAGE_EDGE_THRESHOLD: f64 = 25.0;
pub const IMAGE_EDGE_DENSITY_MULTIPLIER: f64 = 3.0;
pub const IMAGE_TEXTURE_VAR_NORMALIZATION: f64 = 80.0;
pub const IMAGE_NOISE_NORMALIZATION: f64 = 30.0;
pub const IMAGE_LAPLACIAN_CENTER: i32 = 4;
pub const IMAGE_SHARPNESS_NORMALIZATION: f64 = 100.0;
// --- Image Detection & Safety Limits ---
/// Maximum memory allocation for decoding large images (2GB).
pub const MAX_IMAGE_DECODE_ALLOC_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Maximum file size for proactive image analysis (512MB).
pub const MAX_IMAGE_ANALYSIS_FILE_SIZE: u64 = 512 * 1024 * 1024;
/// PNG Quantization Heuristic: Lower score boundary for "Lossless" gray zone.
pub const PNG_QUANT_THRESHOLD_LOW: f64 = 0.40;
/// PNG Quantization Heuristic: Upper score boundary for "Lossy" classification.
pub const PNG_QUANT_THRESHOLD_HIGH: f64 = 0.58;
// --- Video Explorer Iteration & Search Limits ---
pub const ABSOLUTE_MIN_CRF: f32 = 0.0;
pub const ABSOLUTE_MAX_CRF: f32 = 51.0;
pub const SEARCH_STEP_COARSE: f32 = 2.0;
pub const SEARCH_STEP_FINE: f32 = 0.5;
pub const SEARCH_STEP_ULTRA_FINE: f32 = 0.25;
pub const SEARCH_STEP_CPU_FINEST: f32 = 0.1;
pub const STAGE_B1_MAX_ITERATIONS: u32 = 20;
pub const STAGE_B2_MAX_ITERATIONS: u32 = 25;
pub const STAGE_B_BIDIRECTIONAL_MAX_ITERATIONS: u32 = 18;
pub const BINARY_SEARCH_MAX_ITERATIONS: u32 = 12;
pub const GLOBAL_MAX_ITERATIONS: u32 = 500;
/// Files below this size are considered "small" for compression verification (10MB).
pub const SMALL_FILE_THRESHOLD_BYTES: u64 = 10 * 1024 * 1024;
/// Minimum absolute metadata margin allowed (2KB).
pub const METADATA_MARGIN_MIN_BYTES: u64 = 2048;
/// Maximum absolute metadata margin allowed (100KB).
pub const METADATA_MARGIN_MAX_BYTES: u64 = 102_400;
/// Target metadata overhead percentage (0.5%).
pub const METADATA_MARGIN_RATIO: f64 = 0.005;
pub const MOV_OVERHEAD_PERCENT: f64 = 0.005;
pub const MP4_OVERHEAD_PERCENT: f64 = 0.001;
pub const MKV_OVERHEAD_PERCENT: f64 = 0.0005;
pub const DEFAULT_OVERHEAD_PERCENT: f64 = 0.002;
pub const ULTIMATE_REQUIRED_ZERO_GAINS: u32 = 100;
pub const NORMAL_REQUIRED_ZERO_GAINS: u32 = 4;
pub const LONG_VIDEO_REQUIRED_ZERO_GAINS: u32 = 3;
// --- Additional Quality & Duration Boundaries ---
/// FPS below which an animation is considered "PPT-like" slow-playback. (Duplicated from loop intent section for visibility)
pub const PPT_FPS_THRESHOLD: f64 = 5.0;
/// Negligible duration for effectively static images (seconds).
pub const NEGLIGIBLE_DURATION_SECS: f64 = 0.01;
/// HQ HD Resolution: Width (1280).
pub const HQ_HD_WIDTH: u32 = 1280;
/// HQ HD Resolution: Height (720).
pub const HQ_HD_HEIGHT: u32 = 720;
/// Total pixels for HD (1280x720 = 921,600).
pub const HQ_PIX_COUNT_HD: u64 = 921_600;
// --- Extension Constants ---
pub const EXT_MOV: &str = "MOV";
pub const EXT_MP4: &str = "MP4";
pub const EXT_MKV: &str = "mkv";
pub const EXT_WEBP: &str = "webp";
pub const EXT_GIF: &str = "gif";
pub const EXT_JXL: &str = "jxl";
pub const EXT_AVIF: &str = "avif";
pub const EXT_APNG: &str = "apng";
pub const EXT_PNG: &str = "png";
// --- External Tools & Binary Names ---
pub const TOOL_FFMPEG: &str = "ffmpeg";
pub const TOOL_FFPROBE: &str = "ffprobe";
pub const TOOL_CJXL: &str = "cjxl";
pub const TOOL_DJXL: &str = "djxl";
pub const TOOL_JXLINFO: &str = "jxlinfo";
pub const TOOL_WEBPMUX: &str = "webpmux";
pub const TOOL_GIFSKI: &str = "gifski";
pub const TOOL_MAGICK: &str = "magick";
pub const TOOL_IDENTIFY: &str = "identify";
pub const TOOL_SIPS: &str = "sips";
pub const TOOL_EXIFTOOL: &str = "exiftool";
pub const TOOL_DWEBP: &str = "dwebp";
pub const TOOL_X265: &str = "x265";
pub const TOOL_AVIFENC: &str = "avifenc";
pub const TOOL_DOVI: &str = "dovi_tool";
pub const TOOL_HDR10PLUS: &str = "hdr10plus_tool";
// --- SVT-AV1 Defaults ---
/// Default preset for SVT-AV1 (6 = Balanced).
pub const FFMPEG_SVTAV1_DEFAULT_PRESET: &str = "6";
// --- FFmpeg Command Flags & Arguments ---
pub const FFMPEG_ARG_OVERWRITE: &str = "-y";
pub const FFMPEG_ARG_HIDE_BANNER: &str = "-hide_banner";
pub const FFMPEG_ARG_THREADS: &str = "-threads";
pub const FFMPEG_ARG_INPUT: &str = "-i";
pub const FFMPEG_ARG_MAP: &str = "-map";
pub const FFMPEG_ARG_FORMAT: &str = "-f";
pub const FFMPEG_ARG_FRAMES_VIDEO: &str = "-frames:v";
pub const FFMPEG_ARG_CODEC_VIDEO: &str = "-c:v";
pub const FFMPEG_ARG_CRF: &str = "-crf";
pub const FFMPEG_ARG_PRESET: &str = "-preset";
pub const FFMPEG_ARG_PIX_FMT: &str = "-pix_fmt";
pub const FFMPEG_ARG_VSYNC: &str = "-vsync";
pub const FFMPEG_ARG_NO_AUDIO: &str = "-an";
pub const FFMPEG_ARG_FILTER_COMPLEX: &str = "-filter_complex";
pub const FFMPEG_ARG_FILTER_LAVFI: &str = "-lavfi";
pub const FFMPEG_ARG_SELECT_STREAMS: &str = "-select_streams";
pub const FFMPEG_ARG_COUNT_FRAMES: &str = "-count_frames";
pub const FFMPEG_ARG_SHOW_ENTRIES: &str = "-show_entries";
pub const FFMPEG_ARG_OUTPUT_FORMAT: &str = "-of";
pub const FFMPEG_ARG_PLAYS: &str = "-plays";
pub const FFMPEG_ARG_X265_PARAMS: &str = "-x265-params";
pub const FFMPEG_ARG_PROFILE_VIDEO: &str = "-profile:v";
pub const FFMPEG_ARG_TAG_VIDEO: &str = "-tag:v";
pub const FFMPEG_ARG_TUNE: &str = "-tune";
pub const FFMPEG_ARG_RC: &str = "-rc";
pub const FFMPEG_ARG_QUALITY: &str = "-quality";
pub const FFMPEG_ARG_LOG_LEVEL: &str = "-v";
// --- JXL Argument Constants ---
pub const JXL_ARG_DISTANCE: &str = "-d";
pub const JXL_ARG_EFFORT: &str = "-e";
pub const JXL_ARG_THREADS: &str = "-j";
pub const JXL_ARG_LOSSLESS_JPEG: &str = "--lossless_jpeg=1";
pub const JXL_ARG_COLOR_SPACE: &str = "color_space";
pub const JXL_ARG_COMPRESS_BOXES: &str = "--compress_boxes=0";
pub const JXL_ARG_ALLOW_JPEG_RECON: &str = "--allow_jpeg_reconstruction";
pub const JXL_ARG_ICC_PATHNAME: &str = "icc_pathname";
// --- JXL Standardized Parameters ---
/// Quality distance for ultimate mode (Limit Mode)
pub const JXL_ULTIMATE_DISTANCE: f32 = 0.001;
/// Effort level for ultimate mode (Limit Mode)
pub const JXL_ULTIMATE_EFFORT: u8 = 10;
/// Default effort level for standard mode
pub const JXL_DEFAULT_EFFORT: u8 = 7;
/// Runtime JXL policy: default mode always emits `e7`, ultimate mode always emits `e10`.
#[must_use]
pub const fn jxl_effort_for_mode(ultimate: bool) -> u8 {
    if ultimate {
        JXL_ULTIMATE_EFFORT
    } else {
        JXL_DEFAULT_EFFORT
    }
}
/// Runtime JXL policy: only `e7` and `e10` are supported.
#[must_use]
pub const fn is_supported_jxl_effort(effort: u8) -> bool {
    effort == JXL_DEFAULT_EFFORT || effort == JXL_ULTIMATE_EFFORT
}
/// Runtime JXL policy: ultimate mode pins the distance to [`JXL_ULTIMATE_DISTANCE`].
#[must_use]
pub const fn jxl_distance_for_mode(requested_distance: f32, ultimate: bool) -> f32 {
    if ultimate {
        JXL_ULTIMATE_DISTANCE
    } else {
        requested_distance
    }
}
// --- JXL Distance Exploration (Ultimate Explore Mode) ---
/// Smallest allowed distance for extreme JXL exploration.
pub const JXL_EXPLORE_FLOOR: f32 = JXL_ULTIMATE_DISTANCE;
/// Hard ceiling — exploration MUST stay strictly below this value.
pub const JXL_EXPLORE_CEILING: f32 = f32::from_bits(1.0f32.to_bits() - 1);
/// Binary search convergence threshold: stop when hi − lo < this value.
/// Set to floor/10 so that the narrowest bracket still resolves a meaningful distance delta.
pub const JXL_EXPLORE_BINARY_SEARCH_PRECISION: f32 = JXL_EXPLORE_FLOOR / 10.0;
/// Maximum total exploration iterations across both phases.
pub const JXL_EXPLORE_MAX_ITERATIONS: u32 = 50;
// --- ImageMagick Argument Constants ---
pub const MAGICK_ARG_STRIP: &str = "-strip";
pub const MAGICK_ARG_DEPTH: &str = "-depth";
pub const MAGICK_ARG_DEFINE: &str = "-define";
pub const MAGICK_ARG_SET: &str = "-set";
pub const MAGICK_ARG_COLORSPACE: &str = "colorspace";
// --- FFmpeg Value Constants ---
pub const FFMPEG_VAL_VFR: &str = "vfr";
pub const FFMPEG_VAL_ERROR: &str = "error";
pub const FFMPEG_VAL_CSV_CLEAN: &str = "csv=p=0";
pub const FFMPEG_VAL_JSON: &str = "json";
pub const VAL_QUIET: &str = "quiet";
pub const VAL_MEDIUM: &str = "medium";
pub const VAL_MAIN: &str = "main";
pub const VAL_HIGH: &str = "high";
pub const VAL_P4: &str = "p4";
pub const VAL_HQ: &str = "hq";
pub const VAL_VBR: &str = "vbr";
// --- Prototypical Priors & Scoring Fallbacks ---
/// Default probability/affinity prior when feature data is missing (0.5 = Neutral).
pub const DEFAULT_SCORE_PRIOR: f64 = 0.5;
/// Default aspect ratio fallback (1.0 = Square).
pub const DEFAULT_ASPECT_RATIO: f64 = 1.0;
/// Default compression ratio fallback for raw/unweighted samples.
pub const DEFAULT_COMPRESSION_RATIO: f64 = 1.0;
/// Default palette size fallback (256 colors).
pub const DEFAULT_PALETTE_SIZE: f64 = 256.0;
/// Default frame complexity/payload fallback.
pub const DEFAULT_COMPLEXITY_PRIOR: f64 = 0.5;
/// Default quality fallback for JPEG files when markers are unreadable (85 = Standard High).
pub const FALLBACK_QUALITY_JPEG: u8 = 85;
/// Default compression level for PNG files when unknown (6 = Medium).
pub const FALLBACK_COMPRESSION_PNG: u8 = 6;
/// Default CRF fallback for video when BPP-to-CRF LUT fails (35 = Safe Standard).
pub const FALLBACK_CRF_VIDEO: f32 = 35.0;
/// Default quality fallback for AVIF files (85).
pub const FALLBACK_QUALITY_AVIF: u8 = 85;
/// Neutral weight for feature calculations (1.0).
pub const FEATURE_WEIGHT_NEUTRAL: f64 = 1.0;
// --- GPU Search Step & Boundary Constants ---
/// CRF step size for coarse search in ultimate mode (High precision).
pub const GPU_SEARCH_STEP_ULTIMATE: f32 = 0.5;
/// CRF step size for coarse search in normal mode (Standard).
pub const GPU_SEARCH_STEP_NORMAL: f32 = 2.0;
/// Sampling rate multiplier for short videos (<= 1 min).
pub const GPU_SAMPLE_RATE_SHORT: usize = 1;
/// Sampling rate multiplier for standard videos (> 1 min).
pub const GPU_SAMPLE_RATE_STANDARD: usize = 3;
/// Maximum allowable failures during fine-search phase (Ultimate).
pub const MAX_FINE_SEARCH_FAILURES_ULTIMATE: usize = 20;
/// Maximum allowable failures during fine-search phase (Normal).
pub const MAX_FINE_SEARCH_FAILURES_NORMAL: usize = 3;
/// Deceleration multiplier for search convergence (Ultimate).
pub const GPU_SEARCH_DECEL_ULTIMATE: f32 = 1.0;
/// Deceleration multiplier for search convergence (Normal).
pub const GPU_SEARCH_DECEL_NORMAL: f32 = 2.0;
// --- Video Detection & Quality Bonuses ---
/// Bit depth threshold for HDR/Extended Dynamic Range (10-bit).
pub const HDR_BIT_DEPTH_THRESHOLD: u32 = 10;
/// Quality scoring bonus for HDR/10-bit content.
pub const HDR_QUALITY_BONUS: u32 = 5;
// --- Convergence & Minimum Gain Thresholds ---
/// Minimum consecutive gainless iterations before exit (Ultimate).
pub const ULTIMATE_MIN_GAINS: u32 = 15;
/// Minimum consecutive gainless iterations before exit (Normal).
pub const NORMAL_MIN_GAINS: u32 = 3;
/// Default SSIM fallback value when measurement fails (0.0 = Minimum).
pub const DEFAULT_SSIM_PRIOR: f64 = 0.0;
pub const IMAGE_CONFIDENCE_MIN_EDGE_DENSITY: f64 = 0.01;
pub const IMAGE_CONFIDENCE_MAX_EDGE_DENSITY: f64 = 0.90;
pub const IMAGE_CONFIDENCE_MIN_COLOR_DIVERSITY: f64 = 0.01;
pub const IMAGE_CONFIDENCE_MAX_COLOR_DIVERSITY: f64 = 0.99;
pub const IMAGE_SAMPLING_PIXELS_ULTRA_LARGE: u64 = 4_000_000;
pub const IMAGE_SAMPLING_STEP_ULTRA_LARGE: usize = 4;
pub const IMAGE_SAMPLING_STEP_LARGE: usize = 2;
pub const IMAGE_SAMPLING_STEP_NORMAL: usize = 1;
pub const SHARPNESS_SAMPLING_PIXELS_LARGE: u64 = 1_000_000;
pub const SHARPNESS_SAMPLING_STEP_LARGE: usize = 10;
pub const SHARPNESS_SAMPLING_STEP_MEDIUM: usize = 5;
pub const SHARPNESS_SAMPLING_STEP_NORMAL: usize = 1;
pub const CONTRAST_SAMPLING_STEP_LARGE: usize = 20;
pub const CONTRAST_SAMPLING_STEP_MEDIUM: usize = 10;
pub const CONTRAST_SAMPLING_STEP_NORMAL: usize = 1;
pub const MEMORY_PRESSURE_LOW_RATIO: f64 = 0.30;
pub const MEMORY_PRESSURE_LOW_MIN_MB: u64 = 3072;
pub const MEMORY_PRESSURE_NORMAL_RATIO: f64 = 0.15;
pub const MEMORY_PRESSURE_NORMAL_MIN_MB: u64 = 1536;
pub const INTERLACE_DETECTION_MIN_DURATION_SECS: f64 = 4.0;
pub const INTERLACE_DETECTION_MAX_DURATION_SECS: f64 = 18.0;
pub const HDR_TRANSFER_PQ: &str = "smpte2084";
pub const HDR_TRANSFER_HLG: &str = "arib-std-b67";
pub const SEARCH_STEP_GPU_COARSE: f32 = 4.0;
pub const SEARCH_STEP_GPU_MEDIUM: f32 = 1.0;
pub const IMAGE_SIZE_THRESHOLD_LARGE: u64 = 100_000;
pub const IMAGE_SIZE_THRESHOLD_MEDIUM: u64 = 10_000;
pub const IMAGE_SIZE_THRESHOLD_SMALL: u64 = 5_000;
pub const PNG_ALPHA_INDEXED_FACTOR_HIGH: f64 = 1.0;
pub const PNG_ALPHA_INDEXED_FACTOR_MEDIUM: f64 = 0.7;
pub const PNG_ALPHA_INDEXED_FACTOR_LOW: f64 = 0.3;
pub const PNG_ALPHA_INDEXED_FACTOR_MIN: f64 = 0.1;
pub const PNG_PALETTE_FACTOR_NEAR_MAX: f64 = 1.0;
pub const PNG_PALETTE_FACTOR_LARGE: f64 = 0.8;
pub const PNG_PALETTE_FACTOR_MEDIUM: f64 = 0.5;
pub const PNG_PALETTE_FACTOR_SMALL: f64 = 0.3;
pub const PNG_PALETTE_FACTOR_MIN: f64 = 0.1;
pub const PNG_ENTROPY_RATIO_THRESHOLD_HIGH: f64 = 0.6;
pub const PNG_ENTROPY_RATIO_THRESHOLD_LOW: f64 = 0.5;
pub const PNG_COLORS_PER_MP_THRESHOLD: f64 = 50.0;
pub const EXPLORE_CONFIDENCE_HIGH: f64 = 0.85;
pub const EXPLORE_CONFIDENCE_NORMAL: f64 = 0.75;
pub const EXPLORE_CONFIDENCE_MEDIUM: f64 = 0.7;
pub const EXPLORE_CONFIDENCE_LOW: f64 = 0.6;
pub const EXPLORE_BINARY_SEARCH_PRECISION_SIZE: f32 = 0.5;
pub const EXPLORE_BINARY_SEARCH_PRECISION_QUALITY: f32 = 1.0;
pub const EXPLORE_QUALITY_WINDOW_CRF: f32 = 5.0;
pub const BPP_LOW_GATE_HEVC: f64 = 0.03;
pub const BPP_LOW_GATE_AV1: f64 = 0.02;
pub const JXL_DISTANCE_PLATEAU: f64 = 0.01;
pub const JXL_MICRO_PRESSURE_LIMIT: f64 = 0.070_389_327_9;
pub const HEIC_MAX_ITEMS: u32 = 500_000;
pub const HEIC_MAX_COMPONENTS: u32 = 50_000;
pub const HEIC_MAX_EXTENTS: u32 = 50_000;
pub const HEIC_MAX_MEMORY_LIMIT: u64 = 15 * 1024 * 1024 * 1024;
pub const NEGLIGIBLE_DURATION_F32: f32 = 0.01;
pub const GPU_COARSE_SEARCH_DEFAULT_AUDIO_BITRATE: u64 = 128_000;
pub const LOOP_INTENT_NEUTRAL_CONFIDENCE: f64 = 0.55;
pub const LOOP_INTENT_CLOSURE_HIGH: f64 = 0.82;
pub const LOOP_INTENT_CLOSURE_LOW: f64 = 0.35;
pub const LOOP_INTENT_PERIODICITY_HIGH: f64 = 0.72;
pub const LOOP_INTENT_PERIODICITY_LOW: f64 = 0.32;
pub const LOOP_INTENT_IFRAME_RATIO_TARGET: f64 = 0.50;
pub const LOOP_INTENT_IFRAME_RATIO_HIGH: f64 = 0.85;
pub const LOOP_INTENT_IFRAME_RATIO_LOW: f64 = 0.15;
pub const LOOP_INTENT_Z_SCORE_STRENGTH: f64 = 1.5;
pub const LOOP_INTENT_ANTI_LOOP_THRESHOLD: f64 = 0.45;
pub const LOOP_INTENT_SIGNAL_BONUS: f64 = 0.06;
pub const LOOP_INTENT_KNN_HIGH: f64 = 0.65;
pub const LOOP_INTENT_KNN_LOW: f64 = 0.35;
pub const LOOP_INTENT_KNN_SCALE: f64 = 0.90;
pub const LOOP_INTENT_KNN_MIN_DELTA: f64 = 0.08;
pub const LOOP_INTENT_KNN_MAX_DELTA: f64 = 0.28;
pub const LOOP_INTENT_TREE_SCALE: f64 = 0.45;
pub const LOOP_INTENT_TREE_MIN_DELTA: f64 = 0.05;
pub const LOOP_INTENT_TREE_MAX_DELTA: f64 = 0.22;
pub const LOOP_INTENT_DEFAULT_STRENGTH: f64 = 0.25;
pub const LOOP_INTENT_MIN_STRENGTH: f64 = 0.15;
pub const LOOP_INTENT_MAX_STRENGTH: f64 = 1.0;
pub const LOOP_INTENT_PROB_MIN: f64 = 0.01;
pub const LOOP_INTENT_PROB_MAX: f64 = 0.99;
pub const LOOP_INTENT_ZERO_MOTION_THRESHOLD: f64 = 0.70;
pub const LOOP_INTENT_MEDIAN_SCALING: f64 = 0.60;
pub const LOOP_INTENT_NEUTRAL_PROB: f64 = 0.50;
pub const LOOP_INTENT_RELIABILITY_BONUS_HIGH: f64 = 0.85;
pub const LOOP_INTENT_RELIABILITY_BONUS_MEDIUM: f64 = 0.75;
pub const LOOP_INTENT_MOTION_GINI_NEUTRAL: f64 = 0.55;
pub const LOOP_INTENT_ARBITRATION_MARKER_BONUS: f64 = 0.24;
pub const LOOP_INTENT_ARBITRATION_TRANSPARENCY_BONUS: f64 = 0.22;
pub const LOOP_INTENT_ARBITRATION_AUDIO_BONUS: f64 = 0.14;
pub const LOOP_INTENT_ARBITRATION_METADATA_BONUS: f64 = 0.10;
pub const LOOP_INTENT_SUPPORT_HIGH: f64 = 0.80;
pub const KNN_VECTOR_DEFAULT_AUDIO_SCORE: f64 = 0.55;
pub const KNN_VECTOR_BASELINE_FPS: f64 = 30.0;
pub const KNN_VECTOR_FPS_NORMALIZATION_SCALE: f64 = 1.2;
pub const KNN_VECTOR_LAFFIN_FPS_WEIGHT: f64 = 0.10;
pub const KNN_VECTOR_LAFFIN_FREQ_WEIGHT: f64 = 0.45;
pub const KNN_VECTOR_LAFFIN_CADENCE_WEIGHT: f64 = 0.25;
pub const KNN_VECTOR_LAFFIN_AUDIO_WEIGHT: f64 = 0.20;
pub const KNN_VECTOR_CAT_MEME_WEIGHT: f64 = 1.2;
pub const KNN_VECTOR_CAT_NAME_WEIGHT: f64 = 0.8;
pub const KNN_VECTOR_CAT_NATIVE_WEIGHT: f64 = 0.6;
pub const KNN_VECTOR_CAT_HIGH_VALUE_WEIGHT: f64 = 1.5;
pub const KNN_VECTOR_CAT_TRANS_WEIGHT: f64 = 1.5;
pub const KNN_VECTOR_CAT_ICC_WEIGHT: f64 = 1.2;
pub const KNN_VECTOR_CAT_COMPLEX_WEIGHT: f64 = 1.2;
pub const EXPLORE_SUCCESS_SIZE_MARGIN: f64 = 0.1;
pub const JXL_BOUNDARY_LOW_RATIO: f64 = 0.95;
pub const JXL_BOUNDARY_HIGH_RATIO: f64 = 1.05;
pub const JXL_REGION_BUCKET_COUNT: f64 = 6.0;
pub const JXL_DISTANCE_VISUAL_LOSSLESS_MAX: f64 = 0.1;
pub const JXL_DISTANCE_BALANCED_MAX: f64 = 0.3;
pub const JXL_BOUNDARY_PRESSURE_STOPS_MAX: f64 = 0.584_962_500_7;
pub const JXL_WIDE_PRESSURE_STOPS_MAX: f64 = 1.321_928_094_9;
pub const JXL_EXPLORE_PLATEAU_LIMIT: f64 = 0.99;
pub const JXL_EXPLORE_FLOOR_LIMIT: f64 = 0.1;
pub const PNG_DEFAULT_SAFETY_BIT_DEPTH: u8 = 16;
pub const AVIF_DEFAULT_SAFETY_BIT_DEPTH: u8 = 8;
pub const HEIC_DEFAULT_SAFETY_BIT_DEPTH: u8 = 8;
pub const BPP_THRESHOLD_ULTRA: f64 = 5.0;
pub const BPP_THRESHOLD_VERY_HIGH: f64 = 1.0;
pub const BPP_THRESHOLD_HIGH: f64 = 0.3;
pub const BPP_THRESHOLD_MEDIUM: f64 = 0.1;
pub const BPP_THRESHOLD_LOW: f64 = 0.05;
pub const BPP_THRESHOLD_VERY_LOW: f64 = 0.02;
pub const BPP_FACTOR_MODERN: f64 = 0.6;
pub const BPP_FACTOR_INEFFICIENT: f64 = 2.0;
pub const CHROMA_FACTOR_YUV420: f64 = 1.0;
pub const CHROMA_FACTOR_YUV422: f64 = 1.05;
pub const CHROMA_FACTOR_YUV444: f64 = 1.15;
pub const CHROMA_FACTOR_RGB: f64 = 1.20;
pub const JPEG_IJG_LUMINANCE_BASE: [[u16; 8]; 8] = [
    [16, 11, 10, 16, 24, 40, 51, 61],
    [12, 12, 14, 19, 26, 58, 60, 55],
    [14, 13, 16, 24, 40, 57, 69, 56],
    [14, 17, 22, 29, 51, 87, 80, 62],
    [18, 22, 37, 56, 68, 109, 103, 77],
    [24, 35, 55, 64, 81, 104, 113, 92],
    [49, 64, 78, 87, 103, 121, 120, 101],
    [72, 92, 95, 98, 112, 100, 103, 99],
];
pub const JPEG_IJG_CHROMINANCE_BASE: [[u16; 8]; 8] = [
    [17, 18, 24, 47, 99, 99, 99, 99],
    [18, 21, 26, 66, 99, 99, 99, 99],
    [24, 26, 56, 99, 99, 99, 99, 99],
    [47, 66, 99, 99, 99, 99, 99, 99],
    [99, 99, 99, 99, 99, 99, 99, 99],
    [99, 99, 99, 99, 99, 99, 99, 99],
    [99, 99, 99, 99, 99, 99, 99, 99],
    [99, 99, 99, 99, 99, 99, 99, 99],
];
pub const JPEG_SSE_WEIGHTS: [[f64; 8]; 8] = [
    [1.0, 0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3],
    [0.9, 0.85, 0.75, 0.65, 0.55, 0.45, 0.35, 0.25],
    [0.8, 0.75, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2],
    [0.7, 0.65, 0.6, 0.5, 0.4, 0.3, 0.2, 0.15],
    [0.6, 0.55, 0.5, 0.4, 0.3, 0.2, 0.15, 0.1],
    [0.5, 0.45, 0.4, 0.3, 0.2, 0.15, 0.1, 0.08],
    [0.4, 0.35, 0.3, 0.2, 0.15, 0.1, 0.08, 0.05],
    [0.3, 0.25, 0.2, 0.15, 0.1, 0.08, 0.05, 0.03],
];
pub const MICROSECONDS_PER_SECOND: f64 = 1_000_000.0;
pub const MSSSIM_PROGRESS_PRINT_STEP: u32 = 10;
pub const DURATION_THRESHOLD_SUSPICIOUS: f32 = 0.25;
pub const DURATION_THRESHOLD_MIN: f32 = 0.01;
pub const FALLBACK_FPS: f32 = 10.0;
pub const JPEG_QUALITY_MAPPING_V1_PSNR_BASE: f64 = 45.0;
pub const JPEG_QUALITY_MAPPING_V1_SSIM_BASE: f64 = 0.98;
pub const F64_EPSILON: f64 = 1e-6;
pub const F32_EPSILON: f32 = 1e-4;
pub const SSIM_EPSILON: f64 = 1e-4;
pub const CRF_EPSILON: f32 = 0.01;
pub const PSNR_EPSILON: f64 = 0.1;
// --- JXL Distance Mapping Constants ---
/// Scaling factor to map 0-100 quality to 0-10 distance.
pub const JXL_QUALITY_MAP_DIVISOR: f32 = 10.0;
/// Conservative bias (retains more quality).
pub const JXL_QUALITY_BIAS_CONSERVATIVE: f32 = -0.2;
/// Aggressive bias (prioritizes smaller size).
pub const JXL_QUALITY_BIAS_AGGRESSIVE: f32 = 0.3;
/// Maximum JXL distance allowed for quality-matched output.
pub const JXL_MAX_DISTANCE: f32 = 5.0;
pub const JXL_MIN_DISTANCE: f32 = 0.0;
/// Scaling factor for content-type distance adjustment.
pub const JXL_CONTENT_ADJ_SCALE: f32 = 0.1;
// --- Analysis Confidence Weights ---
pub const CONF_W_DIMENSIONS: f64 = 25.0;
pub const CONF_W_FILE_SIZE: f64 = 20.0;
pub const CONF_W_BPP: f64 = 10.0;
pub const CONF_W_CODEC: f64 = 8.0;
pub const CONF_W_BITRATE: f64 = 5.0;
pub const CONF_W_GOP: f64 = 4.0;
pub const CONF_W_B_FRAMES: f64 = 3.0;
pub const CONF_W_PIX_FMT: f64 = 3.0;
pub const CONF_W_COLOR: f64 = 3.0;
pub const CONF_W_CONTENT: f64 = 2.0;
pub const CONF_W_COMPLEXITY: f64 = 3.0;
// --- Codec Efficiency Multipliers ---
pub const EFF_MULT_PLACEBO: f64 = 0.80;
pub const EFF_MULT_VERYSLOW: f64 = 0.85;
pub const EFF_MULT_SLOW: f64 = 0.90;
pub const EFF_MULT_MEDIUM: f64 = 1.0;
pub const EFF_MULT_FAST: f64 = 1.10;
pub const EFF_MULT_VERYFAST: f64 = 1.15;
pub const EFF_MULT_SUPERFAST: f64 = 1.20;
pub const EFF_MULT_ULTRAFAST: f64 = 1.30;
// --- Resolution Factor Thresholds ---
pub const RES_FACTOR_THRESHOLD_ULTRA_HD: f64 = 8.0; // 8MP
pub const RES_FACTOR_THRESHOLD_FULL_HD: f64 = 2.0; // 2MP
pub const RES_FACTOR_THRESHOLD_SD: f64 = 0.5; // 0.5MP
pub const RES_FACTOR_SLOPE: f64 = 0.05;
pub const RES_FACTOR_BASE_UHD: f64 = 0.80;
pub const RES_FACTOR_BASE_FHD: f64 = 0.85;
pub const RES_FACTOR_BASE_SD: f64 = 0.90;
pub const RES_FACTOR_BASE_THUMB: f64 = 0.95;
// --- GPU Sampling & Search Constants ---
pub const GPU_SAMPLE_DURATION: f32 = 60.0;
pub const GPU_SEGMENT_DURATION: f32 = 15.0;
pub const GPU_SAMPLE_DURATION_ULTIMATE: f32 = 60.0;
pub const GPU_SEGMENT_DURATION_ULTIMATE: f32 = 13.0;
pub const GPU_MIN_DURATION_FOR_SAMPLING: f64 = 60.0;
pub const GPU_COARSE_STEP: f32 = 1.0;
pub const GPU_ABSOLUTE_MAX_ITERATIONS: u32 = 750;
pub const GPU_MAX_ITERATIONS: u32 = GPU_ABSOLUTE_MAX_ITERATIONS;
// --- CRF Estimation Formulas ---
pub const CRF_EST_H26X_SLOPE: f64 = 6.0;
pub const CRF_EST_H26X_INTERCEPT: f64 = 50.0;
pub const CRF_EST_H26X_MAX: f64 = 35.0;
pub const CRF_EST_H26X_MIN: f64 = 18.0;
pub const CRF_EST_AV1_SLOPE: f64 = 5.0;
pub const CRF_EST_AV1_INTERCEPT: f64 = 46.0;
pub const CRF_EST_AV1_MAX: f64 = 35.0;
pub const CRF_EST_AV1_MIN: f64 = 15.0;
pub const JXL_QUAL_EST_SLOPE: f64 = 15.0;
pub const JXL_QUAL_EST_BPP_SCALE: f64 = 5.0;
pub const JXL_QUAL_EST_INTERCEPT: f64 = 70.0;
// --- MS-SSIM Sampling Strategy ---
pub const MSSSIM_DURATION_THRESHOLD_FULL: f64 = 60.0;
pub const MSSSIM_DURATION_THRESHOLD_THIRD: f64 = 300.0;
pub const MSSSIM_DURATION_THRESHOLD_TENTH: f64 = 1800.0;
pub const MSSSIM_RATE_FULL: u32 = 1;
pub const MSSSIM_RATE_THIRD: u32 = 3;
pub const MSSSIM_RATE_TENTH: u32 = 10;
// --- Content Complexity Factors ---
pub const COMPLEXITY_SI_BASE: f64 = 50.0;
pub const COMPLEXITY_TI_BASE: f64 = 20.0;
pub const COMPLEXITY_SI_RATIO_HIGH: f64 = 1.3;
pub const COMPLEXITY_SI_RATIO_LOW: f64 = 0.7;
pub const COMPLEXITY_SI_FACTOR_HIGH: f64 = 1.15;
pub const COMPLEXITY_SI_FACTOR_LOW: f64 = 0.85;
pub const COMPLEXITY_TI_RATIO_HIGH: f64 = 1.5;
pub const COMPLEXITY_TI_RATIO_LOW: f64 = 0.5;
pub const COMPLEXITY_TI_FACTOR_HIGH: f64 = 1.10;
pub const COMPLEXITY_TI_FACTOR_LOW: f64 = 0.90;
pub const BPP_EXPECTED_UHD: f64 = 0.15;
pub const BPP_EXPECTED_FHD: f64 = 0.20;
pub const BPP_EXPECTED_SD: f64 = 0.30;
pub const BPP_EXPECTED_THUMB: f64 = 0.45;
// --- Aspect Ratio Factors ---
pub const ASPECT_RATIO_ULTRA_WIDE: f64 = 2.5;
pub const ASPECT_RATIO_WIDE: f64 = 2.0;
pub const ASPECT_RATIO_TALL: f64 = 0.5;
pub const ASPECT_FACTOR_EXTREME: f64 = 1.08;
pub const ASPECT_FACTOR_MODERATE: f64 = 1.04;
// --- Color Depth Factors ---
pub const COLOR_DEPTH_FACTOR_GIF: f64 = 1.3;
pub const COLOR_DEPTH_FACTOR_10BIT: f64 = 1.25;
pub const COLOR_DEPTH_FACTOR_12BIT: f64 = 1.5;
pub const COLOR_DEPTH_FACTOR_16BIT: f64 = 2.0;
// --- GOP & Chroma Factors ---
pub const GOP_FACTOR_I_ONLY: f64 = 0.70;
pub const GOP_FACTOR_VERY_SHORT: f64 = 0.85;
pub const GOP_FACTOR_STANDARD: f64 = 1.0;
pub const GOP_FACTOR_LONG: f64 = 1.15;
pub const GOP_FACTOR_VERY_LONG: f64 = 1.20;
pub const GOP_FACTOR_EXTREME: f64 = 1.25;
pub const B_FRAME_BONUS_1: f64 = 1.05;
pub const B_FRAME_BONUS_2: f64 = 1.08;
pub const B_FRAME_BONUS_MANY: f64 = 1.12;
pub const CHROMA_FACTOR_444: f64 = 1.15;
pub const CHROMA_FACTOR_422: f64 = 1.08;
pub const HDR_FACTOR_TRUE: f64 = 1.20;
pub const HDR_FACTOR_BT2020: f64 = 1.15;
// --- Complexity BPP Ratio Factors ---
pub const COMPLEXITY_RATIO_HIGH_THRESHOLD: f64 = 2.0;
pub const COMPLEXITY_RATIO_LOW_THRESHOLD: f64 = 0.5;
pub const COMPLEXITY_RATIO_MAX_FACTOR: f64 = 1.15;
pub const COMPLEXITY_RATIO_MIN_FACTOR: f64 = 0.95;
pub const COMPLEXITY_RATIO_SLOPE: f64 = 0.15;
// --- JPEG Quality Mapping (PSNR) ---
pub const JPEG_MAP_PSNR_H95_SLOPE: f64 = 0.5;
pub const JPEG_MAP_PSNR_H85_SLOPE: f64 = 0.7;
pub const JPEG_MAP_PSNR_H85_BASE: f64 = 38.0;
pub const JPEG_MAP_PSNR_H75_SLOPE: f64 = 0.6;
pub const JPEG_MAP_PSNR_H75_BASE: f64 = 32.0;
pub const JPEG_MAP_PSNR_H60_SLOPE: f64 = 0.27;
pub const JPEG_MAP_PSNR_H60_BASE: f64 = 28.0;
pub const JPEG_MAP_PSNR_LOW_SLOPE: f64 = 0.13;
pub const JPEG_MAP_PSNR_LOW_BASE: f64 = 20.0;
// --- JPEG Quality Mapping (SSIM) ---
pub const JPEG_MAP_SSIM_H95_SLOPE: f64 = 0.004;
pub const JPEG_MAP_SSIM_H85_SLOPE: f64 = 0.003;
pub const JPEG_MAP_SSIM_H85_BASE: f64 = 0.95;
pub const JPEG_MAP_SSIM_H75_SLOPE: f64 = 0.005;
pub const JPEG_MAP_SSIM_H75_BASE: f64 = 0.90;
pub const JPEG_MAP_SSIM_H60_SLOPE: f64 = 0.0067;
pub const JPEG_MAP_SSIM_H60_BASE: f64 = 0.80;
pub const JPEG_MAP_SSIM_LOW_SLOPE: f64 = 0.003;
pub const JPEG_MAP_SSIM_LOW_BASE: f64 = 0.60;
// --- JPEG Analysis Weights & Thresholds ---
pub const JPEG_LUMA_WEIGHT: f64 = 0.7;
pub const JPEG_CHROMA_WEIGHT: f64 = 0.3;
pub const JPEG_CONFIDENCE_THRESHOLD_STANDARD: f64 = 0.95;
pub const JPEG_CONFIDENCE_THRESHOLD_STRICT: f64 = 0.98;
// --- Loop Intent Decision Tree Thresholds ---
pub const TREE_CLOSURE_HIGH_THRESHOLD: f64 = 0.82;
pub const TREE_CLOSURE_LOW_THRESHOLD: f64 = 0.35;
pub const TREE_PERIODICITY_HIGH_THRESHOLD: f64 = 0.72;
pub const TREE_PERIODICITY_LOW_THRESHOLD: f64 = 0.32;
pub const TREE_FREQUENCY_HIGH_THRESHOLD: f64 = 0.75;
pub const TREE_FREQUENCY_LOW_THRESHOLD: f64 = 0.25;
pub const TREE_CADENCE_HIGH_THRESHOLD: f64 = 0.90;
pub const TREE_JITTER_HIGH_THRESHOLD: f64 = 0.82;
pub const TREE_JITTER_LOW_THRESHOLD: f64 = 0.25;
pub const TREE_IFRAME_RATIO_HIGH_THRESHOLD: f64 = 0.85;
pub const TREE_IFRAME_RATIO_LOW_THRESHOLD: f64 = 0.15;
pub const TREE_BPF_Z_THRESHOLD: f64 = 1.5;
pub const DEFAULT_GIF_FPS_FALLBACK: f64 = 12.0;
// --- Image Quality Detection Weights & Thresholds ---
pub const DETECTION_WEIGHT_STRUCTURAL: f64 = 0.35;
pub const DETECTION_WEIGHT_METADATA: f64 = 0.15;
pub const DETECTION_WEIGHT_STATISTICAL: f64 = 0.30;
pub const DETECTION_WEIGHT_HEURISTIC: f64 = 0.20;
pub const DETECTION_LOSSY_THRESHOLD: f64 = 0.58;
pub const DETECTION_LOSSLESS_THRESHOLD: f64 = 0.40;
// --- Codec Efficiency Ratios (Baseline: H.264 = 1.0) ---
pub const EFF_RATIO_AV1: f64 = 0.50;
pub const EFF_RATIO_VP9: f64 = 0.70;
pub const EFF_RATIO_VP8: f64 = 0.85;
pub const EFF_RATIO_HEVC: f64 = 0.65;
pub const EFF_RATIO_VVC: f64 = 0.35;
pub const EFF_RATIO_AV2: f64 = 0.35;
pub const EFF_RATIO_JXL: f64 = 0.60;
pub const EFF_RATIO_AVIF: f64 = 0.55;
pub const EFF_RATIO_WEBP_ANIM: f64 = 0.90;
pub const EFF_RATIO_WEBP_STATIC: f64 = 0.75;
pub const EFF_RATIO_MPEG4: f64 = 1.30;
pub const EFF_RATIO_MPEG2: f64 = 1.80;
pub const EFF_RATIO_MJPEG: f64 = 2.50;
pub const EFF_RATIO_GIF: f64 = 3.00;
pub const EFF_RATIO_BMP: f64 = 3.00;
pub const EFF_RATIO_WMV: f64 = 1.10;
pub const EFF_RATIO_THEORA: f64 = 1.20;
pub const EFF_RATIO_TIFF: f64 = 1.20;
pub const EFF_RATIO_REALVIDEO: f64 = 2.00;
pub const EFF_RATIO_FLASH: f64 = 1.50;
pub const EFF_RATIO_PRORES: f64 = 1.80;
// --- Quality Matcher Scoring ---
pub const MATCHER_SCORE_TRUST_WEIGHT: f64 = 4.0;
pub const MATCHER_SCORE_FORMAT_WEIGHT: f64 = 3.0;
pub const MATCHER_SCORE_BITRATE_WEIGHT: f64 = 2.0;
pub const MATCHER_BIAS_CONSERVATIVE: f64 = -2.0;
pub const MATCHER_BIAS_AGGRESSIVE: f64 = 2.0;
pub const MATCHER_CRF_ROUNDING_FACTOR: f64 = 2.0;
// PNG Statistical Analysis Constants
pub const PNG_DITHERING_THRESHOLD: f64 = 0.5;
pub const PNG_BANDING_THRESHOLD: f64 = 0.5;
pub const PNG_FREQ_THRESHOLD: f64 = 0.5;
pub const PNG_USAGE_RATIO_CRITICAL: f64 = 0.9;
pub const PNG_USAGE_RATIO_HIGH: f64 = 0.8;
pub const PNG_USAGE_RATIO_MEDIUM: f64 = 0.7;
pub const PNG_USAGE_RATIO_RELAXED: f64 = 0.5;
pub const PNG_ANOMALY_SCORE_CRITICAL: f64 = 0.85;
pub const PNG_ANOMALY_SCORE_HIGH: f64 = 0.8;
pub const PNG_ANOMALY_SCORE_MEDIUM: f64 = 0.7;
pub const PNG_ANOMALY_SCORE_LOW: f64 = 0.5;
pub const PNG_ANOMALY_SCORE_MIN: f64 = 0.35;

// PNG Entropy and Efficiency Constants
pub const PNG_ENTROPY_PALETTE_SIZE_MEDIUM: f64 = 64.0;
pub const PNG_ENTROPY_PALETTE_SIZE_LARGE: f64 = 128.0;
pub const PNG_ENTROPY_ANOMALY_MAX: f64 = 0.75;
pub const PNG_ENTROPY_ANOMALY_HIGH: f64 = 0.7;
pub const PNG_ENTROPY_ANOMALY_THRESHOLD: f64 = 0.4;
pub const PNG_SIZE_EFFICIENCY_THRESHOLD: f64 = 0.15;
pub const PNG_SIZE_EFFICIENCY_ANOMALY: f64 = 0.6;
pub const PNG_PALETTE_DENSITY_HIGH: f64 = 0.5;
pub const PNG_PALETTE_DENSITY_MEDIUM: f64 = 0.3;
pub const PNG_PALETTE_SCORE_LOW: f64 = 0.1;
pub const PNG_PALETTE_SCORE_MEDIUM: f64 = 0.15;
pub const PNG_PALETTE_SCORE_HIGH: f64 = 0.3;

// Video Metadata Thresholds
pub const VIDEO_NEGLIGIBLE_DURATION_SECS: f64 = 0.1;

#[cfg(test)]
mod tests {
    use super::{
        JXL_DEFAULT_EFFORT, JXL_ULTIMATE_DISTANCE, JXL_ULTIMATE_EFFORT, is_supported_jxl_effort,
        jxl_distance_for_mode, jxl_effort_for_mode,
    };
    #[test]
    fn test_jxl_effort_policy_is_mode_locked() {
        assert_eq!(jxl_effort_for_mode(false), JXL_DEFAULT_EFFORT);
        assert_eq!(jxl_effort_for_mode(true), JXL_ULTIMATE_EFFORT);
        assert!(is_supported_jxl_effort(JXL_DEFAULT_EFFORT));
        assert!(is_supported_jxl_effort(JXL_ULTIMATE_EFFORT));
        assert!(!is_supported_jxl_effort(6));
        assert!(!is_supported_jxl_effort(8));
    }

    #[test]
    fn test_jxl_distance_policy_pins_ultimate_mode() {
        assert!((jxl_distance_for_mode(0.4, false) - 0.4).abs() < f32::EPSILON);
        assert!((jxl_distance_for_mode(0.4, true) - JXL_ULTIMATE_DISTANCE).abs() < f32::EPSILON);
    }
}
