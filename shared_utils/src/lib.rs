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
pub mod simple_progress;
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
#[cfg(test)]
mod video_explorer_tests;
pub mod checkpoint;
pub mod xmp_merger;
pub mod flag_validator;
pub mod gpu_accel;
pub mod modern_ui;
pub mod realtime_progress;
pub mod lru_cache;
pub mod error_handler;
pub mod ssim_mapping;

pub use progress::{
    // 🔥 v5.31: 新增粗进度条
    CoarseProgressBar,
    // 🔥 v5.88: 详细粗进度条（视频探索专用）
    DetailedCoarseProgressBar,
    // 🔥 v5.5: 新增固定底部进度条
    FixedBottomProgress, ProgressStats, ExploreProgress, ExploreLogger,
    GlobalProgressManager,
    // 原有导出
    create_progress_bar, create_detailed_progress_bar, create_compact_progress_bar,
    create_progress_bar_with_eta, SmartProgressBar,
    create_spinner, create_multi_progress,
    BatchProgress, format_bytes, format_duration,
};
pub use safety::*;
pub use batch::*;
pub use report::*;
pub use ffprobe::{FFprobeResult, FFprobeError, probe_video, get_duration, get_frame_count, parse_frame_rate, detect_bit_depth, is_ffprobe_available};
pub use tools::*;
pub use codecs::*;
pub use metadata::{preserve_metadata, preserve_pro, copy_metadata};
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
    // 🔥 v5.74: 透明度报告类型
    SsimSource, IterationMetrics, TransparencyReport,
    // 🔥 v5.74: Preset 配置
    EncoderPreset,
    // New API: mode-specific functions
    explore_size_only, explore_quality_match, explore_precise_quality_match,
    // 🔥 v4.5: 精确质量匹配 + 压缩
    explore_precise_quality_match_with_compression,
    // 🔥 v4.6: 仅压缩 + 压缩+质量
    explore_compress_only, explore_compress_with_quality,
    // HEVC convenience functions
    explore_hevc, explore_hevc_size_only, explore_hevc_quality_match,
    explore_hevc_compress_only, explore_hevc_compress_with_quality,
    // AV1 convenience functions
    explore_av1, explore_av1_size_only, explore_av1_quality_match,
    explore_av1_compress_only, explore_av1_compress_with_quality,
    // Precision module (精确度规范)
    precision,
    // 🔥 v5.72: 三阶段搜索
    precision::SearchPhase, precision::ThreePhaseSearch,
};

// 🔥 v5.0: GPU 控制变体 (deprecated, GPU is now automatic)
// 保留向后兼容，但不推荐使用
#[allow(deprecated)]
pub use video_explorer::{
    explore_precise_quality_match_with_compression_gpu,
    explore_precise_quality_match_gpu,
    explore_compress_only_gpu,
    explore_compress_with_quality_gpu,
    explore_size_only_gpu,
    explore_quality_match_gpu,
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

// 🔥 v4.6: Flag 组合验证器
pub use flag_validator::{
    FlagMode, FlagValidation, validate_flags, validate_flags_result, print_flag_help,
};

// 🔥 v4.9: GPU 加速模块
// 🔥 v5.0: 新增 GPU→CPU 边界估算函数
// 🔥 v5.1: 新增 GPU 粗略搜索 + CRF 映射
pub use gpu_accel::{
    GpuAccel, GpuEncoder, GpuType,
    // v5.0: GPU→CPU 边界估算
    estimate_cpu_search_center, gpu_boundary_to_cpu_range,
    // v5.1: GPU 粗略搜索
    GpuCoarseResult, GpuCoarseConfig, CrfMapping,
    gpu_coarse_search, gpu_coarse_search_with_log, get_cpu_search_range_from_gpu,
};

// 🔥 v5.1: GPU+CPU 智能探索
pub use video_explorer::{
    explore_with_gpu_coarse_search,
    explore_hevc_with_gpu_coarse,
    explore_av1_with_gpu_coarse,
};

// 🔥 v5.19: 现代化 UI/UX 模块
pub use modern_ui::{
    colors, symbols, progress_style,
    render_progress_bar, render_colored_progress, ProgressStyle,
    ExploreProgressState,
    print_result_box, print_stage, print_substage,
    print_success, print_warning, print_error, print_info,
    format_size, format_size_change,
    spinner_frame, spinner_dots,
};

// 🔥 v5.20: 真正的实时进度条
#[allow(deprecated)]
pub use realtime_progress::{
    // 🔥 v5.34: 新的基于迭代计数的进度条（推荐）
    SimpleIterationProgress,
    // v5.31: 旧的基于CRF范围的进度条（已弃用但保留兼容）
    RealtimeExploreProgress, RealtimeSpinner,
    // 🔥 v5.72: 详细进度状态
    DetailedProgressState,
};

// 🔥 v5.72: LRU缓存模块
pub use lru_cache::{LruCache, CacheEntry, SerializableCache};

// 🔥 v5.72: 统一错误处理模块
pub use error_handler::{ErrorCategory, ErrorAction, handle_error};

// 🔥 v5.74: PSNR→SSIM 动态映射模块
pub use ssim_mapping::{PsnrSsimMapping, MappingPoint};
