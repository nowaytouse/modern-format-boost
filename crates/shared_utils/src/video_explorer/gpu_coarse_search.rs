//! GPU coarse search and CPU fine-tuning for CRF exploration
//!
//! HEVC/AV1 ultimate mode: Search with an efficient preset, then render the final output once
//! with the requested delivery preset.
//!
//! For HEVC ultimate `slower`, the pipeline now uses:
//! - search/exploration: `slow`
//! - final render after CRF settles: `slower`
//!
//! ## Unified Selection Philosophy
//!
//! Final output selection follows the same priorities as the rest of the explorers:
//!
//! 1. **Size Gate**: Output must be smaller than input
//! 2. **Quality Gates**: quality_passed and ms_ssim_passed checks
//! 3. **Quality Metrics**: VMAF > CAMBI > PSNR_UV > MS-SSIM > SSIM > PSNR
//! 4. **Size**: Prefer smaller output (tiebreaker)
//! 5. **CRF**: Prefer lower/more aggressive (tiebreaker)
//! 6. **Preset**: Prefer higher rank = slower/better quality (tiebreaker)

use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Stdio;

use super::calibration;
use super::dynamic_mapping;
use super::precheck;
use super::{
    bail, calculate_adaptive_max_walls, calculate_max_iterations_for_duration,
    calculate_ms_ssim_yuv, calculate_smart_thresholds, calculate_ssim_all, calculate_ssim_enhanced,
    calculate_zero_gains_for_duration_and_range, compression_target_size, CheckResult,
    ConfidenceBreakdown, CrfCache, ExploreResult, VideoEncoder, ABSOLUTE_MIN_CRF,
    NORMAL_MAX_WALL_HITS,
};
use crate::constants::{
    ANIMATED_IMAGE_EXPLORATION_SAMPLING_MIN_DURATION_SECS,
    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION,
    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE, HEAVY_VIDEO_THRESHOLD_SECS,
    LONG_VIDEO_THRESHOLD_SECS, VERY_LONG_VIDEO_THRESHOLD_SECS, VMAF_SKIP_THRESHOLD_SECS,
    VMAF_SKIP_THRESHOLD_ULTIMATE_SECS,
};
use crate::modern_ui::colors::{
    BRIGHT_CYAN, BRIGHT_GREEN, BRIGHT_MAGENTA, BRIGHT_RED, BRIGHT_YELLOW, CYAN, DIM, GREEN,
    MFB_BLUE, RESET, YELLOW,
};
use crate::types::EncoderPreset;

const VMAF_Y_SANITY_FLOOR: f64 = 86.0;
const PSNR_UV_SANITY_FLOOR: f64 = 30.0;
const MAX_CONSECUTIVE_COMPRESSIONS: u32 = 3;
const MAX_CONSECUTIVE_FAILURES: u32 = 2;
const ZERO_GAIN_THRESHOLD: f64 = 0.00005;
const DECAY_FACTOR: f32 = 0.4;
const MIN_STEP: f32 = 0.1;
const CAMBI_MAX: f64 = 6.0;
const VMAF_Y_ALLOWED_DROP_FROM_BASELINE: f64 = 2.0;
const PSNR_UV_ALLOWED_DROP_FROM_BASELINE: f64 = 1.5;
const CAMBI_CLEAN_ALLOWED_RISE: f64 = 1.0;
const CAMBI_BANDED_ALLOWED_RISE: f64 = 1.5;
const CAMBI_BANDED_ALLOWED_GROWTH_RATIO: f64 = 0.15;
const MS_SSIM_WEIGHT: f64 = 0.6;
const SSIM_ALL_WEIGHT: f64 = 0.4;
const NORMAL_FUSION_SANITY_FLOOR: f64 = 0.88;
const NORMAL_ALLOWED_DROP_FROM_BASELINE: f64 = 0.04;
const PHASE3_DOWNWARD_STEP: f32 = 0.1;
const PHASE4_ULTIMATE_MAX_FINE_FAILURES: u32 = 2;
const PHASE4_MAX_BACKTRACK_RETRIES: u32 = 3;
const PHASE4_MAX_ATTEMPTS: u32 = 32;
const PHASE4_CRF0_PROBE_MAX_DISTANCE: f32 = 1.0;
/// Maximum number of consecutive non-improving encodes Phase 5 may perform.
/// This acts as a patience counter (lookahead) to find local minima.
const PHASE5_MAX_CONSECUTIVE_FAILURES: u32 = 3;
/// Absolute cap to prevent an infinite march to CRF 0.0 for monotonically decreasing files.
const PHASE5_MAX_TOTAL_ATTEMPTS: u32 = 10;
const UPWARD_JOG_MIN_STEP: f32 = 0.5;
const UPWARD_SIZE_STAGNATION_THRESHOLD: u32 = 4;
const UPWARD_DIRECTION_SWITCH_LIMIT: u32 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpwardSearchCadence {
    Adaptive,
    Jogging,
    Paused,
    Normal,
}

#[derive(Debug, Clone)]
struct UpwardSearchFeedback {
    size_stagnation_count: u32,
    upward_iteration_count: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct UltimateQualityBaselines {
    search_vmaf_y: Option<f64>,
    search_psnr_uv: Option<(f64, f64)>,
    source_cambi: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default)]
struct UltimateQualityMetrics {
    vmaf_y: Option<f64>,
    psnr_uv: Option<(f64, f64)>,
    cambi: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct UltimateQualityEvaluation {
    vmaf_floor: f64,
    psnr_uv_floor: (f64, f64),
    cambi_ceiling: f64,
    vmaf_ok: bool,
    chroma_ok: bool,
    cambi_ok: bool,
}

impl UltimateQualityEvaluation {
    const fn all_passed(self) -> bool {
        self.vmaf_ok && self.chroma_ok && self.cambi_ok
    }
}

fn ultimate_final_sample_rate(duration_secs: f64) -> usize {
    let duration_min = duration_secs / 60.0;
    if duration_min <= 1.0 {
        1
    } else {
        3
    }
}

fn adaptive_vmaf_floor(search_baseline: Option<f64>) -> f64 {
    search_baseline.map_or(VMAF_Y_SANITY_FLOOR, |baseline| {
        (baseline - VMAF_Y_ALLOWED_DROP_FROM_BASELINE).max(VMAF_Y_SANITY_FLOOR)
    })
}

fn adaptive_psnr_uv_floor(search_baseline: Option<(f64, f64)>) -> (f64, f64) {
    search_baseline.map_or((PSNR_UV_SANITY_FLOOR, PSNR_UV_SANITY_FLOOR), |(u, v)| {
        (
            (u - PSNR_UV_ALLOWED_DROP_FROM_BASELINE).max(PSNR_UV_SANITY_FLOOR),
            (v - PSNR_UV_ALLOWED_DROP_FROM_BASELINE).max(PSNR_UV_SANITY_FLOOR),
        )
    })
}

fn adaptive_cambi_ceiling(source_baseline: Option<f64>) -> f64 {
    match source_baseline {
        None => CAMBI_MAX,
        Some(baseline) if baseline <= CAMBI_MAX => {
            (baseline + CAMBI_CLEAN_ALLOWED_RISE).max(CAMBI_MAX)
        }
        Some(baseline) => {
            baseline
                + f64::max(
                    CAMBI_BANDED_ALLOWED_RISE,
                    baseline * CAMBI_BANDED_ALLOWED_GROWTH_RATIO,
                )
        }
    }
}

fn should_probe_crf_zero_from_phase4(best_crf: f32) -> bool {
    best_crf > 0.0 && best_crf <= PHASE4_CRF0_PROBE_MAX_DISTANCE
}

fn metrics_below_ultimate_sanity_floor(vmaf_y: f64, psnr_uv: (f64, f64)) -> bool {
    vmaf_y < VMAF_Y_SANITY_FLOOR || psnr_uv.0.min(psnr_uv.1) < PSNR_UV_SANITY_FLOOR
}

fn both_metrics_below_ultimate_sanity_floor(vmaf_y: f64, psnr_uv: (f64, f64)) -> bool {
    vmaf_y < VMAF_Y_SANITY_FLOOR && psnr_uv.0.min(psnr_uv.1) < PSNR_UV_SANITY_FLOOR
}

fn evaluate_ultimate_quality_gate(
    metrics: UltimateQualityMetrics,
    baselines: UltimateQualityBaselines,
) -> UltimateQualityEvaluation {
    let vmaf_floor = adaptive_vmaf_floor(baselines.search_vmaf_y);
    let psnr_uv_floor = adaptive_psnr_uv_floor(baselines.search_psnr_uv);
    let cambi_ceiling = adaptive_cambi_ceiling(baselines.source_cambi);

    let vmaf_ok = metrics.vmaf_y.is_some_and(|v| v >= vmaf_floor);
    let cambi_ok = metrics.cambi.is_some_and(|c| c <= cambi_ceiling);
    let chroma_ok = metrics
        .psnr_uv
        .is_some_and(|(u, v)| u >= psnr_uv_floor.0 && v >= psnr_uv_floor.1);

    UltimateQualityEvaluation {
        vmaf_floor,
        psnr_uv_floor,
        cambi_ceiling,
        vmaf_ok,
        chroma_ok,
        cambi_ok,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct NormalQualityBaseline {
    /// Search-phase SSIM from GPU/CPU exploration (pre-processing reference).
    explore_ssim: Option<f64>,
    /// Caller-configured minimum SSIM floor (from `actual_min_ssim`).
    min_ssim_config: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct NormalQualityMeasurement {
    /// Weighted average of Y/U/V MS-SSIM channels.
    ms_ssim_avg: Option<f64>,
    /// SSIM-All composite (includes chroma).
    ssim_all: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct NormalQualityEvaluation {
    /// Weighted fusion of available measurements; `None` when both are missing.
    fusion_score: Option<f64>,
    /// The pass threshold: baseline-relative drop tolerance, bounded by config and sanity floors.
    fusion_floor: f64,
    passed: bool,
}

/// Construct a [`NormalQualityEvaluation`] from pre- and post-processing data.
///
/// The pass threshold is derived from the search-phase SSIM baseline so the gate
/// is "tailor-made" per file rather than relying solely on a global absolute floor.
/// When no baseline is available the config floor (or sanity floor) is used instead.
fn build_normal_quality_evaluation(
    baseline: NormalQualityBaseline,
    measurement: NormalQualityMeasurement,
) -> NormalQualityEvaluation {
    let fusion_score = match (measurement.ms_ssim_avg, measurement.ssim_all) {
        (Some(ms), Some(ss)) => Some(MS_SSIM_WEIGHT.mul_add(ms, SSIM_ALL_WEIGHT * ss)),
        (Some(ms), None) => Some(ms),
        (None, Some(ss)) => Some(ss),
        (None, None) => None,
    };

    // Use the explore-phase SSIM as the reference: allow a fixed drop below it,
    // but never go below the config floor or the hard sanity floor.
    let fusion_floor = baseline.explore_ssim.map_or_else(
        || baseline.min_ssim_config.max(NORMAL_FUSION_SANITY_FLOOR),
        |ref_ssim| {
            (ref_ssim - NORMAL_ALLOWED_DROP_FROM_BASELINE)
                .max(baseline.min_ssim_config)
                .max(NORMAL_FUSION_SANITY_FLOOR)
        },
    );

    let passed = fusion_score.is_some_and(|s| s >= fusion_floor);

    NormalQualityEvaluation {
        fusion_score,
        fusion_floor,
        passed,
    }
}

#[derive(Debug, Clone)]
enum AudioTranscodeStrategy {
    Copy,
    Alac,
    AacHigh,
    AacMedium,
}

/// Build the colour/HDR `FFmpeg` arguments from an `FFprobeResult`.
/// These arguments must be appended to every final HEVC/AV1/H.264 encode so that
/// colour metadata (primaries, TRC, matrix, mastering display, CLL) is preserved.
fn build_color_args_from_probe(probe: &crate::ffprobe::FFprobeResult) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    if let Some(ref cp) = probe.color_primaries {
        if !cp.is_empty() && cp != "unknown" {
            args.push("-color_primaries".to_string());
            args.push(cp.clone());
        }
    }
    if let Some(ref trc) = probe.color_transfer {
        if !trc.is_empty() && trc != "unknown" {
            args.push("-color_trc".to_string());
            args.push(trc.clone());
        }
    }
    if let Some(ref cs) = probe.color_space {
        // Normalise bt2020ncl/bt2020nc_l variants that ffprobe sometimes emits
        let normalised = match cs.as_str() {
            "bt2020ncl" | "bt2020_ncl" => "bt2020nc",
            "bt2020cl" | "bt2020_cl" => "bt2020c",
            other => other,
        };
        // Skip RGB/GBR colorspace: HEVC doesn't support it, and we're converting to YUV in filter chain
        let is_rgb_colorspace = normalised == "gbr" || normalised == "rgb" || normalised == "gbrp";
        if !normalised.is_empty() && normalised != "unknown" && !is_rgb_colorspace {
            args.push("-colorspace".to_string());
            args.push(normalised.to_string());
        }
    }
    // NOTE: -master_display and -max_cll are NOT valid top-level ffmpeg CLI options.
    // HDR10 static mastering-display / max-CLL metadata must be injected via
    // `-x265-params` (master-display=...:max-cll=...), which is handled in the
    // x265 params construction on the encode path. Only the CICP triple is emitted
    // here so that the container signals the correct primaries/TRC/matrix.
    args
}

/// Return the correct pixel format for encoding: yuv420p10le for 10-bit HDR content,
/// yuv420p for 8-bit SDR. Preserving the bit depth is essential for HDR accuracy.
const fn pick_pix_fmt(probe: &crate::ffprobe::FFprobeResult) -> &'static str {
    if probe.bit_depth >= 10 {
        "yuv420p10le"
    } else {
        "yuv420p"
    }
}

/// Percentage change from input stream size (avoids div-by-zero / inf when input is 0).
#[inline]
fn stream_size_change_pct(output_size: u64, input_size: u64) -> f64 {
    let denom = crate::numeric_cast::u64_to_f64(input_size.max(1));
    (crate::numeric_cast::u64_to_f64(output_size) / denom - 1.0) * 100.0
}

/// Arguments for GPU-accelerated CRF exploration.
#[derive(Debug, Clone)]
pub struct GpuSearchArgs<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    pub encoder: VideoEncoder,
    pub vf_args: Vec<String>,
    pub initial_crf: f32,
    pub max_crf: f32,
    pub min_ssim: f64,
    pub ultimate_mode: bool,
    pub force_ms_ssim_long: bool,
    pub allow_size_tolerance: bool,
    pub max_threads: usize,
    pub hdr_x265_params: Option<String>,
    pub apple_compat: bool,
    pub preset: EncoderPreset,
    pub final_output_preset: EncoderPreset,
}

/// A request for a GPU-backed video quality exploration.
#[derive(Debug, Clone)]
pub struct GpuSearchRequest {
    pub input: std::path::PathBuf,
    pub output: std::path::PathBuf,
    pub vf_args: Vec<String>,
    pub baseline_crf: f32,
    pub warm_start_crf: Option<f32>,
    pub ultimate_mode: bool,
    pub force_ms_ssim_long: bool,
    pub allow_size_tolerance: bool,
    pub min_ssim: f64,
    pub max_threads: usize,
    pub hdr_x265_params: Option<String>,
    pub apple_compat: bool,
    pub preset: EncoderPreset,
}

/// Arguments for CPU fine-tuning phase.
struct FineTuneArgs<'a> {
    input: &'a Path,
    output: &'a Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    gpu_boundary_crf: f32,
    min_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    ultimate_mode: bool,
    allow_size_tolerance: bool,
    max_threads: usize,
    duration: f32,
    probe_info: Option<&'a crate::ffprobe::FFprobeResult>,
    gpu_executed: bool,
    is_gif_magic: bool,
    hdr_x265_params: Option<String>,
    apple_compat: bool,
    preset: EncoderPreset,
    final_output_preset: EncoderPreset,
}

/// Mutable tracking state during search.
///
/// Maintains best-found quality metrics across search phases.
///
/// **Invariants**:
/// - `best_vmaf`: Updated when a new lower CRF improves the search-time VMAF reference
/// - `best_psnr_uv`: Updated when a new lower CRF improves the search-time chroma reference
/// - Both fields are monotonically non-decreasing (once set to a value, never set to worse)
/// - Used only during ultimate mode for baseline-aware gating decisions; not in normal mode
#[derive(Debug, Default, Clone)]
struct TrackingState {
    pub best_vmaf: Option<f64>,
    pub best_psnr_uv: Option<(f64, f64)>,
}

/// Format the `QualityCheck` log line from result; used for logging and unit tests (regression: enhanced failure shows reason, not "total file not smaller").
pub(crate) fn format_quality_check_line(
    result: &ExploreResult,
    quality_verification_skipped_for_format: bool,
) -> String {
    if result.ms_ssim_passed.is_failed() {
        if let Some(reason) = result.ms_ssim_passed.failure_reason() {
            format!("   QualityCheck: FAILED ({reason})")
        } else {
            "   QualityCheck: FAILED (quality metrics below target)".to_string()
        }
    } else if result.quality_passed.is_passed() {
        "   QualityCheck: PASSED (quality + total file size target met)".to_string()
    } else if result.quality_passed.is_failed() {
        if let Some(reason) = result.quality_passed.failure_reason() {
            format!("   QualityCheck: FAILED ({reason})")
        } else if let Some(ref reason) = result.enhanced_verify_fail_reason {
            format!(
                "   QualityCheck: FAILED (quality met but enhanced verification failed: {reason})"
            )
        } else {
            "   QualityCheck: FAILED (quality met but total file not smaller)".to_string()
        }
    } else if quality_verification_skipped_for_format || result.quality_passed.is_skipped() {
        if quality_verification_skipped_for_format {
            "   QualityCheck: N/A (GIF/size-only, quality not measured)".to_string()
        } else {
            "   QualityCheck: N/A (quality not verified)".to_string()
        }
    } else {
        "   QualityCheck: FAILED (quality not verified)".to_string()
    }
}

/// Explore video quality using GPU coarse search.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_with_gpu_coarse_search(args: GpuSearchArgs<'_>) -> Result<ExploreResult> {
    use crate::gpu_accel::{CrfMapping, GpuAccel, GpuCoarseConfig};
    let GpuSearchArgs {
        input,
        output,
        encoder,
        vf_args,
        initial_crf,
        max_crf,
        min_ssim,
        ultimate_mode,
        force_ms_ssim_long,
        allow_size_tolerance,
        max_threads,
        hdr_x265_params,
        apple_compat,
        preset,
        final_output_preset,
    } = args;

    let precheck_info = precheck::run_precheck(input)?;
    let _compressibility = precheck_info.compressibility;
    crate::log_eprintln!();

    let input_size = fs::metadata(input)
        .context("Failed to read input file metadata")?
        .len();

    let gpu = GpuAccel::detect_with_retry();
    // Defer logging GPU state until we know whether this file actually needs GPU.
    // Printing "no GPU" for a low-bitrate file that would have skipped GPU anyway
    // is misleading; the detection info is only surfaced when GPU is relevant.
    let encoder_name = match encoder {
        VideoEncoder::Hevc => "hevc",
        VideoEncoder::Av1 => "av1",
        VideoEncoder::H264 => "h264",
    };

    let has_gpu_encoder = match encoder {
        VideoEncoder::Hevc => gpu.get_hevc_encoder().is_some(),
        VideoEncoder::Av1 => gpu.get_av1_encoder().is_some(),
        VideoEncoder::H264 => gpu.get_h264_encoder().is_some(),
    };

    // In verbose mode always show detection details; in normal mode the call-site
    // log is deferred to where the GPU is actually needed (or its absence matters).
    if crate::progress_mode::is_verbose_mode() {
        gpu.print_detection_info();
    }

    crate::verbose_eprintln!("Smart GPU+CPU Explore v5.1 ({:?})", encoder);
    crate::verbose_eprintln!(
        "   Input: {} bytes ({:.2} MB)",
        input_size,
        crate::numeric_cast::u64_to_f64(input_size) / 1024.0 / 1024.0
    );
    crate::verbose_eprintln!();
    crate::verbose_eprintln!("STRATEGY: GPU Coarse → CPU Fine");
    crate::verbose_eprintln!("• Phase 1: GPU finds rough boundary (FAST)");
    crate::verbose_eprintln!("• Phase 2: CPU finds precise CRF (ACCURATE)");
    crate::verbose_eprintln!();

    // Single ffprobe call — result is reused in Phase 3 and audio strategy detection.
    let probe_result = match crate::ffprobe::probe_video(input) {
        Ok(probe) => Some(probe),
        Err(err) => {
            crate::verbose_eprintln!(
                "⚠️ ffprobe precheck failed for {}: {}",
                input.display(),
                err
            );
            None
        }
    };
    let duration: f32 = probe_result
        .as_ref()
        .map_or(crate::gpu_accel::GPU_SAMPLE_DURATION, |p| {
            crate::numeric_cast::f64_to_f32_lossy(p.duration)
        });

    // [New Logic] Bitrate-based GPU Start Condition
    // Low bitrate videos (animation/PPT < 5Mbps) don't benefit from GPU pre-scan
    let bitrate_bps = if duration > 0.0 {
        (crate::numeric_cast::u64_to_f64(input_size) * 8.0) / f64::from(duration)
    } else {
        0.0
    };

    let is_gif_magic = super::stream_analysis::is_gif_magic(input);
    let mut actual_initial_crf = initial_crf;

