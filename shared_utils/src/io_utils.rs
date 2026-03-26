//! IO Utilities - Safe file operations and hardened cleanup
//!
//! Provides unified error handling for common I/O tasks like temporary
//! file removal, ensuring that genuine system errors are logged while
//! expected "not found" cases are handled silently.

use std::fs;
use std::path::Path;
use tracing::warn;

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
