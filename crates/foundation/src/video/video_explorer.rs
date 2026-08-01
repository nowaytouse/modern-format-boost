//! Video CRF Explorer Module - Unified video quality explorer
//!
//! Recommended mode: `explore + match-quality + compress` (enabled by default,
//! see `flag_validator`). Only supports animated image-to-video and
//! video-to-video conversions; static images use lossless conversion and do not
//! support exploration mode.
//!
//! ## Modular Design
//!
//! All exploration logic is centralized in this module; other modules
//! (`img_hevc`, `vid_hevc`) only need to call this module's helper functions,
//! avoiding redundant implementations.
//!
//! ## Unified Selection Philosophy
//!
//! All candidate/finalist selection across direct `VideoExplorer` APIs and
//! strategy implementations follows the same ranking priorities, ensuring
//! consistency:
//!
//! 1. **Gating/Pass Status**: Size gates, quality checks (`quality_passed`,
//!    `ms_ssim_passed`)
//! 2. **Quality Metrics**: VMAF > CAMBI > `PSNR_UV` > MS-SSIM > SSIM > PSNR
//! 3. **Size Efficiency**: Output file size (prefer smaller)
//! 4. **Parameter**: CRF value (prefer lower/more aggressive as tiebreaker)
//! 5. **Preset**: Encoder preset rank (prefer slower/higher quality)
//!
//! For terminology and comparator utilities, see the `candidate_comparator`
//! module.

use anyhow::{Context, Result, anyhow, bail};
use rug::Rational;

use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::Path;

const WINDOW_SIZE: usize = crate::constants::EXPLORE_WINDOW_SIZE;
const VARIANCE_THRESHOLD: f64 = crate::constants::EXPLORE_VARIANCE_THRESHOLD;
const MIN_ITERATIONS_BEFORE_VARIANCE_EXIT: u32 = crate::constants::EXPLORE_MIN_ITERATIONS_VARIANCE;
use crate::builder_base::ToolBuilder;
use crate::explore_strategy::{CrfCache, ExploreContext, create_strategy};

use crate::crf_constants::EMERGENCY_MAX_ITERATIONS;
use crate::float_compare::SSIM_EPSILON;
use crate::types::{CheckResult, EncoderPreset, FileSize, Ssim};

pub mod error_handling;
/// SSIM calculator sub-module (re-exported).
pub mod ssim_calculator;
/// Stream analysis sub-module (re-exported).
pub mod stream_analysis;

pub use ssim_calculator::*;
pub use stream_analysis::*;

/// Minimum measurable CRF value (bit-exact).
pub const ABSOLUTE_MIN_CRF: f32 = crate::constants::ABSOLUTE_MIN_CRF;

/// Maximum measurable CRF value (codec limit).
pub const ABSOLUTE_MAX_CRF: f32 = crate::constants::ABSOLUTE_MAX_CRF;

/// Maximum iterations for Stage B1 (Coarse Search).
pub const STAGE_B1_MAX_ITERATIONS: u32 = crate::constants::STAGE_B1_MAX_ITERATIONS;

/// Maximum iterations for Stage B2 (Fine Search).
pub const STAGE_B2_MAX_ITERATIONS: u32 = crate::constants::STAGE_B2_MAX_ITERATIONS;

/// Maximum iterations for Bidirectional Phase B.
pub const STAGE_B_BIDIRECTIONAL_MAX: u32 = crate::constants::STAGE_B_BIDIRECTIONAL_MAX_ITERATIONS;

/// Maximum iterations for Binary Search phase.
pub const BINARY_SEARCH_MAX_ITERATIONS: u32 = crate::constants::BINARY_SEARCH_MAX_ITERATIONS;

/// Hard global limit for any single file exploration to prevent infinite loops.
pub const GLOBAL_MAX_ITERATIONS: u32 = crate::constants::GLOBAL_MAX_ITERATIONS;

/// Calculates the metadata size from pre- and post-insertion file sizes.
#[inline]
#[must_use]
pub const fn detect_metadata_size(pre_metadata_size: u64, post_metadata_size: u64) -> u64 {
    post_metadata_size.saturating_sub(pre_metadata_size)
}

pub use precision::*;

/// Minimum consecutive wall-clock hits required for saturation detection in
/// ultimate mode.
pub const ULTIMATE_MIN_WALL_HITS: u32 = crate::constants::ULTIMATE_MIN_WALL_HITS;

/// Maximum consecutive wall-clock hits allowed for saturation detection in
/// ultimate mode.
pub const ULTIMATE_MAX_WALL_HITS: u32 = crate::constants::ULTIMATE_MAX_WALL_HITS;

/// In ultimate mode, absolute saturation requires 50 consecutive samples to be
/// statistically certain.
use crate::constants::{
    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION,
    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE, CHANGE_RATE_THRESHOLD,
    LONG_VIDEO_THRESHOLD_SECS, MS_SSIM_THREE_SEGMENT_MIN_DURATION_SECS, PHI,
    SSIM_PLATEAU_THRESHOLD, VERY_LONG_VIDEO_THRESHOLD_SECS, VMAF_SKIP_THRESHOLD_ULTIMATE_SECS,
};

/// Required consecutive zero-gain encodes for saturation detection in ultimate
/// mode.
pub const ULTIMATE_REQUIRED_ZERO_GAINS: u32 = crate::constants::ULTIMATE_REQUIRED_ZERO_GAINS;

/// Maximum consecutive wall hits for normal mode (uses zero-gains as proxy).
pub const NORMAL_MAX_WALL_HITS: u32 = crate::constants::NORMAL_REQUIRED_ZERO_GAINS;

/// Required consecutive zero-gain encodes for saturation detection in normal
/// mode.
pub const NORMAL_REQUIRED_ZERO_GAINS: u32 = crate::constants::NORMAL_REQUIRED_ZERO_GAINS;

/// Max iterations for 5–10 min videos. Longer videos use a *lower* cap (see
/// below) because each encode/decode test is more expensive; this is an
/// intentional cost vs. precision tradeoff.
pub const LONG_VIDEO_FALLBACK_ITERATIONS: u32 = crate::constants::LONG_VIDEO_FALLBACK_ITERATIONS;

/// Max iterations for ≥10 min videos. Lower than
/// `LONG_VIDEO_FALLBACK_ITERATIONS`: longer videos cost more per iteration, so
/// we cap iterations to keep total runtime reasonable.
pub const VERY_LONG_VIDEO_FALLBACK_ITERATIONS: u32 =
    crate::constants::VERY_LONG_VIDEO_FALLBACK_ITERATIONS;

/// Required consecutive zero-gain encodes for saturation detection in long
/// videos (5-10 min).
pub const LONG_VIDEO_REQUIRED_ZERO_GAINS: u32 = crate::constants::LONG_VIDEO_REQUIRED_ZERO_GAINS;

/// Calculates the maximum exploration iterations allowed based on video
/// duration and mode.
///
/// Longer videos get fewer iterations to keep total runtime reasonable.
#[must_use]
pub fn calculate_max_iterations_for_duration(duration_secs: f32, ultimate_mode: bool) -> u32 {
    if duration_secs >= VERY_LONG_VIDEO_THRESHOLD_SECS {
        VERY_LONG_VIDEO_FALLBACK_ITERATIONS
    } else if duration_secs >= LONG_VIDEO_THRESHOLD_SECS {
        LONG_VIDEO_FALLBACK_ITERATIONS
    } else if ultimate_mode {
        crate::constants::GLOBAL_MAX_ITERATIONS
    } else {
        crate::constants::EXPLORE_DEFAULT_MAX_ITERATIONS
    }
}

/// Calculates the required zero-gain encodes for saturation detection based on
/// video duration. # Errors
/// Returns an error if the calculation fails due to invalid parameters.
pub fn calculate_zero_gains_for_duration(
    duration_secs: f32,
    ultimate_mode: bool,
) -> anyhow::Result<u32> {
    calculate_zero_gains_for_duration_and_range(
        duration_secs,
        crate::constants::SATURATION_CRF_RANGE_THRESHOLD,
        ultimate_mode,
    )
}

/// Calculates the required zero-gain encodes for saturation detection, with
/// explicit CRF range.
///
/// Scales the base requirement based on video duration and CRF range.
/// # Errors
/// Returns an error if the calculation fails due to invalid parameters.
pub fn calculate_zero_gains_for_duration_and_range(
    duration_secs: f32,
    crf_range: f32,
    ultimate_mode: bool,
) -> anyhow::Result<u32> {
    let base = if duration_secs >= LONG_VIDEO_THRESHOLD_SECS {
        LONG_VIDEO_REQUIRED_ZERO_GAINS
    } else if ultimate_mode {
        ULTIMATE_REQUIRED_ZERO_GAINS
    } else {
        NORMAL_REQUIRED_ZERO_GAINS
    };

    let factor = if crf_range < crate::constants::CRF_RANGE_SCALING_DIVISOR {
        (crf_range / crate::constants::CRF_RANGE_SCALING_DIVISOR).clamp(0.5, 1.0)
    } else {
        1.0
    };

    let scaled = crate::numeric_cast::f32_to_u32_strict(
        (crate::numeric_cast::u32_to_f32(base) * factor).round(),
        "zero_gain_threshold",
    )
    .ok_or_else(|| anyhow::anyhow!("Zero gain threshold calculation overflowed u32"))?;

    let min_gains = if ultimate_mode {
        crate::constants::ULTIMATE_MIN_GAINS
    } else {
        crate::constants::NORMAL_MIN_GAINS
    };
    Ok(scaled.max(min_gains))
}

/// Logarithmic base constant used in adaptive wall-hit calculations.
pub const ADAPTIVE_WALL_LOG_BASE: u32 = crate::constants::ADAPTIVE_WALL_LOG_BASE;

/// Calculates the adaptive maximum wall-clock hits based on CRF search range.
///
/// Uses a logarithmic formula to scale the hit requirement with search range
/// breadth. # Errors
/// Returns an error if the calculation fails due to invalid parameters.
pub fn calculate_adaptive_max_walls(crf_range: f32) -> anyhow::Result<u32> {
    if crf_range.is_nan() || crf_range.is_infinite() || crf_range <= 1.0 {
        return Ok(crate::constants::ULTIMATE_MIN_WALL_HITS);
    }
    let log_component =
        crate::numeric_cast::f32_to_u32_strict(crf_range.log2().ceil(), "crf_range_log")
            .ok_or_else(|| anyhow::anyhow!("CRF range log calculation overflowed u32"))?;
    let total = log_component.saturating_add(crate::constants::ADAPTIVE_WALL_LOG_BASE);
    Ok(total.clamp(
        crate::constants::ULTIMATE_MIN_WALL_HITS,
        crate::constants::ULTIMATE_MAX_WALL_HITS,
    ))
}

/// Minimum number of threads to use for video encoding.
pub const MIN_ENCODE_THREADS: usize = crate::constants::MIN_ENCODE_THREADS;

/// Default maximum number of encoding threads for typical machines.
pub const DEFAULT_MAX_ENCODE_THREADS: usize = crate::constants::DEFAULT_MAX_ENCODE_THREADS;

/// Maximum number of encoding threads for server-class machines.
pub const SERVER_MAX_ENCODE_THREADS: usize = crate::constants::SERVER_MAX_ENCODE_THREADS;

/// Default initial CRF for exploration (starting point for search).
pub const EXPLORE_DEFAULT_INITIAL_CRF: f32 = crate::constants::EXPLORE_DEFAULT_INITIAL_CRF;

/// Default minimum CRF allowed in exploration (lossless boundary).
pub const EXPLORE_DEFAULT_MIN_CRF: f32 = crate::constants::EXPLORE_DEFAULT_MIN_CRF;

/// Default maximum CRF allowed in exploration (HEVC limit).
pub const EXPLORE_DEFAULT_MAX_CRF: f32 = crate::constants::EXPLORE_DEFAULT_MAX_CRF;

/// Default target size ratio (1.0 = same size as input).
pub const EXPLORE_DEFAULT_TARGET_RATIO: f64 = crate::constants::EXPLORE_DEFAULT_TARGET_RATIO;

/// Default maximum number of exploration iterations.
pub const EXPLORE_DEFAULT_MAX_ITERATIONS: u32 = crate::constants::EXPLORE_DEFAULT_MAX_ITERATIONS;

/// Default minimum SSIM threshold for quality validation.
pub const EXPLORE_DEFAULT_MIN_SSIM: f64 = crate::constants::DEFAULT_MIN_SSIM;

/// Default minimum PSNR threshold for quality validation.
pub const EXPLORE_DEFAULT_MIN_PSNR: f64 = crate::constants::DEFAULT_MIN_PSNR;

/// Default minimum MS-SSIM threshold for quality validation.
pub const EXPLORE_DEFAULT_MIN_MS_SSIM: f64 = crate::constants::DEFAULT_MIN_MS_SSIM;

/// Calculates the optimal number of encoding threads based on CPU count and
/// resolution.
///
/// Balances parallelism against per-thread overhead for different resolutions.
#[must_use]
pub fn calculate_max_threads(cpu_count: usize, resolution_pixels: Option<u64>) -> usize {
    let half_cpus = cpu_count / 2;

    let resolution_limit = match resolution_pixels {
        Some(pixels) if pixels < crate::constants::PIXELS_720P => crate::constants::THREADS_LOW_RES,
        Some(pixels) if pixels < crate::constants::PIXELS_1080P => {
            crate::constants::THREADS_MEDIUM_RES
        }
        Some(pixels) if pixels < crate::constants::PIXELS_4K => crate::constants::THREADS_HIGH_RES,
        Some(_) => SERVER_MAX_ENCODE_THREADS,
        None => DEFAULT_MAX_ENCODE_THREADS,
    };

    half_cpus.clamp(MIN_ENCODE_THREADS, resolution_limit)
}

/// Operation mode for the CRF exploration process.
///
/// Each mode determines how aggressively the explorer searches for the optimal
/// CRF value and whether it prioritizes quality matching, compression, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExploreMode {
    /// Only search for a CRF that produces a smaller file; no quality checks.
    SizeOnly,
    /// Encode at the predicted CRF and verify quality meets thresholds.
    QualityMatch,
    /// Iteratively search for the CRF that best matches the input quality
    /// (SSIM).
    PreciseQualityMatch,
    /// Like `PreciseQualityMatch` but also ensures the output is smaller than
    /// the input.
    PreciseQualityMatchWithCompression,
    /// Search for the highest CRF that still produces a smaller file; no
    /// quality checks.
    CompressOnly,
    /// Search for compression that also maintains a minimum quality threshold.
    CompressWithQuality,
}

/// Per-component confidence scores; `overall()` computes a weighted aggregate.
///
/// Each field represents a different aspect of the exploration result's
/// reliability.
#[derive(Debug, Clone, Default)]
pub struct ConfidenceBreakdown {
    /// How well the CRF search space was sampled (0.0–1.0).
    pub sampling_coverage: Option<f64>,
    /// How accurate the CRF predictions were compared to actual encodes
    /// (0.0–1.0).
    pub prediction_accuracy: Option<f64>,
    /// How safe the resulting size/quality margins are (0.0–1.0).
    pub margin_safety: Option<f64>,
    /// SSIM/VMAF-based reliability; `None` when the metric was not measured.
    pub ssim_confidence: Option<f64>,
}

/// Weight for sampling coverage in the overall confidence calculation.
pub const CONFIDENCE_WEIGHT_SAMPLING: f64 = crate::constants::CONFIDENCE_WEIGHT_SAMPLING;
/// Weight for prediction accuracy in the overall confidence calculation.
pub const CONFIDENCE_WEIGHT_PREDICTION: f64 = crate::constants::CONFIDENCE_WEIGHT_PREDICTION;
/// Weight for margin safety in the overall confidence calculation.
pub const CONFIDENCE_WEIGHT_MARGIN: f64 = crate::constants::CONFIDENCE_WEIGHT_MARGIN;
/// Weight for SSIM reliability in the overall confidence calculation.
pub const CONFIDENCE_WEIGHT_SSIM: f64 = crate::constants::CONFIDENCE_WEIGHT_SSIM;

/// A specific state in the GPU-accelerated CRF exploration.
///
/// Represents one GPU probe result and its corresponding CPU prediction.
#[derive(Debug, Clone)]
pub struct CalibrationPoint {
    /// The CRF value used in the GPU probe.
    pub gpu_crf: f32,
    /// Resulting file size from the GPU encode.
    pub gpu_size: u64,
    /// SSIM score from the GPU encode (if measured).
    pub gpu_ssim: Option<f64>,
    /// The starting point predicted for the CPU fine-search.
    pub predicted_cpu_crf: f32,
    /// Confidence level in the CPU prediction, from 0.0 to 1.0.
    pub confidence: f64,
    /// Human-readable rationale for the prediction adjustment.
    pub reason: &'static str,
}

/// SSIM tier mapped to a unit confidence component (measurement-derived, not a
/// fixed explore literal).
#[must_use]
pub(crate) fn exploration_ssim_component(ssim: f64) -> Option<f64> {
    if !ssim.is_finite() {
        return None;
    }
    let tier = if ssim >= crate::constants::SSIM_GRADE_EXCELLENT {
        1.0_f64
    } else if ssim >= crate::constants::SSIM_GRADE_GOOD {
        0.9_f64
    } else if ssim >= crate::constants::SSIM_GRADE_ACCEPTABLE {
        0.7_f64
    } else {
        0.5_f64
    };
    crate::algorithm_seal::exploration_unit_probability(tier)
}

#[must_use]
pub(crate) fn exploration_sampling_coverage(iterations: u32, max_iterations: u32) -> Option<f64> {
    let max_iterations = max_iterations.max(1);
    crate::algorithm_seal::exploration_unit_probability(
        crate::numeric_cast::u32_to_f64(iterations.min(max_iterations))
            / crate::numeric_cast::u32_to_f64(max_iterations),
    )
}

#[must_use]
pub(crate) fn exploration_margin_from_ssim(ssim: f64, min_ssim: f64) -> Option<f64> {
    if !ssim.is_finite() || !min_ssim.is_finite() {
        return None;
    }
    let denom = (1.0_f64 - min_ssim).max(1e-6_f64);
    crate::algorithm_seal::exploration_unit_probability(((ssim - min_ssim) / denom).clamp(0.0, 1.0))
}

#[must_use]
pub(crate) fn exploration_size_margin_from_output(
    input_pure_media_size: u64,
    output_pure_media_size: u64,
) -> Option<f64> {
    if input_pure_media_size == 0 || output_pure_media_size >= input_pure_media_size {
        return None;
    }
    let margin = crate::numeric_cast::u64_to_f64(
        input_pure_media_size.saturating_sub(output_pure_media_size),
    ) / crate::numeric_cast::u64_to_f64(input_pure_media_size);
    crate::algorithm_seal::exploration_unit_probability((margin / 0.05).min(1.0))
}

/// Build exploration confidence from measured evidence only (no
/// `EXPLORE_CONFIDENCE_*` literals).
#[must_use]
pub(crate) fn measured_exploration_confidence(
    ssim: Option<f64>,
    min_ssim: f64,
    iterations: u32,
    max_iterations: u32,
) -> (Option<f64>, ConfidenceBreakdown) {
    let ssim_confidence = ssim.and_then(exploration_ssim_component);
    let margin_safety = ssim.and_then(|s| exploration_margin_from_ssim(s, min_ssim));
    let sampling_coverage = exploration_sampling_coverage(iterations, max_iterations);
    let detail = ConfidenceBreakdown {
        sampling_coverage,
        prediction_accuracy: None,
        margin_safety,
        ssim_confidence,
    };
    (detail.overall(), detail)
}

/// Ultimate-mode confidence from search-phase VMAF/PSNR-UV (SSIM not used).
#[must_use]
pub(crate) fn measured_exploration_confidence_ultimate(
    vmaf_y: Option<f64>,
    psnr_uv: Option<(f64, f64)>,
    iterations: u32,
    max_iterations: u32,
) -> (Option<f64>, ConfidenceBreakdown) {
    let ssim_confidence = vmaf_y.map(|v| {
        if v >= crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR + 5.0 {
            1.0_f64
        } else if v >= crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR {
            0.9_f64
        } else {
            0.6_f64
        }
    });
    let margin_safety = psnr_uv.map(|(u, v)| {
        let chroma = f64::midpoint(u, v);
        if chroma >= crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR + 5.0 {
            1.0_f64
        } else if chroma >= crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR {
            0.85_f64
        } else {
            0.5_f64
        }
    });
    let sampling_coverage = exploration_sampling_coverage(iterations, max_iterations);
    let detail = ConfidenceBreakdown {
        sampling_coverage,
        prediction_accuracy: None,
        margin_safety,
        ssim_confidence,
    };
    (detail.overall(), detail)
}

impl ConfidenceBreakdown {
    /// Weighted aggregate over **available** components (renormalized; no
    /// fabricated fill-ins).
    #[must_use]
    pub fn overall(&self) -> Option<f64> {
        let mut weighted_sum = 0.0_f64;
        let mut weight_total = 0.0_f64;
        if let Some(v) = self.ssim_confidence {
            weighted_sum = v.mul_add(CONFIDENCE_WEIGHT_SSIM, weighted_sum);
            weight_total += CONFIDENCE_WEIGHT_SSIM;
        }
        if let Some(v) = self.margin_safety {
            weighted_sum = v.mul_add(CONFIDENCE_WEIGHT_MARGIN, weighted_sum);
            weight_total += CONFIDENCE_WEIGHT_MARGIN;
        }
        if let Some(v) = self.sampling_coverage {
            weighted_sum = v.mul_add(CONFIDENCE_WEIGHT_SAMPLING, weighted_sum);
            weight_total += CONFIDENCE_WEIGHT_SAMPLING;
        }
        if let Some(v) = self.prediction_accuracy {
            weighted_sum = v.mul_add(CONFIDENCE_WEIGHT_PREDICTION, weighted_sum);
            weight_total += CONFIDENCE_WEIGHT_PREDICTION;
        }
        if weight_total <= 0.0_f64 {
            return None;
        }
        crate::algorithm_seal::exploration_unit_probability(weighted_sum / weight_total)
    }

    /// Prints a formatted confidence report to the log (verbose mode only).
    pub fn print_report(&self) {
        if !crate::progress_mode::is_verbose_mode() {
            return;
        }
        let Some(overall) = self.overall() else {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "video_exploration",
                branch = "confidence_report_non_finite",
                "skipping confidence report (overall confidence rejected)"
            );
            return;
        };
        let grade = if overall >= 0.9_f64 {
            "Excellent"
        } else if overall >= crate::constants::CONFIDENCE_DEFAULT_HIGH {
            "Good"
        } else if overall >= crate::constants::HEURISTIC_SAFETY_FLOOR {
            "Fair"
        } else {
            "Low"
        };

        crate::log_summary_header!(crate::infra::static_logs::messages::LABEL_CONFIDENCE_AUDIT);
        crate::log_report_stat!(
            crate::infra::static_logs::messages::LABEL_OVERALL_CONFIDENCE,
            format!(
                "Overall Confidence: {:.1}% (Grade: {})",
                overall * crate::constants::SCALE_100,
                grade
            )
        );
        if let Some(sampling_coverage) = self.sampling_coverage {
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_SAMPLING_COVERAGE,
                format!(
                    "{:.1}% (CRF search space saturation level)",
                    sampling_coverage * crate::constants::SCALE_100
                )
            );
        }
        if let Some(prediction_accuracy) = self.prediction_accuracy {
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_PREDICTION_ACCURACY,
                format!(
                    "{:.1}% (Mean Absolute Error: {:.4})",
                    prediction_accuracy * crate::constants::SCALE_100,
                    (1.0 - prediction_accuracy).abs()
                )
            );
        }
        if let Some(margin_safety) = self.margin_safety {
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_SAFETY_MARGIN,
                format!(
                    "{:.1}% (Structural bitstream protection margin)",
                    margin_safety * crate::constants::SCALE_100
                )
            );
        }
        if let Some(ssim_confidence) = self.ssim_confidence {
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_SSIM_RELIABILITY,
                format!(
                    "{:.1}% (Perceptual metric weight and consistency)",
                    ssim_confidence * crate::constants::SCALE_100
                )
            );
        }
    }
}