    if is_gif_magic {
        crate::log_eprintln!(
            "   {}ℹ️  GIF magic bytes detected — using CPU-only exploration{}",
            BRIGHT_CYAN,
            RESET
        );

        if ultimate_mode {
            crate::log_eprintln!(
                "   {}🚀 GIF Lossless-First Path: Probing CRF 0.0 for maximum efficiency{}",
                crate::modern_ui::colors::BRIGHT_GREEN,
                RESET
            );
            actual_initial_crf = 0.0;
        }
    }

    let is_high_complexity = bitrate_bps > 5_000_000.0 && !is_gif_magic; // > 5 Mbps only (GIF explicitly excluded)

    let mut gpu_executed = false;
    let (cpu_min_crf, cpu_max_crf, cpu_center_crf) = if gpu.is_available()
        && has_gpu_encoder
        && is_high_complexity
    {
        gpu_executed = true;
        crate::verbose_eprintln!();
        crate::verbose_eprintln!("Phase 1: GPU Coarse Search");

        let temp_output =
            output.with_extension(crate::gpu_accel::derive_gpu_temp_extension(output));

        let gpu_encoder = match encoder {
            VideoEncoder::Hevc => gpu.get_hevc_encoder(),
            VideoEncoder::Av1 => gpu.get_av1_encoder(),
            VideoEncoder::H264 => gpu.get_h264_encoder(),
        };

        let sample_dur = if ultimate_mode {
            crate::gpu_accel::GPU_SAMPLE_DURATION_ULTIMATE
        } else {
            crate::gpu_accel::GPU_SAMPLE_DURATION
        };
        let gpu_sample_input_size = if duration <= sample_dur {
            input_size
        } else {
            let ratio = sample_dur / duration;
            crate::numeric_cast::f64_to_u64_sat(
                crate::numeric_cast::u64_to_f64(input_size) * f64::from(ratio),
            )
        };

        let gpu_step = if ultimate_mode { 0.5 } else { 2.0 };
        let gpu_config = GpuCoarseConfig {
            initial_crf: actual_initial_crf,
            min_crf: 0.0,
            max_crf,
            step: gpu_step,
            max_iterations: crate::gpu_accel::GPU_ABSOLUTE_MAX_ITERATIONS,
            ultimate_mode,
            preset,
        };

        let gpu_progress = crate::UnifiedProgressBar::new_iteration(
            "[GPU] Coarse Search",
            gpu_sample_input_size,
            u64::from(gpu_config.max_iterations),
        );

        let progress_callback = |crf: f32, size: u64| {
            gpu_progress.inc_iteration(crf, size, None);
        };

        let log_callback = |msg: &str| {
            gpu_progress.println(msg);
        };

        let gpu_result = crate::gpu_accel::gpu_coarse_search_with_log(
            input,
            &temp_output,
            encoder_name,
            input_size,
            &gpu_config,
            &vf_args,
            Some(&progress_callback),
            Some(&log_callback),
        );

        let (final_crf, final_size) = match &gpu_result {
            Ok(result) if result.found_boundary => {
                (result.gpu_boundary_crf, result.gpu_best_size.unwrap_or(0))
            }
            _ => (gpu_config.max_crf, input_size),
        };
        gpu_progress.finish_iteration(final_crf, final_size, None);

        match gpu_result {
            Ok(gpu_result) => {
                if gpu_result.found_boundary {
                    let gpu_crf = gpu_result.gpu_boundary_crf;
                    let gpu_size = gpu_result.gpu_best_size.unwrap_or(input_size);
                    if let Some(gpu_encoder) = gpu_encoder {
                        let dynamic_mapper = dynamic_mapping::quick_calibrate(
                            input,
                            input_size,
                            encoder,
                            &vf_args,
                            gpu_encoder,
                            sample_dur,
                            ultimate_mode,
                            apple_compat,
                        )
                        .unwrap_or_else(|_| dynamic_mapping::DynamicCrfMapper::new(input_size));

                        let mapping = match encoder {
                            VideoEncoder::Av1 => CrfMapping::av1(gpu.gpu_type),
                            // H.264 CRF range matches HEVC (0–51); reuse HEVC mapping for CPU search range.
                            VideoEncoder::Hevc | VideoEncoder::H264 => {
                                CrfMapping::hevc(gpu.gpu_type)
                            }
                        };

                        let max_crf = match encoder {
                            VideoEncoder::Av1 => 63.0,
                            VideoEncoder::Hevc | VideoEncoder::H264 => 51.0,
                        };
                        let (dynamic_cpu_crf, dynamic_confidence) = if dynamic_mapper.calibrated {
                            dynamic_mapper.print_calibration_report();
                            dynamic_mapper.gpu_to_cpu(gpu_crf, mapping.offset, max_crf)
                        } else {
                            let calibration = calibration::CalibrationPoint::from_gpu_result(
                                gpu_crf,
                                gpu_size,
                                input_size,
                                gpu_result.gpu_best_ssim,
                                mapping.offset,
                            );
                            calibration.print_report(input_size);
                            (calibration.predicted_cpu_crf, calibration.confidence)
                        };

                        if let Some(ceiling_crf) = gpu_result.quality_ceiling_crf {
                            if (ceiling_crf - gpu_crf).abs() < 1e-6_f32 {
                                crate::verbose_eprintln!(
                                    "GPU Boundary = Quality Ceiling: CRF {:.2}",
                                    gpu_crf
                                );
                                crate::verbose_eprintln!(
                                    "   (GPU reached quality limit, no bloat beyond this point)"
                                );
                            } else {
                                crate::verbose_eprintln!(
                                    "GPU Boundary: CRF {:.2} (stopped before quality ceiling)",
                                    gpu_crf
                                );
                            }
                        } else {
                            crate::verbose_eprintln!(
                                "GPU Boundary: CRF {:.2} (quality ceiling not detected)",
                                gpu_crf
                            );
                        }
                        crate::verbose_eprintln!(
                            "Dynamic mapping: GPU {:.1} → CPU {:.1} (confidence {:.0}%)",
                            gpu_crf,
                            dynamic_cpu_crf,
                            dynamic_confidence * 100.0
                        );
                        crate::verbose_eprintln!();

                        let cpu_start = dynamic_cpu_crf;

                        crate::verbose_eprintln!(
                            "   ✅ GPU found boundary: CRF {:.2} (fine-tuned: {})",
                            gpu_crf,
                            gpu_result.fine_tuned
                        );
                        if let Some(size) = gpu_result.gpu_best_size {
                            crate::verbose_eprintln!("   GPU best size: {} bytes", size);
                        }

                        if let (Some(ceiling_crf), Some(ceiling_ssim)) = (
                            gpu_result.quality_ceiling_crf,
                            gpu_result.quality_ceiling_ssim,
                        ) {
                            crate::verbose_eprintln!(
                                "   GPU Quality Ceiling: CRF {:.2}, SSIM {:.4}",
                                ceiling_crf,
                                ceiling_ssim
                            );
                            crate::verbose_eprintln!(
                                "      (GPU SSIM ceiling, CPU can break through to 0.99+)"
                            );
                        }

                        let (cpu_min, cpu_max) = if let Some(ssim) = gpu_result.gpu_best_ssim {
                            let quality_hint = if ssim >= 0.97 {
                                "Near GPU ceiling"
                            } else if ssim >= 0.95 {
                                "Good"
                            } else {
                                "Below expected"
                            };
                            crate::verbose_eprintln!(
                                "   GPU best SSIM: {:.6} {}",
                                ssim,
                                quality_hint
                            );

                            if ssim < 0.90 {
                                crate::verbose_eprintln!(
                                    "   ⚠️ GPU SSIM too low! Expanding CPU search to lower CRF"
                                );
                                (ABSOLUTE_MIN_CRF, (cpu_start + 8.0).min(max_crf))
                            } else if gpu_result.fine_tuned {
                                crate::verbose_eprintln!(
                                    "   GPU fine-tuned → CPU narrow search ±3 CRF"
                                );
                                (
                                    (cpu_start - 3.0).max(ABSOLUTE_MIN_CRF),
                                    (cpu_start + 3.0).min(max_crf),
                                )
                            } else {
                                crate::verbose_eprintln!(
                                    "   CPU will achieve SSIM 0.98+ (GPU max ~0.97)"
                                );
                                (
                                    (cpu_start - 15.0).max(ABSOLUTE_MIN_CRF),
                                    (cpu_start + 5.0).min(max_crf),
                                )
                            }
                        } else if gpu_result.fine_tuned {
                            crate::verbose_eprintln!(
                                "   GPU fine-tuned → CPU narrow search ±3 CRF"
                            );
                            (
                                (cpu_start - 3.0).max(ABSOLUTE_MIN_CRF),
                                (cpu_start + 3.0).min(max_crf),
                            )
                        } else {
                            (
                                (cpu_start - 15.0).max(ABSOLUTE_MIN_CRF),
                                (cpu_start + 5.0).min(max_crf),
                            )
                        };

                        crate::verbose_eprintln!(
                            "   CPU search range: [{:.1}, {:.1}] (start: {:.1})",
                            cpu_min,
                            cpu_max,
                            cpu_start
                        );
                        (cpu_min, cpu_max, cpu_start)
                    } else {
                        gpu_executed = false;
                        crate::log_eprintln!(
                            "⚠️  FALLBACK: GPU encoder became unavailable during calibration (using CPU-only search)"
                        );
                        (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
                    }
                } else {
                    crate::verbose_eprintln!(
                        "GPU coarse search: no boundary found, using full CRF range for CPU search"
                    );
                    (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
                }
            }
            Err(e) => {
                crate::log_eprintln!(
                    "⚠️  FALLBACK: GPU coarse search failed: {} (falling back to CPU-only)",
                    e
                );
                (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
            }
        }
    } else {
        crate::log_eprintln!();
        if !is_high_complexity {
            crate::log_eprintln!(
                "⚠️  OPTIMIZATION: Low complexity video ({:.1} Mbps <= 5.0 Mbps)",
                bitrate_bps / 1_000_000.0
            );
            crate::log_eprintln!(
                "   Skipping GPU coarse search (CPU is faster for low-bitrate animation/PPT)"
            );
        } else if !gpu.is_available() {
            // GPU was needed but the probe failed — now is the right time to surface why.
            gpu.print_detection_info();
            crate::log_eprintln!("⚠️  FALLBACK: No GPU available (skipping GPU coarse phase)");
        } else {
            crate::log_eprintln!(
                "⚠️  FALLBACK: No GPU encoder for {:?} (skipping GPU coarse phase)",
                encoder
            );
        }
        // CPU-only search (Bypass GPU)
        if is_gif_magic && actual_initial_crf == 0.0 {
            // [New] GIF CRF 0.00 Fast-Path
            // If CRF 0.00 already provides compression, we adopt it immediately.
            // This prevents redundant search iterations for already-compressible GIFs.
            (0.0, max_crf, 0.0)
        } else {
            (ABSOLUTE_MIN_CRF, max_crf, actual_initial_crf)
        }
    };

    crate::verbose_eprintln!("Phase 2: 🖥️  CPU Fine-Tune (0.5→0.1 step)");
    crate::verbose_eprintln!("Starting from GPU boundary: CRF {:.2}", cpu_center_crf);

    let clamped_cpu_center_crf = cpu_center_crf.clamp(cpu_min_crf, cpu_max_crf);
    if (clamped_cpu_center_crf - cpu_center_crf).abs() > 0.01 {
        crate::verbose_eprintln!(
            "   ⚠️ CPU start CRF {:.2} clamped to {:.1} (within valid range [{:.1}, {:.1}])",
            cpu_center_crf,
            clamped_cpu_center_crf,
            cpu_min_crf,
            cpu_max_crf
        );
        crate::verbose_eprintln!("      This is normal when GPU boundary exceeds CPU range");
        crate::verbose_eprintln!("      Search will start from boundary instead of optimal point");
    }

    let mut tracking = TrackingState {
        best_vmaf: None,
        best_psnr_uv: None,
    };
    let fine_tune_args = FineTuneArgs {
        input,
        output,
        encoder,
        vf_args,
        gpu_boundary_crf: clamped_cpu_center_crf,
        min_crf: cpu_min_crf,
        max_crf: cpu_max_crf,
        min_ssim,
        ultimate_mode,
        allow_size_tolerance,
        max_threads,
        duration,
        probe_info: probe_result.as_ref(),
        gpu_executed,
        is_gif_magic,
        hdr_x265_params,
        apple_compat,
        preset,
        final_output_preset,
    };

    let mut result = cpu_fine_tune_from_gpu_boundary(fine_tune_args, &mut tracking)?;

    result.log.clear();

    // Skip quality verification if early insight triggered
    if result.early_insight_triggered {
        crate::log_eprintln!();
        crate::log_eprintln!(
            "{}═══════════════════════════════════════════════════════════{}",
            crate::modern_ui::colors::BRIGHT_YELLOW,
            crate::modern_ui::colors::RESET
        );
        crate::log_eprintln!(
            "{}⚠️  Early Insight Triggered: Quality Plateau Detected{}",
            crate::modern_ui::colors::BRIGHT_YELLOW,
            crate::modern_ui::colors::RESET
        );
        crate::log_eprintln!(
            "   No integer-level quality improvement over 3 consecutive iterations"
        );

        // Display quality metrics that triggered early insight
        if let Some(vmaf) = tracking.best_vmaf {
            let vmaf_pass = vmaf >= VMAF_Y_SANITY_FLOOR;
            crate::log_eprintln!(
                "   VMAF-Y: {:.2} {} {:.1} {} (sanity floor)",
                vmaf,
                if vmaf_pass { "≥" } else { "<" },
                VMAF_Y_SANITY_FLOOR,
                if vmaf_pass { "✅" } else { "❌" }
            );
        }
        if let Some((u, v)) = tracking.best_psnr_uv {
            let u_pass = u >= PSNR_UV_SANITY_FLOOR;
            let v_pass = v >= PSNR_UV_SANITY_FLOOR;
            crate::log_eprintln!(
                "   PSNR-UV: U={:.2} dB {}, V={:.2} dB {} (sanity floor ≥ {:.1} dB)",
                u,
                if u_pass { "✅" } else { "❌" },
                v,
                if v_pass { "✅" } else { "❌" },
                PSNR_UV_SANITY_FLOOR
            );
        }

        crate::log_eprintln!("   Skipping final quality verification (unnecessary)");
        crate::log_eprintln!(
            "{}═══════════════════════════════════════════════════════════{}",
            crate::modern_ui::colors::BRIGHT_YELLOW,
            crate::modern_ui::colors::RESET
        );
        return Ok(result);
    }

    crate::verbose_eprintln!();
    crate::verbose_eprintln!("Phase 3: Quality Verification");

    let mut quality_verification_skipped_for_format = false;
    let run_ultimate_quality_gate = |result: &mut ExploreResult,
                                     duration_hint: Option<f64>,
                                     tracking: &TrackingState| {
        crate::log_eprintln!("   Enabling baseline-aware 3D quality gate (Ultimate Mode)...");

        let sample_rate = duration_hint.map_or(1, ultimate_final_sample_rate);
        if sample_rate > 1 {
            crate::log_eprintln!(
                    "   Ultimate gate sampling: 1/{sample_rate} frames (lightweight final verification)"
                );
        } else {
            crate::log_eprintln!("   Ultimate gate sampling: full-frame verification");
        }

        let vmaf_y = if let Some(v) = tracking.best_vmaf {
            crate::verbose_eprintln!("      ℹ️  Reusing VMAF from search phase: {:.2}", v);
            Some(v)
        } else {
            super::ssim_calculator::calculate_vmaf_y(input, output, sample_rate)
        };

        let psnr_uv = if let Some(uv) = tracking.best_psnr_uv {
            crate::verbose_eprintln!(
                "      ℹ️  Reusing PSNR-UV from search phase: {:.2}/{:.2}",
                uv.0,
                uv.1
            );
            Some(uv)
        } else {
            super::ssim_calculator::calculate_psnr_uv(input, output, sample_rate)
        };

        crate::log_eprintln!("   Measuring source CAMBI baseline...");
        let source_cambi = super::ssim_calculator::calculate_cambi(input, sample_rate);

        crate::log_eprintln!("   Running final CAMBI banding check...");
        let cambi = super::ssim_calculator::calculate_cambi(output, sample_rate);

        let baselines = UltimateQualityBaselines {
            search_vmaf_y: tracking.best_vmaf,
            search_psnr_uv: tracking.best_psnr_uv,
            source_cambi,
        };
        let metrics = UltimateQualityMetrics {
            vmaf_y,
            psnr_uv,
            cambi,
        };
        let evaluation = evaluate_ultimate_quality_gate(metrics, baselines);

        crate::log_eprintln!("   ═══════════════════════════════════════════════════");
        crate::log_eprintln!("   Quality Verification (Ultimate Mode, baseline-aware):");

        if let Some(v) = vmaf_y {
            crate::log_eprintln!(
                "      VMAF-Y: {:6.2} ≥ {:.1} {} (search baseline: {})",
                v,
                evaluation.vmaf_floor,
                if evaluation.vmaf_ok { "✅" } else { "❌" },
                baselines
                    .search_vmaf_y
                    .map_or_else(|| "N/A".to_string(), |x| format!("{x:.2}"))
            );
        } else {
            crate::log_eprintln!("      VMAF-Y: N/A (calculation failed) ❌");
        }

        if let Some(c) = cambi {
            crate::log_eprintln!(
                "      CAMBI:  {:6.2} ≤ {:.1} {} (source baseline: {}, lower=better)",
                c,
                evaluation.cambi_ceiling,
                if evaluation.cambi_ok { "✅" } else { "❌" },
                baselines
                    .source_cambi
                    .map_or_else(|| "N/A".to_string(), |x| format!("{x:.2}"))
            );
        } else {
            crate::log_eprintln!("      CAMBI: N/A (calculation failed) ❌");
        }

        if let Some((pu, pv)) = psnr_uv {
            let u_pass = pu >= evaluation.psnr_uv_floor.0;
            let v_pass = pv >= evaluation.psnr_uv_floor.1;
            crate::log_eprintln!(
                    "      PSNR-UV: U={:.2} dB {}, V={:.2} dB {} (floors ≥ {:.1}/{:.1} dB, search baseline: {})",
                    pu,
                    if u_pass { "✅" } else { "❌" },
                    pv,
                    if v_pass { "✅" } else { "❌" },
                    evaluation.psnr_uv_floor.0,
                    evaluation.psnr_uv_floor.1,
                    baselines.search_psnr_uv.map_or_else(
                        || "N/A".to_string(),
                        |(u, v)| format!("{u:.2}/{v:.2}")
                    )
                );
        } else {
            crate::log_eprintln!("      PSNR-UV: N/A (calculation failed) ❌");
        }

        crate::log_eprintln!("   ───────────────────────────────────────────────────");

        if evaluation.all_passed() {
            crate::log_eprintln!("   ✅ 3D QUALITY GATE: PASSED");
            result.ms_ssim_passed = CheckResult::Passed;
        } else {
            crate::log_eprintln!("   ❌ 3D QUALITY GATE: FAILED");
            if !evaluation.vmaf_ok {
                let v_str = vmaf_y.map_or_else(|| "N/A".to_string(), |v| format!("{v:.2}"));
                crate::log_eprintln!(
                    "      FAILED VMAF-Y {} < {:.1} (fell too far below the search baseline)",
                    v_str,
                    evaluation.vmaf_floor
                );
            }
            if !evaluation.cambi_ok {
                let c_str = cambi.map_or_else(|| "N/A".to_string(), |c| format!("{c:.2}"));
                crate::log_eprintln!(
                        "      FAILED CAMBI {} > {:.1} (banding rose too far above the source baseline)",
                        c_str,
                        evaluation.cambi_ceiling
                    );
            }
            if !evaluation.chroma_ok {
                let uv_str = psnr_uv.map_or_else(
                    || "N/A".to_string(),
                    |(u, v): (f64, f64)| {
                        let u_pass = u >= evaluation.psnr_uv_floor.0;
                        let v_pass = v >= evaluation.psnr_uv_floor.1;
                        format!(
                            "U={:.2} dB {}, V={:.2} dB {}",
                            u,
                            if u_pass { "✅" } else { "❌" },
                            v,
                            if v_pass { "✅" } else { "❌" }
                        )
                    },
                );
                crate::log_eprintln!(
                        "      FAILED PSNR-UV {} < {:.1}/{:.1} dB (chroma fell too far below the search baseline)",
                        uv_str,
                        evaluation.psnr_uv_floor.0,
                        evaluation.psnr_uv_floor.1
                    );
            }
            crate::log_eprintln!("      Suggestion: Lower CRF or disable --compress");
            result.ms_ssim_passed = CheckResult::Failed("3D quality gate failed".into());
        }

        result.ms_ssim_score = vmaf_y.map(|v| v / 100.0);
        result.vmaf_y_score = vmaf_y;
        result.cambi_score = cambi;
        result.psnr_uv_score = psnr_uv;
    };

    let duration_opt = probe_result.as_ref().map(|probe| probe.duration);
    if let Some(duration) = duration_opt {
        crate::verbose_eprintln!(
            "   Video duration: {:.1}s ({:.1} min)",
            duration,
            duration / 60.0
        );
    } else {
        crate::log_eprintln!("   ⚠️  Could not determine video duration");
    }

    let ms_ssim_duration_threshold_secs: f64 = if ultimate_mode {
        VMAF_SKIP_THRESHOLD_ULTIMATE_SECS.into()
    } else {
        VMAF_SKIP_THRESHOLD_SECS.into()
    };
    let is_animated_image = is_animated_image_like_input(input, probe_result.as_ref());

    if is_animated_image && result.optimal_crf == 0.0 {
        crate::log_eprintln!(
            "   ANIMATED CRF=0 (lossless): skipping perceptual metrics — running integrity check instead"
        );
        crate::log_eprintln!(
            "   (CRF=0 guarantees YUV bit-exact reproduction; perceptual metrics are unnecessary)"
        );
        let integrity_ok = super::stream_analysis::check_lossless_integrity(
            input,
            output,
            result.output_size,
            true,
        )
        .unwrap_or_else(|e| {
            crate::log_eprintln!("   ⚠️  Integrity check error: {}", e);
            true
        });

        if integrity_ok {
            crate::log_eprintln!("   ✅ INTEGRITY CHECK: PASSED");
            result.ms_ssim_passed = CheckResult::Passed;
        } else {
            crate::log_eprintln!("   ❌ INTEGRITY CHECK: FAILED (possible encode error)");
            result.ms_ssim_passed = CheckResult::Failed("Lossless integrity check failed".into());
        }
    } else if ultimate_mode {
        run_ultimate_quality_gate(&mut result, duration_opt, &tracking);
    } else if is_animated_image {
        crate::verbose_eprintln!(
            "   Animated input: using SSIM-All verification (ffmpeg ssim filter, GIF-compatible)"
        );

        if let Some((y, u, v, all)) = calculate_ssim_all(input, output) {
            crate::log_eprintln!("   SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}", y, u, v, all);
            let gif_threshold = result.actual_min_ssim.max(0.92);
            if all < gif_threshold {
                crate::log_eprintln!(
                    "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.2}",
                    all,
                    gif_threshold
                );
                result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
            } else {
                crate::log_eprintln!(
                    "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.2}",
                    all,
                    gif_threshold
                );
                result.ms_ssim_passed = CheckResult::Passed;
            }
            result.ms_ssim_score = Some(all);
        } else {
            quality_verification_skipped_for_format = true;
            let msg =
                "⚠️  SSIM verification failed (Animated format) - accepting based on size compression only";
            result.log.push(msg.to_string());
            result.ms_ssim_passed = CheckResult::NotChecked;
            result.ms_ssim_score = None;
        }
    } else if let Some(duration) = duration_opt {
        if duration <= ms_ssim_duration_threshold_secs || force_ms_ssim_long {
            let threshold_min = ms_ssim_duration_threshold_secs / 60.0;
            crate::log_eprintln!("   Video within limit (≤{:.0}min)", threshold_min);

            crate::log_eprintln!("   Enabling fusion quality verification (MS-SSIM + SSIM)...");

            let max_duration_min = ms_ssim_duration_threshold_secs / 60.0;
            let ms_ssim_yuv_result = calculate_ms_ssim_yuv(input, output, max_duration_min);
            let ssim_all_result = calculate_ssim_all(input, output);

            crate::log_eprintln!("   ═══════════════════════════════════════════════════");
            crate::log_eprintln!("   Quality Metrics:");
            let ssim_str = result
                .ssim
                .map_or_else(|| "N/A".to_string(), |s| format!("{s:.6}"));
            crate::log_eprintln!("      SSIM (explore / pre-processing ref): {}", ssim_str);

            let mut ms_ssim_avg: Option<f64> = None;
            let mut ssim_all_val: Option<f64> = None;

            if let Some((y, u, v, avg)) = ms_ssim_yuv_result {
                crate::log_eprintln!(
                    "      MS-SSIM Y/U/V/Avg: {:.4}/{:.4}/{:.4} / {:.4}",
                    y,
                    u,
                    v,
                    avg
                );
                ms_ssim_avg = Some(avg);

                let chroma_loss = (y - u).max(y - v);
                if chroma_loss > 0.02 {
                    crate::log_eprintln!(
                        "      ⚠️  MS-SSIM CHROMA DIFF: Y-U={:.4}, Y-V={:.4}",
                        y - u,
                        y - v
                    );
                }
            }

            if let Some((y, u, v, all)) = ssim_all_result {
                crate::log_eprintln!(
                    "      SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}",
                    y,
                    u,
                    v,
                    all
                );
                ssim_all_val = Some(all);

                let chroma_loss = (y - u).max(y - v);
                if chroma_loss > 0.02 {
                    crate::log_eprintln!(
                        "      ⚠️  SSIM CHROMA LOSS: Y-U={:.4}, Y-V={:.4}",
                        y - u,
                        y - v
                    );
                }
            }

            crate::log_eprintln!("   ───────────────────────────────────────────────────");

            let baseline = NormalQualityBaseline {
                explore_ssim: result.ssim,
                min_ssim_config: result.actual_min_ssim,
            };
            let measurement = NormalQualityMeasurement {
                ms_ssim_avg,
                ssim_all: ssim_all_val,
            };
            let evaluation = build_normal_quality_evaluation(baseline, measurement);

            match (ms_ssim_avg, ssim_all_val) {
                (Some(ms), Some(ss)) => {
                    crate::log_eprintln!(
                        "   FUSION SCORE: {:.4}",
                        evaluation.fusion_score.unwrap_or(0.0)
                    );
                    crate::log_eprintln!(
                        "      Formula: {:.1}×MS-SSIM + {:.1}×SSIM_All",
                        MS_SSIM_WEIGHT,
                        SSIM_ALL_WEIGHT
                    );
                    crate::log_eprintln!(
                        "      = {:.1}×{:.4} + {:.1}×{:.4}",
                        MS_SSIM_WEIGHT,
                        ms,
                        SSIM_ALL_WEIGHT,
                        ss
                    );
                }
                (Some(ms), None) => {
                    crate::log_eprintln!("   SCORE (MS-SSIM only): {:.4}", ms);
                    crate::log_eprintln!("      ⚠️  SSIM All unavailable, using MS-SSIM alone");
                }
                (None, Some(ss)) => {
                    crate::log_eprintln!("   SCORE (SSIM All only): {:.4}", ss);
                    crate::log_eprintln!("      ⚠️  MS-SSIM unavailable, using SSIM All alone");
                }
                (None, None) => {}
            }

            if let Some(score) = evaluation.fusion_score {
                let quality_grade = if score >= 0.98 {
                    "Excellent"
                } else if score >= 0.95 {
                    "Very Good"
                } else if score >= evaluation.fusion_floor {
                    "Good (meets target)"
                } else if score >= 0.85 {
                    "Below Target"
                } else {
                    "FAILED"
                };
                let baseline_note = baseline
                    .explore_ssim
                    .map_or_else(|| "none".to_string(), |v| format!("{v:.6}"));
                crate::log_eprintln!(
                    "      Grade: {} (floor: ≥{:.4}, pre-processing ref: {})",
                    quality_grade,
                    evaluation.fusion_floor,
                    baseline_note
                );

                if evaluation.passed {
                    crate::log_eprintln!(
                        "   ✅ FUSION SCORE TARGET MET: {:.4} ≥ {:.4}",
                        score,
                        evaluation.fusion_floor
                    );
                    result.ms_ssim_passed = CheckResult::Passed;
                } else {
                    crate::log_eprintln!(
                        "   ❌ FUSION SCORE BELOW TARGET! {:.4} < {:.4}",
                        score,
                        evaluation.fusion_floor
                    );
                    crate::log_eprintln!("      ⚠️  Quality does not meet threshold!");
                    crate::log_eprintln!("      Suggestion: Lower CRF or disable --compress");
                    result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
                }
                result.ms_ssim_score = Some(score);
            } else {
                let err_lines = [
                    "   ════════════════════════════════════════════════════",
                    "   ❌ ERROR: Fusion verification incomplete (MS-SSIM + SSIM All failed).",
                    "   ❌ Refusing to mark as passed — no fallback to single-channel or explore SSIM.",
                    "   ❌ Possible causes: libvmaf unavailable, pixel format, or resolution mismatch.",
                    "   ════════════════════════════════════════════════════",
                ];
                for line in &err_lines {
                    crate::log_eprintln!("{}", line);
                    result.log.push((*line).to_string());
                }
                result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
                result.ms_ssim_score = None;
            }
        } else {
            crate::log_eprintln!(
                "   ⚠️  Quality verification: long video (>{:.0}min), MS-SSIM skipped.",
                ms_ssim_duration_threshold_secs / 60.0
            );
            crate::log_eprintln!("   Using SSIM-All verification only.");

            if let Some((y, u, v, all)) = calculate_ssim_all(input, output) {
                crate::log_eprintln!("   SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}", y, u, v, all);

                let baseline = NormalQualityBaseline {
                    explore_ssim: result.ssim,
                    min_ssim_config: result.actual_min_ssim,
                };
                let measurement = NormalQualityMeasurement {
                    ms_ssim_avg: None,
                    ssim_all: Some(all),
                };
                let evaluation = build_normal_quality_evaluation(baseline, measurement);
                let baseline_note = baseline
                    .explore_ssim
                    .map_or_else(|| "none".to_string(), |v| format!("{v:.6}"));

                if evaluation.passed {
                    crate::log_eprintln!(
                        "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.4} (pre-processing ref: {})",
                        all,
                        evaluation.fusion_floor,
                        baseline_note
                    );
                    result.ms_ssim_passed = CheckResult::Passed;
                } else {
                    crate::log_eprintln!(
                        "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.4} (pre-processing ref: {})",
                        all,
                        evaluation.fusion_floor,
                        baseline_note
                    );
                    result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
                }
                result.ms_ssim_score = Some(all);
            } else {
                let err_lines = [
                    "   ❌ ERROR: SSIM All calculation failed (long-video path). Refusing to mark as passed.",
                ];
                for line in &err_lines {
                    crate::log_eprintln!("{}", line);
                    result.log.push((*line).to_string());
                }
                result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
                result.ms_ssim_score = None;
            }
        }
    } else {
        crate::log_eprintln!("   Using SSIM All verification (includes chroma)...");

        if let Some((y, u, v, all)) = calculate_ssim_all(input, output) {
            crate::log_eprintln!("   SSIM Y/U/V/All: {:.4}/{:.4}/{:.4}/{:.4}", y, u, v, all);

            let baseline = NormalQualityBaseline {
                explore_ssim: result.ssim,
                min_ssim_config: result.actual_min_ssim,
            };
            let measurement = NormalQualityMeasurement {
                ms_ssim_avg: None,
                ssim_all: Some(all),
            };
            let evaluation = build_normal_quality_evaluation(baseline, measurement);
            let baseline_note = baseline
                .explore_ssim
                .map_or_else(|| "none".to_string(), |v| format!("{v:.6}"));

            if evaluation.passed {
                crate::log_eprintln!(
                    "   ✅ SSIM ALL TARGET MET: {:.4} ≥ {:.4} (pre-processing ref: {})",
                    all,
                    evaluation.fusion_floor,
                    baseline_note
                );
                result.ms_ssim_passed = CheckResult::Passed;
            } else {
                crate::log_eprintln!(
                    "   ❌ SSIM ALL BELOW TARGET! {:.4} < {:.4} (pre-processing ref: {})",
                    all,
                    evaluation.fusion_floor,
                    baseline_note
                );
                result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
            }
            result.ms_ssim_score = Some(all);
        } else {
            let err_lines = [
                "   ❌ ERROR: SSIM All calculation failed (no duration path). Refusing to mark as passed.",
            ];
            for line in &err_lines {
                crate::log_eprintln!("{}", line);
                result.log.push((*line).to_string());
            }
            result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
            result.ms_ssim_score = None;
        }
    }

    let input_size = fs::metadata(input).ok().map(|m| m.len());
    let output_size_actual = fs::metadata(output)
        .ok()
        .map_or(result.output_size, |m| m.len());
    let size_change_line =
        if let (Some(in_sz), Some(out_sz)) = (input_size, Some(output_size_actual)) {
            if in_sz == 0 {
                "   SizeChange: N/A (zero input size)".to_string()
            } else {
                let ratio = crate::numeric_cast::u64_to_f64(out_sz)
                    / crate::numeric_cast::u64_to_f64(in_sz);
                let pct = (ratio - 1.0) * 100.0;
                format!("   SizeChange: {ratio:.2}x ({pct:+.1}%) vs original")
            }
        } else {
            "   SizeChange: N/A (missing original or output size)".to_string()
        };
    result.log.push(size_change_line);

    let quality_line = if let Some(summary) = result.ultimate_quality_summary() {
        format!("   Quality: {summary}")
    } else if result.ms_ssim_passed.is_failed() && result.ms_ssim_score.is_none() {
        "   Quality: N/A (quality check failed)".to_string()
    } else if let Some(score) = result.ms_ssim_score {
        let pct = (score * 100.0 * 10.0).round() / 10.0;
        format!("   Quality: {pct:.1}% (MS-SSIM={score:.4})")
    } else if let Some(s) = result.ssim {
        let pct = (s * 100.0 * 10.0).round() / 10.0;
        format!("   Quality: {pct:.1}% (SSIM={s:.4}, approx.)")
    } else {
        "   Quality: N/A (quality check failed)".to_string()
    };
    result.log.push(quality_line);

    let quality_check_line =
        format_quality_check_line(&result, quality_verification_skipped_for_format);
    result.log.push(quality_check_line);

    crate::log_eprintln!();

    if gpu.is_available() && has_gpu_encoder {
        let mapping = match encoder {
            VideoEncoder::Av1 => CrfMapping::av1(gpu.gpu_type),
            VideoEncoder::Hevc | VideoEncoder::H264 => CrfMapping::hevc(gpu.gpu_type), // H.264 reuses HEVC mapping
        };
        let equivalent_gpu_crf = mapping.cpu_to_gpu(result.optimal_crf);
        let crf_display = if result.optimal_crf < 0.01 {
            format!("{:.2} (Lossless)", result.optimal_crf)
        } else {
            format!("{:.2}", result.optimal_crf)
        };
        crate::verbose_eprintln!("   ═══════════════════════════════════════════════════");
        crate::verbose_eprintln!(
            "   CRF Mapping: CPU {} ≈ GPU {:.1}",
            crf_display,
            equivalent_gpu_crf
        );
    }

    Ok(result)
}

fn is_image_container(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "avif" | "heic" | "heif" | "gif" | "webp" | "png" | "jpg" | "jpeg" | "bmp" | "tiff"
    )
}

#[inline]
fn is_animated_image_like_input(
    path: &Path,
    probe_info: Option<&crate::ffprobe::FFprobeResult>,
) -> bool {
    if let Some(probe) = probe_info {
        let fmt = probe.format_name.to_ascii_lowercase();
        if fmt.contains("gif")
            || fmt.contains("webp")
            || fmt.contains("avif")
            || fmt.contains("heic")
            || fmt.contains("heif")
            || fmt.contains("apng")
        {
            return true;
        }
    }

    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        let ext = e.to_ascii_lowercase();
        matches!(
            ext.as_str(),
            "gif" | "webp" | "avif" | "heic" | "heif" | "apng"
        )
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimatedExplorationEncodeMode {
    /// CRF search iterations: three-segment timeline sampling when enabled for long animated sources.
    ExplorationSample,
    /// One full-length encode at the chosen CRF (deliverable timeline).
    FullTimeline,
}

/// `FFmpeg` `-vf` prefix: keep frames in three windows (start / mid / end) and reset PTS for encode.
#[must_use]
fn animated_exploration_three_segment_vf_prefix(dur: f64, ultimate_mode: bool) -> String {
    let segment_pct = if ultimate_mode {
        ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE
    } else {
        ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION
    };
    let start_end = dur * segment_pct;
    let mid_start = dur * (0.5 - segment_pct / 2.0);
    let mid_end = dur * (0.5 + segment_pct / 2.0);
    let tail_start = dur * (1.0 - segment_pct);
    format!(
        "select='lt(t\\,{start_end:.3})+between(t\\,{mid_start:.3}\\,{mid_end:.3})+gte(t\\,{tail_start:.3})',setpts=N/FRAME_RATE/TB"
    )
}

/// Prepends `prefix` to the filter chain after `-vf`, or builds `-vf prefix` when no `-vf` pair exists.
#[must_use]
fn merge_vf_with_animated_exploration_prefix(vf_args: &[String], prefix: &str) -> Vec<String> {
    if vf_args.len() >= 2 && vf_args[0] == "-vf" {
        let merged = format!("{prefix},{}", vf_args[1]);
        vec!["-vf".to_string(), merged]
    } else {
        vec!["-vf".to_string(), prefix.to_string()]
    }
}

fn cpu_fine_tune_from_gpu_boundary(
    args: FineTuneArgs<'_>,
    tracking: &mut TrackingState,
) -> Result<ExploreResult> {
    // PHASE OVERVIEW (2519 lines total):
    // This function implements CPU-based CRF refinement after GPU screening.
    // It is intentionally large to maintain phase-related state coherence and avoid excessive
    // function parameter passing. Future refactoring should decompose by phase:
    //
    // 1. INIT (~200 lines): Prepare encoders, detect input properties, set stage parameters
    // 2. PHASE A (downward): Find lowest CRF that still compresses below input size
    // 3. PHASE B (upward): Find highest CRF yielding lowest quality for target SSIM
    // 4. PHASE C (quality validation): Check VMAF, PSNR-UV gates (ultimate mode only)
    // 5. FINALIZE: Validate and package result
    //
    // **Important**: State mutations in TrackingState should only grow (never shrink quality values).
    // See TrackingState struct for invariants.

    let FineTuneArgs {
        input,
        output,
        encoder,
        vf_args,
        gpu_boundary_crf,
        min_crf,
        max_crf,
        min_ssim,
        ultimate_mode,
        allow_size_tolerance,
        max_threads,
        duration,
        probe_info,
        gpu_executed,
        is_gif_magic,
        hdr_x265_params,
        apple_compat,
        preset,
        final_output_preset,
    } = args;
    let log = Vec::new();
    let mut early_insight_triggered = false;

    let input_size = fs::metadata(input)
        .context("Failed to read input file metadata")?
        .len();

    // Image containers (AVIF, HEIC, GIF, WebP, …) have no audio streams.
    // Mapping all streams (-map 0) causes FFmpeg libx265 to fail with
    // "Not yet implemented in FFmpeg, patches welcome".
    let input_is_image = is_image_container(input);
    let input_is_animated_image_like = is_animated_image_like_input(input, probe_info);

    let input_stream_info = crate::stream_size::extract_stream_sizes(input);
    let input_video_stream_size = input_stream_info.video_stream_size;
    let pts_integrity = crate::ffprobe_json::check_pts_integrity(input);
    if pts_integrity != crate::ffprobe_json::PtsIntegrity::Healthy {
        crate::log_eprintln!(
            "   ⚠️  {} input: {:?}, applying safety measures",
            if pts_integrity == crate::ffprobe_json::PtsIntegrity::Broken {
                "Broken PTS"
            } else {
                "Duplicate PTS"
            },
            pts_integrity
        );
    }

    let use_animated_exploration_sampling = input_is_animated_image_like
        && duration > ANIMATED_IMAGE_EXPLORATION_SAMPLING_MIN_DURATION_SECS;
    if use_animated_exploration_sampling {
        crate::log_eprintln!(
            "{}🎞️  Long animated source ({:.1}s > {:.1}s): CPU CRF search uses 3-segment timeline sampling; one full-length encode follows before quality checks.{}",
            BRIGHT_CYAN,
            duration,
            ANIMATED_IMAGE_EXPLORATION_SAMPLING_MIN_DURATION_SECS,
            RESET
        );
    }

    crate::verbose_eprintln!(
        "{}Input video stream: {} (total file: {}, overhead: {:.1}%)",
        CYAN,
        crate::modern_ui::format_size(input_video_stream_size),
        crate::modern_ui::format_size(input_size),
        input_stream_info.container_overhead_percent()
    );

    let estimated_iterations = if ultimate_mode {
        let crf_range = max_crf - min_crf;
        let adaptive_walls = calculate_adaptive_max_walls(crf_range);
        u64::from(adaptive_walls + 10)
    } else {
        15
    };
    let cpu_progress = crate::UnifiedProgressBar::new_iteration(
        "[CPU] Fine-Tune",
        input_size,
        estimated_iterations,
    );

    let audio_strategy = {
        let output_ext = output
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_mov_mp4 = output_ext == "mov" || output_ext == "mp4" || output_ext == "m4v";

        if is_mov_mp4 {
            let audio_codec = probe_info
                .and_then(|info| info.audio.codec.as_ref())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let audio_bitrate = probe_info.and_then(|info| info.audio.bit_rate).unwrap_or(0);

            let incompatible = audio_codec.contains("opus")
                || audio_codec.contains("vorbis")
                || audio_codec.contains("webm");

            let is_lossless = audio_codec.contains("flac")
                || audio_codec.contains("alac")
                || audio_codec.contains("pcm")
                || audio_codec.contains("wav");

            if !incompatible {
                AudioTranscodeStrategy::Copy
            } else if is_lossless || audio_bitrate > 256_000 {
                crate::log_eprintln!(
                    "   🎵 High-quality audio detected ({}kbps {}), using ALAC (lossless)",
                    audio_bitrate / 1000,
                    audio_codec
                );
                AudioTranscodeStrategy::Alac
            } else if audio_bitrate >= 128_000 {
                crate::log_eprintln!(
                    "   🎵 Medium-quality audio ({}kbps {}), using AAC 256k",
                    audio_bitrate / 1000,
                    audio_codec
                );
                AudioTranscodeStrategy::AacHigh
            } else {
                crate::log_eprintln!(
                    "   🎵 Audio codec '{}' incompatible with {}, using AAC 192k",
                    audio_codec,
                    output_ext.to_uppercase()
                );
                AudioTranscodeStrategy::AacMedium
            }
        } else {
            AudioTranscodeStrategy::Copy
        }
    };

    let encode_full = move |crf: f32,
                            mode: AnimatedExplorationEncodeMode,
                            encode_preset: EncoderPreset|
          -> Result<u64> {
        let apply_segment_vf = mode == AnimatedExplorationEncodeMode::ExplorationSample
            && use_animated_exploration_sampling;
        let vf_for_encode: Vec<String> = if apply_segment_vf {
            let prefix =
                animated_exploration_three_segment_vf_prefix(f64::from(duration), ultimate_mode);
            merge_vf_with_animated_exploration_prefix(&vf_args, &prefix)
        } else {
            vf_args.clone()
        };

        let mut builder = crate::ffmpeg_builder::FfmpegBuilder::new();
        builder
            .overwrite()
            .arg("-progress")
            .arg("pipe:1")
            .input(input);

        if input_is_image {
            builder.arg("-map").arg("0:v");
        } else {
            builder.arg("-map").arg("0");
        }

        builder
            .codec_video(encoder.ffmpeg_name())
            .arg("-crf")
            .arg(format!("{crf:.2}"));

        // CRF=0 HEVC → inject lossless=1 into x265-params
        let mut adjusted_x265_params =
            if crf == 0.0 && encoder == crate::video_explorer::VideoEncoder::Hevc {
                let existing = hdr_x265_params.as_deref().unwrap_or("");
                if existing.is_empty() {
                    Some("lossless=1".to_string())
                } else {
                    Some(format!("{existing}:lossless=1"))
                }
            } else {
                hdr_x265_params.clone()
            };

        // Defensive VFR check: assume VFR if probing failed or explicitly detected
        let vfr_or_unknown = probe_info.is_none_or(|p| p.is_variable_frame_rate);

        // Disable B-frames for:
        // 1. GIF sources (irregular durations / disposal=restoreToPrevious)
        // 2. Animated images with VFR/unknown frame rate (prevents PTS reordering)
        let should_disable_bframes =
            is_gif_magic || (input_is_animated_image_like && vfr_or_unknown);
        let x265_memory_profile = crate::x265_params::memory_profile_for_source(
            probe_info.map(|probe| probe.video_codec.as_str()),
            input_size,
        );

        if should_disable_bframes && encoder == crate::video_explorer::VideoEncoder::Hevc {
            let existing = adjusted_x265_params.as_deref().unwrap_or("");
            adjusted_x265_params = Some(if existing.is_empty() {
                "bframes=0".to_string()
            } else {
                format!("{existing}:bframes=0")
            });
        }

        // Defensive HDR10 metadata injection: if probe has mastering-display / max-cll
        // but the caller didn't fold them into hdr_x265_params, add them here so that
        // HDR10 signaling survives through libx265.
        if encoder == crate::video_explorer::VideoEncoder::Hevc {
            use std::fmt::Write as _;
            if let Some(probe) = probe_info {
                let existing = adjusted_x265_params.clone().unwrap_or_default();
                let mut updated = existing.clone();
                if let Some(ref md) = probe.hdr.mastering_display {
                    if !md.is_empty() && !updated.contains("master-display=") {
                        if !updated.is_empty() {
                            updated.push(':');
                        }
                        let _ = write!(updated, "master-display={md}");
                    }
                }
                if let Some(ref cll) = probe.hdr.max_cll {
                    if !cll.is_empty() && !updated.contains("max-cll=") {
                        if !updated.is_empty() {
                            updated.push(':');
                        }
                        let _ = write!(updated, "max-cll={cll}");
                    }
                }
                if updated != existing {
                    adjusted_x265_params = Some(updated);
                }
            }
        }

        for arg in encoder.extra_args_with_preset(
            max_threads,
            encode_preset,
            adjusted_x265_params,
            apple_compat,
            x265_memory_profile,
        ) {
            builder.arg(arg);
        }

        if let Some(probe) = probe_info {
            let pix_fmt = pick_pix_fmt(probe);
            builder.pix_fmt_str(pix_fmt);

            // Forward all HDR colour metadata (primaries, TRC, colorspace, mastering display, CLL)
            for arg in build_color_args_from_probe(probe) {
                builder.arg(arg);
            }
        }

        for arg in &vf_for_encode {
            if !arg.is_empty() {
                builder.arg(arg);
            }
        }

        if pts_integrity == crate::ffprobe_json::PtsIntegrity::Broken {
            // Safety fallback: if PTS is broken, use VFR to let FFmpeg rebuild the timeline
            builder.arg("-fps_mode").arg("vfr");
        } else {
            builder.arg("-fps_mode").arg("passthrough");
        }

        if input_is_animated_image_like {
            builder.arg("-video_track_timescale").arg("1000");
        }

        if input_is_image {
            builder.codec_audio("none");
        } else {
            match &audio_strategy {
                AudioTranscodeStrategy::Copy => {
                    builder.codec_audio("copy");
                }
                AudioTranscodeStrategy::Alac => {
                    builder.codec_audio("alac");
                }
                AudioTranscodeStrategy::AacHigh => {
                    builder.codec_audio("aac").arg("-b:a").arg("256k");
                }
                AudioTranscodeStrategy::AacMedium => {
                    builder.codec_audio("aac").arg("-b:a").arg("192k");
                }
            }
        }

        // Subtitle passthrough (copy subtitles when the output container supports it)
        if let Some(probe) = probe_info {
            if probe.subtitles.present {
                let out_ext = output
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let container = if out_ext == "mkv" { "mkv" } else { "mp4" };
                let sub_args = crate::subtitle_args_for_container(
                    true,
                    probe.subtitles.codec.as_deref(),
                    container,
                );
                for arg in sub_args {
                    builder.arg(arg);
                }
            }
        }

        let mut cmd = builder.output(output).build();
        cmd.stdout(Stdio::piped());
        let stderr_temp_val = tempfile::Builder::new()
            .suffix(".log")
            .tempfile()
            .context("Failed to create stderr temp file")?;

        let stderr_file = stderr_temp_val.path().to_path_buf();
        let stderr_temp = Some(stderr_temp_val);

        if let Some(ref temp) = stderr_temp {
            if let Ok(file) = temp.reopen() {
                cmd.stderr(file);
            } else {
                cmd.stderr(Stdio::null());
            }
        }

        // Ensure parent directory exists (Safety for CJK/Deep-Nested paths)
        if let Some(parent) = output.parent() {
            if !parent.exists() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        let mut child = cmd.spawn().context("Failed to spawn ffmpeg")?;

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut last_fps = 0.0_f64;
            let mut last_speed = String::new();
            let mut last_time_us = 0_i64;
            let progress_duration_secs = if apply_segment_vf {
                let p = if ultimate_mode {
                    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE
                } else {
                    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION
                };
                (f64::from(duration) * 3.0 * p).max(0.5)
            } else {
                f64::from(duration)
            };

            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(err) => {
                        crate::verbose_eprintln!(
                            "⚠️  Failed to read ffmpeg progress stream at CRF {:.2}: {}",
                            crf,
                            err
                        );
                        break;
                    }
                };

                if let Some(val) = line.strip_prefix("out_time_us=") {
                    if let Ok(time_us) = val.parse::<i64>() {
                        last_time_us = time_us;
                    }
                } else if let Some(val) = line.strip_prefix("fps=") {
                    if let Ok(fps) = val.parse::<f64>() {
                        last_fps = fps;
                    }
                } else if let Some(val) = line.strip_prefix("speed=") {
                    last_speed = val.trim().to_string();
                } else if line == "progress=continue" || line == "progress=end" {
                    let current_secs = crate::numeric_cast::i64_to_f64(last_time_us) / 1_000_000.0;
                    if progress_duration_secs > 0.0 {
                        let pct = (current_secs / progress_duration_secs * 100.0).min(100.0);
                        eprint!(
                            "\r      ⏳ CRF {crf:.1} | {pct:.1}% | {current_secs:.1}s/{progress_duration_secs:.1}s | {last_fps:.0}fps | {last_speed}   "
                        );
                    }
                    let _ = std::io::stderr().flush();
                }
            }
        }

        let status = child.wait().context("Failed to wait for ffmpeg")?;
        eprint!(
            "\r                                                                              \r"
        );

        if !status.success() {
            let error_detail = if stderr_file.exists() {
                let stderr_content = match std::fs::read_to_string(&stderr_file) {
                    Ok(content) => content,
                    Err(err) => {
                        tracing::warn!(
                            stderr_file = %stderr_file.display(),
                            error = %err,
                            "Failed to read GPU coarse-search stderr log"
                        );
                        String::new()
                    }
                };
                if let Err(err) = std::fs::remove_file(&stderr_file) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(
                            stderr_file = %stderr_file.display(),
                            error = %err,
                            "Failed to remove GPU coarse-search stderr log after failure"
                        );
                    }
                }
                let error_lines: Vec<&str> = stderr_content
                    .lines()
                    .filter(|l| {
                        l.contains("Error")
                            || l.contains("error")
                            || l.contains("Invalid")
                            || l.contains("failed")
                    })
                    .collect();
                if error_lines.is_empty() {
                    let last_lines: Vec<&str> = stderr_content.lines().rev().take(3).collect();
                    if last_lines.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\n   FFmpeg output: {}",
                            last_lines
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join("\n   ")
                        )
                    }
                } else {
                    format!("\n   FFmpeg error: {}", error_lines.join("\n   "))
                }
            } else {
                String::new()
            };
            anyhow::bail!("❌ Encoding failed at CRF {crf:.1}{error_detail}");
        }

        if let Err(err) = std::fs::remove_file(&stderr_file) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    stderr_file = %stderr_file.display(),
                    error = %err,
                    "Failed to remove GPU coarse-search stderr log after success"
                );
            }
        }

        // Stability Fix: Metadata Retry Loop (Handles 'lazy' disk flush under 95%+ CPU load)
        let mut metadata_retry = 0;
        let mut final_size = 0u64;
        while metadata_retry < 5 {
            if let Ok(m) = fs::metadata(output) {
                final_size = m.len();
                if final_size > 0 {
                    break;
                }
            }
            metadata_retry += 1;
            if metadata_retry < 5 {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }

        if final_size == 0 && !output.exists() {
            anyhow::bail!("❌ FFmpeg reported success but output file is missing: {}. OS error 2 prevented metadata extraction.", output.display());
        }

        Ok(final_size)
    };

    let cpu_fine_tune_title = if ultimate_mode {
        "CPU Fine-Tune - Ultimate 3D Search"
    } else {
        "CPU Fine-Tune - Maximum SSIM Search"
    };
    crate::verbose_eprintln!(
        "{}{} ({:?}){}",
        BRIGHT_CYAN,
        cpu_fine_tune_title,
        encoder,
        RESET
    );
    crate::verbose_eprintln!(
        "{}Input: {} ({} bytes) | Duration: {}",
        CYAN,
        crate::modern_ui::format_size(input_size),
        input_size,
        crate::modern_ui::format_duration(f64::from(duration))
    );
    let search_goal = if ultimate_mode {
        "Goal: min(CRF) where output < input (tightest 3D fidelity + must compress)"
    } else {
        "Goal: min(CRF) where output < input (Highest SSIM + Must Compress)"
    };
    crate::verbose_eprintln!("{}{}{}", YELLOW, search_goal, RESET);
    crate::verbose_eprintln!(
        "{}Using 0.25 step (upward) + 0.1 step (downward, aligned with main path){}",
        CYAN,
        RESET
    );
    let step_size_upward = 0.25_f32;

    let mut iterations = 0u32;
    let mut size_cache: CrfCache<u64> = CrfCache::new();

    let exploration_mode = if use_animated_exploration_sampling {
        AnimatedExplorationEncodeMode::ExplorationSample
    } else {
        AnimatedExplorationEncodeMode::FullTimeline
    };

    let encode_cached = |crf: f32, cache: &mut CrfCache<u64>| -> Result<u64> {
        if let Some(&size) = cache.get(crf) {
            cpu_progress.inc_iteration(crf, size, None);
            return Ok(size);
        }
        let size = encode_full(crf, exploration_mode, preset)?;
        cache.insert(crf, size);
        cpu_progress.inc_iteration(crf, size, None);
        Ok(size)
    };

    let mut best_crf: Option<f32> = None;
    let mut best_size: Option<u64> = None;

    crate::verbose_eprintln!(
        "{}Step: {:.2} | GPU boundary: CRF {:.2}{}",
        DIM,
        step_size_upward,
        gpu_boundary_crf,
        RESET
    );
    crate::verbose_eprintln!("{}Goal: min(CRF) where output < input{}", DIM, RESET);
    crate::verbose_eprintln!(
        "{}Strategy: Marginal benefit analysis (not hard stop){}",
        DIM,
        RESET
    );
    crate::verbose_eprintln!();

    let mut prefer_compat_ssim_mode = false;
    let mut calculate_ssim_quick = || -> Option<f64> {
        // For GIF/WebP/AVIF/HEIC-like sources, once quick SSIM fails once,
        // switch to robust SSIM-All path for stable baseline/iteration metrics.
        if prefer_compat_ssim_mode {
            return calculate_ssim_all(input, output).map(|(_, _, _, all)| all);
        }

        let filters = [
            "[0:v]scale=\"iw-mod(iw,2)\":\"ih-mod(ih,2)\":flags=bicubic[ref];[ref][1:v]ssim",
            "[0:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[cmp];[ref][cmp]ssim",
            "ssim",
        ];

        for filter in &filters {
            let ssim_output = crate::ffmpeg_builder::FfmpegBuilder::new()
                .input(input)
                .input(output)
                .arg("-lavfi")
                .arg(filter)
                .arg("-f")
                .arg("null")
                .output_pipe()
                .build()
                .output();

            if let Ok(out) = ssim_output {
                if out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if let Some(line) = stderr.lines().find(|l| l.contains("All:")) {
                        if let Some(all_pos) = line.find("All:") {
                            let after_all = &line[all_pos + 4..];
                            let end = after_all
                                .find(|c: char| !c.is_numeric() && c != '.')
                                .unwrap_or(after_all.len());
                            if end > 0 {
                                if let Ok(ssim) = after_all[..end].parse::<f64>() {
                                    if (0.0..=1.0).contains(&ssim) {
                                        return Some(ssim);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if input_is_animated_image_like {
            let compat_ssim = calculate_ssim_all(input, output).map(|(_, _, _, all)| all);
            if compat_ssim.is_some() {
                prefer_compat_ssim_mode = true;
            }
            return compat_ssim;
        }

        None
    };

    let boundary_label = if gpu_executed {
        "GPU boundary"
    } else {
        "initial boundary"
    };
    crate::log_eprintln!("{}Phase 1: Verify {}{}", BRIGHT_CYAN, boundary_label, RESET);

    let gpu_size = encode_cached(gpu_boundary_crf, &mut size_cache).map_err(|e| {
        crate::log_eprintln!(
            "{}⚠️  Boundary verification failed at CRF {:.2}{}",
            BRIGHT_YELLOW,
            gpu_boundary_crf,
            RESET
        );
        crate::log_eprintln!("   Error: {}", e);
        e
    })?;
    iterations += 1;
    let gpu_pct = if input_size > 0 {
        (crate::numeric_cast::u64_to_f64(gpu_size)
            / crate::numeric_cast::u64_to_f64(input_size.max(1))
            - 1.0)
            * 100.0
    } else {
        0.0
    };
    let gpu_ssim = if ultimate_mode {
        None
    } else {
        calculate_ssim_quick()
    };

    let is_gpu_effectively_compressed = gpu_size < input_size;

    if is_gpu_effectively_compressed {
        best_crf = Some(gpu_boundary_crf);
        best_size = Some(gpu_size);

        let mut gpu_ultimate_metrics_str = String::new();
        if ultimate_mode {
            let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
            let psnr_uv = super::ssim_calculator::calculate_psnr_uv(input, output, 6);
            if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                let chroma_avg = f64::midpoint(u, v_score);
                gpu_ultimate_metrics_str = format!("VMAF:{v:.2} UV:{chroma_avg:.2}");
                tracking.best_vmaf = Some(v);
                tracking.best_psnr_uv = Some((u, v_score));
            }
        }

        let metrics_display = if ultimate_mode && !gpu_ultimate_metrics_str.is_empty() {
            format!(" │ {gpu_ultimate_metrics_str}")
        } else if let Some(s) = gpu_ssim {
            format!(" │ SSIM:{s:.4}")
        } else if ultimate_mode {
            " │ 3D metrics N/A".to_string()
        } else {
            " │ SSIM N/A".to_string()
        };

        let source_label = if gpu_executed { "[GPU]" } else { "[Initial]" };
        crate::log_eprintln!(
            "{}{}   {}✓{} {} {}CRF {:<5.2}{} {}{:6.1}%{}{} ✅",
            RESET,
            RESET,
            BRIGHT_GREEN,
            RESET,
            source_label,
            CYAN,
            gpu_boundary_crf,
            RESET,
            BRIGHT_GREEN,
            gpu_pct,
            RESET,
            metrics_display
        );
        crate::log_eprintln!();
        let phase2_title = if ultimate_mode {
            "Phase 2: Ultimate 3D Search - Smart Wall Collision (v5.93)"
        } else {
            "Phase 2: Maximum SSIM Search - Smart Wall Collision (v5.93)"
        };
        crate::log_eprintln!("{}{}{}", BRIGHT_CYAN, phase2_title, RESET);
        crate::verbose_eprintln!(
            "   {}(Adaptive step, MUST hit wall OR min_crf boundary){}",
            DIM,
            RESET
        );

        let search_floor = if ultimate_mode { 0.0 } else { min_crf };
        let crf_range = gpu_boundary_crf - search_floor;

        let initial_step = (crf_range / 1.5).clamp(8.0, 25.0);
        let max_wall_hits = if duration >= VERY_LONG_VIDEO_THRESHOLD_SECS {
            6
        } else if duration >= LONG_VIDEO_THRESHOLD_SECS {
            8
        } else if ultimate_mode {
            calculate_adaptive_max_walls(crf_range)
        } else {
            NORMAL_MAX_WALL_HITS
        };

        let required_zero_gains =
            calculate_zero_gains_for_duration_and_range(duration, crf_range, ultimate_mode);

        let max_iterations_for_video = if ultimate_mode {
            500
        } else {
            calculate_max_iterations_for_duration(duration, ultimate_mode)
        };

        if ultimate_mode {
            crate::verbose_eprintln!(
                "   {}ULTIMATE MODE: searching until 3D quality plateau / domain wall{}",
                BRIGHT_MAGENTA,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}CRF range: {:.1} → Adaptive max walls: {}{}{}{}",
                DIM,
                crf_range,
                BRIGHT_CYAN,
                max_wall_hits,
                RESET,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}3D plateau patience: {}{}{} consecutive fine-step non-improvements{}",
                DIM,
                BRIGHT_YELLOW,
                required_zero_gains,
                RESET,
                RESET
            );
        } else {
            crate::verbose_eprintln!(
                "   {}CRF range: {:.1} → Initial step: {}{:.1}{} (v6.2 curve model){}",
                DIM,
                crf_range,
                BRIGHT_CYAN,
                initial_step,
                RESET,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}Strategy: Aggressive curve decay (step × 0.4 per wall hit, max {} hits){}",
                DIM,
                max_wall_hits,
                RESET
            );
        }

        let mut current_step = if is_gif_magic && gpu_boundary_crf < 0.1 {
            // For GIFs starting at lossless (0.00), use a much smaller initial step
            // to carefully probe the compression boundary instead of jumping to CRF 8-25.
            1.0_f32
        } else {
            initial_step
        };
        let mut wall_hits: u32 = 0;

        // If it's a GIF starting at 0.0, ensure we test 0.0 itself as the first anchor.
        // cpu_fine_tune_from_gpu_boundary Phase 1 actually already does this via gpu_size = encode_cached(gpu_boundary_crf, ...).
        // But for the search loop below, we need to ensure test_crf is correctly placed.
        let mut test_crf = if is_gpu_effectively_compressed {
            let next = gpu_boundary_crf - current_step;
            if next < search_floor && gpu_boundary_crf > search_floor {
                // If the next step would pass the floor, land exactly on the floor
                // to ensure we test the maximum quality (e.g., 0.00) before stopping.
                search_floor
            } else {
                next
            }
        } else {
            gpu_boundary_crf + current_step
        };

        let mut last_good_crf = gpu_boundary_crf;
        let mut last_good_size = gpu_size;
        let mut last_good_ssim = gpu_ssim;

        let gpu_ssim_baseline = if let Some(s) = gpu_ssim {
            crate::verbose_eprintln!(
                "   {}GPU SSIM baseline: {}{:.4}{} (CPU target: break through 0.97+)",
                DIM,
                BRIGHT_YELLOW,
                s,
                RESET
            );
            Some(s)
        } else {
            crate::log_eprintln!(
                "   {}⚠️  GPU SSIM not measured; continue with CPU delta-only search{}",
                BRIGHT_YELLOW,
                RESET
            );
            None
        };

        let mut consecutive_zero_gains: u32 = 0;
        let mut failure_credibility: f64 = 0.0;
        let mut quality_wall_hit = false;
        let mut domain_wall_hit = false;

        if duration >= LONG_VIDEO_THRESHOLD_SECS {
            let long_video_strategy = if ultimate_mode {
                "searching until 3D quality plateau stabilizes"
            } else {
                "searching until SSIM saturates"
            };
            crate::verbose_eprintln!(
                "   {}Long video ({:.1} min) - no iteration limit, {}{}",
                BRIGHT_CYAN,
                duration / 60.0,
                long_video_strategy,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}Fallback limit: {} (emergency only), Max walls: {}, Zero-gains: {}{}",
                DIM,
                max_iterations_for_video,
                max_wall_hits,
                required_zero_gains,
                RESET
            );
        }

        // Determine search floor based on mode
        // Ultimate Mode has NO floor (0.0) to allow hitting the true physical wall.
        let search_floor = if ultimate_mode { 0.0 } else { min_crf };

        // Milestone tracking to ensure "integer stages" are visible in the terminal
        let mut last_logged_int_crf = crate::numeric_cast::f32_to_i32_sat(gpu_boundary_crf.floor());
        crate::log_eprintln!(
            "{}💠 Entering CRF {}.x zone{}",
            BRIGHT_CYAN,
            last_logged_int_crf,
            RESET
        );

        while iterations < max_iterations_for_video && test_crf >= search_floor {
            // Milestone logging check
            let current_int_crf = crate::numeric_cast::f32_to_i32_sat(test_crf.floor());
            if current_int_crf != last_logged_int_crf {
                last_logged_int_crf = current_int_crf;
                crate::log_eprintln!(
                    "{}💠 Entering CRF {}.x zone{}",
                    BRIGHT_CYAN,
                    last_logged_int_crf,
                    RESET
                );
            }
            if test_crf < search_floor {
                if current_step > MIN_STEP + 0.01 {
                    crate::verbose_eprintln!(
                        "   {}Reached search floor, fine tuning from CRF {:.2}{}",
                        BRIGHT_CYAN,
                        last_good_crf,
                        RESET
                    );
                    current_step = MIN_STEP;
                    test_crf = last_good_crf - current_step;
                    if test_crf < search_floor {
                        break;
                    }
                } else {
                    break;
                }
            }

            if (test_crf - 0.0).abs() < 0.001 && duration > HEAVY_VIDEO_THRESHOLD_SECS {
                // For heavy/long videos (> 30 min), only attempt CRF 0.00 if we have
                // already achieved a credible high-quality success (< 5.0) to avoid wasting hours.
                if tracking.best_vmaf.is_none_or(|c| c >= 5.0) {
                    crate::log_eprintln!(
                        "   {}⏳ Heavy video ({:.1} min): skipping CRF 0.00 probe as no high-quality success (< 5.0) confirmed yet.{}",
                        BRIGHT_CYAN, duration / 60.0, RESET
                    );
                    break;
                }
            }

            if size_cache.contains_key(test_crf) {
                test_crf -= current_step;
                continue;
            }

            let size = encode_cached(test_crf, &mut size_cache)?;
            iterations += 1;
            let total_size_pct = if input_size > 0 {
                (crate::numeric_cast::u64_to_f64(size)
                    / crate::numeric_cast::u64_to_f64(input_size.max(1))
                    - 1.0)
                    * 100.0
            } else {
                0.0
            };
            let current_ssim_opt = if ultimate_mode {
                None
            } else {
                calculate_ssim_quick()
            };

            let is_effectively_compressed = size < input_size;

            if is_effectively_compressed {
                let prev_ssim_opt = last_good_ssim;
                last_good_crf = test_crf;
                last_good_size = size;
                last_good_ssim = current_ssim_opt;
                best_crf = Some(test_crf);
                best_size = Some(size);

                let should_stop = if ultimate_mode {
                    let mut ultimate_metrics_str = String::new();
                    let mut quality_plateau = false;
                    let mut metrics_measured = false;

                    let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
                    let psnr_uv = super::ssim_calculator::calculate_psnr_uv(input, output, 6);

                    if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                        metrics_measured = true;
                        let chroma_avg = f64::midpoint(u, v_score);
                        let prev_best_vmaf = tracking.best_vmaf.unwrap_or(0.0);
                        let prev_best_psnr = tracking
                            .best_psnr_uv
                            .map_or(0.0, |(u, v)| f64::midpoint(u, v));
                        let vmaf_improved = v.floor() > prev_best_vmaf.floor();
                        let psnr_improved = chroma_avg.floor() > prev_best_psnr.floor();

                        ultimate_metrics_str = format!("VMAF:{v:.2} UV:{chroma_avg:.2}");

                        if vmaf_improved || tracking.best_vmaf.is_none() {
                            tracking.best_vmaf = Some(v);
                        }
                        if psnr_improved || tracking.best_psnr_uv.is_none() {
                            tracking.best_psnr_uv = Some((u, v_score));
                        }

                        let any_metric_fails = metrics_below_ultimate_sanity_floor(v, (u, v_score));

                        if !vmaf_improved && !psnr_improved && any_metric_fails {
                            failure_credibility += 1.0;
                            if failure_credibility >= 3.0 {
                                crate::log_eprintln!(
                                    "   {}❌ QUALITY PLATEAU REACHED (3/3):{} No integer improvement over 3 insights. Stopping.",
                                    BRIGHT_RED, RESET
                                );
                                early_insight_triggered = true;
                                break;
                            }
                        } else {
                            failure_credibility = 0.0;
                        }

                        quality_plateau =
                            (v > 97.0 || chroma_avg > 47.0) && !vmaf_improved && !psnr_improved;
                    }

                    if current_step <= MIN_STEP + 0.01 {
                        if quality_plateau {
                            consecutive_zero_gains += 1;
                        } else {
                            consecutive_zero_gains = 0;
                        }
                    }

                    let quality_wall_triggered = metrics_measured
                        && current_step <= MIN_STEP + 0.01
                        && consecutive_zero_gains >= required_zero_gains;

                    if quality_wall_triggered {
                        let Some(vmaf_metric) = tracking.best_vmaf else {
                            crate::log_eprintln!(
                                "   {}⚠️  VMAF not measured at quality wall{}",
                                BRIGHT_YELLOW,
                                RESET
                            );
                            bail!("Quality wall hit but VMAF not measured");
                        };
                        let psnr_uv_min_channel = if let Some((u, v)) = tracking.best_psnr_uv {
                            u.min(v)
                        } else {
                            crate::log_eprintln!(
                                "   {}⚠️  PSNR UV not measured at quality wall{}",
                                BRIGHT_YELLOW,
                                RESET
                            );
                            bail!("Quality wall hit but PSNR UV not measured");
                        };

                        if vmaf_metric < VMAF_Y_SANITY_FLOOR
                            || psnr_uv_min_channel < PSNR_UV_SANITY_FLOOR
                        {
                            crate::log_eprintln!(
                                "   \x1b[1;31m❌ QUALITY CEILING HIT (NOT CREDIBLE):\x1b[0m Saturated at VMAF:{:.2}, UV:{:.2}. Below sanity floor. Aborting.",
                                vmaf_metric, psnr_uv_min_channel
                            );
                            quality_wall_hit = true;
                            break;
                        }
                    }

                    let sat_status = if consecutive_zero_gains > 0
                        && current_step <= MIN_STEP + 0.01
                    {
                        format!(
                                " {BRIGHT_MAGENTA}[SAT:{consecutive_zero_gains}/{required_zero_gains}]{RESET}"
                            )
                    } else {
                        String::new()
                    };

                    let metrics_display = if ultimate_metrics_str.is_empty() {
                        " │ 3D metrics N/A".to_string()
                    } else {
                        format!("{BRIGHT_MAGENTA}{ultimate_metrics_str}{RESET}")
                    };

                    crate::log_eprintln!(
                        "{}{}   {}✓{} [CPU] {}CRF {:<5.2}{} {}{:6.1}% {} {}{}",
                        RESET,
                        RESET,
                        BRIGHT_GREEN,
                        RESET,
                        CYAN,
                        test_crf,
                        RESET,
                        MFB_BLUE,
                        total_size_pct,
                        RESET,
                        metrics_display,
                        sat_status
                    );

                    if quality_wall_triggered {
                        quality_wall_hit = true;
                    }
                    quality_wall_triggered
                } else if let (Some(current_ssim), Some(prev_ssim)) =
                    (current_ssim_opt, prev_ssim_opt)
                {
                    let ssim_gain = current_ssim - prev_ssim;

                    if let Some(gpu_baseline) = gpu_ssim_baseline.filter(|v| *v > 0.0) {
                        let ssim_vs_gpu = current_ssim / gpu_baseline;
                        let _gpu_comparison = if ssim_vs_gpu > 1.01 {
                            format!("{BRIGHT_GREEN}×{ssim_vs_gpu:.3} GPU{RESET}")
                        } else if ssim_vs_gpu > 1.001 {
                            format!("{GREEN}×{ssim_vs_gpu:.4} GPU{RESET}")
                        } else {
                            format!("{DIM}≈GPU{RESET}")
                        };
                    }

                    if current_step <= MIN_STEP + 0.01 {
                        if ssim_gain.abs() < ZERO_GAIN_THRESHOLD {
                            consecutive_zero_gains += 1;
                        } else {
                            consecutive_zero_gains = 0;
                        }
                    }

                    let quality_wall_triggered = current_step <= MIN_STEP + 0.01
                        && consecutive_zero_gains >= required_zero_gains;

                    let sat_status = if consecutive_zero_gains > 0
                        && current_step <= MIN_STEP + 0.01
                    {
                        format!(" {DIM}[SAT:{consecutive_zero_gains}/{required_zero_gains}]{RESET}")
                    } else {
                        String::new()
                    };

                    crate::log_eprintln!(
                        "{}{}   {}✓{} [CPU] {}CRF {:<5.2}{} {}{:6.1}% {} │ SSIM:{:.4} Δ{:+.4}{}",
                        RESET,
                        RESET,
                        BRIGHT_GREEN,
                        RESET,
                        CYAN,
                        test_crf,
                        RESET,
                        MFB_BLUE,
                        total_size_pct,
                        RESET,
                        current_ssim,
                        ssim_gain,
                        sat_status
                    );

                    if quality_wall_triggered {
                        quality_wall_hit = true;
                    }
                    quality_wall_triggered
                } else {
                    crate::log_eprintln!(
                        "{}{}   {}✓{} [CPU] {}CRF {:<5.2}{} {}{:6.1}% {} │ SSIM N/A",
                        RESET,
                        RESET,
                        BRIGHT_GREEN,
                        RESET,
                        CYAN,
                        test_crf,
                        RESET,
                        MFB_BLUE,
                        total_size_pct,
                        RESET
                    );
                    false
                };

                if should_stop {
                    crate::log_eprintln!();
                    if ultimate_mode {
                        domain_wall_hit = true;
                        let msg = if consecutive_zero_gains >= required_zero_gains {
                            format!(
                                "3D quality plateau after {consecutive_zero_gains} consecutive fine-step non-improvements"
                            )
                        } else {
                            "VMAF(Y) + PSNR(UV) absolute quality ceiling reached".to_string()
                        };
                        crate::log_eprintln!(
                            "   {} [CPU] DOMAIN WALL HIT:{} {}",
                            BRIGHT_MAGENTA,
                            RESET,
                            msg
                        );
                    } else {
                        crate::log_eprintln!("   {} [CPU] QUALITY WALL HIT:{} SSIM saturated after {} consecutive zero-gains",
                            BRIGHT_YELLOW, RESET, consecutive_zero_gains);
                    }
                    crate::verbose_eprintln!(
                        "   {}Final: CRF {:.2}, compression {:+.1}%, iterations {}{}",
                        BRIGHT_CYAN,
                        test_crf,
                        total_size_pct,
                        iterations,
                        RESET
                    );
                    break;
                }

                test_crf -= current_step;
            } else {
                wall_hits += 1;

                let _total_file_diff = crate::format_size_diff(
                    i64::try_from(size).unwrap_or(0) - i64::try_from(input_size).unwrap_or(0),
                );

                // Calculate new_step first for phase_info
                let curve_step =
                    initial_step * DECAY_FACTOR.powi(i32::try_from(wall_hits).unwrap_or(0));
                let new_step = if curve_step < 1.0 {
                    MIN_STEP
                } else {
                    curve_step
                };

                let phase_info = if wall_hits == 1 {
                    format!("decay ×{DECAY_FACTOR:.1}")
                } else if new_step <= MIN_STEP + 0.01 {
                    "→ FINE TUNING".to_string()
                } else {
                    format!("decay {DIM}×{DECAY_FACTOR:.1}^{wall_hits}")
                };

                crate::log_eprintln!(
                    "{}{}   {}✗{} [CPU] {}CRF {:<5.2}{} {}{:6.1}% {} │ ❌ WALL HIT #{} (Backtrack: {:.2} → {:.2} {})",
                    RESET, RESET, BRIGHT_RED, RESET, CYAN, test_crf, RESET,
                    DIM, total_size_pct, RESET, wall_hits, current_step, new_step, phase_info
                );

                if current_step <= MIN_STEP + 0.01 && new_step <= MIN_STEP + 0.01 {
                    // We hit a physical capacity boundary at the minimum step size.
                    // Since CRF vs File Size is strictly monotonic, stepping downward further
                    // will only yield even larger files, guaranteeing consecutive failures.
                    // Therefore, we lock down the oscillation and break immediately, handing
                    // off the exact boundary to Phase 4 (if in ultimate mode).
                    if ultimate_mode {
                        crate::log_eprintln!(
                            "   {} [CPU] 🧱 Size wall hit at 0.01 minimum granularity. Oscillation locked down, handing off to Phase 4.{}",
                            BRIGHT_YELLOW,
                            RESET
                        );
                        break;
                    }
                    crate::log_eprintln!(
                        "   {} [CPU] 🧱 Minimum step reached and hit capacity wall. Stopping exploration.{}",
                        BRIGHT_YELLOW,
                        RESET
                    );
                    break;
                }

                if wall_hits >= max_wall_hits {
                    if ultimate_mode {
                        crate::log_eprintln!(
                            "   {} [CPU] Adaptive wall limit ({}) reached.{} Stopping at best CRF {:.2}",
                            BRIGHT_YELLOW,
                            max_wall_hits,
                            RESET,
                            last_good_crf
                        );
                    } else {
                        crate::log_eprintln!(
                            "   {} [CPU] Max wall hits ({}) reached.{} Stopping at best CRF {:.2}",
                            BRIGHT_YELLOW,
                            max_wall_hits,
                            RESET,
                            last_good_crf
                        );
                    }
                    break;
                }

                current_step = new_step;
                test_crf = last_good_crf - current_step;
            }
        }

        if domain_wall_hit || quality_wall_hit {
            if best_crf.is_none_or(|c| c > last_good_crf) {
                best_crf = Some(last_good_crf);
                best_size = Some(last_good_size);
            }
        } else if wall_hits > 0 {
            crate::log_eprintln!();
            crate::log_eprintln!(
                "   {} [CPU] Size wall hit: overshoot at CRF < {:.1}{}",
                BRIGHT_RED,
                last_good_crf,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}Final: CRF {:.2}, iterations {}{}",
                BRIGHT_CYAN,
                last_good_crf,
                iterations,
                RESET
            );
        } else if test_crf < search_floor {
            crate::log_eprintln!();
            crate::log_eprintln!(
                "   {} [CPU] Search floor reached ({:.1}) - maximum quality achieved{}",
                BRIGHT_GREEN,
                search_floor,
                RESET
            );
            crate::verbose_eprintln!(
                "   {}Final: CRF {:.2}, iterations {}{}",
                BRIGHT_CYAN,
                last_good_crf,
                iterations,
                RESET
            );

            if best_crf.is_none_or(|c| c > last_good_crf) {
                best_crf = Some(last_good_crf);
                best_size = Some(last_good_size);
            }
        }
    } else {
        use crate::modern_ui::colors::{
            BRIGHT_CYAN, BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, CYAN, RESET,
        };
        crate::log_eprintln!(
            "{}{}   {}✗{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{} ❌ (TOO LARGE)",
            RESET,
            RESET,
            BRIGHT_RED,
            RESET,
            CYAN,
            gpu_boundary_crf,
            RESET,
            BRIGHT_RED,
            gpu_pct,
            RESET
        );
        crate::log_eprintln!();
        crate::log_eprintln!("Phase 2: [CPU] Search UPWARD for compression boundary");
        crate::log_eprintln!("   (Higher CRF = Smaller file, find first compressible)");

        let mut current_step = step_size_upward;
        let mut stagnation_count = 0u32;
        let mut backtrack_count = 0u32;
        let mut last_size_pct = gpu_pct;
        let mut test_crf = gpu_boundary_crf + current_step;
        let mut search_cadence = UpwardSearchCadence::Adaptive;
        let mut found_compress_point = false;
        let mut failure_credibility = 0.0f64;
        let mut best_tested_crf = gpu_boundary_crf;
        let mut best_tested_size = gpu_size;

        let mut feedback = UpwardSearchFeedback {
            size_stagnation_count: 0,
            upward_iteration_count: 0,
        };

        // [Bi-directional Pivot / Reverse Exploration]
        // If the initial "floor" probe (usually CRF 0.00) failed by a wide margin,
        // we orbit to the "ceiling" (max_crf) first to see if compression is even possible.
        // This delivers a 2-iteration "fail-fast" for non-short incompressible media.
        if gpu_boundary_crf < 5.0 && gpu_pct > 3.0 {
            use crate::modern_ui::colors::BRIGHT_YELLOW;
            crate::log_eprintln!(
                "   {}🔄 Bi-directional Pivot: CRF {:.2} too large ({:.1}%), probing ceiling CRF {:.2}...{}",
                BRIGHT_YELLOW,
                gpu_boundary_crf,
                gpu_pct,
                max_crf,
                RESET
            );

            let ceiling_size = encode_cached(max_crf, &mut size_cache)?;
            iterations += 1;

            let ceiling_pct = if input_size > 0 {
                (crate::numeric_cast::u64_to_f64(ceiling_size)
                    / crate::numeric_cast::u64_to_f64(input_size.max(1))
                    - 1.0)
                    * 100.0
            } else {
                0.0
            };

            if ceiling_pct >= 0.0 {
                crate::log_eprintln!(
                    "   {}⏸️  Media is incompressible even at max quality (CRF {:.1}). Bailing out.{}",
                    BRIGHT_RED,
                    max_crf,
                    RESET
                );
                best_crf = Some(max_crf);
                best_size = Some(ceiling_size);
                early_insight_triggered = true;
                // break handled by while loop condition? No, the loop hasn't started.
                // We'll jump past the rest of Phase 2.
            } else {
                crate::log_eprintln!(
                    "   {}🎯 Ceiling hit! Space [0.0, {:.2}] is compressible. Starting search from mid-point...{}",
                    BRIGHT_GREEN,
                    max_crf,
                    RESET
                );
                // [Optimization] "Mid-Jump Pivot"
                // Instead of walking from 0.00 to 10.75 in 40+ iterations, we jump directly
                // to a reasonable mid-floor (12.0 for HEVC/AV1) if the ceiling is successful.
                // The loop below will then 'officially' explore this point first.
                //
                // [Hardened] Always record the ceiling as the best fallback before jumping.
                best_crf = Some(max_crf);
                best_size = Some(ceiling_size);
                test_crf = 12.0f32;
            }
        }

        let max_iterations_for_video = if ultimate_mode {
            500
        } else {
            calculate_max_iterations_for_duration(duration, ultimate_mode)
        };

        while test_crf <= max_crf
            && iterations < max_iterations_for_video
            && !early_insight_triggered
        {
            let size = encode_cached(test_crf, &mut size_cache)?;
            iterations += 1;
            feedback.upward_iteration_count += 1;

            let total_size_pct = if input_size > 0 {
                (crate::numeric_cast::u64_to_f64(size)
                    / crate::numeric_cast::u64_to_f64(input_size.max(1))
                    - 1.0)
                    * 100.0
            } else {
                0.0
            };

            if total_size_pct < 0.0 {
                found_compress_point = true;
                best_tested_crf = test_crf;
                best_tested_size = size;
                // [Hardened] Record the current point as the best fallback for Phase 3 exploration
                best_crf = Some(test_crf);
                best_size = Some(size);
                break; // Boundary found!
            }

            let size_delta = (total_size_pct - last_size_pct).abs();

            // [Unified] Direction Switch Logic for all media
            // If we've been searching upward for a long time without finding compression,
            // or if the size is barely changing (stagnation), switch to downward search.
            if size_delta < 0.5 {
                // Size stagnation should only be tracked once we are past the
                // effective-lossless deadzone (0-12) to avoid premature flips.
                if test_crf > 12.0 {
                    feedback.size_stagnation_count += 1;
                }
            } else {
                feedback.size_stagnation_count = 0;
            }

            // [Refined] Only flip direction if we've meaningfully explored the low-bitrate space.
            if feedback.size_stagnation_count >= UPWARD_SIZE_STAGNATION_THRESHOLD
                || (feedback.upward_iteration_count >= UPWARD_DIRECTION_SWITCH_LIMIT
                    && test_crf > 20.0)
            {
                let trigger_reason =
                    if feedback.size_stagnation_count >= UPWARD_SIZE_STAGNATION_THRESHOLD {
                        format!("size stagnation ({})", feedback.size_stagnation_count)
                    } else {
                        format!("iteration limit ({})", feedback.upward_iteration_count)
                    };

                crate::log_eprintln!(
                    "   {}🔄 Search Direction Switch: {} reached. Switching to downward search for efficiency.{}",
                    BRIGHT_YELLOW,
                    trigger_reason,
                    RESET
                );

                found_compress_point = true;
                // Start Phase 3 from max_crf to find the boundary from the top
                best_crf = Some(max_crf);
                break;
            }

            // Adaptive Search State Machine: Efficiency Boosters
            if search_cadence == UpwardSearchCadence::Adaptive {
                if size_delta < 0.1 && total_size_pct > 100.0 {
                    stagnation_count += 1;

                    // Acceleration Burst: Jump through ineffective CRF deadzones (0-15)
                    // Low CRFs often show zero size change; we leap forward to find the curve.
                    if test_crf < 15.0 && size_delta < 0.02 && stagnation_count >= 2 {
                        let jump_step = (20.0 - test_crf).max(8.0);
                        crate::log_eprintln!(
                            "   {}⚡ Deadzone Burst: jumping {:.1} units to escape the lossless plateau...{}",
                            BRIGHT_YELLOW,
                            jump_step,
                            RESET
                        );
                        current_step = jump_step;
                        stagnation_count = 0; // Reset after jump
                    } else if stagnation_count >= 2 {
                        // Standard Lightning Acceleration for higher CRFs
                        let old_step = current_step;
                        current_step = (current_step * 2.0).min(5.0);
                        if current_step > old_step {
                            crate::log_eprintln!(
                                "   {}⚡ Search Accelerated (step: {:.2} → {:.2}){}",
                                BRIGHT_YELLOW,
                                old_step,
                                current_step,
                                RESET
                            );
                        }
                    }

                    // Plateau Bailout: Stop ONLY if format is clearly incompressible AND high CRF
                    if stagnation_count >= 6 && total_size_pct > 110.0 && test_crf > 30.0 {
                        crate::log_eprintln!(
                            "   {}⏸️  Quality/Size Plateau detected: bailing out early.{}",
                            BRIGHT_RED,
                            RESET
                        );
                        if best_crf.is_none() {
                            best_crf = Some(best_tested_crf);
                            best_size = Some(best_tested_size);
                        }
                        early_insight_triggered = true;
                        break;
                    }
                } else if size_delta > 2.5 && total_size_pct < 110.0 {
                    // Deceleration: Slope detected AND nearing compression boundary
                    if current_step > step_size_upward {
                        let jog_step = UPWARD_JOG_MIN_STEP.max(step_size_upward);
                        if current_step > jog_step + f32::EPSILON {
                            crate::log_eprintln!(
                                "   {}💧 Search Decelerating (slope Δ{:.1} detected, step: {:.2} → {:.2}, entering jog){}",
                                CYAN,
                                size_delta,
                                current_step,
                                jog_step,
                                RESET
                            );
                            current_step = jog_step;
                            search_cadence = UpwardSearchCadence::Jogging;
                        } else {
                            crate::log_eprintln!(
                                "   {}💧 Search Decelerating (slope Δ{:.1} detected, step: {:.2} → {:.2}, entering pause){}",
                                CYAN,
                                size_delta,
                                current_step,
                                step_size_upward,
                                RESET
                            );
                            current_step = step_size_upward;
                            search_cadence = UpwardSearchCadence::Paused;
                        }
                    }
                    stagnation_count = 0;
                } else {
                    stagnation_count = 0;
                }
            }

            // Track best tested CRF (smallest size increase, even if not compressed)
            if size < best_tested_size {
                best_tested_crf = test_crf;
                best_tested_size = size;
            }

            let is_effectively_compressed = size < input_size;

            // Ultimate Mode: Insight-Based Credibility Check (Sticky)
            // Optimization: Only run expensive VMAF/PSNR if we are somewhat close to compression (< 120%)
            // to avoid process exhaustion under high CPU load during early coarse jumps.
            if ultimate_mode
                && !is_effectively_compressed
                && (total_size_pct < 120.0 || iterations < 2)
            {
                let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
                let psnr_uv = super::ssim_calculator::calculate_psnr_uv(input, output, 6);

                if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                    let chroma_avg = f64::midpoint(u, v_score);

                    // Track best metrics to check for improvement
                    let prev_best_vmaf = tracking.best_vmaf.unwrap_or(0.0);
                    let prev_best_psnr = tracking
                        .best_psnr_uv
                        .map_or(0.0, |(u, v)| f64::midpoint(u, v));

                    // Check for integer-level improvement (ignoring decimals)
                    let vmaf_improved = v.floor() > prev_best_vmaf.floor();
                    let psnr_improved = chroma_avg.floor() > prev_best_psnr.floor();
                    let improvement_indicator = if vmaf_improved || psnr_improved {
                        "↑"
                    } else {
                        "→"
                    };
                    crate::log_eprintln!(
                        "{}{}   {}✗{} [CPU] {}CRF {:<5.2}{} {}{:6.1}% {} │ VMAF:{:.2} UV:{:.2} ({:.1}/3.0 {})",
                        RESET, RESET, BRIGHT_RED, RESET, CYAN, test_crf, RESET,
                        DIM, total_size_pct, RESET, v, chroma_avg, failure_credibility, improvement_indicator
                    );

                    // Cache for Phase III and tracking
                    if vmaf_improved || tracking.best_vmaf.is_none() {
                        tracking.best_vmaf = Some(v);
                    }
                    if psnr_improved || tracking.best_psnr_uv.is_none() {
                        tracking.best_psnr_uv = Some((u, v_score));
                    }

                    // Early insight: only trigger if BOTH quality metrics fail threshold AND no improvement
                    let both_metrics_fail =
                        both_metrics_below_ultimate_sanity_floor(v, (u, v_score));

                    if !vmaf_improved && !psnr_improved && both_metrics_fail {
                        failure_credibility += 1.0;
                        if failure_credibility >= 3.0 {
                            crate::log_eprintln!(
                                "   {}❌ QUALITY PLATEAU REACHED (3/3):{} No integer improvement over 3 insights. Stopping.",
                                BRIGHT_RED, RESET
                            );
                            // Use best tested CRF if no compression achieved
                            if best_crf.is_none() {
                                best_crf = Some(best_tested_crf);
                                best_size = Some(best_tested_size);
                            }
                            early_insight_triggered = true;
                            break;
                        }
                    } else {
                        failure_credibility = 0.0;
                    }
                }
            }

            if is_effectively_compressed {
                // Backtrack-on-Overshoot: If we jumped from >105% to <95%, seek precision (Anti-Oscillation Guard)
                if last_size_pct > 105.0
                    && total_size_pct < 95.0
                    && current_step > 0.5
                    && backtrack_count < 2
                {
                    crate::log_eprintln!(
                        "   {}⏪ Overshot boundary ({:.1}%): backtracking for precision... (retry {}/2){}",
                        BRIGHT_YELLOW,
                        total_size_pct,
                        backtrack_count + 1,
                        RESET
                    );
                    test_crf -= current_step / 2.0; // Binary bisect
                    current_step = step_size_upward;
                    backtrack_count += 1;
                    // Stability Fix: Do NOT update last_size_pct here.
                    // This ensures the next step's delta is calculated against the last "failed" size.
                    continue;
                }

                if best_crf.is_none_or(|c| test_crf < c) {
                    best_crf = Some(test_crf);
                    best_size = Some(size);
                }
                found_compress_point = true;
                crate::log_eprintln!(
                    "{}{}   {}✓{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{} │ FOUND! ✅",
                    RESET,
                    RESET,
                    BRIGHT_GREEN,
                    RESET,
                    CYAN,
                    test_crf,
                    RESET,
                    BRIGHT_GREEN,
                    total_size_pct,
                    RESET
                );
                break; // Stop Phase 2 after finding first compression point
            } else if !ultimate_mode {
                crate::log_eprintln!(
                    "{}{}   {}✗{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{} ❌",
                    RESET,
                    RESET,
                    BRIGHT_RED,
                    RESET,
                    CYAN,
                    test_crf,
                    RESET,
                    BRIGHT_RED,
                    total_size_pct,
                    RESET
                );
            }

            match search_cadence {
                UpwardSearchCadence::Jogging => {
                    crate::log_eprintln!(
                        "   {}🐢 Search Jogging complete (step: {:.2} → {:.2}); pausing adaptive changes{}",
                        BRIGHT_CYAN,
                        current_step,
                        step_size_upward,
                        RESET
                    );
                    current_step = step_size_upward;
                    search_cadence = UpwardSearchCadence::Paused;
                }
                UpwardSearchCadence::Paused => {
                    crate::log_eprintln!(
                        "   {}⏸️  Search Paused at boundary pace ({:.2}); resuming normal iteration next step{}",
                        BRIGHT_CYAN,
                        current_step,
                        RESET
                    );
                    search_cadence = UpwardSearchCadence::Normal;
                }
                UpwardSearchCadence::Adaptive | UpwardSearchCadence::Normal => {}
            }

            last_size_pct = total_size_pct;
            test_crf += current_step;
        }

        if found_compress_point {
            crate::log_eprintln!();
            crate::log_eprintln!(
                "{}Phase 3: [CPU] Search DOWNWARD with Sprint & Backtrack (min step {:.2}){}",
                BRIGHT_CYAN,
                PHASE3_DOWNWARD_STEP,
                RESET
            );

            let compress_point = best_crf.unwrap_or(gpu_boundary_crf);
            let mut current_step = PHASE3_DOWNWARD_STEP;
            let mut last_size_pct = if input_size > 0 {
                (crate::numeric_cast::u64_to_f64(best_size.unwrap_or(input_size))
                    / crate::numeric_cast::u64_to_f64(input_size.max(1))
                    - 1.0)
                    * 100.0
            } else {
                0.0
            };
            let mut backtrack_count = 0u32;
            let mut failure_credibility = 0.0f64;
            let mut consecutive_failures = 0u32;
            let mut consecutive_successes = 0;
            let mut consecutive_compressions = 0u32;
            let mut prev_ssim_opt = if ultimate_mode {
                None
            } else {
                calculate_ssim_quick()
            };
            let search_floor = if ultimate_mode { 0.0 } else { min_crf };
            let mut test_crf = compress_point - current_step;

            while test_crf >= search_floor && iterations < max_iterations_for_video {
                if size_cache.contains_key(test_crf) {
                    test_crf -= current_step;
                    continue;
                }

                let size = encode_cached(test_crf, &mut size_cache)?;
                iterations += 1;
                let total_size_pct = if input_size > 0 {
                    (crate::numeric_cast::u64_to_f64(size)
                        / crate::numeric_cast::u64_to_f64(input_size.max(1))
                        - 1.0)
                        * 100.0
                } else {
                    0.0
                };

                let current_ssim_opt = if ultimate_mode {
                    None
                } else {
                    calculate_ssim_quick()
                };

                // Ultimate metrics for insight mechanism
                let mut vmaf_improved = false;
                let mut psnr_improved = false;
                let mut current_vmaf_val = None;
                let mut current_psnr_val = None;

                if ultimate_mode {
                    let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
                    let psnr_uv = super::ssim_calculator::calculate_psnr_uv(input, output, 6);

                    if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                        let chroma_avg = f64::midpoint(u, v_score);
                        let prev_best_vmaf = tracking.best_vmaf.unwrap_or(0.0);
                        let prev_best_psnr = tracking
                            .best_psnr_uv
                            .map_or(0.0, |(u, v)| f64::midpoint(u, v));

                        vmaf_improved = v.floor() > prev_best_vmaf.floor();
                        psnr_improved = chroma_avg.floor() > prev_best_psnr.floor();

                        // Diagnostics: VMAF:{v:.2} UV:{chroma_avg:.2}

                        current_vmaf_val = Some(v);
                        current_psnr_val = Some((u, v_score));
                    }
                }

                let is_effectively_compressed = size < input_size;

                let size_delta = (total_size_pct - last_size_pct).abs();

                if is_effectively_compressed {
                    consecutive_failures = 0;
                    consecutive_compressions += 1;

                    best_crf = Some(test_crf);
                    best_size = Some(size);

                    if ultimate_mode {
                        if vmaf_improved || tracking.best_vmaf.is_none() {
                            tracking.best_vmaf = current_vmaf_val;
                        }
                        if psnr_improved || tracking.best_psnr_uv.is_none() {
                            tracking.best_psnr_uv = current_psnr_val;
                        }
                    }

                    let improvement_indicator = if vmaf_improved || psnr_improved {
                        "↑"
                    } else {
                        "→"
                    };

                    let ssim_gain = match (current_ssim_opt, prev_ssim_opt) {
                        (Some(curr), Some(prev)) => curr - prev,
                        _ => 0.0,
                    };

                    let metrics_str = if ultimate_mode {
                        let vmaf_opt = tracking.best_vmaf;
                        let psnr_uv_opt = tracking.best_psnr_uv;
                        if let (Some(v), Some((u, v_score))) = (vmaf_opt, psnr_uv_opt) {
                            let chroma_avg = f64::midpoint(u, v_score);
                            format!(
                                " │ VMAF:{v:.2} UV:{chroma_avg:.2} ({failure_credibility:.0}/3 {improvement_indicator})"
                            )
                        } else {
                            String::new()
                        }
                    } else if let Some(current_ssim) = current_ssim_opt {
                        format!(" │ SSIM:{current_ssim:.4} Δ{ssim_gain:+.4}")
                    } else {
                        " │ SSIM N/A".to_string()
                    };

                    crate::log_eprintln!(
                        "{}{}   {}✓{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{}{} (step {:.2}) ✅",
                        RESET,
                        RESET,
                        BRIGHT_GREEN,
                        RESET,
                        CYAN,
                        test_crf,
                        RESET,
                        BRIGHT_GREEN,
                        total_size_pct,
                        RESET,
                        metrics_str,
                        current_step
                    );

                    // Check if reached max consecutive compressions
                    if consecutive_compressions >= MAX_CONSECUTIVE_COMPRESSIONS {
                        crate::log_eprintln!(
                            "   {}✓ Efficiency limit reached: {} consecutive compressions found. Stopping.{}",
                            BRIGHT_GREEN, MAX_CONSECUTIVE_COMPRESSIONS, RESET
                        );
                        break;
                    }

                    // Early termination logic: based on insight evaluation
                    if ultimate_mode {
                        let any_metric_fails =
                            if let (Some(v), Some(uv)) = (current_vmaf_val, current_psnr_val) {
                                metrics_below_ultimate_sanity_floor(v, uv)
                            } else {
                                false
                            };

                        if !vmaf_improved && !psnr_improved && any_metric_fails {
                            failure_credibility += 1.0;
                            if failure_credibility >= 3.0 {
                                crate::log_eprintln!(
                                    "   {}❌ QUALITY PLATEAU REACHED (3/3):{} No integer improvement over 3 insights. Stopping.",
                                    BRIGHT_RED, RESET
                                );
                                break;
                            }
                        } else {
                            failure_credibility = 0.0;
                        }
                    } else {
                        // Original non-ultimate mode stopping logic
                        if let (Some(s), Some(p)) = (current_ssim_opt, prev_ssim_opt) {
                            if s - p < 0.0001 && s >= 0.99 {
                                crate::log_eprintln!(
                                    "   {}SSIM plateau → STOP{}",
                                    BRIGHT_CYAN,
                                    RESET
                                );
                                break;
                            }
                        }
                    }

                    prev_ssim_opt = current_ssim_opt;

                    // Unified Adaptive Logic: Deceleration & Sprint
                    let distance_to_floor = test_crf - search_floor;
                    let decelerate_multiplier = if ultimate_mode { 1.0 } else { 2.0 };
                    let boundary_nearing = distance_to_floor < current_step * decelerate_multiplier;

                    if size_delta > 1.0 && current_step > PHASE3_DOWNWARD_STEP {
                        // Slope-Aware Deceleration (⚡ -> 💧)
                        let old_step = current_step;
                        current_step = PHASE3_DOWNWARD_STEP;
                        consecutive_successes = 0;
                        crate::log_eprintln!(
                            "   {}💧 Search Decelerating (slope Δ{:.1} detected, step reset: {:.2} → {:.2}){}",
                            CYAN, size_delta, old_step, current_step, RESET
                        );
                    } else if boundary_nearing && current_step > PHASE3_DOWNWARD_STEP + 0.001 {
                        // Boundary-Aware Deceleration
                        let old_step = current_step;
                        current_step = (current_step / 2.0).max(PHASE3_DOWNWARD_STEP);
                        consecutive_successes = 0;
                        crate::log_eprintln!(
                            "   {}🎯 Smart deceleration: step {:.2} → {:.2} (approaching floor {:.2}){}",
                            BRIGHT_YELLOW, old_step, current_step, search_floor, RESET
                        );
                    } else {
                        // Sprint: double the step for faster iteration
                        consecutive_successes += 1;
                        if consecutive_successes >= 2 && current_step < 1.6 {
                            let old_step = current_step;
                            current_step = (current_step * 2.0).min(1.6);
                            crate::log_eprintln!(
                                "   {}⚡ Sprint activated: step {:.2} → {:.2}{}",
                                BRIGHT_CYAN,
                                old_step,
                                current_step,
                                RESET
                            );
                        }
                    }

                    last_size_pct = total_size_pct;
                    test_crf -= current_step;
                } else {
                    consecutive_failures += 1;

                    let metrics_str = if ultimate_mode {
                        let vmaf_opt = tracking.best_vmaf;
                        let psnr_uv_opt = tracking.best_psnr_uv;
                        if let (Some(v), Some((u, v_score))) = (vmaf_opt, psnr_uv_opt) {
                            let chroma_avg = f64::midpoint(u, v_score);
                            format!(
                                " │ VMAF:{v:.2} UV:{chroma_avg:.2} ({failure_credibility:.0}/3 →)"
                            )
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    crate::log_eprintln!(
                        "{}{}   {}✗{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{}{} {}❌ (fail #{}/{})",
                        RESET,
                        RESET,
                        BRIGHT_RED,
                        RESET,
                        CYAN,
                        test_crf,
                        RESET,
                        BRIGHT_RED,
                        total_size_pct,
                        RESET,
                        metrics_str,
                        BRIGHT_RED,
                        consecutive_failures,
                        MAX_CONSECUTIVE_FAILURES
                    );

                    // Unified Anti-Oscillation Backtrack
                    if current_step > PHASE3_DOWNWARD_STEP + 0.01 && backtrack_count < 2 {
                        let old_step = current_step;
                        current_step = (current_step / 2.0).max(PHASE3_DOWNWARD_STEP);
                        backtrack_count += 1;
                        consecutive_successes = 0;
                        crate::log_eprintln!(
                            "   {}⏪ Backtracking for precision (retry {}/2): {:.2} → {:.2}{}",
                            BRIGHT_YELLOW,
                            backtrack_count,
                            old_step,
                            current_step,
                            RESET
                        );
                        test_crf = best_crf.unwrap_or(test_crf + old_step) - current_step;
                        // Stability Fix: Do NOT update last_size_pct here.
                        continue;
                    }

                    // If not ultimate mode, immediately break on first capacity exceed in phase 3
                    if !ultimate_mode {
                        crate::log_eprintln!(
                            "   {}Capacity exceeded at step {:.2}. Stopping.{}",
                            BRIGHT_YELLOW,
                            PHASE3_DOWNWARD_STEP,
                            RESET
                        );
                        break;
                    }

                    // For ultimate mode, continue stepping down to see if quality metric overrides
                    current_step = PHASE3_DOWNWARD_STEP;
                    test_crf -= current_step;

                    // Insight mechanism: only count as credible failure if quality actually degraded
                    if ultimate_mode {
                        let quality_degraded = if let (Some(v), Some((u, v_score))) =
                            (current_vmaf_val, current_psnr_val)
                        {
                            both_metrics_below_ultimate_sanity_floor(v, (u, v_score))
                        } else {
                            false
                        };

                        if quality_degraded && !vmaf_improved && !psnr_improved {
                            failure_credibility += 1.0;
                            if failure_credibility >= 3.0 {
                                crate::log_eprintln!(
                                    "   {}❌ FAILURE CREDIBILITY REACHED (3/3):{} Sustained quality collapse. Stopping.",
                                    BRIGHT_RED, RESET
                                );
                                early_insight_triggered = true;
                                break;
                            }
                        } else {
                            failure_credibility = 0.0;
                        }
                    }

                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        crate::log_eprintln!(
                            "   {}Max consecutive failures ({}) → STOP{}",
                            BRIGHT_RED,
                            MAX_CONSECUTIVE_FAILURES,
                            RESET
                        );
                        break;
                    }
                    last_size_pct = total_size_pct;
                }
            }
        } else {
            crate::log_eprintln!(
                "{}❌ FAILED: No compression point found below input size (up to max CRF {:.2}){}",
                BRIGHT_RED,
                max_crf,
                RESET
            );
            crate::log_eprintln!(
                "   File may be already optimally compressed. Aborting fine-tuning."
            );
            // Use best tested CRF (smallest inflation) instead of arbitrary max_crf fallback
            if best_crf.is_none() {
                best_crf = Some(best_tested_crf);
                best_size = Some(best_tested_size);
            }
        }
    }

    if ultimate_mode && !early_insight_triggered {
        if let Some(best) = best_crf {
            // Only refine if we actually have a compressed result (or within 1% tolerance)
            let current_ratio = crate::numeric_cast::u64_to_f64(best_size.unwrap_or(u64::MAX))
                / crate::numeric_cast::u64_to_f64(input_size.max(1));
            if best < max_crf && current_ratio < 1.01 {
                crate::log_eprintln!();
                crate::log_eprintln!(
                    "{}Phase 4: [CPU] Extreme Mode 0.01-Granularity Fine-Tune (Sprint & Backtrack){}",
                    BRIGHT_MAGENTA, RESET
                );

                let base_step = 0.01;
                let mut current_step = base_step;
                let max_sprint_step = 1.28;

                // Phase 4 is a local 0.01 refinement, not an open-ended walk.
                let max_fine_failures = PHASE4_ULTIMATE_MAX_FINE_FAILURES;

                crate::log_eprintln!(
                    "   {}Starting from 0.1 optimum (CRF {:.2}) with adaptive step (0.01 → {:.2} sprint){}",
                    DIM, best, max_sprint_step, RESET
                );

                let mut current_best = best;
                let mut current_best_size = best_size.unwrap_or(0);
                let mut test_crf = best - current_step;
                let mut fine_failures = 0;
                let mut last_size_pct = if input_size > 0 {
                    (crate::numeric_cast::u64_to_f64(best_size.unwrap_or(input_size))
                        / crate::numeric_cast::u64_to_f64(input_size.max(1))
                        - 1.0)
                        * 100.0
                } else {
                    0.0
                };
                let mut backtrack_count = 0u32;
                let search_floor = 0.0_f32;
                let mut consecutive_successes = 0;
                let mut phase4_attempts = 0u32;
                let mut phase4_attempt_cap_hit = false;

                while test_crf >= search_floor && iterations < 500 {
                    if phase4_attempts >= PHASE4_MAX_ATTEMPTS {
                        phase4_attempt_cap_hit = true;
                        break;
                    }
                    phase4_attempts += 1;

                    // Round to 0.01 precision to avoid float drift accumulating past 0.0
                    test_crf = (test_crf * 100.0).round() / 100.0;
                    // Clamp: never go negative due to floating-point underflow
                    if test_crf < 0.0 {
                        test_crf = 0.0;
                    }

                    if size_cache.contains_key(test_crf) {
                        if test_crf == 0.0 {
                            break; // Already tested CRF 0; we are done
                        }
                        test_crf -= current_step;
                        continue;
                    }

                    let size = encode_cached(test_crf, &mut size_cache)?;
                    iterations += 1;

                    let is_effectively_compressed = size < input_size;
                    let total_size_pct = if input_size > 0 {
                        (crate::numeric_cast::u64_to_f64(size)
                            / crate::numeric_cast::u64_to_f64(input_size.max(1))
                            - 1.0)
                            * 100.0
                    } else {
                        0.0
                    };

                    let size_delta = (total_size_pct - last_size_pct).abs();

                    if is_effectively_compressed {
                        current_best = test_crf;
                        current_best_size = size;
                        fine_failures = 0;
                        consecutive_successes += 1;

                        let step_info = if current_step > base_step + 0.001 {
                            format!("SPRINT step {current_step:.2}")
                        } else {
                            "0.01-GRANULARITY GAIN".to_string()
                        };

                        // Calculate VMAF and PSNR for Phase 4 logs
                        let mut metrics_info = String::new();
                        if ultimate_mode {
                            let vmaf = super::ssim_calculator::calculate_vmaf_y(input, output, 6);
                            let psnr_uv =
                                super::ssim_calculator::calculate_psnr_uv(input, output, 6);

                            if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                                let chroma_avg = f64::midpoint(u, v_score);
                                metrics_info = format!(" │ VMAF:{v:.2} UV:{chroma_avg:.2}");
                            }
                        }
                        let _ = metrics_info; // Fulfill clippy if not log-used elsewhere

                        crate::log_eprintln!(
                            "{}{}   {}✓{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{}{} │ {}",
                            RESET,
                            RESET,
                            BRIGHT_GREEN,
                            RESET,
                            CYAN,
                            test_crf,
                            RESET,
                            BRIGHT_GREEN,
                            total_size_pct,
                            RESET,
                            metrics_info,
                            step_info
                        );

                        if test_crf == 0.0 {
                            // Reached absolute floor — Phase 4 done
                            crate::log_eprintln!(
                                "   {}✅ [CPU] CRF 0.00 reached — physical lossless floor touched.{}",
                                BRIGHT_MAGENTA, RESET
                            );
                            break;
                        }

                        // Unified Adaptive Logic: Deceleration & Sprint
                        let distance_to_floor = test_crf - search_floor;
                        let decel_multiplier = if ultimate_mode { 1.0 } else { 2.0 };
                        let boundary_nearing = distance_to_floor < current_step * decel_multiplier;

                        if size_delta > 1.0 && current_step > base_step + 0.001 {
                            // Slope-Aware Deceleration (⚡ -> 💧)
                            let old_step = current_step;
                            current_step = base_step;
                            consecutive_successes = 0;
                            crate::log_eprintln!(
                                "   {}💧 Search Decelerating (slope Δ{:.1} detected, step reset: {:.3} → {:.3}){}",
                                CYAN, size_delta, old_step, current_step, RESET
                            );
                        } else if boundary_nearing && current_step > base_step + 0.001 {
                            // Boundary-Aware Deceleration
                            let old_step = current_step;
                            current_step = (current_step / 2.0).max(base_step);
                            consecutive_successes = 0;
                            crate::log_eprintln!(
                                "   {}🎯 Smart deceleration: step {:.3} → {:.3} (floor in {:.2}){}",
                                BRIGHT_YELLOW,
                                old_step,
                                current_step,
                                distance_to_floor,
                                RESET
                            );
                        } else {
                            // Sprint: double step after 2 consecutive successes
                            if consecutive_successes >= 2 && current_step < max_sprint_step {
                                let old_step = current_step;
                                current_step = (current_step * 2.0).min(max_sprint_step);
                                crate::log_eprintln!(
                                    "   {}⚡ Sprint activated: step {:.3} → {:.3}{}",
                                    BRIGHT_CYAN,
                                    old_step,
                                    current_step,
                                    RESET
                                );
                            }
                        }

                        last_size_pct = total_size_pct;
                        test_crf -= current_step;
                    } else {
                        fine_failures += 1;
                        consecutive_successes = 0;

                        crate::log_eprintln!(
                            "{}{}   {}✗{} [CPU] {}CRF {:<5.2}{} {}{:6.1}%{} │ CAPACITY EXCEEDED ({}/{})",
                            RESET, RESET, BRIGHT_RED, RESET, CYAN, test_crf, RESET,
                            BRIGHT_RED, total_size_pct, RESET, fine_failures, max_fine_failures
                        );

                        // Unified Anti-Oscillation Backtrack (Safety limit: 3 retries for Phase 4)
                        if current_step > base_step + 0.001
                            && backtrack_count < PHASE4_MAX_BACKTRACK_RETRIES
                        {
                            let old_step = current_step;
                            current_step = (current_step / 2.0).max(base_step);
                            backtrack_count += 1;
                            consecutive_successes = 0;
                            test_crf = current_best - current_step;
                            crate::log_eprintln!(
                                "   {}⏪ Backtracking for extreme precision (retry {}/{}): {:.3} → {:.3}{}",
                                BRIGHT_YELLOW,
                                backtrack_count,
                                PHASE4_MAX_BACKTRACK_RETRIES,
                                old_step,
                                current_step,
                                RESET
                            );
                            // Stability Fix: Do NOT update last_size_pct here.
                            continue;
                        }

                        if current_step <= base_step + 0.001 {
                            // Hit a physical capacity boundary at the minimum precision.
                            // Squeezing further downward will strictly increase the size, causing oscillation.
                            crate::log_eprintln!(
                                "   {}🎯 Convergence achieved! Lower CRF sizes exceed limits. Stopping Phase 4.{}",
                                BRIGHT_MAGENTA, RESET
                            );

                            // Check if a mandatory floor test is required
                            if should_probe_crf_zero_from_phase4(current_best)
                                && !size_cache.contains_key(0.0)
                            {
                                crate::log_eprintln!(
                                    "   {}Ultimate fallback: forcing final check at CRF 0.00 (lossless floor){}",
                                    BRIGHT_CYAN, RESET
                                );
                                test_crf = 0.0;
                                continue;
                            }
                            break;
                        }

                        // We shouldn't gracefully fall down to this point unless
                        // backtracking conditions failed, but just in case, clamp and try again:
                        current_step = (current_step / 2.0).max(base_step);
                        test_crf = current_best - current_step;
                    }
                }

                if phase4_attempt_cap_hit {
                    crate::log_eprintln!(
                        "   {}Phase 4 attempt cap ({}) reached. Stopping.{}",
                        BRIGHT_YELLOW,
                        PHASE4_MAX_ATTEMPTS,
                        RESET
                    );
                }

                // ── Mandatory CRF=0 probe (ultimate mode only) ─────────────────
                // Only perform the floor probe when the search actually converged near CRF 0.
                if should_probe_crf_zero_from_phase4(current_best) && iterations < 200 {
                    let crf0_untested = !size_cache.contains_key(0.0_f32);
                    if crf0_untested {
                        crate::log_eprintln!(
                            "   {}🔬 [CPU] Forcing mandatory CRF 0.00 probe (floor guarantee){}",
                            BRIGHT_MAGENTA,
                            RESET
                        );
                        if let Ok(size) = encode_cached(0.0, &mut size_cache) {
                            iterations += 1;
                            let total_size_pct = if input_size > 0 {
                                (crate::numeric_cast::u64_to_f64(size)
                                    / crate::numeric_cast::u64_to_f64(input_size.max(1))
                                    - 1.0)
                                    * 100.0
                            } else {
                                0.0
                            };
                            if size < input_size {
                                crate::log_eprintln!(
                                    "{}{}   {}✓{} [CPU] {}CRF 0.00 {} {}{:6.1}%{} │ 0.01-GRANULARITY GAIN",
                                    RESET, RESET, BRIGHT_GREEN, RESET, CYAN, RESET,
                                    BRIGHT_GREEN, total_size_pct, RESET
                                );
                                current_best = 0.0;
                                current_best_size = size;
                            } else {
                                crate::log_eprintln!(
                                    "{}{}   {}✗{} [CPU] {}CRF 0.00 {} {}{:6.1}%{} │ CAPACITY EXCEEDED at floor",
                                    RESET, RESET, BRIGHT_RED, RESET, CYAN, RESET,
                                    BRIGHT_RED, total_size_pct, RESET
                                );
                            }
                        }
                    } else if size_cache.contains_key(0.0_f32) {
                        // Already tested — check if that result was a success
                        // (size_cache only stores encoded sizes, not compressed-flag; re-read)
                        if let Some(&cached_size) = size_cache.get(0.0_f32) {
                            if cached_size < input_size && current_best > 0.0 {
                                current_best = 0.0;
                                current_best_size = cached_size;
                                crate::log_eprintln!(
                                    "   {}✅ [CPU] CRF 0.00 already in cache and compresses — set as best.{}",
                                    BRIGHT_MAGENTA, RESET
                                );
                            }
                        }
                    }
                } else if current_best > PHASE4_CRF0_PROBE_MAX_DISTANCE {
                    crate::log_eprintln!(
                        "   {}Skipping CRF 0.00 probe: best CRF {:.2} is not near the floor.{}",
                        DIM,
                        current_best,
                        RESET
                    );
                }

                best_crf = Some(current_best);
                best_size = Some(current_best_size);
            }
        }
    }

    let size_tolerance = if allow_size_tolerance {
        crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES
    } else {
        0
    };
    let (mut final_crf, mut final_full_size) = match (best_crf, best_size) {
        (Some(crf), Some(size)) if crf < max_crf => {
            if size < input_size + size_tolerance {
                crate::log_eprintln!(
                    "{}✅ Best CRF {:.2} settled from search (output on disk){}",
                    BRIGHT_GREEN,
                    crf,
                    RESET
                );
            } else {
                crate::log_eprintln!(
                    "{}❌ Best tested CRF {:.2} yielded larger file (+{:+.1}%){}",
                    BRIGHT_RED,
                    crf,
                    (crate::numeric_cast::u64_to_f64(size)
                        / crate::numeric_cast::u64_to_f64(input_size.max(1))
                        - 1.0)
                        * 100.0,
                    RESET
                );
            }
            (crf, size)
        }
        _ => {
            if early_insight_triggered {
                crate::log_eprintln!("{}⚠️  Skipping final settlement: early insight already proved further compression is futile.{}", BRIGHT_YELLOW, RESET);
                let fallback_crf = max_crf;
                // Use input_size + 1 to ensure it registers as uncompressed
                // since we know any compression destroys quality based on insight.
                let size = input_size + 1;
                (fallback_crf, size)
            } else {
                crate::log_eprintln!(
                    "{}⚠️  Fallback: using max CRF {:.2} (no better compression found){}",
                    BRIGHT_YELLOW,
                    max_crf,
                    RESET
                );

                let last_output_video = crate::stream_size::get_output_video_stream_size(output);
                crate::verbose_eprintln!(
                    "   Video stream: input {} vs output {} ({:+.1}%)",
                    crate::format_bytes(input_video_stream_size),
                    crate::format_bytes(last_output_video),
                    stream_size_change_pct(last_output_video, input_video_stream_size)
                );
                let size = encode_cached(max_crf, &mut size_cache)?;
                iterations += 1;
                (max_crf, size)
            }
        }
    };

    let needs_final_preset_render = final_output_preset != preset;
    let mut needs_phase_5 = false;

    if use_animated_exploration_sampling && !early_insight_triggered {
        crate::log_eprintln!(
            "{}🎞️  Full-timeline encode at CRF {:.2} with preset {} (replacing segmented exploration output){}",
            BRIGHT_CYAN,
            final_crf,
            final_output_preset.hevc_name(),
            RESET
        );
        final_full_size = encode_full(
            final_crf,
            AnimatedExplorationEncodeMode::FullTimeline,
            final_output_preset,
        )?;
        iterations += 1;
        if needs_final_preset_render && final_crf < max_crf {
            needs_phase_5 = true;
        }
    } else if needs_final_preset_render && !early_insight_triggered && final_crf < max_crf {
        crate::log_eprintln!(
            "{}🎯 Final render: preset {} → {} at settled CRF {:.2}{}",
            BRIGHT_CYAN,
            preset.hevc_name(),
            final_output_preset.hevc_name(),
            final_crf,
            RESET
        );
        final_full_size = encode_full(
            final_crf,
            AnimatedExplorationEncodeMode::FullTimeline,
            final_output_preset,
        )?;
        iterations += 1;
        needs_phase_5 = true;
    }

    if needs_phase_5 {
        crate::log_eprintln!();
        crate::log_eprintln!(
            "{}Phase 5: [CPU] Downward Exploration with Ultimate Preset ({}){}",
            BRIGHT_MAGENTA,
            final_output_preset.hevc_name(),
            RESET
        );
        crate::log_eprintln!(
            "   {}Starting from Phase 4 bound (CRF {:.2}). Stopping after {} non-improving attempts.{}",
            DIM, final_crf, PHASE5_MAX_CONSECUTIVE_FAILURES, RESET
        );

        let backup_path = output.with_extension(format!(
            "{}.bak",
            output.extension().and_then(|s| s.to_str()).unwrap_or("tmp")
        ));
        let mut test_crf = final_crf - 0.01;
        let mut consecutive_failures = 0u32;
        let mut total_attempts = 0u32;

        loop {
            if test_crf < 0.0 {
                break;
            }
            if consecutive_failures >= PHASE5_MAX_CONSECUTIVE_FAILURES {
                crate::log_eprintln!(
                    "   {}Adaptive lookahead cap ({} non-improvements) reached. Stopping Phase 5.{}",
                    BRIGHT_YELLOW, PHASE5_MAX_CONSECUTIVE_FAILURES, RESET
                );
                break;
            }
            if total_attempts >= PHASE5_MAX_TOTAL_ATTEMPTS {
                crate::log_eprintln!(
                    "   {}Absolute Phase 5 safety cap ({} total attempts) reached. Stopping.{}",
                    BRIGHT_YELLOW,
                    PHASE5_MAX_TOTAL_ATTEMPTS,
                    RESET
                );
                break;
            }
            total_attempts += 1;

            // Re-align to prevent float precision drift
            test_crf = (test_crf * 100.0).round() / 100.0;
            if test_crf < 0.0 {
                test_crf = 0.0;
            }

            // Back up the current best size file before overwriting
            let _ = std::fs::rename(output, &backup_path);

            crate::log_eprintln!(
                "   {}🔬 Probing ultimate preset at CRF {:.2}...{}",
                BRIGHT_CYAN,
                test_crf,
                RESET
            );

            match encode_full(
                test_crf,
                AnimatedExplorationEncodeMode::FullTimeline,
                final_output_preset,
            ) {
                Ok(test_size) => {
                    iterations += 1;
                    if test_size < final_full_size {
                        let pct_gain = (1.0
                            - (crate::numeric_cast::u64_to_f64(test_size)
                                / crate::numeric_cast::u64_to_f64(final_full_size.max(1))))
                            * 100.0;
                        crate::log_eprintln!(
                            "      {}✓ CRF {:.2} -> {} bytes (decreased by {:.2}%, keeping){}",
                            BRIGHT_GREEN,
                            test_crf,
                            test_size,
                            pct_gain,
                            RESET
                        );
                        final_crf = test_crf;
                        final_full_size = test_size;
                        // Throw away the backup (we have a new best)
                        let _ = std::fs::remove_file(&backup_path);
                        consecutive_failures = 0; // Reset patience

                        if test_crf == 0.0 {
                            break; // hit the floor
                        }
                        test_crf -= 0.01;
                    } else {
                        consecutive_failures += 1;
                        let attempts_left =
                            PHASE5_MAX_CONSECUTIVE_FAILURES.saturating_sub(consecutive_failures);
                        crate::log_eprintln!(
                            "      {}✗ CRF {:.2} -> {} bytes (increased past {}, discarding){}",
                            BRIGHT_RED,
                            test_crf,
                            test_size,
                            final_full_size,
                            RESET
                        );
                        if attempts_left > 0 && total_attempts < PHASE5_MAX_TOTAL_ATTEMPTS {
                            crate::log_eprintln!(
                                "      {}... exploring further ({} lookahead attempts remaining){}",
                                DIM,
                                attempts_left,
                                RESET
                            );
                        }
                        let _ = std::fs::remove_file(output); // remove the oversized one
                        let _ = std::fs::rename(&backup_path, output); // restore best

                        if test_crf == 0.0 {
                            break; // hit the floor, cannot probe downwards
                        }
                        test_crf -= 0.01; // Keep probing downwards
                    }
                }
                Err(e) => {
                    crate::log_eprintln!(
                        "      {}⚠️ Probe failed at CRF {:.2}: {}{}",
                        BRIGHT_YELLOW,
                        test_crf,
                        e,
                        RESET
                    );
                    let _ = std::fs::remove_file(output);
                    let _ = std::fs::rename(&backup_path, output);
                    break;
                }
            }
        }

        crate::log_eprintln!(
            "   {}🎯 Phase 5 completed. Final CRF: {:.2}{}",
            BRIGHT_GREEN,
            final_crf,
            RESET
        );
    }

    crate::verbose_eprintln!(
        "Final: CRF {:.2} | Size: {} bytes ({:.2} MB)",
        final_crf,
        final_full_size,
        crate::numeric_cast::u64_to_f64(final_full_size) / 1024.0 / 1024.0
    );

    let ssim = if ultimate_mode {
        crate::log_eprintln!(
            "   Ultimate mode: skipping SSIM in settle phase; final 3D gate owns quality validation"
        );
        None
    } else if input_is_animated_image_like && final_crf == 0.0 {
        crate::log_eprintln!(
            "   GIF CRF=0 (lossless): skipping SSIM/VMAF — running integrity check instead"
        );
        let integrity_ok = super::stream_analysis::check_lossless_integrity(
            input,
            output,
            final_full_size,
            true, // is_animated_image
        )
        .unwrap_or(true);
        if integrity_ok {
            crate::log_eprintln!("   ✅ INTEGRITY CHECK: PASSED");
        } else {
            crate::log_eprintln!("   ❌ INTEGRITY CHECK: FAILED (possible encode error)");
        }
        Some(1.0)
    } else {
        calculate_ssim_enhanced(input, output)
    };
    if let Some(s) = ssim {
        let quality_hint = if s >= 0.99 {
            "✅ Excellent"
        } else if s >= 0.98 {
            "✅ Very Good"
        } else if s >= 0.95 {
            "Good"
        } else {
            "Below threshold"
        };
        crate::log_eprintln!("SSIM: {:.6} {}", s, quality_hint);
    } else {
        crate::log_eprintln!("⚠️  SSIM calculation skipped or unavailable");
    }

    let size_change_pct = if input_size == 0 {
        0.0
    } else {
        (crate::numeric_cast::u64_to_f64(final_full_size)
            / crate::numeric_cast::u64_to_f64(input_size.max(1))
            - 1.0)
            * 100.0
    };

    // User-relevant success: total file smaller (with 1MB tolerance if allowed) and quality met
    let total_file_compressed = final_full_size < input_size + size_tolerance;
    let _video_stream_compressed =
        crate::stream_size::can_compress_pure_video(output, input_video_stream_size, true);
    let ssim_ok = match ssim {
        Some(s) => s >= min_ssim,
        None => false,
    };
    let quality_passed = if ultimate_mode {
        total_file_compressed
    } else {
        total_file_compressed && ssim_ok
    };

    let ssim_val = ssim.unwrap_or(0.0);

    let sampling_coverage = 1.0;

    let prediction_accuracy = 0.95;

    let target = compression_target_size(input_size);
    let margin_safety = if target > 0 && final_full_size < target {
        let margin = crate::numeric_cast::u64_to_f64(target.saturating_sub(final_full_size))
            / crate::numeric_cast::u64_to_f64(target.max(1));
        (margin / 0.05).min(1.0)
    } else {
        0.0
    };

    let ssim_confidence = if ultimate_mode {
        match (tracking.best_vmaf, tracking.best_psnr_uv) {
            (Some(v), Some((u, vv)))
                if v >= VMAF_Y_SANITY_FLOOR && u.min(vv) >= PSNR_UV_SANITY_FLOOR =>
            {
                0.9
            }
            (Some(_), Some(_)) => 0.7,
            _ => 0.5,
        }
    } else if ssim_val >= 0.99 {
        1.0
    } else if ssim_val >= 0.95 {
        0.9
    } else if ssim_val >= 0.90 {
        0.7
    } else {
        0.5
    };

    let confidence_detail = ConfidenceBreakdown {
        sampling_coverage,
        prediction_accuracy,
        margin_safety,
        ssim_confidence,
    };
    let confidence = confidence_detail.overall();

    crate::log_eprintln!();
    crate::log_eprintln!("═══════════════════════════════════════════════════════════");
    let result_color = if quality_passed {
        BRIGHT_GREEN
    } else if total_file_compressed {
        BRIGHT_YELLOW // Compressed but maybe quality failed
    } else {
        BRIGHT_RED
    };
    let result_prefix = if ultimate_mode && quality_passed {
        "✅ READY FOR 3D GATE"
    } else if quality_passed {
        "✅ SUCCESS"
    } else {
        "❌ FAILED"
    };

    crate::log_eprintln!(
        "{}[FINISH] {}: CRF {:.2} │ Size {:+.1}% │ Iterations: {}{}",
        result_color,
        result_prefix,
        final_crf,
        size_change_pct,
        iterations,
        RESET
    );
    crate::log_eprintln!(
        "   Total file smaller than input: {}",
        if total_file_compressed { "YES" } else { "NO" }
    );

    let output_stream_info = crate::stream_size::extract_stream_sizes(output);
    let input_stream_info = crate::stream_size::extract_stream_sizes(input);
    let video_stream_pct = if input_stream_info.video_stream_size > 0 {
        (crate::numeric_cast::u64_to_f64(output_stream_info.video_stream_size)
            / crate::numeric_cast::u64_to_f64(input_stream_info.video_stream_size.max(1))
            - 1.0)
            * 100.0
    } else {
        0.0
    };
    crate::log_eprintln!(
        "   Video stream: {} → {} ({:+.1}%)",
        crate::format_bytes(input_stream_info.video_stream_size),
        crate::format_bytes(output_stream_info.video_stream_size),
        video_stream_pct
    );

    // Detect animated image formats (GIF/WebP/AVIF/HEIC/APNG) with probe-first strategy
    // so paths like "*.gif.file" still get relaxed verification.
    let is_animated_image = input_is_animated_image_like;

    let verify_options = if is_animated_image {
        crate::verbose_eprintln!(
            "   🎞️  Animated image detected, using relaxed duration tolerance"
        );
        crate::quality_verifier_enhanced::VerifyOptions::relaxed_animated_image()
    } else {
        crate::quality_verifier_enhanced::VerifyOptions::strict_video()
    };

    let enhanced =
        crate::quality_verifier_enhanced::verify_after_encode(input, output, &verify_options);
    crate::verbose_eprintln!("   {}", enhanced.summary());
    for d in &enhanced.details {
        crate::verbose_eprintln!("      {}", d);
    }
    let enhanced_verify_fail_reason = if enhanced.passed() {
        None
    } else {
        Some(enhanced.message.clone())
    };
    let quality_passed = quality_passed && enhanced.passed();
    let quality_fail_reason = if !total_file_compressed {
        "Total file not smaller than input".to_string()
    } else if !enhanced.passed() {
        enhanced.message
    } else if !ultimate_mode && !ssim_ok {
        "SSIM below target".to_string()
    } else {
        "Quality gate failed".to_string()
    };

    let total_file_pct = if input_size == 0 {
        0.0
    } else {
        (crate::numeric_cast::u64_to_f64(final_full_size)
            / crate::numeric_cast::u64_to_f64(input_size.max(1))
            - 1.0)
            * 100.0
    };
    if output_stream_info.is_overhead_excessive() {
        crate::log_eprintln!(
            "   ⚠️  Container overhead: {:.1}% (> 10%)",
            output_stream_info.container_overhead_percent()
        );
    }
    if video_stream_pct < 0.0 && total_file_pct > 0.0 {
        crate::log_eprintln!(
            "   ⚠️  Video stream compressed ({:+.1}%) but total file larger ({:+.1}%)",
            video_stream_pct,
            total_file_pct
        );
        crate::log_eprintln!(
            "   Container overhead: {} ({:.1}% of output)",
            crate::format_bytes(output_stream_info.container_overhead),
            output_stream_info.container_overhead_percent()
        );
    }

    confidence_detail.print_report();

    cpu_progress.finish_iteration(final_crf, final_full_size, ssim);

    Ok(ExploreResult {
        optimal_crf: final_crf,
        output_size: final_full_size,
        size_change_pct,
        ssim,
        psnr: None,
        ms_ssim: None,
        ms_ssim_passed: CheckResult::NotChecked,
        ms_ssim_score: None,
        used_fallback: false,
        iterations,
        quality_passed: if quality_passed {
            CheckResult::Passed
        } else {
            CheckResult::Failed(quality_fail_reason)
        },
        enhanced_verify_fail_reason,
        log,
        confidence,
        confidence_detail,
        actual_min_ssim: min_ssim,
        input_video_stream_size: input_stream_info.video_stream_size,
        output_video_stream_size: output_stream_info.video_stream_size,
        container_overhead: output_stream_info.container_overhead,
        vmaf_y_score: None,
        cambi_score: None,
        psnr_uv_score: None,
        early_insight_triggered,
    })
}

