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
                assert_eq!(approx_eq_f64(a, b), approx_eq_f64(b, a),
                    "Symmetry failed for {} and {}", a, b);
            }
        }
    }

    // Property test: reflexivity
    #[test]
    fn test_approx_eq_reflexivity() {
        let values = [0.0, 1.0, -1.0, 0.5, 100.0, -100.0, f64::MIN_POSITIVE, f64::MAX / 2.0];
        for &a in &values {
            assert!(approx_eq_f64(a, a), "Reflexivity failed for {}", a);
        }
    }
}
