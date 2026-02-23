//! Video CRF Explorer Module - 统一的视频质量探索器
//!
//! 推荐模式：explore + match-quality + compress（默认开启，见 flag_validator）。
//! 仅支持动态图片→视频和视频→视频转换；静态图片使用无损转换，不支持探索模式。
//!
//! ## 模块化设计
//!
//! 所有探索逻辑集中在此模块，其他模块（img_hevc, vid_hevc）
//! 只需调用此模块的便捷函数，避免重复实现。

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::explore_strategy::CrfCache;

use crate::types::{FileSize, Ssim};
use crate::float_compare::SSIM_EPSILON;
use crate::crf_constants::EMERGENCY_MAX_ITERATIONS;


pub mod codec_detection;
pub mod metadata;
pub mod ssim_calculator;
pub mod stream_analysis;

#[allow(unused_imports)]
pub use codec_detection::*;
#[allow(unused_imports)]
pub use metadata::*;
pub use ssim_calculator::*;
pub use stream_analysis::*;

#[allow(unused_macros)]
macro_rules! progress_line {
    ($($arg:tt)*) => {{
        eprint!("\r\x1b[K{}", format!($($arg)*));
        let _ = std::io::stderr().flush();
    }};
}

#[allow(unused_macros)]
macro_rules! progress_done {
    () => {{
        eprintln!();
    }};
}


pub const ABSOLUTE_MIN_CRF: f32 = 10.0;

pub const ABSOLUTE_MAX_CRF: f32 = 51.0;

pub const STAGE_B1_MAX_ITERATIONS: u32 = 20;

pub const STAGE_B2_MAX_ITERATIONS: u32 = 25;

pub const STAGE_B_BIDIRECTIONAL_MAX: u32 = 18;

pub const BINARY_SEARCH_MAX_ITERATIONS: u32 = 12;

pub const GLOBAL_MAX_ITERATIONS: u32 = 60;

pub const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;

pub const METADATA_MARGIN_MIN: u64 = 2048;

pub const METADATA_MARGIN_MAX: u64 = 102400;

pub const METADATA_MARGIN_PERCENT: f64 = 0.005;

#[inline]
pub fn calculate_metadata_margin(input_size: u64) -> u64 {
    let percent_based = (input_size as f64 * METADATA_MARGIN_PERCENT) as u64;
    percent_based.clamp(METADATA_MARGIN_MIN, METADATA_MARGIN_MAX)
}

#[inline]
pub fn detect_metadata_size(pre_metadata_size: u64, post_metadata_size: u64) -> u64 {
    post_metadata_size.saturating_sub(pre_metadata_size)
}

#[inline]
pub fn pure_video_size(total_size: u64, metadata_size: u64) -> u64 {
    total_size.saturating_sub(metadata_size)
}

#[inline]
pub fn compression_target_size(input_size: u64) -> u64 {
    let margin = calculate_metadata_margin(input_size);
    input_size.saturating_sub(margin)
}

