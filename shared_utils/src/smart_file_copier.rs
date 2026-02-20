//! 🔥 v7.3.2: Smart File Copier - 统一的文件复制模块
//!
//! 功能：
//! - ✅ 保留完整目录结构
//! - ✅ 保留文件元数据（时间戳、权限）
//! - ✅ 自动合并 XMP 边车文件
//! - ✅ 响亮报错，完全透明
//!
//! 这个模块统一了所有转换器中的文件复制逻辑，避免代码重复。

use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// 🔥 v8.2.2: 检测文件的实际格式（通过魔法字节）
/// 
/// 返回格式名称（小写），如 "jpeg", "png", "webp", "heic", "tiff" 等
fn detect_content_format(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut buffer = [0u8; 24];
    
    if file.read_exact(&mut buffer).is_err() {
        return None;
    }
    
    // JPEG: FF D8 FF
    if buffer.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpeg".to_string());
    }
    
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if buffer.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("png".to_string());
    }
    
    // GIF: 47 49 46 38 39 61 (GIF89a) or 47 49 46 38 37 61 (GIF87a)
    if buffer.starts_with(&[0x47, 0x49, 0x46, 0x38]) {
        return Some("gif".to_string());
    }
    
    // WebP: RIFF....WEBP
    if buffer.starts_with(&[0x52, 0x49, 0x46, 0x46]) && buffer[8..12] == [0x57, 0x45, 0x42, 0x50] {
        return Some("webp".to_string());
    }
    
    // HEIC/HEIF: 00 00 00 18 66 74 79 70 (ftyp box)
    // Brands: heic, heix, heim, heis, mif1, msf1
    if buffer.len() >= 12 && buffer[4..8] == [0x66, 0x74, 0x79, 0x70] {
        let brand = std::str::from_utf8(&buffer[8..12]).ok()?;
        if matches!(brand, "heic" | "heix" | "heim" | "heis" | "mif1" | "msf1") {
            return Some("heic".to_string());
        }
        // AVIF: brand avif or avis
        if matches!(brand, "avif" | "avis") {
            return Some("avif".to_string());
        }
    }
    
    // TIFF: II* (little-endian) or MM* (big-endian)
    if buffer.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || buffer.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        return Some("tiff".to_string());
    }

    // JXL codestream: FF 0A
    if buffer.starts_with(&[0xFF, 0x0A]) {
        return Some("jxl".to_string());
    }

    // JXL container: 00 00 00 0C 4A 58 4C 20 0D 0A 87 0A
    if buffer.starts_with(&[0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A]) {
        return Some("jxl".to_string());
    }
    
    None
}

