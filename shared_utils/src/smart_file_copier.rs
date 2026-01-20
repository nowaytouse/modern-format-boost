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
use std::path::{Path, PathBuf};

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

    // 🔥 复制文件
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

    // 🔥 保留元数据（时间戳、权限）+ 自动合并 XMP
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
