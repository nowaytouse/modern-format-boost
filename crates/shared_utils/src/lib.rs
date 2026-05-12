#![allow(internal_features)]
#![feature(core_intrinsics, portable_simd, try_blocks, specialization)]
#![allow(incomplete_features)]
#![deny(clippy::all, clippy::pedantic, clippy::nursery)]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::panic))]
#![allow(
    clippy::multiple_crate_versions,
    reason = "Legitimate deviation from standard linting rules justified by specific project architecture."
)]
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
//! - Conversion utilities (`Conversion..Result`, `ConvertOptions`, anti-duplicate)
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
#[derive(Debug, Clone, Copy, PartialEq)]
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

    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.0.is_finite()
    }

    #[must_use]
    pub const fn mul_add(self, a: Self, b: Self) -> Self {
        Self(self.0.mul_add(a.0, b.0))
    }

    #[must_use]
    pub const fn abs(self) -> Self {
        Self(self.0.abs())
    }

    #[must_use]
    pub const fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }
}

#[cfg(not(feature = "high-precision"))]
impl std::fmt::Display for Rational {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(not(feature = "high-precision"))]
impl std::ops::Neg for Rational {
    type Output = Self;

    fn neg(self) -> Self {
        Self(-self.0)
    }
}

#[cfg(not(feature = "high-precision"))]
macro_rules! impl_rational_from_int {
    ($($lossless:ty),* ; $($lossy:ty),*) => {
        $(
            impl From<$lossless> for Rational {
                fn from(value: $lossless) -> Self {
                    Self(f64::from(value))
                }
            }
        )*
        $(
            impl From<$lossy> for Rational {
                fn from(value: $lossy) -> Self {
                    Self(crate::numeric_cast::i64_to_f64(value as i64))
                }
            }
        )*
    };
}

// Removed impl_rational_from_int_lossy and impl_rational_cmp_int_lossy to prevent silent precision loss on 64-bit integers.

#[cfg(not(feature = "high-precision"))]
macro_rules! impl_rational_from_pair {
    ($(($num:ty, $den:ty)),+ $(,)?) => {
        $(
            impl From<($num, $den)> for Rational {
                fn from((numerator, denominator): ($num, $den)) -> Self {
                    if denominator == 0 {
                        Self(1.0)
                    } else {
                        Self(crate::numeric_cast::i64_to_f64(numerator as i64) / crate::numeric_cast::i64_to_f64(denominator as i64))
                    }
                }
            }
        )+
    };
}

#[cfg(not(feature = "high-precision"))]
impl_rational_from_pair!((u64, u64), (u32, u32), (usize, usize), (i32, i32));

#[cfg(not(feature = "high-precision"))]
impl_rational_from_int!(i32, u32, u8; i64, u64, usize);

#[cfg(not(feature = "high-precision"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Integer(i64);

#[cfg(not(feature = "high-precision"))]
impl Integer {
    #[must_use]
    pub const fn from_f64(v: f64) -> Self {
        Self(crate::numeric_cast::raw::f64_to_i64(v))
    }
}

#[cfg(not(feature = "high-precision"))]
impl From<u64> for Integer {
    fn from(v: u64) -> Self {
        Self(crate::numeric_cast::raw::u64_to_i64(v))
    }
}

#[cfg(not(feature = "high-precision"))]
impl From<i32> for Integer {
    fn from(v: i32) -> Self {
        Self(i64::from(v))
    }
}

#[cfg(not(feature = "high-precision"))]
impl From<usize> for Integer {
    fn from(v: usize) -> Self {
        Self(crate::numeric_cast::usize_to_i64_sat(v))
    }
}

#[cfg(not(feature = "high-precision"))]
impl From<Integer> for Rational {
    fn from(v: Integer) -> Self {
        Self(crate::numeric_cast::i64_to_f64(v.0))
    }
}

