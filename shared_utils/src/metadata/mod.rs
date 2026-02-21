//! Metadata Preservation Module
//!
//! 分层保留：Internal (ExifTool) / Network / System (ACL, xattr, timestamps)。
//! 时间戳统一入口：单文件经 `apply_file_timestamps(src, dst)`，目录树经
//! `save_directory_timestamps` → `apply_saved_timestamps_to_dst` / `restore_directory_timestamps`，
//! 避免多处重复实现。exiftool 会改写文件，故时间戳一律在写操作之后设置。

use std::io;
use std::path::Path;

mod exif;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod network;
#[cfg(target_os = "windows")]
mod windows;

pub use exif::preserve_internal_metadata;

/// 唯一入口：将源文件的时间戳（atime/mtime，macOS 下含创建时间与 Date Added）应用到目标文件。
/// 所有“按源文件恢复目标时间戳”的逻辑均经此函数，避免重复实现。
fn apply_file_timestamps(src: &Path, dst: &Path) {
    let Ok(m) = std::fs::metadata(src) else {
        return;
    };
    let atime = filetime::FileTime::from_last_access_time(&m);
    let mtime = filetime::FileTime::from_last_modification_time(&m);
    if let Err(e) = filetime::set_file_times(dst, atime, mtime) {
        eprintln!("⚠️ [metadata] Failed to set file times: {}", e);
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(created) = m.created() {
            let _ = macos::set_creation_time(dst, created);
        }
        if let Ok(added) = macos::get_added_time(src) {
            let _ = macos::set_added_time(dst, added);
        }
    }
}

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
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = macos::copy_native_metadata(src, dst) {
            eprintln!("⚠️ [metadata] macOS native copy failed: {}", e);
        }
        if let Err(e) = exif::preserve_internal_metadata(src, dst) {
            eprintln!("⚠️ [metadata] Internal metadata failed: {}", e);
        }
        let _ = network::verify_network_metadata(src, dst);
        apply_file_timestamps(src, dst);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Err(e) = exif::preserve_internal_metadata(src, dst) {
            eprintln!("⚠️ [metadata] Internal metadata failed: {}", e);
        }
        let _ = network::verify_network_metadata(src, dst);
        #[cfg(target_os = "linux")]
        let _ = linux::preserve_linux_attributes(src, dst);
        #[cfg(target_os = "windows")]
        let _ = windows::preserve_windows_attributes(src, dst);
        copy_xattrs_manual(src, dst);
        if let Ok(metadata) = std::fs::metadata(src) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode();
                let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode));
            }
        }
        apply_file_timestamps(src, dst);
        Ok(())
    }
}

/// Alias for preserve_pro
pub fn preserve_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    preserve_pro(src, dst)
}

/// 🔥 v4.8: 便捷函数 - 复制元数据（静默错误）
/// 🔥 v5.76: 自动合并XMP边车文件；时间戳统一经 apply_file_timestamps 在最后应用。
///
/// 流程：preserve_metadata → merge_xmp_sidecar → apply_file_timestamps（merge 会改文件，故时间戳最后再设）
pub fn copy_metadata(src: &Path, dst: &Path) {
    if let Err(e) = preserve_metadata(src, dst) {
        eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }
    merge_xmp_sidecar(src, dst);
    apply_file_timestamps(src, dst);
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
                eprintln!(
                    "⚠️ Failed to create directory {}: {}",
                    dst_path.display(),
                    e
                );
                continue;
            }
        }

        // 复制权限
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            if let Err(e) =
                std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(mode))
            {
                eprintln!(
                    "⚠️ Failed to set permissions for {}: {}",
                    dst_path.display(),
                    e
                );
            }
        }

        // 复制时间戳
        let atime = filetime::FileTime::from_last_access_time(metadata);
        let mtime = filetime::FileTime::from_last_modification_time(metadata);
        if let Err(e) = filetime::set_file_times(&dst_path, atime, mtime) {
            eprintln!(
                "⚠️ Failed to set timestamps for {}: {}",
                dst_path.display(),
                e
            );
        }

        // macOS: 复制创建时间
        #[cfg(target_os = "macos")]
        {
            if let Ok(created) = metadata.created() {
                if let Err(e) = macos::set_creation_time(&dst_path, created) {
                    eprintln!(
                        "⚠️ Failed to set creation time for {}: {}",
                        dst_path.display(),
                        e
                    );
                }
            }
        }

        // 复制扩展属性
        copy_dir_xattrs(src_path, &dst_path);
    }

    Ok(())
}

