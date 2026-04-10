//! JXL Distance Explorer for Ultimate Mode
//!
//! Two-phase screening algorithm to identify distance candidates for JXL e10 finalization:
//!
//! **Phase 1 (Ladder)**: Test predefined distances, promote candidates based on quality/region.
//! **Phase 2 (Binary Search)**: Refine promising regions with adaptive step sizing.
//!
//! ## Unified Selection Philosophy
//!
//! Finalist promotion uses consistent priorities (see `candidate_comparator` for theory):
//!
//! 1. **Quality Gates**: Output must compress (size < input)
//! 2. **Quality Metrics**: Best compression (lowest size) preferred
//! 3. **Boundary Detection**: Candidates near 95–105% of input size promoted
//! 4. **Region Coverage**: Promotes one candidate per distance region for diversity
//! 5. **Score-based Ranking**: Finalists ranked by promotion score, then size, then distance
//!
//! Terminology (unified with HEVC/VideoExplorer):
//! - **Screening**: Phase 1 ladder + Phase 2 binary search exploration
//! - **Candidate**: A specific distance value with its output size
//! - **Finalist shortlist**: Curated subset promoted for e10 finalization

use crate::constants::{
    JXL_EXPLORE_BINARY_SEARCH_PRECISION, JXL_EXPLORE_CEILING, JXL_EXPLORE_LADDER,
    JXL_EXPLORE_MAX_ITERATIONS,
};
use std::collections::HashSet;

