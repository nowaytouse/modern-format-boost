//! SSIM (Structural Similarity Index) Type-Safe Wrapper
//!
//! 提供编译期保证的 SSIM 值范围验证 (0.0-1.0)。

use std::fmt;

/// SSIM 专用 epsilon（比通用 F64_EPSILON 更宽松）
/// 用于 SSIM 值的近似相等比较
pub const SSIM_EPSILON: f64 = 1e-4;

/// SSIM 最小值
pub const SSIM_MIN: f64 = 0.0;

/// SSIM 最大值
pub const SSIM_MAX: f64 = 1.0;

/// SSIM 显示精度（小数位数）
pub const SSIM_DISPLAY_PRECISION: usize = 6;

// ============================================================================
// SsimError
// ============================================================================

/// SSIM 错误类型
#[derive(Debug, Clone, PartialEq)]
pub enum SsimError {
    /// SSIM 值超出有效范围 [0.0, 1.0]
    OutOfRange { value: f64 },
    /// NaN 或 Inf 值
    InvalidFloat,
}

impl fmt::Display for SsimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SsimError::OutOfRange { value } => {
                write!(f, "SSIM {:.6} out of range [0.0, 1.0]", value)
            }
            SsimError::InvalidFloat => {
                write!(f, "Invalid SSIM: NaN or Infinity")
            }
        }
    }
}

impl std::error::Error for SsimError {}

// ============================================================================
// Ssim Newtype
// ============================================================================

/// 类型安全的 SSIM 值
///
/// SSIM (Structural Similarity Index) 范围为 [0.0, 1.0]，
/// 其中 1.0 表示完全相同，0.0 表示完全不同。
///
/// # Examples
/// ```
/// use shared_utils::types::ssim::Ssim;
///
/// let ssim = Ssim::new(0.95).unwrap();
/// assert_eq!(ssim.value(), 0.95);
/// assert_eq!(ssim.display(), "0.950000");
///
/// // 超出范围会返回错误
/// assert!(Ssim::new(1.5).is_err());
/// assert!(Ssim::new(-0.1).is_err());
/// ```
#[derive(Clone, Copy)]
pub struct Ssim(f64);

impl Ssim {
    /// 完美 SSIM（完全相同）
    pub const PERFECT: Ssim = Ssim(1.0);

    /// 零 SSIM（完全不同）
    pub const ZERO: Ssim = Ssim(0.0);

    /// 创建 SSIM 值，验证范围
    ///
    /// # Arguments
    /// * `value` - SSIM 值
    ///
    /// # Returns
    /// * `Ok(Ssim)` - 如果值在 [0.0, 1.0] 范围内
    /// * `Err(SsimError)` - 如果值超出范围或为 NaN/Inf
    pub fn new(value: f64) -> Result<Self, SsimError> {
        // 检查 NaN 和 Inf
        if value.is_nan() || value.is_infinite() {
            return Err(SsimError::InvalidFloat);
        }

        // 检查范围
        if value < SSIM_MIN || value > SSIM_MAX {
            return Err(SsimError::OutOfRange { value });
        }

        Ok(Self(value))
    }

    /// 获取原始 SSIM 值
    #[inline]
    pub fn value(&self) -> f64 {
        self.0
    }

    /// 近似相等比较
    ///
    /// 使用 SSIM_EPSILON 进行比较，处理浮点精度问题。
    #[inline]
    pub fn approx_eq(&self, other: &Self) -> bool {
        (self.0 - other.0).abs() < SSIM_EPSILON
    }

    /// 格式化显示（6 位小数）
    pub fn display(&self) -> String {
        format!("{:.6}", self.0)
    }

    /// 检查是否达到阈值
    ///
    /// 使用 SSIM_EPSILON 进行容差比较。
    #[inline]
    pub fn meets_threshold(&self, threshold: f64) -> bool {
        self.0 >= threshold - SSIM_EPSILON
    }

    /// 钳制到有效范围（不返回错误）
    pub fn clamped(value: f64) -> Self {
        let clamped = if value.is_nan() || value.is_infinite() {
            0.0
        } else {
            value.clamp(SSIM_MIN, SSIM_MAX)
        };
        Self(clamped)
    }

    /// 转换为百分比字符串
    pub fn as_percent(&self) -> String {
        format!("{:.2}%", self.0 * 100.0)
    }

    /// 获取质量描述
    pub fn quality_description(&self) -> &'static str {
        if self.0 >= 0.99 {
            "Excellent (visually lossless)"
        } else if self.0 >= 0.95 {
            "Very Good"
        } else if self.0 >= 0.90 {
            "Good"
        } else if self.0 >= 0.80 {
            "Fair"
        } else {
            "Poor"
        }
    }
}

// ============================================================================
// Trait Implementations
// ============================================================================

impl fmt::Debug for Ssim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ssim({:.6})", self.0)
    }
}

impl fmt::Display for Ssim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6}", self.0)
    }
}

impl PartialEq for Ssim {
    fn eq(&self, other: &Self) -> bool {
        self.approx_eq(other)
    }
}

impl PartialOrd for Ssim {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssim_valid_range() {
        assert!(Ssim::new(0.0).is_ok());
        assert!(Ssim::new(1.0).is_ok());
        assert!(Ssim::new(0.5).is_ok());
        assert!(Ssim::new(0.95).is_ok());
    }

    #[test]
    fn test_ssim_invalid_range() {
        assert!(Ssim::new(-0.1).is_err());
        assert!(Ssim::new(1.1).is_err());
        assert!(Ssim::new(-1.0).is_err());
        assert!(Ssim::new(2.0).is_err());
    }

    #[test]
    fn test_ssim_nan_inf() {
        assert!(Ssim::new(f64::NAN).is_err());
        assert!(Ssim::new(f64::INFINITY).is_err());
        assert!(Ssim::new(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn test_ssim_display_precision() {
        let ssim = Ssim::new(0.123456789).unwrap();
        let display = ssim.display();
        // 应该有 6 位小数
        assert_eq!(display, "0.123457"); // 四舍五入
        assert_eq!(display.len(), 8); // "0." + 6 digits
    }

    #[test]
    fn test_ssim_meets_threshold() {
        let ssim = Ssim::new(0.95).unwrap();
        assert!(ssim.meets_threshold(0.95));
        assert!(ssim.meets_threshold(0.94));
        assert!(!ssim.meets_threshold(0.96));
    }

    #[test]
    fn test_ssim_clamped() {
        let clamped = Ssim::clamped(1.5);
        assert_eq!(clamped.value(), 1.0);

        let clamped_neg = Ssim::clamped(-0.5);
        assert_eq!(clamped_neg.value(), 0.0);

        let clamped_nan = Ssim::clamped(f64::NAN);
        assert_eq!(clamped_nan.value(), 0.0);
    }

    #[test]
    fn test_ssim_quality_description() {
        assert_eq!(
            Ssim::new(0.99).unwrap().quality_description(),
            "Excellent (visually lossless)"
        );
        assert_eq!(Ssim::new(0.95).unwrap().quality_description(), "Very Good");
        assert_eq!(Ssim::new(0.90).unwrap().quality_description(), "Good");
        assert_eq!(Ssim::new(0.80).unwrap().quality_description(), "Fair");
        assert_eq!(Ssim::new(0.70).unwrap().quality_description(), "Poor");
    }
}
