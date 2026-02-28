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

static EXIFTOOL_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn is_exiftool_available() -> bool {
    *EXIFTOOL_AVAILABLE.get_or_init(|| which::which("exiftool").is_ok())
}

fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| crate::SUPPORTED_VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn get_best_date_from_source(src: &Path) -> Option<String> {
    let output = Command::new("exiftool")
        .arg("-s3")
        .arg("-d")
        .arg("%Y:%m:%d %H:%M:%S")
        .arg("-XMP-photoshop:DateCreated")
        .arg("-XMP-xmp:CreateDate")
        .arg("-EXIF:DateTimeOriginal")
        .arg("-EXIF:CreateDate")
        .arg(crate::safe_path_arg(src).as_ref())
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.contains("0000:00:00") {
            return Some(trimmed.to_string());
        }
    }

    if let Ok(metadata) = std::fs::metadata(src) {
        if let Ok(mtime) = metadata.modified() {
            let datetime: chrono::DateTime<chrono::Local> = mtime.into();
            return Some(datetime.format("%Y:%m:%d %H:%M:%S").to_string());
        }
    }

    None
}

pub fn preserve_internal_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    match preserve_internal_metadata_core(src, dst) {
        Ok(_) => Ok(()),
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("Not a valid") || err_str.contains("looks more like") {
                eprintln!("⚠️ Metadata preservation failed: {}", err_str);
                eprintln!("⚠️ Attempting content-aware fallback...");

                let hint = crate::extract_suggested_extension(&err_str);
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
                    }
                }
            }
            Err(e)
        }
    }
}

fn preserve_internal_metadata_fallback(
    src: &Path,
    dst: &Path,
    hint_ext: Option<&str>,
) -> io::Result<()> {
    let detected_ext = if let Some(hint) = hint_ext {
        hint.to_string()
    } else {
        crate::common_utils::detect_real_extension(dst)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Cannot detect file content")
            })?
            .to_string()
    };

    let current_ext = crate::common_utils::get_extension_lowercase(dst);

    if detected_ext.eq_ignore_ascii_case(&current_ext) {
        return Err(io::Error::other(format!(
            "Extension matches content ({}), fallback skipped",
            detected_ext
        )));
    }

    eprintln!(
        "⚠️ Temporary rename to .{} for metadata preservation...",
        detected_ext
    );

    let temp_path = dst.with_extension(&detected_ext);
    if temp_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("Temporary fallback path exists: {}", temp_path.display()),
        ));
    }

    std::fs::rename(dst, &temp_path)?;

    let result = preserve_internal_metadata_core(src, &temp_path);

    if let Err(e) = std::fs::rename(&temp_path, dst) {
        eprintln!(
            "❌ CRITICAL: Failed to restore filename from {} to {}: {}",
            temp_path.display(),
            dst.display(),
            e
        );
        if temp_path.exists() && !dst.exists() {
            eprintln!("   🔧 Attempting emergency recovery via copy...");
            if let Ok(()) = std::fs::copy(&temp_path, dst).map(|_| ()) {
                let _ = std::fs::remove_file(&temp_path);
                eprintln!("   ✅ Emergency recovery succeeded");
            } else {
                eprintln!(
                    "   ❌ Emergency recovery FAILED. File stranded at: {}",
                    temp_path.display()
                );
            }
        }
        return Err(e);
    }

    result
}

