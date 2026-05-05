//! `AppError` - Unified application error type
//!
//! Provides clear error categorization, distinguishing between recoverable and non-recoverable errors.

use crate::error_handler::ErrorCategory;
use crate::types::{CrfError, IterationError, SsimError};
use std::fmt;
use std::fmt::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub enum AppError {
    /// Input file was not found.
    FileNotFound {
        path: PathBuf,
        operation: Option<String>,
    },

    /// Failed to read an existing file.
    FileReadError {
        path: PathBuf,
        source: std::io::Error,
        operation: Option<String>,
    },

    /// Failed to write to a file.
    FileWriteError {
        path: PathBuf,
        source: std::io::Error,
        operation: Option<String>,
    },

    /// Directory was not found.
    DirectoryNotFound {
        path: PathBuf,
        operation: Option<String>,
    },

    /// The provided CRF (Constant Rate Factor) value is invalid.
    InvalidCrf(CrfError),

    /// The provided SSIM threshold is invalid or out of range.
    InvalidSsim(SsimError),

    /// The search algorithm exceeded the maximum number of iterations.
    IterationLimitExceeded(IterationError),

    /// An error occurred during `FFmpeg` execution.
    FfmpegError {
        message: String,
        stderr: String,
        exit_code: Option<i32>,
        command: Option<String>,
        file_path: Option<PathBuf>,
    },

    /// An error occurred during `FFprobe` execution.
    FfprobeError {
        message: String,
        stderr: String,
        command: Option<String>,
        file_path: Option<PathBuf>,
    },

    /// An external tool (e.g. `ffmpeg`, `ffprobe`) was not found in PATH.
    ToolNotFound {
        tool_name: String,
        operation: Option<String>,
    },

    /// The conversion failed because the output size was not smaller than the input size.
    CompressionFailed {
        input_size: u64,
        output_size: u64,
        file_path: Option<PathBuf>,
    },

    /// The output quality did not meet the required threshold.
    QualityValidationFailed {
        expected_ssim: f64,
        actual_ssim: f64,
        file_path: Option<PathBuf>,
    },

    /// The output file already exists and will not be overwritten.
    OutputExists {
        path: PathBuf,
        operation: Option<String>,
    },

    /// Generic I/O error.
    Io(std::io::Error),

    /// Catch-all for other errors.
    Other(anyhow::Error),
}

