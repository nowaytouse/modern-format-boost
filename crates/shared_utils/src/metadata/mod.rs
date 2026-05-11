//! Metadata Preservation Module
//!
//! Layered preservation: Internal (`ExifTool`) / Network / System (ACL, xattr, timestamps).
//! Unified entry point for timestamps: single files via `apply_file_timestamps(src, dst)`, directory trees via
//! `save_directory_timestamps` → `apply_saved_timestamps_to_dst` / `restore_directory_timestamps`,
//! Avoids redundant implementations. `ExifTool` rewrites files, so timestamps are always set after write operations.

use crate::builder_base::ToolBuilder;
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

pub use exif::preserve_internal;
#[cfg(target_os = "macos")]
pub use macos::append_mfb_branding;

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn apply_file_timestamps(src: &Path, dst: &Path) {
    crate::log_info!(
        crate::static_logs::messages::LABEL_METADATA,
        &format!(
            "apply_file_timestamps: {} → {}",
            src.display(),
            dst.display()
        )
    );
    let Ok(m) = std::fs::metadata(src) else {
        crate::log_info!(
            crate::static_logs::messages::LABEL_METADATA,
            "Failed to read source metadata"
        );
        return;
    };

    // Platform-specific creation time preservation FIRST (before atime/mtime)
    // This is critical because filetime::set_file_times may reset creation time on some systems
    #[cfg(target_os = "macos")]
    {
        if let Ok(created) = m.created() {
            crate::log_info!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("Original creation time: {created:?}")
            );
            if let Err(e) = macos::set_creation_time(dst, created) {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to set creation time: {e}")
                );
            } else {
                crate::log_info!(
                    crate::static_logs::messages::LABEL_METADATA,
                    "Set creation time successfully"
                );
            }
        } else {
            crate::log_info!(
                crate::static_logs::messages::LABEL_METADATA,
                "Failed to read original creation time"
            );
        }
        if let Ok(added) = macos::get_added_time(src) {
            if let Err(e) = macos::set_added_time(dst, added) {
                crate::log_info!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to set added time: {e}")
                );
            } else {
                crate::log_info!(
                    crate::static_logs::messages::LABEL_METADATA,
                    "Set added time successfully"
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: Use filetime crate's set_file_times which preserves creation time
        if let Ok(created) = m.created() {
            let ctime = filetime::FileTime::from_system_time(created);
            let atime = filetime::FileTime::from_last_access_time(&m);
            // On Windows, filetime::set_file_times also sets creation time
            if let Err(e) = filetime::set_file_times(dst, atime, ctime) {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to set Windows creation time: {e}")
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: Try to preserve birth time if available (requires statx on newer kernels)
        // Note: Most Linux filesystems don't support setting birth time, so this is best-effort
        if let Ok(created) = m.created() {
            // Linux typically doesn't allow setting birth time, but we try anyway
            // This often fails on Linux filesystems, but when it does we should still make it visible.
            if let Err(e) = linux::try_set_birth_time(dst, created) {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to preserve Linux birth time: {e}")
                );
            }
        }
    }

    // Set atime/mtime AFTER creation time
    let atime = filetime::FileTime::from_last_access_time(&m);
    let mtime = filetime::FileTime::from_last_modification_time(&m);
    if let Err(e) = filetime::set_file_times(dst, atime, mtime) {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!("Failed to set file times: {e}")
        );
    } else {
        crate::log_info!(
            crate::static_logs::messages::LABEL_METADATA,
            "Set atime/mtime successfully"
        );
    }

    // RE-APPLY creation time on macOS after setting atime/mtime
    // This is necessary because filetime::set_file_times may reset creation time
    #[cfg(target_os = "macos")]
    {
        if let Ok(created) = m.created() {
            crate::log_info!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("Re-applying creation time after atime/mtime: {created:?}")
            );
            if let Err(e) = macos::set_creation_time(dst, created) {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to re-set creation time: {e}")
                );
            }
        }
    }

    // Verify creation time was preserved (macOS only)
    #[cfg(target_os = "macos")]
    {
        if let (Ok(expected_created), Ok(dst_meta)) = (m.created(), std::fs::metadata(dst))
            && let Ok(actual_created) = dst_meta.created()
        {
            crate::log_info!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("Verified creation time: {actual_created:?}")
            );
            // Check if it matches (allow 1 second tolerance for filesystem precision)
            let diff = if actual_created > expected_created {
                actual_created
                    .duration_since(expected_created)
                    .unwrap_or_default()
            } else {
                expected_created
                    .duration_since(actual_created)
                    .unwrap_or_default()
            };
            if diff.as_secs() > 1 {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Creation time mismatch! Expected: {expected_created:?}, Got: {actual_created:?}, Diff: {diff:?}"
                    )
                );
            }
        }
    }
}

