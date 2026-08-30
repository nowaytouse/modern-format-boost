//! Shared JPEG XL effort selection policy.
//!
//! The policy is intentionally centralized so JPEG bitstream encode and
//! direct JXL encode paths do not drift. Effort is an encoder policy, not a
//! quality-search axis: every encode phase receives one policy-selected effort.
//!
//! ## Encoder tiers
//! Direct pixel encoding uses the two bounded production tiers:
//!   - Normal   → e7  (`ultimate = false`)
//!   - Ultimate → e10 (`ultimate = true`)
//!
//! JPEG bitstream transcode is a separate primitive. libjxl's e11 path is
//! optimized for lossless JPEG reconstruction and is selected for that context;
//! a tool that rejects the expert switch receives the explicit production
//! fallback. It must not leak into ordinary pixel encoding, where e11 can be
//! disproportionately slow.
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
/// levels by output size. JPEG bitstream transcode is deliberately isolated
/// from direct pixel encoding because its e11 implementation has different
/// performance characteristics.
#[must_use]
pub fn effort_plan(kind: JxlEffortContext, ultimate: bool) -> Vec<JxlEffortPlan> {
    let effort = match kind {
        JxlEffortContext::DirectEncode => encoder_effort(ultimate),
        JxlEffortContext::JpegLosslessTranscode => constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT,
    };
    vec![JxlEffortPlan::Single(effort)]
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
        assert_eq!(
            efforts(&plan),
            vec![constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT]
        );
    }

    #[test]
    fn ultimate_tier_produces_e10() {
        assert_eq!(encoder_effort(true), constants::JXL_ULTIMATE_EFFORT);
        let plan = effort_plan(JxlEffortContext::DirectEncode, true);
        assert_eq!(efforts(&plan), vec![constants::JXL_ULTIMATE_EFFORT]);
        let plan = effort_plan(JxlEffortContext::JpegLosslessTranscode, true);
        assert_eq!(
            efforts(&plan),
            vec![constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT]
        );
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
