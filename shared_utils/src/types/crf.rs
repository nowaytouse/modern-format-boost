//! CRF (Constant Rate Factor) Type-Safe Wrapper
//!
//! 提供编译期保证的 CRF 值范围验证。
//! 
//! ## 设计原理
//! - 使用泛型 `Crf<E>` 区分不同编码器的 CRF 范围
//! - `EncoderBounds` trait 定义编码器特定的边界
//! - 创建时验证，运行时无需重复检查

use std::marker::PhantomData;
use std::fmt;
use crate::float_compare::approx_eq_f32;

/// CRF 缓存键乘数（用于整数键生成）
pub const CRF_CACHE_KEY_MULTIPLIER: f32 = 100.0;

/// CRF 专用 epsilon（用于近似相等比较）
pub const CRF_EPSILON: f32 = 0.01;

// ============================================================================
// Encoder Marker Types
// ============================================================================

/// HEVC/H.265 编码器标记
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HevcEncoder;

/// AV1 编码器标记
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1Encoder;

/// VP9 编码器标记
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vp9Encoder;

/// x264/H.264 编码器标记
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X264Encoder;

// ============================================================================
// EncoderBounds Trait
// ============================================================================

/// 编码器边界约束 trait
/// 
/// 定义每种编码器的 CRF 有效范围和默认值。
pub trait EncoderBounds: Clone + Copy {
    /// 最小 CRF（最高质量，通常为 0 = 无损）
    const MIN: f32;
    /// 最大 CRF（最低质量）
    const MAX: f32;
    /// 默认 CRF（推荐质量）
    const DEFAULT: f32;
    /// 视觉无损 CRF
    const VISUALLY_LOSSLESS: f32;
    /// 编码器名称（用于错误消息）
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

// ============================================================================
// CrfError
// ============================================================================

/// CRF 错误类型
#[derive(Debug, Clone, PartialEq)]
pub enum CrfError {
    /// CRF 值超出有效范围
    OutOfRange {
        value: f32,
        min: f32,
        max: f32,
        encoder: &'static str,
    },
    /// 无效的缓存键
    InvalidCacheKey {
        key: u32,
        encoder: &'static str,
    },
    /// NaN 或 Inf 值
    InvalidFloat {
        encoder: &'static str,
    },
}

impl fmt::Display for CrfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CrfError::OutOfRange { value, min, max, encoder } => {
                write!(f, "{} CRF {:.2} out of range [{:.1}, {:.1}]", encoder, value, min, max)
            }
            CrfError::InvalidCacheKey { key, encoder } => {
                write!(f, "Invalid {} CRF cache key: {}", encoder, key)
            }
            CrfError::InvalidFloat { encoder } => {
                write!(f, "Invalid {} CRF: NaN or Infinity", encoder)
            }
        }
    }
}

impl std::error::Error for CrfError {}

// ============================================================================
// Crf<E> Newtype
// ============================================================================

/// 类型安全的 CRF 值
/// 
/// 泛型参数 `E` 指定编码器类型，决定有效的 CRF 范围。
/// 
/// # Examples
/// ```
/// use shared_utils::types::crf::{Crf, HevcEncoder, Av1Encoder};
/// 
/// // HEVC CRF (0-51)
/// let hevc_crf = Crf::<HevcEncoder>::new(23.0).unwrap();
/// assert_eq!(hevc_crf.value(), 23.0);
/// 
/// // AV1 CRF (0-63)
/// let av1_crf = Crf::<Av1Encoder>::new(30.0).unwrap();
/// assert_eq!(av1_crf.value(), 30.0);
/// 
/// // 超出范围会返回错误
/// assert!(Crf::<HevcEncoder>::new(60.0).is_err());
/// ```
#[derive(Clone, Copy)]
pub struct Crf<E: EncoderBounds> {
    value: f32,
    _marker: PhantomData<E>,
}

