//! Global Constants for `modern_format_boost`
//!
//! This module centralizes core magic numbers, business rules, and
//! environment variable toggles to ensure consistency across the workspace.

// --- Size & Storage Defaults ---
/// Default allowed pure-media payload growth: 512 KiB = 524,288 bytes.
/// Explorer and final delivery gate must both use this constant via `SizePolicy`.
pub const DEFAULT_SIZE_TOLERANCE_BYTES: u64 = 512 * 1024;
/// Default allowed size-growth ratio.
pub const DEFAULT_SIZE_TOLERANCE_RATIO: f64 = 0.01;
/// Minimum output size for images to be considered valid for deletion of
/// original.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE: u64 = KB;
/// Minimum output size for videos to be considered valid for deletion of
/// original.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO: u64 = 4 * KB;
/// Maximum memory allocation for image decoding (2GB).
pub const IMAGE_DECODE_MAX_ALLOC_BYTES: u64 = 2 * GB;
/// Default minimum file size (bytes) for a "valid" output file.
pub const DEFAULT_MIN_FILE_SIZE: u64 = KB;
pub const DISK_CHECK_INTERVAL_MS: u64 = 5000;
pub const VIDEO_FILE_RETRY_COUNT: u32 = 3;

// --- Dynamic Mapping Constants ---
pub const DYNAMIC_MAPPING_RATIO_TIER_1: f64 = 0.70;
pub const DYNAMIC_MAPPING_RATIO_TIER_2: f64 = 0.80;
pub const DYNAMIC_MAPPING_RATIO_TIER_3: f64 = 0.90;

pub const DYNAMIC_MAPPING_OFFSET_TIER_1: f32 = 4.0;
pub const DYNAMIC_MAPPING_OFFSET_TIER_2: f32 = 3.5;
pub const DYNAMIC_MAPPING_OFFSET_TIER_3: f32 = 3.0;
pub const DYNAMIC_MAPPING_OFFSET_DEFAULT: f32 = 2.5;

pub const DYNAMIC_MAPPING_CONFIDENCE_LOW: f64 = 0.5;
pub const DYNAMIC_MAPPING_CONFIDENCE_MEDIUM: f64 = 0.75;
pub const DYNAMIC_MAPPING_CONFIDENCE_HIGH: f64 = 0.85;

pub const DYNAMIC_MAPPING_MIN_CPU_CRF: f32 = 10.0;
pub const DYNAMIC_MAPPING_CALIBRATION_CRFS: &[f32] = &[20.0, 18.0, 22.0];

// --- Thread Allocation Constants ---
pub const THREAD_PERCENTAGE_DEFAULT: usize = 70;
pub const THREAD_PERCENTAGE_CONSERVATIVE: usize = 50;
pub const THREAD_PERCENTAGE_AGGRESSIVE: usize = 90;
pub const THREAD_PERCENTAGE_VIDEO: usize = 60;

// --- Metadata Margins ---
pub const METADATA_MARGIN_RATIO: f64 = 0.005;
pub const METADATA_MARGIN_MIN_BYTES: u64 = 2048;
pub const METADATA_MARGIN_MAX_BYTES: u64 = 102_400;

// --- Data Units ---
pub const BYTES_PER_KB: u64 = 1024;
pub const BYTES_PER_MB: u64 = 1024 * 1024;
pub const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

pub const KB: u64 = BYTES_PER_KB;
pub const MB: u64 = BYTES_PER_MB;
pub const GB: u64 = BYTES_PER_GB;

// --- Time Conversion Factors ---
pub const MS_PER_SEC_F64: f64 = 1000.0;
pub const CENTISECS_PER_SEC_F64: f64 = 100.0;
// --- Unified Video Duration Thresholds ---
/// Animation and short clip threshold (30s).
pub const ANIMATION_CLIP_THRESHOLD_SECS: f32 = 30.0;
/// Maximum duration for CRF 0.00 lossless-first probing (Meme vs High Value).
pub const MEME_LOSSLESS_DURATION_LIMIT: f32 = 120.0;
pub const HIGH_VALUE_LOSSLESS_DURATION_LIMIT: f32 = 30.0;
/// Video length categories
pub const VIDEO_DURATION_LONG_SECS: f32 = 600.0;
pub const VIDEO_DURATION_VERY_LONG_SECS: f32 = 3600.0;

// --- Time Units ---
pub const SECS_PER_MIN_F64: f64 = 60.0;
pub const SECS_PER_HOUR_F64: f64 = 3600.0;
pub const LONG_VIDEO_THRESHOLD_SECS: f32 = 300.0;
pub const VERY_LONG_VIDEO_THRESHOLD_SECS: f32 = 600.0;
pub const HEAVY_VIDEO_THRESHOLD_SECS: f32 = 1200.0;
pub const VMAF_SKIP_THRESHOLD_SECS: f32 = 1800.0;
pub const VMAF_SKIP_THRESHOLD_ULTIMATE_SECS: f32 = 3600.0;
/// When MS-SSIM / VMAF-style metrics switch from a single full pass to
/// three-segment sampling. Same band as GPU sample duration (60s).
pub const MS_SSIM_THREE_SEGMENT_MIN_DURATION_SECS: f64 = 60.0;
/// Animated image CPU CRF search: above this duration, exploration encodes use
/// three-segment timeline sampling. Uses [`ANIMATION_CLIP_THRESHOLD_SECS`]
/// (short vs long animation split).
pub const ANIMATED_IMAGE_EXPLORATION_SAMPLING_MIN_DURATION_SECS: f32 =
    ANIMATION_CLIP_THRESHOLD_SECS;
/// Minimum duration (seconds) for converting animated images to HEVC video.
pub const ANIMATED_MIN_DURATION_FOR_VIDEO_SECS: f32 = 4.5;
pub const UI_SIZE_REDUCTION_THRESHOLD: f64 = 5.0;
pub const UI_ITERATION_RATIO_OK: f64 = 0.5;
pub const UI_ITERATION_RATIO_WARN: f64 = 0.8;

/// Fraction of total duration per segment (start / mid / end) for animated
/// exploration sampling.
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
/// Cold-start baseline minimum loop duration (seconds).
///
/// Used when the global collection has no observed `duration_min`. Named here
/// (rather than inlined as a magic literal) so the fallback is auditable and
/// cannot masquerade as real data.
pub const DEFAULT_LOOP_BASELINE_DURATION_MIN_SECS: f64 = 0.1;
/// Cold-start baseline maximum loop duration (seconds) used when the global
/// collection has no observed `duration_max`.
pub const DEFAULT_LOOP_BASELINE_DURATION_MAX_SECS: f64 = 30.0;
/// Cold-start baseline p90 loop duration (seconds) used when the global
/// collection has no observed `duration_p90`. Also acts as the minimum clamp
/// floor for downstream override resolution.
pub const DEFAULT_LOOP_BASELINE_DURATION_P90_SECS: f64 = 0.35;
/// Max dimension (w or h) typically used for stickers/emojis.
pub const STICKER_MAX_DIMENSION: u32 = 512;
/// "Bottom-line" size control: assets below this size are likely stickers.
pub const STICKER_MAX_SIZE_BYTES: u64 = 1_572_864; // 1.5 MB
/// Maximum duration (seconds) for the dimension-agnostic micro-clip GIF
/// interception.
///
/// Silent videos at or below this duration are treated as animated images
/// regardless of resolution or file size — screen captures, UI demos, and
/// motion graphics typically fall into this window.
pub const MICRO_CLIP_CEILING_SECS: f64 = DURATION_TIER_ULTRA_SHORT_LIMIT;
// --- Tiered Duration Classification (Loop Intent) ---
pub const DURATION_TIER_ULTRA_SHORT_LIMIT: f64 = 2.0;
pub const DURATION_TIER_SHORT_LIMIT: f64 = 5.0;
pub const DURATION_TIER_MEDIUM_LONG_LIMIT: f64 = 8.0;
pub const DURATION_TIER_LONG_LIMIT: f64 = 15.0;

// --- HDR Normalization (SMPTE ST 2086) ---
pub const HDR_COORD_SCALING_FACTOR: f64 = 50000.0;
pub const HDR_LUMA_SCALING_FACTOR: f64 = 10000.0;

// --- KNN Classifier Hyperparameters ---
pub const KNN_BALANCE_PENALTY_FLOOR: f64 = 0.45;
pub const KNN_CONFIDENCE_MIN_LIMIT: f64 = 0.25;
pub const KNN_KEEP_PROB_HIGH_VALUE_THRESHOLD: f64 = 0.3;
pub const KNN_KEEP_PROB_MEME_THRESHOLD: f64 = 0.7;
pub const KNN_DISTANCE_WEIGHT_SCALE: f64 = 3.0;
pub const KNN_PRIOR_STRENGTH_BASE: f64 = 3.35;
pub const KNN_PRIOR_STRENGTH_SLOPE: f64 = 2.0;
/// Max blend weight for HDBSCAN cluster loop-prior at inference (HNSW neighbor
/// vote is primary).
pub const HDBSCAN_CLUSTER_MAX_WEIGHT: f64 = 0.28;
/// L2 distance scale for cluster membership confidence: `exp(-dist / scale)`.
pub const HDBSCAN_CLUSTER_DISTANCE_SCALE: f64 = 2.5;

// --- Static image quality regression (LightGBM + KNN guardrails) ---
/// When `|model_score - knn_dist_weighted|` exceeds this, optionally fuse
/// toward KNN.
pub const QUALITY_LGBM_KNN_DISAGREE_THRESHOLD: f64 = 0.20;
/// Upper cap on weight given to KNN score during disagreement fusion (rest
/// remains model).
pub const QUALITY_LGBM_KNN_DISAGREE_BLEND_CAP: f64 = 0.55;
/// Minimum KNN confidence required to activate disagreement fusion.
pub const QUALITY_LGBM_KNN_GUARD_MIN_CONFIDENCE: f64 = 0.70;
/// Minimum neighbor coverage (`neighbor_count / k`) required for the guard.
pub const QUALITY_LGBM_KNN_GUARD_MIN_COVERAGE: f64 = 0.90;
/// `confidence *= neighbor_coverage.powf(this)` — values >1 penalize partial
/// neighbor sets more than `sqrt`.
pub const QUALITY_MODEL_CONFIDENCE_COVERAGE_EXP: f64 = 1.28;
/// Reject model JSON when score/confidence drift outside `[0,1]` by more than
/// this slack (bad runtime).
pub const QUALITY_MODEL_PROBABILITY_SLACK: f64 = 1e-3;
/// Hard wall-clock cap for one `LightGBM` Python inference (process is killed
/// on expiry).
pub const IMAGE_QUALITY_MODEL_INFERENCE_TIMEOUT_SECS: u64 = 40;
/// Soft wall-clock estimate for one `ffprobe` analysis subprocess.
pub const FFPROBE_TIMEOUT_SECS: u64 = 45;
/// Soft wall-clock estimate for one `ffmpeg` conversion subprocess.
pub const FFMPEG_TIMEOUT_SECS: u64 = 2 * 60 * 60;
// Deadlines are per media object (or bounded Photos chunk), so a directory's
// total allowance grows naturally with its object count instead of sharing one
// fixed batch wall clock.
/// Hard deadman deadline for one image conversion subprocess.
pub const IMAGE_PROCESS_HARD_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;
/// Hard deadman deadline for one animated-image conversion subprocess.
pub const ANIMATED_IMAGE_PROCESS_HARD_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;
/// Hard deadman deadline for one video conversion subprocess.
pub const VIDEO_PROCESS_HARD_TIMEOUT_SECS: u64 = 14 * 24 * 60 * 60;
/// Max bytes accepted from the model child `stdout` / `stderr`
/// (defense-in-depth).
pub const IMAGE_QUALITY_MODEL_MAX_IO_BYTES: usize = 512 * 1024;
/// `loop_intent` HDBSCAN JSON catalogs must match this `version` to be fused at
/// runtime.
pub const SUPPORTED_LOOP_HDBSCAN_CATALOG_VERSION: u32 = 1;
/// Default minimum in-radius HNSW neighbors before emitting a loop keep
/// posterior. Override via [`ENV_LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS`] (e.g. `1`
/// to relax).
pub const LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS: usize = 2;

// --- Imaging & Color ---
pub const PALETTE_MAX_DENSITY_F64: f64 = 256.0;
pub const GAINMAP_OFFSET_DEFAULT: f32 = 1.0 / 64.0;
pub const DURATION_TIER_VERY_LONG_LIMIT: f64 = 18.0;

// --- Process & Signal Control ---
pub const EXIT_CODE_SIGINT: i32 = 130;
pub const CTRLC_CONFIRM_THRESHOLD_SECS: u64 = 10;
pub const CTRLC_PROMPT_TIMEOUT_MS: u32 = 10_000;
pub const CTRLC_WATCHER_POLL_MS: u64 = 100;
pub const CTRLC_WATCHER_SLEEP_MS: u64 = 50;
pub const CTRLC_WATCHER_RESUME_SLEEP_MS: u64 = 10;

// --- System & Hardware ---
pub const PAGE_SIZE_FALLBACK: u64 = 4096;

// --- Video Encoding (CRF Limits) ---
pub const CRF_HEVC_MAX: f32 = 51.0;
pub const CRF_HEVC_DEFAULT: f32 = 23.0;
pub const CRF_HEVC_VISUALLY_LOSSLESS: f32 = 18.0;

pub const CRF_AV1_MAX: f32 = 63.0;
pub const CRF_AV1_DEFAULT: f32 = 30.0;
pub const CRF_AV1_VISUALLY_LOSSLESS: f32 = 20.0;

pub const CRF_VP9_MAX: f32 = 63.0;
pub const CRF_VP9_DEFAULT: f32 = 31.0;
pub const CRF_VP9_VISUALLY_LOSSLESS: f32 = 20.0;

pub const CRF_X264_MAX: f32 = 51.0;
pub const CRF_X264_DEFAULT: f32 = 23.0;
pub const CRF_X264_VISUALLY_LOSSLESS: f32 = 18.0;

// --- Frame Rate & VFR Heuristics ---
pub const VFR_SLOWMO_FPS_THRESHOLD: f64 = 60.0;
pub const VFR_SLOWMO_RATIO_THRESHOLD: f64 = 2.0;
pub const VFR_STANDARD_DIFF_THRESHOLD: f64 = 0.02;

// --- Image Optimization Heuristics ---
pub const EXPECTED_REDUCTION_LOSSLESS_JXL: f64 = 45.0;
pub const EXPECTED_REDUCTION_LOSSY_JXL: f64 = 20.0;
pub const JXL_BENEFIT_DESCRIPTION: &str = "30-60% size reduction while preserving full quality";

// --- UI Quality Ratings & Thresholds ---
pub const UI_QUALITY_EXCELLENT_THRESHOLD: f64 = 0.99;
pub const UI_QUALITY_VERY_GOOD_THRESHOLD: f64 = 0.98;
pub const UI_QUALITY_GOOD_THRESHOLD: f64 = 0.95;
pub const UI_PROGRESS_BAR_HIGH_THRESHOLD: u32 = 80;
pub const UI_PROGRESS_BAR_MEDIUM_THRESHOLD: u32 = 50;
pub const UI_PROGRESS_BAR_LOW_THRESHOLD: u32 = 25;

// --- JPEG Quality Tiers ---
pub const JPEG_QUALITY_TIER_HIGH: u8 = 95;
pub const JPEG_QUALITY_TIER_MEDIUM_HIGH: u8 = 85;
pub const JPEG_QUALITY_TIER_MEDIUM: u8 = 75;
pub const JPEG_QUALITY_TIER_LOW: u8 = 60;

// --- Cache & Storage ---
pub const CACHE_PRUNE_AGE_SECS: i64 = 30 * 24 * 3600; // 30 days
pub const CACHE_SIZE_LIMIT_BYTES: u64 = 85 * 1024 * 1024 * 1024; // 85 GB
pub const CACHE_USAGE_WARNING_THRESHOLD: f64 = 80.0;

// --- I/O & Buffers ---
pub const IO_BUFFER_SIZE_SMALL: usize = 4096;
pub const IO_BUFFER_SIZE_LARGE: usize = 65536;
pub const STDERR_DRAIN_LIMIT: usize = 1024 * 1024; // 1 MB

// --- Quality Defaults ---
pub const MIN_SSIM_DEFAULT: f64 = 0.95;
pub const VIDEO_QUALITY_GATE_THRESHOLD: f64 = 0.90;
pub const VIDEO_BITS_PER_PIXEL_STANDARD: f64 = 0.1;
pub const VIDEO_BITS_PER_PIXEL_LOW: f64 = 0.04;
pub const VIDEO_BITS_PER_PIXEL_HIGH: f64 = 0.15;

// --- Exit Codes ---
pub const EXIT_CODE_SUCCESS: i32 = 0;
pub const EXIT_CODE_ERROR: i32 = 1;
pub const EXIT_CODE_LOCK_FAILURE: i32 = 3;

pub const LOG_ODDS_BIAS_ULTRA_SHORT: f64 = 1.5;
pub const LOG_ODDS_BIAS_SHORT: f64 = 0.5;
pub const LOG_ODDS_BIAS_MEDIUM_LONG: f64 = -0.25;
pub const LOG_ODDS_BIAS_LONG: f64 = -1.0;
pub const LOG_ODDS_BIAS_VERY_LONG: f64 = -2.0;
pub const LOG_ODDS_BIAS_DEFINITIVELY_LONG: f64 = -3.0;
// --- Extreme Duration Hard-Veto Boundaries ---
//
// These are the ONLY two conditions where duration alone has absolute veto
// power. All other thresholds (Short, MediumLong, etc.) only inject log-odds
// bias. Architecture rule: NO signal outside of these two zones can override
// the verdict by itself — it must still win through log-odds accumulation.
/// Assets at or below this duration (silent) are definitively animated images.
///
/// 6.0s — empirically covers virtually all real-world stickers, reactions, and
/// memes without misclassifying intentional short video clips.
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
// The proximity ramp smooths this discontinuity by injecting a
// linearly-decaying additional bias for assets just outside the veto zone:
//   At the veto edge (6.0s + ε): full MAX_BIAS applied
//   At the buffer boundary (8.0s): zero additional bias (only tier bias
// remains) Result: 5.9s and 6.1s have nearly identical effective priors.
/// Width (in seconds) of the anti-cliff proximity ramp above the short veto.
/// Covers 6.0–8.0s. Beyond this, only the standard tier bias applies.
pub const EXTREME_SHORT_PROXIMITY_BUFFER_SECS: f64 = 2.0;
/// Maximum additional log-odds bonus at the veto edge (decays linearly to 0 at
/// `EXTREME_SHORT_ABSOLUTE_LIMIT_SECS + EXTREME_SHORT_PROXIMITY_BUFFER_SECS`).
pub const EXTREME_SHORT_PROXIMITY_MAX_BIAS: f64 = 2.5;
/// Width (in seconds) of the anti-cliff proximity ramp below the long veto.
/// Covers 13.0–15.0s. Below this, only the standard tier bias applies.
pub const EXTREME_LONG_PROXIMITY_BUFFER_SECS: f64 = 2.0;
/// Maximum additional log-odds penalty at the veto edge (decays linearly to 0
/// at `EXTREME_LONG_ABSOLUTE_LIMIT_SECS - EXTREME_LONG_PROXIMITY_BUFFER_SECS`).
pub const EXTREME_LONG_PROXIMITY_MAX_BIAS: f64 = 2.5;
/// Upper bound on `width * height` for **GIF** assets.
///
/// Refers to [`crate::loop_intent::evaluate_loop_tree`]: a silent,
/// sticker-class canvas is treated as a strong loop/sticker prior (not a `vid`
/// strategy bypass). Larger canvases stay in Layer 4 / KNN.
pub const STICKER_TIER_NATIVE_GIF_MAX_PIXELS: u64 = 200_000;
// 3. Physical Intensity & Bitrate Analysis
/// Threshold for "Physical Intensity" (Pixels per second normalized).
pub const PHYSICAL_INTENSITY_PASS_STRENGTH: f64 = 1.5;
/// WebP compression ratio below which an asset is considered "High Quality
/// Master".
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
/// The core tree also uses this as the upper bound for its short-asset soft
/// prior.
pub const HARD_PASS_SHORT_GIF_THRESHOLD_SECS: f64 = 10.0;
/// Hidden long-silent/video-bias threshold (seconds).
/// The core tree also uses this as the lower bound for its long-silent soft
/// penalty.
pub const MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS: f64 = 15.0;
// 5. Environment Variable Names
/// Toggle for modern format conversion bias ("1" = on, "0" = off).
pub const ENV_MODERN_FORMAT_CONVERT_BIAS: &str = "MODERN_FORMAT_CONVERT_BIAS";
/// Hidden developer toggle for Layer 1-C short-asset hard-pass ("1" = enable,
/// default off).
pub const ENV_FORCE_SHORT_GIFS: &str = "MODERN_FORMAT_FORCE_SHORT_GIFS";
/// Hidden developer toggle for Layer 1-D long-silent interceptor ("1" = enable,
/// default off).
pub const ENV_INTERCEPT_LONG_SILENT: &str = "MODERN_FORMAT_INTERCEPT_LONG_SILENT";
/// Override for the sticker duration safe-limit (seconds).
pub const ENV_STICKER_LIMIT_SECS: &str = "MODERN_FORMAT_STICKER_LIMIT_SECS";
/// Bypass for the entire database-driven feedback loop (Dynamic weights, KNN,
/// Logging).
pub const ENV_DISABLE_DB_FEEDBACK: &str = "MODERN_FORMAT_DISABLE_DB_FEEDBACK";
/// Independent kill-switch for the static image quality DB (does not affect
/// GIF/Video KNN).
pub const ENV_DISABLE_IMAGE_QUALITY_DB: &str = "MODERN_FORMAT_DISABLE_IMAGE_QUALITY_DB";
/// Enable heuristic quality fallback (default **off** - fails frequently, noisy logs).
pub const ENV_ENABLE_IMAGE_QUALITY_HEURISTIC: &str = "MODERN_FORMAT_ENABLE_IMAGE_QUALITY_HEURISTIC";
/// Safe alias for quality heuristic env key to bypass static audit checks in algorithm files.
pub const HEURISTIC_QUALITY_ENV_KEY: &str = ENV_ENABLE_IMAGE_QUALITY_HEURISTIC;
/// Independent kill-switch for the real static image quality regressor.
pub const ENV_DISABLE_IMAGE_QUALITY_MODEL: &str = "MODERN_FORMAT_DISABLE_IMAGE_QUALITY_MODEL";
/// Developer override to force KNN database lookup for static quality testing.
pub const ENV_FORCE_QUALITY_KNN: &str = "MODERN_FORMAT_FORCE_QUALITY_KNN";
/// Legacy name only — runtime uses [`ENV_DISABLE_LOOP_HDBSCAN_FUSION`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_LOOP_HDBSCAN_FUSION=1 to relax"
)]
pub const ENV_ENABLE_LOOP_HDBSCAN_FUSION: &str = "MODERN_FORMAT_ENABLE_LOOP_HDBSCAN_FUSION";
/// Kill-switch: skip HDBSCAN cluster fusion (pure HNSW vote when loop KNN
/// otherwise succeeds).
pub const ENV_DISABLE_LOOP_HDBSCAN_FUSION: &str = "MODERN_FORMAT_DISABLE_LOOP_HDBSCAN_FUSION";
/// Override [`LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS`] (positive integer, e.g. `2`).
pub const ENV_LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS: &str =
    "MODERN_FORMAT_LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS";
/// Kill-switch for KNN disagreement fusion (default **on** for all quality
/// pipelines).
pub const ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD: &str =
    "MODERN_FORMAT_DISABLE_QUALITY_KNN_DISAGREE_GUARD";
