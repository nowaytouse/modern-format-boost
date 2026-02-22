//! FileSize Type-Safe Wrapper
//!
//! 提供类型安全的文件大小操作，防止溢出和负数。

use std::fmt;

pub const METADATA_MARGIN_PERCENT: f64 = 0.005;

pub const METADATA_MARGIN_MIN: u64 = 2048;

pub const METADATA_MARGIN_MAX: u64 = 102400;


#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileSize(u64);

impl FileSize {
    pub const ZERO: FileSize = FileSize(0);

    pub const KB: u64 = 1024;
    pub const MB: u64 = 1024 * 1024;
    pub const GB: u64 = 1024 * 1024 * 1024;

    #[inline]
    pub const fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    #[inline]
    pub const fn from_kb(kb: u64) -> Self {
        Self(kb * Self::KB)
    }

    #[inline]
    pub const fn from_mb(mb: u64) -> Self {
        Self(mb * Self::MB)
    }

    #[inline]
    pub const fn bytes(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn saturating_sub(&self, other: FileSize) -> FileSize {
        FileSize(self.0.saturating_sub(other.0))
    }

    #[inline]
    pub fn saturating_add(&self, other: FileSize) -> FileSize {
        FileSize(self.0.saturating_add(other.0))
    }

    pub fn compression_ratio(&self, original: FileSize) -> Option<f64> {
        if original.0 == 0 {
            None
        } else {
            Some(self.0 as f64 / original.0 as f64)
        }
    }

    pub fn size_change_percent(&self, original: FileSize) -> Option<f64> {
        if original.0 == 0 {
            None
        } else {
            Some((self.0 as f64 - original.0 as f64) / original.0 as f64 * 100.0)
        }
    }

    pub fn display(&self) -> String {
        if self.0 >= Self::GB {
            format!("{:.2} GB", self.0 as f64 / Self::GB as f64)
        } else if self.0 >= Self::MB {
            format!("{:.2} MB", self.0 as f64 / Self::MB as f64)
        } else if self.0 >= Self::KB {
            format!("{:.2} KB", self.0 as f64 / Self::KB as f64)
        } else {
            format!("{} B", self.0)
        }
    }

    pub fn metadata_margin(&self) -> FileSize {
        let percent_based = (self.0 as f64 * METADATA_MARGIN_PERCENT) as u64;
        let margin = percent_based.clamp(METADATA_MARGIN_MIN, METADATA_MARGIN_MAX);
        FileSize(margin)
    }

    pub fn compression_target(&self) -> FileSize {
        self.saturating_sub(self.metadata_margin())
    }

    pub fn can_compress_to(&self, target: FileSize) -> bool {
        self.0 > target.0
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
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
        assert_eq!(mb.bytes(), 1024 * 1024);
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
        assert_eq!(ratio, Some(0.5));

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
        assert_eq!(FileSize::new(1024 * 1024).display(), "1.00 MB");
        assert_eq!(FileSize::new(1024 * 1024 * 1024).display(), "1.00 GB");
    }

    #[test]
    fn test_metadata_margin() {
        let small = FileSize::new(100 * 1024);
        assert_eq!(small.metadata_margin().bytes(), METADATA_MARGIN_MIN);

        let medium = FileSize::new(10 * 1024 * 1024);
        let expected = (10 * 1024 * 1024) as f64 * METADATA_MARGIN_PERCENT;
        assert_eq!(medium.metadata_margin().bytes(), expected as u64);

        let large = FileSize::new(100 * 1024 * 1024 * 1024);
        assert_eq!(large.metadata_margin().bytes(), METADATA_MARGIN_MAX);
    }

    #[test]
    fn test_size_change_percent() {
        let output = FileSize::new(800);
        let input = FileSize::new(1000);

        let change = output.size_change_percent(input);
        assert_eq!(change, Some(-20.0));

        let larger = FileSize::new(1200);
        let change = larger.size_change_percent(input);
        assert_eq!(change, Some(20.0));
    }
}
