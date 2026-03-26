//! Shared Image Quality Error Types
//!
//! Migrated from `img_hevc/img_av1` to eliminate duplication.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ImgQualityError {
    /// The image format is not supported by the current encoder or tool.
    #[error("Image format not supported: {0}")]
    UnsupportedFormat(String),

    /// An error occurred while reading the image file.
    #[error("Failed to read image: {0}")]
    ImageReadError(String),

    /// An error occurred during image quality or structure analysis.
    #[error("Failed to analyze image: {0}")]
    AnalysisError(String),

    /// The image conversion process failed.
    #[error("Conversion failed: {0}")]
    ConversionError(String),

    /// A required external tool was not found in PATH.
    #[error("External tool not found: {0}")]
    ToolNotFound(String),

    /// Standard I/O error.
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Error from the `image` crate.
    #[error("Image processing error: {0}")]
    ImageError(#[from] image::ImageError),

    /// The requested feature or format is not implemented yet.
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// The file was intentionally skipped (e.g. anti-duplicate).
    #[error("Skip file: {0}")]
    SkipFile(String),
}

pub type Result<T> = std::result::Result<T, ImgQualityError>;
