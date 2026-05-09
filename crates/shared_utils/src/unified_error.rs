//! Unified Error Handling Module
//!
//! This module integrates all error handling features:
//! - Error type definitions (`AppError`, `ImgQualityError`, `VidQualityError`)
//! - Error categorization (`ErrorCategory`)
//! - Error handling (`handle_error`, `report_error`)
//! - Error logging (`ErrorSeverity`, `log_enhanced_error`)
//!
//! ## Design Principles
//! - No silent fallback: All errors must be handled explicitly
//! - Transparent diagnostics: Full error chain and contextual information
//! - Unified interface: All error types implement the same interface

use std::fmt;
use std::fmt::Write;
use std::path::PathBuf;

// Re-export types from modules we're keeping
pub use crate::error_handler::{
    ErrorAction, ErrorCategory, ResultExt, add_context, handle_error, install_panic_handler,
    report_error,
};
pub use crate::error_logging::{ErrorSeverity, classify_error, log_enhanced_error};
pub use crate::types::{CrfError, IterationError, SsimError};

// ─── Unified Error Types ─────────────────────────────────────────────────────

/// Master application error type - unified across all modules
#[derive(Debug)]
pub enum UnifiedError {
    // File & I/O errors
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

    // Video-specific errors
    VideoFormatNotSupported(String),
    VideoReadError(String),
    FFprobeError(String),
    FFmpegError {
        message: String,
        stderr: String,
        exit_code: Option<i32>,
        command: Option<String>,
        file_path: Option<PathBuf>,
    },
    ConversionError(String),
    AnalysisError(String),
    GeneralError(String),

    // Image-specific errors
    ImageFormatNotSupported(String),
    ImageReadError(String),
    ImageAnalysisError(String),
    ImageProcessingError(image::ImageError),

    // Validation errors
    InvalidCrf(CrfError),
    InvalidSsim(SsimError),
    IterationLimitExceeded(IterationError),
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

    // Tool errors
    ToolNotFound {
        tool_name: String,
        operation: Option<String>,
    },

    // General errors
    OutputExists {
        path: PathBuf,
        operation: Option<String>,
    },
    Io(std::io::Error),
    NotImplemented(String),
    SkipFile(String),
    ResultAnomaly(String),
    Other(anyhow::Error),
}