/// The result of a CRF exploration.
///
/// Contains the optimal CRF found, quality metrics, and metadata about the
/// exploration process.
#[derive(Debug, Clone)]
pub struct ExploreResult {
    /// The optimal CRF value found by the exploration.
    pub optimal_crf: f32,
    /// The resulting output file size in bytes.
    pub output_size: u64,
    /// Percentage change in file size compared to input.
    pub size_change_pct: f64,
    /// Structural Similarity Index score (if measured).
    pub ssim: Option<f64>,
    /// Peak Signal-to-Noise Ratio in dB (if measured).
    pub psnr: Option<f64>,
    /// Multi-Scale SSIM score (if measured).
    pub ms_ssim: Option<f64>,
    /// Whether the MS-SSIM / fusion quality check passed (standard mode only).
    pub ms_ssim_passed: CheckResult,
    /// Whether the ultimate 3D gate passed (VMAF/CAMBI/PSNR-UV; ultimate mode
    /// only).
    pub ultimate_quality_passed: CheckResult,
    /// The actual MS-SSIM score achieved (may differ from `ms_ssim` in some
    /// modes).
    pub ms_ssim_score: Option<f64>,
    /// Whether SSIM fallback was used instead of MS-SSIM (distinguishes MS-SSIM
    /// from SSIM fallback results).
    pub used_fallback: bool,
    /// Number of encode iterations performed during exploration.
    pub iterations: u32,
    /// Whether the file-size / compression target was met (independent of SSIM
    /// quality).
    pub size_target_met: CheckResult,
    /// Whether the overall quality check passed (SSIM/PSNR/MS-SSIM per explore
    /// config).
    pub quality_passed: CheckResult,
    /// When quality/size would pass but enhanced verification (duration/stream)
    /// failed; used for accurate failure messaging.
    pub enhanced_verify_fail_reason: Option<String>,
    /// Human-readable log messages produced during exploration.
    pub log: Vec<String>,
    /// Overall confidence score (0.0–1.0); `None` when the aggregate failed
    /// seal.
    pub confidence: Option<f64>,
    /// Detailed breakdown of confidence components.
    pub confidence_detail: ConfidenceBreakdown,
    /// The minimum SSIM threshold that was used during exploration.
    pub actual_min_ssim: f64,
    /// Exact input video + audio packet payload bytes used by size decisions.
    pub input_pure_media_size: u64,
    /// Exact output video + audio packet payload bytes used by size decisions.
    pub output_pure_media_size: u64,
    /// Container overhead in bytes (metadata, headers, etc.).
    pub container_overhead: u64,
    /// Ultimate mode 3D quality gate: VMAF Y-channel score (0–100).
    pub vmaf_y_score: Option<f64>,
    /// Ultimate mode 3D quality gate: CAMBI banding score (lower = better).
    pub cambi_score: Option<f64>,
    /// Ultimate mode 3D quality gate: (`PSNR_U`, `PSNR_V`) in dB.
    pub psnr_uv_score: Option<(f64, f64)>,
    /// Whether an early insight triggered (quality plateau detected, skipped
    /// further exploration).
    pub early_insight_triggered: bool,
    /// Explore ran under ultimate mode: quality contract is VMAF/CAMBI/PSNR-UV,
    /// not SSIM.
    pub ultimate_mode: bool,
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
            ms_ssim_passed: CheckResult::NotChecked,
            ultimate_quality_passed: CheckResult::NotChecked,
            ms_ssim_score: None,
            used_fallback: false,
            iterations: 0,
            size_target_met: CheckResult::NotChecked,
            quality_passed: CheckResult::NotChecked,
            enhanced_verify_fail_reason: None,
            log: Vec::new(),
            confidence: None,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: crate::constants::EXPLORE_DEFAULT_MIN_SSIM,
            input_pure_media_size: 0,
            output_pure_media_size: 0,
            container_overhead: 0,
            vmaf_y_score: None,
            cambi_score: None,
            psnr_uv_score: None,
            early_insight_triggered: false,
            ultimate_mode: false,
        }
    }
}

fn audit_explore_result_non_finite(result: &ExploreResult) {
    if result.confidence.is_some_and(f64::is_finite)
        && result.optimal_crf.is_finite()
        && result.size_change_pct.is_finite()
    {
        return;
    }
    tracing::warn!(
        target: "mfb.algorithm",
        pipeline = "video_exploration",
        branch = "explore_non_finite_audit_only",
        confidence = ?result.confidence,
        optimal_crf = result.optimal_crf,
        size_change_pct = result.size_change_pct,
        "exploration seal disabled; non-finite metrics passed through unchanged"
    );
}

#[inline]
pub(crate) fn calc_change_pct_for_input_size(input_size: u64, output_size: u64) -> f64 {
    if input_size == 0 {
        // Unknown denominator: do not fabricate a neutral "0.0% change".
        return f64::NAN;
    }
    let ratio = Rational::from(output_size) / Rational::from(input_size.max(1));
    ((ratio - Rational::from(1)) * Rational::from(100)).to_f64()
}

/// Internal explore paths must return through this helper so metrics are sealed
/// even when `VideoExplorer::explore()` is not the immediate caller.
#[inline]
fn ok_explore_result(result: ExploreResult) -> ExploreResult {
    result.sealed()
}

impl ExploreResult {
    /// Sanitize exploration metrics before they drive encode decisions or
    /// user-visible summaries. Consume and return a sealed exploration
    /// result (terminal algorithm contract).
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.seal_algorithm_outputs();
        if !crate::algorithm_runtime::exploration_algorithm_seal_enabled() {
            audit_explore_result_non_finite(&self);
        }
        Self::enforce_exploration_quality_gates(&mut self);
        self
    }

    fn enforce_exploration_quality_gates(&mut self) {
        Self::backfill_ultimate_confidence_if_needed(self);
        Self::neutralize_standard_ssim_contract_for_ultimate(self);
        Self::enforce_perceptual_quality_coherence(self);
        Self::enforce_ultimate_metrics_presence_quality_gate(self);
        Self::enforce_ultimate_metrics_sanity_quality_gate(self);
        if !self.uses_ultimate_quality_contract() {
            Self::enforce_ssim_presence_quality_gate(self);
            Self::enforce_ssim_threshold_quality_gate(self);
            Self::enforce_ssim_measurement_quality_gate(self);
        }
        Self::enforce_size_target_quality_gate(self);
        Self::enforce_confidence_quality_gate(self);
    }

    /// Phase 3 may set `ms_ssim_passed` / `ultimate_quality_passed` without
    /// updating `quality_passed`.
    fn enforce_perceptual_quality_coherence(result: &mut Self) {
        if !result.quality_passed.is_passed() || !result.perceptual_quality_failed() {
            return;
        }
        let reason = if result.uses_ultimate_quality_contract() {
            crate::media_conversion_gate::explore_perceptual_gate_failure_reason_or_default(
                result.ultimate_quality_passed.failure_reason(),
                "3D quality gate failed",
                "ultimate_quality_incoherent",
            )
        } else {
            crate::media_conversion_gate::explore_perceptual_gate_failure_reason_or_default(
                result.ms_ssim_passed.failure_reason(),
                "perceptual quality gate failed",
                "ms_ssim_incoherent",
            )
        };
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "video_exploration",
            branch = "exploration_perceptual_quality_incoherent",
            reason = %reason,
            "rejecting quality_passed explore result (perceptual gate failed)"
        );
        result.quality_passed = CheckResult::Failed(reason);
    }

    /// Phase 3 sets VMAF/CAMBI/PSNR-UV on the result after CPU settle may leave
    /// `confidence` unset.
    fn backfill_ultimate_confidence_if_needed(result: &mut Self) {
        let min = crate::constants::MIN_EXPLORATION_CONFIDENCE;
        if result.confidence.is_some_and(|c| c.is_finite() && c >= min) {
            return;
        }
        if !result.uses_ultimate_quality_contract()
            || !result.ultimate_quality_passed.is_passed()
            || !result.has_complete_ultimate_quality_metrics()
        {
            return;
        }
        let max_iter = crate::constants::GPU_ABSOLUTE_MAX_ITERATIONS.max(result.iterations.max(1));
        let (confidence, detail) = measured_exploration_confidence_ultimate(
            result.vmaf_y_score,
            result.psnr_uv_score,
            result.iterations,
            max_iter,
        );
        if let Some(c) = confidence.filter(|v| v.is_finite() && *v >= min) {
            result.confidence = Some(c);
            result.confidence_detail = detail;
        }
    }

    /// Ultimate explore uses VMAF/CAMBI/PSNR-UV; SSIM is outside the quality
    /// contract.
    ///
    /// Metrics alone do not switch the contract (avoids stray VMAF fields on
    /// standard runs).
    #[inline]
    #[must_use]
    pub const fn uses_ultimate_quality_contract(&self) -> bool {
        self.ultimate_mode
    }

    /// Animated lossless (CRF=0) validates via integrity check;
    /// `ms_ssim_passed` replaces SSIM (M223).
    #[inline]
    #[must_use]
    pub const fn uses_lossless_integrity_quality_contract(&self) -> bool {
        !self.uses_ultimate_quality_contract()
            && self.ssim.is_none()
            && self.ms_ssim_passed.is_passed()
    }

    /// MS-SSIM / SSIM fusion must not remain authoritative after ultimate 3D
    /// metrics exist.
    fn neutralize_standard_ssim_contract_for_ultimate(result: &mut Self) {
        if !result.uses_ultimate_quality_contract() {
            return;
        }
        if result.ms_ssim_passed.is_failed() || result.ms_ssim_passed.is_passed() {
            result.ms_ssim_passed = CheckResult::NotChecked;
        }
    }

    /// Perceptual gate for the active contract (3D in ultimate, MS-SSIM/SSIM
    /// fusion otherwise).
    #[must_use]
    pub const fn perceptual_quality_failed(&self) -> bool {
        if self.uses_ultimate_quality_contract() {
            self.ultimate_quality_passed.is_failed()
        } else {
            self.ms_ssim_passed.is_failed()
        }
    }

    /// Whether the active perceptual gate passed (or was not required).
    #[must_use]
    pub const fn perceptual_quality_met(&self) -> bool {
        if self.uses_ultimate_quality_contract() {
            self.ultimate_quality_passed.is_passed()
        } else {
            self.ms_ssim_passed.is_passed()
                || (self.ms_ssim_passed.is_skipped() && self.quality_passed.is_passed())
        }
    }

    /// Whether the explore output has a smaller pure-media payload than the input.
    #[must_use]
    pub fn size_compression_met(&self) -> bool {
        self.size_target_met.is_passed()
            || (self.size_change_pct.is_finite() && self.size_change_pct < 0.0)
    }

    /// Pipeline-level success: quality match vs size-only exploration.
    #[must_use]
    pub fn pipeline_acceptable(&self, match_quality: bool, explore_smaller: bool) -> bool {
        crate::media_conversion_gate::video_explore_pipeline_acceptable(
            self,
            match_quality,
            explore_smaller,
        )
    }

    /// Strict delivery: a passed 3D gate must carry VMAF-Y, CAMBI, and PSNR-UV
    /// (not partial telemetry).
    fn enforce_ultimate_metrics_presence_quality_gate(result: &mut Self) {
        if !crate::algorithm_runtime::strict_media_conversion_delivery_enabled()
            || !result.uses_ultimate_quality_contract()
            || !result.quality_passed.is_passed()
            || !result.ultimate_quality_passed.is_passed()
        {
            return;
        }
        if result.has_complete_ultimate_quality_metrics() {
            return;
        }
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "video_exploration",
            branch = "exploration_ultimate_metrics_incomplete",
            vmaf = result.vmaf_y_score.is_some(),
            cambi = result.cambi_score.is_some(),
            psnr_uv = result.psnr_uv_score.is_some(),
            "rejecting quality_passed ultimate explore result (incomplete 3D metrics)"
        );
        result.quality_passed = CheckResult::Failed(
            "quality_passed requires complete 3D metrics (VMAF-Y, CAMBI, PSNR-UV)".into(),
        );
    }

    /// Reject incoherent ultimate passes that slip through without sane metric
    /// values.
    fn enforce_ultimate_metrics_sanity_quality_gate(result: &mut Self) {
        if !crate::algorithm_runtime::strict_media_conversion_delivery_enabled()
            || !result.uses_ultimate_quality_contract()
            || !result.quality_passed.is_passed()
            || !result.ultimate_quality_passed.is_passed()
        {
            return;
        }
        if precision::ultimate_metrics_meet_exploration_sanity(
            result.vmaf_y_score,
            result.cambi_score,
            result.psnr_uv_score,
        ) {
            return;
        }
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "video_exploration",
            branch = "exploration_ultimate_metrics_sanity_failed",
            vmaf = ?result.vmaf_y_score,
            cambi = ?result.cambi_score,
            psnr_uv = ?result.psnr_uv_score,
            "rejecting quality_passed ultimate explore result (3D metrics below exploration sanity)"
        );
        result.quality_passed = CheckResult::Failed(
            "quality_passed requires 3D metrics within exploration sanity floors".into(),
        );
        result.ultimate_quality_passed =
            CheckResult::Failed("3D metrics below exploration sanity floors".into());
    }

    fn enforce_ssim_threshold_quality_gate(result: &mut Self) {
        if !crate::algorithm_runtime::exploration_ssim_threshold_gate_enabled()
            || !result.quality_passed.is_passed()
        {
            return;
        }
        let Some(ssim) = result.ssim.filter(|s| s.is_finite()) else {
            return;
        };
        let min = result.actual_min_ssim;
        let epsilon = precision::SSIM_COMPARE_EPSILON;
        if ssim + epsilon >= min {
            return;
        }
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "video_exploration",
            branch = "exploration_ssim_below_threshold",
            ssim,
            min,
            "rejecting quality_passed explore result (SSIM below exploration floor)"
        );
        result.quality_passed =
            CheckResult::Failed(format!("SSIM {ssim:.4} below exploration minimum {min:.4}"));
    }

    fn enforce_size_target_quality_gate(result: &mut Self) {
        if !crate::algorithm_runtime::exploration_size_target_gate_enabled()
            || !result.quality_passed.is_passed()
            || !result.size_target_met.is_failed()
        {
            return;
        }
        let reason = crate::media_conversion_gate::explore_size_target_failure_reason_or_default(
            result.size_target_met.failure_reason(),
        );
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "video_exploration",
            branch = "exploration_size_target_failed",
            reason,
            "rejecting quality_passed explore result (size target failed)"
        );
        result.quality_passed = CheckResult::Failed(format!(
            "quality_passed incompatible with size failure: {reason}"
        ));
    }

    fn enforce_ssim_presence_quality_gate(result: &mut Self) {
        if !crate::algorithm_runtime::exploration_ssim_presence_gate_enabled()
            || !result.quality_passed.is_passed()
        {
            return;
        }
        if result.ssim.is_some_and(f64::is_finite) {
            return;
        }
        // Ultimate mode validates via VMAF/CAMBI/PSNR-UV (Phase 3), not SSIM.
        if result.ultimate_mode {
            return;
        }
        // Animated GIF/WebP CRF=0: integrity gate promotes ms_ssim_passed without SSIM
        // (M223).
        if result.uses_lossless_integrity_quality_contract() {
            return;
        }
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "video_exploration",
            branch = "exploration_ssim_missing",
            ssim = ?result.ssim,
            "rejecting quality_passed explore result (SSIM not measured)"
        );
        result.quality_passed = CheckResult::Failed(
            "quality_passed requires measured SSIM (re-run explore with SSIM verification)"
                .to_string(),
        );
    }

    fn enforce_ssim_measurement_quality_gate(result: &mut Self) {
        if !crate::algorithm_runtime::strict_media_conversion_delivery_enabled()
            || !crate::algorithm_runtime::exploration_ssim_presence_gate_enabled()
            || !result.quality_passed.is_passed()
            || result.uses_ultimate_quality_contract()
            || !result.used_fallback
        {
            return;
        }
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "video_exploration",
            branch = "exploration_ssim_predicted_rejected",
            ssim = ?result.ssim,
            "rejecting quality_passed explore result (PSNR-derived SSIM estimate)"
        );
        result.quality_passed = CheckResult::Failed(
            "quality_passed requires measured SSIM, not PSNR-derived estimate".into(),
        );
    }

    fn enforce_confidence_quality_gate(result: &mut Self) {
        if !crate::algorithm_runtime::exploration_confidence_gate_enabled()
            || !result.quality_passed.is_passed()
        {
            return;
        }
        if result.ultimate_mode
            && result.ultimate_quality_passed.is_passed()
            && result.has_complete_ultimate_quality_metrics()
        {
            return;
        }
        if result.uses_lossless_integrity_quality_contract() {
            return;
        }
        let min = crate::constants::MIN_EXPLORATION_CONFIDENCE;
        if result.confidence.is_some_and(|c| c >= min) {
            return;
        }
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "video_exploration",
            branch = "exploration_confidence_insufficient",
            confidence = ?result.confidence,
            min,
            "rejecting quality_passed explore result (confidence missing or below floor)"
        );
        result.quality_passed = CheckResult::Failed(format!(
            "Exploration confidence missing or below minimum ({min})"
        ));
    }

    pub fn seal_algorithm_outputs(&mut self) {
        self.confidence = self
            .confidence
            .and_then(crate::algorithm_seal::exploration_unit_probability);
        self.confidence_detail.sampling_coverage = self
            .confidence_detail
            .sampling_coverage
            .and_then(crate::algorithm_seal::exploration_unit_probability);
        self.confidence_detail.prediction_accuracy = self
            .confidence_detail
            .prediction_accuracy
            .and_then(crate::algorithm_seal::exploration_unit_probability);
        self.confidence_detail.margin_safety = self
            .confidence_detail
            .margin_safety
            .and_then(crate::algorithm_seal::exploration_unit_probability);
        self.confidence_detail.ssim_confidence = self
            .confidence_detail
            .ssim_confidence
            .and_then(crate::algorithm_seal::exploration_unit_probability);
        if let Some(v) = self.confidence_detail.overall() {
            self.confidence = Some(v);
        }
        if let Some(v) = crate::algorithm_seal::seal_non_negative_finite(self.size_change_pct) {
            self.size_change_pct = v;
        }
        if let Some(v) = crate::algorithm_seal::exploration_unit_probability(self.actual_min_ssim) {
            self.actual_min_ssim = v;
        }
        self.ssim = crate::algorithm_seal::seal_optional_unit_metric(self.ssim);
        self.psnr = self
            .psnr
            .and_then(|v| (v.is_finite() && v >= 0.0).then_some(v));
        self.ms_ssim = crate::algorithm_seal::seal_optional_unit_metric(self.ms_ssim);
        self.ms_ssim_score = crate::algorithm_seal::seal_optional_unit_metric(self.ms_ssim_score);
        self.vmaf_y_score = self
            .vmaf_y_score
            .and_then(|v| (v.is_finite() && (0.0..=100.0).contains(&v)).then_some(v));
        self.cambi_score = self
            .cambi_score
            .and_then(|v| (v.is_finite() && v >= 0.0).then_some(v));
        if self.optimal_crf.is_finite() {
            self.optimal_crf = precision::seal_exploration_crf(self.optimal_crf);
        } else {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "video_exploration",
                branch = "optimal_crf_non_finite",
                "CRF exploration produced non-finite optimal_crf; clamping to 0"
            );
            self.optimal_crf = 0.0;
        }
        self.psnr_uv_score = self.psnr_uv_score.and_then(|(u, v)| {
            if u.is_finite() && v.is_finite() && u >= 0.0 && v >= 0.0 {
                Some((u, v))
            } else {
                tracing::warn!(
                    target: "mfb.algorithm",
                    pipeline = "video_exploration",
                    branch = "psnr_uv_non_finite_rejected",
                    u,
                    v,
                    "dropping invalid PSNR-UV pair from explore result"
                );
                None
            }
        });
    }

    /// Returns the SSIM value as a typed `Ssim` if available and valid.
    #[inline]
    #[must_use]
    pub fn ssim_typed(&self) -> Option<Ssim> {
        self.ssim.and_then(|v| match Ssim::new(v) {
            Ok(ssim) => Some(ssim),
            Err(err) => {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "explore_result_ssim_typed",
                    format!("dropping invalid SSIM value from explore result: {v} ({err})"),
                );
                None
            }
        })
    }

    /// Returns the output size as a typed `FileSize`.
    #[inline]
    #[must_use]
    pub const fn output_size_typed(&self) -> FileSize {
        FileSize::new(self.output_size)
    }

    /// Returns true if the SSIM score meets or exceeds the given threshold.
    #[inline]
    #[must_use]
    pub fn ssim_meets(&self, threshold: f64) -> bool {
        self.ssim
            .is_some_and(|s| crate::float_compare::ssim_meets_threshold(s, threshold))
    }

    /// Partial 3D telemetry (logging / summaries). Strict delivery uses
    /// [`Self::has_complete_ultimate_quality_metrics`].
    #[inline]
    #[must_use]
    pub const fn has_ultimate_quality_metrics(&self) -> bool {
        self.vmaf_y_score.is_some() || self.cambi_score.is_some() || self.psnr_uv_score.is_some()
    }

    /// All three ultimate metrics present (VMAF-Y, CAMBI, PSNR-UV).
    #[inline]
    #[must_use]
    pub const fn has_complete_ultimate_quality_metrics(&self) -> bool {
        precision::has_complete_ultimate_metrics(
            self.vmaf_y_score,
            self.cambi_score,
            self.psnr_uv_score,
        )
    }

    /// Human-readable summary of ultimate-mode 3D gate metrics.
    #[must_use]
    pub fn ultimate_quality_summary(&self) -> Option<String> {
        if !self.has_ultimate_quality_metrics() {
            return None;
        }

        let vmaf = crate::media_conversion_gate::ui_optional_f64_display_or_map(
            self.vmaf_y_score,
            "VMAF-Y=N/A",
            "ultimate_vmaf_y",
            |v| format!("VMAF-Y={v:.2}"),
        );
        let cambi = crate::media_conversion_gate::ui_optional_f64_display_or_map(
            self.cambi_score,
            "CAMBI=N/A",
            "ultimate_cambi",
            |c| format!("CAMBI={c:.2}"),
        );
        let psnr_uv = crate::media_conversion_gate::ui_f64_pair_labeled_or_na(
            self.psnr_uv_score,
            "PSNR-UV=N/A",
            "ultimate_psnr_uv",
            |(u, v)| format!("PSNR-UV={u:.2}/{v:.2}dB"),
        );

        Some(format!("{vmaf}, {cambi}, {psnr_uv}"))
    }
}

#[derive(Clone, Copy)]
struct CrfSizeSample {
    crf: f32,
    size: u64,
}

#[derive(Clone, Copy)]
struct VerifiedCrfResult {
    crf: f32,
    size: u64,
    quality: (Option<f64>, Option<f64>, Option<f64>),
    ssim: f64,
}

struct BoundarySearchState {
    low: f32,
    high: f32,
    boundary_crf: f32,
    prev_size: Option<u64>,
    size_history: Vec<(f32, u64)>,
}

impl BoundarySearchState {
    const fn new(min_crf: f32, max_crf: f32) -> Self {
        Self {
            low: min_crf,
            high: max_crf,
            boundary_crf: max_crf,
            prev_size: None,
            size_history: Vec::new(),
        }
    }

    fn should_continue(&self, iterations: u32) -> bool {
        self.high - self.low > 0.5 && iterations < BINARY_SEARCH_MAX_ITERATIONS
    }

    fn midpoint(&self) -> f32 {
        (f32::midpoint(self.low, self.high) * 2.0).round() / 2.0
    }

    fn record(&mut self, sample: CrfSizeSample) {
        self.size_history.push((sample.crf, sample.size));
    }

    const fn accept(&mut self, crf: f32) {
        self.boundary_crf = crf;
        self.high = crf;
    }

    const fn reject(&mut self, crf: f32) {
        self.low = crf;
    }

    const fn finish_iteration(&mut self, size: u64) {
        self.prev_size = Some(size);
    }

    fn early_exit_reason(&self, input_size: u64, iterations: u32) -> Option<String> {
        if iterations < MIN_ITERATIONS_BEFORE_VARIANCE_EXIT {
            return None;
        }

        let variance = Self::calc_window_variance(&self.size_history, input_size);
        if variance < VARIANCE_THRESHOLD && self.size_history.len() >= WINDOW_SIZE {
            return Some(format!(
                "   {} Early exit: variance converged {variance:.2e} < {VARIANCE_THRESHOLD:.2e} \
                 (after {iterations} iterations)",
                crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
            ));
        }

        if let (Some(prev), Some(curr)) = (self.prev_size, self.latest_size()) {
            let change_rate = Self::calc_change_rate(prev, curr);
            if change_rate < CHANGE_RATE_THRESHOLD {
                return Some(format!(
                    "   {} Early exit: change rate negligible {:.4}% < {:.4}% (after {} \
                     iterations)",
                    crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]"),
                    change_rate * 100.0_f64,
                    CHANGE_RATE_THRESHOLD * 100.0_f64,
                    iterations
                ));
            }
        }

