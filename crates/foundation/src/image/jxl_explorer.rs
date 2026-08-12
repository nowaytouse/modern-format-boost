//! JXL Distance Explorer for Ultimate Mode
//!
//! Two-phase screening algorithm to identify JXL distance candidates for
//! same-effort verification:
//!
//! **Phase 1 (Ladder)**: Test predefined distances, promote candidates based on
//! quality/region. **Phase 2 (Binary Search)**: Refine promising regions with
//! adaptive step sizing.
//!
//! ## Unified Selection Philosophy
//!
//! Finalist promotion uses consistent priorities (see `candidate_comparator`
//! for theory):
//!
//! 1. **Quality Gates**: Output must compress (`size < input`)
//! 2. **Quality Axis**: Among fitting candidates, the lowest distance wins
//! 3. **Size Tiebreaker**: Size is compared only at the same distance
//! 4. **Boundary Detection**: Candidates near 95–105% of input size promoted
//! 5. **Region Coverage**: Promotes one candidate per distance region for
//!    diversity
//! 6. **Score-based Ranking**: Finalists ranked by promotion score, then size,
//!    then distance
//!
//! Terminology (unified with HEVC/VideoExplorer):
//! - **Screening**: Phase 1 ladder + Phase 2 binary search exploration
//! - **Candidate**: A specific distance value with its output size
//! - **Finalist shortlist**: Curated subset promoted for same-domain verification

use crate::constants::{
    JXL_EXPLORE_BINARY_SEARCH_PRECISION, JXL_EXPLORE_CEILING, JXL_EXPLORE_FLOOR,
    JXL_EXPLORE_MAX_ITERATIONS,
};
#[cfg(feature = "high-precision")]
use rug::Rational;
use std::collections::HashSet;

const JXL_FINALIST_LIMIT: usize = crate::constants::JXL_FINALIST_LIMIT;
const JXL_NEAR_BEST_MARGIN_RATIO: f64 = crate::constants::JXL_DISTANCE_PLATEAU;
const JXL_BOUNDARY_LOW_RATIO: f64 = crate::constants::JXL_BOUNDARY_LOW_RATIO;
const JXL_BOUNDARY_HIGH_RATIO: f64 = crate::constants::JXL_BOUNDARY_HIGH_RATIO;
const JXL_REGION_BUCKET_COUNT: f64 = crate::constants::JXL_REGION_BUCKET_COUNT;

// --- Perceptual Band Boundaries ---
//
// These boundaries partition the JXL distance space into perceptual quality
// tiers. They are empirical constants derived from the JXL specification's
// distance semantics:
//
//   d ≤ 0.01  → "plateau" — mathematically lossless or indistinguishable from
// it   d ≤ 0.1   → "visually lossless" — no visible artifacts at normal viewing
//   d ≤ 0.3   → "balanced" — quality/size sweet spot for archival
//   d > 0.3   → "ceiling sweep" — aggressive compression, visible trade-offs
//
// The interpolation strategy differs per tier:
//   - MicroAdjust (plateau):  log10 interpolation + smoothstep — distances are
//     so small that linear steps would collapse to a single float; log-space
//     preserves resolution.
//   - BoundaryPush/WidePush:  linear interpolation + smoothstep — in the
//     perceptual range, equal ΔDistance ≈ equal ΔJND, so linear spacing tracks
//     perception.
//   - CeilingSweep:           linear interpolation with diminishing returns via
//     normalized excess pressure — prevents runaway distance growth.
//
// NOTE: These are manually calibrated partitions, not natural constants. If
// dataset distribution changes significantly, recalibrate via telemetry-driven
// analysis of (initial_ratio, pressure_stops, chosen_profile, target_distance,
// outcome_quality) tuples logged by the screening pass.
const JXL_DISTANCE_CEILING_PLATEAU_MAX: f64 = crate::constants::JXL_DISTANCE_PLATEAU;
const JXL_DISTANCE_VISUAL_LOSSLESS_MAX: f64 = crate::constants::JXL_DISTANCE_VISUAL_LOSSLESS_MAX;
const JXL_DISTANCE_BALANCED_MAX: f64 = crate::constants::JXL_DISTANCE_BALANCED_MAX;

// --- Pressure-Stop Boundaries ---
//
// Pressure stops = log2(initial_ratio) — how many doublings the initial JXL
// output exceeds the input. Each boundary maps to a profile that governs search
// intensity and distance range. Values are log2 of the ratio thresholds:
//
//   ≤ 0.0704 stops (~1.05×) → MicroAdjust:   file nearly fits; fine-tune near
// d=0   ≤ 0.5850 stops (~1.50×) → BoundaryPush:  moderate oversize; push to
// visual lossless   ≤ 1.3219 stops (~2.50×) → WidePush:      significant
// oversize; explore balanced range
//   > 1.3219 stops           → CeilingSweep:  extreme oversize; sweep toward
//   > ceiling
const JXL_MICRO_PRESSURE_STOPS_MAX: f64 = crate::constants::JXL_MICRO_PRESSURE_LIMIT; // log2(1.05)
const JXL_BOUNDARY_PRESSURE_STOPS_MAX: f64 = crate::constants::JXL_BOUNDARY_PRESSURE_STOPS_MAX; // log2(1.50)
const JXL_WIDE_PRESSURE_STOPS_MAX: f64 = crate::constants::JXL_WIDE_PRESSURE_STOPS_MAX; // log2(2.50)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlPromotionReason {
    Baseline,
    BetterThanCurrentBest,
    NearCurrentBest,
    BoundaryRegion,
    NewRegion,
    AdjacentToBest,
}

