#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![allow(clippy::too_many_lines)]
#[cfg(not(feature = "high-precision"))]
pub use foundation::Rational;
#[cfg(feature = "high-precision")]
pub use rug::Rational;

pub mod analyzer;
pub use foundation::constants;
pub mod formats;
pub mod heic_analysis;
pub mod jpeg_analysis;
pub mod lossless_converter;
pub mod metrics;
pub mod recommender;

pub mod conversion_api;
pub mod detection_api;

pub use analyzer::{ImageAnalysis, analyze_image};
pub use foundation::constants::*;
pub use heic_analysis::HeicAnalysis;
pub use jpeg_analysis::JpegQualityAnalysis;
pub use lossless_converter::{ConvertFlags, ConvertOptions, TaskResult};
pub use metrics::{
    calculate_ms_ssim, calculate_psnr, calculate_ssim, psnr_quality_description,
    ssim_quality_description,
};
pub use recommender::{UpgradeRecommendation, get_recommendation};

pub use conversion_api::{
    ConfigFlags, ConversionConfig, ConversionOutput, TargetFormat, determine_strategy,
    smart_convert,
};
pub use detection_api::{
    CompressionType, DetectedFormat, DetectionResult, ImageType, detect_image,
};

pub use foundation::unified_error::{ImgQualityError, Result};