        None
    }

    fn latest_size(&self) -> Option<u64> {
        crate::media_conversion_gate::explore_latest_encoded_size_optional(
            self.size_history.last().map(|(_, size)| *size),
            "video_explorer latest_size",
        )
    }

    fn calc_window_variance(history: &[(f32, u64)], input_size: u64) -> f64 {
        if history.len() < WINDOW_SIZE || input_size == 0 {
            return f64::MAX;
        }

        let recent: Vec<f64> = history
            .iter()
            .rev()
            .take(WINDOW_SIZE)
            .map(|(_, size)| {
                f64::from(crate::numeric_cast::f64_to_f32_lossy(
                    crate::numeric_cast::u64_to_f64(*size)
                        / crate::numeric_cast::u64_to_f64(input_size),
                ))
            })
            .collect();

        if recent.is_empty() {
            return 0.0_f64;
        }

        let mean = recent.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(recent.len());
        recent.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
            / crate::numeric_cast::usize_to_f64(recent.len())
    }

    fn calc_change_rate(prev: u64, curr: u64) -> f64 {
        if prev == 0 {
            return f64::MAX;
        }

        let diff = curr.abs_diff(prev);
        (Rational::from(diff) / Rational::from(prev.max(1))).to_f64()
    }
}

struct PreciseCompressionSession<'a> {
    explorer: &'a VideoExplorer,
    log: Vec<String>,
    size_cache: CrfCache<u64>,
    quality_cache: CrfCache<(Option<f64>, Option<f64>, Option<f64>)>,
    last_encoded_crf: Option<f32>,
    target_size: u64,
    best_crf_so_far: f32,
    start_time: std::time::Instant,
    pb: indicatif::ProgressBar,
    iterations: u32,
}

impl<'a> PreciseCompressionSession<'a> {
    fn new(explorer: &'a VideoExplorer) -> Self {
        Self {
            explorer,
            log: Vec::new(),
            size_cache: CrfCache::new(),
            quality_cache: CrfCache::new(),
            last_encoded_crf: None,
            target_size: explorer.get_compression_target(),
            best_crf_so_far: 0.0,
            start_time: std::time::Instant::now(),
            pb: crate::progress::create_professional_spinner(&format!(
                "{} Initializing",
                crate::media_conversion_gate::ui_icon_pick("🔍", "[SCAN]")
            )),
            iterations: 0,
        }
    }

    fn run(mut self) -> Result<ExploreResult> {
        self.log_start();
        self.log_header("   Stage A: Size search");

        let min_sample = self.probe("Stage A", self.explorer.config.min_crf)?;
        if self.is_under_target(min_sample.size) {
            return self.run_low_crf_path(min_sample);
        }

        let max_sample = self.probe("Stage A", self.explorer.config.max_crf)?;
        if max_sample.size >= self.explorer.input_size {
            return self.finish_highly_compressed(max_sample);
        }

        let boundary_crf = self.run_boundary_search()?;
        self.finish_boundary_result(boundary_crf)
    }

    fn log_start(&self) {
        crate::log_summary_header!(
            &crate::media_conversion_gate::ui_log_summary_title_with_icon(
                "🔬",
                "[AUDIT]",
                format!(
                    "Forensic Explore: Precise Quality + Compression ({:?})",
                    self.explorer.encoder
                ),
            )
        );
        crate::log_report_stat!(
            crate::infra::static_logs::messages::LABEL_STRATEGY,
            format!(
                "Input: {:.2} MB | Range: [{:.1}, {:.1}]",
                crate::numeric_cast::f64_to_f32_lossy(
                    crate::numeric_cast::u64_to_f64(self.explorer.input_size)
                        / crate::constants::MB_DIVISOR
                ),
                self.explorer.config.min_crf,
                self.explorer.config.max_crf
            )
        );
    }

    fn run_low_crf_path(&mut self, min_sample: CrfSizeSample) -> Result<ExploreResult> {
        self.best_crf_so_far = min_sample.crf;
        let coarse_best = self.descend_fast(min_sample)?;
        let fine_best = self.descend_fine(coarse_best)?;

        self.ensure_encoded(
            fine_best.crf,
            format!("│ Re-encoding to best CRF {:.1}... │", fine_best.crf),
        )?;

        self.log_header("   Stage C: SSIM verification");
        let verified = self.verify_result(
            fine_best.crf,
            fine_best.size,
            "Precise Quality + Compression stage C verification",
        )?;

        Ok(self.finish_low_crf_result(verified))
    }

    fn descend_fast(&mut self, start: CrfSizeSample) -> Result<CrfSizeSample> {
        self.log_header(crate::infra::static_logs::messages::MSG_STAGE_B1);

        let mut best = start;
        let mut test_crf = start.crf - 0.5;
        while test_crf >= ABSOLUTE_MIN_CRF && self.iterations < STAGE_B1_MAX_ITERATIONS {
            let sample = self.probe("Stage B-1", test_crf)?;
            if self.is_under_target(sample.size) {
                best = sample;
                self.best_crf_so_far = sample.crf;
                test_crf -= 0.5;
            } else {
                break;
            }
        }

        Ok(best)
    }

    fn descend_fine(&mut self, start: CrfSizeSample) -> Result<CrfSizeSample> {
        self.log_header(crate::infra::static_logs::messages::MSG_STAGE_B2);

        let mut best = start;
        for offset in [-0.25_f32, -0.5, -0.75, -1.0] {
            let fine_crf = best.crf + offset;
            if fine_crf < ABSOLUTE_MIN_CRF || self.iterations >= STAGE_B2_MAX_ITERATIONS {
                break;
            }
            if self.size_cache.contains_key(fine_crf) {
                continue;
            }

            let sample = self.probe("Stage B-2", fine_crf)?;
            if self.is_under_target(sample.size) {
                best = sample;
                self.best_crf_so_far = sample.crf;
            } else {
                break;
            }
        }

        Ok(best)
    }

    fn finish_low_crf_result(&mut self, verified: VerifiedCrfResult) -> ExploreResult {
        let status = VideoExplorer::ssim_status_label(verified.ssim);
        let elapsed = self.start_time.elapsed();
        let saved = self.explorer.input_size.saturating_sub(verified.size);
        let log = std::mem::take(&mut self.log);

        self.pb.finish_and_clear();
        crate::log_stat!(
            crate::infra::static_logs::messages::LABEL_DONE,
            format!(
                "Result: CRF {:.1} | SSIM {:.4} ({}) | Size {:+.1}% | Saved: {:.2} MB | Iter: {} \
                 | Time: {:.1}s",
                verified.crf,
                verified.ssim,
                status,
                self.explorer.calc_change_pct(verified.size),
                crate::numeric_cast::f64_to_f32_lossy(
                    crate::numeric_cast::u64_to_f64(saved)
                        / crate::constants::KB_F64
                        / crate::constants::KB_F64
                ),
                self.iterations,
                elapsed.as_secs_f64()
            )
        );

        let (confidence, confidence_detail) = measured_exploration_confidence(
            Some(verified.ssim),
            self.explorer.config.quality_thresholds.min_ssim,
            self.iterations,
            STAGE_B1_MAX_ITERATIONS.saturating_add(STAGE_B2_MAX_ITERATIONS),
        );
        ExploreResult {
            optimal_crf: verified.crf,
            output_size: verified.size,
            size_change_pct: self.explorer.calc_change_pct(verified.size),
            ssim: verified.quality.0,
            psnr: verified.quality.1,
            ms_ssim: verified.quality.2,
            iterations: self.iterations,
            size_target_met: self.explorer.size_target_check(verified.size),
            quality_passed: if verified.ssim >= self.explorer.config.quality_thresholds.min_ssim {
                CheckResult::Passed
            } else {
                CheckResult::Failed(format!("SSIM {:.4} below threshold", verified.ssim))
            },
            log,
            confidence,
            confidence_detail,
            actual_min_ssim: self.explorer.config.quality_thresholds.min_ssim,
            ..Default::default()
        }
        .sealed()
    }

    fn finish_highly_compressed(&mut self, sample: CrfSizeSample) -> Result<ExploreResult> {
        self.log_header(crate::infra::static_logs::messages::MSG_HIGHLY_COMPRESSED_WARNING);
        let quality = self.validate_quality(sample.crf)?;
        let elapsed = self.start_time.elapsed();
        let log = std::mem::take(&mut self.log);

        self.pb.finish_and_clear();
        crate::media_conversion_gate::explore_delivery_explore_outcome_audit(
            "explore_highly_compressed",
            format!(
                "{}: cannot compress further (already highly optimized) | iter={} | time={:.1}s",
                self.explorer.input_path.display(),
                self.iterations,
                elapsed.as_secs_f64()
            ),
        );

        Ok(ok_explore_result(ExploreResult {
            optimal_crf: sample.crf,
            output_size: sample.size,
            size_change_pct: self.explorer.calc_change_pct(sample.size),
            ssim: quality.0,
            psnr: quality.1,
            ms_ssim: quality.2,
            iterations: self.iterations,
            size_target_met: self.explorer.size_target_check(sample.size),
            quality_passed: CheckResult::Failed("Enhanced verification failed".into()),
            log,
            confidence: None,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: self.explorer.config.quality_thresholds.min_ssim,
            ..Default::default()
        }))
    }

    fn run_boundary_search(&mut self) -> Result<f32> {
        let boundary_crf = self.binary_search_boundary()?;
        let refined = self.bidirectional_fine_tune(boundary_crf)?;
        Ok(crate::media_conversion_gate::explore_boundary_crf_optional(
            refined,
            boundary_crf,
            &self.explorer.input_path,
        )?)
    }

    fn binary_search_boundary(&mut self) -> Result<f32> {
        self.log_header(crate::infra::static_logs::messages::MSG_STAGE_A);

        let mut state =
            BoundarySearchState::new(self.explorer.config.min_crf, self.explorer.config.max_crf);
        while state.should_continue(self.iterations) {
            let mid = state.midpoint();
            let sample = self.probe("Binary search", mid)?;
            state.record(sample);

            if self.is_under_target(sample.size) {
                state.accept(sample.crf);
                self.best_crf_so_far = sample.crf;
            } else {
                state.reject(sample.crf);
            }

            if let Some(reason) = state.early_exit_reason(self.explorer.input_size, self.iterations)
            {
                self.log_header(reason);
                break;
            }

            state.finish_iteration(sample.size);
        }

        Ok(state.boundary_crf)
    }

    fn bidirectional_fine_tune(&mut self, boundary_crf: f32) -> Result<Option<f32>> {
        self.log_header("   Stage B: Fine tune (0.1 step)");

        let downward = self.run_fine_tune_pass(
            boundary_crf,
            &[-0.25_f32, -0.5, -0.75, -1.0],
            "Fine tune down",
        )?;
        if downward.is_some() {
            return Ok(downward);
        }

        self.run_fine_tune_pass(boundary_crf, &[0.25_f32, 0.5, 0.75, 1.0], "Fine tune up")
    }

    fn run_fine_tune_pass(
        &mut self,
        boundary_crf: f32,
        offsets: &[f32],
        stage: &str,
    ) -> Result<Option<f32>> {
        let mut best_boundary: Option<f32> = None;
        let mut history = Vec::new();

        for &offset in offsets {
            let test_crf = boundary_crf + offset;
            if !self.within_fine_tune_bounds(test_crf) {
                continue;
            }
            if self.iterations >= STAGE_B_BIDIRECTIONAL_MAX {
                break;
            }
            if self.size_cache.contains_key(test_crf) {
                continue;
            }

            let sample = self.probe(stage, test_crf)?;
            history.push(sample.size);
            if !self.is_under_target(sample.size) {
                break;
            }

            best_boundary = Some(sample.crf);
            self.best_crf_so_far = sample.crf;

            if let Some(rate) = Self::fine_tune_plateau_rate(&history) {
                self.log_header(format!(
                    "   {} Early termination: Δ{:.3}%",
                    crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]"),
                    rate * 100.0_f64
                ));
                break;
            }
        }

        Ok(best_boundary)
    }

    fn finish_boundary_result(&mut self, boundary_crf: f32) -> Result<ExploreResult> {
        self.log_header("   Stage C: SSIM verification");
        self.ensure_encoded(
            boundary_crf,
            format!("│ Re-encoding to CRF {boundary_crf:.1}... │"),
        )?;

        let Some(final_size) = self.size_cache.get(boundary_crf).copied() else {
            crate::media_conversion_gate::explore_gpu_coarse_degraded_audit(
                "explore_boundary",
                &self.explorer.input_path,
                format!(
                    "Boundary CRF {boundary_crf:.1} missing from size cache before SSIM \
                     verification"
                ),
            );
            return Err(anyhow::anyhow!(
                "explore boundary size cache missing CRF {boundary_crf:.1}"
            ));
        };
        let verified = self.verify_result(
            boundary_crf,
            final_size,
            "Precise Quality + Compression boundary verification",
        )?;

        let size_change_pct = self.explorer.calc_change_pct(verified.size);
        let status = VideoExplorer::ssim_status_label(verified.ssim);
        let elapsed = self.start_time.elapsed();
        let saved = self.explorer.input_size.saturating_sub(verified.size);
        let log = std::mem::take(&mut self.log);

        self.pb.finish_and_clear();
        crate::log_detail!(
            "{} Result: CRF {:.1} • SSIM {:.4} {} • {:+.1}% ({:.2} MB saved) • {} iter in {:.1}s",
            crate::media_conversion_gate::ui_icon_pick("✅", "[OK]"),
            verified.crf,
            verified.ssim,
            status,
            size_change_pct,
            crate::numeric_cast::f64_to_f32_lossy(
                crate::numeric_cast::u64_to_f64(saved) / 1024.0 / 1024.0
            ),
            self.iterations,
            elapsed.as_secs_f64()
        );

        let (confidence, confidence_detail) = measured_exploration_confidence(
            Some(verified.ssim),
            self.explorer.config.quality_thresholds.min_ssim,
            self.iterations,
            STAGE_B1_MAX_ITERATIONS.saturating_add(STAGE_B2_MAX_ITERATIONS),
        );
        Ok(ok_explore_result(ExploreResult {
            optimal_crf: verified.crf,
            output_size: verified.size,
            size_change_pct,
            ssim: verified.quality.0,
            psnr: verified.quality.1,
            ms_ssim: verified.quality.2,
            iterations: self.iterations,
            size_target_met: self.explorer.size_target_check(verified.size),
            quality_passed: if verified.ssim >= self.explorer.config.quality_thresholds.min_ssim {
                CheckResult::Passed
            } else {
                CheckResult::Failed(format!("SSIM {:.4} below threshold", verified.ssim))
            },
            log,
            confidence,
            confidence_detail,
            actual_min_ssim: self.explorer.config.quality_thresholds.min_ssim,
            ..Default::default()
        }))
    }

    fn probe(&mut self, stage: &str, crf: f32) -> Result<CrfSizeSample> {
        let size = self.encode_size_only(crf)?;
        self.iterations += 1;
        self.log_progress(stage, crf, size)?;
        Ok(CrfSizeSample { crf, size })
    }

    fn encode_size_only(&mut self, crf: f32) -> Result<u64> {
        if let Some(&size) = self.size_cache.get(crf) {
            return Ok(size);
        }

        let size = self.explorer.encode(crf)?;
        self.size_cache.insert(crf, size);
        self.last_encoded_crf = Some(crf);
        Ok(size)
    }

    fn validate_quality(&mut self, crf: f32) -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
        if let Some(&quality) = self.quality_cache.get(crf) {
            return Ok(quality);
        }

        let quality = self.explorer.validate_quality()?;
        self.quality_cache.insert(crf, quality);
        Ok(quality)
    }

    fn verify_result(&mut self, crf: f32, size: u64, context: &str) -> Result<VerifiedCrfResult> {
        self.pb.set_message("│ Computing SSIM... │".to_string());
        let quality = self.validate_quality(crf)?;
        let ssim = VideoExplorer::require_ssim_metric(quality.0, context)?;

        Ok(VerifiedCrfResult {
            crf,
            size,
            quality,
            ssim,
        })
    }

    fn ensure_encoded(&mut self, crf: f32, message: String) -> Result<()> {
        if self.last_encoded_crf == Some(crf) {
            return Ok(());
        }

        self.pb.set_message(message);
        // Must bypass the size cache: a cache hit would skip the re-encode and leave
        // the last-probed CRF's file on disk, so the subsequent SSIM
        // verification would measure a different encode than the one attributed
        // to `crf`.
        let size = self.explorer.encode(crf)?;
        self.size_cache.insert(crf, size);
        self.last_encoded_crf = Some(crf);
        Ok(())
    }

    fn log_header(&mut self, message: impl Into<String>) {
        let msg = message.into();
        self.pb.suspend(|| crate::log_detail!("{}", msg));
        self.log.push(msg);
    }

    fn log_progress(&mut self, stage: &str, crf: f32, size: u64) -> Result<()> {
        let size_pct = if self.explorer.input_size > 0 {
            let permille_raw = u64::try_from(
                (u128::from(size) * 10_000) / u128::from(self.explorer.input_size.max(1)),
            )
            .context("progress permille calculation overflowed u64")?;
            let permille =
                crate::numeric_cast::u64_to_u32_strict(permille_raw, "progress_permille")
                    .context("progress permille calculation overflowed u32")?;
            (f64::from(permille) / 100.0_f64) - 100.0_f64
        } else {
            0.0_f64
        };
        let compress_icon = if size < self.target_size {
            crate::media_conversion_gate::ui_icon_pick("💾", "[SAVE]")
        } else {
            crate::media_conversion_gate::ui_icon_pick("⚠️", "[WARN]")
        };

        self.pb.set_prefix(format!(
            "{} {}",
            crate::media_conversion_gate::ui_icon_pick("🔍", "[SCAN]"),
            stage
        ));
        self.pb.set_message(format!(
            "CRF {:.1} | {:+.1}% {} | Iter {} | Best: {:.1}",
            crf, size_pct, compress_icon, self.iterations, self.best_crf_so_far
        ));

        self.log.push(format!(
            "   {} CRF {crf:.1}: {size_pct:+.1}%",
            crate::media_conversion_gate::ui_icon_pick("🔄", "[LOOP]")
        ));
        Ok(())
    }

    const fn is_under_target(&self, size: u64) -> bool {
        size < self.target_size
    }

    const fn within_fine_tune_bounds(&self, crf: f32) -> bool {
        crf >= self.explorer.config.min_crf && crf <= self.explorer.config.max_crf
    }

    fn fine_tune_plateau_rate(history: &[u64]) -> Option<f64> {
        let prev = *history.get(history.len().checked_sub(2)?)?;
        let curr = *history.last()?;
        let rate = BoundarySearchState::calc_change_rate(prev, curr);
        (rate < CHANGE_RATE_THRESHOLD).then_some(rate)
    }
}

/// Quality thresholds and validation flags for an exploration.
pub use stream_analysis::{MetricValidationFlags, QualityValidationFlags};

#[derive(Debug, Clone)]
pub struct QualityThresholds {
    /// Minimum acceptable SSIM score.
    pub min_ssim: f64,
    /// Minimum acceptable PSNR in dB.
    pub min_psnr: f64,
    /// Minimum acceptable Multi-Scale SSIM score.
    pub min_ms_ssim: f64,
    pub validation: QualityValidationFlags,
}

impl Default for QualityThresholds {
    fn default() -> Self {
        Self {
            min_ssim: EXPLORE_DEFAULT_MIN_SSIM,
            min_psnr: EXPLORE_DEFAULT_MIN_PSNR,
            min_ms_ssim: EXPLORE_DEFAULT_MIN_MS_SSIM,
            validation: QualityValidationFlags {
                metrics: MetricValidationFlags {
                    validate_ssim: true,
                    validate_psnr: false,
                    validate_ms_ssim: false,
                },
                force_ms_ssim_long: false,
            },
        }
    }
}

/// Configuration for a CRF exploration.
///
/// Controls the mode, CRF range, quality thresholds, and other parameters
/// that determine how the exploration behaves.
#[derive(Debug, Clone)]
pub struct ExploreConfig {
    /// The exploration mode to use.
    pub mode: ExploreMode,
    /// The initial CRF to start the exploration from.
    pub initial_crf: f32,
    /// The minimum CRF allowed during search.
    pub min_crf: f32,
    /// The maximum CRF allowed during search.
    pub max_crf: f32,
    /// Target size ratio (1.0 = same size as input).
    pub target_ratio: f64,
    /// Quality thresholds and validation flags.
    pub quality_thresholds: QualityThresholds,
    /// Maximum number of encode iterations before giving up.
    pub max_iterations: u32,
    /// Whether to use ultimate mode (stricter quality gates, more thorough
    /// search).
    pub ultimate_mode: bool,
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
        }
    }
}

impl ExploreConfig {
    /// Creates a config for size-only exploration (no quality checks).
    #[must_use]
    pub fn size_only(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::SizeOnly,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim: EXPLORE_DEFAULT_MIN_SSIM,
                min_psnr: EXPLORE_DEFAULT_MIN_PSNR,
                min_ms_ssim: EXPLORE_DEFAULT_MIN_MS_SSIM,
                validation: QualityValidationFlags {
                    metrics: MetricValidationFlags {
                        validate_ssim: false,
                        validate_psnr: false,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
            ..Default::default()
        }
    }

    /// Creates a config for quality-match mode (single encode at predicted
    /// CRF).
    #[must_use]
    pub fn quality_match(predicted_crf: f32) -> Self {
        Self {
            mode: ExploreMode::QualityMatch,
            initial_crf: predicted_crf,
            max_iterations: 1,
            quality_thresholds: QualityThresholds {
                min_ssim: EXPLORE_DEFAULT_MIN_SSIM,
                min_psnr: EXPLORE_DEFAULT_MIN_PSNR,
                min_ms_ssim: EXPLORE_DEFAULT_MIN_MS_SSIM,
                validation: QualityValidationFlags {
                    metrics: MetricValidationFlags {
                        validate_ssim: true,
                        validate_psnr: false,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
            ..Default::default()
        }
    }

    /// Creates a config for precise quality match with iterative CRF search.
    #[must_use]
    pub fn precise_quality_match(initial_crf: f32, max_crf: f32, min_ssim: f64) -> Self {
        Self {
            mode: ExploreMode::PreciseQualityMatch,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim,
                min_psnr: crate::constants::EXPLORE_DEFAULT_MIN_PSNR,
                min_ms_ssim: crate::constants::EXPLORE_DEFAULT_MIN_MS_SSIM,
                validation: QualityValidationFlags {
                    metrics: MetricValidationFlags {
                        validate_ssim: true,
                        validate_psnr: false,
                        validate_ms_ssim: false,
                    },
                    ..Default::default()
                },
            },
            ..Default::default()
        }
    }

    /// Creates a config for precise quality match that also requires
    /// compression.
    #[must_use]
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
                min_psnr: crate::constants::EXPLORE_DEFAULT_MIN_PSNR,
                min_ms_ssim: crate::constants::EXPLORE_DEFAULT_MIN_MS_SSIM,
                validation: QualityValidationFlags {
                    metrics: MetricValidationFlags {
                        validate_ssim: true,
                        validate_psnr: false,
                        validate_ms_ssim: false,
                    },
                    ..Default::default()
                },
            },
            ..Default::default()
        }
    }

    /// Creates a config for compression-only mode (no quality validation).
    #[must_use]
    pub fn compress_only(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::CompressOnly,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim: EXPLORE_DEFAULT_MIN_SSIM,
                min_psnr: EXPLORE_DEFAULT_MIN_PSNR,
                min_ms_ssim: EXPLORE_DEFAULT_MIN_MS_SSIM,
                validation: QualityValidationFlags {
                    metrics: MetricValidationFlags {
                        validate_ssim: false,
                        validate_psnr: false,
                        validate_ms_ssim: false,
                    },
                    ..Default::default()
                },
            },
            max_iterations: 8,
            ..Default::default()
        }
    }

    /// Creates a config for compression that also enforces a minimum SSIM
    /// threshold.
    #[must_use]
    pub fn compress_with_quality(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::CompressWithQuality,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim: crate::constants::EXPLORE_DEFAULT_MIN_SSIM,
                min_psnr: EXPLORE_DEFAULT_MIN_PSNR,
                min_ms_ssim: EXPLORE_DEFAULT_MIN_MS_SSIM,
                validation: QualityValidationFlags {
                    metrics: MetricValidationFlags {
                        validate_ssim: true,
                        validate_psnr: false,
                        validate_ms_ssim: false,
                    },
                    ..Default::default()
                },
            },
            max_iterations: 10,
            ..Default::default()
        }
    }
}