impl JxlPromotionReason {
    const fn label(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::BetterThanCurrentBest => "better-than-best",
            Self::NearCurrentBest => "near-best",
            Self::BoundaryRegion => "boundary",
            Self::NewRegion => "new-region",
            Self::AdjacentToBest => "adjacent",
        }
    }

    const fn weight(self) -> u32 {
        match self {
            Self::Baseline => 1,
            Self::NewRegion => 2,
            Self::BoundaryRegion => 3,
            Self::AdjacentToBest => 4,
            Self::NearCurrentBest => 5,
            Self::BetterThanCurrentBest => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JxlScreenedCandidate {
    pub distance: f32,
    pub output_size: u64,
    pub ladder_phase: bool,
    pub reasons: Vec<JxlPromotionReason>,
}

impl JxlScreenedCandidate {
    fn has_reason(&self, reason: JxlPromotionReason) -> bool {
        self.reasons.contains(&reason)
    }

    fn promotion_score(&self) -> u32 {
        self.reasons.iter().map(|reason| reason.weight()).sum()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JxlScreeningResult {
    pub best_distance: f32,
    pub best_output_size: u64,
    pub iterations: u32,
    pub screened_candidates: Vec<JxlScreenedCandidate>,
    pub finalists: Vec<JxlScreenedCandidate>,
    pub log: Vec<String>,
    /// Telemetry: ratio of initial JXL output to input (≥1.0 when oversize).
    pub initial_ratio: f64,
    /// Telemetry: `log2(initial_ratio)` — oversize severity in doublings.
    pub pressure_stops: f64,
    /// Telemetry: exploration profile selected for this file.
    pub profile_label: &'static str,
    /// Telemetry: target distance the adaptive plan aimed for.
    pub target_distance: f32,
}

impl JxlScreeningResult {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.seal_algorithm_outputs();
        self
    }

    /// Sanitize exploration distances and telemetry before finalist verification.
    pub fn seal_algorithm_outputs(&mut self) {
        if let Some(d) = crate::algorithm_seal::seal_jxl_distance(self.best_distance) {
            self.best_distance = d;
        }
        if let Some(d) = crate::algorithm_seal::seal_jxl_distance(self.target_distance) {
            self.target_distance = d;
        }
        if let Some(r) = crate::algorithm_seal::seal_non_negative_ratio(self.initial_ratio) {
            self.initial_ratio = r;
        }
        if let Some(p) = crate::algorithm_seal::seal_finite_scalar(self.pressure_stops) {
            self.pressure_stops = p;
        }
        for candidate in &mut self.screened_candidates {
            if let Some(d) = crate::algorithm_seal::seal_jxl_distance(candidate.distance) {
                candidate.distance = d;
            }
        }
        for candidate in &mut self.finalists {
            if let Some(d) = crate::algorithm_seal::seal_jxl_distance(candidate.distance) {
                candidate.distance = d;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JxlExploreResult {
    /// Effort domain in which accepted distance and size were measured.
    pub encoder_domain: crate::exploration_policy::EncoderDomain,
    pub outcome: crate::exploration_policy::ExplorationOutcome,
    pub accepted_distance: f32,
    pub output_size: u64,
    pub iterations: u32,
    pub ladder_phase: bool,
    pub screened_best_distance: f32,
    pub screened_best_size: u64,
    pub promoted_distances: Vec<f32>,
    pub log: Vec<String>,
    /// Telemetry: ratio of initial JXL output to input.
    pub initial_ratio: f64,
    /// Telemetry: `log2(initial_ratio)` — oversize severity in doublings.
    pub pressure_stops: f64,
    /// Telemetry: exploration profile selected for this file.
    pub profile_label: &'static str,
    /// Telemetry: target distance the adaptive plan aimed for.
    pub target_distance: f32,
}

impl JxlExploreResult {
    /// Consume and return a sealed JXL exploration result.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.seal_algorithm_outputs();
        self.enforce_delivery_gates();
        self
    }

    fn enforce_delivery_gates(&mut self) {
        if !crate::algorithm_runtime::strict_media_conversion_delivery_enabled() {
            return;
        }
        if self.output_size == 0 {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "jxl_exploration",
                branch = "jxl_explore_zero_output",
                "JXL explore result has zero output_size under strict delivery"
            );
        }
        if !self.accepted_distance.is_finite() {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "jxl_exploration",
                branch = "jxl_explore_non_finite_distance",
                distance = self.accepted_distance,
                "JXL explore result has non-finite accepted_distance under strict delivery"
            );
            self.accepted_distance = crate::constants::JXL_MIN_DISTANCE;
        }
    }

    /// Strict delivery: non-zero output, finite sealed distance, at least one
    /// screening iteration.
    #[must_use]
    pub fn delivery_acceptable(&self) -> bool {
        if !crate::algorithm_runtime::strict_media_conversion_delivery_enabled() {
            return true;
        }
        matches!(
            self.encoder_domain,
            crate::exploration_policy::EncoderDomain::Jxl { .. }
        ) && self.outcome == crate::exploration_policy::ExplorationOutcome::ExploredOptimized
            && self.output_size > 0
            && self.accepted_distance.is_finite()
            && self.iterations > 0
    }

    /// Sanitize finalized JXL exploration outputs.
    pub fn seal_algorithm_outputs(&mut self) {
        if let Some(d) = crate::algorithm_seal::seal_jxl_distance(self.accepted_distance) {
            self.accepted_distance = d;
        } else if !self.accepted_distance.is_finite() {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "jxl_exploration",
                branch = "jxl_accepted_distance_non_finite",
                "non-finite accepted_distance; clamping to JXL floor"
            );
            self.accepted_distance = crate::constants::JXL_MIN_DISTANCE;
        }
        if let Some(d) = crate::algorithm_seal::seal_jxl_distance(self.screened_best_distance) {
            self.screened_best_distance = d;
        }
        if let Some(d) = crate::algorithm_seal::seal_jxl_distance(self.target_distance) {
            self.target_distance = d;
        }
        if let Some(r) = crate::algorithm_seal::seal_non_negative_ratio(self.initial_ratio) {
            self.initial_ratio = r;
        }
        if let Some(p) = crate::algorithm_seal::seal_finite_scalar(self.pressure_stops) {
            self.pressure_stops = p;
        }
        for distance in &mut self.promoted_distances {
            if let Some(d) = crate::algorithm_seal::seal_jxl_distance(*distance) {
                *distance = d;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JxlExplorationProfile {
    MicroAdjust,
    BoundaryPush,
    WidePush,
    CeilingSweep,
}

impl JxlExplorationProfile {
    const fn label(self) -> &'static str {
        match self {
            Self::MicroAdjust => "micro-adjust",
            Self::BoundaryPush => "boundary-push",
            Self::WidePush => "wide-push",
            Self::CeilingSweep => "ceiling-sweep",
        }
    }
}

/// Calculates pressure stops for oversized images.
///
/// Uses logarithmic scaling to determine how much pressure should
/// be applied to reduce file size for very large images.
///
/// # Arguments
/// * `initial_ratio` - The initial compression ratio
///
/// # Returns
/// Pressure stop value for oversized images
fn oversize_pressure_stops(initial_ratio: f64) -> f64 {
    initial_ratio.max(1.0).log2()
}

#[derive(Debug, Clone, PartialEq)]
struct JxlExplorationPlan {
    profile: JxlExplorationProfile,
    target_distance: f32,
    ladder: Vec<f32>,
}

type DistanceKey = u32;

/// Clamps a distance value to the valid exploration range `[FLOOR, CEILING]`.
#[inline]
#[must_use]
pub const fn clamp_explore_distance(distance: f32) -> f32 {
    if !distance.is_finite() {
        return JXL_EXPLORE_FLOOR;
    }

    distance.clamp(JXL_EXPLORE_FLOOR, JXL_EXPLORE_CEILING)
}

/// Converts a distance value to a hashable key.
///
/// Uses the raw bits of the clamped distance value to create
/// a key suitable for use in hash maps and sets.
///
/// # Arguments
/// * `distance` - The distance value to convert
///
/// # Returns
/// Hashable distance key
const fn distance_key(distance: f32) -> DistanceKey {
    clamp_explore_distance(distance).to_bits()
}

/// Trims trailing zeros and decimal point from a string.
///
/// Removes unnecessary trailing zeros from decimal strings
/// to create cleaner, more readable numeric representations.
///
/// # Arguments
/// * `raw` - The raw string to trim
///
/// # Returns
/// Cleaned string without trailing zeros
fn trim_decimal_string(mut raw: String) -> String {
    if raw.contains('.') {
        while raw.ends_with('0') {
            raw.pop();
        }
        if raw.ends_with('.') {
            raw.pop();
        }
    }
    raw
}

/// Formats a scalar value for logging with appropriate precision.
///
/// Uses different precision levels based on the magnitude of the value
/// to provide readable output while maintaining accuracy.
///
/// # Arguments
/// * `value` - The scalar value to format
///
/// # Returns
/// Formatted string representation
fn format_scalar_for_log(value: f64) -> String {
    let normalized = value.max(0.0);
    let raw = if normalized >= crate::constants::JXL_EXPLORE_PLATEAU_LIMIT {
        format!("{normalized:.8}")
    } else if normalized < crate::constants::JXL_DISTANCE_PLATEAU {
        format!("{normalized:.6}")
    } else if normalized < crate::constants::JXL_EXPLORE_FLOOR_LIMIT {
        format!("{normalized:.4}")
    } else {
        format!("{normalized:.3}")
    };

    trim_decimal_string(raw)
}

#[must_use]
pub fn format_distance_for_log(distance: f32) -> String {
    format_scalar_for_log(f64::from(clamp_explore_distance(distance)))
}

fn size_ratio(size: u64, input_size: u64) -> f64 {
    if input_size == 0 {
        return f64::NAN;
    }
    #[cfg(feature = "high-precision")]
    {
        (Rational::from(size) / Rational::from(input_size)).to_f64()
    }
    #[cfg(not(feature = "high-precision"))]
    {
        crate::numeric_cast::u64_to_f64(size) / crate::numeric_cast::u64_to_f64(input_size)
    }
}

fn size_ratio_pct(size: u64, input_size: u64) -> f64 {
    size_ratio(size, input_size) * 100.0
}

fn improvement_ratio(previous_size: u64, current_size: u64, input_size: u64) -> f64 {
    if input_size == 0 {
        return f64::NAN;
    }
    if current_size >= previous_size {
        0.0
    } else {
        #[cfg(feature = "high-precision")]
        {
            (Rational::from(previous_size - current_size) / Rational::from(input_size)).to_f64()
        }
        #[cfg(not(feature = "high-precision"))]
        {
            crate::numeric_cast::u64_to_f64(previous_size - current_size)
                / crate::numeric_cast::u64_to_f64(input_size)
        }
    }
}

fn exploration_profile(initial_ratio: f64) -> JxlExplorationProfile {
    let pressure_stops = oversize_pressure_stops(initial_ratio);

    if pressure_stops <= JXL_MICRO_PRESSURE_STOPS_MAX {
        JxlExplorationProfile::MicroAdjust
    } else if pressure_stops <= JXL_BOUNDARY_PRESSURE_STOPS_MAX {
        JxlExplorationProfile::BoundaryPush
    } else if pressure_stops <= JXL_WIDE_PRESSURE_STOPS_MAX {
        JxlExplorationProfile::WidePush
    } else {
        JxlExplorationProfile::CeilingSweep
    }
}

fn candidate_region_key(distance: f32) -> i32 {
    let floor_log = f64::from(JXL_EXPLORE_FLOOR).log10();
    let ceiling_log = f64::from(JXL_EXPLORE_CEILING).log10();
    let distance_log = f64::from(clamp_explore_distance(distance)).log10();
    let span = (ceiling_log - floor_log).max(f64::EPSILON);
    let normalized = ((distance_log - floor_log) / span).clamp(0.0, 0.999_999);

    match crate::numeric_cast::f64_to_i32_strict(
        (normalized * JXL_REGION_BUCKET_COUNT).floor(),
        "region_bucket",
    ) {
        Some(v) => v,
        None => unreachable!(
            "CRITICAL: region bucket ({}) is outside i32 range in jxl_explorer \
             candidate_region_key (distance={})",
            normalized * JXL_REGION_BUCKET_COUNT,
            distance
        ),
    }
}

fn canonicalize_generated_distance(distance: f64) -> Result<f32, String> {
    if !distance.is_finite() {
        return Err("adaptive JXL exploration generated a non-finite distance".to_string());
    }

    let floor = f64::from(JXL_EXPLORE_FLOOR);
    if distance + f64::EPSILON < floor {
        return Err(format!(
            "adaptive JXL exploration generated d={distance:.9} below the floor d={}",
            format_distance_for_log(JXL_EXPLORE_FLOOR)
        ));
    }

    let clamped = distance.clamp(floor, f64::from(JXL_EXPLORE_CEILING));
    let mut as_f32 = crate::numeric_cast::f64_to_f32_lossy(clamped);
    if as_f32 < JXL_EXPLORE_FLOOR {
        as_f32 = JXL_EXPLORE_FLOOR;
    }
    if as_f32 >= 1.0 {
        as_f32 = JXL_EXPLORE_CEILING;
    }

    Ok(as_f32)
}

fn normalize_ratio_band(value: f64, start: f64, end: f64) -> f64 {
    let span = (end - start).max(f64::EPSILON);
    ((value - start) / span).clamp(0.0, 1.0)
}

fn smoothstep01(value: f64) -> f64 {
    let clamped = value.clamp(0.0, 1.0);
    clamped * clamped * 2.0f64.mul_add(-clamped, 3.0)
}

/// Interpolate in **log10-distance space** (plateau tier only).
///
/// Used for the `MicroAdjust` profile where distances are sub-PLATEAU. In this
/// range, linear steps would collapse to identical f32 values, so log-space
/// preserves resolution across the near-lossless plateau. Smoothstep easing
/// prevents clustering at band edges.
fn interpolate_plateau_distance(
    min_distance: f64,
    max_distance: f64,
    normalized: f64,
) -> Result<f32, String> {
    let t = smoothstep01(normalized);
    let min_log = min_distance.log10();
    let max_log = max_distance.log10();
    canonicalize_generated_distance(10f64.powf((max_log - min_log).mul_add(t, min_log)))
}

/// Interpolate in **linear distance space** (perceptual tiers).
///
/// Used for `BoundaryPush`, `WidePush`, and `CeilingSweep` profiles. In the
/// d=PLATEAU..1.0 range, equal Δdistance ≈ equal ΔJND, so linear spacing tracks
/// perceptual quality steps. Smoothstep easing concentrates probes near the
/// band center where the quality/size trade-off is steepest.
fn interpolate_perceptual_distance(
    min_distance: f64,
    max_distance: f64,
    normalized: f64,
) -> Result<f32, String> {
    let t = smoothstep01(normalized);
    canonicalize_generated_distance((max_distance - min_distance).mul_add(t, min_distance))
}

fn profile_distance_range(profile: JxlExplorationProfile) -> (f64, f64) {
    match profile {
        JxlExplorationProfile::MicroAdjust => (
            f64::from(JXL_EXPLORE_FLOOR),
            JXL_DISTANCE_CEILING_PLATEAU_MAX,
        ),
        JxlExplorationProfile::BoundaryPush => (
            JXL_DISTANCE_CEILING_PLATEAU_MAX,
            JXL_DISTANCE_VISUAL_LOSSLESS_MAX,
        ),
        JxlExplorationProfile::WidePush => {
            (JXL_DISTANCE_VISUAL_LOSSLESS_MAX, JXL_DISTANCE_BALANCED_MAX)
        }
        JxlExplorationProfile::CeilingSweep => {
            (JXL_DISTANCE_BALANCED_MAX, f64::from(JXL_EXPLORE_CEILING))
        }
    }
}

/// Fixed anchor distances for each profile tier.
///
/// These are mandatory probe points during Phase 1 ladder construction. They
/// ensure the search always samples at known perceptual boundaries, regardless
/// of the adaptive interpolation budget.
///
/// **Overfitting risk**: The anchors are hand-picked for typical photographic
/// content. Image distributions that cluster heavily around specific
/// compression ratios may "get stuck" in dense anchor regions. Monitor the
/// telemetry fields (`initial_ratio`, `pressure_stops`, `profile`,
/// `target_distance`) logged by the screening pass to detect anchor regions
/// that consistently fail to produce break-even candidates.
const fn profile_anchor_distances(profile: JxlExplorationProfile) -> &'static [f64] {
    match profile {
        JxlExplorationProfile::MicroAdjust => &[JXL_DISTANCE_CEILING_PLATEAU_MAX],
        JxlExplorationProfile::BoundaryPush => &[
            JXL_DISTANCE_CEILING_PLATEAU_MAX,
            crate::constants::JXL_ANCHOR_DIST_0_03,
            crate::constants::JXL_ANCHOR_DIST_0_06,
            JXL_DISTANCE_VISUAL_LOSSLESS_MAX,
        ],
        JxlExplorationProfile::WidePush => &[
            JXL_DISTANCE_CEILING_PLATEAU_MAX,
            JXL_DISTANCE_VISUAL_LOSSLESS_MAX,
            crate::constants::JXL_ANCHOR_DIST_0_15,
            crate::constants::JXL_ANCHOR_DIST_0_20,
            JXL_DISTANCE_BALANCED_MAX,
        ],
        JxlExplorationProfile::CeilingSweep => &[
            JXL_DISTANCE_CEILING_PLATEAU_MAX,
            JXL_DISTANCE_VISUAL_LOSSLESS_MAX,
            JXL_DISTANCE_BALANCED_MAX,
            crate::constants::JXL_ANCHOR_DIST_0_50,
            crate::constants::JXL_ANCHOR_DIST_0_75,
        ],
    }
}

fn target_distance_for_ratio(
    initial_ratio: f64,
    profile: JxlExplorationProfile,
) -> Result<f32, String> {
    let pressure_stops = oversize_pressure_stops(initial_ratio);
    match profile {
        JxlExplorationProfile::MicroAdjust => interpolate_plateau_distance(
            f64::from(JXL_EXPLORE_FLOOR),
            JXL_DISTANCE_CEILING_PLATEAU_MAX,
            normalize_ratio_band(pressure_stops, 0.0, JXL_MICRO_PRESSURE_STOPS_MAX),
        ),
        JxlExplorationProfile::BoundaryPush => interpolate_perceptual_distance(
            JXL_DISTANCE_CEILING_PLATEAU_MAX,
            JXL_DISTANCE_VISUAL_LOSSLESS_MAX,
            normalize_ratio_band(
                pressure_stops,
                JXL_MICRO_PRESSURE_STOPS_MAX,
                JXL_BOUNDARY_PRESSURE_STOPS_MAX,
            ),
        ),
        JxlExplorationProfile::WidePush => interpolate_perceptual_distance(
            JXL_DISTANCE_VISUAL_LOSSLESS_MAX,
            JXL_DISTANCE_BALANCED_MAX,
            normalize_ratio_band(
                pressure_stops,
                JXL_BOUNDARY_PRESSURE_STOPS_MAX,
                JXL_WIDE_PRESSURE_STOPS_MAX,
            ),
        ),
        JxlExplorationProfile::CeilingSweep => {
            let excess_pressure = (pressure_stops - JXL_WIDE_PRESSURE_STOPS_MAX).max(0.0);
            let normalized = excess_pressure / (excess_pressure + 1.0);
            interpolate_perceptual_distance(
                JXL_DISTANCE_BALANCED_MAX,
                f64::from(JXL_EXPLORE_CEILING),
                normalized,
            )
        }
    }
}

fn build_adaptive_ladder(
    profile: JxlExplorationProfile,
    target_distance: f32,
    probe_count: usize,
) -> Result<Vec<f32>, String> {
    if target_distance <= JXL_EXPLORE_FLOOR + f32::EPSILON {
        return Ok(Vec::new());
    }

    let mut ladder = Vec::new();
    let mut seen = HashSet::new();
    let target_distance_f64 = f64::from(target_distance);

    for &anchor in profile_anchor_distances(profile) {
        if anchor <= f64::from(JXL_EXPLORE_FLOOR) {
            continue;
        }
        if anchor > target_distance_f64 + f64::EPSILON {
            continue;
        }

        let candidate = canonicalize_generated_distance(anchor)?;
        if candidate > JXL_EXPLORE_FLOOR + f32::EPSILON && seen.insert(distance_key(candidate)) {
            ladder.push(candidate);
        }
    }

    let interpolation_budget = probe_count.saturating_sub(ladder.len()).max(1);
    let (band_min, _) = profile_distance_range(profile);
    let interpolation_start = band_min.min(target_distance_f64);

    for probe_idx in 1..=interpolation_budget {
        let progress = crate::numeric_cast::usize_to_f64(probe_idx)
            / crate::numeric_cast::usize_to_f64(interpolation_budget);
        let candidate = if profile == JxlExplorationProfile::MicroAdjust {
            interpolate_plateau_distance(interpolation_start, target_distance_f64, progress)?
        } else {
            interpolate_perceptual_distance(interpolation_start, target_distance_f64, progress)?
        };
        if candidate > JXL_EXPLORE_FLOOR + f32::EPSILON && seen.insert(distance_key(candidate)) {
            ladder.push(candidate);
        }
    }

    ladder.sort_by(f32::total_cmp);
    Ok(ladder)
}

fn build_exploration_plan(
    input_size: u64,
    initial_size: u64,
) -> Result<JxlExplorationPlan, String> {
    let initial_ratio = size_ratio(initial_size, input_size);
    let profile = exploration_profile(initial_ratio);
    let target_distance = target_distance_for_ratio(initial_ratio, profile)?;
    let distance_span = (f64::from(target_distance) / f64::from(JXL_EXPLORE_FLOOR))
        .max(1.0)
        .log2();
    let (min_probes, max_probes) = match profile {
        JxlExplorationProfile::MicroAdjust => (
            crate::constants::JXL_PROBE_COUNT_MIN_MICRO,
            crate::constants::JXL_PROBE_COUNT_MAX_MICRO,
        ),
        JxlExplorationProfile::BoundaryPush => (
            crate::constants::JXL_PROBE_COUNT_MIN_BOUNDARY,
            crate::constants::JXL_PROBE_COUNT_MAX_BOUNDARY,
        ),
        JxlExplorationProfile::WidePush => (
            crate::constants::JXL_PROBE_COUNT_MIN_WIDE,
            crate::constants::JXL_PROBE_COUNT_MAX_WIDE,
        ),
        JxlExplorationProfile::CeilingSweep => (
            crate::constants::JXL_PROBE_COUNT_MIN_CEILING,
            crate::constants::JXL_PROBE_COUNT_MAX_CEILING,
        ),
    };
    let probe_count_base =
        crate::numeric_cast::f64_to_usize_strict(distance_span.ceil(), "jxl_probe_count")
            .ok_or_else(|| {
                "adaptive JXL exploration generated an invalid probe count".to_string()
            })?;
    let probe_count = probe_count_base
        .checked_add(crate::constants::JXL_PROBE_COUNT_BONUS)
        .ok_or_else(|| "adaptive JXL exploration probe count overflowed".to_string())?
        .clamp(min_probes, max_probes);
    let ladder = build_adaptive_ladder(profile, target_distance, probe_count)?;

    Ok(JxlExplorationPlan {
        profile,
        target_distance,
        ladder,
    })
}

fn near_best_margin_f64(input_size: u64) -> Option<u64> {
    crate::media_conversion_gate::delivery_jxl_margin_u64_or_one(
        crate::numeric_cast::f64_to_u64_strict(
            crate::numeric_cast::u64_to_f64(input_size) * JXL_NEAR_BEST_MARGIN_RATIO,
            "jxl_margin",
        ),
        format!(
            "NUMERIC ANOMALY: JXL near-best margin overflow/NaN (input_size={input_size}, \
             ratio={JXL_NEAR_BEST_MARGIN_RATIO})"
        ),
    )
}

fn near_best_margin(input_size: u64) -> Option<u64> {
    #[cfg(feature = "high-precision")]
    {
        let Some(ratio) = crate::numeric_cast::f64_to_rational_strict(
            JXL_NEAR_BEST_MARGIN_RATIO,
            "JXL_NEAR_BEST_MARGIN_RATIO",
        ) else {
            crate::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                "jxl_near_best_margin",
                "NUMERIC ANOMALY: JXL_NEAR_BEST_MARGIN_RATIO non-finite; using f64 margin path",
            );
            return near_best_margin_f64(input_size);
        };
        let margin = Rational::from(input_size) * ratio;
        crate::media_conversion_gate::delivery_jxl_margin_u64_or_one(
            crate::numeric_cast::f64_to_u64_strict(margin.to_f64(), "margin"),
            format!(
                "NUMERIC ANOMALY: failed to convert JXL margin to u64 (input_size={input_size}, \
                 margin_f64={})",
                margin.to_f64()
            ),
        )
    }
    #[cfg(not(feature = "high-precision"))]
    {
        near_best_margin_f64(input_size)
    }
}

fn near_best(size: u64, best_size: u64, input_size: u64) -> bool {
    // margin=None means the cast anomaly was already logged by the gate;
    // conservatively treat it as zero (no tolerance window) rather than
    // silently fabricating a default via unwrap_or.
    let threshold = match near_best_margin(input_size) {
        Some(margin) => best_size.saturating_add(margin),
        None => best_size,
    };
    size <= threshold
}

fn near_boundary(size: u64, input_size: u64) -> bool {
    let ratio = size_ratio(size, input_size);
    (JXL_BOUNDARY_LOW_RATIO..=JXL_BOUNDARY_HIGH_RATIO).contains(&ratio)
}

fn add_reason(
    candidates: &mut [JxlScreenedCandidate],
    idx: usize,
    reason: JxlPromotionReason,
    log: &mut Vec<String>,
) {
    let Some(candidate) = candidates.get_mut(idx) else {
        return;
    };
    if candidate.has_reason(reason) {
        return;
    }

    candidate.reasons.push(reason);
    log.push(format!(
        "   ✓ JXL Shortlist promotion: distance d={} qualified as '{}' candidate for \
         same-domain verification",
        format_distance_for_log(candidate.distance),
        reason.label()
    ));
}

fn shortlist_finalists(
    candidates: &[JxlScreenedCandidate],
    best_idx: usize,
    input_size: u64,
    size_policy: crate::exploration_policy::SizePolicy,
) -> Vec<JxlScreenedCandidate> {
    // Tier 1: below-source candidates (output < input), sorted by ascending d.
    // These are the only candidates that can produce a net saving. Highest quality
    // (lowest d) first so verification has the best chance of confirming the
    // highest-quality valid winner.
    let mut below_source: Vec<_> = candidates
        .iter()
        .filter(|c| size_policy.fits(c.output_size, input_size))
        .collect();
    below_source.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.output_size.cmp(&b.output_size))
    });

    // Tier 2: near-boundary oversize candidates (100–105% of input), sorted by
    // ascending d. These sit just above break-even and are retained only as
    // boundary evidence in the same encoder-effort domain.
    let mut near_boundary_cands: Vec<_> = candidates
        .iter()
        .filter(|c| {
            !size_policy.fits(c.output_size, input_size) && near_boundary(c.output_size, input_size)
        })
        .collect();
    near_boundary_cands.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.output_size.cmp(&b.output_size))
    });

    // Tier 3: everything else with a promotion reason (oversize but promoted),
    // sorted by promotion score descending then ascending d.
    let mut promoted_oversize: Vec<_> = candidates
        .iter()
        .filter(|c| {
            !size_policy.fits(c.output_size, input_size)
                && !near_boundary(c.output_size, input_size)
                && !c.reasons.is_empty()
        })
        .collect();
    promoted_oversize.sort_by(|a, b| {
        b.promotion_score()
            .cmp(&a.promotion_score())
            .then_with(|| a.distance.total_cmp(&b.distance))
    });

    let mut finalists = Vec::new();
    let mut selected = HashSet::new();

    // Always include the best known below-source candidate first (guaranteed slot).
    if let Some(best) = candidates.get(best_idx) {
        include_finalist(&mut finalists, &mut selected, best);
    }

    // Fill remaining slots: tier 1 → tier 2 → tier 3.
    for tier in [
        &below_source[..],
        &near_boundary_cands[..],
        &promoted_oversize[..],
    ] {
        for candidate in tier {
            if finalists.len() >= JXL_FINALIST_LIMIT {
                break;
            }
            include_finalist(&mut finalists, &mut selected, candidate);
        }
        if finalists.len() >= JXL_FINALIST_LIMIT {
            break;
        }
    }

    // Final order: ascending d (lowest = highest quality first).
    finalists.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.output_size.cmp(&b.output_size))
    });
    finalists
}

