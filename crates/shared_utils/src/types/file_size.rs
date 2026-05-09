//! `FileSize` Type-Safe Wrapper
//!
//! Provides type-safe file size operations to prevent overflow and negative values.

use std::fmt;

pub use crate::constants::{
    METADATA_MARGIN_MAX_BYTES as METADATA_MARGIN_MAX,
    METADATA_MARGIN_MIN_BYTES as METADATA_MARGIN_MIN,
    METADATA_MARGIN_RATIO as METADATA_MARGIN_PERCENT,
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileSize(u64);

impl FileSize {
    pub const ZERO: Self = Self(0);

    pub const KB: u64 = crate::constants::KB;
    pub const MB: u64 = crate::constants::MB;
    pub const GB: u64 = crate::constants::GB;

    #[inline]
    #[must_use]
    pub const fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    #[inline]
    #[must_use]
    pub const fn from_kb(kb: u64) -> Self {
        Self(kb * Self::KB)
    }

    #[inline]
    #[must_use]
    pub const fn from_mb(mb: u64) -> Self {
        Self(mb * Self::MB)
    }

    #[inline]
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.0
    }

    #[inline]
    #[must_use]
    pub const fn saturating_sub(&self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    #[inline]
    #[must_use]
    pub const fn saturating_add(&self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    #[must_use]
    pub fn compression_ratio(&self, original: Self) -> Option<f64> {
        if original.0 == 0 {
            None
        } else {
            Some(
                crate::numeric_cast::u64_to_f64(self.0)
                    / crate::numeric_cast::u64_to_f64(original.0),
            )
        }
    }

    #[must_use]
    pub fn size_change_percent(&self, original: Self) -> Option<f64> {
        if original.0 == 0 {
            None
        } else {
            Some(
                (crate::numeric_cast::u64_to_f64(self.0)
                    - crate::numeric_cast::u64_to_f64(original.0))
                    / crate::numeric_cast::u64_to_f64(original.0)
                    * crate::constants::SCALE_100,
            )
        }
    }

    #[must_use]
    pub fn display(&self) -> String {
        if self.0 >= Self::GB {
            format!(
                "{:.2} GB",
                crate::numeric_cast::u64_to_f64(self.0) / crate::numeric_cast::u64_to_f64(Self::GB)
            )
        } else if self.0 >= Self::MB {
            format!(
                "{:.2} MB",
                crate::numeric_cast::u64_to_f64(self.0) / crate::numeric_cast::u64_to_f64(Self::MB)
            )
        } else if self.0 >= Self::KB {
            format!(
                "{:.2} KB",
                crate::numeric_cast::u64_to_f64(self.0) / crate::numeric_cast::u64_to_f64(Self::KB)
            )
        } else {
            format!("{} B", self.0)
        }
    }

    #[must_use]
    pub fn metadata_margin(&self) -> Self {
        let percent_based = crate::numeric_cast::f64_to_u64_sat(
            crate::numeric_cast::u64_to_f64(self.0) * METADATA_MARGIN_PERCENT,
        );
        let margin = percent_based.clamp(METADATA_MARGIN_MIN, METADATA_MARGIN_MAX);
        Self(margin)
    }

    #[must_use]
    pub fn compression_target(&self) -> Self {
        self.saturating_sub(self.metadata_margin())
    }

    #[must_use]
    pub const fn can_compress_to(&self, target: Self) -> bool {
        self.0 > target.0
    }

    #[inline]
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for FileSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileSize({} = {})", self.0, self.display())
    }
}

impl fmt::Display for FileSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}

impl Default for FileSize {
    fn default() -> Self {
        Self::ZERO
    }
}

impl From<u64> for FileSize {
    fn from(bytes: u64) -> Self {
        Self::new(bytes)
    }
}

impl From<FileSize> for u64 {
    fn from(size: FileSize) -> Self {
        size.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_size_creation() {
        let size = FileSize::new(1024);
        assert_eq!(size.bytes(), 1024);

        let kb = FileSize::from_kb(1);
        assert_eq!(kb.bytes(), 1024);

        let mb = FileSize::from_mb(1);
        assert_eq!(mb.bytes(), 1_048_576);
    }

    #[test]
    fn test_saturating_sub() {
        let a = FileSize::new(100);
        let b = FileSize::new(30);

        assert_eq!(a.saturating_sub(b).bytes(), 70);

        assert_eq!(b.saturating_sub(a).bytes(), 0);

        assert_eq!(a.saturating_sub(a).bytes(), 0);
    }

    #[test]
    fn test_compression_ratio() {
        let output = FileSize::new(500);
        let input = FileSize::new(1000);

        let ratio = output.compression_ratio(input);
        assert_eq!(ratio, Some(0.5_f64));

        let zero = FileSize::ZERO;
        assert_eq!(output.compression_ratio(zero), None);
    }

    #[test]
    fn test_compression_ratio_zero_original() {
        let output = FileSize::new(100);
        let zero = FileSize::ZERO;
        assert!(output.compression_ratio(zero).is_none());
    }

    #[test]
    fn test_display() {
        assert_eq!(FileSize::new(500).display(), "500 B");
        assert_eq!(FileSize::new(1024).display(), "1.00 KB");
        assert_eq!(FileSize::new(1_048_576).display(), "1.00 MB");
        assert_eq!(FileSize::new(1_048_576 * 1024).display(), "1.00 GB");
    }

    #[test]
    fn test_metadata_margin() {
        let small = FileSize::new(100 * 1024);
        assert_eq!(small.metadata_margin().bytes(), METADATA_MARGIN_MIN);

        let medium = FileSize::new(10 * 1_048_576);
        let expected = crate::numeric_cast::u64_to_f64(10 * 1_048_576) * METADATA_MARGIN_PERCENT;
        assert_eq!(
            medium.metadata_margin().bytes(),
            crate::numeric_cast::f64_to_u64_sat(expected)
        );

        let large = FileSize::new(100 * 1_048_576 * 1024);
        assert_eq!(large.metadata_margin().bytes(), METADATA_MARGIN_MAX);
    }

    #[test]
    fn test_size_change_percent() {
        let output = FileSize::new(800);
        let input = FileSize::new(1000);

        let change = output.size_change_percent(input);
        assert_eq!(change, Some(-20.0_f64));

        let larger = FileSize::new(1200);
        let change = larger.size_change_percent(input);
        assert_eq!(change, Some(20.0_f64));
    }
}
