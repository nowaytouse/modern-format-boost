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

pub mod batch;
pub mod checkpoint;
pub mod codecs;
pub mod conversion;
pub mod date_analysis;
pub mod error_handler;
pub mod explore_strategy;
pub mod ffprobe;
pub mod flag_validator;
pub mod gpu_accel;
pub mod image_quality_detector;
pub mod lru_cache;
pub mod metadata;
pub mod modern_ui;
pub mod progress;
pub mod quality_matcher;
pub mod realtime_progress;
pub mod report;
pub mod safety;
pub mod simple_progress;
pub mod ssim_mapping;
pub mod tools;
pub mod video;
pub mod video_explorer;
#[cfg(test)]
mod video_explorer_tests;
pub mod video_quality_detector;
pub mod xmp_merger;
// 🔥 v6.4.7: FFmpeg 进程管理模块（防死锁）
pub mod ffmpeg_process;
// 🔥 v6.4.9: 代码质量模块
pub mod crf_constants;
pub mod float_compare;
pub mod path_validator;
// 🔥 v7.9: Smart thread management for Apple Silicon
pub mod thread_manager;

pub mod path_safety;
pub use path_safety::safe_path_arg;
// 🔥 v6.5: FFprobe JSON 解析模块
pub mod ffprobe_json;
// 🔥 v6.7: 纯视频流大小提取模块
pub mod stream_size;
// 🔥 v6.7: 纯媒体压缩验证器
pub mod pure_media_verifier;
// 🔥 v7.1: 类型安全模块
pub mod types;
// 🔥 v7.1: 统一错误类型
pub mod app_error;
// 🔥 v6.9.13: 文件复制模块（无遗漏设计）
pub mod file_copier;
// 🔥 v7.3.2: 智能文件复制模块（统一目录结构+元数据保留）
pub mod smart_file_copier;

// 🔥 v7.3.2: 进度条模式控制（解决并行输出混乱）
pub mod progress_mode;

// 🔥 v8.0: 统一进度条系统
pub mod unified_progress;
pub use unified_progress::UnifiedProgressBar;

// 🔥 v7.5: 文件排序模块（优先处理小文件）
pub mod file_sorter;

// 🔥 v7.6: MS-SSIM智能采样模块
pub mod msssim_sampling;

// 🔥 v7.6: MS-SSIM心跳检测模块
pub mod msssim_heartbeat;

// 🔥 v7.6: MS-SSIM进度监控模块
pub mod msssim_progress;

// 🔥 v7.6: MS-SSIM并行计算模块
pub mod msssim_parallel;

// 🔥 v7.7: 通用心跳系统
pub mod heartbeat_manager;
pub mod universal_heartbeat;

// 🔥 v7.8: 统一日志系统
pub mod logging;

// 🔥 v7.8: 通用工具函数模块
pub mod common_utils;

// 🔥 v6.9.17: x265 CPU编码器模块
pub mod x265_encoder;

// 🔥 v7.2: 独立 VMAF 工具集成（绕过 ffmpeg libvmaf 依赖）
pub mod vmaf_standalone;

// 🔥 Refactor: Shared CLI Runner
pub mod cli_runner;

// 🔥 Refactor: Shared Errors
pub mod errors;

// 🔥 Refactor: Shared Conversion Types
pub mod conversion_types;

pub use batch::*;
pub use codecs::*;
pub use conversion::*;
pub use date_analysis::{
    analyze_directory, print_analysis, DateAnalysisConfig, DateAnalysisResult, DateSource,
    FileDateInfo,
};
pub use ffprobe::{
    detect_bit_depth, get_duration, get_frame_count, is_ffprobe_available, parse_frame_rate,
    probe_video, FFprobeError, FFprobeResult,
};
pub use metadata::{copy_metadata, preserve_directory_metadata, preserve_metadata, preserve_pro};
pub use progress::{
    create_compact_progress_bar,
    create_detailed_progress_bar,
    create_multi_progress,
    // 原有导出
    create_progress_bar,
    create_progress_bar_with_eta,
    create_spinner,
    format_bytes,
    format_duration,
    BatchProgress,
    // 🔥 v5.31: 新增粗进度条
    CoarseProgressBar,
    // 🔥 v5.88: 详细粗进度条（视频探索专用）
    DetailedCoarseProgressBar,
    ExploreLogger,
    ExploreProgress,
    // 🔥 v5.5: 新增固定底部进度条
    FixedBottomProgress,
    GlobalProgressManager,
    ProgressStats,
    SmartProgressBar,
};
pub use quality_matcher::{
    // CRF/distance calculation
    calculate_av1_crf,
    // v3.0 with options
    calculate_av1_crf_with_options,
    calculate_hevc_crf,
    calculate_hevc_crf_with_options,
    calculate_jxl_distance,
    calculate_jxl_distance_with_options,
    from_image_analysis,
    from_video_detection,
    // Utilities
    log_quality_analysis,
    parse_source_codec,
    should_skip_image_format,
    should_skip_video_codec,
    should_skip_video_codec_apple_compat,
    AnalysisDetails,
    ContentType,
    // Core types
    EncoderType,
    // v3.0 Enhanced types
    MatchMode,
    MatchedQuality,
    QualityAnalysis,
    QualityBias,
    SkipDecision,
    SourceCodec,
    VideoAnalysisBuilder,
};
pub use report::*;
pub use safety::*;
pub use tools::*;
pub use video::*;