/// 🔥 v8.2.2: 检查并修正文件扩展名以匹配实际内容
/// 
/// 如果文件扩展名与实际内容格式不匹配，重命名文件为正确的扩展名
/// 这对于处理"伪装"文件（如 HEIC 内容但 .jpeg 扩展名）很重要
/// 
/// 返回：如果扩展名被修正，返回新路径；否则返回原路径
pub fn fix_extension_if_mismatch(path: &Path) -> Result<PathBuf> {
    let current_ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    
    if let Some(content_format) = detect_content_format(path) {
        // 检查扩展名是否与内容匹配
        let is_mismatch = match content_format.as_str() {
            "jpeg" => !matches!(current_ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif"),
            "png" => current_ext != "png",
            "webp" => current_ext != "webp",
            "gif" => current_ext != "gif",
            "heic" => !matches!(current_ext.as_str(), "heic" | "heif" | "hif"),
            "avif" => current_ext != "avif",
            "jxl" => current_ext != "jxl",
            "tiff" => !matches!(current_ext.as_str(), "tiff" | "tif"),
            _ => false,
        };
        
        if is_mismatch {
            // Create new path
            let new_path = path.with_extension(&content_format);

            // 🔥 v8.2.4: Safety — refuse to overwrite a DIFFERENT file that already exists
            if new_path.exists() {
                // Check if it's the same inode (hard link) or truly different
                let src_meta = fs::metadata(path);
                let dst_meta = fs::metadata(&new_path);
                let same_file = match (src_meta, dst_meta) {
                    #[cfg(unix)]
                    (Ok(s), Ok(d)) => {
                        use std::os::unix::fs::MetadataExt;
                        s.ino() == d.ino() && s.dev() == d.dev()
                    }
                    _ => false,
                };
                if !same_file {
                    eprintln!("⚠️  [Extension Fix] SKIPPED: {} -> .{} (target {} already exists)",
                        path.display(), content_format, new_path.display());
                    return Ok(path.to_path_buf());
                }
            }

            eprintln!("⚠️  [Extension Fix] {} -> .{} (content does not match extension)",
                     path.display(), content_format);

            // Rename file
            fs::rename(path, &new_path)
                .with_context(|| format!("Failed to rename {} to {}", path.display(), new_path.display()))?;

            eprintln!("✅  [Extension Fix] Complete: {}", new_path.display());

            return Ok(new_path);
        }
    }
    
    Ok(path.to_path_buf())
}

/// 🔥 v7.3.2: 智能文件复制 - 保留目录结构 + 元数据 + XMP
///
/// 这是所有转换器应该使用的统一复制函数。
///
/// # 功能
/// - 自动计算相对路径，保留目录结构
/// - 自动创建目标目录
/// - 保留文件时间戳和权限
/// - 自动合并 XMP 边车文件
/// - 响亮报错，不静默失败
///
/// # 参数
/// - `source`: 源文件路径
/// - `output_dir`: 输出目录
/// - `base_dir`: 基准目录（用于计算相对路径）
/// - `verbose`: 是否打印详细信息
///
/// # 返回
/// - `Ok(PathBuf)`: 目标文件路径
/// - `Err`: 复制失败的详细错误
///
/// # 示例
/// ```ignore
/// let dest = smart_copy_with_structure(
///     &input_file,
///     &output_dir,
///     Some(&base_dir),
///     true
/// )?;
/// ```
pub fn smart_copy_with_structure(
    source: &Path,
    output_dir: &Path,
    base_dir: Option<&Path>,
    verbose: bool,
) -> Result<PathBuf> {
    // 🔥 计算目标路径（保留目录结构）
    let dest = if let Some(base) = base_dir {
        let rel_path = source.strip_prefix(base).unwrap_or(source);
        output_dir.join(rel_path)
    } else {
        // 没有 base_dir，使用文件名（向后兼容）
        let file_name = source.file_name().context("Source file has no filename")?;
        output_dir.join(file_name)
    };

    // 🔥 创建目标目录
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // 🔥 复制文件（字节级复制，不修改内容）
    if !dest.exists() {
        fs::copy(source, &dest).with_context(|| {
            format!("Failed to copy {} to {}", source.display(), dest.display())
        })?;

        if verbose {
            eprintln!("   📋 Copied: {} → {}", source.display(), dest.display());
        }
    } else if verbose {
        eprintln!("   ⏭️  Already exists: {}", dest.display());
    }

    // 🔥 v8.2.2: 内容感知扩展名修正
    // 在元数据处理前，先修正扩展名以匹配实际内容
    // 这样后续的 magick 结构修复和 exiftool 元数据处理才能正确识别格式
    let dest = fix_extension_if_mismatch(&dest)?;

    // 🔥 保留元数据（时间戳、权限）+ 自动合并 XMP
    // 此时 dest 已经是正确的扩展名，元数据处理会正确识别格式
    crate::copy_metadata(source, &dest);

    Ok(dest)
}

/// 🔥 v7.3.2: 批量智能复制（用于跳过/失败场景）
///
/// 当转换失败或跳过时，使用此函数复制原始文件到输出目录。
///
/// # 参数
/// - `source`: 源文件路径
/// - `output_dir`: 输出目录（如果为 None，不执行复制）
/// - `base_dir`: 基准目录
/// - `verbose`: 是否打印详细信息
///
/// # 返回
/// - `Ok(Some(PathBuf))`: 复制成功，返回目标路径
/// - `Ok(None)`: 没有 output_dir，跳过复制
/// - `Err`: 复制失败（响亮报错）
pub fn copy_on_skip_or_fail(
    source: &Path,
    output_dir: Option<&Path>,
    base_dir: Option<&Path>,
    verbose: bool,
) -> Result<Option<PathBuf>> {
    if let Some(out_dir) = output_dir {
        match smart_copy_with_structure(source, out_dir, base_dir, verbose) {
            Ok(dest) => Ok(Some(dest)),
            Err(e) => {
                // 🔥 响亮报错！
                eprintln!("❌ COPY FAILED: {}", e);
                eprintln!("   Source: {}", source.display());
                eprintln!("   Output dir: {}", out_dir.display());
                Err(e)
            }
        }
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_smart_copy_preserves_structure() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("input");
        let output = temp.path().join("output");

        // 创建测试文件
        fs::create_dir_all(base.join("photos/2024")).unwrap();
        let source = base.join("photos/2024/test.txt");
        fs::write(&source, "test").unwrap();

        // 执行复制
        let dest = smart_copy_with_structure(&source, &output, Some(&base), false).unwrap();

        // 验证目录结构
        assert_eq!(dest, output.join("photos/2024/test.txt"));
        assert!(dest.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "test");
    }

    #[test]
    fn test_copy_on_skip_with_none() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("test.txt");
        fs::write(&source, "test").unwrap();

        // output_dir 为 None 应该返回 Ok(None)
        let result = copy_on_skip_or_fail(&source, None, None, false).unwrap();
        assert!(result.is_none());
    }
}