/// Preserve "Pro" metadata (XMP, ICC, etc.).
///
/// # Errors
/// Returns an `io::Result` if preservation fails.
pub fn preserve_pro(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // copyfile: copies ACL + STAT + xattr in one syscall
        if let Err(e) = macos::copy_native_metadata(src, dst) {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("macOS native copy failed: {e}")
            );
            // Fallback: manual xattr copy if copyfile failed
            copy_xattrs_manual(src, dst);
        }
        // ExifTool: EXIF/IPTC/XMP internal tags
        if let Err(e) = exif::preserve_internal(src, dst) {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("Internal metadata failed: {e}")
            );
        }
        // Network xattrs — copy + verify
        network::preserve_network_metadata(src, dst);

        // Unix permission bits (copyfile covers STAT but be explicit)
        if let Ok(meta) = std::fs::metadata(src) {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode();
            if let Err(e) = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode)) {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to preserve macOS permission bits: {e}")
                );
            }
        }
        // Timestamps last (ExifTool rewrites file, so must come after)
        apply_file_timestamps(src, dst);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        // ExifTool: EXIF/IPTC/XMP internal tags
        if let Err(e) = exif::preserve_internal(src, dst) {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("Internal metadata failed: {e}")
            );
        }
        // Network xattrs — copy + verify
        network::preserve_network_metadata(src, dst);
        // Platform-specific attributes
        #[cfg(target_os = "linux")]
        if let Err(e) = linux::preserve_linux_attributes(src, dst) {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("Linux attribute preservation failed: {e}")
            );
        }
        #[cfg(target_os = "windows")]
        if let Err(e) = windows::preserve_windows_attributes(src, dst) {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("Windows attribute preservation failed: {e}")
            );
        }
        // Generic xattr copy (covers any remaining xattrs not handled above)
        copy_xattrs_manual(src, dst);
        // Unix permission bits
        #[cfg(unix)]
        if let Ok(meta) = std::fs::metadata(src) {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode();
            if let Err(e) = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode)) {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to preserve Unix permission bits: {e}")
                );
            }
        }
        // Timestamps last
        apply_file_timestamps(src, dst);
        Ok(())
    }
}

/// Preserve all metadata from source to destination.
///
/// # Errors
/// Returns an `io::Result` if preservation fails.
pub fn preserve(src: &Path, dst: &Path) -> io::Result<()> {
    preserve_pro(src, dst)
}

/// Merge source's XMP sidecar into destination (for conversion output). Idempotent if no sidecar.
pub fn merge_xmp_sidecar_into_dest(src: &Path, dst: &Path) {
    merge_xmp_sidecar(src, dst);
}

pub fn copy(src: &Path, dst: &Path) {
    if let Err(e) = preserve(src, dst) {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!("Failed to preserve metadata: {e}")
        );
    }
    merge_xmp_sidecar(src, dst);
    apply_file_timestamps(src, dst);
}