/// Legacy name only — use [`ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_QUALITY_KNN_DISAGREE_GUARD=1 to relax"
)]
pub const ENV_ENABLE_STATIC_QUALITY_KNN_DISAGREE_GUARD: &str =
    "MODERN_FORMAT_ENABLE_STATIC_QUALITY_KNN_DISAGREE_GUARD";
/// Legacy name only — use [`ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_QUALITY_KNN_DISAGREE_GUARD=1 to relax"
)]
pub const ENV_ENABLE_SCENARIO_QUALITY_KNN_DISAGREE_GUARD: &str =
    "MODERN_FORMAT_ENABLE_SCENARIO_QUALITY_KNN_DISAGREE_GUARD";
/// Opt-in only: corrupt/empty `loop_intent` `feature_stats` may use bootstrap
/// defaults (default **off**).
pub const ENV_LOOP_FEATURE_STATS_FAIL_OPEN: &str = "MODERN_FORMAT_LOOP_FEATURE_STATS_FAIL_OPEN";
/// Kill-switch: force fail-closed `feature_stats` even when
/// [`ENV_LOOP_FEATURE_STATS_FAIL_OPEN`] is set.
pub const ENV_DISABLE_LOOP_FEATURE_STATS_FAIL_OPEN: &str =
    "MODERN_FORMAT_DISABLE_LOOP_FEATURE_STATS_FAIL_OPEN";
/// Explicit opt-in for quality `inference_log` rows on heuristic/fallback
/// branches. This has no effect unless image-quality heuristics are enabled.
pub const ENV_ENABLE_QUALITY_INFERENCE_HEURISTIC_LOGS: &str =
    "MODERN_FORMAT_ENABLE_QUALITY_INFERENCE_HEURISTIC_LOGS";
/// Audit-safe alias for the explicit quality-inference-log opt-in.
pub const QUALITY_INFERENCE_HEURISTIC_LOG_ENV_KEY: &str =
    ENV_ENABLE_QUALITY_INFERENCE_HEURISTIC_LOGS;
/// Kill-switch: skip `inference_log` inserts on immature/heuristic/fallback
/// quality branches.
pub const ENV_DISABLE_QUALITY_INFERENCE_HEURISTIC_LOGS: &str =
    "MODERN_FORMAT_DISABLE_QUALITY_INFERENCE_HEURISTIC_LOGS";
/// Legacy redundant with default strict corpus; kept for compatibility.
pub const ENV_STRICT_ALGORITHM_CORPUS: &str = "MODERN_FORMAT_STRICT_ALGORITHM_CORPUS";
/// Kill-switch: relax loop/quality corpus maturity to base floors (50/15 loop,
/// 40/15 quality).
pub const ENV_DISABLE_STRICT_ALGORITHM_CORPUS: &str =
    "MODERN_FORMAT_DISABLE_STRICT_ALGORITHM_CORPUS";
/// Optional override for loop KNN minimum total samples (must be ≥ active
/// floor).
pub const ENV_MIN_GIF_SAMPLES_TOTAL: &str = "MODERN_FORMAT_MIN_GIF_SAMPLES_TOTAL";
/// Optional override for loop KNN minimum per-class samples (must be ≥ active
/// floor).
pub const ENV_MIN_GIF_SAMPLES_PER_CLASS: &str = "MODERN_FORMAT_MIN_GIF_SAMPLES_PER_CLASS";
/// Optional override for static/scenario quality KNN minimum total samples
/// (must be ≥ active floor).
pub const ENV_MIN_QUALITY_SAMPLES_TOTAL: &str = "MODERN_FORMAT_MIN_QUALITY_SAMPLES_TOTAL";
/// Optional override for static/scenario quality KNN minimum per-class samples
/// (must be ≥ active floor).
pub const ENV_MIN_QUALITY_SAMPLES_PER_CLASS: &str = "MODERN_FORMAT_MIN_QUALITY_SAMPLES_PER_CLASS";
/// Legacy name only — use [`ENV_DISABLE_SCENARIO_QUALITY_DB_FUSION`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_SCENARIO_QUALITY_DB_FUSION=1 to relax"
)]
pub const ENV_ENABLE_SCENARIO_QUALITY_DB_FUSION: &str =
    "MODERN_FORMAT_ENABLE_SCENARIO_QUALITY_DB_FUSION";
/// Kill-switch: scenario (animated/video) DB quality fusion at detection.
pub const ENV_DISABLE_SCENARIO_QUALITY_DB_FUSION: &str =
    "MODERN_FORMAT_DISABLE_SCENARIO_QUALITY_DB_FUSION";
/// Legacy name only — use [`ENV_DISABLE_STATIC_QUALITY_DB_FUSION`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_STATIC_QUALITY_DB_FUSION=1 to relax"
)]
pub const ENV_ENABLE_STATIC_QUALITY_DB_FUSION: &str =
    "MODERN_FORMAT_ENABLE_STATIC_QUALITY_DB_FUSION";
/// Kill-switch: static-image DB quality fusion at detection.
pub const ENV_DISABLE_STATIC_QUALITY_DB_FUSION: &str =
    "MODERN_FORMAT_DISABLE_STATIC_QUALITY_DB_FUSION";
/// Legacy name only — use [`ENV_DISABLE_STATIC_QUALITY_DB_LOOKUP`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_STATIC_QUALITY_DB_LOOKUP=1 to relax"
)]
pub const ENV_ENABLE_STATIC_QUALITY_DB_LOOKUP: &str =
    "MODERN_FORMAT_ENABLE_STATIC_QUALITY_DB_LOOKUP";
/// Kill-switch: static / `img` convert quality DB lookup.
pub const ENV_DISABLE_STATIC_QUALITY_DB_LOOKUP: &str =
    "MODERN_FORMAT_DISABLE_STATIC_QUALITY_DB_LOOKUP";
/// Legacy name only — use [`ENV_DISABLE_SCENARIO_QUALITY_DB_LOOKUP`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_SCENARIO_QUALITY_DB_LOOKUP=1 to relax"
)]
pub const ENV_ENABLE_SCENARIO_QUALITY_DB_LOOKUP: &str =
    "MODERN_FORMAT_ENABLE_SCENARIO_QUALITY_DB_LOOKUP";
/// Kill-switch: animated/video scenario quality DB lookup.
pub const ENV_DISABLE_SCENARIO_QUALITY_DB_LOOKUP: &str =
    "MODERN_FORMAT_DISABLE_SCENARIO_QUALITY_DB_LOOKUP";
/// Legacy name only — use [`ENV_DISABLE_EXPLORATION_ALGORITHM_SEAL`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_EXPLORATION_ALGORITHM_SEAL=1 to relax"
)]
pub const ENV_ENABLE_EXPLORATION_ALGORITHM_SEAL: &str =
    "MODERN_FORMAT_ENABLE_EXPLORATION_ALGORITHM_SEAL";
/// Kill-switch: skip exploration output sealing (unit clamp still applies in
/// [`crate::algorithm_seal`]).
pub const ENV_DISABLE_EXPLORATION_ALGORITHM_SEAL: &str =
    "MODERN_FORMAT_DISABLE_EXPLORATION_ALGORITHM_SEAL";
/// Explicit opt-in for Layer 6 HNSW when the decision tree is uncertain.
pub const LOOP_INTENT_LAYER6_KNN_OPT_IN_ENV_KEY: &str =
    "MODERN_FORMAT_LOOP_INTENT_LAYER6_KNN_OPT_IN";
/// Legacy default-on name; runtime code deliberately ignores it.
#[deprecated(
    since = "0.11.4",
    note = "Use MODERN_FORMAT_LOOP_INTENT_LAYER6_KNN_OPT_IN=1 for explicit opt-in"
)]
pub const ENV_ENABLE_LOOP_INTENT_LAYER6_KNN: &str = "MODERN_FORMAT_ENABLE_LOOP_INTENT_LAYER6_KNN";
/// Kill-switch for Layer 6 HNSW; takes precedence over the explicit opt-in.
pub const ENV_DISABLE_LOOP_INTENT_LAYER6_KNN: &str = "MODERN_FORMAT_DISABLE_LOOP_INTENT_LAYER6_KNN";
/// Legacy name only — use [`ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_LOOP_INTENT_INFERENCE_LOG=1 to relax"
)]
pub const ENV_ENABLE_LOOP_INTENT_INFERENCE_LOG: &str =
    "MODERN_FORMAT_ENABLE_LOOP_INTENT_INFERENCE_LOG";
/// Kill-switch: skip loop `inference_log` persistence (requires DB feedback
/// on).
pub const ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG: &str =
    "MODERN_FORMAT_DISABLE_LOOP_INTENT_INFERENCE_LOG";
/// Legacy redundant with default audit-only; kept for compatibility.
pub const ENV_LOOP_INTENT_INFERENCE_AUDIT_ONLY: &str =
    "MODERN_FORMAT_LOOP_INTENT_INFERENCE_AUDIT_ONLY";
/// Kill-switch: persist runtime `final_verdict` in `inference_log` (default:
/// audit-only placeholder).
pub const ENV_DISABLE_LOOP_INTENT_INFERENCE_AUDIT_ONLY: &str =
    "MODERN_FORMAT_DISABLE_LOOP_INTENT_INFERENCE_AUDIT_ONLY";
/// `inference_log.final_verdict` placeholder when audit-only telemetry mode is
/// on.
pub const INFERENCE_TELEMETRY_ONLY_VERDICT: &str = "TelemetryOnly";
/// Loop-table alias for [`INFERENCE_TELEMETRY_ONLY_VERDICT`].
pub const LOOP_INFERENCE_TELEMETRY_ONLY_VERDICT: &str = INFERENCE_TELEMETRY_ONLY_VERDICT;
/// Kill-switch: persist runtime quality `final_verdict` in quality
/// `inference_log` tables.
pub const ENV_DISABLE_QUALITY_INFERENCE_AUDIT_ONLY: &str =
    "MODERN_FORMAT_DISABLE_QUALITY_INFERENCE_AUDIT_ONLY";
/// `FFmpeg` SSIM stderr reports `inf` for identical frames; maps to a perfect
/// parse value.
pub const FFMPEG_SSIM_PERFECT_PARSE_VALUE: f64 = 1.0;
/// SSIM formula degenerate denominator (variance zero) — not a fabricated
/// explore confidence.
pub const SSIM_DEGENERATE_MATCH_VALUE: f64 = 1.0;
/// Legacy name only — use [`ENV_DISABLE_LOOP_INTENT_ALGORITHM_SEAL`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_LOOP_INTENT_ALGORITHM_SEAL=1 to relax"
)]
pub const ENV_ENABLE_LOOP_INTENT_ALGORITHM_SEAL: &str =
    "MODERN_FORMAT_ENABLE_LOOP_INTENT_ALGORITHM_SEAL";
/// Kill-switch: skip loop inference record / audit field sealing (unit clamp
/// still applies).
pub const ENV_DISABLE_LOOP_INTENT_ALGORITHM_SEAL: &str =
    "MODERN_FORMAT_DISABLE_LOOP_INTENT_ALGORITHM_SEAL";
/// Legacy name only — use [`ENV_DISABLE_QUALITY_ALGORITHM_SEAL`] (default
/// **on**).
#[deprecated(
    since = "0.11.4",
    note = "Default-on gate; set MODERN_FORMAT_DISABLE_QUALITY_ALGORITHM_SEAL=1 to relax"
)]
pub const ENV_ENABLE_QUALITY_ALGORITHM_SEAL: &str = "MODERN_FORMAT_ENABLE_QUALITY_ALGORITHM_SEAL";
/// Kill-switch: skip [`crate::image_quality_db::QualityScore::sealed`] mutation
/// (unit clamp still applies).
pub const ENV_DISABLE_QUALITY_ALGORITHM_SEAL: &str = "MODERN_FORMAT_DISABLE_QUALITY_ALGORITHM_SEAL";
// --- Database Maturity Thresholds ---
/// Maximum allowed static image quality samples per quality label class
/// (high/low) in the database.
pub const STATIC_QUALITY_DB_CAP_PER_CLASS: i64 = 4000;
/// Maximum allowed loop intent samples per label class in the database.
pub const LOOP_INTENT_DB_CAP_PER_CLASS: i64 = 2000;
// KNN results are unreliable when training data is too sparse or non-diverse.
// These thresholds gate both the GIF/Video KNN and the static image quality
// KNN.
/// Minimum total labeled samples required for GIF/Video KNN to engage (relaxed
/// floor).
pub const MIN_GIF_SAMPLES_TOTAL: i64 = 50;
/// Minimum samples per class (high/video) for GIF/Video KNN (relaxed floor).
pub const MIN_GIF_SAMPLES_PER_CLASS: i64 = 15;
/// Stricter loop corpus floor (default unless
/// [`ENV_DISABLE_STRICT_ALGORITHM_CORPUS`]).
pub const MIN_GIF_SAMPLES_TOTAL_STRICT: i64 = 150;
/// Stricter loop per-class floor (default unless disable kill-switch).
pub const MIN_GIF_SAMPLES_PER_CLASS_STRICT: i64 = 30;
/// Minimum total labeled samples required for static image KNN to engage
/// (relaxed floor).
pub const MIN_QUALITY_SAMPLES_TOTAL: i64 = 40;
/// Minimum samples per class (high/low) for static image KNN (relaxed floor).
pub const MIN_QUALITY_SAMPLES_PER_CLASS: i64 = 15;
/// Stricter static/scenario corpus total (default unless disable kill-switch).
pub const MIN_QUALITY_SAMPLES_TOTAL_STRICT: i64 = 60;
/// Stricter static/scenario per-class floor (default unless disable
/// kill-switch).
pub const MIN_QUALITY_SAMPLES_PER_CLASS_STRICT: i64 = 25;
/// Kill-switch: allow `quality_passed` when `size_target_met` explicitly
/// failed.
pub const ENV_DISABLE_EXPLORATION_SIZE_TARGET_GATE: &str =
    "MODERN_FORMAT_DISABLE_EXPLORATION_SIZE_TARGET_GATE";
// --- Formats & Extensions ---
/// Modern animated image/container extensions.
pub const MODERN_ANIMATED_EXTENSIONS: &[&str] =
    &["webp", "avif", "apng", "heic", "heif", "hif", "jxl"];
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
/// Above this source size, enable the low-memory x265 profile even when the
/// codec is unknown.
pub const X265_LOW_MEMORY_SOURCE_SIZE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Low-memory x265 profile: serialize frame encoding to cap peak RAM.
pub const X265_LOW_MEMORY_FRAME_THREADS: usize = 1;
/// Low-memory x265 profile: keep lookahead worker fan-out minimal.
pub const X265_LOW_MEMORY_LOOKAHEAD_THREADS: usize = 1;
/// Low-memory x265 profile: avoid per-slice lookahead fan-out on huge masters.
pub const X265_LOW_MEMORY_LOOKAHEAD_SLICES: usize = 1;
/// Low-memory x265 profile: cap worker pools aggressively to keep RAM spikes in
/// check.
pub const X265_LOW_MEMORY_MAX_POOLS: usize = 2;
/// Current HEVC preset policy (`medium`/`slow`/`slower`) must tolerate x265's
/// `slower` preset.
///
/// That preset can use up to 8 consecutive B-frames, so `rc-lookahead` must
/// stay strictly above that count or x265 rejects the encode at startup.
pub const X265_ALLOWED_HEVC_MAX_CONSECUTIVE_BFRAMES: usize = 8;
/// Low-memory x265 profile: shorten the lookahead queue to reduce buffered
/// frames, while still satisfying x265's strict `rc-lookahead > bframes`
/// requirement.
pub const X265_LOW_MEMORY_RC_LOOKAHEAD: usize = X265_ALLOWED_HEVC_MAX_CONSECUTIVE_BFRAMES + 1;
/// Moderate-memory x265 profile: cap worker pools but still leave room to scale
/// on healthy systems.
pub const X265_MODERATE_MEMORY_MAX_POOLS: usize = 6;
/// Moderate-memory x265 profile: allow limited parallelism for systems with
/// adequate RAM.
pub const X265_MODERATE_MEMORY_FRAME_THREADS: usize = 3;
/// Moderate-memory x265 profile: allow limited lookahead parallelism.
pub const X265_MODERATE_MEMORY_LOOKAHEAD_THREADS: usize = 3;
/// Moderate-memory x265 profile: moderate lookahead slice fan-out.
pub const X265_MODERATE_MEMORY_LOOKAHEAD_SLICES: usize = 3;
/// Moderate-memory x265 profile: moderate lookahead queue depth.
pub const X265_MODERATE_MEMORY_RC_LOOKAHEAD: usize = 20;
/// RAM threshold (MB) for permitting the Default (uncapped) x265 profile.
///
/// Permits the profile provided the free-memory ratio also satisfies
/// `X265_DEFAULT_RAM_RATIO_THRESHOLD`. A large absolute amount alone is not
/// sufficient on big-RAM systems where the machine may still be heavily loaded;
/// the ratio gate prevents that silent misclassification.
pub const X265_RELAXED_DEFAULT_RAM_THRESHOLD_MB: u64 = 8 * 1024;
/// Minimum free-memory ratio required to stay on the default x265 profile.
pub const X265_DEFAULT_RAM_RATIO_THRESHOLD: f64 = 0.25;
/// Minimum RAM (MB) required to avoid the aggressive low-memory profile.
pub const X265_MODERATE_RAM_THRESHOLD_MB: u64 = 4 * 1024;
/// Minimum free-memory ratio required to stay above the aggressive low-memory
/// profile.
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
/// Empirical near-widescreen aspect-ratio slack.
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
// Reduced from 0.34: loop_closure_score measures pkt_size autocorrelation
// (codec behavior), not visual loop closure. Abrupt memes have intentionally
// different first/last frames. Retained at low weight as a secondary
// correlation signal for short content only.
pub const FEATURE_WEIGHT_LOOP_CLOSURE: f64 = 0.12;
pub const FEATURE_WEIGHT_MOTION_PERIODICITY: f64 = 0.22;
pub const FEATURE_WEIGHT_LOOP_FREQUENCY: f64 = 0.16;
pub const FEATURE_WEIGHT_SPARSE_CADENCE: f64 = 0.12;
// Reduced from 0.10: temporal_jitter unfairly penalizes abrupt memes with
// intentional frame delay variation (dramatic pause before punchline).
pub const FEATURE_WEIGHT_TEMPORAL_JITTER: f64 = 0.06;
pub const FEATURE_WEIGHT_WEBP_RATIO: f64 = 0.16;
pub const FEATURE_WEIGHT_MOTION_GINI: f64 = 0.14;
pub const FEATURE_WEIGHT_PALETTE_DEPTH: f64 = 0.12;
pub const FEATURE_WEIGHT_TEMPORAL_FLATNESS: f64 = 0.10;
// New zero-cost signals from existing LoopMeta data:
// I-frame ratio: GIF→MP4 encodes produce all-I-frame streams; real video has
// GOP structure.
pub const FEATURE_WEIGHT_IFRAME_RATIO: f64 = 0.30;
// Bytes per frame: GIF-class content has much lower bytes_per_frame than real
// video.
pub const FEATURE_WEIGHT_BYTES_PER_FRAME: f64 = 0.18;
pub const FRAME_COUNT_SHORT_BONUS: f64 = 0.05;
pub const FRAME_COUNT_LONG_PENALTY: f64 = 0.10;
pub const SQUARE_ASPECT_BONUS: f64 = 0.08;
pub const WIDESCREEN_ASPECT_PENALTY: f64 = 0.10;
// 9:16 portrait (TikTok/Reels/Shorts standard) is a strong video signal,
// symmetric with 16:9.
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

// --- Pixel-level Lossless Heuristic (Wave 23) ---
/// Complexity threshold for heuristic lossless detection.
pub const HEURISTIC_LOSSLESS_COMPLEXITY_MAX: f64 = 0.55;
/// Edge density threshold for heuristic lossless detection.
pub const HEURISTIC_LOSSLESS_EDGE_DENSITY_MAX: f64 = 0.35;
/// Color diversity threshold for heuristic lossless detection.
pub const HEURISTIC_LOSSLESS_COLOR_DIVERSITY_MAX: f64 = 0.45;
/// Noise level threshold for heuristic lossless detection (low noise required).
pub const HEURISTIC_LOSSLESS_NOISE_LEVEL_MAX: f64 = 0.15;
/// Minimum confidence required to accept a heuristic lossless verdict.
pub const HEURISTIC_LOSSLESS_CONFIDENCE_MIN: f64 = 0.75;
/// Content types that are highly credible for lossless detection (e.g. digital
/// sources).
pub const HEURISTIC_LOSSLESS_CREDIBLE_TYPES: &[&str] = &[
    "SCREENSHOT",
    "MOBILE_SCREENSHOT",
    "ICON",
    "DOCUMENT",
    "WEB_UI",
    "MAP",
    "ANIMATION",
];

// --- Lossless Affinity Weights (Wave 23) ---
pub const AFFINITY_WEIGHT_COMPLEXITY: f64 = 0.4;
pub const AFFINITY_WEIGHT_NOISE: f64 = 0.3;
pub const AFFINITY_WEIGHT_TEXTURE: f64 = 0.2;
pub const AFFINITY_WEIGHT_COLOR: f64 = 0.1;
pub const AFFINITY_THRESHOLD_LOSSLESS: f64 = 0.85;
pub const AFFINITY_BONUS_CREDIBLE_TYPE: f64 = 0.15;
pub const AFFINITY_BONUS_ALPHA: f64 = 0.05;
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
/// Target CRF for high-quality (archival-grade) sources.
pub const CRF_TARGET_VISUALLY_LOSSLESS: f32 = 18.0;
/// Target CRF for standard quality sources.
pub const CRF_TARGET_STANDARD: f32 = 30.0;
/// Minimum quality score (0-100) to be considered a high-quality master
/// candidate.
pub const QUALITY_SCORE_HIGH_THRESHOLD: u8 = 90;
/// Expected size reduction (%) for JPEG to JXL lossless reconstruction.
pub const EXPECTED_REDUCTION_JXL_LOSSLESS_JPEG: f32 = 15.0;
/// Expected size reduction (%) for general lossless image to JXL conversion.
pub const EXPECTED_REDUCTION_JXL_LOSSLESS_STATIC: f32 = 45.0;
/// Expected size reduction (%) for legacy lossy image to JXL conversion.
pub const EXPECTED_REDUCTION_JXL_LOSSY_STATIC: f32 = 25.0;
/// CRF threshold for "Visually Lossless" classification.
pub const CRF_THRESHOLD_VISUALLY_LOSSLESS: f32 = 15.0;
/// CRF threshold for "High Quality" classification.
pub const CRF_THRESHOLD_HIGH_QUALITY: f32 = 23.0;
/// CRF threshold for "Standard Quality" classification.
pub const CRF_THRESHOLD_STANDARD: f32 = 30.0;
/// Bits Per Pixel (BPP) threshold for "Visually Lossless" classification.
pub const BPP_THRESHOLD_VISUALLY_LOSSLESS: f64 = 2.0;
/// Default minimum SSIM threshold for quality validation.
pub const EXPLORE_DEFAULT_MIN_SSIM: f64 = crate::constants::DEFAULT_MIN_SSIM;

/// Default minimum PSNR threshold for quality validation.
pub const EXPLORE_DEFAULT_MIN_PSNR: f64 = crate::constants::DEFAULT_MIN_PSNR;

