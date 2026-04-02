//! Global Constants for `modern_format_boost`
//!
//! This module centralizes core magic numbers and business rules to ensure
//! consistency across the workspace.

/// Default size tolerance (1MB) allowed for conversions that improve compatibility
/// or when absolute byte tolerance is enabled.
/// 1MB = 1,048,576 bytes
pub const DEFAULT_SIZE_TOLERANCE_BYTES: u64 = 1_048_576;

/// Default size tolerance percentage (1%) for video/animated image conversions.
pub const DEFAULT_SIZE_TOLERANCE_RATIO: f64 = 0.01;

/// Minimum output size for images to be considered valid for deletion of original.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE: u64 = 1024;

/// Minimum output size for videos to be considered valid for deletion of original.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO: u64 = 4096;

// --- Unified Video Duration Thresholds ---

/// Animation and short clip threshold (30s).
/// Clips shorter than this may use CRF 0.00 lossless-first probing in Ultimate mode.
pub const ANIMATION_CLIP_THRESHOLD_SECS: f32 = 30.0;

/// Maximum duration for CRF 0.00 lossless-first probing when media is identified as a meme (low entropy).
pub const MEME_LOSSLESS_DURATION_LIMIT: f32 = 120.0;

/// Maximum duration for CRF 0.00 lossless-first probing when media is identified as high-value/art (high entropy).
pub const HIGH_VALUE_LOSSLESS_DURATION_LIMIT: f32 = 30.0;

/// Medium to Long video threshold (5 min).
pub const LONG_VIDEO_THRESHOLD_SECS: f32 = 300.0;

/// Very Long video threshold (10 min).
pub const VERY_LONG_VIDEO_THRESHOLD_SECS: f32 = 600.0;

/// Heavy video threshold (20 min).
pub const HEAVY_VIDEO_THRESHOLD_SECS: f32 = 1200.0;

// --- Quality Matrix Thresholds ---

/// When to skip expensive VMAF calculation in normal mode (5 min).
pub const VMAF_SKIP_THRESHOLD_SECS: f64 = 300.0;

/// When to skip expensive VMAF calculation in ultimate mode (25 min).
pub const VMAF_SKIP_THRESHOLD_ULTIMATE_SECS: f64 = 1500.0;

/// Modern animated image/container extensions considered 'animated-like' for loop intent checks.
pub const MODERN_ANIMATED_EXTENSIONS: &[&str] = &[
    "webp",
    "avif",
    "apng",
    "heic",
    "heif",
    "jxl",
];
