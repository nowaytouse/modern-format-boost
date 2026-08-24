//! IO Utilities - Safe file operations and hardened cleanup
//!
//! Provides unified error handling for common I/O tasks like temporary
//! file removal, ensuring that genuine system errors are logged while
//! expected "not found" cases are handled silently.

use std::fs;
use std::path::{Path, PathBuf};

/// Read file metadata with a retry mechanism to handle transient OS locks or
/// network glitches.
///
/// Default: 3 retries with 100ms delay.
/// This prevents one-off "Failed to read file metadata" errors from breaking
/// batch processing. Get metadata with retry.
///
/// # Errors
/// Returns an error if the metadata cannot be retrieved after all retries.
pub fn metadata_with_retry<P: AsRef<Path>>(path: P) -> std::io::Result<fs::Metadata> {
    let p = path.as_ref();
    let mut last_err = None;

    for i in 0_i32..3_i32 {
        match fs::metadata(p) {
            Ok(m) => return Ok(m),
            Err(e) => {
                // If the file is not found, retry won't help. Return immediately.
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Err(e);
                }

                last_err = Some(e);
                if i < 2_i32 {
                    std::thread::sleep(std::time::Duration::from_millis(
                        crate::constants::RETRY_DELAY_LONG_MS,
                    ));
                }
            }
        }
    }

    let Some(err) = last_err else {
        let err =
            std::io::Error::other("metadata retry loop exhausted without capturing an OS error");
        crate::media_conversion_gate::delivery_io_path_audit(
            "delivery_intent",
            p,
            format!(
                "FILE METADATA HARD FAILURE: Retry loop exhausted for '{}' | Forensic: No \
                 underlying OS error captured",
                p.display()
            ),
        );
        return Err(err);
    };

    crate::media_conversion_gate::delivery_io_path_audit(
        "delivery_io",
        p,
        format!(
            "FILE METADATA HARD FAILURE: Persistent read failure after 3 retries for '{}' | \
             System Error: {}",
            p.display(),
            err
        ),
    );

    Err(err)
}

/// Safely remove a file, ignoring its absence but logging other errors.
///
/// This is preferred over `let _ = fs::remove_file(path)` as it ensures
/// that permission issues or locked file errors are surfaced as warnings
/// in the run logs and terminal, while missing files (common in temp cleanups)
/// are handled silently.
/// Safely remove a file.
///
/// # Errors
/// Returns an error if the file cannot be removed.
pub fn safe_remove_file<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    let p = path.as_ref();
    match fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            crate::media_conversion_gate::delivery_io_path_audit(
                "delivery_io",
                p,
                format!(
                    "FILE REMOVAL FAILURE: Unexpected error while deleting '{}' (non-NotFound) | \
                     System Error: {}",
                    p.display(),
                    e
                ),
            );
            Err(e)
        }
    }
}

/// Safely remove a directory and its contents recursively, ignoring absence.
/// Safely remove a directory and all its contents.
///
/// # Errors
/// Returns an error if the directory cannot be removed.
pub fn safe_remove_dir_all<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    let p = path.as_ref();
    match fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            crate::media_conversion_gate::delivery_io_path_audit(
                "delivery_io",
                p,
                format!(
                    "DIR REMOVAL FAILURE: Unexpected error while deleting tree '{}' \
                     (non-NotFound) | System Error: {}",
                    p.display(),
                    e
                ),
            );
            Err(e)
        }
    }
}

/// Remove only empty directories at and below a user-selected root.
///
/// Candidates are canonicalized, must remain inside `root`, and may not cross
/// symlinks. Removal uses `remove_dir`, never recursive deletion, so a directory
/// that gains content concurrently is preserved by the operating system.
///
/// # Errors
/// Returns an error when the root/candidate escapes the controlled scope or an
/// empty directory cannot be removed for a reason other than absence/content.
pub fn prune_empty_directories_within(
    root: &Path,
    candidates: &[PathBuf],
) -> std::io::Result<usize> {
    prune_empty_directory_candidates_within(root, candidates, true)
}

/// Remove empty descendants below a controlled root while preserving the root.
///
/// This is used when the user selected a single file: its containing directory
/// is a safety boundary, not itself a user-selected cleanup target.
///
/// # Errors
/// Returns an error under the same fail-closed rules as
/// [`prune_empty_directories_within`].
pub fn prune_empty_descendants_within(
    root: &Path,
    candidates: &[PathBuf],
) -> std::io::Result<usize> {
    prune_empty_directory_candidates_within(root, candidates, false)
}

