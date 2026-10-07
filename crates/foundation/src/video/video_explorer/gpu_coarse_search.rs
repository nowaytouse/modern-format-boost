//! GPU coarse search and CPU fine-tuning for CRF exploration
//!
//! HEVC/AV1 ultimate mode: Search with an efficient preset, then render the
//! final output once with the requested delivery preset.
//!
//! For HEVC ultimate `slower`, the pipeline now uses:
//! - search/exploration: `slow`
//! - final render after CRF settles: `slower`
//!
//! ## Unified Selection Philosophy
//!
//! Final output selection follows the same priorities as the rest of the
//! explorers:
//!
//! 1. **Evidence Gate**: the candidate must belong to the final encoder domain
//! 2. **Size Gate**: output must satisfy the active shared size policy
//! 3. **Quality Gates**: standard → `ms_ssim_passed` / SSIM fusion; ultimate →
//!    `ultimate_quality_passed` (VMAF/CAMBI/PSNR-UV)
//! 4. **Quality Coordinate**: within one domain, lower CRF wins
//! 5. **Size**: compare only as a same-quality tiebreaker
//!
//! Search-preset or sampled-timeline CRF values are locator hints.  A changed
//! preset/timeline establishes fresh anchors and a bracket in the final domain.

use crate::builder_base::ToolBuilder;
use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::Stdio;

use super::calibration;
use super::dynamic_mapping;
use super::precheck;
use super::{
    ABSOLUTE_MIN_CRF, CheckResult, CrfCache, ExploreResult, NORMAL_MAX_WALL_HITS, VideoEncoder,
    bail, calculate_adaptive_max_walls, calculate_max_iterations_for_duration,
    calculate_ms_ssim_yuv, calculate_smart_thresholds, calculate_ssim_all, calculate_ssim_enhanced,
    calculate_zero_gains_for_duration_and_range,
};
use crate::constants::{
    ANIMATED_IMAGE_EXPLORATION_SAMPLING_MIN_DURATION_SECS,
    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION,
    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE, HEAVY_VIDEO_THRESHOLD_SECS,
    LONG_VIDEO_THRESHOLD_SECS, VERY_LONG_VIDEO_THRESHOLD_SECS, VMAF_SKIP_THRESHOLD_SECS,
    VMAF_SKIP_THRESHOLD_ULTIMATE_SECS,
};
use crate::exploration_policy::{
    DomainCoordinate, ProbeOutcome, SizePolicy, TimelineDomain, VideoCodecDomain,
};
use crate::modern_ui::colors::{BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, DIM, GREEN, RESET};
use crate::types::EncoderPreset;

const MAX_CONSECUTIVE_COMPRESSIONS: u32 = crate::constants::GPU_COARSE_MAX_CONSECUTIVE_COMPRESSIONS;
const MAX_CONSECUTIVE_FAILURES: u32 = crate::constants::GPU_COARSE_MAX_CONSECUTIVE_FAILURES;
const PHASE4_ULTIMATE_MAX_FINE_FAILURES: u32 = crate::constants::PHASE4_ULTIMATE_MAX_FINE_FAILURES;
const PHASE4_MAX_BACKTRACK_RETRIES: u32 = crate::constants::PHASE4_MAX_BACKTRACK_RETRIES;
const PHASE4_MAX_ATTEMPTS: u32 = crate::constants::PHASE4_MAX_ATTEMPTS;
/// Maximum number of consecutive failed final-domain probes before retaining
/// the last verified candidate.
const PHASE5_MAX_CONSECUTIVE_FAILURES: u32 = crate::constants::PHASE5_MAX_CONSECUTIVE_FAILURES;
/// Absolute cap for final-domain anchor and bracket probes.
const PHASE5_MAX_TOTAL_ATTEMPTS: u32 = crate::constants::PHASE5_MAX_TOTAL_ATTEMPTS;
const UPWARD_SIZE_STAGNATION_THRESHOLD: u32 =
    crate::constants::GPU_COARSE_UPWARD_SIZE_STAGNATION_THRESHOLD;
const UPWARD_DIRECTION_SWITCH_LIMIT: u32 =
    crate::constants::GPU_COARSE_UPWARD_DIRECTION_SWITCH_LIMIT;

mod crf_ui {
    /// Plain-aware glyphs for CRF probe / phase log lines (M58 — routed via
    /// delivery gate).
    #[inline]
    #[must_use]
    pub(super) fn pass_prefix() -> String {
        crate::media_conversion_gate::ui_icon_pick("✓", "[+]")
    }

    #[inline]
    #[must_use]
    pub(super) fn fail_prefix() -> String {
        crate::media_conversion_gate::ui_icon_pick("✗", "[x]")
    }

    #[inline]
    #[must_use]
    pub(super) fn pass_tag() -> String {
        use crate::modern_ui::symbols::{self, plain};
        crate::media_conversion_gate::ui_icon_pick(symbols::SUCCESS, plain::SUCCESS)
    }

    #[inline]
    #[must_use]
    pub(super) fn fail_tag() -> String {
        use crate::modern_ui::symbols::{self, plain};
        crate::media_conversion_gate::ui_icon_pick(symbols::ERROR, plain::ERROR)
    }
}

#[inline]
#[must_use]
fn crf_pass_prefix() -> String {
    crf_ui::pass_prefix()
}

#[inline]
#[must_use]
fn crf_fail_prefix() -> String {
    crf_ui::fail_prefix()
}

#[inline]
#[must_use]
fn crf_pass_tag() -> String {
    crf_ui::pass_tag()
}

#[inline]
#[must_use]
fn crf_fail_tag() -> String {
    crf_ui::fail_tag()
}

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
    vmaf_floor: Option<f64>,
    psnr_uv_floor: Option<(f64, f64)>,
    cambi_ceiling: Option<f64>,
    vmaf_ok: bool,
    chroma_ok: bool,
    cambi_ok: bool,
}

impl UltimateQualityEvaluation {
    const fn all_passed(self) -> bool {
        self.vmaf_ok && self.chroma_ok && self.cambi_ok
    }
}

fn adaptive_cambi_ceiling(source_baseline: Option<f64>) -> Option<f64> {
    let baseline = source_baseline?;
    Some(if baseline <= crate::constants::EXPLORATION_CAMBI_MAX {
        (baseline + crate::constants::EXPLORATION_CAMBI_CLEAN_ALLOWED_RISE)
            .max(crate::constants::EXPLORATION_CAMBI_MAX)
    } else {
        baseline
            + f64::max(
                crate::constants::EXPLORATION_CAMBI_BANDED_ALLOWED_RISE,
                baseline * crate::constants::EXPLORATION_CAMBI_BANDED_GROWTH_RATIO,
            )
    })
}

fn should_probe_crf_zero_from_phase4(best_crf: f32) -> bool {
    best_crf > 0.0 && best_crf <= crate::constants::EXPLORATION_PHASE4_MAX_DISTANCE
}

fn evaluate_ultimate_quality_gate(
    metrics: UltimateQualityMetrics,
    baselines: UltimateQualityBaselines,
) -> UltimateQualityEvaluation {
    let vmaf_floor = Some(crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR);
    let psnr_uv_floor = Some((
        crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR,
        crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR,
    ));
    let cambi_ceiling = adaptive_cambi_ceiling(baselines.source_cambi);

    let vmaf_ok = match (metrics.vmaf_y, vmaf_floor) {
        (Some(v), Some(floor)) => v >= floor,
        _ => false,
    };
    let cambi_ok = match (metrics.cambi, cambi_ceiling) {
        (Some(c), Some(ceiling)) => c <= ceiling,
        _ => false,
    };
    let chroma_ok = match (metrics.psnr_uv, psnr_uv_floor) {
        (Some((u, v)), Some((floor_u, floor_v))) => u >= floor_u && v >= floor_v,
        _ => false,
    };

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
    /// The pass threshold: baseline-relative drop tolerance, bounded by config
    /// and sanity floors.
    fusion_floor: f64,
    passed: bool,
}

/// Construct a [`NormalQualityEvaluation`] from pre- and post-processing data.
///
/// The pass threshold is derived from the search-phase SSIM baseline so the
/// gate is "tailor-made" per file rather than relying solely on a global
/// absolute floor. When no baseline is available the config floor (or sanity
/// floor) is used instead.
fn build_normal_quality_evaluation(
    baseline: NormalQualityBaseline,
    measurement: NormalQualityMeasurement,
) -> NormalQualityEvaluation {
    let fusion_score = match (measurement.ms_ssim_avg, measurement.ssim_all) {
        (Some(ms), Some(ss)) => Some(
            crate::constants::EXPLORATION_MS_SSIM_WEIGHT
                .mul_add(ms, crate::constants::EXPLORATION_SSIM_ALL_WEIGHT * ss),
        ),
        (Some(ms), None) => Some(ms),
        (None, Some(ss)) => Some(ss),
        (None, None) => None,
    };

    // Use the explore-phase SSIM as the reference: allow a fixed drop below it,
    // but never go below the config floor or the hard sanity floor.
    let fusion_floor = crate::media_conversion_gate::explore_fusion_ssim_floor(
        baseline.explore_ssim,
        baseline.min_ssim_config,
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

/// Build the container-level colour `FFmpeg` arguments from an `FFprobeResult`.
/// This only emits the CICP triple for YUV output encodes; HDR10-family
/// metadata is injected separately through encoder-specific params.
fn build_color_args_from_probe(probe: &crate::ffprobe::FFprobeResult) -> Vec<String> {
    crate::build_yuv_output_ffmpeg_color_args(
        probe.color_space.as_deref(),
        probe.color_transfer.as_deref(),
        probe.color_primaries.as_deref(),
    )
}

/// Return the correct pixel format for encoding: yuv420p10le when HDR metadata
/// or high-bit-depth source precision should be preserved, otherwise yuv420p.
fn pick_pix_fmt(probe: &crate::ffprobe::FFprobeResult) -> &'static str {
    crate::hevc_yuv420_output_pix_fmt(probe)
}

/// Percentage change from input stream size (returns `NaN` when input size is
/// unknown/zero).
#[inline]
fn stream_size_change_pct(output_size: u64, input_size: u64) -> f64 {
    super::calc_change_pct_for_input_size(input_size, output_size)
}

#[derive(Debug, Clone, Default)]
pub struct GpuSearchFlags {
    pub features: GpuSearchFeatures,
    pub validation: GpuSearchValidation,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GpuSearchFeatures {
    pub ultimate_mode: bool,
    pub apple_compat: bool,
    pub archive_mode: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GpuSearchValidation {
    pub force_ms_ssim_long: bool,
    pub allow_size_tolerance: bool,
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
    pub flags: GpuSearchFlags,
    pub max_threads: usize,
    pub hdr_x265_params: Option<String>,
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
    pub flags: GpuSearchFlags,
    pub min_ssim: f64,
    pub max_threads: usize,
    pub hdr_x265_params: Option<String>,
    pub preset: EncoderPreset,
}

#[derive(Debug, Clone, Default)]
struct FineTuneFlags {
    pub features: FineTuneFeatures,
    pub status: FineTuneStatus,
}

#[derive(Debug, Clone, Default)]
struct FineTuneFeatures {
    pub ultimate_mode: bool,
    pub apple_compat: bool,
    pub is_gif_magic: bool,
}

#[derive(Debug, Clone, Default)]
struct FineTuneStatus {
    pub allow_size_tolerance: bool,
    pub gpu_executed: bool,
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
    flags: FineTuneFlags,
    archive_mode: bool,
    max_threads: usize,
    duration: f32,
    probe_info: Option<&'a crate::ffprobe::FFprobeResult>,
    hdr_x265_params: Option<String>,
    preset: EncoderPreset,
    final_output_preset: EncoderPreset,
}

/// Mutable tracking state during search.
///
/// Maintains best-found quality metrics across search phases.
///
/// **Invariants**:
/// - `best_vmaf`: Updated when a new lower CRF improves the search-time VMAF
///   reference
/// - `best_psnr_uv`: Updated when a new lower CRF improves the search-time
///   chroma reference
/// - Both fields are monotonically non-decreasing (once set to a value, never
///   set to worse)
/// - Used only during ultimate mode for baseline-aware gating decisions; not in
///   normal mode
#[derive(Debug, Default, Clone)]
struct TrackingState {
    pub best_vmaf: Option<f64>,
    pub best_psnr_uv: Option<(f64, f64)>,
}

/// Format the `QualityCheck` log line from result; used for logging and unit
/// tests (regression: enhanced failure shows reason, not "pure media not
/// smaller").
///
/// Diagnostic only — exploration accept/reject uses
/// [`ExploreResult::pipeline_acceptable`](crate::ExploreResult::pipeline_acceptable),
/// not this formatter.
pub(crate) fn format_quality_check_line(
    result: &ExploreResult,
    quality_verification_skipped_for_format: bool,
) -> String {
    if result.uses_ultimate_quality_contract() {
        if result.ultimate_quality_passed.is_failed() {
            return crate::media_conversion_gate::explore_quality_check_failed_line(
                result.ultimate_quality_passed.failure_reason(),
                "3D quality gate",
                "ultimate_quality_check",
            );
        }
        if result.ultimate_quality_passed.is_passed() && result.quality_passed.is_passed() {
            return "   QualityCheck: PASSED (3D gate + pure media size target met)".to_string();
        }
        if result.quality_passed.is_failed() {
            return crate::media_conversion_gate::explore_quality_check_failed_line(
                result.quality_passed.failure_reason(),
                "3D gate pending or size target",
                "ultimate_quality_size_check",
            );
        }
        return "   QualityCheck: N/A (3D gate pending Phase 3 verification)".to_string();
    }
    if result.ms_ssim_passed.is_failed() {
        return crate::media_conversion_gate::explore_quality_check_failed_line(
            result.ms_ssim_passed.failure_reason(),
            "quality metrics below target",
            "ms_ssim_quality_check",
        );
    }
    if result.quality_passed.is_passed() {
        return "   QualityCheck: PASSED (quality + pure media size target met)".to_string();
    }
    if result.size_target_met.is_passed() && result.quality_passed.is_skipped() {
        return "   QualityCheck: PASSED (size target met; quality gate not required)".to_string();
    }
    if result.quality_passed.is_failed() {
        return match result.quality_passed.failure_reason() {
            Some(reason) => format!("   QualityCheck: FAILED ({reason})"),
            None => match result.enhanced_verify_fail_reason.as_deref() {
                Some(reason) => format!(
                    "   QualityCheck: FAILED (quality met but enhanced verification failed: \
                     {reason})"
                ),
                None => crate::media_conversion_gate::explore_quality_check_failed_line(
                    None,
                    "quality met but pure media not smaller",
                    "quality_passed_enhanced",
                ),
            },
        };
    }
    if quality_verification_skipped_for_format || result.quality_passed.is_skipped() {
        if quality_verification_skipped_for_format {
            return "   QualityCheck: N/A (GIF/size-only, quality not measured)".to_string();
        }
        return "   QualityCheck: N/A (quality not verified)".to_string();
    }
    "   QualityCheck: FAILED (quality not verified)".to_string()
}

#[derive(Clone, Copy)]
struct CpuSearchPlan {
    min_crf: f32,
    max_crf: f32,
    center_crf: f32,
    gpu_executed: bool,
}

#[derive(Clone, Copy)]
struct CpuCalibration {
    start_crf: f32,
}

struct ExploreSession<'a> {
    input: &'a Path,
    output: &'a Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    flags: GpuSearchFlags,
    max_threads: usize,
    hdr_x265_params: Option<String>,
    preset: EncoderPreset,
    final_output_preset: EncoderPreset,
    input_size: u64,
    input_pure_media_size: u64,
    gpu: crate::gpu_accel::GpuAccel,
    encoder_name: &'static str,
    has_gpu_encoder: bool,
    probe_result: Option<crate::ffprobe::FFprobeResult>,
    duration: f32,
    is_gif_magic: bool,
}

impl<'a> ExploreSession<'a> {
    fn new(args: GpuSearchArgs<'a>) -> Result<Self> {
        let GpuSearchArgs {
            input,
            output,
            encoder,
            vf_args,
            initial_crf,
            max_crf,
            min_ssim,
            flags,
            max_threads,
            hdr_x265_params,
            preset,
            final_output_preset,
        } = args;

        let _ = precheck::run(input)?;
        let input_size = fs::metadata(input)
            .context("Failed to read input file metadata")?
            .len();
        let input_pure_media_size = crate::stream_size::measure_strict_pure_media(input)
            .with_context(|| {
                format!(
                    "Strict pure-media input measurement failed for {}",
                    input.display()
                )
            })?
            .pure_media_size();
        let gpu = crate::gpu_accel::GpuAccel::detect_with_retry();
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
        if crate::progress_mode::is_verbose_mode() {
            gpu.print_detection_info();
        }

        let probe_result = match crate::ffprobe::probe_video(input) {
            Ok(probe) => Some(probe),
            Err(err) => {
                crate::media_conversion_gate::explore_gpu_coarse_audit(
                    "explore_gpu_ffprobe",
                    input,
                    format!(
                        "ffprobe precheck failed for {path}: {err}",
                        path = input.display()
                    ),
                );
                None
            }
        };
        let duration = crate::media_conversion_gate::explore_gpu_sample_duration_optional(
            probe_result.as_ref().and_then(|probe| probe.duration),
            "gpu_coarse_search session",
        )
        .ok_or_else(|| {
            anyhow::anyhow!(
                "GPU coarse search requires measured ffprobe duration for {}",
                input.display()
            )
        })?;
        let hdr_x265_params = if matches!(encoder, VideoEncoder::Hevc) {
            if let Some(probe) = probe_result.as_ref() {
                crate::hdr::merge_hevc_x265_params_from_probe(hdr_x265_params.as_deref(), probe)
            } else {
                hdr_x265_params
            }
        } else {
            hdr_x265_params
        };

        Ok(Self {
            input,
            output,
            encoder,
            vf_args,
            initial_crf,
            max_crf,
            min_ssim,
            flags,
            max_threads,
            hdr_x265_params,
            preset,
            final_output_preset,
            input_size,
            input_pure_media_size,
            gpu,
            encoder_name,
            has_gpu_encoder,
            probe_result,
            duration,
            is_gif_magic: super::stream_analysis::is_gif_magic(input)
                .with_context(|| format!("Failed to probe GIF magic for {}", input.display()))?,
        })
    }

    fn run(self) -> Result<ExploreResult> {
        self.log_start();
        let plan = self.cpu_search_plan()?;
        Self::log_cpu_fine_tune_start(plan);

        let mut tracking = TrackingState::default();
        let mut result =
            cpu_fine_tune_from_gpu_boundary(self.build_fine_tune_args(plan), &mut tracking)?;
        result.log.clear();

        let _early_insight = Self::handle_early_insight(&result, &tracking);

        let quality_verification_skipped_for_format = ExploreQualityVerifier {
            input: self.input,
            output: self.output,
            probe_result: self.probe_result.as_ref(),
            flags: self.flags.clone(),
        }
        .verify(&mut result)?;

        self.append_result_log_lines(&mut result, quality_verification_skipped_for_format)?;
        self.log_gpu_mapping(&result);
        Ok(result.sealed())
    }

    fn log_start(&self) {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_GPU,
            &format!("Smart GPU+CPU Explore v5.1 ({:?})", self.encoder)
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_GPU,
            &format!(
                "Input: {} bytes ({size_mb:.2} MB)",
                self.input_size,
                size_mb =
                    crate::numeric_cast::u64_to_f64(self.input_size) / 1_024.0_f64 / 1_024.0_f64
            )
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_STRATEGY,
            "GPU Coarse → CPU Fine"
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            "GPU finds rough boundary (FAST)"
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            "CPU finds precise CRF (ACCURATE)"
        );
    }

    fn cpu_search_plan(&self) -> Result<CpuSearchPlan> {
        let actual_initial_crf = self.actual_initial_crf();
        match self.gpu_search_plan(actual_initial_crf)? {
            Some(plan) => Ok(plan),
            None => Ok(self.cpu_only_search_plan(actual_initial_crf)),
        }
    }

    fn actual_initial_crf(&self) -> f32 {
        if !self.is_gif_magic {
            return self.initial_crf;
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_SYSTEM,
            "GIF magic bytes detected — using CPU-only exploration"
        );
        if self.flags.features.ultimate_mode {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_SYSTEM,
                "🚀 GIF Lossless-First Path: Probing CRF 0.0 for maximum efficiency"
            );
            0.0
        } else {
            self.initial_crf
        }
    }

    fn gpu_search_plan(&self, actual_initial_crf: f32) -> Result<Option<CpuSearchPlan>> {
        if !(self.gpu.is_available() && self.has_gpu_encoder && self.is_high_complexity()) {
            self.log_gpu_skip_reason(actual_initial_crf);
            return Ok(None);
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            "GPU Coarse Search"
        );
        let gpu_result = self.run_gpu_coarse_phase(actual_initial_crf)?;
        self.derive_cpu_search_plan_from_gpu(&gpu_result, actual_initial_crf)
            .map(Some)
    }

    fn is_high_complexity(&self) -> bool {
        self.bitrate_bps() > crate::constants::GPU_SEARCH_HIGH_COMPLEXITY_BITRATE_THRESHOLD
            && !self.is_gif_magic
    }

    fn bitrate_bps(&self) -> f64 {
        if self.duration > 0.0 {
            (crate::numeric_cast::u64_to_f64(self.input_pure_media_size) * 8.0_f64)
                / f64::from(self.duration)
        } else {
            0.0_f64
        }
    }

    fn log_gpu_skip_reason(&self, actual_initial_crf: f32) {
        if !self.is_high_complexity() {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                &format!(
                    "OPTIMIZATION: Low complexity video ({rate:.1} Mbps <= 5.0 Mbps)",
                    rate = self.bitrate_bps() / 1_000_000.0_f64
                )
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                "Skipping GPU coarse search (CPU is faster for low-bitrate animation/PPT)"
            );
        } else if !self.gpu.is_available() {
            self.gpu.print_detection_info();
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_GPU,
                "FALLBACK: No GPU available (skipping GPU coarse phase)"
            );
        } else {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_GPU,
                &format!(
                    "FALLBACK: No GPU encoder for {:?} (skipping GPU coarse phase)",
                    self.encoder
                )
            );
        }

        // Reserved: skip-reason logging does not currently vary by initial CRF;
        // parameter kept for signature parity with the GPU-executed path.
        let _ = actual_initial_crf;
    }

    fn cpu_only_search_plan(&self, actual_initial_crf: f32) -> CpuSearchPlan {
        let center_crf =
            if self.is_gif_magic && crate::float_compare::approx_eq_crf(actual_initial_crf, 0.0) {
                0.0
            } else {
                actual_initial_crf
            };
        let min_crf = if crate::float_compare::approx_eq_crf(center_crf, 0.0) {
            0.0
        } else {
            ABSOLUTE_MIN_CRF
        };
        CpuSearchPlan {
            min_crf,
            max_crf: self.max_crf,
            center_crf,
            gpu_executed: false,
        }
    }

    fn run_gpu_coarse_phase(
        &self,
        actual_initial_crf: f32,
    ) -> Result<crate::gpu_accel::GpuCoarseResult> {
        let temp_output = self
            .output
            .with_extension(crate::gpu_accel::derive_gpu_temp_extension(self.output));
        let sample_dur = if self.flags.features.ultimate_mode {
            crate::constants::GPU_SAMPLE_DURATION_ULTIMATE
        } else {
            crate::constants::GPU_SAMPLE_DURATION
        };
        let gpu_sample_input_size = if self.duration <= sample_dur {
            self.input_pure_media_size
        } else {
            let ratio = sample_dur / self.duration;
            crate::numeric_cast::f64_to_u64_strict(
                crate::numeric_cast::u64_to_f64(self.input_pure_media_size) * f64::from(ratio),
                "gpu_sample_input_size",
            )
            .ok_or_else(|| anyhow::anyhow!("GPU sample input size calculation overflowed u64"))?
        };
        let gpu_config = crate::gpu_accel::GpuCoarseConfig {
            initial_crf: actual_initial_crf,
            min_crf: 0.0,
            max_crf: self.max_crf,
            step: if self.flags.features.ultimate_mode {
                crate::constants::GPU_SEARCH_ULTIMATE_STEP
            } else {
                crate::constants::GPU_SEARCH_NORMAL_STEP
            },
            max_iterations: crate::constants::GPU_ABSOLUTE_MAX_ITERATIONS,
            ultimate_mode: self.flags.features.ultimate_mode,
            preset: self.preset,
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
            self.input,
            &temp_output,
            self.encoder_name,
            self.input_pure_media_size,
            &gpu_config,
            &self.vf_args,
            Some(&progress_callback),
            Some(&log_callback),
        )?;
        let (final_crf, final_size) = if gpu_result.found_boundary {
            let crf = gpu_result.gpu_boundary_crf.ok_or_else(|| {
                anyhow::anyhow!("GPU search reported boundary found but missing boundary CRF")
            })?;
            let size = gpu_result.gpu_best_size.ok_or_else(|| {
                anyhow::anyhow!("GPU search reported boundary found but missing best size metadata")
            })?;
            (crf, size)
        } else {
            (gpu_config.max_crf, self.input_pure_media_size)
        };
        gpu_progress.finish_iteration(final_crf, final_size, None);
        Ok(gpu_result)
    }

    fn derive_cpu_search_plan_from_gpu(
        &self,
        gpu_result: &crate::gpu_accel::GpuCoarseResult,
        _actual_initial_crf: f32,
    ) -> Result<CpuSearchPlan> {
        if !gpu_result.found_boundary {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_GPU,
                "GPU coarse search: no boundary found, using full CRF range for CPU search"
            );
            return Ok(CpuSearchPlan {
                min_crf: ABSOLUTE_MIN_CRF,
                max_crf: self.max_crf,
                center_crf: self.initial_crf,
                gpu_executed: true,
            });
        }

        let gpu_crf = gpu_result.gpu_boundary_crf.ok_or_else(|| {
            anyhow::anyhow!(
                "Inconsistent GPU result: boundary found but boundary CRF missing in \
                 post-processing"
            )
        })?;
        let gpu_size = gpu_result.gpu_best_size.ok_or_else(|| {
            anyhow::anyhow!(
                "Inconsistent GPU result: boundary found but size missing in post-processing"
            )
        })?;
        let gpu_encoder = self.selected_gpu_encoder().ok_or_else(|| {
            anyhow::anyhow!(
                "GPU encoder became unavailable during calibration; refusing CPU-only fabrication \
                 fallback"
            )
        })?;

        let calibration = self.calibrate_cpu_start(gpu_result, gpu_encoder, gpu_crf, gpu_size)?;
        Self::log_gpu_boundary_details(gpu_result, gpu_crf, calibration.start_crf);
        let (cpu_min, cpu_max) = self.derive_cpu_range(gpu_result, calibration.start_crf);
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            &format!(
                "CPU search range: [{cpu_min:.1}, {cpu_max:.1}] (start: {cpu_start:.1})",
                cpu_start = calibration.start_crf
            )
        );

        Ok(CpuSearchPlan {
            min_crf: cpu_min,
            max_crf: cpu_max,
            center_crf: calibration.start_crf,
            gpu_executed: true,
        })
    }

    const fn selected_gpu_encoder(&self) -> Option<&crate::gpu_accel::GpuEncoder> {
        match self.encoder {
            VideoEncoder::Hevc => self.gpu.get_hevc_encoder(),
            VideoEncoder::Av1 => self.gpu.get_av1_encoder(),
            VideoEncoder::H264 => self.gpu.get_h264_encoder(),
        }
    }

    fn calibrate_cpu_start(
        &self,
        gpu_result: &crate::gpu_accel::GpuCoarseResult,
        gpu_encoder: &crate::gpu_accel::GpuEncoder,
        gpu_crf: f32,
        gpu_size: u64,
    ) -> Result<CpuCalibration> {
        let sample_dur = if self.flags.features.ultimate_mode {
            crate::constants::GPU_SAMPLE_DURATION_ULTIMATE
        } else {
            crate::constants::GPU_SAMPLE_DURATION
        };
        let dynamic_mapper = dynamic_mapping::quick_calibrate(
            self.input,
            self.input_pure_media_size,
            self.encoder,
            &self.vf_args,
            gpu_encoder,
            sample_dur,
            self.flags.features.ultimate_mode,
            self.flags.features.apple_compat,
        )?;
        let mapping = match self.encoder {
            VideoEncoder::Av1 => crate::gpu_accel::CrfMapping::av1(self.gpu.gpu_type),
            VideoEncoder::Hevc | VideoEncoder::H264 => {
                crate::gpu_accel::CrfMapping::hevc(self.gpu.gpu_type)
            }
        };
        let codec_max_crf = match self.encoder {
            VideoEncoder::Av1 => {
                crate::numeric_cast::f64_to_f32_lossy(crate::constants::AV1_CRF_MAX_F64)
            }
            VideoEncoder::Hevc | VideoEncoder::H264 => {
                crate::numeric_cast::f64_to_f32_lossy(crate::constants::HEVC_CRF_MAX_F64)
            }
        };
        let cpu_start = if dynamic_mapper.calibrated {
            dynamic_mapper.print_calibration_report();
            dynamic_mapper
                .gpu_to_cpu(gpu_crf, mapping.offset, codec_max_crf)
                .0
        } else {
            let calibration = calibration::Point::from_gpu_result(
                gpu_crf,
                gpu_size,
                self.input_pure_media_size,
                if self.flags.features.ultimate_mode {
                    None
                } else {
                    gpu_result.gpu_best_ssim
                },
                mapping.offset,
            );
            calibration.print_report(self.input_pure_media_size);
            calibration.predicted_cpu_crf
        };

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DYNAMIC,
            &format!("Dynamic mapping: GPU {gpu_crf:.1} → CPU {cpu_start:.1}")
        );
        Ok(CpuCalibration {
            start_crf: cpu_start,
        })
    }

    fn log_gpu_boundary_details(
        gpu_result: &crate::gpu_accel::GpuCoarseResult,
        gpu_crf: f32,
        cpu_start: f32,
    ) {
        if let Some(ceiling_crf) = gpu_result.quality_ceiling_crf {
            if (ceiling_crf - gpu_crf).abs() < f32::EPSILON {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_GPU,
                    &format!(
                        "GPU Boundary = Quality Ceiling: CRF {gpu_crf:.2} (GPU reached quality \
                         limit, no bloat beyond this point)"
                    )
                );
            } else {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_GPU,
                    &format!("GPU Boundary: CRF {gpu_crf:.2} (stopped before quality ceiling)")
                );
            }
        } else {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_GPU,
                &format!("GPU Boundary: CRF {gpu_crf:.2} (quality ceiling not detected)")
            );
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            &format!(
                "GPU found boundary: CRF {:.2} (fine-tuned: {})",
                gpu_crf, gpu_result.fine_tuned
            )
        );
        if let Some(size) = gpu_result.gpu_best_size {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_GPU,
                &format!("GPU best size: {size} bytes")
            );
        }
        if let Some((ceiling_crf, ceiling_psnr)) = gpu_result
            .quality_ceiling_crf
            .zip(gpu_result.quality_ceiling_psnr)
        {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_GPU_QUALITY,
                &format!(
                    "CRF {ceiling_crf:.2}, PSNR {ceiling_psnr:.2}dB (GPU PSNR plateau, CPU can \
                     still break through)"
                )
            );
        }
        // Reserved: boundary-detail logging reports GPU results only; the CPU start
        // CRF is logged by derive_cpu_range. Parameter kept for call-site symmetry.
        let _ = cpu_start;
    }

    fn derive_cpu_range(
        &self,
        gpu_result: &crate::gpu_accel::GpuCoarseResult,
        cpu_start: f32,
    ) -> (f32, f32) {
        // Ultimate contract: CPU range from GPU CRF/size only — never GPU SSIM.
        if self.flags.features.ultimate_mode {
            if gpu_result.fine_tuned {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_1,
                    "Ultimate mode: GPU fine-tuned — narrow CPU search (SSIM ignored)"
                );
                return (
                    (cpu_start - crate::constants::CPU_SEARCH_NARROW_RANGE).max(ABSOLUTE_MIN_CRF),
                    (cpu_start + crate::constants::CPU_SEARCH_NARROW_RANGE).min(self.max_crf),
                );
            }
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_1,
                "Ultimate mode: CPU search from GPU boundary (VMAF-driven explore; SSIM ignored)"
            );
            return (
                (cpu_start - crate::constants::CPU_SEARCH_NORMAL_RANGE).max(ABSOLUTE_MIN_CRF),
                (cpu_start + 5.0).min(self.max_crf),
            );
        }

        if let Some(ssim) = gpu_result.gpu_best_ssim {
            let quality_hint = if ssim >= crate::constants::GPU_SEARCH_CEILING_SSIM_THRESHOLD {
                "Near GPU ceiling"
            } else if ssim >= crate::constants::GPU_SEARCH_GOOD_SSIM_THRESHOLD {
                "Good"
            } else {
                "Below expected"
            };
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_1,
                &format!("GPU best SSIM: {ssim:.6} {quality_hint}")
            );

            if ssim < crate::constants::GPU_SEARCH_LOW_SSIM_CRITICAL_THRESHOLD {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_1,
                    crate::infra::static_logs::messages::MSG_GPU_LOW_SSIM_EXPAND
                );
                return (
                    ABSOLUTE_MIN_CRF,
                    (cpu_start + crate::constants::CPU_SEARCH_EXTENSION_RANGE).min(self.max_crf),
                );
            }
            if gpu_result.fine_tuned {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_1,
                    crate::infra::static_logs::messages::MSG_GPU_FINE_TUNED_NARROW
                );
                return (
                    (cpu_start - crate::constants::CPU_SEARCH_NARROW_RANGE).max(ABSOLUTE_MIN_CRF),
                    (cpu_start + crate::constants::CPU_SEARCH_NARROW_RANGE).min(self.max_crf),
                );
            }

            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_1,
                &format!(
                    "CPU will achieve SSIM {}+ (GPU max ~{})",
                    crate::constants::SSIM_GRADE_EXCELLENT,
                    crate::constants::GPU_SEARCH_CEILING_SSIM_THRESHOLD
                )
            );
            return (
                (cpu_start - crate::constants::CPU_SEARCH_NORMAL_RANGE).max(ABSOLUTE_MIN_CRF),
                (cpu_start + 5.0).min(self.max_crf),
            );
        }

        if gpu_result.fine_tuned {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                crate::infra::static_logs::messages::MSG_GPU_FINE_TUNED_NARROW
            );
            (
                (cpu_start - 3.0).max(ABSOLUTE_MIN_CRF),
                (cpu_start + 3.0).min(self.max_crf),
            )
        } else {
            (
                (cpu_start - 15.0).max(ABSOLUTE_MIN_CRF),
                (cpu_start + 5.0).min(self.max_crf),
            )
        }
    }

    fn log_cpu_fine_tune_start(plan: CpuSearchPlan) {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            &format!(
                "{} CPU Fine-Tune (0.5→0.1 step)",
                crate::media_conversion_gate::ui_icon_pick("🖥️", "[CPU]")
            )
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            &format!("Starting from GPU boundary: CRF {:.2}", plan.center_crf)
        );
        let clamped = plan.center_crf.clamp(plan.min_crf, plan.max_crf);
        if (clamped - plan.center_crf).abs() > 0.01 {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "CPU start CRF {:.2} clamped to {:.1} (range [{:.1}, {:.1}])",
                    plan.center_crf, clamped, plan.min_crf, plan.max_crf
                )
            );
        }
    }

    fn build_fine_tune_args(&self, plan: CpuSearchPlan) -> FineTuneArgs<'_> {
        FineTuneArgs {
            input: self.input,
            output: self.output,
            encoder: self.encoder,
            vf_args: self.vf_args.clone(),
            gpu_boundary_crf: plan.center_crf.clamp(plan.min_crf, plan.max_crf),
            min_crf: plan.min_crf,
            max_crf: plan.max_crf,
            min_ssim: self.min_ssim,
            flags: FineTuneFlags {
                features: FineTuneFeatures {
                    ultimate_mode: self.flags.features.ultimate_mode,
                    apple_compat: self.flags.features.apple_compat,
                    is_gif_magic: self.is_gif_magic,
                },
                status: FineTuneStatus {
                    allow_size_tolerance: self.flags.validation.allow_size_tolerance,
                    gpu_executed: plan.gpu_executed,
                },
            },
            archive_mode: self.flags.features.archive_mode,
            max_threads: self.max_threads,
            duration: self.duration,
            probe_info: self.probe_result.as_ref(),
            hdr_x265_params: self.hdr_x265_params.clone(),
            preset: self.preset,
            final_output_preset: self.final_output_preset,
        }
    }

    fn handle_early_insight(result: &ExploreResult, tracking: &TrackingState) -> bool {
        if !result.early_insight_triggered {
            return false;
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            &format!(
                "{} Early Insight Triggered: Quality Plateau Detected",
                crate::media_conversion_gate::ui_icon_pick(
                    crate::modern_ui::symbols::WARNING,
                    crate::modern_ui::symbols::plain::WARNING,
                )
            )
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            "No integer-level quality improvement over 3 consecutive iterations"
        );
        if let Some(vmaf) = tracking.best_vmaf {
            let vmaf_pass = vmaf >= crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR;
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "VMAF-Y: {vmaf:.2} {op} {floor:.1} {icon} (sanity floor)",
                    op = if vmaf_pass { "≥" } else { "<" },
                    floor = crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR,
                    icon = crate::modern_ui::symbols::ok_fail_icon(vmaf_pass)
                )
            );
        }
        if let Some((u, v)) = tracking.best_psnr_uv {
            let u_pass = u >= crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR;
            let v_pass = v >= crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR;
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "PSNR-UV: U={u:.2} dB {u_icon}, V={v:.2} dB {v_icon} (sanity floor ≥ \
                     {floor:.1} dB)",
                    u_icon = crate::modern_ui::symbols::ok_fail_icon(u_pass),
                    v_icon = crate::modern_ui::symbols::ok_fail_icon(v_pass),
                    floor = crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR
                )
            );
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            "Quality plateau detected — Phase 3 verification still runs (3D quality gate applies)"
        );
        false
    }

    fn append_result_log_lines(
        &self,
        result: &mut ExploreResult,
        quality_verification_skipped_for_format: bool,
    ) -> Result<()> {
        let output_size_actual = fs::metadata(self.output)
            .with_context(|| {
                format!(
                    "Failed to read GPU output metadata: {}",
                    self.output.display()
                )
            })?
            .len();
        let size_change_line = if result.input_pure_media_size == 0 {
            "   PureMediaChange: N/A (zero input payload)".to_string()
        } else {
            let ratio = crate::numeric_cast::u64_to_f64(result.output_pure_media_size)
                / crate::numeric_cast::u64_to_f64(result.input_pure_media_size);
            let pct = (ratio - 1.0_f64) * 100.0_f64;
            format!("   PureMediaChange: {ratio:.2}x ({pct:+.1}%) vs original payload")
        };
        result.log.push(size_change_line);
        result.log.push(format!(
            "   TotalFile: {} → {}",
            crate::format_bytes(self.input_size),
            crate::format_bytes(output_size_actual)
        ));

        let quality_line = if let Some(summary) = result.ultimate_quality_summary() {
            format!("   Quality: {summary}")
        } else if !result.uses_ultimate_quality_contract()
            && result.ms_ssim_passed.is_failed()
            && result.ms_ssim_score.is_none()
            && result.ssim.is_none()
        {
            "   Quality: N/A (quality check failed)".to_string()
        } else if let Some(score) = result.ms_ssim_score {
            let pct = (score * 100.0 * 10.0).round() / 10.0_f64;
            format!("   Quality: {pct:.1}% (MS-SSIM={score:.4})")
        } else if let Some(ssim) = result.ssim {
            let pct = (ssim * 100.0 * 10.0).round() / 10.0_f64;
            if result.used_fallback {
                format!("   Quality: {pct:.1}% (SSIM~{ssim:.4}, predicted)")
            } else {
                format!("   Quality: {pct:.1}% (SSIM={ssim:.4})")
            }
        } else if result.quality_passed.is_passed()
            && result.ssim.is_none()
            && result.ms_ssim_passed.is_passed()
        {
            "   Quality: passed (lossless integrity gate, SSIM not measured)".to_string()
        } else {
            "   Quality: N/A (quality check failed)".to_string()
        };
        result.log.push(quality_line);
        result.log.push(format_quality_check_line(
            result,
            quality_verification_skipped_for_format,
        ));
        Ok(())
    }

    fn log_gpu_mapping(&self, result: &ExploreResult) {
        if !(self.gpu.is_available() && self.has_gpu_encoder) {
            return;
        }

        let mapping = match self.encoder {
            VideoEncoder::Av1 => crate::gpu_accel::CrfMapping::av1(self.gpu.gpu_type),
            VideoEncoder::Hevc | VideoEncoder::H264 => {
                crate::gpu_accel::CrfMapping::hevc(self.gpu.gpu_type)
            }
        };
        let equivalent_gpu_crf = mapping.cpu_to_gpu(result.optimal_crf);
        let crf_display = if result.optimal_crf < 0.01 {
            format!("{:.2} (Lossless)", result.optimal_crf)
        } else {
            format!("{:.2}", result.optimal_crf)
        };
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_GPU,
            &format!("CRF Mapping: CPU {crf_display} ≈ GPU {equivalent_gpu_crf:.1}")
        );
    }
}