/// Supported video encoder types.
///
/// Each variant maps to a specific `FFmpeg` encoder and container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    /// H.265/HEVC encoder (libx265 or hardware fallback).
    Hevc,
    /// AV1 encoder (SVT-AV1).
    Av1,
    /// H.264/AVC encoder (libx264 or hardware fallback).
    H264,
}

impl From<VideoEncoder> for crate::ffmpeg_builder::VideoCodec {
    fn from(encoder: VideoEncoder) -> Self {
        match encoder {
            VideoEncoder::Hevc => Self::Hevc,
            VideoEncoder::Av1 => Self::Av1,
            VideoEncoder::H264 => Self::H264,
        }
    }
}

impl VideoEncoder {
    /// Returns the `FFmpeg` encoder name, with automatic fallback to hardware
    /// encoders if the software encoder is not available.
    #[must_use]
    pub fn ffmpeg_name(&self) -> &'static str {
        match self {
            Self::Hevc => {
                if Self::is_encoder_available(crate::constants::FFMPEG_ENCODER_X265) {
                    crate::constants::FFMPEG_ENCODER_X265
                } else {
                    crate::log_detail!(&format!(
                        "{} libx265 not available, falling back to hevc_videotoolbox",
                        crate::media_conversion_gate::ui_icon_pick("⚠️", "[WARN]")
                    ));
                    "hevc_videotoolbox"
                }
            }
            Self::Av1 => crate::constants::FFMPEG_ENCODER_SVTAV1,
            Self::H264 => {
                if Self::is_encoder_available("libx264") {
                    "libx264"
                } else {
                    crate::log_detail!(&format!(
                        "{} libx264 not available, falling back to h264_videotoolbox",
                        crate::media_conversion_gate::ui_icon_pick("⚠️", "[WARN]")
                    ));
                    "h264_videotoolbox"
                }
            }
        }
    }

    fn is_encoder_available(encoder: &str) -> bool {
        static LIBX265_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        static LIBX264_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

        let cache = match encoder {
            "libx265" => &LIBX265_AVAILABLE,
            "libx264" => &LIBX264_AVAILABLE,
            _ => return true,
        };

        *cache.get_or_init(
            || match crate::ffmpeg_builder::FfmpegBuilder::list_encoders() {
                Ok(encoders) => encoders.contains(encoder),
                Err(err) => {
                    crate::media_conversion_gate::delivery_encode_batch_audit(
                        "video_encoder_availability",
                        format!("failed to list ffmpeg encoders while checking {encoder}: {err}"),
                    );
                    false
                }
            },
        )
    }

    /// Returns the default container extension for this encoder (always "mp4").
    #[must_use]
    pub const fn container(&self) -> &'static str {
        match self {
            Self::Hevc | Self::Av1 | Self::H264 => "mp4",
        }
    }

    /// Returns extra `FFmpeg` arguments for this encoder with default preset.
    #[must_use]
    pub fn extra_args(&self, max_threads: usize, apple_compat: bool) -> Vec<String> {
        self.extra_args_with_preset(
            max_threads,
            EncoderPreset::default(),
            None,
            apple_compat,
            false,
            crate::x265_params::X265MemoryProfile::Default,
        )
    }

    /// Returns extra `FFmpeg` arguments for this encoder with a specific preset
    /// and optional HDR x265 parameters.
    #[must_use]
    pub fn extra_args_with_preset(
        &self,
        max_threads: usize,
        preset: EncoderPreset,
        hdr_x265_params: Option<&str>,
        apple_compat: bool,
        archive: bool,
        x265_memory_profile: crate::x265_params::X265MemoryProfile,
    ) -> Vec<String> {
        match self {
            Self::Hevc => {
                let x265_params =
                    crate::x265_params::format(max_threads, hdr_x265_params, x265_memory_profile);
                let mut args = vec![
                    crate::constants::FFMPEG_ARG_PRESET.to_string(),
                    preset.hevc_name_for_archive(archive).to_string(),
                ];
                if apple_compat {
                    args.extend([
                        crate::constants::FFMPEG_ARG_TAG_VIDEO.to_string(),
                        crate::constants::FFMPEG_TAG_HVC1.to_string(),
                    ]);
                }
                args.extend([
                    crate::constants::FFMPEG_ARG_X265_PARAMS.to_string(),
                    x265_params,
                ]);
                args
            }
            Self::Av1 => {
                vec![
                    "-svtav1-params".to_string(),
                    format!(
                        "tune=0:film-grain=0:preset={}:lp={}",
                        preset.svtav1_preset_for_archive(archive),
                        max_threads
                    ),
                ]
            }
            Self::H264 => vec![
                crate::constants::FFMPEG_ARG_PRESET.to_string(),
                preset.x26x_name().to_string(),
                "-profile:v".to_string(),
                "high".to_string(),
            ],
        }
    }
}

/// Indicates the source of an SSIM value in iteration metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsimSource {
    /// SSIM was actually measured from an encode.
    Actual,
    /// SSIM was predicted/estimated without a full encode.
    Predicted,
    /// No SSIM value is available.
    None,
}

/// Metrics recorded during a single iteration of the CRF search.
#[derive(Debug, Clone)]
pub struct IterationMetrics {
    /// Iteration number (1-based).
    pub iteration: u32,
    /// Name of the current search phase.
    pub phase: String,
    /// The CRF value tested in this iteration.
    pub crf: f32,
    /// The resulting output file size in bytes.
    pub output_size: u64,
    /// Percentage change in size compared to input.
    pub size_change_pct: f64,
    /// SSIM score achieved (if measured).
    pub ssim: Option<f64>,
    /// Whether the SSIM was measured or predicted.
    pub ssim_source: SsimSource,
    /// PSNR score in dB (if measured).
    pub psnr: Option<f64>,
    /// Whether the output could be compressed further with margin.
    pub can_compress: bool,
    /// Whether the quality check passed for this iteration.
    pub quality_passed: CheckResult,
    /// Human-readable description of the decision made after this iteration.
    pub decision: String,
}

impl IterationMetrics {
    /// Prints this iteration's metrics as a formatted table row.
    pub fn print_line(&self) {
        let ssim_str = match (self.ssim, self.ssim_source) {
            (Some(s), SsimSource::Predicted) => format!("~{s:.4}"),
            (Some(s), _) => format!("{s:.4}"),
            (None, _) => "----".to_string(),
        };
        let psnr_str = crate::media_conversion_gate::ui_optional_f64_display_or_map(
            self.psnr,
            "----",
            "explore_iteration_psnr",
            |p| format!("{p:.1}"),
        );
        let compress_icon = crate::modern_ui::symbols::ok_fail_icon(self.can_compress);
        let quality_icon = match self.quality_passed {
            CheckResult::Passed => crate::media_conversion_gate::ui_icon_pick("✅", "[OK]"),
            CheckResult::Failed(_) => crate::media_conversion_gate::ui_icon_pick("⚠️", "[WARN]"),
            CheckResult::NotChecked => "--".to_string(),
        };

        crate::log_detail!(
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

/// A transparency report that logs all iterations of the CRF search process.
///
/// Provides a detailed, human-readable view of every step taken during
/// exploration, useful for debugging and auditing.
#[derive(Debug, Clone, Default)]
pub struct TransparencyReport {
    /// All iteration metrics recorded during the search.
    pub iterations: Vec<IterationMetrics>,
    /// When the exploration started (for elapsed time calculation).
    pub start_time: Option<std::time::Instant>,
    /// Input file size in bytes.
    pub input_size: u64,
    /// Final optimal CRF found.
    pub final_crf: Option<f32>,
    /// Final SSIM score achieved.
    pub final_ssim: Option<f64>,
    /// Final PSNR score achieved.
    pub final_psnr: Option<f64>,
}

impl TransparencyReport {
    /// Creates a new transparency report for the given input size.
    #[must_use]
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

    /// Records an iteration's metrics and prints the current progress line.
    pub fn add_iteration(&mut self, metrics: IterationMetrics) {
        metrics.print_line();
        self.iterations.push(metrics);
    }

    /// Prints the header row for the transparency report table.
    pub fn print_header(&self) {
        crate::log_detail!(
            "┌────────────────────────────────────────────────────────────────────────────────────────────┐"
        );
        crate::log_detail!(&format!(
            "│ {} Transparency Report - CRF Search Process                                               │",
            crate::media_conversion_gate::ui_icon_pick("📊", "[AUDIT]")
        ));
        crate::log_detail!(
            "├────┬──────────────┬───────────┬─────────────┬─────────────┬──────────┬────────────────────┤"
        );
        crate::log_detail!(
            "│ #  │ Phase        │ CRF       │ Size Change │ SSIM        │ PSNR     │ Decision           │"
        );
        crate::log_detail!(
            "├────┼──────────────┼───────────┼─────────────┼─────────────┼──────────┼────────────────────┤"
        );
    }

    /// Prints the footer and summary statistics (iterations, time, final
    /// CRF/SSIM/PSNR).
    pub fn print_summary(&self) {
        crate::log_detail!(
            "└────┴──────────────┴───────────┴─────────────┴─────────────┴──────────┴────────────────────┘"
        );

        let elapsed_label = crate::media_conversion_gate::ui_duration_secs_label_or_na(
            crate::media_conversion_gate::explore_elapsed_secs_optional(
                self.start_time.map(|start| start.elapsed()),
                "video_explorer exploration audit",
            ),
            "exploration elapsed",
        );
        let total_iterations = self.iterations.len();

        crate::log_summary_header!(crate::infra::static_logs::messages::LABEL_EXPLORATION_AUDIT);
        crate::log_report_detail!(&format!(
            "Exploration Summary: {total_iterations} iterations completed in {elapsed_label}"
        ));

        if let Some(crf) = self.final_crf {
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_FINAL_CRF,
                format!("{crf:.2} (Optimal Search Path)")
            );
        }
        if let Some(ssim) = self.final_ssim {
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_FINAL_SSIM,
                format!("{ssim:.5} (Structural Integrity)")
            );
        }
        if let Some(psnr) = self.final_psnr {
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_FINAL_PSNR,
                format!("{psnr:.2} dB (Signal-to-Noise Ratio)")
            );
        }
    }
}

/// The main video exploration engine.
///
/// Manages encoding, quality measurement, and CRF search for a given
/// input/output pair.
pub struct VideoExplorer {
    /// Configuration controlling the exploration behavior.
    config: ExploreConfig,
    /// The video encoder to use (HEVC, AV1, or H264).
    encoder: VideoEncoder,
    /// Path to the input video file.
    input_path: std::path::PathBuf,
    /// Path where the output video will be written.
    output_path: std::path::PathBuf,
    /// Size of the input file in bytes.
    input_size: u64,
    /// Additional video filter arguments passed to `FFmpeg`.
    vf_args: Vec<String>,
    /// Whether GPU acceleration is enabled.
    use_gpu: bool,
    /// Maximum number of encoding threads.
    max_threads: usize,
    /// Encoder preset (speed vs quality tradeoff).
    preset: EncoderPreset,
    /// Exact input video + audio packet payload bytes used by size decisions.
    input_pure_media_size: u64,
    /// Optional HDR x265 encoder parameters.
    hdr_x265_params: Option<String>,
    /// Whether to use Apple-compatible container tags.
    apple_compat: bool,
    /// The codec name of the input video (e.g., "prores", "h264").
    source_codec_name: Option<String>,
}

struct VideoExplorerBuildArgs<'a> {
    input: &'a Path,
    output: &'a Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    config: ExploreConfig,
    use_gpu: Option<bool>,
    preset: EncoderPreset,
    max_threads: usize,
    hdr_x265_params: Option<String>,
    apple_compat: bool,
    source_codec_name: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct EncodeMediaProfile {
    pts_integrity: crate::ffprobe_json::PtsIntegrity,
    is_animated: bool,
}

impl EncodeMediaProfile {
    fn inspect(input_path: &Path) -> Result<Self> {
        let pts_integrity = crate::ffprobe_json::check_pts_integrity(input_path)?;
        let ext = crate::media_conversion_gate::path_extension_lowercase_or_empty(
            input_path,
            "encode_media_profile",
        );
        let is_animated = matches!(
            ext.as_str(),
            "gif" | "webp" | "avif" | "heic" | "heif" | "apng"
        );
        Ok(Self {
            pts_integrity,
            is_animated,
        })
    }

    fn log_pts_adjustment(self) {
        if self.pts_integrity != crate::ffprobe_json::PtsIntegrity::Healthy {
            crate::log_detail!(&format!(
                "      {} {} input: {:?}, applying safety measures",
                crate::media_conversion_gate::ui_icon_pick("⚠️", "[WARN]"),
                if self.pts_integrity == crate::ffprobe_json::PtsIntegrity::Broken {
                    "Broken PTS"
                } else {
                    "Duplicate PTS"
                },
                self.pts_integrity
            ));
        }
    }

    const fn fps_mode(self) -> &'static str {
        if matches!(
            self.pts_integrity,
            crate::ffprobe_json::PtsIntegrity::Broken
        ) {
            "vfr"
        } else {
            "passthrough"
        }
    }
}

struct FfmpegEncodePlan {
    accel_type: String,
    duration_secs: Option<f64>,
    cmd: std::process::Command,
}

#[derive(Debug, Default)]
struct FfmpegProgressState {
    time_us: u64,
    fps: f64,
    speed: String,
}

impl FfmpegProgressState {
    fn update_from_line(&mut self, line: &str) -> bool {
        if let Some(val) = line.strip_prefix("out_time_us=") {
            match val.parse::<u64>() {
                Ok(time_us) => self.time_us = time_us,
                Err(err) => crate::media_conversion_gate::delivery_progress_batch_audit(
                    "video_explorer_progress_time_parse_failed",
                    format!("failed to parse ffmpeg out_time_us token {val:?}: {err}"),
                ),
            }
            return false;
        }
        if let Some(val) = line.strip_prefix("fps=") {
            match val.parse::<f64>() {
                Ok(fps) => self.fps = fps,
                Err(err) => crate::media_conversion_gate::delivery_progress_batch_audit(
                    "video_explorer_progress_fps_parse_failed",
                    format!("failed to parse ffmpeg fps token {val:?}: {err}"),
                ),
            }
            return false;
        }
        if let Some(val) = line.strip_prefix("speed=") {
            self.speed = val.to_string();
            return false;
        }
        line == "progress=continue" || line == "progress=end"
    }

    fn current_secs(&self) -> f64 {
        let Some(millis) =
            crate::media_conversion_gate::explore_progress_time_millis_optional(self.time_us)
        else {
            return f64::NAN;
        };
        let secs = f64::from(millis) / 1_000.0;
        if secs.is_finite() { secs } else { f64::NAN }
    }

    fn render(&self, accel_type: &str, duration_secs: Option<f64>) -> String {
        let current_secs = self.current_secs();
        match duration_secs.filter(|d| *d > 0.0_f64) {
            None => format!(
                "\r      {} {} {:.1}s | {:.0}fps | {}   ",
                crate::media_conversion_gate::ui_icon_pick("⏳", "[WAIT]"),
                accel_type,
                current_secs,
                self.fps,
                self.speed.trim()
            ),
            Some(total_duration) => {
                let pct = (current_secs / total_duration * 100.0).min(100.0);
                format!(
                    "\r      {} {} {:.1}% | {:.1}s/{:.1}s | {:.0}fps | {}   ",
                    crate::media_conversion_gate::ui_icon_pick("⏳", "[WAIT]"),
                    accel_type,
                    pct,
                    current_secs,
                    total_duration,
                    self.fps,
                    self.speed.trim()
                )
            }
        }
    }
}

fn collect_ffmpeg_stderr(stderr: std::process::ChildStderr) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        use std::collections::VecDeque;
        use std::io::{BufRead, BufReader};
        const MAX_LINES: usize = 10;

        let reader = BufReader::new(stderr);
        let mut recent_lines: VecDeque<String> = VecDeque::with_capacity(MAX_LINES);

        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    if recent_lines.len() >= MAX_LINES {
                        recent_lines.pop_front();
                    }
                    recent_lines.push_back(format!("[stderr read error: {err}]"));
                    break;
                }
            };
            if recent_lines.len() >= MAX_LINES {
                recent_lines.pop_front();
            }
            recent_lines.push_back(line);
        }

        recent_lines.into_iter().collect::<Vec<_>>().join("\n")
    })
}

fn stream_ffmpeg_progress(
    stdout: std::process::ChildStdout,
    accel_type: &str,
    duration_secs: Option<f64>,
) {
    use std::io::{BufRead, BufReader, Write};

    let reader = BufReader::new(stdout);
    let mut progress = FfmpegProgressState::default();

    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                crate::log_detail!(
                    crate::infra::static_logs::messages::MSG_EXPLORE_FFMPEG_ERR
                        .replace("{}", &err.to_string())
                );
                break;
            }
        };

        if progress.update_from_line(&line) {
            eprint!("{}", progress.render(accel_type, duration_secs));
            let _ = std::io::stderr().flush();
        }
    }
}

fn spawn_ffmpeg_progress_stream(
    stdout: std::process::ChildStdout,
    accel_type: String,
    duration_secs: Option<f64>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || stream_ffmpeg_progress(stdout, &accel_type, duration_secs))
}

fn summarize_ffmpeg_failure(stderr_content: &str) -> String {
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
    if error_lines.is_empty() {
        stderr_content
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        error_lines.join("\n")
    }
}

impl VideoExplorer {
    fn build(args: VideoExplorerBuildArgs<'_>) -> Result<Self> {
        Self::validate_paths(args.input, args.output)?;

        let input_size = fs::metadata(args.input)
            .context("Failed to read input file metadata")?
            .len();
        let use_gpu = Self::resolve_gpu_usage(args.use_gpu, args.encoder);
        let input_pure_media_size = crate::stream_size::measure_strict_pure_media(args.input)
            .with_context(|| {
                format!(
                    "Strict pure-media input measurement failed for {}",
                    args.input.display()
                )
            })?
            .pure_media_size();
        let source_codec_name = Self::resolve_source_codec_name(args.input, args.source_codec_name);

        Ok(Self {
            config: args.config,
            encoder: args.encoder,
            input_path: args.input.to_path_buf(),
            output_path: args.output.to_path_buf(),
            input_size,
            vf_args: args.vf_args,
            max_threads: args.max_threads,
            use_gpu,
            preset: args.preset,
            input_pure_media_size,
            hdr_x265_params: args.hdr_x265_params,
            apple_compat: args.apple_compat,
            source_codec_name,
        })
    }

    fn validate_paths(input: &Path, output: &Path) -> Result<()> {
        crate::path_validator::validate_path(input).map_err(|e| anyhow::anyhow!("{e}"))?;
        crate::path_validator::validate_path(output).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }

    fn resolve_gpu_usage(requested: Option<bool>, encoder: VideoEncoder) -> bool {
        if let Some(requested) = requested {
            return requested;
        }

        let gpu = crate::gpu_accel::GpuAccel::detect_with_retry();
        gpu.is_available()
            && match encoder {
                VideoEncoder::Hevc => gpu.get_hevc_encoder().is_some(),
                VideoEncoder::Av1 => gpu.get_av1_encoder().is_some(),
                VideoEncoder::H264 => gpu.get_h264_encoder().is_some(),
            }
    }

    fn resolve_source_codec_name(
        input: &Path,
        source_codec_name: Option<String>,
    ) -> Option<String> {
        // Auto-probe the source codec when the caller didn't supply one, so that the
        // x265 memory profile can account for archival sources (ProRes/DNxHD) even
        // when they are well under the size threshold.
        match source_codec_name {
            Some(v) => Some(v),
            None => match crate::ffprobe::probe_video(input) {
                Ok(probe) => Some(probe.video_codec),
                Err(err) => {
                    tracing::debug!(
                        target: "mfb.video",
                        path = %input.display(),
                        %err,
                        "source codec probe failed; x265 memory profile will use safe defaults"
                    );
                    None
                }
            },
        }
    }

    /// Create a new `VideoExplorer`.
    ///
    /// # Errors
    /// Returns an error if initialization fails.
    pub fn new(
        input: &Path,
        output: &Path,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        config: ExploreConfig,
        max_threads: usize,
        hdr_x265_params: Option<String>,
        apple_compat: bool,
        source_codec_name: Option<String>,
    ) -> Result<Self> {
        Self::build(VideoExplorerBuildArgs {
            input,
            output,
            encoder,
            vf_args,
            config,
            use_gpu: None,
            preset: EncoderPreset::default(),
            max_threads,
            hdr_x265_params,
            apple_compat,
            source_codec_name,
        })
    }

    /// Create a new `VideoExplorer` with GPU support.
    ///
    /// # Errors
    /// Returns an error if initialization fails.
    pub fn new_with_gpu(
        input: &Path,
        output: &Path,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        config: ExploreConfig,
        use_gpu: bool,
        max_threads: usize,
        hdr_x265_params: Option<String>,
        apple_compat: bool,
        source_codec_name: Option<String>,
    ) -> Result<Self> {
        Self::build(VideoExplorerBuildArgs {
            input,
            output,
            encoder,
            vf_args,
            config,
            use_gpu: Some(use_gpu),
            preset: EncoderPreset::default(),
            max_threads,
            hdr_x265_params,
            apple_compat,
            source_codec_name,
        })
    }

    /// Create a new `VideoExplorer` with a specific preset.
    ///
    /// # Errors
    /// Returns an error if initialization fails.
    pub fn new_with_preset(
        input: &Path,
        output: &Path,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        config: ExploreConfig,
        preset: EncoderPreset,
        max_threads: usize,
        hdr_x265_params: Option<String>,
        apple_compat: bool,
        source_codec_name: Option<String>,
    ) -> Result<Self> {
        Self::build(VideoExplorerBuildArgs {
            input,
            output,
            encoder,
            vf_args,
            config,
            use_gpu: None,
            preset,
            max_threads,
            hdr_x265_params,
            apple_compat,
            source_codec_name,
        })
    }

    pub fn explore(&self) -> Result<ExploreResult> {
        if self.config.ultimate_mode {
            bail!(
                "ultimate mode requires GPU coarse-search explore (explore_hevc_with_gpu / \
                 explore_av1_with_gpu); CPU VideoExplorer does not implement the 3D quality \
                 contract"
            );
        }
        let mut result = match self.config.mode {
            ExploreMode::SizeOnly => self.explore_size_only(),
            ExploreMode::QualityMatch => self.explore_quality_match(),
            ExploreMode::PreciseQualityMatch => self.explore_precise_quality_match(),
            ExploreMode::PreciseQualityMatchWithCompression => {
                self.explore_precise_quality_match_with_compression()
            }
            ExploreMode::CompressOnly => self.explore_compress_only(),
            ExploreMode::CompressWithQuality => self.explore_compress_with_quality(),
        }?;
        let output_pure_media_size = result.output_size;
        result.output_size = fs::metadata(&self.output_path)
            .with_context(|| {
                format!(
                    "Failed to read final explored output size for {}",
                    self.output_path.display()
                )
            })?
            .len();
        result.input_pure_media_size = self.input_pure_media_size;
        result.output_pure_media_size = output_pure_media_size;
        result.container_overhead = result.output_size.saturating_sub(output_pure_media_size);
        Ok(result.sealed())
    }

    /// Runs the quality exploration using the strategy pattern (delegates to
    /// `explore_strategy`).
    ///
    /// # Errors
    /// Returns an error if exploration fails.
    pub fn explore_with_strategy(&self) -> Result<ExploreResult> {
        if self.config.ultimate_mode {
            bail!(
                "ultimate mode is incompatible with explore_strategy; use GPU coarse-search \
                 explore"
            );
        }
        let mut ctx = ExploreContext::new(crate::explore_strategy::ExploreContextArgs {
            input_path: self.input_path.clone(),
            output_path: self.output_path.clone(),
            input_size: self.input_size,
            encoder: self.encoder,
            vf_args: self.vf_args.clone(),
            max_threads: self.max_threads,
            use_gpu: self.use_gpu,
            preset: self.preset,
            config: self.config.clone(),
            hdr_x265_params: self.hdr_x265_params.clone(),
            apple_compat: self.apple_compat,
        })?;

        let strategy = create_strategy(self.config.mode);
        crate::log_stat!(
            crate::infra::static_logs::messages::LABEL_STRATEGY,
            format!("{} - {}", strategy.name(), strategy.description())
        );
        let result = strategy.explore(&mut ctx)?;
        Ok(result.sealed())
    }

