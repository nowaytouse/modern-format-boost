//! Type-Safe Wrappers Module
//!
//! 提供类型安全的包装器，将数学假设从注释提升到类型系统层面。
//!
//! ## 模块列表
//! - `crf`: CRF (Constant Rate Factor) 类型安全包装
//! - `ssim`: SSIM (Structural Similarity Index) 类型安全包装
//! - `file_size`: 文件大小类型安全包装
//! - `iteration`: 迭代次数守卫

pub mod crf;
pub mod file_size;
pub mod iteration;
pub mod ssim;

// Re-exports for convenience
pub use crf::{Av1Encoder, Crf, CrfError, EncoderBounds, HevcEncoder, Vp9Encoder, X264Encoder};
pub use file_size::FileSize;
pub use iteration::{IterationError, IterationGuard};
pub use ssim::{Ssim, SsimError, SSIM_EPSILON};

// ============================================================================
// Property-Based Tests
// ============================================================================

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 1: CRF Validation Correctness**
    // *For any* f32 value, Crf::new should succeed if and only if the value
    // is within encoder-specific bounds [MIN, MAX].
    // **Validates: Requirements 1.1, 1.2**
    // ========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn crf_hevc_validation_property(value in -100.0f32..100.0f32) {
            let result = Crf::<HevcEncoder>::new(value);
            let in_range = value >= 0.0 && value <= 51.0;
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
            let in_range = value >= 0.0 && value <= 63.0;
            prop_assert_eq!(result.is_ok(), in_range,
                "AV1 CRF {} should be {} but was {}",
                value,
                if in_range { "valid" } else { "invalid" },
                if result.is_ok() { "valid" } else { "invalid" }
            );
        }
    }

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 2: CRF Cache Key Round-Trip**
    // *For any* valid Crf value, converting to cache key and back should
    // produce an approximately equal value (within cache precision).
    // **Validates: Requirements 1.4**
    // ========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn crf_cache_key_round_trip_hevc(value in 0.0f32..51.0f32) {
            let original = Crf::<HevcEncoder>::new(value).unwrap();
            let key = original.to_cache_key();
            let recovered = Crf::<HevcEncoder>::from_cache_key(key).unwrap();

            // 缓存键精度为 0.01，所以差异应该 < 0.01
            let diff = (original.value() - recovered.value()).abs();
            prop_assert!(diff < 0.01,
                "Round-trip failed: {} -> {} -> {}, diff = {}",
                original.value(), key, recovered.value(), diff
            );
        }

        #[test]
        fn crf_cache_key_round_trip_av1(value in 0.0f32..63.0f32) {
            let original = Crf::<Av1Encoder>::new(value).unwrap();
            let key = original.to_cache_key();
            let recovered = Crf::<Av1Encoder>::from_cache_key(key).unwrap();

            let diff = (original.value() - recovered.value()).abs();
            prop_assert!(diff < 0.01,
                "Round-trip failed: {} -> {} -> {}, diff = {}",
                original.value(), key, recovered.value(), diff
            );
        }
    }

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 3: SSIM Validation Correctness**
    // *For any* f64 value, Ssim::new should succeed if and only if the value
    // is within [0.0, 1.0].
    // **Validates: Requirements 2.1, 2.2**
    // ========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn ssim_validation_property(value in -2.0f64..2.0f64) {
            let result = Ssim::new(value);
            let in_range = value >= 0.0 && value <= 1.0;
            prop_assert_eq!(result.is_ok(), in_range,
                "SSIM {} should be {} but was {}",
                value,
                if in_range { "valid" } else { "invalid" },
                if result.is_ok() { "valid" } else { "invalid" }
            );
        }
    }

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 4: SSIM Display Precision**
    // *For any* valid Ssim value, the display string should contain exactly
    // 6 decimal places.
    // **Validates: Requirements 2.4**
    // ========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn ssim_display_precision_property(value in 0.0f64..1.0f64) {
            let ssim = Ssim::new(value).unwrap();
            let display = ssim.display();

            // 格式应该是 "X.XXXXXX"，小数点后 6 位
            let parts: Vec<&str> = display.split('.').collect();
            prop_assert_eq!(parts.len(), 2, "Display should have decimal point");
            prop_assert_eq!(parts[1].len(), 6,
                "Display '{}' should have 6 decimal places, got {}",
                display, parts[1].len()
            );
        }
    }

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 5: FileSize Saturating Arithmetic**
    // *For any* two FileSize values a and b, a.saturating_sub(b) should return
    // FileSize(0) when b > a, and a - b otherwise.
    // **Validates: Requirements 3.1**
    // ========================================================================
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

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 6: FileSize Compression Ratio Safety**
    // *For any* FileSize values, compression_ratio should return None for zero
    // original, and Some(ratio) otherwise where 0.0 <= ratio.
    // **Validates: Requirements 3.2**
    // ========================================================================
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
                let r = ratio.unwrap();
                prop_assert!(r >= 0.0,
                    "compression_ratio should be >= 0, got {}", r
                );
            }
        }
    }

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 7: IterationGuard Termination**
    // *For any* IterationGuard, calling increment() more than max times should
    // return Err(IterationLimitExceeded).
    // **Validates: Requirements 6.1**
    // ========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn iteration_guard_termination_property(max in 1u32..100) {
            let mut guard = IterationGuard::new(max, "test");

            // 前 max 次应该成功
            for i in 1..=max {
                let result = guard.increment();
                prop_assert!(result.is_ok(),
                    "Iteration {} of {} should succeed", i, max
                );
            }

            // 第 max+1 次应该失败
            let result = guard.increment();
            prop_assert!(result.is_err(),
                "Iteration {} of {} should fail", max + 1, max
            );
        }
    }
}