/// Preserve directory metadata (timestamps, etc.).
///
/// # Errors
/// Returns an `io::Result` if preservation fails.
pub fn preserve_directory(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    use std::collections::HashMap;

    let mut dir_metadata: HashMap<std::path::PathBuf, std::fs::Metadata> = HashMap::new();

    if src_dir.is_dir() {
        if let Ok(meta) = std::fs::metadata(src_dir) {
            dir_metadata.insert(src_dir.to_path_buf(), meta);
        }

        collect_dir_metadata(src_dir, &mut dir_metadata)?;
    }

    for (src_path, metadata) in &dir_metadata {
        let rel_path = src_path.strip_prefix(src_dir).unwrap_or(src_path);
        let dst_path = dst_dir.join(rel_path);

        if !dst_path.exists()
            && let Err(e) = std::fs::create_dir_all(&dst_path)
        {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!(
                    "Failed to create directory {path}: {e}",
                    path = dst_path.display()
                )
            );
            continue;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            if let Err(e) =
                std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(mode))
            {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Failed to set permissions for {path}: {e}",
                        path = dst_path.display()
                    )
                );
            }
        }

        // macOS: set creation time BEFORE atime/mtime (will re-apply after)
        #[cfg(target_os = "macos")]
        {
            if let Ok(created) = metadata.created()
                && let Err(e) = macos::set_creation_time(&dst_path, created)
            {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Failed to set creation time for {path}: {e}",
                        path = dst_path.display()
                    )
                );
            }
        }

        let atime = filetime::FileTime::from_last_access_time(metadata);
        let mtime = filetime::FileTime::from_last_modification_time(metadata);
        if let Err(e) = filetime::set_file_times(&dst_path, atime, mtime) {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!(
                    "Failed to set timestamps for {path}: {e}",
                    path = dst_path.display()
                )
            );
        }

        // macOS: re-apply creation time AFTER atime/mtime (filetime may reset it)
        #[cfg(target_os = "macos")]
        {
            if let Ok(created) = metadata.created()
                && let Err(e) = macos::set_creation_time(&dst_path, created)
            {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Failed to set creation time for {path}: {e}",
                        path = dst_path.display()
                    )
                );
            }
            // Also preserve added time for directories
            if let Ok(added) = macos::get_added_time(src_path)
                && let Err(e) = macos::set_added_time(&dst_path, added)
            {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Failed to set added time for {path}: {e}",
                        path = dst_path.display()
                    )
                );
            }
        }

        copy_dir_xattrs(src_path, &dst_path);
    }

    Ok(())
}

pub fn preserve_directory_with_log(base_dir: &Path, output_dir: &Path) {
    crate::log_info!(
        crate::static_logs::messages::LABEL_METADATA,
        "📁 Preserving directory metadata..."
    );
    if let Err(e) = preserve_directory(base_dir, output_dir) {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!("Failed to preserve directory metadata: {e}")
        );
    } else {
        crate::log_info!(
            crate::static_logs::messages::LABEL_METADATA,
            "✅ Directory metadata preserved"
        );
    }
}

/// Save directory timestamps to a map.
///
/// # Errors
/// Returns an `io::Result` if saving fails.
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

pub fn restore_directory_timestamps<S>(
    saved: &std::collections::HashMap<
        std::path::PathBuf,
        (filetime::FileTime, filetime::FileTime),
        S,
    >,
) where
    S: std::hash::BuildHasher,
{
    let mut failed_count = 0_i32;
    let mut total_count = 0_i32;

    for (path, (atime, mtime)) in saved {
        if path.exists() && path.is_dir() {
            total_count += 1_i32;
            if let Err(e) = filetime::set_file_times(path, *atime, *mtime) {
                failed_count += 1_i32;
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Failed to restore directory timestamp for {path}: {e}",
                        path = path.display()
                    )
                );
            }
        }
    }

    if failed_count > 0_i32 {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!(
                "TIMESTAMP VERIFICATION: {failed_count}/{total_count} directories failed (possible filesystem protection or network mount)"
            )
        );
    }
}

pub fn apply_saved_timestamps_to_dst<S>(
    saved: &std::collections::HashMap<
        std::path::PathBuf,
        (filetime::FileTime, filetime::FileTime),
        S,
    >,
    src_root: &Path,
    dst_root: &Path,
) where
    S: std::hash::BuildHasher,
{
    let mut failed_count = 0_i32;
    let mut total_count = 0_i32;

    for (src_path, (atime, mtime)) in saved {
        if let Ok(rel_path) = src_path.strip_prefix(src_root) {
            let dst_path = dst_root.join(rel_path);
            if dst_path.exists() && dst_path.is_dir() {
                total_count += 1_i32;
                if let Err(e) = filetime::set_file_times(&dst_path, *atime, *mtime) {
                    failed_count += 1_i32;
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_METADATA,
                        &format!(
                            "Failed to apply directory timestamp to {path}: {e}",
                            path = dst_path.display()
                        )
                    );
                }
            }
        }
    }

    if failed_count > 0_i32 {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!(
                "TIMESTAMP VERIFICATION: {failed_count}/{total_count} destination directories failed (possible filesystem protection or network mount)"
            )
        );
    }
}

fn copy_file_timestamps_only(src: &Path, dst: &Path) {
    apply_file_timestamps(src, dst);
}

