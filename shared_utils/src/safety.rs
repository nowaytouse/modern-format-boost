//! Safety Module
//!
//! Provides safety checks to prevent accidental damage to system directories
//! Reference: media/CONTRIBUTING.md - Robust Safety & Loud Errors requirement

use std::path::Path;

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
    "/opt",
];

pub fn check_dangerous_directory(path: &Path) -> Result<(), String> {
    let path_str = path.to_string_lossy();

    for dangerous in DANGEROUS_DIRS {
        if path_str == *dangerous {
            return Err(format!(
                "🚨 DANGEROUS OPERATION BLOCKED!\n\
                 ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\
                 ❌ Target directory '{}' is a protected system directory.\n\
                 ❌ Operating on this directory could cause IRREVERSIBLE DAMAGE to your system.\n\
                 \n\
                 💡 Please specify a safe subdirectory instead.\n\
                 ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
                dangerous
            ));
        }
    }

    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let components: Vec<_> = canonical.components().collect();

    if components.len() <= 3 {
        let path_str = canonical.to_string_lossy();
        if path_str.starts_with("/Users/") || path_str.starts_with("/home/") {
            if components.len() <= 3 {
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
        }
    }

    Ok(())
}

pub fn check_safe_for_destructive(path: &Path, operation: &str) -> Result<(), String> {
    check_dangerous_directory(path)?;

    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
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

pub fn check_extension_whitelist(path: &Path, whitelist: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| whitelist.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dangerous_directories() {
        assert!(check_dangerous_directory(Path::new("/")).is_err());
        assert!(check_dangerous_directory(Path::new("/System")).is_err());
        assert!(check_dangerous_directory(Path::new("/usr")).is_err());
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
}
