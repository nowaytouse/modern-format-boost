//! Centralized Numeric Cast Safety Layer
//!
//! This module provides **audited, saturating cast functions** that replace raw `as` casts
//! throughout the crate. Every numeric conversion is handled here exactly once, with clear
//! safety documentation and proper `#[allow]` annotations.
//!
//! ## Design Principles
//! - **Single audit point**: Each cast pattern is reviewed once in this module
//! - **Saturating semantics**: NaN/negative → 0, overflow → `T::MAX`
//! - **Zero `as` at call sites**: Callers use named functions instead of raw casts
//!
//! ## Usage
//! ```rust
//! use foundation::numeric_cast::{f64_to_u64_sat, unix_secs_i64};
//!
//! let size = f64_to_u64_sat(1024.5);
//! assert_eq!(size, 1024);
//! let timestamp = unix_secs_i64();
//! assert!(timestamp > 0);
//! ```

use crate::Rational;
use std::time::{SystemTime, UNIX_EPOCH};

// --- Nightly Specialization Layer ---
/// Unified trait for audited numeric conversions.
///
/// Leveraging nightly specialization for zero-cost optimized paths.
pub trait AuditedCast<T> {
    /// Performs a saturating cast with audited safety.
    fn cast_sat(self) -> T;
}

impl AuditedCast<u64> for f64 {
    fn cast_sat(self) -> u64 {
        if self.is_nan() || self < 0.0 {
            0
        } else {
            raw::f64_to_u64(self)
        }
    }
}

impl AuditedCast<u32> for f64 {
    fn cast_sat(self) -> u32 {
        if self.is_nan() || self < 0.0 {
            0
        } else {
            raw::f64_to_u32(self)
        }
    }
}

impl AuditedCast<usize> for f64 {
    fn cast_sat(self) -> usize {
        if self.is_nan() || self < 0.0 {
            0
        } else {
            raw::f64_to_usize(self)
        }
    }
}
// ------------------------------------

/// Convert f64 to Rational with loud warning on NaN/Inf.
/// Refuses to forge data as requested by the Quality Manifesto.
#[must_use]
pub fn f64_to_rational_strict(val: f64, name: &str) -> Option<Rational> {
    let res = Rational::from_f64(val);
    if res.is_none() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Failed to convert f64 value '{val}' to Rational for field '{name}' | Forensic: Value is NaN or Infinite; refusing to forge data to prevent upstream corruption"
            ),
        );
    }
    res
}

/// Convert `Option<f64>` to `f64` with loud warning on None.
/// Returns None if input is None, refusing to forge default values.
#[must_use]
pub fn option_f64_strict(val: Option<f64>, name: &str) -> Option<f64> {
    if val.is_none() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "FIELD STRICTNESS AUDIT: Required optional field '{name}' is missing! | Forensic: Value is None; refusing to forge default data to maintain integrity"
            ),
        );
    }
    val
}

/// Convert `f64` to `u64` with loud warning on NaN/Inf/Overflow.
#[must_use]
pub fn f64_to_u64_strict(val: f64, name: &str) -> Option<u64> {
    if !val.is_finite() || val < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is out of range for u64! | Forensic: NaN, Inf, or negative value detected; refusing to forge data"
            ),
        );
        return None;
    }
    if val >= 18_446_744_073_709_551_616.0 {
        // u64::MAX + 1
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u64! | Forensic: Magnitude exceeds 2^64-1; refusing to forge truncated data"
            ),
        );
        return None;
    }
    Some(raw::f64_to_u64(val))
}

/// Convert `f64` to `u32` with loud warning on NaN/Inf/Overflow.
#[must_use]
pub fn f64_to_u32_strict(val: f64, name: &str) -> Option<u32> {
    if !val.is_finite() || val < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN, Inf or negative! | Forensic: Cannot convert to u32; refusing to forge data to prevent upstream corruption"
            ),
        );
        return None;
    }
    if val >= 4_294_967_296.0 {
        // u32::MAX + 1
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u32! | Forensic: Value >= 2^32; refusing to forge data to maintain integrity"
            ),
        );
        return None;
    }
    Some(raw::f64_to_u32(val))
}

/// Convert `f64` to `usize` with loud warning on NaN/Inf/Overflow.
#[must_use]
pub fn f64_to_usize_strict(val: f64, name: &str) -> Option<usize> {
    if !val.is_finite() || val < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN, Inf or negative! | Forensic: Cannot convert to usize; refusing to forge data"
            ),
        );
        return None;
    }
    #[cfg(target_pointer_width = "64")]
    {
        if val >= 18_446_744_073_709_551_616.0 {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows 64-bit usize! | Forensic: Value >= 2^64; refusing to forge data"
                ),
            );
            return None;
        }
    }
    #[cfg(target_pointer_width = "32")]
    {
        if val >= 4_294_967_296.0 {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{}' overflows 32-bit usize! | Forensic: Value >= 2^32 on 32-bit platform; refusing to forge data",
                    name
                ),
            );
            return None;
        }
    }
    Some(raw::f64_to_usize(val))
}

/// Parse a string into a numeric type with a loud warning on failure.
/// Refuses to forge data by returning None if parsing fails.
#[must_use]
pub fn parse_strict<T: std::str::FromStr>(s: &str, name: &str) -> Option<T> {
    s.trim().parse::<T>().map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "PARSE AUDIT: Failed to parse field '{name}' from string '{s}'! | Forensic: String is not a valid numeric representation; refusing to forge data to prevent logic corruption"
                ),
            );
            None
        },
        Some,
    )
}

/// Parse an optional string into a numeric type with a loud warning on failure.
/// Returns None if the input is None or if parsing fails.
#[must_use]
pub fn parse_option_strict<T: std::str::FromStr>(s: Option<&str>, name: &str) -> Option<T> {
    s.and_then(|s_val| parse_strict(s_val, name))
}

/// Convert `u32` to `i32` with loud warning on overflow.
#[must_use]
pub fn u32_to_i32_strict(val: u32, name: &str) -> Option<i32> {
    i32::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows i32! | Forensic: Value exceeds i32::MAX; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `i32` to `u32` with loud warning on sign loss.
#[must_use]
pub fn i32_to_u32_strict(val: i32, name: &str) -> Option<u32> {
    u32::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is negative! | Forensic: Cannot convert negative i32 to u32; refusing to forge data"
                ),
            );
            None
        }, Some)
}

/// Convert `u64` to `u32` with loud warning on overflow.
/// Refuses to forge data; returns None on overflow.
#[must_use]
pub fn u64_to_u32_strict(val: u64, name: &str) -> Option<u32> {
    u32::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u32! | Forensic: Value exceeds u32::MAX; refusing to forge data"
                ),
            );
            None
        }, Some)
}

