//! AppError - 统一的应用错误类型
//!
//! 提供清晰的错误分类，区分可恢复和不可恢复错误。

use crate::error_handler::ErrorCategory;
use crate::types::{CrfError, IterationError, SsimError};
use std::fmt;
use std::path::PathBuf;


#[derive(Debug)]
pub enum AppError {
    FileNotFound {
        path: PathBuf,
        operation: Option<String>,
    },

    FileReadError {
        path: PathBuf,
        source: std::io::Error,
        operation: Option<String>,
    },

    FileWriteError {
        path: PathBuf,
        source: std::io::Error,
        operation: Option<String>,
    },

    DirectoryNotFound {
        path: PathBuf,
        operation: Option<String>,
    },

    InvalidCrf(CrfError),

    InvalidSsim(SsimError),

    IterationLimitExceeded(IterationError),

    FfmpegError {
        message: String,
        stderr: String,
        exit_code: Option<i32>,
        command: Option<String>,
        file_path: Option<PathBuf>,
    },

    FfprobeError {
        message: String,
        stderr: String,
        command: Option<String>,
        file_path: Option<PathBuf>,
    },

    ToolNotFound {
        tool_name: String,
        operation: Option<String>,
    },

    CompressionFailed {
        input_size: u64,
        output_size: u64,
        file_path: Option<PathBuf>,
    },

    QualityValidationFailed {
        expected_ssim: f64,
        actual_ssim: f64,
        file_path: Option<PathBuf>,
    },

    OutputExists {
        path: PathBuf,
        operation: Option<String>,
    },

    Io(std::io::Error),

    Other(anyhow::Error),
}

impl AppError {
    pub fn is_recoverable(&self) -> bool {
        true
    }

    pub fn category(&self) -> ErrorCategory {
        match self {
            AppError::FileNotFound { .. } | AppError::DirectoryNotFound { .. } => {
                ErrorCategory::Fatal
            }

            AppError::FileReadError { .. } | AppError::FileWriteError { .. } | AppError::Io(_) => {
                ErrorCategory::Fatal
            }

            AppError::InvalidCrf(_) | AppError::InvalidSsim(_) => ErrorCategory::Recoverable,

            AppError::FfmpegError { .. }
            | AppError::FfprobeError { .. }
            | AppError::ToolNotFound { .. } => ErrorCategory::Fatal,

            AppError::CompressionFailed { .. } | AppError::QualityValidationFailed { .. } => {
                ErrorCategory::Recoverable
            }

            AppError::OutputExists { .. } => ErrorCategory::Optional,

            AppError::IterationLimitExceeded(_) => ErrorCategory::Recoverable,

            AppError::Other(_) => ErrorCategory::Fatal,
        }
    }

