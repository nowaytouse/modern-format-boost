//! vid - Video Quality Analysis and HEVC/H.265/AV1 Conversion API
//!
//! Provides precise video analysis with intelligent format conversion:
//! - HEVC Lossless MKV for archival (lossless sources)
//! - HEVC MP4 or MOV for delivery, depending on compatibility mode

pub mod animated_image;
pub mod codecs;
pub mod conversion_api;
pub mod detection_api;
pub mod ffprobe;

pub use conversion_api::{
    auto_convert, auto_convert_with_cache, determine_strategy, determine_strategy_with_apple_compat,
};
pub use detection_api::{
    detect_video, ColorSpace, CompressionType, DetectedCodec, VideoDetectionResult,
};
pub use ffprobe::{probe_video, FFprobeResult};
pub use shared_utils::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, TargetVideoFormat,
};

pub use shared_utils::unified_error::{Result, VidQualityError};
