//! ExifTool wrapper for internal metadata preservation
//!
//! Performance optimizations:
//! - Cached exiftool availability check
//! - Minimal argument set for common cases
//! - Fast path for same-format conversions
//!
//! 🔥 视频元数据特殊处理：
//! - QuickTime Create Date / Modify Date 需要从源文件日期推断
//! - GIF/PNG 等图像格式转视频时，源文件没有 QuickTime 元数据
//! - 需要从 XMP:DateCreated 或文件修改时间设置 QuickTime 日期

use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// Cached exiftool availability (checked once per process)
static EXIFTOOL_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check if exiftool is available (cached)
fn is_exiftool_available() -> bool {
    *EXIFTOOL_AVAILABLE.get_or_init(|| which::which("exiftool").is_ok())
}

/// 视频文件扩展名
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "avi"];

/// 检查是否是视频文件
fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// 从源文件获取最佳日期（用于设置 QuickTime 日期）
/// 优先级：XMP:DateCreated > EXIF:DateTimeOriginal > File Modification Date
fn get_best_date_from_source(src: &Path) -> Option<String> {
    let output = Command::new("exiftool")
        .arg("-s3") // 只输出值
        .arg("-d")
        .arg("%Y:%m:%d %H:%M:%S") // 日期格式
        .arg("-XMP-photoshop:DateCreated")
        .arg("-XMP-xmp:CreateDate")
        .arg("-EXIF:DateTimeOriginal")
        .arg("-EXIF:CreateDate")
        .arg(src)
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // 返回第一个非空日期
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.contains("0000:00:00") {
            return Some(trimmed.to_string());
        }
    }

    // 如果没有内部日期，使用文件修改时间
    if let Ok(metadata) = std::fs::metadata(src) {
        if let Ok(mtime) = metadata.modified() {
            let datetime: chrono::DateTime<chrono::Local> = mtime.into();
            return Some(datetime.format("%Y:%m:%d %H:%M:%S").to_string());
        }
    }

    None
}

/// Extract suggested extension from ExifTool error message
/// Example: "Error: Not a valid JPEG (looks more like a PNG)" -> Some("png")
fn extract_suggested_extension(error_msg: &str) -> Option<String> {
    if let Some(start) = error_msg.find("looks more like a ") {
        let rest = &error_msg[start + "looks more like a ".len()..];
        if let Some(end) = rest.find(')') {
             return Some(rest[..end].trim().to_lowercase());
        }
    }
    None
}

/// Preserve internal metadata via ExifTool
///
/// Performance: ~50-200ms per file depending on metadata complexity
///
/// 🔥 视频文件特殊处理：
/// - 复制所有元数据后，检查 QuickTime 日期是否为空
/// - 如果为空，从源文件的 XMP/EXIF 日期或文件修改时间设置
pub fn preserve_internal_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    match preserve_internal_metadata_core(src, dst) {
        Ok(_) => Ok(()),
        Err(e) => {
            // Check for content/extension mismatch
            // Error typically looks like "Error: Not a valid JPEG (looks more like a MOV)"
            let err_str = e.to_string();
            if err_str.contains("Not a valid") || err_str.contains("looks more like") {
                eprintln!("⚠️ Metadata preservation failed: {}", err_str);
                eprintln!("⚠️ Attempting content-aware fallback...");
                
                let hint = extract_suggested_extension(&err_str);
                if let Some(ref h) = hint {
                     eprintln!("💡 ExifTool suggests content is: {}", h);
                }

                match preserve_internal_metadata_fallback(src, dst, hint.as_deref()) {
                    Ok(_) => {
                        eprintln!("✅ Metadata fallback successful for {}", dst.display());
                        return Ok(());
                    }
                    Err(fallback_err) => {
                        eprintln!("❌ Metadata fallback failed: {}", fallback_err);
                        // Return original error as it is likely more descriptive for the user
                    }
                }
            }
            // Return original error if fallback fails or isn't applicable
            Err(e)
        }
    }
}

/// Fallback strategy: Rename file to match its content and retry
fn preserve_internal_metadata_fallback(src: &Path, dst: &Path, hint_ext: Option<&str>) -> io::Result<()> {
    // 1. Detect real extension
    // Priority: Hint > Detection
    let detected_ext = if let Some(hint) = hint_ext {
        hint.to_string()
    } else {
        crate::common_utils::detect_real_extension(dst)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Cannot detect file content"))?
            .to_string()
    };
    
    let current_ext = crate::common_utils::get_extension_lowercase(dst);
    
    // If extensions match, fallback is useless
    if detected_ext.eq_ignore_ascii_case(&current_ext) {
        return Err(io::Error::other(format!("Extension matches content ({}), fallback skipped", detected_ext)));
    }

    eprintln!("⚠️ Temporary rename to .{} for metadata preservation...", detected_ext);

    // 2. Temporary rename
    let temp_path = dst.with_extension(&detected_ext);
    if temp_path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("Temporary fallback path exists: {}", temp_path.display())));
    }

    std::fs::rename(dst, &temp_path)?;

    // 3. Retry operation (use scope guard pattern logic)
    let result = preserve_internal_metadata_core(src, &temp_path);

    // 4. Restore filename (Critical!)
    if let Err(e) = std::fs::rename(&temp_path, dst) {
        eprintln!("❌ CRITICAL: Failed to restore filename from {} to {}", temp_path.display(), dst.display());
        return Err(e);
    }

    result
}