    pub fn user_message(&self) -> String {
        match self {
            AppError::FileNotFound { path, operation } => {
                let mut msg = format!("❌ File not found: {}", path.display());
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Operation: {}", op));
                }
                msg
            }
            AppError::DirectoryNotFound { path, operation } => {
                let mut msg = format!("❌ Directory not found: {}", path.display());
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Operation: {}", op));
                }
                msg
            }
            AppError::FileReadError {
                path,
                source,
                operation,
            } => {
                let mut msg = format!("❌ Failed to read file {}: {}", path.display(), source);
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Operation: {}", op));
                }
                msg
            }
            AppError::FileWriteError {
                path,
                source,
                operation,
            } => {
                let mut msg = format!("❌ Failed to write file {}: {}", path.display(), source);
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Operation: {}", op));
                }
                msg
            }
            AppError::InvalidCrf(e) => {
                format!("❌ Invalid CRF value: {}", e)
            }
            AppError::InvalidSsim(e) => {
                format!("❌ Invalid SSIM value: {}", e)
            }
            AppError::IterationLimitExceeded(e) => {
                format!("⚠️ Iteration limit exceeded: {}", e)
            }
            AppError::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path,
            } => {
                let code_str = exit_code
                    .map(|c| format!(" (exit code: {})", c))
                    .unwrap_or_default();
                let mut msg = format!("❌ FFmpeg failed{}: {}", code_str, message);
                if let Some(path) = file_path {
                    msg.push_str(&format!("\n   File: {}", path.display()));
                }
                if let Some(cmd) = command {
                    msg.push_str(&format!("\n   Command: {}", cmd));
                }
                if !stderr.is_empty() {
                    msg.push_str(&format!("\n   Error output: {}", stderr));
                }
                msg
            }
            AppError::FfprobeError {
                message,
                stderr,
                command,
                file_path,
            } => {
                let mut msg = format!("❌ FFprobe failed: {}", message);
                if let Some(path) = file_path {
                    msg.push_str(&format!("\n   File: {}", path.display()));
                }
                if let Some(cmd) = command {
                    msg.push_str(&format!("\n   Command: {}", cmd));
                }
                if !stderr.is_empty() {
                    msg.push_str(&format!("\n   Error output: {}", stderr));
                }
                msg
            }
            AppError::ToolNotFound {
                tool_name,
                operation,
            } => {
                let mut msg = format!(
                    "❌ Tool not found: {}\n💡 Please ensure {} is installed and in PATH",
                    tool_name, tool_name
                );
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Needed for: {}", op));
                }
                msg
            }
            AppError::CompressionFailed {
                input_size,
                output_size,
                file_path,
            } => {
                let ratio = *output_size as f64 / *input_size as f64 * 100.0;
                let mut msg = format!(
                    "❌ Compression failed: output ({} bytes) >= input ({} bytes), ratio {:.1}%",
                    output_size, input_size, ratio
                );
                if let Some(path) = file_path {
                    msg.push_str(&format!("\n   File: {}", path.display()));
                }
                msg
            }
            AppError::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                file_path,
            } => {
                let mut msg = format!(
                    "❌ Quality validation failed: expected SSIM >= {:.4}, actual {:.4}",
                    expected_ssim, actual_ssim
                );
                if let Some(path) = file_path {
                    msg.push_str(&format!("\n   File: {}", path.display()));
                }
                msg
            }
            AppError::OutputExists { path, operation } => {
                let mut msg = format!("⏭️ Output file exists: {}", path.display());
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Operation: {}", op));
                }
                msg
            }
            AppError::Io(e) => {
                format!("❌ IO error: {}", e)
            }
            AppError::Other(e) => {
                format!("❌ Error: {}", e)
            }
        }
    }

    pub fn is_skip(&self) -> bool {
        matches!(self, AppError::OutputExists { .. })
    }


    pub fn with_file_path(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match self {
            AppError::FileNotFound { operation, .. } => AppError::FileNotFound { path, operation },
            AppError::FileReadError {
                source, operation, ..
            } => AppError::FileReadError {
                path,
                source,
                operation,
            },
            AppError::FileWriteError {
                source, operation, ..
            } => AppError::FileWriteError {
                path,
                source,
                operation,
            },
            AppError::DirectoryNotFound { operation, .. } => {
                AppError::DirectoryNotFound { path, operation }
            }
            AppError::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                ..
            } => AppError::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path: Some(path),
            },
            AppError::FfprobeError {
                message,
                stderr,
                command,
                ..
            } => AppError::FfprobeError {
                message,
                stderr,
                command,
                file_path: Some(path),
            },
            AppError::CompressionFailed {
                input_size,
                output_size,
                ..
            } => AppError::CompressionFailed {
                input_size,
                output_size,
                file_path: Some(path),
            },
            AppError::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                ..
            } => AppError::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                file_path: Some(path),
            },
            AppError::OutputExists { operation, .. } => AppError::OutputExists { path, operation },
            other => other,
        }
    }

    pub fn with_operation(self, operation: impl Into<String>) -> Self {
        let operation = Some(operation.into());
        match self {
            AppError::FileNotFound { path, .. } => AppError::FileNotFound { path, operation },
            AppError::FileReadError { path, source, .. } => AppError::FileReadError {
                path,
                source,
                operation,
            },
            AppError::FileWriteError { path, source, .. } => AppError::FileWriteError {
                path,
                source,
                operation,
            },
            AppError::DirectoryNotFound { path, .. } => {
                AppError::DirectoryNotFound { path, operation }
            }
            AppError::ToolNotFound { tool_name, .. } => AppError::ToolNotFound {
                tool_name,
                operation,
            },
            AppError::OutputExists { path, .. } => AppError::OutputExists { path, operation },
            other => other,
        }
    }

    pub fn with_command(self, command: impl Into<String>) -> Self {
        let command = Some(command.into());
        match self {
            AppError::FfmpegError {
                message,
                stderr,
                exit_code,
                file_path,
                ..
            } => AppError::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path,
            },
            AppError::FfprobeError {
                message,
                stderr,
                file_path,
                ..
            } => AppError::FfprobeError {
                message,
                stderr,
                command,
                file_path,
            },
            other => other,
        }
    }
}


impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::FileNotFound { path, operation } => {
                write!(f, "File not found: {}", path.display())?;
                if let Some(op) = operation {
                    write!(f, " (during: {})", op)?;
                }
                Ok(())
            }
            AppError::DirectoryNotFound { path, operation } => {
                write!(f, "Directory not found: {}", path.display())?;
                if let Some(op) = operation {
                    write!(f, " (during: {})", op)?;
                }
                Ok(())
            }
            AppError::FileReadError {
                path,
                source,
                operation,
            } => {
                write!(f, "Failed to read {}: {}", path.display(), source)?;
                if let Some(op) = operation {
                    write!(f, " (during: {})", op)?;
                }
                Ok(())
            }
            AppError::FileWriteError {
                path,
                source,
                operation,
            } => {
                write!(f, "Failed to write {}: {}", path.display(), source)?;
                if let Some(op) = operation {
                    write!(f, " (during: {})", op)?;
                }
                Ok(())
            }
            AppError::InvalidCrf(e) => write!(f, "Invalid CRF: {}", e),
            AppError::InvalidSsim(e) => write!(f, "Invalid SSIM: {}", e),
            AppError::IterationLimitExceeded(e) => write!(f, "{}", e),
            AppError::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path,
            } => {
                write!(f, "FFmpeg error: {}", message)?;
                if let Some(code) = exit_code {
                    write!(f, " (exit code: {})", code)?;
                }
                if let Some(path) = file_path {
                    write!(f, "\n  File: {}", path.display())?;
                }
                if let Some(cmd) = command {
                    write!(f, "\n  Command: {}", cmd)?;
                }
                if !stderr.is_empty() {
                    write!(f, "\n  Stderr: {}", stderr)?;
                }
                Ok(())
            }
            AppError::FfprobeError {
                message,
                stderr,
                command,
                file_path,
            } => {
                write!(f, "FFprobe error: {}", message)?;
                if let Some(path) = file_path {
                    write!(f, "\n  File: {}", path.display())?;
                }
                if let Some(cmd) = command {
                    write!(f, "\n  Command: {}", cmd)?;
                }
                if !stderr.is_empty() {
                    write!(f, "\n  Stderr: {}", stderr)?;
                }
                Ok(())
            }
            AppError::ToolNotFound {
                tool_name,
                operation,
            } => {
                write!(f, "Tool not found: {}", tool_name)?;
                if let Some(op) = operation {
                    write!(f, " (needed for: {})", op)?;
                }
                Ok(())
            }
            AppError::CompressionFailed {
                input_size,
                output_size,
                file_path,
            } => {
                write!(
                    f,
                    "Compression failed: output ({}) >= input ({})",
                    output_size, input_size
                )?;
                if let Some(path) = file_path {
                    write!(f, "\n  File: {}", path.display())?;
                }
                Ok(())
            }
            AppError::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                file_path,
            } => {
                write!(
                    f,
                    "Quality validation failed: expected SSIM >= {:.4}, got {:.4}",
                    expected_ssim, actual_ssim
                )?;
                if let Some(path) = file_path {
                    write!(f, "\n  File: {}", path.display())?;
                }
                Ok(())
            }
            AppError::OutputExists { path, operation } => {
                write!(f, "Output exists: {}", path.display())?;
                if let Some(op) = operation {
                    write!(f, " (during: {})", op)?;
                }
                Ok(())
            }
            AppError::Io(e) => write!(f, "IO error: {}", e),
            AppError::Other(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppError::FileReadError { source, .. } => Some(source),
            AppError::FileWriteError { source, .. } => Some(source),
            AppError::Io(e) => Some(e),
            _ => None,
        }
    }
}


impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<CrfError> for AppError {
    fn from(e: CrfError) -> Self {
        AppError::InvalidCrf(e)
    }
}

impl From<SsimError> for AppError {
    fn from(e: SsimError) -> Self {
        AppError::InvalidSsim(e)
    }
}