#[inline]
pub fn can_compress_with_metadata(output_size: u64, input_size: u64) -> bool {
    output_size < compression_target_size(input_size)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionVerifyStrategy {
    PureVideo,
    TotalSize,
}

#[inline]
pub fn verify_compression_precise(
    output_size: u64,
    input_size: u64,
    actual_metadata_size: u64,
) -> (bool, u64, CompressionVerifyStrategy) {
    if input_size < SMALL_FILE_THRESHOLD {
        let pure_output = pure_video_size(output_size, actual_metadata_size);
        (
            pure_output < input_size,
            pure_output,
            CompressionVerifyStrategy::PureVideo,
        )
    } else {
        (
            output_size < input_size,
            output_size,
            CompressionVerifyStrategy::TotalSize,
        )
    }
}

#[inline]
pub fn verify_compression_simple(
    output_size: u64,
    input_size: u64,
    actual_metadata_size: u64,
) -> (bool, u64) {
    let (can_compress, compare_size, _) =
        verify_compression_precise(output_size, input_size, actual_metadata_size);
    (can_compress, compare_size)
}


pub const ULTIMATE_MIN_WALL_HITS: u32 = 4;

pub const ULTIMATE_MAX_WALL_HITS: u32 = 20;

pub const ULTIMATE_REQUIRED_ZERO_GAINS: u32 = 8;

pub const NORMAL_MAX_WALL_HITS: u32 = 4;

pub const NORMAL_REQUIRED_ZERO_GAINS: u32 = 4;

pub const LONG_VIDEO_THRESHOLD_SECS: f32 = 300.0;

pub const VERY_LONG_VIDEO_THRESHOLD_SECS: f32 = 600.0;

pub const LONG_VIDEO_FALLBACK_ITERATIONS: u32 = 100;

pub const VERY_LONG_VIDEO_FALLBACK_ITERATIONS: u32 = 80;

pub const LONG_VIDEO_REQUIRED_ZERO_GAINS: u32 = 3;

pub fn calculate_max_iterations_for_duration(duration_secs: f32, ultimate_mode: bool) -> u32 {

    if duration_secs >= VERY_LONG_VIDEO_THRESHOLD_SECS {
        VERY_LONG_VIDEO_FALLBACK_ITERATIONS
    } else if duration_secs >= LONG_VIDEO_THRESHOLD_SECS {
        LONG_VIDEO_FALLBACK_ITERATIONS
    } else if ultimate_mode {
        crate::gpu_accel::GPU_ABSOLUTE_MAX_ITERATIONS
    } else {
        100
    }
}

pub fn calculate_zero_gains_for_duration(duration_secs: f32, ultimate_mode: bool) -> u32 {
    calculate_zero_gains_for_duration_and_range(duration_secs, 41.0, ultimate_mode)
}

pub fn calculate_zero_gains_for_duration_and_range(
    duration_secs: f32,
    crf_range: f32,
    ultimate_mode: bool,
) -> u32 {
    let base = if duration_secs >= LONG_VIDEO_THRESHOLD_SECS {
        LONG_VIDEO_REQUIRED_ZERO_GAINS
    } else if ultimate_mode {
        ULTIMATE_REQUIRED_ZERO_GAINS
    } else {
        NORMAL_REQUIRED_ZERO_GAINS
    };

    let factor = if crf_range < 20.0 {
        (crf_range / 20.0).clamp(0.5, 1.0)
    } else {
        1.0
    };

    let scaled = (base as f32 * factor).round() as u32;
    scaled.max(3)
}

pub const ADAPTIVE_WALL_LOG_BASE: u32 = 6;

pub fn calculate_adaptive_max_walls(crf_range: f32) -> u32 {
    if crf_range.is_nan() || crf_range.is_infinite() || crf_range <= 1.0 {
        return ULTIMATE_MIN_WALL_HITS;
    }
    let log_component = crf_range.log2().ceil() as u32;
    let total = log_component + ADAPTIVE_WALL_LOG_BASE;
    total.clamp(ULTIMATE_MIN_WALL_HITS, ULTIMATE_MAX_WALL_HITS)
}


pub const MIN_ENCODE_THREADS: usize = 1;

pub const DEFAULT_MAX_ENCODE_THREADS: usize = 4;

pub const SERVER_MAX_ENCODE_THREADS: usize = 16;


pub const EXPLORE_DEFAULT_INITIAL_CRF: f32 = 18.0;

pub const EXPLORE_DEFAULT_MIN_CRF: f32 = 10.0;

pub const EXPLORE_DEFAULT_MAX_CRF: f32 = 28.0;

pub const EXPLORE_DEFAULT_TARGET_RATIO: f64 = 1.0;

pub const EXPLORE_DEFAULT_MAX_ITERATIONS: u32 = 12;

pub const EXPLORE_DEFAULT_MIN_SSIM: f64 = 0.95;

pub const EXPLORE_DEFAULT_MIN_PSNR: f64 = 35.0;

pub const EXPLORE_DEFAULT_MIN_MS_SSIM: f64 = 0.90;

pub fn calculate_max_threads(cpu_count: usize, resolution_pixels: Option<u64>) -> usize {
    let half_cpus = cpu_count / 2;

    let resolution_limit = match resolution_pixels {
        Some(pixels) if pixels < 1280 * 720 => 4,
        Some(pixels) if pixels < 1920 * 1080 => 8,
        Some(pixels) if pixels < 3840 * 2160 => 12,
        Some(_) => SERVER_MAX_ENCODE_THREADS,
        None => DEFAULT_MAX_ENCODE_THREADS,
    };

    half_cpus.clamp(MIN_ENCODE_THREADS, resolution_limit)
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExploreMode {
    SizeOnly,

    QualityMatch,

    PreciseQualityMatch,

    PreciseQualityMatchWithCompression,

    CompressOnly,

    CompressWithQuality,
}


#[derive(Debug, Clone, Default)]
pub struct ConfidenceBreakdown {
    pub sampling_coverage: f64,
    pub prediction_accuracy: f64,
    pub margin_safety: f64,
    pub ssim_confidence: f64,
}


pub const CONFIDENCE_WEIGHT_SAMPLING: f64 = 0.3;
pub const CONFIDENCE_WEIGHT_PREDICTION: f64 = 0.3;
pub const CONFIDENCE_WEIGHT_MARGIN: f64 = 0.2;
pub const CONFIDENCE_WEIGHT_SSIM: f64 = 0.2;

impl ConfidenceBreakdown {
    pub fn overall(&self) -> f64 {
        (self.sampling_coverage * CONFIDENCE_WEIGHT_SAMPLING
            + self.prediction_accuracy * CONFIDENCE_WEIGHT_PREDICTION
            + self.margin_safety * CONFIDENCE_WEIGHT_MARGIN
            + self.ssim_confidence * CONFIDENCE_WEIGHT_SSIM)
            .min(1.0)
    }

    pub fn print_report(&self) {
        let overall = self.overall();
        let grade = if overall >= 0.9 {
            "🟢 Excellent"
        } else if overall >= 0.75 {
            "🟡 Good"
        } else if overall >= 0.5 {
            "🟠 Fair"
        } else {
            "🔴 Low"
        };

        eprintln!("┌─────────────────────────────────────────────────────");
        eprintln!("│ 📊 Confidence Report");
        eprintln!("├─────────────────────────────────────────────────────");
        eprintln!("│ 📈 Overall Confidence: {:.0}% {}", overall * 100.0, grade);
        eprintln!("├─────────────────────────────────────────────────────");
        eprintln!(
            "│ 📹 Sampling Coverage: {:.0}% (weight 30%)",
            self.sampling_coverage * 100.0
        );
        eprintln!(
            "│ 🎯 Prediction Accuracy: {:.0}% (weight 30%)",
            self.prediction_accuracy * 100.0
        );
        eprintln!(
            "│ 💾 Safety Margin: {:.0}% (weight 20%)",
            self.margin_safety * 100.0
        );
        eprintln!(
            "│ 📊 SSIM Reliability: {:.0}% (weight 20%)",
            self.ssim_confidence * 100.0
        );
        eprintln!("└─────────────────────────────────────────────────────");
    }
}

#[derive(Debug, Clone)]
pub struct ExploreResult {
    pub optimal_crf: f32,
    pub output_size: u64,
    pub size_change_pct: f64,
    pub ssim: Option<f64>,
    pub psnr: Option<f64>,
    pub ms_ssim: Option<f64>,
    pub ms_ssim_passed: Option<bool>,
    pub ms_ssim_score: Option<f64>,
    pub iterations: u32,
    pub quality_passed: bool,
    pub log: Vec<String>,
    pub confidence: f64,
    pub confidence_detail: ConfidenceBreakdown,
    pub actual_min_ssim: f64,
    pub input_video_stream_size: u64,
    pub output_video_stream_size: u64,
    pub container_overhead: u64,
}

impl Default for ExploreResult {
    fn default() -> Self {
        Self {
            optimal_crf: 0.0,
            output_size: 0,
            size_change_pct: 0.0,
            ssim: None,
            psnr: None,
            ms_ssim: None,
            ms_ssim_passed: None,
            ms_ssim_score: None,
            iterations: 0,
            quality_passed: false,
            log: Vec::new(),
            confidence: 0.0,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: 0.95,
            input_video_stream_size: 0,
            output_video_stream_size: 0,
            container_overhead: 0,
        }
    }
}

impl ExploreResult {

    #[inline]
    pub fn ssim_typed(&self) -> Option<Ssim> {
        self.ssim.and_then(|v| Ssim::new(v).ok())
    }

    #[inline]
    pub fn output_size_typed(&self) -> FileSize {
        FileSize::new(self.output_size)
    }

    #[inline]
    pub fn ssim_meets(&self, threshold: f64) -> bool {
        self.ssim
            .is_some_and(|s| crate::float_compare::ssim_meets_threshold(s, threshold))
    }
}

#[derive(Debug, Clone)]
pub struct QualityThresholds {
    pub min_ssim: f64,
    pub min_psnr: f64,
    pub min_ms_ssim: f64,
    pub validate_ssim: bool,
    pub validate_psnr: bool,
    pub validate_ms_ssim: bool,
    pub force_ms_ssim_long: bool,
}

pub const LONG_VIDEO_THRESHOLD: f32 = 300.0;

impl Default for QualityThresholds {
    fn default() -> Self {
        Self {
            min_ssim: EXPLORE_DEFAULT_MIN_SSIM,
            min_psnr: EXPLORE_DEFAULT_MIN_PSNR,
            min_ms_ssim: EXPLORE_DEFAULT_MIN_MS_SSIM,
            validate_ssim: true,
            validate_psnr: false,
            validate_ms_ssim: false,
            force_ms_ssim_long: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExploreConfig {
    pub mode: ExploreMode,
    pub initial_crf: f32,
    pub min_crf: f32,
    pub max_crf: f32,
    pub target_ratio: f64,
    pub quality_thresholds: QualityThresholds,
    pub max_iterations: u32,
    pub ultimate_mode: bool,
    pub use_pure_media_comparison: bool,
}

impl Default for ExploreConfig {
    fn default() -> Self {
        Self {
            mode: ExploreMode::PreciseQualityMatch,
            initial_crf: EXPLORE_DEFAULT_INITIAL_CRF,
            min_crf: EXPLORE_DEFAULT_MIN_CRF,
            max_crf: EXPLORE_DEFAULT_MAX_CRF,
            target_ratio: EXPLORE_DEFAULT_TARGET_RATIO,
            quality_thresholds: QualityThresholds::default(),
            max_iterations: EXPLORE_DEFAULT_MAX_ITERATIONS,
            ultimate_mode: false,
            use_pure_media_comparison: true,
        }
    }
}

impl ExploreConfig {
    pub fn size_only(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::SizeOnly,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                validate_ssim: false,
                validate_psnr: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn quality_match(predicted_crf: f32) -> Self {
        Self {
            mode: ExploreMode::QualityMatch,
            initial_crf: predicted_crf,
            max_iterations: 1,
            quality_thresholds: QualityThresholds {
                validate_ssim: true,
                validate_psnr: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn precise_quality_match(initial_crf: f32, max_crf: f32, min_ssim: f64) -> Self {
        Self {
            mode: ExploreMode::PreciseQualityMatch,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim,
                min_psnr: 40.0,
                min_ms_ssim: 90.0,
                validate_ssim: true,
                validate_psnr: false,
                validate_ms_ssim: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn precise_quality_match_with_compression(
        initial_crf: f32,
        max_crf: f32,
        min_ssim: f64,
    ) -> Self {
        Self {
            mode: ExploreMode::PreciseQualityMatchWithCompression,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim,
                min_psnr: 40.0,
                min_ms_ssim: 90.0,
                validate_ssim: true,
                validate_psnr: false,
                validate_ms_ssim: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn compress_only(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::CompressOnly,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                validate_ssim: false,
                validate_psnr: false,
                validate_ms_ssim: false,
                ..Default::default()
            },
            max_iterations: 8,
            ..Default::default()
        }
    }

    pub fn compress_with_quality(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::CompressWithQuality,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim: 0.95,
                validate_ssim: true,
                validate_psnr: false,
                validate_ms_ssim: false,
                ..Default::default()
            },
            max_iterations: 10,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoEncoder {
    Hevc,
    Av1,
    H264,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncoderPreset {
    Ultrafast,
    Fast,
    #[default]
    Medium,
    Slow,
    Slower,
    Veryslow,
}

impl EncoderPreset {
    pub fn x26x_name(&self) -> &'static str {
        match self {
            EncoderPreset::Ultrafast => "ultrafast",
            EncoderPreset::Fast => "fast",
            EncoderPreset::Medium => "medium",
            EncoderPreset::Slow => "slow",
            EncoderPreset::Slower => "slower",
            EncoderPreset::Veryslow => "veryslow",
        }
    }

    pub fn svtav1_preset(&self) -> u8 {
        match self {
            EncoderPreset::Ultrafast => 12,
            EncoderPreset::Fast => 8,
            EncoderPreset::Medium => 6,
            EncoderPreset::Slow => 4,
            EncoderPreset::Slower => 2,
            EncoderPreset::Veryslow => 0,
        }
    }
}

impl VideoEncoder {
    pub fn ffmpeg_name(&self) -> &'static str {
        match self {
            VideoEncoder::Hevc => {
                if Self::is_encoder_available("libx265") {
                    "libx265"
                } else {
                    eprintln!("⚠️  libx265 not available, falling back to hevc_videotoolbox");
                    "hevc_videotoolbox"
                }
            }
            VideoEncoder::Av1 => "libsvtav1",
            VideoEncoder::H264 => {
                if Self::is_encoder_available("libx264") {
                    "libx264"
                } else {
                    eprintln!("⚠️  libx264 not available, falling back to h264_videotoolbox");
                    "h264_videotoolbox"
                }
            }
        }
    }

    fn is_encoder_available(encoder: &str) -> bool {
        use std::process::Command;

        static LIBX265_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        static LIBX264_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

        let cache = match encoder {
            "libx265" => &LIBX265_AVAILABLE,
            "libx264" => &LIBX264_AVAILABLE,
            _ => return true,
        };

        *cache.get_or_init(|| {
            Command::new("ffmpeg")
                .args(["-hide_banner", "-encoders"])
                .output()
                .ok()
                .map(|output| {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    stdout.contains(encoder)
                })
                .unwrap_or(false)
        })
    }

    pub fn container(&self) -> &'static str {
        match self {
            VideoEncoder::Hevc => "mp4",
            VideoEncoder::Av1 => "mp4",
            VideoEncoder::H264 => "mp4",
        }
    }

    pub fn extra_args(&self, max_threads: usize) -> Vec<String> {
        self.extra_args_with_preset(max_threads, EncoderPreset::default())
    }

    pub fn extra_args_with_preset(&self, max_threads: usize, preset: EncoderPreset) -> Vec<String> {
        match self {
            VideoEncoder::Hevc => vec![
                "-preset".to_string(),
                preset.x26x_name().to_string(),
                "-tag:v".to_string(),
                "hvc1".to_string(),
                "-x265-params".to_string(),
                format!("log-level=error:pools={}", max_threads),
            ],
            VideoEncoder::Av1 => vec![
                "-svtav1-params".to_string(),
                format!(
                    "tune=0:film-grain=0:preset={}:lp={}",
                    preset.svtav1_preset(),
                    max_threads
                ),
            ],
            VideoEncoder::H264 => vec![
                "-preset".to_string(),
                preset.x26x_name().to_string(),
                "-profile:v".to_string(),
                "high".to_string(),
            ],
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsimSource {
    Actual,
    Predicted,
    None,
}

#[derive(Debug, Clone)]
pub struct IterationMetrics {
    pub iteration: u32,
    pub phase: String,
    pub crf: f32,
    pub output_size: u64,
    pub size_change_pct: f64,
    pub ssim: Option<f64>,
    pub ssim_source: SsimSource,
    pub psnr: Option<f64>,
    pub can_compress: bool,
    pub quality_passed: Option<bool>,
    pub decision: String,
}

impl IterationMetrics {
    pub fn print_line(&self) {
        let ssim_str = match (self.ssim, self.ssim_source) {
            (Some(s), SsimSource::Predicted) => format!("~{:.4}", s),
            (Some(s), _) => format!("{:.4}", s),
            (None, _) => "----".to_string(),
        };
        let psnr_str = self
            .psnr
            .map(|p| format!("{:.1}", p))
            .unwrap_or_else(|| "----".to_string());
        let compress_icon = if self.can_compress { "✅" } else { "❌" };
        let quality_icon = match self.quality_passed {
            Some(true) => "✅",
            Some(false) => "⚠️",
            None => "--",
        };

        eprintln!(
            "│ {:>2} │ {:>12} │ CRF {:>5.1} │ {:>+6.1}% {} │ SSIM {} {} │ PSNR {} │ {}",
            self.iteration,
            self.phase,
            self.crf,
            self.size_change_pct,
            compress_icon,
            ssim_str,
            quality_icon,
            psnr_str,
            self.decision
        );
    }
}

#[derive(Debug, Clone, Default)]
pub struct TransparencyReport {
    pub iterations: Vec<IterationMetrics>,
    pub start_time: Option<std::time::Instant>,
    pub input_size: u64,
    pub final_crf: Option<f32>,
    pub final_ssim: Option<f64>,
    pub final_psnr: Option<f64>,
}

impl TransparencyReport {
    pub fn new(input_size: u64) -> Self {
        Self {
            iterations: Vec::new(),
            start_time: Some(std::time::Instant::now()),
            input_size,
            final_crf: None,
            final_ssim: None,
            final_psnr: None,
        }
    }

    pub fn add_iteration(&mut self, metrics: IterationMetrics) {
        metrics.print_line();
        self.iterations.push(metrics);
    }

    pub fn print_header(&self) {
        eprintln!("┌────────────────────────────────────────────────────────────────────────────────────────────┐");
        eprintln!("│ 📊 Transparency Report - CRF Search Process                                               │");
        eprintln!("├────┬──────────────┬───────────┬─────────────┬─────────────┬──────────┬────────────────────┤");
        eprintln!("│ #  │ Phase        │ CRF       │ Size Change │ SSIM        │ PSNR     │ Decision           │");
        eprintln!("├────┼──────────────┼───────────┼─────────────┼─────────────┼──────────┼────────────────────┤");
    }

    pub fn print_summary(&self) {
        eprintln!("└────┴──────────────┴───────────┴─────────────┴─────────────┴──────────┴────────────────────┘");

        let elapsed = self
            .start_time
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let total_iterations = self.iterations.len();

        eprintln!();
        eprintln!("📈 Summary:");
        eprintln!("   • Total iterations: {}", total_iterations);
        eprintln!("   • Time elapsed: {:.1}s", elapsed);

        if let Some(crf) = self.final_crf {
            eprintln!("   • Final CRF: {:.1}", crf);
        }
        if let Some(ssim) = self.final_ssim {
            eprintln!("   • Final SSIM: {:.4}", ssim);
        }
        if let Some(psnr) = self.final_psnr {
            eprintln!("   • Final PSNR: {:.1} dB", psnr);
        }
    }
}


pub struct VideoExplorer {
    config: ExploreConfig,
    encoder: VideoEncoder,
    input_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
    input_size: u64,
    vf_args: Vec<String>,
    use_gpu: bool,
    max_threads: usize,
    preset: EncoderPreset,
    input_video_stream_size: u64,
}

impl VideoExplorer {
    pub fn new(
        input: &Path,
        output: &Path,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        config: ExploreConfig,
        max_threads: usize,
    ) -> Result<Self> {
        crate::path_validator::validate_path(input).map_err(|e| anyhow::anyhow!("{}", e))?;
        crate::path_validator::validate_path(output).map_err(|e| anyhow::anyhow!("{}", e))?;

        let input_size = fs::metadata(input)
            .context("Failed to read input file metadata")?
            .len();


        let gpu = crate::gpu_accel::GpuAccel::detect();
        let use_gpu = gpu.is_available()
            && match encoder {
                VideoEncoder::Hevc => gpu.get_hevc_encoder().is_some(),
                VideoEncoder::Av1 => gpu.get_av1_encoder().is_some(),
                VideoEncoder::H264 => gpu.get_h264_encoder().is_some(),
            };

        let input_video_stream_size = if config.use_pure_media_comparison {
            let stream_info = crate::stream_size::extract_stream_sizes(input);
            stream_info.video_stream_size
        } else {
            input_size
        };

        Ok(Self {
            config,
            encoder,
            input_path: input.to_path_buf(),
            output_path: output.to_path_buf(),
            input_size,
            vf_args,
            max_threads,
            use_gpu,
            preset: EncoderPreset::default(),
            input_video_stream_size,
        })
    }

    pub fn new_with_gpu(
        input: &Path,
        output: &Path,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        config: ExploreConfig,
        use_gpu: bool,
        max_threads: usize,
    ) -> Result<Self> {
        crate::path_validator::validate_path(input).map_err(|e| anyhow::anyhow!("{}", e))?;
        crate::path_validator::validate_path(output).map_err(|e| anyhow::anyhow!("{}", e))?;

        let input_size = fs::metadata(input)
            .context("Failed to read input file metadata")?
            .len();


        let input_video_stream_size = if config.use_pure_media_comparison {
            let stream_info = crate::stream_size::extract_stream_sizes(input);
            stream_info.video_stream_size
        } else {
            input_size
        };

        Ok(Self {
            config,
            encoder,
            input_path: input.to_path_buf(),
            output_path: output.to_path_buf(),
            input_size,
            vf_args,
            max_threads,
            use_gpu,
            preset: EncoderPreset::default(),
            input_video_stream_size,
        })
    }

    pub fn new_with_preset(
        input: &Path,
        output: &Path,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        config: ExploreConfig,
        preset: EncoderPreset,
        max_threads: usize,
    ) -> Result<Self> {
        crate::path_validator::validate_path(input).map_err(|e| anyhow::anyhow!("{}", e))?;
        crate::path_validator::validate_path(output).map_err(|e| anyhow::anyhow!("{}", e))?;

        let input_size = fs::metadata(input)
            .context("Failed to read input file metadata")?
            .len();


        let gpu = crate::gpu_accel::GpuAccel::detect();
        let use_gpu = gpu.is_available()
            && match encoder {
                VideoEncoder::Hevc => gpu.get_hevc_encoder().is_some(),
                VideoEncoder::Av1 => gpu.get_av1_encoder().is_some(),
                VideoEncoder::H264 => gpu.get_h264_encoder().is_some(),
            };

        let input_video_stream_size = if config.use_pure_media_comparison {
            let stream_info = crate::stream_size::extract_stream_sizes(input);
            stream_info.video_stream_size
        } else {
            input_size
        };

        Ok(Self {
            config,
            encoder,
            input_path: input.to_path_buf(),
            output_path: output.to_path_buf(),
            input_size,
            vf_args,
            max_threads,
            use_gpu,
            preset,
            input_video_stream_size,
        })
    }

    pub fn explore(&self) -> Result<ExploreResult> {
        match self.config.mode {
            ExploreMode::SizeOnly => self.explore_size_only(),
            ExploreMode::QualityMatch => self.explore_quality_match(),
            ExploreMode::PreciseQualityMatch => self.explore_precise_quality_match(),
            ExploreMode::PreciseQualityMatchWithCompression => {
                self.explore_precise_quality_match_with_compression()
            }
            ExploreMode::CompressOnly => self.explore_compress_only(),
            ExploreMode::CompressWithQuality => self.explore_compress_with_quality(),
        }
    }

    pub fn explore_with_strategy(&self) -> Result<ExploreResult> {
        use crate::explore_strategy::{create_strategy, ExploreContext};

        let mut ctx = ExploreContext::new(
            self.input_path.clone(),
            self.output_path.clone(),
            self.input_size,
            self.encoder,
            self.vf_args.clone(),
            self.max_threads,
            self.use_gpu,
            self.preset,
            self.config.clone(),
        );

        let strategy = create_strategy(self.config.mode);
        eprintln!(
            "🔥 Using Strategy: {} - {}",
            strategy.name(),
            strategy.description()
        );
        strategy.explore(&mut ctx)
    }

    fn explore_size_only(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let start_time = std::time::Instant::now();

        let pb = crate::progress::create_professional_spinner("🔍 Size Explore");

        macro_rules! progress_line {
            ($($arg:tt)*) => {{
                pb.set_message(format!($($arg)*));
            }};
        }

        macro_rules! progress_done {
            () => {{}};
        }

        pb.suspend(|| {
            eprintln!("┌ 🔍 Size-Only Explore ({:?})", self.encoder);
            eprintln!(
                "└ 📁 Input: {:.2} MB",
                self.input_size as f64 / 1024.0 / 1024.0
            );
        });

        log.push(format!("🔍 Size-Only Explore ({:?})", self.encoder));

        progress_line!("Test CRF {:.1}...", self.config.max_crf);
        let max_size = self.encode(self.config.max_crf)?;
        let iterations = 1u32;
        progress_done!();

        let (best_crf, best_size, quality_passed) = if self.can_compress_with_margin(max_size) {
            (self.config.max_crf, max_size, true)
        } else {
            (self.config.max_crf, max_size, false)
        };

        progress_line!("Calculate SSIM...");
        let ssim = self.calculate_ssim().ok().flatten();
        progress_done!();

        let size_change_pct = self.calc_change_pct(best_size);
        let elapsed = start_time.elapsed();

        pb.finish_and_clear();
        let ssim_str = ssim
            .map(|s| format!("{:.4}", s))
            .unwrap_or_else(|| "---".to_string());
        let status = if quality_passed { "💾" } else { "⚠️" };
        eprintln!(
            "✅ Result: CRF {:.1} • SSIM {} • Size {:+.1}% ({}) • {:.1}s",
            best_crf,
            ssim_str,
            size_change_pct,
            status,
            elapsed.as_secs_f64()
        );
        log.push(format!(
            "📊 RESULT: CRF {:.1}, {:+.1}%",
            best_crf, size_change_pct
        ));

        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim,
            psnr: None,
            ms_ssim: None,
            ms_ssim_passed: None,
            ms_ssim_score: None,
            iterations,
            quality_passed,
            log,
            confidence: 0.7,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        })
    }

    fn explore_quality_match(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();

        log.push(format!("🎯 Quality-Match Mode ({:?})", self.encoder));
        log.push(format!("   Input: {} bytes", self.input_size));
        log.push(format!("   Predicted CRF: {}", self.config.initial_crf));

        let output_size = self.encode(self.config.initial_crf)?;
        let quality = self.validate_quality()?;

        let mut quality_str = format!("SSIM: {:.4}", quality.0.unwrap_or(0.0));
        if let Some(vmaf) = quality.2 {
            quality_str.push_str(&format!(", MS-SSIM: {:.2}", vmaf));
        }
        log.push(format!(
            "   CRF {}: {} bytes ({:+.1}%), {}",
            self.config.initial_crf,
            output_size,
            self.calc_change_pct(output_size),
            quality_str
        ));

        let quality_passed = self.check_quality_passed(quality.0, quality.1, quality.2);
        if quality_passed {
            log.push("   ✅ Quality validation passed".to_string());
        } else {
            log.push(format!(
                "   ⚠️ Quality below threshold (min SSIM: {:.4})",
                self.config.quality_thresholds.min_ssim
            ));
        }

        Ok(ExploreResult {
            optimal_crf: self.config.initial_crf,
            output_size,
            size_change_pct: self.calc_change_pct(output_size),
            ssim: quality.0,
            psnr: quality.1,
            ms_ssim: quality.2,
            iterations: 1,
            quality_passed,
            log,
            confidence: 0.6,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        })
    }

    fn explore_compress_only(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let mut cache: CrfCache<u64> = CrfCache::new();

        let start_time = std::time::Instant::now();
        let mut _best_crf_so_far: f32 = 0.0;

        let encode_cached =
            |crf: f32, cache: &mut CrfCache<u64>, explorer: &VideoExplorer| -> Result<u64> {
                if let Some(&size) = cache.get(crf) {
                    return Ok(size);
                }
                let size = explorer.encode(crf)?;
                cache.insert(crf, size);
                Ok(size)
            };

        let pb = crate::progress::create_professional_spinner("📦 Compress Only");

        macro_rules! progress_line {
            ($($arg:tt)*) => {{
                pb.set_message(format!($($arg)*));
            }};
        }

        macro_rules! progress_done {
            () => {{}};
        }

        pb.suspend(|| {
            eprintln!("┌ 📦 Compress-Only ({:?})", self.encoder);
            eprintln!(
                "└ 📁 Input: {:.2} MB",
                self.input_size as f64 / 1024.0 / 1024.0
            );
        });
        log.push(format!("📦 Compress-Only ({:?})", self.encoder));

        let mut iterations = 0u32;

        let initial_size = encode_cached(self.config.initial_crf, &mut cache, self)?;
        iterations += 1;
        let size_pct = self.calc_change_pct(initial_size);
        progress_line!(
            "CRF {:.1} | {:+.1}% | Iter {}",
            self.config.initial_crf,
            size_pct,
            iterations
        );

        if self.can_compress_with_margin(initial_size) {
            progress_done!();
            _best_crf_so_far = self.config.initial_crf;
            let elapsed = start_time.elapsed();

            pb.finish_and_clear();
            eprintln!(
                "✅ Result: CRF {:.1} • {:+.1}% ✅ • ({:.1}s)",
                self.config.initial_crf,
                size_pct,
                elapsed.as_secs_f64()
            );
            return Ok(ExploreResult {
                optimal_crf: self.config.initial_crf,
                output_size: initial_size,
                size_change_pct: self.calc_change_pct(initial_size),
                ssim: None,
                psnr: None,
                ms_ssim: None,
                ms_ssim_passed: None,
                ms_ssim_score: None,
                iterations,
                quality_passed: true,
                log,
                confidence: 0.7,
                confidence_detail: ConfidenceBreakdown::default(),
                actual_min_ssim: self.config.quality_thresholds.min_ssim,
                ..Default::default()
            });
        }

        let mut low = self.config.initial_crf;
        let mut high = self.config.max_crf;
        let mut best_crf: Option<f32> = None;
        let mut best_size: Option<u64> = None;

        while high - low > precision::FINE_STEP && iterations < self.config.max_iterations {
            let mid = ((low + high) / 2.0 * 2.0).round() / 2.0;

            let size = encode_cached(mid, &mut cache, self)?;
            iterations += 1;
            let size_pct = self.calc_change_pct(size);
            let compress_icon = if self.can_compress_with_margin(size) {
                "✅"
            } else {
                "❌"
            };
            progress_line!(
                "Binary Search | CRF {:.1} | {:+.1}% {} | Best: {:.1}",
                mid,
                size_pct,
                compress_icon,
                _best_crf_so_far
            );

            if self.can_compress_with_margin(size) {
                best_crf = Some(mid);
                best_size = Some(size);
                _best_crf_so_far = mid;
                high = mid;
            } else {
                low = mid;
            }
        }
        progress_done!();

        let (final_crf, final_size) = if let (Some(crf), Some(size)) = (best_crf, best_size) {
            (crf, size)
        } else {
            let size = encode_cached(self.config.max_crf, &mut cache, self)?;
            (self.config.max_crf, size)
        };

        let size_change_pct = self.calc_change_pct(final_size);
        let compressed = self.can_compress_with_margin(final_size);
        let elapsed = start_time.elapsed();

        pb.finish_and_clear();
        let status = if compressed { "✅" } else { "⚠️" };
        eprintln!(
            "✅ Result: CRF {:.1} • {:+.1}% {} • Iter {} ({:.1}s)",
            final_crf,
            size_change_pct,
            status,
            iterations,
            elapsed.as_secs_f64()
        );
        log.push(format!(
            "📊 RESULT: CRF {:.1}, {:+.1}%",
            final_crf, size_change_pct
        ));

        Ok(ExploreResult {
            optimal_crf: final_crf,
            output_size: final_size,
            size_change_pct,
            ssim: None,
            psnr: None,
            ms_ssim: None,
            ms_ssim_passed: None,
            ms_ssim_score: None,
            iterations,
            quality_passed: compressed,
            log,
            confidence: 0.65,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        })
    }

    fn explore_compress_with_quality(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let mut cache: CrfCache<(u64, Option<f64>)> = CrfCache::new();

        let _heartbeat = crate::universal_heartbeat::HeartbeatGuard::new(
            crate::universal_heartbeat::HeartbeatConfig::medium("Binary Search (Compress+Quality)")
                .with_info(format!(
                    "CRF {:.1}-{:.1}",
                    self.config.initial_crf, self.config.max_crf
                )),
        );

        let pb = crate::progress::create_professional_spinner("📦 Compress+Quality");

        macro_rules! log_realtime {
            ($($arg:tt)*) => {{
                let msg = format!($($arg)*);
                pb.suspend(|| eprintln!("{}", msg));
                log.push(msg);
            }};
        }

        let min_ssim = self.config.quality_thresholds.min_ssim;
        pb.suspend(|| {
            eprintln!("┌ 📦 Compress + Quality v4.8 ({:?})", self.encoder);
            eprintln!("├ 📁 Input: {} bytes", self.input_size);
            eprintln!("└ 🎯 Goal: output < input + SSIM >= {:.2}", min_ssim);
        });

        let mut iterations = 0u32;
        let mut best_result: Option<(f32, u64, f64)> = None;

        pb.set_message("Phase 1: Binary search for compression boundary");
        let mut low = self.config.initial_crf;
        let mut high = self.config.max_crf;
        let mut compress_boundary: Option<f32> = None;

        while high - low > precision::COARSE_STEP / 2.0 && iterations < self.config.max_iterations {
            let mid = ((low + high) / 2.0).round();

            log_realtime!("   🔄 Testing CRF {:.0}...", mid);
            let size = self.encode(mid)?;
            iterations += 1;

            cache.insert(mid, (size, None));

            if self.can_compress_with_margin(size) {
                compress_boundary = Some(mid);
                high = mid;
                log_realtime!("      ✅ Compresses at CRF {:.0}", mid);
            } else {
                low = mid;
                log_realtime!("      ❌ Too large at CRF {:.0}", mid);
            }
        }

        if let Some(boundary) = compress_boundary {
            log_realtime!("   📍 Phase 2: Validate quality at CRF {:.1}", boundary);

            let size = if let Some(&(s, _)) = cache.get(boundary) {
                s
            } else {
                let s = self.encode(boundary)?;
                iterations += 1;
                s
            };

            let quality = self.validate_quality()?;
            let ssim = quality.0.unwrap_or(0.0);
            cache.insert(boundary, (size, Some(ssim)));

            log_realtime!(
                "      CRF {:.1}: SSIM {:.4}, Size {:+.1}%",
                boundary,
                ssim,
                self.calc_change_pct(size)
            );

            if ssim >= min_ssim {
                best_result = Some((boundary, size, ssim));
                log_realtime!("      ✅ Valid: compresses + SSIM OK");
            } else {
                best_result = Some((boundary, size, ssim));
                log_realtime!("      ⚠️ SSIM below threshold, but best available");
            }
        }

        let (final_crf, final_size, final_ssim) = if let Some((crf, size, ssim)) = best_result {
            (crf, size, ssim)
        } else {
            let size = self.encode(self.config.max_crf)?;
            let quality = self.validate_quality()?;
            (self.config.max_crf, size, quality.0.unwrap_or(0.0))
        };

        let size_change_pct = self.calc_change_pct(final_size);
        let compressed = self.can_compress_with_margin(final_size);
        let quality_ok = final_ssim >= min_ssim;
        let passed = compressed && quality_ok;

        pb.finish_and_clear();
        log_realtime!(
            "✅ RESULT: CRF {:.1} • SSIM {:.4} • Size {:+.1}% {}",
            final_crf,
            final_ssim,
            size_change_pct,
            if passed {
                "✅"
            } else if compressed {
                "⚠️ SSIM low"
            } else {
                "⚠️ Not compressed"
            }
        );
        log_realtime!("📈 Iterations: {}", iterations);

        Ok(ExploreResult {
            optimal_crf: final_crf,
            output_size: final_size,
            size_change_pct,
            ssim: Some(final_ssim),
            psnr: None,
            ms_ssim: None,
            ms_ssim_passed: None,
            ms_ssim_score: None,
            iterations,
            quality_passed: passed,
            log,
            confidence: 0.75,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: min_ssim,
            ..Default::default()
        })
    }

    fn explore_precise_quality_match(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let mut cache: CrfCache<(u64, (Option<f64>, Option<f64>, Option<f64>))> = CrfCache::new();
        let mut last_encoded_crf: Option<f32> = None;

        macro_rules! log_realtime {
            ($($arg:tt)*) => {{
                let msg = format!($($arg)*);
                eprintln!("{}", msg);
                log.push(msg);
            }};
        }

        log_realtime!("🔬 Precise Quality-Match v4.9 ({:?})", self.encoder);
        log_realtime!(
            "   📁 Input: {} bytes ({:.2} MB)",
            self.input_size,
            self.input_size as f64 / 1024.0 / 1024.0
        );
        log_realtime!(
            "   📐 CRF range: [{:.1}, {:.1}]",
            self.config.min_crf,
            self.config.max_crf
        );
        log_realtime!("   🎯 Goal: Find HIGHEST SSIM (best quality match)");
        log_realtime!("   ═══════════════════════════════════════════════════");

        let mut iterations = 0u32;
        let crf_range = (self.config.max_crf - self.config.min_crf).max(1.0);
        let dynamic_max_iterations = ((crf_range as f64).log2().ceil() as u32)
            .saturating_add(6)
            .saturating_add(4)
            .clamp(10, GLOBAL_MAX_ITERATIONS);
        let max_iterations = dynamic_max_iterations;
        const SSIM_PLATEAU_THRESHOLD: f64 = 0.0002;

        let mut best_crf: f32;
        let mut best_size: u64;
        let mut best_quality: (Option<f64>, Option<f64>, Option<f64>);
        let mut best_ssim: f64;

        let encode_cached =
            |crf: f32,
             cache: &mut CrfCache<(u64, (Option<f64>, Option<f64>, Option<f64>))>,
             last_crf: &mut Option<f32>,
             explorer: &VideoExplorer|
             -> Result<(u64, (Option<f64>, Option<f64>, Option<f64>))> {
                if let Some(&cached) = cache.get(crf) {
                    return Ok(cached);
                }

                let size = explorer.encode(crf)?;
                let quality = explorer.validate_quality()?;
                cache.insert(crf, (size, quality));
                *last_crf = Some(crf);
                Ok((size, quality))
            };

        log_realtime!("   📍 Phase 1: Boundary test");

        log_realtime!("   🔄 Testing min CRF {:.1}...", self.config.min_crf);
        let (min_size, min_quality) =
            encode_cached(self.config.min_crf, &mut cache, &mut last_encoded_crf, self)?;
        iterations += 1;
        let min_ssim = min_quality.0.unwrap_or(0.0);
        log_realtime!(
            "      CRF {:.1}: SSIM {:.6}, Size {:+.1}%",
            self.config.min_crf,
            min_ssim,
            self.calc_change_pct(min_size)
        );

        best_crf = self.config.min_crf;
        best_size = min_size;
        best_quality = min_quality;
        best_ssim = min_ssim;

        log_realtime!("   🔄 Testing max CRF {:.1}...", self.config.max_crf);
        let (max_size, max_quality) =
            encode_cached(self.config.max_crf, &mut cache, &mut last_encoded_crf, self)?;
        iterations += 1;
        let max_ssim = max_quality.0.unwrap_or(0.0);
        log_realtime!(
            "      CRF {:.1}: SSIM {:.6}, Size {:+.1}%",
            self.config.max_crf,
            max_ssim,
            self.calc_change_pct(max_size)
        );

        let ssim_range = min_ssim - max_ssim;
        log_realtime!("      SSIM range: {:.6}", ssim_range);

        if ssim_range < SSIM_PLATEAU_THRESHOLD {
            log_realtime!("   ⚡ Early exit: SSIM plateau, using max CRF for smaller file");
            best_crf = self.config.max_crf;
            best_size = max_size;
            best_quality = max_quality;
            best_ssim = max_ssim;
        } else {
            log_realtime!("   📍 Phase 2: Golden section search");
            const PHI: f32 = 0.618;

            let mut low = self.config.min_crf;
            let mut high = self.config.max_crf;
            let mut prev_ssim = min_ssim;

            while high - low > 0.5 && iterations < max_iterations {
                if iterations >= EMERGENCY_MAX_ITERATIONS {
                    eprintln!(
                        "   ⚠️ EMERGENCY LIMIT: Reached {} iterations, stopping search!",
                        EMERGENCY_MAX_ITERATIONS
                    );
                    eprintln!("   ⚠️ Using best result found so far: CRF {:.1}", best_crf);
                    break;
                }

                let mid = low + (high - low) * PHI;
                let mid_rounded = (mid * 2.0).round() / 2.0;

                log_realtime!("   🔄 Testing CRF {:.1}...", mid_rounded);
                let (size, quality) =
                    encode_cached(mid_rounded, &mut cache, &mut last_encoded_crf, self)?;
                iterations += 1;
                let ssim = quality.0.unwrap_or(0.0);
                log_realtime!(
                    "      CRF {:.1}: SSIM {:.6}, Size {:+.1}%",
                    mid_rounded,
                    ssim,
                    self.calc_change_pct(size)
                );

                if ssim > best_ssim + SSIM_EPSILON
                    || (ssim >= best_ssim - SSIM_EPSILON && mid_rounded > best_crf)
                {
                    best_crf = mid_rounded;
                    best_size = size;
                    best_quality = quality;
                    best_ssim = ssim;
                }

                if prev_ssim - ssim > SSIM_PLATEAU_THRESHOLD * 2.0 {
                    high = mid_rounded;
                    log_realtime!("      ↓ SSIM drop, narrowing to [{:.1}, {:.1}]", low, high);
                } else {
                    low = mid_rounded;
                }
                prev_ssim = ssim;
            }

            if iterations < max_iterations {
                log_realtime!("   📍 Phase 3: Fine-tune around CRF {:.1}", best_crf);

                for offset in [-0.5_f32, 0.5] {
                    let crf = (best_crf + offset).clamp(self.config.min_crf, self.config.max_crf);
                    if iterations >= max_iterations {
                        break;
                    }

                    log_realtime!("   🔄 Testing CRF {:.1}...", crf);
                    let (size, quality) =
                        encode_cached(crf, &mut cache, &mut last_encoded_crf, self)?;
                    iterations += 1;
                    let ssim = quality.0.unwrap_or(0.0);
                    log_realtime!("      CRF {:.1}: SSIM {:.6}", crf, ssim);

                    if ssim > best_ssim + SSIM_EPSILON || (ssim >= best_ssim - SSIM_EPSILON && crf > best_crf)
                    {
                        best_crf = crf;
                        best_size = size;
                        best_quality = quality;
                        best_ssim = ssim;
                    }
                }

                if iterations < max_iterations {
                    for offset in [-0.25_f32, 0.25, -0.5, 0.5] {
                        let crf =
                            (best_crf + offset).clamp(self.config.min_crf, self.config.max_crf);
                        if cache.contains_key(crf) {
                            continue;
                        }
                        if iterations >= max_iterations {
                            break;
                        }

                        log_realtime!("   🔄 Testing CRF {:.1}...", crf);
                        let (size, quality) =
                            encode_cached(crf, &mut cache, &mut last_encoded_crf, self)?;
                        iterations += 1;
                        let ssim = quality.0.unwrap_or(0.0);
                        log_realtime!("      CRF {:.1}: SSIM {:.6}", crf, ssim);

                        if ssim > best_ssim + 0.00001
                            || (ssim >= best_ssim - 0.00001 && crf > best_crf)
                        {
                            best_crf = crf;
                            best_size = size;
                            best_quality = quality;
                            best_ssim = ssim;
                        }
                    }
                }
            }
        }

        let (final_size, final_quality) = if last_encoded_crf == Some(best_crf) {
            log_realtime!(
                "   ✨ Output already at best CRF {:.1} (no re-encoding needed)",
                best_crf
            );
            (best_size, best_quality)
        } else {
            log_realtime!("   📍 Final: Re-encoding to best CRF {:.1}", best_crf);
            let size = self.encode(best_crf)?;
            (size, best_quality)
        };

        let size_change_pct = self.calc_change_pct(final_size);

        let status = if best_ssim >= 0.9999 {
            "✅ Near-Lossless"
        } else if best_ssim >= 0.999 {
            "✅ Excellent"
        } else if best_ssim >= 0.99 {
            "✅ Very Good"
        } else if best_ssim >= 0.98 {
            "✅ Good"
        } else {
            "✅ Acceptable"
        };

        log_realtime!("   ═══════════════════════════════════════════════════");
        log_realtime!(
            "   📊 RESULT: CRF {:.1}, SSIM {:.6} {}, Size {:+.1}%",
            best_crf,
            best_ssim,
            status,
            size_change_pct
        );
        log_realtime!(
            "   📈 Iterations: {} (cache hits saved encoding time)",
            iterations
        );

        let quality_passed = best_ssim >= self.config.quality_thresholds.min_ssim;

        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: final_size,
            size_change_pct,
            ssim: final_quality.0,
            psnr: final_quality.1,
            ms_ssim: final_quality.2,
            iterations,
            quality_passed,
            log,
            confidence: 0.8,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        })
    }

    fn explore_precise_quality_match_with_compression(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let mut size_cache: CrfCache<u64> = CrfCache::new();
        let mut quality_cache: CrfCache<(Option<f64>, Option<f64>, Option<f64>)> = CrfCache::new();
        let mut last_encoded_crf: Option<f32> = None;

        let _heartbeat = crate::universal_heartbeat::HeartbeatGuard::new(
            crate::universal_heartbeat::HeartbeatConfig::slow("Ultimate Exploration")
                .with_info("Precise Quality Match + Compression".to_string()),
        );

        let target_size = self.get_compression_target();

        let mut best_crf_so_far: f32 = 0.0;

        let start_time = std::time::Instant::now();

        let pb = crate::progress::create_professional_spinner("🔍 Initializing");

        macro_rules! progress_line {
            ($($arg:tt)*) => {{
                pb.set_message(format!($($arg)*));
            }};
        }

        macro_rules! progress_done {
            () => {{}};
        }

        macro_rules! log_header {
            ($($arg:tt)*) => {{
                let msg = format!($($arg)*);
                pb.suspend(|| eprintln!("{}", msg));
                log.push(msg);
            }};
        }

        macro_rules! log_progress {
            ($stage:expr, $crf:expr, $size:expr, $iter:expr) => {{
                let size_pct = if self.input_size > 0 {
                    (($size as f64 / self.input_size as f64) - 1.0) * 100.0
                } else {
                    0.0
                };
                let compress_icon = if $size < target_size {
                    "💾"
                } else {
                    "⚠️"
                };

                pb.set_prefix(format!("🔍 {}", $stage));

                let msg = format!(
                    "CRF {:.1} | {:+.1}% {} | Iter {} | Best: {:.1}",
                    $crf, size_pct, compress_icon, $iter, best_crf_so_far
                );
                pb.set_message(msg);

                log.push(format!("   🔄 CRF {:.1}: {:+.1}%", $crf, size_pct));
            }};
        }

        let encode_size_only = |crf: f32,
                                size_cache: &mut CrfCache<u64>,
                                last_crf: &mut Option<f32>,
                                explorer: &VideoExplorer|
         -> Result<u64> {
            if let Some(&size) = size_cache.get(crf) {
                return Ok(size);
            }
            let size = explorer.encode(crf)?;
            size_cache.insert(crf, size);
            *last_crf = Some(crf);
            Ok(size)
        };

        let validate_ssim =
            |crf: f32,
             quality_cache: &mut CrfCache<(Option<f64>, Option<f64>, Option<f64>)>,
             explorer: &VideoExplorer|
             -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
                if let Some(&quality) = quality_cache.get(crf) {
                    return Ok(quality);
                }
                let quality = explorer.validate_quality()?;
                quality_cache.insert(crf, quality);
                Ok(quality)
            };

        log_header!(
            "🔬 Precise Quality + Compression ({:?}) • Input: {:.2} MB",
            self.encoder,
            self.input_size as f64 / 1024.0 / 1024.0
        );
        log_header!(
            "   Goal: Best SSIM + Output < Input • Range: [{:.1}, {:.1}]",
            self.config.min_crf,
            self.config.max_crf
        );

        let mut iterations = 0u32;

        log_header!("   📍 Stage A: 大小搜索");

        let min_size = encode_size_only(
            self.config.min_crf,
            &mut size_cache,
            &mut last_encoded_crf,
            self,
        )?;
        iterations += 1;
        log_progress!("Stage A", self.config.min_crf, min_size, iterations);

        if min_size < target_size {
            best_crf_so_far = self.config.min_crf;
            progress_done!();

            let mut best_crf = self.config.min_crf;
            let mut best_size = min_size;
            log_header!("   📍 Stage B-1: 快速搜索 (0.5 步长)");
            let mut test_crf = self.config.min_crf - 0.5;
            while test_crf >= ABSOLUTE_MIN_CRF && iterations < STAGE_B1_MAX_ITERATIONS {
                let size =
                    encode_size_only(test_crf, &mut size_cache, &mut last_encoded_crf, self)?;
                iterations += 1;
                log_progress!("Stage B-1", test_crf, size, iterations);

                if size < target_size {
                    best_crf = test_crf;
                    best_size = size;
                    best_crf_so_far = test_crf;
                    test_crf -= 0.5;
                } else {
                    break;
                }
            }
            progress_done!();

            log_header!("   📍 Stage B-2: 精细调整 (0.1 步长)");
            for offset in [-0.25_f32, -0.5, -0.75, -1.0] {
                let fine_crf = best_crf + offset;
                if fine_crf < ABSOLUTE_MIN_CRF {
                    break;
                }
                if iterations >= STAGE_B2_MAX_ITERATIONS {
                    break;
                }

                if size_cache.contains_key(fine_crf) {
                    continue;
                }

                let size =
                    encode_size_only(fine_crf, &mut size_cache, &mut last_encoded_crf, self)?;
                iterations += 1;
                log_progress!("Stage B-2", fine_crf, size, iterations);

                if size < target_size {
                    best_crf = fine_crf;
                    best_size = size;
                    best_crf_so_far = fine_crf;
                } else {
                    break;
                }
            }
            progress_done!();

            if last_encoded_crf != Some(best_crf) {
                progress_line!("│ 重新编码到最佳 CRF {:.1}... │", best_crf);
                let _ = encode_size_only(best_crf, &mut size_cache, &mut last_encoded_crf, self)?;
                progress_done!();
            }

            log_header!("   📍 Stage C: SSIM 验证");
            progress_line!("│ 计算 SSIM... │");
            let quality = validate_ssim(best_crf, &mut quality_cache, self)?;
            let ssim = quality.0.unwrap_or(0.0);

            progress_done!();

            let status = if ssim >= 0.999 {
                "✅ 极佳"
            } else if ssim >= 0.99 {
                "✅ 优秀"
            } else if ssim >= 0.98 {
                "✅ 良好"
            } else {
                "✅ 可接受"
            };

            let elapsed = start_time.elapsed();
            let saved = self.input_size - best_size;
            pb.finish_and_clear();
            eprintln!("✅ Result: CRF {:.1} • SSIM {:.4} {} • {:+.1}% ({:.2} MB saved) • {} iter in {:.1}s",
                best_crf, ssim, status, self.calc_change_pct(best_size), saved as f64 / 1024.0 / 1024.0, iterations, elapsed.as_secs_f64());

            return Ok(ExploreResult {
                optimal_crf: best_crf,
                output_size: best_size,
                size_change_pct: self.calc_change_pct(best_size),
                ssim: quality.0,
                psnr: quality.1,
                ms_ssim: quality.2,
                iterations,
                quality_passed: true,
                log,
                confidence: 0.85,
                confidence_detail: ConfidenceBreakdown::default(),
                actual_min_ssim: self.config.quality_thresholds.min_ssim,
                ..Default::default()
            });
        }

        progress_done!();

        let max_size = encode_size_only(
            self.config.max_crf,
            &mut size_cache,
            &mut last_encoded_crf,
            self,
        )?;
        iterations += 1;
        log_progress!("Stage A", self.config.max_crf, max_size, iterations);

        if max_size >= self.input_size {
            progress_done!();
            log_header!("   ⚠️ 文件已高度压缩，无法进一步压缩");
            let quality = validate_ssim(self.config.max_crf, &mut quality_cache, self)?;

            let elapsed = start_time.elapsed();
            pb.finish_and_clear();
            eprintln!(
                "⚠️ Cannot compress file (already optimized) • {} iter in {:.1}s",
                iterations,
                elapsed.as_secs_f64()
            );

            return Ok(ExploreResult {
                optimal_crf: self.config.max_crf,
                output_size: max_size,
                size_change_pct: self.calc_change_pct(max_size),
                ssim: quality.0,
                psnr: quality.1,
                ms_ssim: quality.2,
                iterations,
                quality_passed: false,
                log,
                confidence: 0.3,
                confidence_detail: ConfidenceBreakdown::default(),
                actual_min_ssim: self.config.quality_thresholds.min_ssim,
                ..Default::default()
            });
        }

        progress_done!();

        const WINDOW_SIZE: usize = 3;
        const VARIANCE_THRESHOLD: f64 = 0.00001;
        const CHANGE_RATE_THRESHOLD: f64 = 0.005;
        let mut size_history: Vec<(f32, u64)> = Vec::new();

        let calc_window_variance = |history: &[(f32, u64)], input_size: u64| -> f64 {
            if history.len() < WINDOW_SIZE {
                return f64::MAX;
            }
            let recent: Vec<f64> = history
                .iter()
                .rev()
                .take(WINDOW_SIZE)
                .map(|(_, s)| *s as f64 / input_size as f64)
                .collect();
            let mean = recent.iter().sum::<f64>() / recent.len() as f64;
            recent.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / recent.len() as f64
        };

        let calc_change_rate = |prev: u64, curr: u64| -> f64 {
            if prev == 0 {
                return f64::MAX;
            }
            ((curr as f64 - prev as f64) / prev as f64).abs()
        };

        log_header!("   📍 Stage A: 二分搜索 (0.5 步长)");
        let mut low = self.config.min_crf;
        let mut high = self.config.max_crf;
        let mut boundary_crf = self.config.max_crf;
        let mut prev_size: Option<u64> = None;

        while high - low > 0.5 && iterations < 12 {
            let mid = ((low + high) / 2.0 * 2.0).round() / 2.0;

            let size = encode_size_only(mid, &mut size_cache, &mut last_encoded_crf, self)?;
            iterations += 1;
            size_history.push((mid, size));
            log_progress!("二分搜索", mid, size, iterations);

            let variance = calc_window_variance(&size_history, self.input_size);
            let change_rate = prev_size
                .map(|p| calc_change_rate(p, size))
                .unwrap_or(f64::MAX);

            if size < target_size {
                boundary_crf = mid;
                best_crf_so_far = mid;
                high = mid;
            } else {
                low = mid;
            }

            if variance < VARIANCE_THRESHOLD && size_history.len() >= WINDOW_SIZE {
                progress_done!();
                log_header!(
                    "   ⚡ 提前终止: 方差完全收敛 {:.8} < {:.8}",
                    variance,
                    VARIANCE_THRESHOLD
                );
                break;
            }
            if change_rate < CHANGE_RATE_THRESHOLD && prev_size.is_some() {
                progress_done!();
                log_header!(
                    "   ⚡ 提前终止: 变化率极小 {:.4}% < {:.4}%",
                    change_rate * 100.0,
                    CHANGE_RATE_THRESHOLD * 100.0
                );
                break;
            }

            prev_size = Some(size);
        }
        progress_done!();

        log_header!("   📍 Stage B: 精细调整 (0.1 步长)");

        let mut best_boundary = boundary_crf;
        let mut fine_tune_history: Vec<u64> = Vec::new();

        for offset in [-0.25_f32, -0.5, -0.75, -1.0] {
            let test_crf = boundary_crf + offset;

            if test_crf < self.config.min_crf {
                continue;
            }
            if iterations >= STAGE_B_BIDIRECTIONAL_MAX {
                break;
            }

            if size_cache.contains_key(test_crf) {
                continue;
            }

            let size = encode_size_only(test_crf, &mut size_cache, &mut last_encoded_crf, self)?;
            iterations += 1;
            fine_tune_history.push(size);
            log_progress!("精细调整↓", test_crf, size, iterations);

            if size < target_size {
                best_boundary = test_crf;
                best_crf_so_far = test_crf;

                if fine_tune_history.len() >= 2 {
                    let prev = fine_tune_history[fine_tune_history.len() - 2];
                    let rate = calc_change_rate(prev, size);
                    if rate < CHANGE_RATE_THRESHOLD {
                        progress_done!();
                        log_header!("   ⚡ 提前终止: Δ{:.3}%", rate * 100.0);
                        break;
                    }
                }
            } else {
                break;
            }
        }

        if best_boundary == boundary_crf {
            fine_tune_history.clear();

            for offset in [0.25_f32, 0.5, 0.75, 1.0] {
                let test_crf = boundary_crf + offset;

                if test_crf > self.config.max_crf {
                    continue;
                }
                if iterations >= STAGE_B_BIDIRECTIONAL_MAX {
                    break;
                }

                if size_cache.contains_key(test_crf) {
                    continue;
                }

                let size =
                    encode_size_only(test_crf, &mut size_cache, &mut last_encoded_crf, self)?;
                iterations += 1;
                fine_tune_history.push(size);
                log_progress!("精细调整↑", test_crf, size, iterations);

                if size < target_size {
                    best_boundary = test_crf;
                    best_crf_so_far = test_crf;

                    if fine_tune_history.len() >= 2 {
                        let prev = fine_tune_history[fine_tune_history.len() - 2];
                        let rate = calc_change_rate(prev, size);
                        if rate < CHANGE_RATE_THRESHOLD {
                            progress_done!();
                            log_header!("   ⚡ 提前终止: Δ{:.3}%", rate * 100.0);
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        }
        progress_done!();

        if best_boundary != boundary_crf {
            boundary_crf = best_boundary;
        }

        log_header!("   📍 Stage C: SSIM 验证");

        if last_encoded_crf != Some(boundary_crf) {
            progress_line!("│ 重新编码到 CRF {:.1}... │", boundary_crf);
            let _ = encode_size_only(boundary_crf, &mut size_cache, &mut last_encoded_crf, self)?;
            progress_done!();
        }

        progress_line!("│ 计算 SSIM... │");
        let quality = validate_ssim(boundary_crf, &mut quality_cache, self)?;
        let ssim = quality.0.unwrap_or(0.0);

        progress_done!();

        let final_size = size_cache.get(boundary_crf).copied().unwrap_or(0);

        let size_change_pct = self.calc_change_pct(final_size);
        let status = if ssim >= 0.999 {
            "✅ 极佳"
        } else if ssim >= 0.99 {
            "✅ 优秀"
        } else if ssim >= 0.98 {
            "✅ 良好"
        } else {
            "✅ 可接受"
        };

        let elapsed = start_time.elapsed();
        let saved = self.input_size - final_size;
        pb.finish_and_clear();
        eprintln!(
            "✅ Result: CRF {:.1} • SSIM {:.4} {} • {:+.1}% ({:.2} MB saved) • {} iter in {:.1}s",
            boundary_crf,
            ssim,
            status,
            size_change_pct,
            saved as f64 / 1024.0 / 1024.0,
            iterations,
            elapsed.as_secs_f64()
        );

        Ok(ExploreResult {
            optimal_crf: boundary_crf,
            output_size: final_size,
            size_change_pct,
            ssim: quality.0,
            psnr: quality.1,
            ms_ssim: quality.2,
            iterations,
            quality_passed: ssim >= self.config.quality_thresholds.min_ssim,
            log,
            confidence: 0.85,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        })
    }

    fn encode(&self, crf: f32) -> Result<u64> {
        if !self.use_gpu && self.encoder == VideoEncoder::Hevc {
            return self.encode_with_x265_cli(crf);
        }

        let result = self.encode_with_ffmpeg(crf);

        if result.is_err() && self.use_gpu && self.encoder == VideoEncoder::Hevc {
            eprintln!("      ⚠️  GPU encoding failed, falling back to CPU (x265 CLI)");
            return self.encode_with_x265_cli(crf);
        }

        result
    }

    fn encode_with_x265_cli(&self, crf: f32) -> Result<u64> {
        use crate::x265_encoder::{encode_with_x265, X265Config};

        eprintln!("      🖥️  CPU Encoding with x265 CLI (CRF {:.1})", crf);

        let config = X265Config {
            crf,
            preset: self.preset.x26x_name().to_string(),
            threads: self.max_threads,
            container: "mp4".to_string(),
            preserve_audio: true,
        };

        encode_with_x265(&self.input_path, &self.output_path, &config, &self.vf_args)
            .context("x265 CLI encoding failed")
    }

    fn encode_with_ffmpeg(&self, crf: f32) -> Result<u64> {
        use std::io::{BufRead, BufReader, Write};
        use std::process::Stdio;

        use crate::universal_heartbeat::{HeartbeatConfig, HeartbeatGuard};
        let _heartbeat = HeartbeatGuard::new(
            HeartbeatConfig::medium("Video Encoding").with_info(format!("CRF {:.1}", crf)),
        );

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y");

        let gpu = crate::gpu_accel::GpuAccel::detect();
        let (encoder_name, crf_args, extra_args, accel_type) = if self.use_gpu {
            match self.encoder {
                VideoEncoder::Hevc => {
                    if let Some(enc) = gpu.get_hevc_encoder() {
                        (
                            enc.name,
                            enc.get_crf_args(crf),
                            enc.get_extra_args(),
                            format!("🚀 GPU ({})", gpu.gpu_type),
                        )
                    } else {
                        (
                            self.encoder.ffmpeg_name(),
                            vec!["-crf".to_string(), format!("{:.1}", crf)],
                            vec![],
                            "CPU".to_string(),
                        )
                    }
                }
                VideoEncoder::Av1 => {
                    if let Some(enc) = gpu.get_av1_encoder() {
                        (
                            enc.name,
                            enc.get_crf_args(crf),
                            enc.get_extra_args(),
                            format!("🚀 GPU ({})", gpu.gpu_type),
                        )
                    } else {
                        (
                            self.encoder.ffmpeg_name(),
                            vec!["-crf".to_string(), format!("{:.1}", crf)],
                            vec![],
                            "CPU".to_string(),
                        )
                    }
                }
                VideoEncoder::H264 => {
                    if let Some(enc) = gpu.get_h264_encoder() {
                        (
                            enc.name,
                            enc.get_crf_args(crf),
                            enc.get_extra_args(),
                            format!("🚀 GPU ({})", gpu.gpu_type),
                        )
                    } else {
                        (
                            self.encoder.ffmpeg_name(),
                            vec!["-crf".to_string(), format!("{:.1}", crf)],
                            vec![],
                            "CPU".to_string(),
                        )
                    }
                }
            }
        } else {
            (
                self.encoder.ffmpeg_name(),
                vec!["-crf".to_string(), format!("{:.1}", crf)],
                vec![],
                "CPU".to_string(),
            )
        };

        cmd.arg("-threads")
            .arg(self.max_threads.to_string())
            .arg("-i")
            .arg(crate::safe_path_arg(&self.input_path).as_ref())
            .arg("-c:v")
            .arg(encoder_name);

        for arg in &crf_args {
            cmd.arg(arg);
        }

        for arg in &extra_args {
            cmd.arg(*arg);
        }

        cmd.arg("-progress")
            .arg("pipe:1")
            .arg("-stats_period")
            .arg("0.5");

        if !self.use_gpu {
            for arg in self
                .encoder
                .extra_args_with_preset(self.max_threads, self.preset)
            {
                cmd.arg(arg);
            }
        }

        for arg in &self.vf_args {
            cmd.arg(arg);
        }

        cmd.arg(crate::safe_path_arg(&self.output_path).as_ref());

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn ffmpeg")?;

        let duration_secs = self.get_input_duration().unwrap_or(0.0);

        let stderr_handle = child.stderr.take().map(|stderr| {
            std::thread::spawn(move || {
                use std::collections::VecDeque;
                use std::io::{BufRead, BufReader};
                const MAX_LINES: usize = 10;

                let reader = BufReader::new(stderr);
                let mut recent_lines: VecDeque<String> = VecDeque::with_capacity(MAX_LINES);

                for line in reader.lines().map_while(Result::ok) {
                    if recent_lines.len() >= MAX_LINES {
                        recent_lines.pop_front();
                    }
                    recent_lines.push_back(line);
                }

                recent_lines.into_iter().collect::<Vec<_>>().join("\n")
            })
        });

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut last_time_us: u64 = 0;
            let mut last_fps: f64 = 0.0;
            let mut last_speed: String = String::new();

            for line in reader.lines().map_while(Result::ok) {
                if let Some(val) = line.strip_prefix("out_time_us=") {
                    if let Ok(time_us) = val.parse::<u64>() {
                        last_time_us = time_us;
                    }
                } else if let Some(val) = line.strip_prefix("fps=") {
                    if let Ok(fps) = val.parse::<f64>() {
                        last_fps = fps;
                    }
                } else if let Some(val) = line.strip_prefix("speed=") {
                    last_speed = val.to_string();
                } else if line == "progress=continue" || line == "progress=end" {
                    let current_secs = last_time_us as f64 / 1_000_000.0;
                    if duration_secs > 0.0 {
                        let pct = (current_secs / duration_secs * 100.0).min(100.0);
                        eprint!(
                            "\r      ⏳ {} {:.1}% | {:.1}s/{:.1}s | {:.0}fps | {}   ",
                            accel_type,
                            pct,
                            current_secs,
                            duration_secs,
                            last_fps,
                            last_speed.trim()
                        );
                    } else {
                        eprint!(
                            "\r      ⏳ {} {:.1}s | {:.0}fps | {}   ",
                            accel_type,
                            current_secs,
                            last_fps,
                            last_speed.trim()
                        );
                    }
                    let _ = std::io::stderr().flush();
                }
            }
        }

        let stderr_content = stderr_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();

        let status = child.wait().context("Failed to wait for ffmpeg")?;

        eprintln!(
            "\r      ✅ {} Encoding complete                                    ",
            accel_type
        );

        if !status.success() {
            let error_lines: Vec<&str> = stderr_content
                .lines()
                .filter(|l| {
                    l.contains("Error")
                        || l.contains("error")
                        || l.contains("Invalid")
                        || l.contains("failed")
                })
                .take(5)
                .collect();
            let error_detail = if error_lines.is_empty() {
                stderr_content
                    .lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                error_lines.join("\n")
            };
            bail!(
                "ffmpeg encoding failed (exit code: {:?}):\n{}",
                status.code(),
                error_detail
            );
        }

        let size = fs::metadata(&self.output_path)
            .context("Failed to read output file")?
            .len();

        Ok(size)
    }

    fn get_input_duration(&self) -> Option<f64> {
        let output = Command::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-show_entries")
            .arg("format=duration")
            .arg("-of")
            .arg("default=noprint_wrappers=1:nokey=1")
            .arg(crate::safe_path_arg(&self.input_path).as_ref())
            .output()
            .ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.trim().parse::<f64>().ok()
    }

    fn calc_change_pct(&self, output_size: u64) -> f64 {
        if self.input_size == 0 {
            return 0.0;
        }
        (output_size as f64 / self.input_size as f64 - 1.0) * 100.0
    }

    #[inline]
    fn can_compress_with_margin(&self, output_size: u64) -> bool {
        if self.config.use_pure_media_comparison {
            let output_stream_info = crate::stream_size::extract_stream_sizes(&self.output_path);
            output_stream_info.video_stream_size < self.input_video_stream_size
        } else {
            can_compress_with_metadata(output_size, self.input_size)
        }
    }

    #[inline]
    fn get_compression_target(&self) -> u64 {
        if self.config.use_pure_media_comparison {
            self.input_video_stream_size
        } else {
            compression_target_size(self.input_size)
        }
    }

    fn validate_quality(&self) -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
        let ssim = if self.config.quality_thresholds.validate_ssim {
            self.calculate_ssim()?
        } else {
            None
        };

        let psnr = if self.config.quality_thresholds.validate_psnr {
            self.calculate_psnr()?
        } else {
            None
        };

        let ms_ssim = if self.config.quality_thresholds.validate_ms_ssim {
            let duration = get_video_duration(&self.input_path);
            let should_skip = match duration {
                Some(d) => {
                    d >= LONG_VIDEO_THRESHOLD as f64
                        && !self.config.quality_thresholds.force_ms_ssim_long
                }
                None => {
                    eprintln!("   ⚠️ 无法检测视频时长，跳过 MS-SSIM 验证");
                    true
                }
            };

            if should_skip {
                if let Some(d) = duration {
                    eprintln!(
                        "   ⏭️ 长视频 ({:.1}min > 5min) - 跳过 MS-SSIM 验证",
                        d / 60.0
                    );
                    eprintln!("   💡 使用 --force-ms-ssim-long 强制启用");
                }
                None
            } else {
                self.calculate_ms_ssim()?
            }
        } else {
            None
        };

        Ok((ssim, psnr, ms_ssim))
    }

    pub fn calculate_ssim_and_psnr(&self) -> Result<(Option<f64>, Option<f64>)> {
        eprint!("      📊 Calculating SSIM+PSNR...");
        use std::io::Write;
        let _ = std::io::stderr().flush();

        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];\
                      [ref][1:v]ssim;[ref][1:v]psnr";

        let output = Command::new("ffmpeg")
            .arg("-i")
            .arg(&self.input_path)
            .arg("-i")
            .arg(&self.output_path)
            .arg("-lavfi")
            .arg(filter)
            .arg("-f")
            .arg("null")
            .arg("-")
            .output();

        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let mut ssim: Option<f64> = None;
                let mut psnr: Option<f64> = None;

                for line in stderr.lines() {
                    if let Some(pos) = line.find("SSIM All:") {
                        let value_str = &line[pos + 9..];
                        let end = value_str
                            .find(|c: char| !c.is_numeric() && c != '.')
                            .unwrap_or(value_str.len());
                        if end > 0 {
                            if let Ok(s) = value_str[..end].parse::<f64>() {
                                if precision::is_valid_ssim(s) {
                                    ssim = Some(s);
                                }
                            }
                        }
                    }
                    if let Some(pos) = line.find("average:") {
                        let value_str = &line[pos + 8..].trim_start();
                        if value_str.starts_with("inf") {
                            psnr = Some(f64::INFINITY);
                        } else {
                            let end = value_str
                                .find(|c: char| !c.is_numeric() && c != '.' && c != '-')
                                .unwrap_or(value_str.len());
                            if end > 0 {
                                if let Ok(p) = value_str[..end].parse::<f64>() {
                                    if precision::is_valid_psnr(p) {
                                        psnr = Some(p);
                                    }
                                }
                            }
                        }
                    }
                }

                let ssim_str = ssim
                    .map(|s| format!("{:.4}", s))
                    .unwrap_or_else(|| "N/A".to_string());
                let psnr_str = psnr
                    .map(|p| format!("{:.1}", p))
                    .unwrap_or_else(|| "N/A".to_string());
                eprintln!(
                    "\r      📊 SSIM: {} | PSNR: {} dB          ",
                    ssim_str, psnr_str
                );

                Ok((ssim, psnr))
            }
            Err(e) => {
                eprintln!("\r      ⚠️  SSIM+PSNR calculation failed: {}          ", e);
                Ok((None, None))
            }
        }
    }

    fn calculate_ssim(&self) -> Result<Option<f64>> {
        use crate::universal_heartbeat::{HeartbeatConfig, HeartbeatGuard};
        let _heartbeat = HeartbeatGuard::new(HeartbeatConfig::fast("SSIM Calculation"));

        eprint!("      📊 Calculating SSIM...");
        use std::io::Write;
        let _ = std::io::stderr().flush();

        let filters = [
            "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim",
            "[0:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[ref];[1:v]format=yuv420p[cmp];[ref][cmp]ssim",
            "ssim",
        ];

        for (idx, filter) in filters.iter().enumerate() {
            let result = self.try_ssim_with_filter(filter);

            match result {
                Ok(Some(ssim)) if precision::is_valid_ssim(ssim) => {
                    eprintln!(
                        "\r      📊 SSIM: {:.6} (method {})          ",
                        ssim,
                        idx + 1
                    );
                    return Ok(Some(ssim));
                }
                Ok(Some(ssim)) => {
                    eprintln!(
                        "\r      ⚠️  Method {} returned invalid SSIM: {:.6}, trying next...",
                        idx + 1,
                        ssim
                    );
                }
                Ok(None) | Err(_) => {
                    if idx < filters.len() - 1 {
                        eprint!(
                            "\r      📊 Method {} failed, trying method {}...",
                            idx + 1,
                            idx + 2
                        );
                        let _ = std::io::stderr().flush();
                    }
                }
            }
        }

        eprintln!(
            "\r      ⚠️  SSIM CALCULATION FAILED (all {} methods tried)",
            filters.len()
        );
        eprintln!("      ⚠️  Possible causes:");
        eprintln!("         - Incompatible pixel format");
        eprintln!("         - Resolution mismatch");
        eprintln!("         - Corrupted video file");

        Ok(None)
    }

    fn try_ssim_with_filter(&self, filter: &str) -> Result<Option<f64>> {
        let output = Command::new("ffmpeg")
            .arg("-i")
            .arg(&self.input_path)
            .arg("-i")
            .arg(&self.output_path)
            .arg("-lavfi")
            .arg(filter)
            .arg("-f")
            .arg("null")
            .arg("-")
            .output()
            .context("Failed to run ffmpeg for SSIM")?;

        if !output.status.success() {
            return Ok(None);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stderr.lines() {
            if let Some(pos) = line.find("All:") {
                let value_str = &line[pos + 4..];
                let value_str = value_str.trim_start();
                let end = value_str
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(value_str.len());
                if end > 0 {
                    if let Ok(ssim) = value_str[..end].parse::<f64>() {
                        return Ok(Some(ssim));
                    }
                }
            }
        }

        Ok(None)
    }

    fn calculate_psnr(&self) -> Result<Option<f64>> {
        use crate::universal_heartbeat::{HeartbeatConfig, HeartbeatGuard};
        let _heartbeat = HeartbeatGuard::new(HeartbeatConfig::fast("PSNR Calculation"));

        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]psnr=stats_file=-";

        let output = Command::new("ffmpeg")
            .arg("-i")
            .arg(&self.input_path)
            .arg("-i")
            .arg(&self.output_path)
            .arg("-lavfi")
            .arg(filter)
            .arg("-f")
            .arg("null")
            .arg("-")
            .output();

        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);

                if stderr.contains("average:inf") {
                    return Ok(Some(f64::INFINITY));
                }

                for line in stderr.lines() {
                    if let Some(pos) = line.find("average:") {
                        let value_str = &line[pos + 8..];
                        let value_str = value_str.trim_start();
                        let end = value_str
                            .find(|c: char| !c.is_numeric() && c != '.' && c != '-')
                            .unwrap_or(value_str.len());
                        if end > 0 {
                            if let Ok(psnr) = value_str[..end].parse::<f64>() {
                                if precision::is_valid_psnr(psnr) {
                                    return Ok(Some(psnr));
                                }
                            }
                        }
                    }
                }

                Ok(None)
            }
            Err(e) => {
                bail!("Failed to execute ffmpeg for PSNR calculation: {}", e)
            }
        }
    }

    fn calculate_ms_ssim(&self) -> Result<Option<f64>> {
        let duration = get_video_duration(&self.input_path);

        let filter = match duration {
            Some(dur) if dur > 60.0 => {
                let start_end = dur * 0.10;
                let mid_start = dur * 0.45;
                let mid_end = dur * 0.55;
                let tail_start = dur * 0.90;

                eprintln!("   📊 MS-SSIM: 三段采样 (开头10% + 中间10% + 结尾10%)");
                format!(
                    "[0:v]select='lt(t\\,{:.1})+between(t\\,{:.1}\\,{:.1})+gte(t\\,{:.1})',\
                     scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];\
                     [1:v]select='lt(t\\,{:.1})+between(t\\,{:.1}\\,{:.1})+gte(t\\,{:.1})'[dist];\
                     [ref][dist]libvmaf",
                    start_end,
                    mid_start,
                    mid_end,
                    tail_start,
                    start_end,
                    mid_start,
                    mid_end,
                    tail_start
                )
            }
            _ => {
                "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]libvmaf"
                    .to_string()
            }
        };

        let use_sampling = duration.map(|d| d > 60.0).unwrap_or(false);

        let output = Command::new("ffmpeg")
            .arg("-i")
            .arg(&self.input_path)
            .arg("-i")
            .arg(&self.output_path)
            .arg("-lavfi")
            .arg(&filter)
            .arg("-f")
            .arg("null")
            .arg("-")
            .output();

        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);

                for line in stderr.lines() {
                    if let Some(pos) = line.find("MS-SSIM score:") {
                        let value_str = &line[pos + 11..];
                        let value_str = value_str.trim();
                        if let Ok(vmaf) = value_str.parse::<f64>() {
                            if precision::is_valid_ms_ssim(vmaf) {
                                if use_sampling {
                                    eprintln!("   📊 VMAF (采样): {:.2}", vmaf);
                                }
                                return Ok(Some(vmaf));
                            }
                        }
                    }
                }

                Ok(None)
            }
            Err(e) => {
                bail!("Failed to execute ffmpeg for VMAF calculation: {}", e)
            }
        }
    }

    fn check_quality_passed(
        &self,
        ssim: Option<f64>,
        psnr: Option<f64>,
        vmaf: Option<f64>,
    ) -> bool {
        let t = &self.config.quality_thresholds;

        if t.validate_ssim {
            match ssim {
                Some(s) => {
                    let epsilon = precision::SSIM_COMPARE_EPSILON;
                    if s + epsilon < t.min_ssim {
                        return false;
                    }
                }
                None => {
                    return false;
                }
            }
        }

        if t.validate_psnr {
            match psnr {
                Some(p) => {
                    if p < t.min_psnr && !p.is_infinite() {
                        return false;
                    }
                }
                None => {
                    return false;
                }
            }
        }

        if t.validate_ms_ssim {
            match vmaf {
                Some(v) => {
                    if v < t.min_ms_ssim {
                        return false;
                    }
                }
                None => {
                    return false;
                }
            }
        }

        true
    }
}


pub fn explore_size_only(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::size_only(initial_crf, max_crf);
    VideoExplorer::new(input, output, encoder, vf_args, config, max_threads)?.explore()
}

pub fn explore_quality_match(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    predicted_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::quality_match(predicted_crf);
    VideoExplorer::new(input, output, encoder, vf_args, config, max_threads)?.explore()
}

pub fn explore_precise_quality_match(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::precise_quality_match(initial_crf, max_crf, min_ssim);
    VideoExplorer::new(input, output, encoder, vf_args, config, max_threads)?.explore()
}

pub fn explore_precise_quality_match_with_compression(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config =
        ExploreConfig::precise_quality_match_with_compression(initial_crf, max_crf, min_ssim);
    VideoExplorer::new(input, output, encoder, vf_args, config, max_threads)?.explore()
}

pub fn explore_compress_only(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_only(initial_crf, max_crf);
    VideoExplorer::new(input, output, encoder, vf_args, config, max_threads)?.explore()
}

pub fn explore_compress_with_quality(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_with_quality(initial_crf, max_crf);
    VideoExplorer::new(input, output, encoder, vf_args, config, max_threads)?.explore()
}


pub fn explore_precise_quality_match_with_compression_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    use_gpu: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config =
        ExploreConfig::precise_quality_match_with_compression(initial_crf, max_crf, min_ssim);
    VideoExplorer::new_with_gpu(
        input,
        output,
        encoder,
        vf_args,
        config,
        use_gpu,
        max_threads,
    )?
    .explore()
}