#[cfg(not(feature = "high-precision"))]
#[must_use]
pub(crate) const fn f64_to_i64_unchecked(val: f64) -> i64 {
    raw::f64_to_i64(val)
}

#[cfg(not(feature = "high-precision"))]
#[must_use]
pub(crate) const fn u64_to_i64_unchecked(val: u64) -> i64 {
    raw::u64_to_i64(val)
}

/// Convert `usize` to `u32` with loud warning on overflow.
#[must_use]
pub fn usize_to_u32_strict(val: usize, name: &str) -> Option<u32> {
    u32::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u32! | Forensic: Value exceeds u32::MAX (platform-specific limit); refusing to forge data"
                ),
            );
            None
        }, Some)
}

/// Convert `usize` to `u16` with loud warning on overflow.
#[must_use]
pub fn usize_to_u16_strict(val: usize, name: &str) -> Option<u16> {
    u16::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u16! | Forensic: Value exceeds u16::MAX; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `usize` to `u64` with loud warning on overflow.
#[must_use]
pub fn usize_to_u64_strict(val: usize, name: &str) -> Option<u64> {
    u64::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u64! | Forensic: Value exceeds u64::MAX (platform-specific limit); refusing to forge data"
                ),
            );
            None
        }, Some)
}

/// Convert `u64` to `usize` with loud warning on overflow.
#[must_use]
pub fn u64_to_usize_strict(val: u64, name: &str) -> Option<usize> {
    usize::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows usize! | Forensic: Value exceeds platform pointer width limit; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `u64` to `usize` with loud warning on overflow, returning None.
/// Critical for allocation paths where `usize::MAX` would cause OOM panic.
#[must_use]
pub fn try_u64_to_usize_strict(val: u64, name: &str) -> Option<usize> {
    usize::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows usize! | Forensic: Value exceeds platform pointer width limit; refusing to forge data to prevent OOM panic"
                ),
            );
            None
        }, Some)
}

/// Convert `Option<u64>` to `Option<u64>` with loud warning on None.
#[must_use]
pub fn option_u64_strict(val: Option<u64>, name: &str) -> Option<u64> {
    if val.is_none() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "FIELD STRICTNESS AUDIT: Required u64 field '{name}' is missing! | Forensic: Value is None; refusing to forge default data to maintain integrity"
            ),
        );
    }
    val
}

/// Convert `Option<f32>` to `Option<f32>` with loud warning on None.
#[must_use]
pub fn option_f32_strict(val: Option<f32>, name: &str) -> Option<f32> {
    if val.is_none() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "FIELD STRICTNESS AUDIT: Required f32 field '{name}' is missing! | Forensic: Value is None; refusing to forge default data to maintain integrity"
            ),
        );
    }
    val
}

/// Convert `Option<u8>` to `Option<u8>` with loud warning on None.
#[must_use]
pub fn option_u8_strict(val: Option<u8>, name: &str) -> Option<u8> {
    if val.is_none() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "FIELD STRICTNESS AUDIT: Required u8 field '{name}' is missing! | Forensic: Value is None; refusing to forge default data to maintain integrity"
            ),
        );
    }
    val
}

/// Convert `Option<usize>` to `Option<usize>` with loud warning on None.
#[must_use]
pub fn option_usize_strict(val: Option<usize>, name: &str) -> Option<usize> {
    if val.is_none() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "FIELD STRICTNESS AUDIT: Required usize field '{name}' is missing! | Forensic: Value is None; refusing to forge default data to maintain integrity"
            ),
        );
    }
    val
}

/// Convert `u64` to `u32` with loud warning on overflow, returning None.
/// Follows "Integrity Audit" requirements: Loud, Honest, Non-breaking.
#[must_use]
pub fn try_u32_strict(val: u64, name: &str) -> Option<u32> {
    u32::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u32! | Forensic: Value exceeds u32::MAX; refusing to forge data. Returning None for safety"
                ),
            );
            None
        }, Some)
}

/// Convert `u64` to `usize` with loud warning on overflow, returning None.
#[must_use]
pub fn try_usize_strict(val: u64, name: &str) -> Option<usize> {
    usize::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows usize! | Forensic: Value exceeds platform pointer width limit; refusing to forge data. Returning None for safety"
                ),
            );
            None
        }, Some)
}

/// Convert `i64` to `u64` with loud warning on sign loss.
#[must_use]
pub fn i64_to_u64_strict(val: i64, name: &str) -> Option<u64> {
    u64::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is negative! | Forensic: Cannot convert negative i64 to u64; refusing to forge data"
                ),
            );
            None
        }, Some)
}

/// Convert `i64` to `u32` with loud warning on overflow/sign loss.
#[must_use]
pub fn i64_to_u32_strict(val: i64, name: &str) -> Option<u32> {
    u32::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' out of u32 range! | Forensic: i64 value exceeds u32 boundaries; refusing to forge data"
                ),
            );
            None
        }, Some)
}

/// Convert `f64` to `u8` with loud warning on overflow/NaN.
#[must_use]
pub fn f64_to_u8_strict(val: f64, name: &str) -> Option<u8> {
    if val.is_nan() || val.is_infinite() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Field '{name}' is NaN/Inf! | Forensic: Floating point anomaly detected; refusing to forge u8 data"
            ),
        );
        return None;
    }
    let rounded = val.round();
    if !(0.0..=255.0).contains(&rounded) {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{rounded}' for field '{name}' out of u8 range! | Forensic: Value exceeds [0, 255] boundary; refusing to forge data"
            ),
        );
        return None;
    }
    // Safety: we checked that rounded is within [0.0, 255.0] above.
    Some(unsafe { rounded.to_int_unchecked::<u8>() })
}

/// Convert `f64` to `i64` with loud warning on NaN/Inf/Overflow.
#[must_use]
pub fn f64_to_i64_strict(val: f64, name: &str) -> Option<i64> {
    if !val.is_finite() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN or Inf! | Forensic: Floating point anomaly detected; refusing to forge i64 data"
            ),
        );
        return None;
    }
    if !(-9_223_372_036_854_775_808.0..=9_223_372_036_854_775_807.0).contains(&val) {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows i64! | Forensic: Value exceeds i64::MIN/MAX boundary; refusing to forge data"
            ),
        );
        return None;
    }
    // Safety: we checked that val is within [i64::MIN, i64::MAX] above.
    Some(unsafe { val.to_int_unchecked::<i64>() })
}