const JXL_FINALIST_LIMIT: usize = 8;
const JXL_NEAR_BEST_MARGIN_RATIO: f64 = 0.01;
const JXL_BOUNDARY_LOW_RATIO: f64 = 0.95;
const JXL_BOUNDARY_HIGH_RATIO: f64 = 1.05;

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
    fn label(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::BetterThanCurrentBest => "better-than-best",
            Self::NearCurrentBest => "near-best",
            Self::BoundaryRegion => "boundary",
            Self::NewRegion => "new-region",
            Self::AdjacentToBest => "adjacent",
        }
    }

    fn weight(self) -> u32 {
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct JxlExploreResult {
    pub accepted_distance: f32,
    pub output_size: u64,
    pub iterations: u32,
    pub ladder_phase: bool,
    pub screened_best_distance: f32,
    pub screened_best_size: u64,
    pub promoted_distances: Vec<f32>,
    pub log: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpwardSearchCadence {
    Adaptive,
    Jogging,
    Paused,
    Normal,
}

fn clamp_explore_distance(distance: f32) -> f32 {
    distance.clamp(JXL_EXPLORE_LADDER[0], JXL_EXPLORE_CEILING)
}

fn distance_key(distance: f32) -> i32 {
    // SAFETY: Clamped distance is in range [0.001, 0.999], so * 1000.0 yields [1, 999].
    // After rounding, always fits safely in i32 without truncation. Used as HashSet key only.
    let rounded = (clamp_explore_distance(distance) * 1000.0).round();
    #[allow(clippy::cast_possible_truncation)]
    let key = rounded as i32;
    key
}

fn size_ratio(size: u64, input_size: u64) -> f64 {
    if input_size == 0 {
        1.0
    } else {
        crate::numeric_cast::u64_to_f64(size) / crate::numeric_cast::u64_to_f64(input_size)
    }
}

fn size_ratio_pct(size: u64, input_size: u64) -> f64 {
    size_ratio(size, input_size) * 100.0
}

fn improvement_ratio(previous_size: u64, current_size: u64, input_size: u64) -> f64 {
    if input_size == 0 || current_size >= previous_size {
        0.0
    } else {
        crate::numeric_cast::u64_to_f64(previous_size - current_size)
            / crate::numeric_cast::u64_to_f64(input_size)
    }
}

fn round_phase_two_distance(distance: f32) -> f32 {
    let precision = JXL_EXPLORE_BINARY_SEARCH_PRECISION.max(0.001);
    let rounded = (distance / precision).ceil() * precision;
    clamp_explore_distance((rounded * 1000.0).round() / 1000.0)
}

fn next_phase_two_candidate(
    current_distance: f32,
    current_step: f32,
    tested: &HashSet<i32>,
) -> Option<f32> {
    let rounded = round_phase_two_distance(current_distance + current_step);
    if rounded > current_distance + f32::EPSILON && !tested.contains(&distance_key(rounded)) {
        return Some(rounded);
    }

    let ceiling = clamp_explore_distance(JXL_EXPLORE_CEILING);
    if ceiling > current_distance + f32::EPSILON && !tested.contains(&distance_key(ceiling)) {
        return Some(ceiling);
    }

    None
}

fn candidate_region_key(distance: f32) -> i32 {
    if distance <= 0.01 + f32::EPSILON {
        0
    } else if distance <= 0.1 + f32::EPSILON {
        1
    } else if distance <= 0.25 + f32::EPSILON {
        2
    } else if distance <= 0.5 + f32::EPSILON {
        3
    } else if distance <= 0.75 + f32::EPSILON {
        4
    } else {
        5
    }
}

fn near_best_margin(input_size: u64) -> u64 {
    crate::numeric_cast::f64_to_u64_sat(
        crate::numeric_cast::u64_to_f64(input_size) * JXL_NEAR_BEST_MARGIN_RATIO,
    )
    .max(1)
}

fn near_best(size: u64, best_size: u64, input_size: u64) -> bool {
    size <= best_size.saturating_add(near_best_margin(input_size))
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
    if candidates[idx].has_reason(reason) {
        return;
    }

    candidates[idx].reasons.push(reason);
    log.push(format!(
        "Promoted d={:.3} for e10 finalization ({})",
        candidates[idx].distance,
        reason.label()
    ));
}

fn shortlist_finalists(
    candidates: &[JxlScreenedCandidate],
    best_idx: usize,
) -> Vec<JxlScreenedCandidate> {
    let mut finalists = Vec::new();
    let mut selected = HashSet::new();
    include_finalist(&mut finalists, &mut selected, &candidates[best_idx]);
    include_finalist(&mut finalists, &mut selected, &candidates[0]);

    let mut mandatory: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            candidate.has_reason(JxlPromotionReason::BoundaryRegion)
                || candidate.has_reason(JxlPromotionReason::AdjacentToBest)
        })
        .collect();
    mandatory.sort_by(|left, right| {
        left.output_size
            .cmp(&right.output_size)
            .then_with(|| left.distance.total_cmp(&right.distance))
    });
    for candidate in mandatory {
        include_finalist(&mut finalists, &mut selected, candidate);
    }

    let mut ranked: Vec<_> = candidates
        .iter()
        .filter(|candidate| !candidate.reasons.is_empty())
        .collect();
    ranked.sort_by(|left, right| {
        right
            .promotion_score()
            .cmp(&left.promotion_score())
            .then_with(|| left.output_size.cmp(&right.output_size))
            .then_with(|| left.distance.total_cmp(&right.distance))
    });

    for candidate in ranked {
        if finalists.len() >= JXL_FINALIST_LIMIT {
            break;
        }
        include_finalist(&mut finalists, &mut selected, candidate);
    }

    finalists.sort_by(|left, right| {
        left.output_size
            .cmp(&right.output_size)
            .then_with(|| left.distance.total_cmp(&right.distance))
    });
    finalists
}

fn include_finalist(
    finalists: &mut Vec<JxlScreenedCandidate>,
    selected: &mut HashSet<i32>,
    candidate: &JxlScreenedCandidate,
) {
    if selected.insert(distance_key(candidate.distance)) {
        finalists.push(candidate.clone());
    }
}

/// Finalizes screening results by shortlisting top finalist candidates.
///
/// Uses promotion scoring, boundary detection, and quality considerations to select finalists.
/// The finalists are then re-evaluated with e10 parameters for ultimate mode.
///
/// Term definitions (see `candidate_comparator` for terminology):
/// - **Finalists**: Selected subset from screened candidates for final evaluation
/// - **Promotion**: Reason(s) a candidate was included in finalist set
fn finalize_screening_result(
    candidates: Vec<JxlScreenedCandidate>,
    best_idx: usize,
    iterations: u32,
    mut log: Vec<String>,
) -> JxlScreeningResult {
    let finalists = shortlist_finalists(&candidates, best_idx);
    let finalist_summary = finalists
        .iter()
        .map(|candidate| format!("d={:.3}", candidate.distance))
        .collect::<Vec<_>>()
        .join(", ");
    log.push(format!(
        "e10 finalist shortlist ({}): {finalist_summary}",
        finalists.len()
    ));

    JxlScreeningResult {
        best_distance: candidates[best_idx].distance,
        best_output_size: candidates[best_idx].output_size,
        iterations,
        screened_candidates: candidates,
        finalists,
        log,
    }
}

