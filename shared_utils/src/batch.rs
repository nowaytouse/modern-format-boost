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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_result() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());
        result.skip();

        assert_eq!(result.total, 4);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.success_rate(), 50.0);
    }
}