impl UnifiedError {
    /// Check if error is recoverable
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        true
    }

    /// Get error category.
    ///
    /// ## Semantics
    /// - `Optional`: Optimization target not met, but original is preserved. (Triggers Skip & Copy)
    /// - `Recoverable`: Non-fatal failure in processing (e.g. analysis error). (Triggers Error & No Copy)
    /// - `Fatal`: System or I/O failure that should stop the batch. (Triggers Error & No Copy)
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            // Priority 1: Fatal (stops or blocks critical flow)
            Self::FileNotFound { .. }
            | Self::DirectoryNotFound { .. }
            | Self::FileWriteError { .. }
            | Self::Io(_)
            | Self::ToolNotFound { .. }
            | Self::NotImplemented(_) => ErrorCategory::Fatal,

            // Priority 2: Optional (Optimization failures -> Skips)
            // These should trigger an automatic copy of the original file to the output.
            Self::OutputExists { .. }
            | Self::SkipFile(_)
            | Self::CompressionFailed { .. }
            | Self::IterationLimitExceeded(_)
            | Self::QualityValidationFailed { .. } => ErrorCategory::Optional,

            // Priority 3: Recoverable (Hard failures in processing -> Errors)
            // These should NOT trigger a copy.
            _ => ErrorCategory::Recoverable,
        }
    }

    /// Get user-friendly error message with emoji indicators
    #[must_use]
    pub fn user_message(&self) -> String {
        if let Some(msg) = self.io_user_message() {
            return msg;
        }
        if let Some(msg) = self.media_user_message() {
            return msg;
        }
        if let Some(msg) = self.ffmpeg_user_message() {
            return msg;
        }
        if let Some(msg) = self.image_user_message() {
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

    fn media_user_message(&self) -> Option<String> {
        match self {
            Self::VideoFormatNotSupported(fmt) => {
                Some(format!("❌ Video format not supported: {fmt}"))
            }
            Self::VideoReadError(err) => Some(format!("❌ Failed to read video: {err}")),
            Self::FFprobeError(err) => Some(format!("❌ FFprobe failed: {err}")),
            Self::ConversionError(err) => Some(format!("❌ Conversion failed: {err}")),
            Self::AnalysisError(err) => Some(format!("❌ Analysis failed: {err}")),
            _ => None,
        }
    }

    fn ffmpeg_user_message(&self) -> Option<String> {
        match self {
            Self::FFmpegError {
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
            _ => None,
        }
    }

    fn image_user_message(&self) -> Option<String> {
        match self {
            Self::ImageFormatNotSupported(fmt) => {
                Some(format!("❌ Image format not supported: {fmt}"))
            }
            Self::ImageReadError(err) => Some(format!("❌ Failed to read image: {err}")),
            Self::ImageAnalysisError(err) => Some(format!("❌ Failed to analyze image: {err}")),
            Self::ImageProcessingError(err) => Some(format!("❌ Image processing error: {err}")),
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
                let mut msg = format!("⏭️  Output file exists: {}", path.display());
                if let Some(op) = operation {
                    write!(msg, "\n   Operation: {op}").expect("String formatting should not fail");
                }
                msg
            }
            Self::NotImplemented(msg) => format!("❌ Not implemented: {msg}"),
            Self::SkipFile(msg) => format!("⏭️  Skip file: {msg}"),
            Self::ResultAnomaly(msg) => format!("❌ Result anomaly: {msg}"),
            Self::GeneralError(err) => format!("❌ Error: {err}"),
            Self::Other(e) => format!("❌ Error: {e}"),
            _ => format!("❌ Unknown error: {self}"),
        }
    }

    /// Check if this error should be treated as a "Skip" (original preserved).
    ///
    /// A Skip means we decided not to (or couldn't) optimize the file, but the
    /// file itself is fine. In a batch with an output directory, Skips trigger
    /// an automatic copy of the original file.
    #[must_use]
    pub const fn is_skip(&self) -> bool {
        matches!(self.category(), ErrorCategory::Optional)
    }

    /// Check if this error should trigger an automatic copy of the original to output.
    ///
    /// Based on the "Loud and Honest" policy:
    /// - Optimization failures (Skips) -> Copy original (ensure complete output set).
    /// - Processing failures (Errors) -> Do NOT copy (avoid silent corruption/partial data).
    #[must_use]
    pub const fn should_copy_original(&self) -> bool {
        self.is_skip()
    }

    /// Add file path to error
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
            Self::FFmpegError {
                message,
                stderr,
                exit_code,
                command,
                ..
            } => Self::FFmpegError {
                message,
                stderr,
                exit_code,
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
            Self::ResultAnomaly(msg) => {
                Self::ResultAnomaly(format!("{msg} (at {})", path.display()))
            }
            other => other,
        }
    }

    /// Add operation context to error
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
            Self::ResultAnomaly(msg) => {
                let op_str = operation.as_deref().unwrap_or("unknown operation");
                Self::ResultAnomaly(format!("{msg} [during {op_str}]"))
            }
            other => other,
        }
    }

    /// Add command to error
    #[must_use]
    pub fn with_command(self, command: impl Into<String>) -> Self {
        let command = Some(command.into());
        match self {
            Self::FFmpegError {
                message,
                stderr,
                exit_code,
                file_path,
                ..
            } => Self::FFmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path,
            },
            other => other,
        }
    }
}

