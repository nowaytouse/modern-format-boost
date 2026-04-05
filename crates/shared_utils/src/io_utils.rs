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
pub fn metadata_with_retry<P: AsRef<Path>>(path: P) -> std::io::Result<fs::Metadata> {
    let p = path.as_ref();
    let mut last_err = None;

    for i in 0..3 {
        match fs::metadata(p) {
            Ok(m) => return Ok(m),
            Err(e) => {
                // If the file is not found, retry won't help. Return immediately.
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Err(e);
                }

                last_err = Some(e);
                if i < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }

    let err = last_err.expect("Metadata retry loop failed unexpectedly without an error");

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
pub fn robust_move(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Err(e) = std::fs::rename(src, dst) {
        // EXDEV (OS Error 18) indicates "Cross-device link"
        if e.kind() == std::io::ErrorKind::InvalidInput
            || e.raw_os_error() == Some(18)
            || e.to_string().to_lowercase().contains("crosses devices")
        {
            std::fs::copy(src, dst)?;
            std::fs::remove_file(src)?;
        } else {
            return Err(e);
        }
    }
    Ok(())
}