#[cfg(not(feature = "high-precision"))]
impl std::ops::Mul<Self> for Integer {
    type Output = Self;
    fn mul(self, other: Self) -> Self {
        Self(self.0 * other.0)
    }
}

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
impl PartialEq<i32> for Rational {
    fn eq(&self, other: &i32) -> bool {
        (self.0 - f64::from(*other)).abs() < f64::EPSILON
    }
}

#[cfg(not(feature = "high-precision"))]
impl PartialOrd<i32> for Rational {
    fn partial_cmp(&self, other: &i32) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&f64::from(*other))
    }
}

#[cfg(not(feature = "high-precision"))]
impl PartialEq<f64> for Rational {
    fn eq(&self, other: &f64) -> bool {
        (self.0 - *other).abs() < f64::EPSILON
    }
}

#[cfg(not(feature = "high-precision"))]
impl PartialOrd<f64> for Rational {
    fn partial_cmp(&self, other: &f64) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(other)
    }
}

#[cfg(not(feature = "high-precision"))]
impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

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

/// Centralized static log messages and themed logging.
#[macro_use]
pub mod static_logs;

/// Cache system for analysis results.
pub mod analysis_cache;
/// Batch processing engine for handling multiple files.
pub mod batch;
pub mod builder_base;
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
pub mod process_runner;
pub mod tool_builders;

pub use builder_base::ToolBuilder;
pub use image_builders::{
    AvifencBuilder, DwebpBuilder, ExiftoolBuilder, GifskiBuilder, IdentifyBuilder, MagickBuilder,
    SipsBuilder, WebpmuxBuilder,
};
pub use tool_builders::{
    HostnameBuilder, KillBuilder, PsBuilder, RsyncBuilder, VmafBuilder, X265Builder,
    X265EncodingFlags, X265Flags, X265IoFlags,
};
/// Image quality analytics and scoring.
pub mod image_quality_detector;
/// High-level builder for `cjxl` and `djxl` commands.
pub mod jxl_builder;
pub use jxl_builder::{CjxlBuilder, DjxlBuilder};
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

#[cfg(test)]
mod semantic_tests;

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
/// `HDR` and Color Space unification.
pub mod hdr;
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
/// Shared data types and structures.
pub mod types;

/// User-facing progress mode configuration.
pub mod progress_mode;

/// Global `Ctrl+C` handling and guard system.
pub mod ctrlc_guard;
#[cfg(test)]
mod ctrlc_guard_tests;

/// Unified progress bar interface.
pub mod unified_progress;
pub use unified_progress::Bar as UnifiedProgressBar;

/// Intelligent file sorting and prioritization.
pub mod file_sorter;

/// `MS-SSIM` temporal sampling strategies.
pub mod msssim_sampling;

/// `MS-SSIM` specific progress reporting.
pub mod msssim_progress;

/// Parallelized `MS-SSIM` calculation engine.
pub mod msssim_parallel;

/// Asynchronous and robust error logging system.
/// Shared terminal and file logging initialization.
pub mod logging;

/// General-purpose utility functions.
pub mod common_utils;

/// `AVIF` and `AV1` bitstream health verification.
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
pub use video_detection::{
    CompressionType, DetectedCodec, Detection as VideoDetectionResult, VideoFlags, VideoHdrFlags,
    VideoStreamFlags, detect_video, detect_video_with_cache,
};

/// Shared database interface for quality matching.
pub mod database;
pub mod database_vector;
/// Depth map extraction and embedding.
#[cfg(feature = "jpegxl-ffi")]
pub mod depth_channel;
/// Gainmap to `HDR` synthesis pipeline.
/// Image type and format detection.
pub mod image_analyzer;
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
pub use database::{SampleMatch, lookup_similar_samples};
#[cfg(feature = "jpegxl-ffi")]
pub use depth_channel::{
    DepthMap, DepthType, encode_jxl_depth_fallback, encode_jxl_with_depth, extract_depth_from_heic,
};
pub use image_quality_db::{QualityScore, lookup_image_quality};
pub use loop_intent::{
    LoopMeta, Verdict as LoopIntentVerdict, apply_apple_compat_modern_animation_policy,
    assess as assess_loop_intent, assess_from_meta as assess_loop_intent_from_meta,
    assess_from_probe as assess_loop_intent_from_probe, identify as identify_loop_intent,
    is_lossless_exploration_safe, should_use_gif_fast_path,
};