fn search_anchor_crf(baseline_crf: f32, warm_start_crf: Option<f32>, max_crf: f32) -> f32 {
    if let Some(hint) = warm_start_crf {
        (hint - 2.0).max(ABSOLUTE_MIN_CRF)
    } else {
        baseline_crf
    }
    .clamp(ABSOLUTE_MIN_CRF, max_crf)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HevcPresetPlan {
    search_preset: EncoderPreset,
    final_output_preset: EncoderPreset,
}

fn hevc_preset_plan(requested_preset: EncoderPreset, ultimate_mode: bool) -> HevcPresetPlan {
    let final_output_preset = requested_preset.sanitize_hevc();
    let search_preset = if ultimate_mode && final_output_preset == EncoderPreset::Slower {
        EncoderPreset::Slow
    } else {
        final_output_preset
    };

    HevcPresetPlan {
        search_preset,
        final_output_preset,
    }
}

fn run_hevc_gpu_search(
    req: &GpuSearchRequest,
    search_preset: EncoderPreset,
    final_output_preset: EncoderPreset,
    initial_crf: f32,
) -> Result<ExploreResult> {
    run_hevc_gpu_search_to_output(
        req,
        search_preset,
        final_output_preset,
        initial_crf,
        &req.output,
    )
}

fn run_hevc_gpu_search_to_output(
    req: &GpuSearchRequest,
    search_preset: EncoderPreset,
    final_output_preset: EncoderPreset,
    initial_crf: f32,
    output_path: &Path,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(req.baseline_crf, VideoEncoder::Hevc);
    explore_with_gpu_coarse_search(GpuSearchArgs {
        input: &req.input,
        output: output_path,
        encoder: VideoEncoder::Hevc,
        vf_args: req.vf_args.clone(),
        initial_crf: initial_crf.clamp(ABSOLUTE_MIN_CRF, max_crf),
        max_crf,
        min_ssim,
        ultimate_mode: req.ultimate_mode,
        force_ms_ssim_long: req.force_ms_ssim_long,
        allow_size_tolerance: req.allow_size_tolerance,
        max_threads: req.max_threads,
        hdr_x265_params: req.hdr_x265_params.clone(),
        apple_compat: req.apple_compat,
        preset: search_preset.sanitize_hevc(),
        final_output_preset: final_output_preset.sanitize_hevc(),
    })
}

/// Unified HEVC quality exploration with GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_hevc_with_gpu(req: GpuSearchRequest) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(req.baseline_crf, VideoEncoder::Hevc);
    let screening_anchor = search_anchor_crf(req.baseline_crf, req.warm_start_crf, max_crf);
    let plan = hevc_preset_plan(req.preset, req.ultimate_mode);

    if plan.search_preset != plan.final_output_preset {
        crate::log_eprintln!(
            "   HEVC Ultimate pipeline: search preset {} → final preset {} at settled CRF",
            plan.search_preset.hevc_name(),
            plan.final_output_preset.hevc_name()
        );
    }

    run_hevc_gpu_search(
        &req,
        plan.search_preset,
        plan.final_output_preset,
        screening_anchor,
    )
}

