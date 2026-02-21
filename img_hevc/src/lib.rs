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
pub mod conversion_api;
pub mod detection_api;

// Core exports
pub use analyzer::{analyze_image, ImageAnalysis};
pub use heic_analysis::HeicAnalysis;
pub use jpeg_analysis::JpegQualityAnalysis;
pub use lossless_converter::{
    convert_to_gif_apple_compat, is_high_quality_animated, ConversionResult, ConvertOptions,
};
pub use metrics::{
    calculate_ms_ssim, calculate_psnr, calculate_ssim, psnr_quality_description,
    ssim_quality_description,
};
pub use quality_core::{ConversionRecommendation, QualityAnalysis, QualityParams};
pub use recommender::{get_recommendation, UpgradeRecommendation};

// New API exports
pub use conversion_api::{
    determine_strategy, simple_convert, smart_convert, ConversionConfig, ConversionOutput,
    TargetFormat,
};
pub use detection_api::{
    detect_image, CompressionType, DetectedFormat, DetectionResult, ImageType,
};

// 🔥 Refactor: Use shared error types (migrated to shared_utils)
pub use shared_utils::img_errors::{ImgQualityError, Result};
