//! File Copier Module
//! 
//! 🔥 v6.9.13: 无遗漏设计 - 复制不支持的文件
//! 
//! 确保输出目录包含所有文件：
//! - 支持的格式：由主程序转换
//! - 不支持的格式：直接复制
//! - XMP边车：已被合并，不单独复制

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// 支持的图像格式（会被转换，不需要复制）
pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", 
    "tiff", "tif", "heic", "heif", "avif", "bmp"
];

/// 支持的视频格式（会被转换，不需要复制）
pub const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "webm", "m4v", "wmv", "flv"
];

/// 会被合并的边车格式（不需要复制）
pub const SIDECAR_EXTENSIONS: &[&str] = &["xmp"];

/// 复制结果
#[derive(Debug, Clone)]
pub struct CopyResult {
    pub total_files: usize,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub errors: Vec<(PathBuf, String)>,
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
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    
    // 跳过隐藏文件
    if path.file_name()
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
/// # Arguments
/// * `input_dir` - 输入目录
/// * `output_dir` - 输出目录
/// * `recursive` - 是否递归处理子目录
/// 
/// # Returns
/// 复制结果统计
pub fn copy_unsupported_files(
    input_dir: &Path,
    output_dir: &Path,
    recursive: bool,
) -> CopyResult {
    let mut result = CopyResult::new();
    
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
        let rel_path = path.strip_prefix(input_dir).unwrap_or(path);
        let dest = output_dir.join(rel_path);
        
        // 创建目标目录
        if let Some(parent) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                result.failed += 1;
                result.errors.push((path.to_path_buf(), format!("Failed to create dir: {}", e)));
                continue;
            }
        }
        
        // 复制文件
        match std::fs::copy(path, &dest) {
            Ok(_) => {
                result.copied += 1;
                // 🔥 响亮报告：复制了哪些文件
                let ext = path.extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown");
                println!("📦 Copied unsupported file (.{}): {}", ext, path.display());
                
                // 🔥 v6.9.15: 尝试合并XMP边车（如果是媒体类文件）
                // 对于非媒体文件，XMP无法合并，需要单独复制XMP边车
                if let Err(_) = crate::merge_xmp_for_copied_file(path, &dest) {
                    // XMP合并失败或不存在，检查是否需要复制XMP边车文件
                    copy_xmp_sidecar_if_exists(path, &dest);
                }
            }
            Err(e) => {
                result.failed += 1;
                result.errors.push((path.to_path_buf(), e.to_string()));
                // 🔥 响亮报错
                eprintln!("❌ Failed to copy {}: {}", path.display(), e);
            }
        }
    }
    
    result
}

/// 复制XMP边车文件（如果存在）
/// 用于非媒体文件，因为XMP无法合并到这些文件中
fn copy_xmp_sidecar_if_exists(source: &Path, dest: &Path) {
    let source_str = source.to_string_lossy();
    let dest_str = dest.to_string_lossy();
    
    // 尝试多种XMP命名模式
    let xmp_patterns = [
        format!("{}.xmp", source_str),           // file.psd.xmp
        format!("{}.XMP", source_str),           // file.psd.XMP
        source.with_extension("xmp").to_string_lossy().to_string(),  // file.xmp
    ];
    
    for xmp_source in &xmp_patterns {
        let xmp_path = Path::new(xmp_source);
        if xmp_path.exists() {
            // 计算目标XMP路径
            let xmp_dest = format!("{}.xmp", dest_str);
            if let Err(e) = std::fs::copy(xmp_path, &xmp_dest) {
                eprintln!("⚠️ Failed to copy XMP sidecar: {}", e);
            } else {
                println!("   📋 Copied XMP sidecar: {}", xmp_path.display());
            }
            return;
        }
    }
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
        if path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false) 
        {
            continue;
        }
        
        stats.total += 1;
        
        let ext = path.extension()
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
        (true, format!("✅ Verification passed: {} files (no loss)", actual))
    } else if diff > 0 {
        (false, format!("❌ Verification FAILED: missing {} files! (expected {}, got {})", 
            diff, expected, actual))
    } else {
        // 输出比预期多（可能是动图转换生成了额外文件）
        (true, format!("⚠️ Output has {} extra files (expected {}, got {})", 
            -diff, expected, actual))
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