/// Screens JXL distance candidates to identify finalists for e10 ultimate finalization.
///
/// ## Terminology (unified with HEVC/other explorers)
/// - **Screening phase**: Initial exploration of distance values (Phase 1 ladder, Phase 2 binary)
/// - **Candidate**: A specific distance value with its output size
/// - **Finalist shortlist**: Curated subset of candidates promoted for ultimate finalization
/// - **Winner**: Chosen by the caller based on finalists (not this function's responsibility)
///
/// See `candidate_comparator` module for unified ranking terminology and philosophy.
///
/// # Errors
/// Returns `Err` if the `try_candidate` closure fails for any tested distance.
pub fn screen_jxl_candidates<F>(
    input_size: u64,
    initial_size: u64,
    mut try_candidate: F,
) -> Result<Option<JxlScreeningResult>, String>
where
    F: FnMut(f32) -> Result<u64, String>,
{
    if input_size == 0 {
        return Ok(None);
    }

    let mut log = Vec::new();
    let initial_distance = clamp_explore_distance(JXL_EXPLORE_LADDER[0]);
    let mut iterations = 1u32;
    let mut tested = HashSet::new();
    tested.insert(distance_key(initial_distance));

    let mut candidates = vec![JxlScreenedCandidate {
        distance: initial_distance,
        output_size: initial_size,
        ladder_phase: true,
        reasons: Vec::new(),
    }];
    add_reason(&mut candidates, 0, JxlPromotionReason::Baseline, &mut log);
    add_reason(&mut candidates, 0, JxlPromotionReason::NewRegion, &mut log);

    log.push(format!(
        "Phase 1 ladder: d={initial_distance:.3} -> {:.1}% of input",
        size_ratio_pct(initial_size, input_size)
    ));

    let mut best_idx = 0usize;
    let mut region_keys = HashSet::new();
    region_keys.insert(candidate_region_key(initial_distance));

    // Condition A (Hard Constraint): If d=0.001 is already safe (<= 100% size), stop exploring.
    // Quality is already safe and beneficial, so further exploration cost is not worth it.
    let ratio = size_ratio(initial_size, input_size);
    if ratio <= 1.0 {
        log.push(format!(
            "   Early exit: d={initial_distance:.3} is already safe and beneficial ({:.1}% ≤ 100%)",
            ratio * 100.0
        ));
        return Ok(Some(finalize_screening_result(
            candidates, 0, iterations, log,
        )));
    }

    if near_boundary(initial_size, input_size) {
        add_reason(
            &mut candidates,
            0,
            JxlPromotionReason::BoundaryRegion,
            &mut log,
        );
    }

    let mut phase_two_baseline = None;
    let mut pending_adjacent_promotion = false;

    for &candidate_distance in JXL_EXPLORE_LADDER.iter().skip(1) {
        if iterations >= JXL_EXPLORE_MAX_ITERATIONS {
            break;
        }

        let candidate_distance = clamp_explore_distance(candidate_distance);
        if !tested.insert(distance_key(candidate_distance)) {
            continue;
        }

        let previous_size = candidates
            .last()
            .map_or(initial_size, |candidate| candidate.output_size);
        let size = try_candidate(candidate_distance)?;
        iterations += 1;
        let delta_pct = improvement_ratio(previous_size, size, input_size) * 100.0;
        let trend = if size < previous_size { "↓" } else { "→" };

        log.push(format!(
            "Phase 1 ladder: d={candidate_distance:.3} -> {:.1}% of input ({trend} {delta_pct:.1}%)",
            size_ratio_pct(size, input_size)
        ));

        candidates.push(JxlScreenedCandidate {
            distance: candidate_distance,
            output_size: size,
            ladder_phase: true,
            reasons: Vec::new(),
        });
        let current_idx = candidates.len() - 1;

        if pending_adjacent_promotion {
            add_reason(
                &mut candidates,
                current_idx,
                JxlPromotionReason::AdjacentToBest,
                &mut log,
            );
            pending_adjacent_promotion = false;
        }

        if region_keys.insert(candidate_region_key(candidate_distance)) {
            add_reason(
                &mut candidates,
                current_idx,
                JxlPromotionReason::NewRegion,
                &mut log,
            );
        }

        if near_boundary(size, input_size) {
            add_reason(
                &mut candidates,
                current_idx,
                JxlPromotionReason::BoundaryRegion,
                &mut log,
            );
        }

        if size < candidates[best_idx].output_size {
            if current_idx > 0 {
                add_reason(
                    &mut candidates,
                    current_idx - 1,
                    JxlPromotionReason::AdjacentToBest,
                    &mut log,
                );
            }
            add_reason(
                &mut candidates,
                current_idx,
                JxlPromotionReason::BetterThanCurrentBest,
                &mut log,
            );
            pending_adjacent_promotion = true;
            best_idx = current_idx;
        } else if near_best(size, candidates[best_idx].output_size, input_size) {
            add_reason(
                &mut candidates,
                current_idx,
                JxlPromotionReason::NearCurrentBest,
                &mut log,
            );
        }

        if candidate_distance >= 0.1 - f32::EPSILON {
            phase_two_baseline = Some(current_idx);
        }
    }

    let Some(mut current_idx) = phase_two_baseline else {
        return Ok(Some(finalize_screening_result(
            candidates, best_idx, iterations, log,
        )));
    };

    let precision = JXL_EXPLORE_BINARY_SEARCH_PRECISION.max(0.001);
    let mut current_step = 0.1_f32;
    let mut cadence = UpwardSearchCadence::Adaptive;

    while iterations < JXL_EXPLORE_MAX_ITERATIONS {
        let Some(next_distance) =
            next_phase_two_candidate(candidates[current_idx].distance, current_step, &tested)
        else {
            break;
        };

        tested.insert(distance_key(next_distance));
        let size = try_candidate(next_distance)?;
        iterations += 1;

        log.push(format!(
            "Phase 2 probe: d={next_distance:.3} -> {:.1}% of input (step {:.3})",
            size_ratio_pct(size, input_size),
            current_step
        ));

        candidates.push(JxlScreenedCandidate {
            distance: next_distance,
            output_size: size,
            ladder_phase: false,
            reasons: Vec::new(),
        });
        let probe_idx = candidates.len() - 1;

        if pending_adjacent_promotion {
            add_reason(
                &mut candidates,
                probe_idx,
                JxlPromotionReason::AdjacentToBest,
                &mut log,
            );
            pending_adjacent_promotion = false;
        }

        if region_keys.insert(candidate_region_key(next_distance)) {
            add_reason(
                &mut candidates,
                probe_idx,
                JxlPromotionReason::NewRegion,
                &mut log,
            );
        }

        if near_boundary(size, input_size) {
            add_reason(
                &mut candidates,
                probe_idx,
                JxlPromotionReason::BoundaryRegion,
                &mut log,
            );
        }

        if size < candidates[best_idx].output_size {
            add_reason(
                &mut candidates,
                current_idx,
                JxlPromotionReason::AdjacentToBest,
                &mut log,
            );
            add_reason(
                &mut candidates,
                probe_idx,
                JxlPromotionReason::BetterThanCurrentBest,
                &mut log,
            );
            pending_adjacent_promotion = true;
            best_idx = probe_idx;
        } else if near_best(size, candidates[best_idx].output_size, input_size) {
            add_reason(
                &mut candidates,
                probe_idx,
                JxlPromotionReason::NearCurrentBest,
                &mut log,
            );
        }

        let current_ratio = size_ratio(size, input_size);
        let previous_ratio = size_ratio(candidates[current_idx].output_size, input_size);
        let ratio_drop_pct = (previous_ratio - current_ratio).abs() * 100.0;
        let improvement = improvement_ratio(candidates[current_idx].output_size, size, input_size);
        let near_break_even = near_boundary(size, input_size);

        if near_break_even && current_step > precision + f32::EPSILON {
            let old_step = current_step;
            current_step = (current_step / 2.0).max(precision);
            cadence = if current_step > precision + f32::EPSILON {
                UpwardSearchCadence::Jogging
            } else {
                UpwardSearchCadence::Paused
            };
            log.push(format!(
                "   Search Decelerating (ratio {:.1}%, step: {:.3} -> {:.3}, near break-even)",
                current_ratio * 100.0,
                old_step,
                current_step
            ));
        } else if improvement > 0.10 && current_step < 0.4 {
            let old_step = current_step;
            current_step = (current_step * 2.0)
                .min(0.4)
                .min((JXL_EXPLORE_CEILING - next_distance).max(precision));
            if current_step > old_step + f32::EPSILON {
                cadence = UpwardSearchCadence::Adaptive;
                log.push(format!(
                    "   Search Accelerated (drop Δ{ratio_drop_pct:.1}%, step: {old_step:.3} -> {current_step:.3})"
                ));
            }
        } else {
            match cadence {
                UpwardSearchCadence::Jogging => {
                    cadence = UpwardSearchCadence::Paused;
                    log.push(format!(
                        "   Search Jogging complete at step {current_step:.3}; pausing adaptive changes"
                    ));
                }
                UpwardSearchCadence::Paused => {
                    cadence = UpwardSearchCadence::Normal;
                    log.push(format!(
                        "   Search Paused at boundary pace ({current_step:.3}); resuming next probe"
                    ));
                }
                UpwardSearchCadence::Adaptive | UpwardSearchCadence::Normal => {}
            }
        }

        current_idx = probe_idx;
    }

    Ok(Some(finalize_screening_result(
        candidates, best_idx, iterations, log,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finalist_distances(result: &JxlScreeningResult) -> Vec<i32> {
        result
            .finalists
            .iter()
            .map(|candidate| distance_key(candidate.distance))
            .collect()
    }

    #[test]
    fn test_screening_keeps_best_ladder_candidate() {
        let result = screen_jxl_candidates(100, 120, |distance| match distance_key(distance) {
            10 => Ok(90),
            _ => Ok(110),
        })
        .expect("exploration should succeed")
        .expect("screening result should exist");

        assert!((result.best_distance - 0.01).abs() < f32::EPSILON);
        assert_eq!(result.best_output_size, 90);
        assert!(result.iterations >= 3);
        assert!(finalist_distances(&result).contains(&distance_key(0.01)));
        assert!(finalist_distances(&result).contains(&distance_key(0.001)));
    }

    #[test]
    fn test_screening_never_reaches_one() {
        let result = screen_jxl_candidates(100, 140, |_distance| Ok(130))
            .expect("exploration should not fail")
            .expect("screening result should exist");

        assert!(!result.screened_candidates.is_empty());
        assert!(result
            .screened_candidates
            .iter()
            .all(|candidate| candidate.distance < 1.0));
        assert!(result
            .screened_candidates
            .iter()
            .any(|candidate| (candidate.distance - 0.999).abs() < 0.000_5));
    }

    #[test]
    fn test_screening_promotes_adjacent_and_boundary_candidates() {
        let result = screen_jxl_candidates(100, 104, |distance| {
            let size = match distance_key(distance) {
                10 => 99,
                100 => 100,
                200 => 98,
                250 => 101,
                _ => 110,
            };
            Ok(size)
        })
        .expect("exploration should succeed")
        .expect("screening result should exist");

        let finalists = finalist_distances(&result);
        assert!(finalists.contains(&distance_key(0.01)));
        assert!(finalists.contains(&distance_key(0.1)));
        assert!(finalists.contains(&distance_key(0.2)));
        assert!(finalists.contains(&distance_key(0.25)));
    }

    #[test]
    fn test_screening_logs_acceleration_and_deceleration() {
        let result = screen_jxl_candidates(100, 150, |distance| {
            let size = match distance_key(distance) {
                10 => 130,
                100 => 120,
                200 => 108,
                400 => 104,
                500 => 99,
                _ => 140,
            };
            Ok(size)
        })
        .expect("exploration should succeed")
        .expect("screening result should exist");

        assert!(
            result
                .log
                .iter()
                .any(|line| line.contains("Search Accelerated")),
            "expected acceleration log, got {:?}",
            result.log
        );
        assert!(
            result
                .log
                .iter()
                .any(|line| line.contains("Search Decelerating")),
            "expected deceleration log, got {:?}",
            result.log
        );
        assert!(result.best_distance < 1.0);
    }

    #[test]
    fn test_screening_early_exit_on_safe_initial_result() {
        // Condition A: d=0.001 is safe (90 <= 100), should exit immediately
        let mut calls = 0;
        let result = screen_jxl_candidates(100, 90, |_distance| {
            calls += 1;
            Ok(50) // Should never be called
        })
        .expect("exploration should succeed")
        .expect("screening result should exist");

        assert!((result.best_distance - 0.001).abs() < f32::EPSILON);
        assert_eq!(result.best_output_size, 90);
        assert_eq!(result.iterations, 1);
        assert_eq!(calls, 0); // No further probes
        assert!(result.log.iter().any(|line| line.contains("Early exit")));
    }
}