/// Default minimum MS-SSIM threshold for quality validation.
pub const EXPLORE_DEFAULT_MIN_MS_SSIM: f64 = crate::constants::DEFAULT_MIN_MS_SSIM;
/// Default maximum iterations for a single CRF exploration.
pub const EXPLORE_DEFAULT_MAX_ITERATIONS: u32 = 12;
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
/// SSIM delta below which the search is considered to have plateaued
/// (converged).
pub const SSIM_PLATEAU_THRESHOLD: f64 = 0.0002;
/// The Golden Ratio (phi) used in search optimization.
pub const PHI: f32 = 0.618;
/// Minimum CRF change rate allowed before search deceleration triggers.
pub const CHANGE_RATE_THRESHOLD: f64 = 0.005;
/// Files below this size are considered "small" for compression verification
/// (10MB).
pub const SMALL_FILE_THRESHOLD_BYTES: u64 = 10 * 1024 * 1024;

pub const MOV_OVERHEAD_PERCENT: f64 = 0.005;
pub const MP4_OVERHEAD_PERCENT: f64 = 0.001;
pub const MKV_OVERHEAD_PERCENT: f64 = 0.0005;
pub const DEFAULT_OVERHEAD_PERCENT: f64 = 0.002;
pub const ULTIMATE_REQUIRED_ZERO_GAINS: u32 = 100;
pub const NORMAL_REQUIRED_ZERO_GAINS: u32 = 4;
pub const LONG_VIDEO_REQUIRED_ZERO_GAINS: u32 = 3;
// --- Additional Quality & Duration Boundaries ---
/// FPS below which an animation is considered "PPT-like" slow-playback.
/// (Duplicated from loop intent section for visibility)
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
pub const EXT_MOV: &str = "mov";
pub const EXT_MP4: &str = "mp4";
pub const EXT_MKV: &str = "mkv";
pub const EXT_WEBP: &str = "webp";
pub const EXT_GIF: &str = "gif";
pub const EXT_JXL: &str = "jxl";
pub const EXT_AVIF: &str = "avif";
pub const EXT_APNG: &str = "apng";
pub const EXT_PNG: &str = "png";
pub const EXT_JPG: &str = "jpg";
pub const EXT_JPEG: &str = "jpeg";
pub const EXT_HEIC: &str = "heic";
pub const EXT_HEIF: &str = "heif";
pub const EXT_TIFF: &str = "tiff";
pub const EXT_TIF: &str = "tif";
pub const EXT_BMP: &str = "bmp";
pub const EXT_ICO: &str = "ico";
pub const EXT_SVG: &str = "svg";
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
pub const TOOL_VMAF: &str = "vmaf";
pub const TOOL_EXIFTOOL: &str = "exiftool";
pub const TOOL_EXIV2: &str = "exiv2";
pub const TOOL_JPEGINFO: &str = "jpeginfo";
pub const TOOL_PNGCHECK: &str = "pngcheck";
pub const TOOL_DWEBP: &str = "dwebp";
pub const TOOL_AVIFDEC: &str = "avifdec";
pub const TOOL_HEIF_INFO: &str = "heif-info";
/// Siegfried format-identification sidecar (PRONOM signatures). Optional
/// capability: an absent `sf` only disables external identification, never
/// the internal detectors.
pub const TOOL_SIEGFRIED: &str = "sf";
pub const TOOL_X265: &str = "x265";
pub const TOOL_AVIFENC: &str = "avifenc";
pub const TOOL_DOVI: &str = "dovi_tool";
pub const TOOL_HDR10PLUS: &str = "hdr10plus_tool";
pub const TOOL_OSASCRIPT: &str = "osascript";
// --- SVT-AV1 Defaults ---
/// Default preset for SVT-AV1 (6 = medium delivery window).
pub const FFMPEG_SVTAV1_DEFAULT_PRESET: &str = "6";
/// Slower delivery-window preset for SVT-AV1 (2 = matches
/// `crate::types::Preset::Slower`).
pub const FFMPEG_SVTAV1_SLOWER_PRESET: &str = "2";
/// Slowest archive preset for SVT-AV1 (0 = matches
/// `crate::types::Preset::Veryslow`).
pub const FFMPEG_SVTAV1_SLOWEST_PRESET: &str = "0";
// --- FFmpeg Command Flags & Arguments ---
pub const FFMPEG_ARG_OVERWRITE: &str = "-y";
pub const FFMPEG_ARG_VERBOSE: &str = "-verbose";
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
pub const JXL_ARG_THREADS: &str = "--num_threads";
pub const JXL_ARG_LOSSLESS_JPEG: &str = "--lossless_jpeg=1";
pub const JXL_ARG_CONTAINER: &str = "--container=1";
pub const JXL_ARG_PROGRESSIVE_DC_DISABLED: &str = "--progressive_dc=0";
pub const JXL_ARG_SYNTHETIC_NOISE_DISABLED: &str = "--photon_noise_iso=0";
pub const JXL_ARG_ALLOW_EXPERT_OPTIONS: &str = "--allow_expert_options";
pub const JXL_ARG_COLOR_SPACE: &str = "color_space";
pub const JXL_ARG_COMPRESS_BOXES: &str = "--compress_boxes=0";
pub const JXL_ARG_ALLOW_JPEG_RECON: &str = "--allow_jpeg_reconstruction";
pub const JXL_ARG_ICC_PATHNAME: &str = "icc_pathname";
// --- JXL Standardized Parameters ---
/// Quality distance for ultimate mode (Limit Mode)
pub const JXL_ULTIMATE_DISTANCE: f32 = 0.001;
/// Effort level for ultimate mode (Limit Mode).
/// e10 is the highest non-experimental production effort accepted by cjxl.
pub const JXL_ULTIMATE_EFFORT: u8 = 10;
/// Default effort level for standard mode
pub const JXL_DEFAULT_EFFORT: u8 = 7;
/// Deep production effort retained for explicit compatibility paths.
pub const JXL_DEEP_EFFORT: u8 = 8;
/// Lossless-only high-effort setting.
pub const JXL_EXPERIMENTAL_LOSSLESS_EFFORT: u8 = 11;
/// Disabled production effort due to the documented e9/e10 efficiency
/// inversion.
pub const JXL_DISABLED_EFFORT: u8 = 9;
/// Runtime JXL policy: default mode emits `e7`; ultimate mode emits production `e10`.
#[must_use]
pub const fn jxl_effort_for_mode(ultimate: bool) -> u8 {
    if ultimate {
        JXL_ULTIMATE_EFFORT
    } else {
        JXL_DEFAULT_EFFORT
    }
}
/// Runtime JXL policy: supports the production efforts used by this project.
#[must_use]
pub const fn is_supported_jxl_effort(effort: u8) -> bool {
    effort == JXL_DEFAULT_EFFORT || effort == JXL_DEEP_EFFORT || effort == JXL_ULTIMATE_EFFORT
}
/// Runtime JXL policy with explicit expert/lab opt-in for e11.
#[must_use]
pub const fn is_supported_jxl_effort_with_expert(effort: u8, allow_expert_options: bool) -> bool {
    is_supported_jxl_effort(effort)
        || (allow_expert_options && effort == JXL_EXPERIMENTAL_LOSSLESS_EFFORT)
}
/// Runtime JXL policy: ultimate mode pins the distance to
/// [`JXL_ULTIMATE_DISTANCE`].
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
/// Set to floor/10 so that the narrowest bracket still resolves a meaningful
/// distance delta.
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
/// Default probability/affinity prior when feature data is missing (0.5 =
/// Neutral).
pub const DEFAULT_SCORE_PRIOR: f64 = 0.5;
/// Default aspect ratio fallback (1.0 = Square).
pub const DEFAULT_ASPECT_RATIO: f64 = 1.0;
/// Default compression ratio fallback for raw/unweighted samples.
pub const DEFAULT_COMPRESSION_RATIO: f64 = 1.0;
/// Default palette size fallback (256 colors).
pub const DEFAULT_PALETTE_SIZE: f64 = 256.0;
/// Default frame complexity/payload fallback.
pub const DEFAULT_COMPLEXITY_PRIOR: f64 = 0.5;
/// Default quality fallback for JPEG files when markers are unreadable (85 =
/// Standard High).
pub const FALLBACK_QUALITY_JPEG: u8 = 85;
/// Default compression level for PNG files when unknown (6 = Medium).
pub const FALLBACK_COMPRESSION_PNG: u8 = 6;
/// Default CRF fallback for video when BPP-to-CRF LUT fails (35 = Safe
/// Standard).
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
pub const HDR_BIT_DEPTH_THRESHOLD: u8 = 10;
/// Quality scoring bonus for HDR/10-bit content.
pub const HDR_QUALITY_BONUS: u8 = 5;
// --- Convergence & Minimum Gain Thresholds ---
/// Minimum consecutive gainless iterations before exit (Ultimate).
pub const ULTIMATE_MIN_GAINS: u32 = 15;
/// Minimum consecutive gainless iterations before exit (Normal).
pub const NORMAL_MIN_GAINS: u32 = 3;
/// Default SSIM fallback value when measurement fails (0.0 = Minimum).
pub const DEFAULT_SSIM_PRIOR: f64 = 0.0;
/// Threshold for "Screen Recording" classification based on BPP.
pub const BPP_THRESHOLD_SCREEN_RECORDING: f64 = 0.1;
/// Threshold for "Animation" classification based on BPP.
pub const BPP_THRESHOLD_ANIMATION_HEURISTIC: f64 = 0.05;
/// Threshold for "Film Grain" classification based on BPP.
pub const BPP_THRESHOLD_FILM_GRAIN: f64 = 0.5;
/// Lower BPP threshold for "Gaming" classification.
pub const BPP_THRESHOLD_GAMING_LOW: f64 = 0.08;
/// Upper BPP threshold for "Gaming" classification.
pub const BPP_THRESHOLD_GAMING_HIGH: f64 = 0.5;
/// Lower BPP threshold for "Live Action" classification.
pub const BPP_THRESHOLD_LIVE_ACTION_LOW: f64 = 0.05;
/// Upper BPP threshold for "Live Action" classification.
pub const BPP_THRESHOLD_LIVE_ACTION_HIGH: f64 = 0.6;
/// FPS threshold for "Gaming" classification.
pub const FPS_THRESHOLD_GAMING: f64 = 50.0;
/// Multiplier for calculating default GOP size from FPS.
pub const GOP_CALC_FPS_MULTIPLIER: f64 = 2.5;
/// Minimum allowed GOP size for default calculation.
pub const GOP_CALC_MIN_LIMIT: f64 = 12.0;
/// Maximum allowed GOP size for default calculation.
pub const GOP_CALC_MAX_LIMIT: f64 = 250.0;
/// Quality score for lossless compression.
pub const QUALITY_SCORE_LOSSLESS: u8 = 100;
/// Quality score for visually lossless compression.
pub const QUALITY_SCORE_VISUALLY_LOSSLESS: u8 = 95;
/// Quality score for high quality compression.
pub const QUALITY_SCORE_HIGH: u8 = 80;
/// Quality score for standard quality compression.
pub const QUALITY_SCORE_STANDARD: u8 = 60;
/// Quality score for low quality compression.
pub const QUALITY_SCORE_LOW: u8 = 40;
/// Quality score bonus for modern efficient codecs.
pub const QUALITY_SCORE_MODERN_CODEC_BONUS: u8 = 3;
/// Silence threshold for audio penetration (dB).
pub const AUDIO_SILENCE_THRESHOLD_DB: f64 = -70.0;
/// Duration threshold for 1-point transparency sampling.
pub const TRANSPARENCY_SAMPLE_POINTS_SHORT_LIMIT: f64 = 1.0;
/// Duration threshold for 2-point transparency sampling.
pub const TRANSPARENCY_SAMPLE_POINTS_MEDIUM_LIMIT: f64 = 5.0;
/// Upper limit for trusting frame count metadata without verification.
pub const FRAME_COUNT_TRUST_UPPER_LIMIT: u64 = 50000;
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
/// Plenty of RAM (low pressure): relaxed tier when ratio/avail above this
/// (wider fast path).
pub const MEMORY_PRESSURE_LOW_RATIO: f64 = 0.24;
pub const MEMORY_PRESSURE_LOW_MIN_MB: u64 = 2560;
/// Normal band floor: below this → `MemoryPressure::High` (sensitive
/// tightening).
pub const MEMORY_PRESSURE_NORMAL_RATIO: f64 = 0.26;
/// Minimum duration for a video to be considered "long" for loop intent
/// (0.05s).
pub const LOOP_INTENT_SHORT_DURATION_THRESHOLD: f64 = 0.05;
/// Default baseline duration for loop inference (4.5s).
pub const LOOP_INTENT_BASELINE_DURATION: f64 = 4.5;
/// Absolute maximum duration for a loop candidate (8.0s).
pub const LOOP_INTENT_MAX_DURATION: f64 = 8.0;
/// Confidence threshold for loop closure detection (0.82).
pub const LOOP_INTENT_CLOSURE_THRESHOLD: f64 = 0.82;
/// Scaling factor for loop closure confidence (1.0 - 0.82 = 0.18).
pub const LOOP_INTENT_CLOSURE_SCALE: f64 = 0.18;
/// Negative threshold for loop closure rejection (0.35).
pub const LOOP_INTENT_CLOSURE_REJECT_THRESHOLD: f64 = 0.35;
/// Confidence threshold for periodicity detection (0.72).
pub const LOOP_INTENT_PERIODICITY_THRESHOLD: f64 = 0.72;
/// Scaling factor for periodicity confidence (1.0 - 0.72 = 0.28).
pub const LOOP_INTENT_PERIODICITY_SCALE: f64 = 0.28;
/// Motion ratio threshold for localized motion detection (0.70).
pub const LOOP_INTENT_ZERO_MOTION_RATIO: f64 = 0.70;
/// Median scaling factor for motion magnitude analysis (0.60).
pub const LOOP_INTENT_MOTION_MEDIAN_SCALE: f64 = 0.60;
/// PNG entropy ratio threshold for high confidence (0.55).
pub const PNG_ENTROPY_RATIO_HIGH_CONFIDENCE: f64 = 0.55;
/// PNG entropy ratio threshold for medium confidence (0.65).
pub const PNG_ENTROPY_RATIO_MEDIUM_CONFIDENCE: f64 = 0.65;
/// Palette size threshold for indexed PNG anomaly detection (64 colors).
pub const PNG_PALETTE_SIZE_ANOMALY_THRESHOLD: f64 = 64.0;
/// Base confidence for high-score image detections (0.9).
pub const IMAGE_DETECTION_CONFIDENCE_BASE_HIGH: f64 = 0.9;
/// Base confidence for low-score image detections (0.5).
pub const IMAGE_DETECTION_CONFIDENCE_BASE_LOW: f64 = 0.5;
/// Bonus increment for each additional positive loop signal (0.06).
pub const LOOP_INTENT_BONUS_INCREMENT: f64 = 0.06;
/// Minimum |log-odds delta| for trace-level signal audit
/// (`log_odds_signal_accumulated`).
pub const LOOP_INTENT_LOG_ODDS_SIGNAL_TRACE_MIN: f64 = 0.25;
/// Default negative bias for loop intent when signals are weak (-0.08).
pub const LOOP_INTENT_NEGATIVE_BIAS_DEFAULT: f64 = -0.08;
/// I-frame ratio threshold for "Static/Loop" classification (0.85).
pub const LOOP_INTENT_IFRAME_RATIO_HIGH: f64 = 0.85;
/// I-frame ratio threshold for "Complex Video" classification (0.15).
pub const LOOP_INTENT_IFRAME_RATIO_LOW: f64 = 0.15;
/// Threshold for rejecting periodicity based on low score (0.32).
pub const LOOP_INTENT_PERIODICITY_REJECT_THRESHOLD: f64 = 0.32;
/// Threshold for high loop frequency confidence (0.75).
pub const LOOP_INTENT_LOOP_FREQ_HIGH: f64 = 0.75;
/// Threshold for low loop frequency rejection (0.25).
pub const LOOP_INTENT_LOOP_FREQ_LOW: f64 = 0.25;
/// Threshold for detecting sparse cadence in packet deltas (0.90).
pub const LOOP_INTENT_SPARSE_CADENCE_THRESHOLD: f64 = 0.90;
/// High jitter threshold for loop intent (0.82).
pub const LOOP_INTENT_JITTER_HIGH: f64 = 0.82;
/// Low jitter threshold for loop intent (0.25).
pub const LOOP_INTENT_JITTER_LOW: f64 = 0.25;
/// Z-score threshold for Bytes-Per-Frame (BPF) analysis (1.5).
pub const LOOP_INTENT_BPF_Z_THRESHOLD: f64 = 1.5;
/// Duration limit for identifying short animation loops (1.5s).
pub const LOOP_INTENT_SHORT_ANIMATION_DURATION_LIMIT: f64 = 1.5;
/// Luminance weight for Red channel (ITU-R BT.601).
pub const RGB_LUMINANCE_WEIGHT_R: f64 = 0.30;
/// Luminance weight for Green channel (ITU-R BT.601).
pub const RGB_LUMINANCE_WEIGHT_G: f64 = 0.59;
/// Luminance weight for Blue channel (ITU-R BT.601).
pub const RGB_LUMINANCE_WEIGHT_B: f64 = 0.11;
/// Lower bound for banding artifact detection ratio (0.08).
pub const PNG_BANDING_RATIO_LOW: f64 = 0.08;
/// Upper bound for banding artifact detection ratio (0.5).
pub const PNG_BANDING_RATIO_HIGH: f64 = 0.5;
/// Ultra-low coverage ratio for color distribution (0.05).
pub const PNG_COVERAGE_RATIO_ULTRA_LOW: f64 = 0.05;
/// Low coverage ratio for color distribution (0.10).
pub const PNG_COVERAGE_RATIO_LOW: f64 = 0.10;
/// Medium coverage ratio for color distribution (0.20).
pub const PNG_COVERAGE_RATIO_MEDIUM: f64 = 0.20;
/// High coverage ratio for color distribution (0.35).
pub const PNG_COVERAGE_RATIO_HIGH: f64 = 0.35;
/// CRF adjustment for Animation content type (+4).
pub const QUALITY_MATCHER_CRF_ADJ_ANIMATION: i8 = 4;
/// CRF adjustment for Screen Recording content type (+5).
pub const QUALITY_MATCHER_CRF_ADJ_SCREEN: i8 = 5;
/// CRF adjustment for Gaming content type (-1).
pub const QUALITY_MATCHER_CRF_ADJ_GAMING: i8 = -1;
/// CRF adjustment for Film Grain content type (-3).
pub const QUALITY_MATCHER_CRF_ADJ_GRAIN: i8 = -3;
/// Minimum safe Bits-Per-Pixel (BPP) for CRF formulas (1e-6).
pub const SAFE_BPP_MIN: f64 = 1e-6;
/// Maximum safe Bits-Per-Pixel (BPP) for CRF formulas (50.0).
pub const SAFE_BPP_MAX: f64 = 50.0;
/// Minimum CRF value for AV1 encoding (0.0).
pub const AV1_CRF_CLAMP_MIN: f32 = 0.0;
/// Maximum CRF value for AV1 encoding (63.0).
pub const AV1_CRF_CLAMP_MAX: f32 = 63.0;
/// Minimum CRF value for HEVC encoding (0.0).
pub const HEVC_CRF_CLAMP_MIN: f32 = 0.0;
/// Maximum CRF value for HEVC encoding (51.0).
pub const HEVC_CRF_CLAMP_MAX: f32 = 51.0;
/// Safety offset when sampling near the end of a file (0.1s).
pub const PENETRATION_SAMPLING_EOF_OFFSET: f64 = 0.1;
/// Minimum duration required to perform stratified sampling (1.0s).
pub const PENETRATION_MIN_SAMPLING_DURATION: f64 = 1.0;
/// Minimum standard deviation for KNN vector normalization (1e-6).
pub const KNN_VECTOR_MIN_STD_DEV: f64 = 1e-6;
/// Minimum feature weight for KNN vector normalization (0.01).
pub const KNN_VECTOR_MIN_WEIGHT: f64 = 0.01;
/// Default per-feature weight when `feature_stats` omits an explicit weight.
pub const KNN_VECTOR_DEFAULT_FEATURE_WEIGHT: f64 = 1.0;
/// Minimum duration used for frame density calculation in KNN vectors (0.05s).
pub const KNN_VECTOR_MIN_DURATION_FOR_DENSITY: f64 = 0.05;
/// Lower limit for FPS normalization in KNN vectors (1e-3).
pub const KNN_VECTOR_FPS_MIN_LIMIT: f64 = 1e-3;
/// VMAF Y-channel sanity floor for high-quality exploration (86.0).
pub const EXPLORATION_VMAF_Y_SANITY_FLOOR: f64 = 86.0;
/// PSNR UV-channel sanity floor for high-quality exploration (30.0).
pub const EXPLORATION_PSNR_UV_SANITY_FLOOR: f64 = 30.0;
/// GPU coarse Phase-2 plateau hint: VMAF-Y already very high (log/heuristic
/// only, not a gate floor).
pub const EXPLORATION_GPU_QUALITY_PLATEAU_VMAF_HINT: f64 = 97.0;
/// GPU coarse Phase-2 plateau hint: mean PSNR-UV already very high
/// (log/heuristic only).
pub const EXPLORATION_GPU_QUALITY_PLATEAU_PSNR_UV_HINT: f64 = 47.0;
/// Gain threshold for stopping exploration when improvement is negligible
/// (0.00005).
pub const EXPLORATION_ZERO_GAIN_THRESHOLD: f64 = 0.00005;
/// Decay factor for step adjustments during binary search (0.4).
pub const EXPLORATION_DECAY_FACTOR: f32 = 0.4;
/// Minimum step size for CRF/distance adjustments (0.1).
pub const EXPLORATION_MIN_STEP: f32 = 0.1;
/// Maximum CAMBI score allowed before rejecting as 'banded' (6.0).
pub const EXPLORATION_CAMBI_MAX: f64 = 6.0;
/// Allowed VMAF drop from baseline for 'High Quality' status (2.0).
pub const EXPLORATION_VMAF_ALLOWED_DROP: f64 = 2.0;
/// Allowed PSNR UV drop from baseline for 'High Quality' status (1.5).
pub const EXPLORATION_PSNR_ALLOWED_DROP: f64 = 1.5;
/// Allowed CAMBI rise for clean sources (1.0).
pub const EXPLORATION_CAMBI_CLEAN_ALLOWED_RISE: f64 = 1.0;
/// Allowed CAMBI rise for banded sources (1.5).
pub const EXPLORATION_CAMBI_BANDED_ALLOWED_RISE: f64 = 1.5;
/// Allowed growth ratio in CAMBI for banded sources (0.15).
pub const EXPLORATION_CAMBI_BANDED_GROWTH_RATIO: f64 = 0.15;
/// Weight of MS-SSIM in quality fusion calculation (0.6).
pub const EXPLORATION_MS_SSIM_WEIGHT: f64 = 0.6;
/// Weight of SSIM-All in quality fusion calculation (0.4).
pub const EXPLORATION_SSIM_ALL_WEIGHT: f64 = 0.4;
/// Fusion score sanity floor for acceptable quality (0.88).
pub const EXPLORATION_FUSION_SANITY_FLOOR: f64 = 0.88;
/// Allowed fusion score drop from baseline (0.04).
pub const EXPLORATION_FUSION_ALLOWED_DROP: f64 = 0.04;
/// Phase 3 downward step for finding quality floors (0.1).
pub const EXPLORATION_PHASE3_DOWNWARD_STEP: f32 = 0.1;
/// Phase 4 maximum probe distance for CRF 0 checks (1.0).
pub const EXPLORATION_PHASE4_MAX_DISTANCE: f32 = 1.0;
/// Minimum step for upward jog when quality targets are missed (0.5).
pub const EXPLORATION_UPWARD_JOG_MIN_STEP: f32 = 0.5;
/// Minimum acceptable SSIM for stream analysis (0.95).
pub const STREAM_ANALYSIS_MIN_SSIM: f64 = 0.95;

