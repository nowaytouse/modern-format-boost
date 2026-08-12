//! Shared product policy for IMG and VID exploration.

use crate::types::EncoderPreset;
use serde::{Deserialize, Serialize};

/// The size eligibility rule selected by the calling product mode.
///
/// Delivery, metadata and file-integrity requirements deliberately do not
/// belong here: they are independent evidence gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizePolicy {
    /// A lossy replacement must remove at least one pure-media byte.
    StrictlySmaller,
    /// A caller explicitly permits a bounded amount of growth.
    AllowGrowth { max_extra_bytes: u64 },
}

impl SizePolicy {
    /// Build the shared strict policy, optionally allowing a caller-selected
    /// bounded amount of growth.
    #[must_use]
    pub const fn strict_or_allow_growth(allow_growth: bool, max_extra_bytes: u64) -> Self {
        if allow_growth {
            Self::AllowGrowth { max_extra_bytes }
        } else {
            Self::StrictlySmaller
        }
    }

    /// Return whether `candidate_size` satisfies this policy relative to the
    /// measured source payload.
    #[must_use]
    pub const fn fits(self, candidate_size: u64, source_size: u64) -> bool {
        match self {
            Self::StrictlySmaller => candidate_size < source_size,
            Self::AllowGrowth { max_extra_bytes } => {
                candidate_size <= source_size.saturating_add(max_extra_bytes)
            }
        }
    }
}

/// A measurement cannot be silently fabricated for probe failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome<T, E> {
    Fits(T),
    Oversize(T),
    Failed(E),
    Unverifiable(E),
}

impl<T, E> ProbeOutcome<T, E> {
    /// Return the measured candidate only for outcomes backed by a real size.
    #[must_use]
    pub const fn measured(&self) -> Option<&T> {
        match self {
            Self::Fits(candidate) | Self::Oversize(candidate) => Some(candidate),
            Self::Failed(_) | Self::Unverifiable(_) => None,
        }
    }
}

/// Timeline coverage is part of the quality-coordinate domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimelineDomain {
    Still,
    Sampled,
    Full,
}

/// Video codecs whose CRF/CQ coordinates are not mutually portable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodecDomain {
    Hevc,
    Av1,
    H264,
}

/// Encoder work settings which change the product represented by an otherwise
/// equal quality coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderTuning {
    Preset(EncoderPreset),
}

/// Complete comparison domain for IMG/VID quality coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderDomain {
    Jxl {
        effort: u8,
    },
    Avif {
        speed: u8,
    },
    Video {
        codec: VideoCodecDomain,
        tuning: EncoderTuning,
        timeline: TimelineDomain,
    },
}

/// A numeric quality coordinate whose unit is meaningful only together with
/// its encoder domain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DomainCoordinate {
    domain: EncoderDomain,
    value: QualityCoordinate,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum QualityCoordinate {
    JxlDistance(f32),
    AvifQuality(u8),
    VideoCrf(f32),
}

impl DomainCoordinate {
    #[must_use]
    pub const fn jxl(distance: f32, effort: u8) -> Self {
        Self {
            domain: EncoderDomain::jxl(effort),
            value: QualityCoordinate::JxlDistance(distance),
        }
    }

    #[must_use]
    pub const fn avif(quality: u8, speed: u8) -> Self {
        Self {
            domain: EncoderDomain::avif(speed),
            value: QualityCoordinate::AvifQuality(quality),
        }
    }

    #[must_use]
    pub const fn video(
        crf: f32,
        codec: VideoCodecDomain,
        preset: EncoderPreset,
        timeline: TimelineDomain,
    ) -> Self {
        Self {
            domain: EncoderDomain::video(codec, EncoderTuning::Preset(preset), timeline),
            value: QualityCoordinate::VideoCrf(crf),
        }
    }

    #[must_use]
    pub fn same_unit_as(self, other: Self) -> bool {
        self.domain == other.domain
            && std::mem::discriminant(&self.value) == std::mem::discriminant(&other.value)
    }

    #[must_use]
    pub const fn domain(self) -> EncoderDomain {
        self.domain
    }
}

impl EncoderDomain {
    #[must_use]
    pub const fn jxl(effort: u8) -> Self {
        Self::Jxl { effort }
    }

    #[must_use]
    pub const fn avif(speed: u8) -> Self {
        Self::Avif { speed }
    }

    #[must_use]
    pub const fn video(
        codec: VideoCodecDomain,
        tuning: EncoderTuning,
        timeline: TimelineDomain,
    ) -> Self {
        Self::Video {
            codec,
            tuning,
            timeline,
        }
    }

    /// A raw CRF/quality/distance coordinate is reusable only when every
    /// domain component is equal.
    #[must_use]
    pub fn can_reuse_coordinate_in(self, target: Self) -> bool {
        self == target
    }
}