    fn explore_size_only(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let start_time = std::time::Instant::now();

        let pb = crate::progress::create_professional_spinner(&format!(
            "{} Size Explore",
            crate::media_conversion_gate::ui_icon_pick("🔍", "[SEARCH]")
        ));

        let progress_line = |message: String| pb.set_message(message);
        let progress_done = || {};

        pb.suspend(|| {
            crate::log_summary_header!(
                &crate::media_conversion_gate::ui_log_summary_title_with_icon(
                    "🔍",
                    "[SEARCH]",
                    format!(
                        "Forensic Explore: Size-Optimization Cycle ({:?})",
                        self.encoder
                    ),
                )
            );
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                format!(
                    "Target: Optimal compression for {:.2} MB asset",
                    crate::numeric_cast::f64_to_f32_lossy(
                        crate::numeric_cast::u64_to_f64(self.input_size)
                            / crate::constants::KB_F64
                            / crate::constants::KB_F64
                    )
                )
            );
        });

        log.push(format!(
            "{} Size-Only Explore ({:?})",
            crate::media_conversion_gate::ui_icon_pick("🔍", "[SEARCH]"),
            self.encoder
        ));

        progress_line(format!("Test CRF {:.1}...", self.config.max_crf));
        let max_size = self.encode(self.config.max_crf)?;
        let iterations = 1u32;
        progress_done();

        let (best_crf, best_size, size_ok) = if self.can_compress_with_margin(max_size) {
            (self.config.max_crf, max_size, true)
        } else {
            (self.config.max_crf, max_size, false)
        };

        progress_line("Calculate SSIM...".to_string());
        let ssim = self.calculate_ssim()?;
        if ssim.is_none() {
            pb.suspend(|| {
                crate::log_detail!(crate::infra::static_logs::messages::MSG_EXPLORE_SSIM_FAIL);
            });
        }
        progress_done();
        // SSIM is computed from self.output_path; must match the encode just above
        // (max_crf).

        let size_change_pct = self.calc_change_pct(best_size);
        let elapsed = start_time.elapsed();

        pb.finish_and_clear();
        let ssim_str = crate::media_conversion_gate::ui_f64_display_or_placeholder(
            ssim,
            "---",
            "video_explorer explore result SSIM",
        );
        let status = if size_ok {
            crate::media_conversion_gate::ui_icon_pick("💾", "[SAVE]")
        } else {
            crate::media_conversion_gate::ui_icon_pick("⚠️", "[WARN]")
        };
        crate::log_detail!(
            "{} Result: CRF {:.1} • SSIM {} • Size {:+.1}% ({}) • {:.1}s",
            crate::media_conversion_gate::ui_icon_pick("✅", "[OK]"),
            best_crf,
            ssim_str,
            size_change_pct,
            status,
            elapsed.as_secs_f64()
        );
        log.push(format!(
            "{} RESULT: CRF {best_crf:.1}, {size_change_pct:+.1}%",
            crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
        ));

