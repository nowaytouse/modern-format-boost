#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
// Legacy conversion paths exceed pedantic 100-LOC limit; split tracked separately from foundation.
#![allow(clippy::too_many_lines)]
//! vid - Video Quality Analysis and HEVC/H.265/AV1 Conversion API
//!
//! Provides precise video analysis with intelligent format conversion:
//! - HEVC Lossless MKV for archival (lossless sources)
//! - HEVC MP4 or MOV for delivery, depending on compatibility mode

#[macro_use]
extern crate foundation;

#[cfg(not(feature = "high-precision"))]
pub use foundation::Rational;
#[cfg(feature = "high-precision")]
pub use rug::Rational;

pub mod animated_image;
pub mod codecs;
pub mod conversion_api;
pub mod detection_api;
pub mod ffprobe;
pub use foundation::constants;

pub use conversion_api::{auto_convert_with_cache, determine_strategy_with_apple_compat};
pub use detection_api::{ColorSpace, CompressionType, DetectedCodec, Detection, detect_video};
pub use ffprobe::{FFprobeResult, probe_video};
pub use foundation::conversion_types::{
    ConfigFlags, ConversionConfig, ConversionOutput, ConversionStrategy, TargetVideoFormat,
};

pub use foundation::unified_error::{Result, VidQualityError};
