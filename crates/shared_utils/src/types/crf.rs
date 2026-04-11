//! CRF (Constant Rate Factor) Type-Safe Wrapper
//!
//! Provides compile-time range validation for CRF values.
//!
//! ## Design Principles
//! - Use generic `Crf<E>` to distinguish CRF ranges for different encoders
//! - `EncoderBounds` trait defines encoder-specific boundaries
//! - Validated at creation, no redundant runtime checks needed

use crate::float_compare::approx_eq_f32;
use std::fmt;
use std::marker::PhantomData;

pub use crate::crf_constants::CRF_CACHE_KEY_MULTIPLIER;
pub use crate::float_compare::CRF_EPSILON;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HevcEncoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1Encoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vp9Encoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X264Encoder;

pub trait EncoderBounds: Clone + Copy {
    const MIN: f32;
    const MAX: f32;
    const DEFAULT: f32;
    const VISUALLY_LOSSLESS: f32;
    const NAME: &'static str;
}

impl EncoderBounds for HevcEncoder {
    const MIN: f32 = 0.0;
    const MAX: f32 = 51.0;
    const DEFAULT: f32 = 23.0;
    const VISUALLY_LOSSLESS: f32 = 18.0;
    const NAME: &'static str = "HEVC";
}

impl EncoderBounds for Av1Encoder {
    const MIN: f32 = 0.0;
    const MAX: f32 = 63.0;
    const DEFAULT: f32 = 30.0;
    const VISUALLY_LOSSLESS: f32 = 20.0;
    const NAME: &'static str = "AV1";
}

impl EncoderBounds for Vp9Encoder {
    const MIN: f32 = 0.0;
    const MAX: f32 = 63.0;
    const DEFAULT: f32 = 31.0;
    const VISUALLY_LOSSLESS: f32 = 20.0;
    const NAME: &'static str = "VP9";
}

impl EncoderBounds for X264Encoder {
    const MIN: f32 = 0.0;
    const MAX: f32 = 51.0;
    const DEFAULT: f32 = 23.0;
    const VISUALLY_LOSSLESS: f32 = 18.0;
    const NAME: &'static str = "x264";
}

#[derive(Debug, Clone, PartialEq)]
pub enum CrfError {
    OutOfRange {
        value: f32,
        min: f32,
        max: f32,
        encoder: &'static str,
    },
    InvalidCacheKey {
        key: u32,
        encoder: &'static str,
    },
    InvalidFloat {
        encoder: &'static str,
    },
}

impl fmt::Display for CrfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange {
                value,
                min,
                max,
                encoder,
            } => {
                write!(
                    f,
                    "{encoder} CRF {value:.2} out of range [{min:.1}, {max:.1}]"
                )
            }
            Self::InvalidCacheKey { key, encoder } => {
                write!(f, "Invalid {encoder} CRF cache key: {key}")
            }
            Self::InvalidFloat { encoder } => {
                write!(f, "Invalid {encoder} CRF: NaN or Infinity")
            }
        }
    }
}

impl std::error::Error for CrfError {}

#[derive(Clone, Copy)]
pub struct Crf<E: EncoderBounds> {
    value: f32,
    _marker: PhantomData<E>,
}

impl<E: EncoderBounds> Crf<E> {
    /// Create a new CRF value.
    ///
    /// # Errors
    /// Returns an error if the value is out of range.
    pub fn new(value: f32) -> Result<Self, CrfError> {
        if value.is_nan() || value.is_infinite() {
            return Err(CrfError::InvalidFloat { encoder: E::NAME });
        }

        if value < E::MIN || value > E::MAX {
            return Err(CrfError::OutOfRange {
                value,
                min: E::MIN,
                max: E::MAX,
                encoder: E::NAME,
            });
        }

        Ok(Self {
            value,
            _marker: PhantomData,
        })
    }

