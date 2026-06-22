//! `FFprobe` wrapper module
//!
//! Re-exports from `foundation::ffprobe` to eliminate duplication.
//! Provides a thin wrapper for error type conversion.

pub use foundation::ffprobe::{
    FFprobeError, FFprobeResult, detect_bit_depth, get_duration, get_frame_count,
    is_ffprobe_available, parse_frame_rate,
};

use crate::{Result, VidQualityError};
use std::path::Path;

/// Probe video file using ffprobe.
///
/// # Errors
/// Returns an error if the file is invalid or ffprobe fails.
pub fn probe_video(path: &Path) -> Result<FFprobeResult> {
    foundation::ffprobe::probe_video(path).map_err(VidQualityError::from)
}