/// Product-level result of an optimization decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorationOutcome {
    Adopted,
    LosslessTranscoded,
    ExploredOptimized,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::{
        DomainCoordinate, EncoderDomain, EncoderTuning, ExplorationOutcome, ProbeOutcome,
        SizePolicy, TimelineDomain, VideoCodecDomain,
    };
    use crate::types::EncoderPreset;

    #[test]
    fn strict_smaller_rejects_equality() {
        let policy = SizePolicy::StrictlySmaller;
        assert!(policy.fits(999, 1_000));
        assert!(!policy.fits(1_000, 1_000));
        assert!(!policy.fits(1_001, 1_000));
    }

    #[test]
    fn bounded_growth_uses_a_checked_inclusive_ceiling() {
        let policy = SizePolicy::AllowGrowth {
            max_extra_bytes: 20,
        };
        assert!(policy.fits(1_020, 1_000));
        assert!(!policy.fits(1_021, 1_000));
        assert!(policy.fits(u64::MAX, u64::MAX));
    }

    #[test]
    fn optional_growth_selects_one_shared_policy() {
        assert_eq!(
            SizePolicy::strict_or_allow_growth(false, 20),
            SizePolicy::StrictlySmaller
        );
        assert_eq!(
            SizePolicy::strict_or_allow_growth(true, 20),
            SizePolicy::AllowGrowth {
                max_extra_bytes: 20
            }
        );
    }

    #[test]
    fn equal_coordinate_is_reusable_only_inside_the_same_encoder_domain() {
        let search = EncoderDomain::video(
            VideoCodecDomain::Hevc,
            EncoderTuning::Preset(EncoderPreset::Slow),
            TimelineDomain::Full,
        );
        let final_preset = EncoderDomain::video(
            VideoCodecDomain::Hevc,
            EncoderTuning::Preset(EncoderPreset::Slower),
            TimelineDomain::Full,
        );
        let sampled = EncoderDomain::video(
            VideoCodecDomain::Hevc,
            EncoderTuning::Preset(EncoderPreset::Slow),
            TimelineDomain::Sampled,
        );

        assert!(search.can_reuse_coordinate_in(search));
        assert!(!search.can_reuse_coordinate_in(final_preset));
        assert!(!search.can_reuse_coordinate_in(sampled));
    }

    #[test]
    fn img_effort_and_speed_are_part_of_the_coordinate_domain() {
        assert_ne!(EncoderDomain::jxl(7), EncoderDomain::jxl(11));
        assert_ne!(EncoderDomain::avif(8), EncoderDomain::avif(0));
    }

    #[test]
    fn raw_quality_numbers_share_units_only_inside_one_domain() {
        let search = DomainCoordinate::video(
            20.0,
            VideoCodecDomain::Hevc,
            EncoderPreset::Slow,
            TimelineDomain::Full,
        );
        let same_domain = DomainCoordinate::video(
            19.5,
            VideoCodecDomain::Hevc,
            EncoderPreset::Slow,
            TimelineDomain::Full,
        );
        let other_preset = DomainCoordinate::video(
            20.0,
            VideoCodecDomain::Hevc,
            EncoderPreset::Slower,
            TimelineDomain::Full,
        );
        assert!(search.same_unit_as(same_domain));
        assert!(!search.same_unit_as(other_preset));
        assert!(!DomainCoordinate::jxl(1.0, 11).same_unit_as(DomainCoordinate::jxl(1.0, 7)));
        assert!(!DomainCoordinate::avif(80, 8).same_unit_as(DomainCoordinate::avif(80, 0)));
    }

    #[test]
    fn failed_and_unverifiable_probes_have_no_measured_size_boundary() {
        let failed: ProbeOutcome<u64, &str> = ProbeOutcome::Failed("encoder");
        let unknown: ProbeOutcome<u64, &str> = ProbeOutcome::Unverifiable("parser");
        let fits: ProbeOutcome<u64, &str> = ProbeOutcome::Fits(900);
        let oversize: ProbeOutcome<u64, &str> = ProbeOutcome::Oversize(1_100);

        assert!(failed.measured().is_none());
        assert!(unknown.measured().is_none());
        assert_eq!(fits.measured(), Some(&900));
        assert_eq!(oversize.measured(), Some(&1_100));
    }

    #[test]
    fn detailed_outcome_keeps_product_semantics_distinct() {
        assert_ne!(
            ExplorationOutcome::Adopted,
            ExplorationOutcome::LosslessTranscoded
        );
        assert_ne!(
            ExplorationOutcome::LosslessTranscoded,
            ExplorationOutcome::ExploredOptimized
        );
        assert_ne!(
            ExplorationOutcome::ExploredOptimized,
            ExplorationOutcome::Failed
        );
    }
}