fn prune_empty_directory_candidates_within(
    root: &Path,
    candidates: &[PathBuf],
    remove_root: bool,
) -> std::io::Result<usize> {
    let invalid = |message: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
    crate::safety::check_safe_for_destructive(root, "remove empty directories")
        .map_err(&invalid)?;
    crate::safety::check_apple_photos_library(root).map_err(&invalid)?;

    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(invalid(format!(
            "empty-directory cleanup root must be a real directory: {}",
            root.display()
        )));
    }
    let canonical_root = root.canonicalize()?;
    let mut directories = std::collections::BTreeSet::new();
    if remove_root {
        directories.insert(canonical_root.clone());
    }

    for candidate in candidates {
        let relative = candidate.strip_prefix(root).map_err(|_| {
            invalid(format!(
                "empty-directory cleanup candidate is not lexically inside selected root: {}",
                candidate.display()
            ))
        })?;
        let mut lexical_component = root.to_path_buf();
        for component in relative.components() {
            lexical_component.push(component.as_os_str());
            let metadata = match fs::symlink_metadata(&lexical_component) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(error),
            };
            if metadata.file_type().is_symlink() {
                return Err(invalid(format!(
                    "empty-directory cleanup refuses symlink components: {}",
                    lexical_component.display()
                )));
            }
        }
        let metadata = match fs::symlink_metadata(candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid(format!(
                "empty-directory cleanup candidate must be a real directory: {}",
                candidate.display()
            )));
        }
        let canonical = candidate.canonicalize()?;
        if !canonical.starts_with(&canonical_root) {
            return Err(invalid(format!(
                "empty-directory cleanup candidate escaped selected root: {}",
                candidate.display()
            )));
        }
        let mut current = canonical.as_path();
        loop {
            if remove_root || current != canonical_root {
                directories.insert(current.to_path_buf());
            }
            if current == canonical_root {
                break;
            }
            let Some(parent) = current.parent() else {
                break;
            };
            current = parent;
        }
    }

    let mut directories: Vec<_> = directories.into_iter().collect();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut removed = 0;
    for directory in directories {
        match fs::remove_dir(&directory) {
            Ok(()) => removed += 1,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(removed)
}

/// Robust move that handles cross-filesystem boundaries (EXDEV).
///
/// If `fs::rename` fails because the source and destination are on different
/// mount points (e.g. system SSD to external HDD), falls back to `copy` +
/// `delete`. Move a file robustly.
///
/// # Errors
/// Returns an error if the move fails.
pub fn robust_move(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Err(e) = std::fs::rename(src, dst) {
        if is_cross_device_rename_error(&e) {
            copy_via_destination_staging(src, dst)?;
        } else {
            return Err(e);
        }
    }
    Ok(())
}

/// Flush a committed file and the directory entry that names it.
///
/// A successful `rename` alone does not prove that either the file contents or
/// its new directory entry survived a power loss. Delivery code calls this
/// after the final rename, before it records a durable manifest or removes a
/// source file.
///
/// # Errors
/// Returns an error when the committed file cannot be flushed, or, on Unix,
/// when its parent directory cannot be flushed.
pub fn sync_committed_file_and_parent(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    sync_parent_directory(path)
}

/// Flush the parent directory entry for a newly created or renamed path.
///
/// Directory handles are not portable to every supported platform. Unix
/// performs the durability barrier; other platforms keep the file-level
/// barrier supplied by [`sync_committed_file_and_parent`].
///
/// # Errors
/// Returns an error on Unix when the path has no parent or that directory
/// cannot be opened/flushed.
pub fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("committed path has no parent directory: {}", path.display()),
            )
        })?;
        std::fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn is_cross_device_rename_error(err: &std::io::Error) -> bool {
    let detail = err.to_string().to_ascii_lowercase();
    matches!(err.raw_os_error(), Some(17_i32 | 18_i32))
        || detail.contains("cross-device")
        || detail.contains("crosses devices")
        || detail.contains("not same device")
}

