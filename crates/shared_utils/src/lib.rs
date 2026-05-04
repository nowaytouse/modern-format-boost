//! Shared Utilities for `modern_format_boost` tools
#![allow(clippy::multiple_crate_versions)]
//!
//! This crate provides common functionality shared across `img` and `vid`:
//! - Progress bar with ETA
//! - Safety checks (dangerous directory detection)
//! - Batch processing utilities
//! - Common logging and reporting
//! - `FFprobe` wrapper for video analysis
//! - External tools detection
//! - Codec information
//! - Metadata preservation (EXIF/IPTC/xattr/timestamps/ACL)
//! - Conversion utilities (`ConversionResult`, `ConvertOptions`, anti-duplicate)
//! - Date analysis (deep EXIF/XMP date extraction)
//! - Quality matching (unified CRF/distance calculation for all encoders)
//! - Unified version management (program, cache, schema versions)

#![warn(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::manual_let_else,
    clippy::items_after_statements
)]

#[cfg(feature = "high-precision")]
pub use rug::Rational;

#[cfg(not(feature = "high-precision"))]
extern crate self as rug;

#[cfg(not(feature = "high-precision"))]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Rational(f64);

#[cfg(not(feature = "high-precision"))]
impl Rational {
    #[must_use]
    pub fn from_f64(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    #[must_use]
    pub const fn to_f64(self) -> f64 {
        self.0
    }
}

#[cfg(not(feature = "high-precision"))]
impl std::fmt::Display for Rational {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(not(feature = "high-precision"))]
macro_rules! impl_rational_from_int_lossless {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for Rational {
                fn from(value: $ty) -> Self {
                    Self(f64::from(value))
                }
            }
        )+
    };
}

#[cfg(not(feature = "high-precision"))]
macro_rules! impl_rational_from_int_lossy {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for Rational {
                fn from(value: $ty) -> Self {
                    #[allow(clippy::cast_precision_loss)]
                    Self(value as f64)
                }
            }
        )+
    };
}

#[cfg(not(feature = "high-precision"))]
impl_rational_from_int_lossless!(u8, u16, u32, i8, i16, i32);
#[cfg(not(feature = "high-precision"))]
impl_rational_from_int_lossy!(u64, usize, i64, isize);

#[cfg(not(feature = "high-precision"))]
macro_rules! impl_rational_cmp_int_lossless {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl PartialEq<$ty> for Rational {
                fn eq(&self, other: &$ty) -> bool {
                    self.0 == f64::from(*other)
                }
            }

            impl PartialOrd<$ty> for Rational {
                fn partial_cmp(&self, other: &$ty) -> Option<std::cmp::Ordering> {
                    self.0.partial_cmp(&f64::from(*other))
                }
            }
        )+
    };
}

#[cfg(not(feature = "high-precision"))]
macro_rules! impl_rational_cmp_int_lossy {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl PartialEq<$ty> for Rational {
                fn eq(&self, other: &$ty) -> bool {
                    #[allow(clippy::cast_precision_loss)]
                    { self.0 == *other as f64 }
                }
            }

            impl PartialOrd<$ty> for Rational {
                fn partial_cmp(&self, other: &$ty) -> Option<std::cmp::Ordering> {
                    #[allow(clippy::cast_precision_loss)]
                    { self.0.partial_cmp(&(*other as f64)) }
                }
            }
        )+
    };
}

#[cfg(not(feature = "high-precision"))]
impl_rational_cmp_int_lossless!(u8, u16, u32, i8, i16, i32);
#[cfg(not(feature = "high-precision"))]
impl_rational_cmp_int_lossy!(u64, usize, i64, isize);

#[cfg(not(feature = "high-precision"))]
macro_rules! impl_rational_from_pair {
    ($(($num:ty, $den:ty)),+ $(,)?) => {
        $(
            impl From<($num, $den)> for Rational {
                fn from((numerator, denominator): ($num, $den)) -> Self {
                    if denominator == 0 {
                        Self(1.0)
                    } else {
                        Self(numerator as f64 / denominator as f64)
                    }
                }
            }
        )+
    };
}

