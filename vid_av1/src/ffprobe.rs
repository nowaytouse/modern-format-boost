//! FFprobe wrapper module
//!
//! Re-exports from shared_utils::ffprobe to eliminate duplication.
//! Provides a thin wrapper for error type conversion.

pub use shared_utils::ffprobe::{
    detect_bit_depth, get_duration, get_frame_count, is_ffprobe_available, parse_frame_rate,
    FFprobeError, FFprobeResult,
};

use crate::{Result, VidQualityError};
use std::path::Path;

/// Probe video file using ffprobe (wrapper with VidQualityError conversion)
pub fn probe_video(path: &Path) -> Result<FFprobeResult> {
    shared_utils::ffprobe::probe_video(path).map_err(VidQualityError::from)
}