fn copy_via_destination_staging(src: &Path, dst: &Path) -> std::io::Result<()> {
    let src_len = fs::metadata(src)?.len();
    let parent = crate::media_conversion_gate::output_parent_or_dot(dst);
    let staging_hint =
        crate::media_conversion_gate::path_robust_move_staging_path(dst, "robust_move_staging");
    let staging_suffix = staging_hint
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .map_or_else(|| String::from(".tmp"), |ext| format!(".{ext}"));
    let staging = crate::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
        "robust_move_cross_device",
        parent,
        "mfb-move-",
        &staging_suffix,
    )?
    .into_temp_path();

    let copied = match fs::copy(src, &staging) {
        Ok(bytes) => bytes,
        Err(err) => {
            crate::media_conversion_gate::delivery_remove_file_or_audit(
                "robust_move_staging_rollback",
                &staging,
            );
            return Err(err);
        }
    };

    let staging_len = match fs::metadata(&staging) {
        Ok(meta) => meta.len(),
        Err(err) => {
            crate::media_conversion_gate::delivery_remove_file_or_audit(
                "robust_move_staging_rollback",
                &staging,
            );
            return Err(err);
        }
    };

    if copied != src_len || staging_len != src_len {
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "robust_move_staging_rollback",
            &staging,
        );
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Cross-device move copy length mismatch for {} -> {}: source={src_len}, \
                 copied={copied}, staging={staging_len}",
                src.display(),
                dst.display()
            ),
        ));
    }

    if let Err(err) = fs::rename(&staging, dst) {
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "robust_move_staging_rollback",
            &staging,
        );
        return Err(err);
    }

    if let Err(err) = fs::remove_file(src) {
        crate::media_conversion_gate::delivery_io_path_audit(
            "delivery_io",
            src,
            format!(
                "SOURCE REMOVAL FAILURE: Cross-device move committed to '{}' but failed to delete \
                 source '{}' | System Error: {}",
                dst.display(),
                src.display(),
                err
            ),
        );
        return Err(err);
    }

    Ok(())
}

/// Extract the last `n` non-empty lines from a stderr buffer, joined by `" |
/// "`.
///
/// `stderr.lines().last()` on ffmpeg/exiftool output typically returns an empty
/// string (trailing newline) or a meaningless summary line. This helper returns
/// the tail of the actually-informative lines so error messages include the
/// root-cause diagnostic.
#[must_use]
pub fn tail_error_lines(stderr: &str, n: usize) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    // `start` is always ≤ `lines.len()` because `saturating_sub` cannot produce a
    // value larger than the operand; direct indexing is sound.
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

/// Systematic safe byte access for media metadata parsing.
///
/// Follows the "Quality Manifesto": Loud (warns), Honest (returns None/Err on
/// failure), and Non-blocking (prevents panics).
pub trait ByteSliceExt {
    /// Safely read a u32 (Little Endian) with a loud warning on failure.
    fn get_u32_le_strict(&self, pos: usize, name: &str) -> Option<u32>;
    /// Safely read a u32 (Big Endian) with a loud warning on failure.
    fn get_u32_be_strict(&self, pos: usize, name: &str) -> Option<u32>;
    /// Safely read a u64 (Big Endian) with a loud warning on failure.
    fn get_u64_be_strict(&self, pos: usize, name: &str) -> Option<u64>;
    /// Safely read a u16 (Little Endian) with a loud warning on failure.
    fn get_u16_le_strict(&self, pos: usize, name: &str) -> Option<u16>;
    /// Safely read a u16 (Big Endian) with a loud warning on failure.
    fn get_u16_be_strict(&self, pos: usize, name: &str) -> Option<u16>;
    /// Safely read a single byte with a loud warning on failure.
    fn get_byte_strict(&self, pos: usize, name: &str) -> Option<u8>;
}

