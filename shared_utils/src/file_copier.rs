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

/// 支持的图像格式（会被转换，不需要复制）
pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif", "avif",
    "bmp",
];

/// 分析阶段使用的图像扩展名子集（不含 heic/heif/avif，供 analyze 命令使用）
pub const IMAGE_EXTENSIONS_ANALYZE: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif",
];

pub const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "ts", "mts",
];

/// 会被合并的边车格式（不需要复制）
pub const SIDECAR_EXTENSIONS: &[&str] = &["xmp"];

/// 复制结果
///
/// 包含详细的操作统计和错误信息，支持批量操作的弹性处理
#[derive(Debug, Clone)]
pub struct CopyResult {
    /// 总文件数（包括需要复制和跳过的）
    pub total_files: usize,
    /// 成功复制的文件数
    pub copied: usize,
    /// 跳过的文件数（支持的格式、边车文件等）
    pub skipped: usize,
    /// 失败的文件数
    pub failed: usize,
    /// 错误列表：(文件路径, 错误消息, 操作类型)
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

/// 检查文件是否需要复制（不是支持的格式，也不是边车文件）
fn should_copy_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    // 跳过隐藏文件
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
    {
        return false;
    }

    // 跳过支持的图像格式（会被转换）
    if SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }

    // 跳过支持的视频格式（会被转换）
    if SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }

    // 跳过边车文件（会被合并）
    if SIDECAR_EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }

    true
}