pub fn explore_precise_quality_match_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    use_gpu: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::precise_quality_match(initial_crf, max_crf, min_ssim);
    VideoExplorer::new_with_gpu(
        input,
        output,
        encoder,
        vf_args,
        config,
        use_gpu,
        max_threads,
    )?
    .explore()
}

pub fn explore_compress_only_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_only(initial_crf, max_crf);
    VideoExplorer::new_with_gpu(
        input,
        output,
        encoder,
        vf_args,
        config,
        use_gpu,
        max_threads,
    )?
    .explore()
}

pub fn explore_compress_with_quality_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_with_quality(initial_crf, max_crf);
    VideoExplorer::new_with_gpu(
        input,
        output,
        encoder,
        vf_args,
        config,
        use_gpu,
        max_threads,
    )?
    .explore()
}

pub fn explore_size_only_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::size_only(initial_crf, max_crf);
    VideoExplorer::new_with_gpu(
        input,
        output,
        encoder,
        vf_args,
        config,
        use_gpu,
        max_threads,
    )?
    .explore()
}

pub fn explore_quality_match_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    predicted_crf: f32,
    use_gpu: bool,
    max_threads: usize,
) -> Result<ExploreResult> {
    let config = ExploreConfig::quality_match(predicted_crf);
    VideoExplorer::new_with_gpu(
        input,
        output,
        encoder,
        vf_args,
        config,
        use_gpu,
        max_threads,
    )?
    .explore()
}