pub use hdr::{
    GainMapParams, IntermediateFormat, color_info_to_cicp, color_info_to_ffmpeg_args,
    color_info_to_x265_hdr_params, convert_heic_with_gainmap_to_jxl, convert_ultrahdr_jpeg_to_jxl,
    convert_ultrahdr_jpeg_to_jxl_migration, decode_hdr_image_to_png16, dv_x265_profile_string,
    extract_dv_rpu, extract_hevc_bitstream, get_hdr_pix_fmt, is_dovi_tool_available,
    should_use_hdr_decode,
};

pub use batch::*;
pub use codecs::*;
pub use constants::*;
pub use conversion::*;
pub use date_analysis::{
    AnalysisConfig, AnalysisResult, DateSource, FileDateInfo, analyze_directory, print_analysis,
};
pub use ffprobe::{
    FFprobeError, FFprobeResult, detect_bit_depth, get_duration, get_frame_count,
    is_ffprobe_available, parse_frame_rate, probe_video,
};
pub use metadata::{
    apply_saved_timestamps_to_dst, copy, preserve, preserve_directory, preserve_directory_with_log,
    preserve_pro, restore_directory_timestamps, restore_timestamps_from_source_to_output,
    save_directory_timestamps,
};
pub use progress::{
    Batch, CoarseProgressBar, DetailedCoarseProgressBar, Explore, ExploreLogger, FixedBottom,
    GlobalProgressManager, SmartProgressBar, Stats, create_compact_progress_bar,
    create_detailed_progress_bar, create_multi, create_progress_bar, create_progress_bar_with_eta,
    create_spinner, format_bytes, format_duration, wrap_output,
};
pub use quality_matcher::{
    AnalysisDetails, AppleContextFlags, AppleFallbackFlags, AppleFallbackKeepRequest,
    AppleOutcomeFlags, ContentType, EncoderType, MatchMode, MatchedQuality, QualityAnalysis,
    QualityBias, SkipDecision, SourceCodec, VideoAnalysisBuilder, calculate_av1_crf,
    calculate_av1_crf_with_options, calculate_hevc_crf, calculate_hevc_crf_with_options,
    calculate_jxl_distance, calculate_jxl_distance_with_options, from_image_analysis,
    from_video_detection, is_apple_incompatible_video_codec, is_apple_native_format,
    is_size_guard_active, log_quality_analysis, parse_source_codec,
    should_keep_apple_fallback_hevc_output, should_skip_image_format, should_skip_video_codec,
    should_skip_video_codec_apple_compat,
};
pub use report::*;
pub use safety::*;
pub use tools::*;
pub use video::*;

pub use image_quality_detector::{
    ImageContentType, ImageQualityAnalysis, analyze_image_quality, analyze_image_quality_from_path,
    log_media_info_for_image_quality,
};

pub use video_quality_detector::{
    ChromaSubsampling, CompressionLevel, VideoCodecType, VideoContentType, VideoQualityAnalysis,
    analyze_video_quality, analyze_video_quality_from_detection, log_media_info_for_quality,
    to_quality_analysis as video_to_quality_analysis,
};

pub use video_explorer::{
    CompressionVerifyStrategy, ExploreConfig, ExploreMode, ExploreResult, IterationMetrics,
    METADATA_MARGIN_MAX, METADATA_MARGIN_MIN, METADATA_MARGIN_PERCENT, QualityThresholds,
    SMALL_FILE_THRESHOLD, SsimSource, TransparencyReport, VideoEncoder, VideoExplorer,
    calculate_metadata_margin, can_compress_with_metadata, compression_target_size,
    detect_metadata_size, explore_av1, explore_av1_compress_only,
    explore_av1_compress_with_quality, explore_av1_quality_match, explore_av1_size_only,
    explore_compress_only, explore_compress_with_quality, explore_hevc, explore_hevc_compress_only,
    explore_hevc_compress_with_quality, explore_hevc_quality_match, explore_hevc_size_only,
    explore_precise_quality_match, explore_precise_quality_match_with_compression,
    explore_quality_match, explore_size_only, precision, precision::SearchPhase,
    precision::ThreePhaseSearch, pure_video_size, verify_compression_precise,
    verify_compression_simple,
};