impl From<IterationError> for AppError {
    fn from(e: IterationError) -> Self {
        AppError::IterationLimitExceeded(e)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Other(e)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_error_is_recoverable() {
        let error = AppError::FileNotFound {
            path: PathBuf::from("/test"),
            operation: None,
        };
        assert!(error.is_recoverable());

        let error = AppError::CompressionFailed {
            input_size: 1000,
            output_size: 1100,
            file_path: None,
        };
        assert!(error.is_recoverable());
    }

    #[test]
    fn test_app_error_category() {
        let error = AppError::FileNotFound {
            path: PathBuf::from("/test"),
            operation: None,
        };
        assert_eq!(error.category(), ErrorCategory::Fatal);

        let error = AppError::FfmpegError {
            message: "test".to_string(),
            stderr: "".to_string(),
            exit_code: Some(1),
            command: None,
            file_path: None,
        };
        assert_eq!(error.category(), ErrorCategory::Fatal);

        let error = AppError::OutputExists {
            path: PathBuf::from("/test.mp4"),
            operation: None,
        };
        assert_eq!(error.category(), ErrorCategory::Optional);
    }

    #[test]
    fn test_app_error_is_skip() {
        let error = AppError::OutputExists {
            path: PathBuf::from("/test.mp4"),
            operation: None,
        };
        assert!(error.is_skip());

        let error = AppError::FileNotFound {
            path: PathBuf::from("/test"),
            operation: None,
        };
        assert!(!error.is_skip());
    }

    #[test]
    fn test_app_error_user_message() {
        let error = AppError::ToolNotFound {
            tool_name: "ffmpeg".to_string(),
            operation: None,
        };
        let msg = error.user_message();
        assert!(msg.contains("ffmpeg"));
        assert!(msg.contains("PATH"));
    }

    #[test]
    fn test_app_error_from_io() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let app_error: AppError = io_error.into();
        assert!(matches!(app_error, AppError::Io(_)));
    }

    #[test]
    fn test_with_file_path() {
        let error = AppError::CompressionFailed {
            input_size: 1000,
            output_size: 1100,
            file_path: None,
        };
        let error = error.with_file_path("/test/video.mp4");
        let msg = format!("{}", error);
        assert!(msg.contains("/test/video.mp4"));
    }

    #[test]
    fn test_with_operation() {
        let error = AppError::FileNotFound {
            path: PathBuf::from("/test"),
            operation: None,
        };
        let error = error.with_operation("converting to HEVC");
        let msg = format!("{}", error);
        assert!(msg.contains("converting to HEVC"));
    }

    #[test]
    fn test_with_command() {
        let error = AppError::FfmpegError {
            message: "encoding failed".to_string(),
            stderr: "".to_string(),
            exit_code: Some(1),
            command: None,
            file_path: None,
        };
        let error = error.with_command("ffmpeg -i input.mp4 output.mp4");
        let msg = format!("{}", error);
        assert!(msg.contains("ffmpeg -i input.mp4 output.mp4"));
    }
}


#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;


    fn arb_app_error() -> impl Strategy<Value = AppError> {
        prop_oneof![
            any::<String>().prop_map(|s| AppError::FileNotFound {
                path: PathBuf::from(s),
                operation: None,
            }),
            any::<String>().prop_map(|s| AppError::DirectoryNotFound {
                path: PathBuf::from(s),
                operation: None,
            }),
            any::<String>().prop_map(|s| AppError::ToolNotFound {
                tool_name: s,
                operation: None,
            }),
            (any::<u64>(), any::<u64>()).prop_map(|(i, o)| AppError::CompressionFailed {
                input_size: i,
                output_size: o,
                file_path: None,
            }),
            any::<String>().prop_map(|s| AppError::OutputExists {
                path: PathBuf::from(s),
                operation: None,
            }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn app_error_recoverability_property(error in arb_app_error()) {
            prop_assert!(error.is_recoverable(),
                "AppError {:?} should be recoverable", error
            );
        }

        #[test]
        fn app_error_has_category(error in arb_app_error()) {
            let _category = error.category();
        }

        #[test]
        fn app_error_has_user_message(error in arb_app_error()) {
            let msg = error.user_message();
            prop_assert!(!msg.is_empty(),
                "AppError {:?} should have non-empty user message", error
            );
        }
    }
}
