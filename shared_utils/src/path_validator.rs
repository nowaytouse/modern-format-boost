//! Path Validation Module
//!
//! Provides path sanitization and validation to prevent command injection attacks.
//! 路径验证模块，防止命令注入攻击。

use std::fmt;
use std::path::Path;

/// Dangerous shell metacharacters that could enable command injection
/// 危险的 shell 元字符，可能导致命令注入
const DANGEROUS_CHARS: &[char] = &[
    ';',  // Command separator
    '|',  // Pipe
    '&',  // Background/AND
    '$',  // Variable expansion
    '`',  // Command substitution
    '(',  // Subshell
    ')',  // Subshell
    '{',  // Brace expansion
    '}',  // Brace expansion
    '<',  // Input redirection
    '>',  // Output redirection
    '\n', // Newline (command separator)
    '\r', // Carriage return
    '\0', // Null byte
];

/// Path validation error
/// 路径验证错误
#[derive(Debug, Clone)]
pub enum PathValidationError {
    /// Path contains a dangerous character
    /// 路径包含危险字符
    DangerousCharacter {
        character: char,
        path: String,
    },
    /// Path is empty
    /// 路径为空
    EmptyPath,
    /// Path contains null byte
    /// 路径包含空字节
    NullByte(String),
}

impl fmt::Display for PathValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathValidationError::DangerousCharacter { character, path } => {
                write!(f, "❌ PATH SECURITY ERROR: Dangerous character '{}' found in path: {}", 
                    character, path)
            }
            PathValidationError::EmptyPath => {
                write!(f, "❌ PATH SECURITY ERROR: Empty path provided")
            }
            PathValidationError::NullByte(path) => {
                write!(f, "❌ PATH SECURITY ERROR: Null byte found in path: {}", path)
            }
        }
    }
}

impl std::error::Error for PathValidationError {}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.5: 安全路径转换 (避免 unwrap panic)
// ═══════════════════════════════════════════════════════════════

/// 路径转换错误
#[derive(Debug, Clone)]
pub struct PathConversionError {
    pub path_display: String,
    pub reason: String,
}

impl fmt::Display for PathConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "⚠️ PATH CONVERSION ERROR: {} (path: {})", self.reason, self.path_display)
    }
}

impl std::error::Error for PathConversionError {}

/// 安全地将 Path 转换为 &str，失败时返回 Result
/// 🔥 v6.5: 替代 path.to_str().unwrap() 避免 panic
pub fn path_to_str_safe(path: &Path) -> Result<&str, PathConversionError> {
    path.to_str().ok_or_else(|| {
        let err = PathConversionError {
            path_display: path.to_string_lossy().to_string(),
            reason: "Path contains non-UTF-8 characters".to_string(),
        };
        eprintln!("{}", err);
        err
    })
}

