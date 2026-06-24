//! Unified Candidate Ranking and Comparison Utilities
//!
//! Centralizes all finalist selection and quality comparison logic to ensure
//! consistent ordering priorities across HEVC ultimate, JXL ultimate, and
//! direct exploration modes.
//!
//! ## Unified Ranking Terminology
//!
//! To maintain consistency across all exploration modes, we use these terms:
//!
//! - **Screening**: Initial phase of testing multiple parameter values (fast,
//!   coarse-grained)
//! - **Finalist/Shortlist**: A curated subset of screened candidates promoted
//!   for final evaluation
//! - **Candidate**: A specific parameter value with its encoded result (part of
//!   finalist set)
//! - **Winner/Selection**: The single best candidate chosen after comparison
//!
//! ### HEVC Ultimate Terminology
//! - "Stage 1 screening" → fast preset screening phase
//! - "Stage 2 slower shortlist" → finalist CRFs for slower presets
//! - "candidate pool" → all encoded Stage 2 results
//! - "Selected HEVC ultimate winner" → final choice
//!
//! ### JXL Ultimate Terminology
//! - "Phase 1 ladder" → screening phase testing distance ladder
//! - "Phase 2 probe" → refined binary search phase
//! - "e10 finalist shortlist" → final distance candidates for e10 finalization
//! - "Promoted" → candidate included in finalist set for finalization
//!
//! ## Ranking Philosophy
//!
//! All comparators follow this priority chain (highest priority first):
//!
//! 1. **Gating/Pass Status**: Size gate, quality checks (`quality_passed`,
//!    `ms_ssim_passed`)
//! 2. **Quality Metrics**: VMAF > CAMBI > `PSNR_UV` > MS-SSIM > SSIM > PSNR
//! 3. **Size/Efficiency**: Output file size (prefer smaller)
//! 4. **Parameter**: CRF (prefer aggressive/lower) or Distance (prefer higher
//!    compression)
//! 5. **Preset/Strategy**: Encoder preset rank (prefer slower/better quality)

use std::cmp::Ordering;

/// Compares two optional quality metrics in descending order (higher is
/// better).
///
/// Semantic correctness: `None` means absent/missing data and is treated as the
/// worst case (0.0). This ensures transitivity: if A > B and B > None, then A >
/// None. NOTE: When used with `min_by`, this correctly selects the highest
/// quality candidate.
#[inline]
#[must_use]
pub fn compare_quality_desc(left: Option<f64>, right: Option<f64>, epsilon: f64) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            if (left - right).abs() > epsilon {
                right.total_cmp(&left)
            } else {
                Ordering::Equal
            }
        }
        (Some(_), None) => Ordering::Less, // left has value, right is absent → left wins
        (None, Some(_)) => Ordering::Greater, // left is absent, right has value → right wins
        (None, None) => Ordering::Equal,   // both absent → equal
    }
}

/// Compares two optional quality metrics in ascending order (lower is better).
///
/// Semantic correctness: `None` means absent/missing data and is treated as the
/// worst case (infinity). This ensures transitivity: if A < B and B < None,
/// then A < None. NOTE: When used with `min_by`, this correctly selects the
/// lowest value candidate.
#[inline]
#[must_use]
pub fn compare_quality_asc(left: Option<f64>, right: Option<f64>, epsilon: f64) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            if (left - right).abs() > epsilon {
                left.total_cmp(&right)
            } else {
                Ordering::Equal
            }
        }
        (Some(_), None) => Ordering::Less, // left has value, right is absent → left wins
        (None, Some(_)) => Ordering::Greater, // left is absent, right has value → right wins
        (None, None) => Ordering::Equal,   // both absent → equal
    }
}

/// Compares two optional (lower, upper) quality metric pairs in descending
/// order.
///
/// Uses the floor (minimum) of each pair as the primary comparison, then
/// individual components.
#[inline]
#[must_use]
pub fn compare_quality_pair_desc(
    left: Option<(f64, f64)>,
    right: Option<(f64, f64)>,
    epsilon: f64,
) -> Ordering {
    match (left, right) {
        (Some((left_a, left_b)), Some((right_a, right_b))) => {
            let left_floor = left_a.min(left_b);
            let right_floor = right_a.min(right_b);
            compare_quality_desc(Some(left_floor), Some(right_floor), epsilon)
                .then_with(|| compare_quality_desc(Some(left_a), Some(right_a), epsilon))
                .then_with(|| compare_quality_desc(Some(left_b), Some(right_b), epsilon))
        }
        _ => Ordering::Equal,
    }
}

