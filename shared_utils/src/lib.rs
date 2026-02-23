//! Shared Utilities for modern_format_boost tools
//!
//! This crate provides common functionality shared across imgquality, vidquality, and vid-hevc:
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
pub mod report;
pub mod safety;
pub mod ssim_mapping;
pub mod tools;
pub mod video;
pub mod video_explorer;
#[cfg(test)]
mod video_explorer_tests;
pub mod video_quality_detector;
pub mod xmp_merger;
pub mod ffmpeg_process;
pub mod crf_constants;
pub mod float_compare;
pub mod path_validator;
pub mod thread_manager;

pub mod path_safety;
pub use path_safety::safe_path_arg;
pub mod ffprobe_json;
pub mod stream_size;
pub mod pure_media_verifier;
pub mod quality_verifier_enhanced;
pub mod types;
pub mod app_error;
pub mod file_copier;
pub mod smart_file_copier;

pub mod progress_mode;

pub mod unified_progress;
pub use unified_progress::UnifiedProgressBar;

pub mod file_sorter;

pub mod msssim_sampling;

pub mod msssim_heartbeat;

pub mod msssim_progress;

pub mod msssim_parallel;

pub mod heartbeat_manager;
pub mod universal_heartbeat;

pub mod logging;

pub mod common_utils;

pub mod jxl_utils;
pub mod avif_av1_health;

pub mod x265_encoder;

pub mod vmaf_standalone;

pub mod cli_runner;

pub mod errors;

pub mod conversion_types;

pub mod video_detection;

pub mod img_errors;
pub mod image_analyzer;
pub mod image_detection;
pub mod image_formats;
pub mod image_heic_analysis;
pub mod image_jpeg_analysis;
pub mod image_metrics;
pub mod image_quality_core;
pub mod image_recommender;

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
pub use metadata::{
    apply_saved_timestamps_to_dst, copy_metadata, preserve_directory_metadata,
    preserve_directory_metadata_with_log, preserve_metadata, preserve_pro,
    restore_directory_timestamps, restore_timestamps_from_source_to_output,
    save_directory_timestamps,
};
pub use progress::{
    create_compact_progress_bar,
    create_detailed_progress_bar,
    create_multi_progress,
    create_progress_bar,
    create_progress_bar_with_eta,
    create_spinner,
    format_bytes,
    format_duration,
    BatchProgress,
    CoarseProgressBar,
    DetailedCoarseProgressBar,
    ExploreLogger,
    ExploreProgress,
    FixedBottomProgress,
    GlobalProgressManager,
    ProgressStats,
    SmartProgressBar,
};
pub use quality_matcher::{
    calculate_av1_crf,
    calculate_av1_crf_with_options,
    calculate_hevc_crf,
    calculate_hevc_crf_with_options,
    calculate_jxl_distance,
    calculate_jxl_distance_with_options,
    from_image_analysis,
    from_video_detection,
    log_quality_analysis,
    parse_source_codec,
    should_skip_image_format,
    should_skip_video_codec,
    should_skip_video_codec_apple_compat,
    AnalysisDetails,
    ContentType,
    EncoderType,
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
    analyze_image_quality,
    ImageContentType,
    ImageQualityAnalysis,
    RoutingDecision,
};

pub use video_quality_detector::{
    analyze_video_quality,
    to_quality_analysis as video_to_quality_analysis,
    ChromaSubsampling,
    CompressionLevel,
    VideoCodecType,
    VideoContentType,
    VideoQualityAnalysis,
    VideoRoutingDecision,
};

pub use video_explorer::{
    calculate_metadata_margin,
    can_compress_with_metadata,
    compression_target_size,
    detect_metadata_size,
    explore_av1,
    explore_av1_compress_only,
    explore_av1_compress_with_quality,
    explore_av1_quality_match,
    explore_av1_size_only,
    explore_compress_only,
    explore_compress_with_quality,
    explore_hevc,
    explore_hevc_compress_only,
    explore_hevc_compress_with_quality,
    explore_hevc_quality_match,
    explore_hevc_size_only,
    explore_precise_quality_match,
    explore_precise_quality_match_with_compression,
    explore_quality_match,
    explore_size_only,
    precision,
    precision::SearchPhase,
    precision::ThreePhaseSearch,
    pure_video_size,
    verify_compression_precise,
    verify_compression_simple,
    CompressionVerifyStrategy,
    EncoderPreset,
    ExploreConfig,
    ExploreMode,
    ExploreResult,
    IterationMetrics,
    QualityThresholds,
    SsimSource,
    TransparencyReport,
    VideoEncoder,
    VideoExplorer,
    METADATA_MARGIN_MAX,
    METADATA_MARGIN_MIN,
    METADATA_MARGIN_PERCENT,
    SMALL_FILE_THRESHOLD,
};

#[allow(deprecated)]
pub use video_explorer::{
    explore_compress_only_gpu, explore_compress_with_quality_gpu,
    explore_precise_quality_match_gpu, explore_precise_quality_match_with_compression_gpu,
    explore_quality_match_gpu, explore_size_only_gpu,
};

#[allow(deprecated)]
pub use video_explorer::full_explore;
#[allow(deprecated)]
pub use video_explorer::quick_explore;

pub use checkpoint::{safe_delete_original, verify_output_integrity, CheckpointManager};

pub use xmp_merger::{
    merge_xmp_for_copied_file,
    MergeResult,
    MergeSummary,
    XmpFile,
    XmpMerger,
    XmpMergerConfig,
};

pub use flag_validator::{
    print_flag_help, validate_flags, validate_flags_result, validate_flags_result_with_ultimate,
    validate_flags_with_ultimate, FlagMode, FlagValidation,
};

