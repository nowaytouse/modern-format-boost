//! Shared JPEG XL effort selection policy.
//!
//! The policy is intentionally centralized so JPEG bitstream encode and
//! direct JXL encode paths do not drift. Effort is an encoder policy, not a
//! quality-search axis: every encode phase receives one policy-selected effort.
//!
//! ## Encoder tiers
//! Only two tiers exist at the algorithm layer:
//!   - Normal   → e7  (`ultimate = false`)
//!   - Ultimate → e11 (`ultimate = true`)
//!
//! `archive` is a product mode, not a third encoder tier. Normalize it to the
//! Ultimate tier here so individual encode paths cannot drift:
//!
//! ```rust,ignore
//! encoder_effort_for_mode(ultimate, archive)
//! ```

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

/// Map product flags onto the two encoder tiers.
#[must_use]
pub const fn effective_ultimate(ultimate: bool, archive: bool) -> bool {
    ultimate || archive
}

/// Resolve product flags and select one of the two encoder tiers.
#[must_use]
pub const fn encoder_effort_for_mode(ultimate: bool, archive: bool) -> u8 {
    encoder_effort(effective_ultimate(ultimate, archive))
}

/// Build the encoder-effort plan from product flags without exposing their
/// normalization to every encode path.
#[must_use]
pub fn effort_plan_for_mode(
    kind: JxlEffortContext,
    ultimate: bool,
    archive: bool,
) -> Vec<JxlEffortPlan> {
    effort_plan(kind, effective_ultimate(ultimate, archive))
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
    fn normal_tier_produces_e7() {
        assert_eq!(encoder_effort(false), constants::JXL_DEFAULT_EFFORT);
        let plan = effort_plan(JxlEffortContext::DirectEncode, false);
        assert_eq!(efforts(&plan), vec![constants::JXL_DEFAULT_EFFORT]);
        let plan = effort_plan(JxlEffortContext::JpegLosslessTranscode, false);
        assert_eq!(efforts(&plan), vec![constants::JXL_DEFAULT_EFFORT]);
    }

    #[test]
    fn ultimate_tier_produces_e11() {
        assert_eq!(encoder_effort(true), constants::JXL_ULTIMATE_EFFORT);
        let plan = effort_plan(JxlEffortContext::DirectEncode, true);
        assert_eq!(efforts(&plan), vec![constants::JXL_ULTIMATE_EFFORT]);
        let plan = effort_plan(JxlEffortContext::JpegLosslessTranscode, true);
        assert_eq!(efforts(&plan), vec![constants::JXL_ULTIMATE_EFFORT]);
    }

    #[test]
    fn archive_normalizes_to_ultimate_in_policy() {
        let cases = [
            (false, false, false), // Normal
            (false, true, true),   // Ultimate
            (true, false, true),   // archive → Ultimate
            (true, true, true),    // archive+ultimate → Ultimate
        ];
        for (archive, ultimate, expected_ultimate) in cases {
            let effective = effective_ultimate(ultimate, archive);
            assert_eq!(effective, expected_ultimate);
            assert_eq!(
                encoder_effort_for_mode(ultimate, archive),
                encoder_effort(effective)
            );
        }
    }

    #[test]
    fn effort_plan_produces_single_item() {
        assert_eq!(
            effort_plan(JxlEffortContext::DirectEncode, false).len(),
            1,
            "effort plan must always contain exactly one item"
        );
        assert_eq!(
            effort_plan(JxlEffortContext::DirectEncode, true).len(),
            1,
            "effort plan must always contain exactly one item"
        );
        assert_eq!(
            efforts(&effort_plan_for_mode(
                JxlEffortContext::DirectEncode,
                false,
                true,
            )),
            vec![constants::JXL_ULTIMATE_EFFORT]
        );
    }
}
