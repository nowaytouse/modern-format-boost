//! Safety Module
//!
//! Provides safety checks to prevent accidental damage to system directories
//! Reference: media/CONTRIBUTING.md - Robust Safety & Loud Errors requirement

use std::path::{Component, Path, PathBuf};

const DANGEROUS_DIRS: &[&str] = &[
    "/",
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/etc",
    "/var",
    "/private",
    "/Library",
    "/Applications",
    "/Users",
    "/home",
    "/root",
    "/boot",
    "/dev",
    "/proc",
    "/sys",
    "/tmp",
    "/private/tmp",
    "/opt",
];

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        crate::media_conversion_gate::delivery_safety_relative_base_or_root("safety_normalize_path")
            .join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn normalized_danger_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![normalize_path_lexically(path)];
    match path.canonicalize() {
        Ok(canonical) => {
            let canonical = normalize_path_lexically(&canonical);
            if !candidates.iter().any(|candidate| candidate == &canonical) {
                candidates.push(canonical);
            }
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "path_safety",
                format!(
                    "failed to canonicalize {} for danger check: {e}",
                    path.display()
                ),
            );
        }
    }
    candidates
}

fn is_exact_dangerous_dir(path: &Path) -> bool {
    normalized_danger_candidates(path).iter().any(|candidate| {
        let candidate = candidate.to_string_lossy();
        DANGEROUS_DIRS.contains(&candidate.as_ref())
    })
}

fn is_home_rootish(path: &Path) -> bool {
    normalized_danger_candidates(path).iter().any(|candidate| {
        let path_str = candidate.to_string_lossy();
        let components = candidate.components().count();
        (path_str.starts_with("/Users/") || path_str.starts_with("/home/")) && components <= 3
    })
}

/// Check if a directory is dangerous to perform operations in.
///
/// # Errors
/// Returns an error if the directory is considered dangerous.
pub fn check_dangerous_directory(path: &Path) -> Result<(), String> {
    if is_exact_dangerous_dir(path) {
        return Err(crate::media_conversion_gate::ui_safety_system_dir_blocked(
            path,
        ));
    }

    if is_home_rootish(path) {
        return Err(crate::media_conversion_gate::ui_safety_home_root_blocked(
            path,
        ));
    }

    Ok(())
}

/// Check if a path is safe for destructive operations.
///
/// # Errors
/// Returns an error if the operation is considered unsafe.
pub fn check_safe_for_destructive(path: &Path, operation: &str) -> Result<(), String> {
    check_dangerous_directory(path)?;

    let canonical = normalize_path_lexically(
        &crate::media_conversion_gate::canonicalize_for_tool_input(path),
    );
    let path_str = canonical.to_string_lossy();

    if path_str.contains("/Desktop") || path_str.contains("/Downloads") {
        crate::media_conversion_gate::delivery_runtime_path_audit(
            "delivery_ui",
            path,
            format!(
                "WARNING: You are about to {} files in '{}'. This is a common location for \
                 important files. Make sure you have backups before proceeding.",
                operation,
                path.display()
            ),
        );
    }

    Ok(())
}

#[must_use]
pub fn is_extension_allowed(path: &Path, allowed_extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| allowed_extensions.contains(&e.to_lowercase().as_str()))
}

/// Check if a path is inside an Apple Photos library package
///
/// Apple Photos libraries are special package directories (*.photoslibrary)
/// that contain a complex internal structure managed by Photos.app. Direct
/// manipulation of files inside these packages can corrupt the library database
/// and cause data loss.
///
/// This function checks if the given path is:
/// 1. Inside a directory ending with .photoslibrary
/// 2. Inside a directory ending with .photolibrary (older format)
///
/// Returns an error if the path is inside a Photos library.
/// Check if a path is an Apple Photos library.
///
/// # Errors
/// Returns an error if the path is an Apple Photos library.
pub fn check_apple_photos_library(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("resolve relative Photos guard path: {error}"))?
            .join(path)
    };
    let existing = absolute
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| {
            format!(
                "Photos guard found no existing ancestor for {}",
                path.display()
            )
        })?;
    let suffix = absolute.strip_prefix(existing).map_err(|error| {
        format!(
            "Photos guard could not resolve {} below {}: {error}",
            path.display(),
            existing.display()
        )
    })?;
    let canonical = existing
        .canonicalize()
        .map_err(|error| format!("canonicalize Photos guard ancestor: {error}"))?
        .join(suffix);
    let canonical = normalize_path_lexically(&canonical);

    // Check each component of the path
    for ancestor in canonical.ancestors() {
        if let Some(name) = ancestor.file_name().and_then(|n| n.to_str())
            && (name.ends_with(".photoslibrary") || name.ends_with(".photolibrary"))
        {
            return Err(
                crate::media_conversion_gate::ui_safety_photos_library_blocked(path, ancestor),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dangerous_directories() {
        assert!(check_dangerous_directory(Path::new("/")).is_err());
        assert!(check_dangerous_directory(Path::new("/System")).is_err());
        assert!(check_dangerous_directory(Path::new("/usr")).is_err());
        assert!(check_dangerous_directory(Path::new("/Users/")).is_err());
        assert!(check_dangerous_directory(Path::new("/private/tmp")).is_err());
        assert!(check_dangerous_directory(Path::new("/Users/test/..")).is_err());
    }

    #[test]
    fn test_safe_directories() {
        assert!(check_dangerous_directory(Path::new("/Users/test/Documents/photos")).is_ok());
    }

    #[test]
    fn test_extension_allowed() {
        let allowed = &["png", "jpg", "jpeg"];
        assert!(is_extension_allowed(Path::new("test.png"), allowed));
        assert!(is_extension_allowed(Path::new("test.PNG"), allowed));
        assert!(!is_extension_allowed(Path::new("test.exe"), allowed));
    }

    #[test]
    fn test_apple_photos_library_detection() {
        // Test .photoslibrary detection
        assert!(
            check_apple_photos_library(Path::new(
                "/Users/test/Pictures/My Library.photoslibrary/Masters/2024/01/01/IMG_1234.jpg"
            ))
            .is_err()
        );

        // Test .photolibrary detection (older format)
        assert!(
            check_apple_photos_library(Path::new(
                "/Users/test/Pictures/My Library.photolibrary/Masters/2024/01/01/IMG_1234.jpg"
            ))
            .is_err()
        );

        // Test safe paths
        assert!(
            check_apple_photos_library(Path::new("/Users/test/Pictures/Exports/IMG_1234.jpg"))
                .is_ok()
        );
        assert!(
            check_apple_photos_library(Path::new("/Users/test/Documents/photos/IMG_1234.jpg"))
                .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_photos_guard_resolves_existing_symlink_ancestor_for_new_output() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("Debug.photoslibrary");
        let alias = temp.path().join("Alias");
        std::fs::create_dir_all(&library).unwrap();
        std::os::unix::fs::symlink(&library, &alias).unwrap();

        assert!(check_apple_photos_library(&alias.join("new/output")).is_err());
        assert!(check_apple_photos_library(&temp.path().join("safe/new/output")).is_ok());
    }
}