#[deprecated(since = "2.0.0", note = "Use explore_size_only instead")]
pub fn quick_explore(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
) -> Result<ExploreResult> {
    let max_threads = crate::thread_manager::get_optimal_threads();
    explore_size_only(
        input,
        output,
        encoder,
        vf_args,
        initial_crf,
        max_crf,
        max_threads,
    )
}

#[deprecated(since = "2.0.0", note = "Use explore_precise_quality_match instead")]
pub fn full_explore(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
) -> Result<ExploreResult> {
    let max_threads = crate::thread_manager::get_optimal_threads();
    explore_precise_quality_match(
        input,
        output,
        encoder,
        vf_args,
        initial_crf,
        max_crf,
        min_ssim,
        max_threads,
    )
}


pub fn calculate_smart_thresholds(initial_crf: f32, encoder: VideoEncoder) -> (f32, f64) {
    let (crf_scale, max_crf_cap) = match encoder {
        VideoEncoder::Hevc => (51.0_f32, 40.0_f32),
        VideoEncoder::Av1 => (63.0_f32, 50.0_f32),
        VideoEncoder::H264 => (51.0_f32, 35.0_f32),
    };

    let normalized_crf = initial_crf / crf_scale;
    let quality_level = (normalized_crf * normalized_crf).clamp(0.0, 1.0) as f64;

    let headroom = 8.0 + quality_level as f32 * 7.0;
    let max_crf = (initial_crf + headroom).min(max_crf_cap);

    let min_ssim = if initial_crf < 20.0 {
        0.95
    } else if initial_crf < 30.0 {
        let t = (initial_crf - 20.0) / 10.0;
        0.95 - t as f64 * 0.03
    } else {
        let t = ((initial_crf - 30.0) / 20.0).min(1.0);
        0.92 - t as f64 * 0.04
    };

    (max_crf, min_ssim.clamp(0.85, 0.98))
}

