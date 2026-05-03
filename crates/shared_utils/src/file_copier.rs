//! File Copier Module
//!
//! Ensures the output directory contains all files by copying unsupported formats
//! while skipping converted files and merged XMP sidecars.

use crate::quality_matcher::SourceCodec;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = SourceCodec::supported_image_extensions();
pub const IMAGE_EXTENSIONS_FOR_CONVERT: &[&str] = SourceCodec::image_extensions_for_convert();
pub const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = SourceCodec::supported_video_extensions();

pub const IMAGE_EXTENSIONS_ANALYZE: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif",
];

pub const SIDECAR_EXTENSIONS: &[&str] = &["xmp"];

#[derive(Debug, Clone)]
pub struct CopyResult {
    pub total_files: usize,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub errors: Vec<(PathBuf, String, String)>,
}

impl CopyResult {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total_files: 0,
            copied: 0,
            skipped: 0,
            failed: 0,
            errors: Vec::new(),
        }
    }
}

impl Default for CopyResult {
    fn default() -> Self {
        Self::new()
    }
}

fn should_copy_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.starts_with('.') {
        return false;
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    !SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str())
        && !SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str())
        && !SIDECAR_EXTENSIONS.contains(&ext.as_str())
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
pub fn copy_unsupported_files(input_dir: &Path, output_dir: &Path, recursive: bool) -> CopyResult {
    let mut result = CopyResult::new();

    info!(
        input_dir = %input_dir.display(),
        output_dir = %output_dir.display(),
        recursive = recursive,
        "Starting batch file copy operation"
    );

    let walker = if recursive {
        WalkDir::new(input_dir).follow_links(true)
    } else {
        WalkDir::new(input_dir).max_depth(1)
    };

    let mut total_files = 0usize;
    for entry in walker {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_file() && should_copy_file(entry.path()) {
                    total_files += 1;
                }
            }
            Err(err) => {
                warn!(
                    input_dir = %input_dir.display(),
                    error = %err,
                    "Failed to inspect directory entry during pre-scan"
                );
            }
        }
    }

    debug!(total_files = total_files, "Pre-scan completed");

    let walker = if recursive {
        WalkDir::new(input_dir).follow_links(true)
    } else {
        WalkDir::new(input_dir).max_depth(1)
    };

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                let path = err
                    .path()
                    .map_or_else(|| input_dir.to_path_buf(), Path::to_path_buf);
                let error_msg = format!("Directory traversal failed: {err}");
                warn!(
                    path = %path.display(),
                    error = %err,
                    "Directory traversal failed during batch copy"
                );
                result.failed += 1;
                result.errors.push((path, error_msg, "walkdir".to_string()));
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        result.total_files += 1;

        if !should_copy_file(path) {
            result.skipped += 1;
            continue;
        }

        let rel_path = match path.strip_prefix(input_dir) {
            Ok(p) => p,
            Err(e) => {
                let error_msg = format!("Failed to compute relative path: {e}");
                error!(
                    file = %path.display(),
                    input_dir = %input_dir.display(),
                    error = %e,
                    "Path computation failed"
                );
                eprintln!("❌ Path error for {}: {}", path.display(), error_msg);
                result.failed += 1;
                result
                    .errors
                    .push((path.to_path_buf(), error_msg, "compute_path".to_string()));
                continue;
            }
        };

        let dest = output_dir.join(rel_path);

        // Concurrency Guard: If running img and vid in parallel, they might both try to copy
        // the same unsupported file (e.g. document.pdf). Check if it already exists
        // with the correct size to avoid redundant I/O and potential write-contention.
        if dest.exists() {
            if let (Ok(src_meta), Ok(dst_meta)) =
                (std::fs::metadata(path), std::fs::metadata(&dest))
            {
                if src_meta.len() == dst_meta.len() {
                    debug!(file = %path.display(), "Skipping unsupported file copy (already exists in destination with matching size)");
                    result.skipped += 1;
                    continue;
                }
            }
        }

        if let Some(parent) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                let error_msg = format!("Failed to create directory: {e}");
                error!(
                    file = %path.display(),
                    dest_dir = %parent.display(),
                    error = %e,
                    "Directory creation failed"
                );
                eprintln!(
                    "❌ Failed to create directory for {}: {}",
                    path.display(),
                    error_msg
                );
                result.failed += 1;
                result
                    .errors
                    .push((path.to_path_buf(), error_msg, "create_dir".to_string()));
                continue;
            }
        }

        match std::fs::copy(path, &dest) {
            Ok(_) => {
                result.copied += 1;

                crate::copy_metadata(path, &dest);

                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown");
                println!("📦 Copied unsupported file (.{}): {}", ext, path.display());

                debug!(
                    source = %path.display(),
                    dest = %dest.display(),
                    extension = ext,
                    "File copied successfully"
                );

                match crate::merge_xmp_for_copied_file(path, &dest) {
                    Ok(true) => {
                        debug!(file = %path.display(), "XMP merged successfully");
                    }
                    Ok(false) => {
                        debug!(file = %path.display(), "No XMP sidecar found");
                    }
                    Err(e) => {
                        warn!(
                            file = %path.display(),
                            error = %e,
                            "XMP merge failed, trying to copy sidecar"
                        );
                        println!("⚠️ XMP merge failed ({e}), trying to copy sidecar...");
                        copy_xmp_sidecar_if_exists(path, &dest);
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("Copy failed: {e}");
                error!(
                    source = %path.display(),
                    dest = %dest.display(),
                    error = %e,
                    error_kind = ?e.kind(),
                    "File copy operation failed"
                );
                eprintln!("❌ Failed to copy {}: {}", path.display(), e);
                result.failed += 1;
                result
                    .errors
                    .push((path.to_path_buf(), error_msg, "copy_file".to_string()));
            }
        }
    }

    info!(
        total = result.total_files,
        copied = result.copied,
        skipped = result.skipped,
        failed = result.failed,
        "Batch file copy operation completed"
    );

    if result.failed > 0 {
        warn!(
            failed_count = result.failed,
            "Some files failed to copy, see errors for details"
        );
        eprintln!(
            "⚠️ Batch copy completed with {} failures out of {} files",
            result.failed, result.total_files
        );
    }

    result
}

