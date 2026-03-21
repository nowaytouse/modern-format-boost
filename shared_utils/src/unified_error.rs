//! Unified Error Handling Module - 统一错误处理系统
//!
//! 此模块整合了所有错误处理功能：
//! - 错误类型定义（AppError, ImgQualityError, VidQualityError）
//! - 错误分类（ErrorCategory）
//! - 错误处理（handle_error, report_error）
//! - 错误日志（ErrorSeverity, log_enhanced_error）
//!
//! ## 设计原则
//! - 无静默回退：所有错误必须显式处理
//! - 透明诊断：完整的错误链和上下文信息
//! - 统一接口：所有错误类型实现相同的接口

use std::fmt;
use std::path::PathBuf;

// Re-export types from modules we're keeping
pub use crate::error_handler::{
    add_context, handle_error, install_panic_handler, report_error, ErrorAction, ErrorCategory,
    ResultExt,
};
pub use crate::error_logging::{classify_error, log_enhanced_error, ErrorSeverity};
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
    Other(anyhow::Error),
}

impl UnifiedError {
    /// Check if error is recoverable
    pub fn is_recoverable(&self) -> bool {
        true
    }

    /// Get error category
    pub fn category(&self) -> ErrorCategory {
        match self {
            UnifiedError::FileNotFound { .. }
            | UnifiedError::DirectoryNotFound { .. }
            | UnifiedError::FileReadError { .. }
            | UnifiedError::FileWriteError { .. }
            | UnifiedError::Io(_)
            | UnifiedError::FFprobeError { .. }
            | UnifiedError::FFmpegError { .. }
            | UnifiedError::ToolNotFound { .. } => ErrorCategory::Fatal,

            UnifiedError::InvalidCrf(_)
            | UnifiedError::InvalidSsim(_)
            | UnifiedError::CompressionFailed { .. }
            | UnifiedError::QualityValidationFailed { .. }
            | UnifiedError::IterationLimitExceeded(_) => ErrorCategory::Recoverable,

            UnifiedError::OutputExists { .. } => ErrorCategory::Optional,

            UnifiedError::Other(_) => ErrorCategory::Fatal,

            UnifiedError::VideoFormatNotSupported(_)
            | UnifiedError::VideoReadError(_)
            | UnifiedError::ImageFormatNotSupported(_)
            | UnifiedError::ImageReadError(_)
            | UnifiedError::ImageAnalysisError(_)
            | UnifiedError::ImageProcessingError(_)
            | UnifiedError::NotImplemented(_)
            | UnifiedError::SkipFile(_)
            | UnifiedError::ConversionError(_)
            | UnifiedError::AnalysisError(_)
            | UnifiedError::GeneralError(_) => ErrorCategory::Recoverable,
        }
    }