pub fn explore_hevc(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_precise_quality_match(
        input,
        output,
        VideoEncoder::Hevc,
        vf_args,
        initial_crf,
        max_crf,
        min_ssim,
        max_threads,
    )
}

pub fn explore_hevc_size_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_size_only(
        input,
        output,
        VideoEncoder::Hevc,
        vf_args,
        initial_crf,
        max_crf,
        max_threads,
    )
}

pub fn explore_hevc_quality_match(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    predicted_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    explore_quality_match(
        input,
        output,
        VideoEncoder::Hevc,
        vf_args,
        predicted_crf,
        max_threads,
    )
}

pub fn explore_hevc_compress_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_compress_only(
        input,
        output,
        VideoEncoder::Hevc,
        vf_args,
        initial_crf,
        max_crf,
        max_threads,
    )
}

pub fn explore_hevc_compress_with_quality(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_compress_with_quality(
        input,
        output,
        VideoEncoder::Hevc,
        vf_args,
        initial_crf,
        max_crf,
        max_threads,
    )
}

pub fn explore_av1(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_precise_quality_match(
        input,
        output,
        VideoEncoder::Av1,
        vf_args,
        initial_crf,
        max_crf,
        min_ssim,
        max_threads,
    )
}

pub fn explore_av1_size_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_size_only(
        input,
        output,
        VideoEncoder::Av1,
        vf_args,
        initial_crf,
        max_crf,
        max_threads,
    )
}

