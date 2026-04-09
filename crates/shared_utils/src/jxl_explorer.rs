use crate::constants::{
    JXL_EXPLORE_BINARY_SEARCH_PRECISION, JXL_EXPLORE_CEILING, JXL_EXPLORE_LADDER,
    JXL_EXPLORE_MAX_ITERATIONS,
};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct JxlExploreResult {
    pub accepted_distance: f32,
    pub output_size: u64,
    pub iterations: u32,
    pub ladder_phase: bool,
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
    (clamp_explore_distance(distance) * 1000.0).round() as i32
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

fn finish_result(
    accepted_distance: f32,
    output_size: u64,
    iterations: u32,
    ladder_phase: bool,
    log: Vec<String>,
) -> JxlExploreResult {
    JxlExploreResult {
        accepted_distance,
        output_size,
        iterations,
        ladder_phase,
        log,
    }
}

pub fn explore_jxl_distance<F>(
    input_size: u64,
    initial_size: u64,
    mut try_candidate: F,
) -> Result<Option<JxlExploreResult>, String>
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

    log.push(format!(
        "Phase 1 ladder: d={initial_distance:.3} -> {:.1}% of input",
        size_ratio_pct(initial_size, input_size)
    ));

    if initial_size < input_size {
        return Ok(Some(finish_result(
            initial_distance,
            initial_size,
            iterations,
            true,
            log,
        )));
    }

    let mut last_size = initial_size;
    let mut phase_two_baseline = None;

    for &candidate in JXL_EXPLORE_LADDER.iter().skip(1) {
        if iterations >= JXL_EXPLORE_MAX_ITERATIONS {
            break;
        }

        let candidate = clamp_explore_distance(candidate);
        if !tested.insert(distance_key(candidate)) {
            continue;
        }

        let size = try_candidate(candidate)?;
        iterations += 1;
        let delta_pct = improvement_ratio(last_size, size, input_size) * 100.0;
        let trend = if size < last_size { "↓" } else { "→" };

        log.push(format!(
            "Phase 1 ladder: d={candidate:.3} -> {:.1}% of input ({trend} {delta_pct:.1}%)",
            size_ratio_pct(size, input_size)
        ));

        if size < input_size {
            return Ok(Some(finish_result(candidate, size, iterations, true, log)));
        }

        if candidate >= 0.1 - f32::EPSILON {
            phase_two_baseline = Some((candidate, size));
        }
        last_size = size;
    }

    let Some((mut current_distance, mut current_size)) = phase_two_baseline else {
        return Ok(None);
    };

    let precision = JXL_EXPLORE_BINARY_SEARCH_PRECISION.max(0.001);
    let mut current_step = 0.1_f32;
    let mut cadence = UpwardSearchCadence::Adaptive;

    while iterations < JXL_EXPLORE_MAX_ITERATIONS {
        let Some(next_distance) = next_phase_two_candidate(current_distance, current_step, &tested)
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

        if size < input_size {
            return Ok(Some(finish_result(
                next_distance,
                size,
                iterations,
                false,
                log,
            )));
        }

        let current_ratio = size_ratio(size, input_size);
        let previous_ratio = size_ratio(current_size, input_size);
        let ratio_drop_pct = (previous_ratio - current_ratio).abs() * 100.0;
        let improvement = improvement_ratio(current_size, size, input_size);
        let near_break_even = (0.95..=1.05).contains(&current_ratio);

        if near_break_even && current_step > precision + f32::EPSILON {
            let old_step = current_step;
            current_step = (current_step / 2.0).max(precision);
            cadence = if current_step > precision + f32::EPSILON {
                UpwardSearchCadence::Jogging
            } else {
                UpwardSearchCadence::Paused
            };
            log.push(format!(
                "   💧 Search Decelerating (ratio {:.1}%, step: {:.3} -> {:.3}, near break-even)",
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
                    "   ⚡ Search Accelerated (drop Δ{ratio_drop_pct:.1}%, step: {old_step:.3} -> {current_step:.3})"
                ));
            }
        } else {
            match cadence {
                UpwardSearchCadence::Jogging => {
                    cadence = UpwardSearchCadence::Paused;
                    log.push(format!(
                        "   🐢 Search Jogging complete at step {:.3}; pausing adaptive changes",
                        current_step
                    ));
                }
                UpwardSearchCadence::Paused => {
                    cadence = UpwardSearchCadence::Normal;
                    log.push(format!(
                        "   ⏸️ Search Paused at boundary pace ({:.3}); resuming next probe",
                        current_step
                    ));
                }
                UpwardSearchCadence::Adaptive | UpwardSearchCadence::Normal => {}
            }
        }

        current_distance = next_distance;
        current_size = size;
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explorer_accepts_ladder_success() {
        let result = explore_jxl_distance(100, 120, |distance| match distance_key(distance) {
            10 => Ok(90),
            _ => Ok(110),
        })
        .expect("exploration should succeed")
        .expect("0.01 should compress");

        assert_eq!(result.accepted_distance, 0.01);
        assert!(result.ladder_phase);
        assert_eq!(result.output_size, 90);
        assert_eq!(result.iterations, 2);
    }

    #[test]
    fn test_explorer_never_reaches_one() {
        let mut seen = Vec::new();
        let result = explore_jxl_distance(100, 140, |distance| {
            seen.push(distance);
            Ok(130)
        })
        .expect("exploration should not fail");

        assert!(result.is_none());
        assert!(!seen.is_empty());
        assert!(seen.iter().all(|distance| *distance < 1.0));
        assert!(seen
            .iter()
            .any(|distance| (*distance - 0.999).abs() < 0.000_5));
    }

    #[test]
    fn test_explorer_can_accept_phase_two_probe() {
        let result = explore_jxl_distance(100, 140, |distance| {
            let size = match distance_key(distance) {
                10 => 125,
                100 => 110,
                200 => 95,
                _ => 130,
            };
            Ok(size)
        })
        .expect("exploration should succeed")
        .expect("phase two should compress");

        assert!(!result.ladder_phase);
        assert_eq!(result.accepted_distance, 0.2);
        assert_eq!(result.output_size, 95);
    }

    #[test]
    fn test_explorer_logs_acceleration_and_deceleration() {
        let result = explore_jxl_distance(100, 150, |distance| {
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
        .expect("phase two should eventually compress");

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
        assert!(result.accepted_distance < 1.0);
    }
}
