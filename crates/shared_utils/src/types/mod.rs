//! Type-Safe Wrappers Module
//!
//! Provides type-safe wrappers to elevate mathematical assumptions to the type system level.
//!
//! ## Module List
//! - `crf`: Type-safe wrapper for CRF (Constant Rate Factor)
//! - `ssim`: Type-safe wrapper for SSIM (Structural Similarity Index)
//! - `file_size`: Type-safe wrapper for file sizes
//! - `iteration`: Guard for iteration counts

pub mod crf;
pub mod file_size;
pub mod iteration;
pub mod perception;
pub mod preset;
pub mod ssim;

pub use crf::{
    Av1Encoder, Crf, EncoderBounds, Error as CrfError, HevcEncoder, Vp9Encoder, X264Encoder,
};
pub use file_size::FileSize;
pub use iteration::{Error as IterationError, Guard as IterationGuard};
pub use perception::{ProcessHistory, Visual};
pub use preset::Preset as EncoderPreset;
pub use ssim::{Error as SsimError, SSIM_EPSILON, Ssim};

/// Result of a specific verification check.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CheckResult {
    /// The check was performed and passed.
    Passed,
    /// The check was performed and failed with the given reason.
    Failed(String),
    /// The check was not performed (e.g., not required by options or prerequisite failed).
    NotChecked,
}