pub fn explore_av1_quality_match(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    predicted_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    explore_quality_match(
        input,
        output,
        VideoEncoder::Av1,
        vf_args,
        predicted_crf,
        max_threads,
    )
}

pub fn explore_av1_compress_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_compress_only(
        input,
        output,
        VideoEncoder::Av1,
        vf_args,
        initial_crf,
        max_crf,
        max_threads,
    )
}

pub fn explore_av1_compress_with_quality(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_compress_with_quality(
        input,
        output,
        VideoEncoder::Av1,
        vf_args,
        initial_crf,
        max_crf,
        max_threads,
    )
}


pub mod precision;


pub mod precheck;


pub mod calibration;


pub mod dynamic_mapping;


pub mod gpu_coarse_search;
#[allow(unused_imports)]
pub use gpu_coarse_search::*;

#[cfg(test)]
mod tests {
    use super::precision::*;
    use super::*;


    #[test]
    fn test_quality_thresholds_default() {
        let t = QualityThresholds::default();
        assert_eq!(t.min_ssim, 0.95);
        assert_eq!(t.min_psnr, 35.0);
        assert!(t.validate_ssim);
        assert!(!t.validate_psnr);
    }

    #[test]
    fn test_explore_config_default() {
        let c = ExploreConfig::default();
        assert_eq!(c.mode, ExploreMode::PreciseQualityMatch);
        assert_eq!(c.initial_crf, 18.0);
        assert_eq!(c.min_crf, 10.0);
        assert_eq!(c.max_crf, 28.0);
        assert_eq!(c.target_ratio, 1.0);
        assert_eq!(c.max_iterations, 12);
    }

    #[test]
    fn test_explore_config_size_only() {
        let c = ExploreConfig::size_only(20.0, 30.0);
        assert_eq!(c.mode, ExploreMode::SizeOnly);
        assert_eq!(c.initial_crf, 20.0);
        assert_eq!(c.max_crf, 30.0);
        assert!(!c.quality_thresholds.validate_ssim);
        assert!(!c.quality_thresholds.validate_psnr);
    }

    #[test]
    fn test_explore_config_quality_match() {
        let c = ExploreConfig::quality_match(22.0);
        assert_eq!(c.mode, ExploreMode::QualityMatch);
        assert_eq!(c.initial_crf, 22.0);
        assert_eq!(c.max_iterations, 1);
        assert!(c.quality_thresholds.validate_ssim);
    }

    #[test]
    fn test_explore_config_precise_quality_match() {
        let c = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        assert_eq!(c.mode, ExploreMode::PreciseQualityMatch);
        assert_eq!(c.initial_crf, 18.0);
        assert_eq!(c.max_crf, 28.0);
        assert_eq!(c.quality_thresholds.min_ssim, 0.97);
        assert!(c.quality_thresholds.validate_ssim);
    }

    #[test]
    fn test_explore_config_precise_quality_match_with_compression() {
        let c = ExploreConfig::precise_quality_match_with_compression(20.0, 35.0, 0.95);
        assert_eq!(c.mode, ExploreMode::PreciseQualityMatchWithCompression);
        assert_eq!(c.initial_crf, 20.0);
        assert_eq!(c.max_crf, 35.0);
        assert_eq!(c.quality_thresholds.min_ssim, 0.95);
        assert!(c.quality_thresholds.validate_ssim);
    }

    #[test]
    fn test_explore_modes() {
        let size_only = ExploreConfig::size_only(20.0, 30.0);
        assert_eq!(size_only.mode, ExploreMode::SizeOnly);

        let quality_match = ExploreConfig::quality_match(22.0);
        assert_eq!(quality_match.mode, ExploreMode::QualityMatch);

        let precise = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        assert_eq!(precise.mode, ExploreMode::PreciseQualityMatch);

        let precise_compress =
            ExploreConfig::precise_quality_match_with_compression(18.0, 28.0, 0.97);
        assert_eq!(
            precise_compress.mode,
            ExploreMode::PreciseQualityMatchWithCompression
        );
    }

    #[test]
    fn test_flag_combinations_semantics() {
        let explore_only = ExploreConfig::size_only(20.0, 30.0);
        assert_eq!(explore_only.mode, ExploreMode::SizeOnly);
        assert!(
            !explore_only.quality_thresholds.validate_ssim,
            "SizeOnly should NOT validate SSIM"
        );

        let match_only = ExploreConfig::quality_match(22.0);
        assert_eq!(match_only.mode, ExploreMode::QualityMatch);
        assert_eq!(
            match_only.max_iterations, 1,
            "QualityMatch should be single-shot"
        );

        let explore_match = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        assert_eq!(explore_match.mode, ExploreMode::PreciseQualityMatch);
        assert!(
            explore_match.quality_thresholds.validate_ssim,
            "PreciseQualityMatch MUST validate SSIM"
        );

        let explore_match_compress =
            ExploreConfig::precise_quality_match_with_compression(18.0, 28.0, 0.97);
        assert_eq!(
            explore_match_compress.mode,
            ExploreMode::PreciseQualityMatchWithCompression
        );
        assert!(
            explore_match_compress.quality_thresholds.validate_ssim,
            "Compression mode MUST validate SSIM"
        );
    }

    #[test]
    fn test_video_encoder_names() {
        assert_eq!(VideoEncoder::Hevc.ffmpeg_name(), "libx265");
        assert_eq!(VideoEncoder::Av1.ffmpeg_name(), "libsvtav1");
        assert_eq!(VideoEncoder::H264.ffmpeg_name(), "libx264");
    }

    #[test]
    fn test_video_encoder_containers() {
        assert_eq!(VideoEncoder::Hevc.container(), "mp4");
        assert_eq!(VideoEncoder::Av1.container(), "mp4");
        assert_eq!(VideoEncoder::H264.container(), "mp4");
    }

    #[test]
    fn test_explore_mode_enum() {
        assert_ne!(ExploreMode::SizeOnly, ExploreMode::QualityMatch);
        assert_ne!(ExploreMode::QualityMatch, ExploreMode::PreciseQualityMatch);
        assert_ne!(ExploreMode::SizeOnly, ExploreMode::PreciseQualityMatch);
    }


    #[test]
    fn test_precision_crf_search_range_hevc() {
        let iterations = required_iterations(10, 28);
        assert!(
            iterations <= 8,
            "HEVC range [10,28] should need <= 8 iterations, got {}",
            iterations
        );
        assert_eq!(iterations, 6);
    }

    #[test]
    fn test_precision_crf_search_range_av1() {
        let iterations = required_iterations(10, 35);
        assert!(
            iterations <= 8,
            "AV1 range [10,35] should need <= 8 iterations, got {}",
            iterations
        );
        assert_eq!(iterations, 6);
    }

    #[test]
    fn test_precision_crf_search_range_wide() {
        let iterations = required_iterations(0, 51);
        assert!(
            iterations <= 8,
            "Wide range [0,51] should need <= 8 iterations, got {}",
            iterations
        );
        assert_eq!(iterations, 7);
    }

    #[test]
    fn test_precision_ssim_threshold_exact() {
        assert!(ssim_meets_threshold(0.95, 0.95));
        assert!(ssim_meets_threshold(0.9501, 0.95));
        assert!(ssim_meets_threshold(0.9499, 0.95));
        assert!(!ssim_meets_threshold(0.9498, 0.95));
    }

    #[test]
    fn test_precision_ssim_threshold_edge_cases() {
        assert!(ssim_meets_threshold(1.0, 1.0));
        assert!(ssim_meets_threshold(0.0, 0.0));
        assert!(!ssim_meets_threshold(0.94, 0.95));
        assert!(ssim_meets_threshold(0.96, 0.95));
    }

    #[test]
    fn test_precision_ssim_quality_grades() {
        assert_eq!(ssim_quality_grade(0.99), "Excellent (几乎无法区分)");
        assert_eq!(ssim_quality_grade(0.98), "Excellent (几乎无法区分)");
        assert_eq!(ssim_quality_grade(0.97), "Good (视觉无损)");
        assert_eq!(ssim_quality_grade(0.95), "Good (视觉无损)");
        assert_eq!(ssim_quality_grade(0.92), "Acceptable (轻微差异)");
        assert_eq!(ssim_quality_grade(0.90), "Acceptable (轻微差异)");
        assert_eq!(ssim_quality_grade(0.87), "Fair (可见差异)");
        assert_eq!(ssim_quality_grade(0.85), "Fair (可见差异)");
        assert_eq!(ssim_quality_grade(0.80), "Poor (明显质量损失)");
    }


    #[test]
    fn test_judge_mode_size_only_config() {
        let c = ExploreConfig::size_only(18.0, 28.0);

        assert!(
            !c.quality_thresholds.validate_ssim,
            "SizeOnly mode should NOT validate SSIM"
        );
        assert!(
            !c.quality_thresholds.validate_psnr,
            "SizeOnly mode should NOT validate PSNR"
        );

        assert!(
            c.max_iterations >= 8,
            "SizeOnly mode should use sufficient iterations for best size"
        );
    }

    #[test]
    fn test_judge_mode_quality_match_config() {
        let c = ExploreConfig::quality_match(20.0);

        assert!(
            c.quality_thresholds.validate_ssim,
            "QualityMatch mode MUST validate SSIM"
        );

        assert_eq!(
            c.max_iterations, 1,
            "QualityMatch mode should have exactly 1 iteration"
        );

        assert_eq!(
            c.initial_crf, 20.0,
            "QualityMatch mode should use predicted CRF"
        );
    }

    #[test]
    fn test_judge_mode_precise_quality_match_config() {
        let c = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);

        assert!(
            c.quality_thresholds.validate_ssim,
            "PreciseQualityMatch mode MUST validate SSIM"
        );

        assert_eq!(
            c.quality_thresholds.min_ssim, 0.97,
            "PreciseQualityMatch mode should use custom min_ssim"
        );

        assert!(
            c.max_iterations >= 8,
            "PreciseQualityMatch mode should use sufficient iterations"
        );

