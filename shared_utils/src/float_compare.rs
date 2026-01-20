//! Float Comparison Module
//!
//! Provides unified floating-point comparison utilities with consistent epsilon values.
//! 统一的浮点数比较工具，避免精度问题导致的 bug。

/// Epsilon for f64 comparisons (1e-6)
/// 用于 f64 比较的容差值
pub const F64_EPSILON: f64 = 1e-6;

/// Epsilon for f32 comparisons (1e-4)
/// 用于 f32 比较的容差值
pub const F32_EPSILON: f32 = 1e-4;

/// Check if two f64 values are approximately equal
/// 检查两个 f64 值是否近似相等
#[inline]
pub fn approx_eq_f64(a: f64, b: f64) -> bool {
    (a - b).abs() < F64_EPSILON
}

/// Check if two f32 values are approximately equal
/// 检查两个 f32 值是否近似相等
#[inline]
pub fn approx_eq_f32(a: f32, b: f32) -> bool {
    (a - b).abs() < F32_EPSILON
}

/// Check if an f64 value is approximately zero
/// 检查 f64 值是否近似为零
#[inline]
pub fn approx_zero_f64(a: f64) -> bool {
    a.abs() < F64_EPSILON
}

/// Check if an f32 value is approximately zero
/// 检查 f32 值是否近似为零
#[inline]
pub fn approx_zero_f32(a: f32) -> bool {
    a.abs() < F32_EPSILON
}

/// Check if a is approximately less than or equal to b (f64)
/// 检查 a 是否近似小于等于 b
#[inline]
pub fn approx_le_f64(a: f64, b: f64) -> bool {
    a < b + F64_EPSILON
}

/// Check if a is approximately greater than or equal to b (f64)
/// 检查 a 是否近似大于等于 b
#[inline]
pub fn approx_ge_f64(a: f64, b: f64) -> bool {
    a > b - F64_EPSILON
}

// ============================================================================
// 🔥 v7.1: Domain-Specific Epsilon Values
// ============================================================================

/// SSIM 专用 epsilon（比通用 F64_EPSILON 更宽松）
/// SSIM 值通常在 0.9-1.0 范围内，需要更宽松的比较
pub const SSIM_EPSILON: f64 = 1e-4;

/// CRF 专用 epsilon（用于缓存键比较）
/// CRF 值通常是整数或 0.5 步进，0.01 足够精确
pub const CRF_EPSILON: f32 = 0.01;

/// PSNR 专用 epsilon（dB 单位）
pub const PSNR_EPSILON: f64 = 0.1;

// ============================================================================
// 🔥 v7.1: Domain-Specific Comparison Functions
// ============================================================================

/// 比较两个 SSIM 值是否近似相等
///
/// 使用 SSIM_EPSILON (1e-4) 进行比较。
#[inline]
pub fn approx_eq_ssim(a: f64, b: f64) -> bool {
    (a - b).abs() < SSIM_EPSILON
}

/// 比较两个 CRF 值是否近似相等
///
/// 使用 CRF_EPSILON (0.01) 进行比较。
#[inline]
pub fn approx_eq_crf(a: f32, b: f32) -> bool {
    (a - b).abs() < CRF_EPSILON
}

/// 比较两个 PSNR 值是否近似相等
///
/// 使用 PSNR_EPSILON (0.1 dB) 进行比较。
#[inline]
pub fn approx_eq_psnr(a: f64, b: f64) -> bool {
    (a - b).abs() < PSNR_EPSILON
}

/// 检查 SSIM 是否达到阈值
///
/// 使用 SSIM_EPSILON 进行容差比较。
/// 例如：ssim_meets_threshold(0.9499, 0.95) 返回 true
#[inline]
pub fn ssim_meets_threshold(ssim: f64, threshold: f64) -> bool {
    ssim >= threshold - SSIM_EPSILON
}

