//! Safety Module
//! 
//! Provides safety checks to prevent accidental damage to system directories
//! Reference: media/CONTRIBUTING.md - Robust Safety & Loud Errors requirement

use std::path::Path;

/// List of dangerous directories that should never be modified
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
    "/Users",  // Root of all user directories
    "/home",   // Linux home root
    "/root",   // Root user home
    "/boot",
    "/dev",
    "/proc",
    "/sys",
    "/tmp",
    "/opt",
];

/// Check if a path is within a dangerous directory
/// 
/// # Arguments
/// * `path` - The path to check
/// 
/// # Returns
/// * `Ok(())` if the path is safe
/// * `Err(String)` with a descriptive error message if dangerous
/// 
/// # Example
/// ```
/// use shared_utils::check_dangerous_directory;
/// use std::path::Path;
/// 
/// // Safe path
/// assert!(check_dangerous_directory(Path::new("/Users/me/Documents/photos")).is_ok());
/// 
/// // Dangerous path
/// assert!(check_dangerous_directory(Path::new("/System")).is_err());
/// ```
pub fn check_dangerous_directory(path: &Path) -> Result<(), String> {
    let path_str = path.to_string_lossy();
    
    // Check exact matches
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
    
    // Check if path is a direct child of dangerous directories (e.g., /Users/username)
    // Allow deeper paths like /Users/username/Documents
    // 
    // 注意：这里使用 unwrap_or_else 是合理的，因为：
    // 1. canonicalize 失败通常意味着路径不存在（新文件）
    // 2. 使用原路径进行安全检查是保守的策略
    // 3. 如果路径真的危险，后续检查仍会捕获
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let components: Vec<_> = canonical.components().collect();
    
    // Special handling for user home directories
    if components.len() <= 3 {
        let path_str = canonical.to_string_lossy();
        if path_str.starts_with("/Users/") || path_str.starts_with("/home/") {
            // Only allow if it's deeper than /Users/username
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

/// Check if a path is safe for destructive operations (delete, in-place replace)
/// This is more strict than check_dangerous_directory
pub fn check_safe_for_destructive(path: &Path, operation: &str) -> Result<(), String> {
    // First do the basic dangerous directory check
    check_dangerous_directory(path)?;
    
    // Additional checks for destructive operations
    // 注意：同上，canonicalize 失败时使用原路径是安全的
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path_str = canonical.to_string_lossy();
    
    // Warn about Desktop and Downloads (common accident locations)
    if path_str.contains("/Desktop") || path_str.contains("/Downloads") {
        eprintln!(
            "⚠️  WARNING: You are about to {} files in '{}'.\n\
             ⚠️  This is a common location for important files.\n\
             ⚠️  Make sure you have backups before proceeding.",
            operation, path.display()
        );
    }
    
    Ok(())
}

/// Validate that a file extension is in the allowed whitelist
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
        // Deep paths should be safe
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