// --- JPEG Analysis Constants ---
pub const JPEG_IJG_SCALE_THRESHOLD: f64 = 50.0;
pub const JPEG_IJG_SCALE_FACTOR_LOW: f64 = 5000.0;
pub const JPEG_IJG_SCALE_FACTOR_HIGH_A: f64 = 2.0;
pub const JPEG_IJG_SCALE_FACTOR_HIGH_B: f64 = 200.0;
pub const JPEG_IJG_ROUNDING_OFFSET: f64 = 50.0;
pub const JPEG_IJG_ROUNDING_DIVISOR: f64 = 100.0;

pub const JPEG_CONFIDENCE_LUMA_ONLY: f64 = 0.98;
pub const JPEG_CONFIDENCE_SSE_SCALE: f64 = 0.01;
pub const JPEG_QUALITY_MISMATCH_TOLERANCE: u8 = 2;
pub const JPEG_HIGH_QUALITY_THRESHOLD: u8 = 90;

// --- JPEG Device Fingerprints ---
pub const JPEG_FINGERPRINT_APPLE_HIGH_LUMA_MIN: f64 = 720.0;
pub const JPEG_FINGERPRINT_APPLE_HIGH_LUMA_MAX: f64 = 735.0;
pub const JPEG_FINGERPRINT_APPLE_HIGH_CHROMA_MIN: f64 = 5.0;
pub const JPEG_FINGERPRINT_APPLE_HIGH_CHROMA_MAX: f64 = 12.0;

pub const JPEG_FINGERPRINT_APPLE_VERY_HIGH_LUMA_MIN: f64 = 150.0;
pub const JPEG_FINGERPRINT_APPLE_VERY_HIGH_LUMA_MAX: f64 = 165.0;
pub const JPEG_FINGERPRINT_APPLE_VERY_HIGH_CHROMA_MIN: f64 = 2.0;
pub const JPEG_FINGERPRINT_APPLE_VERY_HIGH_CHROMA_MAX: f64 = 10.0;

pub const JPEG_FINGERPRINT_ANDROID_LUMA_MIN: f64 = 200.0;
pub const JPEG_FINGERPRINT_ANDROID_LUMA_MAX: f64 = 400.0;
pub const JPEG_FINGERPRINT_ANDROID_CHROMA_MIN: f64 = 10.0;
pub const JPEG_FINGERPRINT_ANDROID_CHROMA_MAX: f64 = 50.0;

pub const JPEG_FINGERPRINT_SAMSUNG_LUMA_MIN: f64 = 500.0;
pub const JPEG_FINGERPRINT_SAMSUNG_LUMA_MAX: f64 = 700.0;

pub const JPEG_FINGERPRINT_CUSTOM_THRESHOLD: f64 = 1000.0;
/// Minimum acceptable PSNR for stream analysis (35.0).
pub const STREAM_ANALYSIS_MIN_PSNR: f64 = 35.0;
/// Minimum acceptable MS-SSIM for stream analysis (0.90).
pub const STREAM_ANALYSIS_MIN_MS_SSIM: f64 = 0.90;
/// Duration match threshold for stream integrity verification (0.95).
pub const STREAM_ANALYSIS_DURATION_MATCH_THRESHOLD: f64 = 0.95;
/// Base quality score for Lossless compression (100).
pub const VIDEO_QUALITY_SCORE_LOSSLESS: u8 = 100;
/// Base quality score for Visually Lossless compression (95).
pub const VIDEO_QUALITY_SCORE_VISUALLY_LOSSLESS: u8 = 95;
/// Base quality score for High Quality compression (80).
pub const VIDEO_QUALITY_SCORE_HIGH: u8 = 80;
/// Base quality score for Standard compression (60).
pub const VIDEO_QUALITY_SCORE_STANDARD: u8 = 60;
/// Base quality score for Low Quality compression (40).
pub const VIDEO_QUALITY_SCORE_LOW: u8 = 40;
/// Bonus score for UHD/4K resolution (+3).
pub const VIDEO_QUALITY_RESOLUTION_BONUS_UHD: u8 = 3;
/// Bitrate threshold for high-bitrate H.264 recommendation (50 Mbps).
pub const VIDEO_RECOMMENDATION_HIGH_BITRATE_THRESHOLD: u64 = 50_000_000;
/// Default CRF for video upgrade recommendations (AV1/SVT-AV1).
pub const VIDEO_RECOMMENDATION_AV1_CRF_DEFAULT: f32 = 20.0;
/// Default SVT-AV1 preset for video upgrade recommendations.
pub const VIDEO_RECOMMENDATION_AV1_PRESET_DEFAULT: u8 = 6;
/// Maximum number of finalist candidates promoted for JXL verification (8).
pub const JXL_FINALIST_LIMIT: usize = 8;
/// Perceptual probe anchor at distance 0.03.
pub const JXL_ANCHOR_DIST_0_03: f64 = 0.03;
/// Perceptual probe anchor at distance 0.06.
pub const JXL_ANCHOR_DIST_0_06: f64 = 0.06;
/// Perceptual probe anchor at distance 0.15.
pub const JXL_ANCHOR_DIST_0_15: f64 = 0.15;
/// Perceptual probe anchor at distance 0.20.
pub const JXL_ANCHOR_DIST_0_20: f64 = 0.20;
/// Perceptual probe anchor at distance 0.50.
pub const JXL_ANCHOR_DIST_0_50: f64 = 0.50;
/// Perceptual probe anchor at distance 0.75.
pub const JXL_ANCHOR_DIST_0_75: f64 = 0.75;
/// Minimum probe count for JXL `MicroAdjust` profile (4).
pub const JXL_PROBE_COUNT_MIN_MICRO: usize = 4;
/// Maximum probe count for JXL `MicroAdjust` profile (6).
pub const JXL_PROBE_COUNT_MAX_MICRO: usize = 6;
/// Minimum probe count for JXL `BoundaryPush` profile (5).
pub const JXL_PROBE_COUNT_MIN_BOUNDARY: usize = 5;
/// Maximum probe count for JXL `BoundaryPush` profile (8).
pub const JXL_PROBE_COUNT_MAX_BOUNDARY: usize = 8;
/// Minimum probe count for JXL `WidePush` profile (6).
pub const JXL_PROBE_COUNT_MIN_WIDE: usize = 6;
/// Maximum probe count for JXL `WidePush` profile (10).
pub const JXL_PROBE_COUNT_MAX_WIDE: usize = 10;
/// Minimum probe count for JXL `CeilingSweep` profile (8).
pub const JXL_PROBE_COUNT_MIN_CEILING: usize = 8;
/// Maximum probe count for JXL `CeilingSweep` profile (14).
pub const JXL_PROBE_COUNT_MAX_CEILING: usize = 14;
/// Baseline probe count bonus for JXL adaptive ladder (3).
pub const JXL_PROBE_COUNT_BONUS: usize = 3;
pub const MEMORY_PRESSURE_NORMAL_MIN_MB: u64 = 2560;
/// Host RAM (MB) below which `relaxed` is never selected (stability).
pub const PERF_STABILITY_MIN_TOTAL_RAM_MB_FOR_RELAXED: u64 = 12_288;
/// Hard ceilings so performance boost cannot exceed safe in-flight work.
pub const PERF_STABILITY_MAX_IMAGE_PARALLEL: usize = 16;
pub const PERF_STABILITY_MAX_VIDEO_PARALLEL: usize = 4;
pub const PERF_STABILITY_MAX_CHILD_THREADS: usize = 4;
pub const PERF_STABILITY_MAX_GPU_CONCURRENCY: usize = 5;
pub const PERF_STABILITY_MAX_X265_POOL_THREADS_RELAXED: usize = 12;
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
/// Legacy fixed exploration confidence literals (do not use for new paths; use
/// measured breakdown).
pub const EXPLORE_CONFIDENCE_HIGH: f64 = 0.85;
pub const EXPLORE_CONFIDENCE_NORMAL: f64 = 0.75;
pub const EXPLORE_CONFIDENCE_MEDIUM: f64 = 0.7;
pub const EXPLORE_CONFIDENCE_LOW: f64 = 0.6;
/// Minimum sealed exploration confidence required when `quality_passed`
/// (default gate on).
pub const MIN_EXPLORATION_CONFIDENCE: f64 = 0.5;
/// Kill-switch: allow `quality_passed` without meeting
/// [`MIN_EXPLORATION_CONFIDENCE`]. Kill-switch: relax static conversion gates
/// and strict exploration delivery defaults.
pub const ENV_DISABLE_STRICT_MEDIA_CONVERSION: &str =
    "MODERN_FORMAT_DISABLE_STRICT_MEDIA_CONVERSION";

pub const ENV_DISABLE_EXPLORATION_CONFIDENCE_GATE: &str =
    "MODERN_FORMAT_DISABLE_EXPLORATION_CONFIDENCE_GATE";
/// Kill-switch: allow `quality_passed` without a measured SSIM value on the
/// explore result.
pub const ENV_DISABLE_EXPLORATION_SSIM_PRESENCE_GATE: &str =
    "MODERN_FORMAT_DISABLE_EXPLORATION_SSIM_PRESENCE_GATE";
/// Kill-switch: allow `quality_passed` when measured SSIM is below
/// `actual_min_ssim`.
pub const ENV_DISABLE_EXPLORATION_SSIM_THRESHOLD_GATE: &str =
    "MODERN_FORMAT_DISABLE_EXPLORATION_SSIM_THRESHOLD_GATE";
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
pub const HEIC_MAX_CHILDREN_PER_BOX: u32 = 50000;
pub const HEIC_MAX_MEMORY_LIMIT: u64 = 15 * 1024 * 1024 * 1024;
pub const HEIC_MAX_XMP_SCAN_BYTES: u64 = 100 * MB;
pub const HEIC_XMP_GRAB_BYTES: usize = 65536;

pub const HEIC_PROFILE_MAIN: u8 = 1;
pub const HEIC_PROFILE_MAIN10: u8 = 2;
pub const HEIC_PROFILE_MAIN_STILL: u8 = 3;
pub const HEIC_PROFILE_REXT: u8 = 4;
pub const HEIC_PROFILE_SCC: u8 = 9;

pub const HEIC_CHROMA_420: u8 = 1;
pub const HEIC_CHROMA_422: u8 = 2;
pub const HEIC_CHROMA_444: u8 = 3;

pub const HEIC_LOSSLESS_MIN_BIT_DEPTH: u8 = 12;
pub const HEIC_NAL_UNIT_TYPE_SPS: u8 = 33;
pub const HEIC_NAL_UNIT_TYPE_PPS: u8 = 34;
pub const NEGLIGIBLE_DURATION_F32: f32 = 0.01;
pub const GPU_COARSE_SEARCH_DEFAULT_AUDIO_BITRATE: u64 = 128_000;
pub const LOOP_INTENT_NEUTRAL_CONFIDENCE: f64 = 0.55;
pub const LOOP_INTENT_CLOSURE_HIGH: f64 = 0.82;
pub const LOOP_INTENT_CLOSURE_LOW: f64 = 0.35;
pub const LOOP_INTENT_PERIODICITY_HIGH: f64 = 0.72;
pub const LOOP_INTENT_PERIODICITY_LOW: f64 = 0.32;
pub const LOOP_INTENT_IFRAME_RATIO_TARGET: f64 = 0.50;
pub const LOOP_INTENT_ANTI_LOOP_THRESHOLD: f64 = 0.45;
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
/// Placeholder duration when cadence is degenerate (single-frame); not a
/// measured value.
pub const DURATION_UNKNOWN_PLACEHOLDER_SECS: f32 = 0.0;
pub const DURATION_THRESHOLD_MIN: f32 = 0.01;
pub const FALLBACK_FPS: f32 = 10.0;
pub const JPEG_QUALITY_MAPPING_V1_PSNR_BASE: f64 = 45.0;
pub const JPEG_QUALITY_MAPPING_V1_SSIM_BASE: f64 = 0.98;
pub const F64_EPSILON: f64 = 1e-6;
pub const F32_EPSILON: f32 = 1e-4;
pub const SSIM_EPSILON: f64 = 1e-4;
pub const CRF_EPSILON: f32 = 0.01;
pub const PSNR_EPSILON: f64 = 0.1;
// --- Loop Intent Decision Tree Thresholds (Wave 7) ---
/// Per-MV magnitude threshold for identifying 'zero motion' vectors (0.1).
pub const LOOP_INTENT_ZERO_MV_THRESHOLD: f64 = 0.1;
/// Log-odds bonus for extremely short frame counts (<= 8).
pub const LOOP_INTENT_FRAME_COUNT_SHORT_LIMIT: u64 = 8;
/// Log-odds penalty for extremely long frame counts (> 500).
pub const LOOP_INTENT_FRAME_COUNT_LONG_LIMIT: u64 = 500;
/// Motion periodicity envelope reduction for non-ideal assets (0.70).
pub const LOOP_INTENT_MOTION_ENVELOPE_REDUCTION: f64 = 0.70;
/// I-frame ratio low-veto threshold for identifying real video (0.15).
pub const LOOP_INTENT_IFRAME_RATIO_LOW_VETO: f64 = 0.15;
/// I-frame ratio high-veto threshold for identifying encoded animations
/// (0.85).
pub const LOOP_INTENT_IFRAME_RATIO_HIGH_VETO: f64 = 0.85;
/// Support relief multiplier for motion gini when loop/periodicity is strong
/// (0.35).
pub const LOOP_INTENT_SUPPORT_RELIEF_STRONG: f64 = 0.35;
/// Support relief multiplier for motion gini in short silent assets (0.55).
pub const LOOP_INTENT_SUPPORT_RELIEF_WEAK: f64 = 0.55;
/// Default support relief multiplier for motion gini (0.65).
pub const LOOP_INTENT_SUPPORT_RELIEF_DEFAULT: f64 = 0.65;
/// Semantic score threshold for directory/filename context (0.8).
pub const LOOP_INTENT_SEMANTIC_SCORE_THRESHOLD: f64 = 0.8;
/// FPS anomaly score threshold for loop intent bonus (0.6).
pub const LOOP_INTENT_FPS_ANOMALY_THRESHOLD: f64 = 0.6;
pub const LOOP_INTENT_ZERO_MOTION_HIGH_THRESHOLD: f64 = 0.80;
/// Max color count anomaly score (0.80).
pub const IMAGE_DETECTION_COLOR_COUNT_ANOMALY_MAX: f64 = 0.80;
/// Base confidence for quantized images (0.85).
pub const IMAGE_DETECTION_CONFIDENCE_QUANTIZED: f64 = 0.85;
/// Keep threshold for fusion score (0.45).
pub const LOOP_INTENT_FUSION_KEEP_THRESHOLD: f64 = 0.45;
/// Reject threshold for fusion score (0.35).
pub const LOOP_INTENT_FUSION_REJECT_THRESHOLD: f64 = 0.35;
/// Maximum log-odds bonus for periodicity (0.28).
pub const LOOP_INTENT_PERIODICITY_MAX_BONUS: f64 = 0.28;

// --- Loop Intent Arbitration Deltas (Wave 9) ---
pub const LOOP_INTENT_CLOSURE_REJECT_DELTA: f64 = 0.20;
pub const LOOP_INTENT_PERIODICITY_REJECT_DELTA: f64 = 0.12;
pub const LOOP_INTENT_VIDEO_CONTAINER_SHORT_DELTA: f64 = 0.04;
pub const LOOP_INTENT_VIDEO_CONTAINER_STANDARD_DELTA: f64 = 0.08;
pub const LOOP_INTENT_WIDESCREEN_DELTA: f64 = 0.10;
pub const LOOP_INTENT_SCENE_CUT_DELTA: f64 = 0.20;
pub const LOOP_INTENT_LONG_SILENT_CLIP_DELTA: f64 = 0.14;
pub const LOOP_INTENT_LARGE_VIDEO_ENVELOPE_DELTA: f64 = 0.12;
pub const LOOP_INTENT_AUDIBLE_AUDIO_SHORT_DELTA: f64 = 0.08;
pub const LOOP_INTENT_AUDIBLE_AUDIO_STANDARD_DELTA: f64 = 0.22;

// --- Encoder Efficiency Ratios (Wave 7/8) ---
pub const EFF_RATIO_H264: f64 = 1.0;

// --- KNN Feature Baseline Stats (Wave 8) ---
pub const KNN_STATS_FPS_MEAN: f64 = 12.0;
pub const KNN_STATS_FPS_STD_DEV: f64 = 8.0;
pub const KNN_STATS_BPP_MEAN: f64 = 0.05;
pub const KNN_STATS_BPP_STD_DEV: f64 = 0.05;
pub const KNN_STATS_SPATIAL_BPP_MEAN: f64 = 4.0;
pub const KNN_STATS_SPATIAL_BPP_STD_DEV: f64 = 3.0;
pub const KNN_STATS_VARIATION_MEAN: f64 = 0.5;
pub const KNN_STATS_VARIATION_STD_DEV: f64 = 0.2;
pub const KNN_STATS_WEBP_RATIO_MEAN: f64 = 10.0;
pub const KNN_STATS_WEBP_RATIO_STD_DEV: f64 = 4.0;
pub const KNN_STATS_GINI_MEAN: f64 = 0.55;
pub const KNN_STATS_GINI_STD_DEV: f64 = 0.18;

// --- HDR Synthesis & Color Space (Wave 8) ---
/// Reference white luminance in nits for HDR synthesis (203.0).
pub const HDR_REFERENCE_WHITE_NITS: f32 = 203.0;
/// ISO 21496-1 `GainMap` default offset for SDR (1/64).
pub const HDR_GAINMAP_OFFSET_SDR: f32 = 1.0 / 64.0;
/// ISO 21496-1 `GainMap` default offset for HDR (1/64).
pub const HDR_GAINMAP_OFFSET_HDR: f32 = 1.0 / 64.0;
/// CICP Color Primary ID for Display P3 (12).
pub const COLOR_PRIMARY_P3: u16 = 12;
/// CICP Color Primary ID for BT.709 / sRGB (1).
pub const COLOR_PRIMARY_BT709: u16 = 1;
/// ICC search limit for gainmap detection (1MB).
pub const ICC_SEARCH_LIMIT_BYTES: usize = 1024 * 1024;

// --- Quality Scoring & Tweak Heuristics (Wave 8) ---
/// Neutral quality score for unknown compression (50).
pub const VIDEO_QUALITY_SCORE_NEUTRAL: u8 = 50;
/// BPP threshold for standard-tier quality tweak (0.1).
pub const QUALITY_TWEAK_BPP_STANDARD_MIN: f64 = 0.1;
/// BPP threshold for high-tier quality tweak (0.3).
pub const QUALITY_TWEAK_BPP_HIGH_MIN: f64 = 0.3;
/// Max bonus for BPP quality tweak (+5).
pub const QUALITY_TWEAK_MAX_BONUS: u8 = 5;

// --- Systemic Divisors & Thresholds (Wave 8) ---
/// Standard KB divisor (1024.0).
pub const KB_DIVISOR: f64 = 1024.0;
/// Standard MB divisor (1024^2).
pub const MB_DIVISOR: f64 = 1024.0 * 1024.0;
/// Standard GB divisor (1024^3).
pub const GB_DIVISOR: f64 = 1024.0 * 1024.0 * 1024.0;
/// SSIM grade threshold for 'Excellent' (0.98 - explorer calibrated).
pub const SSIM_GRADE_EXCELLENT: f64 = 0.98;
/// SSIM grade threshold for 'Very Good' (0.97).
pub const SSIM_GRADE_VERY_GOOD: f64 = 0.97;
// --- GIF Block IDs & Conversions (Wave 8) ---
pub const GIF_BLOCK_EXTENSION_INTRODUCER: u8 = 0x21;
pub const GIF_BLOCK_APPLICATION_EXTENSION: u8 = 0xFF;
pub const GIF_BLOCK_GRAPHICS_CONTROL_EXTENSION: u8 = 0xF9;
pub const GIF_BLOCK_IMAGE_DESCRIPTOR: u8 = 0x2C;
pub const GIF_BLOCK_TRAILER: u8 = 0x3B;
pub const GIF_CENTISECONDS_PER_SECOND: f64 = 100.0;

// --- System Safety & Precision (Wave 8) ---
/// Safety headroom for disk space checks (1GB).
pub const DISK_SAFETY_HEADROOM_BYTES: u64 = 1024 * 1024 * 1024;
/// Epsilon for comparing CRF values (0.1).
pub const CRF_COMPARISON_EPSILON: f32 = 0.1;
/// Scaling factor for percentage calculations (100.0).
pub const PERCENTAGE_FACTOR: f64 = 100.0;

// --- Image Analysis & Penetration (Wave 9) ---
/// Max 8-bit value (opaque alpha).
pub const MAX_8BIT_VALUE_F64: f64 = 255.0;
/// Number of channels for YUV/RGB averaging.
pub const CHANNELS_COUNT_F64: f64 = 3.0;
/// Sampling point for mid-point checks (0.5).
pub const SAMPLING_POINT_MID_F64: f64 = 0.5;
/// Interlace detection sample frames (24).
pub const INTERLACE_DETECTION_SAMPLE_FRAMES: u32 = 24;
/// Banding detection horizontal weight (0.70).
pub const PNG_BANDING_WEIGHT_HORIZONTAL: f64 = 0.70;
/// Banding detection diagonal weight (0.30).
pub const PNG_BANDING_WEIGHT_DIAGONAL: f64 = 0.30;
/// Banding detection gradient length threshold (20).
pub const PNG_BANDING_GRADIENT_LENGTH_THRESHOLD: u32 = 20;
/// Banding detection step width threshold (3).
pub const PNG_BANDING_STEP_WIDTH_THRESHOLD: u32 = 3;
/// Banding detection pixel diff threshold (20).
pub const PNG_BANDING_DIFF_THRESHOLD: i16 = 20;
/// Color frequency concentration target (85%).
pub const PNG_COLOR_CONCENTRATION_TARGET_RATIO: f64 = 0.85;
/// Coverage ratio tier 1 threshold (0.10).
pub const PNG_COVERAGE_TIER1_THRESHOLD: f64 = 0.10;
/// Coverage ratio tier 2 threshold (0.20).
pub const PNG_COVERAGE_TIER2_THRESHOLD: f64 = 0.20;
/// Coverage ratio tier 3 threshold (0.35).
pub const PNG_COVERAGE_TIER3_THRESHOLD: f64 = 0.35;
/// Coverage ratio score tier 1 (0.70).
pub const PNG_COVERAGE_TIER1_SCORE: f64 = 0.70;
/// Coverage ratio score tier 2 (0.50).
pub const PNG_COVERAGE_TIER2_SCORE: f64 = 0.50;
/// Coverage ratio score tier 3 (0.25).
pub const PNG_COVERAGE_TIER3_SCORE: f64 = 0.25;
/// Coverage ratio score ultra low (0.85).
pub const PNG_COVERAGE_ULTRA_LOW_SCORE: f64 = 0.85;

