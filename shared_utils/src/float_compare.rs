//! Float Comparison Module
//!
//! Provides unified floating-point comparison utilities with consistent epsilon values.
//! 统一的浮点数比较工具，避免精度问题导致的 bug。

pub const F64_EPSILON: f64 = 1e-6;

pub const F32_EPSILON: f32 = 1e-4;

#[inline]
pub fn approx_eq_f64(a: f64, b: f64) -> bool {
    (a - b).abs() < F64_EPSILON
}

#[inline]
pub fn approx_eq_f32(a: f32, b: f32) -> bool {
    (a - b).abs() < F32_EPSILON
}

#[inline]
pub fn approx_zero_f64(a: f64) -> bool {
    a.abs() < F64_EPSILON
}

#[inline]
pub fn approx_zero_f32(a: f32) -> bool {
    a.abs() < F32_EPSILON
}

#[inline]
pub fn approx_le_f64(a: f64, b: f64) -> bool {
    a < b + F64_EPSILON
}

#[inline]
pub fn approx_ge_f64(a: f64, b: f64) -> bool {
    a > b - F64_EPSILON
}

pub const SSIM_EPSILON: f64 = 1e-4;

pub const CRF_EPSILON: f32 = 0.01;

pub const PSNR_EPSILON: f64 = 0.1;

#[inline]
pub fn approx_eq_ssim(a: f64, b: f64) -> bool {
    (a - b).abs() < SSIM_EPSILON
}

#[inline]
pub fn approx_eq_crf(a: f32, b: f32) -> bool {
    (a - b).abs() < CRF_EPSILON
}

#[inline]
pub fn approx_eq_psnr(a: f64, b: f64) -> bool {
    (a - b).abs() < PSNR_EPSILON
}

#[inline]
pub fn ssim_meets_threshold(ssim: f64, threshold: f64) -> bool {
    ssim >= threshold - SSIM_EPSILON
}

#[inline]
pub fn ssim_below_threshold(ssim: f64, threshold: f64) -> bool {
    ssim < threshold - SSIM_EPSILON
}

#[inline]
pub fn crf_in_range(crf: f32, min: f32, max: f32) -> bool {
    crf >= min - CRF_EPSILON && crf <= max + CRF_EPSILON
}

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
        assert!(approx_eq_f64(1.0, 1.0 + 1e-7));
        assert!(approx_eq_f64(1.0, 1.0 - 1e-7));
    }

    #[test]
    fn test_approx_eq_f64_outside_epsilon() {
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
        assert!(approx_le_f64(1.0, 1.0 + 1e-7));
        assert!(approx_le_f64(0.9, 1.0));
        assert!(!approx_le_f64(1.1, 1.0));
    }

    #[test]
    fn test_approx_ge_f64() {
        assert!(approx_ge_f64(1.0, 1.0));
        assert!(approx_ge_f64(1.0, 1.0 - 1e-7));
        assert!(approx_ge_f64(1.1, 1.0));
        assert!(!approx_ge_f64(0.9, 1.0));
    }

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
        assert!(ssim_meets_threshold(0.9499, 0.95));
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

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

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