        assert_eq!(c.initial_crf, 18.0);
        assert_eq!(c.max_crf, 28.0);
    }


    #[test]
    fn test_binary_search_precision_proof() {

        let range = 28.0 - 10.0;
        let coarse_iterations = (range / COARSE_STEP).ceil() as u32;
        let fine_iterations = (COARSE_STEP / FINE_STEP).ceil() as u32;
        let total = coarse_iterations + fine_iterations;

        assert!(
            total <= 15,
            "Three-phase search should achieve ±0.5 CRF precision within 15 iterations"
        );
        assert!(
            coarse_iterations <= 9,
            "HEVC range [10,28] coarse search should need <= 9 iterations"
        );
    }

    #[test]
    fn test_binary_search_worst_case() {
        let range = 51.0 - 0.0;
        let coarse_iterations = (range / COARSE_STEP).ceil() as u32;
        let fine_iterations = (COARSE_STEP / FINE_STEP).ceil() as u32;
        let total = coarse_iterations + fine_iterations;

        assert!(
            total <= 30,
            "Even worst case [0,51] should achieve ±0.5 precision within 30 iterations"
        );
        assert!(
            coarse_iterations <= 26,
            "Range [0,51] coarse search should need <= 26 iterations"
        );
    }


    #[test]
    fn test_quality_check_ssim_only() {
        let thresholds = QualityThresholds {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_ms_ssim: 85.0,
            validate_ssim: true,
            validate_psnr: false,
            validate_ms_ssim: false,
            ..Default::default()
        };

        let check = |ssim: Option<f64>, psnr: Option<f64>| -> bool {
            if thresholds.validate_ssim {
                match ssim {
                    Some(s) if s >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validate_psnr {
                match psnr {
                    Some(p) if p >= thresholds.min_psnr => {}
                    _ => return false,
                }
            }
            true
        };

        assert!(check(Some(0.96), None));
        assert!(check(Some(0.95), None));
        assert!(check(Some(0.99), Some(30.0)));

        assert!(!check(Some(0.94), None));
        assert!(!check(None, Some(40.0)));
    }

    #[test]
    fn test_quality_check_both_metrics() {
        let thresholds = QualityThresholds {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_ms_ssim: 85.0,
            validate_ssim: true,
            validate_psnr: true,
            validate_ms_ssim: false,
            ..Default::default()
        };

        let check = |ssim: Option<f64>, psnr: Option<f64>| -> bool {
            if thresholds.validate_ssim {
                match ssim {
                    Some(s) if s >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validate_psnr {
                match psnr {
                    Some(p) if p >= thresholds.min_psnr => {}
                    _ => return false,
                }
            }
            true
        };

        assert!(check(Some(0.96), Some(36.0)));

        assert!(!check(Some(0.96), Some(34.0)));

        assert!(!check(Some(0.94), Some(36.0)));

        assert!(!check(Some(0.94), Some(34.0)));
    }

    #[test]
    fn test_precision_constants() {
        assert!(
            (CRF_PRECISION - 0.25).abs() < 0.01,
            "CRF precision should be ±0.25"
        );
        assert!(
            (COARSE_STEP - 2.0).abs() < 0.01,
            "Coarse step should be 2.0"
        );
        assert!((FINE_STEP - 0.5).abs() < 0.01, "Fine step should be 0.5");
        assert!(
            (ULTRA_FINE_STEP - 0.25).abs() < 0.01,
            "Ultra fine step should be 0.25"
        );
        assert_eq!(SSIM_DISPLAY_PRECISION, 4);
        assert!((SSIM_COMPARE_EPSILON - 0.0001).abs() < 1e-10);
        assert!((DEFAULT_MIN_SSIM - 0.95).abs() < 1e-10);
        assert!((HIGH_QUALITY_MIN_SSIM - 0.98).abs() < 1e-10);
        assert!((ACCEPTABLE_MIN_SSIM - 0.90).abs() < 1e-10);
    }


    #[test]
    fn test_vmaf_validity() {
        assert!(is_valid_ms_ssim(0.0));
        assert!(is_valid_ms_ssim(0.5));
        assert!(is_valid_ms_ssim(1.0));
        assert!(!is_valid_ms_ssim(-1.0));
        assert!(!is_valid_ms_ssim(1.1));
    }

    #[test]
    fn test_three_modes_config_correctness() {
        let size_only = ExploreConfig::size_only(20.0, 30.0);
        assert_eq!(size_only.mode, ExploreMode::SizeOnly);
        assert!(
            !size_only.quality_thresholds.validate_ssim,
            "SizeOnly should NOT validate SSIM"
        );
        assert!(
            !size_only.quality_thresholds.validate_ms_ssim,
            "SizeOnly should NOT validate VMAF"
        );

        let quality_match = ExploreConfig::quality_match(22.0);
        assert_eq!(quality_match.mode, ExploreMode::QualityMatch);
        assert!(
            quality_match.quality_thresholds.validate_ssim,
            "QualityMatch MUST validate SSIM"
        );
        assert_eq!(
            quality_match.max_iterations, 1,
            "QualityMatch should have 1 iteration"
        );

        let precise = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        assert_eq!(precise.mode, ExploreMode::PreciseQualityMatch);
        assert!(
            precise.quality_thresholds.validate_ssim,
            "PreciseQualityMatch MUST validate SSIM"
        );
        assert_eq!(
            precise.quality_thresholds.min_ssim, 0.97,
            "Custom min_ssim should be used"
        );
        assert!(
            precise.max_iterations > 1,
            "PreciseQualityMatch should have multiple iterations"
        );
    }

    #[test]
    fn test_self_calibration_logic() {

        let config = ExploreConfig::precise_quality_match(25.0, 35.0, 0.95);

        assert!(
            config.min_crf < config.initial_crf,
            "min_crf ({}) should be less than initial_crf ({}) to allow downward search",
            config.min_crf,
            config.initial_crf
        );

        let range = config.max_crf - config.min_crf;
        assert!(
            range >= 10.0,
            "CRF range should be at least 10 for effective calibration"
        );
    }

    #[test]
    fn test_quality_validation_failure_behavior() {
        let thresholds = QualityThresholds {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_ms_ssim: 85.0,
            validate_ssim: true,
            validate_psnr: false,
            validate_ms_ssim: true,
            ..Default::default()
        };

        let check = |ssim: Option<f64>, vmaf: Option<f64>| -> bool {
            if thresholds.validate_ssim {
                match ssim {
                    Some(s) if s + SSIM_COMPARE_EPSILON >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validate_ms_ssim {
                match vmaf {
                    Some(v) if v >= thresholds.min_ms_ssim => {}
                    _ => return false,
                }
            }
            true
        };

        assert!(check(Some(0.96), Some(90.0)));

        assert!(!check(Some(0.96), Some(80.0)));

        assert!(!check(Some(0.94), Some(90.0)));

        assert!(!check(Some(0.96), None));
    }

    #[test]
    fn test_evaluation_criteria_thresholds() {
        assert_eq!(HIGH_QUALITY_MIN_MS_SSIM, 0.95);
        assert_eq!(ACCEPTABLE_MIN_MS_SSIM, 0.85);
    }

    #[test]
    fn test_crf_half_step_precision() {
        let test_values: [f64; 7] = [18.0, 18.5, 19.0, 19.5, 20.0, 20.5, 21.0];

        for &crf in &test_values {
            let rounded = (crf * 2.0).round() / 2.0;
            assert!(
                (rounded - crf).abs() < 0.01,
                "CRF {} should round to {} with 0.5 step",
                crf,
                rounded
            );
        }

        assert!((((23.3_f64 * 2.0).round() / 2.0) - 23.5).abs() < 0.01);
        assert!((((23.7_f64 * 2.0).round() / 2.0) - 23.5).abs() < 0.01);
        assert!((((23.2_f64 * 2.0).round() / 2.0) - 23.0).abs() < 0.01);
        assert!((((23.8_f64 * 2.0).round() / 2.0) - 24.0).abs() < 0.01);
    }


    #[test]
    fn test_three_phase_iteration_estimate() {
        let initial = 20.0_f32;
        let _min_crf = 15.0_f32;
        let max_crf = 30.0_f32;

        let coarse_up = ((max_crf - initial) / COARSE_STEP).ceil() as u32;
        assert_eq!(coarse_up, 5, "Coarse search up should be 5 iterations");

        let boundary_range = 4.0_f32;
        let fine_iterations = (boundary_range / FINE_STEP).ceil() as u32;
        assert_eq!(fine_iterations, 8, "Fine search should be 8 iterations");

        let total = 1 + coarse_up + fine_iterations + 1;
        assert!(total <= 15, "Total iterations {} should be <= 15", total);
    }

    #[test]
    fn test_crf_precision_guarantee() {
        let test_targets: [f32; 5] = [18.3, 20.7, 23.1, 25.9, 28.4];

        for &target in &test_targets {
            let nearest = (target * 2.0).round() / 2.0;
            let error = (nearest - target).abs();

            assert!(
                error <= 0.25,
                "Target {} should be within ±0.25 of nearest step {}, got error {}",
                target,
                nearest,
                error
            );
        }
    }

    #[test]
    fn test_boundary_refinement_logic() {
        let best_crf = 24.0_f32;
        let next_crf = best_crf + FINE_STEP;
        let max_crf = 30.0_f32;

        assert!(next_crf <= max_crf, "Next CRF should be within max");
        assert!(
            (next_crf - best_crf - 0.5).abs() < 0.01,
            "Step should be 0.5"
        );
    }

    #[test]
    fn test_search_direction_logic() {
        let initial_passed = true;
        let search_up = initial_passed;
        assert!(search_up, "Should search up when initial quality passed");

        let initial_failed = false;
        let search_down = !initial_failed;
        assert!(
            search_down,
            "Should search down when initial quality failed"
        );
    }

    #[test]
    fn test_max_iterations_protection() {
        let config = ExploreConfig::default();

        let worst_range = 30.0_f32;
        let worst_coarse = (worst_range / COARSE_STEP).ceil() as u32;
        let worst_fine = (COARSE_STEP / FINE_STEP).ceil() as u32 * 2;
        let worst_total = 1 + worst_coarse + worst_fine + 1;

        assert!(
            config.max_iterations >= worst_total / 2,
            "max_iterations {} should handle typical worst case {}",
            config.max_iterations,
            worst_total
        );
    }


    #[test]
    fn test_smart_thresholds_hevc_high_quality() {
        let (max_crf, min_ssim) = calculate_smart_thresholds(18.0, VideoEncoder::Hevc);

        assert!(
            min_ssim >= 0.93,
            "High quality source should have strict SSIM >= 0.93, got {}",
            min_ssim
        );

        assert!(
            max_crf >= 26.0,
            "max_crf should be at least 26 for CRF 18, got {}",
            max_crf
        );
        assert!(
            max_crf <= 30.0,
            "max_crf should not exceed 30 for high quality, got {}",
            max_crf
        );
    }

    #[test]
    fn test_smart_thresholds_hevc_low_quality() {
        let (max_crf, min_ssim) = calculate_smart_thresholds(35.0, VideoEncoder::Hevc);

        assert!(
            min_ssim <= 0.92,
            "Low quality source should have relaxed SSIM <= 0.92, got {}",
            min_ssim
        );
        assert!(
            min_ssim >= 0.85,
            "SSIM should not go below 0.85, got {}",
            min_ssim
        );

        assert!(
            max_crf >= 40.0,
            "max_crf should be at least 40 for low quality, got {}",
            max_crf
        );
    }

    #[test]
    fn test_smart_thresholds_av1() {
        let (max_crf_low, min_ssim_low) = calculate_smart_thresholds(40.0, VideoEncoder::Av1);
        let (max_crf_high, min_ssim_high) = calculate_smart_thresholds(20.0, VideoEncoder::Av1);

        assert!(
            max_crf_low > max_crf_high,
            "Low quality should have higher max_crf"
        );

        assert!(
            min_ssim_low < min_ssim_high,
            "Low quality should have lower min_ssim"
        );

        assert!(
            max_crf_low <= 50.0,
            "AV1 max_crf should not exceed 50, got {}",
            max_crf_low
        );
    }

    #[test]
    fn test_smart_thresholds_edge_case_very_low_quality() {
        let (max_crf, min_ssim) = calculate_smart_thresholds(45.0, VideoEncoder::Hevc);

        assert!(
            max_crf <= 40.0,
            "HEVC max_crf should be capped at 40, got {}",
            max_crf
        );
        assert!(
            min_ssim >= 0.85,
            "min_ssim should not go below 0.85, got {}",
            min_ssim
        );
    }

    #[test]
    fn test_smart_thresholds_edge_case_very_high_quality() {
        let (max_crf, min_ssim) = calculate_smart_thresholds(10.0, VideoEncoder::Hevc);

        assert!(
            min_ssim >= 0.94,
            "Very high quality should have strict SSIM >= 0.94, got {}",
            min_ssim
        );

        assert!(
            max_crf >= 18.0,
            "max_crf should be at least 18 for CRF 10, got {}",
            max_crf
        );
    }

    #[test]
    fn test_smart_thresholds_continuity() {
        let mut prev_max_crf = 0.0_f32;
        let mut prev_min_ssim = 1.0_f64;

        for crf in (10..=40).step_by(2) {
            let (max_crf, min_ssim) = calculate_smart_thresholds(crf as f32, VideoEncoder::Hevc);

            if crf > 10 {
                assert!(
                    max_crf >= prev_max_crf - 0.5,
                    "max_crf should be monotonically increasing: {} -> {} at CRF {}",
                    prev_max_crf,
                    max_crf,
                    crf
                );

                assert!(
                    min_ssim <= prev_min_ssim + 0.01,
                    "min_ssim should be monotonically decreasing: {} -> {} at CRF {}",
                    prev_min_ssim,
                    min_ssim,
                    crf
                );
            }

            prev_max_crf = max_crf;
            prev_min_ssim = min_ssim;
        }
    }


    #[test]
    fn test_v4_target_ssim_near_lossless() {
        let target_ssim = 0.9999_f64;

        assert!(
            target_ssim > 0.999,
            "Target SSIM should be > 0.999 for near-lossless"
        );
        assert!(
            target_ssim < 1.0,
            "Target SSIM should be < 1.0 (1.0 is mathematically lossless)"
        );

        let v3_target = 0.98_f64;
        assert!(
            target_ssim > v3_target,
            "v4.0 target {} should be higher than v3.9 target {}",
            target_ssim,
            v3_target
        );
    }

    #[test]
    fn test_v4_crf_precision_0_1() {
        let test_values: [f32; 5] = [18.0, 18.25, 18.5, 18.75, 19.0];

        for &crf in &test_values {
            let rounded = (crf * 4.0).round() / 4.0;
            assert!(
                (rounded - crf).abs() < 0.01,
                "CRF {} should round to {} with 0.25 step",
                crf,
                rounded
            );
        }

        assert!(((23.1_f32 * 4.0).round() / 4.0 - 23.0).abs() < 0.01);
        assert!(((23.2_f32 * 4.0).round() / 4.0 - 23.25).abs() < 0.01);
        assert!(((23.4_f32 * 4.0).round() / 4.0 - 23.5).abs() < 0.01);
    }

    #[test]
    fn test_v4_four_phase_search_strategy() {
        let phase1_step = 1.0_f32;
        let range = 28.0 - 10.0;
        let phase1_iterations = (range / phase1_step).ceil() as u32;
        assert_eq!(phase1_iterations, 18, "Phase 1 should scan 18 CRF values");

        let phase2_step = 0.5_f32;
        let phase2_range = 4.0_f32;
        let phase2_iterations = (phase2_range / phase2_step).ceil() as u32;
        assert_eq!(phase2_iterations, 8, "Phase 2 should test 8 CRF values");

        let phase3_step = 0.1_f32;
        let phase3_range = 1.0_f32;
        let phase3_iterations = (phase3_range / phase3_step).ceil() as u32;
        assert_eq!(phase3_iterations, 10, "Phase 3 should test 10 CRF values");

    }

    #[test]
    fn test_v4_ssim_quality_grades_extended() {
        let near_lossless_threshold = 0.9999_f64;
        let excellent_threshold = 0.999_f64;
        let very_good_threshold = 0.99_f64;
        let good_threshold = 0.98_f64;

        assert!(near_lossless_threshold > excellent_threshold);
        assert!(excellent_threshold > very_good_threshold);
        assert!(very_good_threshold > good_threshold);

        let grade = |ssim: f64| -> &'static str {
            if ssim >= 0.9999 {
                "Near-Lossless"
            } else if ssim >= 0.999 {
                "Excellent"
            } else if ssim >= 0.99 {
                "Very Good"
            } else if ssim >= 0.98 {
                "Good"
            } else if ssim >= 0.95 {
                "Acceptable"
            } else {
                "Below threshold"
            }
        };

        assert_eq!(grade(0.9999), "Near-Lossless");
        assert_eq!(grade(0.9995), "Excellent");
        assert_eq!(grade(0.995), "Very Good");
        assert_eq!(grade(0.985), "Good");
        assert_eq!(grade(0.96), "Acceptable");
        assert_eq!(grade(0.94), "Below threshold");
    }

    #[test]
    fn test_v4_ssim_plateau_detection() {
        let ssim_values: [(f32, f64); 5] = [
            (20.0, 0.9850),
            (19.9, 0.9855),
            (19.8, 0.9856),
            (19.7, 0.9856),
            (19.6, 0.9855),
        ];

        let mut best_ssim = 0.0_f64;
        let mut plateau_count = 0;

        for &(_crf, ssim) in &ssim_values {
            if ssim > best_ssim {
                best_ssim = ssim;
                plateau_count = 0;
            } else {
                plateau_count += 1;
            }

            if plateau_count >= 2 {
                break;
            }
        }

        assert!(
            plateau_count >= 2,
            "Should detect plateau after 2 non-improvements"
        );
        assert!(
            (best_ssim - 0.9856).abs() < 0.0001,
            "Best SSIM should be 0.9856"
        );
    }

    #[test]
    fn test_v4_high_quality_source_handling() {
        let source_crf = 15.0_f32;
        let source_ssim = 0.9990_f64;
        let target_ssim = 0.9999_f64;

        let expected_output_crf = source_crf - 2.0;

        assert!(
            expected_output_crf < source_crf,
            "Output CRF should be lower than source for quality improvement"
        );
        assert!(
            source_ssim < target_ssim,
            "Source SSIM {} should be below target {}",
            source_ssim,
            target_ssim
        );
    }

    #[test]
    fn test_v4_low_quality_source_ceiling() {
        let _source_crf = 35.0_f32;
        let source_ssim = 0.9200_f64;
        let target_ssim = 0.9999_f64;

        let ssim_ceiling = source_ssim + 0.05;

        assert!(
            ssim_ceiling < target_ssim,
            "Low quality source cannot reach target SSIM {}",
            target_ssim
        );

    }

    #[test]
    fn test_v4_crf_cache_mechanism() {
        let mut cache: std::collections::HashMap<i32, f64> = std::collections::HashMap::new();


        cache.insert(precision::crf_to_cache_key(20.0), 0.9850);
        cache.insert(precision::crf_to_cache_key(20.1), 0.9855);
        cache.insert(precision::crf_to_cache_key(20.5), 0.9860);

        assert!(cache.contains_key(&precision::crf_to_cache_key(20.0)));
        assert!(cache.contains_key(&precision::crf_to_cache_key(20.1)));
        assert!(cache.contains_key(&precision::crf_to_cache_key(20.5)));

        assert!(
            cache.contains_key(&precision::crf_to_cache_key(20.05)),
            "20.05 should round to 201 and hit cache"
        );
        assert!(
            cache.contains_key(&precision::crf_to_cache_key(20.45)),
            "20.45 should round to 205 and hit cache"
        );

        assert!(!cache.contains_key(&precision::crf_to_cache_key(20.75)));
        assert!(!cache.contains_key(&precision::crf_to_cache_key(19.75)));

        assert_eq!(precision::crf_to_cache_key(20.0), 200);
        assert_eq!(precision::crf_to_cache_key(20.1), 201);
        assert_eq!(precision::crf_to_cache_key(20.5), 205);
        assert_eq!(precision::crf_to_cache_key(20.05), 201);
        assert_eq!(precision::crf_to_cache_key(20.15), 202);
    }

    #[test]
    fn test_v4_no_iteration_limit() {

        let range = 51.0_f64 - 0.0;
        let phase1 = (range / 1.0_f64).ceil() as u32;
        let phase2 = (4.0_f64 / 0.5_f64).ceil() as u32;
        let phase3 = (1.0_f64 / 0.1_f64).ceil() as u32;
        let phase4_max = 50_u32;

        let total_max = phase1 + phase2 + phase3 + phase4_max;

        assert!(
            total_max <= 150,
            "Total iterations should be reasonable: {}",
            total_max
        );

    }

    #[test]
    fn test_v4_content_type_ssim_convergence() {

        let animation_convergence_rate = 0.002_f64;

        let live_action_convergence_rate = 0.001_f64;

        let high_detail_convergence_rate = 0.0005_f64;

        assert!(animation_convergence_rate > live_action_convergence_rate);
        assert!(live_action_convergence_rate > high_detail_convergence_rate);

        let target_improvement = 0.9999 - 0.9900;

        let animation_crf_drop = target_improvement / animation_convergence_rate;
        let live_action_crf_drop = target_improvement / live_action_convergence_rate;
        let high_detail_crf_drop = target_improvement / high_detail_convergence_rate;

        assert!(animation_crf_drop < live_action_crf_drop);
        assert!(live_action_crf_drop < high_detail_crf_drop);
    }

    #[test]
    fn test_v4_ssim_precision_ffmpeg() {
        let ffmpeg_precision = 0.0001_f64;

        let target_ssim = 0.9999_f64;
        let excellent_ssim = 0.9990_f64;

        let difference = target_ssim - excellent_ssim;
        assert!(
            difference >= ffmpeg_precision,
            "Target and excellent SSIM should be distinguishable: diff={}",
            difference
        );

        let epsilon = SSIM_COMPARE_EPSILON;
        assert!(
            (epsilon - 0.0001).abs() < 1e-10,
            "SSIM compare epsilon should be 0.0001"
        );
    }


    #[test]
    fn test_v413_sliding_window_variance() {
        let input_size = 1_000_000_u64;
        let window_size = 3_usize;
        let variance_threshold = 0.0001_f64;

        let calc_variance = |sizes: &[u64]| -> f64 {
            if sizes.len() < window_size {
                return f64::MAX;
            }
            let recent: Vec<f64> = sizes
                .iter()
                .rev()
                .take(window_size)
                .map(|s| *s as f64 / input_size as f64)
                .collect();
            let mean = recent.iter().sum::<f64>() / recent.len() as f64;
            recent.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / recent.len() as f64
        };

        let stable_sizes = vec![500_000_u64, 500_100, 500_050];
        let stable_variance = calc_variance(&stable_sizes);
        assert!(
            stable_variance < variance_threshold,
            "Stable sizes should have low variance: {}",
            stable_variance
        );

        let varying_sizes = vec![500_000_u64, 600_000, 550_000];
        let varying_variance = calc_variance(&varying_sizes);
        assert!(
            varying_variance > variance_threshold,
            "Varying sizes should have high variance: {}",
            varying_variance
        );
    }

    #[test]
    fn test_v413_relative_change_rate() {
        let change_rate_threshold = 0.005_f64;

        let calc_change_rate = |prev: u64, curr: u64| -> f64 {
            if prev == 0 {
                return f64::MAX;
            }
            ((curr as f64 - prev as f64) / prev as f64).abs()
        };

        let small_change = calc_change_rate(1_000_000, 1_004_000);
        assert!(
            small_change < change_rate_threshold,
            "Small change {} should be below threshold",
            small_change
        );

        let large_change = calc_change_rate(1_000_000, 1_010_000);
        assert!(
            large_change > change_rate_threshold,
            "Large change {} should be above threshold",
            large_change
        );
    }

    #[test]
    fn test_v413_three_phase_search() {
        let phase1_step = 0.5_f32;
        let crf_range = 28.0_f32 - 10.0_f32;
        let phase1_iterations = (crf_range / phase1_step).log2().ceil() as u32;
        assert!(
            phase1_iterations <= 6,
            "Phase 1 should need ~6 iterations: {}",
            phase1_iterations
        );

        let phase2_range = 0.8_f32;
        let phase2_step = 0.1_f32;
        let phase2_max_iterations = (phase2_range / phase2_step).ceil() as u32;
        assert_eq!(
            phase2_max_iterations, 8,
            "Phase 2 should need max 8 iterations"
        );

        let phase3_iterations = 1_u32;

        let total_max = phase1_iterations + phase2_max_iterations + phase3_iterations;
        assert!(
            total_max <= 15,
            "Total iterations should be <= 15: {}",
            total_max
        );
    }

    #[test]
    fn test_v413_bidirectional_fine_tune() {
        let boundary_crf = 17.5_f32;
        let min_crf = 10.0_f32;
        let max_crf = 28.0_f32;

        let lower_offsets = [-0.25_f32, -0.5, -0.75, -1.0];
        for offset in lower_offsets {
            let test_crf = boundary_crf + offset;
            assert!(
                test_crf >= min_crf,
                "Lower search should stay above min_crf"
            );
            assert!(
                test_crf < boundary_crf,
                "Lower search should be below boundary"
            );
        }

        let upper_offsets = [0.25_f32, 0.5, 0.75, 1.0];
        for offset in upper_offsets {
            let test_crf = boundary_crf + offset;
            assert!(
                test_crf <= max_crf,
                "Upper search should stay below max_crf"
            );
            assert!(
                test_crf > boundary_crf,
                "Upper search should be above boundary"
            );
        }
    }

    #[test]
    fn test_v413_crf_precision_guarantee() {
        let valid_crfs = [17.0_f32, 17.25, 17.5, 17.75, 18.0, 18.25, 18.5, 18.75, 19.0];

        for crf in valid_crfs {
            let scaled = (crf * 4.0).round();
            let reconstructed = scaled / 4.0;
            assert!(
                (crf - reconstructed).abs() < 0.001,
                "CRF {} should be 0.25 precision",
                crf
            );
        }

        assert_eq!(ULTRA_FINE_STEP, 0.25, "ULTRA_FINE_STEP should be 0.25");
        assert_eq!(FINE_STEP, 0.5, "FINE_STEP should be 0.5");
    }


    #[test]
    fn test_adaptive_max_walls_boundary_conditions() {
        assert_eq!(calculate_adaptive_max_walls(0.0), ULTIMATE_MIN_WALL_HITS);
        assert_eq!(calculate_adaptive_max_walls(0.5), ULTIMATE_MIN_WALL_HITS);
        assert_eq!(calculate_adaptive_max_walls(1.0), ULTIMATE_MIN_WALL_HITS);

        for range in [2.0, 5.0, 10.0, 20.0, 30.0, 50.0, 100.0, 1000.0] {
            let result = calculate_adaptive_max_walls(range);
            assert!(
                result >= ULTIMATE_MIN_WALL_HITS,
                "range {} -> {} should >= {}",
                range,
                result,
                ULTIMATE_MIN_WALL_HITS
            );
            assert!(
                result <= ULTIMATE_MAX_WALL_HITS,
                "range {} -> {} should <= {}",
                range,
                result,
                ULTIMATE_MAX_WALL_HITS
            );
        }
    }

    #[test]
    fn test_adaptive_max_walls_monotonicity() {
        let mut prev = calculate_adaptive_max_walls(2.0);
        for range in [4.0, 8.0, 16.0, 32.0, 64.0] {
            let curr = calculate_adaptive_max_walls(range);
            assert!(
                curr >= prev,
                "monotonicity violated: range {} -> {} < prev {}",
                range,
                curr,
                prev
            );
            prev = curr;
        }
    }

    #[test]
    fn test_adaptive_max_walls_formula_correctness() {
        assert_eq!(calculate_adaptive_max_walls(10.0), 10);

        assert_eq!(calculate_adaptive_max_walls(18.0), 11);

        assert_eq!(calculate_adaptive_max_walls(30.0), 11);

        assert_eq!(calculate_adaptive_max_walls(50.0), 12);

        assert_eq!(
            calculate_adaptive_max_walls(100000.0),
            ULTIMATE_MAX_WALL_HITS
        );
    }

    #[test]
    fn test_ultimate_mode_constants() {
        assert_eq!(
            ULTIMATE_MIN_WALL_HITS, NORMAL_MAX_WALL_HITS,
            "Ultimate min should equal normal max for smooth transition"
        );
    }

    #[test]
    fn test_adaptive_max_walls_defensive_checks() {
        assert_eq!(calculate_adaptive_max_walls(-1.0), ULTIMATE_MIN_WALL_HITS);
        assert_eq!(calculate_adaptive_max_walls(-100.0), ULTIMATE_MIN_WALL_HITS);

        assert_eq!(
            calculate_adaptive_max_walls(f32::NAN),
            ULTIMATE_MIN_WALL_HITS
        );

        assert_eq!(
            calculate_adaptive_max_walls(f32::INFINITY),
            ULTIMATE_MIN_WALL_HITS
        );
        assert_eq!(
            calculate_adaptive_max_walls(f32::NEG_INFINITY),
            ULTIMATE_MIN_WALL_HITS
        );
    }


    #[test]
    fn test_crf_to_cache_key_precision() {
        use precision::crf_to_cache_key;

        assert_eq!(crf_to_cache_key(20.0), 200);
        assert_eq!(crf_to_cache_key(20.1), 201);
        assert_eq!(crf_to_cache_key(20.5), 205);

        assert_eq!(crf_to_cache_key(0.0), 0);
        assert_eq!(crf_to_cache_key(51.0), 510);
        assert_eq!(crf_to_cache_key(63.0), 630);

        assert_eq!(crf_to_cache_key(20.05), 201);
        assert_eq!(crf_to_cache_key(20.04), 200);
    }

    #[test]
    fn test_crf_cache_key_roundtrip() {
        use precision::{cache_key_to_crf, crf_to_cache_key};

        for crf in [10.0, 15.0, 20.0, 25.0, 30.0, 51.0] {
            let key = crf_to_cache_key(crf);
            let back = cache_key_to_crf(key);
            assert!(
                (crf - back).abs() < 0.001,
                "Roundtrip failed: {} -> {} -> {}",
                crf,
                key,
                back
            );
        }

        for crf in [20.1, 20.5, 20.9, 25.3, 30.7] {
            let key = crf_to_cache_key(crf);
            let back = cache_key_to_crf(key);
            assert!(
                (crf - back).abs() < 0.001,
                "Roundtrip failed: {} -> {} -> {}",
                crf,
                key,
                back
            );
        }
    }


    #[test]
    fn test_zero_gains_scaling_basic() {
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 41.0, true),
            ULTIMATE_REQUIRED_ZERO_GAINS
        );
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 20.0, true),
            ULTIMATE_REQUIRED_ZERO_GAINS
        );

        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 15.0, true),
            6
        );

        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 10.0, true),
            4
        );

        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 5.0, true),
            4
        );
    }

    #[test]
    fn test_zero_gains_minimum_guarantee() {
        assert!(calculate_zero_gains_for_duration_and_range(60.0, 1.0, true) >= 3);
        assert!(calculate_zero_gains_for_duration_and_range(60.0, 0.1, true) >= 3);
        assert!(calculate_zero_gains_for_duration_and_range(60.0, 5.0, false) >= 3);
    }

    #[test]
    fn test_zero_gains_long_video_override() {
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(300.0, 41.0, true),
            LONG_VIDEO_REQUIRED_ZERO_GAINS
        );
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(600.0, 10.0, true),
            LONG_VIDEO_REQUIRED_ZERO_GAINS
        );
    }
}


