//! Shared JPEG XL effort selection policy.
//!
//! The policy is intentionally centralized so JPEG bitstream encode and
//! direct JXL encode paths do not drift. Effort is an encoder policy, not a
//! quality-search axis: every encode phase receives one policy-selected effort.

use crate::constants;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlEffortPlan {
    Single(u8),
}

impl JxlEffortPlan {
    #[must_use]
    pub const fn effort(self) -> u8 {
        match self {
            Self::Single(effort) => effort,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlEffortContext {
    DirectEncode,
    JpegLosslessTranscode,
}

#[must_use]
pub const fn encoder_effort(ultimate: bool) -> u8 {
    constants::jxl_effort_for_mode(ultimate)
}

#[must_use]
pub const fn direct_encode_effort_for_archive(archive: bool, ultimate: bool) -> u8 {
    if archive {
        constants::JXL_ULTIMATE_EFFORT
    } else {
        constants::jxl_effort_for_mode(ultimate)
    }
}

#[must_use]
pub const fn archive_effort(kind: JxlEffortContext) -> u8 {
    match kind {
        JxlEffortContext::DirectEncode => constants::JXL_ULTIMATE_EFFORT,
        JxlEffortContext::JpegLosslessTranscode => constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT,
    }
}

#[must_use]
pub fn archive_effort_plan(kind: JxlEffortContext) -> Vec<JxlEffortPlan> {
    vec![JxlEffortPlan::Single(archive_effort(kind))]
}

/// Build the encoder-effort plan.
///
/// The returned vector intentionally contains one item. Quality exploration
/// may vary distance, but it must not run extra encodes merely to rank effort
/// levels by output size.
#[must_use]
pub fn effort_plan(_kind: JxlEffortContext, ultimate: bool) -> Vec<JxlEffortPlan> {
    vec![JxlEffortPlan::Single(encoder_effort(ultimate))]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn efforts(plan: &[JxlEffortPlan]) -> Vec<u8> {
        plan.iter().map(|item| item.effort()).collect()
    }

    #[test]
    fn effort_policy_depends_on_mode_not_input_size_or_exploration() {
        assert_eq!(
            effort_plan(JxlEffortContext::JpegLosslessTranscode, true),
            vec![JxlEffortPlan::Single(constants::JXL_ULTIMATE_EFFORT)]
        );
        assert_eq!(
            effort_plan(JxlEffortContext::DirectEncode, true),
            vec![JxlEffortPlan::Single(constants::JXL_ULTIMATE_EFFORT)]
        );
    }

    #[test]
    fn direct_encode_uses_one_encoder_policy_effort() {
        let plan = effort_plan(JxlEffortContext::DirectEncode, false);
        assert_eq!(efforts(&plan), vec![constants::JXL_DEFAULT_EFFORT]);
    }

    #[test]
    fn jpeg_lossless_normal_mode_uses_one_default_effort() {
        let plan = effort_plan(JxlEffortContext::JpegLosslessTranscode, false);
        assert_eq!(efforts(&plan), vec![constants::JXL_DEFAULT_EFFORT]);
    }

    #[test]
    fn archive_mode_hard_overrides_jpeg_lossless_encode_to_e11() {
        assert_eq!(
            archive_effort_plan(JxlEffortContext::JpegLosslessTranscode),
            vec![JxlEffortPlan::Single(
                constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT
            )]
        );
    }

    #[test]
    fn archive_mode_hard_overrides_direct_encode_to_e11() {
        assert_eq!(
            archive_effort_plan(JxlEffortContext::DirectEncode),
            vec![JxlEffortPlan::Single(constants::JXL_ULTIMATE_EFFORT)]
        );
    }

    #[test]
    fn archive_direct_encode_effort_uses_e11_without_requiring_ultimate() {
        assert_eq!(direct_encode_effort_for_archive(false, false), 7);
        assert_eq!(direct_encode_effort_for_archive(false, true), 11);
        assert_eq!(direct_encode_effort_for_archive(true, false), 11);
        assert_eq!(direct_encode_effort_for_archive(true, true), 11);
    }

    #[test]
    fn ultimate_mode_uses_one_final_domain_effort() {
        let plan = effort_plan(JxlEffortContext::DirectEncode, true);
        assert_eq!(efforts(&plan), vec![constants::JXL_ULTIMATE_EFFORT]);
    }
}