/// 检查 SSIM 是否严格低于阈值
///
/// 使用 SSIM_EPSILON 进行容差比较。
#[inline]
pub fn ssim_below_threshold(ssim: f64, threshold: f64) -> bool {
    ssim < threshold - SSIM_EPSILON
}

/// 检查 CRF 是否在有效范围内
///
/// # Arguments
/// * `crf` - CRF 值
/// * `min` - 最小值（包含）
/// * `max` - 最大值（包含）
#[inline]
pub fn crf_in_range(crf: f32, min: f32, max: f32) -> bool {
    crf >= min - CRF_EPSILON && crf <= max + CRF_EPSILON
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approx_eq_f64_equal() {
        assert!(approx_eq_f64(1.0, 1.0));
        assert!(approx_eq_f64(0.0, 0.0));
        assert!(approx_eq_f64(-1.0, -1.0));
    }

    #[test]
    fn test_approx_eq_f64_within_epsilon() {
        // Values within epsilon should be equal
        assert!(approx_eq_f64(1.0, 1.0 + 1e-7));
        assert!(approx_eq_f64(1.0, 1.0 - 1e-7));
    }

    #[test]
    fn test_approx_eq_f64_outside_epsilon() {
        // Values outside epsilon should not be equal
        assert!(!approx_eq_f64(1.0, 1.0 + 1e-5));
        assert!(!approx_eq_f64(1.0, 1.0 - 1e-5));
    }

    #[test]
    fn test_approx_eq_f32_equal() {
        assert!(approx_eq_f32(1.0, 1.0));
        assert!(approx_eq_f32(0.0, 0.0));
    }

    #[test]
    fn test_approx_eq_f32_within_epsilon() {
        assert!(approx_eq_f32(1.0, 1.0 + 1e-5));
        assert!(approx_eq_f32(1.0, 1.0 - 1e-5));
    }

    #[test]
    fn test_approx_eq_f32_outside_epsilon() {
        assert!(!approx_eq_f32(1.0, 1.0 + 1e-3));
        assert!(!approx_eq_f32(1.0, 1.0 - 1e-3));
    }

    #[test]
    fn test_approx_zero_f64() {
        assert!(approx_zero_f64(0.0));
        assert!(approx_zero_f64(1e-7));
        assert!(approx_zero_f64(-1e-7));
        assert!(!approx_zero_f64(1e-5));
        assert!(!approx_zero_f64(-1e-5));
    }

    #[test]
    fn test_approx_zero_f32() {
        assert!(approx_zero_f32(0.0));
        assert!(approx_zero_f32(1e-5));
        assert!(approx_zero_f32(-1e-5));
        assert!(!approx_zero_f32(1e-3));
        assert!(!approx_zero_f32(-1e-3));
    }

    #[test]
    fn test_approx_le_f64() {
        assert!(approx_le_f64(1.0, 1.0));
        assert!(approx_le_f64(1.0, 1.0 + 1e-7)); // within epsilon
        assert!(approx_le_f64(0.9, 1.0));
        assert!(!approx_le_f64(1.1, 1.0));
    }

    #[test]
    fn test_approx_ge_f64() {
        assert!(approx_ge_f64(1.0, 1.0));
        assert!(approx_ge_f64(1.0, 1.0 - 1e-7)); // within epsilon
        assert!(approx_ge_f64(1.1, 1.0));
        assert!(!approx_ge_f64(0.9, 1.0));
    }

    // Property test: symmetry
    #[test]
    fn test_approx_eq_symmetry() {
        let values = [0.0, 1.0, -1.0, 0.5, 100.0, -100.0, 1e-7, 1e-5];
        for &a in &values {
            for &b in &values {
                assert_eq!(
                    approx_eq_f64(a, b),
                    approx_eq_f64(b, a),
                    "Symmetry failed for {} and {}",
                    a,
                    b
                );
            }
        }
    }

    // Property test: reflexivity
    #[test]
    fn test_approx_eq_reflexivity() {
        let values = [
            0.0,
            1.0,
            -1.0,
            0.5,
            100.0,
            -100.0,
            f64::MIN_POSITIVE,
            f64::MAX / 2.0,
        ];
        for &a in &values {
            assert!(approx_eq_f64(a, a), "Reflexivity failed for {}", a);
        }
    }

    // ========================================================================
    // 🔥 v7.1: Domain-Specific Tests
    // ========================================================================

    #[test]
    fn test_approx_eq_ssim() {
        assert!(approx_eq_ssim(0.95, 0.95));
        assert!(approx_eq_ssim(0.95, 0.95 + 1e-5));
        assert!(!approx_eq_ssim(0.95, 0.96));
    }

    #[test]
    fn test_approx_eq_crf() {
        assert!(approx_eq_crf(23.0, 23.0));
        assert!(approx_eq_crf(23.0, 23.005));
        assert!(!approx_eq_crf(23.0, 23.5));
    }

    #[test]
    fn test_ssim_meets_threshold() {
        assert!(ssim_meets_threshold(0.95, 0.95));
        assert!(ssim_meets_threshold(0.9499, 0.95)); // within epsilon
        assert!(ssim_meets_threshold(0.96, 0.95));
        assert!(!ssim_meets_threshold(0.94, 0.95));
    }

    #[test]
    fn test_crf_in_range() {
        assert!(crf_in_range(23.0, 0.0, 51.0));
        assert!(crf_in_range(0.0, 0.0, 51.0));
        assert!(crf_in_range(51.0, 0.0, 51.0));
        assert!(!crf_in_range(52.0, 0.0, 51.0));
        assert!(!crf_in_range(-1.0, 0.0, 51.0));
    }
}