/// 复制不支持的文件到输出目录
///
/// 🔥 v7.8: 增强错误处理
/// - 批量操作弹性：单个文件失败不影响其他文件
/// - 所有错误都包含文件路径和操作上下文
/// - 详细的日志记录
///
/// # Arguments
/// * `input_dir` - 输入目录
/// * `output_dir` - 输出目录
/// * `recursive` - 是否递归处理子目录
///
/// # Returns
/// 复制结果统计，包含所有错误信息
pub fn copy_unsupported_files(input_dir: &Path, output_dir: &Path, recursive: bool) -> CopyResult {
    let mut result = CopyResult::new();

    // 记录操作开始
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

    // 🔥 v7.7: 预扫描文件数量,决定是否启用心跳
    let total_files: usize = walker
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| should_copy_file(e.path()))
        .count();

    debug!(total_files = total_files, "Pre-scan completed");

    // 🔥 v7.7: 心跳检测 - 仅当文件数>10时启用
    let _heartbeat = if total_files > 10 {
        Some(crate::universal_heartbeat::HeartbeatGuard::new(
            crate::universal_heartbeat::HeartbeatConfig::medium("Batch File Copy")
                .with_info(format!("{} files", total_files)),
        ))
    } else {
        None
    };

    // 重新创建walker进行实际复制
    let walker = if recursive {
        WalkDir::new(input_dir).follow_links(true)
    } else {
        WalkDir::new(input_dir).max_depth(1)
    };

    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        result.total_files += 1;

        if !should_copy_file(path) {
            result.skipped += 1;
            continue;
        }

        // 计算相对路径
        let rel_path = match path.strip_prefix(input_dir) {
            Ok(p) => p,
            Err(e) => {
                // 🔥 响亮报错：路径处理失败
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
                continue; // 🔥 批量操作弹性：继续处理其他文件
            }
        };

        let dest = output_dir.join(rel_path);

        // 创建目标目录
        if let Some(parent) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                // 🔥 响亮报错：目录创建失败
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
                continue; // 🔥 批量操作弹性：继续处理其他文件
            }
        }

        // 复制文件
        match std::fs::copy(path, &dest) {
            Ok(_) => {
                result.copied += 1;

                // 🔥 v7.4.6: 保留元数据（时间戳、权限、xattr）
                crate::copy_metadata(path, &dest);

                // 🔥 响亮报告：复制了哪些文件
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

                // 🔥 v6.9.16: 优先尝试合并XMP（ExifTool支持PSD等多种格式）
                // 只有合并失败时才复制XMP边车文件
                match crate::merge_xmp_for_copied_file(path, &dest) {
                    Ok(true) => {
                        // XMP合并成功，已打印消息
                        debug!(file = %path.display(), "XMP merged successfully");
                    }
                    Ok(false) => {
                        // 没有找到XMP边车，无需处理
                        debug!(file = %path.display(), "No XMP sidecar found");
                    }
                    Err(e) => {
                        // 🔥 XMP合并失败，回退到复制边车文件
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
                // 🔥 响亮报错：文件复制失败
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
                // 🔥 批量操作弹性：继续处理其他文件
            }
        }
    }

    // 记录操作完成
    info!(
        total = result.total_files,
        copied = result.copied,
        skipped = result.skipped,
        failed = result.failed,
        "Batch file copy operation completed"
    );

    // 如果有失败，响亮报告
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

/// 复制XMP边车文件（如果存在）
/// 用于非媒体文件，因为XMP无法合并到这些文件中
///
/// 🔥 v7.8: 增强错误处理和日志记录
fn copy_xmp_sidecar_if_exists(source: &Path, dest: &Path) {
    let source_str = source.to_string_lossy();
    let dest_str = dest.to_string_lossy();

    // 尝试多种XMP命名模式
    let xmp_patterns = [
        format!("{}.xmp", source_str), // file.psd.xmp
        format!("{}.XMP", source_str), // file.psd.XMP
        source.with_extension("xmp").to_string_lossy().to_string(), // file.xmp
    ];

    for xmp_source in &xmp_patterns {
        let xmp_path = Path::new(xmp_source);
        if xmp_path.exists() {
            // 计算目标XMP路径
            let xmp_dest = format!("{}.xmp", dest_str);

            match std::fs::copy(xmp_path, &xmp_dest) {
                Ok(_) => {
                    // 🔥 v7.4.6: 保留XMP文件的元数据
                    crate::copy_metadata(xmp_path, Path::new(&xmp_dest));
                    println!("   📋 Copied XMP sidecar: {}", xmp_path.display());

                    debug!(
                        source = %xmp_path.display(),
                        dest = %xmp_dest,
                        "XMP sidecar copied successfully"
                    );
                }
                Err(e) => {
                    // 🔥 响亮报错：XMP复制失败
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

    // 没有找到XMP边车文件
    debug!(
        source = %source.display(),
        "No XMP sidecar found for file"
    );
}

/// 统计目录中的文件数量
#[derive(Debug, Clone)]
pub struct FileStats {
    pub total: usize,
    pub images: usize,
    pub videos: usize,
    pub sidecars: usize,
    pub others: usize,
}

impl FileStats {
    /// 预期输出数量 = 全部文件 - 边车文件（边车被合并）
    pub fn expected_output(&self) -> usize {
        self.total - self.sidecars
    }
}

/// 统计目录中的文件
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

    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        // 跳过隐藏文件
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

/// 验证输出完整性
#[derive(Debug)]
pub struct VerifyResult {
    pub passed: bool,
    pub expected: usize,
    pub actual: usize,
    pub diff: i64,
    pub message: String,
}

/// 验证输出目录的文件数量是否符合预期
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
        // 输出比预期多（可能是动图转换生成了额外文件）
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
        // 支持的格式不应复制
        assert!(!should_copy_file(Path::new("test.jpg")));
        assert!(!should_copy_file(Path::new("test.PNG")));
        assert!(!should_copy_file(Path::new("test.mp4")));

        // 边车文件不应复制
        assert!(!should_copy_file(Path::new("test.xmp")));

        // 不支持的格式应该复制
        assert!(should_copy_file(Path::new("test.psd")));
        assert!(should_copy_file(Path::new("test.txt")));
        assert!(should_copy_file(Path::new("test.pdf")));

        // 隐藏文件不应复制
        assert!(!should_copy_file(Path::new(".DS_Store")));
    }
}
