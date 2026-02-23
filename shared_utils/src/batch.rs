//! Batch Processing Module
//!
//! Provides utilities for batch file processing with proper error handling
//! Reference: media/CONTRIBUTING.md - Batch Processing Capability requirement
//!
//! 🔥 v7.5: 添加文件排序功能，优先处理小文件

use crate::file_sorter::{sort_by_size_ascending, SortStrategy};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn collect_files(dir: &Path, extensions: &[&str], recursive: bool) -> Vec<PathBuf> {
    let walker = if recursive {
        WalkDir::new(dir).follow_links(true)
    } else {
        WalkDir::new(dir).max_depth(1)
    };

    walker
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| crate::common_utils::has_extension(e.path(), extensions))
        .map(|e| e.path().to_path_buf())
        .collect()
}

pub fn collect_files_sorted(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
    sort_strategy: SortStrategy,
) -> Vec<PathBuf> {
    let files = collect_files(dir, extensions, recursive);

    match sort_strategy {
        SortStrategy::None => files,
        SortStrategy::SizeAscending => sort_by_size_ascending(files),
        _ => crate::file_sorter::FileSorter::new(sort_strategy).sort(files),
    }
}

pub fn collect_files_small_first(dir: &Path, extensions: &[&str], recursive: bool) -> Vec<PathBuf> {
    collect_files_sorted(dir, extensions, recursive, SortStrategy::SizeAscending)
}

pub fn calculate_directory_size_by_extensions(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> u64 {
    let walker = if recursive {
        WalkDir::new(dir).follow_links(true)
    } else {
        WalkDir::new(dir).max_depth(1)
    };

    walker
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| crate::common_utils::has_extension(e.path(), extensions))
        .filter_map(|e| std::fs::metadata(e.path()).ok())
        .map(|m| m.len())
        .sum()
}

pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif", "avif",
    "jxl", "bmp",
];

pub const ANIMATED_EXTENSIONS: &[&str] = &["gif", "webp", "png"];

#[derive(Debug, Clone)]
pub struct BatchResult {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<(PathBuf, String)>,
}

impl BatchResult {
    pub fn new() -> Self {
        Self {
            total: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            errors: Vec::new(),
        }
    }

    pub fn success(&mut self) {
        self.total += 1;
        self.succeeded += 1;
    }

    pub fn fail(&mut self, path: PathBuf, error: String) {
        self.total += 1;
        self.failed += 1;
        self.errors.push((path, error));
    }

    pub fn skip(&mut self) {
        self.total += 1;
        self.skipped += 1;
    }

    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            (self.succeeded as f64 / self.total as f64) * 100.0
        }
    }
}

impl Default for BatchResult {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_result_new() {
        let result = BatchResult::new();
        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_batch_result_success() {
        let mut result = BatchResult::new();
        result.success();

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn test_batch_result_fail() {
        let mut result = BatchResult::new();
        result.fail(PathBuf::from("test.png"), "Error message".to_string());

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].1, "Error message");
    }

    #[test]
    fn test_batch_result_skip() {
        let mut result = BatchResult::new();
        result.skip();

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_batch_result_mixed() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());
        result.skip();

        assert_eq!(result.total, 4);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_success_rate_empty() {
        let result = BatchResult::new();
        assert!(
            (result.success_rate() - 100.0).abs() < 0.01,
            "Empty batch should have 100% success rate"
        );
    }

    #[test]
    fn test_success_rate_all_success() {
        let mut result = BatchResult::new();
        for _ in 0..10 {
            result.success();
        }
        assert!(
            (result.success_rate() - 100.0).abs() < 0.01,
            "All success should be 100%"
        );
    }

    #[test]
    fn test_success_rate_all_fail() {
        let mut result = BatchResult::new();
        for i in 0..10 {
            result.fail(PathBuf::from(format!("file{}.png", i)), "Error".to_string());
        }
        assert!(
            (result.success_rate() - 0.0).abs() < 0.01,
            "All fail should be 0%"
        );
    }

    #[test]
    fn test_success_rate_50_percent() {
        let mut result = BatchResult::new();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());

        assert!(
            (result.success_rate() - 50.0).abs() < 0.01,
            "1 success, 1 fail should be 50%, got {}",
            result.success_rate()
        );
    }

    #[test]
    fn test_success_rate_with_skipped() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.skip();
        result.skip();

        assert!(
            (result.success_rate() - 50.0).abs() < 0.01,
            "2 success, 2 skipped should be 50%, got {}",
            result.success_rate()
        );
    }

    #[test]
    fn test_strict_success_rate_formula() {
        let test_cases = [
            (10, 0, 0, 100.0),
            (5, 5, 0, 50.0),
            (3, 1, 0, 75.0),
            (1, 3, 0, 25.0),
            (0, 10, 0, 0.0),
            (7, 2, 1, 70.0),
        ];

        for (success, fail, skip, expected) in test_cases {
            let mut result = BatchResult::new();
            for _ in 0..success {
                result.success();
            }
            for i in 0..fail {
                result.fail(PathBuf::from(format!("f{}.png", i)), "E".to_string());
            }
            for _ in 0..skip {
                result.skip();
            }

            let rate = result.success_rate();
            let expected_calc = if result.total == 0 {
                100.0
            } else {
                (result.succeeded as f64 / result.total as f64) * 100.0
            };

            assert!(
                (rate - expected).abs() < 0.001,
                "STRICT: {}s/{}f/{}k expected {}%, got {}%",
                success,
                fail,
                skip,
                expected,
                rate
            );
            assert!(
                (rate - expected_calc).abs() < 0.0001,
                "STRICT: Formula mismatch"
            );
        }
    }

    #[test]
    fn test_strict_large_numbers() {
        let mut result = BatchResult::new();

        for _ in 0..500_000 {
            result.success();
        }
        for i in 0..500_000 {
            result.fail(PathBuf::from(format!("f{}.png", i)), "E".to_string());
        }

        assert_eq!(result.total, 1_000_000);
        assert!(
            (result.success_rate() - 50.0).abs() < 0.001,
            "STRICT: Large batch should calculate correctly"
        );
    }

    #[test]
    fn test_consistency_success_rate() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());

        let rate1 = result.success_rate();
        let rate2 = result.success_rate();
        let rate3 = result.success_rate();

        assert!((rate1 - rate2).abs() < 0.0000001);
        assert!((rate2 - rate3).abs() < 0.0000001);
    }

    #[test]
    fn test_total_equals_sum() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.success();
        result.fail(PathBuf::from("f1.png"), "E".to_string());
        result.fail(PathBuf::from("f2.png"), "E".to_string());
        result.skip();

        assert_eq!(
            result.total,
            result.succeeded + result.failed + result.skipped,
            "STRICT: total must equal succeeded + failed + skipped"
        );
    }
}
