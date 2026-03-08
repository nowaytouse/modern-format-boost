//! 🔥 v7.3.2: Smart File Copier - 统一的文件复制模块
//!
//! 功能：
//! - ✅ 保留完整目录结构
//! - ✅ 保留文件元数据（时间戳、权限）
//! - ✅ 自动合并 XMP 边车文件
//! - ✅ 响亮报错，完全透明
//!
//! 这个模块统一了所有转换器中的文件复制逻辑，避免代码重复。
//!
//! ## 扩展名修正与校验顺序
//! - `fix_extension_if_mismatch` 按文件头魔数修正扩展名（避免伪装扩展名导致误判/panic）。
//! - 设计约定：**先修正、再按扩展名做分支**。视频/图片入口（cli_runner、img_*）均在处理前调用修正，后续所有「仅验证扩展名」的逻辑应基于修正后的路径。参见 CODE_AUDIT.md §36。

use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Minimum buffer size for format detection. Video containers need at least 12 bytes (e.g. RIFF+AVI), ftyp uses 8..12.
const DETECT_BUF_LEN: usize = 32;

fn detect_content_format(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut buffer = [0u8; DETECT_BUF_LEN];

    if file.read_exact(&mut buffer).is_err() {
        return None;
    }

    // --- Image formats (checked first so e.g. RIFF is not mistaken for AVI when it's WebP) ---
    if buffer.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpeg".to_string());
    }

    if buffer.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("png".to_string());
    }

    if buffer.starts_with(&[0x47, 0x49, 0x46, 0x38]) {
        return Some("gif".to_string());
    }

    if buffer.starts_with(&[0x52, 0x49, 0x46, 0x46]) && buffer[8..12] == [0x57, 0x45, 0x42, 0x50] {
        return Some("webp".to_string());
    }

    if buffer.len() >= 12 && buffer[4..8] == [0x66, 0x74, 0x79, 0x70] {
        let brand = std::str::from_utf8(&buffer[8..12]).ok()?;
        if matches!(brand, "heic" | "heix" | "heim" | "heis" | "mif1" | "msf1") {
            return Some("heic".to_string());
        }
        if matches!(brand, "avif" | "avis") {
            return Some("avif".to_string());
        }
        // MP4/MOV: ftyp with video brands (isom, mp41, mp42, M4V, qt  , etc.)
        if matches!(
            brand,
            "isom" | "iso2" | "mp41" | "mp42" | "m4v " | "avc1" | "msdh" | "dash" | "ndas"
        ) {
            return Some("mp4".to_string());
        }
        if brand == "qt  " {
            return Some("mov".to_string());
        }
    }

    if buffer.starts_with(&[0x49, 0x49, 0x2A, 0x00])
        || buffer.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
    {
        return Some("tiff".to_string());
    }

    if buffer.starts_with(&[0xFF, 0x0A]) {
        return Some("jxl".to_string());
    }

    if buffer.starts_with(&[
        0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
    ]) {
        return Some("jxl".to_string());
    }

    // --- Video containers (checked after all image/anim formats so GIF/WebP/AVIF are never misclassified as video) ---
    // AVI: RIFF....AVI 
    if buffer.starts_with(&[0x52, 0x49, 0x46, 0x46])
        && buffer.len() >= 12
        && buffer[8..12] == [0x41, 0x56, 0x49, 0x20]
    {
        return Some("avi".to_string());
    }

    // FLV
    if buffer.starts_with(&[0x46, 0x4C, 0x56]) {
        return Some("flv".to_string());
    }

    // EBML → MKV/WebM (same magic; we use mkv as generic container)
    if buffer.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("mkv".to_string());
    }

    // ASF (WMV/WMA container)
    if buffer.starts_with(&[
        0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA,
        0x00, 0x62, 0xCE, 0x6C,
    ]) {
        return Some("wmv".to_string());
    }

    None
}

pub fn fix_extension_if_mismatch(path: &Path) -> Result<PathBuf> {
    let current_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if let Some(content_format) = detect_content_format(path) {
        let is_mismatch = match content_format.as_str() {
            "jpeg" => !matches!(current_ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif"),
            "png" => current_ext != "png",
            "webp" => current_ext != "webp",
            "gif" => current_ext != "gif",
            "heic" => !matches!(current_ext.as_str(), "heic" | "heif" | "hif"),
            "avif" => current_ext != "avif",
            "jxl" => current_ext != "jxl",
            "tiff" => !matches!(current_ext.as_str(), "tiff" | "tif"),
            "mp4" => !matches!(current_ext.as_str(), "mp4" | "m4v"),
            "mov" => current_ext != "mov",
            "avi" => current_ext != "avi",
            "flv" => current_ext != "flv",
            "mkv" => !matches!(current_ext.as_str(), "mkv" | "webm"),
            "wmv" => !matches!(current_ext.as_str(), "wmv" | "asf"),
            _ => false,
        };

        if is_mismatch {
            let new_path = path.with_extension(&content_format);

            if new_path.exists() {
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
                    eprintln!(
                        "⚠️  [Extension Fix] SKIPPED: {} -> .{} (target {} already exists)",
                        path.display(),
                        content_format,
                        new_path.display()
                    );
                    return Ok(path.to_path_buf());
                }
            }

            eprintln!(
                "⚠️  [Extension Fix] {} -> .{} (content does not match extension)",
                path.display(),
                content_format
            );

            fs::rename(path, &new_path).with_context(|| {
                format!(
                    "Failed to rename {} to {}",
                    path.display(),
                    new_path.display()
                )
            })?;

            eprintln!("✅  [Extension Fix] Complete: {}", new_path.display());

            return Ok(new_path);
        }
    }

    Ok(path.to_path_buf())
}

pub fn smart_copy_with_structure(
    source: &Path,
    output_dir: &Path,
    base_dir: Option<&Path>,
    verbose: bool,
) -> Result<PathBuf> {
    let dest = if let Some(base) = base_dir {
        let rel_path = source.strip_prefix(base).unwrap_or(source);
        output_dir.join(rel_path)
    } else {
        let file_name = source.file_name().context("Source file has no filename")?;
        output_dir.join(file_name)
    };

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

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

    let dest = fix_extension_if_mismatch(&dest)?;

    crate::copy_metadata(source, &dest);

    Ok(dest)
}

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

        fs::create_dir_all(base.join("photos/2024")).unwrap();
        let source = base.join("photos/2024/test.txt");
        fs::write(&source, "test").unwrap();

        let dest = smart_copy_with_structure(&source, &output, Some(&base), false).unwrap();

        assert_eq!(dest, output.join("photos/2024/test.txt"));
        assert!(dest.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "test");
    }

    #[test]
    fn test_copy_on_skip_with_none() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("test.txt");
        fs::write(&source, "test").unwrap();

        let result = copy_on_skip_or_fail(&source, None, None, false).unwrap();
        assert!(result.is_none());
    }

    /// Content is video (MP4 ftyp+isom) but extension was wrong → corrected to .mp4.
    #[test]
    fn test_fix_extension_video_content_wrong_ext() {
        let temp = TempDir::new().unwrap();
        // File named .jpg but content is MP4 (ftyp box, isom brand)
        let wrong_ext = temp.path().join("video.jpg");
        let mut header = [0u8; 32];
        header[4..8].copy_from_slice(b"ftyp");
        header[8..12].copy_from_slice(b"isom");
        fs::write(&wrong_ext, header).unwrap();

        let fixed = fix_extension_if_mismatch(&wrong_ext).unwrap();
        assert_eq!(fixed.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert!(fixed.exists());
        assert!(!wrong_ext.exists());
    }
}