impl UnifiedError {
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
                Self::FFmpegError {
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
                Self::FFprobeError(err) => write!(f, "FFprobe error: {err}"),
                _ => unreachable!(),
            }
        };

        if matches!(self, Self::FFmpegError { .. } | Self::FFprobeError(_)) {
            Some(fmt_closure(f))
        } else {
            None
        }
    }

    fn fmt_image_error(&self, f: &mut fmt::Formatter<'_>) -> Option<fmt::Result> {
        match self {
            Self::ImageFormatNotSupported(fmt) => {
                Some(write!(f, "Image format not supported: {fmt}"))
            }
            Self::ImageReadError(err) => Some(write!(f, "Failed to read image: {err}")),
            Self::ImageAnalysisError(err) => Some(write!(f, "Failed to analyze image: {err}")),
            Self::ImageProcessingError(err) => Some(write!(f, "Image processing error: {err}")),
            _ => None,
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
            Self::NotImplemented(msg) => write!(f, "Not implemented: {msg}"),
            Self::SkipFile(msg) => write!(f, "Skip file: {msg}"),
            Self::ResultAnomaly(msg) => write!(f, "Result anomaly: {msg}"),
            Self::VideoFormatNotSupported(fmt) => write!(f, "Video format not supported: {fmt}"),
            Self::VideoReadError(err) => write!(f, "Failed to read video: {err}"),
            Self::ConversionError(err) => write!(f, "Conversion error: {err}"),
            Self::AnalysisError(err) => write!(f, "Analysis error: {err}"),
            Self::GeneralError(err) => write!(f, "General error: {err}"),
            Self::Other(e) => write!(f, "{e}"),
            _ => write!(f, "{self:?}"),
        }
    }
}

impl fmt::Display for UnifiedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(res) = self.fmt_io_error(f) {
            return res;
        }
        if let Some(res) = self.fmt_ffmpeg_error(f) {
            return res;
        }
        if let Some(res) = self.fmt_image_error(f) {
            return res;
        }
        if let Some(res) = self.fmt_validation_error(f) {
            return res;
        }
        self.fmt_system_error(f)
    }
}

impl std::error::Error for UnifiedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FileReadError { source, .. } | Self::FileWriteError { source, .. } => {
                Some(source)
            }
            Self::Io(e) => Some(e),
            Self::ImageProcessingError(e) => Some(e),
            _ => None,
        }
    }
}

// From implementations for easy conversion
impl From<std::io::Error> for UnifiedError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<anyhow::Error> for UnifiedError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e)
    }
}

impl From<CrfError> for UnifiedError {
    fn from(e: CrfError) -> Self {
        Self::InvalidCrf(e)
    }
}

impl From<SsimError> for UnifiedError {
    fn from(e: SsimError) -> Self {
        Self::InvalidSsim(e)
    }
}

impl From<IterationError> for UnifiedError {
    fn from(e: IterationError) -> Self {
        Self::IterationLimitExceeded(e)
    }
}

impl From<image::ImageError> for UnifiedError {
    fn from(e: image::ImageError) -> Self {
        Self::ImageProcessingError(e)
    }
}

impl From<crate::ffprobe::FFprobeError> for UnifiedError {
    fn from(e: crate::ffprobe::FFprobeError) -> Self {
        match e {
            crate::ffprobe::FFprobeError::ToolNotFound(s) => Self::ToolNotFound {
                tool_name: s,
                operation: Some("video probing".to_string()),
            },
            crate::ffprobe::FFprobeError::IoError(e) => Self::Io(e),
            other => Self::FFprobeError(other.to_string()),
        }
    }
}

// Type aliases for backward compatibility
pub type Result<T> = std::result::Result<T, UnifiedError>;
pub type ImgResult<T> = std::result::Result<T, UnifiedError>;
pub type VidResult<T> = std::result::Result<T, UnifiedError>;

// Legacy type alias for VidQualityError
pub type VidQualityError = UnifiedError;

// Convenience constructors
impl UnifiedError {
    pub fn file_not_found(path: impl Into<PathBuf>) -> Self {
        Self::FileNotFound {
            path: path.into(),
            operation: None,
        }
    }