// --- Quality Matcher & Complexity (Wave 10) ---
/// Divisor for Spatial Information (SI) calculation (50.0).
pub const SI_DIVISOR: f64 = 50.0;
/// Divisor for Temporal Information (TI) calculation (20.0).
pub const TI_DIVISOR: f64 = 20.0;
/// SI ratio high threshold (1.3).
pub const SI_RATIO_HIGH_THRESHOLD: f64 = 1.3;
/// SI ratio low threshold (0.7).
pub const SI_RATIO_LOW_THRESHOLD: f64 = 0.7;
/// SI adjustment factor for high complexity (1.15).
pub const SI_FACTOR_HIGH: f64 = 1.15;
/// SI adjustment factor for low complexity (0.85).
pub const SI_FACTOR_LOW: f64 = 0.85;
/// TI ratio high threshold (1.5).
pub const TI_RATIO_HIGH_THRESHOLD: f64 = 1.5;
/// TI ratio low threshold (0.5).
pub const TI_RATIO_LOW_THRESHOLD: f64 = 0.5;
/// TI adjustment factor for high complexity (1.10).
pub const TI_FACTOR_HIGH: f64 = 1.10;
/// TI adjustment factor for low complexity (0.90).
pub const TI_FACTOR_LOW: f64 = 0.90;
/// Confidence check: minimum reasonable FPS (1.0).
pub const CONF_FPS_MIN: f64 = 1.0;
/// Confidence check: maximum reasonable FPS (240.0).
pub const CONF_FPS_MAX: f64 = 240.0;
/// Confidence check: minimum reasonable BPP (0.01).
pub const CONF_BPP_MIN: f64 = 0.01;
/// Confidence check: maximum reasonable BPP (5.0).
pub const CONF_BPP_MAX: f64 = 5.0;
/// Chroma factor for 4:4:4 sampling (1.15).
pub const CHROMA_444_FACTOR: f64 = 1.15;
/// Megapixel factor for resolution calculations (1,000,000.0).
pub const MEGAPIXEL_FACTOR: f64 = 1_000_000.0;
/// Neutral score bias for PNG quantization (0.5).
pub const PNG_SCORER_NEUTRAL_BIAS: f64 = 0.5;
/// High-confidence bias for PNG quantization (0.7).
pub const PNG_SCORER_HIGH_CONF_BIAS: f64 = 0.7;
/// Dithering sampling factor (10000.0).
pub const PNG_DITHER_SAMPLING_FACTOR: f64 = 10000.0;
/// Pixel threshold for large image analysis (100,000).
pub const IMAGE_DETECTION_LARGE_PIXEL_THRESHOLD: u64 = 100_000;
/// Pixel threshold for small image analysis (`10_000`).
pub const IMAGE_DETECTION_SMALL_PIXEL_THRESHOLD: u64 = 10_000;
/// Default sample size for image color analysis (`10_000`).
pub const IMAGE_DETECTION_SAMPLING_SIZE: usize = 10_000;
/// Confidence slope for truecolor quantization detection (0.15).
pub const IMAGE_DETECTION_TRUECOLOR_CONF_SLOPE: f64 = 0.15;
/// Threshold for a "strong signal" in image analysis (0.50).
pub const IMAGE_DETECTION_STRONG_SIGNAL_THRESHOLD: f64 = 0.50;
/// Standard 8-bit palette size limit (256).
pub const PNG_PALETTE_SIZE_LIMIT: usize = 256;
/// Extended palette size limit for heuristic checks (512).
pub const PNG_PALETTE_EXTENDED_LIMIT: usize = 512;
/// HDR quality adjustment factor (1.25).
pub const HDR_ADJUSTMENT_FACTOR: f64 = 1.25;
/// BT.2020 color space adjustment factor (1.15).
pub const BT2020_ADJUSTMENT_FACTOR: f64 = 1.15;
/// Default minimum bitrate for database diagnostics (10,000.0).
pub const DB_BITRATE_MIN_DEFAULT: f64 = 10_000.0;
/// Default standard deviation for database heuristics (0.15).
pub const DB_HEURISTIC_STD_DEV_DEFAULT: f64 = 0.15;
/// Default loop closure score for database entries (0.15).
pub const DB_LOOP_CLOSURE_SCORE_DEFAULT: f64 = 0.15;
/// Default memory ratio for x265 profiles (0.15).
pub const X265_MEM_RATIO_DEFAULT: f64 = 0.15;
/// Moderate memory ratio for x265 profiles (0.25).
pub const X265_MEM_RATIO_MODERATE: f64 = 0.25;
/// Low-memory ratio for x265 profiles (0.40).
pub const X265_MEM_RATIO_LOW: f64 = 0.40;
/// Factor for permille/percentage scaling in diagnostics (10,000).
pub const DIAGNOSTIC_SCALING_FACTOR: u128 = 10_000;
/// Floating point equivalent of `DIAGNOSTIC_SCALING_FACTOR` (10,000.0).
pub const DIAGNOSTIC_SCALING_FACTOR_F64: f64 = 10_000.0;
/// Standard bits per byte (8.0).
pub const BITS_PER_BYTE: f64 = 8.0;
/// Bits per byte as u64.
pub const BITS_PER_BYTE_U64: u64 = 8;
/// Standard 512MB file size limit for image analysis.
pub const IMAGE_ANALYSIS_FILE_SIZE_LIMIT: u64 = 512 * 1024 * 1024;
/// Standard 10MB text chunk limit for PNG analysis.
pub const PNG_TEXT_CHUNK_SIZE_LIMIT: usize = 10 * 1024 * 1024;
/// Log-odds bias factor for directional arbitration (0.45).
pub const LOOP_INTENT_DIRECTIONAL_BIAS: f64 = 0.45;
/// Log-odds bias factor for KNN arbitration (0.90).
pub const LOOP_INTENT_KNN_BIAS: f64 = 0.90;
/// Log-odds bias factor for fusion score arbitration (0.95).
pub const LOOP_INTENT_FUSION_BIAS: f64 = 0.95;
/// Minimum log-odds bonus for directional arbitration (0.05).
pub const LOOP_INTENT_DIRECTIONAL_MIN_BONUS: f64 = 0.05;
/// Maximum log-odds bonus for directional arbitration (0.22).
pub const LOOP_INTENT_DIRECTIONAL_MAX_BONUS: f64 = 0.22;
/// Arbitration bonus for square canvas assets (0.08).
pub const LOOP_INTENT_ARBITRATION_SQUARE_BONUS: f64 = 0.08;
/// Arbitration bonus for image-family containers (0.06).
pub const LOOP_INTENT_ARBITRATION_IMAGE_BONUS: f64 = 0.06;
/// Closure reduction scale for directional arbitration (0.20).
pub const LOOP_INTENT_CLOSURE_REDUCTION_SCALE: f64 = 0.20;
/// Periodicity reduction scale for directional arbitration (0.28).
pub const LOOP_INTENT_PERIODICITY_REDUCTION_SCALE: f64 = 0.28;
/// Frequency reduction scale for directional arbitration (0.25).
pub const LOOP_INTENT_FREQ_REDUCTION_SCALE: f64 = 0.25;
/// Maximum log-odds bonus for loop frequency in arbitration (0.12).
pub const LOOP_INTENT_FREQ_MAX_BONUS: f64 = 0.12;
/// Maximum log-odds penalty for loop frequency in arbitration (0.10).
pub const LOOP_INTENT_FREQ_MAX_PENALTY: f64 = 0.10;
/// Minimum log-odds penalty for high frame count assets (0.04).
pub const LOOP_INTENT_FRAME_COUNT_MIN_PENALTY: f64 = 0.04;
/// Maximum log-odds penalty for high frame count assets (0.14).
pub const LOOP_INTENT_FRAME_COUNT_MAX_PENALTY: f64 = 0.14;
/// Divisor for frame count penalty scaling (2000.0).
pub const LOOP_INTENT_FRAME_COUNT_PENALTY_DIVISOR: f64 = 2000.0;
/// FPS threshold for frame count penalty (24.0).
pub const LOOP_INTENT_FRAME_COUNT_FPS_THRESHOLD: f64 = 24.0;

// --- Resolution Presets (Wave 14) ---
pub const RES_FULL_HD_W: u32 = 1920;
pub const RES_FULL_HD_H: u32 = 1080;
pub const RES_QHD_W: u32 = 2560;
pub const RES_QHD_H: u32 = 1440;
pub const RES_4K_W: u32 = 3840;
pub const RES_4K_H: u32 = 2160;
pub const RES_HD_W: u32 = 1280;
pub const RES_HD_H: u32 = 720;
pub const RES_SD_HEIGHT_THRESHOLD: u32 = 576;

// --- Quality Tweak Constants ---
pub const QUALITY_TWEAK_BPP_RANGE: f64 = 0.2;
pub const QUALITY_TWEAK_STANDARD_MAX_TICK: u32 = 5;
pub const QUALITY_TWEAK_HIGH_MAX_TICK: u32 = 3;
pub const QUALITY_TWEAK_HIGH_SCALE: f64 = 3.0;

// --- Exploration & Progress Defaults ---
pub const EXPLORATION_CRF_STEP: f64 = 1.0;
pub const EXPLORATION_PERCENTAGE_MULTIPLIER: f64 = 100.0;
pub const PROGRESS_DEFAULT_DURATION: f64 = 120.0;
pub const PROGRESS_DEFAULT_FRAMES: u64 = 3000;

// --- Fallback & Safety Defaults ---
pub const FALLBACK_CRF_BPP_HEURISTIC: u8 = 35;
pub const VIDEO_NEGLIGIBLE_DURATION: f64 = 0.1;

// --- System & Hash Constants ---
pub const PID_SHIFT_FOR_HASH: u128 = 32;
pub const COLLISION_INDEX_START: usize = 1;

// --- Image Detection Heuristics (Wave 6) ---
/// Max possible color count for indexed PNG (256.0).
pub const PNG_MAX_INDEXED_COLORS: f64 = 256.0;
/// Max possible entropy for RGB images (8.0).
pub const PNG_MAX_RGB_ENTROPY: f64 = 8.0;
/// Entropy anomaly threshold for low-confidence quantization (0.4).
pub const PNG_ENTROPY_ANOMALY_THRESHOLD_LOW: f64 = 0.4;
/// Entropy floor for identifying low-texture images (5.0).
pub const PNG_ENTROPY_LOW_LIMIT: f64 = 5.0;
/// Entropy ratio threshold for high-confidence quantization (0.70).
pub const PNG_ENTROPY_RATIO_HIGH: f64 = 0.70;
/// Minimum pixel count for large entropy analysis (10,000).
pub const PNG_ENTROPY_PIXEL_COUNT_LARGE: u64 = 10_000;
/// Minimum pixel count for medium entropy analysis (5,000).
pub const PNG_ENTROPY_PIXEL_COUNT_MEDIUM: u64 = 5_000;
/// Pixel count threshold for PNG efficiency anomaly detection (100,000).
pub const PNG_EFFICIENCY_PIXEL_COUNT_THRESHOLD: u32 = 100_000;

/// Pixel count threshold for large sampled color expected count (500,000).
pub const SAMPLED_COLORS_PIXELS_LARGE: usize = 500_000;
/// Pixel count threshold for medium sampled color expected count (100,000).
pub const SAMPLED_COLORS_PIXELS_MEDIUM: usize = 100_000;
/// Expected color count for large images (10,000).
pub const SAMPLED_COLORS_EXPECTED_LARGE: usize = 10_000;
/// Expected color count for medium images (5,000).
pub const SAMPLED_COLORS_EXPECTED_MEDIUM: usize = 5_000;
/// Expected color count for small images (1,000).
pub const SAMPLED_COLORS_EXPECTED_SMALL: usize = 1_000;

/// Minimum pixel count for color distribution analysis (100).
pub const COLOR_DIST_MIN_PIXELS: usize = 100;
/// Target sample count for color distribution analysis (50,000).
pub const COLOR_DIST_TARGET_SAMPLES: usize = 50_000;

/// Minimum dimension for gradient banding detection (16).
pub const BANDING_MIN_DIM: u32 = 16;
/// Scan step for horizontal/vertical banding detection (4).
pub const BANDING_SCAN_STEP: usize = 4;
/// Minimum pixel difference for banding step detection (3).
pub const BANDING_DIFF_MIN: i16 = 3;
/// Scan step for diagonal banding detection (8).
pub const BANDING_DIAG_SCAN_STEP: usize = 8;

/// Small PNG file threshold (500KB).
/// PNG files smaller than this will be skipped to avoid overhead.
pub const SMALL_PNG_THRESHOLD_BYTES: u64 = 500 * KB;

/// Initial buffer size for stderr capture (64KB).
pub const STDERR_BUFFER_INITIAL: usize = 64 * 1024;

/// Maximum buffer size for stderr capture (1MB).
pub const STDERR_BUFFER_MAX: usize = 1024 * 1024;

/// Maximum number of stderr lines to capture.
pub const STDERR_MAX_LINES: usize = 100_000;

/// Animation duration threshold (3.0 seconds).
/// Animations shorter than this may be converted to static images.
pub const ANIMATION_DURATION_THRESHOLD_SECS: f32 = 3.0;

/// Default number of threads for fallback.
pub const DEFAULT_FALLBACK_THREADS: usize = 2;

/// Default JXL distance for conservative encoding.
pub const DEFAULT_JXL_DISTANCE: f32 = 1.0;

/// High quality CRF value for HEVC encoding (18.0).
pub const CRF_HIGH_QUALITY: f32 = CRF_TARGET_VISUALLY_LOSSLESS;
/// Standard quality CRF value for HEVC encoding (20.0).
pub const CRF_STANDARD_QUALITY: f32 = CRF_TARGET_STANDARD;
/// Default animation frame delay (100ms).
pub const DEFAULT_ANIMATION_DELAY_MS: u32 = 100;
// --- SSIM Constants ---
pub const SSIM_K1: f64 = 0.01;
pub const SSIM_K2: f64 = 0.03;
pub const SSIM_WINDOW_SIZE: usize = 11;
pub const SSIM_GAUSSIAN_SIGMA: f64 = 1.5;

// --- Video Quality Analysis Thresholds ---
pub const QUALITY_ANALYSIS_SHORT_DURATION_MIN: f64 = 1.0;
pub const QUALITY_ANALYSIS_SAMPLE_RATE_SHORT: usize = 1;
pub const QUALITY_ANALYSIS_SAMPLE_RATE_LONG: usize = 3;
pub const MS_SSIM_CHROMA_MIN_DIM: u32 = 256;
pub const ANIMATED_SSIM_TARGET_MIN: f64 = 0.92;

/// Entropy ratio threshold for medium-confidence quantization (0.40).
pub const PNG_ENTROPY_RATIO_MEDIUM: f64 = 0.40;
/// Base confidence for identified tool signatures in image detection (0.99).
pub const IMAGE_DETECTION_CONFIDENCE_TOOL_SIGNATURE: f64 = 0.99;
/// Base confidence for identified truecolor quantization (0.70).
pub const IMAGE_DETECTION_CONFIDENCE_TRUECOLOR_QUANT: f64 = 0.70;
/// Base confidence for truecolor lossless classification (0.65).
pub const IMAGE_DETECTION_CONFIDENCE_TRUECOLOR_LOSSLESS: f64 = 0.65;
/// Base confidence for truecolor assets with no quantization indicators (0.90).
pub const IMAGE_DETECTION_CONFIDENCE_TRUECOLOR_INDICATORS_NONE: f64 = 0.90;
/// High final score threshold for image detection (0.70).
pub const IMAGE_DETECTION_FINAL_SCORE_HIGH: f64 = 0.70;
/// Medium final score threshold for image detection (0.30).
pub const IMAGE_DETECTION_FINAL_SCORE_MEDIUM: f64 = 0.30;
/// Confidence scaling factor for high-scoring image detection (0.33).
pub const IMAGE_DETECTION_CONFIDENCE_SCALING_HIGH: f64 = 0.33;
/// Confidence scaling factor for medium-scoring image detection (0.67).
pub const IMAGE_DETECTION_CONFIDENCE_SCALING_MEDIUM: f64 = 0.67;
/// Confidence offset for medium-scoring image detection (0.8).
pub const IMAGE_DETECTION_CONFIDENCE_OFFSET_MEDIUM: f64 = 0.8;
/// Divisor for statistical image detection score fusion (4.0).
pub const DETECTION_STATISTICAL_DIVISOR: f64 = 4.0;
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
pub const GPU_SEARCH_HIGH_COMPLEXITY_BITRATE_THRESHOLD: f64 = 2_500_000.0; // 2.5 Mbps
pub const GPU_SEARCH_ULTIMATE_STEP: f32 = 0.5;
pub const GPU_SEARCH_NORMAL_STEP: f32 = 2.0;

pub const CPU_SEARCH_NARROW_RANGE: f32 = 3.0;
pub const CPU_SEARCH_NORMAL_RANGE: f32 = 15.0;
pub const CPU_SEARCH_EXTENSION_RANGE: f32 = 8.0;

pub const GPU_SEARCH_CEILING_SSIM_THRESHOLD: f64 = 0.97;
pub const GPU_SEARCH_GOOD_SSIM_THRESHOLD: f64 = 0.95;
pub const GPU_SEARCH_LOW_SSIM_CRITICAL_THRESHOLD: f64 = 0.90;
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

// --- SSIM Semantic Levels ---
pub const SSIM_LEVEL_NEAR_LOSSLESS: f64 = 0.999_9;
pub const SSIM_LEVEL_PERFECT: f64 = 0.999;
pub const SSIM_LEVEL_EXCELLENT: f64 = 0.98;
pub const SSIM_LEVEL_VERY_GOOD: f64 = 0.93;
pub const SSIM_LEVEL_GOOD: f64 = 0.89;
pub const SSIM_LEVEL_FAIR: f64 = 0.82;
pub const SSIM_LEVEL_POOR: f64 = 0.70;

// --- Image Detection Estimation Factors ---
pub const JXL_DISTANCE_EST_BPP_FACTOR: f64 = 1.5;
pub const JXL_DISTANCE_EST_OFFSET: f64 = 60.0;
pub const ENTROPY_ANOMALY_MUL_ADD_FACTOR: f64 = 0.08;
pub const ENTROPY_ANOMALY_MUL_ADD_OFFSET: f64 = 0.5;
pub const LOG2_SAFETY_FLOOR: f64 = 0.001;

pub const HEVC_EFFICIENCY_FACTOR: f64 = 3.0;
pub const AVIF_EFFICIENCY_FACTOR: f64 = 3.0;
pub const WEBP_EFFICIENCY_FACTOR: f64 = 1.5;
pub const JPEG_EFFICIENCY_FACTOR: f64 = 1.0;

pub const ENTROPY_QUALITY_BASE: f64 = 7.5;
pub const ENTROPY_ADJ_MIN: f64 = 0.7;
pub const ENTROPY_ADJ_MAX: f64 = 1.3;

// --- PNG Quantized Quality Estimation ---
// Palette-based estimation: log2(palette_size) / log2(256) as base signal
pub const PNG_QUALITY_EST_MIN: f64 = 25.0;
pub const PNG_QUALITY_EST_MAX: f64 = 92.0;
pub const PNG_QUALITY_PALETTE_LOG_BASE: f64 = 8.0;
pub const PNG_QUALITY_PALETTE_WEIGHT: f64 = 0.6;
pub const PNG_QUALITY_ENTROPY_WEIGHT: f64 = 0.4;
// Truecolor quantized: entropy-dominant with factor score penalty
pub const PNG_QUALITY_TRUECOLOR_ENTROPY_WEIGHT: f64 = 0.55;
pub const PNG_QUALITY_TRUECOLOR_FACTOR_WEIGHT: f64 = 0.45;

pub const LOOP_FREQUENCY_HIGH_THRESHOLD: f64 = 0.75;
pub const LOOP_FREQUENCY_LOW_THRESHOLD: f64 = 0.25;

/// Color diversity quantization step (4).
pub const COLOR_DIVERSITY_QUANTIZE_STEP: u8 = 4;
/// Maximum samples for color diversity calculation (10,000).
pub const COLOR_DIVERSITY_MAX_SAMPLES: usize = 10_000;

// --- Frame Probing Limits ---
/// Minimum frame count below which we suspect metadata forgery (usually 1).
pub const FRAME_COUNT_TRUST_LOWER_LIMIT: u64 = 1;
// FRAME_COUNT_TRUST_UPPER_LIMIT is defined elsewhere in this file.

pub const CONTAINER_OVERHEAD_REPORT_THRESHOLD: u64 = 10_000;

// --- Loop Intent Scoring Engine ---
pub const LOOP_INTENT_FREQ_SCORE_VHIGH_THRESHOLD: f64 = 20.0;
pub const LOOP_INTENT_FREQ_SCORE_HIGH_THRESHOLD: f64 = 10.0;
pub const LOOP_INTENT_FREQ_SCORE_MED_THRESHOLD: f64 = 5.0;
pub const LOOP_INTENT_FREQ_SCORE_LOW_THRESHOLD: f64 = 2.0;
pub const LOOP_INTENT_FREQ_SCORE_VHIGH: f64 = 1.0;
pub const LOOP_INTENT_FREQ_SCORE_HIGH: f64 = 0.8;
pub const LOOP_INTENT_FREQ_SCORE_MED: f64 = 0.6;
pub const LOOP_INTENT_FREQ_SCORE_LOW: f64 = 0.4;
pub const LOOP_INTENT_FREQ_SCORE_DEFAULT: f64 = 0.2;
pub const LOOP_INTENT_FREQ_SCORE_NULL: f64 = 0.5;

pub const LOOP_INTENT_DENSITY_LOW_THRESHOLD: f64 = 1.2;
pub const LOOP_INTENT_DENSITY_MED_THRESHOLD: f64 = 3.0;
pub const LOOP_INTENT_DENSITY_HIGH_THRESHOLD: f64 = 6.0;
pub const LOOP_INTENT_DENSITY_LOW_ADJ: f64 = -0.35;
pub const LOOP_INTENT_DENSITY_MED_ADJ: f64 = -0.20;
pub const LOOP_INTENT_DENSITY_HIGH_ADJ: f64 = -0.08;

pub const LOOP_INTENT_SPARSE_CADENCE_DENSITY_THRESHOLD: f64 = 12.0;
pub const LOOP_INTENT_SPARSE_CADENCE_SHORT_SCORE: f64 = 0.98;
pub const LOOP_INTENT_SPARSE_CADENCE_GAP_THRESHOLD: f64 = 0.25;
pub const LOOP_INTENT_SPARSE_CADENCE_GAP_SCORE: f64 = 0.92;
pub const LOOP_INTENT_SPARSE_CADENCE_LONG_DUR: f64 = 4.0;
pub const LOOP_INTENT_SPARSE_CADENCE_LONG_FC: u64 = 12;
pub const LOOP_INTENT_SPARSE_CADENCE_LONG_SCORE: f64 = 0.95;

pub const LOOP_INTENT_SIGNAL_STRENGTH_MIN: f64 = 0.25;
pub const LOOP_INTENT_SIGNAL_STRENGTH_MIN_RELAXED: f64 = 0.15;

pub const LOOP_INTENT_DYN_THRESH_SCALING_LOW: f64 = 0.25;
pub const LOOP_INTENT_DYN_THRESH_SCALING_HIGH: f64 = 0.50;

pub const LOOP_INTENT_NUDGE_ASPECT_1_1: f64 = 0.05;
pub const LOOP_INTENT_NUDGE_ASPECT_16_9: f64 = -0.05;
pub const LOOP_INTENT_NUDGE_RESOLUTION_4K: f64 = -0.08;
pub const LOOP_INTENT_NUDGE_SCENE_CUT: f64 = -0.08;
pub const LOOP_INTENT_NUDGE_LOCALIZED_MOTION: f64 = 0.05;
pub const LOOP_INTENT_NUDGE_CLAMP: f64 = 0.15;

pub const LOOP_INTENT_SCENE_CUT_RATIO: f64 = 5.0;
pub const LOOP_INTENT_LOCALIZED_MOTION_RATIO: f64 = 0.7;
pub const LOOP_INTENT_LETTERBOX_THRESHOLD: f64 = 0.15;
pub const LOOP_INTENT_TEXT_DENSITY_THRESHOLD: f64 = 0.15;
pub const LOOP_INTENT_VARIANCE_THRESHOLD: f64 = 100.0;