pub use gpu_accel::{
    estimate_cpu_search_center,
    get_cpu_search_range_from_gpu,
    gpu_boundary_to_cpu_range,
    gpu_coarse_search,
    gpu_coarse_search_with_log,
    CrfMapping,
    GpuAccel,
    GpuCoarseConfig,
    GpuCoarseResult,
    GpuEncoder,
    GpuType,
};

pub use video_explorer::{
    explore_av1_with_gpu_coarse,
    explore_hevc_with_gpu_coarse,
    explore_hevc_with_gpu_coarse_full,
    explore_hevc_with_gpu_coarse_ultimate,
    explore_with_gpu_coarse_search,
};

pub use modern_ui::{
    colors, format_size, format_size_change, format_size_diff, print_error, print_info,
    print_result_box, print_stage, print_substage, print_success, print_warning, progress_style,
    render_colored_progress, render_progress_bar, spinner_dots, spinner_frame, symbols,
    ExploreProgressState, ProgressStyle,
};

pub use lru_cache::{CacheEntry, LruCache, SerializableCache};

pub use error_handler::{handle_error, ErrorAction, ErrorCategory};

pub use ssim_mapping::{MappingPoint, PsnrSsimMapping};

pub use explore_strategy::{
    create_strategy, CompressOnlyStrategy, CompressWithQualityStrategy,
    ExploreContext, ExploreStrategy, PreciseQualityMatchStrategy,
    PreciseQualityMatchWithCompressionStrategy, ProgressConfig, QualityMatchStrategy,
    SizeOnlyStrategy, SsimResult,
};

pub use ffmpeg_process::{
    format_ffmpeg_error, is_recoverable_error, FfmpegProcess, FfmpegProgressParser,
};

pub use float_compare::{
    approx_eq_crf,
    approx_eq_f32,
    approx_eq_f64,
    approx_eq_psnr,
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
    F64_EPSILON,
    PSNR_EPSILON,
    SSIM_EPSILON as FLOAT_SSIM_EPSILON,
};

pub use path_validator::{validate_path, validate_paths, PathValidationError};

pub use crf_constants::{
    AV1_CRF_DEFAULT,
    AV1_CRF_MAX,
    AV1_CRF_MIN,
    AV1_CRF_PRACTICAL_MAX,
    AV1_CRF_VISUALLY_LOSSLESS,
    CRF_CACHE_KEY_MULTIPLIER,
    CRF_CACHE_MAX_VALID,
    EMERGENCY_MAX_ITERATIONS as CRF_EMERGENCY_MAX_ITERATIONS,
    HEVC_CRF_DEFAULT,
    HEVC_CRF_MAX,
    HEVC_CRF_MIN,
    HEVC_CRF_PRACTICAL_MAX,
    HEVC_CRF_VISUALLY_LOSSLESS,
    NORMAL_MAX_ITERATIONS,
    VP9_CRF_DEFAULT,
    VP9_CRF_MAX,
    VP9_CRF_MIN,
    X264_CRF_DEFAULT,
    X264_CRF_MAX,
    X264_CRF_MIN,
};

pub use ffprobe_json::{extract_color_info as ffprobe_extract_color_info, ColorInfo};

pub use stream_size::{
    extract_stream_sizes, get_container_overhead_percent, ExtractionMethod, StreamSizeInfo,
    DEFAULT_OVERHEAD_PERCENT, MKV_OVERHEAD_PERCENT, MOV_OVERHEAD_PERCENT, MP4_OVERHEAD_PERCENT,
};

pub use pure_media_verifier::{
    is_video_compressed, verify_pure_media_compression, video_compression_ratio,
    PureMediaVerifyResult,
};

pub use types::{
    Av1Encoder, Crf, CrfError, EncoderBounds, FileSize, HevcEncoder, IterationError,
    IterationGuard, Ssim, SsimError, Vp9Encoder, X264Encoder, SSIM_EPSILON,
};

pub use app_error::AppError;

pub use file_copier::{
    copy_unsupported_files, count_files as count_all_files, verify_output_completeness, CopyResult,
    FileStats, VerifyResult, IMAGE_EXTENSIONS_ANALYZE, SIDECAR_EXTENSIONS,
    SUPPORTED_IMAGE_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS,
};
pub use smart_file_copier::{
    copy_on_skip_or_fail, fix_extension_if_mismatch, smart_copy_with_structure,
};

pub use file_sorter::{
    sort_by_name, sort_by_size_ascending, sort_by_size_descending, FileInfo, FileSorter,
    SortStrategy,
};

pub use msssim_sampling::{SamplingConfig, SamplingStrategy};

pub use msssim_heartbeat::Heartbeat;

pub use msssim_progress::MsssimProgressMonitor;

pub use msssim_parallel::{MsssimResult, ParallelMsssimCalculator};

pub use heartbeat_manager::{HeartbeatManager, ProgressBarGuard};
pub use universal_heartbeat::{HeartbeatConfig, HeartbeatGuard, UniversalHeartbeat};

pub use logging::{
    flush_logs, init_logging, log_external_tool, log_operation_end, log_operation_start, LogConfig,
};

pub use common_utils::{
    compute_relative_path,
    copy_file_with_context,
    ensure_dir_exists,
    ensure_parent_dir_exists,
    execute_command_with_logging,
    extract_digits,
    extract_suggested_extension,
    format_command_string,
    get_command_version,
    get_extension_lowercase,
    has_extension,
    is_command_available,
    is_hidden_file,
    normalize_path_string,
    parse_float_or_default,
    truncate_string,
};

pub use thread_manager::{
    calculate_optimal_threads, disable_multi_instance_mode, enable_multi_instance_mode,
    get_ffmpeg_threads, get_optimal_threads, get_rsync_path, get_rsync_version, is_multi_instance,
    ThreadConfig,
};
