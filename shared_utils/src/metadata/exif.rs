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

use std::path::Path;
use std::process::Command;
use std::io;
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
        .arg("-s3")  // 只输出值
        .arg("-d").arg("%Y:%m:%d %H:%M:%S")  // 日期格式
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

/// Preserve internal metadata via ExifTool
/// 
/// Performance: ~50-200ms per file depending on metadata complexity
/// 
/// 🔥 视频文件特殊处理：
/// - 复制所有元数据后，检查 QuickTime 日期是否为空
/// - 如果为空，从源文件的 XMP/EXIF 日期或文件修改时间设置
pub fn preserve_internal_metadata(src: &Path, dst: &Path) -> io::Result<()> {
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
    let output = Command::new("exiftool")
        .arg("-tagsfromfile").arg(src)
        .arg("-all:all")              // Copy all metadata
        .arg("-ICC_Profile<ICC_Profile") // Ensure ICC is copied
        .arg("-use").arg("MWG")       // Metadata Working Group standard
        .arg("-api").arg("LargeFileSupport=1")
        .arg("-overwrite_original")   // Don't create backup
        .arg("-q")                    // Quiet mode
        .arg("-m")                    // Ignore minor errors (faster)
        .arg(dst)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Don't fail on minor warnings
        if !stderr.contains("Warning") {
            return Err(io::Error::new(io::ErrorKind::Other, format!("ExifTool failed: {}", stderr)));
        }
    }
    
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