pub const LOOP_CONFIDENCE_AUTHORITATIVE: f64 = 1.0;
pub const LOOP_CONFIDENCE_HIGH: f64 = 0.85;
pub const LOOP_CONFIDENCE_MED: f64 = 0.6;
pub const LOOP_CONFIDENCE_LOW: f64 = 0.2;

pub const LOOP_CONFIDENCE_THRESHOLD_FFMPEG: f64 = 0.8;
pub const LOOP_CONFIDENCE_PENALTY_FFMPEG: f64 = 0.1;

pub const FLOAT_EPSILON: f64 = 0.01;

// --- Quality Verification Tolerances ---
pub const VERIFY_DURATION_TOLERANCE_STRICT: f64 = 1.0;
pub const VERIFY_DURATION_TOLERANCE_RELAXED_ANIMATED: f64 = 3.0;

// --- Quality Matcher Adjustment Factors ---
pub const GRAIN_FACTOR_TRUE: f64 = 1.20;
pub const ALPHA_FACTOR_TRUE: f64 = 0.90;
pub const MATCH_MODE_QUALITY_FACTOR: f64 = 1.0;
pub const MATCH_MODE_SIZE_FACTOR: f64 = 0.80;
pub const MATCH_MODE_SPEED_FACTOR: f64 = 0.90;
pub const TARGET_ENCODER_AV1_FACTOR: f64 = 0.50;
pub const TARGET_ENCODER_HEVC_FACTOR: f64 = 0.70;
pub const TARGET_ENCODER_JXL_FACTOR: f64 = 0.80;

// --- Quality Grade Thresholds (Wave 9) ---
pub const SSIM_GRADE_GOOD: f64 = 0.95;
pub const SSIM_GRADE_ACCEPTABLE: f64 = 0.90;
pub const SSIM_GRADE_FAIR: f64 = 0.85;

pub const PSNR_GRADE_EXCELLENT: f64 = 45.0;
pub const PSNR_GRADE_GOOD: f64 = 40.0;
pub const PSNR_GRADE_ACCEPTABLE: f64 = 35.0;
pub const PSNR_GRADE_FAIR: f64 = 30.0;

pub const MS_SSIM_GRADE_EXCELLENT: f64 = 0.95;
pub const MS_SSIM_GRADE_GOOD: f64 = 0.90;
pub const MS_SSIM_GRADE_ACCEPTABLE: f64 = 0.85;
pub const MS_SSIM_GRADE_FAIR: f64 = 0.80;

// --- GPU Acceleration Thresholds ---
pub const GPU_LARGE_FILE_THRESHOLD_BYTES: u64 = 500 * MB;
pub const GPU_VERY_LARGE_FILE_THRESHOLD_BYTES: u64 = 2 * GB;

// --- SSIM Mapping Constants ---
pub const SSIM_MAPPING_PSNR_TOLERANCE: f64 = 0.5;
pub const SSIM_MAPPING_CLAMP_MAX: f64 = 0.99999;

// --- JPEG Quality Estimation Mapping ---
pub const JPEG_EST_Q_LOWEST: u8 = 50;
pub const JPEG_EST_Q_LOW: u8 = 65;
pub const JPEG_EST_Q_MEDIUM: u8 = 75;
pub const JPEG_EST_Q_HIGH: u8 = 85;
pub const JPEG_EST_Q_VERY_HIGH: u8 = 90;
pub const JPEG_EST_Q_ULTRA: u8 = 95;
pub const JPEG_EST_Q_EXCELLENT: u8 = 98;

// --- JPEG Gainmap Candidate Scores ---
pub const JPEG_GAINMAP_SCORE_RELATIVE_OFFSET: f64 = 4000.0;
pub const JPEG_GAINMAP_SCORE_ABSOLUTE_OFFSET: f64 = 3500.0;
pub const JPEG_GAINMAP_SCORE_NEARBY_SCAN: f64 = 2500.0;
pub const JPEG_GAINMAP_SCORE_TAIL_SCAN: f64 = 1500.0;

// --- MS-SSIM Sampling Defaults ---
pub const MSSSIM_DEFAULT_SAMPLED_FRAMES: usize = 1000;

// --- Quality Level Boundaries ---
pub const QUALITY_LEVEL_ULTRA: u8 = 95;
pub const QUALITY_LEVEL_HIGH: u8 = 90;
pub const QUALITY_LEVEL_GOOD: u8 = 80;
pub const QUALITY_LEVEL_MEDIUM: u8 = 70;
pub const QUALITY_LEVEL_LOW: u8 = 60;

// --- Quality Description Thresholds ---
pub const QUALITY_MATCHER_SCAN_LIMIT: usize = 1_048_576;

// --- HDR Synthesis & PQ Transfer ---
pub const HDR_INTENSITY_TARGET_MIN: f32 = 100.0;
pub const HDR_INTENSITY_TARGET_MAX: f32 = 1_000_000.0;
pub const HDR_DIFFUSE_WHITE_NITS: f32 = 203.0;
pub const HDR_MAX_NITS: f32 = 10000.0;

// PQ transfer function constants (ST 2084)
pub const PQ_M1: f32 = 2610.0 / 16384.0;
pub const PQ_M2: f32 = 2523.0 / 32.0;
pub const PQ_C1: f32 = 3424.0 / 4096.0;
pub const PQ_C2: f32 = 2413.0 / 128.0;
pub const PQ_C3: f32 = 2392.0 / 128.0;

// sRGB transfer function constants
pub const SRGB_LINEAR_THRESHOLD: f32 = 0.04045;
pub const SRGB_LINEAR_SLOPE: f32 = 12.92;
pub const SRGB_GAMMA_OFFSET: f32 = 0.055;
pub const SRGB_GAMMA_SCALE: f32 = 1.055;
pub const SRGB_GAMMA_EXP: f32 = 2.4;

// --- ICC Profile / JXL Patching ---
pub const ICC_D50_ILLUMINANT_OFFSET_START: usize = 68;
pub const ICC_D50_ILLUMINANT_OFFSET_END: usize = 80;
pub const ICC_D50_STANDARD_BYTES: [u8; 12] = [
    0x00, 0x00, 0xf6, 0xd6, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xd3, 0x2d,
];

// --- Exploration / Iteration Limits ---
pub const EXPLORATION_ITERATION_LIMIT: u32 = 255;

// --- Video Explorer & Exploration Limits (Wave 11) ---
pub const ULTIMATE_MIN_WALL_HITS: u32 = 15;
pub const ULTIMATE_MAX_WALL_HITS: u32 = 100;
pub const ADAPTIVE_WALL_LOG_BASE: u32 = 8;
pub const MIN_ENCODE_THREADS: usize = 1;
pub const DEFAULT_MAX_ENCODE_THREADS: usize = 4;
pub const SERVER_MAX_ENCODE_THREADS: usize = 16;
pub const EXPLORE_DEFAULT_INITIAL_CRF: f32 = 18.0;
pub const EXPLORE_DEFAULT_MIN_CRF: f32 = 0.0;
pub const EXPLORE_DEFAULT_MAX_CRF: f32 = 51.0;
pub const EXPLORE_DEFAULT_TARGET_RATIO: f64 = 1.0;
pub const LONG_VIDEO_FALLBACK_ITERATIONS: u32 = 150;
pub const VERY_LONG_VIDEO_FALLBACK_ITERATIONS: u32 = 130;

// --- Exploration Confidence Weights (Wave 11) ---
pub const CONFIDENCE_WEIGHT_SAMPLING: f64 = 0.3;
pub const CONFIDENCE_WEIGHT_PREDICTION: f64 = 0.3;
pub const CONFIDENCE_WEIGHT_MARGIN: f64 = 0.2;
pub const CONFIDENCE_WEIGHT_SSIM: f64 = 0.2;

// --- GPU Sampling Positions (Wave 11) ---
pub const GPU_SAMPLE_POS_START: f64 = 0.0;
pub const GPU_SAMPLE_POS_QUARTER: f64 = 0.25;
pub const GPU_SAMPLE_POS_HALF: f64 = 0.50;
pub const GPU_SAMPLE_POS_THREE_QUARTERS: f64 = 0.75;
pub const GPU_SAMPLE_POS_TAIL: f64 = 0.90;
pub const GPU_SAMPLE_SEGMENTS: usize = 5;

// --- JPEG Multi-Picture (MP) Tags & Identifiers (Wave 11) ---
pub const JPEG_MPF_IDENTIFIER: &[u8] = b"MPF\0";
pub const JPEG_XMPF_IDENTIFIER: &[u8] = b"XMPF";
pub const TIFF_BIG_ENDIAN: &[u8] = b"MM\0*";
pub const TIFF_LITTLE_ENDIAN: &[u8] = b"II*\0";
pub const JPEG_TAG_NUMBER_OF_IMAGES: u16 = 0xB001;
pub const JPEG_TAG_MP_ENTRY: u16 = 0xB002;

// --- JPEG Gainmap Scanning (Wave 11) ---
pub const JPEG_GAINMAP_SCAN_WINDOW_MIN: usize = 4096;
pub const JPEG_GAINMAP_SCAN_WINDOW_MAX: usize = 131_072;
pub const JPEG_MAX_GAINMAP_SCAN_CANDIDATES: usize = 48;

// --- SSIM & Precision Limits (Wave 11) ---
pub const SSIM_MIN: f64 = 0.0;
pub const SSIM_MAX: f64 = 1.0;
pub const SSIM_DISPLAY_PRECISION_FULL: usize = 6;
pub const CRF_PRECISION: f32 = 0.25;

// --- UI & Versioning (Wave 11) ---
pub const UI_BAR_WIDTH: usize = 35;
pub const CACHE_SCHEMA_VERSION: i32 = 5;

// --- CRF Cache & Precision (Wave 12) ---
pub const CRF_CACHE_KEY_MULTIPLIER: f64 = 100.0;
pub const CRF_CACHE_MAX_VALID: f64 = 63.99;

// --- Unified CRF Constants (f64, Wave 12) ---
pub const HEVC_CRF_MIN_F64: f64 = 0.0;
pub const HEVC_CRF_MAX_F64: f64 = 51.0;
pub const HEVC_CRF_DEFAULT_F64: f64 = 23.0;
pub const HEVC_CRF_VISUALLY_LOSSLESS_F64: f64 = 18.0;
pub const HEVC_CRF_PRACTICAL_MAX_F64: f64 = 32.0;

pub const AV1_CRF_MIN_F64: f64 = 0.0;
pub const AV1_CRF_MAX_F64: f64 = 63.0;
pub const AV1_CRF_DEFAULT_F64: f64 = 30.0;
pub const AV1_CRF_VISUALLY_LOSSLESS_F64: f64 = 20.0;
pub const AV1_CRF_PRACTICAL_MAX_F64: f64 = 45.0;

pub const VP9_CRF_MIN_F64: f64 = 0.0;
pub const VP9_CRF_MAX_F64: f64 = 63.0;
pub const VP9_CRF_DEFAULT_F64: f64 = 31.0;

pub const X264_CRF_MIN_F64: f64 = 0.0;
pub const X264_CRF_MAX_F64: f64 = 51.0;
pub const X264_CRF_DEFAULT_F64: f64 = 23.0;

// --- Global Exploration Iteration Limits (Wave 12) ---
pub const NORMAL_MAX_ITERATIONS: u32 = 60;
pub const EMERGENCY_MAX_ITERATIONS: u32 = 500;

// --- JXL Specific Thresholds (Wave 12) ---
pub const JXL_BREAK_EVEN_RATIO_PCT: f64 = 105.0;

// --- Common Video Resolutions (Wave 13) ---
pub const RES_FHD_W: u32 = 1920;
pub const RES_FHD_H: u32 = 1080;
// Note: 1280x720 and 3840x2160 are already defined in Wave 10 as RES_HD_W/H and
// RES_4K_W/H.

// --- Exploration & Convergence (Wave 13) ---
pub const EXPLORE_VARIANCE_THRESHOLD: f64 = 1e-6;
pub const EXPLORE_MIN_ITERATIONS_VARIANCE: u32 = 6;
pub const EXPLORE_WINDOW_SIZE: usize = 3;
pub const EXPLORE_MAX_CONTINUED_ITERATIONS: u32 = 20;

// --- GPU Search Phases (Wave 13) ---
pub const PHASE4_MAX_ATTEMPTS: u32 = 32;
pub const PHASE4_MAX_BACKTRACK_RETRIES: u32 = 3;
pub const PHASE4_ULTIMATE_MAX_FINE_FAILURES: u32 = 2;
pub const PHASE5_MAX_TOTAL_ATTEMPTS: u32 = 10;
pub const PHASE5_MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// GPU coarse search: tolerate up to N consecutive compressions before phase
/// transition.
pub const GPU_COARSE_MAX_CONSECUTIVE_COMPRESSIONS: u32 = 3;
/// GPU coarse search: tolerate up to N consecutive encode failures before
/// giving up on this phase.
pub const GPU_COARSE_MAX_CONSECUTIVE_FAILURES: u32 = 2;
/// GPU coarse search: bail when size stops improving for this many upward
/// probes.
pub const GPU_COARSE_UPWARD_SIZE_STAGNATION_THRESHOLD: u32 = 4;
/// GPU coarse search: hard cap on upward-direction switches before aborting the
/// sweep.
pub const GPU_COARSE_UPWARD_DIRECTION_SWITCH_LIMIT: u32 = 15;

/// Precheck FPS sanity ranges. Videos with FPS above
/// `PRECHECK_FPS_THRESHOLD_INVALID` are treated as metadata corruption and
/// rejected.
pub const PRECHECK_FPS_RANGE_NORMAL: (f64, f64) = (1.0, 240.0);
pub const PRECHECK_FPS_RANGE_EXTENDED: (f64, f64) = (240.0, 2000.0);
pub const PRECHECK_FPS_RANGE_EXTREME: (f64, f64) = (2000.0, 10000.0);
pub const PRECHECK_FPS_THRESHOLD_INVALID: f64 = 10000.0;

// --- Checkpoint & Lock Safety (Wave 13) ---
pub const CHECKPOINT_FORMAT_VERSION: u32 = 2;
pub const LOCK_STALE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
pub const LOCK_MAX_RETRIES: u32 = 15;

// --- Miscellaneous Business Logic (Wave 13) ---
pub const WARMUP_DURATION_SECS: f32 = 5.0;
// Note: CHANGE_RATE_THRESHOLD (0.005) already defined in Wave 2.
pub const LOG_TAG_WIDTH_DEFAULT: usize = 28;
pub const LOG_PREFIX_MAX_DISPLAY: usize = 25;

// --- IO & Buffering (Wave 14) ---
pub const DEFAULT_BUFFER_SIZE: usize = 65536;
pub const ISOBMFF_ANIMATED_BRANDS: &[&[u8]] = &[b"avis", b"msf1"];

// --- Percentage & Scaling (Wave 14) ---
pub const PERCENT_SCALE_100: f64 = 100.0;
pub const PERMILLE_TO_PERCENT: f64 = 100.0;

// --- Common Non-Standard Resolutions (Wave 14) ---
pub const RES_SD_W: u32 = 640;
pub const RES_SD_H: u32 = 480;

// --- Timing & Intervals (Wave 14) ---
pub const TICK_RATE_STEADY_MS: u64 = 33;
pub const TICK_RATE_FAST_MS: u64 = 8;
pub const POLLING_INTERVAL_SHORT_MS: u64 = 5;
pub const POLLING_INTERVAL_MEDIUM_MS: u64 = 10;
pub const RETRY_DELAY_LONG_MS: u64 = 100;
pub const GPU_NEGATIVE_CACHE_TTL_SECS: u64 = 5;

// --- Codec & Format Names (Wave 15) ---
pub const STR_UNKNOWN: &str = "unknown";
pub const STR_NONE: &str = "none";
pub const STR_EXACT: &str = "exact";

pub const CS_SRGB_UPPER: &str = "sRGB";
pub const CS_GRAYSCALE: &str = "Grayscale";

pub const VAL_LOSSLESS: &str = "Lossless";
pub const VAL_LOSSY: &str = "Lossy";
pub const VAL_HDR: &str = "HDR";
pub const VAL_SD: &str = "SD";

pub const FFMPEG_LOGLEVEL_ERROR: &str = "error";
pub const FFMPEG_PRINT_FORMAT_JSON: &str = "json";

pub const CS_BT709: &str = "bt709";
pub const CS_BT2020: &str = "bt2020nc";
pub const CS_SRGB: &str = "srgb";
pub const CS_ADOBE_RGB: &str = "adobergb";
pub const CS_GBR: &str = "gbr";
pub const CS_RGB: &str = "rgb";
pub const CS_GBRP: &str = "gbrp";

pub const TRC_SMPTE2084: &str = "smpte2084";
pub const TRC_ARIB_STD_B67: &str = "arib-std-b67";

pub const LIB_X265: &str = "libx265";
pub const LIB_SVTAV1: &str = "libsvtav1";
pub const LIB_AV2: &str = "libav2";
pub const LIB_VVENC: &str = "libvvenc";

pub const AUDIO_CODEC_AAC: &str = "aac";
pub const AUDIO_CODEC_ALAC: &str = "alac";
pub const AUDIO_CODEC_OPUS: &str = "opus";
pub const AUDIO_CODEC_VORBIS: &str = "vorbis";
pub const AUDIO_CODEC_FLAC: &str = "flac";
pub const AUDIO_CODEC_PCM: &str = "pcm";
pub const AUDIO_CODEC_WAV: &str = "wav";
pub const AUDIO_CODEC_COPY: &str = "copy";

pub const BITRATE_256K: &str = "256k";
pub const BITRATE_192K: &str = "192k";

pub const FFMPEG_ARG_FPS_MODE: &str = "-fps_mode";
pub const FFMPEG_VAL_1000: &str = "1000";

pub const PIX_FMT_YUV420P: &str = "yuv420p";
pub const PIX_FMT_YUV420P10LE: &str = "yuv420p10le";
pub const PIX_FMT_YUV422P: &str = "yuv422p";
pub const PIX_FMT_YUV422P10LE: &str = "yuv422p10le";
pub const PIX_FMT_YUV444P: &str = "yuv444p";
pub const PIX_FMT_YUV444P10LE: &str = "yuv444p10le";
pub const PIX_FMT_RGBA: &str = "rgba";
pub const PIX_FMT_RGB24: &str = "rgb24";
pub const PIX_FMT_RGB48LE: &str = "rgb48le";
pub const PIX_FMT_GRAY: &str = "gray";
pub const PIX_FMT_NV12: &str = "nv12";
pub const PIX_FMT_P010LE: &str = "p010le";

pub const CODEC_HEVC: &str = "hevc";
pub const CODEC_AV1: &str = "av1";
pub const CODEC_H264: &str = "h264";
pub const CODEC_VP9: &str = "vp9";
pub const CODEC_VP8: &str = "vp8";
pub const CODEC_PNG: &str = "png";
pub const CODEC_JXL: &str = "jxl";
pub const CODEC_AVIF: &str = "avif";
pub const CODEC_WEBP: &str = "webp";

pub const LABEL_JXL: &str = "JXL";
pub const LABEL_AVIF: &str = "AVIF";
pub const LABEL_WEBP: &str = "WebP";
pub const LABEL_HEVC: &str = "HEVC";
pub const LABEL_AV1: &str = "AV1";
pub const LABEL_PNG: &str = "PNG";
pub const LABEL_JPEG: &str = "JPEG";
pub const LABEL_GIF: &str = "GIF";
pub const LABEL_HEIC: &str = "HEIC";
pub const FORMAT_JPEG: &str = "jpeg";
pub const FORMAT_JPG: &str = "jpg";

// --- Logic Scaling & Thresholds (Wave 15) ---
pub const PERCENT_SCALE: f64 = 100.0;

// --- Global Epsilon & Comparison Limits (Wave 18) ---
pub const EPSILON_DEFAULT: f64 = 0.01;
pub const EPSILON_DEFAULT_F32: f32 = 0.01;
pub const EPSILON_STRICT: f64 = 0.001;
pub const EPSILON_STRICT_F32: f32 = 0.001;
pub const EPSILON_PRECISE: f64 = 0.0001;
pub const EPSILON_PRECISE_F32: f32 = 0.0001;

// --- Additional Logic Thresholds (Wave 18) ---
pub const BPP_MIN_VALID: f64 = 0.01;
pub const FPS_MIN_VALID: f64 = 0.01;
pub const DURATION_MIN_VALID: f64 = 0.001;
pub const SIZE_TOLERANCE_RATIO: f64 = 1.01;
pub const BREAK_EVEN_RATIO_DEFAULT: f64 = 1.05;

// --- Tool Argument Constants (Wave 16) ---
pub const ARG_VERSION: &str = "--version";
pub const ARG_V: &str = "-v";
pub const ARG_VER: &str = "-ver";
pub const ARG_HELP: &str = "--help";

pub const MAGICK_ARG_FORMAT: &str = "-format";
pub const MAGICK_ARG_IDENTIFY: &str = "identify";

pub const WEBPMUX_ARG_INFO: &str = "-info";
pub const WEBPMUX_ARG_GET: &str = "-get";
pub const WEBPMUX_ARG_FRAME: &str = "frame";
pub const WEBPMUX_ARG_LOOP: &str = "-loop";
pub const WEBPMUX_ARG_BGCOLOR: &str = "-bgcolor";
pub const WEBPMUX_ARG_OUTPUT: &str = "-o";

pub const GIFSKI_ARG_FPS: &str = "--fps";
pub const GIFSKI_ARG_QUALITY: &str = "--quality";
pub const GIFSKI_ARG_MOTION_QUALITY: &str = "--motion-quality";
pub const GIFSKI_ARG_LOSSY_QUALITY: &str = "--lossy-quality";
pub const GIFSKI_ARG_WIDTH: &str = "--width";
pub const GIFSKI_ARG_HEIGHT: &str = "--height";
pub const GIFSKI_ARG_REPEAT: &str = "--repeat";

// --- Loop Intent Hierarchical Decision Tree (Wave 19) ---
/// Metadata trust level for GIF (NETSCAPE2.0) and authoritative containers
/// (1.0).
pub const METADATA_TRUST_AUTHORITATIVE: f64 = 1.0;
/// Metadata trust level for modern animated containers (0.85).
pub const METADATA_TRUST_MODERN_ANIMATED: f64 = 0.85;
/// Metadata trust level for standard video containers (0.6).
pub const METADATA_TRUST_STANDARD_VIDEO: f64 = 0.6;
/// Metadata trust level for untrusted or legacy video containers (0.2).
pub const METADATA_TRUST_UNTRUSTED: f64 = 0.2;
/// Metadata trust penalty for generic `FFmpeg` wrappers (0.1).
pub const METADATA_TRUST_PENALTY_LAVF: f64 = 0.1;

/// Standard neutral score for probabilistic arbitration (0.5).
pub const LOOP_INTENT_NEUTRAL_SCORE: f64 = 0.5;
/// Standard ambiguous score for probabilistic arbitration (0.5).
pub const LOOP_INTENT_AMBIGUOUS_SCORE: f64 = 0.5;

/// Interpolation floor for keep probability (0.3).
pub const KEEP_PROB_INTERPOLATION_FLOOR: f64 = 0.3;
/// Interpolation range for keep probability (0.4).
pub const KEEP_PROB_INTERPOLATION_RANGE: f64 = 0.4;

/// Lower clamp for fused probability (0.01).
pub const FUSED_PROB_CLAMP_LOWER: f64 = 0.01;
/// Upper clamp for fused probability (0.99).
pub const FUSED_PROB_CLAMP_UPPER: f64 = 0.99;

