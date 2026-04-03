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

// --- Loop Intent System (Tree & KNN) ---

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

// --- Formats & Extensions ---

/// Modern animated image/container extensions.
pub const MODERN_ANIMATED_EXTENSIONS: &[&str] = &["webp", "avif", "apng", "heic", "heif", "jxl"];