fn include_finalist(
    finalists: &mut Vec<JxlScreenedCandidate>,
    selected: &mut HashSet<DistanceKey>,
    candidate: &JxlScreenedCandidate,
) {
    if selected.insert(distance_key(candidate.distance)) {
        finalists.push(candidate.clone());
    }
}

fn candidate_reason_summary(candidate: &JxlScreenedCandidate, input_size: u64) -> String {
    let reasons = candidate
        .reasons
        .iter()
        .map(|reason| reason.label())
        .collect::<Vec<_>>()
        .join("+");
    let stage = if candidate.ladder_phase {
        "screen"
    } else {
        "refine"
    };
    format!(
        "d={} ({stage}, {:.1}% of input, {reasons})",
        format_distance_for_log(candidate.distance),
        size_ratio_pct(candidate.output_size, input_size)
    )
}

/// Finalizes screening results by shortlisting top finalist candidates.
///
/// Uses promotion scoring, boundary detection, and quality considerations to
/// select finalists. The finalists are then re-evaluated in the same encoder
/// effort domain for ultimate mode.
///
/// Term definitions (see `candidate_comparator` for terminology):
/// - **Finalists**: Selected subset from screened candidates for final
///   evaluation
/// - **Promotion**: Reason(s) a candidate was included in finalist set
fn finalize_screening_result(
    input_size: u64,
    size_policy: crate::exploration_policy::SizePolicy,
    candidates: Vec<JxlScreenedCandidate>,
    best_idx: usize,
    iterations: u32,
    mut log: Vec<String>,
    initial_ratio: f64,
    pressure_stops: f64,
    profile_label: &'static str,
    target_distance: f32,
) -> JxlScreeningResult {
    let finalists = shortlist_finalists(&candidates, best_idx, input_size, size_policy);
    let finalist_summary = finalists
        .iter()
        .map(|candidate| candidate_reason_summary(candidate, input_size))
        .collect::<Vec<_>>()
        .join(", ");
    log.push(format!(
        "Tailored same-domain shortlist ({}): {finalist_summary}",
        finalists.len()
    ));

    // Structured telemetry for data-driven calibration.
    // Collect these lines to fit band boundaries statistically rather than
    // manually.
    let Some(best) = candidates.get(best_idx) else {
        let _ = crate::media_conversion_gate::jxl_best_telemetry_optional(
            None,
            None,
            "jxl_explorer screening telemetry: missing best_idx",
        );
        log.push(format!(
            "ERROR: best_idx {best_idx} out of range ({} candidates); refusing fabricated \
             screening winner",
            candidates.len()
        ));
        // Route through the same seal as the normal path so the NaN/u64::MAX sentinels
        // get the standard seal-time audit instead of leaving unsealed.
        return JxlScreeningResult {
            best_distance: f32::NAN,
            best_output_size: u64::MAX,
            iterations,
            screened_candidates: candidates,
            finalists,
            log,
            initial_ratio,
            pressure_stops,
            profile_label,
            target_distance,
        }
        .sealed();
    };
    let best_dist = best.distance;
    let best_size = best.output_size;
    if !best_dist.is_finite() {
        let _ = crate::media_conversion_gate::jxl_best_telemetry_optional(
            Some(best.distance),
            Some(best.output_size),
            "jxl_explorer screening telemetry: non-finite best distance",
        );
    }

    log.push(format!(
        "TELEMETRY: initial_ratio={initial_ratio:.6} pressure_stops={pressure_stops:.4} \
         profile={profile_label} target_distance={} best_distance={} best_pct={} \
         iterations={iterations} finalists={}",
        format_distance_for_log(target_distance),
        if best_dist.is_finite() {
            format_distance_for_log(best_dist)
        } else {
            "N/A".to_string()
        },
        if best_dist.is_finite() && best_size > 0 {
            format!("{:.1}", size_ratio_pct(best_size, input_size))
        } else {
            "N/A".to_string()
        },
        finalists.len()
    ));

    let result = JxlScreeningResult {
        best_distance: best_dist,
        best_output_size: best_size,
        iterations,
        screened_candidates: candidates,
        finalists,
        log,
        initial_ratio,
        pressure_stops,
        profile_label,
        target_distance,
    };
    result.sealed()
}