fn copy_xmp_sidecar_if_exists(source: &Path, dest: &Path) {
    let source_str = source.to_string_lossy();
    let dest_str = dest.to_string_lossy();

    let xmp_patterns = [
        format!("{source_str}.xmp"),
        format!("{source_str}.XMP"),
        source.with_extension("xmp").to_string_lossy().to_string(),
    ];

    for xmp_source in &xmp_patterns {
        let xmp_path = Path::new(xmp_source);
        if xmp_path.exists() {
            let xmp_dest = format!("{dest_str}.xmp");

            match std::fs::copy(xmp_path, &xmp_dest) {
                Ok(_) => {
                    crate::copy_metadata(xmp_path, Path::new(&xmp_dest));
                    println!("   📋 Copied XMP sidecar: {}", xmp_path.display());

                    debug!(
                        source = %xmp_path.display(),
                        dest = %xmp_dest,
                        "XMP sidecar copied successfully"
                    );
                }
                Err(e) => {
                    error!(
                        source = %xmp_path.display(),
                        dest = %xmp_dest,
                        error = %e,
                        error_kind = ?e.kind(),
                        "Failed to copy XMP sidecar"
                    );
                    eprintln!(
                        "⚠️ Failed to copy XMP sidecar {}: {}",
                        xmp_path.display(),
                        e
                    );
                }
            }
            return;
        }
    }

    debug!(
        source = %source.display(),
        "No XMP sidecar found for file"
    );
}

#[derive(Debug, Clone)]
pub struct FileStats {
    pub total: usize,
    pub images: usize,
    pub videos: usize,
    pub sidecars: usize,
    pub others: usize,
}

impl FileStats {
    #[must_use]
    pub const fn expected_output(&self) -> usize {
        self.total - self.sidecars
    }
}

pub fn count_files(dir: &Path, recursive: bool) -> FileStats {
    let mut stats = FileStats {
        total: 0,
        images: 0,
        videos: 0,
        sidecars: 0,
        others: 0,
    };

    let walker = if recursive {
        WalkDir::new(dir).follow_links(true)
    } else {
        WalkDir::new(dir).max_depth(1)
    };

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                warn!(
                    dir = %dir.display(),
                    error = %err,
                    "Failed to inspect directory entry while counting files"
                );
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }

        stats.total += 1;

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();

        if SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            stats.images += 1;
        } else if SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            stats.videos += 1;
        } else if SIDECAR_EXTENSIONS.contains(&ext.as_str()) {
            stats.sidecars += 1;
        } else {
            stats.others += 1;
        }
    }

    stats
}

#[derive(Debug)]
pub struct VerifyResult {
    pub passed: bool,
    pub expected: usize,
    pub actual: usize,
    pub diff: i64,
    pub message: String,
}

#[must_use]
pub fn verify_output_completeness(
    input_dir: &Path,
    output_dir: &Path,
    recursive: bool,
) -> VerifyResult {
    let input_stats = count_files(input_dir, recursive);
    let output_stats = count_files(output_dir, recursive);

    let expected = input_stats.expected_output();
    let actual = output_stats.total;
    let diff = crate::numeric_cast::usize_to_i64_sat(expected)
        - crate::numeric_cast::usize_to_i64_sat(actual);

    let (passed, message) = match diff.cmp(&0) {
        std::cmp::Ordering::Equal => (
            true,
            format!("✅ Verification passed: {actual} files (no loss)"),
        ),
        std::cmp::Ordering::Greater => (
            false,
            format!(
                "❌ Verification FAILED: missing {diff} files! (expected {expected}, got {actual})"
            ),
        ),
        std::cmp::Ordering::Less => (
            true,
            format!(
                "⚠️ Output has {} extra files (expected {}, got {})",
                -diff, expected, actual
            ),
        ),
    };

    VerifyResult {
        passed,
        expected,
        actual,
        diff,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_copy_file() {
        assert!(!should_copy_file(Path::new("test.jpg")));
        assert!(!should_copy_file(Path::new("test.PNG")));
        assert!(!should_copy_file(Path::new("test.mp4")));

        assert!(!should_copy_file(Path::new("test.xmp")));

        assert!(should_copy_file(Path::new("test.psd")));
        assert!(should_copy_file(Path::new("test.txt")));
        assert!(should_copy_file(Path::new("test.pdf")));

        assert!(!should_copy_file(Path::new(".DS_Store")));
    }
}