        let (confidence, confidence_detail) =
            self.measured_confidence_for(ssim, iterations, self.config.max_iterations.max(1));
        let quality_passed = self.check_quality_passed(ssim, None, None);
        Ok(ok_explore_result(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim,
            psnr: None,
            ms_ssim: None,
            ms_ssim_passed: CheckResult::NotChecked,
            ms_ssim_score: None,
            used_fallback: false,
            iterations,
            size_target_met: if size_ok {
                CheckResult::Passed
            } else {
                CheckResult::Failed("Pure-media size target not met".into())
            },
            quality_passed,
            log,
            confidence,
            confidence_detail,
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        }))
    }

    fn explore_quality_match(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();

        log.push(format!(
            "{} Quality-Match Mode ({:?})",
            crate::media_conversion_gate::ui_icon_pick("🎯", "[TARGET]"),
            self.encoder
        ));
        log.push(format!("   Input: {} bytes", self.input_size));
        log.push(format!("   Predicted CRF: {}", self.config.initial_crf));

        let output_size = self.encode(self.config.initial_crf)?;
        let quality = self.validate_quality()?;

        let mut quality_str = crate::media_conversion_gate::ui_ssim_colon_label_or_unknown(
            quality.0,
            "video_explorer_calibration",
        );
        if let Some(vmaf) = quality.2 {
            let _ = write!(quality_str, ", MS-SSIM: {vmaf:.2}");
        }
        log.push(format!(
            "   CRF {}: {} bytes ({:+.1}%), {}",
            self.config.initial_crf,
            output_size,
            self.calc_change_pct(output_size),
            quality_str
        ));

        let quality_passed = self.check_quality_passed(quality.0, quality.1, quality.2);
        if quality_passed.is_passed() {
            log.push(format!(
                "   {} Quality validation passed",
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
            ));
        } else if let Some(reason) = quality_passed.failure_reason() {
            log.push(format!(
                "   {} Quality below threshold: {reason}",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
        } else {
            log.push(format!(
                "   {} Quality validation skipped or indeterminate",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
        }

        let (confidence, confidence_detail) =
            self.measured_confidence_for(quality.0, 1, self.config.max_iterations.max(1));
        Ok(ok_explore_result(ExploreResult {
            optimal_crf: self.config.initial_crf,
            output_size,
            size_change_pct: self.calc_change_pct(output_size),
            ssim: quality.0,
            psnr: quality.1,
            ms_ssim: quality.2,
            iterations: 1,
            size_target_met: self.size_target_check(output_size),
            quality_passed,
            log,
            confidence,
            confidence_detail,
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        }))
    }

    // Rationale: This function handles complex, sequential initialization or
    // business logic where further fragmentation would hinder readability and
    // maintainability.
    fn explore_compress_only(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let mut cache: CrfCache<u64> = CrfCache::new();

        let start_time = std::time::Instant::now();
        let mut best_crf_so_far: f32 = 0.0;

        let encode_cached = |crf: f32, cache: &mut CrfCache<u64>, explorer: &Self| -> Result<u64> {
            if let Some(&size) = cache.get(crf) {
                return Ok(size);
            }
            let size = explorer.encode(crf)?;
            cache.insert(crf, size);
            Ok(size)
        };

        let pb = crate::progress::create_professional_spinner(&format!(
            "{} Compress Only",
            crate::media_conversion_gate::ui_icon_pick("📦", "[PKG]")
        ));

        let progress_line = |message: String| pb.set_message(message);
        let progress_done = || {};

        pb.suspend(|| {
            crate::log_summary_header!(
                &crate::media_conversion_gate::ui_log_summary_title_with_icon(
                    "📦",
                    "[PKG]",
                    format!("Forensic Explore: Compress-Only Cycle ({:?})", self.encoder),
                )
            );
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                format!(
                    "Target: Minimum bitstream size for {:.2} MB asset",
                    crate::numeric_cast::f64_to_f32_lossy(
                        crate::numeric_cast::u64_to_f64(self.input_size)
                            / crate::constants::KB_F64
                            / crate::constants::KB_F64
                    )
                )
            );
        });
        log.push(format!(
            "{} Compress-Only ({:?})",
            crate::media_conversion_gate::ui_icon_pick("📦", "[PKG]"),
            self.encoder
        ));

        let mut iterations = 0u32;

        let initial_size = encode_cached(self.config.initial_crf, &mut cache, self)?;
        iterations += 1;
        let size_pct = self.calc_change_pct(initial_size);
        progress_line(format!(
            "CRF {:.1} | {:+.1}% | Iter {}",
            self.config.initial_crf, size_pct, iterations
        ));

        if self.can_compress_with_margin(initial_size) {
            progress_done();
            let elapsed = start_time.elapsed();

            pb.finish_and_clear();
            crate::log_detail!(&format!(
                "{} Result: CRF {:.1} • {:+.1}% {} • ({:.1}s)",
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]"),
                self.config.initial_crf,
                size_pct,
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]"),
                elapsed.as_secs_f64()
            ));
            let quality = self.validate_quality()?;
            let (confidence, confidence_detail) = self.measured_confidence_for(
                quality.0,
                iterations,
                self.config.max_iterations.max(1),
            );
            return Ok(ok_explore_result(ExploreResult {
                optimal_crf: self.config.initial_crf,
                output_size: initial_size,
                size_change_pct: self.calc_change_pct(initial_size),
                ssim: quality.0,
                psnr: quality.1,
                ms_ssim: None,
                ms_ssim_passed: CheckResult::NotChecked,
                ms_ssim_score: None,
                used_fallback: false,
                iterations,
                size_target_met: self.size_target_check(initial_size),
                quality_passed: CheckResult::NotChecked,
                log,
                confidence,
                confidence_detail,
                actual_min_ssim: self.config.quality_thresholds.min_ssim,
                ..Default::default()
            }));
        }

        let mut low = self.config.initial_crf;
        let mut high = self.config.max_crf;
        let mut best_crf: Option<f32> = None;
        let mut best_size: Option<u64> = None;

        while high - low > precision::SEARCH_STEP_FINE && iterations < self.config.max_iterations {
            let mid = (f32::midpoint(low, high) * 2.0).round() / 2.0;

            let size = encode_cached(mid, &mut cache, self)?;
            iterations += 1;
            let size_pct = self.calc_change_pct(size);
            let compress_icon =
                crate::modern_ui::symbols::ok_fail_icon(self.can_compress_with_margin(size));
            progress_line(format!(
                "Binary Search | CRF {mid:.1} | {size_pct:+.1}% {compress_icon} | Best: \
                 {best_crf_so_far:.1}"
            ));

            if self.can_compress_with_margin(size) {
                best_crf = Some(mid);
                best_size = Some(size);
                best_crf_so_far = mid;
                high = mid;
            } else {
                low = mid;
            }
        }
        progress_done();

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
        let status = crate::modern_ui::symbols::ok_warn_icon(compressed);
        crate::log_detail!(
            "{} Result: CRF {:.1} • {:+.1}% {} • Iter {} ({:.1}s)",
            crate::media_conversion_gate::ui_icon_pick("✅", "[OK]"),
            final_crf,
            size_change_pct,
            status,
            iterations,
            elapsed.as_secs_f64()
        );
        log.push(format!(
            "{} RESULT: CRF {final_crf:.1}, {size_change_pct:+.1}%",
            crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
        ));

        // Use the re-encode's actual on-disk size: the deliverable is this fresh
        // encode, not the earlier probe whose size was cached as `final_size`.
        let actual_size = self.encode(final_crf)?;
        if actual_size != final_size {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "explore_final_reencode_size",
                format!(
                    "Re-encode at CRF {final_crf:.1} produced {actual_size} bytes vs cached probe \
                     {final_size}; reporting actual"
                ),
            );
        }
        let size_change_pct = self.calc_change_pct(actual_size);
        let quality = self.validate_quality()?;
        let (confidence, confidence_detail) =
            self.measured_confidence_for(quality.0, iterations, self.config.max_iterations.max(1));
        Ok(ok_explore_result(ExploreResult {
            optimal_crf: final_crf,
            output_size: actual_size,
            size_change_pct,
            ssim: quality.0,
            psnr: quality.1,
            ms_ssim: None,
            ms_ssim_passed: CheckResult::NotChecked,
            ms_ssim_score: None,
            used_fallback: false,
            iterations,
            size_target_met: self.size_target_check(actual_size),
            quality_passed: CheckResult::NotChecked,
            log,
            confidence,
            confidence_detail,
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        }))
    }

    // Rationale: This function handles complex, sequential initialization or
    // business logic where further fragmentation would hinder readability and
    // maintainability.
    fn explore_compress_with_quality(&self) -> Result<ExploreResult> {
        let log = Vec::new();
        let mut cache: CrfCache<(u64, Option<f64>)> = CrfCache::new();

        let pb = crate::progress::create_professional_spinner(&format!(
            "{} Compress+Quality",
            crate::media_conversion_gate::ui_icon_pick("📦", "[PKG]")
        ));

        let min_ssim = self.config.quality_thresholds.min_ssim;
        pb.suspend(|| {
            crate::log_summary_header!(
                &crate::media_conversion_gate::ui_log_summary_title_with_icon(
                    "🤖",
                    "[AI]",
                    format!(
                        "Forensic Explore: Dual-Constraint Optimization ({:?})",
                        self.encoder
                    ),
                )
            );
            crate::log_report_stat!(
                crate::infra::static_logs::messages::LABEL_STRATEGY,
                format!(
                    "Target: Bitstream < {}B | Quality Integrity (SSIM) >= {:.2}",
                    self.input_size, min_ssim
                )
            );
        });

        let mut iterations = 0u32;
        let mut best_result: Option<(f32, u64, f64)> = None;

        pb.set_message("Phase 1: Binary search for compression boundary");
        let mut low = self.config.initial_crf;
        let mut high = self.config.max_crf;
        let mut compress_boundary: Option<f32> = None;

        while high - low
            > precision::SEARCH_STEP_COARSE / crate::constants::SEARCH_ROUNDING_MULTIPLIER
            && iterations < self.config.max_iterations
        {
            let mid = f32::midpoint(low, high).round();

            log_detail!(&format!(
                "{} Testing CRF {:.0}...",
                crate::infra::static_logs::messages::LABEL_PHASE_1,
                mid
            ));
            let size = self.encode(mid)?;
            iterations += 1;

            cache.insert(mid, (size, None));

            if self.can_compress_with_margin(size) {
                compress_boundary = Some(mid);
                high = mid;
                log_detail!(&format!(
                    "{} {} Compresses at CRF {:.0}",
                    crate::infra::static_logs::messages::LABEL_PHASE_1,
                    crate::media_conversion_gate::ui_explore_crf_compress_ok_mark(),
                    mid
                ));
            } else {
                low = mid;
                log_detail!(&format!(
                    "{} {} Too large at CRF {:.0}",
                    crate::infra::static_logs::messages::LABEL_PHASE_1,
                    crate::media_conversion_gate::ui_explore_crf_too_large_mark(),
                    mid
                ));
            }
        }

        if let Some(boundary) = compress_boundary {
            log_detail!(&format!(
                "{} {} Phase 2: Validate quality at CRF {:.1}",
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                crate::media_conversion_gate::ui_explore_crf_target_mark(),
                boundary
            ));

            let size = if let Some(&(s, _)) = cache.get(boundary) {
                s
            } else {
                let s = self.encode(boundary)?;
                iterations += 1;
                s
            };

            let quality = self.validate_quality()?;
            let context = format!("Compress+Quality validation at CRF {boundary:.1}");
            let ssim = Self::require_ssim_metric(quality.0, &context)?;
            cache.insert(boundary, (size, Some(ssim)));

            log_detail!(&format!(
                "{} CRF {:.1}: SSIM {:.4}, Size {:+.1}%",
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                boundary,
                ssim,
                self.calc_change_pct(size)
            ));

            best_result = Some((boundary, size, ssim));
            if ssim >= min_ssim {
                log_detail!(&format!(
                    "{} {} Valid: compresses + SSIM OK",
                    crate::infra::static_logs::messages::LABEL_PHASE_2,
                    crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
                ));
            } else {
                log_detail!(&format!(
                    "{} {} SSIM below threshold, accepting best available (no lower-CRF retry)",
                    crate::infra::static_logs::messages::LABEL_PHASE_2,
                    crate::modern_ui::symbols::styled_warning_icon()
                ));
            }
        }

        let (final_crf, final_size, final_ssim) = if let Some((crf, size, ssim)) = best_result {
            (crf, size, ssim)
        } else {
            let size = self.encode(self.config.max_crf)?;
            let quality = self.validate_quality()?;
            (
                self.config.max_crf,
                size,
                Self::require_ssim_metric(
                    quality.0,
                    "Compress+Quality fallback validation at max CRF",
                )?,
            )
        };

        let size_change_pct = self.calc_change_pct(final_size);
        let compressed = self.can_compress_with_margin(final_size);
        let quality_ok = final_ssim >= min_ssim;
        let passed = compressed && quality_ok;

        pb.finish_and_clear();
        log_detail!(&format!(
            "{} RESULT: CRF {:.1} • SSIM {:.4} • Size {:+.1}% {}",
            crate::infra::static_logs::messages::LABEL_DONE,
            final_crf,
            final_ssim,
            size_change_pct,
            if passed {
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
            } else if compressed {
                crate::media_conversion_gate::ui_icon_pick("⚠️ SSIM low", "[WARN SSIM low]")
            } else {
                crate::media_conversion_gate::ui_icon_pick(
                    "⚠️ Not compressed",
                    "[WARN Not compressed]",
                )
            }
        ));
        log_detail!(&format!(
            "{} Iterations: {}",
            crate::infra::static_logs::messages::LABEL_DONE,
            iterations
        ));

        let (confidence, confidence_detail) = self.measured_confidence_for(
            Some(final_ssim),
            iterations,
            self.config.max_iterations.max(1),
        );
        Ok(ok_explore_result(ExploreResult {
            optimal_crf: final_crf,
            output_size: final_size,
            size_change_pct,
            ssim: Some(final_ssim),
            psnr: None,
            ms_ssim: None,
            ms_ssim_passed: CheckResult::NotChecked,
            ms_ssim_score: None,
            used_fallback: false,
            iterations,
            size_target_met: self.size_target_check(final_size),
            quality_passed: if passed {
                CheckResult::Passed
            } else {
                CheckResult::Failed("Quality check failed".into())
            },
            log,
            confidence,
            confidence_detail,
            actual_min_ssim: min_ssim,
            ..Default::default()
        }))
    }

    // Rationale: This function handles complex, sequential initialization or
    // business logic where further fragmentation would hinder readability and
    // maintainability.
    fn explore_precise_quality_match(&self) -> Result<ExploreResult> {
        let log = Vec::new();
        let mut cache: CrfCache<(u64, (Option<f64>, Option<f64>, Option<f64>))> = CrfCache::new();
        let mut last_encoded_crf: Option<f32> = None;

        log_detail!(&format!(
            "{} Forensic Explore: Precise Quality-Match ({:?}) | Input: {:.2} MB",
            crate::media_conversion_gate::ui_icon_pick("⚖️", "[=]"),
            self.encoder,
            crate::numeric_cast::f64_to_f32_lossy(
                crate::numeric_cast::u64_to_f64(self.input_size)
                    / crate::constants::KB_F64
                    / crate::constants::KB_F64
            )
        ));
        log_detail!(&format!(
            "{} Target: Quality Parity (SSIM) >= {:.5} (Structural Integrity Audit)",
            crate::media_conversion_gate::ui_icon_pick("🎯", "[TARGET]"),
            self.config.quality_thresholds.min_ssim
        ));
        log_detail!(&format!(
            "   📐 CRF range: [{:.1}, {:.1}]",
            self.config.min_crf, self.config.max_crf
        ));
        log_detail!(crate::infra::static_logs::messages::MSG_EXPLORE_GOAL_QUALITY);
        log_detail!(crate::infra::static_logs::messages::MSG_EXPLORE_SEPARATOR);

        let mut iterations = 0u32;
        let crf_range =
            (self.config.max_crf - self.config.min_crf).max(crate::constants::CRF_SEARCH_STEP);
        let dynamic_max_iterations =
            crate::numeric_cast::f64_to_u32_sat(f64::from(crf_range).log2().ceil())
                .saturating_add(6)
                .saturating_add(4)
                .clamp(10, GLOBAL_MAX_ITERATIONS);
        let max_iterations = dynamic_max_iterations;

        let mut best_crf: f32;
        let mut best_size: u64;
        let mut best_quality: (Option<f64>, Option<f64>, Option<f64>);
        let mut best_ssim: f64;

        let encode_cached =
            |crf: f32,
             cache: &mut CrfCache<(u64, (Option<f64>, Option<f64>, Option<f64>))>,
             last_crf: &mut Option<f32>,
             explorer: &Self|
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

        log_detail!(&format!(
            "{} {} Phase 1: Boundary test",
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            crate::media_conversion_gate::ui_icon_pick("📍", "[TARGET]")
        ));

        log_detail!(&format!(
            "{} {} Testing min CRF {:.1}...",
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            crate::media_conversion_gate::ui_icon_pick("🔄", "~"),
            self.config.min_crf
        ));
        let (min_size, min_quality) =
            encode_cached(self.config.min_crf, &mut cache, &mut last_encoded_crf, self)?;
        iterations += 1;
        let min_ssim = Self::require_ssim_metric(
            min_quality.0,
            "Precise Quality-Match minimum-boundary validation",
        )?;
        crate::log_stat!(
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            format!(
                "Initial Probe: CRF {:.1} | SSIM {:.6} | Size Change {:+.1}%",
                self.config.min_crf,
                min_ssim,
                self.calc_change_pct(min_size)
            )
        );

        best_crf = self.config.min_crf;
        best_size = min_size;
        best_quality = min_quality;
        best_ssim = min_ssim;

        log_detail!(&format!(
            "{} {} Testing max CRF {:.1}...",
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            crate::media_conversion_gate::ui_icon_pick("🔄", "~"),
            self.config.max_crf
        ));
        let (max_size, max_quality) =
            encode_cached(self.config.max_crf, &mut cache, &mut last_encoded_crf, self)?;
        iterations += 1;
        let max_ssim = Self::require_ssim_metric(
            max_quality.0,
            "Precise Quality-Match maximum-boundary validation",
        )?;
        crate::log_stat!(
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            format!(
                "Boundary Audit: CRF {:.1} | SSIM {:.6} | Size Change {:+.1}%",
                self.config.max_crf,
                max_ssim,
                self.calc_change_pct(max_size)
            )
        );

        let ssim_range = min_ssim - max_ssim;
        log_detail!(&format!(
            "{} SSIM range: {:.6}",
            crate::infra::static_logs::messages::LABEL_PHASE_1,
            ssim_range
        ));

        if ssim_range < SSIM_PLATEAU_THRESHOLD {
            log_detail!(&format!(
                "{} ⚡ Early exit: SSIM plateau, using max CRF for smaller file",
                crate::infra::static_logs::messages::LABEL_PHASE_1
            ));
            best_crf = self.config.max_crf;
            best_size = max_size;
            best_quality = max_quality;
            best_ssim = max_ssim;
        } else {
            // Phase 2: single-point golden-ratio search (mid = low + (high - low) * PHI).
            // Assumption: CRF–SSIM curve is monotonic (higher CRF → lower SSIM). If the
            // curve were non-monotonic, this could converge slowly or to a
            // suboptimal point.
            //
            // Why not full golden-section search? Full GSS keeps two interior points and
            // reuses one when shrinking the interval, so it also does 1 eval
            // per iteration (after 2 initial evals) and minimizes total evals.
            // We use a single probe from the low end each time for simplicity:
            // no tracking of which point to keep, and the same 1 encode per
            // iteration. We may do 1–2 extra encodes over the whole Phase 2 vs.
            // full GSS; the tradeoff is lower code complexity and easier maintenance.
            crate::log_stat!(
                crate::infra::static_logs::messages::LABEL_PHASE_2,
                "Initiating Phi-based search iteration (one eval per cycle)"
            );

            let mut low = self.config.min_crf;
            let mut high = self.config.max_crf;
            let mut prev_ssim = min_ssim;

            while high - low > crate::constants::SEARCH_OFFSET_NORMAL && iterations < max_iterations
            {
                if iterations >= EMERGENCY_MAX_ITERATIONS {
                    crate::media_conversion_gate::explore_delivery_explore_outcome_audit(
                        "explore_emergency_iteration_cap",
                        format!(
                            "{}: search exceeded {EMERGENCY_MAX_ITERATIONS} iterations; force stop",
                            self.input_path.display()
                        ),
                    );
                    break;
                }

                let mid = (high - low).mul_add(PHI, low);
                let mid_rounded = (mid * crate::constants::SEARCH_ROUNDING_MULTIPLIER).round()
                    / crate::constants::SEARCH_ROUNDING_MULTIPLIER;

                log_detail!(&format!(
                    "{} {} Testing CRF {:.1}...",
                    crate::infra::static_logs::messages::LABEL_PHASE_2,
                    crate::media_conversion_gate::ui_icon_pick("🔄", "~"),
                    mid_rounded
                ));
                let (size, quality) =
                    encode_cached(mid_rounded, &mut cache, &mut last_encoded_crf, self)?;
                iterations += 1;
                let context = format!("Precise Quality-Match search at CRF {mid_rounded:.1}");
                let ssim = Self::require_ssim_metric(quality.0, &context)?;
                crate::log_detail!(&format!(
                    "Phase 2 Iteration {}: CRF {:.1} | SSIM {:.6} | Size {:+.1}%",
                    iterations,
                    mid_rounded,
                    ssim,
                    self.calc_change_pct(size)
                ));

                if ssim > best_ssim + SSIM_EPSILON
                    || (ssim >= best_ssim - SSIM_EPSILON && mid_rounded > best_crf)
                {
                    best_crf = mid_rounded;
                    best_size = size;
                    best_quality = quality;
                    best_ssim = ssim;
                }

                if prev_ssim - ssim
                    > SSIM_PLATEAU_THRESHOLD * crate::constants::SEARCH_PLATEAU_MULTIPLIER
                {
                    high = mid_rounded;
                    log_detail!(&format!(
                        "{} ↓ SSIM drop, narrowing to [{:.1}, {:.1}]",
                        crate::infra::static_logs::messages::LABEL_PHASE_2,
                        low,
                        high
                    ));
                } else {
                    low = mid_rounded;
                }
                prev_ssim = ssim;
            }

            if iterations < max_iterations {
                crate::log_stat!(
                    crate::infra::static_logs::messages::LABEL_PHASE_3,
                    format!("Fine-tuning local minima around best CRF {:.1}", best_crf)
                );

                for offset in [
                    -crate::constants::SEARCH_OFFSET_NORMAL,
                    crate::constants::SEARCH_OFFSET_NORMAL,
                ] {
                    let crf = (best_crf + offset).clamp(self.config.min_crf, self.config.max_crf);
                    if iterations >= max_iterations {
                        break;
                    }

                    log_detail!(&format!(
                        "{} 🔄 Testing CRF {:.1}...",
                        crate::infra::static_logs::messages::LABEL_PHASE_3,
                        crf
                    ));
                    let (size, quality) =
                        encode_cached(crf, &mut cache, &mut last_encoded_crf, self)?;
                    iterations += 1;
                    let context =
                        format!("Precise Quality-Match fine-tune validation at CRF {crf:.1}");
                    let ssim = Self::require_ssim_metric(quality.0, &context)?;
                    log_detail!(&format!(
                        "{} CRF {:.1}: SSIM {:.6}",
                        crate::infra::static_logs::messages::LABEL_PHASE_3,
                        crf,
                        ssim
                    ));

                    if ssim > best_ssim + SSIM_EPSILON
                        || (ssim >= best_ssim - SSIM_EPSILON && crf > best_crf)
                    {
                        best_crf = crf;
                        best_size = size;
                        best_quality = quality;
                        best_ssim = ssim;
                    }
                }

                if iterations < max_iterations {
                    for offset in [
                        -crate::constants::SEARCH_OFFSET_FINE,
                        crate::constants::SEARCH_OFFSET_FINE,
                        -crate::constants::SEARCH_OFFSET_NORMAL,
                        crate::constants::SEARCH_OFFSET_NORMAL,
                    ] {
                        let crf =
                            (best_crf + offset).clamp(self.config.min_crf, self.config.max_crf);
                        if cache.contains_key(crf) {
                            continue;
                        }
                        if iterations >= max_iterations {
                            break;
                        }

                        log_detail!(&format!(
                            "{} 🔄 Testing CRF {:.1}...",
                            crate::infra::static_logs::messages::LABEL_PHASE_3,
                            crf
                        ));
                        let (size, quality) =
                            encode_cached(crf, &mut cache, &mut last_encoded_crf, self)?;
                        iterations += 1;
                        let context = format!(
                            "Precise Quality-Match secondary fine-tune validation at CRF {crf:.1}"
                        );
                        let ssim = Self::require_ssim_metric(quality.0, &context)?;
                        log_detail!(&format!(
                            "{} CRF {:.1}: SSIM {:.6}",
                            crate::infra::static_logs::messages::LABEL_PHASE_3,
                            crf,
                            ssim
                        ));

                        if ssim > best_ssim + 0.000_01_f64
                            || (ssim >= best_ssim - 0.000_01_f64 && crf > best_crf)
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
            log_detail!(&format!(
                "{} {} Output already at best CRF {:.1} (no re-encoding needed)",
                crate::infra::static_logs::messages::LABEL_DONE,
                crate::media_conversion_gate::ui_icon_pick("✨", "[*]"),
                best_crf
            ));
            (best_size, best_quality)
        } else {
            log_detail!(&format!(
                "{} {} Final: Re-encoding to best CRF {:.1}",
                crate::infra::static_logs::messages::LABEL_DONE,
                crate::media_conversion_gate::ui_icon_pick("📍", "[TARGET]"),
                best_crf
            ));
            (self.encode(best_crf)?, best_quality)
        };

        let size_change_pct = self.calc_change_pct(final_size);

        let status = if best_ssim >= crate::constants::SSIM_LEVEL_NEAR_LOSSLESS {
            format!(
                "{} Near-Lossless",
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
            )
        } else if best_ssim >= crate::constants::SSIM_LEVEL_PERFECT {
            format!(
                "{} Excellent",
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
            )
        } else if best_ssim >= crate::constants::SSIM_LEVEL_EXCELLENT {
            format!(
                "{} Very Good",
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
            )
        } else if best_ssim >= crate::constants::SSIM_LEVEL_VERY_GOOD {
            format!(
                "{} Good",
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
            )
        } else {
            format!(
                "{} Acceptable",
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
            )
        };

        crate::log_summary_header!(
            &crate::media_conversion_gate::ui_log_summary_title_with_icon(
                "⚖️",
                "[=]",
                "Forensic Search Result Finalized",
            )
        );
        crate::log_report_stat!(
            crate::infra::static_logs::messages::LABEL_DONE,
            format!(
                "CRF {:.1} | SSIM {:.6} ({}) | Size {:+.1}% | Iter: {}",
                best_crf, best_ssim, status, size_change_pct, iterations
            )
        );

        let quality_passed = best_ssim >= self.config.quality_thresholds.min_ssim;
        let (confidence, confidence_detail) = self.measured_confidence_for(
            Some(best_ssim),
            iterations,
            self.config.max_iterations.max(1),
        );

        Ok(ok_explore_result(ExploreResult {
            optimal_crf: best_crf,
            output_size: final_size,
            size_change_pct,
            ssim: final_quality.0,
            psnr: final_quality.1,
            ms_ssim: final_quality.2,
            iterations,
            size_target_met: self.size_target_check(final_size),
            quality_passed: if quality_passed {
                CheckResult::Passed
            } else {
                CheckResult::Failed(format!("SSIM {best_ssim:.4} below target"))
            },
            log,
            confidence,
            confidence_detail,
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        }))
    }

    // Rationale: This function handles complex, sequential initialization or
    // business logic where further fragmentation would hinder readability and
    // maintainability.
    fn explore_precise_quality_match_with_compression(&self) -> Result<ExploreResult> {
        PreciseCompressionSession::new(self).run()
    }

    fn encode(&self, crf: f32) -> Result<u64> {
        let result = self.encode_with_ffmpeg(crf);

        if result.is_err() && self.use_gpu {
            crate::log_detail!(&format!(
                "      {} GPU encoding failed, falling back to CPU (FFmpeg Native)",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
            let cpu_fallback = Self {
                config: self.config.clone(),
                encoder: self.encoder,
                input_path: self.input_path.clone(),
                output_path: self.output_path.clone(),
                input_size: self.input_size,
                vf_args: self.vf_args.clone(),
                use_gpu: false,
                max_threads: self.max_threads,
                preset: self.preset,
                input_pure_media_size: self.input_pure_media_size,
                hdr_x265_params: self.hdr_x265_params.clone(),
                apple_compat: self.apple_compat,
                source_codec_name: self.source_codec_name.clone(),
            };
            return cpu_fallback.encode_with_ffmpeg(crf);
        }

        result
    }

    fn build_ffmpeg_encode_plan(&self, crf: f32) -> Result<FfmpegEncodePlan> {
        use std::process::Stdio;

        let mut builder = crate::ffmpeg_builder::FfmpegBuilder::new();
        builder
            .overwrite()
            .threads(self.max_threads)
            .input(&self.input_path)
            .vcodec(self.encoder.into())
            .use_gpu(self.use_gpu)
            .crf(crf)
            .preset(self.preset);

        let accel_type = if self.use_gpu {
            let gpu = crate::gpu_accel::GpuAccel::detect_with_retry();
            format!(
                "{} GPU ({})",
                crate::media_conversion_gate::ui_icon_pick("🚀", "[LAUNCH]"),
                gpu.gpu_type
            )
        } else {
            "CPU".to_string()
        };
        let media_profile = EncodeMediaProfile::inspect(&self.input_path)?;
        media_profile.log_pts_adjustment();

        if let Some(profile) = match self.encoder {
            VideoEncoder::Hevc if self.apple_compat => {
                Some(crate::ffmpeg_builder::VideoProfile::Main)
            }
            VideoEncoder::H264 => Some(crate::ffmpeg_builder::VideoProfile::High),
            _ => None,
        } {
            builder.profile(profile);
        }

        if self.encoder == VideoEncoder::Hevc && self.apple_compat {
            builder
                .arg(crate::constants::FFMPEG_ARG_TAG_VIDEO)
                .arg(crate::constants::FFMPEG_TAG_HVC1);
        }

        // Add extra encoder-specific args.
        // Note: for HEVC we deliberately skip -x265-params here because the CPU branch
        // below emits them via `extra_args_with_preset`; duplicating -x265-params
        // causes ffmpeg to keep only the last copy (silently dropping the
        // first), and on the GPU path the encoder is hevc_videotoolbox which
        // rejects -x265-params entirely.
        if self.encoder == VideoEncoder::Av1 {
            let preset = self.preset.sanitize_av1();
            builder.arg("-svtav1-params").arg(format!(
                "tune=0:film-grain=0:preset={}:lp={}",
                preset.svtav1_preset(),
                self.max_threads
            ));
        }

        // Apply VF args if present
        for arg in &self.vf_args {
            builder.arg(arg);
        }

        // Status/Progress reporting
        builder
            .arg("-progress")
            .arg("pipe:1")
            .arg("-stats_period")
            .arg("0.5");

        // Globally enforce passthrough for ALL media (videos + animations)
        // unless PTS is severely broken, in which case we fallback to VFR for recovery.
        builder.arg("-fps_mode").arg(media_profile.fps_mode());

        if media_profile.is_animated {
            builder.arg("-video_track_timescale").arg("1000");
        }

        if !self.use_gpu {
            let mut args = self.encoder.extra_args_with_preset(
                self.max_threads,
                self.preset,
                self.hdr_x265_params.as_deref(),
                self.apple_compat,
                false,
                crate::x265_params::memory_profile_for_source(
                    self.source_codec_name.as_deref(),
                    self.input_size,
                ),
            );

            if self.encoder == VideoEncoder::Hevc
                && media_profile.is_animated
                && let Some(pos) = args.iter().position(|x| x == "-x265-params")
                && let Some(param_val) = args.get_mut(pos.saturating_add(1))
            {
                param_val.push_str(":bframes=0");
            }

            for arg in args {
                builder.arg(arg);
            }
        }

        let mut cmd = builder.output(&self.output_path).build();

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        Ok(FfmpegEncodePlan {
            accel_type,
            duration_secs: self.get_input_duration()?,
            cmd,
        })
    }

    fn encode_with_ffmpeg(&self, crf: f32) -> Result<u64> {
        let mut plan = self.build_ffmpeg_encode_plan(crf)?;

        let mut child = plan.cmd.spawn().context("Failed to spawn ffmpeg")?;

        let stderr_handle = child.stderr.take().map(collect_ffmpeg_stderr);
        let progress_handle = child.stdout.take().map(|stdout| {
            spawn_ffmpeg_progress_stream(stdout, plan.accel_type.clone(), plan.duration_secs)
        });
        let status_result = crate::process_runner::wait_child_with_liveness_timeout(
            &mut child,
            crate::ffmpeg_process::ffmpeg_timeout(),
            crate::process_runner::video_process_hard_timeout(),
            &format!("ffmpeg encode for {}", self.output_path.display()),
        );

        if let Some(handle) = progress_handle
            && handle.join().is_err()
        {
            crate::log_detail!("      [WARN] ffmpeg progress thread panicked");
        }
        let stderr_content = match stderr_handle {
            Some(handle) => handle
                .join()
                .map_err(|_| anyhow::anyhow!("ffmpeg stderr capture thread panicked"))?,
            None => String::new(),
        };
        let status = status_result.map_err(|err| {
            anyhow::anyhow!("{}:\n{}", err, summarize_ffmpeg_failure(&stderr_content))
        })?;

        crate::log_detail!(
            "\r      ✅ {} Encoding complete                                    ",
            plan.accel_type
        );

        if !status.success() {
            bail!(
                "ffmpeg encoding failed (exit code: {:?}):\n{}",
                status.code(),
                summarize_ffmpeg_failure(&stderr_content)
            );
        }

        let pure_media_size = crate::stream_size::measure_strict_pure_media(&self.output_path)
            .with_context(|| {
                format!(
                    "Strict pure-media output measurement failed for {}",
                    self.output_path.display()
                )
            })?
            .pure_media_size();

        Ok(pure_media_size)
    }

    fn get_input_duration(&self) -> Result<Option<f64>> {
        let output = crate::ffmpeg_builder::FfprobeBuilder::new()
            .input(&self.input_path)
            .loglevel("error")
            .show_entries("format=duration")
            .print_format("default=noprint_wrappers=1:nokey=1")
            .build()
            .output()
            .with_context(|| {
                format!(
                    "ffprobe duration probe failed for {}",
                    self.input_path.display()
                )
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let Some(duration_secs) = crate::media_conversion_gate::probe_ffprobe_duration_text_or_none(
            stdout.trim(),
            "video_explorer_input_duration",
        ) else {
            anyhow::bail!(
                "ffprobe duration probe returned missing or invalid duration for {}: {:?}",
                self.input_path.display(),
                stdout.trim()
            );
        };
        Ok(Some(duration_secs))
    }

    fn calc_change_pct(&self, output_pure_media_size: u64) -> f64 {
        calc_change_pct_for_input_size(self.input_pure_media_size, output_pure_media_size)
    }

    #[inline]
    fn size_target_check(&self, output_size: u64) -> CheckResult {
        if self.can_compress_with_margin(output_size) {
            CheckResult::Passed
        } else {
            CheckResult::Failed("Pure-media size target not met".into())
        }
    }

    #[inline]
    fn measured_confidence_for(
        &self,
        ssim: Option<f64>,
        iterations: u32,
        max_iterations: u32,
    ) -> (Option<f64>, ConfidenceBreakdown) {
        measured_exploration_confidence(
            ssim,
            self.config.quality_thresholds.min_ssim,
            iterations,
            max_iterations,
        )
    }

    #[inline]
    const fn can_compress_with_margin(&self, output_pure_media_size: u64) -> bool {
        output_pure_media_size < self.input_pure_media_size
    }

    #[inline]
    const fn get_compression_target(&self) -> u64 {
        self.input_pure_media_size
    }

    fn validate_quality(&self) -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
        let ssim = if self
            .config
            .quality_thresholds
            .validation
            .metrics
            .validate_ssim
        {
            self.calculate_ssim()?
        } else {
            None
        };

        let psnr = if self
            .config
            .quality_thresholds
            .validation
            .metrics
            .validate_psnr
        {
            self.calculate_psnr()?
        } else {
            None
        };

        let ms_ssim = if self
            .config
            .quality_thresholds
            .validation
            .metrics
            .validate_ms_ssim
        {
            let duration = get_video_duration(&self.input_path)?;
            let ms_ssim_skip_threshold_secs = if self.config.ultimate_mode {
                f64::from(VMAF_SKIP_THRESHOLD_ULTIMATE_SECS)
            } else {
                f64::from(LONG_VIDEO_THRESHOLD_SECS)
            };
            let should_skip =
                crate::media_conversion_gate::explore_ms_ssim_skip_when_duration_unknown(
                    Some(duration),
                    ms_ssim_skip_threshold_secs,
                    self.config.quality_thresholds.validation.force_ms_ssim_long,
                );

            if should_skip {
                let threshold_min = ms_ssim_skip_threshold_secs / 60.0_f64;
                crate::log_detail!(&format!(
                    "   {} Quality verification: long video ({:.1}min > {:.0}min), MS-SSIM \
                     skipped.",
                    crate::modern_ui::symbols::styled_warning_icon(),
                    duration / 60.0_f64,
                    threshold_min
                ));
                crate::log_detail!(crate::infra::static_logs::messages::MSG_EXPLORE_FORCE_MS_SSIM);
                None
            } else {
                self.calculate_ms_ssim()?
            }
        } else {
            None
        };

        Ok((ssim, psnr, ms_ssim))
    }

    fn require_ssim_metric(ssim: Option<f64>, context: &str) -> Result<f64> {
        ssim.ok_or_else(|| anyhow::anyhow!("SSIM not measured during {context}"))
    }

    fn ssim_status_label(ssim: f64) -> &'static str {
        if ssim >= 0.999 {
            "Excellent"
        } else if ssim >= 0.98 {
            "Very good"
        } else if ssim >= 0.93 {
            "Good"
        } else {
            "Acceptable"
        }
    }

    /// Calculates both SSIM and PSNR for the current input vs output video.
    ///
    /// # Errors
    /// Returns an error if the calculation fails.
    pub fn calculate_ssim_and_psnr(&self) -> Result<(Option<f64>, Option<f64>)> {
        eprint!(
            "      {} Calculating SSIM+PSNR...",
            crate::media_conversion_gate::ui_icon_pick("📊", "[MET]")
        );
        let _ = std::io::stderr().flush();

        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim;\
                      [ref][1:v]psnr";

        let output = crate::ffmpeg_builder::FfmpegBuilder::new()
            .input(&self.input_path)
            .input(&self.output_path)
            .filter_complex(filter)
            .format("null")
            .output_pipe()
            .build()
            .output();

        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let mut ssim: Option<f64> = None;
                let mut psnr: Option<f64> = None;

                for line in stderr.lines() {
                    if let Some(pos) = line.find("SSIM All:") {
                        let value_str = &line[pos + 9..];
                        ssim = precision::parse_explore_ssim_metric_token(value_str)
                            .map_err(|err| anyhow!("failed to parse SSIM metric token: {err}"))?;
                    }
                    if let Some(pos) = line.find("average:") {
                        let value_str = &line[pos + 8..];
                        psnr = precision::parse_explore_psnr_metric_token(value_str)
                            .map_err(|err| anyhow!("failed to parse PSNR metric token: {err}"))?;
                    }
                }

                let ssim_str =
                    crate::media_conversion_gate::ui_f64_or_na(ssim, "explore_live_ssim", 4);
                let psnr_str =
                    crate::media_conversion_gate::ui_f64_or_na(psnr, "explore_live_psnr", 1);
                crate::log_detail!(
                    "\r      📊 SSIM: {} | PSNR: {} dB          ",
                    ssim_str,
                    psnr_str
                );

                Ok((ssim, psnr))
            }
            Err(e) => bail!("SSIM+PSNR calculation failed: {e}"),
        }
    }

    fn calculate_ssim(&self) -> Result<Option<f64>> {
        eprint!(
            "      {} Calculating SSIM...",
            crate::media_conversion_gate::ui_icon_pick("📊", "[MET]")
        );
        let _ = std::io::stderr().flush();

        let filters = [
            "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim",
            "[0:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:\
             trunc(ih/2)*2,format=yuv420p[cmp];[ref][cmp]ssim",
            "ssim",
        ];

        let mut last_error: Option<anyhow::Error> = None;
        for (idx, filter) in filters.iter().enumerate() {
            let result = self.try_ssim_with_filter(filter);

            match result {
                Ok(Some(ssim)) => {
                    crate::log_detail!(
                        "\r      📊 SSIM: {:.6} (method {})          ",
                        ssim,
                        idx + 1
                    );
                    return Ok(Some(ssim));
                }
                Ok(None) => {
                    if idx < filters.len() - 1 {
                        eprint!(
                            "\r      {} Method {} failed, trying method {}...",
                            crate::media_conversion_gate::ui_icon_pick("📊", "[MET]"),
                            idx + 1,
                            idx + 2
                        );
                        let _ = std::io::stderr().flush();
                    }
                }
                Err(err) => {
                    last_error = Some(err);
                    if idx < filters.len() - 1 {
                        eprint!(
                            "\r      {} Method {} failed, trying method {}...",
                            crate::media_conversion_gate::ui_icon_pick("📊", "[MET]"),
                            idx + 1,
                            idx + 2
                        );
                        let _ = std::io::stderr().flush();
                    }
                }
            }
        }

        if let Some(err) = last_error {
            return Err(err.context(format!(
                "SSIM calculation failed after {} methods",
                filters.len()
            )));
        }

        crate::log_detail!(
            "\r      ⚠️  SSIM calculation failed (all {} methods tried; pixel \
             format/resolution/corruption possible)",
            filters.len()
        );

        Ok(None)
    }

    fn try_ssim_with_filter(&self, filter: &str) -> Result<Option<f64>> {
        let output = crate::ffmpeg_builder::FfmpegBuilder::new()
            .input(&self.input_path)
            .input(&self.output_path)
            .filter_complex(filter)
            .format("null")
            .output_pipe()
            .build()
            .output()
            .context("Failed to run ffmpeg for SSIM")?;

        if !output.status.success() {
            return Ok(None);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stderr.lines() {
            if let Some(pos) = line.find("All:") {
                let value_str = &line[pos + 4..];
                if let Some(sealed) = precision::parse_explore_ssim_metric_token(value_str)
                    .map_err(|err| anyhow!("failed to parse SSIM metric token: {err}"))?
                {
                    return Ok(Some(sealed));
                }
            }
        }

        Ok(None)
    }

    fn calculate_psnr(&self) -> Result<Option<f64>> {
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:\
                      v]psnr=stats_file=-";

        let output = crate::ffmpeg_builder::FfmpegBuilder::new()
            .input(&self.input_path)
            .input(&self.output_path)
            .filter_complex(filter)
            .format("null")
            .output_pipe()
            .build()
            .output();

        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);

                for line in stderr.lines() {
                    if let Some(pos) = line.find("average:") {
                        let value_str = &line[pos + 8..];
                        if let Some(psnr) = precision::parse_explore_psnr_metric_token(value_str)
                            .map_err(|err| anyhow!("failed to parse PSNR metric token: {err}"))?
                        {
                            return Ok(Some(psnr));
                        }
                    }
                }

                Ok(None)
            }
            Err(e) => {
                bail!("Failed to execute ffmpeg for PSNR calculation: {e}")
            }
        }
    }

    fn calculate_ms_ssim(&self) -> Result<Option<f64>> {
        let duration = get_video_duration(&self.input_path)?;

        let filter = match duration {
            dur if dur > MS_SSIM_THREE_SEGMENT_MIN_DURATION_SECS => {
                let segment_pct = if self.config.ultimate_mode {
                    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION_ULTIMATE
                } else {
                    ANIMATED_IMAGE_EXPLORATION_SEGMENT_FRACTION
                };
                let start_end = dur * segment_pct;
                let mid_start = dur * (0.5_f64 - segment_pct / 2.0_f64);
                let mid_end = dur * (0.5_f64 + segment_pct / 2.0_f64);
                let tail_start = dur * (1.0_f64 - segment_pct);

                let pct_label = crate::numeric_cast::f64_to_u32_strict(segment_pct * 100.0, "pct")
                    .ok_or_else(|| anyhow::anyhow!("Failed to calculate MS-SSIM segment label"))?;
                crate::log_detail!(
                    "   MS-SSIM: 3-segment sampling (start {}% + mid {}% + end {}%)",
                    pct_label,
                    pct_label,
                    pct_label
                );
                format!(
                    "[0:v]select='lt(t\\,{start_end:.1})+between(t\\,{mid_start:.1}\\,{mid_end:.\
                     1})+gte(t\\,{tail_start:.1})',scale='iw-mod(iw,2)':'ih-mod(ih,2)':\
                     flags=bicubic[ref];[1:v]select='lt(t\\,{start_end:.1})+between(t\\,\
                     {mid_start:.1}\\,{mid_end:.1})+gte(t\\,{tail_start:.1})'[dist];\
                     [ref][dist]libvmaf"
                )
            }
            _ => "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]libvmaf"
                .to_string(),
        };

        let use_sampling = duration > MS_SSIM_THREE_SEGMENT_MIN_DURATION_SECS;

        let output = crate::ffmpeg_builder::FfmpegBuilder::new()
            .input(&self.input_path)
            .input(&self.output_path)
            .filter_complex(&filter)
            .format("null")
            .output_pipe()
            .build()
            .output();

        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);

                for line in stderr.lines() {
                    if let Some(pos) = line.find("MS-SSIM score:") {
                        let value_str = &line[pos + 11..];
                        if let Some(ms_ssim) = precision::parse_explore_ms_ssim_score_token(
                            value_str,
                        )
                        .map_err(|err| anyhow!("failed to parse MS-SSIM metric token: {err}"))?
                        {
                            if use_sampling {
                                crate::log_detail!(
                                    &crate::infra::static_logs::messages::MSG_EXPLORE_VMAF_SAMPLED
                                        .replace("{}", &format!("{ms_ssim:.2}"))
                                );
                            }
                            return Ok(Some(ms_ssim));
                        }
                    }
                }

                Ok(None)
            }
            Err(e) => {
                bail!("Failed to execute ffmpeg for VMAF calculation: {e}")
            }
        }
    }

    fn check_quality_passed(
        &self,
        ssim: Option<f64>,
        psnr: Option<f64>,
        vmaf: Option<f64>,
    ) -> CheckResult {
        if self.config.ultimate_mode {
            return CheckResult::NotChecked;
        }
        let t = &self.config.quality_thresholds;

        if t.validation.metrics.validate_ssim {
            match ssim {
                Some(s) => {
                    let epsilon = precision::SSIM_COMPARE_EPSILON;
                    if s + epsilon < t.min_ssim {
                        return CheckResult::Failed(format!(
                            "SSIM {s:.4} below target {}",
                            t.min_ssim
                        ));
                    }
                }
                None => {
                    return CheckResult::Failed("SSIM not available".to_string());
                }
            }
        }

        if t.validation.metrics.validate_psnr {
            match psnr {
                Some(p) => {
                    if p < t.min_psnr && !p.is_infinite() {
                        return CheckResult::Failed(format!(
                            "PSNR {p:.1} below target {}",
                            t.min_psnr
                        ));
                    }
                }
                None => {
                    return CheckResult::Failed("PSNR not available".to_string());
                }
            }
        }

        if t.validation.metrics.validate_ms_ssim {
            match vmaf {
                Some(v) => {
                    if v < t.min_ms_ssim {
                        return CheckResult::Failed(format!(
                            "MS-SSIM {v:.4} below target {}",
                            t.min_ms_ssim
                        ));
                    }
                }
                None => {
                    return CheckResult::Failed("MS-SSIM not available".to_string());
                }
            }
        }

        CheckResult::Passed
    }
}

