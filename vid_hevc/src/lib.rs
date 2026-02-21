//! vid-hevc - Video Quality Analysis and HEVC/H.265 Conversion API
//!
//! Provides precise video analysis with intelligent format conversion:
//! - HEVC Lossless MKV for archival (lossless sources)
//! - HEVC MP4 for compression (lossy sources)
//!
//! ## Simple Mode
//! ```rust,ignore
//! use vid_hevc::simple_convert;
//! use std::path::Path;
//!
//! let input = Path::new("video.mp4");
//! let output_dir = Some(Path::new("output/"));
//! simple_convert(input, output_dir)?;
//! ```

pub mod codecs;
pub mod conversion_api;
pub mod detection_api;
pub mod ffprobe;

// Re-exports
pub use conversion_api::{
    auto_convert, determine_strategy, determine_strategy_with_apple_compat, simple_convert,
};
pub use detection_api::{
    detect_video, ColorSpace, CompressionType, DetectedCodec, VideoDetectionResult,
};
// 🔥 v9.2: Use shared types
pub use ffprobe::{probe_video, FFprobeResult};
pub use shared_utils::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, TargetVideoFormat,
};

// 🔥 v9.1: Use shared error types
pub use shared_utils::errors::{Result, VidQualityError};