pub use types::EncoderPreset;

pub use video_explorer::{
    explore_compress_only_gpu, explore_compress_with_quality_gpu,
    explore_precise_quality_match_gpu, explore_precise_quality_match_with_compression_gpu,
    explore_quality_match_gpu, explore_size_only_gpu,
};

pub use checkpoint::{Manager, safe_delete_original, verify_output_integrity};

pub use quality_verifier_enhanced::{
    EnhancedVerifyResult, VerifyOptions, verify_after_encode, verify_output_file,
};

pub use xmp_merger::{
    Config as XmpMergerConfig, MergeResult, MergeSummary, XmpFile, XmpMerger,
    merge_xmp_for_copied_file,
};

pub use flag_validator::{
    FlagBase, FlagMode, FlagRequest, FlagTier, FlagValidation, print_flag_help, validate_flags,
    validate_flags_result, validate_flags_result_with_ultimate, validate_flags_with_ultimate,
};

pub use gpu_accel::{
    CrfMapping, GpuAccel, GpuCoarseConfig, GpuCoarseResult, GpuEncoder, GpuType,
    estimate_cpu_search_center, get_cpu_search_range_from_gpu, gpu_boundary_to_cpu_range,
    gpu_coarse_search, gpu_coarse_search_with_log,
};

pub use video_explorer::{
    GpuSearchFeatures, GpuSearchFlags, GpuSearchRequest, GpuSearchValidation, explore_av1_with_gpu,
    explore_gpu_coarse, explore_hevc_with_gpu, is_gif_magic,
};

pub use modern_ui::{
    ColorGuard, ExploreProgressState, LogLevel, LogRouter, LogTarget, ProgressStyle, TerminalColor,
    TerminalLogger, UpstreamToolLogger, colors, fmt_compress_status, fmt_crf, fmt_final_result,
    fmt_iterations, fmt_search_result, fmt_size_pct, fmt_ssim, format_size, format_size_change,
    format_size_diff, init_enhanced_logging, init_terminal_logger, print_error, print_header,
    print_info, print_result_box, print_separator, print_stage, print_substage, print_success,
    print_warning, progress_style, render_colored_progress, render_progress_bar, spinner_dots,
    spinner_frame, styles, symbols, terminal_logger,
};

pub use lru_cache::{CacheEntry, LruCache, SerializableCache};

pub use error_handler::{ErrorAction, ErrorCategory, handle_error};

// Re-export unified error types
pub use unified_error::{
    ImgResult, Result as UnifiedResult, UnifiedError, VidQualityError, VidResult,
};

pub use anyhow;

pub use ssim_mapping::{MappingPoint, PsnrSsim};

pub use explore_strategy::{
    CompressOnlyStrategy, CompressWithQualityStrategy, ExploreContext, ExploreStrategy,
    PreciseQualityMatchStrategy, PreciseQualityMatchWithCompressionStrategy, ProgressConfig,
    QualityMatchStrategy, SizeOnlyStrategy, SsimResult, create_strategy,
};

pub use ffmpeg_builder::{
    FfmpegBuilder, FfprobeBuilder, PixFmt, StreamType, VideoCodec, VideoProfile,
};
pub use ffmpeg_process::{
    FfmpegProcess, FfmpegProgressParser, format_ffmpeg_error, is_recoverable_error,
};
pub use float_compare::{
    CRF_EPSILON, F32_EPSILON, F64_EPSILON, PSNR_EPSILON, SSIM_EPSILON as FLOAT_SSIM_EPSILON,
    approx_eq_crf, approx_eq_f32, approx_eq_f64, approx_eq_psnr, approx_eq_ssim, approx_ge_f64,
    approx_le_f64, approx_zero_f32, approx_zero_f64, crf_in_range, ssim_below_threshold,
    ssim_meets_threshold,
};