    #[must_use]
    pub const fn default_value() -> Self {
        Self {
            value: E::DEFAULT,
            _marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn visually_lossless() -> Self {
        Self {
            value: E::VISUALLY_LOSSLESS,
            _marker: PhantomData,
        }
    }

    #[inline]
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    #[inline]
    #[must_use]
    pub fn to_cache_key(&self) -> u32 {
        crate::numeric_cast::f32_to_u32_sat((self.value * CRF_CACHE_KEY_MULTIPLIER).round())
    }

    /// Create a CRF value from a cache key.
    ///
    /// # Errors
    /// Returns an error if the key is invalid.
    pub fn from_cache_key(key: u32) -> Result<Self, CrfError> {
        let value = crate::numeric_cast::u32_to_f32(key) / CRF_CACHE_KEY_MULTIPLIER;
        Self::new(value).map_err(|_| CrfError::InvalidCacheKey {
            key,
            encoder: E::NAME,
        })
    }

    #[inline]
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        approx_eq_f32(self.value, other.value)
    }

    #[inline]
    #[must_use]
    pub const fn encoder_name(&self) -> &'static str {
        E::NAME
    }

    #[inline]
    #[must_use]
    pub const fn valid_range() -> (f32, f32) {
        (E::MIN, E::MAX)
    }

    #[must_use]
    pub const fn clamped(value: f32) -> Self {
        let clamped = if value.is_nan() || value.is_infinite() {
            E::DEFAULT
        } else {
            value.clamp(E::MIN, E::MAX)
        };
        Self {
            value: clamped,
            _marker: PhantomData,
        }
    }
}

impl<E: EncoderBounds> fmt::Debug for Crf<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Crf<{}>({:.2})", E::NAME, self.value)
    }
}

impl<E: EncoderBounds> fmt::Display for Crf<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1}", self.value)
    }
}

impl<E: EncoderBounds> PartialEq for Crf<E> {
    fn eq(&self, other: &Self) -> bool {
        self.approx_eq(other)
    }
}

impl<E: EncoderBounds> Default for Crf<E> {
    fn default() -> Self {
        Self::default_value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hevc_crf_valid_range() {
        assert!(Crf::<HevcEncoder>::new(0.0).is_ok());
        assert!(Crf::<HevcEncoder>::new(51.0).is_ok());
        assert!(Crf::<HevcEncoder>::new(23.0).is_ok());

        assert!(Crf::<HevcEncoder>::new(-1.0).is_err());
        assert!(Crf::<HevcEncoder>::new(52.0).is_err());
    }

    #[test]
    fn test_av1_crf_valid_range() {
        assert!(Crf::<Av1Encoder>::new(0.0).is_ok());
        assert!(Crf::<Av1Encoder>::new(63.0).is_ok());
        assert!(Crf::<Av1Encoder>::new(30.0).is_ok());

        assert!(Crf::<Av1Encoder>::new(-1.0).is_err());
        assert!(Crf::<Av1Encoder>::new(64.0).is_err());
    }

    #[test]
    fn test_crf_nan_inf() {
        assert!(Crf::<HevcEncoder>::new(f32::NAN).is_err());
        assert!(Crf::<HevcEncoder>::new(f32::INFINITY).is_err());
        assert!(Crf::<HevcEncoder>::new(f32::NEG_INFINITY).is_err());
    }

    #[test]
    fn test_crf_cache_key_round_trip() {
        let original = Crf::<HevcEncoder>::new(23.5).unwrap();
        let key = original.to_cache_key();
        let recovered = Crf::<HevcEncoder>::from_cache_key(key).unwrap();
        assert!(original.approx_eq(&recovered));
    }

    #[test]
    fn test_crf_default() {
        let hevc = Crf::<HevcEncoder>::default();
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(hevc.value()),
            23.0
        ));

        let av1 = Crf::<Av1Encoder>::default();
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(av1.value()),
            30.0
        ));
    }

    #[test]
    fn test_crf_clamped() {
        let clamped = Crf::<HevcEncoder>::clamped(100.0);
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(clamped.value()),
            51.0
        ));

        let clamped_nan = Crf::<HevcEncoder>::clamped(f32::NAN);
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(clamped_nan.value()),
            23.0
        ));
    }

    #[test]
    fn test_crf_display() {
        let crf = Crf::<HevcEncoder>::new(23.5).unwrap();
        assert_eq!(format!("{crf}"), "23.5");
        assert_eq!(format!("{crf:?}"), "Crf<HEVC>(23.50)");
    }
}
