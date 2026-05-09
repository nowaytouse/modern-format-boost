//! IO Utilities - Safe file operations and hardened cleanup
//!
//! Provides unified error handling for common I/O tasks like temporary
//! file removal, ensuring that genuine system errors are logged while
//! expected "not found" cases are handled silently.

use std::fs;
use std::path::Path;
use tracing::warn;

/// Read file metadata with a retry mechanism to handle transient OS locks or network glitches.
///
/// Default: 3 retries with 100ms delay.
/// This prevents one-off "Failed to read file metadata" errors from breaking batch processing.
/// Get metadata with retry.
///
/// # Errors
/// Returns an error if the metadata cannot be retrieved after all retries.
///
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
        warn!(
            path = %p.display(),
            error = %err,
            "HARD FAILURE: metadata retry loop ended without an underlying OS error"
        );
        return Err(err);
    };

    warn!(
        path = %p.display(),
        error = %err,
        "HARD FAILURE: Persistent metadata read failure after 3 retries"
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
            warn!(
                path = %p.display(),
                error = %e,
                "Failed to remove file (non-NotFound error)"
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
            warn!(
                path = %p.display(),
                error = %e,
                "Failed to remove directory (non-NotFound error)"
            );
            Err(e)
        }
    }
}

/// Robust move that handles cross-filesystem boundaries (EXDEV).
///
/// If `fs::rename` fails because the source and destination are on different
/// mount points (e.g. system SSD to external HDD), falls back to `copy` + `delete`.
/// Move a file robustly.
///
/// # Errors
/// Returns an error if the move fails.
pub fn robust_move(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Err(e) = std::fs::rename(src, dst) {
        // EXDEV (OS Error 18) indicates "Cross-device link"
        if e.kind() == std::io::ErrorKind::InvalidInput
            || e.raw_os_error() == Some(18_i32)
            || e.to_string().to_lowercase().contains("crosses devices")
        {
            let staging = dst.extension().and_then(|e| e.to_str()).map_or_else(
                || dst.with_extension("mfb-tmp"),
                |ext| dst.with_extension(format!("{ext}.mfb-tmp")),
            );
            if staging.exists() {
                std::fs::remove_file(&staging).unwrap_or_else(|e| {
                    tracing::warn!("Non-fatal cleanup/fallback operation failed: {}", e);
                });
            }
            if let Err(copy_err) = std::fs::copy(src, &staging) {
                std::fs::remove_file(&staging).unwrap_or_else(|e| {
                    tracing::warn!("Non-fatal cleanup/fallback operation failed: {}", e);
                });
                return Err(copy_err);
            }
            if let Err(rename_err) = std::fs::rename(&staging, dst) {
                std::fs::remove_file(&staging).unwrap_or_else(|e| {
                    tracing::warn!("Non-fatal cleanup/fallback operation failed: {}", e);
                });
                return Err(rename_err);
            }
            std::fs::remove_file(src)?;
        } else {
            return Err(e);
        }
    }
    Ok(())
}

/// Extract the last `n` non-empty lines from a stderr buffer, joined by `" | "`.
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
    // `start` is always ≤ `lines.len()` because `saturating_sub` cannot produce a value
    // larger than the operand; direct indexing is sound.
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

/// Systematic safe byte access for media metadata parsing.
///
/// Follows the "Quality Manifesto": Loud (warns), Honest (returns None/Err on failure),
/// and Non-blocking (prevents panics).
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
        self.get(pos..pos + 4).map_or_else(
            || {
                warn!(
                    "☢️ [ANOMALY] Required 4 bytes for '{}' missing at pos {}! Refusing to forge data.",
                    name, pos
                );
                None
            },
            |b| {
                // Sound: b.len() is guaranteed 4 by get(pos..pos+4).
                Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            },
        )
    }

    fn get_u32_be_strict(&self, pos: usize, name: &str) -> Option<u32> {
        self.get(pos..pos + 4).map_or_else(
            || {
                warn!(
                    "☢️ [ANOMALY] Required 4 bytes for '{}' missing at pos {}! Refusing to forge data.",
                    name, pos
                );
                None
            },
            |b| {
                // Sound: b.len() is guaranteed 4 by get(pos..pos+4).
                Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            },
        )
    }

    fn get_u64_be_strict(&self, pos: usize, name: &str) -> Option<u64> {
        self.get(pos..pos + 8).map_or_else(
            || {
                warn!(
                    "☢️ [ANOMALY] Required 8 bytes for '{}' missing at pos {}! Refusing to forge data.",
                    name, pos
                );
                None
            },
            |b| {
                // Sound: b.len() is guaranteed 8 by get(pos..pos+8).
                Some(u64::from_be_bytes([
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                ]))
            },
        )
    }

    fn get_u16_le_strict(&self, pos: usize, name: &str) -> Option<u16> {
        self.get(pos..pos + 2).map_or_else(
            || {
                warn!(
                    "☢️ [ANOMALY] Required 2 bytes for '{}' missing at pos {}! Refusing to forge data.",
                    name, pos
                );
                None
            },
            |b| {
                // Sound: b.len() is guaranteed 2 by get(pos..pos+2).
                Some(u16::from_le_bytes([b[0], b[1]]))
            },
        )
    }

    fn get_u16_be_strict(&self, pos: usize, name: &str) -> Option<u16> {
        self.get(pos..pos + 2).map_or_else(
            || {
                warn!(
                    "☢️ [ANOMALY] Required 2 bytes for '{}' missing at pos {}! Refusing to forge data.",
                    name, pos
                );
                None
            },
            |b| {
                // Sound: b.len() is guaranteed 2 by get(pos..pos+2).
                Some(u16::from_be_bytes([b[0], b[1]]))
            },
        )
    }

    fn get_byte_strict(&self, pos: usize, name: &str) -> Option<u8> {
        self.get(pos).copied().or_else(|| {
            warn!(
                "☢️ [ANOMALY] Required byte for '{}' missing at pos {}! Refusing to forge data.",
                name, pos
            );
            None
        })
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
}