    /// Get user-friendly error message with emoji indicators
    pub fn user_message(&self) -> String {
        match self {
            UnifiedError::FileNotFound { path, operation } => {
                let mut msg = format!("❌ File not found: {}", path.display());
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Operation: {}", op));
                }
                msg
            }
            UnifiedError::DirectoryNotFound { path, operation } => {
                let mut msg = format!("❌ Directory not found: {}", path.display());
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Operation: {}", op));
                }
                msg
            }
            UnifiedError::FileReadError {
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
            UnifiedError::FileWriteError {
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
            UnifiedError::VideoFormatNotSupported(fmt) => {
                format!("❌ Video format not supported: {}", fmt)
            }
            UnifiedError::VideoReadError(err) => {
                format!("❌ Failed to read video: {}", err)
            }
            UnifiedError::FFprobeError(err) => {
                format!("❌ FFprobe failed: {}", err)
            }
            UnifiedError::FFmpegError {
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
            UnifiedError::ConversionError(err) => {
                format!("❌ Conversion failed: {}", err)
            }
            UnifiedError::AnalysisError(err) => {
                format!("❌ Analysis failed: {}", err)
            }
            UnifiedError::GeneralError(err) => {
                format!("❌ Error: {}", err)
            }
            UnifiedError::ImageFormatNotSupported(fmt) => {
                format!("❌ Image format not supported: {}", fmt)
            }
            UnifiedError::ImageReadError(err) => {
                format!("❌ Failed to read image: {}", err)
            }
            UnifiedError::ImageAnalysisError(err) => {
                format!("❌ Failed to analyze image: {}", err)
            }
            UnifiedError::ImageProcessingError(err) => {
                format!("❌ Image processing error: {}", err)
            }
            UnifiedError::InvalidCrf(e) => {
                format!("❌ Invalid CRF value: {}", e)
            }
            UnifiedError::InvalidSsim(e) => {
                format!("❌ Invalid SSIM value: {}", e)
            }
            UnifiedError::IterationLimitExceeded(e) => {
                format!("⚠️ Iteration limit exceeded: {}", e)
            }
            UnifiedError::ToolNotFound {
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
            UnifiedError::CompressionFailed {
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
            UnifiedError::QualityValidationFailed {
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
            UnifiedError::OutputExists { path, operation } => {
                let mut msg = format!("⏭️  Output file exists: {}", path.display());
                if let Some(op) = operation {
                    msg.push_str(&format!("\n   Operation: {}", op));
                }
                msg
            }
            UnifiedError::Io(e) => {
                format!("❌ IO error: {}", e)
            }
            UnifiedError::NotImplemented(msg) => {
                format!("❌ Not implemented: {}", msg)
            }
            UnifiedError::SkipFile(msg) => {
                format!("⏭️  Skip file: {}", msg)
            }
            UnifiedError::Other(e) => {
                format!("❌ Error: {}", e)
            }
        }
    }

    /// Check if this error should skip the file
    pub fn is_skip(&self) -> bool {
        matches!(
            self,
            UnifiedError::OutputExists { .. } | UnifiedError::SkipFile(_)
        )
    }

    /// Add file path to error
    pub fn with_file_path(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match self {
            UnifiedError::FileNotFound { operation, .. } => {
                UnifiedError::FileNotFound { path, operation }
            }
            UnifiedError::FileReadError {
                source, operation, ..
            } => UnifiedError::FileReadError {
                path,
                source,
                operation,
            },
            UnifiedError::FileWriteError {
                source, operation, ..
            } => UnifiedError::FileWriteError {
                path,
                source,
                operation,
            },
            UnifiedError::DirectoryNotFound { operation, .. } => {
                UnifiedError::DirectoryNotFound { path, operation }
            }
            UnifiedError::FFmpegError {
                message,
                stderr,
                exit_code,
                command,
                ..
            } => UnifiedError::FFmpegError {
                message,
                stderr,
                exit_code,
                command,
                file_path: Some(path),
            },
            UnifiedError::CompressionFailed {
                input_size,
                output_size,
                ..
            } => UnifiedError::CompressionFailed {
                input_size,
                output_size,
                file_path: Some(path),
            },
            UnifiedError::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                ..
            } => UnifiedError::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                file_path: Some(path),
            },
            UnifiedError::OutputExists { operation, .. } => {
                UnifiedError::OutputExists { path, operation }
            }
            other => other,
        }
    }

    /// Add operation context to error
    pub fn with_operation(self, operation: impl Into<String>) -> Self {
        let operation = Some(operation.into());
        match self {
            UnifiedError::FileNotFound { path, .. } => {
                UnifiedError::FileNotFound { path, operation }
            }
            UnifiedError::FileReadError { path, source, .. } => UnifiedError::FileReadError {
                path,
                source,
                operation,
            },
            UnifiedError::FileWriteError { path, source, .. } => UnifiedError::FileWriteError {
                path,
                source,
                operation,
            },
            UnifiedError::DirectoryNotFound { path, .. } => {
                UnifiedError::DirectoryNotFound { path, operation }
            }
            UnifiedError::ToolNotFound { tool_name, .. } => UnifiedError::ToolNotFound {
                tool_name,
                operation,
            },
            UnifiedError::OutputExists { path, .. } => {
                UnifiedError::OutputExists { path, operation }
            }
            other => other,
        }
    }

    /// Add command to error
    pub fn with_command(self, command: impl Into<String>) -> Self {
        let command = Some(command.into());
        match self {
            UnifiedError::FFmpegError {
                message,
                stderr,
                exit_code,
                file_path,
                ..
            } => UnifiedError::FFmpegError {
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

impl fmt::Display for UnifiedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnifiedError::FileNotFound { path, operation } => {
                write!(f, "File not found: {}", path.display())?;
                if let Some(op) = operation {
                    write!(f, " (during: {})", op)?;
                }
                Ok(())
            }
            UnifiedError::DirectoryNotFound { path, operation } => {
                write!(f, "Directory not found: {}", path.display())?;
                if let Some(op) = operation {
                    write!(f, " (during: {})", op)?;
                }
                Ok(())
            }
            UnifiedError::FileReadError {
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
            UnifiedError::FileWriteError {
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
            UnifiedError::VideoFormatNotSupported(fmt) => {
                write!(f, "Video format not supported: {}", fmt)
            }
            UnifiedError::VideoReadError(err) => write!(f, "Failed to read video: {}", err),
            UnifiedError::FFprobeError(err) => write!(f, "FFprobe error: {}", err),
            UnifiedError::FFmpegError {
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
            UnifiedError::ConversionError(err) => write!(f, "Conversion error: {}", err),
            UnifiedError::AnalysisError(err) => write!(f, "Analysis error: {}", err),
            UnifiedError::GeneralError(err) => write!(f, "General error: {}", err),
            UnifiedError::ImageFormatNotSupported(fmt) => {
                write!(f, "Image format not supported: {}", fmt)
            }
            UnifiedError::ImageReadError(err) => write!(f, "Failed to read image: {}", err),
            UnifiedError::ImageAnalysisError(err) => write!(f, "Failed to analyze image: {}", err),
            UnifiedError::ImageProcessingError(err) => {
                write!(f, "Image processing error: {}", err)
            }
            UnifiedError::InvalidCrf(e) => write!(f, "Invalid CRF: {}", e),
            UnifiedError::InvalidSsim(e) => write!(f, "Invalid SSIM: {}", e),
            UnifiedError::IterationLimitExceeded(e) => write!(f, "{}", e),
            UnifiedError::ToolNotFound {
                tool_name,
                operation,
            } => {
                write!(f, "Tool not found: {}", tool_name)?;
                if let Some(op) = operation {
                    write!(f, " (needed for: {})", op)?;
                }
                Ok(())
            }
            UnifiedError::CompressionFailed {
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
            UnifiedError::QualityValidationFailed {
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
            UnifiedError::OutputExists { path, operation } => {
                write!(f, "Output exists: {}", path.display())?;
                if let Some(op) = operation {
                    write!(f, " (during: {})", op)?;
                }
                Ok(())
            }
            UnifiedError::Io(e) => write!(f, "IO error: {}", e),
            UnifiedError::NotImplemented(msg) => write!(f, "Not implemented: {}", msg),
            UnifiedError::SkipFile(msg) => write!(f, "Skip file: {}", msg),
            UnifiedError::Other(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for UnifiedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            UnifiedError::FileReadError { source, .. } => Some(source),
            UnifiedError::FileWriteError { source, .. } => Some(source),
            UnifiedError::Io(e) => Some(e),
            UnifiedError::ImageProcessingError(e) => Some(e),
            _ => None,
        }
    }
}

// From implementations for easy conversion
impl From<std::io::Error> for UnifiedError {
    fn from(e: std::io::Error) -> Self {
        UnifiedError::Io(e)
    }
}

impl From<anyhow::Error> for UnifiedError {
    fn from(e: anyhow::Error) -> Self {
        UnifiedError::Other(e)
    }
}

impl From<CrfError> for UnifiedError {
    fn from(e: CrfError) -> Self {
        UnifiedError::InvalidCrf(e)
    }
}

impl From<SsimError> for UnifiedError {
    fn from(e: SsimError) -> Self {
        UnifiedError::InvalidSsim(e)
    }
}

impl From<IterationError> for UnifiedError {
    fn from(e: IterationError) -> Self {
        UnifiedError::IterationLimitExceeded(e)
    }
}

impl From<image::ImageError> for UnifiedError {
    fn from(e: image::ImageError) -> Self {
        UnifiedError::ImageProcessingError(e)
    }
}

impl From<crate::ffprobe::FFprobeError> for UnifiedError {
    fn from(e: crate::ffprobe::FFprobeError) -> Self {
        match e {
            crate::ffprobe::FFprobeError::ToolNotFound(s) => UnifiedError::ToolNotFound {
                tool_name: s,
                operation: Some("video probing".to_string()),
            },
            crate::ffprobe::FFprobeError::IoError(e) => UnifiedError::Io(e),
            other => UnifiedError::FFprobeError(other.to_string()),
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
        UnifiedError::FileNotFound {
            path: path.into(),
            operation: None,
        }
    }

    pub fn tool_not_found(tool_name: impl Into<String>) -> Self {
        UnifiedError::ToolNotFound {
            tool_name: tool_name.into(),
            operation: None,
        }
    }

    pub fn video_not_supported(format: impl Into<String>) -> Self {
        UnifiedError::VideoFormatNotSupported(format.into())
    }

    pub fn image_not_supported(format: impl Into<String>) -> Self {
        UnifiedError::ImageFormatNotSupported(format.into())
    }

    pub fn not_implemented(msg: impl Into<String>) -> Self {
        UnifiedError::NotImplemented(msg.into())
    }

    pub fn skip_file(msg: impl Into<String>) -> Self {
        UnifiedError::SkipFile(msg.into())
    }

    pub fn conversion_error(msg: impl Into<String>) -> Self {
        UnifiedError::ConversionError(msg.into())
    }

    pub fn analysis_error(msg: impl Into<String>) -> Self {
        UnifiedError::AnalysisError(msg.into())
    }

    pub fn general_error(msg: impl Into<String>) -> Self {
        UnifiedError::GeneralError(msg.into())
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
        assert_eq!(err.category(), ErrorCategory::Recoverable);

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
    fn test_unified_error_convenience_constructors() {
        let err = UnifiedError::tool_not_found("ffmpeg");
        assert!(matches!(err, UnifiedError::ToolNotFound { .. }));

        let err = UnifiedError::video_not_supported("avi");
        assert!(matches!(err, UnifiedError::VideoFormatNotSupported(_)));

        let err = UnifiedError::image_not_supported("tiff");
        assert!(matches!(err, UnifiedError::ImageFormatNotSupported(_)));
    }
}
