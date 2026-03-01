//! Constants for img_hevc conversion tool

/// Small PNG file threshold (500KB)
/// PNG files smaller than this will be skipped to avoid overhead
pub const SMALL_PNG_THRESHOLD_BYTES: u64 = 500 * 1024;

/// Initial buffer size for stderr capture (1MB)
pub const STDERR_BUFFER_INITIAL: usize = 1024 * 1024;

/// Maximum buffer size for stderr capture (10MB)
/// Prevents memory overflow from long-running processes
pub const STDERR_BUFFER_MAX: usize = 10 * 1024 * 1024;

/// Maximum number of stderr lines to capture
/// Prevents memory overflow from verbose output
pub const STDERR_MAX_LINES: usize = 100_000;

/// Animation duration threshold (3.0 seconds)
/// Animations shorter than this may be converted to static images
pub const ANIMATION_DURATION_THRESHOLD_SECS: f32 = 3.0;

/// High quality CRF value for HEVC encoding
pub const CRF_HIGH_QUALITY: f32 = 18.0;

/// Standard quality CRF value for HEVC encoding
pub const CRF_STANDARD_QUALITY: f32 = 20.0;

/// Default number of threads for fallback
pub const DEFAULT_FALLBACK_THREADS: usize = 2;

/// Default JXL distance for conservative encoding
pub const DEFAULT_JXL_DISTANCE: f32 = 1.0;