pub use image_quality_detector::{
    // Main analysis function
    analyze_image_quality,
    ImageContentType,
    // Core types
    ImageQualityAnalysis,
    RoutingDecision,
};

pub use video_quality_detector::{
    // Main analysis function
    analyze_video_quality,
    // Integration helper
    to_quality_analysis as video_to_quality_analysis,
    ChromaSubsampling,
    CompressionLevel,
    VideoCodecType,
    VideoContentType,
    // Core types
    VideoQualityAnalysis,
    VideoRoutingDecision,
};

pub use video_explorer::{
    // 🔥 v6.4.3: 动态元数据余量（百分比 + 最小值策略）
    calculate_metadata_margin,
    can_compress_with_metadata,
    compression_target_size,
    detect_metadata_size,
    // AV1 convenience functions
    explore_av1,
    explore_av1_compress_only,
    explore_av1_compress_with_quality,
    explore_av1_quality_match,
    explore_av1_size_only,
    // 🔥 v4.6: 仅压缩 + 压缩+质量
    explore_compress_only,
    explore_compress_with_quality,
    // HEVC convenience functions
    explore_hevc,
    explore_hevc_compress_only,
    explore_hevc_compress_with_quality,
    explore_hevc_quality_match,
    explore_hevc_size_only,
    explore_precise_quality_match,
    // 🔥 v4.5: 精确质量匹配 + 压缩
    explore_precise_quality_match_with_compression,
    explore_quality_match,
    // New API: mode-specific functions
    explore_size_only,
    // Precision module (精确度规范)
    precision,
    // 🔥 v5.72: 三阶段搜索
    precision::SearchPhase,
    precision::ThreePhaseSearch,
    pure_video_size,
    verify_compression_precise,
    verify_compression_simple,
    CompressionVerifyStrategy,
    // 🔥 v5.74: Preset 配置
    EncoderPreset,
    ExploreConfig,
    // Explore mode enum
    ExploreMode,
    // Core types
    ExploreResult,
    IterationMetrics,
    QualityThresholds,
    // 🔥 v5.74: 透明度报告类型
    SsimSource,
    TransparencyReport,
    VideoEncoder,
    VideoExplorer,
    METADATA_MARGIN_MAX,
    METADATA_MARGIN_MIN,
    METADATA_MARGIN_PERCENT,
    SMALL_FILE_THRESHOLD,
};

// 🔥 v5.0: GPU 控制变体 (deprecated, GPU is now automatic)
// 保留向后兼容，但不推荐使用
#[allow(deprecated)]
pub use video_explorer::{
    explore_compress_only_gpu, explore_compress_with_quality_gpu,
    explore_precise_quality_match_gpu, explore_precise_quality_match_with_compression_gpu,
    explore_quality_match_gpu, explore_size_only_gpu,
};

// Legacy API re-exports (deprecated but still available)
#[allow(deprecated)]
pub use video_explorer::full_explore;
#[allow(deprecated)]
pub use video_explorer::quick_explore;

pub use checkpoint::{safe_delete_original, verify_output_integrity, CheckpointManager};

pub use xmp_merger::{
    merge_xmp_for_copied_file, // 🔥 v6.9.11: 复制文件时合并XMP
    MergeResult,
    MergeSummary,
    XmpFile,
    XmpMerger,
    XmpMergerConfig,
};

