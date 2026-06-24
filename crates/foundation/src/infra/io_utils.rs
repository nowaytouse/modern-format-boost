//! IO Utilities - Safe file operations and hardened cleanup
//!
//! Provides unified error handling for common I/O tasks like temporary
//! file removal, ensuring that genuine system errors are logged while
//! expected "not found" cases are handled silently.

use std::fs;
use std::path::Path;

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