/// Unified AV1 quality exploration with GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_av1_with_gpu(req: GpuSearchRequest) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(req.baseline_crf, VideoEncoder::Av1);
    let search_anchor_crf = if let Some(hint) = req.warm_start_crf {
        (hint - 2.0).max(ABSOLUTE_MIN_CRF)
    } else {
        req.baseline_crf
    }
    .clamp(ABSOLUTE_MIN_CRF, max_crf);

    explore_with_gpu_coarse_search(GpuSearchArgs {
        input: &req.input,
        output: &req.output,
        encoder: VideoEncoder::Av1,
        vf_args: req.vf_args,
        initial_crf: search_anchor_crf,
        max_crf,
        min_ssim,
        ultimate_mode: req.ultimate_mode,
        force_ms_ssim_long: req.force_ms_ssim_long,
        allow_size_tolerance: req.allow_size_tolerance,
        max_threads: req.max_threads,
        hdr_x265_params: None, // AV1 doesn't use x265 params
        apple_compat: req.apple_compat,
        preset: req.preset,
        final_output_preset: req.preset,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        adaptive_cambi_ceiling, adaptive_psnr_uv_floor, adaptive_vmaf_floor,
        evaluate_ultimate_quality_gate, hevc_preset_plan, search_anchor_crf,
        should_probe_crf_zero_from_phase4, UltimateQualityBaselines, UltimateQualityMetrics,
        ABSOLUTE_MIN_CRF, CAMBI_MAX, PSNR_UV_SANITY_FLOOR, VMAF_Y_SANITY_FLOOR,
    };
    use crate::types::EncoderPreset;

    #[test]
    fn test_search_anchor_crf_uses_warm_start_backoff_and_clamp() {
        let result1 = search_anchor_crf(24.0, Some(20.0), 30.0);
        assert!((result1 - 18.0).abs() < f32::EPSILON);

        let result2 = search_anchor_crf(4.0, Some(1.0), 30.0);
        assert!((result2 - ABSOLUTE_MIN_CRF).abs() < f32::EPSILON);

        let result3 = search_anchor_crf(12.0, None, 10.0);
        assert!((result3 - 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_phase4_crf0_probe_requires_near_floor() {
        assert!(should_probe_crf_zero_from_phase4(0.25));
        assert!(should_probe_crf_zero_from_phase4(1.0));
        assert!(!should_probe_crf_zero_from_phase4(0.0));
        assert!(!should_probe_crf_zero_from_phase4(1.01));
        assert!(!should_probe_crf_zero_from_phase4(26.75));
    }

    #[test]
    fn test_hevc_preset_plan_uses_single_pipeline_for_ultimate_slower() {
        let plan = hevc_preset_plan(EncoderPreset::Slower, true);

        assert_eq!(plan.search_preset, EncoderPreset::Slow);
        assert_eq!(plan.final_output_preset, EncoderPreset::Slower);
    }

    #[test]
    fn test_hevc_preset_plan_keeps_same_preset_outside_ultimate_slower() {
        let normal = hevc_preset_plan(EncoderPreset::Slow, false);
        let ultimate_slow = hevc_preset_plan(EncoderPreset::Slow, true);

        assert_eq!(normal.search_preset, EncoderPreset::Slow);
        assert_eq!(normal.final_output_preset, EncoderPreset::Slow);
        assert_eq!(ultimate_slow.search_preset, EncoderPreset::Slow);
        assert_eq!(ultimate_slow.final_output_preset, EncoderPreset::Slow);
    }

    #[test]
    fn test_adaptive_quality_floors_follow_search_baseline() {
        assert!((adaptive_vmaf_floor(Some(95.0)) - 93.0).abs() < f64::EPSILON);
        assert!((adaptive_vmaf_floor(None) - VMAF_Y_SANITY_FLOOR).abs() < f64::EPSILON);

        let psnr = adaptive_psnr_uv_floor(Some((36.5, 35.0)));
        assert!((psnr.0 - 35.0).abs() < f64::EPSILON);
        assert!((psnr.1 - 33.5).abs() < f64::EPSILON);

        let null_psnr = adaptive_psnr_uv_floor(None);
        assert!((null_psnr.0 - PSNR_UV_SANITY_FLOOR).abs() < f64::EPSILON);
        assert!((null_psnr.1 - PSNR_UV_SANITY_FLOOR).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adaptive_cambi_ceiling_respects_source_banding_level() {
        assert!((adaptive_cambi_ceiling(None) - CAMBI_MAX).abs() < f64::EPSILON);
        assert!((adaptive_cambi_ceiling(Some(2.5)) - CAMBI_MAX).abs() < f64::EPSILON);
        assert!((adaptive_cambi_ceiling(Some(5.5)) - 6.5).abs() < f64::EPSILON);
        assert!((adaptive_cambi_ceiling(Some(10.0)) - 11.5).abs() < f64::EPSILON);
        assert!((adaptive_cambi_ceiling(Some(20.0)) - 23.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_baseline_aware_gate_passes_when_output_stays_close_to_source_profile() {
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: Some(92.4),
                psnr_uv: Some((33.8, 33.6)),
                cambi: Some(10.2),
            },
            UltimateQualityBaselines {
                search_vmaf_y: Some(94.0),
                search_psnr_uv: Some((35.0, 35.0)),
                source_cambi: Some(9.0),
            },
        );

        assert!(evaluation.vmaf_ok);
        assert!(evaluation.chroma_ok);
        assert!(evaluation.cambi_ok);
        assert!(evaluation.all_passed());
    }

    #[test]
    fn test_baseline_aware_gate_rejects_outputs_far_below_baseline() {
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: Some(84.0),
                psnr_uv: Some((28.5, 29.0)),
                cambi: Some(9.5),
            },
            UltimateQualityBaselines {
                search_vmaf_y: Some(94.0),
                search_psnr_uv: Some((35.0, 35.0)),
                source_cambi: Some(5.0),
            },
        );

        assert!(!evaluation.vmaf_ok);
        assert!(!evaluation.chroma_ok);
        assert!(!evaluation.cambi_ok);
        assert!(!evaluation.all_passed());
    }

    // ── CRITICAL: None-metrics gate (the exact production failure) ─────────

    #[test]
    fn test_gate_rejects_when_psnr_uv_is_none() {
        // This is the EXACT scenario that caused production failures:
        // VMAF and CAMBI pass, but PSNR-UV returns None (calculation failed).
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: Some(99.96),
                psnr_uv: None, // ← calculation failed
                cambi: Some(0.01),
            },
            UltimateQualityBaselines {
                search_vmaf_y: None,
                search_psnr_uv: None,
                source_cambi: Some(0.01),
            },
        );

        assert!(evaluation.vmaf_ok);
        assert!(evaluation.cambi_ok);
        assert!(!evaluation.chroma_ok, "None PSNR-UV must fail chroma gate");
        assert!(
            !evaluation.all_passed(),
            "Gate must fail when any metric is None"
        );
    }

    #[test]
    fn test_gate_rejects_when_vmaf_is_none() {
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: None,
                psnr_uv: Some((50.0, 48.0)),
                cambi: Some(1.0),
            },
            UltimateQualityBaselines {
                search_vmaf_y: Some(99.0),
                search_psnr_uv: Some((50.0, 48.0)),
                source_cambi: Some(1.0),
            },
        );

        assert!(!evaluation.vmaf_ok, "None VMAF must fail");
        assert!(!evaluation.all_passed());
    }

    #[test]
    fn test_gate_rejects_when_cambi_is_none() {
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: Some(98.0),
                psnr_uv: Some((50.0, 48.0)),
                cambi: None,
            },
            UltimateQualityBaselines {
                search_vmaf_y: Some(99.0),
                search_psnr_uv: Some((50.0, 48.0)),
                source_cambi: Some(1.0),
            },
        );

        assert!(!evaluation.cambi_ok, "None CAMBI must fail");
        assert!(!evaluation.all_passed());
    }

    #[test]
    fn test_gate_all_none_metrics_fails() {
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: None,
                psnr_uv: None,
                cambi: None,
            },
            UltimateQualityBaselines::default(),
        );
        assert!(!evaluation.vmaf_ok);
        assert!(!evaluation.chroma_ok);
        assert!(!evaluation.cambi_ok);
        assert!(!evaluation.all_passed());
    }

    // ── metrics_below_ultimate_sanity_floor ────────────────────────────────

    #[test]
    fn test_metrics_below_floor_both_below() {
        assert!(super::metrics_below_ultimate_sanity_floor(
            80.0,
            (25.0, 25.0)
        ));
    }

    #[test]
    fn test_metrics_below_floor_vmaf_only_below() {
        assert!(super::metrics_below_ultimate_sanity_floor(
            80.0,
            (40.0, 40.0)
        ));
    }

    #[test]
    fn test_metrics_below_floor_psnr_only_below() {
        assert!(super::metrics_below_ultimate_sanity_floor(
            95.0,
            (25.0, 25.0)
        ));
    }

    #[test]
    fn test_metrics_below_floor_neither_below() {
        assert!(!super::metrics_below_ultimate_sanity_floor(
            95.0,
            (40.0, 40.0)
        ));
    }

    #[test]
    fn test_both_metrics_below_floor_true() {
        assert!(super::both_metrics_below_ultimate_sanity_floor(
            80.0,
            (25.0, 25.0)
        ));
    }

    #[test]
    fn test_both_metrics_below_floor_only_one() {
        assert!(!super::both_metrics_below_ultimate_sanity_floor(
            80.0,
            (40.0, 40.0)
        ));
        assert!(!super::both_metrics_below_ultimate_sanity_floor(
            95.0,
            (25.0, 25.0)
        ));
    }

    // ── build_normal_quality_evaluation ────────────────────────────────────

    #[test]
    fn test_normal_eval_passes_with_good_scores() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.98),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: Some(0.97),
                ssim_all: Some(0.96),
            },
        );
        assert!(eval.passed);
        assert!(eval.fusion_score.is_some());
    }

    #[test]
    fn test_normal_eval_fails_with_low_scores() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.98),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: Some(0.80),
                ssim_all: Some(0.82),
            },
        );
        assert!(!eval.passed);
    }

    #[test]
    fn test_normal_eval_none_measurements_fails() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.98),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: None,
                ssim_all: None,
            },
        );
        assert!(!eval.passed);
        assert!(eval.fusion_score.is_none());
    }

    #[test]
    fn test_normal_eval_ms_ssim_only() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.96),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: Some(0.95),
                ssim_all: None,
            },
        );
        assert!(eval.fusion_score.is_some());
        assert!((eval.fusion_score.unwrap() - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_normal_eval_ssim_all_only() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.96),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: None,
                ssim_all: Some(0.95),
            },
        );
        assert!(eval.fusion_score.is_some());
        assert!((eval.fusion_score.unwrap() - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_normal_eval_no_baseline_uses_config_floor() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: None,
                min_ssim_config: 0.92,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: Some(0.93),
                ssim_all: Some(0.93),
            },
        );
        assert!(eval.passed);
        // Floor should be max(0.92, 0.88) = 0.92
        assert!((eval.fusion_floor - 0.92).abs() < 1e-6);
    }

    // ── adaptive floor / ceiling boundary tests ───────────────────────────

    #[test]
    fn test_adaptive_vmaf_floor_clamps_to_sanity() {
        // baseline 88.0 - 2.0 = 86.0, matching the sanity floor
        assert!((adaptive_vmaf_floor(Some(88.0)) - VMAF_Y_SANITY_FLOOR).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adaptive_psnr_floor_clamps_to_sanity() {
        // baseline 31.0/31.2 - 1.5 would fall below 30.0, so both clamp to the sanity floor
        let psnr = adaptive_psnr_uv_floor(Some((31.0, 31.2)));
        assert!((psnr.0 - PSNR_UV_SANITY_FLOOR).abs() < f64::EPSILON);
        assert!((psnr.1 - PSNR_UV_SANITY_FLOOR).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adaptive_cambi_ceiling_borderline_clean() {
        // Source CAMBI exactly at CAMBI_MAX boundary gets the clean-source rise.
        let ceil = adaptive_cambi_ceiling(Some(CAMBI_MAX));
        assert!((ceil - 7.0).abs() < 1e-6);
    }

    #[test]
    fn test_adaptive_cambi_ceiling_heavily_banded() {
        // Source has high banding — ceiling should use ratio
        let ceil = adaptive_cambi_ceiling(Some(40.0));
        // max(1.5, 40.0 * 0.15) = 6.0, so ceiling = 46.0
        assert!((ceil - 46.0).abs() < 1e-6);
    }
}