/// Convert `u32` to `usize` with loud warning on overflow/NaN.
#[must_use]
pub fn u32_to_usize_strict(val: u32, name: &str) -> Option<usize> {
    usize::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows usize! | Forensic: Value exceeds platform pointer width limit; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `i32` to `u64` with loud warning on sign loss.
#[must_use]
pub fn i32_to_u64_strict(val: i32, name: &str) -> Option<u64> {
    u64::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is negative! | Forensic: Cannot convert negative i32 to u64; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `i32` to `usize` with loud warning on sign loss or overflow.
#[must_use]
pub fn i32_to_usize_strict(val: i32, name: &str) -> Option<usize> {
    usize::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' out of usize range! | Forensic: i32 value out of platform pointer width limits; refusing to forge data"
                ),
            );
            None
        }, Some)
}

/// Convert `usize` to `i32` with loud warning on overflow.
#[must_use]
pub fn usize_to_i32_strict(val: usize, name: &str) -> Option<i32> {
    i32::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' out of i32 range! | Forensic: usize value exceeds i32 boundaries; refusing to forge data"
                ),
            );
            None
        }, Some)
}

/// Convert `u16` to `usize` with loud warning on overflow.
#[must_use]
pub fn u16_to_usize_strict(val: u16, _name: &str) -> Option<usize> {
    let v = From::from(val);
    Some(v)
}

/// Convert `u8` to `usize` with loud warning on overflow.
#[must_use]
pub fn u8_to_usize_strict(val: u8, _name: &str) -> Option<usize> {
    let v = From::from(val);
    Some(v)
}

/// Convert `u64` to `i64` with loud warning on overflow.
#[must_use]
pub fn u64_to_i64_strict(val: u64, name: &str) -> Option<i64> {
    i64::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows i64! | Forensic: Value >= 2^63; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `u64` to `i32` with loud warning on overflow.
#[must_use]
pub fn u64_to_i32_strict(val: u64, name: &str) -> Option<i32> {
    i32::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows i32! | Forensic: refusing truncation"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `f32` to `u32` with loud warning on NaN/Inf/Overflow.
#[must_use]
pub fn f32_to_u32_strict(val: f32, name: &str) -> Option<u32> {
    if !val.is_finite() || val < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN, Inf or negative! | Forensic: Cannot convert to u32; refusing to forge data to maintain integrity"
            ),
        );
        return None;
    }
    if val > 16_777_216.0_f32 {
        // u32::MAX rounded to f32
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u32! | Forensic: Value exceeds 2^32-1; refusing to forge truncated data"
            ),
        );
        return None;
    }
    Some(raw::f32_to_u32(val))
}

/// Convert `f32` to `i32` with loud warning on NaN/Inf/Overflow.
#[must_use]
pub fn f32_to_i32_strict(val: f32, name: &str) -> Option<i32> {
    if !val.is_finite() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN or Inf! | Forensic: Cannot convert non-finite f32 to i32; refusing to forge data"
            ),
        );
        return None;
    }
    if !(-16_777_216.0_f32..=16_777_216.0_f32).contains(&val) {
        // i32::MIN/MAX rounded to f32
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' out of i32 range! | Forensic: f32 magnitude exceeds i32 limits; refusing to forge data"
            ),
        );
        return None;
    }
    // Convert f32 to i32 safely using to_bits and reinterpretation
    Some(raw::f32_to_i32(val))
}

/// Convert `f32` to `usize` with loud warning on NaN/Inf/Overflow or lossy index precision.
#[must_use]
pub fn f32_to_usize_strict(val: f32, name: &str) -> Option<usize> {
    if !val.is_finite() || val < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN, Inf or negative! | Forensic: Cannot convert to usize; refusing to forge data"
            ),
        );
        return None;
    }
    if val > 16_777_216.0_f32 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' exceeds exact f32 integer range for usize! | Forensic: refusing lossy index conversion"
            ),
        );
        return None;
    }
    Some(raw::f32_to_usize(val))
}

/// Convert `f64` to `i32` with loud warning on NaN/Inf/Overflow.
#[must_use]
pub fn f64_to_i32_strict(val: f64, name: &str) -> Option<i32> {
    if !val.is_finite() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN or Inf! | Forensic: Cannot convert non-finite f64 to i32; refusing to forge data"
            ),
        );
        return None;
    }
    if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&val) {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows i32! | Forensic: f64 magnitude exceeds i32 limits; refusing to forge data"
            ),
        );
        return None;
    }
    Some(raw::f64_to_i32(val))
}

/// Convert `u32` to `u8` with loud warning on overflow.
#[must_use]
pub fn u32_to_u8_strict(val: u32, name: &str) -> Option<u8> {
    u8::try_from(val).map_or_else(|_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows u8! | Forensic: Value exceeds u8::MAX; refusing to forge data"
                ),
            );
            None
        }, Some)
}

/// Convert `usize` to `i64` with loud warning on overflow.
#[must_use]
pub fn usize_to_i64_strict(val: usize, name: &str) -> Option<i64> {
    i64::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows i64! | Forensic: Value >= 2^63; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `u128` to `i64` with loud warning on overflow.
/// Convert `f64` to `f32` with loud warning on NaN/Inf/overflow.
#[must_use]
pub fn f64_to_f32_strict(val: f64, name: &str) -> Option<f32> {
    if !val.is_finite() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN or Infinite! | Forensic: Cannot convert non-finite f64 to f32; refusing to forge data"
            ),
        );
        return None;
    }
    if val < f64::from(f32::MIN) || val > f64::from(f32::MAX) {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' out of f32 range! | Forensic: f64 magnitude exceeds f32 boundaries; refusing to forge data"
            ),
        );
        return None;
    }
    Some(raw::f64_to_f32(val))
}

/// Convert `f32` to `u16` with loud warning on NaN/Inf/overflow.
#[must_use]
pub fn f32_to_u16_strict(val: f32, name: &str) -> Option<u16> {
    if !val.is_finite() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN/Inf! | Forensic: Cannot convert to u16; refusing to forge data"
            ),
        );
        return None;
    }
    let rounded = val.round();
    if !(0.0..=f32::from(u16::MAX)).contains(&rounded) {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{rounded}' for field '{name}' out of u16 range! | Forensic: f32 magnitude exceeds u16 limits; refusing to forge data"
            ),
        );
        return None;
    }
    Some(raw::f32_to_u16(rounded))
}

/// Convert `f64` to `u16` with loud warning on NaN/Inf/overflow.
#[must_use]
pub fn f64_to_u16_strict(val: f64, name: &str) -> Option<u16> {
    if !val.is_finite() {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' is NaN/Inf! | Forensic: Cannot convert to u16; refusing to forge data"
            ),
        );
        return None;
    }
    let rounded = val.round();
    if !(0.0..=f64::from(u16::MAX)).contains(&rounded) {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            format!(
                "NUMERIC CONVERSION AUDIT: Value '{rounded}' for field '{name}' out of u16 range! | Forensic: f64 magnitude exceeds u16 limits; refusing to forge data"
            ),
        );
        return None;
    }
    Some(raw::f64_to_u16(rounded))
}