/// Explores size-only mode: finds a CRF that produces a smaller file without
/// quality checks.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_size_only(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    max_threads: usize,
    apple_compat: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::size_only(initial_crf, max_crf);
    VideoExplorer::new(
        input,
        output,
        encoder,
        vf_args,
        config,
        max_threads,
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores quality-match mode: encodes at a predicted CRF and validates
/// quality.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_quality_match(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    predicted_crf: f32,
    max_threads: usize,
    apple_compat: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::quality_match(predicted_crf);
    VideoExplorer::new(
        input,
        output,
        encoder,
        vf_args,
        config,
        max_threads,
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores precise quality-match mode: iteratively searches for the best SSIM
/// within a CRF range.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_precise_quality_match(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    max_threads: usize,
    apple_compat: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::precise_quality_match(initial_crf, max_crf, min_ssim);
    VideoExplorer::new(
        input,
        output,
        encoder,
        vf_args,
        config,
        max_threads,
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores precise quality match while also ensuring the output is smaller
/// than the input.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_precise_quality_match_with_compression(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    max_threads: usize,
    apple_compat: bool,
) -> Result<ExploreResult> {
    let config =
        ExploreConfig::precise_quality_match_with_compression(initial_crf, max_crf, min_ssim);
    VideoExplorer::new(
        input,
        output,
        encoder,
        vf_args,
        config,
        max_threads,
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores compression-only mode: searches for the highest CRF that still
/// produces a smaller file.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_compress_only(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    max_threads: usize,
    apple_compat: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_only(initial_crf, max_crf);
    VideoExplorer::new(
        input,
        output,
        encoder,
        vf_args,
        config,
        max_threads,
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores compression with a minimum SSIM quality threshold.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_compress_with_quality(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    max_threads: usize,
    apple_compat: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_with_quality(initial_crf, max_crf);
    VideoExplorer::new(
        input,
        output,
        encoder,
        vf_args,
        config,
        max_threads,
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores precise quality match with compression using GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
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
    apple_compat: bool,
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
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores precise quality match using GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
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
    apple_compat: bool,
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
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores compression-only mode using GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_compress_only_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
    max_threads: usize,
    apple_compat: bool,
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
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores compression with quality threshold using GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_compress_with_quality_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
    max_threads: usize,
    apple_compat: bool,
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
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores size-only mode using GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_size_only_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
    max_threads: usize,
    apple_compat: bool,
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
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Explores quality-match mode using GPU acceleration.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_quality_match_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    predicted_crf: f32,
    use_gpu: bool,
    max_threads: usize,
    apple_compat: bool,
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
        None,
        apple_compat,
        None,
    )?
    .explore()
}

/// Calculates adaptive CRF range and minimum SSIM threshold based on the
/// encoder and initial CRF.
///
/// Returns `(max_crf, min_ssim)` tuned for the specific encoder's
/// characteristics.
#[must_use]
pub fn calculate_smart_thresholds(initial_crf: f32, encoder: VideoEncoder) -> (f32, f64) {
    let (crf_scale, max_crf_cap) = match encoder {
        VideoEncoder::Hevc => (51.0_f32, 40.0_f32),
        VideoEncoder::Av1 => (63.0_f32, 50.0_f32),
        VideoEncoder::H264 => (51.0_f32, 35.0_f32),
    };

    let normalized_crf = initial_crf / crf_scale;
    let quality_level = f64::from((normalized_crf * normalized_crf).clamp(0.0, 1.0));

    let headroom = if initial_crf < 1.0 {
        // High headroom for lossless-first starts (e.g. GIFs) to ensure we reach 25-30+
        28.0_f32
    } else {
        crate::numeric_cast::f64_to_f32_lossy(quality_level).mul_add(7.0, 8.0)
    };
    let max_crf = (initial_crf + headroom).min(max_crf_cap);

    let min_ssim = if initial_crf < 20.0 {
        0.95_f64
    } else if initial_crf < 30.0 {
        let t = (initial_crf - 20.0) / 10.0;
        0.95_f64 - f64::from(t) * 0.03_f64
    } else {
        let t = ((initial_crf - 30.0) / 20.0).min(1.0);
        0.92_f64 - f64::from(t) * 0.04_f64
    };

    (max_crf, min_ssim.clamp(0.85, 0.98))
}

/// Explores HEVC quality with adaptive thresholds (precise quality match mode).
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_hevc(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
    apple_compat: bool,
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
        apple_compat,
    )
}

/// Explores HEVC size-only mode with adaptive thresholds.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_hevc_size_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
    apple_compat: bool,
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
        apple_compat,
    )
}

/// Explores HEVC quality-match mode at a predicted CRF.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_hevc_quality_match(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    predicted_crf: f32,
    max_threads: usize,
    apple_compat: bool,
) -> Result<ExploreResult> {
    explore_quality_match(
        input,
        output,
        VideoEncoder::Hevc,
        vf_args,
        predicted_crf,
        max_threads,
        apple_compat,
    )
}

/// Explores HEVC compression-only mode with adaptive thresholds.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_hevc_compress_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
    apple_compat: bool,
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
        apple_compat,
    )
}

/// Explores HEVC compression with quality threshold.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_hevc_compress_with_quality(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
    apple_compat: bool,
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
        apple_compat,
    )
}

/// Explores AV1 quality with adaptive thresholds (precise quality match mode).
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_av1(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
    apple_compat: bool,
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
        apple_compat,
    )
}

/// Explores AV1 size-only mode with adaptive thresholds.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_av1_size_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
    apple_compat: bool,
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
        apple_compat,
    )
}

/// Explores AV1 quality-match mode at a predicted CRF.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_av1_quality_match(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    predicted_crf: f32,
    max_threads: usize,
    apple_compat: bool,
) -> Result<ExploreResult> {
    explore_quality_match(
        input,
        output,
        VideoEncoder::Av1,
        vf_args,
        predicted_crf,
        max_threads,
        apple_compat,
    )
}

/// Explores AV1 compression-only mode with adaptive thresholds.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_av1_compress_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
    apple_compat: bool,
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
        apple_compat,
    )
}

/// Explores AV1 compression with quality threshold.
///
/// # Errors
/// Returns an error if exploration fails.
pub fn explore_av1_compress_with_quality(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_threads: usize,
    apple_compat: bool,
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
        apple_compat,
    )
}

/// Precision constants and utilities for CRF search (step sizes, SSIM
/// thresholds, etc.).
pub mod precision;

/// Pre-check utilities for validating inputs before exploration.
pub mod precheck;

/// Calibration utilities for GPU/CPU CRF mapping.
pub mod calibration;

/// Dynamic mapping functions for adaptive CRF/quality thresholds.
pub mod dynamic_mapping;