struct ExploreQualityVerifier<'a> {
    input: &'a Path,
    output: &'a Path,
    probe_result: Option<&'a crate::ffprobe::FFprobeResult>,
    flags: GpuSearchFlags,
}

impl ExploreQualityVerifier<'_> {
    fn verify(&self, result: &mut ExploreResult) -> anyhow::Result<bool> {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            "Quality Verification"
        );

        let duration_opt = self.probe_result.and_then(|probe| probe.duration);
        Self::log_duration(duration_opt);

        let is_animated_image = is_animated_image_like_input(self.input, self.probe_result);
        let animated_lossless =
            is_animated_image && crate::float_compare::approx_eq_crf(result.optimal_crf, 0.0);

        if self.flags.features.ultimate_mode {
            if animated_lossless {
                self.verify_animated_lossless(result);
            } else {
                self.run_ultimate_quality_gate(result, duration_opt)?;
            }
            return Ok(false);
        }
        if animated_lossless {
            self.verify_animated_lossless(result);
            return Ok(false);
        }
        if is_animated_image {
            return Ok(self.verify_animated_ssim_all(result));
        }

        Ok(self.verify_standard_video(result, duration_opt))
    }

    fn log_duration(duration_opt: Option<f64>) {
        if let Some(duration) = duration_opt {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!(
                    "Video duration: {duration:.1}s ({min:.1} min)",
                    min = duration / 60.0_f64
                )
            );
        }
    }

    fn verify_animated_lossless(&self, result: &mut ExploreResult) {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            "ANIMATED CRF=0 (lossless): skipping perceptual metrics — running integrity check \
             instead (CRF=0 guarantees YUV bit-exact reproduction)"
        );
        let integrity_ok = match super::stream_analysis::check_lossless_integrity(
            self.input,
            self.output,
            result.output_size,
            true,
        ) {
            Ok(v) => v,
            Err(err) => {
                crate::media_conversion_gate::explore_gpu_coarse_audit(
                    "explore_gpu_integrity",
                    self.input,
                    format!("Integrity check error: {err}"),
                );
                false
            }
        };

        let ok = crate::media_conversion_gate::ui_icon_pick(
            crate::modern_ui::symbols::SUCCESS,
            crate::modern_ui::symbols::plain::SUCCESS,
        );
        let err = crate::media_conversion_gate::ui_icon_pick(
            crate::modern_ui::symbols::ERROR,
            crate::modern_ui::symbols::plain::ERROR,
        );
        if self.flags.features.ultimate_mode {
            result.ultimate_mode = true;
        }
        if integrity_ok {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!("{ok} INTEGRITY CHECK: PASSED")
            );
            if self.flags.features.ultimate_mode {
                result.ultimate_quality_passed = CheckResult::Passed;
            } else {
                result.ms_ssim_passed = CheckResult::Passed;
            }
        } else {
            crate::media_conversion_gate::explore_gpu_coarse_audit(
                "explore_gpu_integrity",
                self.input,
                format!("{err} INTEGRITY CHECK: FAILED (possible encode error)"),
            );
            let fail = CheckResult::Failed("Lossless integrity check failed".into());
            if self.flags.features.ultimate_mode {
                result.ultimate_quality_passed = fail.clone();
            } else {
                result.ms_ssim_passed = fail.clone();
            }
            result.quality_passed = fail;
        }
    }

    fn run_ultimate_quality_gate(
        &self,
        result: &mut ExploreResult,
        duration_hint: Option<f64>,
    ) -> anyhow::Result<()> {
        result.ultimate_mode = true;
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            "Enabling baseline-aware 3D quality gate (Ultimate Mode)..."
        );

        let Some(sample_rate) =
            crate::media_conversion_gate::explore_ultimate_gate_sample_rate_optional(
                duration_hint,
                "gpu coarse ultimate quality gate",
            )
        else {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_quality",
                "Ultimate 3D gate refused: duration hint absent (no forged full-frame sample rate)",
            );
            result.ultimate_quality_passed =
                CheckResult::Failed("ultimate gate: missing duration hint".into());
            result.quality_passed =
                CheckResult::Failed("ultimate gate: missing duration hint".into());
            return Ok(());
        };
        if sample_rate > 1 {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!(
                    "Ultimate gate sampling: 1/{sample_rate} frames (lightweight final \
                     verification)"
                )
            );
        } else {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                "Ultimate gate sampling: full-frame verification"
            );
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            "Measuring final-product VMAF-Y and PSNR-UV (search metrics are telemetry only)..."
        );
        let vmaf_y =
            super::ssim_calculator::calculate_vmaf_y(self.input, self.output, sample_rate)?;
        let psnr_uv =
            super::ssim_calculator::calculate_psnr_uv(self.input, self.output, sample_rate)?;

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            "Measuring source CAMBI baseline..."
        );
        let source_cambi = super::ssim_calculator::calculate_cambi(self.input, sample_rate)?;

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            "Running final CAMBI banding check..."
        );
        let cambi = super::ssim_calculator::calculate_cambi(self.output, sample_rate)?;

        let baselines = UltimateQualityBaselines { source_cambi };
        let metrics = UltimateQualityMetrics {
            vmaf_y,
            psnr_uv,
            cambi,
        };
        let evaluation = evaluate_ultimate_quality_gate(metrics, baselines);

        Self::log_ultimate_quality_metrics(vmaf_y, psnr_uv, cambi, baselines, evaluation);
        Self::apply_ultimate_quality_gate(result, vmaf_y, psnr_uv, cambi, evaluation);
        Ok(())
    }

    fn log_ultimate_quality_metrics(
        vmaf_y: Option<f64>,
        psnr_uv: Option<(f64, f64)>,
        cambi: Option<f64>,
        baselines: UltimateQualityBaselines,
        evaluation: UltimateQualityEvaluation,
    ) {
        match (vmaf_y, evaluation.vmaf_floor) {
            (Some(v), Some(floor)) => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "VMAF-Y: {v:6.2} ≥ {floor:.1} {} (fresh final-product metric)",
                        crate::modern_ui::symbols::ok_fail_icon(evaluation.vmaf_ok),
                    )
                );
            }
            (Some(v), None) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_quality",
                    format!(
                        "VMAF-Y measured {v:.2} but final floor is absent {}",
                        crf_fail_tag()
                    ),
                );
            }
            (None, _) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_quality",
                    format!("VMAF-Y absent (calculation failed) {}", crf_fail_tag()),
                );
            }
        }

        match (cambi, evaluation.cambi_ceiling, baselines.source_cambi) {
            (Some(c), Some(ceiling), Some(base)) => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "CAMBI:  {c:6.2} ≤ {ceiling:.1} {} (source baseline: {base:.2}, \
                         lower=better)",
                        crate::modern_ui::symbols::ok_fail_icon(evaluation.cambi_ok),
                    )
                );
            }
            (Some(c), None, _) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_crf",
                    format!(
                        "CAMBI measured {c:.2} but adaptive ceiling refused (source baseline \
                         absent) {}",
                        crf_fail_tag()
                    ),
                );
            }
            (None, _, _) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_crf",
                    format!("CAMBI absent (calculation failed) {}", crf_fail_tag()),
                );
            }
            (Some(_), Some(_), None) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_crf",
                    format!(
                        "CAMBI gate incomplete: adaptive ceiling present but source baseline \
                         absent {}",
                        crf_fail_tag()
                    ),
                );
            }
        }

        match (psnr_uv, evaluation.psnr_uv_floor) {
            (Some((pu, pv)), Some((f1, f2))) => {
                let u_pass = pu >= f1;
                let v_pass = pv >= f2;
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "PSNR-UV: U={pu:.2} dB {}, V={pv:.2} dB {} (final floors ≥ {f1:.1}/{f2:.1} dB)",
                        crate::modern_ui::symbols::ok_fail_icon(u_pass),
                        crate::modern_ui::symbols::ok_fail_icon(v_pass),
                    )
                );
            }
            (Some((pu, pv)), None) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_quality",
                    format!(
                        "PSNR-UV measured U={pu:.2}/V={pv:.2} dB but final floors are absent {}",
                        crf_fail_tag()
                    ),
                );
            }
            (None, _) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_quality",
                    format!("PSNR-UV absent (calculation failed) {}", crf_fail_tag()),
                );
            }
        }
    }

    fn apply_ultimate_quality_gate(
        result: &mut ExploreResult,
        vmaf_y: Option<f64>,
        psnr_uv: Option<(f64, f64)>,
        cambi: Option<f64>,
        evaluation: UltimateQualityEvaluation,
    ) {
        let ok = crate::media_conversion_gate::ui_icon_pick(
            crate::modern_ui::symbols::SUCCESS,
            crate::modern_ui::symbols::plain::SUCCESS,
        );
        let err = crate::media_conversion_gate::ui_icon_pick(
            crate::modern_ui::symbols::ERROR,
            crate::modern_ui::symbols::plain::ERROR,
        );
        if evaluation.all_passed() {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!("{ok} 3D QUALITY GATE: PASSED")
            );
            result.ultimate_quality_passed = CheckResult::Passed;
            result.quality_passed = CheckResult::Passed;
        } else {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_coarse",
                format!("{err} 3D QUALITY GATE: FAILED"),
            );
            Self::log_ultimate_quality_failures(vmaf_y, psnr_uv, cambi, evaluation);
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                "Suggestion: Lower CRF or disable --compress"
            );
            result.ultimate_quality_passed = CheckResult::Failed("3D quality gate failed".into());
            result.quality_passed = CheckResult::Failed("3D quality gate failed".into());
        }

        result.vmaf_y_score = vmaf_y;
        result.cambi_score = cambi;
        result.psnr_uv_score = psnr_uv;
    }

    fn log_ultimate_quality_failures(
        vmaf_y: Option<f64>,
        psnr_uv: Option<(f64, f64)>,
        cambi: Option<f64>,
        evaluation: UltimateQualityEvaluation,
    ) {
        if !evaluation.vmaf_ok {
            match (vmaf_y, evaluation.vmaf_floor) {
                (Some(v), Some(floor)) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_quality",
                        format!("FAILED VMAF-Y {v:.2} < {floor:.1} (fresh final-product metric)"),
                    );
                }
                (Some(v), None) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_quality",
                        format!("FAILED VMAF-Y gate: measured {v:.2} but final floor is absent"),
                    );
                }
                (None, _) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_quality",
                        "FAILED VMAF-Y gate: metric absent (calculation failed)",
                    );
                }
            }
        }

        if !evaluation.cambi_ok {
            match (cambi, evaluation.cambi_ceiling) {
                (Some(c), Some(ceiling)) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_coarse",
                        format!(
                            "FAILED CAMBI {c:.2} > {ceiling:.1} (above adaptive ceiling from \
                             source baseline)"
                        ),
                    );
                }
                (Some(c), None) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_coarse",
                        format!(
                            "FAILED CAMBI gate: measured {c:.2} but adaptive ceiling refused \
                             (source baseline absent)"
                        ),
                    );
                }
                (None, _) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_coarse",
                        "FAILED CAMBI gate: metric absent (calculation failed)",
                    );
                }
            }
        }

        if !evaluation.chroma_ok {
            match (psnr_uv, evaluation.psnr_uv_floor) {
                (Some((u, v)), Some((f1, f2))) if u.is_finite() && v.is_finite() => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_quality",
                        format!(
                            "FAILED PSNR-UV U={u:.2}/V={v:.2} dB below final floors \
                             {f1:.1}/{f2:.1} dB"
                        ),
                    );
                }
                (Some((u, v)), None) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_quality",
                        format!(
                            "FAILED PSNR-UV gate: measured U={u:.2}/V={v:.2} dB but final \
                             floors are absent"
                        ),
                    );
                }
                (None, _) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_quality",
                        "FAILED PSNR-UV gate: metric absent (calculation failed)",
                    );
                }
                _ => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_quality",
                        "FAILED PSNR-UV gate: non-finite chroma metrics",
                    );
                }
            }
        }
    }

    fn verify_animated_ssim_all(&self, result: &mut ExploreResult) -> bool {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            "Animated input: using SSIM-All verification (ffmpeg ssim filter, GIF-compatible)"
        );

        let ssim_all = match calculate_ssim_all(self.input, self.output) {
            Ok(value) => value,
            Err(err) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_quality",
                    format!("SSIM-All verification failed: {err}"),
                );
                result.quality_passed =
                    CheckResult::Failed("SSIM-All verification failed".to_string());
                return false;
            }
        };

        if let Some((y, u, v, all)) = ssim_all {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!("SSIM Y/U/V/All: {y:.4}/{u:.4}/{v:.4}/{all:.4}")
            );
            let gif_threshold = result
                .actual_min_ssim
                .max(crate::constants::ANIMATED_SSIM_TARGET_MIN);
            if all < gif_threshold {
                crate::media_conversion_gate::explore_gpu_coarse_audit(
                    "explore_gpu_quality",
                    self.input,
                    format!(
                        "{}  SSIM ALL BELOW TARGET! {all:.4} < {gif_threshold:.2}",
                        crf_fail_tag()
                    ),
                );
                result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
            } else {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "{}  SSIM ALL TARGET MET: {all:.4} ≥ {gif_threshold:.2}",
                        crf_pass_tag()
                    )
                );
                result.ms_ssim_passed = CheckResult::Passed;
            }
            result.ms_ssim_score = Some(all);
            false
        } else {
            result.log.push(format!(
                "{} SSIM verification failed (Animated format) - accepting based on size \
                 compression only",
                crate::media_conversion_gate::ui_icon_pick(
                    crate::modern_ui::symbols::WARNING,
                    crate::modern_ui::symbols::plain::WARNING,
                )
            ));
            result.ms_ssim_passed = CheckResult::NotChecked;
            result.ms_ssim_score = None;
            true
        }
    }

    fn verify_standard_video(&self, result: &mut ExploreResult, duration_opt: Option<f64>) -> bool {
        let threshold_secs = self.ms_ssim_duration_threshold_secs();
        if let Some(duration) = duration_opt {
            if duration <= threshold_secs || self.flags.validation.force_ms_ssim_long {
                self.verify_fusion_quality(result, threshold_secs);
                return false;
            }

            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!(
                    "Quality verification: long video (>{limit:.0}min), MS-SSIM skipped. Using \
                     SSIM-All verification only.",
                    limit = threshold_secs / 60.0_f64
                )
            );
            self.verify_ssim_all_only(
                result,
                &format!(
                    "{} ERROR: SSIM All calculation failed (long-video path). Refusing to mark as \
                     passed.",
                    crf_fail_tag()
                ),
            );
            return false;
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            "Using SSIM All verification (includes chroma)..."
        );
        self.verify_ssim_all_only(
            result,
            &format!(
                "{} ERROR: SSIM All calculation failed (no duration path). Refusing to mark as \
                 passed.",
                crf_fail_tag()
            ),
        );
        false
    }

    fn ms_ssim_duration_threshold_secs(&self) -> f64 {
        if self.flags.features.ultimate_mode {
            VMAF_SKIP_THRESHOLD_ULTIMATE_SECS.into()
        } else {
            VMAF_SKIP_THRESHOLD_SECS.into()
        }
    }

    fn verify_fusion_quality(&self, result: &mut ExploreResult, threshold_secs: f64) {
        if self.flags.features.ultimate_mode {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                "Ultimate mode: skipping MS-SSIM/SSIM fusion (3D quality gate owns perceptual \
                 validation)"
            );
            return;
        }
        let threshold_min = threshold_secs / 60.0_f64;
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            &format!(
                "Video within limit (≤{threshold_min:.0}min), enabling fusion quality \
                 verification (MS-SSIM + SSIM)"
            )
        );

        let baseline = NormalQualityBaseline {
            explore_ssim: result.ssim,
            min_ssim_config: result.actual_min_ssim,
        };
        let Ok(measurement) =
            self.collect_fusion_measurement(threshold_secs / 60.0_f64, baseline.explore_ssim)
        else {
            crate::log_failure!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                "Fusion quality verification aborted (MS-SSIM fail-closed)"
            );
            return;
        };
        let evaluation = build_normal_quality_evaluation(baseline, measurement);

        Self::log_fusion_score_details(measurement, evaluation);
        Self::apply_fusion_evaluation(result, baseline, evaluation);
    }

    fn collect_fusion_measurement(
        &self,
        max_duration_min: f64,
        explore_ssim: Option<f64>,
    ) -> anyhow::Result<NormalQualityMeasurement> {
        let ms_ssim_yuv_result = calculate_ms_ssim_yuv(self.input, self.output, max_duration_min)?;
        let ssim_all_result = calculate_ssim_all(self.input, self.output)?;

        let ssim_str =
            crate::media_conversion_gate::ui_f64_or_na(explore_ssim, "gpu_coarse_explore_ssim", 6);
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            &format!("SSIM (explore / pre-processing ref): {ssim_str}")
        );

        let ms_ssim_avg = ms_ssim_yuv_result.map(|(y, u, v, avg)| {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!("MS-SSIM Y/U/V/Avg: {y:.4}/{u:.4}/{v:.4} / {avg:.4}")
            );
            Self::log_chroma_gap("MS-SSIM CHROMA DIFF", y, u, v);
            avg
        });

        let ssim_all = ssim_all_result.map(|(y, u, v, all)| {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!("SSIM Y/U/V/All: {y:.4}/{u:.4}/{v:.4}/{all:.4}")
            );
            Self::log_chroma_gap("SSIM CHROMA LOSS", y, u, v);
            all
        });

        Ok(NormalQualityMeasurement {
            ms_ssim_avg,
            ssim_all,
        })
    }

    fn log_chroma_gap(label: &str, y: f64, u: f64, v: f64) {
        let chroma_loss = (y - u).max(y - v);
        if chroma_loss > 0.02_f64 {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_coarse",
                format!("{label}: Y-U={yu:.4}, Y-V={yv:.4}", yu = y - u, yv = y - v),
            );
        }
    }

    fn log_fusion_score_details(
        measurement: NormalQualityMeasurement,
        evaluation: NormalQualityEvaluation,
    ) {
        match (measurement.ms_ssim_avg, measurement.ssim_all) {
            (Some(ms), Some(ss)) => {
                let score_str = crate::media_conversion_gate::ui_f64_or_na(
                    evaluation.fusion_score,
                    "gpu_coarse_fusion_score",
                    4,
                );
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "FUSION SCORE: {score_str} ({w1:.1}×MS-SSIM + {w2:.1}×SSIM_All = \
                         {w1:.1}×{ms:.4} + {w2:.1}×{ss:.4})",
                        w1 = crate::constants::EXPLORATION_MS_SSIM_WEIGHT,
                        w2 = crate::constants::EXPLORATION_SSIM_ALL_WEIGHT
                    )
                );
            }
            (Some(ms), None) => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "SCORE (MS-SSIM only): {ms:.4} (SSIM All unavailable, using MS-SSIM alone)"
                    )
                );
            }
            (None, Some(ss)) => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "SCORE (SSIM All only): {ss:.4} (MS-SSIM unavailable, using SSIM All \
                         alone)"
                    )
                );
            }
            (None, None) => {}
        }
    }

    fn apply_fusion_evaluation(
        result: &mut ExploreResult,
        baseline: NormalQualityBaseline,
        evaluation: NormalQualityEvaluation,
    ) {
        if let Some(score) = evaluation.fusion_score {
            use crate::infra::static_logs::messages;
            let quality_grade = if score >= crate::constants::SSIM_GRADE_EXCELLENT {
                messages::VAL_EXCELLENT
            } else if score >= crate::constants::SSIM_GRADE_GOOD {
                messages::VAL_VERY_GOOD
            } else if score >= evaluation.fusion_floor {
                messages::VAL_GOOD_MEETS_TARGET
            } else if score >= crate::constants::SSIM_GRADE_FAIR {
                messages::VAL_BELOW_TARGET
            } else {
                messages::VAL_FAILED
            };
            let baseline_note =
                crate::media_conversion_gate::ui_explore_ssim_ref_or_none(baseline.explore_ssim);
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!(
                    "Grade: {quality_grade} (floor: ≥{floor:.4}, pre-processing ref: \
                     {baseline_note})",
                    floor = evaluation.fusion_floor
                )
            );

            let ok = crate::media_conversion_gate::ui_icon_pick(
                crate::modern_ui::symbols::SUCCESS,
                crate::modern_ui::symbols::plain::SUCCESS,
            );
            let err = crate::media_conversion_gate::ui_icon_pick(
                crate::modern_ui::symbols::ERROR,
                crate::modern_ui::symbols::plain::ERROR,
            );
            if evaluation.passed {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "{ok} FUSION SCORE TARGET MET: {score:.4} ≥ {floor:.4}",
                        floor = evaluation.fusion_floor
                    )
                );
                result.ms_ssim_passed = CheckResult::Passed;
            } else {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_size",
                    format!(
                        "{err} FUSION SCORE BELOW TARGET! {score:.4} < {floor:.4} (Quality does \
                         not meet threshold!)",
                        floor = evaluation.fusion_floor
                    ),
                );
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    "Suggestion: Lower CRF or disable --compress"
                );
                result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
            }
            result.ms_ssim_score = Some(score);
            return;
        }

        crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
            "explore_gpu_quality",
            format!(
                "{} ERROR: Fusion verification incomplete (MS-SSIM + SSIM All failed). Refusing \
                 to mark as passed.",
                crate::media_conversion_gate::ui_icon_pick(
                    crate::modern_ui::symbols::ERROR,
                    crate::modern_ui::symbols::plain::ERROR,
                )
            ),
        );
        result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
        result.ms_ssim_score = None;
    }

    fn verify_ssim_all_only(&self, result: &mut ExploreResult, failure_message: &str) {
        if self.flags.features.ultimate_mode {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                "Ultimate mode: skipping SSIM-All verification (3D quality gate owns perceptual \
                 validation)"
            );
            return;
        }
        let ssim_all = match calculate_ssim_all(self.input, self.output) {
            Ok(value) => value,
            Err(err) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_quality",
                    format!("SSIM-All verification failed: {err}"),
                );
                result.quality_passed =
                    CheckResult::Failed("SSIM-All verification failed".to_string());
                return;
            }
        };

        if let Some((y, u, v, all)) = ssim_all {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!("SSIM Y/U/V/All: {y:.4}/{u:.4}/{v:.4}/{all:.4}")
            );

            let baseline = NormalQualityBaseline {
                explore_ssim: result.ssim,
                min_ssim_config: result.actual_min_ssim,
            };
            let evaluation = build_normal_quality_evaluation(
                baseline,
                NormalQualityMeasurement {
                    ms_ssim_avg: None,
                    ssim_all: Some(all),
                },
            );
            let baseline_note =
                crate::media_conversion_gate::ui_explore_ssim_ref_or_none(baseline.explore_ssim);

            let ok = crate::media_conversion_gate::ui_icon_pick(
                crate::modern_ui::symbols::SUCCESS,
                crate::modern_ui::symbols::plain::SUCCESS,
            );
            let err = crate::media_conversion_gate::ui_icon_pick(
                crate::modern_ui::symbols::ERROR,
                crate::modern_ui::symbols::plain::ERROR,
            );
            if evaluation.passed {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "{ok} SSIM ALL TARGET MET: {all:.4} ≥ {floor:.4} (pre-processing ref: \
                         {baseline_note})",
                        floor = evaluation.fusion_floor
                    )
                );
                result.ms_ssim_passed = CheckResult::Passed;
            } else {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_quality",
                    format!(
                        "{err} SSIM ALL BELOW TARGET! {all:.4} < {floor:.4} (pre-processing ref: \
                         {baseline_note})",
                        floor = evaluation.fusion_floor
                    ),
                );
                result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
            }
            result.ms_ssim_score = Some(all);
            return;
        }

        crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
            "explore_gpu_coarse",
            failure_message,
        );
        result.ms_ssim_passed = CheckResult::Failed("SSIM below target".into());
        result.ms_ssim_score = None;
    }
}