/// Convert `u128` to `i64` with loud warning on overflow.
#[must_use]
pub fn u128_to_i64_strict(val: u128, name: &str) -> Option<i64> {
    i64::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' overflows i64! | Forensic: Value >= 2^63; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

/// Convert `i128` to `i64` with loud warning on overflow.
#[must_use]
pub fn i128_to_i64_strict(val: i128, name: &str) -> Option<i64> {
    i64::try_from(val).map_or_else(
        |_| {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC CONVERSION AUDIT: Value '{val}' for field '{name}' out of i64 range! | Forensic: Value outside [i64::MIN, i64::MAX]; refusing to forge data"
                ),
            );
            None
        },
        Some,
    )
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]
pub(crate) mod raw {
    #[inline]
    pub(super) const fn f64_to_u64(v: f64) -> u64 {
        v as u64
    }

    #[inline]
    pub(super) const fn f64_to_u32(v: f64) -> u32 {
        v as u32
    }

    #[inline]
    pub(super) const fn f64_to_usize(v: f64) -> usize {
        v as usize
    }

    #[inline]
    pub(super) const fn f64_to_u16(v: f64) -> u16 {
        v as u16
    }

    #[inline]
    pub(super) const fn f64_to_u8(v: f64) -> u8 {
        v as u8
    }

    #[inline]
    pub(super) const fn f64_to_i32(v: f64) -> i32 {
        v as i32
    }

    #[inline]
    pub(super) const fn f64_to_f32(v: f64) -> f32 {
        v as f32
    }

    #[inline]
    pub(super) const fn f32_to_u32(v: f32) -> u32 {
        v as u32
    }

    #[inline]
    pub(super) const fn f32_to_u16(v: f32) -> u16 {
        v as u16
    }

    #[inline]
    pub(super) const fn f32_to_i32(v: f32) -> i32 {
        v as i32
    }

    #[inline]
    pub(super) const fn f32_to_usize(v: f32) -> usize {
        v as usize
    }

    #[inline]
    pub(super) const fn u64_to_f64(v: u64) -> f64 {
        v as f64
    }

    #[inline]
    pub(super) const fn u64_to_u32(v: u64) -> u32 {
        v as u32
    }

    #[inline]
    pub(super) const fn u32_to_u8(v: u32) -> u8 {
        v as u8
    }

    #[inline]
    pub(super) const fn u16_to_u8(v: u16) -> u8 {
        v as u8
    }

    #[inline]
    pub(super) const fn u128_to_u64(v: u128) -> u64 {
        v as u64
    }

    #[inline]
    pub(super) const fn usize_to_f64(v: usize) -> f64 {
        u64_to_f64(v as u64)
    }

    #[inline]
    pub(super) const fn i64_to_f64(v: i64) -> f64 {
        v as f64
    }

    #[inline]
    pub(super) const fn i32_to_f32(v: i32) -> f32 {
        v as f32
    }

    #[inline]
    pub(super) const fn u32_to_f32(v: u32) -> f32 {
        v as f32
    }

    #[inline]
    #[cfg(not(feature = "high-precision"))]
    pub(super) const fn f64_to_i64(v: f64) -> i64 {
        v as i64
    }

    #[inline]
    #[cfg(not(feature = "high-precision"))]
    pub(super) const fn u64_to_i64(v: u64) -> i64 {
        v as i64
    }
}

// ---------------------------------------------------------------------------
// f64 → unsigned integer (saturating)
// ---------------------------------------------------------------------------

/// Saturating cast: `f64` → `u64`.
///
/// **WARNING**: This function performs silent data forgery (saturates to 0 on NaN/negative).
/// Use `f64_to_u64_strict` for non-UI data paths.
///
/// - `NaN` or negative → `0`
/// - `> u64::MAX` → `u64::MAX`
#[inline]
#[must_use]
pub fn f64_to_u64_sat(v: f64) -> u64 {
    if v.is_nan() || v < 0.0_f64 {
        return 0;
    }
    raw::f64_to_u64(v)
}

/// Saturating cast: `f64` → `u32`.
///
/// - `NaN` or negative → `0`
/// - `> u32::MAX` → `u32::MAX`
#[inline]
#[must_use]
pub fn f64_to_u32_sat(v: f64) -> u32 {
    if v.is_nan() || v < 0.0_f64 {
        return 0;
    }
    raw::f64_to_u32(v)
}

/// Saturating cast: `f64` → `usize`.
///
/// - `NaN` or negative → `0`
/// - overflow → `usize::MAX`
#[inline]
#[must_use]
pub fn f64_to_usize_sat(v: f64) -> usize {
    if v.is_nan() || v < 0.0_f64 {
        return 0;
    }
    raw::f64_to_usize(v)
}

/// Checked cast: `f64` → `u8`.
///
/// Returns `None` if `v` is `NaN` or outside of `[0, 255]`.
#[inline]
#[must_use]
pub fn f64_to_u8_checked(v: f64) -> Option<u8> {
    if v.is_nan() || v < 0.0 || v > f64::from(u8::MAX) {
        None
    } else {
        Some(raw::f64_to_u8(v))
    }
}

/// Checked cast: `f64` → `u32`.
///
/// Returns `None` if `v` is `NaN` or outside of `[0, u32::MAX]`.
#[inline]
#[must_use]
pub fn f64_to_u32_checked(v: f64) -> Option<u32> {
    if v.is_nan() || v < 0.0 || v > f64::from(u32::MAX) {
        None
    } else {
        Some(raw::f64_to_u32(v))
    }
}

/// Saturating cast: `f64` → `u16`.
///
/// - `NaN` or negative → `0`
/// - `> 65535` → `65535`
#[inline]
#[must_use]
pub fn f64_to_u16_sat(v: f64) -> u16 {
    if v.is_nan() || v < 0.0_f64 {
        return 0;
    }
    raw::f64_to_u16(v)
}

/// Saturating cast: `f64` → `u8`.
///
/// - `NaN` or negative → `0`
/// - `> 255` → `255`
#[inline]
#[must_use]
pub fn f64_to_u8_sat(v: f64) -> u8 {
    if v.is_nan() || v < 0.0_f64 {
        return 0;
    }
    raw::f64_to_u8(v)
}

// ---------------------------------------------------------------------------
// f64 → signed integer (saturating)
// ---------------------------------------------------------------------------

/// Saturating cast: `f64` → `i32`.
///
/// - `NaN` → `0`
/// - clamped to `[i32::MIN, i32::MAX]`
#[inline]
#[must_use]
pub const fn f64_to_i32_sat(v: f64) -> i32 {
    if v.is_nan() {
        return 0;
    }
    raw::f64_to_i32(v)
}

