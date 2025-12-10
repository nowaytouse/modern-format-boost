//! Shared Utilities for modern_format_boost tools
//! 
//! This crate provides common functionality shared across imgquality, vidquality, and vidquality-hevc:
//! - Progress bar with ETA
//! - Safety checks (dangerous directory detection)
//! - Batch processing utilities
//! - Common logging and reporting
//! - FFprobe wrapper for video analysis
//! - External tools detection
//! - Codec information
//! - Metadata preservation (EXIF/IPTC/xattr/timestamps/ACL)
//! - Conversion utilities (ConversionResult, ConvertOptions, anti-duplicate)
//! - Date analysis (deep EXIF/XMP date extraction)
//! - Quality matching (unified CRF/distance calculation for all encoders)

pub mod progress;
pub mod safety;
pub mod batch;
pub mod report;
pub mod ffprobe;
pub mod tools;
pub mod codecs;
pub mod metadata;
pub mod conversion;
pub mod video;
pub mod date_analysis;
pub mod quality_matcher;

pub use progress::*;
pub use safety::*;
pub use batch::*;
pub use report::*;
pub use ffprobe::{FFprobeResult, FFprobeError, probe_video, get_duration, get_frame_count, parse_frame_rate, detect_bit_depth, is_ffprobe_available};
pub use tools::*;
pub use codecs::*;
pub use metadata::{preserve_metadata, preserve_pro};
pub use conversion::*;
pub use video::*;
pub use date_analysis::{analyze_directory, DateAnalysisConfig, DateAnalysisResult, FileDateInfo, DateSource, print_analysis};
pub use quality_matcher::{
    EncoderType, SourceCodec, QualityAnalysis, MatchedQuality, AnalysisDetails,
    calculate_av1_crf, calculate_hevc_crf, calculate_jxl_distance,
    log_quality_analysis, from_video_detection, from_image_analysis,
};
