//! Batch Processing Module
//! 
//! Provides utilities for batch file processing with proper error handling
//! Reference: media/CONTRIBUTING.md - Batch Processing Capability requirement

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Collect files from a directory with extension filtering
/// 
/// # Arguments
/// * `dir` - Directory to scan
/// * `extensions` - List of allowed extensions (lowercase, without dot)
/// * `recursive` - Whether to scan subdirectories
/// 
/// # Returns
/// Vector of file paths matching the criteria
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
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| extensions.contains(&ext.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// Image file extensions commonly supported
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "tiff", "tif", 
    "heic", "heif", "avif", "jxl", "bmp"
];

/// Video file extensions commonly supported
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "mkv", "avi", "webm", "m4v", "wmv", "flv"
];

/// Animated image extensions
pub const ANIMATED_EXTENSIONS: &[&str] = &[
    "gif", "webp", "png"  // PNG can be APNG
];

/// Batch processing result
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

    /// Calculate success rate as percentage
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

// ============================================================
// 🔬 PRECISION VALIDATION TESTS ("裁判" Tests)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // BatchResult Basic Tests
    // ============================================================
    
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
    
    // ============================================================
    // 🔬 Success Rate Calculation Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_success_rate_empty() {
        let result = BatchResult::new();
        // Empty batch should return 100% (no failures)
        assert!((result.success_rate() - 100.0).abs() < 0.01,
            "Empty batch should have 100% success rate");
    }
    
    #[test]
    fn test_success_rate_all_success() {
        let mut result = BatchResult::new();
        for _ in 0..10 {
            result.success();
        }
        assert!((result.success_rate() - 100.0).abs() < 0.01,
            "All success should be 100%");
    }
    
    #[test]
    fn test_success_rate_all_fail() {
        let mut result = BatchResult::new();
        for i in 0..10 {
            result.fail(PathBuf::from(format!("file{}.png", i)), "Error".to_string());
        }
        assert!((result.success_rate() - 0.0).abs() < 0.01,
            "All fail should be 0%");
    }
    
    #[test]
    fn test_success_rate_50_percent() {
        let mut result = BatchResult::new();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());
        
        assert!((result.success_rate() - 50.0).abs() < 0.01,
            "1 success, 1 fail should be 50%, got {}", result.success_rate());
    }
    
    #[test]
    fn test_success_rate_with_skipped() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.skip();
        result.skip();
        
        // 2 success out of 4 total = 50%
        // Note: skipped counts in total but not in succeeded
        assert!((result.success_rate() - 50.0).abs() < 0.01,
            "2 success, 2 skipped should be 50%, got {}", result.success_rate());
    }
    
    // ============================================================
    // 🔬 Strict Mathematical Precision Tests (裁判机制)
    // ============================================================
    
    /// Strict test: Success rate formula must be mathematically correct
    #[test]
    fn test_strict_success_rate_formula() {
        // Formula: (succeeded / total) * 100
        let test_cases = [
            (10, 0, 0, 100.0),  // 10 success, 0 fail, 0 skip = 100%
            (5, 5, 0, 50.0),   // 5 success, 5 fail = 50%
            (3, 1, 0, 75.0),   // 3 success, 1 fail = 75%
            (1, 3, 0, 25.0),   // 1 success, 3 fail = 25%
            (0, 10, 0, 0.0),   // 0 success, 10 fail = 0%
            (7, 2, 1, 70.0),   // 7 success, 2 fail, 1 skip = 70%
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
            
            assert!((rate - expected).abs() < 0.001,
                "STRICT: {}s/{}f/{}k expected {}%, got {}%", 
                success, fail, skip, expected, rate);
            assert!((rate - expected_calc).abs() < 0.0001,
                "STRICT: Formula mismatch");
        }
    }
    
    /// Strict test: Large numbers should not overflow
    #[test]
    fn test_strict_large_numbers() {
        let mut result = BatchResult::new();
        
        // Simulate 1 million files
        for _ in 0..500_000 {
            result.success();
        }
        for i in 0..500_000 {
            result.fail(PathBuf::from(format!("f{}.png", i)), "E".to_string());
        }
        
        assert_eq!(result.total, 1_000_000);
        assert!((result.success_rate() - 50.0).abs() < 0.001,
            "STRICT: Large batch should calculate correctly");
    }
    
    // ============================================================
    // Consistency Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_consistency_success_rate() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());
        
        // Same calculation should always produce same result
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
        
        // Total should always equal succeeded + failed + skipped
        assert_eq!(result.total, result.succeeded + result.failed + result.skipped,
            "STRICT: total must equal succeeded + failed + skipped");
    }
}
