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
//! ```ignore
//! use crate::numeric_cast::{f64_to_u64_sat, unix_secs_i64};
//!
//! let size = f64_to_u64_sat(estimated_bytes);
//! let timestamp = unix_secs_i64();
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

mod raw {
    #![allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]

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
    pub(super) const fn usize_to_f64(v: usize) -> f64 {
        v as f64
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
}

// ---------------------------------------------------------------------------
// f64 → unsigned integer (saturating)
// ---------------------------------------------------------------------------

/// Saturating cast: `f64` → `u64`.
///
/// - `NaN` or negative → `0`
/// - `> u64::MAX` → `u64::MAX`
#[inline]
#[must_use]
pub fn f64_to_u64_sat(v: f64) -> u64 {
    if v.is_nan() || v < 0.0 {
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
    if v.is_nan() || v < 0.0 {
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
    if v.is_nan() || v < 0.0 {
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
    if v.is_nan() || v < 0.0 {
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
    if v.is_nan() || v < 0.0 {
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
        return 0;
    }
    raw::f32_to_u16(v)
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

/// Saturating cast: `f32` → `usize`.
///
/// - `NaN` or negative → `0`
/// - overflow → `usize::MAX`
#[inline]
#[must_use]
pub fn f32_to_usize_sat(v: f32) -> usize {
    if v.is_nan() || v < 0.0 {
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
    usize::try_from(v).unwrap_or(usize::MAX)
}

/// Saturating cast: `u32` → `usize`.
///
/// Lossless on 32-bit and 64-bit targets.
#[inline]
#[must_use]
pub fn u32_to_usize_sat(v: u32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
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
pub fn i32_to_usize_sat(v: i32) -> usize {
    usize::try_from(v).unwrap_or(0)
}

/// Saturating cast: `i64` → `usize`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
pub fn i64_to_usize_sat(v: i64) -> usize {
    usize::try_from(v).unwrap_or(0)
}

/// Saturating cast: `usize` → `i32`.
///
/// Values > `i32::MAX` → `i32::MAX`.
#[inline]
#[must_use]
pub fn usize_to_i32_sat(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Saturating cast: `i64` → `u64`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
pub fn i64_to_u64_sat(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// Saturating cast: `u64` → `i64`.
///
/// Values > `i64::MAX` → `i64::MAX`.
#[inline]
#[must_use]
pub fn u64_to_i64_sat(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
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
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// Saturating cast: `usize` → `u32`.
///
/// Values > `u32::MAX` → `u32::MAX`.
#[inline]
#[must_use]
pub fn usize_to_u32_sat(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// Saturating cast: `usize` → `i64`.
///
/// Values > `i64::MAX` → `i64::MAX`.
#[inline]
#[must_use]
pub fn usize_to_i64_sat(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Saturating cast: `usize` → `u16`.
///
/// Values > `u16::MAX` → `u16::MAX`.
#[inline]
#[must_use]
pub fn usize_to_u16_sat(v: usize) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

/// Saturating cast: `usize` → `u8`.
///
/// Values > `u8::MAX` → `u8::MAX`.
#[inline]
#[must_use]
pub fn usize_to_u8_sat(v: usize) -> u8 {
    u8::try_from(v).unwrap_or(u8::MAX)
}

/// Saturating cast: `u32` → `u8`.
///
/// Values > `u8::MAX` → `u8::MAX`.
#[inline]
#[must_use]
pub fn u32_to_u8_sat(v: u32) -> u8 {
    u8::try_from(v).unwrap_or(u8::MAX)
}

/// Saturating cast: `u32` → `i32`.
///
/// Values > `i32::MAX` → `i32::MAX`.
#[inline]
#[must_use]
pub fn u32_to_i32_sat(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Saturating cast: `i32` → `u32`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
pub fn i32_to_u32_sat(v: i32) -> u32 {
    u32::try_from(v).unwrap_or(0)
}

/// Saturating cast: `i32` → `u64`.
///
/// Negative values → `0`.
#[inline]
#[must_use]
pub fn i32_to_u64_sat(v: i32) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// Lossless promotion: `usize` → `u64`.
///
/// Audited: On 32-bit and 64-bit systems, `usize` fits into `u64`.
#[inline]
#[must_use]
pub fn usize_to_u64(v: usize) -> u64 {
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// Saturating cast: `i32` → `u8`.
///
/// Negative → `0`, > 255 → `255`.
#[inline]
#[must_use]
pub fn i32_to_u8_sat(v: i32) -> u8 {
    u8::try_from(v.clamp(0, i32::from(u8::MAX))).unwrap_or(0)
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
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Saturating cast: `u128` → `u64`.
///
/// Values > `u64::MAX` → `u64::MAX`.
#[inline]
#[must_use]
pub fn u128_to_u64_sat(v: u128) -> u64 {
    u64::try_from(v).unwrap_or(u64::MAX)
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
#[inline]
#[must_use]
pub fn unix_secs_i64() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
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
        assert_eq!(f64_to_i32_sat(42.0), 42);
        assert_eq!(f64_to_i32_sat(-42.0), -42);
    }

    #[test]
    fn f64_to_i32_edge_cases() {
        assert_eq!(f64_to_i32_sat(f64::NAN), 0);
        assert_eq!(f64_to_i32_sat(3_000_000_000.0), i32::MAX);
        assert_eq!(f64_to_i32_sat(-3_000_000_000.0), i32::MIN);
    }

    #[test]
    fn f32_to_i32_edge_cases() {
        assert_eq!(f32_to_i32_sat(f32::NAN), 0);
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
        assert!(ts > 1_700_000_000, "Timestamp should be after 2023");
    }

    #[test]
    fn unix_secs_i64_result_ok() {
        assert!(unix_secs_i64_result().is_ok());
    }
}
