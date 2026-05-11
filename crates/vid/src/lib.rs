//! vid - Video Quality Analysis and HEVC/H.265/AV1 Conversion API
//!
//! Provides precise video analysis with intelligent format conversion:
//! - HEVC Lossless MKV for archival (lossless sources)
//! - HEVC MP4 or MOV for delivery, depending on compatibility mode

#![deny(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::multiple_crate_versions,
    reason = "Legitimate deviation from standard linting rules justified by specific project architecture."
)]
#[macro_use]
extern crate shared_utils;

#[cfg(feature = "high-precision")]
pub use rug::Rational;
#[cfg(not(feature = "high-precision"))]
pub use shared_utils::Rational;

pub mod animated_image;
pub mod codecs;
pub mod conversion_api;
pub mod detection_api;
pub mod ffprobe;
pub use shared_utils::constants;

pub use conversion_api::{
    auto_convert, auto_convert_with_cache, determine_strategy, determine_strategy_with_apple_compat,
};
pub use detection_api::{ColorSpace, CompressionType, DetectedCodec, Detection, detect_video};
pub use ffprobe::{FFprobeResult, probe_video};
pub use shared_utils::conversion_types::{
    ConfigFlags, ConversionConfig, ConversionOutput, ConversionStrategy, TargetVideoFormat,
};

pub use shared_utils::unified_error::{Result, VidQualityError};