// 🔥 v4.6: Flag 组合验证器
// 🔥 v6.2: 添加 ultimate 支持
pub use flag_validator::{
    print_flag_help, validate_flags, validate_flags_result, validate_flags_result_with_ultimate,
    validate_flags_with_ultimate, FlagMode, FlagValidation,
};

// 🔥 v4.9: GPU 加速模块
// 🔥 v5.0: 新增 GPU→CPU 边界估算函数
// 🔥 v5.1: 新增 GPU 粗略搜索 + CRF 映射
pub use gpu_accel::{
    // v5.0: GPU→CPU 边界估算
    estimate_cpu_search_center,
    get_cpu_search_range_from_gpu,
    gpu_boundary_to_cpu_range,
    gpu_coarse_search,
    gpu_coarse_search_with_log,
    CrfMapping,
    GpuAccel,
    GpuCoarseConfig,
    // v5.1: GPU 粗略搜索
    GpuCoarseResult,
    GpuEncoder,
    GpuType,
};

// 🔥 v5.1: GPU+CPU 智能探索
pub use video_explorer::{
    explore_av1_with_gpu_coarse,
    explore_hevc_with_gpu_coarse,
    explore_hevc_with_gpu_coarse_full,     // 🔥 v6.9: 完整参数版本
    explore_hevc_with_gpu_coarse_ultimate, // 🔥 v6.2: 极限探索模式
    explore_with_gpu_coarse_search,
};

// 🔥 v5.19: 现代化 UI/UX 模块
pub use modern_ui::{
    colors, format_size, format_size_change, format_size_diff, print_error, print_info,
    print_result_box, print_stage, print_substage, print_success, print_warning, progress_style,
    render_colored_progress, render_progress_bar, spinner_dots, spinner_frame, symbols,
    ExploreProgressState, ProgressStyle,
};

// 🔥 v5.20: 真正的实时进度条
#[allow(deprecated)]
pub use realtime_progress::{
    // 🔥 v5.72: 详细进度状态
    DetailedProgressState,
    // v5.31: 旧的基于CRF范围的进度条（已弃用但保留兼容）
    RealtimeExploreProgress,
    RealtimeSpinner,
    // 🔥 v5.34: 新的基于迭代计数的进度条（推荐）
    SimpleIterationProgress,
};

// 🔥 v5.72: LRU缓存模块
pub use lru_cache::{CacheEntry, LruCache, SerializableCache};

// 🔥 v5.72: 统一错误处理模块
pub use error_handler::{handle_error, ErrorAction, ErrorCategory};

// 🔥 v5.74: PSNR→SSIM 动态映射模块
pub use ssim_mapping::{MappingPoint, PsnrSsimMapping};

// 🔥 v6.3: Strategy 模式探索器
pub use explore_strategy::{
    create_strategy, strategy_name, CompressOnlyStrategy, CompressWithQualityStrategy,
    ExploreContext, ExploreStrategy, PreciseQualityMatchStrategy,
    PreciseQualityMatchWithCompressionStrategy, ProgressConfig, QualityMatchStrategy,
    SizeOnlyStrategy, SsimResult,
};

// 🔥 v6.4.7: FFmpeg 进程管理（防死锁）
pub use ffmpeg_process::{
    format_ffmpeg_error, is_recoverable_error, FfmpegProcess, FfmpegProgressParser,
};

// 🔥 v6.4.9: 代码质量模块
// 🔥 v7.1: 扩展浮点比较函数
pub use float_compare::{
    approx_eq_crf,
    approx_eq_f32,
    // 通用比较函数
    approx_eq_f64,
    approx_eq_psnr,
    // 🔥 v7.1: 领域特定比较函数
    approx_eq_ssim,
    approx_ge_f64,
    approx_le_f64,
    approx_zero_f32,
    approx_zero_f64,
    crf_in_range,
    ssim_below_threshold,
    ssim_meets_threshold,
    CRF_EPSILON,
    F32_EPSILON,
    // 通用 epsilon
    F64_EPSILON,
    PSNR_EPSILON,
    // 🔥 v7.1: 领域特定 epsilon
    SSIM_EPSILON as FLOAT_SSIM_EPSILON,
};

pub use path_validator::{validate_path, validate_paths, PathValidationError};