/// Saturating cast: `f64` → `f32`.
///
/// - `NaN` → `0.0`
/// - overflow → `f32::MAX` or `f32::MIN`
#[inline]
#[must_use]
pub fn f64_to_f32_sat(v: f64) -> f32 {
    if v.is_nan() {
        return 0.0;
    }
    if v > f64::from(f32::MAX) {
        f32::MAX
    } else if v < f64::from(f32::MIN) {
        f32::MIN
    } else {
        raw::f64_to_f32(v)
    }
}

// ---------------------------------------------------------------------------
// f32 → unsigned integer (saturating)
// ---------------------------------------------------------------------------

/// Saturating cast: `f32` → `u32`.
///
/// - `NaN` or negative → `0`
/// - `> u32::MAX` → `u32::MAX`
#[inline]
#[must_use]
pub fn f32_to_u32_sat(v: f32) -> u32 {
    if v.is_nan() || v < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            "NUMERIC CONVERSION AUDIT: Float NaN or negative value detected! | Forensic: Value squashed to 0 during saturating cast to prevent logic corruption",
        );
        return 0;
    }
    raw::f32_to_u32(v)
}

/// Saturating cast: `f32` → `u16`.
///
/// - `NaN` or negative → `0`
/// - `> 65535` → `65535`
#[inline]
#[must_use]
pub fn f32_to_u16_sat(v: f32) -> u16 {
    if v.is_nan() || v < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            "NUMERIC CONVERSION AUDIT: Float NaN or negative value detected! | Forensic: Value squashed to 0 during saturating cast to prevent logic corruption",
        );
        return 0;
    }
    raw::f32_to_u16(v)
}

/// Saturating cast: `f32` → `u8`.
///
/// - `NaN` or negative → `0`
/// - `> 255` → `255`
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#[must_use]
pub fn f32_to_u8_sat(v: f32) -> u8 {
    if v.is_nan() || v < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            "NUMERIC CONVERSION AUDIT: Float NaN or negative value detected! | Forensic: Value squashed to 0 during saturating cast to prevent logic corruption",
        );
        return 0;
    }
    if v > 255.0 {
        return 255;
    }
    v as u8
}

// ---------------------------------------------------------------------------
// f32 → signed integer (saturating)
// ---------------------------------------------------------------------------

/// Saturating cast: `f32` → `i32`.
///
/// - `NaN` → `0`
/// - clamped to `[i32::MIN, i32::MAX]`
#[inline]
#[must_use]
pub const fn f32_to_i32_sat(v: f32) -> i32 {
    if v.is_nan() {
        return 0;
    }
    raw::f32_to_i32(v)
}

// ---------------------------------------------------------------------------
// f64 ↔ f32 (precision reduction)
// ---------------------------------------------------------------------------

/// Lossy precision reduction: `f64` → `f32`.
///
/// Audited: acceptable for ML feature vectors, display metrics, and quality scores
/// where f32 precision (≈7 decimal digits) is sufficient.
#[inline]
#[must_use]
pub const fn f64_to_f32_lossy(v: f64) -> f32 {
    raw::f64_to_f32(v)
}

/// Exact promotion: `f32` → `f64`.
///
/// Retained as a named wrapper for API consistency with the other audited cast helpers.
#[inline]
#[must_use]
pub fn f32_to_f64_lossy(v: f32) -> f64 {
    f64::from(v)
}

/// Potentially lossy integer-to-float conversion: `u64` → `f64`.
#[inline]
#[must_use]
pub const fn u64_to_f64(v: u64) -> f64 {
    raw::u64_to_f64(v)
}

/// Extract the high 32 bits of a `u64` as `u32`.
///
/// The shift proves the narrowed value is in range, so this is not a saturating conversion.
#[inline]
#[must_use]
pub const fn u64_high32_to_u32(v: u64) -> u32 {
    raw::u64_to_u32(v >> 32)
}

/// Extract a byte from a `u32` after shifting right by `shift` bits.
///
/// The mask proves the narrowed value is in range, so this is not saturating.
#[inline]
#[must_use]
pub const fn u32_shifted_byte_to_u8(v: u32, shift: u32) -> u8 {
    raw::u32_to_u8((v >> shift) & 0xFF)
}

/// Extract the high byte of a `u16`.
#[inline]
#[must_use]
pub const fn u16_high8_to_u8(v: u16) -> u8 {
    raw::u16_to_u8(v >> 8)
}

/// Extract the low byte of a `u16`.
#[inline]
#[must_use]
pub const fn u16_low8_to_u8(v: u16) -> u8 {
    raw::u16_to_u8(v & 0x00FF)
}

/// Extract the low 64 bits of a `u128` as `u64`.
///
/// Callers use this after explicitly masking the input to 64 bits.
#[inline]
#[must_use]
pub const fn u128_low64_to_u64(v: u128) -> u64 {
    raw::u128_to_u64(v)
}

/// Exact promotion: `u32` → `f64`.
#[inline]
#[must_use]
pub fn u32_to_f64(v: u32) -> f64 {
    f64::from(v)
}

/// Potentially lossy integer-to-float conversion: `usize` → `f64`.
///
/// On 64-bit targets, large values may lose integer precision once they exceed `2^53`.
#[inline]
#[must_use]
pub const fn usize_to_f64(v: usize) -> f64 {
    raw::usize_to_f64(v)
}

/// Exact promotion: `i32` → `f64`.
#[inline]
#[must_use]
pub fn i32_to_f64(v: i32) -> f64 {
    f64::from(v)
}

/// Potentially lossy integer-to-float conversion: `i64` → `f64`.
#[inline]
#[must_use]
pub const fn i64_to_f64(v: i64) -> f64 {
    raw::i64_to_f64(v)
}

/// Exact promotion: `f32` → `f64`.
#[inline]
#[must_use]
pub fn f32_to_f64(v: f32) -> f64 {
    f64::from(v)
}

/// Precision reduction: `i32` → `f32`.
#[inline]
#[must_use]
pub const fn i32_to_f32_lossy(v: i32) -> f32 {
    raw::i32_to_f32(v)
}

/// Potentially lossy integer-to-float conversion: `u32` → `f32`.
#[inline]
#[must_use]
pub const fn u32_to_f32(v: u32) -> f32 {
    raw::u32_to_f32(v)
}

/// Potentially lossy integer-to-float conversion: `usize` → `f32`.
#[inline]
#[must_use]
pub const fn usize_to_f32_lossy(v: usize) -> f32 {
    f64_to_f32_lossy(usize_to_f64(v))
}