/// Explore video quality using GPU coarse search.
///
/// # Errors
/// Returns an error if exploration fails.
// Rationale: This function handles complex, sequential initialization or business logic where
// further fragmentation would hinder readability and maintainability.
/// # Panics
/// Panics if the output path cannot be derived from input path.
pub fn explore(args: GpuSearchArgs<'_>) -> Result<ExploreResult> {
    ExploreSession::new(args)?.run()
}

fn is_image_container(path: &Path) -> bool {
    match crate::image::format_detect::detect_true_format(path) {
        Ok(format) => matches!(
            format,
            crate::image::format_detect::FormatKind::Avif
                | crate::image::format_detect::FormatKind::Heic
                | crate::image::format_detect::FormatKind::Heif
                | crate::image::format_detect::FormatKind::Gif
                | crate::image::format_detect::FormatKind::WebP
                | crate::image::format_detect::FormatKind::Png
                | crate::image::format_detect::FormatKind::Jpeg
                | crate::image::format_detect::FormatKind::Bmp
                | crate::image::format_detect::FormatKind::Tiff
        ),
        Err(error) => {
            crate::media_conversion_gate::probe_layer_audit(
                "gpu_coarse_image_container_failed",
                path,
                format!("failed to detect image container for GPU exploration: {error}"),
            );
            false
        }
    }
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

    match crate::quality_matcher::SourceCodec::identify_by_content(path) {
        Ok(codec) => codec.is_some_and(|codec| codec.can_be_animated()),
        Err(error) => {
            crate::media_conversion_gate::probe_layer_audit(
                "gpu_coarse_animated_format_failed",
                path,
                format!("failed to identify animated input for GPU exploration: {error}"),
            );
            false
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimatedExplorationEncodeMode {
    /// CRF search iterations: three-segment timeline sampling when enabled for
    /// long animated sources.
    ExplorationSample,
    /// One full-length encode at the chosen CRF (deliverable timeline).
    FullTimeline,
}

const fn video_codec_domain(encoder: VideoEncoder) -> VideoCodecDomain {
    match encoder {
        VideoEncoder::Hevc => VideoCodecDomain::Hevc,
        VideoEncoder::Av1 => VideoCodecDomain::Av1,
        VideoEncoder::H264 => VideoCodecDomain::H264,
    }
}

const fn timeline_domain(mode: AnimatedExplorationEncodeMode) -> TimelineDomain {
    match mode {
        AnimatedExplorationEncodeMode::ExplorationSample => TimelineDomain::Sampled,
        AnimatedExplorationEncodeMode::FullTimeline => TimelineDomain::Full,
    }
}

const fn video_domain_coordinate(
    crf: f32,
    encoder: VideoEncoder,
    preset: EncoderPreset,
    mode: AnimatedExplorationEncodeMode,
) -> DomainCoordinate {
    DomainCoordinate::video(
        crf,
        video_codec_domain(encoder),
        preset,
        timeline_domain(mode),
    )
}

fn requires_final_domain_calibration(
    encoder: VideoEncoder,
    search_preset: EncoderPreset,
    search_mode: AnimatedExplorationEncodeMode,
    final_preset: EncoderPreset,
    final_mode: AnimatedExplorationEncodeMode,
) -> bool {
    !video_domain_coordinate(0.0, encoder, search_preset, search_mode).same_unit_as(
        video_domain_coordinate(0.0, encoder, final_preset, final_mode),
    )
}

#[must_use]
fn final_domain_candidate_improves_quality(
    size_policy: SizePolicy,
    source_size: u64,
    current_crf: f32,
    _current_size: u64,
    candidate_crf: f32,
    candidate_size: u64,
) -> bool {
    candidate_crf < current_crf && size_policy.fits(candidate_size, source_size)
}

#[derive(Clone, Copy)]
struct RenderedCandidate {
    crf: f32,
    mode: AnimatedExplorationEncodeMode,
    preset: EncoderPreset,
}

impl RenderedCandidate {
    fn matches(self, crf: f32, mode: AnimatedExplorationEncodeMode, preset: EncoderPreset) -> bool {
        crate::float_compare::approx_eq_crf(self.crf, crf)
            && self.mode == mode
            && self.preset == preset
    }
}

fn candidate_is_materialized(
    rendered_candidate: Option<RenderedCandidate>,
    crf: f32,
    mode: AnimatedExplorationEncodeMode,
    preset: EncoderPreset,
) -> bool {
    rendered_candidate.is_some_and(|rendered| rendered.matches(crf, mode, preset))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FineTuneQualityMode {
    Ultimate,
    Standard,
}

impl FineTuneQualityMode {
    const fn is_ultimate(self) -> bool {
        matches!(self, Self::Ultimate)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamMappingMode {
    ImageOnly,
    AllStreams,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimatedInputProfile {
    AnimatedLike,
    Regular,
}

impl AnimatedInputProfile {
    const fn is_animated_like(self) -> bool {
        matches!(self, Self::AnimatedLike)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GifSourceProfile {
    GifMagic,
    Other,
}

impl GifSourceProfile {
    const fn is_gif_magic(self) -> bool {
        matches!(self, Self::GifMagic)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExplorationSamplingProfile {
    SegmentedSearch,
    FullTimeline,
}

impl ExplorationSamplingProfile {
    const fn uses_segment_sampling(self) -> bool {
        matches!(self, Self::SegmentedSearch)
    }
}

struct FineTuneEncoder<'a> {
    input: &'a Path,
    output: &'a Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    max_threads: usize,
    hdr_x265_params: Option<String>,
    probe_info: Option<&'a crate::ffprobe::FFprobeResult>,
    apple_compat: bool,
    archive_mode: bool,
    input_size: u64,
    duration: f32,
    quality_mode: FineTuneQualityMode,
    stream_mapping: StreamMappingMode,
    animated_input: AnimatedInputProfile,
    gif_source: GifSourceProfile,
    sampling_profile: ExplorationSamplingProfile,
    audio_strategy: AudioTranscodeStrategy,
    pts_integrity: crate::ffprobe_json::PtsIntegrity,
    progress_host: Option<std::sync::Arc<crate::UnifiedProgressBar>>,
}

impl FineTuneEncoder<'_> {
    fn encode_full(
        &self,
        crf: f32,
        mode: AnimatedExplorationEncodeMode,
        encode_preset: EncoderPreset,
    ) -> Result<u64> {
        let apply_segment_vf = self.should_apply_segment_sampling(mode);
        let vf_for_encode = self.vf_args_for_mode(mode);

        let mut builder = crate::ffmpeg_builder::FfmpegBuilder::new();
        self.configure_builder(&mut builder, crf, encode_preset, &vf_for_encode);

        let mut cmd = builder.output(self.output).build();
        cmd.stdout(Stdio::piped());

        let stderr_temp = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "gpu_coarse_stderr_log",
            None,
            Some(".log"),
        )
        .context("Failed to create stderr temp file")?;
        let stderr_path = stderr_temp.path().to_path_buf();
        Self::attach_stderr_capture(&mut cmd, &stderr_temp)?;
        self.ensure_output_parent_exists();

        let mut child = cmd.spawn().context("Failed to spawn ffmpeg")?;
        self.stream_progress(&mut child, crf, apply_segment_vf);

        let status = child.wait().context("Failed to wait for ffmpeg")?;
        self.clear_progress_line();

        if !status.success() {
            let error_detail = Self::read_ffmpeg_error_detail(&stderr_path);
            Self::cleanup_stderr_log(&stderr_path, "after failure");
            anyhow::bail!(
                "{} Encoding failed at CRF {crf:.1}{error_detail}",
                crf_fail_tag()
            );
        }

        Self::cleanup_stderr_log(&stderr_path, "after success");
        self.wait_for_output_size()
    }

    fn should_apply_segment_sampling(&self, mode: AnimatedExplorationEncodeMode) -> bool {
        mode == AnimatedExplorationEncodeMode::ExplorationSample
            && self.sampling_profile.uses_segment_sampling()
    }

    fn vf_args_for_mode(&self, mode: AnimatedExplorationEncodeMode) -> Vec<String> {
        if self.should_apply_segment_sampling(mode) {
            let prefix = animated_exploration_three_segment_vf_prefix(
                f64::from(self.duration),
                self.quality_mode.is_ultimate(),
            );
            merge_vf_with_animated_exploration_prefix(&self.vf_args, &prefix)
        } else {
            self.vf_args.clone()
        }
    }

    fn configure_builder(
        &self,
        builder: &mut crate::ffmpeg_builder::FfmpegBuilder,
        crf: f32,
        encode_preset: EncoderPreset,
        vf_for_encode: &[String],
    ) {
        builder
            .overwrite()
            .arg("-progress")
            .arg("pipe:1")
            .input(self.input);

        self.configure_stream_mapping(builder);
        self.configure_video(builder, crf, encode_preset);
        self.configure_probe_metadata(builder);
        Self::append_filter_args(builder, vf_for_encode);
        self.configure_timeline(builder);
        self.configure_audio(builder);
        self.configure_subtitles(builder);
    }

    fn configure_stream_mapping(&self, builder: &mut crate::ffmpeg_builder::FfmpegBuilder) {
        if self.stream_mapping == StreamMappingMode::ImageOnly {
            builder.arg("-map").arg("0:v");
        } else {
            builder.arg("-map").arg("0");
        }
    }

    fn configure_video(
        &self,
        builder: &mut crate::ffmpeg_builder::FfmpegBuilder,
        crf: f32,
        encode_preset: EncoderPreset,
    ) {
        builder
            .codec_video(self.encoder.ffmpeg_name())
            .arg("-crf")
            .arg(format!("{crf:.2}"));

        let adjusted_x265_params = self.adjusted_x265_params(crf);
        let x265_memory_profile = crate::x265_params::memory_profile_for_source(
            self.probe_info.map(|probe| probe.video_codec.as_str()),
            self.input_size,
        );

        for arg in self.encoder.extra_args_with_preset(
            self.max_threads,
            encode_preset,
            adjusted_x265_params.as_deref(),
            self.apple_compat,
            self.archive_mode,
            x265_memory_profile,
        ) {
            builder.arg(arg);
        }
    }

    fn adjusted_x265_params(&self, crf: f32) -> Option<String> {
        let with_lossless = if crate::float_compare::approx_eq_crf(crf, 0.0)
            && self.encoder == VideoEncoder::Hevc
        {
            let existing = crate::media_conversion_gate::x265_params_segment_or_empty(
                self.hdr_x265_params.as_deref(),
            );
            if existing.is_empty() {
                Some("lossless=1".to_string())
            } else {
                Some(format!("{existing}:lossless=1"))
            }
        } else {
            self.hdr_x265_params.clone()
        };

        let with_bframes = if self.should_disable_bframes() && self.encoder == VideoEncoder::Hevc {
            let existing = crate::media_conversion_gate::x265_params_segment_or_empty(
                with_lossless.as_deref(),
            );
            Some(if existing.is_empty() {
                "bframes=0".to_string()
            } else {
                format!("{existing}:bframes=0")
            })
        } else {
            with_lossless
        };

        self.inject_hdr_metadata(with_bframes)
    }

    fn should_disable_bframes(&self) -> bool {
        let vfr_or_unknown = self
            .probe_info
            .is_none_or(|probe| probe.is_variable_frame_rate);
        self.gif_source.is_gif_magic() || (self.animated_input.is_animated_like() && vfr_or_unknown)
    }

    fn inject_hdr_metadata(&self, params: Option<String>) -> Option<String> {
        if self.encoder != VideoEncoder::Hevc {
            return params;
        }

        let Some(probe) = self.probe_info else {
            return params;
        };

        let base = params.as_deref().or(self.hdr_x265_params.as_deref());
        crate::hdr::merge_hevc_x265_params_from_probe(base, probe)
    }

    fn configure_probe_metadata(&self, builder: &mut crate::ffmpeg_builder::FfmpegBuilder) {
        if let Some(probe) = self.probe_info {
            builder.pix_fmt_str(pick_pix_fmt(probe));
            for arg in build_color_args_from_probe(probe) {
                builder.arg(arg);
            }
        }
    }

    fn append_filter_args(
        builder: &mut crate::ffmpeg_builder::FfmpegBuilder,
        vf_for_encode: &[String],
    ) {
        for arg in vf_for_encode {
            if !arg.is_empty() {
                builder.arg(arg);
            }
        }
    }

    fn configure_timeline(&self, builder: &mut crate::ffmpeg_builder::FfmpegBuilder) {
        if self.pts_integrity == crate::ffprobe_json::PtsIntegrity::Broken {
            builder.arg("-fps_mode").arg("vfr");
        } else {
            builder.arg("-fps_mode").arg("passthrough");
        }

        if self.animated_input.is_animated_like() {
            builder.arg("-video_track_timescale").arg("1000");
        }
    }

    fn configure_audio(&self, builder: &mut crate::ffmpeg_builder::FfmpegBuilder) {
        if self.stream_mapping == StreamMappingMode::ImageOnly {
            builder.codec_audio("none");
            return;
        }

        match &self.audio_strategy {
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

    fn configure_subtitles(&self, builder: &mut crate::ffmpeg_builder::FfmpegBuilder) {
        let Some(probe) = self.probe_info else {
            return;
        };
        if !probe.subtitles.present {
            return;
        }

        let out_ext =
            crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(self.output);
        let container = if out_ext == "mkv" { "mkv" } else { "mp4" };
        let sub_args =
            crate::subtitle_args_for_container(true, probe.subtitles.codec.as_deref(), container);
        for arg in sub_args {
            builder.arg(arg);
        }
    }

    fn attach_stderr_capture(
        cmd: &mut std::process::Command,
        stderr_temp: &tempfile::NamedTempFile,
    ) -> Result<()> {
        let file = stderr_temp
            .reopen()
            .context("failed to open GPU coarse-search stderr capture")?;
        cmd.stderr(file);
        Ok(())
    }

    fn ensure_output_parent_exists(&self) {
        crate::media_conversion_gate::delivery_ensure_output_parent_or_audit(
            "gpu_coarse_output_parent",
            self.output,
        );
    }

    fn stream_progress(&self, child: &mut std::process::Child, crf: f32, apply_segment_vf: bool) {
        let Some(stdout) = child.stdout.take() else {
            return;
        };

        let reader = BufReader::new(stdout);
        let mut last_fps = 0.0_f64;
        let mut last_speed = String::new();
        let mut last_time_us = 0_i64;
        let progress_duration_secs = self.progress_duration_secs(apply_segment_vf);

        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_crf",
                        format!("Failed to read ffmpeg progress stream at CRF {crf:.2}: {err}"),
                    );
                    break;
                }
            };

            if let Some(val) = line.strip_prefix("out_time_us=") {
                match val.parse::<i64>() {
                    Ok(time_us) => last_time_us = time_us,
                    Err(err) => crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_crf",
                        format!("failed to parse ffmpeg out_time_us token {val:?}: {err}"),
                    ),
                }
                continue;
            }
            if let Some(val) = line.strip_prefix("fps=") {
                match val.parse::<f64>() {
                    Ok(fps) => last_fps = fps,
                    Err(err) => crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_crf",
                        format!("failed to parse ffmpeg fps token {val:?}: {err}"),
                    ),
                }
                continue;
            }
            if let Some(val) = line.strip_prefix("speed=") {
                last_speed = val.trim().to_string();
                continue;
            }
            if line == "progress=continue" || line == "progress=end" {
                self.print_progress_line(
                    crf,
                    progress_duration_secs,
                    last_time_us,
                    last_fps,
                    &last_speed,
                );
            }
        }
    }

    fn progress_duration_secs(&self, apply_segment_vf: bool) -> f64 {
        if !apply_segment_vf {
            return f64::from(self.duration);
        }

        let segment_fraction = if self.quality_mode.is_ultimate() {
            ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE
        } else {
            ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION
        };
        (f64::from(self.duration) * 3.0 * segment_fraction).max(0.5)
    }

    fn print_progress_line(
        &self,
        crf: f32,
        progress_duration_secs: f64,
        last_time_us: i64,
        last_fps: f64,
        last_speed: &str,
    ) {
        let current_secs = crate::numeric_cast::i64_to_f64(last_time_us) / 1_000_000.0_f64;
        if progress_duration_secs > 0.0_f64 {
            let pct = (current_secs / progress_duration_secs * 100.0).min(100.0);
            let stage = format!(
                "CRF {crf:.1} | {pct:.1}% | {current_secs:.1}s/{progress_duration_secs:.1}s | \
                 {last_fps:.0}fps | {last_speed}"
            );
            if let Some(progress_host) = &self.progress_host {
                progress_host.set_message(&stage);
            } else {
                eprint!("\r      ⏳ {stage}   ");
                let _ = std::io::stderr().flush();
            }
        }
    }

    fn clear_progress_line(&self) {
        if let Some(progress_host) = &self.progress_host {
            progress_host.set_message("");
        } else {
            eprint!(
                "\r                                                                              \
                 \r"
            );
        }
    }

    fn read_ffmpeg_error_detail(stderr_path: &Path) -> String {
        if !stderr_path.exists() {
            return String::new();
        }

        let stderr_content = match crate::infra::logging::read_bounded_diagnostic_file(stderr_path)
        {
            Ok(content) => content,
            Err(err) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_coarse",
                    format!(
                        "Failed to read GPU coarse-search stderr log at {}: {}",
                        stderr_path.display(),
                        err
                    ),
                );
                return format!(
                    "\n   FFmpeg stderr capture could not be read from {}: {err}",
                    stderr_path.display()
                );
            }
        };

        let error_lines: Vec<&str> = stderr_content
            .lines()
            .filter(|line| {
                line.contains("Error")
                    || line.contains("error")
                    || line.contains("Invalid")
                    || line.contains("failed")
            })
            .collect();

        if !error_lines.is_empty() {
            return format!("\n   FFmpeg error: {}", error_lines.join("\n   "));
        }

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
    }

    fn cleanup_stderr_log(stderr_path: &Path, _phase: &str) {
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "gpu_coarse_stderr_log",
            stderr_path,
        );
    }

    fn wait_for_output_size(&self) -> Result<u64> {
        let mut metadata_retry = 0_i32;
        let mut output_ready = false;
        let mut last_metadata_err = None;
        while metadata_retry < 5_i32 {
            match fs::metadata(self.output) {
                Ok(metadata) if metadata.len() > 0 => {
                    output_ready = true;
                    break;
                }
                Ok(_) => {}
                Err(err) => {
                    last_metadata_err = Some(err);
                }
            }
            metadata_retry += 1_i32;
            if metadata_retry < 5_i32 {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }

        if !output_ready {
            anyhow::bail!(
                "{} FFmpeg reported success but output is missing or empty: {}. metadata error: {}",
                crf_fail_tag(),
                self.output.display(),
                last_metadata_err
                    .as_ref()
                    .map_or_else(|| "not recorded".to_string(), ToString::to_string)
            );
        }

        Ok(crate::stream_size::measure_strict_pure_media(self.output)
            .with_context(|| {
                format!(
                    "Strict pure-media output measurement failed for {}",
                    self.output.display()
                )
            })?
            .pure_media_size())
    }
}

/// `FFmpeg` `-vf` prefix: keep frames in three windows (start / mid / end) and
/// reset PTS for encode.
#[must_use]
fn animated_exploration_three_segment_vf_prefix(dur: f64, ultimate_mode: bool) -> String {
    let segment_pct = if ultimate_mode {
        ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE
    } else {
        ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION
    };
    let start_end = dur * segment_pct;
    let mid_start = dur * (0.5_f64 - segment_pct / 2.0_f64);
    let mid_end = dur * (0.5_f64 + segment_pct / 2.0_f64);
    let tail_start = dur * (1.0_f64 - segment_pct);
    format!(
        "select='lt(t\\,{start_end:.3})+between(t\\,{mid_start:.3}\\,{mid_end:.3})+gte(t\\,\
         {tail_start:.3})',setpts=N/FRAME_RATE/TB"
    )
}

/// Prepends `prefix` to the filter chain after `-vf`, or builds `-vf prefix`
/// when no `-vf` pair exists.
#[must_use]
fn merge_vf_with_animated_exploration_prefix(vf_args: &[String], prefix: &str) -> Vec<String> {
    if vf_args.len() >= 2 && vf_args.first().is_some_and(|s| s == "-vf") {
        let Some(vf_chain) = vf_args.get(1) else {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_vf",
                format!(
                    "Animated exploration vf merge: expected filter chain after -vf (len={})",
                    vf_args.len()
                ),
            );
            return vec!["-vf".to_string(), prefix.to_string()];
        };
        let merged = format!("{prefix},{vf_chain}");
        vec!["-vf".to_string(), merged]
    } else {
        vec!["-vf".to_string(), prefix.to_string()]
    }
}

fn cpu_fine_tune_from_gpu_boundary(
    args: FineTuneArgs<'_>,
    tracking: &mut TrackingState,
) -> Result<ExploreResult> {
    CpuFineTuneSession::new(args, tracking)?.run()
}

// ======================================================================
//  CPU fine-tune session
// ----------------------------------------------------------------------
//  Former `cpu_fine_tune_from_gpu_boundary` was a 2.5k-line state machine.
//  It is now decomposed into an explicit `CpuFineTuneSession` where each
//  phase (boundary verification, compressed-path walk, uncompressed-path
//  exploration, Phase 4 refinement, Phase 5 preset upgrade, finalization)
//  owns its logic but shares progress/iteration state via the session.
// ======================================================================

/// Outcome of the GPU boundary verification phase.
/// Drives which of the two downstream search branches runs.
enum BoundaryOutcome {
    /// GPU boundary already produced a smaller file — walk downward
    /// from it to discover the quality wall.
    Compressed {
        gpu_size: u64,
        gpu_ssim: Option<f64>,
    },
    /// GPU boundary inflated the file — orbit upward to find any
    /// compression point before refining downward.
    Uncompressed { gpu_size: u64, gpu_pct: f64 },
}

#[allow(clippy::struct_excessive_bools)]
struct CpuFineTuneSession<'a> {
    // ---- Inputs (immutable) ----------------------------------------
    input: &'a Path,
    output: &'a Path,
    encoder: VideoEncoder,
    gpu_boundary_crf: f32,
    min_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    duration: f32,
    preset: EncoderPreset,
    final_output_preset: EncoderPreset,
    ultimate_mode: bool,
    archive_mode: bool,
    is_gif_magic: bool,
    allow_size_tolerance: bool,
    gpu_executed: bool,

    // ---- Derived from inputs (immutable) ---------------------------
    input_pure_media_size: u64,
    input_is_animated_image_like: bool,
    exploration_mode: AnimatedExplorationEncodeMode,

    // ---- Owned components ------------------------------------------
    fine_tune_encoder: FineTuneEncoder<'a>,
    cpu_progress: std::sync::Arc<crate::UnifiedProgressBar>,

    // ---- Mutable state threaded across phases ----------------------
    size_cache: CrfCache<u64>,
    iterations: u32,
    best_crf: Option<f32>,
    best_size: Option<u64>,
    rendered_candidate: Option<RenderedCandidate>,
    early_insight_triggered: bool,
    prefer_compat_ssim_mode: bool,
    tracking: &'a mut TrackingState,
}