/// 薄封装：调用 preserve_directory_metadata 并统一打印与错误信息，供 hevc/av1 main 复用。
pub fn preserve_directory_metadata_with_log(base_dir: &Path, output_dir: &Path) {
    println!("\n📁 Preserving directory metadata...");
    if let Err(e) = preserve_directory_metadata(base_dir, output_dir) {
        eprintln!("⚠️ Failed to preserve directory metadata: {}", e);
    } else {
        println!("✅ Directory metadata preserved");
    }
}

/// 🔥 v8.2.5: 原地模式保存目录时间戳（用于处理结束后恢复）
/// 处理会修改目录 mtime，需在结束后恢复以保留文件夹元数据
pub fn save_directory_timestamps(
    dir: &Path,
) -> io::Result<
    std::collections::HashMap<std::path::PathBuf, (filetime::FileTime, filetime::FileTime)>,
> {
    use std::collections::HashMap;
    let mut saved = HashMap::new();
    if dir.is_dir() {
        if let Ok(meta) = std::fs::metadata(dir) {
            let atime = filetime::FileTime::from_last_access_time(&meta);
            let mtime = filetime::FileTime::from_last_modification_time(&meta);
            saved.insert(dir.to_path_buf(), (atime, mtime));
        }
        collect_dir_timestamps(dir, &mut saved)?;
    }
    Ok(saved)
}