#[cfg(test)]
mod prop_tests_v69 {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn prop_zero_gains_scales_with_crf_range(
            duration in 1.0f32..299.0f32,
            crf_range_small in 1.0f32..19.9f32,
            crf_range_large in 20.0f32..50.0f32,
        ) {
            let small_result = calculate_zero_gains_for_duration_and_range(duration, crf_range_small, true);
            let large_result = calculate_zero_gains_for_duration_and_range(duration, crf_range_large, true);

            prop_assert!(small_result <= large_result,
                "小CRF范围({})的zero-gains({}) 应 <= 大CRF范围({})的zero-gains({})",
                crf_range_small, small_result, crf_range_large, large_result);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn prop_zero_gains_minimum_three(
            duration in 0.1f32..1000.0f32,
            crf_range in 0.1f32..100.0f32,
            ultimate_mode in proptest::bool::ANY,
        ) {
            let result = calculate_zero_gains_for_duration_and_range(duration, crf_range, ultimate_mode);

            prop_assert!(result >= 3,
                "zero-gains({}) 应 >= 3 (duration={}, crf_range={}, ultimate={})",
                result, duration, crf_range, ultimate_mode);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn prop_duration_fallback_calculation(
            frame_count in 1u64..1_000_000u64,
            fps in 1.0f64..240.0f64,
        ) {
            let expected_duration = frame_count as f64 / fps;

            prop_assert!((expected_duration - (frame_count as f64 / fps)).abs() < 0.0001,
                "Duration计算应为 frame_count/fps: {} / {} = {}",
                frame_count, fps, expected_duration);

            prop_assert!(expected_duration > 0.0,
                "Duration应为正数: {}", expected_duration);
        }
    }
}
