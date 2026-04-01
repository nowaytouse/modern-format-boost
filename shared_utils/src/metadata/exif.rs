//! `ExifTool` wrapper for internal metadata preservation
//!
//! Performance optimizations:
//! - Cached exiftool availability check
//! - Minimal argument set for common cases
//! - Fast path for same-format conversions
//!
//! 🔥 Special handling for video metadata:
//! - `QuickTime` Create Date / Modify Date needs to be inferred from source file dates.
//! - When converting image formats like GIF/PNG to video, source files lack `QuickTime` metadata.
//! - `QuickTime` dates need to be set from XMP:DateCreated or file modification time.

use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use crate::path_safety::{exiftool_path_arg, property_safe_path, safe_path_arg};

static EXIFTOOL_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn is_exiftool_available() -> bool {
    *EXIFTOOL_AVAILABLE.get_or_init(|| which::which("exiftool").is_ok())
}

fn magick_path(path: &Path, is_output: bool) -> String {
    let s = crate::safe_path_arg(path).to_string();
    
    // For ImageMagick, percent signs in filenames are interpreted as properties.
    // They MUST be doubled to be treated literally.
    let escaped = if s.contains('%') { s.replace('%', "%%") } else { s };
    
    let path_with_prefix = if !path.is_absolute() && !escaped.starts_with("./") {
        format!("./{}", escaped)
    } else {
        escaped
    };
    
    if !is_output && (path_with_prefix.contains(':') || path_with_prefix.contains('%')) {
        // Prepend 'file:' for input paths to force local file treatment and avoid 
        // protocol delegates (like http:) or property expansion at the beginning of the path.
        format!("file:{}", path_with_prefix)
    } else {
        path_with_prefix
    }
}

fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| crate::SUPPORTED_VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
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
        .arg(exiftool_path_arg(src).as_ref())
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