#[cfg(not(feature = "high-precision"))]
impl_rational_from_pair!((u64, u64), (u32, u32), (usize, usize), (i32, i32));

#[cfg(not(feature = "high-precision"))]
macro_rules! impl_rational_op {
    ($trait:ident, $method:ident, $op:tt) => {
        impl std::ops::$trait for Rational {
            type Output = Self;

            fn $method(self, rhs: Self) -> Self::Output {
                Self(self.0 $op rhs.0)
            }
        }
    };
}

#[cfg(not(feature = "high-precision"))]
impl_rational_op!(Add, add, +);
#[cfg(not(feature = "high-precision"))]
impl_rational_op!(Sub, sub, -);
#[cfg(not(feature = "high-precision"))]
impl_rational_op!(Mul, mul, *);
#[cfg(not(feature = "high-precision"))]
impl_rational_op!(Div, div, /);

#[cfg(not(feature = "high-precision"))]
impl std::ops::AddAssign for Rational {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

#[cfg(not(feature = "high-precision"))]
impl std::ops::SubAssign for Rational {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

#[cfg(not(feature = "high-precision"))]
impl std::ops::MulAssign for Rational {
    fn mul_assign(&mut self, rhs: Self) {
        self.0 *= rhs.0;
    }
}

#[cfg(not(feature = "high-precision"))]
impl std::ops::DivAssign for Rational {
    fn div_assign(&mut self, rhs: Self) {
        self.0 /= rhs.0;
    }
}

/// Cache system for analysis results.
pub mod analysis_cache;
/// Batch processing engine for handling multiple files.
pub mod batch;
/// Unified candidate ranking and quality comparators.
pub mod candidate_comparator;
/// Checkpoint and integrity management for conversions.
pub mod checkpoint;
/// Video and audio codec definitions and helpers.
pub mod codecs;
/// System-wide constants and thresholds.
pub mod constants;
/// Core conversion logic and options.
pub mod conversion;
/// CRF (Constant Rate Factor) relacionadas constants.
pub mod crf_constants;
/// Metadata-based date extraction and analysis.
pub mod date_analysis;
/// Centralized error handling and recovery logic.
pub mod error_handler;
/// CRF exploration strategies.
pub mod explore_strategy;
/// High-level builder for `FFmpeg` commands.
pub mod ffmpeg_builder;
/// `FFmpeg` process management and progress parsing.
pub mod ffmpeg_process;
/// `FFprobe` wrapper for media identification.
pub mod ffprobe;
/// Command-line flag validation logic.
pub mod flag_validator;
/// Robust floating-point comparison utilities.
pub mod float_compare;
/// `GPU` acceleration detection and calibration.
pub mod gpu_accel;
/// High-level builders for image processing tools.
pub mod image_builders;
/// Image quality analytics and scoring.
pub mod image_quality_detector;
/// High-level builder for `cjxl` and `djxl` commands.
pub mod jxl_builder;
/// JXL distance exploration engine for Ultimate Mode.
pub mod jxl_explorer;
/// Thread-safe `LRU` cache implementation.
pub mod lru_cache;
/// Universal metadata preservation (EXIF, IPTC, xattr, etc.).
pub mod metadata;
/// Modern terminal UI components and styling.
pub mod modern_ui;
/// Path safety and validation utilities.
pub mod path_validator;
/// Progress bar and status reporting system.
pub mod progress;
/// Intelligent quality matching engine.
pub mod quality_matcher;
/// Summary and detailed reporting generation.
pub mod report;
/// High-risk directory and operation safety checks.
pub mod safety;
/// SSIM-to-CRF mapping and prediction.
pub mod ssim_mapping;
/// Platform-aware thread and workload management.
pub mod thread_manager;
/// Path-based tool discovery and health checks.
pub mod tools;
/// Unified error hierarchy for the entire workspace.
pub mod unified_error;
/// Versioning and schema management.
pub mod version;
/// Video-specific processing utilities.
pub mod video;
/// The core video quality exploration engine.
pub mod video_explorer;
// #[cfg(test)]
// mod video_explorer_tests;
// #[cfg(test)]
// mod image_detection_tests;
/// Video quality analytics and scoring.
pub mod video_quality_detector;
/// Metadata-aware `XMP` merging logic.
pub mod xmp_merger;

/// Path-safe argument formatting.
pub mod path_safety;
/// Global process locking for concurrency safety.
pub mod process_lock;
pub use path_safety::safe_path_arg;
/// Application-level error definitions.
pub mod app_error;
/// `FFprobe` `JSON` schema and parsing logic.
pub mod ffprobe_json;
/// Robust file copying with retry logic.
pub mod file_copier;
/// High-bit-depth `HDR` image decoding.
pub mod hdr_decode;
/// `HDR` and Color Space utility functions.
pub mod hdr_utils;
/// Safe numeric casting with saturation and range checks.
pub mod numeric_cast;
/// Verification of byte-exact media stream identity.
pub mod pure_media_verifier;
/// Post-encode quality verification engine.
pub mod quality_verifier_enhanced;
/// Structure-preserving file copying.
pub mod smart_file_copier;
/// Media stream and header size analysis.
pub mod stream_size;
/// System resources and memory monitoring.
pub mod system_memory;
/// Low-level tool process builders.
pub mod tool_builders;
/// Shared data types and structures.
pub mod types;
pub use tool_builders::X265Builder;

/// User-facing progress mode configuration.
pub mod progress_mode;

/// Global `Ctrl+C` handling and guard system.
pub mod ctrlc_guard;
#[cfg(test)]
mod ctrlc_guard_tests;

/// Unified progress bar interface.
pub mod unified_progress;
pub use unified_progress::UnifiedProgressBar;

/// Intelligent file sorting and prioritization.
pub mod file_sorter;

/// `MS-SSIM` temporal sampling strategies.
pub mod msssim_sampling;

/// `MS-SSIM` specific progress reporting.
pub mod msssim_progress;

/// Parallelized `MS-SSIM` calculation engine.
pub mod msssim_parallel;

/// Asynchronous and robust error logging system.
pub mod error_logging;
/// Shared terminal and file logging initialization.
pub mod logging;

/// General-purpose utility functions.
pub mod common_utils;

/// `AVIF` and `AV1` bitstream health verification.
pub mod avif_av1_health;
/// Primitive I/O and filesystem helpers.
pub mod io_utils;
/// `JXL` specific bitstream and identification utilities.
pub mod jxl_utils;

/// Generic `x265` encoder interface.
pub mod x265_encoder;
/// Shared x265 parameter policy helpers.
pub mod x265_params;

/// Standalone `VMAF` calculation wrapper.
pub mod vmaf_standalone;

/// External tool execution and exit code handling.
pub mod cli_runner;

/// Centralized conversion types and enums.
pub mod conversion_types;

/// Audio and subtitle stream passthrough logic.
pub mod media_passthrough;
/// Advanced video stream detection and analysis.
pub mod video_detection;
pub use media_passthrough::{audio_args_for_container, subtitle_args_for_container};

/// Shared database interface for quality matching.
pub mod database;
/// Depth map extraction and embedding.
#[cfg(feature = "jpegxl-ffi")]
pub mod depth_channel;
/// Gainmap to `HDR` synthesis pipeline.
pub mod hdr_synthesis;
/// High-level image analyzer interface.
pub mod image_analyzer;
/// Image type and format detection.
pub mod image_detection;
/// Image format definitions and capabilities.
pub mod image_formats;
/// Apple `HEIC` specific analysis.
pub mod image_heic_analysis;
/// `JPEG` specific bitstream and metadata analysis.
pub mod image_jpeg_analysis;
/// Image quality metrics and score calculation.
pub mod image_metrics;
/// Quality database for image matching.
pub mod image_quality_db;
/// Quality-preserving image conversion recommender.
pub mod image_recommender;
/// Image-specific error types.
pub mod img_errors;
/// Apple Live Photo identification and grouping.
pub mod live_photo;
/// Loop-intent identification and 7-layer decision system.
pub mod loop_intent;
/// Database-backed media indexing types.
pub mod media_index_types;
/// Unified media metadata extraction.
pub mod media_meta_utils;
/// Content-based penetrating detection (bypasses fake metadata).
pub mod media_penetration;
/// Quality-preserving video conversion recommender.
pub mod video_recommender;

pub use blake3;
pub use database::{lookup_similar_samples, SampleMatch};
#[cfg(feature = "jpegxl-ffi")]
pub use depth_channel::{
    encode_jxl_depth_fallback, encode_jxl_with_depth, extract_depth_from_heic, DepthMap, DepthType,
};
pub use image_quality_db::{lookup_image_quality, QualityScore};
pub use loop_intent::{
    apply_apple_compat_modern_animation_policy, assess_loop_intent, assess_loop_intent_from_meta,
    assess_loop_intent_from_probe, identify_loop_intent, is_lossless_exploration_safe,
    should_use_gif_fast_path, LoopIntentVerdict, LoopMeta,
};

pub use hdr_synthesis::{
    convert_heic_with_gainmap_to_jxl_hdr, convert_ultrahdr_jpeg_to_jxl_hdr,
    convert_ultrahdr_jpeg_to_jxl_migration, GainMapParams, HdrIntermediateFormat,
};

pub use batch::*;
pub use codecs::*;
pub use constants::*;
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
    create_compact_progress_bar, create_detailed_progress_bar, create_multi_progress,
    create_progress_bar, create_progress_bar_with_eta, create_spinner, format_bytes,
    format_duration, BatchProgress, CoarseProgressBar, DetailedCoarseProgressBar, ExploreLogger,
    ExploreProgress, FixedBottomProgress, GlobalProgressManager, ProgressStats, SmartProgressBar,
};
pub use quality_matcher::{
    calculate_av1_crf, calculate_av1_crf_with_options, calculate_hevc_crf,
    calculate_hevc_crf_with_options, calculate_jxl_distance, calculate_jxl_distance_with_options,
    from_image_analysis, from_video_detection, is_apple_incompatible_video_codec,
    is_apple_native_format, is_size_guard_active, log_quality_analysis, parse_source_codec,
    should_keep_apple_fallback_hevc_output, should_skip_image_format, should_skip_video_codec,
    should_skip_video_codec_apple_compat, AnalysisDetails, AppleFallbackKeepRequest, ContentType,
    EncoderType, MatchMode, MatchedQuality, QualityAnalysis, QualityBias, SkipDecision,
    SourceCodec, VideoAnalysisBuilder,
};
pub use report::*;
pub use safety::*;
pub use tools::*;
pub use video::*;

pub use image_quality_detector::{
    analyze_image_quality, analyze_image_quality_from_path, log_media_info_for_image_quality,
    ImageContentType, ImageQualityAnalysis,
};

pub use video_quality_detector::{
    analyze_video_quality, analyze_video_quality_from_detection, log_media_info_for_quality,
    to_quality_analysis as video_to_quality_analysis, ChromaSubsampling, CompressionLevel,
    VideoCodecType, VideoContentType, VideoQualityAnalysis,
};

pub use video_explorer::{
    calculate_metadata_margin, can_compress_with_metadata, compression_target_size,
    detect_metadata_size, explore_av1, explore_av1_compress_only,
    explore_av1_compress_with_quality, explore_av1_quality_match, explore_av1_size_only,
    explore_compress_only, explore_compress_with_quality, explore_hevc, explore_hevc_compress_only,
    explore_hevc_compress_with_quality, explore_hevc_quality_match, explore_hevc_size_only,
    explore_precise_quality_match, explore_precise_quality_match_with_compression,
    explore_quality_match, explore_size_only, precision, precision::SearchPhase,
    precision::ThreePhaseSearch, pure_video_size, verify_compression_precise,
    verify_compression_simple, CompressionVerifyStrategy, ExploreConfig, ExploreMode,
    ExploreResult, IterationMetrics, QualityThresholds, SsimSource, TransparencyReport,
    VideoEncoder, VideoExplorer, METADATA_MARGIN_MAX, METADATA_MARGIN_MIN, METADATA_MARGIN_PERCENT,
    SMALL_FILE_THRESHOLD,
};

pub use types::EncoderPreset;

pub use video_explorer::{
    explore_compress_only_gpu, explore_compress_with_quality_gpu,
    explore_precise_quality_match_gpu, explore_precise_quality_match_with_compression_gpu,
    explore_quality_match_gpu, explore_size_only_gpu,
};

pub use checkpoint::{safe_delete_original, verify_output_integrity, CheckpointManager};

pub use quality_verifier_enhanced::{
    verify_after_encode, verify_output_file, EnhancedVerifyResult, VerifyOptions,
    DEFAULT_MIN_FILE_SIZE,
};

pub use xmp_merger::{
    merge_xmp_for_copied_file, MergeResult, MergeSummary, XmpFile, XmpMerger, XmpMergerConfig,
};

pub use flag_validator::{
    print_flag_help, validate_flags, validate_flags_result, validate_flags_result_with_ultimate,
    validate_flags_with_ultimate, FlagMode, FlagRequest, FlagValidation,
};

pub use gpu_accel::{
    estimate_cpu_search_center, get_cpu_search_range_from_gpu, gpu_boundary_to_cpu_range,
    gpu_coarse_search, gpu_coarse_search_with_log, CrfMapping, GpuAccel, GpuCoarseConfig,
    GpuCoarseResult, GpuEncoder, GpuType,
};

pub use video_explorer::{
    explore_av1_with_gpu, explore_hevc_with_gpu, explore_with_gpu_coarse_search, is_gif_magic,
    GpuSearchRequest,
};

pub use modern_ui::{
    colors, format_size, format_size_change, format_size_diff, print_error, print_info,
    print_result_box, print_stage, print_substage, print_success, print_warning, progress_style,
    render_colored_progress, render_progress_bar, spinner_dots, spinner_frame, symbols,
    ExploreProgressState, ProgressStyle,
};

pub use lru_cache::{CacheEntry, LruCache, SerializableCache};

pub use error_handler::{handle_error, ErrorAction, ErrorCategory};

// Re-export unified error types
pub use unified_error::{
    ImgResult, Result as UnifiedResult, UnifiedError, VidQualityError, VidResult,
};

pub use ssim_mapping::{MappingPoint, PsnrSsimMapping};

pub use explore_strategy::{
    create_strategy, CompressOnlyStrategy, CompressWithQualityStrategy, ExploreContext,
    ExploreStrategy, PreciseQualityMatchStrategy, PreciseQualityMatchWithCompressionStrategy,
    ProgressConfig, QualityMatchStrategy, SizeOnlyStrategy, SsimResult,
};

pub use ffmpeg_builder::{
    FfmpegBuilder, FfprobeBuilder, PixFmt, StreamType, VideoCodec, VideoProfile,
};
pub use ffmpeg_process::{
    format_ffmpeg_error, is_recoverable_error, FfmpegProcess, FfmpegProgressParser,
};
pub use image_builders::{
    AvifencBuilder, DwebpBuilder, ExiftoolBuilder, GifskiBuilder, MagickBuilder, SipsBuilder,
    WebpmuxBuilder,
};
pub use jxl_builder::{CjxlBuilder, DjxlBuilder};

pub use float_compare::{
    approx_eq_crf, approx_eq_f32, approx_eq_f64, approx_eq_psnr, approx_eq_ssim, approx_ge_f64,
    approx_le_f64, approx_zero_f32, approx_zero_f64, crf_in_range, ssim_below_threshold,
    ssim_meets_threshold, CRF_EPSILON, F32_EPSILON, F64_EPSILON, PSNR_EPSILON,
    SSIM_EPSILON as FLOAT_SSIM_EPSILON,
};

pub use path_validator::{validate_path, validate_paths, PathValidationError};

pub use crf_constants::{
    AV1_CRF_DEFAULT, AV1_CRF_MAX, AV1_CRF_MIN, AV1_CRF_PRACTICAL_MAX, AV1_CRF_VISUALLY_LOSSLESS,
    CRF_CACHE_KEY_MULTIPLIER, CRF_CACHE_MAX_VALID,
    EMERGENCY_MAX_ITERATIONS as CRF_EMERGENCY_MAX_ITERATIONS, HEVC_CRF_DEFAULT, HEVC_CRF_MAX,
    HEVC_CRF_MIN, HEVC_CRF_PRACTICAL_MAX, HEVC_CRF_VISUALLY_LOSSLESS, NORMAL_MAX_ITERATIONS,
    VP9_CRF_DEFAULT, VP9_CRF_MAX, VP9_CRF_MIN, X264_CRF_DEFAULT, X264_CRF_MAX, X264_CRF_MIN,
};

pub use ffprobe_json::{extract_color_info as ffprobe_extract_color_info, ColorInfo};

pub use hdr_decode::{decode_hdr_image_to_png16, needs_hdr_decode};

pub use hdr_utils::{
    color_info_to_cicp, color_info_to_ffmpeg_args, color_info_to_x265_hdr_params,
    dv_x265_profile_string, extract_dv_rpu, extract_hevc_bitstream, get_hdr_pix_fmt,
    is_dovi_tool_available, should_use_hdr_decode,
};

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
    FileStats, VerifyResult, IMAGE_EXTENSIONS_ANALYZE, IMAGE_EXTENSIONS_FOR_CONVERT,
    SIDECAR_EXTENSIONS, SUPPORTED_IMAGE_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS,
};
pub use smart_file_copier::{
    check_extension_mismatch_readonly, copy_on_skip_or_fail, fix_extension_if_mismatch,
    smart_copy_with_structure,
};

