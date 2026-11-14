//! AVIF quality-boundary search without encoder, metadata or filesystem ownership.

use crate::exploration_policy::SizePolicy;
use std::collections::BTreeSet;

const COARSE_QUALITY_STEP: u8 = 10;
const MIN_QUALITY: u8 = 0;
pub const AVIF_QUALITY_BINARY_PROBE_BUDGET: usize = 7;
pub const AVIF_HANDOFF_QUALITY_FLOOR: u8 = 75;

/// Find the highest fitting quality in the measured bracket with bounded refinement.
/// The callback measures one fixed encoder domain; failures supply no size-order evidence.
pub fn search_highest_fitting_avif_quality_with<Probe>(
    input_size: u64,
    size_policy: Option<SizePolicy>,
    mut probe: Probe,
) -> (Option<u8>, usize)
where
    Probe: FnMut(u8) -> std::result::Result<u64, String>,
{
    if size_policy.is_none() {
        return match probe(100) {
            Ok(_) => (Some(100), 1),
            Err(_) => (None, 1),
        };
    }

    let mut probe_count = 0;
    let mut first_fitting = None;
    let mut lowest_oversize = None;
    let mut failed_qualities = BTreeSet::new();
    let mut probe_quality = |quality: u8| {
        probe_count += 1;
        match probe(quality) {
            Ok(size) if size_policy.is_none_or(|policy| policy.fits(size, input_size)) => {
                first_fitting = Some(quality);
                true
            }
            Ok(_) => {
                lowest_oversize = Some(quality);
                false
            }
            Err(_) => {
                failed_qualities.insert(quality);
                false
            }
        }
    };

    // Q75 belongs to AVIF's own quality domain, not to a converted JXL distance.
    let pivot_fits = probe_quality(AVIF_HANDOFF_QUALITY_FLOOR);
    drop(probe_quality);
    if pivot_fits {
        // 101 is an exclusive sentinel, never passed to the encoder.
        lowest_oversize = Some(101);
    } else if failed_qualities.contains(&AVIF_HANDOFF_QUALITY_FLOOR) {
        // A failed pivot cannot narrow the search interval.
        lowest_oversize = None;
        let mut quality = 100;
        loop {
            probe_count += 1;
            match probe(quality) {
                Ok(size) if size_policy.is_none_or(|policy| policy.fits(size, input_size)) => {
                    first_fitting = Some(quality);
                    break;
                }
                Ok(_) => lowest_oversize = Some(quality),
                Err(_) => {
                    failed_qualities.insert(quality);
                }
            }

            if quality < MIN_QUALITY + COARSE_QUALITY_STEP {
                break;
            }
            quality = quality.saturating_sub(COARSE_QUALITY_STEP);
        }
    } else {
        let mut quality = AVIF_HANDOFF_QUALITY_FLOOR.saturating_sub(COARSE_QUALITY_STEP);
        loop {
            probe_count += 1;
            match probe(quality) {
                Ok(size) if size_policy.is_none_or(|policy| policy.fits(size, input_size)) => {
                    first_fitting = Some(quality);
                    break;
                }
                Ok(_) => lowest_oversize = Some(quality),
                Err(_) => {
                    failed_qualities.insert(quality);
                }
            }

            if quality == MIN_QUALITY {
                break;
            }
            quality = quality.saturating_sub(COARSE_QUALITY_STEP);
        }
    }

    let Some(mut best_quality) = first_fitting else {
        return (None, probe_count);
    };
    if best_quality == 100 {
        return (Some(best_quality), probe_count);
    }

    for _ in 0..AVIF_QUALITY_BINARY_PROBE_BUDGET {
        let Some(oversize_quality) = lowest_oversize else {
            break;
        };
        if u16::from(best_quality) + 1 >= u16::from(oversize_quality) {
            break;
        }
        let midpoint = best_quality + (oversize_quality - best_quality) / 2;
        let Some(candidate) = (best_quality.saturating_add(1)..oversize_quality)
            .filter(|quality| !failed_qualities.contains(quality))
            .min_by_key(|quality| (quality.abs_diff(midpoint), std::cmp::Reverse(*quality)))
        else {
            break;
        };
        probe_count += 1;
        match probe(candidate) {
            Ok(size) if size_policy.is_none_or(|policy| policy.fits(size, input_size)) => {
                best_quality = candidate;
            }
            Ok(_) => lowest_oversize = Some(candidate),
            Err(_) => {
                failed_qualities.insert(candidate);
            }
        }
    }

    (Some(best_quality), probe_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_exact_highest_quality_boundary() {
        let mut probes = Vec::new();
        let (quality, probe_count) = search_highest_fitting_avif_quality_with(
            1_000,
            Some(SizePolicy::StrictlySmaller),
            |quality| {
                probes.push(quality);
                Ok(if quality <= 89 { 900 } else { 1_100 })
            },
        );
        assert_eq!(quality, Some(89));
        assert_eq!(probe_count, 6);
        assert_eq!(probes, vec![75, 88, 94, 91, 89, 90]);
    }

    #[test]
    fn failed_probe_does_not_become_an_oversize_boundary() {
        let (quality, _) = search_highest_fitting_avif_quality_with(
            1_000,
            Some(SizePolicy::StrictlySmaller),
            |quality| {
                if quality == 90 {
                    Err("temporary encoder failure".to_string())
                } else {
                    Ok(if quality <= 89 { 900 } else { 1_100 })
                }
            },
        );
        assert_eq!(quality, Some(89));
    }

    #[test]
    fn failed_q75_pivot_falls_back_to_full_avif_search() {
        let mut probes = Vec::new();
        let (quality, _) = search_highest_fitting_avif_quality_with(
            1_000,
            Some(SizePolicy::StrictlySmaller),
            |quality| {
                probes.push(quality);
                if quality == AVIF_HANDOFF_QUALITY_FLOOR {
                    Err("q75 probe failed".to_string())
                } else {
                    Ok(if quality <= 89 { 900 } else { 1_100 })
                }
            },
        );
        assert_eq!(quality, Some(89));
        assert_eq!(&probes[..3], &[75, 100, 90]);
    }

    #[test]
    fn search_exhausts_before_preserving_source() {
        assert_eq!(MIN_QUALITY, 0);
        let (quality, probe_count) = search_highest_fitting_avif_quality_with(
            1_000,
            Some(SizePolicy::StrictlySmaller),
            |_quality| Ok(1_000),
        );
        assert_eq!(quality, None);
        assert_eq!(probe_count, 9);
    }

    #[test]
    fn search_uses_verified_q0_as_emergency_final_fallback() {
        let mut probes = Vec::new();
        let (quality, probe_count) = search_highest_fitting_avif_quality_with(
            1_000,
            Some(SizePolicy::StrictlySmaller),
            |quality| {
                probes.push(quality);
                Ok(if quality == 0 { 900 } else { 1_100 })
            },
        );
        assert_eq!(quality, Some(0));
        assert_eq!(probe_count, 11);
        assert_eq!(probes, vec![75, 65, 55, 45, 35, 25, 15, 5, 0, 2, 1]);
    }

    #[test]
    fn required_delivery_accepts_verified_oversized_q100_avif() {
        let mut probes = Vec::new();
        let (quality, probe_count) =
            search_highest_fitting_avif_quality_with(1_000, None, |quality| {
                probes.push(quality);
                Ok(1_100)
            });
        assert_eq!(quality, Some(100));
        assert_eq!(probe_count, 1);
        assert_eq!(probes, vec![100]);
    }
}
