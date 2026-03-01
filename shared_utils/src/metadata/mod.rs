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

pub fn preserve_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    preserve_pro(src, dst)
}

/// Merge source's XMP sidecar into destination (for conversion output). Idempotent if no sidecar.
pub fn merge_xmp_sidecar_into_dest(src: &Path, dst: &Path) {
    merge_xmp_sidecar(src, dst);
}

pub fn copy_metadata(src: &Path, dst: &Path) {
    if let Err(e) = preserve_metadata(src, dst) {
        eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }
    merge_xmp_sidecar(src, dst);
    apply_file_timestamps(src, dst);
}

pub fn preserve_directory_metadata(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    use std::collections::HashMap;

    let mut dir_metadata: HashMap<std::path::PathBuf, std::fs::Metadata> = HashMap::new();

    if src_dir.is_dir() {
        if let Ok(meta) = std::fs::metadata(src_dir) {
            dir_metadata.insert(src_dir.to_path_buf(), meta);
        }

        collect_dir_metadata(src_dir, &mut dir_metadata)?;
    }

    for (src_path, metadata) in dir_metadata.iter() {
        let rel_path = src_path.strip_prefix(src_dir).unwrap_or(src_path);
        let dst_path = dst_dir.join(rel_path);

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

        let atime = filetime::FileTime::from_last_access_time(metadata);
        let mtime = filetime::FileTime::from_last_modification_time(metadata);
        if let Err(e) = filetime::set_file_times(&dst_path, atime, mtime) {
            eprintln!(
                "⚠️ Failed to set timestamps for {}: {}",
                dst_path.display(),
                e
            );
        }

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

        copy_dir_xattrs(src_path, &dst_path);
    }

    Ok(())
}

pub fn preserve_directory_metadata_with_log(base_dir: &Path, output_dir: &Path) {
    println!("\n📁 Preserving directory metadata...");
    if let Err(e) = preserve_directory_metadata(base_dir, output_dir) {
        eprintln!("⚠️ Failed to preserve directory metadata: {}", e);
    } else {
        println!("✅ Directory metadata preserved");
    }
}

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

fn copy_file_timestamps_only(src: &Path, dst: &Path) {
    apply_file_timestamps(src, dst);
}

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
                collect_dir_metadata(&path, map)?;
            }
        }
    }
    Ok(())
}

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

/// Fallback: try exiv2 to merge XMP into the destination (exiv2 -i expects sidecar named <stem>.xmp beside image).
/// Returns true if exiv2 merge succeeded. No fake success; only when exiv2 actually succeeds do we return true.
fn try_merge_xmp_exiv2(xmp_path: &Path, dst: &Path) -> bool {
    let Some(parent) = dst.parent() else {
        return false;
    };
    let stem = dst.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
    let sidecar_for_exiv2 = parent.join(format!("{}.xmp", stem));
    if sidecar_for_exiv2 == *xmp_path {
        return false;
    }
    if std::fs::copy(xmp_path, &sidecar_for_exiv2).is_err() {
        return false;
    }
    let out = std::process::Command::new("exiv2")
        .args(["-i", crate::safe_path_arg(dst).as_ref()])
        .output();
    let ok = out
        .as_ref()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(&sidecar_for_exiv2);
    ok
}

fn merge_xmp_sidecar(src: &Path, dst: &Path) {
    let xmp_path = find_xmp_sidecar(src);

    if let Some(xmp) = xmp_path {
        if crate::progress_mode::is_verbose_mode() {
            eprintln!("📋 Found XMP sidecar: {}", xmp.display());
        }

        let config = crate::xmp_merger::XmpMergerConfig {
            delete_xmp_after_merge: false,
            overwrite_original: true,
            preserve_timestamps: true,
            verbose: false,
        };

        let merger = crate::xmp_merger::XmpMerger::new(config);

        crate::progress_mode::xmp_merge_attempt();
        match merger.merge_xmp(&xmp, dst) {
            Ok(()) => {
                crate::progress_mode::xmp_merge_success();
            }
            Err(e) => {
                let err_str = e.to_string();
                let format_unsupported = err_str
                    .to_lowercase()
                    .contains("format error in file");
                if format_unsupported {
                    let line = crate::progress_mode::format_status_line(
                        "   ⚠️  XMP merge skipped (ExifTool does not support writing to this file format)",
                    );
                    crate::progress_mode::emit_stderr(&line);
                } else {
                    crate::progress_mode::xmp_merge_failure(&err_str);
                }
                let fallback_ok = try_merge_xmp_exiv2(&xmp, dst);
                if fallback_ok {
                    crate::progress_mode::xmp_merge_success();
                    if crate::progress_mode::has_log_file() {
                        crate::progress_mode::write_to_log_at_level(
                            tracing::Level::INFO,
                            "   → Fallback: exiv2 merge succeeded (ExifTool had failed).",
                        );
                    }
                } else if crate::progress_mode::has_log_file() && !format_unsupported {
                    crate::progress_mode::write_to_log_at_level(
                        tracing::Level::INFO,
                        "   → Fallback: exiv2 merge failed or exiv2 not available; no fake success.",
                    );
                }
            }
        }
    }
}

fn find_xmp_sidecar(src: &Path) -> Option<std::path::PathBuf> {
    if let Some(ext) = src.extension() {
        let xmp_full = src.with_extension(format!("{}.xmp", ext.to_str()?));
        if xmp_full.exists() {
            return Some(xmp_full);
        }
    }

    let xmp_stem = src.with_extension("xmp");
    if xmp_stem.exists() {
        return Some(xmp_stem);
    }

    if let Some(parent) = src.parent() {
        if let Some(src_stem_raw) = src.file_stem() {
            let src_stem = src_stem_raw.to_string_lossy().to_lowercase();
            let src_root_stem = src_stem.split('.').next().unwrap_or(&src_stem);

            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();

                    if !path
                        .extension()
                        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("xmp"))
                    {
                        continue;
                    }

                    if let Some(xmp_stem_raw) = path.file_stem() {
                        let xmp_stem = xmp_stem_raw.to_string_lossy().to_lowercase();
                        let xmp_root_stem = xmp_stem.split('.').next().unwrap_or(&xmp_stem);

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