pub use crf_constants::{
    AV1_CRF_DEFAULT,
    AV1_CRF_MAX,
    // AV1
    AV1_CRF_MIN,
    AV1_CRF_PRACTICAL_MAX,
    AV1_CRF_VISUALLY_LOSSLESS,
    // Cache
    CRF_CACHE_KEY_MULTIPLIER,
    CRF_CACHE_MAX_VALID,
    EMERGENCY_MAX_ITERATIONS as CRF_EMERGENCY_MAX_ITERATIONS,
    HEVC_CRF_DEFAULT,
    HEVC_CRF_MAX,
    // HEVC
    HEVC_CRF_MIN,
    HEVC_CRF_PRACTICAL_MAX,
    HEVC_CRF_VISUALLY_LOSSLESS,
    // Iterations
    NORMAL_MAX_ITERATIONS,
    VP9_CRF_DEFAULT,
    VP9_CRF_MAX,
    // VP9
    VP9_CRF_MIN,
    X264_CRF_DEFAULT,
    X264_CRF_MAX,
    // x264
    X264_CRF_MIN,
};

// 🔥 v6.5: FFprobe JSON 解析
pub use ffprobe_json::{extract_color_info as ffprobe_extract_color_info, ColorInfo};

// 🔥 v6.7: 纯视频流大小提取
pub use stream_size::{
    extract_stream_sizes, get_container_overhead_percent, ExtractionMethod, StreamSizeInfo,
    DEFAULT_OVERHEAD_PERCENT, MKV_OVERHEAD_PERCENT, MOV_OVERHEAD_PERCENT, MP4_OVERHEAD_PERCENT,
};

// 🔥 v6.7: 纯媒体压缩验证
pub use pure_media_verifier::{
    is_video_compressed, verify_pure_media_compression, video_compression_ratio,
    PureMediaVerifyResult,
};

// 🔥 v7.1: 类型安全包装器
pub use types::{
    Av1Encoder, Crf, CrfError, EncoderBounds, FileSize, HevcEncoder, IterationError,
    IterationGuard, Ssim, SsimError, Vp9Encoder, X264Encoder, SSIM_EPSILON,
};

// 🔥 v7.1: 统一错误类型
pub use app_error::AppError;

// 🔥 v6.9.13: 文件复制模块（无遗漏设计）
pub use file_copier::{
    copy_unsupported_files, count_files as count_all_files, verify_output_completeness, CopyResult,
    FileStats, VerifyResult, SIDECAR_EXTENSIONS, SUPPORTED_IMAGE_EXTENSIONS,
    SUPPORTED_VIDEO_EXTENSIONS,
};
pub use smart_file_copier::{copy_on_skip_or_fail, fix_extension_if_mismatch, smart_copy_with_structure};

// 🔥 v7.5: 文件排序
pub use file_sorter::{
    sort_by_name, sort_by_size_ascending, sort_by_size_descending, FileInfo, FileSorter,
    SortStrategy,
};

// 🔥 v7.6: MS-SSIM智能采样
pub use msssim_sampling::{SamplingConfig, SamplingStrategy};

// 🔥 v7.6: MS-SSIM心跳检测
pub use msssim_heartbeat::Heartbeat;

// 🔥 v7.6: MS-SSIM进度监控
pub use msssim_progress::MsssimProgressMonitor;

// 🔥 v7.6: MS-SSIM并行计算
pub use msssim_parallel::{MsssimResult, ParallelMsssimCalculator};

// 🔥 v7.7: 通用心跳系统
pub use heartbeat_manager::{HeartbeatManager, ProgressBarGuard};
pub use universal_heartbeat::{HeartbeatConfig, HeartbeatGuard, UniversalHeartbeat};

// 🔥 v7.8: 统一日志系统
pub use logging::{
    flush_logs, init_logging, log_external_tool, log_operation_end, log_operation_start, LogConfig,
};

// 🔥 v7.8: 通用工具函数模块
pub use common_utils::{
    // 文件操作
    compute_relative_path,
    copy_file_with_context,
    ensure_dir_exists,
    ensure_parent_dir_exists,
    get_extension_lowercase,
    has_extension,
    is_hidden_file,
    // 字符串处理
    extract_digits,
    normalize_path_string,
    parse_float_or_default,
    truncate_string,
    // 命令执行
    execute_command_with_logging,
    format_command_string,
    get_command_version,
    is_command_available,
};

// 🔥 v7.9: Smart thread management for Apple Silicon
pub use thread_manager::{
    calculate_optimal_threads,
    disable_multi_instance_mode,
    enable_multi_instance_mode,
    get_ffmpeg_threads,
    get_optimal_threads,
    get_rsync_path,
    get_rsync_version,
    is_multi_instance,
    ThreadConfig,
};