// ============================================================================
// 🔥 v7.1: Property-Based Tests
// ============================================================================

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 8: Float Comparison Symmetry**
    // *For any* two f64 values a and b, approx_eq_f64(a, b) == approx_eq_f64(b, a).
    // **Validates: Requirements 5.1**
    // ========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn float_comparison_symmetry_property(a in -1000.0f64..1000.0f64, b in -1000.0f64..1000.0f64) {
            prop_assert_eq!(
                approx_eq_f64(a, b),
                approx_eq_f64(b, a),
                "Symmetry failed for {} and {}", a, b
            );
        }

        #[test]
        fn ssim_comparison_symmetry_property(a in 0.0f64..1.0f64, b in 0.0f64..1.0f64) {
            prop_assert_eq!(
                approx_eq_ssim(a, b),
                approx_eq_ssim(b, a),
                "SSIM symmetry failed for {} and {}", a, b
            );
        }

        #[test]
        fn crf_comparison_symmetry_property(a in 0.0f32..63.0f32, b in 0.0f32..63.0f32) {
            prop_assert_eq!(
                approx_eq_crf(a, b),
                approx_eq_crf(b, a),
                "CRF symmetry failed for {} and {}", a, b
            );
        }
    }

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 9: Float Comparison Reflexivity**
    // *For any* f64 value a (excluding NaN), approx_eq_f64(a, a) == true.
    // **Validates: Requirements 5.1**
    // ========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn float_comparison_reflexivity_property(a in -1000.0f64..1000.0f64) {
            prop_assert!(
                approx_eq_f64(a, a),
                "Reflexivity failed for {}", a
            );
        }

        #[test]
        fn ssim_comparison_reflexivity_property(a in 0.0f64..1.0f64) {
            prop_assert!(
                approx_eq_ssim(a, a),
                "SSIM reflexivity failed for {}", a
            );
        }

        #[test]
        fn crf_comparison_reflexivity_property(a in 0.0f32..63.0f32) {
            prop_assert!(
                approx_eq_crf(a, a),
                "CRF reflexivity failed for {}", a
            );
        }
    }
}