fn copy_file_timestamps_from_source_tree(src_root: &Path, dst_root: &Path) {
    const SOURCE_EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "webp", "heic", "heif", "avif", "gif", "tiff", "tif", "bmp", "jxl",
    ];
    for entry in walkdir::WalkDir::new(dst_root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Failed to inspect destination file while restoring timestamps from source tree (dir={}): {}",
                        dst_root.display(),
                        err
                    )
                );
                continue;
            }
        };
        let dst_path = entry.path();
        if !dst_path.is_file() {
            continue;
        }
        let Ok(rel) = dst_path.strip_prefix(dst_root) else {
            continue;
        };
        let parent = rel.parent().unwrap_or(rel);
        let stem = dst_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem.is_empty() {
            continue;
        }
        let src_parent = src_root.join(parent);
        for ext in SOURCE_EXTENSIONS {
            let src_file = src_parent.join(format!("{stem}.{ext}"));
            if src_file.exists() && src_file.is_file() {
                copy_file_timestamps_only(&src_file, dst_path);
                break;
            }
        }
    }
}

/// Restore timestamps from source directory to output directory.
///
/// # Errors
/// Returns an `io::Result` if restoration fails.
pub fn restore_timestamps_from_source_to_output(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    let saved = save_directory_timestamps(src_dir)?;
    apply_saved_timestamps_to_dst(&saved, src_dir, dst_dir);
    copy_file_timestamps_from_source_tree(src_dir, dst_dir);
    restore_directory_timestamps(&saved);
    Ok(())
}

fn collect_dir_timestamps<S>(
    dir: &Path,
    map: &mut std::collections::HashMap<
        std::path::PathBuf,
        (filetime::FileTime, filetime::FileTime),
        S,
    >,
) -> io::Result<()>
where
    S: std::hash::BuildHasher,
{
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let meta = std::fs::metadata(&path)?;
            let atime = filetime::FileTime::from_last_access_time(&meta);
            let mtime = filetime::FileTime::from_last_modification_time(&meta);
            map.insert(path.clone(), (atime, mtime));
            collect_dir_timestamps(&path, map)?;
        }
    }
    Ok(())
}

fn collect_dir_metadata<S>(
    dir: &Path,
    map: &mut std::collections::HashMap<std::path::PathBuf, std::fs::Metadata, S>,
) -> io::Result<()>
where
    S: std::hash::BuildHasher,
{
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let meta = std::fs::metadata(&path)?;
            map.insert(path.clone(), meta);
            collect_dir_metadata(&path, map)?;
        }
    }
    Ok(())
}

fn copy_dir_xattrs(src: &Path, dst: &Path) {
    match xattr::list(src) {
        Ok(iter) => {
            for name in iter {
                let Some(name_str) = name.to_str() else {
                    continue;
                };
                match xattr::get(src, name_str) {
                    Ok(Some(value)) => {
                        if let Err(e) = xattr::set(dst, name_str, &value) {
                            crate::log_anomaly!(
                                crate::static_logs::messages::LABEL_METADATA,
                                &format!(
                                    "Failed to copy directory xattr '{name_str}' to {path}: {e}",
                                    path = dst.display()
                                )
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        crate::log_anomaly!(
                            crate::static_logs::messages::LABEL_METADATA,
                            &format!(
                                "Failed to read directory xattr '{name_str}' from {path}: {e}",
                                path = src.display()
                            )
                        );
                    }
                }
            }
        }
        Err(e) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!(
                    "Failed to list directory xattrs for {path}: {e}",
                    path = src.display()
                )
            );
        }
    }
}

/// Fallback: try exiv2 to merge XMP into the destination (exiv2 -i expects sidecar named \\<stem\\>.xmp beside image).
/// Returns true if exiv2 merge succeeded. No fake success; only when exiv2 actually succeeds do we return true.
fn try_merge_xmp_exiv2(xmp_path: &Path, dst: &Path) -> bool {
    let Some(parent) = dst.parent() else {
        return false;
    };
    let Some(stem_raw) = dst.file_stem() else {
        return false;
    };
    let stem = stem_raw.to_string_lossy();
    let sidecar_for_exiv2 = parent.join(format!("{stem}.xmp"));
    if sidecar_for_exiv2 == *xmp_path {
        return false;
    }
    if let Err(e) = std::fs::copy(xmp_path, &sidecar_for_exiv2) {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!(
                "Failed to prepare temporary XMP sidecar for exiv2 fallback (xmp={}, dst={}): {}",
                xmp_path.display(),
                dst.display(),
                e
            )
        );
        return false;
    }
    let out = crate::tool_builders::Exiv2Builder::new()
        .arg("-ix")
        .input(dst)
        .build()
        .output();
    let ok = out.as_ref().is_ok_and(|o| o.status.success());
    if let Ok(out) = &out {
        if !out.status.success() {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!(
                    "exiv2 XMP fallback returned non-zero status (dst={}, stderr={})",
                    dst.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                )
            );
        }
    } else if let Err(err) = &out {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!(
                "Failed to launch exiv2 for XMP fallback (dst={}): {}",
                dst.display(),
                err
            )
        );
    }
    if let Err(e) = std::fs::remove_file(&sidecar_for_exiv2) {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!(
                "Failed to remove temporary exiv2 sidecar {path}: {e}",
                path = sidecar_for_exiv2.display()
            )
        );
    }
    ok
}