/// 安全地将 Path 转换为 String，使用 lossy 转换
/// 🔥 v6.5: 非 UTF-8 字符会被替换为 U+FFFD
pub fn path_to_string_lossy(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// 安全地将 Path 转换为 String，失败时返回 Result
pub fn path_to_string_safe(path: &Path) -> Result<String, PathConversionError> {
    path_to_str_safe(path).map(|s| s.to_string())
}

/// Validate a path for safety before using in shell commands
/// 在 shell 命令中使用前验证路径安全性
///
/// # Arguments
/// * `path` - The path to validate
///
/// # Returns
/// * `Ok(())` if the path is safe
/// * `Err(PathValidationError)` if the path contains dangerous characters
///
/// # Example
/// ```
/// use shared_utils::path_validator::validate_path;
/// use std::path::Path;
///
/// let safe_path = Path::new("/home/user/video.mp4");
/// assert!(validate_path(safe_path).is_ok());
///
/// let dangerous_path = Path::new("/home/user/; rm -rf /");
/// assert!(validate_path(dangerous_path).is_err());
/// ```
pub fn validate_path(path: &Path) -> Result<(), PathValidationError> {
    let path_str = path.to_string_lossy();
    
    // Check for empty path
    if path_str.is_empty() {
        eprintln!("⚠️ PATH VALIDATION FAILED: Empty path");
        return Err(PathValidationError::EmptyPath);
    }
    
    // Check for dangerous characters
    for &c in DANGEROUS_CHARS {
        if path_str.contains(c) {
            eprintln!("⚠️ PATH VALIDATION FAILED: Dangerous character '{}' in: {}", c, path_str);
            return Err(PathValidationError::DangerousCharacter {
                character: c,
                path: path_str.to_string(),
            });
        }
    }
    
    Ok(())
}

/// Validate multiple paths at once
/// 一次验证多个路径
pub fn validate_paths(paths: &[&Path]) -> Result<(), PathValidationError> {
    for path in paths {
        validate_path(path)?;
    }
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_paths() {
        let safe_paths = [
            "/home/user/video.mp4",
            "/tmp/test file with spaces.mov",
            "relative/path/to/file.mkv",
            "./current_dir.avi",
            "../parent_dir.webm",
            "/path/with-dashes_and_underscores.mp4",
            "/path/with.multiple.dots.mp4",
            "/中文路径/视频.mp4",
            "/日本語/ビデオ.mp4",
        ];
        
        for path_str in &safe_paths {
            let path = Path::new(path_str);
            assert!(validate_path(path).is_ok(), "Path should be safe: {}", path_str);
        }
    }

    #[test]
    fn test_dangerous_semicolon() {
        let path = Path::new("/home/user/; rm -rf /");
        let result = validate_path(path);
        assert!(result.is_err());
        if let Err(PathValidationError::DangerousCharacter { character, .. }) = result {
            assert_eq!(character, ';');
        }
    }

    #[test]
    fn test_dangerous_pipe() {
        let path = Path::new("/home/user/video.mp4 | cat /etc/passwd");
        let result = validate_path(path);
        assert!(result.is_err());
        if let Err(PathValidationError::DangerousCharacter { character, .. }) = result {
            assert_eq!(character, '|');
        }
    }

    #[test]
    fn test_dangerous_ampersand() {
        let path = Path::new("/home/user/video.mp4 && rm -rf /");
        let result = validate_path(path);
        assert!(result.is_err());
        if let Err(PathValidationError::DangerousCharacter { character, .. }) = result {
            assert_eq!(character, '&');
        }
    }

    #[test]
    fn test_dangerous_dollar() {
        let path = Path::new("/home/$USER/video.mp4");
        let result = validate_path(path);
        assert!(result.is_err());
        if let Err(PathValidationError::DangerousCharacter { character, .. }) = result {
            assert_eq!(character, '$');
        }
    }

    #[test]
    fn test_dangerous_backtick() {
        let path = Path::new("/home/user/`whoami`.mp4");
        let result = validate_path(path);
        assert!(result.is_err());
        if let Err(PathValidationError::DangerousCharacter { character, .. }) = result {
            assert_eq!(character, '`');
        }
    }

    #[test]
    fn test_dangerous_redirect() {
        let path = Path::new("/home/user/video.mp4 > /dev/null");
        let result = validate_path(path);
        assert!(result.is_err());
    }

    #[test]
    fn test_dangerous_newline() {
        let path = Path::new("/home/user/video.mp4\nrm -rf /");
        let result = validate_path(path);
        assert!(result.is_err());
        if let Err(PathValidationError::DangerousCharacter { character, .. }) = result {
            assert_eq!(character, '\n');
        }
    }

    #[test]
    fn test_validate_paths_all_safe() {
        let paths: Vec<&Path> = vec![
            Path::new("/home/user/video1.mp4"),
            Path::new("/home/user/video2.mp4"),
        ];
        assert!(validate_paths(&paths).is_ok());
    }

    #[test]
    fn test_validate_paths_one_dangerous() {
        let paths: Vec<&Path> = vec![
            Path::new("/home/user/video1.mp4"),
            Path::new("/home/user/; rm -rf /"),
        ];
        assert!(validate_paths(&paths).is_err());
    }

    #[test]
    fn test_error_display() {
        let err = PathValidationError::DangerousCharacter {
            character: ';',
            path: "/test/path".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("Dangerous character"));
        assert!(msg.contains(";"));
    }

    // Property test: all dangerous chars are detected
    #[test]
    fn test_all_dangerous_chars_detected() {
        for &c in DANGEROUS_CHARS {
            let path_str = format!("/home/user/test{}file.mp4", c);
            let path = Path::new(&path_str);
            assert!(validate_path(path).is_err(),
                "Dangerous char '{}' should be detected", c);
        }
    }
}