struct JxlScreeningSession {
    input_size: u64,
    size_policy: crate::exploration_policy::SizePolicy,
    log: Vec<String>,
    iterations: u32,
    tested: HashSet<DistanceKey>,
    candidates: Vec<JxlScreenedCandidate>,
    region_keys: HashSet<i32>,
    oversize_best_idx: usize,
    pending_adjacent_promotion: bool,
    d_over: Option<f32>,
    d_under: Option<f32>,
    best_below_idx: Option<usize>,
}

impl JxlScreeningSession {
    fn fits(&self, size: u64) -> bool {
        self.size_policy.fits(size, self.input_size)
    }

    fn new(
        input_size: u64,
        size_policy: crate::exploration_policy::SizePolicy,
        initial_distance: f32,
        initial_size: u64,
    ) -> Self {
        let mut log = Vec::new();
        let mut candidates = vec![JxlScreenedCandidate {
            distance: initial_distance,
            output_size: initial_size,
            ladder_phase: true,
            reasons: Vec::new(),
        }];
        add_reason(&mut candidates, 0, JxlPromotionReason::Baseline, &mut log);
        add_reason(&mut candidates, 0, JxlPromotionReason::NewRegion, &mut log);

        let mut tested = HashSet::new();
        tested.insert(distance_key(initial_distance));
        let mut region_keys = HashSet::new();
        region_keys.insert(candidate_region_key(initial_distance));

        Self {
            input_size,
            size_policy,
            log,
            iterations: 1,
            tested,
            candidates,
            region_keys,
            oversize_best_idx: 0,
            pending_adjacent_promotion: false,
            d_over: Some(initial_distance),
            d_under: None,
            best_below_idx: None,
        }
    }

