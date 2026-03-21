//! File Copier Module
//!
//! 🔥 v6.9.13: 无遗漏设计 - 复制不支持的文件
//! 🔥 v7.8: 增强错误处理 - 添加文件路径上下文，批量操作弹性
//!
//! 确保输出目录包含所有文件：
//! - 支持的格式：由主程序转换
//! - 不支持的格式：直接复制
//! - XMP边车：已被合并，不单独复制
//!
//! ## 错误处理策略
//! - 所有IO错误都包含文件路径上下文
//! - 批量操作在部分失败时继续处理（弹性设计）
//! - 所有失败都记录到日志和错误列表
//! - 响亮报错，不静默失败

use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif", "avif",
    "bmp", "ico", "svg", "jp2", "j2k", "jxl",
];

/// Image extensions to consider when collecting files for conversion (e.g. img-hevc → JXL).
/// Excludes formats that are already the target: .jxl (no point converting JXL→JXL).
pub const IMAGE_EXTENSIONS_FOR_CONVERT: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif", "avif",
    "bmp", "ico", "svg", "jp2", "j2k",
];

/// Video extensions for conversion input. **Do not exclude mov/mp4** by extension:
/// .mov can contain ProRes (must convert) or HEVC (skip by codec); .mp4 can contain H.264 or HEVC.
/// Skip vs convert is decided by **codec detection** (e.g. should_skip_video_codec), not by extension.
pub const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "ts", "mts",
    "m2ts", "m2v", "3gp", "3g2", "ogv", "f4v", "asf",
];

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
    pub fn new() -> Self {
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
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
    {
        return false;
    }

    if SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }

    if SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }

    if SIDECAR_EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }

    true
}

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
    for entry in walker.into_iter() {
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

    let _heartbeat = if total_files > 10 {
        Some(crate::universal_heartbeat::HeartbeatGuard::new(
            crate::universal_heartbeat::HeartbeatConfig::medium("Batch File Copy")
                .with_info(format!("{} files", total_files)),
        ))
    } else {
        None
    };

    let walker = if recursive {
        WalkDir::new(input_dir).follow_links(true)
    } else {
        WalkDir::new(input_dir).max_depth(1)
    };

    for entry in walker.into_iter() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                let path = err
                    .path()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| input_dir.to_path_buf());
                let error_msg = format!("Directory traversal failed: {}", err);
                warn!(
                    path = %path.display(),
                    error = %err,
                    "Directory traversal failed during batch copy"
                );
                result.failed += 1;
                result
                    .errors
                    .push((path, error_msg, "walkdir".to_string()));
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
                let error_msg = format!("Failed to compute relative path: {}", e);
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

        if let Some(parent) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                let error_msg = format!("Failed to create directory: {}", e);
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
                        println!("⚠️ XMP merge failed ({}), trying to copy sidecar...", e);
                        copy_xmp_sidecar_if_exists(path, &dest);
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("Copy failed: {}", e);
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
        format!("{}.xmp", source_str),
        format!("{}.XMP", source_str),
        source.with_extension("xmp").to_string_lossy().to_string(),
    ];

    for xmp_source in &xmp_patterns {
        let xmp_path = Path::new(xmp_source);
        if xmp_path.exists() {
            let xmp_dest = format!("{}.xmp", dest_str);

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
    pub fn expected_output(&self) -> usize {
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

    for entry in walker.into_iter() {
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
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }

        stats.total += 1;

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
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

pub fn verify_output_completeness(
    input_dir: &Path,
    output_dir: &Path,
    recursive: bool,
) -> VerifyResult {
    let input_stats = count_files(input_dir, recursive);
    let output_stats = count_files(output_dir, recursive);

    let expected = input_stats.expected_output();
    let actual = output_stats.total;
    let diff = expected as i64 - actual as i64;

    let (passed, message) = if diff == 0 {
        (
            true,
            format!("✅ Verification passed: {} files (no loss)", actual),
        )
    } else if diff > 0 {
        (
            false,
            format!(
                "❌ Verification FAILED: missing {} files! (expected {}, got {})",
                diff, expected, actual
            ),
        )
    } else {
        (
            true,
            format!(
                "⚠️ Output has {} extra files (expected {}, got {})",
                -diff, expected, actual
            ),
        )
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