/// Compares pass/fail status: pass is strictly less (comes first) than fail.
///
/// Used as a gate check before other comparisons.
#[inline]
#[must_use]
pub const fn compare_pass_gate(left_passed: bool, right_passed: bool) -> Ordering {
    match (left_passed, right_passed) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

/// Compares pass/fail status where pass = Ok and fail = Err.
#[inline]
pub const fn compare_pass_gate_result<E>(left: &Result<(), E>, right: &Result<(), E>) -> Ordering {
    compare_pass_gate(left.is_ok(), right.is_ok())
}

/// Compares file sizes: smaller is better (ascending order).
#[inline]
#[must_use]
pub fn compare_size_asc(left: u64, right: u64) -> Ordering {
    left.cmp(&right)
}

/// Compares CRF values: lower/more aggressive CRF is slightly preferred as
/// tiebreaker. (Higher CRF means lower quality; we prefer candidates that
/// achieved compression at lower CRF.)
#[inline]
#[must_use]
pub fn compare_crf_asc(left: f32, right: f32) -> Ordering {
    left.total_cmp(&right)
}

/// Compares distance values for JXL: higher distance = more compression.
/// Prefer higher distance (reversed comparison).
#[inline]
#[must_use]
pub fn compare_distance_desc(left: f32, right: f32) -> Ordering {
    right.total_cmp(&left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_quality_desc() {
        // Higher quality is better (returns Less for min_by semantics)
        assert_eq!(
            compare_quality_desc(Some(0.95_f64), Some(0.90_f64), 1e-4),
            Ordering::Less
        );
        assert_eq!(
            compare_quality_desc(Some(0.90_f64), Some(0.95_f64), 1e-4),
            Ordering::Greater
        );

        // Within epsilon is equal
        assert_eq!(
            compare_quality_desc(Some(0.95_f64), Some(0.95_f64 + 1e-5_f64), 1e-4),
            Ordering::Equal
        );

        // None is treated as worst case (0.0) - value always wins over None
        assert_eq!(
            compare_quality_desc(None, Some(0.95_f64), 1e-4),
            Ordering::Greater
        );
        assert_eq!(
            compare_quality_desc(Some(0.95_f64), None, 1e-4),
            Ordering::Less
        );
        assert_eq!(compare_quality_desc(None, None, 1e-4), Ordering::Equal);

        // Transitivity: if A > B and B > None, then A > None
        assert_eq!(
            compare_quality_desc(Some(0.95_f64), Some(0.90_f64), 1e-4),
            Ordering::Less
        );
        assert_eq!(
            compare_quality_desc(Some(0.90_f64), None, 1e-4),
            Ordering::Less
        );
        assert_eq!(
            compare_quality_desc(Some(0.95_f64), None, 1e-4),
            Ordering::Less
        );
    }

    #[test]
    fn test_compare_quality_asc() {
        // Lower is better (returns Less for min_by semantics)
        assert_eq!(
            compare_quality_asc(Some(0.01_f64), Some(0.05_f64), 1e-4),
            Ordering::Less
        );
        assert_eq!(
            compare_quality_asc(Some(0.05_f64), Some(0.01_f64), 1e-4),
            Ordering::Greater
        );

        // None is treated as worst case (infinity) - value always wins over None
        assert_eq!(
            compare_quality_asc(None, Some(0.05_f64), 1e-4),
            Ordering::Greater
        );
        assert_eq!(
            compare_quality_asc(Some(0.05_f64), None, 1e-4),
            Ordering::Less
        );
        assert_eq!(compare_quality_asc(None, None, 1e-4), Ordering::Equal);

        // Transitivity: if A < B and B < None, then A < None
        assert_eq!(
            compare_quality_asc(Some(0.01_f64), Some(0.05_f64), 1e-4),
            Ordering::Less
        );
        assert_eq!(
            compare_quality_asc(Some(0.05_f64), None, 1e-4),
            Ordering::Less
        );
        assert_eq!(
            compare_quality_asc(Some(0.01_f64), None, 1e-4),
            Ordering::Less
        );
    }

    #[test]
    fn test_compare_quality_pair_desc() {
        // Pair (0.90, 0.95) has floor 0.90, pair (0.85, 0.92) has floor 0.85
        // Higher floor is better (returns Less for min_by semantics)
        let left = Some((0.90_f64, 0.95_f64));
        let right = Some((0.85_f64, 0.92_f64));
        assert_eq!(compare_quality_pair_desc(left, right, 1e-4), Ordering::Less);
    }

    #[test]
    fn test_compare_pass_gate() {
        assert_eq!(compare_pass_gate(true, false), Ordering::Less);
        assert_eq!(compare_pass_gate(false, true), Ordering::Greater);
        assert_eq!(compare_pass_gate(true, true), Ordering::Equal);
        assert_eq!(compare_pass_gate(false, false), Ordering::Equal);
    }

    #[test]
    fn test_compare_pass_gate_result() {
        let ok1: Result<(), &str> = Ok(());
        let ok2: Result<(), &str> = Ok(());
        let err1: Result<(), &str> = Err("fail");
        let err2: Result<(), &str> = Err("fail");

        assert_eq!(compare_pass_gate_result(&ok1, &err1), Ordering::Less);
        assert_eq!(compare_pass_gate_result(&err1, &ok1), Ordering::Greater);
        assert_eq!(compare_pass_gate_result(&ok1, &ok2), Ordering::Equal);
        assert_eq!(compare_pass_gate_result(&err1, &err2), Ordering::Equal);
    }

    #[test]
    fn test_compare_size_asc() {
        assert_eq!(compare_size_asc(100, 200), Ordering::Less);
        assert_eq!(compare_size_asc(200, 100), Ordering::Greater);
        assert_eq!(compare_size_asc(100, 100), Ordering::Equal);
    }

    #[test]
    fn test_compare_crf_asc() {
        // Lower CRF (more aggressive) is preferred
        assert_eq!(compare_crf_asc(20.0, 25.0), Ordering::Less);
        assert_eq!(compare_crf_asc(25.0, 20.0), Ordering::Greater);
    }

    #[test]
    fn test_compare_distance_desc() {
        // Higher distance (more compression) is preferred
        assert_eq!(compare_distance_desc(0.5, 0.3), Ordering::Less);
        assert_eq!(compare_distance_desc(0.3, 0.5), Ordering::Greater);
    }

    #[test]
    fn test_transitivity_quality_desc() {
        // Transitivity test: ensures None is consistently treated as worst case
        let high = Some(0.95_f64);
        let medium = Some(0.85_f64);
        let low = Some(0.75_f64);
        let none = None;

        // Verify ordering: high > medium > low > none
        assert_eq!(compare_quality_desc(high, medium, 1e-4), Ordering::Less);
        assert_eq!(compare_quality_desc(medium, low, 1e-4), Ordering::Less);
        assert_eq!(compare_quality_desc(low, none, 1e-4), Ordering::Less);

        // Transitivity: if high > medium and medium > none, then high > none
        assert_eq!(compare_quality_desc(high, none, 1e-4), Ordering::Less);

        // Verify reverse ordering
        assert_eq!(compare_quality_desc(none, low, 1e-4), Ordering::Greater);
        assert_eq!(compare_quality_desc(none, medium, 1e-4), Ordering::Greater);
        assert_eq!(compare_quality_desc(none, high, 1e-4), Ordering::Greater);
    }

    #[test]
    fn test_transitivity_quality_asc() {
        // Transitivity test: ensures None is consistently treated as worst case
        // (infinity)
        let low = Some(0.01_f64);
        let medium = Some(0.05_f64);
        let high = Some(0.10_f64);
        let none = None;

        // Verify ordering: low < medium < high < none
        assert_eq!(compare_quality_asc(low, medium, 1e-4), Ordering::Less);
        assert_eq!(compare_quality_asc(medium, high, 1e-4), Ordering::Less);
        assert_eq!(compare_quality_asc(high, none, 1e-4), Ordering::Less);

        // Transitivity: if low < medium and medium < none, then low < none
        assert_eq!(compare_quality_asc(low, none, 1e-4), Ordering::Less);

        // Verify reverse ordering
        assert_eq!(compare_quality_asc(none, high, 1e-4), Ordering::Greater);
        assert_eq!(compare_quality_asc(none, medium, 1e-4), Ordering::Greater);
        assert_eq!(compare_quality_asc(none, low, 1e-4), Ordering::Greater);
    }
}