    fn previous_size(&self, fallback: u64) -> u64 {
        crate::media_conversion_gate::jxl_previous_candidate_size_or_fallback(
            self.candidates
                .last()
                .map(|candidate| candidate.output_size),
            fallback,
            "jxl_explorer::previous_size",
        )
    }

    fn push_candidate(&mut self, distance: f32, output_size: u64, ladder_phase: bool) -> usize {
        self.candidates.push(JxlScreenedCandidate {
            distance,
            output_size,
            ladder_phase,
            reasons: Vec::new(),
        });
        let idx = self.candidates.len() - 1;

        if self.pending_adjacent_promotion {
            add_reason(
                &mut self.candidates,
                idx,
                JxlPromotionReason::AdjacentToBest,
                &mut self.log,
            );
            self.pending_adjacent_promotion = false;
        }
        if self.region_keys.insert(candidate_region_key(distance)) {
            add_reason(
                &mut self.candidates,
                idx,
                JxlPromotionReason::NewRegion,
                &mut self.log,
            );
        }
        if near_boundary(output_size, self.input_size) {
            add_reason(
                &mut self.candidates,
                idx,
                JxlPromotionReason::BoundaryRegion,
                &mut self.log,
            );
        }

        idx
    }

    fn note_phase1_outcome(&mut self, idx: usize, distance: f32, output_size: u64) {
        if self.fits(output_size) {
            if self.d_under.is_none() {
                self.d_under = Some(distance);
            }
            if self.best_below_idx.is_none() {
                self.best_below_idx = Some(idx);
                add_reason(
                    &mut self.candidates,
                    idx,
                    JxlPromotionReason::BetterThanCurrentBest,
                    &mut self.log,
                );
                self.pending_adjacent_promotion = true;
            }
            return;
        }

        let tighter_than_current = self.d_over.is_none_or(|lo| distance > lo);
        let below_d_under = self.d_under.is_none_or(|hi| distance < hi);
        if tighter_than_current && below_d_under {
            self.d_over = Some(distance);
        }
        let oversize_best_size = crate::media_conversion_gate::jxl_screened_output_size_or_max(
            self.candidates
                .get(self.oversize_best_idx)
                .map(|candidate| candidate.output_size),
            "jxl_explorer oversize candidate",
        );
        if output_size < oversize_best_size {
            if idx > 0 {
                add_reason(
                    &mut self.candidates,
                    idx - 1,
                    JxlPromotionReason::AdjacentToBest,
                    &mut self.log,
                );
            }
            add_reason(
                &mut self.candidates,
                idx,
                JxlPromotionReason::BetterThanCurrentBest,
                &mut self.log,
            );
            self.pending_adjacent_promotion = true;
            self.oversize_best_idx = idx;
        } else if near_best(output_size, oversize_best_size, self.input_size) {
            add_reason(
                &mut self.candidates,
                idx,
                JxlPromotionReason::NearCurrentBest,
                &mut self.log,
            );
        }
    }