impl ByteSliceExt for [u8] {
    fn get_u32_le_strict(&self, pos: usize, name: &str) -> Option<u32> {
        let b = crate::media_conversion_gate::probe_io_fixed_slice_or_none(self, pos, 4, name)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn get_u32_be_strict(&self, pos: usize, name: &str) -> Option<u32> {
        let b = crate::media_conversion_gate::probe_io_fixed_slice_or_none(self, pos, 4, name)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn get_u64_be_strict(&self, pos: usize, name: &str) -> Option<u64> {
        let b = crate::media_conversion_gate::probe_io_fixed_slice_or_none(self, pos, 8, name)?;
        Some(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn get_u16_le_strict(&self, pos: usize, name: &str) -> Option<u16> {
        let b = crate::media_conversion_gate::probe_io_fixed_slice_or_none(self, pos, 2, name)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    fn get_u16_be_strict(&self, pos: usize, name: &str) -> Option<u16> {
        let b = crate::media_conversion_gate::probe_io_fixed_slice_or_none(self, pos, 2, name)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }

    fn get_byte_strict(&self, pos: usize, name: &str) -> Option<u8> {
        if let Some(v) = self.get(pos).copied() {
            Some(v)
        } else {
            crate::media_conversion_gate::delivery_io_batch_audit(
                "delivery_io",
                format!("Required byte for '{name}' missing at pos {pos}! Refusing to forge data."),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_error_lines_skips_blank_trailing_lines() {
        let stderr = "first\n[libx265] error: out of memory\nexit\n\n\n";
        let got = tail_error_lines(stderr, 2);
        assert_eq!(got, "[libx265] error: out of memory | exit");
    }

    #[test]
    fn tail_error_lines_empty_input() {
        assert_eq!(tail_error_lines("", 5), "");
        assert_eq!(tail_error_lines("\n\n\n", 5), "");
    }

    #[test]
    fn tail_error_lines_caps_at_available() {
        assert_eq!(tail_error_lines("only line", 5), "only line");
    }

    #[test]
    fn cross_device_staging_does_not_overwrite_fixed_tmp_neighbor() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("source.bin");
        let dst = dir.path().join("output.bin");
        let legacy_fixed_staging = dir.path().join("output.bin.mfb-tmp");

        fs::write(&src, b"converted payload")?;
        fs::write(&legacy_fixed_staging, b"user payload")?;

        copy_via_destination_staging(&src, &dst)?;

        assert_eq!(fs::read(&dst)?, b"converted payload");
        assert_eq!(fs::read(&legacy_fixed_staging)?, b"user payload");
        assert!(!src.exists());

        Ok(())
    }

    #[test]
    fn empty_directory_pruning_is_scoped_and_non_recursive() -> std::io::Result<()> {
        let parent = tempfile::tempdir()?;
        let root = parent.path().join("selected");
        let empty_leaf = root.join("a/b");
        let occupied = root.join("kept");
        fs::create_dir_all(&empty_leaf)?;
        fs::create_dir_all(&occupied)?;
        fs::write(occupied.join("media.jpg"), b"payload")?;

        assert_eq!(
            prune_empty_directories_within(&root, std::slice::from_ref(&empty_leaf))?,
            2
        );
        assert!(!empty_leaf.exists());
        assert!(root.exists());

        fs::remove_file(occupied.join("media.jpg"))?;
        assert_eq!(
            prune_empty_directories_within(&root, std::slice::from_ref(&occupied))?,
            2
        );
        assert!(!root.exists());

        let outside = parent.path().join("outside");
        let selected = parent.path().join("selected-again");
        fs::create_dir_all(&outside)?;
        fs::create_dir_all(&selected)?;
        assert!(prune_empty_directories_within(&selected, &[outside]).is_err());
        assert!(selected.exists());

        let single_file_parent = parent.path().join("single-file-parent");
        let now_empty_child = single_file_parent.join("nested");
        fs::create_dir_all(&now_empty_child)?;
        assert_eq!(
            prune_empty_descendants_within(
                &single_file_parent,
                std::slice::from_ref(&now_empty_child),
            )?,
            1
        );
        assert!(single_file_parent.is_dir());

        let hidden_file_root = parent.path().join("hidden-file-root");
        fs::create_dir_all(&hidden_file_root)?;
        fs::write(
            hidden_file_root.join(".DS_Store"),
            b"user-visible filesystem entry",
        )?;
        assert_eq!(
            prune_empty_directories_within(
                &hidden_file_root,
                std::slice::from_ref(&hidden_file_root),
            )?,
            0
        );
        assert!(hidden_file_root.is_dir());

        let photos_library = parent.path().join("Debug.photoslibrary");
        fs::create_dir_all(&photos_library)?;
        assert!(
            prune_empty_directories_within(&photos_library, std::slice::from_ref(&photos_library),)
                .is_err()
        );
        assert!(photos_library.is_dir());
        assert!(prune_empty_directories_within(Path::new("/tmp"), &[]).is_err());

        #[cfg(unix)]
        {
            let real_leaf = selected.join("real/leaf");
            fs::create_dir_all(&real_leaf)?;
            std::os::unix::fs::symlink(selected.join("real"), selected.join("link"))?;
            assert!(
                prune_empty_directories_within(&selected, &[selected.join("link/leaf")]).is_err()
            );
            assert!(real_leaf.exists());
        }
        Ok(())
    }

    #[test]
    fn test_byte_slice_ext_be() {
        let data: &[u8] = &[0x12, 0x34, 0x56, 0x78, 0x90, 0x12, 0x34, 0x56];
        assert_eq!(data.get_u16_be_strict(0, "test"), Some(0x1234));
        assert_eq!(data.get_u32_be_strict(0, "test"), Some(0x1234_5678));
        assert_eq!(
            data.get_u64_be_strict(0, "test"),
            Some(0x1234_5678_9012_3456)
        );
        assert_eq!(data.get_byte_strict(0, "test"), Some(0x12));

        // Out of bounds
        assert_eq!(data.get_u16_be_strict(7, "test"), None);
        assert_eq!(data.get_u32_be_strict(5, "test"), None);
        assert_eq!(data.get_byte_strict(8, "test"), None);
    }

    #[test]
    fn test_byte_slice_ext_le() {
        let data: &[u8] = &[0x12, 0x34, 0x56, 0x78];
        assert_eq!(data.get_u16_le_strict(0, "test"), Some(0x3412));
        assert_eq!(data.get_u32_le_strict(0, "test"), Some(0x7856_3412));
    }
}
