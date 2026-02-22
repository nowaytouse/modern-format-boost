//! SSIM (Structural Similarity Index) Type-Safe Wrapper
//!
//! 提供编译期保证的 SSIM 值范围验证 (0.0-1.0)。

use std::fmt;

pub use crate::float_compare::SSIM_EPSILON;

pub const SSIM_MIN: f64 = 0.0;

pub const SSIM_MAX: f64 = 1.0;

pub const SSIM_DISPLAY_PRECISION: usize = 6;


#[derive(Debug, Clone, PartialEq)]
pub enum SsimError {
    OutOfRange { value: f64 },
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


#[derive(Clone, Copy)]
pub struct Ssim(f64);

impl Ssim {
    pub const PERFECT: Ssim = Ssim(1.0);

    pub const ZERO: Ssim = Ssim(0.0);

    pub fn new(value: f64) -> Result<Self, SsimError> {
        if value.is_nan() || value.is_infinite() {
            return Err(SsimError::InvalidFloat);
        }

        if !(SSIM_MIN..=SSIM_MAX).contains(&value) {
            return Err(SsimError::OutOfRange { value });
        }

        Ok(Self(value))
    }

    #[inline]
    pub fn value(&self) -> f64 {
        self.0
    }

    #[inline]
    pub fn approx_eq(&self, other: &Self) -> bool {
        (self.0 - other.0).abs() < SSIM_EPSILON
    }

    pub fn display(&self) -> String {
        format!("{:.6}", self.0)
    }

    #[inline]
    pub fn meets_threshold(&self, threshold: f64) -> bool {
        self.0 >= threshold - SSIM_EPSILON
    }

    pub fn clamped(value: f64) -> Self {
        let clamped = if value.is_nan() || value.is_infinite() {
            0.0
        } else {
            value.clamp(SSIM_MIN, SSIM_MAX)
        };
        Self(clamped)
    }

    pub fn as_percent(&self) -> String {
        format!("{:.2}%", self.0 * 100.0)
    }

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
        assert_eq!(display, "0.123457");
        assert_eq!(display.len(), 8);
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