impl<'a> CpuFineTuneSession<'a> {
    const STEP_UPWARD: f32 = 0.25;

    fn new(args: FineTuneArgs<'a>, tracking: &'a mut TrackingState) -> Result<Self> {
        let FineTuneArgs {
            input,
            output,
            encoder,
            vf_args,
            gpu_boundary_crf,
            min_crf,
            max_crf,
            min_ssim,
            flags,
            archive_mode,
            max_threads,
            duration,
            probe_info,
            hdr_x265_params,
            preset,
            final_output_preset,
        } = args;

        let FineTuneFlags {
            features:
                FineTuneFeatures {
                    ultimate_mode,
                    apple_compat,
                    is_gif_magic,
                },
            status:
                FineTuneStatus {
                    allow_size_tolerance,
                    gpu_executed,
                },
        } = flags;

        let input_size = fs::metadata(input)
            .context("Failed to read input file metadata")?
            .len();

        // Image containers (AVIF, HEIC, GIF, WebP, …) have no audio streams.
        // Mapping all streams (-map 0) causes FFmpeg libx265 to fail with
        // "Not yet implemented in FFmpeg, patches welcome".
        let input_is_image = is_image_container(input);
        let input_is_animated_image_like = is_animated_image_like_input(input, probe_info);

        let input_measurement =
            crate::stream_size::measure_strict_pure_media(input).with_context(|| {
                format!(
                    "Strict pure-media input measurement failed for {}",
                    input.display()
                )
            })?;
        let input_pure_media_size = input_measurement.pure_media_size();
        let pts_integrity = crate::ffprobe_json::check_pts_integrity(input)?;
        if pts_integrity != crate::ffprobe_json::PtsIntegrity::Healthy {
            let msg = if pts_integrity == crate::ffprobe_json::PtsIntegrity::Broken {
                "Broken PTS input"
            } else {
                "Duplicate PTS input"
            };
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_integrity",
                format!("{msg}: {pts_integrity:?}, applying safety measures"),
            );
        }

