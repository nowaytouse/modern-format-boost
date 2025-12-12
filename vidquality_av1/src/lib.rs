//! vidquality - Video Quality Analysis and Format Conversion API
//!
//! Provides precise video analysis with intelligent format conversion:
//! - FFV1 MKV for archival (lossless sources)
//! - AV1 MP4 for compression (lossy sources)
//!
//! ## Simple Mode
//! ```rust,ignore
//! use vidquality_av1::simple_convert;
//! use std::path::Path;
//! 
//! let input = Path::new("video.mp4");
//! let output_dir = Some(Path::new("output/"));
//! simple_convert(input, output_dir)?;
//! ```

pub mod detection_api;
pub mod conversion_api;
pub mod ffprobe;
pub mod codecs;

use thiserror::Error;

// Re-exports
pub use detection_api::{detect_video, VideoDetectionResult, DetectedCodec, CompressionType, ColorSpace};
pub use conversion_api::{
    simple_convert, auto_convert, determine_strategy, ConversionConfig, ConversionStrategy, ConversionOutput, TargetVideoFormat
};
pub use ffprobe::{probe_video, FFprobeResult};

#[derive(Error, Debug)]
pub enum VidQualityError {
    #[error("Video format not supported: {0}")]
    UnsupportedFormat(String),

    #[error("Failed to read video: {0}")]
    VideoReadError(String),

    #[error("FFprobe failed: {0}")]
    FFprobeError(String),

    #[error("FFmpeg failed: {0}")]
    FFmpegError(String),

    #[error("Conversion failed: {0}")]
    ConversionError(String),

    #[error("External tool not found: {0}")]
    ToolNotFound(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, VidQualityError>;