impl AppError {
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        true
    }

    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::FileNotFound { .. }
            | Self::DirectoryNotFound { .. }
            | Self::FileReadError { .. }
            | Self::FileWriteError { .. }
            | Self::Io(_)
            | Self::FfmpegError { .. }
            | Self::FfprobeError { .. }
            | Self::ToolNotFound { .. }
            | Self::Other(_) => ErrorCategory::Fatal,

            Self::InvalidCrf(_)
            | Self::InvalidSsim(_)
            | Self::CompressionFailed { .. }
            | Self::QualityValidationFailed { .. }
            | Self::IterationLimitExceeded(_) => ErrorCategory::Recoverable,

            Self::OutputExists { .. } => ErrorCategory::Optional,
        }
    }

    #[must_use]
    pub fn user_message(&self) -> String {
        if let Some(msg) = self.io_user_message() {
            return msg;
        }
        if let Some(msg) = self.ffmpeg_user_message() {
            return msg;
        }
        if let Some(msg) = self.validation_user_message() {
            return msg;
        }
        self.system_user_message()
    }

    fn io_user_message(&self) -> Option<String> {
        match self {
            Self::FileNotFound { path, operation } => {
                let mut msg = format!("❌ File not found: {}", path.display());
                if let Some(op) = operation {
                    write!(msg, "\n   Operation: {op}").expect("String formatting should not fail");
                }
                Some(msg)
            }
            Self::DirectoryNotFound { path, operation } => {
                let mut msg = format!("❌ Directory not found: {}", path.display());
                if let Some(op) = operation {
                    write!(msg, "\n   Operation: {op}").expect("String formatting should not fail");
                }
                Some(msg)
            }
            Self::FileReadError {
                path,
                source,
                operation,
            } => {
                let mut msg = format!("❌ Failed to read file {}: {}", path.display(), source);
                if let Some(op) = operation {
                    write!(msg, "\n   Operation: {op}").expect("String formatting should not fail");
                }
                Some(msg)
            }
            Self::FileWriteError {
                path,
                source,
                operation,
            } => {
                let mut msg = format!("❌ Failed to write file {}: {}", path.display(), source);
                if let Some(op) = operation {
                    write!(msg, "\n   Operation: {op}").expect("String formatting should not fail");
                }
                Some(msg)
            }
            Self::Io(e) => Some(format!("❌ IO error: {e}")),
            _ => None,
        }
    }

    fn ffmpeg_user_message(&self) -> Option<String> {
        match self {
            Self::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path,
            } => {
                let code_str = exit_code
                    .map(|c| format!(" (exit code: {c})"))
                    .unwrap_or_default();
                let mut msg = format!("❌ FFmpeg failed{code_str}: {message}");
                if let Some(path) = file_path {
                    write!(msg, "\n   File: {}", path.display())
                        .expect("String formatting should not fail");
                }
                if let Some(cmd) = command {
                    write!(msg, "\n   Command: {cmd}").expect("String formatting should not fail");
                }
                if !stderr.is_empty() {
                    write!(msg, "\n   Error output: {stderr}")
                        .expect("String formatting should not fail");
                }
                Some(msg)
            }
            Self::FfprobeError {
                message,
                stderr,
                command,
                file_path,
            } => {
                let mut msg = format!("❌ FFprobe failed: {message}");
                if let Some(path) = file_path {
                    write!(msg, "\n   File: {}", path.display())
                        .expect("String formatting should not fail");
                }
                if let Some(cmd) = command {
                    write!(msg, "\n   Command: {cmd}").expect("String formatting should not fail");
                }
                if !stderr.is_empty() {
                    write!(msg, "\n   Error output: {stderr}")
                        .expect("String formatting should not fail");
                }
                Some(msg)
            }
            _ => None,
        }
    }

    fn validation_user_message(&self) -> Option<String> {
        match self {
            Self::InvalidCrf(e) => Some(format!("❌ Invalid CRF value: {e}")),
            Self::InvalidSsim(e) => Some(format!("❌ Invalid SSIM value: {e}")),
            Self::IterationLimitExceeded(e) => Some(format!("⚠️ Iteration limit exceeded: {e}")),
            Self::CompressionFailed {
                input_size,
                output_size,
                file_path,
            } => {
                let ratio = crate::numeric_cast::u64_to_f64(*output_size)
                    / crate::numeric_cast::u64_to_f64(*input_size)
                    * 100.0;
                let mut msg = format!(
                    "❌ Compression failed: output ({output_size} bytes) >= input ({input_size} bytes), ratio {ratio:.1}%"
                );
                if let Some(path) = file_path {
                    write!(msg, "\n   File: {}", path.display())
                        .expect("String formatting should not fail");
                }
                Some(msg)
            }
            Self::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                file_path,
            } => {
                let mut msg = format!(
                    "❌ Quality validation failed: expected SSIM >= {expected_ssim:.4}, actual {actual_ssim:.4}"
                );
                if let Some(path) = file_path {
                    write!(msg, "\n   File: {}", path.display())
                        .expect("String formatting should not fail");
                }
                Some(msg)
            }
            _ => None,
        }
    }

    fn system_user_message(&self) -> String {
        match self {
            Self::ToolNotFound {
                tool_name,
                operation,
            } => {
                let mut msg = format!(
                    "❌ Tool not found: {tool_name}\n💡 Please ensure {tool_name} is installed and in PATH"
                );
                if let Some(op) = operation {
                    write!(msg, "\n   Needed for: {op}")
                        .expect("String formatting should not fail");
                }
                msg
            }
            Self::OutputExists { path, operation } => {
                let mut msg = format!("⏭️ Output file exists: {}", path.display());
                if let Some(op) = operation {
                    write!(msg, "\n   Operation: {op}").expect("String formatting should not fail");
                }
                msg
            }
            Self::Other(e) => format!("❌ Error: {e}"),
            _ => format!("❌ Unknown error: {self}"),
        }
    }

    #[must_use]
    pub const fn is_skip(&self) -> bool {
        matches!(self, Self::OutputExists { .. })
    }

    #[must_use]
    pub fn with_file_path(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match self {
            Self::FileNotFound { operation, .. } => Self::FileNotFound { path, operation },
            Self::FileReadError {
                source, operation, ..
            } => Self::FileReadError {
                path,
                source,
                operation,
            },
            Self::FileWriteError {
                source, operation, ..
            } => Self::FileWriteError {
                path,
                source,
                operation,
            },
            Self::DirectoryNotFound { operation, .. } => {
                Self::DirectoryNotFound { path, operation }
            }
            Self::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                ..
            } => Self::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path: Some(path),
            },
            Self::FfprobeError {
                message,
                stderr,
                command,
                ..
            } => Self::FfprobeError {
                message,
                stderr,
                command,
                file_path: Some(path),
            },
            Self::CompressionFailed {
                input_size,
                output_size,
                ..
            } => Self::CompressionFailed {
                input_size,
                output_size,
                file_path: Some(path),
            },
            Self::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                ..
            } => Self::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                file_path: Some(path),
            },
            Self::OutputExists { operation, .. } => Self::OutputExists { path, operation },
            other => other,
        }
    }

    #[must_use]
    pub fn with_operation(self, operation: impl Into<String>) -> Self {
        let operation = Some(operation.into());
        match self {
            Self::FileNotFound { path, .. } => Self::FileNotFound { path, operation },
            Self::FileReadError { path, source, .. } => Self::FileReadError {
                path,
                source,
                operation,
            },
            Self::FileWriteError { path, source, .. } => Self::FileWriteError {
                path,
                source,
                operation,
            },
            Self::DirectoryNotFound { path, .. } => Self::DirectoryNotFound { path, operation },
            Self::ToolNotFound { tool_name, .. } => Self::ToolNotFound {
                tool_name,
                operation,
            },
            Self::OutputExists { path, .. } => Self::OutputExists { path, operation },
            other => other,
        }
    }

    #[must_use]
    pub fn with_command(self, command: impl Into<String>) -> Self {
        let command = Some(command.into());
        match self {
            Self::FfmpegError {
                message,
                stderr,
                exit_code,
                file_path,
                ..
            } => Self::FfmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path,
            },
            Self::FfprobeError {
                message,
                stderr,
                file_path,
                ..
            } => Self::FfprobeError {
                message,
                stderr,
                command,
                file_path,
            },
            other => other,
        }
    }
}

