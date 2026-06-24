//! Shared JPEG XL effort selection policy.
//!
//! The policy is intentionally centralized so JPEG bitstream transcode and
//! direct JXL encode paths do not drift. Large inputs run measured candidate
//! searches; small inputs stay fixed at e7 to avoid wasting encode time.

use crate::constants;

pub const JXL_EFFORT_SEARCH_THRESHOLD_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlEffortPlan {
    Single(u8),
    Candidate(u8),
}

impl JxlEffortPlan {
    #[must_use]
    pub const fn effort(self) -> u8 {
        match self {
            Self::Single(effort) | Self::Candidate(effort) => effort,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlEffortSearchKind {
    DirectEncode,
    JpegLosslessTranscode,
}

#[must_use]
pub const fn size_ge_1mib(file_size: u64) -> bool {
    file_size >= JXL_EFFORT_SEARCH_THRESHOLD_BYTES
}

#[must_use]
pub const fn screening_effort(ultimate: bool, explore: bool) -> u8 {
    if ultimate && explore {
        constants::JXL_DEFAULT_EFFORT
    } else {
        constants::jxl_effort_for_mode(ultimate)
    }
}

#[must_use]
pub const fn encode_effort_for_size(ultimate: bool, explore: bool, file_size: u64) -> u8 {
    if size_ge_1mib(file_size) {
        screening_effort(ultimate, explore)
    } else {
        constants::JXL_DEFAULT_EFFORT
    }
}

#[must_use]
pub const fn direct_encode_effort_for_archive(archive: bool, ultimate: bool) -> u8 {
    if archive {
        constants::JXL_ULTIMATE_EFFORT
    } else {
        constants::jxl_effort_for_mode(ultimate)
    }
}

fn push_unique(plan: &mut Vec<JxlEffortPlan>, effort: u8) {
    if plan.iter().any(|item| item.effort() == effort) {
        return;
    }
    plan.push(JxlEffortPlan::Candidate(effort));
}

#[must_use]
pub const fn archive_effort(kind: JxlEffortSearchKind) -> u8 {
    match kind {
        JxlEffortSearchKind::DirectEncode => constants::JXL_ULTIMATE_EFFORT,
        JxlEffortSearchKind::JpegLosslessTranscode => constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT,
    }
}

#[must_use]
pub fn archive_effort_search_plan(kind: JxlEffortSearchKind) -> Vec<JxlEffortPlan> {
    vec![JxlEffortPlan::Single(archive_effort(kind))]
}

/// Build the measured effort-search plan.
///
/// Production policy skips e9. e11 is included by default only for JPEG
/// bitstream/lossless exploration because transcode remains materially faster
/// than decoded-pixel encoding while still improving output size.
#[must_use]
pub fn effort_search_plan(
    kind: JxlEffortSearchKind,
    ultimate: bool,
    explore: bool,
    file_size: u64,
    allow_expert_options: bool,
) -> Vec<JxlEffortPlan> {
    let primary = encode_effort_for_size(ultimate, explore, file_size);
    if !size_ge_1mib(file_size) {
        return vec![JxlEffortPlan::Single(primary)];
    }

    let mut plan = Vec::new();
    push_unique(&mut plan, primary);

    let candidates: &[u8] = match (kind, allow_expert_options) {
        (JxlEffortSearchKind::DirectEncode, _) => &[
            constants::JXL_DEFAULT_EFFORT,
            constants::JXL_DEEP_EFFORT,
            constants::JXL_ULTIMATE_EFFORT,
        ],
        (JxlEffortSearchKind::JpegLosslessTranscode, _) => &[
            constants::JXL_DEFAULT_EFFORT,
            constants::JXL_DEEP_EFFORT,
            constants::JXL_ULTIMATE_EFFORT,
            constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT,
        ],
    };

    for &effort in candidates {
        push_unique(&mut plan, effort);
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn efforts(plan: &[JxlEffortPlan]) -> Vec<u8> {
        plan.iter().map(|item| item.effort()).collect()
    }

    #[test]
    fn small_inputs_are_fixed_e7_even_in_ultimate_mode() {
        assert!(!size_ge_1mib(JXL_EFFORT_SEARCH_THRESHOLD_BYTES - 1));
        assert_eq!(
            effort_search_plan(
                JxlEffortSearchKind::JpegLosslessTranscode,
                true,
                true,
                JXL_EFFORT_SEARCH_THRESHOLD_BYTES - 1,
                false,
            ),
            vec![JxlEffortPlan::Single(constants::JXL_DEFAULT_EFFORT)]
        );
    }

    #[test]
    fn direct_encode_large_inputs_use_shared_production_candidates_without_e9() {
        let plan = effort_search_plan(
            JxlEffortSearchKind::DirectEncode,
            false,
            false,
            JXL_EFFORT_SEARCH_THRESHOLD_BYTES,
            false,
        );
        assert_eq!(
            efforts(&plan),
            vec![
                constants::JXL_DEFAULT_EFFORT,
                constants::JXL_DEEP_EFFORT,
                constants::JXL_ULTIMATE_EFFORT,
            ]
        );
        assert!(!efforts(&plan).contains(&constants::JXL_DISABLED_EFFORT));
        assert!(!efforts(&plan).contains(&constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT));
    }

    #[test]
    fn jpeg_lossless_large_inputs_include_e11_by_default() {
        let plan = effort_search_plan(
            JxlEffortSearchKind::JpegLosslessTranscode,
            false,
            false,
            JXL_EFFORT_SEARCH_THRESHOLD_BYTES,
            false,
        );
        assert_eq!(
            efforts(&plan),
            vec![
                constants::JXL_DEFAULT_EFFORT,
                constants::JXL_DEEP_EFFORT,
                constants::JXL_ULTIMATE_EFFORT,
                constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT,
            ]
        );
        assert!(!efforts(&plan).contains(&constants::JXL_DISABLED_EFFORT));
    }

    #[test]
    fn jpeg_lossless_expert_flag_does_not_duplicate_default_e11_candidate() {
        let plan = effort_search_plan(
            JxlEffortSearchKind::JpegLosslessTranscode,
            false,
            false,
            JXL_EFFORT_SEARCH_THRESHOLD_BYTES,
            true,
        );
        assert_eq!(
            efforts(&plan),
            vec![
                constants::JXL_DEFAULT_EFFORT,
                constants::JXL_DEEP_EFFORT,
                constants::JXL_ULTIMATE_EFFORT,
                constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT,
            ]
        );
        assert!(!efforts(&plan).contains(&constants::JXL_DISABLED_EFFORT));
    }

    #[test]
    fn archive_mode_hard_overrides_jpeg_lossless_transcode_to_e11() {
        assert_eq!(
            archive_effort_search_plan(JxlEffortSearchKind::JpegLosslessTranscode),
            vec![JxlEffortPlan::Single(
                constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT
            )]
        );
    }

    #[test]
    fn archive_mode_hard_overrides_direct_encode_to_e10() {
        assert_eq!(
            archive_effort_search_plan(JxlEffortSearchKind::DirectEncode),
            vec![JxlEffortPlan::Single(constants::JXL_ULTIMATE_EFFORT)]
        );
    }

    #[test]
    fn archive_direct_encode_effort_uses_e10_without_requiring_ultimate() {
        assert_eq!(direct_encode_effort_for_archive(false, false), 7);
        assert_eq!(direct_encode_effort_for_archive(false, true), 10);
        assert_eq!(direct_encode_effort_for_archive(true, false), 10);
        assert_eq!(direct_encode_effort_for_archive(true, true), 10);
    }

    #[test]
    fn ultimate_mode_keeps_primary_effort_first_then_shared_candidates() {
        let plan = effort_search_plan(
            JxlEffortSearchKind::DirectEncode,
            true,
            false,
            JXL_EFFORT_SEARCH_THRESHOLD_BYTES,
            false,
        );
        assert_eq!(
            efforts(&plan),
            vec![
                constants::JXL_ULTIMATE_EFFORT,
                constants::JXL_DEFAULT_EFFORT,
                constants::JXL_DEEP_EFFORT,
            ]
        );
    }
}