fn merge_xmp_sidecar(src: &Path, dst: &Path) {
    let xmp_path = find_xmp_sidecar(src);

    if let Some(xmp) = xmp_path {
        if crate::progress_mode::is_verbose_mode() {
            crate::log_info!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!("📋 Found XMP sidecar: {path}", path = xmp.display())
            );
        }

        let config = crate::xmp_merger::Config {
            delete_xmp_after_merge: false,
            overwrite_mode: crate::xmp_merger::OverwriteMode::Original,
            preserve_timestamps: true,
            log_level: crate::xmp_merger::LogLevel::Quiet,
        };

        let merger = crate::xmp_merger::XmpMerger::new(config);

        crate::progress_mode::xmp_merge_attempt();
        match merger.merge_xmp(&xmp, dst) {
            Ok(()) => {
                crate::progress_mode::xmp_merge_success();
            }
            Err(e) => {
                let err_str = e.to_string();
                let format_unsupported = err_str.to_lowercase().contains("format error in file");
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
                    crate::log_info!(
                        crate::static_logs::messages::LABEL_METADATA,
                        "Fallback: exiv2 merge succeeded (ExifTool had failed)."
                    );
                } else if crate::progress_mode::has_log_file() && !format_unsupported {
                    crate::log_info!(
                        crate::static_logs::messages::LABEL_METADATA,
                        "Fallback: exiv2 merge failed or exiv2 not available; no fake success."
                    );
                }
            }
        }
    }
}

pub(crate) fn find_xmp_sidecar(src: &Path) -> Option<std::path::PathBuf> {
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

    if let Some(parent) = src.parent()
        && let Some(src_stem_raw) = src.file_stem()
    {
        let src_stem = src_stem_raw.to_string_lossy().to_lowercase();
        let src_ext = src
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let src_compound = if src_ext.is_empty() {
            src_stem.clone()
        } else {
            format!("{src_stem}.{src_ext}")
        };

        match std::fs::read_dir(parent) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(err) => {
                            crate::log_anomaly!(
                                crate::static_logs::messages::LABEL_METADATA,
                                &format!(
                                    "Failed to inspect sibling file while searching for XMP sidecar (dir={}): {}",
                                    parent.display(),
                                    err
                                )
                            );
                            continue;
                        }
                    };
                    let path = entry.path();

                    if !path.is_file() {
                        continue;
                    }

                    if !path
                        .extension()
                        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("xmp"))
                    {
                        continue;
                    }

                    if let Some(xmp_stem_raw) = path.file_stem() {
                        let xmp_stem = xmp_stem_raw.to_string_lossy().to_lowercase();

                        if xmp_stem == src_stem || xmp_stem == src_compound {
                            return Some(path);
                        }
                    }
                }
            }
            Err(err) => {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Failed to read parent directory while searching for XMP sidecar (dir={}): {}",
                        parent.display(),
                        err
                    )
                );
            }
        }
    }

    None
}

fn copy_xattrs_manual(src: &Path, dst: &Path) {
    match xattr::list(src) {
        Ok(iter) => {
            for name in iter {
                let Some(name_str) = name.to_str() else {
                    continue;
                };
                match xattr::get(src, name_str) {
                    Ok(Some(value)) => {
                        if let Err(e) = xattr::set(dst, name_str, &value) {
                            crate::log_anomaly!(
                                crate::static_logs::messages::LABEL_METADATA,
                                &format!(
                                    "Failed to copy xattr '{name_str}' to {path}: {e}",
                                    path = dst.display()
                                )
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        crate::log_anomaly!(
                            crate::static_logs::messages::LABEL_METADATA,
                            &format!(
                                "Failed to read xattr '{name_str}' from {path}: {e}",
                                path = src.display()
                            )
                        );
                    }
                }
            }
        }
        Err(e) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!(
                    "Failed to list xattrs for {path}: {e}",
                    path = src.display()
                )
            );
        }
    }
}