        let use_animated_exploration_sampling = input_is_animated_image_like
            && duration > ANIMATED_IMAGE_EXPLORATION_SAMPLING_MIN_DURATION_SECS;
        if use_animated_exploration_sampling {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                &format!(
                    "Long animated source ({duration:.1}s > \
                     {ANIMATED_IMAGE_EXPLORATION_SAMPLING_MIN_DURATION_SECS:.1}s): CPU CRF search \
                     uses 3-segment timeline sampling; one full-length encode follows before \
                     quality checks."
                )
            );
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DETECTION,
            &format!(
                "Input pure media: {pure_size} (total file: {total_size}, container/metadata: {overhead})",
                pure_size = crate::modern_ui::format_size(input_pure_media_size),
                total_size = crate::modern_ui::format_size(input_size),
                overhead =
                    crate::modern_ui::format_size(input_size.saturating_sub(input_pure_media_size))
            )
        );

        let estimated_iterations = if ultimate_mode {
            let crf_range = max_crf - min_crf;
            let adaptive_walls = calculate_adaptive_max_walls(crf_range)?;
            u64::from(adaptive_walls + 10)
        } else {
            15
        };
        let cpu_progress = crate::UnifiedProgressBar::new_iteration(
            "[CPU] Fine-Tune",
            input_pure_media_size,
            estimated_iterations,
        );

        let audio_strategy = Self::resolve_audio_strategy(output, probe_info);

        let fine_tune_encoder = FineTuneEncoder {
            input,
            output,
            encoder,
            vf_args,
            max_threads,
            hdr_x265_params,
            probe_info,
            apple_compat,
            archive_mode,
            input_size,
            duration,
            quality_mode: if ultimate_mode {
                FineTuneQualityMode::Ultimate
            } else {
                FineTuneQualityMode::Standard
            },
            stream_mapping: if input_is_image {
                StreamMappingMode::ImageOnly
            } else {
                StreamMappingMode::AllStreams
            },
            animated_input: if input_is_animated_image_like {
                AnimatedInputProfile::AnimatedLike
            } else {
                AnimatedInputProfile::Regular
            },
            gif_source: if is_gif_magic {
                GifSourceProfile::GifMagic
            } else {
                GifSourceProfile::Other
            },
            sampling_profile: if use_animated_exploration_sampling {
                ExplorationSamplingProfile::SegmentedSearch
            } else {
                ExplorationSamplingProfile::FullTimeline
            },
            audio_strategy,
            pts_integrity,
            progress_host: Some(cpu_progress.clone()),
        };

        let cpu_fine_tune_title = if ultimate_mode {
            "CPU Fine-Tune - Ultimate 3D Search"
        } else {
            "CPU Fine-Tune - Maximum SSIM Search"
        };
        let search_goal = if ultimate_mode {
            "Goal: Optimal Compression"
        } else {
            "Goal: Target Quality"
        };
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            &format!(
                "{cpu_fine_tune_title} ({encoder:?}) | Input: {input_sz} ({input_size} bytes) | \
                 {search_goal}",
                input_sz = crate::modern_ui::format_size(input_size)
            )
        );

        let exploration_mode = if use_animated_exploration_sampling {
            AnimatedExplorationEncodeMode::ExplorationSample
        } else {
            AnimatedExplorationEncodeMode::FullTimeline
        };

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            &format!(
                "Step: {step:.2} | GPU boundary: CRF {gpu_boundary_crf:.2} | Strategy: Marginal \
                 benefit analysis",
                step = Self::STEP_UPWARD
            )
        );

        Ok(Self {
            input,
            output,
            encoder,
            gpu_boundary_crf,
            min_crf,
            max_crf,
            min_ssim,
            duration,
            preset,
            final_output_preset,
            ultimate_mode,
            archive_mode,
            is_gif_magic,
            allow_size_tolerance,
            gpu_executed,
            input_pure_media_size,
            input_is_animated_image_like,
            exploration_mode,
            fine_tune_encoder,
            cpu_progress,
            size_cache: CrfCache::new(),
            iterations: 0,
            best_crf: None,
            best_size: None,
            rendered_candidate: None,
            early_insight_triggered: false,
            prefer_compat_ssim_mode: false,
            tracking,
        })
    }

    fn resolve_audio_strategy(
        output: &Path,
        probe_info: Option<&crate::ffprobe::FFprobeResult>,
    ) -> AudioTranscodeStrategy {
        let output_ext =
            crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(output);
        let is_mov_mp4 = output_ext == "mov" || output_ext == "mp4" || output_ext == "m4v";
        if !is_mov_mp4 {
            return AudioTranscodeStrategy::Copy;
        }

        let audio_codec = crate::media_conversion_gate::probe_ffprobe_codec_name_lowercase(
            probe_info.and_then(|info| info.audio.codec.as_deref()),
            "gpu coarse audio strategy",
        );
        let Some(audio_bitrate) =
            crate::media_conversion_gate::explore_gpu_coarse_audio_bitrate_optional(
                probe_info.and_then(|info| info.audio.bit_rate),
            )
        else {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "audio_bitrate_absent",
                "cannot classify audio encode without measured bit_rate; using Copy",
            );
            return AudioTranscodeStrategy::Copy;
        };

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
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                &format!(
                    "High-quality audio detected ({rate}kbps {codec}), using ALAC (lossless)",
                    rate = audio_bitrate / 1000,
                    codec = audio_codec
                )
            );
            AudioTranscodeStrategy::Alac
        } else if audio_bitrate >= crate::constants::GPU_COARSE_SEARCH_DEFAULT_AUDIO_BITRATE {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                &format!(
                    "Medium-quality audio ({rate}kbps {codec}), using AAC 256k",
                    rate = audio_bitrate / 1000,
                    codec = audio_codec
                )
            );
            AudioTranscodeStrategy::AacHigh
        } else {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                &format!(
                    "Audio codec '{audio_codec}' incompatible with {ext}, using AAC 192k",
                    ext = output_ext.to_uppercase()
                )
            );
            AudioTranscodeStrategy::AacMedium
        }
    }

    fn encode_cached(&mut self, crf: f32) -> Result<u64> {
        if let Some(&size) = self.size_cache.get(crf)
            && candidate_is_materialized(
                self.rendered_candidate,
                crf,
                self.exploration_mode,
                self.preset,
            )
        {
            return Ok(size);
        }
        let size = self
            .fine_tune_encoder
            .encode_full(crf, self.exploration_mode, self.preset)?;
        self.rendered_candidate = Some(RenderedCandidate {
            crf,
            mode: self.exploration_mode,
            preset: self.preset,
        });
        self.size_cache.insert(crf, size);
        self.iterations += 1;
        self.cpu_progress.inc_iteration(crf, size, None);
        Ok(size)
    }

    fn pure_media_size_pct(&self, size: u64) -> f64 {
        super::calc_change_pct_for_input_size(self.input_pure_media_size, size)
    }

    const fn size_policy(&self) -> SizePolicy {
        SizePolicy::strict_or_allow_growth(
            self.allow_size_tolerance,
            crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
        )
    }

    const fn candidate_fits(&self, size: u64) -> bool {
        self.size_policy().fits(size, self.input_pure_media_size)
    }

    fn calculate_ssim_quick(&mut self) -> anyhow::Result<Option<f64>> {
        // For GIF/WebP/AVIF/HEIC-like sources, once quick SSIM fails once,
        // switch to robust SSIM-All path for stable baseline/iteration metrics.
        if self.prefer_compat_ssim_mode {
            return Ok(calculate_ssim_all(self.input, self.output)?.map(|(_, _, _, all)| all));
        }

        let filters = [
            "[0:v]scale=\"iw-mod(iw,2)\":\"ih-mod(ih,2)\":flags=bicubic[ref];[ref][1:v]ssim",
            "[0:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:\
             trunc(ih/2)*2,format=yuv420p[cmp];[ref][cmp]ssim",
            "ssim",
        ];

        for filter in &filters {
            let ssim_output = crate::ffmpeg_builder::FfmpegBuilder::new()
                .input(self.input)
                .input(self.output)
                .arg("-lavfi")
                .arg(filter)
                .arg("-f")
                .arg("null")
                .output_pipe()
                .build()
                .output();

            match ssim_output {
                Ok(out) if out.status.success() => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if let Some(line) = stderr.lines().find(|l| l.contains("All:"))
                        && let Some(all_pos) = line.find("All:")
                    {
                        let after_all = &line[all_pos + 4..];
                        if let Some(ssim) =
                            crate::video_explorer::precision::parse_explore_ssim_metric_token(
                                after_all,
                            )
                            .map_err(|err| {
                                anyhow::anyhow!("failed to parse quick SSIM metric token: {err}")
                            })?
                        {
                            return Ok(Some(ssim));
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_ssim",
                        format!("quick SSIM command failed to start: {err}"),
                    );
                }
            }
        }

        if self.input_is_animated_image_like {
            let compat_ssim =
                calculate_ssim_all(self.input, self.output)?.map(|(_, _, _, all)| all);
            if compat_ssim.is_some() {
                self.prefer_compat_ssim_mode = true;
            }
            return Ok(compat_ssim);
        }

        Ok(None)
    }

    fn run(mut self) -> Result<ExploreResult> {
        let outcome = self.verify_boundary()?;
        match outcome {
            BoundaryOutcome::Compressed { gpu_size, gpu_ssim } => {
                self.search_compressed_path(gpu_size, gpu_ssim)?;
            }
            BoundaryOutcome::Uncompressed { gpu_size, gpu_pct } => {
                self.search_uncompressed_path(gpu_size, gpu_pct)?;
            }
        }

        self.run_phase4_refinement()?;
        let (final_crf, final_pure_media_size, run_phase5) = self.prepare_final_settlement()?;
        let (final_crf, final_pure_media_size) =
            self.run_phase5(final_crf, final_pure_media_size, run_phase5)?;
        self.build_result(final_crf, final_pure_media_size)
    }

    fn verify_boundary(&mut self) -> Result<BoundaryOutcome> {
        let boundary_label = if self.gpu_executed {
            "GPU boundary"
        } else {
            "initial boundary"
        };
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            &format!("Verify {boundary_label}")
        );

        let gpu_boundary_crf = self.gpu_boundary_crf;
        let gpu_size = self.encode_cached(gpu_boundary_crf).map_err(|e| {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_crf",
                format!("Boundary verification failed at CRF {gpu_boundary_crf:.2}: {e}"),
            );
            e
        })?;
        let gpu_pct = self.pure_media_size_pct(gpu_size);
        let gpu_ssim = if self.ultimate_mode {
            None
        } else {
            self.calculate_ssim_quick()?
        };

        if !self.candidate_fits(gpu_size) {
            crate::media_conversion_gate::explore_gpu_coarse_audit(
                "explore_gpu_crf",
                self.input,
                format!(
                    "{} [CPU] CRF {gpu_boundary_crf:<5.2} {gpu_pct:6.1}% {} (TOO LARGE)",
                    crf_fail_prefix(),
                    crf_fail_tag()
                ),
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                crate::infra::static_logs::messages::MSG_PHASE_2_UPWARD
            );
            return Ok(BoundaryOutcome::Uncompressed { gpu_size, gpu_pct });
        }

        self.best_crf = Some(gpu_boundary_crf);
        self.best_size = Some(gpu_size);

        let mut gpu_ultimate_metrics_str = String::new();
        if self.ultimate_mode {
            let vmaf = super::ssim_calculator::calculate_vmaf_y(self.input, self.output, 6)?;
            let psnr_uv = super::ssim_calculator::calculate_psnr_uv(self.input, self.output, 6)?;
            if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                let chroma_avg = f64::midpoint(u, v_score);
                gpu_ultimate_metrics_str = format!("VMAF:{v:.2} UV:{chroma_avg:.2}");
                self.tracking.best_vmaf = Some(v);
                self.tracking.best_psnr_uv = Some((u, v_score));
            }
        }

        let metrics_display = if self.ultimate_mode && !gpu_ultimate_metrics_str.is_empty() {
            format!(" │ {gpu_ultimate_metrics_str}")
        } else if let Some(s) = gpu_ssim {
            format!(" │ SSIM:{s:.4}")
        } else {
            String::new()
        };

        let source_label = if self.gpu_executed {
            "[GPU]"
        } else {
            "[Initial]"
        };
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            &format!(
                "{} {source_label} CRF {gpu_boundary_crf:<5.2} {gpu_pct:6.1}% {metrics_display} {}",
                crf_pass_prefix(),
                crf_pass_tag()
            )
        );
        let phase2_title = if self.ultimate_mode {
            crate::infra::static_logs::messages::MSG_PHASE_2_ULTIMATE
        } else {
            crate::infra::static_logs::messages::MSG_PHASE_2_SSIM
        };
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            phase2_title
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_2,
            crate::infra::static_logs::messages::MSG_PHASE_2_STRATEGY
        );

        Ok(BoundaryOutcome::Compressed { gpu_size, gpu_ssim })
    }

    fn search_compressed_path(&mut self, gpu_size: u64, gpu_ssim: Option<f64>) -> Result<()> {
        let gpu_boundary_crf = self.gpu_boundary_crf;
        let search_floor = if self.ultimate_mode {
            0.0
        } else {
            self.min_crf
        };
        let crf_range = gpu_boundary_crf - search_floor;

        let initial_step = (crf_range / 1.5).clamp(8.0, 25.0);
        let max_wall_hits = if self.duration >= VERY_LONG_VIDEO_THRESHOLD_SECS {
            6
        } else if self.duration >= LONG_VIDEO_THRESHOLD_SECS {
            8
        } else if self.ultimate_mode {
            calculate_adaptive_max_walls(crf_range)?
        } else {
            NORMAL_MAX_WALL_HITS
        };

        let required_zero_gains = calculate_zero_gains_for_duration_and_range(
            self.duration,
            crf_range,
            self.ultimate_mode,
        )?;

        let max_iterations_for_video = if self.ultimate_mode {
            500
        } else {
            calculate_max_iterations_for_duration(self.duration, self.ultimate_mode)
        };

        if self.ultimate_mode {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                "ULTIMATE MODE: searching until 3D quality plateau / domain wall"
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!("CRF range: {crf_range:.1} → Adaptive max walls: {max_wall_hits}")
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "3D plateau patience: {required_zero_gains} consecutive fine-step \
                     non-improvements"
                )
            );
        } else {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "CRF range: {crf_range:.1} → Initial step: {initial_step:.1} (v6.2 curve \
                     model)"
                )
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "Strategy: Aggressive curve decay (step × 0.4 per wall hit, max \
                     {max_wall_hits} hits)"
                )
            );
        }

        let mut current_step = if self.is_gif_magic && gpu_boundary_crf < 0.1 {
            1.0_f32
        } else {
            initial_step
        };
        let mut wall_hits: u32 = 0;

        let mut test_crf = {
            let next = gpu_boundary_crf - current_step;
            if next < search_floor && gpu_boundary_crf > search_floor {
                search_floor
            } else {
                next
            }
        };

        let mut last_good_crf = gpu_boundary_crf;
        let mut last_good_size = gpu_size;
        let mut last_good_ssim = gpu_ssim;

        let gpu_ssim_baseline = gpu_ssim.inspect(|s| {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!("GPU SSIM baseline: {s:.4} (CPU target: break through 0.97+)")
            );
        });

        let mut consecutive_zero_gains: u32 = 0;
        let mut failure_credibility: f64 = 0.0;
        let mut quality_wall_hit = false;
        let mut domain_wall_hit = false;

        if self.duration >= LONG_VIDEO_THRESHOLD_SECS {
            let long_video_strategy = if self.ultimate_mode {
                "searching until 3D quality plateau stabilizes"
            } else {
                "searching until SSIM saturates"
            };
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "Long video ({min:.1} min) - {long_video_strategy}",
                    min = self.duration / 60.0
                )
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "Fallback limit: {max_iterations_for_video} (emergency only), Max walls: \
                     {max_wall_hits}, Zero-gains: {required_zero_gains}"
                )
            );
        }

        let mut last_logged_int_crf =
            crate::numeric_cast::f32_to_i32_strict(gpu_boundary_crf.floor(), "milestone_crf");
        if let Some(log_crf) = last_logged_int_crf {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                &format!("Entering CRF {log_crf}.x zone")
            );
        }

        while self.iterations < max_iterations_for_video && test_crf >= search_floor {
            let current_int_crf =
                crate::numeric_cast::f32_to_i32_strict(test_crf.floor(), "current_crf");
            if current_int_crf != last_logged_int_crf {
                last_logged_int_crf = current_int_crf;
                if let Some(log_crf) = last_logged_int_crf {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_STRATEGY,
                        &format!("Entering CRF {log_crf}.x zone")
                    );
                }
            }
            if test_crf < search_floor {
                if current_step > crate::constants::EXPLORATION_MIN_STEP + 0.01 {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_STRATEGY,
                        &format!("Reached search floor, fine tuning from CRF {last_good_crf:.2}")
                    );
                    current_step = crate::constants::EXPLORATION_MIN_STEP;
                    test_crf = last_good_crf - current_step;
                    if test_crf < search_floor {
                        break;
                    }
                } else {
                    break;
                }
            }

            if (test_crf - 0.0).abs() < 0.001
                && self.duration > HEAVY_VIDEO_THRESHOLD_SECS
                && self.tracking.best_vmaf.is_none_or(|c| c >= 5.0_f64)
            {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_STRATEGY,
                    &format!(
                        "Heavy video ({min:.1} min): skipping CRF 0.00 probe as no high-quality \
                         success (< 5.0) confirmed yet",
                        min = self.duration / 60.0
                    )
                );
                break;
            }

            if self.size_cache.contains_key(test_crf) {
                test_crf -= current_step;
                continue;
            }

            let size = self.encode_cached(test_crf)?;
            let pure_media_size_pct = self.pure_media_size_pct(size);
            let current_ssim_opt = if self.ultimate_mode {
                None
            } else {
                self.calculate_ssim_quick()?
            };

            let is_effectively_compressed = self.candidate_fits(size);

            if is_effectively_compressed {
                let prev_ssim_opt = last_good_ssim;
                last_good_crf = test_crf;
                last_good_size = size;
                last_good_ssim = current_ssim_opt;
                self.best_crf = Some(test_crf);
                self.best_size = Some(size);

                let should_stop = if self.ultimate_mode {
                    let mut ultimate_metrics_str = String::new();
                    let mut quality_plateau = false;
                    let mut metrics_measured = false;

                    let vmaf =
                        super::ssim_calculator::calculate_vmaf_y(self.input, self.output, 6)?;
                    let psnr_uv =
                        super::ssim_calculator::calculate_psnr_uv(self.input, self.output, 6)?;

                    if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                        metrics_measured = true;
                        let chroma_avg = f64::midpoint(u, v_score);
                        let prev_best_vmaf_opt = self.tracking.best_vmaf;
                        let prev_best_psnr_opt =
                            self.tracking.best_psnr_uv.map(|(u, v)| f64::midpoint(u, v));
                        let vmaf_improved =
                            prev_best_vmaf_opt.is_some_and(|prev| v.floor() > prev.floor());
                        let psnr_improved = prev_best_psnr_opt
                            .is_some_and(|prev| chroma_avg.floor() > prev.floor());

                        ultimate_metrics_str = format!("VMAF:{v:.2} UV:{chroma_avg:.2}");

                        if vmaf_improved || self.tracking.best_vmaf.is_none() {
                            self.tracking.best_vmaf = Some(v);
                        }
                        if psnr_improved || self.tracking.best_psnr_uv.is_none() {
                            self.tracking.best_psnr_uv = Some((u, v_score));
                        }

                        if !vmaf_improved && !psnr_improved {
                            failure_credibility += 1.0_f64;
                            if failure_credibility >= 3.0_f64 {
                                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                                    "explore_gpu_coarse",
                                    "QUALITY PLATEAU REACHED (3/3): No integer improvement over 3 \
                                     insights. Stopping.",
                                );
                                self.early_insight_triggered = true;
                                break;
                            }
                        } else {
                            failure_credibility = 0.0_f64;
                        }

                        quality_plateau = (v
                            > crate::constants::EXPLORATION_GPU_QUALITY_PLATEAU_VMAF_HINT
                            || chroma_avg
                                > crate::constants::EXPLORATION_GPU_QUALITY_PLATEAU_PSNR_UV_HINT)
                            && !vmaf_improved
                            && !psnr_improved;
                    }

                    if current_step <= crate::constants::EXPLORATION_MIN_STEP + 0.01 {
                        if quality_plateau {
                            consecutive_zero_gains += 1;
                        } else {
                            consecutive_zero_gains = 0;
                        }
                    }

                    let quality_wall_triggered = metrics_measured
                        && current_step <= crate::constants::EXPLORATION_MIN_STEP + 0.01
                        && consecutive_zero_gains >= required_zero_gains;

                    if quality_wall_triggered {
                        let Some(vmaf_metric) = self.tracking.best_vmaf else {
                            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                                "explore_gpu_quality",
                                "VMAF not measured at quality wall",
                            );
                            bail!("Quality wall hit but VMAF not measured");
                        };
                        let psnr_uv_min_channel = if let Some((u, v)) = self.tracking.best_psnr_uv {
                            u.min(v)
                        } else {
                            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                                "explore_gpu_quality",
                                "PSNR UV not measured at quality wall",
                            );
                            bail!("Quality wall hit but PSNR UV not measured");
                        };

                        let vmaf_floor =
                            crate::media_conversion_gate::explore_adaptive_vmaf_y_floor_optional(
                                self.tracking.best_vmaf,
                            );
                        let psnr_floor =
                            crate::media_conversion_gate::explore_adaptive_psnr_uv_floor_optional(
                                self.tracking.best_psnr_uv,
                            );
                        let not_credible =
                            if let (Some(vf), Some((uf, vf2))) = (vmaf_floor, psnr_floor) {
                                vmaf_metric < vf || psnr_uv_min_channel < uf.min(vf2)
                            } else {
                                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                                    "explore_gpu_quality",
                                    format!(
                                        "QUALITY WALL: VMAF:{vmaf_metric:.2} \
                                         UV:{psnr_uv_min_channel:.2} but search baseline absent; \
                                         refusing sanity-floor credibility check"
                                    ),
                                );
                                false
                            };
                        if not_credible {
                            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                                "explore_gpu_quality",
                                format!(
                                    "QUALITY CEILING HIT (NOT CREDIBLE): Saturated at \
                                     VMAF:{vmaf_metric:.2}, UV:{psnr_uv_min_channel:.2}. Below \
                                     adaptive floor from search baseline. Aborting."
                                ),
                            );
                            quality_wall_hit = true;
                            break;
                        }
                    }

                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}% \
                             {metrics_display}{sat_status}",
                            crf_pass_prefix(),
                            metrics_display = if ultimate_metrics_str.is_empty() {
                                "N/A"
                            } else {
                                &ultimate_metrics_str
                            },
                            sat_status = if consecutive_zero_gains > 0
                                && current_step <= crate::constants::EXPLORATION_MIN_STEP + 0.01
                            {
                                format!(" [SAT:{consecutive_zero_gains}/{required_zero_gains}]")
                            } else {
                                String::new()
                            }
                        )
                    );

                    if quality_wall_triggered {
                        quality_wall_hit = true;
                    }
                    quality_wall_triggered
                } else if let (Some(current_ssim), Some(prev_ssim)) =
                    (current_ssim_opt, prev_ssim_opt)
                {
                    let ssim_gain = current_ssim - prev_ssim;

                    if let Some(gpu_baseline) = gpu_ssim_baseline.filter(|v| *v > 0.0_f64) {
                        let ssim_vs_gpu = current_ssim / gpu_baseline;
                        let _gpu_comparison = if ssim_vs_gpu > 1.01_f64 {
                            format!("{BRIGHT_GREEN}×{ssim_vs_gpu:.3} GPU{RESET}")
                        } else if ssim_vs_gpu > 1.001_f64 {
                            format!("{GREEN}×{ssim_vs_gpu:.4} GPU{RESET}")
                        } else {
                            format!("{DIM}≈GPU{RESET}")
                        };
                    }

                    if current_step <= crate::constants::EXPLORATION_MIN_STEP + 0.01 {
                        if ssim_gain.abs() < crate::constants::EXPLORATION_ZERO_GAIN_THRESHOLD {
                            consecutive_zero_gains += 1;
                        } else {
                            consecutive_zero_gains = 0;
                        }
                    }

                    let quality_wall_triggered = current_step
                        <= crate::constants::EXPLORATION_MIN_STEP + 0.01
                        && consecutive_zero_gains >= required_zero_gains;

                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}% │ \
                             SSIM:{current_ssim:.4} Δ{ssim_gain:+.4}{sat_status}",
                            crf_pass_prefix(),
                            sat_status = if consecutive_zero_gains > 0
                                && current_step <= crate::constants::EXPLORATION_MIN_STEP + 0.01
                            {
                                format!(" [SAT:{consecutive_zero_gains}/{required_zero_gains}]")
                            } else {
                                String::new()
                            }
                        )
                    );

                    if quality_wall_triggered {
                        quality_wall_hit = true;
                    }
                    quality_wall_triggered
                } else if let Some(current_ssim) = current_ssim_opt {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}% │ \
                             SSIM:{current_ssim:.4}",
                            crf_pass_prefix()
                        )
                    );
                    false
                } else {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}%",
                            crf_pass_prefix()
                        )
                    );
                    false
                };

                if should_stop {
                    if self.ultimate_mode {
                        domain_wall_hit = true;
                        let msg = if consecutive_zero_gains >= required_zero_gains {
                            format!(
                                "3D quality plateau after {consecutive_zero_gains} consecutive \
                                 fine-step non-improvements"
                            )
                        } else {
                            "VMAF(Y) + PSNR(UV) absolute quality ceiling reached".to_string()
                        };
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_PHASE_2,
                            &format!("[CPU] DOMAIN WALL HIT: {msg}")
                        );
                    } else {
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_PHASE_2,
                            &format!(
                                "[CPU] QUALITY WALL HIT: SSIM saturated after \
                                 {consecutive_zero_gains} consecutive zero-gains"
                            )
                        );
                    }
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "Final: CRF {test_crf:.2}, compression {pure_media_size_pct:+.1}%, \
                             iterations {iter}",
                            iter = self.iterations
                        )
                    );
                    break;
                }

                test_crf -= current_step;
            } else {
                wall_hits += 1;

                let curve_step = initial_step
                    * crate::constants::EXPLORATION_DECAY_FACTOR
                        .powi(crate::numeric_cast::u32_to_i32_sat(wall_hits));
                let new_step = if curve_step < 1.0 {
                    crate::constants::EXPLORATION_MIN_STEP
                } else {
                    curve_step
                };

                let decay_val = crate::constants::EXPLORATION_DECAY_FACTOR;
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_2,
                    &format!(
                        "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}% │ {} WALL HIT \
                         #{wall_hits} (Backtrack: {current_step:.2} → {new_step:.2} {phase_info})",
                        crf_fail_prefix(),
                        crf_fail_tag(),
                        phase_info = if wall_hits == 1 {
                            format!("decay ×{decay_val:.1}")
                        } else if new_step <= crate::constants::EXPLORATION_MIN_STEP + 0.01 {
                            "→ FINE TUNING".to_string()
                        } else {
                            format!("decay ×{decay_val:.1}^{wall_hits}")
                        }
                    )
                );

                if current_step <= crate::constants::EXPLORATION_MIN_STEP + 0.01
                    && new_step <= crate::constants::EXPLORATION_MIN_STEP + 0.01
                {
                    if self.ultimate_mode {
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_PHASE_2,
                            "[CPU] 🧱 Size wall hit at 0.01 minimum granularity. Oscillation \
                             locked down, handing off to Phase 4."
                        );
                        break;
                    }
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        "[CPU] 🧱 Minimum step reached and hit capacity wall. Stopping \
                         exploration."
                    );
                    break;
                }

                if wall_hits >= max_wall_hits {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "[CPU] Adaptive wall limit ({max_wall_hits}) reached. Stopping at \
                             best CRF {last_good_crf:.2}"
                        )
                    );
                    break;
                }

                current_step = new_step;
                test_crf = last_good_crf - current_step;
            }
        }

        if domain_wall_hit || quality_wall_hit {
            if self.best_crf.is_none_or(|c| c > last_good_crf) {
                self.best_crf = Some(last_good_crf);
                self.best_size = Some(last_good_size);
            }
        } else if wall_hits > 0 {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!("[CPU] Size wall hit: overshoot at CRF < {last_good_crf:.1}")
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "Final: CRF {last_good_crf:.2}, iterations {iter}",
                    iter = self.iterations
                )
            );
        } else if test_crf < search_floor {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "[CPU] Search floor reached ({search_floor:.1}) - maximum quality achieved"
                )
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                &format!(
                    "Final: CRF {last_good_crf:.2}, iterations {iter}",
                    iter = self.iterations
                )
            );

            if self.best_crf.is_none_or(|c| c > last_good_crf) {
                self.best_crf = Some(last_good_crf);
                self.best_size = Some(last_good_size);
            }
        }

        Ok(())
    }

    fn search_uncompressed_path(&mut self, gpu_size: u64, gpu_pct: f64) -> Result<()> {
        let gpu_boundary_crf = self.gpu_boundary_crf;
        let max_crf = self.max_crf;

        let mut current_step = Self::STEP_UPWARD;
        let mut stagnation_count = 0u32;
        let mut backtrack_count = 0u32;
        let mut last_size_pct = gpu_pct;
        let mut test_crf = gpu_boundary_crf + current_step;
        let mut search_cadence = UpwardSearchCadence::Adaptive;
        let mut found_compress_point = false;
        let mut failure_credibility = 0.0_f64;
        let mut best_tested_crf = gpu_boundary_crf;
        let mut best_tested_size = gpu_size;

        let mut feedback = UpwardSearchFeedback {
            size_stagnation_count: 0,
            upward_iteration_count: 0,
        };

        // Bi-directional Pivot / Reverse Exploration: when the initial probe failed by
        // a wide margin, orbit to the ceiling first to see if compression is
        // possible at all.
        if gpu_boundary_crf < 5.0 && gpu_pct > 3.0_f64 {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                &format!(
                    "Bi-directional Pivot: CRF {gpu_boundary_crf:.2} too large ({gpu_pct:.1}%), \
                     probing ceiling CRF {max_crf:.2}..."
                )
            );

            let ceiling_size = self.encode_cached(max_crf)?;
            let ceiling_pct = self.pure_media_size_pct(ceiling_size);

            if ceiling_pct >= 0.0_f64 {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_crf",
                    format!(
                        "Media is incompressible even at max quality (CRF {max_crf:.1}). Bailing \
                         out."
                    ),
                );
                self.best_crf = Some(max_crf);
                self.best_size = Some(ceiling_size);
                self.early_insight_triggered = true;
            } else {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_2,
                    &format!(
                        "Ceiling hit! Space [0.0, {max_crf:.2}] is compressible. Starting search \
                         from mid-point..."
                    )
                );
                // Mid-Jump Pivot: record the ceiling as a fallback, then jump to a more useful
                // starting CRF to avoid walking from 0 upwards step-by-step.
                self.best_crf = Some(max_crf);
                self.best_size = Some(ceiling_size);
                test_crf = 12.0_f32;
            }
        }

        let max_iterations_for_video = if self.ultimate_mode {
            500
        } else {
            calculate_max_iterations_for_duration(self.duration, self.ultimate_mode)
        };

        while test_crf <= max_crf
            && self.iterations < max_iterations_for_video
            && !self.early_insight_triggered
        {
            let size = self.encode_cached(test_crf)?;
            feedback.upward_iteration_count += 1;

            let pure_media_size_pct = self.pure_media_size_pct(size);

            if pure_media_size_pct < 0.0_f64 {
                found_compress_point = true;
                best_tested_crf = test_crf;
                best_tested_size = size;
                self.best_crf = Some(test_crf);
                self.best_size = Some(size);
                break;
            }

            let size_delta = (pure_media_size_pct - last_size_pct).abs();

            // Size stagnation past the lossless deadzone or sustained upward iteration requests
            // a measured ceiling pivot before any downward sweep.
            if size_delta < 0.5_f64 {
                if test_crf > 12.0 {
                    feedback.size_stagnation_count += 1;
                }
            } else {
                feedback.size_stagnation_count = 0;
            }

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

                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_STRATEGY,
                    &format!(
                        "Search Direction Switch: {trigger_reason} reached. Probing ceiling CRF \
                         {max_crf:.2} before downward search."
                    )
                );

                let ceiling_size = self.encode_cached(max_crf)?;
                if ceiling_size < best_tested_size {
                    best_tested_crf = max_crf;
                    best_tested_size = ceiling_size;
                }
                if self.candidate_fits(ceiling_size) {
                    found_compress_point = true;
                    self.best_crf = Some(max_crf);
                    self.best_size = Some(ceiling_size);
                } else {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_crf",
                        format!(
                            "Direction switch ceiling probe at CRF {max_crf:.2} remained larger \
                             than the input; no compression point confirmed."
                        ),
                    );
                }
                break;
            }

            if search_cadence == UpwardSearchCadence::Adaptive {
                if size_delta < 0.1_f64 && pure_media_size_pct > 100.0_f64 {
                    stagnation_count += 1;

                    if test_crf < 15.0 && size_delta < 0.02_f64 && stagnation_count >= 2 {
                        let jump_step = (20.0 - test_crf).max(8.0);
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_STRATEGY,
                            &format!(
                                "Deadzone Burst: jumping {jump_step:.1} units to escape the \
                                 lossless plateau..."
                            )
                        );
                        current_step = jump_step;
                        stagnation_count = 0;
                    } else if stagnation_count >= 2 {
                        let old_step = current_step;
                        current_step = (current_step * 2.0).min(5.0);
                        if current_step > old_step {
                            crate::log_info!(
                                crate::infra::static_logs::messages::LABEL_PHASE_2,
                                &format!(
                                    "Search Accelerated (step: {old_step:.2} → {current_step:.2})"
                                )
                            );
                        }
                    }

                    if stagnation_count >= 6 && pure_media_size_pct > 110.0_f64 && test_crf > 30.0 {
                        crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                            "explore_gpu_size",
                            "Quality/Size Plateau detected: bailing out early.",
                        );
                        if self.best_crf.is_none() {
                            self.best_crf = Some(best_tested_crf);
                            self.best_size = Some(best_tested_size);
                        }
                        self.early_insight_triggered = true;
                        break;
                    }
                } else if size_delta > 2.5_f64 && pure_media_size_pct < 110.0_f64 {
                    if current_step > Self::STEP_UPWARD {
                        let jog_step = crate::constants::EXPLORATION_UPWARD_JOG_MIN_STEP
                            .max(Self::STEP_UPWARD);
                        if current_step > jog_step + f32::EPSILON {
                            crate::log_info!(
                                crate::infra::static_logs::messages::LABEL_PHASE_2,
                                &format!(
                                    "Search Decelerating (slope Δ{size_delta:.1} detected, step: \
                                     {current_step:.2} → {jog_step:.2}, entering jog)"
                                )
                            );
                            current_step = jog_step;
                            search_cadence = UpwardSearchCadence::Jogging;
                        } else {
                            crate::log_info!(
                                crate::infra::static_logs::messages::LABEL_PHASE_2,
                                &format!(
                                    "Search Decelerating (slope Δ{size_delta:.1} detected, step: \
                                     {current_step:.2} → {step_up:.2}, entering pause)",
                                    step_up = Self::STEP_UPWARD
                                )
                            );
                            current_step = Self::STEP_UPWARD;
                            search_cadence = UpwardSearchCadence::Paused;
                        }
                    }
                    stagnation_count = 0;
                } else {
                    stagnation_count = 0;
                }
            }

            if size < best_tested_size {
                best_tested_crf = test_crf;
                best_tested_size = size;
            }

            let is_effectively_compressed = self.candidate_fits(size);

            // Ultimate Mode: Insight-Based Credibility Check (Sticky). Only run expensive
            // VMAF/PSNR when we are somewhat close to compression to avoid process
            // exhaustion.
            if self.ultimate_mode
                && !is_effectively_compressed
                && (pure_media_size_pct < 120.0_f64 || self.iterations < 2)
            {
                let vmaf = super::ssim_calculator::calculate_vmaf_y(self.input, self.output, 6)?;
                let psnr_uv =
                    super::ssim_calculator::calculate_psnr_uv(self.input, self.output, 6)?;

                if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                    let chroma_avg = f64::midpoint(u, v_score);

                    let prev_best_vmaf_opt = self.tracking.best_vmaf;
                    let prev_best_psnr_opt =
                        self.tracking.best_psnr_uv.map(|(u, v)| f64::midpoint(u, v));

                    let vmaf_improved =
                        prev_best_vmaf_opt.is_none_or(|prev| v.floor() > prev.floor());
                    let psnr_improved =
                        prev_best_psnr_opt.is_none_or(|prev| chroma_avg.floor() > prev.floor());
                    let improvement_indicator = if vmaf_improved || psnr_improved {
                        "↑"
                    } else {
                        "→"
                    };
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}% │ VMAF:{v:.2} \
                             UV:{chroma_avg:.2} ({failure_credibility:.1}/3.0 \
                             {improvement_indicator})",
                            crf_fail_prefix(),
                        )
                    );

                    if vmaf_improved || self.tracking.best_vmaf.is_none() {
                        self.tracking.best_vmaf = Some(v);
                    }
                    if psnr_improved || self.tracking.best_psnr_uv.is_none() {
                        self.tracking.best_psnr_uv = Some((u, v_score));
                    }

                    if !vmaf_improved && !psnr_improved {
                        failure_credibility += 1.0_f64;
                        if failure_credibility >= 3.0_f64 {
                            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                                "explore_gpu_coarse",
                                "QUALITY PLATEAU REACHED (3/3): No integer improvement over 3 \
                                 insights. Stopping.",
                            );
                            if self.best_crf.is_none() {
                                self.best_crf = Some(best_tested_crf);
                                self.best_size = Some(best_tested_size);
                            }
                            self.early_insight_triggered = true;
                            break;
                        }
                    } else {
                        failure_credibility = 0.0_f64;
                    }
                }
            }

            if is_effectively_compressed {
                // Backtrack-on-Overshoot: if we jumped from >105% to <95%, seek precision
                if last_size_pct > 105.0_f64
                    && pure_media_size_pct < 95.0_f64
                    && current_step > 0.5
                    && backtrack_count < 2
                {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_STRATEGY,
                        &format!(
                            "Overshot boundary ({pure_media_size_pct:.1}%): backtracking for \
                             precision... (retry {retry}/2)",
                            retry = backtrack_count + 1
                        )
                    );
                    test_crf -= current_step / 2.0;
                    current_step = Self::STEP_UPWARD;
                    backtrack_count += 1;
                    continue;
                }

                if self.best_crf.is_none_or(|c| test_crf < c) {
                    self.best_crf = Some(test_crf);
                    self.best_size = Some(size);
                }
                found_compress_point = true;
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_2,
                    &format!(
                        "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}% │ FOUND! {}",
                        crf_pass_prefix(),
                        crf_pass_tag()
                    )
                );
                break;
            } else if !self.ultimate_mode {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_2,
                    &format!(
                        "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}% {}",
                        crf_fail_prefix(),
                        crf_fail_tag()
                    )
                );
            }

            match search_cadence {
                UpwardSearchCadence::Jogging => {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "Search Jogging complete (step: {current_step:.2} → {step_up:.2}); \
                             pausing adaptive changes",
                            step_up = Self::STEP_UPWARD
                        )
                    );
                    current_step = Self::STEP_UPWARD;
                    search_cadence = UpwardSearchCadence::Paused;
                }
                UpwardSearchCadence::Paused => {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        &format!(
                            "Search Paused at boundary pace ({current_step:.2}); resuming normal \
                             iteration next step"
                        )
                    );
                    search_cadence = UpwardSearchCadence::Normal;
                }
                UpwardSearchCadence::Adaptive | UpwardSearchCadence::Normal => {}
            }

            last_size_pct = pure_media_size_pct;
            test_crf += current_step;
        }

        if !found_compress_point {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_crf",
                format!(
                    "FAILED: No compression point found below input size (up to max CRF \
                     {max_crf:.2}). File may be already optimally compressed. Aborting \
                     fine-tuning."
                ),
            );
            if self.best_crf.is_none() {
                self.best_crf = Some(best_tested_crf);
                self.best_size = Some(best_tested_size);
            }
            return Ok(());
        }

        // ---- Phase 3: Downward sprint-and-backtrack from the compression point ----
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_3,
            &format!(
                "Search DOWNWARD with Sprint & Backtrack (min step {step:.2})",
                step = crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP
            )
        );

        let compress_point = self.best_crf.ok_or_else(|| {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_crf",
                "GPU Coarse Search: Failed to find valid best_crf; sampling logic invalidated",
            );
            anyhow::anyhow!("GPU Coarse Search: best_crf not found")
        })?;
        let compress_size = self.best_size.ok_or_else(|| {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_crf",
                "GPU Coarse Search: best_crf exists but best_size is missing; refusing to infer \
                 source-sized baseline",
            );
            anyhow::anyhow!("GPU Coarse Search: best_size not found")
        })?;

        let mut current_step = crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP;
        let mut last_size_pct = self.pure_media_size_pct(compress_size);
        let mut backtrack_count = 0u32;
        let mut failure_credibility = 0.0_f64;
        let mut consecutive_failures = 0u32;
        let mut consecutive_successes = 0_i32;
        let mut consecutive_compressions = 0u32;
        let mut prev_ssim_opt = if self.ultimate_mode {
            None
        } else {
            self.calculate_ssim_quick()?
        };
        let search_floor = if self.ultimate_mode {
            0.0
        } else {
            self.min_crf
        };
        let mut test_crf = compress_point - current_step;

        while test_crf >= search_floor && self.iterations < max_iterations_for_video {
            if self.size_cache.contains_key(test_crf) {
                test_crf -= current_step;
                continue;
            }

            let size = self.encode_cached(test_crf)?;
            let pure_media_size_pct = self.pure_media_size_pct(size);

            let current_ssim_opt = if self.ultimate_mode {
                None
            } else {
                self.calculate_ssim_quick()?
            };

            let mut vmaf_improved = false;
            let mut psnr_improved = false;
            let mut current_vmaf_val = None;
            let mut current_psnr_val = None;

            if self.ultimate_mode {
                let vmaf = super::ssim_calculator::calculate_vmaf_y(self.input, self.output, 6)?;
                let psnr_uv =
                    super::ssim_calculator::calculate_psnr_uv(self.input, self.output, 6)?;

                if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                    let chroma_avg = f64::midpoint(u, v_score);
                    let prev_best_vmaf_opt = self.tracking.best_vmaf;
                    let prev_best_psnr_opt =
                        self.tracking.best_psnr_uv.map(|(u, v)| f64::midpoint(u, v));

                    vmaf_improved = prev_best_vmaf_opt.is_some_and(|prev| v.floor() > prev.floor());
                    psnr_improved =
                        prev_best_psnr_opt.is_some_and(|prev| chroma_avg.floor() > prev.floor());

                    current_vmaf_val = Some(v);
                    current_psnr_val = Some((u, v_score));
                }
            }

            let is_effectively_compressed = self.candidate_fits(size);
            let size_delta = (pure_media_size_pct - last_size_pct).abs();

            if is_effectively_compressed {
                consecutive_failures = 0;
                consecutive_compressions += 1;

                self.best_crf = Some(test_crf);
                self.best_size = Some(size);

                if self.ultimate_mode {
                    if vmaf_improved || self.tracking.best_vmaf.is_none() {
                        self.tracking.best_vmaf = current_vmaf_val;
                    }
                    if psnr_improved || self.tracking.best_psnr_uv.is_none() {
                        self.tracking.best_psnr_uv = current_psnr_val;
                    }
                }

                let improvement_indicator = if vmaf_improved || psnr_improved {
                    "↑"
                } else {
                    "→"
                };

                let ssim_gain = match (current_ssim_opt, prev_ssim_opt) {
                    (Some(curr), Some(prev)) => curr - prev,
                    _ => 0.0_f64,
                };

                let metrics_str = if self.ultimate_mode {
                    let vmaf_opt = self.tracking.best_vmaf;
                    let psnr_uv_opt = self.tracking.best_psnr_uv;
                    if let (Some(v), Some((u, v_score))) = (vmaf_opt, psnr_uv_opt) {
                        let chroma_avg = f64::midpoint(u, v_score);
                        format!(
                            " │ VMAF:{v:.2} UV:{chroma_avg:.2} ({failure_credibility:.0}/3 \
                             {improvement_indicator})"
                        )
                    } else {
                        String::new()
                    }
                } else if let Some(current_ssim) = current_ssim_opt {
                    format!(" │ SSIM:{current_ssim:.4} Δ{ssim_gain:+.4}")
                } else {
                    String::new()
                };

                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}%{metrics_str} (step \
                         {current_step:.2}) {}",
                        crf_pass_prefix(),
                        crf_pass_tag(),
                    )
                );

                if consecutive_compressions >= MAX_CONSECUTIVE_COMPRESSIONS {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_3,
                        &format!(
                            "Efficiency limit reached: {MAX_CONSECUTIVE_COMPRESSIONS} consecutive \
                             compressions found. Stopping."
                        )
                    );
                    break;
                }

                if self.ultimate_mode {
                    if !vmaf_improved && !psnr_improved {
                        failure_credibility += 1.0_f64;
                        if failure_credibility >= 3.0_f64 {
                            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                                "explore_gpu_coarse",
                                "QUALITY PLATEAU REACHED (3/3): No integer improvement over 3 \
                                 insights. Stopping.",
                            );
                            break;
                        }
                    } else {
                        failure_credibility = 0.0_f64;
                    }
                } else if let (Some(s), Some(p)) = (current_ssim_opt, prev_ssim_opt)
                    && s - p < 0.000_1_f64
                    && s >= 0.99_f64
                {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_3,
                        "SSIM plateau → STOP"
                    );
                    break;
                }

                prev_ssim_opt = current_ssim_opt;

                let distance_to_floor = test_crf - search_floor;
                let decelerate_multiplier = if self.ultimate_mode { 1.0 } else { 2.0 };
                let boundary_nearing = distance_to_floor < current_step * decelerate_multiplier;

                if size_delta > 1.0_f64
                    && current_step > crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP
                {
                    let old_step = current_step;
                    current_step = crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP;
                    consecutive_successes = 0_i32;
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_3,
                        &format!(
                            "Search Decelerating (slope Δ{size_delta:.1} detected, step reset: \
                             {old_step:.2} → {current_step:.2})"
                        )
                    );
                } else if boundary_nearing
                    && current_step > crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP + 0.001
                {
                    let old_step = current_step;
                    current_step = (current_step / 2.0)
                        .max(crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP);
                    consecutive_successes = 0_i32;
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_3,
                        &format!(
                            "Smart deceleration: step {old_step:.2} → {current_step:.2} \
                             (approaching floor {search_floor:.2})"
                        )
                    );
                } else {
                    consecutive_successes += 1_i32;
                    if consecutive_successes >= 2_i32 && current_step < 1.6 {
                        let old_step = current_step;
                        current_step = (current_step * 2.0).min(1.6);
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_PHASE_3,
                            &format!("Sprint activated: step {old_step:.2} → {current_step:.2}")
                        );
                    }
                }

                last_size_pct = pure_media_size_pct;
                test_crf -= current_step;
            } else {
                consecutive_failures += 1;

                let metrics_str = if self.ultimate_mode {
                    let vmaf_opt = self.tracking.best_vmaf;
                    let psnr_uv_opt = self.tracking.best_psnr_uv;
                    if let (Some(v), Some((u, v_score))) = (vmaf_opt, psnr_uv_opt) {
                        let chroma_avg = f64::midpoint(u, v_score);
                        format!(" │ VMAF:{v:.2} UV:{chroma_avg:.2} ({failure_credibility:.0}/3 →)")
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}%{metrics_str} {} (fail \
                         {consecutive_failures}/{MAX_CONSECUTIVE_FAILURES})",
                        crf_fail_prefix(),
                        crf_fail_tag(),
                    )
                );

                if current_step > crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP + 0.01
                    && backtrack_count < 2
                {
                    let old_step = current_step;
                    current_step = (current_step / 2.0)
                        .max(crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP);
                    backtrack_count += 1;
                    consecutive_successes = 0_i32;
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_STRATEGY,
                        &format!(
                            "Backtracking for precision (retry {backtrack_count}/2): \
                             {old_step:.2} → {current_step:.2}"
                        )
                    );
                    test_crf = crate::media_conversion_gate::explore_best_crf_or_backtrack_anchor(
                        self.best_crf,
                        test_crf,
                        old_step,
                    ) - current_step;
                    continue;
                }

                if !self.ultimate_mode {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_3,
                        &format!(
                            "Capacity exceeded at step {step:.2}. Stopping.",
                            step = crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP
                        )
                    );
                    break;
                }

                current_step = crate::constants::EXPLORATION_PHASE3_DOWNWARD_STEP;
                test_crf -= current_step;

                if self.ultimate_mode {
                    if !vmaf_improved && !psnr_improved {
                        failure_credibility += 1.0_f64;
                        if failure_credibility >= 3.0_f64 {
                            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                                "explore_gpu_coarse",
                                "FAILURE CREDIBILITY REACHED (3/3): Sustained quality collapse. \
                                 Stopping.",
                            );
                            self.early_insight_triggered = true;
                            break;
                        }
                    } else {
                        failure_credibility = 0.0_f64;
                    }
                }

                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_coarse",
                        format!("Max consecutive failures ({MAX_CONSECUTIVE_FAILURES}) → STOP"),
                    );
                    break;
                }
                last_size_pct = pure_media_size_pct;
            }
        }

        Ok(())
    }

    fn run_phase4_refinement(&mut self) -> Result<()> {
        if !self.ultimate_mode || self.early_insight_triggered {
            return Ok(());
        }
        let Some(best) = self.best_crf else {
            return Ok(());
        };

        let current_ratio = self.best_size.map(|sz| {
            crate::numeric_cast::u64_to_f64(sz)
                / crate::numeric_cast::u64_to_f64(self.input_pure_media_size.max(1))
        });
        if current_ratio.is_none() {
            crate::media_conversion_gate::explore_gpu_coarse_audit(
                "explore_gpu_crf",
                self.input,
                "Skipping ultimate fine-tune because best_crf exists without best_size; refusing \
                 to forge an infinite size ratio",
            );
            return Ok(());
        }
        if !(best < self.max_crf && current_ratio.is_some_and(|ratio| ratio < 1.01_f64)) {
            return Ok(());
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_4,
            "Extreme Mode 0.01-Granularity Fine-Tune (Sprint & Backtrack)"
        );

        let base_step = 0.01_f32;
        let mut current_step = base_step;
        let max_sprint_step = 1.28_f32;
        let max_fine_failures = PHASE4_ULTIMATE_MAX_FINE_FAILURES;

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_4,
            &format!(
                "Starting from 0.1 optimum (CRF {best:.2}) with adaptive step (0.01 → \
                 {max_sprint_step:.2} sprint)"
            )
        );

        let Some(mut current_best_size) = self.best_size else {
            crate::media_conversion_gate::explore_gpu_coarse_audit(
                "explore_gpu_crf",
                self.input,
                "Skipping ultimate fine-tune because best_crf exists without best_size; refusing \
                 to continue with forged state",
            );
            return Ok(());
        };
        let mut current_best = best;
        let mut test_crf = best - current_step;
        let mut fine_failures = 0_i32;
        let mut last_size_pct = self.pure_media_size_pct(current_best_size);
        let mut backtrack_count = 0u32;
        let search_floor = 0.0_f32;
        let mut consecutive_successes = 0_i32;
        let mut phase4_attempts = 0u32;
        let mut phase4_attempt_cap_hit = false;

        while test_crf >= search_floor && self.iterations < 500 {
            if phase4_attempts >= PHASE4_MAX_ATTEMPTS {
                phase4_attempt_cap_hit = true;
                break;
            }
            phase4_attempts += 1;

            // Round to 0.01 precision to avoid float drift accumulating past 0.0
            test_crf = (test_crf * 100.0).round() / 100.0;
            if test_crf < 0.0 {
                test_crf = 0.0;
            }

            if self.size_cache.contains_key(test_crf) {
                if crate::float_compare::approx_eq_crf(test_crf, 0.0) {
                    break;
                }
                test_crf -= current_step;
                continue;
            }

            let size = self.encode_cached(test_crf)?;

            let is_effectively_compressed = self.candidate_fits(size);
            let pure_media_size_pct = self.pure_media_size_pct(size);
            let size_delta = (pure_media_size_pct - last_size_pct).abs();

            if is_effectively_compressed {
                current_best = test_crf;
                current_best_size = size;
                fine_failures = 0_i32;
                consecutive_successes += 1_i32;

                let mut metrics_info = String::new();
                let vmaf = super::ssim_calculator::calculate_vmaf_y(self.input, self.output, 6)?;
                let psnr_uv =
                    super::ssim_calculator::calculate_psnr_uv(self.input, self.output, 6)?;
                if let (Some(v), Some((u, v_score))) = (vmaf, psnr_uv) {
                    let chroma_avg = f64::midpoint(u, v_score);
                    metrics_info = format!(" │ VMAF:{v:.2} UV:{chroma_avg:.2}");
                }

                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_4,
                    &format!(
                        "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}%{metrics_info} │ \
                         {step_info}",
                        crf_pass_prefix(),
                        step_info = if current_step > base_step + 0.001 {
                            format!("SPRINT step {current_step:.2}")
                        } else {
                            "0.01-GRANULARITY GAIN".to_string()
                        }
                    )
                );

                if crate::float_compare::approx_eq_crf(test_crf, 0.0) {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_4,
                        "CRF 0.00 reached — physical lossless floor touched."
                    );
                    break;
                }

                let distance_to_floor = test_crf - search_floor;
                let decel_multiplier = 1.0_f32; // ultimate_mode is true here
                let boundary_nearing = distance_to_floor < current_step * decel_multiplier;

                if size_delta > 1.0_f64 && current_step > base_step + 0.001 {
                    let old_step = current_step;
                    current_step = base_step;
                    consecutive_successes = 0_i32;
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_4,
                        &format!(
                            "Search Decelerating (slope Δ{size_delta:.1} detected, step reset: \
                             {old_step:.3} → {current_step:.3})"
                        )
                    );
                } else if boundary_nearing && current_step > base_step + 0.001 {
                    let old_step = current_step;
                    current_step = (current_step / 2.0).max(base_step);
                    consecutive_successes = 0_i32;
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_4,
                        &format!(
                            "Smart deceleration: step {old_step:.3} → {current_step:.3} (floor in \
                             {distance_to_floor:.2})"
                        )
                    );
                } else if consecutive_successes >= 2_i32 && current_step < max_sprint_step {
                    let old_step = current_step;
                    current_step = (current_step * 2.0).min(max_sprint_step);
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_4,
                        &format!("Sprint activated: step {old_step:.3} → {current_step:.3}")
                    );
                }

                last_size_pct = pure_media_size_pct;
                test_crf -= current_step;
            } else {
                fine_failures += 1_i32;
                consecutive_successes = 0_i32;

                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_4,
                    &format!(
                        "{} [CPU] CRF {test_crf:<5.2} {pure_media_size_pct:6.1}% │ CAPACITY EXCEEDED \
                         ({fine_failures}/{max_fine_failures})",
                        crf_fail_prefix(),
                    )
                );

                if current_step > base_step + 0.001
                    && backtrack_count < PHASE4_MAX_BACKTRACK_RETRIES
                {
                    let old_step = current_step;
                    current_step = (current_step / 2.0).max(base_step);
                    backtrack_count += 1;
                    consecutive_successes = 0_i32;
                    test_crf = current_best - current_step;
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_STRATEGY,
                        &format!(
                            "Backtracking for extreme precision (retry \
                             {backtrack_count}/{PHASE4_MAX_BACKTRACK_RETRIES}): {old_step:.3} → \
                             {current_step:.3}"
                        )
                    );
                    continue;
                }

                if current_step <= base_step + 0.001 {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_4,
                        "Convergence achieved! Lower CRF sizes exceed limits. Stopping Phase 4."
                    );

                    if should_probe_crf_zero_from_phase4(current_best)
                        && !self.size_cache.contains_key(0.0)
                    {
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_PHASE_4,
                            "Ultimate fallback: forcing final check at CRF 0.00 (lossless floor)"
                        );
                        test_crf = 0.0;
                        continue;
                    }
                    break;
                }

                current_step = (current_step / 2.0).max(base_step);
                test_crf = current_best - current_step;
            }
        }

        if phase4_attempt_cap_hit {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_coarse",
                format!("Phase 4 attempt cap ({PHASE4_MAX_ATTEMPTS}) reached. Stopping."),
            );
        }

        // Mandatory CRF=0 probe (ultimate mode only): guarantee we touch the floor
        // when our best is close enough that the physical wall is plausibly nearby.
        if should_probe_crf_zero_from_phase4(current_best) && self.iterations < 200 {
            let crf0_untested = !self.size_cache.contains_key(0.0_f32);
            if crf0_untested {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_4,
                    "Forcing mandatory CRF 0.00 probe (floor guarantee)"
                );
                let size = self.encode_cached(0.0)?;
                let pure_media_size_pct = self.pure_media_size_pct(size);
                if self.candidate_fits(size) {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_4,
                        &format!(
                            "{} [CPU] CRF 0.00 {pure_media_size_pct:6.1}% │ 0.01-GRANULARITY GAIN",
                            crf_pass_prefix(),
                        )
                    );
                    current_best = 0.0;
                    current_best_size = size;
                } else {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_4,
                        &format!(
                            "{} [CPU] CRF 0.00 {pure_media_size_pct:6.1}% │ CAPACITY EXCEEDED at floor",
                            crf_fail_prefix(),
                        )
                    );
                }
            } else if let Some(&cached_size) = self.size_cache.get(0.0_f32)
                && self.candidate_fits(cached_size)
                && current_best > 0.0
            {
                current_best = 0.0;
                current_best_size = cached_size;
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_4,
                    "CRF 0.00 already in cache and compresses — set as best."
                );
            }
        } else if current_best > crate::constants::EXPLORATION_PHASE4_MAX_DISTANCE {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_4,
                &format!(
                    "Skipping CRF 0.00 probe: best CRF {current_best:.2} is not near the floor."
                )
            );
        }

        self.best_crf = Some(current_best);
        self.best_size = Some(current_best_size);

        Ok(())
    }

    fn encode_final_domain(&mut self, crf: f32) -> Result<u64> {
        let size = self.fine_tune_encoder.encode_full(
            crf,
            AnimatedExplorationEncodeMode::FullTimeline,
            self.final_output_preset,
        )?;
        self.rendered_candidate = Some(RenderedCandidate {
            crf,
            mode: AnimatedExplorationEncodeMode::FullTimeline,
            preset: self.final_output_preset,
        });
        self.iterations = self.iterations.saturating_add(1);
        Ok(size)
    }

    const fn classify_final_domain_size(&self, size: u64) -> ProbeOutcome<u64, String> {
        if self.candidate_fits(size) {
            ProbeOutcome::Fits(size)
        } else {
            ProbeOutcome::Oversize(size)
        }
    }

    fn probe_final_domain_preserving_best(
        &mut self,
        crf: f32,
        backup_path: &Path,
    ) -> Result<ProbeOutcome<u64, String>> {
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "gpu_coarse_final_domain_stale_backup",
            backup_path,
        );
        let backup_candidate = self.rendered_candidate;
        std::fs::rename(self.output, backup_path).with_context(|| {
            format!(
                "Final-domain backup rename failed (src={}, dst={})",
                self.output.display(),
                backup_path.display()
            )
        })?;

        let outcome = match self.encode_final_domain(crf) {
            Ok(size) => self.classify_final_domain_size(size),
            Err(error) => ProbeOutcome::Failed(error.to_string()),
        };
        if matches!(outcome, ProbeOutcome::Fits(_)) {
            crate::media_conversion_gate::delivery_remove_file_or_audit(
                "gpu_coarse_final_domain_backup_discard",
                backup_path,
            );
            return Ok(outcome);
        }

        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "gpu_coarse_final_domain_probe_discard",
            self.output,
        );
        if !crate::media_conversion_gate::delivery_rename_or_audit(
            "gpu_coarse_final_domain_restore",
            backup_path,
            self.output,
        ) {
            bail!(
                "Failed to restore final-domain best product from {} to {}",
                backup_path.display(),
                self.output.display()
            );
        }
        self.rendered_candidate = backup_candidate;
        Ok(outcome)
    }

    /// Reconcile the search outcome into a concrete (CRF, output size) pair.
    /// Promotes `best_crf` to a final value (or falls back to `max_crf` when
    /// search never produced one), runs the optional preset upgrade encode,
    /// and reports whether Phase 5 should follow.
    fn prepare_final_settlement(&mut self) -> Result<(f32, u64, bool)> {
        let (final_crf, mut final_pure_media_size) = match (self.best_crf, self.best_size) {
            (Some(crf), Some(size)) => {
                if self.candidate_fits(size) {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_PHASE_3,
                        &format!("Best CRF {crf:.2} selected from search history")
                    );
                } else {
                    crate::media_conversion_gate::explore_gpu_coarse_audit(
                        "explore_gpu_crf",
                        self.input,
                        format!(
                            "Best tested CRF {crf:.2} yielded larger file (+{pct:+.1}%)",
                            pct = self.pure_media_size_pct(size)
                        ),
                    );
                }
                (crf, size)
            }
            (Some(_), None) | (None, Some(_)) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_crf",
                    "Search ended with an unpaired best CRF/size; refusing to fabricate a \
                     settlement candidate",
                );
                bail!("GPU coarse search ended with incomplete best candidate state");
            }
            (None, None) => {
                if self.early_insight_triggered {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_crf",
                        "Early insight ended search without a measured candidate; refusing to \
                         invent a max-CRF size",
                    );
                    bail!("Early insight ended without a measured settlement candidate");
                }
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "Fallback: using max CRF {max:.2} (no better compression found)",
                        max = self.max_crf
                    )
                );

                let last_output_pure_media =
                    crate::stream_size::measure_strict_pure_media(self.output)
                        .with_context(|| {
                            format!(
                                "Strict pure-media output measurement failed for {}",
                                self.output.display()
                            )
                        })?
                        .pure_media_size();
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    &format!(
                        "Pure media: input {in_b} vs output {out_b} ({pct:+.1}%)",
                        in_b = crate::format_bytes(self.input_pure_media_size),
                        out_b = crate::format_bytes(last_output_pure_media),
                        pct = stream_size_change_pct(
                            last_output_pure_media,
                            self.input_pure_media_size
                        )
                    )
                );
                let max_crf = self.max_crf;
                let size = self.encode_cached(max_crf)?;
                (max_crf, size)
            }
        };

        // A search coordinate is a locator only when preset or timeline changes.
        // Materialize it in the final domain, then let Phase 5 establish a fresh
        // final-domain bracket.  Early-insight termination does not waive this
        // contract.
        let run_phase5 = requires_final_domain_calibration(
            self.encoder,
            self.preset,
            self.exploration_mode,
            self.final_output_preset,
            AnimatedExplorationEncodeMode::FullTimeline,
        );
        let (settlement_mode, settlement_preset) = if run_phase5 {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!(
                    "Search CRF {final_crf:.2} is a locator only; calibrating full-timeline preset {}",
                    self.final_output_preset
                        .hevc_name_for_archive(self.archive_mode)
                )
            );
            (
                AnimatedExplorationEncodeMode::FullTimeline,
                self.final_output_preset,
            )
        } else {
            (self.exploration_mode, self.preset)
        };
        if !candidate_is_materialized(
            self.rendered_candidate,
            final_crf,
            settlement_mode,
            settlement_preset,
        ) {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!("Materializing selected CRF {final_crf:.2} on disk")
            );
            final_pure_media_size = if run_phase5 {
                self.encode_final_domain(final_crf)?
            } else {
                self.fine_tune_encoder
                    .encode_full(final_crf, settlement_mode, settlement_preset)?
            };
            self.rendered_candidate = Some(RenderedCandidate {
                crf: final_crf,
                mode: settlement_mode,
                preset: settlement_preset,
            });
            if !run_phase5 {
                self.iterations = self.iterations.saturating_add(1);
            }
        }

        Ok((final_crf, final_pure_media_size, run_phase5))
    }

    fn run_phase5(
        &mut self,
        mut final_crf: f32,
        mut final_pure_media_size: u64,
        run_phase5: bool,
    ) -> Result<(f32, u64)> {
        if !run_phase5 {
            return Ok((final_crf, final_pure_media_size));
        }
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_5,
            &format!(
                "Final-domain calibration with preset {} from locator CRF {final_crf:.2}",
                self.final_output_preset
                    .hevc_name_for_archive(self.archive_mode)
            )
        );
        let backup_path = self.output.with_extension(format!(
            "{}.bak",
            crate::media_conversion_gate::backup_extension_label_or_tmp(self.output)
        ));
        let mut total_attempts = 0u32;
        let mut lower_oversize = (!self.candidate_fits(final_pure_media_size)).then_some(final_crf);

        // The locator coordinate may be oversized in the final preset.  Find a
        // real fitting anchor by walking toward lower quality in that domain.
        if lower_oversize.is_some() {
            let mut step = 0.25_f32;
            let mut found_fit = false;
            while total_attempts < PHASE5_MAX_TOTAL_ATTEMPTS && final_crf < self.max_crf {
                let test_crf = (final_crf + step).min(self.max_crf);
                total_attempts = total_attempts.saturating_add(1);
                match self.encode_final_domain(test_crf) {
                    Ok(size) if self.candidate_fits(size) => {
                        final_crf = test_crf;
                        final_pure_media_size = size;
                        found_fit = true;
                        break;
                    }
                    Ok(_) => {
                        lower_oversize = Some(test_crf);
                        final_crf = test_crf;
                        step = (step * 2.0).min(4.0);
                    }
                    Err(error) => {
                        crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                            "explore_gpu_final_domain",
                            format!(
                                "Final-domain anchor failed at CRF {test_crf:.2}: {error}; boundary unchanged"
                            ),
                        );
                        final_crf = test_crf;
                        step = (step * 2.0).min(4.0);
                    }
                }
            }
            if !found_fit {
                bail!(
                    "Final encoder domain produced no verified fitting candidate through CRF {:.2}",
                    self.max_crf
                );
            }
        }

        // If the locator already fit, locate a real oversized anchor toward
        // higher quality.  Accepted candidates may be larger than the current
        // output as long as the shared size policy still passes.
        if lower_oversize.is_none() && final_crf > 0.0 {
            let mut step = 0.25_f32;
            let mut failures = 0u32;
            while total_attempts < PHASE5_MAX_TOTAL_ATTEMPTS && final_crf > 0.0 {
                let test_crf = (final_crf - step).max(0.0);
                total_attempts = total_attempts.saturating_add(1);
                match self.probe_final_domain_preserving_best(test_crf, &backup_path)? {
                    ProbeOutcome::Fits(size)
                        if final_domain_candidate_improves_quality(
                            self.size_policy(),
                            self.input_pure_media_size,
                            final_crf,
                            final_pure_media_size,
                            test_crf,
                            size,
                        ) =>
                    {
                        final_crf = test_crf;
                        final_pure_media_size = size;
                        failures = 0;
                        if crate::float_compare::approx_eq_crf(test_crf, 0.0) {
                            break;
                        }
                        step = (step * 2.0).min(4.0);
                    }
                    ProbeOutcome::Oversize(_) => {
                        lower_oversize = Some(test_crf);
                        break;
                    }
                    ProbeOutcome::Failed(reason) | ProbeOutcome::Unverifiable(reason) => {
                        failures = failures.saturating_add(1);
                        crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                            "explore_gpu_final_domain",
                            format!(
                                "Final-domain quality anchor failed at CRF {test_crf:.2}: {reason}; boundary unchanged"
                            ),
                        );
                        if failures >= PHASE5_MAX_CONSECUTIVE_FAILURES || step <= 0.01 {
                            break;
                        }
                        step = (step / 2.0).max(0.01);
                    }
                    ProbeOutcome::Fits(_) => break,
                }
            }
        }

        // Refine solely between final-domain measurements.  Probe failure
        // terminates refinement without forging either boundary.
        if let Some(mut lower) = lower_oversize {
            let mut upper = final_crf;
            while upper - lower > 0.01 && total_attempts < PHASE5_MAX_TOTAL_ATTEMPTS {
                let test_crf = (f32::midpoint(lower, upper) * 100.0).round() / 100.0;
                if test_crf <= lower || test_crf >= upper {
                    break;
                }
                total_attempts = total_attempts.saturating_add(1);
                match self.probe_final_domain_preserving_best(test_crf, &backup_path)? {
                    ProbeOutcome::Fits(size) => {
                        final_crf = test_crf;
                        final_pure_media_size = size;
                        upper = test_crf;
                    }
                    ProbeOutcome::Oversize(_) => lower = test_crf,
                    ProbeOutcome::Failed(reason) | ProbeOutcome::Unverifiable(reason) => {
                        crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                            "explore_gpu_final_domain",
                            format!(
                                "Final-domain refinement failed at CRF {test_crf:.2}: {reason}; retaining verified bracket"
                            ),
                        );
                        break;
                    }
                }
            }
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_PHASE_5,
            &format!(
                "Final-domain calibration completed: CRF {final_crf:.2}, {final_pure_media_size} bytes"
            )
        );
        if !candidate_is_materialized(
            self.rendered_candidate,
            final_crf,
            AnimatedExplorationEncodeMode::FullTimeline,
            self.final_output_preset,
        ) {
            bail!(
                "Final-domain calibration selected CRF {final_crf:.2} without a matching materialized product"
            );
        }

        Ok((final_crf, final_pure_media_size))
    }

    /// Run quality verification, package the explore result, and emit the
    /// closing logs.
    fn build_result(
        self,
        final_crf: f32,
        final_pure_media_size: u64,
    ) -> anyhow::Result<ExploreResult> {
        let CpuFineTuneSession {
            input,
            output,
            input_pure_media_size,
            input_is_animated_image_like,
            ultimate_mode,
            duration,
            allow_size_tolerance,
            min_ssim,
            iterations,
            early_insight_triggered,
            cpu_progress,
            tracking,
            ..
        } = self;

        let size_policy = SizePolicy::strict_or_allow_growth(
            allow_size_tolerance,
            crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
        );

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DETECTION,
            &format!(
                "Final: CRF {final_crf:.2} | Pure media: {final_pure_media_size} bytes ({mb:.2} MB)",
                mb = crate::numeric_cast::u64_to_f64(final_pure_media_size)
                    / 1_024.0_f64
                    / 1_024.0_f64
            )
        );

        let (ssim, lossless_integrity_ok) = Self::evaluate_metrics_and_integrity(
            input,
            output,
            final_pure_media_size,
            ultimate_mode,
            input_is_animated_image_like,
            final_crf,
        )?;

        let size_change_pct =
            super::calc_change_pct_for_input_size(input_pure_media_size, final_pure_media_size);

        let pure_media_compressed = size_policy.fits(final_pure_media_size, input_pure_media_size);
        let ssim_ok = ssim.is_some_and(|s| s >= min_ssim);
        let integrity_gate_ok = lossless_integrity_ok == Some(true);
        let mut quality_passed = if ultimate_mode {
            pure_media_compressed
        } else if lossless_integrity_ok.is_some() {
            pure_media_compressed && integrity_gate_ok
        } else {
            pure_media_compressed && ssim_ok
        };

        let (confidence, confidence_detail) = Self::calculate_exploration_confidence(
            ultimate_mode,
            duration,
            iterations,
            ssim,
            min_ssim,
            input_pure_media_size,
            final_pure_media_size,
            tracking,
            &mut quality_passed,
            lossless_integrity_ok,
        );

        let _result_color = if quality_passed {
            BRIGHT_GREEN
        } else if pure_media_compressed {
            BRIGHT_YELLOW
        } else {
            BRIGHT_RED
        };
        let ok = crate::media_conversion_gate::ui_icon_pick(
            crate::modern_ui::symbols::SUCCESS,
            crate::modern_ui::symbols::plain::SUCCESS,
        );
        let err = crate::media_conversion_gate::ui_icon_pick(
            crate::modern_ui::symbols::ERROR,
            crate::modern_ui::symbols::plain::ERROR,
        );
        let result_prefix = if ultimate_mode && quality_passed {
            format!("{ok} {}", crate::infra::static_logs::messages::VAL_READY)
        } else if quality_passed {
            format!("{ok} {}", crate::infra::static_logs::messages::VAL_SUCCESS)
        } else {
            format!("{err} {}", crate::infra::static_logs::messages::VAL_FAILED)
        };

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DETECTION,
            &format!(
                "[FINISH] {result_prefix}: CRF {final_crf:.2} │ Size {size_change_pct:+.1}% │ \
                 Iterations: {iterations}"
            )
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DETECTION,
            &format!(
                "Pure media smaller than input: {}",
                if pure_media_compressed { "YES" } else { "NO" }
            )
        );

        let input_measurement =
            crate::stream_size::measure_strict_pure_media(input).with_context(|| {
                format!(
                    "Strict pure-media verification failed for {}",
                    input.display()
                )
            })?;
        let output_measurement = crate::stream_size::measure_strict_pure_media(output)
            .with_context(|| {
                format!(
                    "Strict pure-media verification failed for {}",
                    output.display()
                )
            })?;
        if input_measurement.pure_media_size() != input_pure_media_size
            || output_measurement.pure_media_size() != final_pure_media_size
        {
            anyhow::bail!(
                "Pure-media size changed during exploration settlement: input {} -> {}, output {} -> {}",
                input_pure_media_size,
                input_measurement.pure_media_size(),
                final_pure_media_size,
                output_measurement.pure_media_size()
            );
        }
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DETECTION,
            &format!(
                "Pure media: {in_b} → {out_b} ({size_change_pct:+.1}%)",
                in_b = crate::format_bytes(input_pure_media_size),
                out_b = crate::format_bytes(final_pure_media_size)
            )
        );
        let output_container_overhead = output_measurement
            .total_file_size
            .saturating_sub(final_pure_media_size);

        let is_animated_image = input_is_animated_image_like;

        let verify_options = if is_animated_image {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DETECTION,
                &format!(
                    "{} Animated image detected, using relaxed duration tolerance",
                    crate::media_conversion_gate::ui_icon_pick("🎞️", "[ANIM]")
                )
            );
            crate::quality_verifier_enhanced::VerifyOptions::relaxed_animated_image()
        } else {
            crate::quality_verifier_enhanced::VerifyOptions::strict_video()
        };

        let enhanced =
            crate::quality_verifier_enhanced::verify_after_encode(input, output, &verify_options);
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_DETECTION,
            &format!("Enhanced Verification Summary: {}", enhanced.summary())
        );
        for d in &enhanced.details {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DETECTION,
                &format!("   Detail: {d}")
            );
        }
        let enhanced_ok = enhanced.passed();
        let mut enhanced_verify_fail_reason = (!enhanced_ok).then_some(enhanced.message);
        let quality_passed = quality_passed && enhanced_ok;
        let quality_passed_check = if ultimate_mode {
            if !pure_media_compressed {
                CheckResult::Failed("Pure media not smaller than input".into())
            } else if let Some(reason) = enhanced_verify_fail_reason.take() {
                CheckResult::Failed(reason)
            } else {
                // Phase 3 verifier promotes NotChecked → Passed/Failed after 3D metrics.
                CheckResult::NotChecked
            }
        } else if quality_passed {
            CheckResult::Passed
        } else if !pure_media_compressed {
            CheckResult::Failed("Pure media not smaller than input".into())
        } else if let Some(reason) = enhanced_verify_fail_reason.take() {
            CheckResult::Failed(reason)
        } else if lossless_integrity_ok == Some(false) {
            CheckResult::Failed("Lossless integrity check failed".into())
        } else if lossless_integrity_ok == Some(true) {
            CheckResult::Passed
        } else if !ssim_ok {
            CheckResult::Failed("SSIM below target".into())
        } else {
            CheckResult::Failed("Quality gate failed".into())
        };

        let total_file_pct = super::calc_change_pct_for_input_size(
            input_measurement.total_file_size,
            output_measurement.total_file_size,
        );
        let output_overhead_pct = if output_measurement.total_file_size == 0 {
            0.0
        } else {
            crate::numeric_cast::u64_to_f64(output_container_overhead)
                / crate::numeric_cast::u64_to_f64(output_measurement.total_file_size)
                * 100.0
        };
        if output_overhead_pct > 10.0 {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_coarse",
                format!("Container/metadata overhead: {output_overhead_pct:.1}%"),
            );
        }
        if size_change_pct < 0.0_f64 && total_file_pct > 0.0_f64 {
            crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                "explore_gpu_coarse",
                format!(
                    "Pure media compressed ({size_change_pct:+.1}%) while total file grew ({total_file_pct:+.1}%)"
                ),
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DETECTION,
                &format!(
                    "Container/metadata overhead: {overhead} ({output_overhead_pct:.1}% of output)",
                    overhead = crate::format_bytes(output_container_overhead)
                )
            );
        }

        confidence_detail.print_report();

        cpu_progress.finish_iteration(final_crf, final_pure_media_size, ssim);

        Ok(ExploreResult {
            optimal_crf: final_crf,
            output_size: output_measurement.total_file_size,
            size_change_pct,
            ssim,
            psnr: None,
            ms_ssim: None,
            ms_ssim_passed: match lossless_integrity_ok {
                Some(true) => CheckResult::Passed,
                Some(false) => CheckResult::Failed("Lossless integrity check failed".into()),
                None => CheckResult::NotChecked,
            },
            ultimate_quality_passed: CheckResult::NotChecked,
            ms_ssim_score: None,
            used_fallback: false,
            iterations,
            size_target_met: if pure_media_compressed {
                CheckResult::Passed
            } else {
                CheckResult::Failed("Pure media not smaller than input".into())
            },
            quality_passed: quality_passed_check,
            enhanced_verify_fail_reason,
            log: Vec::new(),
            confidence,
            confidence_detail,
            actual_min_ssim: min_ssim,
            input_pure_media_size,
            output_pure_media_size: final_pure_media_size,
            container_overhead: output_container_overhead,
            // Search metrics belong to locator candidates.  The verifier fills
            // these fields from the materialized final product.
            vmaf_y_score: None,
            cambi_score: None,
            psnr_uv_score: None,
            early_insight_triggered,
            ultimate_mode,
        })
    }

    fn evaluate_metrics_and_integrity(
        input: &std::path::Path,
        output: &std::path::Path,
        final_pure_media_size: u64,
        ultimate_mode: bool,
        input_is_animated_image_like: bool,
        final_crf: f32,
    ) -> anyhow::Result<(Option<f64>, Option<bool>)> {
        let (ssim, lossless_integrity_ok) = if ultimate_mode {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DETECTION,
                "Ultimate mode: skipping SSIM in settle phase; final 3D gate owns quality \
                 validation"
            );
            (None, None)
        } else if input_is_animated_image_like
            && crate::float_compare::approx_eq_crf(final_crf, 0.0)
        {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DETECTION,
                "GIF CRF=0 (lossless): skipping SSIM/VMAF — running integrity check instead"
            );
            let integrity_ok = match super::stream_analysis::check_lossless_integrity(
                input,
                output,
                final_pure_media_size,
                true,
            ) {
                Ok(v) => v,
                Err(e) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "explore_gpu_integrity",
                        format!("Lossless integrity check failed to execute: {e}"),
                    );
                    false
                }
            };
            if integrity_ok {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    "INTEGRITY CHECK: PASSED"
                );
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "lossless_integrity_quality_gate",
                    "GIF CRF=0: quality gate uses integrity check, not measured SSIM",
                );
            } else {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "explore_gpu_integrity",
                    "INTEGRITY CHECK: FAILED (possible encode error)",
                );
            }
            (None, Some(integrity_ok))
        } else {
            (calculate_ssim_enhanced(input, output)?, None)
        };

        if let Some(s) = ssim {
            let ok = crate::media_conversion_gate::ui_icon_pick(
                crate::modern_ui::symbols::SUCCESS,
                crate::modern_ui::symbols::plain::SUCCESS,
            );
            let quality_hint = if s >= crate::constants::SSIM_GRADE_EXCELLENT {
                format!(
                    "{ok} {}",
                    crate::infra::static_logs::messages::VAL_EXCELLENT
                )
            } else if s >= crate::constants::SSIM_GRADE_GOOD {
                format!(
                    "{ok} {}",
                    crate::infra::static_logs::messages::VAL_VERY_GOOD
                )
            } else if s >= crate::constants::SSIM_GRADE_ACCEPTABLE {
                crate::infra::static_logs::messages::VAL_GOOD.to_string()
            } else {
                "Below threshold".to_string()
            };
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!("SSIM: {s:.6} {quality_hint}")
            );
        } else if let Some(integrity_ok) = lossless_integrity_ok {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                &format!(
                    "SSIM not measured (lossless integrity gate: {})",
                    if integrity_ok { "passed" } else { "failed" }
                )
            );
        } else {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_PHASE_3,
                "SSIM calculation skipped or unavailable"
            );
        }

        Ok((ssim, lossless_integrity_ok))
    }

    fn calculate_exploration_confidence(
        ultimate_mode: bool,
        duration: f32,
        iterations: u32,
        ssim: Option<f64>,
        min_ssim: f64,
        input_size: u64,
        final_pure_media_size: u64,
        tracking: &TrackingState,
        quality_passed: &mut bool,
        lossless_integrity_ok: Option<bool>,
    ) -> (Option<f64>, super::ConfidenceBreakdown) {
        if ultimate_mode {
            let max_iter = calculate_max_iterations_for_duration(duration, true);
            let (conf, detail) = super::measured_exploration_confidence_ultimate(
                tracking.best_vmaf,
                tracking.best_psnr_uv,
                iterations,
                max_iter.max(1),
            );
            let confidence_detail = detail;
            let overall_confidence = conf;
            let confidence = if overall_confidence.is_none() {
                tracing::warn!(
                    target: "mfb.algorithm",
                    pipeline = "video_exploration",
                    branch = "overall_confidence_rejected",
                    "GPU coarse search overall confidence rejected (non-finite aggregate)"
                );
                if lossless_integrity_ok != Some(true) {
                    *quality_passed = false;
                }
                None
            } else {
                overall_confidence
            };

            (confidence, confidence_detail)
        } else {
            let max_iter = calculate_max_iterations_for_duration(duration, false).max(1);
            let (mut overall_confidence, mut confidence_detail) =
                super::measured_exploration_confidence(ssim, min_ssim, iterations, max_iter);
            if let Some(size_margin) =
                super::exploration_size_margin_from_output(input_size, final_pure_media_size)
            {
                confidence_detail.margin_safety = Some(
                    confidence_detail
                        .margin_safety
                        .map_or(size_margin, |existing| existing.max(size_margin)),
                );
                overall_confidence = confidence_detail.overall();
            }
            let confidence = if overall_confidence.is_none() {
                tracing::warn!(
                    target: "mfb.algorithm",
                    pipeline = "video_exploration",
                    branch = "overall_confidence_rejected",
                    "GPU coarse search overall confidence rejected (non-finite aggregate)"
                );
                if lossless_integrity_ok != Some(true) {
                    *quality_passed = false;
                }
                None
            } else {
                overall_confidence
            };

            (confidence, confidence_detail)
        }
    }
}