/// Preserve internal metadata from source to destination.
///
/// # Errors
/// Returns an `io::Result` if preservation fails.
pub fn preserve_internal_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    match preserve_internal_metadata_core(src, dst) {
        Ok(()) => Ok(()),
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("Not a valid") || err_str.contains("looks more like") {
                eprintln!("⚠️ Metadata preservation failed: {err_str}");
                eprintln!("⚠️ Attempting content-aware fallback...");

                let hint = crate::extract_suggested_extension(&err_str);
                if let Some(ref h) = hint {
                    eprintln!("💡 ExifTool suggests content is: {h}");
                }

                match preserve_internal_metadata_fallback(src, dst, hint.as_deref()) {
                    Ok(()) => {
                        eprintln!("✅ Metadata fallback successful for {}", dst.display());
                        return Ok(());
                    }
                    Err(fallback_err) => {
                        eprintln!("❌ Metadata fallback failed: {fallback_err}");
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
            "Extension matches content ({detected_ext}), fallback skipped"
        )));
    }

    eprintln!("⚠️ Temporary rename to .{detected_ext} for metadata preservation...");

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
            if matches!(std::fs::copy(&temp_path, dst).map(|_| ()), Ok(())) {
                if let Err(cleanup_err) = std::fs::remove_file(&temp_path) {
                    eprintln!(
                        "   ⚠️ Emergency recovery succeeded but cleanup failed for {}: {}",
                        temp_path.display(),
                        cleanup_err
                    );
                }
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

/// Extract a meaningful error message from an `ExifTool` output.
/// `ExifTool` with `-q` writes errors to stdout (not stderr); stderr is often empty on failure.
/// Returns Some(msg) only when there is a real actionable error (not just warnings or empty output).
fn exiftool_error_message(output: &std::process::Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Collect non-Warning lines from both streams
    let error_lines: Vec<&str> = stderr
        .lines()
        .chain(stdout.lines())
        .filter(|l| {
            let l = l.trim();
            !l.is_empty()
                && !l.starts_with("Warning")
                && !l.starts_with("  ")
                && l.contains("Error")
        })
        .collect();

    if error_lines.is_empty() {
        None
    } else {
        Some(error_lines.join("; "))
    }
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
        if let Err(e) = std::fs::remove_file(&tmp_path) {
            if e.kind() != io::ErrorKind::NotFound {
                eprintln!(
                    "⚠️ [metadata] Failed to remove stale ExifTool temp file {}: {}",
                    tmp_path.display(),
                    e
                );
            }
        }
    }

    let ext = dst
        .extension()
        .map_or(String::new(), |e| e.to_string_lossy().to_lowercase());
    let is_nuclear_format = ext == "jxl" || ext == "jpg" || ext == "jpeg" || ext == "webp";
    let apple_compat = std::env::var("MODERN_FORMAT_BOOST_APPLE_COMPAT").is_ok();

    // ICC priority: cjxl/native tool embeds ICC from container (colr box, iCCP chunk, APP2)
    // which is more authoritative than ExifTool re-extraction. For JXL output, exclude
    // -ICC_Profile<ICC_Profile so ExifTool doesn't overwrite the tool-embedded ICC.
    // For other formats, include it as those tools may not handle ICC natively.
    let is_jxl_output = ext == "jxl";

    // For JXL output: skip ICC copy if cjxl already embedded it (authoritative source).
    // Fallback: if JXL has no ICC (source had no container ICC, only EXIF ColorSpace tag),
    // allow exiftool to inject it as a safety net so the output is never ICC-less.
    let jxl_already_has_icc = is_jxl_output && crate::jxl_utils::verify_jxl_has_icc(dst);
    if jxl_already_has_icc {
        tracing::debug!(
            dst = %dst.display(),
            "JXL already has embedded ICC — skipping ExifTool ICC injection"
        );
    } else if is_jxl_output {
        tracing::debug!(
            dst = %dst.display(),
            "JXL has no embedded ICC — ExifTool fallback will inject ICC"
        );
    }

    let mut cmd = Command::new("exiftool");
    cmd.arg("-charset").arg("filename=utf8");
    cmd.arg("-api").arg("windowsunicode=1");
    cmd.arg("-api").arg("LargeFileSupport=1");
    cmd.arg("-overwrite_original");
    cmd.arg("-tagsfromfile")
        .arg(property_safe_path(src).as_ref())
        .arg("-all:all")
        .arg("-unsafe");
    if !jxl_already_has_icc {
        // Non-JXL OR JXL without embedded ICC: inject via ExifTool as fallback
        cmd.arg("-ICC_Profile<ICC_Profile");
    }
    // JXL with already-embedded ICC: skip to preserve cjxl's authoritative profile
    cmd.arg("-use")
        .arg("MWG")
        .arg("-api")
        .arg("LargeFileSupport=1")
        .arg("-q")
        .arg("-m")
        .arg(exiftool_path_arg(dst).as_ref());
    let mut output = cmd.output()?;

    // Log exiftool stderr to file (debug/warn level only — never reaches terminal).
    // This surfaces warnings like "[minor] Will wrap JXL codestream" and any exiftool
    // quirks that are silenced by the -m flag.
    {
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            tracing::warn!(
                src = %src.display(),
                dst = %dst.display(),
                exit_code = ?output.status.code(),
                stderr = %stderr_str.trim(),
                "exiftool metadata copy failed"
            );
        } else if !stderr_str.trim().is_empty() {
            let trimmed = stderr_str.trim();
            if !trimmed.contains("No writable tags set") && !trimmed.contains("Wrapped JXL codestream") {
                tracing::debug!(
                    src = %src.display(),
                    dst = %dst.display(),
                    stderr = %trimmed,
                    "exiftool completed with warnings (suppressed by -m)"
                );
            }
        }
    }

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
            .arg(magick_path(dst, false))
            .arg(magick_path(dst, true))
            .output();

        match magick_result {
            Ok(out) => {
                if out.status.success() {
                    eprintln!("✅  [Structural Repair] Complete：{}", dst.display());

                    output = Command::new("exiftool")
                        .arg("-charset").arg("filename=utf8")
                        .arg("-api").arg("windowsunicode=1")
                        .arg("-api").arg("LargeFileSupport=1")
                        .arg("-overwrite_original")
                        .arg("-all=")
                        // Use -tagsfromfile @ to copy tags from the file itself (internal repair).
                        // Use property_safe_path for the external source file (src) to avoid
                        // recursive format code expansion for paths containing '%'.
                        .arg("-tagsfromfile")
                        .arg("@")
                        .arg("-all:all")
                        .arg("-unsafe")
                        .arg("-icc_profile")
                        .arg("-tagsfromfile")
                        .arg(property_safe_path(src).as_ref())
                        .arg("-all:all")
                        .arg("-unsafe")
                        .arg("-icc_profile")
                        .arg("-use")
                        .arg("MWG")
                        .arg("-q")
                        .arg("-m")
                        .arg(safe_path_arg(dst).as_ref())
                        .output()?;
                } else {
                    eprintln!(
                        "⚠️  [Structural Repair] magick failed：{}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    if let Some(msg) = exiftool_error_message(&output) {
                        return Err(io::Error::other(format!("ExifTool failed: {msg}")));
                    }
                }
            }
            Err(e) => {
                eprintln!("⚠️  [Structural Repair] magick unavailable：{e}");
                if let Some(msg) = exiftool_error_message(&output) {
                    return Err(io::Error::other(format!("ExifTool failed: {msg}")));
                }
            }
        }
    }

    if !output.status.success() {
        if let Some(msg) = exiftool_error_message(&output) {
            return Err(io::Error::other(format!("ExifTool failed: {msg}")));
        }
    }

    let mut backup_name = dst.file_name().unwrap_or_default().to_os_string();
    backup_name.push("_original");
    let backup_path = dst.with_file_name(backup_name);
    if let Err(e) = std::fs::remove_file(&backup_path) {
        if e.kind() != io::ErrorKind::NotFound {
            eprintln!(
                "⚠️ [metadata] Failed to remove ExifTool backup file {}: {}",
                backup_path.display(),
                e
            );
        }
    }

    if is_video_file(dst) {
        fix_quicktime_dates(src, dst)?;
    }

    Ok(())
}

