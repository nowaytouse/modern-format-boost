//! FileSize Type-Safe Wrapper
//!
//! 提供类型安全的文件大小操作，防止溢出和负数。

use std::fmt;

/// 元数据余量百分比
pub const METADATA_MARGIN_PERCENT: f64 = 0.005; // 0.5%

/// 元数据余量最小值（字节）
pub const METADATA_MARGIN_MIN: u64 = 2048; // 2KB

/// 元数据余量最大值（字节）
pub const METADATA_MARGIN_MAX: u64 = 102400; // 100KB

// ============================================================================
// FileSize Newtype
// ============================================================================

/// 类型安全的文件大小（字节）
///
/// 提供安全的算术操作，防止溢出和负数。
///
/// # Examples
/// ```
/// use shared_utils::types::file_size::FileSize;
///
/// let size = FileSize::new(1024 * 1024); // 1MB
/// assert_eq!(size.bytes(), 1048576);
/// assert_eq!(size.display(), "1.00 MB");
///
/// // 安全减法
/// let smaller = FileSize::new(500);
/// let result = size.saturating_sub(smaller);
/// assert_eq!(result.bytes(), 1048576 - 500);
///
/// // 不会下溢
/// let result = smaller.saturating_sub(size);
/// assert_eq!(result.bytes(), 0);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileSize(u64);

impl FileSize {
    /// 零大小
    pub const ZERO: FileSize = FileSize(0);

    /// 1 KB
    pub const KB: u64 = 1024;
    /// 1 MB
    pub const MB: u64 = 1024 * 1024;
    /// 1 GB
    pub const GB: u64 = 1024 * 1024 * 1024;

    /// 创建文件大小
    #[inline]
    pub const fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    /// 从 KB 创建
    #[inline]
    pub const fn from_kb(kb: u64) -> Self {
        Self(kb * Self::KB)
    }

    /// 从 MB 创建
    #[inline]
    pub const fn from_mb(mb: u64) -> Self {
        Self(mb * Self::MB)
    }

    /// 获取原始字节数
    #[inline]
    pub const fn bytes(&self) -> u64 {
        self.0
    }

    /// 安全减法（不会下溢）
    ///
    /// 如果 other > self，返回 FileSize(0)
    #[inline]
    pub fn saturating_sub(&self, other: FileSize) -> FileSize {
        FileSize(self.0.saturating_sub(other.0))
    }

    /// 安全加法（不会溢出）
    #[inline]
    pub fn saturating_add(&self, other: FileSize) -> FileSize {
        FileSize(self.0.saturating_add(other.0))
    }

    /// 计算压缩比（处理零除）
    ///
    /// 返回 self / original，如果 original 为零则返回 None。
    ///
    /// # Returns
    /// * `Some(ratio)` - 压缩比，0.0 表示完全压缩，1.0 表示无压缩
    /// * `None` - 如果 original 为零
    pub fn compression_ratio(&self, original: FileSize) -> Option<f64> {
        if original.0 == 0 {
            None
        } else {
            Some(self.0 as f64 / original.0 as f64)
        }
    }

    /// 计算大小变化百分比
    ///
    /// 返回 (self - original) / original * 100
    /// 负数表示减小，正数表示增大。
    pub fn size_change_percent(&self, original: FileSize) -> Option<f64> {
        if original.0 == 0 {
            None
        } else {
            Some((self.0 as f64 - original.0 as f64) / original.0 as f64 * 100.0)
        }
    }

    /// 格式化显示（自动选择单位）
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

    /// 计算元数据余量
    ///
    /// 公式: max(input × 0.5%, 2KB).min(100KB)
    pub fn metadata_margin(&self) -> FileSize {
        let percent_based = (self.0 as f64 * METADATA_MARGIN_PERCENT) as u64;
        let margin = percent_based
            .max(METADATA_MARGIN_MIN)
            .min(METADATA_MARGIN_MAX);
        FileSize(margin)
    }

    /// 计算压缩目标大小（预留元数据余量）
    pub fn compression_target(&self) -> FileSize {
        self.saturating_sub(self.metadata_margin())
    }

    /// 检查是否可以压缩到目标大小
    pub fn can_compress_to(&self, target: FileSize) -> bool {
        self.0 > target.0
    }

    /// 检查是否为零
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

// ============================================================================
// Trait Implementations
// ============================================================================

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

// ============================================================================
// Tests
// ============================================================================

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

        // 正常减法
        assert_eq!(a.saturating_sub(b).bytes(), 70);

        // 不会下溢
        assert_eq!(b.saturating_sub(a).bytes(), 0);

        // 边界情况
        assert_eq!(a.saturating_sub(a).bytes(), 0);
    }

    #[test]
    fn test_compression_ratio() {
        let output = FileSize::new(500);
        let input = FileSize::new(1000);

        let ratio = output.compression_ratio(input);
        assert_eq!(ratio, Some(0.5));

        // 零除
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
        // 小文件：使用最小值
        let small = FileSize::new(100 * 1024); // 100KB
        assert_eq!(small.metadata_margin().bytes(), METADATA_MARGIN_MIN);

        // 中等文件：按比例
        let medium = FileSize::new(10 * 1024 * 1024); // 10MB
        let expected = (10 * 1024 * 1024) as f64 * METADATA_MARGIN_PERCENT;
        assert_eq!(medium.metadata_margin().bytes(), expected as u64);

        // 大文件：使用最大值
        let large = FileSize::new(100 * 1024 * 1024 * 1024); // 100GB
        assert_eq!(large.metadata_margin().bytes(), METADATA_MARGIN_MAX);
    }

    #[test]
    fn test_size_change_percent() {
        let output = FileSize::new(800);
        let input = FileSize::new(1000);

        let change = output.size_change_percent(input);
        assert_eq!(change, Some(-20.0)); // 减小 20%

        let larger = FileSize::new(1200);
        let change = larger.size_change_percent(input);
        assert_eq!(change, Some(20.0)); // 增大 20%
    }
}