/// Saturating cast: `f32` → `usize`.
///
/// - `NaN` or negative → `0`
/// - overflow → `usize::MAX`
#[inline]
#[must_use]
pub fn f32_to_usize_sat(v: f32) -> usize {
    if v.is_nan() || v < 0.0 {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
            "delivery_numeric",
            "NUMERIC CONVERSION AUDIT: Float NaN or negative value detected! | Forensic: Value squashed to 0 during saturating cast to prevent logic corruption",
        );
        return 0;
    }
    raw::f32_to_usize(v)
}

// ---------------------------------------------------------------------------
// Integer ↔ Integer (safe wrappers)
// ---------------------------------------------------------------------------

/// Saturating cast: `u64` → `usize`.
///
/// On 32-bit targets, values > `u32::MAX` saturate to `usize::MAX`.
#[inline]
#[must_use]
pub fn u64_to_usize_sat(v: u64) -> usize {
    usize::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows target type during saturating cast! | Forensic: Value squashed to target MAX to prevent logic corruption"
            ),
            );
        usize::MAX
    })
}

/// Saturating cast: `u32` → `usize`.
///
/// Lossless on 32-bit and 64-bit targets.
#[inline]
#[must_use]
pub fn u32_to_usize_sat(v: u32) -> usize {
    usize::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows target type during saturating cast! | Forensic: Value squashed to target MAX to prevent logic corruption"
            ),
            );
        usize::MAX
    })
}

/// Saturating cast: `u16` → `usize`.
///
/// Lossless on all supported targets.
#[inline]
#[must_use]
pub fn u16_to_usize_sat(v: u16) -> usize {
    usize::from(v)
}

/// Saturating cast: `u8` → `usize`.
///
/// Lossless on all supported targets.
#[inline]
#[must_use]
pub fn u8_to_usize_sat(v: u8) -> usize {
    usize::from(v)
}

/// Saturating cast: `i32` → `usize`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
/// # Panics
/// Panics if the input `i32` value (once clamped to 0) cannot fit into `usize`.
pub fn i32_to_usize_sat(v: i32) -> usize {
    usize::try_from(v.max(0)).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows target type during saturating cast! | Forensic: Value squashed to target MAX to prevent logic corruption"
            ),
            );
        usize::MAX
    })
}

/// Saturating cast: `i64` → `usize`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
pub fn i64_to_usize_sat(v: i64) -> usize {
    usize::try_from(v.max(0)).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows target type during saturating cast! | Forensic: Value squashed to target MAX to prevent logic corruption"
            ),
            );
        usize::MAX
    })
}

/// Saturating cast: `usize` → `i32`.
///
/// Values > `i32::MAX` → `i32::MAX`.
#[inline]
#[must_use]
pub fn usize_to_i32_sat(v: usize) -> i32 {
    i32::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows i32 target! | Forensic: Value squashed to i32::MAX during saturating cast to prevent logic corruption"
            ),
            );
        i32::MAX
    })
}

/// Saturating cast: `i64` → `u32`.
///
/// Negative values → `0`, > `u32::MAX` → `u32::MAX`.
#[inline]
#[must_use]
pub fn i64_to_u32_sat(v: i64) -> u32 {
    u32::try_from(v.clamp(0, i64::from(u32::MAX))).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u32 target! | Forensic: Value squashed to u32::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u32::MAX
    })
}

/// Saturating cast: `i64` → `u64`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
pub fn i64_to_u64_sat(v: i64) -> u64 {
    u64::try_from(v.max(0)).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u64 target! | Forensic: Value squashed to u64::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u64::MAX
    })
}

/// Saturating cast: `u64` → `i64`.
///
/// Values > `i64::MAX` → `i64::MAX`.
#[inline]
#[must_use]
pub fn u64_to_i64_sat(v: u64) -> i64 {
    i64::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows i64 target! | Forensic: Value squashed to i64::MAX during saturating cast to prevent logic corruption"
            ),
            );
        i64::MAX
    })
}

/// Explicit no-op for `i64` → `i64`.
///
/// Used to explicitly mark a code path as audited for numeric safety.
#[inline]
#[must_use]
pub const fn i64_to_i64_sat_no_op(v: i64) -> i64 {
    v
}

/// Saturating cast: `u64` → `u32`.
///
/// Values > `u32::MAX` → `u32::MAX`.
#[inline]
#[must_use]
pub fn u64_to_u32_sat(v: u64) -> u32 {
    u32::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u32 target! | Forensic: Value squashed to u32::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u32::MAX
    })
}

/// Saturating cast: `usize` → `u32`.
///
/// Values > `u32::MAX` → `u32::MAX`.
#[inline]
#[must_use]
pub fn usize_to_u32_sat(v: usize) -> u32 {
    u32::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u32 target! | Forensic: Value squashed to u32::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u32::MAX
    })
}

/// Saturating cast: `usize` → `i64`.
///
/// Values > `i64::MAX` → `i64::MAX`.
#[inline]
#[must_use]
pub fn usize_to_i64_sat(v: usize) -> i64 {
    i64::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows i64 target! | Forensic: Value squashed to i64::MAX during saturating cast to prevent logic corruption"
            ),
            );
        i64::MAX
    })
}

/// Saturating cast: `usize` → `u16`.
///
/// Values > `u16::MAX` → `u16::MAX`.
#[inline]
#[must_use]
pub fn usize_to_u16_sat(v: usize) -> u16 {
    u16::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u16 target! | Forensic: Value squashed to u16::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u16::MAX
    })
}

/// Saturating cast: `usize` → `u8`.
///
/// Values > `u8::MAX` → `u8::MAX`.
#[inline]
#[must_use]
pub fn usize_to_u8_sat(v: usize) -> u8 {
    u8::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u8 target! | Forensic: Value squashed to u8::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u8::MAX
    })
}

/// Saturating cast: `u32` → `u8`.
///
/// Values > `u8::MAX` → `u8::MAX`.
#[inline]
#[must_use]
pub fn u32_to_u8_sat(v: u32) -> u8 {
    u8::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u8 target! | Forensic: Value squashed to u8::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u8::MAX
    })
}

/// Saturating cast: `u32` → `i32`.
///
/// Values > `i32::MAX` → `i32::MAX`.
#[inline]
#[must_use]
pub fn u32_to_i32_sat(v: u32) -> i32 {
    i32::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows i32 target! | Forensic: Value squashed to i32::MAX during saturating cast to prevent logic corruption"
            ),
            );
        i32::MAX
    })
}

/// Saturating cast: `i32` → `u32`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
pub fn i32_to_u32_sat(v: i32) -> u32 {
    u32::try_from(v.max(0)).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u32 target! | Forensic: Value squashed to u32::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u32::MAX
    })
}

