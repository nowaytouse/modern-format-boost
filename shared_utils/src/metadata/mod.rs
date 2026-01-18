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
        
        Ok(())
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

/// 🔥 v4.8: 便捷函数 - 复制元数据（静默错误）
/// 🔥 v5.76: 自动合并XMP边车文件
/// 
/// 与 preserve_metadata 相同，但错误时只打印警告而不返回 Result。
/// 这是各个工具中 copy_metadata 函数的统一实现。
/// 
/// 自动检测并合并XMP边车文件：
/// - photo.jpg.xmp → 合并到输出文件
/// - photo.xmp → 合并到输出文件
pub fn copy_metadata(src: &Path, dst: &Path) {
    // Step 1: 复制源文件的内部元数据
    if let Err(e) = preserve_metadata(src, dst) {
        eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }
    
    // Step 2: 🔥 自动合并XMP边车文件
    merge_xmp_sidecar(src, dst);
}

/// 🔥 v7.4: 保留文件夹元数据（时间戳、权限）
/// 
/// 递归保留整个目录树的元数据：
/// - 时间戳（创建、修改、访问）
/// - 权限（Unix mode）
/// - 扩展属性（xattr）
/// 
/// 用于相邻目录输出模式，确保输出目录结构与源目录完全一致。
pub fn preserve_directory_metadata(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    use std::collections::HashMap;
    
    // Step 1: 收集源目录树的所有目录及其元数据
    let mut dir_metadata: HashMap<std::path::PathBuf, std::fs::Metadata> = HashMap::new();
    
    if src_dir.is_dir() {
        // 🔥 v7.4.9: 确保收集根目录元数据
        if let Ok(meta) = std::fs::metadata(src_dir) {
            dir_metadata.insert(src_dir.to_path_buf(), meta);
        }
        
        // 递归收集所有子目录
        collect_dir_metadata(src_dir, &mut dir_metadata)?;
    }
    
    // Step 2: 应用元数据到目标目录树
    for (src_path, metadata) in dir_metadata.iter() {
        // 计算相对路径
        let rel_path = src_path.strip_prefix(src_dir).unwrap_or(src_path);
        let dst_path = dst_dir.join(rel_path);
        
        // 🔥 v7.4.9: 如果目标目录不存在，创建它（保留结构）
        if !dst_path.exists() {
            if let Err(e) = std::fs::create_dir_all(&dst_path) {
                eprintln!("⚠️ Failed to create directory {}: {}", dst_path.display(), e);
                continue;
            }
        }
        
        // 复制权限
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            if let Err(e) = std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(mode)) {
                eprintln!("⚠️ Failed to set permissions for {}: {}", dst_path.display(), e);
            }
        }
        
        // 复制时间戳
        let atime = filetime::FileTime::from_last_access_time(metadata);
        let mtime = filetime::FileTime::from_last_modification_time(metadata);
        if let Err(e) = filetime::set_file_times(&dst_path, atime, mtime) {
            eprintln!("⚠️ Failed to set timestamps for {}: {}", dst_path.display(), e);
        }
        
        // macOS: 复制创建时间
        #[cfg(target_os = "macos")]
        {
            if let Ok(created) = metadata.created() {
                if let Err(e) = macos::set_creation_time(&dst_path, created) {
                    eprintln!("⚠️ Failed to set creation time for {}: {}", dst_path.display(), e);
                }
            }
        }
        
        // 复制扩展属性
        copy_dir_xattrs(src_path, &dst_path);
    }
    
    Ok(())
}

/// 递归收集目录树的元数据
fn collect_dir_metadata(
    dir: &Path,
    map: &mut std::collections::HashMap<std::path::PathBuf, std::fs::Metadata>,
) -> io::Result<()> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(meta) = std::fs::metadata(&path) {
                    map.insert(path.clone(), meta);
                }
                // 递归
                collect_dir_metadata(&path, map)?;
            }
        }
    }
    Ok(())
}

/// 复制目录的扩展属性
fn copy_dir_xattrs(src: &Path, dst: &Path) {
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

/// 🔥 v5.76: 自动合并XMP边车文件到输出文件
/// 
/// 检测源文件是否有对应的XMP边车文件，如果有则合并到输出文件。
/// 支持两种命名格式：
/// - photo.jpg.xmp (Adobe标准)
/// - photo.xmp (同名不同扩展名)
fn merge_xmp_sidecar(src: &Path, dst: &Path) {
    // 尝试找到XMP边车文件
    let xmp_path = find_xmp_sidecar(src);
    
    if let Some(xmp) = xmp_path {
        eprintln!("📋 Found XMP sidecar: {}", xmp.display());
        
        // 使用XmpMerger合并
        let config = crate::xmp_merger::XmpMergerConfig {
            delete_xmp_after_merge: false,  // 不删除XMP，让用户决定
            overwrite_original: true,
            preserve_timestamps: true,
            verbose: false,
        };
        
        let merger = crate::xmp_merger::XmpMerger::new(config);
        
        match merger.merge_xmp(&xmp, dst) {
            Ok(()) => {
                eprintln!("✅ XMP sidecar merged successfully");
            }
            Err(e) => {
                eprintln!("⚠️ Failed to merge XMP sidecar: {}", e);
            }
        }
    }
}

/// 查找源文件对应的XMP边车文件
fn find_xmp_sidecar(src: &Path) -> Option<std::path::PathBuf> {
    // 策略1: photo.jpg.xmp
    let xmp_full = src.with_extension(
        format!("{}.xmp", src.extension()?.to_str()?)
    );
    if xmp_full.exists() {
        return Some(xmp_full);
    }
    
    // 策略2: photo.xmp
    let xmp_stem = src.with_extension("xmp");
    if xmp_stem.exists() {
        return Some(xmp_stem);
    }
    
    // 策略3: 大小写不敏感 (photo.XMP, photo.Xmp)
    if let Some(parent) = src.parent() {
        if let Some(stem) = src.file_stem() {
            let stem_str = stem.to_string_lossy();
            
            // 扫描目录查找匹配的XMP文件
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if let Some(ext) = path.extension() {
                        if ext.to_string_lossy().to_lowercase() == "xmp" {
                            if let Some(file_stem) = path.file_stem() {
                                let file_stem_str = file_stem.to_string_lossy();
                                // photo.jpg.xmp 或 photo.xmp
                                if file_stem_str.to_lowercase() == stem_str.to_lowercase()
                                    || file_stem_str.to_lowercase() == format!("{}.{}", stem_str, src.extension()?.to_str()?).to_lowercase()
                                {
                                    return Some(path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    None
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