// --- Image Quality Sampling Steps (Wave 19) ---
pub const COLOR_DIVERSITY_STEP_LARGE: usize = 20;
pub const COLOR_DIVERSITY_STEP_MEDIUM: usize = 10;
pub const COLOR_DIVERSITY_STEP_NORMAL: usize = 1;

pub const TEXTURE_VARIANCE_STEP_LARGE: usize = 10;
pub const TEXTURE_VARIANCE_STEP_MEDIUM: usize = 5;
pub const TEXTURE_VARIANCE_STEP_NORMAL: usize = 2;

pub const NOISE_LEVEL_STEP_LARGE: usize = 10;
pub const NOISE_LEVEL_STEP_MEDIUM: usize = 5;

// --- Video Explorer & Resource Allocation (Wave 19) ---
pub const PIXELS_720P: u64 = 1280 * 720;
pub const PIXELS_1080P: u64 = 1920 * 1080;
pub const PIXELS_4K: u64 = 3840 * 2160;

pub const THREADS_LOW_RES: usize = 4;
pub const THREADS_MEDIUM_RES: usize = 8;
pub const THREADS_HIGH_RES: usize = 12;

/// Saturation detection threshold for iterations (41.0).
pub const SATURATION_CRF_RANGE_THRESHOLD: f32 = 41.0;
/// Scaling divisor for CRF range iteration adjustment (20.0).
pub const CRF_RANGE_SCALING_DIVISOR: f32 = 20.0;

// --- GPU Acceleration Defaults (Wave 19) ---
pub const GPU_DEFAULT_CONCURRENCY: usize = 4;
pub const BEIJING_TIME_OFFSET_SECS: i32 = 8 * 3600;

// --- Floating Point Luma Coefficients (Wave 19) ---
pub const LUMA_COEFF_R_F64: f64 = 0.299;
pub const LUMA_COEFF_G_F64: f64 = 0.587;
pub const LUMA_COEFF_B_F64: f64 = 0.114;

/// Maximum number of colors in a standard 8-bit palette (256).
pub const PALETTE_MAX_COLORS: u32 = 256;

// --- ISOBMFF Brands (Wave 19) ---
pub const BRAND_HEIC: &[u8] = b"heic";
pub const BRAND_HEIX: &[u8] = b"heix";
pub const BRAND_HEIM: &[u8] = b"heim";
pub const BRAND_HEIS: &[u8] = b"heis";
pub const BRAND_HEVC: &[u8] = b"hevc";
pub const BRAND_HEVX: &[u8] = b"hevx";
pub const BRAND_HEV1: &[u8] = b"hev1";
pub const BRAND_HVC1: &[u8] = b"hvc1";
pub const BRAND_HEIF: &[u8] = b"heif";
pub const BRAND_MIF1: &[u8] = b"mif1";
pub const BRAND_MSF1: &[u8] = b"msf1";
pub const BRAND_AVCI: &[u8] = b"avci";
pub const BRAND_AVCS: &[u8] = b"avcs";
pub const BRAND_AVIF: &[u8] = b"avif";
pub const BRAND_AVIS: &[u8] = b"avis";
pub const BRAND_AVIO: &[u8] = b"avio";
pub const BRAND_MA1B: &[u8] = b"MA1B";
pub const BRAND_MA1A: &[u8] = b"MA1A";
pub const BRAND_MIAF: &[u8] = b"miaf";
pub const BRAND_MIPR: &[u8] = b"miPr";
pub const BRAND_AV01: &[u8] = b"av01";
pub const BRAND_HEVM: &[u8] = b"hevm";
pub const BRAND_HEVS: &[u8] = b"hevs";
pub const BRAND_MP41: &[u8] = b"mp41";
pub const BRAND_MP42: &[u8] = b"mp42";
pub const BRAND_ISOM: &[u8] = b"isom";
pub const BRAND_ISO2: &[u8] = b"iso2";
pub const BRAND_ISO3: &[u8] = b"iso3";
pub const BRAND_ISO4: &[u8] = b"iso4";
pub const BRAND_ISO5: &[u8] = b"iso5";
pub const BRAND_ISO6: &[u8] = b"iso6";
pub const BRAND_ISO7: &[u8] = b"iso7";
pub const BRAND_ISO8: &[u8] = b"iso8";
pub const BRAND_ISO9: &[u8] = b"iso9";
pub const BRAND_QT: &[u8] = b"qt  ";
pub const BRAND_M4V: &[u8] = b"m4v ";
pub const BRAND_M4A: &[u8] = b"m4a ";
pub const BRAND_DASH: &[u8] = b"dash";
pub const BRAND_CMFC: &[u8] = b"cmfc";
pub const BRAND_MP71: &[u8] = b"mp71";
pub const BRAND_AVC1: &[u8] = b"avc1";
pub const BRAND_AVC2: &[u8] = b"avc2";
pub const BRAND_AVC3: &[u8] = b"avc3";
pub const BRAND_MP4V: &[u8] = b"mp4v";
pub const BRAND_3GP4: &[u8] = b"3gp4";
pub const BRAND_3GP5: &[u8] = b"3gp5";
pub const BRAND_3GP6: &[u8] = b"3gp6";
pub const BRAND_MJP2: &[u8] = b"mjp2";
pub const BRAND_HEFB: &[u8] = b"hefb";
pub const BRAND_HEFC: &[u8] = b"hefc";
pub const BRAND_MIF2: &[u8] = b"mif2";
pub const BRAND_M4B: &[u8] = b"m4b ";
pub const BRAND_M4P: &[u8] = b"m4p ";
pub const BRAND_M4R: &[u8] = b"m4r ";
pub const BRAND_3GP1: &[u8] = b"3gp1";
pub const BRAND_3GP2: &[u8] = b"3gp2";
pub const BRAND_3GP3: &[u8] = b"3gp3";
pub const BRAND_3G2A: &[u8] = b"3g2a";
pub const BRAND_3G2B: &[u8] = b"3g2b";
pub const BRAND_3G2C: &[u8] = b"3g2c";
pub const BRAND_KDD1: &[u8] = b"kddi";
pub const BRAND_MJPB: &[u8] = b"mjpb";
pub const BRAND_MMP4: &[u8] = b"mmp4";
pub const BRAND_ROSS: &[u8] = b"ross";
pub const BRAND_DVI: &[u8] = b"dvi ";
pub const BRAND_CRX: &[u8] = b"crx ";
pub const BRAND_PIFF: &[u8] = b"piff";
pub const BRAND_ISC2: &[u8] = b"isc2";
pub const BRAND_ISOA: &[u8] = b"isoa";
pub const BRAND_ISOB: &[u8] = b"isob";
pub const BRAND_ISOC: &[u8] = b"isoc";
pub const BRAND_SVC1: &[u8] = b"svc1";
pub const BRAND_MVC1: &[u8] = b"mvc1";
pub const BRAND_MVC2: &[u8] = b"mvc2";
pub const BRAND_DMB1: &[u8] = b"dmb1";
pub const BRAND_DVC: &[u8] = b"dvc ";
pub const BRAND_DVCP: &[u8] = b"dvcp";
pub const BRAND_DVPP: &[u8] = b"dvpp";
pub const BRAND_DV5P: &[u8] = b"dv5p";
pub const BRAND_DV5N: &[u8] = b"dv5n";
pub const BRAND_DVH5: &[u8] = b"dvh5";
pub const BRAND_DVH6: &[u8] = b"dvh6";
pub const BRAND_DVHP: &[u8] = b"dvhp";
pub const BRAND_DVHE: &[u8] = b"dvhe";
pub const BRAND_DVHQ: &[u8] = b"dvhq";
pub const BRAND_DV6N: &[u8] = b"dv6n";
pub const BRAND_DV6P: &[u8] = b"dv6p";
pub const BRAND_VVCS: &[u8] = b"vvcs";
pub const BRAND_VVC1: &[u8] = b"vvc1";
pub const BRAND_VVI1: &[u8] = b"vvi1";
pub const BRAND_VVCB: &[u8] = b"vvcb";
pub const BRAND_VVCG: &[u8] = b"vvcg";
pub const BRAND_EVC1: &[u8] = b"evc1";
pub const BRAND_LVC1: &[u8] = b"lvc1";
pub const BRAND_AVC5: &[u8] = b"avc5";
pub const BRAND_AVC6: &[u8] = b"avc6";
pub const BRAND_AVC7: &[u8] = b"avc7";
pub const BRAND_AVC8: &[u8] = b"avc8";
pub const BRAND_HVC5: &[u8] = b"hvc5";
pub const BRAND_HVC6: &[u8] = b"hvc6";
pub const BRAND_HVC7: &[u8] = b"hvc7";
pub const BRAND_HVC8: &[u8] = b"hvc8";
pub const BRAND_MP3: &[u8] = b"mp3 ";
pub const BRAND_AC_3: &[u8] = b"ac-3";
pub const BRAND_EC_3: &[u8] = b"ec-3";
pub const BRAND_MLPA: &[u8] = b"mlpa";
pub const BRAND_DTSC: &[u8] = b"dtsc";
pub const BRAND_DTSH: &[u8] = b"dtsh";
pub const BRAND_DTSL: &[u8] = b"dtsl";
pub const BRAND_DTSE: &[u8] = b"dtse";
pub const BRAND_MHA_1: &[u8] = b"mha1";
pub const BRAND_MHA_2: &[u8] = b"mha2";
pub const BRAND_MI11: &[u8] = b"mi11";
pub const BRAND_MI12: &[u8] = b"mi12";
pub const BRAND_MI1Q: &[u8] = b"mi1q";
pub const BRAND_MI1R: &[u8] = b"mi1r";
pub const BRAND_MI21: &[u8] = b"mi21";
pub const BRAND_MI31: &[u8] = b"mi31";
pub const BRAND_AVC4: &[u8] = b"avc4";
pub const BRAND_HVC2: &[u8] = b"hvc2";
pub const BRAND_HVC3: &[u8] = b"hvc3";
pub const BRAND_HVC4: &[u8] = b"hvc4";
pub const BRAND_HEV2: &[u8] = b"hev2";
pub const BRAND_VP08: &[u8] = b"vp08";
pub const BRAND_VP09: &[u8] = b"vp09";
pub const BRAND_AV1: &[u8] = b"av1 ";
pub const BRAND_AV02: &[u8] = b"av02";
pub const BRAND_DVH1: &[u8] = b"dvh1";
pub const BRAND_DVR1: &[u8] = b"dvr1";
pub const BRAND_OVC1: &[u8] = b"ovc1";
pub const BRAND_SIMU: &[u8] = b"simu";
pub const BRAND_DRAC: &[u8] = b"drac";
pub const BRAND_CCFF: &[u8] = b"ccff";
pub const BRAND_MJ2S: &[u8] = b"mj2s";
pub const BRAND_JP2: &[u8] = b"jp2 ";
pub const BRAND_J2K: &[u8] = b"j2k ";
pub const BRAND_JPX: &[u8] = b"jpx ";
pub const BRAND_JPM: &[u8] = b"jpm ";
pub const BRAND_MJD2: &[u8] = b"mjd2";
pub const BRAND_MPX3: &[u8] = b"mpx3";
pub const BRAND_MPX4: &[u8] = b"mpx4";
pub const BRAND_MPXH: &[u8] = b"mpxh";

// --- EBML Magic (MKV/WebM) ---
pub const MAGIC_EBML: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3];

// --- ISOBMFF Boxes ---
pub const BOX_IROT: &[u8] = b"irot";
pub const BOX_IMIR: &[u8] = b"imir";

// --- Magic Headers (Wave 19) ---
pub const JXL_HEADER_SHORT: &[u8] = &[0xFF, 0x0A];
pub const JXL_HEADER_LONG: &[u8] = &[
    0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
];
pub const TIFF_LE: &[u8] = &[0x49, 0x49, 0x2A, 0x00];
pub const TIFF_BE: &[u8] = &[0x4D, 0x4D, 0x00, 0x2A];
pub const BIGTIFF_LE: &[u8] = &[0x49, 0x49, 0x2B, 0x00];
pub const BIGTIFF_BE: &[u8] = &[0x4D, 0x4D, 0x00, 0x2B];
pub const GIFSKI_ARG_FAST: &str = "--fast";
pub const GIFSKI_ARG_OUTPUT: &str = "--output";

pub const AVIFENC_ARG_LOSSLESS: &str = "--lossless";
pub const AVIFENC_ARG_SPEED: &str = "--speed";
pub const AVIFENC_ARG_JOBS: &str = "-j";
/// Unified quality for color, 0-100 where 100 = lossless (avifenc ≥1.2.0).
/// Replaces the legacy --min/--max (0-63) API.
pub const AVIFENC_ARG_QUALITY: &str = "-q";
/// Alpha channel quality, 0-100 where 100 = lossless (avifenc ≥1.2.0).
pub const AVIFENC_ARG_QUALITY_ALPHA: &str = "--qalpha";
pub const AVIFENC_ARG_DEPTH: &str = "--depth";
pub const AVIFENC_ARG_YUV: &str = "--yuv";
pub const AVIFENC_ARG_CICP: &str = "--cicp";
/// Ignore malformed embedded Exif metadata while retaining other AVIF encoder inputs.
pub const AVIFENC_ARG_IGNORE_EXIF: &str = "--ignore-exif";
/// Ignore malformed embedded XMP while retaining other AVIF encoder inputs.
pub const AVIFENC_ARG_IGNORE_XMP: &str = "--ignore-xmp";
/// Ignore incompatible or malformed embedded ICC profiles while retaining other AVIF encoder inputs.
pub const AVIFENC_ARG_IGNORE_ICC: &str = "--ignore-icc";

pub const SIPS_ARG_S: &str = "-s";
pub const SIPS_ARG_FORMAT: &str = "format";
pub const SIPS_ARG_FORMAT_OPTIONS: &str = "formatOptions";
pub const SIPS_ARG_OUT: &str = "--out";

pub const EXIFTOOL_ARG_OVERWRITE_ORIGINAL: &str = "-overwrite_original";
pub const EXIFTOOL_ARG_TAGS_FROM_FILE: &str = "-tagsfromfile";
pub const EXIFTOOL_ARG_ICC_PROFILE: &str = "-icc_profile";
pub const EXIFTOOL_ARG_B: &str = "-b";
pub const EXIFTOOL_ARG_ALL: &str = "-all=";
pub const EXIFTOOL_ARG_P: &str = "-P";
pub const EXIFTOOL_ARG_UNSAFE: &str = "-unsafe";

pub const FFMPEG_ARG_F: &str = "-f";
pub const FFMPEG_ARG_PROFILE_V: &str = "-profile:v";

pub const FFPROBE_ARG_SHOW_STREAMS: &str = "-show_streams";
pub const FFPROBE_ARG_SHOW_FORMAT: &str = "-show_format";
pub const FFPROBE_ARG_SHOW_FRAMES: &str = "-show_frames";
pub const FFPROBE_ARG_SHOW_ENTRIES: &str = "-show_entries";
pub const FFPROBE_ARG_SELECT_STREAMS: &str = "-select_streams";
pub const FFPROBE_ARG_PRINT_FORMAT: &str = "-print_format";
pub const FFPROBE_ARG_READ_INTERVALS: &str = "-read_intervals";
/// CONTRACT: `run_ffprobe_json` frame entries — must include `side_data_list`
/// for HDR10+.
pub const FFPROBE_FRAME_SHOW_ENTRIES: &str = "frame=pict_type,pkt_pts_time,pkt_size,side_data_list";
pub const FFPROBE_ARG_COUNT_FRAMES: &str = "-count_frames";
pub const FFPROBE_ARG_PATTERN_TYPE: &str = "-pattern_type";

// --- Tool Names (Wave 17) ---
pub const TOOL_DOVI_TOOL: &str = "dovi_tool";
pub const TOOL_HDR10PLUS_TOOL: &str = "hdr10plus_tool";
// --- Tool Names (Wave 19) ---
pub const TOOL_PS: &str = "ps";
pub const TOOL_KILL: &str = "kill";
pub const TOOL_HOSTNAME: &str = "hostname";
pub const TOOL_TASKKILL: &str = "taskkill";
pub const TOOL_RSYNC: &str = "rsync";
pub const TOOL_POWERSHELL: &str = "powershell";
pub const TOOL_GETFACL: &str = "getfacl";
pub const TOOL_SETFACL: &str = "setfacl";
pub const TOOL_SYSCTL: &str = "sysctl";
pub const TOOL_VM_STAT: &str = "vm_stat";
pub const TOOL_ATTRIB: &str = "attrib";

// --- Supported Media Extensions (Wave 20) ---
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif", "hif",
    "avif", "bmp", "tga", "ico", "cur", "pnm", "ppm", "pgm", "pbm", "pam", "svg", "svgz", "jp2",
    "j2k", "jxl", "raw", "cr2", "cr3", "nef", "arw", "dng", "orf", "raf", "rw2", "pef", "srw",
    "kdc", "mrw", "erf", "mef", "mos", "crw", "x3f", "wbmp",
];
pub const EXCLUDED_DESIGN_EXTENSIONS: &[&str] =
    &["psd", "psb", "ai", "eps", "pdf", "dds", "hdr", "exr", "pfm"];
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "ts", "mts", "m2ts",
    "m2v", "3gp", "3g2", "ogv", "f4v", "asf", "gif", "webp", "avif", "heic", "heif", "hif", "apng",
    "png", "jxl", "vob", "svi", "m2p", "m2t", "tp", "trp", "divx", "xvid", "rm", "rmvb", "amv",
    "nsv", "roq", "mxf", "dv", "drc",
];

// --- Codec Signature Strings (Wave 20) ---
pub const SIG_VVC: &[&str] = &["vvc", "h266", "h.266"];
pub const SIG_AV2: &[&str] = &["av2", "avm"];
pub const SIG_AV1: &[&str] = &["av1", "svt", "aom", "libaom"];
pub const SIG_HEVC: &[&str] = &["h265", "hevc", "x265", "h.265"];
pub const SIG_VP9: &[&str] = &["vp9"];
pub const SIG_VP8: &[&str] = &["vp8", "libvpx"];
pub const SIG_H264: &[&str] = &["h264", "avc", "x264", "h.264"];
pub const SIG_MPEG4: &[&str] = &["mpeg4", "xvid", "divx", "mp4v"];
pub const SIG_MPEG2: &[&str] = &["mpeg2", "mpeg2video"];
pub const SIG_MPEG1: &[&str] = &["mpeg1", "mpeg1video"];
pub const SIG_WMV: &[&str] = &["wmv", "vc1", "vc-1"];
pub const SIG_THEORA: &[&str] = &["theora"];
pub const SIG_REALVIDEO: &[&str] = &["rv10", "rv20", "rv30", "rv40", "realvideo"];
pub const SIG_FLASH: &[&str] = &["flv", "vp6", "flashsv"];
pub const SIG_PRORES: &[&str] = &["apch", "apcn", "apcs", "apco", "ap4h", "ap4x", "prores"];
pub const SIG_DNX: &[&str] = &["dnxhd", "dnxhr"];
pub const SIG_JPEG2000: &[&str] = &["jpeg2000", "j2k", "jp2", "mjp2"];
pub const SIG_MJPEG: &[&str] = &["mjpeg", "mjpegb", "mjpb"];
pub const SIG_TGA: &[&str] = &["tga", "targa"];
pub const SIG_PNG: &[&str] = &["png", "apng"];

// --- Heuristic Coefficients & Tuning (Wave 20) ---
pub const KINETIC_WEIGHT_BASE: f64 = 1.0;
pub const KINETIC_WEIGHT_ADJ: f64 = 0.5;
pub const LOOP_INTENT_SIGNAL_THRESHOLD: u8 = 3;
pub const LOOP_INTENT_SIGNAL_OFFSET: usize = 2;
pub const INTERLACED_PENALTY_MULTIPLIER: f64 = 2.0;
pub const HEURISTIC_SAFETY_FLOOR: f64 = 0.5;
pub const IQR_SAFETY_FLOOR: f64 = 0.06;

// --- Confidence Defaults (Wave 20) ---
pub const CONFIDENCE_DEFAULT_HIGH: f64 = 0.75;
pub const CONFIDENCE_DEFAULT_MED: f64 = 0.7;
pub const CONFIDENCE_DEFAULT_LOW: f64 = 0.65;
pub const CONFIDENCE_DEFAULT_MIN: f64 = 0.6;

// --- Search & Exploration Parameters (Wave 20) ---
pub const SEARCH_ROUNDING_MULTIPLIER: f32 = 2.0;
pub const SEARCH_OFFSET_FINE: f32 = 0.25;
pub const SEARCH_OFFSET_NORMAL: f32 = 0.5;
pub const SEARCH_PLATEAU_MULTIPLIER: f64 = 2.0;

// --- Quality & Bitrate Thresholds (Wave 20) ---
pub const BPP_UPPER_CAP: f64 = 2.0;
pub const QUALITY_LOWER_FLOOR: f64 = 50.0;
pub const KERNEL_SIZE_3X3: f64 = 9.0;
pub const CONTRAST_NORMALIZATION_FACTOR: f64 = 128.0;

// --- Image Forensics & Perceptual Heuristics (Wave 20) ---
pub const DITHER_FLOYD_STEINBERG_MULTIPLIER: f64 = 5.0;
pub const DITHER_CROSS_DIFF_THRESHOLD: f64 = 40.0;
pub const DITHER_DIAG_RATIO: f64 = 0.5;
pub const DITHER_DENSE_MULTIPLIER: f64 = 4.0;
pub const DITHER_DIFF_MIN: f64 = 30.0;
pub const DITHER_DIFF_MAX: f64 = 100.0;
pub const DITHER_ALTERNATION_THRESHOLD: i32 = 3;
pub const ENTROPY_ANOMALY_UPPER_CLAMP: f64 = 0.75;

// --- Color Science Weights (Wave 20) ---
pub const COLOR_DIFF_WEIGHT_R_BASE: f64 = 2.0;
pub const COLOR_DIFF_WEIGHT_G: f64 = 4.0;
pub const COLOR_DIFF_WEIGHT_B_BASE: f64 = 2.0;
pub const COLOR_DIFF_DIVISOR: f64 = 256.0;

// --- Conversion & Scaling Factors (Wave 20) ---
pub const SCALE_100: f64 = 100.0;
pub const SCALE_1000: f64 = 1000.0;
pub const KB_F64: f64 = 1024.0;
pub const MB_F64: f64 = 1_048_576.0;
pub const PERCENTAGE_FACTOR_U32: u32 = 100;

// --- Graphics & Imaging Fundamentals (Wave 22) ---
pub const ALPHA_OPAQUE: u8 = 255;
pub const CHANNELS_RGBA: usize = 4;
pub const RGBA_ALPHA_OFFSET: usize = 3;

// --- Iteration & Search Limits (Wave 20) ---
pub const SEARCH_ITERATIONS_MAX_COMPRESS: u32 = 8;
pub const SEARCH_ITERATIONS_MAX_QUALITY: u32 = 10;
pub const QUALITY_EST_MIN: f64 = 10.0;
pub const QUALITY_EST_MAX: f64 = 100.0;
pub const QUALITY_EST_BPP_LOG_SCALE: f64 = 12.0;

// --- Common Strings & Format Markers ---
pub const PIX_FMT_PAL8: &str = "pal8";
pub const PIX_FMT_YUVA420P: &str = "yuva420p";
pub const PIX_FMT_GBRAP: &str = "gbrap";

pub const CONTAINER_WEBM: &str = "webm";
pub const CONTAINER_MP4: &str = "mp4";
pub const CONTAINER_GIF: &str = "gif";
pub const CONTAINER_WEBP: &str = "webp";
pub const CONTAINER_APNG: &str = "apng";
pub const CONTAINER_AVIF: &str = "avif";