/// 恢复已保存的目录时间戳
pub fn restore_directory_timestamps(
    saved: &std::collections::HashMap<std::path::PathBuf, (filetime::FileTime, filetime::FileTime)>,
) {
    for (path, (atime, mtime)) in saved {
        if path.exists() && path.is_dir() {
            if let Err(e) = filetime::set_file_times(path, *atime, *mtime) {
                eprintln!(
                    "⚠️ Failed to restore directory timestamps for {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
}

/// 🔥 v8.2.5: 将保存的源目录时间戳应用到输出目录（相邻模式）
/// 处理过程中源目录被读取( atime 更新)、输出目录被写入( mtime 更新)，需用处理前保存的元数据恢复
pub fn apply_saved_timestamps_to_dst(
    saved: &std::collections::HashMap<std::path::PathBuf, (filetime::FileTime, filetime::FileTime)>,
    src_root: &Path,
    dst_root: &Path,
) {
    for (src_path, (atime, mtime)) in saved {
        if let Ok(rel_path) = src_path.strip_prefix(src_root) {
            let dst_path = dst_root.join(rel_path);
            if dst_path.exists() && dst_path.is_dir() {
                if let Err(e) = filetime::set_file_times(&dst_path, *atime, *mtime) {
                    eprintln!(
                        "⚠️ Failed to apply directory timestamps to {}: {}",
                        dst_path.display(),
                        e
                    );
                }
            }
        }
    }
}

/// 按源文件对目标应用时间戳（复用唯一实现，避免重复）
fn copy_file_timestamps_only(src: &Path, dst: &Path) {
    apply_file_timestamps(src, dst);
}

/// 输出树中每个文件按相对路径在源树中找同名 stem 的源文件（尝试常见扩展名），并复制时间戳
fn copy_file_timestamps_from_source_tree(src_root: &Path, dst_root: &Path) {
    const SOURCE_EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "webp", "heic", "heif", "avif", "gif", "tiff", "tif", "bmp", "jxl",
    ];
    for entry in walkdir::WalkDir::new(dst_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let dst_path = entry.path();
        if !dst_path.is_file() {
            continue;
        }
        let rel = match dst_path.strip_prefix(dst_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let parent = rel.parent().unwrap_or(rel);
        let stem = dst_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem.is_empty() {
            continue;
        }
        let src_parent = src_root.join(parent);
        for ext in SOURCE_EXTENSIONS {
            let src_file = src_parent.join(format!("{}.{}", stem, ext));
            if src_file.exists() && src_file.is_file() {
                copy_file_timestamps_only(&src_file, dst_path);
                break;
            }
        }
    }
}

/// 🔥 v8.2.5: 从源目录树恢复输出目录树的时间戳（目录 + 文件）
/// 用于后处理（如 JXL Container Fix）修改了输出文件/目录后，用源侧时间戳统一恢复。
/// 脚本仅需调用 img-hevc restore-timestamps <src> <dst>，不重复实现逻辑。
pub fn restore_timestamps_from_source_to_output(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    let saved = save_directory_timestamps(src_dir)?;
    apply_saved_timestamps_to_dst(&saved, src_dir, dst_dir);
    copy_file_timestamps_from_source_tree(src_dir, dst_dir);
    restore_directory_timestamps(&saved);
    Ok(())
}

fn collect_dir_timestamps(
    dir: &Path,
    map: &mut std::collections::HashMap<
        std::path::PathBuf,
        (filetime::FileTime, filetime::FileTime),
    >,
) -> io::Result<()> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(meta) = std::fs::metadata(&path) {
                    let atime = filetime::FileTime::from_last_access_time(&meta);
                    let mtime = filetime::FileTime::from_last_modification_time(&meta);
                    map.insert(path.clone(), (atime, mtime));
                }
                collect_dir_timestamps(&path, map)?;
            }
        }
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
            delete_xmp_after_merge: false, // 不删除XMP，让用户决定
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
    // 策略1: 绝对路径直接匹配 (photo.jpg.xmp)
    if let Some(ext) = src.extension() {
        let xmp_full = src.with_extension(format!("{}.xmp", ext.to_str()?));
        if xmp_full.exists() {
            return Some(xmp_full);
        }
    }

    // 策略2: 同名匹配 (photo.xmp)
    let xmp_stem = src.with_extension("xmp");
    if xmp_stem.exists() {
        return Some(xmp_stem);
    }

    // 策略3: 深度扫描与 Stem 解耦匹配 (处理重命名或误导后缀的情况)
    if let Some(parent) = src.parent() {
        if let Some(src_stem_raw) = src.file_stem() {
            let src_stem = src_stem_raw.to_string_lossy().to_lowercase();
            // 如果 src_stem 本身包含点（如 image.jpg），取最左侧部分作为 root_stem
            let src_root_stem = src_stem.split('.').next().unwrap_or(&src_stem);

            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();

                    // 必须是以 .xmp 结尾的文件
                    if !path
                        .extension()
                        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("xmp"))
                    {
                        continue;
                    }

                    if let Some(xmp_stem_raw) = path.file_stem() {
                        let xmp_stem = xmp_stem_raw.to_string_lossy().to_lowercase();
                        // 剥离 XMP stem 中可能存在的原始扩展名 (image.jpg -> image)
                        let xmp_root_stem = xmp_stem.split('.').next().unwrap_or(&xmp_stem);

                        // 匹配逻辑：
                        // 1. 完全匹配 stem (忽略大小写): photo.xmp vs photo.jpg
                        // 2. 匹配双重扩展名 stem: photo.jpg.xmp vs photo.jpg
                        // 3. 匹配 Root Stem (终极回退): photo.jpg.xmp vs photo.png
                        if xmp_stem == src_stem
                            || xmp_stem
                                == format!(
                                    "{}.{}",
                                    src_stem,
                                    src.extension().and_then(|e| e.to_str()).unwrap_or("")
                                )
                            || xmp_root_stem == src_root_stem
                        {
                            return Some(path);
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