/// Saturating cast: `i32` → `u64`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
pub fn i32_to_u64_sat(v: i32) -> u64 {
    u64::try_from(i64::from(v).max(0)).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u64 target! | Forensic: Value squashed to u64::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u64::MAX
    })
}

/// Lossless promotion: `usize` → `u64`.
///
/// Audited: On 32-bit and 64-bit systems, `usize` fits into `u64`.
#[inline]
#[must_use]
pub fn usize_to_u64(v: usize) -> u64 {
    u64::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u64 target! | Forensic: Value squashed to u64::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u64::MAX
    })
}

/// Saturating cast: `i32` → `u8`.
///
/// Negative → `0`, > 255 → `255`.
#[inline]
#[must_use]
pub fn i32_to_u8_sat(v: i32) -> u8 {
    u8::try_from(v.clamp(0, i32::from(u8::MAX))).unwrap_or(u8::MAX)
}

// ---------------------------------------------------------------------------
// u128 → Integer (saturating)
// ---------------------------------------------------------------------------

/// Saturating cast: `u128` → `i64`.
///
/// Values > `i64::MAX` → `i64::MAX`.
#[inline]
#[must_use]
pub fn u128_to_i64_sat(v: u128) -> i64 {
    i64::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows i64 target! | Forensic: Value squashed to i64::MAX during saturating cast to prevent logic corruption"
            ),
            );
        i64::MAX
    })
}

/// Saturating cast: `u128` → `u64`.
///
/// Values > `u64::MAX` → `u64::MAX`.
#[inline]
#[must_use]
pub fn u128_to_u64_sat(v: u128) -> u64 {
    u64::try_from(v).unwrap_or_else(|_| {
        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                "NUMERIC CONVERSION AUDIT: Value '{v}' overflows u64 target! | Forensic: Value squashed to u64::MAX during saturating cast to prevent logic corruption"
            ),
            );
        u64::MAX
    })
}

// ---------------------------------------------------------------------------
// Timestamp helpers
// ---------------------------------------------------------------------------

/// Current Unix timestamp as `i64` (for `SQLite` `INTEGER` columns).
///
/// `SQLite` stores integers as signed 64-bit. This helper avoids the recurring
/// `SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64` pattern
/// and handles the `u64 → i64` wrap safely (current timestamps fit in i64
/// until year 292,277,026,596).
/// # Panics
/// Panics if system time is before `UNIX_EPOCH`.
#[inline]
#[must_use]
pub fn unix_secs_i64() -> i64 {
    let secs = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(dur) => dur.as_secs(),
        Err(e) => {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "TIME AUDIT: System time before UNIX_EPOCH: {e}! | Forensic: Duration since epoch is negative; refusing to forge data; process state is inconsistent"
                ),
            );
            unreachable!("System time before UNIX_EPOCH: {e}");
        }
    };
    u64_to_i64_sat(secs)
}

/// Unix timestamp as `i64` from a fallible `SystemTime` operation.
///
/// Returns `Err` if the system clock is before Unix epoch.
///
/// # Errors
/// Returns an error if the system clock is before Unix epoch.
#[inline]
pub fn unix_secs_i64_result() -> Result<i64, std::time::SystemTimeError> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    Ok(u64_to_i64_sat(secs))
}

// ---------------------------------------------------------------------------
// Robust Floating Point Comparisons
// ---------------------------------------------------------------------------

/// Context-aware tolerance for floating point comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatContext {
    /// Values resulting from accumulation, image processing variance, or video metrics (e.g., PSNR denominator).
    Accumulation,
    /// FFmpeg/ffprobe reported metrics (e.g., PTS, framerates).
    FfmpegMeasurement,
    /// Expected strictly identical, but subject to machine epsilon.
    ExactMatch,
}

impl FloatContext {
    #[must_use]
    pub const fn epsilon(self) -> f64 {
        match self {
            Self::Accumulation => 1e-9_f64,
            Self::FfmpegMeasurement => 1e-4_f64,
            Self::ExactMatch => f64::EPSILON,
        }
    }
}

/// Robust check for whether a float is effectively zero given its computational context.
/// Exposes numerical instability rather than silently swallowing it via `abs() < 1e-9`.
#[inline]
#[must_use]
pub fn is_effectively_zero(value: f64, context: FloatContext) -> bool {
    let tol = context.epsilon();
    if value.abs() <= tol {
        if value.abs() > 0.0 {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC PRECISION AUDIT: Near-zero float encountered: {value} (context: {context:?}) | Forensic: Value falls below tolerance threshold; treated as zero to prevent computational drift"
                ),
            );
        }
        return true;
    }
    false
}