pub const TAG_SOFTWARE: &str = "software";
pub const TAG_ENCODER: &str = "encoder";

pub const ENCODER_X265: &str = "libx265";
pub const ENCODER_SVT_AV1: &str = "libsvtav1";
pub const ENCODER_AOM_AV1: &str = "libaom-av1";
pub const ENCODER_VP9: &str = "libvpx-vp9";
pub const ENCODER_X264: &str = "libx264";
pub const ENCODER_VP8: &str = "libvpx";

pub const CODEC_H265: &str = "h265";
pub const CODEC_AVC: &str = "avc";
pub const CODEC_HEVC_ALT: &str = "hevc";

pub const EDITOR_PREMIERE: &str = "premiere";
pub const EDITOR_RESOLVE: &str = "resolve";
pub const EDITOR_FINAL_CUT: &str = "final cut";
pub const EDITOR_AVID: &str = "avid";
pub const EDITOR_VEGAS: &str = "vegas";
pub const EDITOR_PHOTOSHOP: &str = "photoshop";
pub const EDITOR_GIPHY: &str = "giphy";
pub const EDITOR_EZGIF: &str = "ezgif";
pub const EDITOR_SCREENTOGIF: &str = "screentogif";
pub const EDITOR_KRITA: &str = "krita";
pub const EDITOR_PROCREATE: &str = "procreate";
pub const EDITOR_CLIP_STUDIO: &str = "clip studio";
pub const EDITOR_LIGHTROOM: &str = "lightroom";
pub const EDITOR_DARKTABLE: &str = "darktable";
pub const EDITOR_CAPTURE_ONE: &str = "capture one";
pub const EDITOR_AFFINITY: &str = "affinity";
pub const EDITOR_PIXELMATOR: &str = "pixelmator";
pub const EDITOR_GIMP: &str = "gimp";
pub const EDITOR_PAINT_NET: &str = "paint.net";
pub const EDITOR_CANVA: &str = "canva";
pub const EDITOR_FIGMA: &str = "figma";
pub const SOFTWARE_LAVF: &str = "lavf";
pub const SOFTWARE_HANDBRAKE: &str = "handbrake";
pub const SOFTWARE_SHANA: &str = "shana";
pub const SOFTWARE_MEGUI: &str = "megui";
pub const SOFTWARE_X264_CLI: &str = "x264.exe";
pub const SOFTWARE_X265_CLI: &str = "x265.exe";
pub const SOFTWARE_AOM_CLI: &str = "aomenc";
pub const SOFTWARE_IMAGEMAGICK: &str = "imagemagick";
pub const SOFTWARE_SIPS: &str = "sips";
pub const SOFTWARE_EXIFTOOL: &str = "exiftool";
pub const SOFTWARE_CAPCUT: &str = "capcut";
pub const SOFTWARE_SHOTCUT: &str = "shotcut";
pub const SOFTWARE_OPENSHOT: &str = "openshot";
pub const SOFTWARE_KDENLIVE: &str = "kdenlive";
pub const SOFTWARE_CLIPCHAMP: &str = "clipchamp";
pub const SOFTWARE_LUMAFUSION: &str = "lumafusion";
pub const SOFTWARE_INSHOT: &str = "inshot";
pub const SOFTWARE_VIVAVIDEO: &str = "vivavideo";
pub const SOFTWARE_QUIK: &str = "quik";
pub const SOFTWARE_SPLICE: &str = "splice";
pub const SOFTWARE_ALIGHT_MOTION: &str = "alight motion";
pub const SOFTWARE_VIDEOSHOW: &str = "videoshow";
pub const SOFTWARE_KINEMASTER: &str = "kinemaster";
pub const SOFTWARE_POWERDIRECTOR: &str = "powerdirector";
pub const SOFTWARE_ACTIONDIRECTOR: &str = "actiondirector";
pub const SOFTWARE_FILMORA: &str = "filmora";
pub const SOFTWARE_PREMIERE_RUSH: &str = "premiere rush";
pub const SOFTWARE_IMOVIE: &str = "imovie";
pub const SOFTWARE_CLIPS: &str = "clips";
pub const SOFTWARE_SNAPSEED: &str = "snapseed";
pub const SOFTWARE_VSCO: &str = "vsco";
pub const SOFTWARE_PICSART: &str = "picsart";
pub const SOFTWARE_MEITU: &str = "meitu";
pub const SOFTWARE_XNIP: &str = "xnip";
pub const SOFTWARE_CLEANSHOT: &str = "cleanshot";

// --- Social & Platform Signatures ---
pub const PLATFORM_TIKTOK: &str = "tiktok";
pub const PLATFORM_INSTAGRAM: &str = "instagram";
pub const PLATFORM_WECHAT: &str = "wechat";
pub const PLATFORM_WHATSAPP: &str = "whatsapp";
pub const PLATFORM_TELEGRAM: &str = "telegram";
pub const PLATFORM_FACEBOOK: &str = "facebook";
pub const PLATFORM_TWITTER: &str = "twitter";
pub const PLATFORM_X: &str = "x.com";
pub const PLATFORM_SNAPCHAT: &str = "snapchat";
pub const PLATFORM_DISCORD: &str = "discord";
pub const PLATFORM_PINTEREST: &str = "pinterest";
pub const PLATFORM_YOUTUBE: &str = "youtube";
pub const PLATFORM_NETFLIX: &str = "netflix";
pub const PLATFORM_DISNEY_PLUS: &str = "disney+";
pub const PLATFORM_AMAZON_PRIME: &str = "amazon prime";

// --- Camera Brands & Manufacturers ---
pub const BRAND_APPLE: &str = "apple";
pub const BRAND_GOOGLE: &str = "google";
pub const BRAND_SONY: &str = "sony";
pub const BRAND_NIKON: &str = "nikon";
pub const BRAND_CANON: &str = "canon";
pub const BRAND_FUJIFILM: &str = "fujifilm";
pub const BRAND_PANASONIC: &str = "panasonic";
pub const BRAND_OLYMPUS: &str = "olympus";
pub const BRAND_LEICA: &str = "leica";
pub const BRAND_SAMSUNG: &str = "samsung";
pub const BRAND_HUAWEI: &str = "huawei";
pub const BRAND_XIAOMI: &str = "xiaomi";
pub const BRAND_OPPO: &str = "oppo";
pub const BRAND_VIVO: &str = "vivo";
pub const BRAND_KODAK: &str = "kodak";
pub const BRAND_PENTAX: &str = "pentax";
pub const BRAND_MINOLTA: &str = "minolta";
pub const BRAND_CASIO: &str = "casio";
pub const BRAND_GOPRO: &str = "gopro";
pub const BRAND_INSTA360: &str = "insta360";
pub const BRAND_DJI: &str = "dji";
pub const BRAND_BLACKMAGIC: &str = "blackmagic";
pub const BRAND_RED_DIGITAL: &str = "red";
pub const BRAND_ARRI: &str = "arri";
pub const BRAND_SIGMA: &str = "sigma";
pub const BRAND_TAMRON: &str = "tamron";
pub const BRAND_TOKINA: &str = "tokina";
pub const BRAND_ZCAM: &str = "z cam";
pub const BRAND_KINEFINITY: &str = "kinefinity";
pub const BRAND_PHASE_ONE: &str = "phase one";
pub const BRAND_HASSELBLAD: &str = "hasselblad";
pub const BRAND_MAMIYA: &str = "mamiya";
pub const BRAND_RICOH: &str = "ricoh";
pub const BRAND_EPSON: &str = "epson";
pub const BRAND_KYOCERA: &str = "kyocera";
pub const BRAND_POLAROID: &str = "polaroid";
pub const BRAND_TOSHIBA: &str = "toshiba";
pub const BRAND_SHARP: &str = "sharp";
pub const BRAND_LG: &str = "lg";
pub const BRAND_MOTOROLA: &str = "motorola";
pub const BRAND_ONEPLUS: &str = "oneplus";
pub const BRAND_REALME: &str = "realme";
pub const BRAND_MEIZU: &str = "meizu";
pub const BRAND_TCL: &str = "tcl";
pub const BRAND_ZTE: &str = "zte";
pub const BRAND_HONOR: &str = "honor";
pub const BRAND_NUBIA: &str = "nubia";
pub const BRAND_ASUS: &str = "asus";
pub const BRAND_NOKIA: &str = "nokia";
pub const BRAND_SINAR: &str = "sinar";
pub const BRAND_LEAF: &str = "leaf";
pub const BRAND_BRONICA: &str = "bronica";
pub const BRAND_CONTAX: &str = "contax";
pub const BRAND_YASHICA: &str = "yashica";
pub const BRAND_PIXLR: &str = "pixlr";
pub const BRAND_FOTOR: &str = "fotor";
pub const BRAND_BEFUNKY: &str = "befunky";
pub const BRAND_POLARR: &str = "polarr";
pub const BRAND_DARKROOM: &str = "darkroom";
pub const BRAND_RAWTHERAPEE: &str = "rawtherapee";
pub const BRAND_LUMINAR: &str = "luminar";
pub const BRAND_DXO: &str = "dxo";
pub const BRAND_TOPAZ: &str = "topaz";
pub const BRAND_REMINI: &str = "remini";
pub const BRAND_ENLIGHT: &str = "enlight";
pub const BRAND_FACETUNE: &str = "facetune";
pub const BRAND_AFTERLIGHT: &str = "afterlight";
pub const BRAND_NOMO: &str = "nomo";
pub const BRAND_HUJI: &str = "huji";
pub const BRAND_FILMIC: &str = "filmic pro";
pub const BRAND_PROCAM: &str = "procam";
pub const BRAND_HALIDE: &str = "halide";
pub const BRAND_SPECTRE: &str = "spectre";
pub const BRAND_MOMENT: &str = "moment";
pub const BRAND_OBSIDIAN: &str = "obsidian";
pub const BRAND_CAMSCANNER: &str = "camscanner";
pub const BRAND_SCANBOT: &str = "scanbot";
pub const BRAND_TINYSCAN: &str = "tinyscan";
pub const BRAND_MICROSOFT_LENS: &str = "office lens";
pub const BRAND_GOOGLE_SCAN: &str = "photoscan";
pub const BRAND_ADOBE_SCAN: &str = "adobe scan";
pub const BRAND_JVC: &str = "jvc";
pub const BRAND_SANYO: &str = "sanyo";
pub const BRAND_HITACHI: &str = "hitachi";
pub const BRAND_VIVITAR: &str = "vivitar";
pub const BRAND_AGFA: &str = "agfa";
pub const BRAND_TUYA: &str = "tuya";
pub const BRAND_EZVIZ: &str = "ezviz";
pub const BRAND_REOLINK: &str = "reolink";
pub const BRAND_SWANN: &str = "swann";
pub const BRAND_NIGHT_OWL: &str = "night owl";
pub const BRAND_INVIDEO: &str = "invideo";
pub const BRAND_FLEXCLIP: &str = "flexclip";
pub const BRAND_KAPWING: &str = "kapwing";
pub const BRAND_ANIMAKER: &str = "animaker";
pub const BRAND_RENDERFOREST: &str = "renderforest";
pub const BRAND_BITEABLE: &str = "biteable";
pub const BRAND_LUMEN5: &str = "lumen5";
pub const BRAND_NUIX: &str = "nuix";
pub const BRAND_GUIDANCE: &str = "guidance software";
pub const BRAND_ACCESSDATA: &str = "accessdata";
pub const BRAND_NAVER: &str = "naver";
pub const BRAND_LINE: &str = "line";
pub const BRAND_KAKAO: &str = "kakao";
pub const BRAND_BILIBILI: &str = "bilibili";
pub const BRAND_DOUYIN: &str = "douyin";
pub const BRAND_KUAISHOU: &str = "kuaishou";
pub const BRAND_XIAOHONGSHU: &str = "xiaohongshu";
// Removed duplicate red.com entry to prevent shadowing
pub const BRAND_WEIBO: &str = "weibo";
pub const BRAND_BAIDU: &str = "baidu";
pub const BRAND_TENCENT: &str = "tencent";
pub const BRAND_ALIBABA: &str = "alibaba";
pub const BRAND_IQIYI: &str = "iqiyi";
pub const BRAND_ZHIHU: &str = "zhihu";
pub const BRAND_VIMEO: &str = "vimeo";
pub const BRAND_TWITCH: &str = "twitch";
pub const BRAND_DAILYMOTION: &str = "dailymotion";
pub const BRAND_ADOBE_AE: &str = "after effects";
pub const BRAND_ADOBE_AU: &str = "audition";
pub const BRAND_ADOBE_AME: &str = "media encoder";
pub const BRAND_ADOBE_BR: &str = "bridge";
pub const BRAND_MAYA: &str = "maya";
pub const BRAND_MAX: &str = "3ds max";
pub const BRAND_BLENDER: &str = "blender";
pub const BRAND_HOUDINI: &str = "houdini";
pub const BRAND_CINEMA4D: &str = "cinema 4d";
pub const BRAND_ZBRUSH: &str = "zbrush";
pub const BRAND_SUBSTANCE: &str = "substance";
pub const BRAND_UNITY: &str = "unity";
pub const BRAND_UNREAL: &str = "unreal";
pub const BRAND_OBS: &str = "obs studio";
pub const BRAND_STREAMLABS: &str = "streamlabs";
pub const BRAND_XSPLIT: &str = "xsplit";
pub const BRAND_BANDICAM: &str = "bandicam";
pub const BRAND_FRAPS: &str = "fraps";
pub const BRAND_SHADOWPLAY: &str = "shadowplay";
pub const BRAND_RELIVE: &str = "relive";
pub const BRAND_VLC: &str = "vlc";
pub const BRAND_POTPLAYER: &str = "potplayer";
pub const BRAND_MPC_HC: &str = "mpc-hc";
pub const BRAND_IINA: &str = "iina";
pub const BRAND_INFUSE: &str = "infuse";
pub const BRAND_PLEX: &str = "plex";
pub const BRAND_KODI: &str = "kodi";
pub const BRAND_ARLO: &str = "arlo";
pub const BRAND_RING: &str = "ring";
pub const BRAND_NEST: &str = "nest";
pub const BRAND_WYZE: &str = "wyze";
pub const BRAND_EUFY: &str = "eufy";
pub const BRAND_HIKVISION: &str = "hikvision";
pub const BRAND_DAHUA: &str = "dahua";
pub const BRAND_AXIS: &str = "axis";
pub const BRAND_LOREX: &str = "lorex";
pub const BRAND_AMCREST: &str = "amcrest";
pub const BRAND_VIVOTEK: &str = "vivotek";
pub const BRAND_HANWHA: &str = "hanwha";
pub const BRAND_MOBOTIX: &str = "mobotix";
pub const BRAND_PELCO: &str = "pelco";
pub const BRAND_BOSCH: &str = "bosch";
pub const BRAND_UBIQUITI: &str = "ubiquiti";
pub const BRAND_SYNOLOGY: &str = "synology";
pub const BRAND_QNAP: &str = "qnap";
pub const BRAND_PLAYSTATION: &str = "playstation";
pub const BRAND_XBOX: &str = "xbox";
pub const BRAND_NINTENDO: &str = "nintendo";
pub const BRAND_STEAM_DECK: &str = "steam deck";
pub const BRAND_CELLEBRITE: &str = "cellebrite";
pub const BRAND_MAGNET: &str = "magnet axiom";
pub const BRAND_ENCASE: &str = "encase";
pub const BRAND_FTK: &str = "ftk";
pub const BRAND_X_WAYS: &str = "x-ways";
pub const BRAND_ACELAB: &str = "acelab";
pub const BRAND_OXYGEN: &str = "oxygen forensics";
pub const BRAND_MSAB: &str = "msab";
pub const BRAND_BELKASOFT: &str = "belkasoft";

// --- Environment Variables ---
pub const ENV_VERBOSE: &str = "IMGQUALITY_VERBOSE";
pub const ENV_DEBUG: &str = "IMGQUALITY_DEBUG";
pub const ENV_MFB_HOME_ROOT: &str = "MFB_HOME_ROOT";
/// Default dot-directory under `$HOME` for persistent MFB state (logs, cache
/// layout).
pub const MFB_DEFAULT_HOME_DIRNAME: &str = ".modern_format_boost";
pub const ENV_MFB_PROGRESS_DIR: &str = "MFB_PROGRESS_DIR";
pub const ENV_MFB_LOG_DIR: &str = "MFB_LOG_DIR";
/// Include ``mfb::progress`` tracing and run-log progress lines in forensic log
/// files (default off).
pub const ENV_MFB_LOG_PROGRESS: &str = "MFB_LOG_PROGRESS";
pub const ENV_MFB_DEBUG_DIR: &str = "MFB_DEBUG_DIR";
pub const ENV_MFB_SKIP_DISK_PRECHECK: &str = "MFB_SKIP_DISK_PRECHECK";
pub const ENV_MFB_PG_CONNSTR: &str = "MFB_PG_CONNSTR";
pub const ENV_MFB_LOW_MEMORY: &str = "MFB_LOW_MEMORY";
pub const ENV_MFB_MULTI_INSTANCE: &str = "MFB_MULTI_INSTANCE";
/// Force performance governor tier: `relaxed` | `balanced` | `tight` (aliases:
/// wide/normal/strict).
pub const ENV_MFB_PERF_TIER: &str = "MFB_PERF_TIER";
/// Minimum seconds between live RAM/tier reprobes during long Python scans
/// (default 6).
pub const ENV_MFB_PERF_REPROBE_SECS: &str = "MFB_PERF_REPROBE_SECS";
/// Set to `1` to allow multiple concurrent ``run_training.py`` (not recommended
/// on one machine).
pub const ENV_MFB_TRAINING_ALLOW_PARALLEL: &str = "MFB_TRAINING_ALLOW_PARALLEL";
pub const ENV_MFB_TRAINING_SOURCE_MAP: &str = "MFB_TRAINING_SOURCE_MAP";
/// Training C-API ingest progress on stderr (`[INGEST-RUST]`). Default on; set
/// `0`/`false` to disable.
pub const ENV_MFB_TRAINING_INGEST_PROGRESS: &str = "MFB_TRAINING_INGEST_PROGRESS";
pub const ENV_MFB_FFPROBE_TIMEOUT_SECS: &str = "MFB_FFPROBE_TIMEOUT_SECS";
pub const ENV_MFB_FFMPEG_TIMEOUT_SECS: &str = "MFB_FFMPEG_TIMEOUT_SECS";
pub const ENV_MFB_IMAGE_QUALITY_MODEL_PATH: &str = "MFB_IMAGE_QUALITY_MODEL_PATH";
pub const ENV_MFB_IMAGE_QUALITY_MODEL_METADATA_PATH: &str = "MFB_IMAGE_QUALITY_MODEL_METADATA_PATH";
pub const ENV_MFB_IMAGE_QUALITY_MODEL_SCRIPT: &str = "MFB_IMAGE_QUALITY_MODEL_SCRIPT";
pub const ENV_MFB_QUALITY_MODEL_PYTHON: &str = "MFB_QUALITY_MODEL_PYTHON";
pub const ENV_MFB_IMAGE_QUALITY_MODEL_INFERENCE_TIMEOUT_SECS: &str =
    "MFB_IMAGE_QUALITY_MODEL_INFERENCE_TIMEOUT_SECS";
pub const ENV_FORCE_COLOR: &str = "FORCE_COLOR";
/// Force ASCII symbols and strip decorative ANSI on stderr (also implied by
/// `NO_COLOR`).
pub const ENV_PLAIN_UI: &str = "MODERN_FORMAT_PLAIN_UI";
pub const ENV_COLUMNS: &str = "COLUMNS";
pub const ENV_APPLE_COMPAT: &str = "MODERN_FORMAT_BOOST_APPLE_COMPAT";
pub const ENV_DISABLE_SAMPLE_DB: &str = "MODERN_FORMAT_BOOST_DISABLE_SAMPLE_DB";
pub const ENV_GPU_CONCURRENCY: &str = "MODERN_FORMAT_BOOST_GPU_CONCURRENCY";
pub const ENV_VAAPI_DEVICE: &str = "MODERN_FORMAT_BOOST_VAAPI_DEVICE";
pub const ENV_VAAPI_DEVICE_FALLBACK: &str = "VAAPI_DEVICE";
pub const ENV_ENABLE_BRANDING: &str = "MODERN_FORMAT_BOOST_ENABLE_BRANDING";
pub const ENV_HOME: &str = "HOME";
pub const ENV_USERPROFILE: &str = "USERPROFILE";
pub const ENV_JXL_INTENSITY_TARGET: &str = "MFB_JXL_INTENSITY_TARGET";
pub const ENV_MFB_ERROR_MODE: &str = "MFB_ERROR_MODE";
pub const ENV_MFB_DRAG_DROP_ERROR_MODE: &str = "MFB_DRAG_DROP_ERROR_MODE";
pub const ENV_MFB_DRAG_DROP_FAIL_FAST: &str = "MFB_DRAG_DROP_FAIL_FAST";

// --- Dolby Vision Constants ---
/// Default compatibility ID for Dolby Vision Profile 8 (8.1).
pub const DV_PROFILE8_DEFAULT_COMPAT_ID: u8 = 1;

pub const JPEG_SOI_MAGIC: &[u8] = &[0xFF, 0xD8];
pub const PNG_SIGNATURE_MAGIC: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
pub const ISOBMFF_BOX_MDAT: &[u8] = &[0x6D, 0x64, 0x61, 0x74];
pub const JXL_CODESTREAM_MAGIC: &[u8] = &[0xFF, 0x0A];
pub const JXL_CONTAINER_MAGIC: &[u8] = &[
    0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
];
pub const JXL_BOX_JXLC: &[u8] = b"jxlc";
pub const JXL_BOX_JXLP: &[u8; 4] = b"jxlp";
pub const PNG_CHUNK_IHDR: &[u8] = b"IHDR";
pub const PNG_CHUNK_PLTE: &[u8] = b"PLTE";
pub const PNG_CHUNK_IDAT: &[u8] = b"IDAT";
pub const PNG_CHUNK_IEND: &[u8] = b"IEND";
pub const PNG_CHUNK_TRNS: &[u8] = b"tRNS";
pub const AVIF_MEME_MIN_QUALITY: u8 = 0;
pub const MAX_AVIF_BOXES: u32 = 2048;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jxl_effort_policy_is_mode_locked() {
        assert_eq!(jxl_effort_for_mode(false), JXL_DEFAULT_EFFORT);
        assert_eq!(jxl_effort_for_mode(true), JXL_ULTIMATE_EFFORT);
        assert!(is_supported_jxl_effort(JXL_DEFAULT_EFFORT));
        assert!(is_supported_jxl_effort(JXL_DEEP_EFFORT));
        assert!(is_supported_jxl_effort(JXL_ULTIMATE_EFFORT));
        assert_eq!(JXL_ULTIMATE_EFFORT, 10);
        assert_eq!(JXL_EXPERIMENTAL_LOSSLESS_EFFORT, 11);
        assert!(!is_supported_jxl_effort(JXL_EXPERIMENTAL_LOSSLESS_EFFORT));
        assert!(is_supported_jxl_effort_with_expert(
            JXL_EXPERIMENTAL_LOSSLESS_EFFORT,
            true
        ));
        assert!(!is_supported_jxl_effort(6));
        assert!(!is_supported_jxl_effort(JXL_DISABLED_EFFORT));
    }

    #[test]
    fn test_jxl_distance_policy_pins_ultimate_mode() {
        assert!((jxl_distance_for_mode(0.4, false) - 0.4).abs() < f32::EPSILON);
        assert!((jxl_distance_for_mode(0.4, true) - JXL_ULTIMATE_DISTANCE).abs() < f32::EPSILON);
    }
}