pub use live_photo::is_live_photo;
pub use process_lock::{acquire_dir_lock, get_mfb_tmp_dir, hash_path_to_hex, init_ghost_mode};

pub use file_sorter::{
    sort_by_name, sort_by_size_ascending, sort_by_size_descending, FileInfo, FileSorter,
    SortStrategy,
};

pub use msssim_sampling::{SamplingConfig, SamplingStrategy};

pub use msssim_progress::MsssimProgressMonitor;

pub use msssim_parallel::{MsssimResult, ParallelMsssimCalculator};

pub use logging::{
    flush_logs, init_logging, log_external_tool, log_operation_end, log_operation_start, LogConfig,
};

// Enhanced logging with 24-bit color support
pub mod enhanced_logging;
pub use enhanced_logging::{
    init_enhanced_logging, LogLevel, LogRouter, LogTarget, TerminalColor, UpstreamToolLogger,
};

// Modern terminal logging with color safety
pub mod terminal_logging;
pub use terminal_logging::{init_terminal_logger, terminal_logger, ColorGuard, TerminalLogger};

pub use common_utils::{
    compute_relative_path, copy_file_with_context, ensure_dir_exists, ensure_parent_dir_exists,
    execute_command_with_logging, extract_digits, extract_suggested_extension,
    format_command_string, get_command_version, get_extension_lowercase, has_extension,
    is_command_available, is_hidden_file, normalize_path_string, parse_float_or_default,
    truncate_string,
};

pub use thread_manager::{
    calculate_optimal_threads, disable_multi_instance_mode, enable_multi_instance_mode,
    get_ffmpeg_threads, get_optimal_threads, get_rsync_path, get_rsync_version, is_multi_instance,
    memory_cap_hint, ThreadConfig,
};

pub use version::{cache_algorithm_version, VersionInfo, CACHE_SCHEMA_VERSION, PROGRAM_VERSION};