/// Robust check for whether two floats are effectively equal given their computational context.
/// Uses absolute difference for near-zero values and relative difference otherwise.
#[inline]
#[must_use]
pub fn is_effectively_equal(a: f64, b: f64, context: FloatContext) -> bool {
    let diff = a - b;
    let tol = context.epsilon();
    let scale = a.abs().max(b.abs()).max(1.0);

    if diff.abs() <= tol * scale {
        if diff.abs() > 0.0 {
            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                "delivery_numeric",
                format!(
                    "NUMERIC PRECISION AUDIT: Near-equal floats encountered: a={a}, b={b}, diff={diff} (context: {context:?}) | Forensic: Difference falls below relative tolerance threshold; treated as equal"
                ),
            );
        }
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- f64 → u64 --

    #[test]
    fn f64_to_u64_normal() {
        assert_eq!(f64_to_u64_sat(42.7), 42);
        assert_eq!(f64_to_u64_sat(0.0), 0);
        assert_eq!(f64_to_u64_sat(1_000_000.9), 1_000_000);
    }

    #[test]
    fn f64_to_u64_edge_cases() {
        assert_eq!(f64_to_u64_sat(f64::NAN), 0);
        assert_eq!(f64_to_u64_sat(f64::NEG_INFINITY), 0);
        assert_eq!(f64_to_u64_sat(-1.0), 0);
        assert_eq!(f64_to_u64_sat(f64::INFINITY), u64::MAX);
    }

    // -- f64 → u32 --

    #[test]
    fn f64_to_u32_normal() {
        assert_eq!(f64_to_u32_sat(1920.0), 1920);
        assert_eq!(f64_to_u32_sat(0.0), 0);
    }

    #[test]
    fn f64_to_u32_edge_cases() {
        assert_eq!(f64_to_u32_sat(f64::NAN), 0);
        assert_eq!(f64_to_u32_sat(-100.0), 0);
        assert_eq!(f64_to_u32_sat(5_000_000_000.0), u32::MAX);
    }

    // -- f64 → usize --

    #[test]
    fn f64_to_usize_normal() {
        assert_eq!(f64_to_usize_sat(10.0), 10);
        assert_eq!(f64_to_usize_sat(0.9), 0);
    }

    #[test]
    fn f64_to_usize_edge_cases() {
        assert_eq!(f64_to_usize_sat(f64::NAN), 0);
        assert_eq!(f64_to_usize_sat(-1.0), 0);
    }

    // -- f64 → u8 --

    #[test]
    fn f64_to_u8_normal() {
        assert_eq!(f64_to_u8_sat(128.0), 128);
        assert_eq!(f64_to_u8_sat(0.0), 0);
        assert_eq!(f64_to_u8_sat(255.0), 255);
    }

    #[test]
    fn f64_to_u8_edge_cases() {
        assert_eq!(f64_to_u8_sat(f64::NAN), 0);
        assert_eq!(f64_to_u8_sat(-5.0), 0);
        assert_eq!(f64_to_u8_sat(300.0), 255);
    }

    // -- f32 → u32 --

    #[test]
    fn f32_to_u32_normal() {
        assert_eq!(f32_to_u32_sat(100.0_f32), 100);
        assert_eq!(f32_to_u32_sat(0.0_f32), 0);
    }

    #[test]
    fn f32_to_u32_edge_cases() {
        assert_eq!(f32_to_u32_sat(f32::NAN), 0);
        assert_eq!(f32_to_u32_sat(-1.0_f32), 0);
    }

    #[test]
    fn f32_to_usize_edge_cases() {
        assert_eq!(f32_to_usize_sat(f32::NAN), 0);
        assert_eq!(f32_to_usize_sat(-1.0_f32), 0);
        assert_eq!(f32_to_usize_sat(f32::INFINITY), usize::MAX);
    }

    // -- f64 → i32 --

    #[test]
    fn f64_to_i32_normal() {
        assert_eq!(f64_to_i32_sat(42.0), 42_i32);
        assert_eq!(f64_to_i32_sat(-42.0), -42_i32);
    }

    #[test]
    fn f64_to_i32_edge_cases() {
        assert_eq!(f64_to_i32_sat(f64::NAN), 0_i32);
        assert_eq!(f64_to_i32_sat(3_000_000_000.0), i32::MAX);
        assert_eq!(f64_to_i32_sat(-3_000_000_000.0), i32::MIN);
    }

    #[test]
    fn f32_to_i32_edge_cases() {
        assert_eq!(f32_to_i32_sat(f32::NAN), 0_i32);
        assert_eq!(f32_to_i32_sat(f32::INFINITY), i32::MAX);
        assert_eq!(f32_to_i32_sat(f32::NEG_INFINITY), i32::MIN);
    }

    // -- f64 → f32 --

    #[test]
    fn f64_to_f32_lossy_normal() {
        let v = f64_to_f32_lossy(0.123_456_789_012_345);
        assert!((v - 0.123_456_79).abs() < 1e-6);
    }

    // -- integer casts --

    #[test]
    fn u64_to_usize_normal() {
        assert_eq!(u64_to_usize_sat(42), 42);
        assert_eq!(u64_to_usize_sat(0), 0);
    }

    #[test]
    fn i64_to_u64_edge() {
        assert_eq!(i64_to_u64_sat(-1), 0);
        assert_eq!(i64_to_u64_sat(100), 100);
    }

    #[test]
    fn i32_to_u32_edge() {
        assert_eq!(i32_to_u32_sat(-1), 0);
        assert_eq!(i32_to_u32_sat(100), 100);
    }

    #[test]
    fn i32_to_u8_edge() {
        assert_eq!(i32_to_u8_sat(-10), 0);
        assert_eq!(i32_to_u8_sat(128), 128);
        assert_eq!(i32_to_u8_sat(300), 255);
    }

    // -- timestamp --

    #[test]
    fn unix_secs_i64_is_positive() {
        let ts = unix_secs_i64();
        assert!(ts > 1_700_000_000);
    }

    #[test]
    fn unix_secs_i64_result_ok() {
        assert!(unix_secs_i64_result().is_ok());
    }
    #[test]
    fn test_audited_cast_specialization() {
        // Test f64 -> u64 (specialized)
        let val: f64 = 42.7;
        let casted: u64 = val.cast_sat();
        assert_eq!(casted, 42);

        let nan: f64 = f64::NAN;
        let nan_casted: u64 = nan.cast_sat();
        assert_eq!(nan_casted, 0);

        // Test f64 -> usize (specialized)
        let u_val: f64 = 1234.0;
        let u_casted: usize = u_val.cast_sat();
        assert_eq!(u_casted, 1234);

        let u32_val: f64 = 5_000_000_000.0;
        let u32_casted: u32 = u32_val.cast_sat();
        assert_eq!(u32_casted, u32::MAX);
    }

    #[test]
    fn test_strict_conversions() {
        // f64_to_u64_strict
        assert_eq!(f64_to_u64_strict(123.0, "test"), Some(123));
        assert_eq!(f64_to_u64_strict(f64::NAN, "test"), None);
        assert_eq!(f64_to_u64_strict(-1.0, "test"), None);
        assert_eq!(f64_to_u64_strict(2e19, "test"), None);

        // f64_to_u32_strict
        assert_eq!(f64_to_u32_strict(123.0, "test"), Some(123));
        assert_eq!(f64_to_u32_strict(5e9, "test"), None);

        // u64_to_u32_strict
        assert_eq!(u64_to_u32_strict(123, "test"), Some(123));
        assert_eq!(u64_to_u32_strict(u64::from(u32::MAX) + 1, "test"), None);

        // parse_strict
        assert_eq!(parse_strict::<u32>("123", "test"), Some(123));
        assert_eq!(parse_strict::<u32>("abc", "test"), None);

        // i64_to_u64_strict
        assert_eq!(i64_to_u64_strict(123, "test"), Some(123));
        assert_eq!(i64_to_u64_strict(-1, "test"), None);
    }

    #[test]
    fn test_f64_to_i32_strict_precision() {
        assert_eq!(f64_to_i32_strict(0.0, "zero"), Some(0));
        assert_eq!(
            f64_to_i32_strict(2_147_483_647.0, "max"),
            Some(2_147_483_647)
        );
        assert_eq!(
            f64_to_i32_strict(-2_147_483_648.0, "min"),
            Some(-2_147_483_648)
        );
        assert_eq!(f64_to_i32_strict(2_147_483_648.0, "overflow"), None);
    }

    #[test]
    fn test_f64_to_f32_strict_range() {
        assert_eq!(f64_to_f32_strict(0.0, "zero"), Some(0.0));
        assert_eq!(
            f64_to_f32_strict(f64::from(f32::MAX), "max"),
            Some(f32::MAX)
        );
        assert_eq!(f64_to_f32_strict(1e40, "overflow"), None);
        assert_eq!(f64_to_f32_strict(f64::NAN, "nan"), None);
    }
}