/// Core implementation of metadata preservation
fn preserve_internal_metadata_core(src: &Path, dst: &Path) -> io::Result<()> {
    if !is_exiftool_available() {
        // Only warn once per process
        static WARNED: OnceLock<()> = OnceLock::new();
        WARNED.get_or_init(|| {
            eprintln!("⚠️ [metadata] ExifTool not found. EXIF/IPTC will NOT be preserved.");
        });
        return Ok(());
    }

    // 🚀 Performance: Use minimal argument set
    // -all:all copies everything, individual date tags are redundant
    // 🚀 Performance: Use "Gold Standard" Rebuild (FAQ #20) ONLY when Apple Compatibility mode is active for JXL files.
    // This clears any existing corrupted/compressed block and rebuilds it cleanly, avoiding Brotli corruption.
    // If not in apple_compat mode, we fallback to 100% data preservation (no forced nuclear rebuild).
    let is_jxl = dst.extension().map_or(false, |ext| ext.eq_ignore_ascii_case("jxl"));
    let apple_compat = std::env::var("MODERN_FORMAT_BOOST_APPLE_COMPAT").is_ok();

    let mut output = Command::new("exiftool");
    if is_jxl && apple_compat {
        output
            .arg("-all=") // Nuclear clear (Standardizes format)
            .arg("-tagsfromfile")
            .arg("@") // Restore from self first
            .arg("-all:all")
            .arg("-unsafe")
            .arg("-icc_profile")
            .arg("-tagsfromfile")
            .arg(src) // Then copy from source
            .arg("-all:all")
            .arg("-unsafe")
            .arg("-icc_profile");
    } else {
        output
            .arg("-tagsfromfile")
            .arg(src) // Then copy from source
            .arg("-all:all")
            .arg("-ICC_Profile<ICC_Profile"); // Keep ICC explicit for safety
    }

    let output = output
        .arg("-use")
        .arg("MWG") // Metadata Working Group standard
        .arg("-api")
        .arg("LargeFileSupport=1")
        // 🔥 Remove -overwrite_original to ensure atomic safety during nuclear rebuild.
        // If the process is killed during writing, the original data won't be lost.
        // We will manually delete the _original backup if the command succeeds.
        .arg("-q") // Quiet mode
        .arg("-m") // Ignore minor errors (faster)
        .arg(dst)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Don't fail on minor warnings
        if !stderr.contains("Warning") {
            return Err(io::Error::other(format!("ExifTool failed: {}", stderr)));
        }
    }

    // 🔥 Clean up the backup file created by ExifTool after successful operation
    let mut backup_name = dst.file_name().unwrap_or_default().to_os_string();
    backup_name.push("_original");
    let backup_path = dst.with_file_name(backup_name);
    let _ = std::fs::remove_file(&backup_path);

    // 🔥 视频文件特殊处理：修复 QuickTime 日期
    if is_video_file(dst) {
        fix_quicktime_dates(src, dst)?;
    }

    Ok(())
}

/// 修复视频文件的 QuickTime 日期
///
/// 问题：FFmpeg 转换时会将 QuickTime Create Date 设置为 0000:00:00 00:00:00
/// 解决：从源文件的 XMP/EXIF 日期或文件修改时间设置
fn fix_quicktime_dates(src: &Path, dst: &Path) -> io::Result<()> {
    // 检查 QuickTime 日期是否为空
    let check_output = Command::new("exiftool")
        .arg("-s3")
        .arg("-QuickTime:CreateDate")
        .arg(dst)
        .output()?;

    let current_date = String::from_utf8_lossy(&check_output.stdout);
    let current_date = current_date.trim();

    // 如果日期已经有效，不需要修复
    if !current_date.is_empty() && !current_date.contains("0000:00:00") {
        return Ok(());
    }

    // 获取源文件的最佳日期
    let best_date = match get_best_date_from_source(src) {
        Some(date) => date,
        None => {
            eprintln!("⚠️ [metadata] Cannot determine date for QuickTime metadata");
            return Ok(());
        }
    };

    // 设置 QuickTime 日期
    let output = Command::new("exiftool")
        .arg(format!("-QuickTime:CreateDate={}", best_date))
        .arg(format!("-QuickTime:ModifyDate={}", best_date))
        .arg(format!("-QuickTime:TrackCreateDate={}", best_date))
        .arg(format!("-QuickTime:TrackModifyDate={}", best_date))
        .arg(format!("-QuickTime:MediaCreateDate={}", best_date))
        .arg(format!("-QuickTime:MediaModifyDate={}", best_date))
        .arg("-overwrite_original")
        .arg("-q")
        .arg("-m")
        .arg(dst)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("Warning") && !stderr.is_empty() {
            eprintln!("⚠️ [metadata] Failed to set QuickTime dates: {}", stderr);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_preserve_metadata_mismatch() {
        if !is_exiftool_available() {
            eprintln!("ExifTool not available, skipping test");
            return;
        }
        let temp = TempDir::new().unwrap();
        // Create a complex directory structure
        let complex_dir = temp.path().join("测试 dir/来源/小红书");
        fs::create_dir_all(&complex_dir).unwrap();

        // Create a real PNG
        let src_path = complex_dir.join("src_image.png");
        // 1x1 PNG data
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
            0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41,
            0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
            0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
            0x44, 0xAE, 0x42, 0x60, 0x82
        ];
        fs::write(&src_path, png_data).unwrap();
        
        // Create dst as PNG but named .jpeg
        let dst_path = complex_dir.join("dst_image.jpeg");
        fs::write(&dst_path, png_data).unwrap();
        
        // Run preserve
        let result = preserve_internal_metadata(&src_path, &dst_path);
        
        if let Err(e) = &result {
            println!("Test failed with error: {}", e);
        }
        assert!(result.is_ok(), "Metadata preservation failed for mismatched extension with complex path");
    }
}
