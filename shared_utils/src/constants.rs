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