fn preserve_internal_metadata_core(src: &Path, dst: &Path) -> io::Result<()> {
    if !is_exiftool_available() {
        static WARNED: OnceLock<()> = OnceLock::new();
        WARNED.get_or_init(|| {
            eprintln!("⚠️ [metadata] ExifTool not found. EXIF/IPTC will NOT be preserved.");
        });
        return Ok(());
    }

    // ExifTool writes to <path>_exiftool_tmp then renames; remove leftover from prior run.
    if let Some(name) = dst.file_name() {
        let tmp_path = dst.with_file_name(format!("{}_exiftool_tmp", name.to_string_lossy()));
        let _ = std::fs::remove_file(&tmp_path);
    }

    let ext = dst
        .extension()
        .map_or(String::new(), |e| e.to_string_lossy().to_lowercase());
    let is_nuclear_format = ext == "jxl" || ext == "jpg" || ext == "jpeg" || ext == "webp";
    let apple_compat = std::env::var("MODERN_FORMAT_BOOST_APPLE_COMPAT").is_ok();

    let mut output = Command::new("exiftool")
        .arg("-tagsfromfile")
        .arg(crate::safe_path_arg(src).as_ref())
        .arg("-all:all")
        .arg("-unsafe")
        .arg("-ICC_Profile<ICC_Profile")
        .arg("-use")
        .arg("MWG")
        .arg("-api")
        .arg("LargeFileSupport=1")
        .arg("-q")
        .arg("-m")
        .arg(crate::safe_path_arg(dst).as_ref())
        .output()?;

    let needs_repair = apple_compat && is_nuclear_format && {
        if output.status.success() {
            false
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let is_corrupt = stderr.contains("Error")
                || stderr.contains("corrupt")
                || stderr.contains("invalid")
                || stderr.contains("truncated")
                || stderr.contains("Not a valid");

            if is_corrupt {
                eprintln!(
                    "⚠️  [Structural Repair] {} detected metadata corruption：{}",
                    dst.display(),
                    stderr.lines().next().unwrap_or("unknown error")
                );
            }

            is_corrupt
        }
    };

    if needs_repair {
        eprintln!("🔧  [Structural Repair] executing ImageMagick rebuild...");

        let magick_result = Command::new("magick")
            .arg("--")
            .arg(crate::safe_path_arg(dst).as_ref())
            .arg(crate::safe_path_arg(dst).as_ref())
            .output();

        match magick_result {
            Ok(out) => {
                if out.status.success() {
                    eprintln!("✅  [Structural Repair] Complete：{}", dst.display());

                    output = Command::new("exiftool")
                        .arg("-all=")
                        .arg("-tagsfromfile")
                        .arg("@")
                        .arg("-all:all")
                        .arg("-unsafe")
                        .arg("-icc_profile")
                        .arg("-tagsfromfile")
                        .arg(crate::safe_path_arg(src).as_ref())
                        .arg("-all:all")
                        .arg("-unsafe")
                        .arg("-icc_profile")
                        .arg("-use")
                        .arg("MWG")
                        .arg("-api")
                        .arg("LargeFileSupport=1")
                        .arg("-q")
                        .arg("-m")
                        .arg(crate::safe_path_arg(dst).as_ref())
                        .output()?;
                } else {
                    eprintln!(
                        "⚠️  [Structural Repair] magick failed：{}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stderr.contains("Warning") {
                        return Err(io::Error::other(format!("ExifTool failed: {}", stderr)));
                    }
                }
            }
            Err(e) => {
                eprintln!("⚠️  [Structural Repair] magick unavailable：{}", e);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.contains("Warning") {
                    return Err(io::Error::other(format!("ExifTool failed: {}", stderr)));
                }
            }
        }
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("Warning") {
            return Err(io::Error::other(format!("ExifTool failed: {}", stderr)));
        }
    }

    let mut backup_name = dst.file_name().unwrap_or_default().to_os_string();
    backup_name.push("_original");
    let backup_path = dst.with_file_name(backup_name);
    let _ = std::fs::remove_file(&backup_path);

    if is_video_file(dst) {
        fix_quicktime_dates(src, dst)?;
    }

    Ok(())
}

fn fix_quicktime_dates(src: &Path, dst: &Path) -> io::Result<()> {
    let check_output = Command::new("exiftool")
        .arg("-s3")
        .arg("-QuickTime:CreateDate")
        .arg(crate::safe_path_arg(dst).as_ref())
        .output()?;

    let current_date = String::from_utf8_lossy(&check_output.stdout);
    let current_date = current_date.trim();

    if !current_date.is_empty() && !current_date.contains("0000:00:00") {
        return Ok(());
    }

    let best_date = match get_best_date_from_source(src) {
        Some(date) => date,
        None => {
            eprintln!("⚠️ [metadata] Cannot determine date for QuickTime metadata");
            return Ok(());
        }
    };

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
        .arg(crate::safe_path_arg(dst).as_ref())
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
        let complex_dir = temp.path().join("测试 dir/来源/小红书");
        fs::create_dir_all(&complex_dir).unwrap();

        let src_path = complex_dir.join("src_image.png");
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        fs::write(&src_path, png_data).unwrap();

        let dst_path = complex_dir.join("dst_image.jpeg");
        fs::write(&dst_path, png_data).unwrap();

        let result = preserve_internal_metadata(&src_path, &dst_path);

        if let Err(e) = &result {
            println!("Test failed with error: {}", e);
        }
        assert!(
            result.is_ok(),
            "Metadata preservation failed for mismatched extension with complex path"
        );
    }
}
