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
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
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
    if let Ok(canonical) = path.canonicalize() {
        let canonical = normalize_path_lexically(&canonical);
        if !candidates.iter().any(|candidate| candidate == &canonical) {
            candidates.push(canonical);
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
        return Err(format!(
            "🚨 DANGEROUS OPERATION BLOCKED!\n\
             ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\
             ❌ Target directory '{}' is a protected system directory.\n\
             ❌ Operating on this directory could cause IRREVERSIBLE DAMAGE to your system.\n\
             \n\
             💡 Please specify a safe subdirectory instead.\n\
             ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
            path.display()
        ));
    }

    if is_home_rootish(path) {
        return Err(format!(
            "🚨 DANGEROUS OPERATION BLOCKED!\n\
             ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\
             ❌ Target '{}' is too close to your home directory root.\n\
             ❌ Operating here could affect ALL your personal files.\n\
             \n\
             💡 Please specify a subdirectory like ~/Documents/photos instead.\n\
             ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
            path.display()
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

    let canonical =
        normalize_path_lexically(&path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
    let path_str = canonical.to_string_lossy();

    if path_str.contains("/Desktop") || path_str.contains("/Downloads") {
        eprintln!(
            "⚠️  WARNING: You are about to {} files in '{}'.\n\
             ⚠️  This is a common location for important files.\n\
             ⚠️  Make sure you have backups before proceeding.",
            operation,
            path.display()
        );
    }

    Ok(())
}

#[must_use]
pub fn check_extension_whitelist(path: &Path, whitelist: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| whitelist.contains(&e.to_lowercase().as_str()))
}

/// Check if a path is inside an Apple Photos library package
///
/// Apple Photos libraries are special package directories (*.photoslibrary) that contain
/// a complex internal structure managed by Photos.app. Direct manipulation of files
/// inside these packages can corrupt the library database and cause data loss.
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
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Check each component of the path
    for ancestor in canonical.ancestors() {
        if let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".photoslibrary") || name.ends_with(".photolibrary") {
                return Err(format!(
                    "🚨 APPLE PHOTOS LIBRARY DETECTED!\n\
                     ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\
                     ❌ Target path '{}' is inside an Apple Photos library:\n\
                     ❌ '{}'\n\
                     \n\
                     ⚠️  Direct manipulation of files inside Photos libraries can:\n\
                     ⚠️  • Corrupt the Photos database\n\
                     ⚠️  • Break photo organization and metadata\n\
                     ⚠️  • Cause permanent data loss\n\
                     \n\
                     💡 To process photos from your Photos library:\n\
                     💡 1. Export photos from Photos.app to a separate folder\n\
                     💡 2. Run this tool on the exported folder\n\
                     💡 3. Import the converted photos back into Photos if needed\n\
                     ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
                    path.display(),
                    ancestor.display()
                ));
            }
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
    fn test_extension_whitelist() {
        let whitelist = &["png", "jpg", "jpeg"];
        assert!(check_extension_whitelist(Path::new("test.png"), whitelist));
        assert!(check_extension_whitelist(Path::new("test.PNG"), whitelist));
        assert!(!check_extension_whitelist(Path::new("test.exe"), whitelist));
    }

    #[test]
    fn test_apple_photos_library_detection() {
        // Test .photoslibrary detection
        assert!(check_apple_photos_library(Path::new(
            "/Users/test/Pictures/My Library.photoslibrary/Masters/2024/01/01/IMG_1234.jpg"
        ))
        .is_err());

        // Test .photolibrary detection (older format)
        assert!(check_apple_photos_library(Path::new(
            "/Users/test/Pictures/My Library.photolibrary/Masters/2024/01/01/IMG_1234.jpg"
        ))
        .is_err());

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
}