impl<E: EncoderBounds> Crf<E> {
    /// 创建 CRF 值，验证范围
    /// 
    /// # Arguments
    /// * `value` - CRF 值
    /// 
    /// # Returns
    /// * `Ok(Crf)` - 如果值在有效范围内
    /// * `Err(CrfError)` - 如果值超出范围或为 NaN/Inf
    pub fn new(value: f32) -> Result<Self, CrfError> {
        // 检查 NaN 和 Inf
        if value.is_nan() || value.is_infinite() {
            return Err(CrfError::InvalidFloat { encoder: E::NAME });
        }
        
        // 检查范围
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
    
    /// 创建默认 CRF 值
    pub fn default_value() -> Self {
        Self {
            value: E::DEFAULT,
            _marker: PhantomData,
        }
    }
    
    /// 创建视觉无损 CRF 值
    pub fn visually_lossless() -> Self {
        Self {
            value: E::VISUALLY_LOSSLESS,
            _marker: PhantomData,
        }
    }
    
    /// 获取原始 CRF 值
    #[inline]
    pub fn value(&self) -> f32 {
        self.value
    }
    
    /// 转换为缓存键（处理精度）
    /// 
    /// 将 CRF 值乘以 100 并取整，用于 HashMap 键。
    /// 例如：CRF 23.5 → 2350
    #[inline]
    pub fn to_cache_key(&self) -> u32 {
        (self.value * CRF_CACHE_KEY_MULTIPLIER).round() as u32
    }
    
    /// 从缓存键恢复 CRF 值
    /// 
    /// # Arguments
    /// * `key` - 缓存键
    /// 
    /// # Returns
    /// * `Ok(Crf)` - 如果恢复的值在有效范围内
    /// * `Err(CrfError)` - 如果恢复的值超出范围
    pub fn from_cache_key(key: u32) -> Result<Self, CrfError> {
        let value = key as f32 / CRF_CACHE_KEY_MULTIPLIER;
        Self::new(value).map_err(|_| CrfError::InvalidCacheKey {
            key,
            encoder: E::NAME,
        })
    }
    
    /// 近似相等比较
    /// 
    /// 使用 CRF_EPSILON 进行比较，处理浮点精度问题。
    #[inline]
    pub fn approx_eq(&self, other: &Self) -> bool {
        approx_eq_f32(self.value, other.value)
    }
    
    /// 获取编码器名称
    #[inline]
    pub fn encoder_name(&self) -> &'static str {
        E::NAME
    }
    
    /// 获取有效范围
    #[inline]
    pub fn valid_range() -> (f32, f32) {
        (E::MIN, E::MAX)
    }
    
    /// 钳制到有效范围（不返回错误）
    /// 
    /// 用于需要保证有效值但不想处理错误的场景。
    pub fn clamped(value: f32) -> Self {
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

// ============================================================================
// Trait Implementations
// ============================================================================

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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hevc_crf_valid_range() {
        // 边界值测试
        assert!(Crf::<HevcEncoder>::new(0.0).is_ok());
        assert!(Crf::<HevcEncoder>::new(51.0).is_ok());
        assert!(Crf::<HevcEncoder>::new(23.0).is_ok());
        
        // 超出范围
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
        assert_eq!(hevc.value(), 23.0);
        
        let av1 = Crf::<Av1Encoder>::default();
        assert_eq!(av1.value(), 30.0);
    }

    #[test]
    fn test_crf_clamped() {
        let clamped = Crf::<HevcEncoder>::clamped(100.0);
        assert_eq!(clamped.value(), 51.0);
        
        let clamped_nan = Crf::<HevcEncoder>::clamped(f32::NAN);
        assert_eq!(clamped_nan.value(), 23.0); // 默认值
    }

    #[test]
    fn test_crf_display() {
        let crf = Crf::<HevcEncoder>::new(23.5).unwrap();
        assert_eq!(format!("{}", crf), "23.5");
        assert_eq!(format!("{:?}", crf), "Crf<HEVC>(23.50)");
    }
}
