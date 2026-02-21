//! Shared Image Quality Error Types
//!
//! Migrated from img_hevc/img_av1 to eliminate duplication.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ImgQualityError {
    #[error("Image format not supported: {0}")]
    UnsupportedFormat(String),

    #[error("Failed to read image: {0}")]
    ImageReadError(String),

    #[error("Failed to analyze image: {0}")]
    AnalysisError(String),

    #[error("Conversion failed: {0}")]
    ConversionError(String),

    #[error("External tool not found: {0}")]
    ToolNotFound(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Image processing error: {0}")]
    ImageError(#[from] image::ImageError),
}

pub type Result<T> = std::result::Result<T, ImgQualityError>;
