//! Path Validation Module
//!
//! Provides path sanitization and validation to prevent command injection attacks.
//! 路径验证模块，防止命令注入攻击。

use std::fmt;
use std::path::Path;

const DANGEROUS_CHARS: &[char] = &[
    ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '\n', '\r', '\0',
];

#[derive(Debug, Clone)]
pub enum PathValidationError {
    DangerousCharacter { character: char, path: String },
    EmptyPath,
    NullByte(String),
    InputOutputConflict { path: String },
}

impl fmt::Display for PathValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathValidationError::DangerousCharacter { character, path } => {
                write!(
                    f,
                    "❌ PATH SECURITY ERROR: Dangerous character '{}' found in path: {}",
                    character, path
                )
            }
            PathValidationError::EmptyPath => {
                write!(f, "❌ PATH SECURITY ERROR: Empty path provided")
            }
            PathValidationError::NullByte(path) => {
                write!(
                    f,
                    "❌ PATH SECURITY ERROR: Null byte found in path: {}",
                    path
                )
            }
            PathValidationError::InputOutputConflict { path } => {
                write!(
                    f,
                    "❌ PATH CONFLICT ERROR: Input and output paths are identical: {}",
                    path
                )
            }
        }
    }
}

impl std::error::Error for PathValidationError {}

#[derive(Debug, Clone)]
pub struct PathConversionError {
    pub path_display: String,
    pub reason: String,
}

impl fmt::Display for PathConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "⚠️ PATH CONVERSION ERROR: {} (path: {})",
            self.reason, self.path_display
        )
    }
}

impl std::error::Error for PathConversionError {}

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

pub fn path_to_string_lossy(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub fn path_to_string_safe(path: &Path) -> Result<String, PathConversionError> {
    path_to_str_safe(path).map(|s| s.to_string())
}

pub fn validate_path(path: &Path) -> Result<(), PathValidationError> {
    let path_str = path.to_string_lossy();

    if path_str.is_empty() {
        eprintln!("⚠️ PATH VALIDATION FAILED: Empty path");
        return Err(PathValidationError::EmptyPath);
    }

    for &c in DANGEROUS_CHARS {
        if path_str.contains(c) {
            eprintln!(
                "⚠️ PATH VALIDATION FAILED: Dangerous character '{}' in: {}",
                c, path_str
            );
            return Err(PathValidationError::DangerousCharacter {
                character: c,
                path: path_str.to_string(),
            });
        }
    }

    Ok(())
}

pub fn validate_paths(paths: &[&Path]) -> Result<(), PathValidationError> {
    for path in paths {
        validate_path(path)?;
    }
    Ok(())
}

pub fn check_input_output_conflict(input: &Path, output: &Path) -> Result<(), PathValidationError> {
    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());

    let output_canonical = if output.exists() {
        output
            .canonicalize()
            .unwrap_or_else(|_| output.to_path_buf())
    } else {
        if output.is_relative() {
            std::env::current_dir().unwrap_or_default().join(output)
        } else {
            output.to_path_buf()
        }
    };

    if input_canonical == output_canonical {
        return Err(PathValidationError::InputOutputConflict {
            path: input.display().to_string(),
        });
    }

    Ok(())
}

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
            assert!(
                validate_path(path).is_ok(),
                "Path should be safe: {}",
                path_str
            );
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

    #[test]
    fn test_all_dangerous_chars_detected() {
        for &c in DANGEROUS_CHARS {
            let path_str = format!("/home/user/test{}file.mp4", c);
            let path = Path::new(&path_str);
            assert!(
                validate_path(path).is_err(),
                "Dangerous char '{}' should be detected",
                c
            );
        }
    }
}