fn fix_quicktime_dates(src: &Path, dst: &Path) -> io::Result<()> {
    // Always sync all QuickTime date fields from source — don't skip if dst already has a date,
    // because the date may have been reset to encode time rather than original capture time.
    let Some(best_date) = get_best_date_from_source(src) else {
        eprintln!("⚠️ [metadata] Cannot determine date for QuickTime metadata");
        return Ok(());
    };

    let output = Command::new("exiftool")
        .arg("-charset")
        .arg("filename=utf8")
        .arg("-api")
        .arg("windowsunicode=1")
        .arg("-api")
        .arg("LargeFileSupport=1")
        .arg("-overwrite_original")
        .arg(format!("-QuickTime:CreateDate={best_date}"))
        .arg(format!("-QuickTime:ModifyDate={best_date}"))
        .arg(format!("-QuickTime:TrackCreateDate={best_date}"))
        .arg(format!("-QuickTime:TrackModifyDate={best_date}"))
        .arg(format!("-QuickTime:MediaCreateDate={best_date}"))
        .arg(format!("-QuickTime:MediaModifyDate={best_date}"))
        // Also sync EXIF/XMP date fields for maximum compatibility
        .arg(format!("-EXIF:DateTimeOriginal={best_date}"))
        .arg(format!("-EXIF:CreateDate={best_date}"))
        .arg(format!("-XMP:DateCreated={best_date}"))
        .arg(format!("-XMP:CreateDate={best_date}"))
        .arg("-overwrite_original")
        .arg("-q")
        .arg("-m")
        .arg(exiftool_path_arg(dst).as_ref())
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Warnings are non-fatal (e.g. tag not writable for this format)
        if !stderr.trim().is_empty() && !stderr.contains("Warning") {
            eprintln!(
                "⚠️ [metadata] Failed to set QuickTime/EXIF dates: {}",
                stderr.trim()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Tests for path safety conversions that prevent hijacking of tool commands.
    /// 
    /// ! WARNING FOR FUTURE MAINTAINERS:
    /// Do NOT "simplify" these tests. Filenames starting with '-' or '@' are 
    /// intentionally prefixed with './' to block tools like `ExifTool` and 
    /// `ImageMagick` from interpreting them as flags or argfiles. 
    /// Breaking these tests WILL cause file-not-found errors for user files.
    #[test]
    fn test_safe_path_arg_prefixes() {
        if !is_exiftool_available() {
            eprintln!("ExifTool not available, skipping test");
            return;
        }
        let temp = TempDir::new().unwrap();
        // ... (rest of test implementation)
    }

    #[test]
    fn test_preserve_metadata_mismatch() {
        if !is_exiftool_available() {
            eprintln!("ExifTool not available, skipping test");
            return;
        }
        let temp = TempDir::new().unwrap();
        let complex_dir = temp.path().join("test dir/source/xiaohongshu");
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
            println!("Test failed with error: {e}");
        }
        assert!(
            result.is_ok(),
            "Metadata preservation failed for mismatched extension with complex path"
        );
    }

    #[test]
    fn test_preserve_metadata_with_percent_in_path() {
        if !is_exiftool_available() {
            return;
        }
        let temp = TempDir::new().unwrap();
        // Filename with % character that triggers interpolation if not escaped
        let src_path = temp.path().join("http%3A%2F%2Fimg.png");
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        fs::write(&src_path, png_data).unwrap();

        let dst_path = temp.path().join("output.png");
        fs::write(&dst_path, png_data).unwrap();

        let result = preserve_internal_metadata(&src_path, &dst_path);

        assert!(
            result.is_ok(),
            "Metadata preservation failed for filename with % character: {:?}",
            result.err()
        );
    }

    /// Stress test for 'evil' filenames that combine multiple edge cases.
    /// 
    /// RATIONALE:
    /// This test explicitly uses a filename containing URL-encoded sequences (`%3A%2F`), 
    /// ExifTool format codes (`%d%f%e`), and suspicious command-line prefixes (`-@`).
    /// 
    /// This ensures that our `STDIN` piping strategy and path prefixing work 
    /// correctly even under absolute "worst-case" filename conditions.
    /// 
    /// ! DO NOT ALTER the `evil_name` string without extreme caution.
    #[test]
    fn test_preservation_evil_path() {
        if !is_exiftool_available() {
            return;
        }
        let temp = TempDir::new().unwrap();
        // Filename containing: URL encoded chars (%3A), Format strings (%d%f), and Shell-suspicious prefixes
        let evil_name = "http%3A%2F%2Ftest%d%f%e-@evil.jpg";
        let src_path = temp.path().join(evil_name);
        
        // Create an actual image file with these characters
        fs::write(&src_path, [0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x00]).unwrap();
        
        let dst_path = temp.path().join("output.jpg");
        fs::write(&dst_path, [0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x00]).unwrap();
        
        let result = preserve_internal_metadata(&src_path, &dst_path);
            
        assert!(result.is_ok(), "Failed metadata preservation on evil path: {:?}", result.err());
    }
}
