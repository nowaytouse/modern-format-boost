// Core modules
pub mod analyzer;
pub mod formats;
pub mod heic_analysis;
pub mod jpeg_analysis;
pub mod lossless_converter;
pub mod metrics;
pub mod quality_core;
pub mod recommender;

// Separated API layers
pub mod detection_api;
pub mod conversion_api;

// Core exports
pub use analyzer::{analyze_image, ImageAnalysis};
pub use heic_analysis::HeicAnalysis;
pub use jpeg_analysis::JpegQualityAnalysis;
pub use lossless_converter::{ConversionResult, ConvertOptions, convert_to_gif_apple_compat, is_high_quality_animated};
pub use metrics::{calculate_psnr, calculate_ssim, calculate_ms_ssim, psnr_quality_description, ssim_quality_description};
pub use quality_core::{QualityAnalysis, QualityParams, ConversionRecommendation};
pub use recommender::{get_recommendation, UpgradeRecommendation};

// New API exports
pub use detection_api::{detect_image, DetectionResult, DetectedFormat, ImageType, CompressionType};
pub use conversion_api::{smart_convert, simple_convert, determine_strategy, ConversionConfig, ConversionOutput, TargetFormat};

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