impl AppError {
    fn fmt_io_error(&self, f: &mut fmt::Formatter<'_>) -> Option<fmt::Result> {
        let fmt_closure = |f: &mut fmt::Formatter<'_>| -> fmt::Result {
            match self {
                Self::FileNotFound { path, operation } => {
                    write!(f, "File not found: {}", path.display())?;
                    if let Some(op) = operation {
                        write!(f, " (during: {op})")?;
                    }
                    Ok(())
                }
                Self::DirectoryNotFound { path, operation } => {
                    write!(f, "Directory not found: {}", path.display())?;
                    if let Some(op) = operation {
                        write!(f, " (during: {op})")?;
                    }
                    Ok(())
                }
                Self::FileReadError {
                    path,
                    source,
                    operation,
                } => {
                    write!(f, "Failed to read {}: {}", path.display(), source)?;
                    if let Some(op) = operation {
                        write!(f, " (during: {op})")?;
                    }
                    Ok(())
                }
                Self::FileWriteError {
                    path,
                    source,
                    operation,
                } => {
                    write!(f, "Failed to write {}: {}", path.display(), source)?;
                    if let Some(op) = operation {
                        write!(f, " (during: {op})")?;
                    }
                    Ok(())
                }
                Self::Io(e) => write!(f, "IO error: {e}"),
                _ => unreachable!(),
            }
        };