impl CheckResult {
    /// Returns true if the check did not explicitly fail.
    ///
    /// Prefer [`Self::is_passed`] when the caller needs strict success semantics.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        !self.is_failed()
    }

    /// Returns true only when the check was performed and passed.
    #[must_use]
    pub const fn is_passed(&self) -> bool {
        matches!(self, Self::Passed)
    }

    /// Returns true only when the check was performed and failed.
    #[must_use]
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    /// Returns true when the check was skipped or not applicable.
    #[must_use]
    pub const fn is_skipped(&self) -> bool {
        matches!(self, Self::NotChecked)
    }

    /// Returns the failure reason when the check explicitly failed.
    #[must_use]
    pub const fn failure_reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason.as_str()),
            Self::Passed | Self::NotChecked => None,
        }
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn crf_hevc_validation_property(value in -100.0f32..100.0f32) {
            let result = Crf::<HevcEncoder>::new(value);
            let in_range = (0.0..=51.0).contains(&value);
            prop_assert_eq!(result.is_ok(), in_range,
                "HEVC CRF {} should be {} but was {}",
                value,
                if in_range { "valid" } else { "invalid" },
                if result.is_ok() { "valid" } else { "invalid" }
            );
        }

        #[test]
        fn crf_av1_validation_property(value in -100.0f32..100.0f32) {
            let result = Crf::<Av1Encoder>::new(value);
            let in_range = (0.0..=63.0).contains(&value);
            prop_assert_eq!(result.is_ok(), in_range,
                "AV1 CRF {} should be {} but was {}",
                value,
                if in_range { "valid" } else { "invalid" },
                if result.is_ok() { "valid" } else { "invalid" }
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn crf_cache_key_round_trip_hevc(value in 0.0f32..51.0f32) {
            let original = Crf::<HevcEncoder>::new(value).unwrap_or_else(|e| panic!("error: {e:?}"));
            let key = original.to_cache_key().expect("Valid CRF must yield cache key");
            let recovered = Crf::<HevcEncoder>::from_cache_key(key).unwrap_or_else(|e| panic!("error: {e:?}"));

            let diff = (original.value() - recovered.value()).abs();
            prop_assert!(diff < 0.01,
                "Round-trip failed: {} -> {} -> {}, diff = {}",
                original.value(), key, recovered.value(), diff
            );
        }

        #[test]
        fn crf_cache_key_round_trip_av1(value in 0.0f32..63.0f32) {
            let original = Crf::<Av1Encoder>::new(value).unwrap_or_else(|e| panic!("error: {e:?}"));
            let key = original.to_cache_key().expect("Valid CRF must yield cache key");
            let recovered = Crf::<Av1Encoder>::from_cache_key(key).unwrap_or_else(|e| panic!("error: {e:?}"));

            let diff = (original.value() - recovered.value()).abs();
            prop_assert!(diff < 0.01,
                "Round-trip failed: {} -> {} -> {}, diff = {}",
                original.value(), key, recovered.value(), diff
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn ssim_validation_property(value in -2.0f64..2.0f64) {
            let result = Ssim::new(value);
            let in_range = (0.0_f64..=1.0_f64).contains(&value);
            prop_assert_eq!(result.is_ok(), in_range,
                "SSIM {} should be {} but was {}",
                value,
                if in_range { "valid" } else { "invalid" },
                if result.is_ok() { "valid" } else { "invalid" }
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn ssim_display_precision_property(value in 0.0f64..1.0f64) {
            let ssim = Ssim::new(value).unwrap_or_else(|e| panic!("error: {e:?}"));
            let display = ssim.display();

            let parts: Vec<&str> = display.split('.').collect();
            prop_assert_eq!(parts.len(), 2, "Display should have decimal point");
            prop_assert_eq!(parts.get(1).map_or(0, |p| p.len()), 6,
                "Display '{}' should have 6 decimal places, got {}",
                display, parts.get(1).map_or(0, |p| p.len())
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn file_size_saturating_sub_property(a in 0u64..u64::MAX/2, b in 0u64..u64::MAX/2) {
            let size_a = FileSize::new(a);
            let size_b = FileSize::new(b);
            let result = size_a.saturating_sub(size_b);

            if b > a {
                prop_assert_eq!(result.bytes(), 0,
                    "saturating_sub({}, {}) should be 0, got {}",
                    a, b, result.bytes()
                );
            } else {
                prop_assert_eq!(result.bytes(), a - b,
                    "saturating_sub({}, {}) should be {}, got {}",
                    a, b, a - b, result.bytes()
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn file_size_compression_ratio_property(output in 0u64..1_000_000, original in 0u64..1_000_000) {
            let output_size = FileSize::new(output);
            let original_size = FileSize::new(original);
            let ratio = output_size.compression_ratio(original_size);

            if original == 0 {
                prop_assert!(ratio.is_none(),
                    "compression_ratio with zero original should be None"
                );
            } else {
                prop_assert!(ratio.is_some(),
                    "compression_ratio with non-zero original should be Some"
                );
                let r = ratio.unwrap_or_else(|| panic!("missing ratio"));
                prop_assert!(r >= 0.0_f64,
                    "compression_ratio should be >= 0, got {}", r
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn iteration_guard_termination_property(max in 1u32..100) {
            let mut guard = IterationGuard::new(max, "test");

            for i in 1..=max {
                let result = guard.increment();
                prop_assert!(result.is_ok(),
                    "Iteration {} of {} should succeed", i, max
                );
            }

            let result = guard.increment();
            prop_assert!(result.is_err(),
                "Iteration {} of {} should fail", max + 1, max
            );
        }
    }

    #[test]
    fn check_result_explicit_state_helpers_are_consistent() {
        let passed = CheckResult::Passed;
        assert!(passed.is_ok());
        assert!(passed.is_passed());
        assert!(!passed.is_failed());
        assert!(!passed.is_skipped());
        assert_eq!(passed.failure_reason(), None);

        let failed = CheckResult::Failed("bad".to_string());
        assert!(!failed.is_ok());
        assert!(!failed.is_passed());
        assert!(failed.is_failed());
        assert!(!failed.is_skipped());
        assert_eq!(failed.failure_reason(), Some("bad"));

        let skipped = CheckResult::NotChecked;
        assert!(skipped.is_ok());
        assert!(!skipped.is_passed());
        assert!(!skipped.is_failed());
        assert!(skipped.is_skipped());
        assert_eq!(skipped.failure_reason(), None);
    }
}