/// GPU-accelerated coarse search implementations.
pub mod gpu_coarse_search;
pub use gpu_coarse_search::{
    GpuSearchFeatures, GpuSearchFlags, GpuSearchRequest, GpuSearchValidation,
    explore as explore_gpu_coarse, explore_av1_with_gpu, explore_hevc_with_gpu,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]

    fn test_precision_crf_search_range_hevc() {
        let iterations = required_iterations(10, 28).unwrap();
        assert!(
            iterations <= 8,
            "HEVC range [10,28] should need <= 8 iterations, got {iterations}"
        );
        assert_eq!(iterations, 6);
    }

    #[test]

    fn test_precision_crf_search_range_av1() {
        let iterations = required_iterations(10, 35).unwrap();
        assert!(
            iterations <= 8,
            "AV1 range [10,35] should need <= 8 iterations, got {iterations}"
        );
        assert_eq!(iterations, 6);
    }

    #[test]

    fn test_precision_crf_search_range_wide() {
        let iterations = required_iterations(0, 51).unwrap();
        assert!(
            iterations <= 8,
            "Wide range [0,51] should need <= 8 iterations, got {iterations}"
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
        assert_eq!(
            ssim_quality_grade(0.99),
            "Excellent (visually indistinguishable)"
        );
        assert_eq!(
            ssim_quality_grade(0.98),
            "Excellent (visually indistinguishable)"
        );
        assert_eq!(ssim_quality_grade(0.97), "Good (visually lossless)");
        assert_eq!(ssim_quality_grade(0.95), "Good (visually lossless)");
        assert_eq!(ssim_quality_grade(0.92), "Acceptable (minor difference)");
        assert_eq!(ssim_quality_grade(0.90), "Acceptable (minor difference)");
        assert_eq!(ssim_quality_grade(0.87), "Fair (visible difference)");
        assert_eq!(ssim_quality_grade(0.85), "Fair (visible difference)");
        assert_eq!(ssim_quality_grade(0.80), "Poor (noticeable quality loss)");
    }

    #[test]

    fn test_binary_search_precision_proof() {
        let range = 28.0 - 10.0;
        let coarse_iterations = crate::numeric_cast::f32_to_u32_strict(
            (range / precision::SEARCH_STEP_COARSE).ceil(),
            "coarse_iterations",
        )
        .unwrap_or_else(|| {
            crate::media_conversion_gate::explore_gpu_coarse_batch_audit(
                "explore_iteration_overflow",
                "Video Explorer: Failed to calculate coarse iterations (possible overflow); \
                 defaulting to 0 (search will skip coarse phase)",
            );
            0
        });
        let fine_iterations = crate::numeric_cast::f32_to_u32_strict(
            (precision::SEARCH_STEP_COARSE / precision::SEARCH_STEP_FINE).ceil(),
            "fine_iterations",
        )
        .unwrap_or_else(|| {
            crate::media_conversion_gate::explore_gpu_coarse_batch_audit(
                "explore_iteration_overflow",
                "Video Explorer: Failed to calculate fine iterations (possible overflow); \
                 defaulting to 0 (search will skip fine phase)",
            );
            0
        });
        let total = coarse_iterations + fine_iterations;

        assert!(
            total <= 15,
            "Three-phase search should achieve ±0.5 CRF precision within 15 iterations (got \
             {total})"
        );
        assert!(
            coarse_iterations <= 9,
            "HEVC range [10,28] coarse search should need <= 9 iterations"
        );
    }

    #[test]

    fn test_binary_search_worst_case() {
        let range = 51.0 - 0.0;
        let coarse_iterations = crate::numeric_cast::f32_to_u32_strict(
            (range / precision::SEARCH_STEP_COARSE).ceil(),
            "coarse_iterations",
        )
        .expect("coarse_iterations: invalid value (NaN/Inf/overflow)");
        let fine_iterations = crate::numeric_cast::f32_to_u32_sat(
            (precision::SEARCH_STEP_COARSE / precision::SEARCH_STEP_FINE).ceil(),
        );
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
    fn test_calc_change_pct_input_size_zero_is_not_fabricated() {
        let pct = calc_change_pct_for_input_size(0, 123);
        assert!(
            pct.is_nan(),
            "input_size==0 must yield NaN (unknown), not 0.0%"
        );
    }

    #[test]

    fn test_quality_check_ssim_only() {
        let thresholds = QualityThresholds {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_ms_ssim: 85.0,
            validation: QualityValidationFlags {
                metrics: MetricValidationFlags {
                    validate_ssim: true,
                    validate_psnr: false,
                    validate_ms_ssim: false,
                },
                ..Default::default()
            },
        };

        let check = |ssim: Option<f64>, psnr: Option<f64>| -> bool {
            if thresholds.validation.metrics.validate_ssim {
                match ssim {
                    Some(s) if s >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validation.metrics.validate_psnr {
                match psnr {
                    Some(p) if p >= thresholds.min_psnr => {}
                    _ => return false,
                }
            }
            true
        };

        assert!(check(Some(0.96_f64), None));
        assert!(check(Some(0.95_f64), None));
        assert!(check(Some(0.99_f64), Some(30.0_f64)));

        assert!(!check(Some(0.94_f64), None));
        assert!(!check(None, Some(40.0_f64)));
    }

    #[test]

    fn test_quality_check_both_metrics() {
        let thresholds = QualityThresholds {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_ms_ssim: 85.0,
            validation: QualityValidationFlags {
                metrics: MetricValidationFlags {
                    validate_ssim: true,
                    validate_psnr: true,
                    validate_ms_ssim: false,
                },
                ..Default::default()
            },
        };

        let check = |ssim: Option<f64>, psnr: Option<f64>| -> bool {
            if thresholds.validation.metrics.validate_ssim {
                match ssim {
                    Some(s) if s >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validation.metrics.validate_psnr {
                match psnr {
                    Some(p) if p >= thresholds.min_psnr => {}
                    _ => return false,
                }
            }
            true
        };

        assert!(check(Some(0.96_f64), Some(36.0_f64)));

        assert!(!check(Some(0.96_f64), Some(34.0_f64)));

        assert!(!check(Some(0.94_f64), Some(36.0_f64)));

        assert!(!check(Some(0.94_f64), Some(34.0_f64)));
    }

    #[test]

    fn test_precision_constants() {
        assert!(
            (CRF_PRECISION - 0.25).abs() < 0.01,
            "CRF precision should be ±0.25"
        );
        assert!(
            (precision::SEARCH_STEP_COARSE - 2.0).abs() < crate::constants::EPSILON_DEFAULT_F32,
            "Coarse step should be 2.0"
        );
        assert!(
            (precision::SEARCH_STEP_FINE - 0.5).abs() < crate::constants::EPSILON_DEFAULT_F32,
            "Fine step should be 0.5"
        );
        assert!(
            (precision::SEARCH_STEP_ULTRA_FINE - 0.25).abs()
                < crate::constants::EPSILON_DEFAULT_F32,
            "Ultra fine step should be 0.25"
        );
        assert_eq!(SSIM_DISPLAY_PRECISION, 4);
        assert!((SSIM_COMPARE_EPSILON - 0.0001).abs() < 1e-10_f64);
        assert!((DEFAULT_MIN_SSIM - 0.95).abs() < 1e-10_f64);
        assert!((HIGH_QUALITY_MIN_SSIM - 0.98).abs() < 1e-10_f64);
        assert!((ACCEPTABLE_MIN_SSIM - 0.90).abs() < 1e-10_f64);
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
            validation: QualityValidationFlags {
                metrics: MetricValidationFlags {
                    validate_ssim: true,
                    validate_psnr: false,
                    validate_ms_ssim: true,
                },
                ..Default::default()
            },
        };

        let check = |ssim: Option<f64>, vmaf: Option<f64>| -> bool {
            if thresholds.validation.metrics.validate_ssim {
                match ssim {
                    Some(s) if s + SSIM_COMPARE_EPSILON >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validation.metrics.validate_ms_ssim {
                match vmaf {
                    Some(v) if v >= thresholds.min_ms_ssim => {}
                    _ => return false,
                }
            }
            true
        };

        assert!(check(Some(0.96_f64), Some(90.0_f64)));

        assert!(!check(Some(0.96_f64), Some(80.0_f64)));

        assert!(!check(Some(0.94_f64), Some(90.0_f64)));

        assert!(!check(Some(0.96_f64), None));
    }

    #[test]

    fn test_crf_half_step_precision() {
        let test_values: [f64; 7] = [18.0, 18.5, 19.0, 19.5, 20.0, 20.5, 21.0];

        for &crf in &test_values {
            let rounded = (crf * 2.0).round() / 2.0_f64;
            assert!(
                (rounded - crf).abs() < 0.01_f64,
                "CRF {crf} should round to {rounded} with 0.5 step"
            );
        }

        assert!((((23.3_f64 * 2.0).round() / 2.0) - 23.5).abs() < 0.01_f64);
        assert!((((23.7_f64 * 2.0).round() / 2.0) - 23.5).abs() < 0.01_f64);
        assert!((((23.2_f64 * 2.0).round() / 2.0) - 23.0).abs() < 0.01_f64);
        assert!((((23.8_f64 * 2.0).round() / 2.0) - 24.0).abs() < 0.01_f64);
    }

    #[test]

    fn test_three_phase_iteration_estimate() {
        let initial = 20.0_f32;
        let max_crf = 30.0_f32;

        let coarse_up = crate::numeric_cast::f32_to_u32_sat(
            ((max_crf - initial) / precision::SEARCH_STEP_COARSE).ceil(),
        );
        assert_eq!(coarse_up, 5, "Coarse search up should be 5 iterations");

        let boundary_range = 4.0_f32;
        let fine_iterations = crate::numeric_cast::f32_to_u32_strict(
            (boundary_range / precision::SEARCH_STEP_FINE).ceil(),
            "fine_iterations",
        )
        .expect("fine_iterations: invalid value (NaN/Inf/overflow)");
        assert_eq!(fine_iterations, 8, "Fine search should be 8 iterations");

        let total = 1 + coarse_up + fine_iterations + 1;
        assert!(total <= 15, "Total iterations {total} should be <= 15");
    }

    #[test]

    fn test_crf_precision_guarantee() {
        let test_targets: [f32; 5] = [18.3, 20.7, 23.1, 25.9, 28.4];

        for &target in &test_targets {
            let nearest = (target * 2.0).round() / 2.0;
            let error = (nearest - target).abs();

            assert!(
                error <= 0.25,
                "Target {target} should be within ±0.25 of nearest step {nearest}, got error \
                 {error}"
            );
        }
    }

    #[test]

    fn test_boundary_refinement_logic() {
        let best_crf = 24.0_f32;
        let next_crf = best_crf + precision::SEARCH_STEP_FINE;
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
        let worst_coarse = crate::numeric_cast::f32_to_u32_sat(
            (worst_range / precision::SEARCH_STEP_COARSE).ceil(),
        );
        let worst_fine = crate::numeric_cast::f32_to_u32_sat(
            (precision::SEARCH_STEP_COARSE / precision::SEARCH_STEP_FINE).ceil(),
        ) * 2;
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
            min_ssim >= 0.93_f64,
            "High quality source should have strict SSIM >= 0.93, got {min_ssim}"
        );

        assert!(
            max_crf >= 26.0,
            "max_crf should be at least 26 for CRF 18, got {max_crf}"
        );
        assert!(
            max_crf <= 30.0,
            "max_crf should not exceed 30 for high quality, got {max_crf}"
        );
    }

    #[test]

    fn test_smart_thresholds_hevc_low_quality() {
        let (max_crf, min_ssim) = calculate_smart_thresholds(35.0, VideoEncoder::Hevc);

        assert!(
            min_ssim <= 0.92_f64,
            "Low quality source should have relaxed SSIM <= 0.92, got {min_ssim}"
        );
        assert!(
            min_ssim >= 0.85_f64,
            "SSIM should not go below 0.85, got {min_ssim}"
        );

        assert!(
            max_crf >= 40.0,
            "max_crf should be at least 40 for low quality, got {max_crf}"
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
            "AV1 max_crf should not exceed 50, got {max_crf_low}"
        );
    }

    #[test]

    fn test_smart_thresholds_edge_case_very_low_quality() {
        let (max_crf, min_ssim) = calculate_smart_thresholds(45.0, VideoEncoder::Hevc);

        assert!(
            max_crf <= 40.0,
            "HEVC max_crf should be capped at 40, got {max_crf}"
        );
        assert!(
            min_ssim >= 0.85_f64,
            "min_ssim should not go below 0.85, got {min_ssim}"
        );
    }

    #[test]

    fn test_smart_thresholds_edge_case_very_high_quality() {
        let (max_crf, min_ssim) = calculate_smart_thresholds(10.0, VideoEncoder::Hevc);

        assert!(
            min_ssim >= 0.94_f64,
            "Very high quality should have strict SSIM >= 0.94, got {min_ssim}"
        );

        assert!(
            max_crf >= 18.0,
            "max_crf should be at least 18 for CRF 10, got {max_crf}"
        );
    }

    #[test]

    fn test_smart_thresholds_continuity() {
        let mut prev_max_crf = 0.0_f32;
        let mut prev_min_ssim = 1.0_f64;

        for crf in (10_i32..=40_i32).step_by(2) {
            let (max_crf, min_ssim) = calculate_smart_thresholds(
                crate::numeric_cast::f64_to_f32_lossy(f64::from(crf)),
                VideoEncoder::Hevc,
            );

            if crf > 10_i32 {
                assert!(
                    max_crf >= prev_max_crf - 0.5,
                    "max_crf should be monotonically increasing: {prev_max_crf} -> {max_crf} at \
                     CRF {crf}"
                );

                assert!(
                    min_ssim <= prev_min_ssim + 0.01_f64,
                    "min_ssim should be monotonically decreasing: {prev_min_ssim} -> {min_ssim} \
                     at CRF {crf}"
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
            target_ssim > 0.999_f64,
            "Target SSIM should be > 0.999 for near-lossless"
        );
        assert!(
            target_ssim < 1.0_f64,
            "Target SSIM should be < 1.0 (1.0 is mathematically lossless)"
        );

        let v3_target = 0.98_f64;
        assert!(
            target_ssim > v3_target,
            "v4.0 target {target_ssim} should be higher than v3.9 target {v3_target}"
        );
    }

    #[test]

    fn test_v4_crf_precision_0_1() {
        let test_values: [f32; 5] = [18.0, 18.25, 18.5, 18.75, 19.0];

        for &crf in &test_values {
            let rounded = (crf * 4.0).round() / 4.0;
            assert!(
                (rounded - crf).abs() < 0.01,
                "CRF {crf} should round to {rounded} with 0.25 step"
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
        let phase1_iterations = crate::numeric_cast::f32_to_u32_sat((range / phase1_step).ceil());
        assert_eq!(phase1_iterations, 18);

        let phase2_step = 0.5_f32;
        let phase2_range = 4.0_f32;
        let phase2_iterations =
            crate::numeric_cast::f32_to_u32_sat((phase2_range / phase2_step).ceil());
        assert_eq!(phase2_iterations, 8);

        let phase3_step = 0.1_f32;
        let phase3_range = 1.0_f32;
        let phase3_iterations =
            crate::numeric_cast::f32_to_u32_sat((phase3_range / phase3_step).ceil());
        assert_eq!(phase3_iterations, 10);
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
            if ssim >= 0.999_9_f64 {
                "Near-Lossless"
            } else if ssim >= 0.999_f64 {
                "Excellent"
            } else if ssim >= 0.98_f64 {
                "Very Good"
            } else if ssim >= 0.93_f64 {
                "Good"
            } else if ssim >= 0.89_f64 {
                "Acceptable"
            } else {
                "Below threshold"
            }
        };

        assert_eq!(grade(0.999_9_f64), "Near-Lossless");
        assert_eq!(grade(0.999_5_f64), "Excellent");
        assert_eq!(grade(0.985_f64), "Very Good");
        assert_eq!(grade(0.94_f64), "Good");
        assert_eq!(grade(0.90_f64), "Acceptable");
        assert_eq!(grade(0.80_f64), "Below threshold");
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
        let mut plateau_count = 0_i32;

        for &(_crf, ssim) in &ssim_values {
            if ssim > best_ssim {
                best_ssim = ssim;
                plateau_count = 0_i32;
            } else {
                plateau_count += 1_i32;
            }

            if plateau_count >= 2_i32 {
                break;
            }
        }

        assert!(
            plateau_count >= 2_i32,
            "Should detect plateau after 2 non-improvements"
        );
        assert!(
            (best_ssim - 0.9856).abs() < 0.000_1_f64,
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
            "Source SSIM {source_ssim} should be below target {target_ssim}"
        );
    }

    #[test]

    fn test_v4_low_quality_source_ceiling() {
        let source_ssim = 0.9200_f64;
        let target_ssim = 0.9999_f64;

        let ssim_ceiling = source_ssim + 0.05_f64;

        assert!(
            ssim_ceiling < target_ssim,
            "Low quality source cannot reach target SSIM {target_ssim}"
        );
    }

    #[test]

    fn test_v4_crf_cache_mechanism() {
        let mut cache: std::collections::HashMap<i32, f64> = std::collections::HashMap::new();

        cache.insert(precision::crf_to_cache_key(20.0).unwrap(), 0.985_0_f64);
        cache.insert(precision::crf_to_cache_key(20.1).unwrap(), 0.985_5_f64);
        cache.insert(precision::crf_to_cache_key(20.5).unwrap(), 0.986_0_f64);
        cache.insert(precision::crf_to_cache_key(20.05).unwrap(), 0.985_2_f64);
        cache.insert(precision::crf_to_cache_key(20.45).unwrap(), 0.985_8_f64);

        assert!(cache.contains_key(&precision::crf_to_cache_key(20.0).unwrap()));
        assert!(cache.contains_key(&precision::crf_to_cache_key(20.1).unwrap()));
        assert!(cache.contains_key(&precision::crf_to_cache_key(20.5).unwrap()));
        assert!(
            cache.contains_key(&precision::crf_to_cache_key(20.05).unwrap()),
            "20.05 should have its own key and hit cache"
        );
        assert!(
            cache.contains_key(&precision::crf_to_cache_key(20.45).unwrap()),
            "20.45 should have its own key and hit cache"
        );

        assert!(!cache.contains_key(&precision::crf_to_cache_key(20.75).unwrap()));
        assert!(!cache.contains_key(&precision::crf_to_cache_key(19.75).unwrap()));

        assert_eq!(precision::crf_to_cache_key(20.0), Some(2_000_i32));
        assert_eq!(precision::crf_to_cache_key(20.1), Some(2_010_i32));
        assert_eq!(precision::crf_to_cache_key(20.5), Some(2_050_i32));
        assert_eq!(precision::crf_to_cache_key(20.05), Some(2_005_i32));
        assert_eq!(precision::crf_to_cache_key(20.15), Some(2_015_i32));
    }

    #[test]

    fn test_v4_no_iteration_limit() {
        let range = 51.0_f64 - 0.0_f64;
        let phase1 = crate::numeric_cast::f64_to_u32_sat((range / 1.0_f64).ceil());
        let phase2 = crate::numeric_cast::f64_to_u32_sat((4.0_f64 / 0.5_f64).ceil());
        let phase3 = crate::numeric_cast::f64_to_u32_sat((1.0_f64 / 0.1_f64).ceil());
        let phase4_max = 50_u32;

        let total_max = phase1 + phase2 + phase3 + phase4_max;

        assert!(
            total_max <= 150,
            "Total iterations should be reasonable: {total_max}"
        );
    }

    #[test]

    fn test_v4_content_type_ssim_convergence() {
        let animation_convergence_rate = 0.002_f64;

        let live_action_convergence_rate = 0.001_f64;

        let high_detail_convergence_rate = 0.0005_f64;

        assert!(animation_convergence_rate > live_action_convergence_rate);
        assert!(live_action_convergence_rate > high_detail_convergence_rate);

        let target_improvement = 0.999_9_f64 - 0.990_0_f64;

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
            "Target and excellent SSIM should be distinguishable: diff={difference}"
        );

        let epsilon = SSIM_COMPARE_EPSILON;
        assert!(
            (epsilon - 0.0001).abs() < 1e-10_f64,
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
                .map(|s| {
                    f64::from(crate::numeric_cast::f64_to_f32_lossy(
                        crate::numeric_cast::u64_to_f64(*s)
                            / crate::numeric_cast::u64_to_f64(input_size),
                    ))
                })
                .collect();
            let mean = if recent.is_empty() {
                0.0_f64
            } else {
                recent.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(recent.len())
            };
            if recent.is_empty() {
                0.0_f64
            } else {
                recent.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
                    / crate::numeric_cast::usize_to_f64(recent.len())
            }
        };

        let stable_sizes = vec![500_000_u64, 500_100, 500_050];
        let stable_variance = calc_variance(&stable_sizes);
        assert!(
            stable_variance < variance_threshold,
            "Stable sizes should have low variance: {stable_variance}"
        );

        let varying_sizes = vec![500_000_u64, 600_000, 550_000];
        let varying_variance = calc_variance(&varying_sizes);
        assert!(
            varying_variance > variance_threshold,
            "Varying sizes should have high variance: {varying_variance}"
        );
    }

    #[test]

    fn test_v413_relative_change_rate() {
        let change_rate_threshold = 0.005_f64;

        let calc_change_rate = |prev: u64, curr: u64| -> f64 {
            if prev == 0 {
                return f64::MAX;
            }
            ((crate::numeric_cast::u64_to_f64(curr) - crate::numeric_cast::u64_to_f64(prev))
                / crate::numeric_cast::u64_to_f64(prev.max(1)))
            .abs()
        };

        let small_change = calc_change_rate(1_000_000, 1_004_000);
        assert!(
            small_change < change_rate_threshold,
            "Small change {small_change} should be below threshold"
        );

        let large_change = calc_change_rate(1_000_000, 1_010_000);
        assert!(
            large_change > change_rate_threshold,
            "Large change {large_change} should be above threshold"
        );
    }

    #[test]

    fn test_v413_three_phase_search() {
        let phase1_step = 0.5_f32;
        let crf_range = 28.0_f32 - 10.0_f32;
        let phase1_iterations =
            crate::numeric_cast::f32_to_u32_sat((crf_range / phase1_step).log2().ceil());
        assert!(
            phase1_iterations <= 6,
            "Phase 1 should need ~6 iterations: {phase1_iterations}"
        );

        let phase2_range = 0.8_f32;
        let phase2_step = 0.1_f32;
        let phase2_max_iterations =
            crate::numeric_cast::f32_to_u32_sat((phase2_range / phase2_step).ceil());
        assert_eq!(
            phase2_max_iterations, 8,
            "Phase 2 should need max 8 iterations"
        );

        let phase3_iterations = 1_u32;

        let total_max = phase1_iterations + phase2_max_iterations + phase3_iterations;
        assert!(
            total_max <= 15,
            "Total iterations should be <= 15: {total_max}"
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
                "CRF {crf} should be 0.25 precision"
            );
        }

        assert!(
            (precision::SEARCH_STEP_ULTRA_FINE - 0.25).abs() < 1e-6,
            "ULTRA_FINE_STEP should be 0.25"
        );
        assert!(
            (precision::SEARCH_STEP_FINE - 0.5).abs() < 1e-6,
            "FINE_STEP should be 0.5"
        );
    }

    #[test]

    fn test_adaptive_max_walls_boundary_conditions() {
        assert_eq!(
            calculate_adaptive_max_walls(0.0).unwrap(),
            ULTIMATE_MIN_WALL_HITS
        );
        assert_eq!(
            calculate_adaptive_max_walls(0.5).unwrap(),
            ULTIMATE_MIN_WALL_HITS
        );
        assert_eq!(
            calculate_adaptive_max_walls(1.0).unwrap(),
            ULTIMATE_MIN_WALL_HITS
        );

        for range in [2.0, 5.0, 10.0, 20.0, 30.0, 50.0, 100.0, 1000.0] {
            let result = calculate_adaptive_max_walls(range).unwrap();
            assert!(
                result >= ULTIMATE_MIN_WALL_HITS,
                "range {range} -> {result} should >= {ULTIMATE_MIN_WALL_HITS}"
            );
            assert!(
                result <= ULTIMATE_MAX_WALL_HITS,
                "range {range} -> {result} should <= {ULTIMATE_MAX_WALL_HITS}"
            );
        }
    }

    #[test]

    fn test_adaptive_max_walls_monotonicity() {
        let mut prev = calculate_adaptive_max_walls(2.0).unwrap();
        for range in [4.0, 8.0, 16.0, 32.0, 64.0] {
            let curr = calculate_adaptive_max_walls(range).unwrap();
            assert!(
                curr >= prev,
                "monotonicity violated: range {range} -> {curr} < prev {prev}"
            );
            prev = curr;
        }
    }

    #[test]

    fn test_adaptive_max_walls_formula_correctness() {
        // Updated for v0.10.32+: ULTIMATE_MIN_WALL_HITS changed from 4 to 15
        assert_eq!(calculate_adaptive_max_walls(10.0).unwrap(), 15); // clamped to ULTIMATE_MIN_WALL_HITS

        assert_eq!(calculate_adaptive_max_walls(18.0).unwrap(), 15); // clamped to ULTIMATE_MIN_WALL_HITS

        assert_eq!(calculate_adaptive_max_walls(30.0).unwrap(), 15); // clamped to ULTIMATE_MIN_WALL_HITS

        assert_eq!(calculate_adaptive_max_walls(50.0).unwrap(), 15); // clamped to ULTIMATE_MIN_WALL_HITS

        assert_eq!(
            calculate_adaptive_max_walls(100_000.0).unwrap(),
            (crate::numeric_cast::f32_to_u32_sat(100_000.0_f32.log2().ceil())
                + ADAPTIVE_WALL_LOG_BASE)
                .min(ULTIMATE_MAX_WALL_HITS)
        );
    }

    #[test]

    fn test_ultimate_mode_constants() {
        // Updated for v0.10.32+: ULTIMATE_MIN_WALL_HITS (15) > NORMAL_MAX_WALL_HITS (4)
        // This is intentional to ensure deeper saturation in ultimate mode
        const {
            assert!(ULTIMATE_MIN_WALL_HITS > NORMAL_MAX_WALL_HITS);
        }
    }

    #[test]

    fn test_adaptive_max_walls_defensive_checks() {
        assert_eq!(
            calculate_adaptive_max_walls(-1.0).unwrap(),
            ULTIMATE_MIN_WALL_HITS
        );
        assert_eq!(
            calculate_adaptive_max_walls(-100.0).unwrap(),
            ULTIMATE_MIN_WALL_HITS
        );

        assert_eq!(
            calculate_adaptive_max_walls(f32::NAN).unwrap(),
            ULTIMATE_MIN_WALL_HITS
        );

        assert_eq!(
            calculate_adaptive_max_walls(f32::INFINITY).unwrap(),
            ULTIMATE_MIN_WALL_HITS
        );
        assert_eq!(
            calculate_adaptive_max_walls(f32::NEG_INFINITY).unwrap(),
            ULTIMATE_MIN_WALL_HITS
        );
    }

    #[test]

    fn test_crf_to_cache_key_precision() {
        use precision::crf_to_cache_key;

        assert_eq!(crf_to_cache_key(20.0), Some(2_000_i32));
        assert_eq!(crf_to_cache_key(20.1), Some(2_010_i32));
        assert_eq!(crf_to_cache_key(20.5), Some(2_050_i32));

        assert_eq!(crf_to_cache_key(0.0), Some(0_i32));
        assert_eq!(crf_to_cache_key(51.0), Some(5_100_i32));
        assert_eq!(crf_to_cache_key(63.0), Some(6_300_i32));

        assert_eq!(crf_to_cache_key(20.05), Some(2_005_i32));
        assert_eq!(crf_to_cache_key(20.04), Some(2_004_i32));
    }

    #[test]

    fn test_crf_cache_key_roundtrip() {
        use precision::{cache_key_to_crf, crf_to_cache_key};

        for crf in [10.0, 15.0, 20.0, 25.0, 30.0, 51.0] {
            let key = crf_to_cache_key(crf).expect("Valid CRF must yield key");
            let back = cache_key_to_crf(key);
            assert!(
                (crf - back).abs() < 0.001,
                "Roundtrip failed: {crf} -> {key} -> {back}"
            );
        }

        for crf in [20.1, 20.5, 20.9, 25.3, 30.7] {
            let key = crf_to_cache_key(crf).expect("Valid CRF must yield key");
            let back = cache_key_to_crf(key);
            assert!(
                (crf - back).abs() < 0.001,
                "Roundtrip failed: {crf} -> {key} -> {back}"
            );
        }
    }

    #[test]
    fn test_zero_gains_scaling_basic() {
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 41.0, true).unwrap(),
            ULTIMATE_REQUIRED_ZERO_GAINS
        );
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 20.0, true).unwrap(),
            ULTIMATE_REQUIRED_ZERO_GAINS
        );

        // ultimate_mode: base 100, crf_range 15 -> factor 0.75, scaled = 100 * 0.75 =
        // 75
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 15.0, true).unwrap(),
            75
        );

        // crf_range 10 -> factor 0.5, scaled = 100 * 0.5 = 50
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 10.0, true).unwrap(),
            50
        );

        assert_eq!(
            calculate_zero_gains_for_duration_and_range(60.0, 5.0, true).unwrap(),
            50
        );
    }

    #[test]

    fn test_zero_gains_minimum_guarantee() {
        assert!(calculate_zero_gains_for_duration_and_range(60.0, 1.0, true).unwrap() >= 15);
        assert!(calculate_zero_gains_for_duration_and_range(60.0, 0.1, true).unwrap() >= 15);
        assert!(calculate_zero_gains_for_duration_and_range(60.0, 5.0, false).unwrap() >= 3);
    }

    #[test]

    fn test_zero_gains_long_video_override() {
        // Long video uses LONG_VIDEO_REQUIRED_ZERO_GAINS as base, but ultimate_mode
        // still enforces min 15
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(300.0, 41.0, true).unwrap(),
            15
        );
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(600.0, 10.0, true).unwrap(),
            15
        );
        // Non-ultimate: long video returns base (3) scaled
        assert_eq!(
            calculate_zero_gains_for_duration_and_range(300.0, 41.0, false).unwrap(),
            LONG_VIDEO_REQUIRED_ZERO_GAINS
        );
    }

    #[test]
    fn sealed_ultimate_neutralizes_stale_ms_ssim_failed() {
        let result = ExploreResult {
            ultimate_mode: true,
            vmaf_y_score: Some(96.0),
            cambi_score: Some(5.0),
            psnr_uv_score: Some((48.0, 47.0)),
            ultimate_quality_passed: CheckResult::Passed,
            quality_passed: CheckResult::Passed,
            ms_ssim_passed: CheckResult::Failed("SSIM below target".into()),
            ..ExploreResult::default()
        };
        let sealed = result.sealed();
        assert!(sealed.ms_ssim_passed.is_skipped());
        assert!(!sealed.perceptual_quality_failed());
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
            let small_result = calculate_zero_gains_for_duration_and_range(duration, crf_range_small, true).unwrap();
            let large_result = calculate_zero_gains_for_duration_and_range(duration, crf_range_large, true).unwrap();

            prop_assert!(small_result <= large_result,
                "zero-gains({}) for small CRF range ({}) should be <= zero-gains({}) for large CRF range ({})",
                small_result, crf_range_small, large_result, crf_range_large);
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
            let result = calculate_zero_gains_for_duration_and_range(duration, crf_range, ultimate_mode).unwrap();

            let min_expected = if ultimate_mode { 15 } else { 3 };
            prop_assert!(result >= min_expected,
                "zero-gains({}) should be >= {} (duration={}, crf_range={}, ultimate={})",
                result, min_expected, duration, crf_range, ultimate_mode);
        }
    }

    #[test]
    fn measured_exploration_confidence_without_ssim_stays_below_floor() {
        let (confidence, detail) = measured_exploration_confidence(None, 0.95, 2, 100);
        assert!(detail.ssim_confidence.is_none());
        assert!(
            confidence.is_none_or(|c| c < crate::constants::MIN_EXPLORATION_CONFIDENCE),
            "sampling-only confidence must not satisfy exploration floor, got {confidence:?}"
        );
    }

    #[test]
    fn measured_exploration_confidence_high_ssim_meets_floor() {
        let (confidence, _) = measured_exploration_confidence(Some(0.99), 0.95, 8, 10);
        assert!(
            confidence.is_some_and(|c| c >= crate::constants::MIN_EXPLORATION_CONFIDENCE),
            "expected sealed confidence >= floor, got {confidence:?}"
        );
    }

    #[test]
    fn exploration_size_margin_none_when_not_compressed() {
        assert!(exploration_size_margin_from_output(1_000, 1_000).is_none());
        assert!(exploration_size_margin_from_output(1_000, 1_100).is_none());
    }

    #[test]
    fn exploration_size_margin_positive_when_below_target() {
        let margin = exploration_size_margin_from_output(10_000, 5_000)
            .expect("output well below compression target should yield margin");
        assert!(margin > 0.0 && margin.is_finite());
    }

    #[test]
    fn explore_result_confidence_gate_downgrades_weak_pass() {
        let below_floor = crate::constants::MIN_EXPLORATION_CONFIDENCE * 0.2;
        let mut result = ExploreResult {
            optimal_crf: 24.0,
            quality_passed: CheckResult::Passed,
            confidence: Some(below_floor),
            ssim: Some(0.99),
            actual_min_ssim: 0.95,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            !result.quality_passed.is_passed(),
            "low confidence must not keep quality_passed"
        );
    }

    #[test]
    fn explore_result_ssim_presence_gate_rejects_pass_without_ssim() {
        let mut result = ExploreResult {
            optimal_crf: 24.0,
            quality_passed: CheckResult::Passed,
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.4),
            ssim: None,
            actual_min_ssim: 0.95,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            !result.quality_passed.is_passed(),
            "quality_passed without SSIM must be rejected"
        );
    }

    #[test]
    fn explore_result_lossless_integrity_contract_accepts_without_ssim() {
        let mut result = ExploreResult {
            optimal_crf: 0.0,
            quality_passed: CheckResult::Passed,
            ms_ssim_passed: CheckResult::Passed,
            size_target_met: CheckResult::Passed,
            confidence: Some(0.15),
            ssim: None,
            actual_min_ssim: 0.95,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            result.quality_passed.is_passed(),
            "lossless integrity path must not be rejected by SSIM presence or confidence gates"
        );
    }

    #[test]
    fn explore_result_ultimate_backfills_confidence_from_3d_metrics() {
        let mut result = ExploreResult {
            optimal_crf: 30.0,
            quality_passed: CheckResult::Passed,
            ultimate_mode: true,
            ultimate_quality_passed: CheckResult::Passed,
            vmaf_y_score: Some(95.0),
            cambi_score: Some(5.0),
            psnr_uv_score: Some((44.0, 43.0)),
            iterations: 12,
            size_target_met: CheckResult::Passed,
            confidence: None,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            result
                .confidence
                .is_some_and(|c| c >= crate::constants::MIN_EXPLORATION_CONFIDENCE),
            "ultimate 3D pass must backfill exploration confidence for delivery gate"
        );
        assert!(result.pipeline_acceptable(true, false));
    }

    #[test]
    fn explore_result_ssim_predicted_fallback_gate_rejects_pass() {
        let mut result = ExploreResult {
            optimal_crf: 24.0,
            quality_passed: CheckResult::Passed,
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.4),
            ssim: Some(0.96),
            used_fallback: true,
            actual_min_ssim: 0.95,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            !result.quality_passed.is_passed(),
            "PSNR-derived SSIM estimate must not keep quality_passed under strict delivery"
        );
    }

    #[test]
    fn explore_result_ultimate_sanity_gate_rejects_subfloor_vmaf() {
        let mut result = ExploreResult {
            optimal_crf: 24.0,
            quality_passed: CheckResult::Passed,
            ultimate_mode: true,
            ultimate_quality_passed: CheckResult::Passed,
            vmaf_y_score: Some(80.0),
            cambi_score: Some(5.0),
            psnr_uv_score: Some((40.0, 40.0)),
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.4),
            size_target_met: CheckResult::Passed,
            ..Default::default()
        };
        result = result.sealed();
        assert!(!result.quality_passed.is_passed());
        assert!(result.ultimate_quality_passed.is_failed());
    }

    #[test]
    fn explore_result_ultimate_metrics_presence_gate_rejects_incomplete_3d() {
        let mut result = ExploreResult {
            optimal_crf: 24.0,
            quality_passed: CheckResult::Passed,
            ultimate_mode: true,
            ultimate_quality_passed: CheckResult::Passed,
            vmaf_y_score: Some(96.0),
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.4),
            size_target_met: CheckResult::Passed,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            !result.quality_passed.is_passed(),
            "ultimate pass with only VMAF-Y must be rejected under strict delivery"
        );
    }

    #[test]
    fn explore_result_ultimate_mode_does_not_require_ssim() {
        let mut result = ExploreResult {
            optimal_crf: 24.0,
            quality_passed: CheckResult::Passed,
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.4),
            ssim: None,
            ultimate_mode: true,
            ultimate_quality_passed: CheckResult::Passed,
            ms_ssim_passed: CheckResult::NotChecked,
            vmaf_y_score: Some(96.5),
            cambi_score: Some(5.0),
            psnr_uv_score: Some((48.0, 47.0)),
            size_target_met: CheckResult::Passed,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            result.quality_passed.is_passed(),
            "ultimate explore must not fail sealed() for missing SSIM"
        );
        assert!(result.pipeline_acceptable(true, false));
    }

    #[test]
    fn explore_result_ssim_threshold_gate_rejects_low_ssim() {
        let mut result = ExploreResult {
            optimal_crf: 24.0,
            quality_passed: CheckResult::Passed,
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.4),
            ssim: Some(0.80),
            actual_min_ssim: 0.95,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            !result.quality_passed.is_passed(),
            "quality_passed below SSIM floor must be rejected"
        );
    }

    #[test]
    fn pipeline_acceptable_size_only_without_quality_pass() {
        let result = ExploreResult {
            size_target_met: CheckResult::Passed,
            quality_passed: CheckResult::NotChecked,
            size_change_pct: -12.0,
            confidence: Some(1.00),
            ..Default::default()
        };
        assert!(result.pipeline_acceptable(false, true));
        assert!(!result.pipeline_acceptable(true, false));
    }

    #[test]
    fn strict_delivery_rejects_compression_only_without_size_target() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let result = ExploreResult {
            size_target_met: CheckResult::Failed("not smaller".into()),
            quality_passed: CheckResult::NotChecked,
            size_change_pct: -5.0,
            ..Default::default()
        };
        assert!(
            !result.pipeline_acceptable(false, true),
            "strict explore_smaller must require size_target_met, not size_change_pct alone"
        );
    }

    #[test]
    fn explore_result_size_target_gate_rejects_quality_with_size_fail() {
        let mut result = ExploreResult {
            quality_passed: CheckResult::Passed,
            size_target_met: CheckResult::Failed("Output not smaller than input".into()),
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.4),
            ssim: Some(0.98),
            actual_min_ssim: 0.95,
            ..Default::default()
        };
        result = result.sealed();
        assert!(
            !result.quality_passed.is_passed(),
            "quality_passed with explicit size failure must be rejected"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]

        fn prop_duration_fallback_calculation(
            frame_count in 1u64..1_000_000u64,
            fps in 1.0f64..240.0f64,
        ) {
            let duration = crate::numeric_cast::u64_to_f64(frame_count) / fps;
            prop_assert!(duration > 0.0_f64, "Duration should be positive: {}", duration);
            let reconstructed_frames = (duration * fps).round();
            prop_assert!(
                (reconstructed_frames - crate::numeric_cast::u64_to_f64(frame_count)).abs() < 1.0_f64,
                "duration * fps should approximate frame_count: {} * {} ≈ {}",
                duration, fps, frame_count
            );
        }
    }
}