fn search_anchor_crf(baseline_crf: f32, warm_start_crf: Option<f32>, max_crf: f32) -> f32 {
    if warm_start_crf.is_none() {
        crate::media_conversion_gate::explore_gpu_coarse_fallback_audit(
            "search_anchor_crf",
            format!("missing warm_start_crf; using baseline_crf {baseline_crf:.2}"),
        );
    }
    crate::media_conversion_gate::explore_search_anchor_crf_or_baseline(
        warm_start_crf,
        baseline_crf,
    )
    .clamp(ABSOLUTE_MIN_CRF, max_crf)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HevcPresetPlan {
    search_preset: EncoderPreset,
    final_output_preset: EncoderPreset,
}

fn hevc_preset_plan(
    requested_preset: EncoderPreset,
    ultimate_mode: bool,
    archive_mode: bool,
) -> HevcPresetPlan {
    let final_output_preset = if archive_mode {
        EncoderPreset::Veryslow
    } else {
        requested_preset.sanitize_hevc()
    };
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av1PresetPlan {
    search_preset: EncoderPreset,
    final_output_preset: EncoderPreset,
}

fn av1_preset_plan(
    requested_preset: EncoderPreset,
    ultimate_mode: bool,
    archive_mode: bool,
) -> Av1PresetPlan {
    let final_output_preset = if archive_mode {
        EncoderPreset::Veryslow
    } else {
        requested_preset.sanitize_av1()
    };
    let search_preset = if ultimate_mode && final_output_preset == EncoderPreset::Slower {
        EncoderPreset::Slow
    } else {
        final_output_preset
    };

    Av1PresetPlan {
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
    explore(GpuSearchArgs {
        input: &req.input,
        output: output_path,
        encoder: VideoEncoder::Hevc,
        vf_args: req.vf_args.clone(),
        initial_crf: initial_crf.clamp(ABSOLUTE_MIN_CRF, max_crf),
        max_crf,
        min_ssim,
        flags: req.flags.clone(),
        max_threads: req.max_threads,
        hdr_x265_params: req.hdr_x265_params.clone(),
        preset: search_preset,
        final_output_preset,
    })
}

/// Unified HEVC quality exploration with GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_hevc_with_gpu(req: &GpuSearchRequest) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(req.baseline_crf, VideoEncoder::Hevc);
    let screening_anchor = search_anchor_crf(req.baseline_crf, req.warm_start_crf, max_crf);
    let plan = hevc_preset_plan(
        req.preset,
        req.flags.features.ultimate_mode,
        req.flags.features.archive_mode,
    );

    if plan.search_preset != plan.final_output_preset {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_STRATEGY,
            &format!(
                "HEVC Ultimate pipeline: search preset {s} → final preset {f} at settled CRF",
                s = plan
                    .search_preset
                    .hevc_name_for_archive(req.flags.features.archive_mode),
                f = plan
                    .final_output_preset
                    .hevc_name_for_archive(req.flags.features.archive_mode)
            )
        );
    }

    run_hevc_gpu_search(
        req,
        plan.search_preset,
        plan.final_output_preset,
        screening_anchor,
    )
}

/// Unified AV1 quality exploration with GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_av1_with_gpu(req: &GpuSearchRequest) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(req.baseline_crf, VideoEncoder::Av1);
    let search_anchor_crf = search_anchor_crf(req.baseline_crf, req.warm_start_crf, max_crf);
    let plan = av1_preset_plan(
        req.preset,
        req.flags.features.ultimate_mode,
        req.flags.features.archive_mode,
    );

    if plan.search_preset != plan.final_output_preset {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_STRATEGY,
            &format!(
                "AV1 Ultimate pipeline: search preset {s} → final preset {f} at settled CRF",
                s = plan.search_preset.x26x_name(),
                f = plan.final_output_preset.x26x_name()
            )
        );
    }

    explore(GpuSearchArgs {
        input: &req.input,
        output: &req.output,
        encoder: VideoEncoder::Av1,
        vf_args: req.vf_args.clone(),
        initial_crf: search_anchor_crf.clamp(ABSOLUTE_MIN_CRF, max_crf),
        max_crf,
        min_ssim,
        flags: req.flags.clone(),
        max_threads: req.max_threads,
        hdr_x265_params: None, // AV1 doesn't use x265 params
        preset: plan.search_preset,
        final_output_preset: plan.final_output_preset,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AnimatedInputProfile, ExplorationSamplingProfile, FineTuneEncoder, FineTuneQualityMode,
        GifSourceProfile, StreamMappingMode, UltimateQualityBaselines, UltimateQualityMetrics,
        adaptive_cambi_ceiling, av1_preset_plan, build_color_args_from_probe,
        evaluate_ultimate_quality_gate, format_quality_check_line, hevc_preset_plan, pick_pix_fmt,
        search_anchor_crf, should_probe_crf_zero_from_phase4,
    };
    use crate::constants::EXPLORATION_CAMBI_MAX;
    use crate::ffprobe::{FFprobeAudioInfo, FFprobeHdrInfo, FFprobeResult, FFprobeSubtitleInfo};
    use crate::types::CheckResult;
    use crate::types::EncoderPreset;
    use crate::video_explorer::ExploreResult;
    use crate::video_explorer::VideoEncoder;
    const ABSOLUTE_MIN_CRF: f32 = super::ABSOLUTE_MIN_CRF;

    fn metrics_below_ultimate_sanity_floor(vmaf_y: f64, psnr_uv: (f64, f64)) -> bool {
        vmaf_y < crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR
            || psnr_uv.0.min(psnr_uv.1) < crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR
    }

    fn both_metrics_below_ultimate_sanity_floor(vmaf_y: f64, psnr_uv: (f64, f64)) -> bool {
        vmaf_y < crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR
            && psnr_uv.0.min(psnr_uv.1) < crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR
    }

    fn mock_probe() -> FFprobeResult {
        FFprobeResult {
            format_name: "mov".to_string(),
            duration: Some(1.0),
            size: 1,
            bit_rate: None,
            video_codec: "hevc".to_string(),
            video_codec_long: "H.265 / HEVC".to_string(),
            width: 3840,
            height: 2160,
            frame_rate: Some(24.0),
            avg_frame_rate: Some(24.0),
            frame_count: Some(24),
            pix_fmt: "yuv420p".to_string(),
            color_space: Some("bt709".to_string()),
            color_transfer: None,
            color_primaries: Some("bt709".to_string()),
            bit_depth: None,
            bit_depth_inferred_from_pix_fmt: false,
            audio: FFprobeAudioInfo::default(),
            profile: None,
            level: None,
            max_b_frames: Some(0),
            encoder_settings: None,
            video_bit_rate: None,
            refs: None,
            hdr: FFprobeHdrInfo::default(),
            subtitles: FFprobeSubtitleInfo::default(),
            is_variable_frame_rate: false,
            stream_index: 0,
            tags: std::collections::HashMap::new(),
            loop_count: None,
            frame_types: Vec::new(),
            pts_deltas: Vec::new(),
            mv_magnitudes: Vec::new(),
            pkt_sizes: Vec::new(),
        }
    }

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
    fn test_pick_pix_fmt_uses_10_bit_for_hdr_transfer_without_explicit_depth() {
        let mut probe = mock_probe();
        probe.color_transfer = Some(crate::constants::HDR_TRANSFER_PQ.to_string());
        probe.color_space = Some("bt2020nc".to_string());
        probe.color_primaries = Some("bt2020".to_string());

        assert_eq!(pick_pix_fmt(&probe), "yuv420p10le");
    }

    #[test]
    fn test_pick_pix_fmt_preserves_inferred_10_bit_precision() {
        let mut probe = mock_probe();
        probe.bit_depth = Some(10);
        probe.bit_depth_inferred_from_pix_fmt = true;
        probe.pix_fmt = "yuv420p10le".to_string();

        assert_eq!(pick_pix_fmt(&probe), "yuv420p10le");
    }

    #[test]
    fn test_build_color_args_from_probe_normalizes_yuv_colorspace() {
        let mut probe = mock_probe();
        probe.color_space = Some("bt2020_ncl".to_string());
        probe.color_transfer = Some(crate::constants::HDR_TRANSFER_PQ.to_string());
        probe.color_primaries = Some("bt2020".to_string());

        assert_eq!(
            build_color_args_from_probe(&probe),
            vec![
                "-colorspace",
                "bt2020nc",
                "-color_trc",
                crate::constants::HDR_TRANSFER_PQ,
                "-color_primaries",
                "bt2020"
            ]
        );
    }

    #[test]
    fn test_build_color_args_from_probe_skips_rgb_matrix_for_yuv_output() {
        let mut probe = mock_probe();
        probe.color_space = Some("rgb".to_string());
        probe.color_transfer = Some("bt709".to_string());
        probe.color_primaries = Some("bt709".to_string());

        assert_eq!(
            build_color_args_from_probe(&probe),
            vec!["-color_trc", "bt709", "-color_primaries", "bt709"]
        );
    }

    #[test]
    fn test_inject_hdr_metadata_skips_hlg_static_metadata() {
        let mut probe = mock_probe();
        probe.pix_fmt = "yuv420p10le".to_string();
        probe.color_space = Some("bt2020nc".to_string());
        probe.color_transfer = Some(crate::constants::HDR_TRANSFER_HLG.to_string());
        probe.color_primaries = Some("bt2020".to_string());
        probe.hdr = FFprobeHdrInfo {
            mastering_display: Some(
                "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)".to_string(),
            ),
            max_cll: Some("1000,400".to_string()),
            ..Default::default()
        };

        let encoder = FineTuneEncoder {
            input: std::path::Path::new("/tmp/input.mp4"),
            output: std::path::Path::new("/tmp/output.mp4"),
            encoder: VideoEncoder::Hevc,
            vf_args: Vec::new(),
            max_threads: 1,
            hdr_x265_params: None,
            probe_info: Some(&probe),
            apple_compat: false,
            archive_mode: false,
            input_size: 1,
            duration: 1.0,
            quality_mode: FineTuneQualityMode::Standard,
            stream_mapping: StreamMappingMode::AllStreams,
            animated_input: AnimatedInputProfile::Regular,
            gif_source: GifSourceProfile::Other,
            sampling_profile: ExplorationSamplingProfile::FullTimeline,
            audio_strategy: super::AudioTranscodeStrategy::Copy,
            pts_integrity: crate::ffprobe_json::PtsIntegrity::Healthy,
            progress_host: None,
        };

        assert_eq!(encoder.inject_hdr_metadata(None), None);
        assert_eq!(
            encoder.inject_hdr_metadata(Some("bframes=0".to_string())),
            Some("bframes=0".to_string())
        );
    }

    #[test]
    fn test_inject_hdr_metadata_backfills_hdr10_flags_for_hdr10plus_json() {
        let mut probe = mock_probe();
        probe.pix_fmt = "yuv420p10le".to_string();
        probe.color_space = Some("bt2020nc".to_string());
        probe.color_transfer = Some(crate::constants::HDR_TRANSFER_PQ.to_string());
        probe.color_primaries = Some("bt2020".to_string());

        let encoder = FineTuneEncoder {
            input: std::path::Path::new("/tmp/input.mp4"),
            output: std::path::Path::new("/tmp/output.mp4"),
            encoder: VideoEncoder::Hevc,
            vf_args: Vec::new(),
            max_threads: 1,
            hdr_x265_params: Some("dhdr10-info=/tmp/hdr10plus.json".to_string()),
            probe_info: Some(&probe),
            apple_compat: false,
            archive_mode: false,
            input_size: 1,
            duration: 1.0,
            quality_mode: FineTuneQualityMode::Standard,
            stream_mapping: StreamMappingMode::AllStreams,
            animated_input: AnimatedInputProfile::Regular,
            gif_source: GifSourceProfile::Other,
            sampling_profile: ExplorationSamplingProfile::FullTimeline,
            audio_strategy: super::AudioTranscodeStrategy::Copy,
            pts_integrity: crate::ffprobe_json::PtsIntegrity::Healthy,
            progress_host: None,
        };

        assert_eq!(
            encoder.inject_hdr_metadata(Some("dhdr10-info=/tmp/hdr10plus.json".to_string())),
            Some("dhdr10-info=/tmp/hdr10plus.json:hdr10=1:hdr-opt=1:repeat-headers=1".to_string())
        );
    }

    #[test]
    fn test_inject_hdr_metadata_backfills_hdr10_flags_for_pq_probe_without_existing_params() {
        let mut probe = mock_probe();
        probe.pix_fmt = "yuv420p10le".to_string();
        probe.color_space = Some("bt2020nc".to_string());
        probe.color_transfer = Some(crate::constants::HDR_TRANSFER_PQ.to_string());
        probe.color_primaries = Some("bt2020".to_string());

        let encoder = FineTuneEncoder {
            input: std::path::Path::new("/tmp/input.mp4"),
            output: std::path::Path::new("/tmp/output.mp4"),
            encoder: VideoEncoder::Hevc,
            vf_args: Vec::new(),
            max_threads: 1,
            hdr_x265_params: None,
            probe_info: Some(&probe),
            apple_compat: false,
            archive_mode: false,
            input_size: 1,
            duration: 1.0,
            quality_mode: FineTuneQualityMode::Standard,
            stream_mapping: StreamMappingMode::AllStreams,
            animated_input: AnimatedInputProfile::Regular,
            gif_source: GifSourceProfile::Other,
            sampling_profile: ExplorationSamplingProfile::FullTimeline,
            audio_strategy: super::AudioTranscodeStrategy::Copy,
            pts_integrity: crate::ffprobe_json::PtsIntegrity::Healthy,
            progress_host: None,
        };

        assert_eq!(
            encoder.inject_hdr_metadata(None),
            Some("hdr10=1:hdr-opt=1:repeat-headers=1".to_string())
        );
    }

    #[test]
    fn test_hevc_preset_plan_uses_single_pipeline_for_ultimate_slower() {
        let plan = hevc_preset_plan(EncoderPreset::Slower, true, false);

        assert_eq!(plan.search_preset, EncoderPreset::Slow);
        assert_eq!(plan.final_output_preset, EncoderPreset::Slower);
    }

    #[test]
    fn test_hevc_preset_plan_keeps_same_preset_outside_ultimate_slower() {
        let normal = hevc_preset_plan(EncoderPreset::Slow, false, false);
        let ultimate_slow = hevc_preset_plan(EncoderPreset::Slow, true, false);

        assert_eq!(normal.search_preset, EncoderPreset::Slow);
        assert_eq!(normal.final_output_preset, EncoderPreset::Slow);
        assert_eq!(ultimate_slow.search_preset, EncoderPreset::Slow);
        assert_eq!(ultimate_slow.final_output_preset, EncoderPreset::Slow);
    }

    #[test]
    fn test_av1_preset_plan_parity_with_hevc_ultimate_slower() {
        let plan = av1_preset_plan(EncoderPreset::Slower, true, false);
        assert_eq!(plan.search_preset, EncoderPreset::Slow);
        assert_eq!(plan.final_output_preset, EncoderPreset::Slower);
    }

    #[test]
    fn test_av1_preset_plan_clamps_fast_presets_to_medium() {
        let plan = av1_preset_plan(EncoderPreset::Ultrafast, false, false);
        assert_eq!(plan.search_preset, EncoderPreset::Medium);
        assert_eq!(plan.final_output_preset, EncoderPreset::Medium);
    }

    #[test]
    fn test_archive_preset_plans_preserve_slowest_available() {
        let hevc = hevc_preset_plan(EncoderPreset::Medium, false, true);
        let av1 = av1_preset_plan(EncoderPreset::Medium, false, true);

        assert_eq!(hevc.search_preset, EncoderPreset::Veryslow);
        assert_eq!(hevc.final_output_preset, EncoderPreset::Veryslow);
        assert_eq!(av1.search_preset, EncoderPreset::Veryslow);
        assert_eq!(av1.final_output_preset, EncoderPreset::Veryslow);
    }

    #[test]
    fn test_adaptive_cambi_ceiling_respects_source_banding_level() {
        assert!(adaptive_cambi_ceiling(None).is_none());
        assert!(
            (adaptive_cambi_ceiling(Some(2.5_f64)).expect("cambi ceiling") - EXPLORATION_CAMBI_MAX)
                .abs()
                < f64::EPSILON
        );
        assert!((adaptive_cambi_ceiling(Some(5.5_f64)).expect("cambi") - 6.5).abs() < f64::EPSILON);
        assert!(
            (adaptive_cambi_ceiling(Some(10.0_f64)).expect("cambi") - 11.5).abs() < f64::EPSILON
        );
        assert!(
            (adaptive_cambi_ceiling(Some(20.0_f64)).expect("cambi") - 23.0).abs() < f64::EPSILON
        );
    }

    #[test]
    fn test_baseline_aware_gate_passes_when_output_stays_close_to_source_profile() {
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: Some(92.4_f64),
                psnr_uv: Some((33.8_f64, 33.6_f64)),
                cambi: Some(10.2_f64),
            },
            UltimateQualityBaselines {
                source_cambi: Some(9.0_f64),
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
                vmaf_y: Some(84.0_f64),
                psnr_uv: Some((28.5_f64, 29.0_f64)),
                cambi: Some(9.5_f64),
            },
            UltimateQualityBaselines {
                source_cambi: Some(5.0_f64),
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
                vmaf_y: Some(99.96_f64),
                psnr_uv: None, // ← calculation failed
                cambi: Some(0.01_f64),
            },
            UltimateQualityBaselines {
                source_cambi: Some(0.01_f64),
            },
        );

        assert!(
            evaluation.vmaf_ok,
            "fresh VMAF uses the invariant final floor"
        );
        assert!(evaluation.cambi_ok);
        assert!(!evaluation.chroma_ok, "None PSNR-UV must fail chroma gate");
        assert!(
            !evaluation.all_passed(),
            "Gate must fail when any metric is None or baseline absent for VMAF/PSNR"
        );
    }

    #[test]
    fn test_gate_rejects_when_vmaf_is_none() {
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: None,
                psnr_uv: Some((50.0_f64, 48.0_f64)),
                cambi: Some(1.0_f64),
            },
            UltimateQualityBaselines {
                source_cambi: Some(1.0_f64),
            },
        );

        assert!(!evaluation.vmaf_ok, "None VMAF must fail");
        assert!(!evaluation.all_passed());
    }

    #[test]
    fn test_gate_rejects_when_cambi_is_none() {
        let evaluation = evaluate_ultimate_quality_gate(
            UltimateQualityMetrics {
                vmaf_y: Some(98.0_f64),
                psnr_uv: Some((50.0_f64, 48.0_f64)),
                cambi: None,
            },
            UltimateQualityBaselines {
                source_cambi: Some(1.0_f64),
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
        assert!(metrics_below_ultimate_sanity_floor(
            80.0,
            (25.0_f64, 25.0_f64)
        ));
    }

    #[test]
    fn test_metrics_below_floor_vmaf_only_below() {
        assert!(metrics_below_ultimate_sanity_floor(
            80.0,
            (40.0_f64, 40.0_f64)
        ));
    }

    #[test]
    fn test_metrics_below_floor_psnr_only_below() {
        assert!(metrics_below_ultimate_sanity_floor(
            95.0,
            (25.0_f64, 25.0_f64)
        ));
    }

    #[test]
    fn test_metrics_below_floor_neither_below() {
        assert!(!metrics_below_ultimate_sanity_floor(
            95.0,
            (40.0_f64, 40.0_f64)
        ));
    }

    #[test]
    fn test_both_metrics_below_floor_true() {
        assert!(both_metrics_below_ultimate_sanity_floor(
            80.0,
            (25.0_f64, 25.0_f64)
        ));
    }

    #[test]
    fn test_both_metrics_below_floor_only_one() {
        assert!(!both_metrics_below_ultimate_sanity_floor(
            80.0,
            (40.0_f64, 40.0_f64)
        ));
        assert!(!both_metrics_below_ultimate_sanity_floor(
            95.0,
            (25.0_f64, 25.0_f64)
        ));
    }

    // ── build_normal_quality_evaluation ────────────────────────────────────

    #[test]
    fn test_normal_eval_passes_with_good_scores() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.98_f64),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: Some(0.97_f64),
                ssim_all: Some(0.96_f64),
            },
        );
        assert!(eval.passed);
        assert!(eval.fusion_score.is_some());
    }

    #[test]
    fn test_normal_eval_fails_with_low_scores() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.98_f64),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: Some(0.80_f64),
                ssim_all: Some(0.82_f64),
            },
        );
        assert!(!eval.passed);
    }

    #[test]
    fn test_normal_eval_none_measurements_fails() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.98_f64),
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
                explore_ssim: Some(0.96_f64),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: Some(0.95_f64),
                ssim_all: None,
            },
        );
        let score = eval
            .fusion_score
            .expect("ms_ssim_avg path must set fusion_score");
        assert!((score - 0.95).abs() < 1e-6_f64);
    }

    #[test]
    fn test_normal_eval_ssim_all_only() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: Some(0.96_f64),
                min_ssim_config: 0.90,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: None,
                ssim_all: Some(0.95_f64),
            },
        );
        let score = eval
            .fusion_score
            .expect("ssim_all path must set fusion_score");
        assert!((score - 0.95).abs() < 1e-6_f64);
    }

    #[test]
    fn test_normal_eval_no_baseline_uses_config_floor() {
        let eval = super::build_normal_quality_evaluation(
            super::NormalQualityBaseline {
                explore_ssim: None,
                min_ssim_config: 0.92,
            },
            super::NormalQualityMeasurement {
                ms_ssim_avg: Some(0.93_f64),
                ssim_all: Some(0.93_f64),
            },
        );
        assert!(eval.passed);
        // Floor should be max(0.92, 0.88) = 0.92
        assert!((eval.fusion_floor - 0.92).abs() < 1e-6_f64);
    }

    // ── adaptive floor / ceiling boundary tests ───────────────────────────

    #[test]
    fn test_adaptive_vmaf_floor_clamps_to_sanity() {
        // baseline 88.0 - 2.0 = 86.0, matching the sanity floor
        assert!(
            (crate::media_conversion_gate::explore_adaptive_vmaf_y_floor(Some(88.0_f64))
                - crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR)
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn test_adaptive_psnr_floor_clamps_to_sanity() {
        // baseline 31.0/31.2 - 1.5 would fall below 30.0, so both clamp to the sanity
        // floor
        let psnr = crate::media_conversion_gate::explore_adaptive_psnr_uv_floor_optional(Some((
            31.0_f64, 31.2_f64,
        )))
        .expect("psnr floor");
        assert!((psnr.0 - crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR).abs() < f64::EPSILON);
        assert!((psnr.1 - crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adaptive_cambi_ceiling_borderline_clean() {
        // Source CAMBI exactly at EXPLORATION_CAMBI_MAX boundary gets the clean-source
        // rise.
        let ceil = adaptive_cambi_ceiling(Some(EXPLORATION_CAMBI_MAX)).expect("cambi ceiling");
        assert!((ceil - 7.0).abs() < 1e-6_f64);
    }

    #[test]
    fn test_adaptive_cambi_ceiling_heavily_banded() {
        // Source has high banding — ceiling should use ratio
        let ceil = adaptive_cambi_ceiling(Some(40.0_f64)).expect("cambi ceiling");
        // max(1.5, 40.0 * 0.15) = 6.0, so ceiling = 46.0
        assert!((ceil - 46.0).abs() < 1e-6_f64);
    }

    #[test]
    fn format_quality_check_line_size_only_pass() {
        let result = ExploreResult {
            size_target_met: CheckResult::Passed,
            quality_passed: CheckResult::NotChecked,
            ..Default::default()
        };
        let line = format_quality_check_line(&result, false);
        assert!(line.contains("PASSED"));
        assert!(line.contains("size target"));
    }

    #[test]
    fn calc_change_pct_zero_input_is_nan_not_fabricated_zero() {
        let pct = super::super::calc_change_pct_for_input_size(0, 500);
        assert!(
            pct.is_nan(),
            "zero input_size must return NaN, not fabricated 0.0: {pct}"
        );
    }

    #[test]
    fn cached_candidate_reuse_requires_materialized_identity() {
        let rendered = super::RenderedCandidate {
            crf: 20.8,
            mode: super::AnimatedExplorationEncodeMode::FullTimeline,
            preset: EncoderPreset::Slow,
        };

        assert!(super::candidate_is_materialized(
            Some(rendered),
            20.8,
            super::AnimatedExplorationEncodeMode::FullTimeline,
            EncoderPreset::Slow
        ));
        assert!(!super::candidate_is_materialized(
            None,
            20.8,
            super::AnimatedExplorationEncodeMode::FullTimeline,
            EncoderPreset::Slow
        ));
        assert!(!super::candidate_is_materialized(
            Some(rendered),
            20.7,
            super::AnimatedExplorationEncodeMode::FullTimeline,
            EncoderPreset::Slow
        ));
        assert!(!super::candidate_is_materialized(
            Some(rendered),
            20.8,
            super::AnimatedExplorationEncodeMode::ExplorationSample,
            EncoderPreset::Slow
        ));
        assert!(!super::candidate_is_materialized(
            Some(rendered),
            20.8,
            super::AnimatedExplorationEncodeMode::FullTimeline,
            EncoderPreset::Slower
        ));
    }

    #[test]
    fn final_domain_calibration_is_required_for_preset_or_timeline_changes() {
        assert!(!super::requires_final_domain_calibration(
            VideoEncoder::Hevc,
            EncoderPreset::Slow,
            super::AnimatedExplorationEncodeMode::FullTimeline,
            EncoderPreset::Slow,
            super::AnimatedExplorationEncodeMode::FullTimeline,
        ));
        assert!(super::requires_final_domain_calibration(
            VideoEncoder::Hevc,
            EncoderPreset::Slow,
            super::AnimatedExplorationEncodeMode::FullTimeline,
            EncoderPreset::Slower,
            super::AnimatedExplorationEncodeMode::FullTimeline,
        ));
        assert!(super::requires_final_domain_calibration(
            VideoEncoder::Hevc,
            EncoderPreset::Slow,
            super::AnimatedExplorationEncodeMode::ExplorationSample,
            EncoderPreset::Slow,
            super::AnimatedExplorationEncodeMode::FullTimeline,
        ));
    }

    #[test]
    fn final_domain_quality_improvement_may_be_larger_than_current_output() {
        let policy = crate::exploration_policy::SizePolicy::StrictlySmaller;
        assert!(super::final_domain_candidate_improves_quality(
            policy, 1_000, 20.0, 700, 19.5, 900,
        ));
        assert!(!super::final_domain_candidate_improves_quality(
            policy, 1_000, 20.0, 700, 19.5, 1_000,
        ));
        assert!(!super::final_domain_candidate_improves_quality(
            policy, 1_000, 20.0, 700, 20.5, 600,
        ));
    }
}