    fn note_discovery_outcome(&mut self, idx: usize, distance: f32, output_size: u64) -> bool {
        if self.fits(output_size) {
            self.d_under = Some(distance);
            self.best_below_idx = Some(idx);
            add_reason(
                &mut self.candidates,
                idx,
                JxlPromotionReason::BetterThanCurrentBest,
                &mut self.log,
            );
            true
        } else {
            self.d_over = Some(distance);
            false
        }
    }

    fn note_refinement_outcome(
        &mut self,
        idx: usize,
        distance: f32,
        output_size: u64,
        lo: &mut f32,
        hi: &mut f32,
    ) {
        if self.fits(output_size) {
            *hi = distance;
            if self.best_below_idx.is_none_or(|best_idx| {
                self.candidates
                    .get(best_idx)
                    .is_none_or(|c| distance < c.distance)
            }) {
                self.best_below_idx = Some(idx);
                add_reason(
                    &mut self.candidates,
                    idx,
                    JxlPromotionReason::BetterThanCurrentBest,
                    &mut self.log,
                );
            }
        } else {
            *lo = distance;
        }
    }
}

/// Screens JXL distance candidates to identify finalists for same-effort
/// ultimate verification.
///
/// ## Terminology (unified with HEVC/other explorers)
/// - **Screening phase**: Initial exploration of distance values (Phase 1
///   ladder, Phase 2 binary)
/// - **Candidate**: A specific distance value with its output size
/// - **Finalist shortlist**: Curated subset of candidates promoted for ultimate
///   finalization
/// - **Winner**: Chosen by the caller based on finalists (not this function's
///   responsibility)
///
/// See `candidate_comparator` module for unified ranking terminology and
/// philosophy.
///
/// # Errors
/// Returns `Err` if the `try_candidate` closure fails for any tested distance.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
pub fn screen_jxl_candidates<F>(
    input_size: u64,
    initial_size: u64,
    size_policy: crate::exploration_policy::SizePolicy,
    mut try_candidate: F,
) -> Result<Option<JxlScreeningResult>, String>
where
    F: FnMut(f32) -> Result<u64, String>,
{
    if input_size == 0 {
        return Ok(None);
    }

    let initial_distance = clamp_explore_distance(JXL_EXPLORE_FLOOR);
    let mut session =
        JxlScreeningSession::new(input_size, size_policy, initial_distance, initial_size);

    // Condition A (Hard Constraint): If d=0.001 is already safe (<= 100% size),
    // stop exploring. Quality is already safe and beneficial, so further
    // exploration cost is not worth it.
    let ratio = size_ratio(initial_size, input_size);
    if size_policy.fits(initial_size, input_size) {
        session.log.push(format!(
            "Early exit: the required floor d={} is already safe ({:.1}% of input)",
            format_distance_for_log(initial_distance),
            ratio * 100.0_f64
        ));
        return Ok(Some(finalize_screening_result(
            input_size,
            size_policy,
            session.candidates,
            0,
            session.iterations,
            session.log,
            ratio,
            0.0,
            "early-exit",
            JXL_EXPLORE_FLOOR,
        )));
    }

    let plan = build_exploration_plan(input_size, initial_size)?;
    let pressure_stops = oversize_pressure_stops(ratio);
    let (band_min, band_max) = profile_distance_range(plan.profile);
    session.log.push(format!(
        "Adaptive plan ({}, +{pressure_stops:.2} stops) for this file: baseline d={} is {:.1}% of \
         input, phase 1 will probe {} perceptual-band distances in d={}..{} up to d={}",
        plan.profile.label(),
        format_distance_for_log(initial_distance),
        size_ratio_pct(initial_size, input_size),
        plan.ladder.len(),
        format_scalar_for_log(band_min),
        format_scalar_for_log(band_max),
        format_distance_for_log(plan.target_distance)
    ));
    session.log.push(format!(
        "Phase 0 baseline: d={} -> {:.1}% of input",
        format_distance_for_log(initial_distance),
        size_ratio_pct(initial_size, input_size)
    ));

    if near_boundary(initial_size, input_size) {
        add_reason(
            &mut session.candidates,
            0,
            JxlPromotionReason::BoundaryRegion,
            &mut session.log,
        );
    }

    for (probe_idx, &candidate_distance) in plan.ladder.iter().enumerate() {
        if session.iterations >= JXL_EXPLORE_MAX_ITERATIONS {
            break;
        }

        let candidate_distance = clamp_explore_distance(candidate_distance);
        if candidate_distance + f32::EPSILON < JXL_EXPLORE_FLOOR {
            return Err(format!(
                "adaptive JXL exploration produced d={} below the floor d={}",
                format_distance_for_log(candidate_distance),
                format_distance_for_log(JXL_EXPLORE_FLOOR)
            ));
        }
        if !session.tested.insert(distance_key(candidate_distance)) {
            continue;
        }

        let previous_size = session.previous_size(initial_size);
        let size = try_candidate(candidate_distance)?;
        session.iterations += 1;
        let delta_pct = improvement_ratio(previous_size, size, input_size) * 100.0_f64;
        let trend = if size < previous_size { "↓" } else { "→" };
        let status = if near_boundary(size, input_size) {
            "near break-even"
        } else if session.fits(size) {
            "below source"
        } else {
            "still oversize"
        };

        session.log.push(format!(
            "Phase 1 adaptive probe {}/{}: d={} -> {:.1}% of input ({trend} {delta_pct:.1}%, \
             {status})",
            probe_idx + 1,
            plan.ladder.len(),
            format_distance_for_log(candidate_distance),
            size_ratio_pct(size, input_size)
        ));

        let current_idx = session.push_candidate(candidate_distance, size, true);
        session.note_phase1_outcome(current_idx, candidate_distance, size);
    }

    // --- Phase 2: Find break-even bracket, then binary search ---
    //
    // Objective: find the lowest d where output < input (highest quality that still
    // compresses).
    //
    // If Phase 1 left d_under unset (break-even is beyond target_distance), probe
    // upward from d_over toward JXL_EXPLORE_CEILING until d_under is
    // discovered, consuming budget. Once [lo=d_over, hi=d_under] is
    // established, binary search narrows it to precision.
    //
    // If no d_under is ever found, return None (skip JXL — nothing compresses below
    // source).
    if session.d_under.is_none()
        && let Some(start) = session.d_over
    {
        // Discovery: probe upward with exponentially growing steps until d_under found
        let precision = JXL_EXPLORE_BINARY_SEARCH_PRECISION.max(f32::EPSILON);
        let mut probe = start;
        let mut step = (plan.target_distance - start).max(precision);

        session.log.push(format!(
            "Phase 2 discovery: extending from d={} toward ceiling (no d_under in Phase 1)",
            format_distance_for_log(start)
        ));

        while session.iterations < JXL_EXPLORE_MAX_ITERATIONS {
            let next = canonicalize_generated_distance(f64::from(probe) + f64::from(step))?;
            if next >= JXL_EXPLORE_CEILING || next <= probe + f32::EPSILON {
                break;
            }
            if session.tested.contains(&distance_key(next)) {
                probe = next;
                step = (step * 2.0).min(JXL_EXPLORE_CEILING - next);
                continue;
            }
            session.tested.insert(distance_key(next));

            let size = try_candidate(next)?;
            session.iterations += 1;

            let status = if near_boundary(size, input_size) {
                "near break-even"
            } else if session.fits(size) {
                "below source"
            } else {
                "still oversize"
            };

            session.log.push(format!(
                "Phase 2 discovery: d={} -> {:.1}% of input ({status})",
                format_distance_for_log(next),
                size_ratio_pct(size, input_size)
            ));

            let probe_idx = session.push_candidate(next, size, false);
            if session.note_discovery_outcome(probe_idx, next, size) {
                break; // hand off to binary search
            }
            probe = next;
            step = (step * 2.0).min(JXL_EXPLORE_CEILING - next);
        }
    }

    if let (Some(mut lo), Some(mut hi)) = (session.d_over, session.d_under) {
        let precision = JXL_EXPLORE_BINARY_SEARCH_PRECISION.max(f32::EPSILON);

        session.log.push(format!(
            "   🔍 JXL Phase 2 Refinement: Initializing adaptive binary search bracket \
             [lower={lo:.4}, upper={hi:.4}] with target precision {precision:.4}"
        ));

        while session.iterations < JXL_EXPLORE_MAX_ITERATIONS && hi - lo >= precision {
            let mid = canonicalize_generated_distance(f64::midpoint(f64::from(lo), f64::from(hi)))?;

            if session.tested.contains(&distance_key(mid))
                || mid <= lo + f32::EPSILON
                || mid >= hi - f32::EPSILON
            {
                break;
            }
            session.tested.insert(distance_key(mid));

            let size = try_candidate(mid)?;
            session.iterations += 1;

            let status = if near_boundary(size, input_size) {
                "NEAR BREAK-EVEN"
            } else if session.fits(size) {
                "SAFE (BELOW SOURCE)"
            } else {
                "STILL OVERSIZE"
            };

            session.log.push(format!(
                "      • Binary search iteration {}: probed d={} resulted in {:.1}% of source \
                 size ({status})",
                session.iterations,
                format_distance_for_log(mid),
                size_ratio_pct(size, input_size)
            ));

            let probe_idx = session.push_candidate(mid, size, false);
            session.note_refinement_outcome(probe_idx, mid, size, &mut lo, &mut hi);
        }
    }

    // Determine the best candidate index.
    // Priority: lowest d where output < input (best_below_idx).
    // If nothing ever beat the source, skip JXL entirely.
    let Some(final_best_idx) = session.best_below_idx else {
        session.log.push(format!(
            "No candidate beat source size ({input_size}B); skipping JXL"
        ));
        return Ok(None);
    };

    Ok(Some(finalize_screening_result(
        input_size,
        size_policy,
        session.candidates,
        final_best_idx,
        session.iterations,
        session.log,
        ratio,
        pressure_stops,
        plan.profile.label(),
        plan.target_distance,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screening_keeps_best_ladder_candidate() {
        let result = screen_jxl_candidates(
            100,
            120,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |distance| {
                if distance
                    <= crate::numeric_cast::f64_to_f32_lossy(crate::constants::JXL_DISTANCE_PLATEAU)
                {
                    Ok(90)
                } else {
                    Ok(110)
                }
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"))
        .unwrap_or_else(|| panic!("screening result should exist"));

        assert_eq!(result.best_output_size, 90);
        assert!(result.best_distance > JXL_EXPLORE_FLOOR);
        assert!(result.iterations >= 2);
        // Shortlist must contain at least one below-source candidate (output_size=90)
        assert!(
            result
                .finalists
                .iter()
                .any(|candidate| candidate.output_size == 90)
        );
        // All finalists must be below source (no oversize candidates when below-source
        // ones fill slots)
        let all_below = result
            .finalists
            .iter()
            .all(|candidate| candidate.output_size < 100);
        assert!(
            all_below,
            "expected all finalists to be below source when enough qualify, got {:?}",
            result
                .finalists
                .iter()
                .map(|c| (c.distance, c.output_size))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_screening_stays_bounded_below_ceiling() {
        // All probes always return 130 > input(100). No candidate beats the source.
        // With binary search, Phase 2 is skipped (no d_under). Result should be None.
        let result = screen_jxl_candidates(
            100,
            140,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |_distance| Ok(130),
        )
        .unwrap_or_else(|e| panic!("error: {e:?}"));

        assert!(
            result.is_none(),
            "expected None when all candidates are oversize, but got Some"
        );
    }

    #[test]
    fn test_screening_promotes_adjacent_and_boundary_candidates() {
        let result = screen_jxl_candidates(
            100,
            104,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |distance| {
                let size = if distance <= 0.002 {
                    99
                } else if distance
                    <= crate::numeric_cast::f64_to_f32_lossy(crate::constants::JXL_DISTANCE_PLATEAU)
                {
                    100
                } else if distance <= 0.03 {
                    98
                } else {
                    101
                };
                Ok(size)
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"))
        .unwrap_or_else(|| panic!("screening result should exist"));

        assert!(result.finalists.iter().any(|candidate| {
            candidate
                .reasons
                .contains(&JxlPromotionReason::BoundaryRegion)
        }));
        assert!(result.finalists.iter().any(|candidate| {
            candidate
                .reasons
                .contains(&JxlPromotionReason::AdjacentToBest)
        }));
    }

    #[test]
    fn test_screening_logs_deceleration_near_break_even() {
        // File at 1.5x oversize; break-even occurs at d > 0.1.
        // Phase 2 binary search should find a qualifying candidate below source.
        let result = screen_jxl_candidates(
            100,
            150,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |distance| {
                let size = if distance <= 0.1 {
                    105 // oversize: 105%
                } else {
                    98 // below source
                };
                Ok(size)
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"))
        .unwrap_or_else(|| panic!("screening result should exist"));

        // best_distance must be a below-source d
        assert!(result.best_output_size < 100);
        assert!(result.best_distance < 1.0);
        // Phase 2 binary search log should appear
        assert!(
            result
                .log
                .iter()
                .any(|line| line.contains("JXL Phase 2 Refinement")),
            "expected JXL Phase 2 Refinement log, got {:?}",
            result.log
        );
    }

    #[test]
    fn test_screening_early_exit_on_safe_initial_result() {
        // Condition A: d=0.001 is safe (90 <= 100), should exit immediately
        let mut calls = 0_i32;
        let result = screen_jxl_candidates(
            100,
            90,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |_distance| {
                calls += 1_i32;
                Ok(50) // Should never be called
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"))
        .unwrap_or_else(|| panic!("screening result should exist"));

        assert!((result.best_distance - JXL_EXPLORE_FLOOR).abs() < f32::EPSILON);
        assert_eq!(result.best_output_size, 90);
        assert_eq!(result.iterations, 1);
        assert_eq!(calls, 0_i32); // No further probes
        assert!(result.log.iter().any(|line| line.contains("Early exit")));
    }

    #[test]
    fn test_screening_never_retests_the_floor_distance() {
        let mut probed = Vec::new();

        let _result = screen_jxl_candidates(
            100,
            130,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |distance| {
                probed.push(distance);
                Ok(if distance < 0.02 { 120 } else { 95 })
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"))
        .unwrap_or_else(|| panic!("screening result should exist"));

        assert_ne!(probed.len(), 0);
        assert!(
            probed
                .iter()
                .all(|distance| *distance > JXL_EXPLORE_FLOOR + f32::EPSILON)
        );
    }

    #[test]
    fn test_screening_rejects_distances_below_the_floor() {
        let err = canonicalize_generated_distance(0.0009)
            .err()
            .unwrap_or_else(|| panic!("sub-floor values must fail"));
        assert!(err.contains("below the floor"));
    }

    #[test]
    fn test_target_distance_growth_is_bounded_by_profile_band() {
        let micro_ratio = 1.02_f64;
        let boundary_ratio = 1.35_f64;
        let wide_ratio = 2.0_f64;
        let ceiling_ratio = 10.0_f64;

        let micro_target = target_distance_for_ratio(micro_ratio, exploration_profile(micro_ratio))
            .unwrap_or_else(|_| panic!("failed to get target distance"));
        let boundary_target =
            target_distance_for_ratio(boundary_ratio, exploration_profile(boundary_ratio))
                .unwrap_or_else(|_| panic!("failed to get target distance"));
        let wide_target = target_distance_for_ratio(wide_ratio, exploration_profile(wide_ratio))
            .unwrap_or_else(|_| panic!("failed to get target distance"));
        let ceiling_target =
            target_distance_for_ratio(ceiling_ratio, exploration_profile(ceiling_ratio))
                .unwrap_or_else(|_| panic!("failed to get target distance"));

        assert!(micro_target > JXL_EXPLORE_FLOOR);
        {
            assert!(
                f64::from(micro_target)
                    <= JXL_DISTANCE_CEILING_PLATEAU_MAX + f64::from(f32::EPSILON)
            );
            assert!(boundary_target > micro_target);
            assert!(
                f64::from(boundary_target)
                    <= JXL_DISTANCE_VISUAL_LOSSLESS_MAX + f64::from(f32::EPSILON)
            );
            assert!(wide_target > boundary_target);
            assert!(f64::from(wide_target) <= JXL_DISTANCE_BALANCED_MAX + f64::from(f32::EPSILON));
        }
        assert!(ceiling_target > wide_target);
        assert!(ceiling_target < JXL_EXPLORE_CEILING);
    }

    #[test]
    fn test_ceiling_sweep_uses_denser_phase_one_ladder() {
        let micro_plan = build_exploration_plan(100, 102)
            .unwrap_or_else(|e| panic!("failed to build plan: {e:?}"));
        let ceiling_plan = build_exploration_plan(100, 600)
            .unwrap_or_else(|e| panic!("failed to build plan: {e:?}"));

        assert!(ceiling_plan.ladder.len() > micro_plan.ladder.len());
        assert!(ceiling_plan.target_distance > micro_plan.target_distance);
        assert!(ceiling_plan.ladder.last().is_some());
    }

    #[test]
    fn test_profile_boundaries_follow_oversize_pressure_calibration() {
        assert_eq!(
            exploration_profile(1.04),
            JxlExplorationProfile::MicroAdjust
        );
        assert_eq!(
            exploration_profile(1.10),
            JxlExplorationProfile::BoundaryPush
        );
        assert_eq!(exploration_profile(1.90), JxlExplorationProfile::WidePush);
        assert_eq!(
            exploration_profile(3.0),
            JxlExplorationProfile::CeilingSweep
        );
    }

    #[test]
    fn test_boundary_push_interpolates_in_perceptual_distance_space() {
        let midpoint_stops = f64::midpoint(
            JXL_MICRO_PRESSURE_STOPS_MAX,
            JXL_BOUNDARY_PRESSURE_STOPS_MAX,
        );
        let midpoint_ratio = midpoint_stops.exp2();
        let target = target_distance_for_ratio(midpoint_ratio, exploration_profile(midpoint_ratio))
            .unwrap_or_else(|_| panic!("failed to get target distance"));

        assert!(
            (target - 0.055).abs() < 0.01,
            "mid-band perceptual target should stay near linear JND midpoint, got {target}"
        );
        assert!(
            target > 0.03,
            "target should no longer follow log-distance interpolation"
        );
    }

    #[test]
    fn test_phase_two_respects_target_ceiling() {
        // Scenario: 1.19x oversize, every probe always oversize.
        // No d_under is ever found, so Phase 2 is skipped and result is None.
        let mut probed = Vec::new();
        let result = screen_jxl_candidates(
            1000,
            1190,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |distance| {
                probed.push(distance);
                Ok(1100) // always oversize
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"));

        assert!(
            result.is_none(),
            "expected None when no candidate beats source, but got Some"
        );
        // All probed distances must be < 1.0 (never reaches the hard ceiling)
        for &d in &probed {
            assert!(d < 1.0, "probed d={d} must be below d=1.0");
        }
    }

    #[test]
    fn test_phase_two_converges_early_on_break_even() {
        // Scenario: file starts oversize, break-even occurs around d=0.005.
        // Binary search should narrow the bracket and converge well below budget.
        let result = screen_jxl_candidates(
            1000,
            1200,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |distance| {
                let size = if distance <= 0.005 {
                    1050 // oversize
                } else {
                    950 // below source
                };
                Ok(size)
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"))
        .unwrap_or_else(|| panic!("screening result should exist"));

        // Should NOT exhaust the full 50-iteration budget
        assert!(
            result.iterations < JXL_EXPLORE_MAX_ITERATIONS,
            "expected early convergence but used {}/{} iterations",
            result.iterations,
            JXL_EXPLORE_MAX_ITERATIONS
        );
        // Binary search log should be present
        assert!(
            result
                .log
                .iter()
                .any(|line| line.contains("JXL Phase 2 Refinement")),
            "expected JXL Phase 2 Refinement log, got {:?}",
            result.log
        );
        // Result must be below source
        assert!(result.best_output_size < 1000);
    }

    #[test]
    fn test_phase_two_does_not_exhaust_budget_on_monotonic_improvement() {
        // Size decreases monotonically as d increases; break-even near d=0.1.
        // Binary search should converge without exhausting the 50-iteration budget.
        let result = screen_jxl_candidates(
            1000,
            1192,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |distance| {
                let size = crate::numeric_cast::f64_to_u64_sat(
                    f64::from(distance).mul_add(-2000.0, 1200.0).max(800.0),
                );
                Ok(size)
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"))
        .unwrap_or_else(|| panic!("screening result should exist"));

        // Must NOT exhaust the full budget
        assert!(
            result.iterations < JXL_EXPLORE_MAX_ITERATIONS,
            "should not exhaust budget but used {}/{} iterations",
            result.iterations,
            JXL_EXPLORE_MAX_ITERATIONS
        );
        // best_distance must be a below-source candidate
        assert!(
            result.best_output_size < 1000,
            "best_output_size={} should be below source (1000)",
            result.best_output_size
        );
        // best_distance must be the lowest qualifying d
        assert!(result.best_distance < 1.0);
    }

    #[test]
    fn test_no_winner_skips_jxl() {
        // All probes return oversize — no candidate ever beats the source.
        // Expected result is None, not a fallback to d=0.001.
        let result = screen_jxl_candidates(
            100,
            200,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |_distance| Ok(150),
        )
        .unwrap_or_else(|e| panic!("error: {e:?}"));

        assert!(
            result.is_none(),
            "expected None when all probes are oversize, got Some"
        );
    }

    #[test]
    fn test_phase_two_returns_lowest_qualifying_d() {
        // Known break-even: d <= 0.04 is oversize, d > 0.04 is below source.
        // Binary search should converge best_distance to <= 0.04 + precision.
        let result = screen_jxl_candidates(
            1000,
            1300,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |distance| {
                if distance <= 0.04 {
                    Ok(1100) // oversize
                } else {
                    Ok(990) // below source
                }
            },
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"))
        .unwrap_or_else(|| panic!("screening result should exist"));

        let precision = f64::from(JXL_EXPLORE_BINARY_SEARCH_PRECISION) * 2.0_f64;
        assert!(
            f64::from(result.best_distance) <= 0.04_f64 + precision,
            "best_distance={} should converge to <= 0.04 + precision ({})",
            result.best_distance,
            0.04_f64 + precision
        );
        assert!(result.best_output_size < 1000);
    }

    #[test]
    fn screen_jxl_candidates_zero_input_is_fail_closed() {
        let result = screen_jxl_candidates(
            0,
            100,
            crate::exploration_policy::SizePolicy::StrictlySmaller,
            |_| Ok(50),
        )
        .unwrap_or_else(|e| panic!("exploration failed: {e:?}"));
        assert!(
            result.is_none(),
            "zero input_size must not run screening with fabricated ratio defaults"
        );
    }
}

#[cfg(test)]
mod tolerance_tests {
    use super::*;

    #[test]
    fn test_tolerance_highest_quality_boundary_starts_early() {
        let source_pure = 10 * 1024 * 1024; // 10 MiB
        let policy = crate::exploration_policy::SizePolicy::AllowGrowth {
            max_extra_bytes: 512 * 1024,
        };

        let result =
            screen_jxl_candidates(source_pure, source_pure + 700 * 1024, policy, |distance| {
                if distance <= 0.001 {
                    Ok(source_pure + 700 * 1024)
                } else if distance <= 0.05 {
                    Ok(source_pure + 400 * 1024)
                } else if distance <= 0.10 {
                    Ok(source_pure + 100 * 1024)
                } else {
                    Ok(source_pure - 100 * 1024)
                }
            })
            .unwrap_or_else(|e| panic!("exploration failed: {:?}", e))
            .unwrap_or_else(|| panic!("screening result should exist"));

        // Highest quality fitting candidate starts at d0.05.
        // It should converge to d <= 0.05 + precision
        let precision = f64::from(JXL_EXPLORE_BINARY_SEARCH_PRECISION) * 2.0_f64;
        assert!(
            f64::from(result.best_distance) <= 0.05_f64 + precision,
            "best_distance={} should converge to <= 0.05 + precision ({})",
            result.best_distance,
            0.05_f64 + precision
        );
        // And the output size should fit the tolerance.
        assert!(
            policy.fits(result.best_output_size, source_pure),
            "best_output_size={} must fit policy",
            result.best_output_size
        );
    }
}
