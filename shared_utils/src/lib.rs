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
pub mod image_quality_detector;
pub mod video_quality_detector;
pub mod video_explorer;
pub mod checkpoint;
pub mod xmp_merger;

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
    // Core types
    EncoderType, SourceCodec, QualityAnalysis, MatchedQuality, AnalysisDetails,
    SkipDecision,
    // v3.0 Enhanced types
    MatchMode, QualityBias, ContentType, VideoAnalysisBuilder,
    // CRF/distance calculation
    calculate_av1_crf, calculate_hevc_crf, calculate_jxl_distance,
    // v3.0 with options
    calculate_av1_crf_with_options, calculate_hevc_crf_with_options, calculate_jxl_distance_with_options,
    // Utilities
    log_quality_analysis, from_video_detection, from_image_analysis,
    should_skip_video_codec, should_skip_video_codec_apple_compat, should_skip_image_format, parse_source_codec,
};

pub use image_quality_detector::{
    // Core types
    ImageQualityAnalysis, ImageContentType, RoutingDecision,
    // Main analysis function
    analyze_image_quality,
};

pub use video_quality_detector::{
    // Core types
    VideoQualityAnalysis, VideoCodecType, ChromaSubsampling, 
    VideoContentType, CompressionLevel, VideoRoutingDecision,
    // Main analysis function
    analyze_video_quality,
    // Integration helper
    to_quality_analysis as video_to_quality_analysis,
};

pub use video_explorer::{
    // Core types
    ExploreResult, ExploreConfig, QualityThresholds, VideoEncoder, VideoExplorer,
    // Explore mode enum
    ExploreMode,
    // New API: mode-specific functions
    explore_size_only, explore_quality_match, explore_precise_quality_match,
    // HEVC convenience functions
    explore_hevc, explore_hevc_size_only, explore_hevc_quality_match,
    // AV1 convenience functions
    explore_av1, explore_av1_size_only, explore_av1_quality_match,
    // Precision module (精确度规范)
    precision,
};

// Legacy API re-exports (deprecated but still available)
#[allow(deprecated)]
pub use video_explorer::quick_explore;
#[allow(deprecated)]
pub use video_explorer::full_explore;

pub use checkpoint::{
    CheckpointManager, verify_output_integrity, safe_delete_original,
};

pub use xmp_merger::{
    XmpMerger, XmpMergerConfig, XmpFile, MergeResult, MergeSummary,
};