        if matches!(
            self,
            Self::FileNotFound { .. }
                | Self::DirectoryNotFound { .. }
                | Self::FileReadError { .. }
                | Self::FileWriteError { .. }
                | Self::Io(_)
        ) {
            Some(fmt_closure(f))
        } else {
            None
        }
    }

    fn fmt_ffmpeg_error(&self, f: &mut fmt::Formatter<'_>) -> Option<fmt::Result> {
        let fmt_closure = |f: &mut fmt::Formatter<'_>| -> fmt::Result {
            match self {
                Self::FfmpegError {
                    message,
                    stderr,
                    exit_code,
                    command,
                    file_path,
                } => {
                    write!(f, "FFmpeg error: {message}")?;
                    if let Some(code) = exit_code {
                        write!(f, " (exit code: {code})")?;
                    }
                    if let Some(path) = file_path {
                        write!(f, "\n  File: {}", path.display())?;
                    }
                    if let Some(cmd) = command {
                        write!(f, "\n  Command: {cmd}")?;
                    }
                    if !stderr.is_empty() {
                        write!(f, "\n  Stderr: {stderr}")?;
                    }
                    Ok(())
                }
                Self::FfprobeError {
                    message,
                    stderr,
                    command,
                    file_path,
                } => {
                    write!(f, "FFprobe error: {message}")?;
                    if let Some(path) = file_path {
                        write!(f, "\n  File: {}", path.display())?;
                    }
                    if let Some(cmd) = command {
                        write!(f, "\n  Command: {cmd}")?;
                    }
                    if !stderr.is_empty() {
                        write!(f, "\n  Stderr: {stderr}")?;
                    }
                    Ok(())
                }
                _ => unreachable!(),
            }
        };

        if matches!(self, Self::FfmpegError { .. } | Self::FfprobeError { .. }) {
            Some(fmt_closure(f))
        } else {
            None
        }
    }

    fn fmt_validation_error(&self, f: &mut fmt::Formatter<'_>) -> Option<fmt::Result> {
        let fmt_closure = |f: &mut fmt::Formatter<'_>| -> fmt::Result {
            match self {
                Self::InvalidCrf(e) => write!(f, "Invalid CRF: {e}"),
                Self::InvalidSsim(e) => write!(f, "Invalid SSIM: {e}"),
                Self::IterationLimitExceeded(e) => write!(f, "{e}"),
                Self::CompressionFailed {
                    input_size,
                    output_size,
                    file_path,
                } => {
                    write!(
                        f,
                        "Compression failed: output ({output_size}) >= input ({input_size})"
                    )?;
                    if let Some(path) = file_path {
                        write!(f, "\n  File: {}", path.display())?;
                    }
                    Ok(())
                }
                Self::QualityValidationFailed {
                    expected_ssim,
                    actual_ssim,
                    file_path,
                } => {
                    write!(
                        f,
                        "Quality validation failed: expected SSIM >= {expected_ssim:.4}, got {actual_ssim:.4}"
                    )?;
                    if let Some(path) = file_path {
                        write!(f, "\n  File: {}", path.display())?;
                    }
                    Ok(())
                }
                _ => unreachable!(),
            }
        };

        if matches!(
            self,
            Self::InvalidCrf(_)
                | Self::InvalidSsim(_)
                | Self::IterationLimitExceeded(_)
                | Self::CompressionFailed { .. }
                | Self::QualityValidationFailed { .. }
        ) {
            Some(fmt_closure(f))
        } else {
            None
        }
    }

    fn fmt_system_error(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolNotFound {
                tool_name,
                operation,
            } => {
                write!(f, "Tool not found: {tool_name}")?;
                if let Some(op) = operation {
                    write!(f, " (needed for: {op})")?;
                }
                Ok(())
            }
            Self::OutputExists { path, operation } => {
                write!(f, "Output exists: {}", path.display())?;
                if let Some(op) = operation {
                    write!(f, " (during: {op})")?;
                }
                Ok(())
            }
            Self::Other(e) => write!(f, "{e}"),
            _ => write!(f, "{self:?}"),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(res) = self.fmt_io_error(f) {
            return res;
        }
        if let Some(res) = self.fmt_ffmpeg_error(f) {
            return res;
        }
        if let Some(res) = self.fmt_validation_error(f) {
            return res;
        }
        self.fmt_system_error(f)
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FileReadError { source, .. }
            | Self::FileWriteError { source, .. }
            | Self::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<CrfError> for AppError {
    fn from(e: CrfError) -> Self {
        Self::InvalidCrf(e)
    }
}

impl From<SsimError> for AppError {
    fn from(e: SsimError) -> Self {
        Self::InvalidSsim(e)
    }
}

impl From<IterationError> for AppError {
    fn from(e: IterationError) -> Self {
        Self::IterationLimitExceeded(e)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e)
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
            stderr: String::new(),
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
        let msg = format!("{error}");
        assert!(msg.contains("/test/video.mp4"));
    }

    #[test]
    fn test_with_operation() {
        let error = AppError::FileNotFound {
            path: PathBuf::from("/test"),
            operation: None,
        };
        let error = error.with_operation("converting to HEVC");
        let msg = format!("{error}");
        assert!(msg.contains("converting to HEVC"));
    }

    #[test]
    fn test_with_command() {
        let error = AppError::FfmpegError {
            message: "encoding failed".to_string(),
            stderr: String::new(),
            exit_code: Some(1),
            command: None,
            file_path: None,
        };
        let error = error.with_command("ffmpeg -i input.mp4 output.mp4");
        let msg = format!("{error}");
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