    pub fn tool_not_found(tool_name: impl Into<String>) -> Self {
        Self::ToolNotFound {
            tool_name: tool_name.into(),
            operation: None,
        }
    }

    pub fn video_not_supported(format: impl Into<String>) -> Self {
        Self::VideoFormatNotSupported(format.into())
    }

    pub fn image_not_supported(format: impl Into<String>) -> Self {
        Self::ImageFormatNotSupported(format.into())
    }

    pub fn not_implemented(msg: impl Into<String>) -> Self {
        Self::NotImplemented(msg.into())
    }

    pub fn skip_file(msg: impl Into<String>) -> Self {
        Self::SkipFile(msg.into())
    }

    pub fn conversion_error(msg: impl Into<String>) -> Self {
        Self::ConversionError(msg.into())
    }

    pub fn analysis_error(msg: impl Into<String>) -> Self {
        Self::AnalysisError(msg.into())
    }

    pub fn general_error(msg: impl Into<String>) -> Self {
        Self::GeneralError(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_error_display() {
        let err = UnifiedError::file_not_found("/test/path");
        assert!(err.to_string().contains("File not found"));
        assert!(err.user_message().contains("❌"));
    }

    #[test]
    fn test_unified_error_category() {
        let err = UnifiedError::file_not_found("/test");
        assert_eq!(err.category(), ErrorCategory::Fatal);

        let err = UnifiedError::CompressionFailed {
            input_size: 1000,
            output_size: 1100,
            file_path: None,
        };
        assert_eq!(err.category(), ErrorCategory::Optional);

        let err = UnifiedError::OutputExists {
            path: PathBuf::from("/test"),
            operation: None,
        };
        assert_eq!(err.category(), ErrorCategory::Optional);
    }

    #[test]
    fn test_unified_error_with_context() {
        let err = UnifiedError::file_not_found("/test")
            .with_operation("reading metadata")
            .with_file_path("/specific/path");

        let msg = err.user_message();
        assert!(msg.contains("reading metadata"));
    }

    #[test]
    fn test_unified_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let err: UnifiedError = io_err.into();
        assert!(matches!(err, UnifiedError::Io(_)));
    }

    #[test]
    fn test_optimization_failure_semantics() {
        // Optimization failures (Iteration limit / Quality threshold) MUST be Optional Skips
        let iter_err = UnifiedError::IterationLimitExceeded(crate::IterationError {
            current: 100,
            max: 100,
            context: "test search".to_string(),
        });
        assert_eq!(
            iter_err.category(),
            ErrorCategory::Optional,
            "Iteration limit should be Optional category"
        );
        assert!(
            iter_err.is_skip(),
            "Iteration limit should trigger is_skip=true"
        );

        let quality_err = UnifiedError::QualityValidationFailed {
            actual_ssim: 0.85,
            expected_ssim: 0.95,
            file_path: None,
        };
        assert_eq!(
            quality_err.category(),
            ErrorCategory::Optional,
            "Quality failure should be Optional category"
        );
        assert!(
            quality_err.is_skip(),
            "Quality failure should trigger is_skip=true"
        );

        // Hard system failures MUST NOT be Optional Skips
        let fatal_err = UnifiedError::file_not_found("/missing");
        assert_eq!(fatal_err.category(), ErrorCategory::Fatal);
        assert!(!fatal_err.is_skip(), "Fatal errors must NOT be skips");

        let tool_err = UnifiedError::tool_not_found("ffmpeg");
        assert_eq!(tool_err.category(), ErrorCategory::Fatal);
        assert!(!tool_err.is_skip(), "Tool missing must NOT be skips");

        let io_err = UnifiedError::Io(std::io::Error::other("disk crash"));
        assert_eq!(io_err.category(), ErrorCategory::Fatal);
        assert!(!io_err.is_skip(), "IO errors must NOT be skips");
    }
}
