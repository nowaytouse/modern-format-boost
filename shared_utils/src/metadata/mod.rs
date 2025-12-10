//! Metadata Preservation Module
//! 
//! Complete metadata preservation across all layers:
//! - Internal: EXIF/IPTC/XMP via ExifTool
//! - Network: WhereFroms, User Tags
//! - System: ACL, Flags, Xattr, Timestamps
//!
//! Performance optimizations:
//! - macOS: copyfile() first (fast), then exiftool for internal metadata
//! - Cached tool availability checks
//! - Parallel-safe with OnceLock
//!
//! 🔥 关键：时间戳必须在最后设置！
//! exiftool 的 -overwrite_original 会修改文件，从而更新时间戳。
//! 因此 filetime::set_file_times() 必须在所有操作完成后执行。

use std::path::Path;
use std::io;

mod exif;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;
mod network;

pub use exif::preserve_internal_metadata;

/// Nuclear Preservation: The Ultimate Metadata Strategy
/// 
/// Performance: ~100-300ms per file on macOS (copyfile + exiftool)
/// 
/// 🔥 质量宣言说明：元数据保留失败时打印警告但继续是合理的，因为：
/// 1. 元数据丢失不应阻止文件转换（核心功能）
/// 2. 用户会看到警告消息，知道发生了什么
/// 3. 某些格式（如 MP4）可能不支持某些元数据类型
/// 4. 这是"尽力而为"的策略，而非"全有或全无"
/// 
/// 🔥 重要：不复制 COPYFILE_DATA (1<<3)！那会复制文件内容，导致转换无效！
/// 🔥 关键：时间戳在最后设置，因为 exiftool 会修改文件时间戳！
pub fn preserve_pro(src: &Path, dst: &Path) -> io::Result<()> {
    // 🚀 Performance: macOS fast path - copyfile first (handles ACL, xattr, timestamps)
    #[cfg(target_os = "macos")]
    {
        // 🔥 先读取源文件时间戳，保存起来，最后再设置
        let src_times = std::fs::metadata(src).ok().map(|m| {
            (
                filetime::FileTime::from_last_access_time(&m),
                filetime::FileTime::from_last_modification_time(&m),
            )
        });
        
        // Step 1: System Layer (fast, ~5ms)
        // copyfile handles: ACL, XATTR (不依赖它的时间戳复制，因为不可靠)
        if let Err(e) = macos::copy_native_metadata(src, dst) {
            eprintln!("⚠️ [metadata] macOS native copy failed: {}", e);
        }
        
        // Step 2: 保存创建时间和Date Added，稍后设置
        // ⚠️ 不在这里设置！因为 exiftool 会覆盖文件，重置创建时间
        let src_created = std::fs::metadata(src).ok().and_then(|m| m.created().ok());
        let src_added = macos::get_added_time(src).ok();
        
        // Step 3: Internal Metadata via ExifTool (~100-200ms)
        // This handles EXIF, IPTC, XMP, ICC that copyfile doesn't touch
        // ⚠️ 注意：exiftool -overwrite_original 会修改文件，更新时间戳！
        if let Err(e) = exif::preserve_internal_metadata(src, dst) {
            eprintln!("⚠️ [metadata] Internal metadata failed: {}", e);
        }
        
        // Step 4: Network metadata verification (fast, ~1ms)
        let _ = network::verify_network_metadata(src, dst);
        
        // Step 5: 🔥 最后设置时间戳！这是关键！
        // 必须在 exiftool 之后执行，否则时间戳会被覆盖
        if let Some((atime, mtime)) = src_times {
            if let Err(e) = filetime::set_file_times(dst, atime, mtime) {
                eprintln!("⚠️ [metadata] Failed to set file times: {}", e);
            }
        }
        
        // Step 6: 🔥 macOS创建时间和Date Added（必须在最后！）
        // filetime::set_file_times 只设置 atime/mtime，不设置创建时间
        // 必须使用 setattrlist 单独设置创建时间
        if let Some(created) = src_created {
            if let Err(e) = macos::set_creation_time(dst, created) {
                eprintln!("⚠️ [metadata] Failed to set creation time: {}", e);
            }
        }
        if let Some(added) = src_added {
            if let Err(e) = macos::set_added_time(dst, added) {
                eprintln!("⚠️ [metadata] Failed to set added time: {}", e);
            }
        }
        
        return Ok(());
    }

    // Non-macOS path (Linux/Windows)
    #[cfg(not(target_os = "macos"))]
    {
        // 🔥 先读取源文件时间戳，保存起来，最后再设置
        let src_times = std::fs::metadata(src).ok().map(|m| {
            (
                filetime::FileTime::from_last_access_time(&m),
                filetime::FileTime::from_last_modification_time(&m),
            )
        });
        
        // Step 1: Internal Metadata (Exif, MakerNotes, ICC)
        // ⚠️ 注意：exiftool -overwrite_original 会修改文件，更新时间戳！
        if let Err(e) = exif::preserve_internal_metadata(src, dst) {
            eprintln!("⚠️ [metadata] Internal metadata failed: {}", e);
        }

        // Step 2: Network & User Context (Verification)
        let _ = network::verify_network_metadata(src, dst);

        // Step 3: Platform-specific
        #[cfg(target_os = "linux")]
        { let _ = linux::preserve_linux_attributes(src, dst); }

        #[cfg(target_os = "windows")]
        { let _ = windows::preserve_windows_attributes(src, dst); }

        // Step 4: xattrs + permissions
        copy_xattrs_manual(src, dst);

        if let Ok(metadata) = std::fs::metadata(src) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode();
                let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode));
            }
        }
        
        // Step 5: 🔥 最后设置时间戳！这是关键！
        // 必须在 exiftool 之后执行，否则时间戳会被覆盖
        if let Some((atime, mtime)) = src_times {
            let _ = filetime::set_file_times(dst, atime, mtime);
        }
        
        Ok(())
    }
}

/// Alias for preserve_pro
pub fn preserve_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    preserve_pro(src, dst)
}

#[cfg(not(target_os = "macos"))]
fn copy_xattrs_manual(src: &Path, dst: &Path) {
    if let Ok(iter) = xattr::list(src) {
        for name in iter {
            if let Some(name_str) = name.to_str() {
                if let Ok(Some(value)) = xattr::get(src, name_str) {
                    let _ = xattr::set(dst, name_str, &value);
                }
            }
        }
    }
}