pub use path_validator::{PathValidationError, validate_path, validate_paths};

pub use crf_constants::{
    AV1_CRF_DEFAULT, AV1_CRF_MAX, AV1_CRF_MIN, AV1_CRF_PRACTICAL_MAX, AV1_CRF_VISUALLY_LOSSLESS,
    CRF_CACHE_KEY_MULTIPLIER, CRF_CACHE_MAX_VALID,
    EMERGENCY_MAX_ITERATIONS as CRF_EMERGENCY_MAX_ITERATIONS, HEVC_CRF_DEFAULT, HEVC_CRF_MAX,
    HEVC_CRF_MIN, HEVC_CRF_PRACTICAL_MAX, HEVC_CRF_VISUALLY_LOSSLESS, NORMAL_MAX_ITERATIONS,
};

pub use stream_size::{
    DEFAULT_OVERHEAD_PERCENT, ExtractionMethod, Info as StreamSizeInfo, MKV_OVERHEAD_PERCENT,
    MOV_OVERHEAD_PERCENT, MP4_OVERHEAD_PERCENT, extract_stream_sizes,
    get_container_overhead_percent, get_output_video,
};

pub use pure_media_verifier::{
    PureMediaVerifyResult, is_video_compressed, verify_pure_media_compression,
    video_compression_ratio,
};

pub use types::{
    Av1Encoder, Crf, CrfError, EncoderBounds, FileSize, HevcEncoder, IterationError,
    IterationGuard, SSIM_EPSILON, Ssim, SsimError, Vp9Encoder, X264Encoder,
};

pub use app_error::AppError;

pub use file_copier::{
    CopyResult, FileStats, IMAGE_EXTENSIONS_ANALYZE, IMAGE_EXTENSIONS_FOR_CONVERT,
    SIDECAR_EXTENSIONS, SUPPORTED_IMAGE_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS, VerifyResult,
    copy_unsupported_files, count_files as count_all_files, verify_output_completeness,
};
pub use smart_file_copier::{
    check_extension_mismatch_readonly, copy_on_skip_or_fail, fix_extension_if_mismatch,
    smart_copy_with_structure,
};

pub use live_photo::is_live as is_live_photo;
pub use process_lock::{acquire_dir_lock, get_mfb_tmp_dir, hash_path_to_hex, init_ghost_mode};

pub use file_sorter::{
    FileInfo, FileSorter, SortStrategy, sort_by_name, sort_by_size_ascending,
    sort_by_size_descending,
};

pub use msssim_sampling::{SamplingConfig, SamplingStrategy};

pub use msssim_progress::Monitor as MsssimProgressMonitor;

pub use msssim_parallel::{MsssimResult, ParallelMsssimCalculator};

pub use logging::{
    LogConfig, flush_logs, init as init_logging, log_external_tool, log_operation_end,
    log_operation_start,
};

pub use common_utils::{
    compute_relative_path, copy_file_with_context, ensure_dir_exists, ensure_parent_dir_exists,
    execute_command_with_logging, extract_digits, extract_suggested_extension,
    format_command_string, get_command_version, get_extension_lowercase, has_extension,
    is_command_available, is_hidden_file, normalize_path_string, parse_float_or_default,
    truncate_string,
};

pub use thread_manager::{
    ThreadConfig, calculate_optimal_threads, disable_multi_instance_mode,
    enable_multi_instance_mode, get_ffmpeg_threads, get_optimal_threads, get_rsync_path,
    get_rsync_version, is_multi_instance, memory_cap_hint,
};

pub use version::{CACHE_SCHEMA_VERSION, Info as VersionInfo, PROGRAM_VERSION, cache_algorithm};
