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
use std::fmt::Write as _;
use std::path::PathBuf;

/// Batch error handling shared by drag/drop, image, video, and Photos import.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BatchErrorMode {
    #[default]
    LogAndContinue,
    FailFast,
}

impl BatchErrorMode {
    #[must_use]
    pub fn current() -> Self {
        let legacy_fail_fast = match std::env::var(crate::constants::ENV_MFB_DRAG_DROP_FAIL_FAST) {
            Ok(value) => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(std::env::VarError::NotPresent) => false,
            Err(error) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "batch_error_mode_env",
                    format!(
                        "failed to read {}: {error}; using fail-fast",
                        crate::constants::ENV_MFB_DRAG_DROP_FAIL_FAST
                    ),
                );
                true
            }
        };
        if legacy_fail_fast {
            return Self::FailFast;
        }

        for name in [
            crate::constants::ENV_MFB_ERROR_MODE,
            crate::constants::ENV_MFB_DRAG_DROP_ERROR_MODE,
        ] {
            match std::env::var(name) {
                Ok(value) => return Self::parse(&value),
                Err(std::env::VarError::NotPresent) => {}
                Err(error) => {
                    crate::media_conversion_gate::delivery_runtime_batch_audit(
                        "batch_error_mode_env",
                        format!("failed to read {name}: {error}; using fail-fast"),
                    );
                    return Self::FailFast;
                }
            }
        }
        Self::LogAndContinue
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        let normalized = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        match normalized.as_str() {
            "debug" | "fail-fast" | "failfast" | "abort" | "strict" => Self::FailFast,
            "" | "continue" | "log-and-continue" | "batch-report" | "report" | "normal" => {
                Self::LogAndContinue
            }
            _ => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "batch_error_mode_invalid",
                    format!("unknown batch error mode {value:?}; using fail-fast"),
                );
                Self::FailFast
            }
        }
    }

    #[must_use]
    pub const fn is_fail_fast(self) -> bool {
        matches!(self, Self::FailFast)
    }

    /// Fatal system errors always stop; ordinary per-file errors stop only in
    /// fail-fast mode. Optional outcomes remain skips in both modes.
    #[must_use]
    pub fn should_abort_error(self, error: &anyhow::Error) -> bool {
        let unified = error.downcast_ref::<UnifiedError>().or_else(|| {
            error
                .chain()
                .find_map(|cause| cause.downcast_ref::<UnifiedError>())
        });
        match unified.map(UnifiedError::category) {
            Some(ErrorCategory::Optional) => false,
            Some(ErrorCategory::Recoverable) => self.is_fail_fast(),
            Some(ErrorCategory::Fatal) | None => true,
        }
    }
}

#[inline]
fn append_operation_line(msg: &mut String, operation: &str) {
    let _ = write!(msg, "\n   Operation: {operation}");
}

#[inline]
fn append_file_line(msg: &mut String, path: &std::path::Path) {
    let _ = write!(msg, "\n   File: {}", path.display());
}

#[inline]
fn user_err(detail: impl fmt::Display) -> String {
    crate::media_conversion_gate::ui_user_facing_error(detail)
}
#[inline]
fn user_warn(detail: impl fmt::Display) -> String {
    crate::media_conversion_gate::ui_user_facing_warning(detail)
}

// Re-export types from modules we're keeping
pub use crate::error_handler::{
    ErrorAction, ErrorCategory, ResultExt, add_context, handle_error, install_panic_handler,
    report_error,
};
pub use crate::infra::static_logs::{ErrorSeverity, classify_error, log_enhanced_error};
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
    IoError(std::io::Error),
    NotImplemented(String),
    SkipFile(String),
    ResultAnomaly(String),
    // Arithmetic & Calculation Errors
    NumericError(String),
    NumericOverflow(String),

    Other(anyhow::Error),
}

impl UnifiedError {
    /// Check if error is recoverable
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(self.category(), ErrorCategory::Recoverable)
    }

    /// Get error category.
    ///
    /// ## Semantics
    /// - `Optional`: Optimization target not met, but original is preserved.
    ///   (Triggers Skip & Copy)
    /// - `Recoverable`: Non-fatal failure in processing (e.g. analysis error).
    ///   (Triggers Error & No Copy)
    /// - `Fatal`: System or I/O failure that should stop the batch. (Triggers
    ///   Error & No Copy)
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            // Priority 1: Fatal (stops or blocks critical flow)
            Self::FileNotFound { .. }
            | Self::DirectoryNotFound { .. }
            | Self::FileWriteError { .. }
            | Self::IoError(_)
            | Self::ToolNotFound { .. }
            | Self::NotImplemented(_)
            | Self::Other(_) => ErrorCategory::Fatal,

            // Priority 2: Optional (Optimization failures -> Skips)
            // These should trigger an automatic copy of the original file to the output.
            Self::OutputExists { .. }
            | Self::SkipFile(_)
            | Self::CompressionFailed { .. }
            | Self::IterationLimitExceeded(_) => ErrorCategory::Optional,

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
                let mut msg = user_err(format!("File not found: {}", path.display()));
                if let Some(op) = operation {
                    append_operation_line(&mut msg, op);
                }
                Some(msg)
            }
            Self::DirectoryNotFound { path, operation } => {
                let mut msg = user_err(format!("Directory not found: {}", path.display()));
                if let Some(op) = operation {
                    append_operation_line(&mut msg, op);
                }
                Some(msg)
            }
            Self::FileReadError {
                path,
                source,
                operation,
            } => {
                let mut msg = user_err(format!("Failed to read file {}: {source}", path.display()));
                if let Some(op) = operation {
                    append_operation_line(&mut msg, op);
                }
                Some(msg)
            }
            Self::FileWriteError {
                path,
                source,
                operation,
            } => {
                let mut msg =
                    user_err(format!("Failed to write file {}: {source}", path.display()));
                if let Some(op) = operation {
                    append_operation_line(&mut msg, op);
                }
                Some(msg)
            }
            Self::IoError(e) => Some(user_err(format!("IO error: {e}"))),
            _ => None,
        }
    }

    fn media_user_message(&self) -> Option<String> {
        match self {
            Self::VideoFormatNotSupported(fmt) => {
                Some(user_err(format!("Video format not supported: {fmt}")))
            }
            Self::VideoReadError(err) => Some(user_err(format!("Failed to read video: {err}"))),
            Self::FFprobeError(err) => Some(user_err(format!("FFprobe failed: {err}"))),
            Self::ConversionError(err) => Some(user_err(format!("Conversion failed: {err}"))),
            Self::AnalysisError(err) => Some(user_err(format!("Analysis failed: {err}"))),
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
                let code_str = crate::media_conversion_gate::ui_exit_code_suffix_or_empty(
                    *exit_code,
                    "unified_error FFmpeg user message",
                );
                let mut msg = user_err(format!("FFmpeg failed{code_str}: {message}"));
                if let Some(path) = file_path {
                    append_file_line(&mut msg, path);
                }
                if let Some(cmd) = command {
                    let _ = write!(msg, "\n   Command: {cmd}");
                }
                if !stderr.is_empty() {
                    let _ = write!(msg, "\n   Error output: {stderr}");
                }
                Some(msg)
            }
            _ => None,
        }
    }

    fn image_user_message(&self) -> Option<String> {
        match self {
            Self::ImageFormatNotSupported(fmt) => {
                Some(user_err(format!("Image format not supported: {fmt}")))
            }
            Self::ImageReadError(err) => Some(user_err(format!("Failed to read image: {err}"))),
            Self::ImageAnalysisError(err) => {
                Some(user_err(format!("Failed to analyze image: {err}")))
            }
            Self::ImageProcessingError(err) => {
                Some(user_err(format!("Image processing error: {err}")))
            }
            _ => None,
        }
    }

    fn validation_user_message(&self) -> Option<String> {
        match self {
            Self::InvalidCrf(e) => Some(user_err(format!("Invalid CRF value: {e}"))),
            Self::InvalidSsim(e) => Some(user_err(format!("Invalid SSIM value: {e}"))),
            Self::IterationLimitExceeded(e) => {
                Some(user_warn(format!("Iteration limit exceeded: {e}")))
            }
            Self::CompressionFailed {
                input_size,
                output_size,
                file_path,
            } => {
                let ratio = crate::numeric_cast::u64_to_f64(*output_size)
                    / crate::numeric_cast::u64_to_f64(*input_size)
                    * 100.0;
                let mut msg = user_err(format!(
                    "Compression target not met: output ({output_size} bytes) >= input ({input_size} \
                     bytes), ratio {ratio:.1}%"
                ));
                if let Some(path) = file_path {
                    append_file_line(&mut msg, path);
                }
                Some(msg)
            }
            Self::QualityValidationFailed {
                expected_ssim,
                actual_ssim,
                file_path,
            } => {
                let mut msg = user_err(format!(
                    "Quality validation failed: expected SSIM >= {expected_ssim:.4}, actual \
                     {actual_ssim:.4}"
                ));
                if let Some(path) = file_path {
                    append_file_line(&mut msg, path);
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
                    "{}\n   {} Please ensure {tool_name} is installed and in PATH",
                    user_err(format!("Tool not found: {tool_name}")),
                    crate::modern_ui::symbols::pick("💡", "[HINT]")
                );
                if let Some(op) = operation {
                    let _ = write!(msg, "\n   Needed for: {op}");
                }
                msg
            }
            Self::OutputExists { path, operation } => {
                let mut msg = format!(
                    "{} Output file exists: {}",
                    crate::modern_ui::symbols::pick("⏭️", "[SKIP]"),
                    path.display()
                );
                if let Some(op) = operation {
                    append_operation_line(&mut msg, op);
                }
                msg
            }
            Self::NotImplemented(msg) => user_err(format!("Not implemented: {msg}")),
            Self::SkipFile(msg) => format!(
                "{} Skip file: {msg}",
                crate::modern_ui::symbols::pick("⏭️", "[SKIP]")
            ),
            Self::ResultAnomaly(msg) => user_err(format!("Result anomaly: {msg}")),
            Self::GeneralError(err) => user_err(format!("Error: {err}")),
            Self::Other(e) => user_err(format!("Error: {e}")),
            _ => user_err(format!("Unknown error: {self}")),
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

    /// Check if this error should trigger an automatic copy of the original to
    /// output.
    ///
    /// Based on the "Loud and Honest" policy:
    /// - Optimization failures (Skips) -> Copy original (ensure complete output
    ///   set).
    /// - Processing failures (Errors) -> Do NOT copy (avoid silent
    ///   corruption/partial data).
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
                let op_str = crate::media_conversion_gate::trace_label_or_default(
                    operation.as_deref(),
                    "unknown operation",
                );
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
                Self::IoError(e) => write!(f, "IO error: {e}"),
                _ => unreachable!(),
            }
        };

        if matches!(
            self,
            Self::FileNotFound { .. }
                | Self::DirectoryNotFound { .. }
                | Self::FileReadError { .. }
                | Self::FileWriteError { .. }
                | Self::IoError(_)
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
                        "Compression target not met: output ({output_size}) >= input ({input_size})"
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
                        "Quality validation failed: expected SSIM >= {expected_ssim:.4}, got \
                         {actual_ssim:.4}"
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
            Self::NumericError(err) => write!(f, "Numeric error: {err}"),
            Self::NumericOverflow(err) => write!(f, "Numeric overflow: {err}"),
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
            Self::IoError(e) => Some(e),
            Self::ImageProcessingError(e) => Some(e),
            _ => None,
        }
    }
}

// From implementations for easy conversion
impl From<std::io::Error> for UnifiedError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
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
            crate::ffprobe::FFprobeError::IoError(e) => Self::IoError(e),
            other => Self::FFprobeError(other.to_string()),
        }
    }
}

// Type aliases for backward compatibility
pub type Result<T> = std::result::Result<T, UnifiedError>;
pub type ImgResult<T> = std::result::Result<T, UnifiedError>;
pub type VidResult<T> = std::result::Result<T, UnifiedError>;

// Legacy type aliases for backward compatibility
pub type VidQualityError = UnifiedError;
pub type ImgQualityError = UnifiedError;

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
        assert!(err.user_message().contains("File not found"));
    }

    #[test]
    fn test_unified_error_category() {
        let err = UnifiedError::file_not_found("/test");
        assert_eq!(err.category(), ErrorCategory::Fatal);
        assert!(!err.is_recoverable());

        let err = UnifiedError::CompressionFailed {
            input_size: 1000,
            output_size: 1100,
            file_path: None,
        };
        assert_eq!(err.category(), ErrorCategory::Optional);
        assert!(!err.is_recoverable());

        let err = UnifiedError::OutputExists {
            path: PathBuf::from("/test"),
            operation: None,
        };
        assert_eq!(err.category(), ErrorCategory::Optional);
        assert!(!err.is_recoverable());

        let err = UnifiedError::analysis_error("bad media");
        assert_eq!(err.category(), ErrorCategory::Recoverable);
        assert!(err.is_recoverable());
    }

    #[test]
    fn batch_error_mode_preserves_normal_batches_and_fails_fast_when_requested() {
        assert_eq!(
            BatchErrorMode::parse("log_and_continue"),
            BatchErrorMode::LogAndContinue
        );
        assert_eq!(BatchErrorMode::parse("debug"), BatchErrorMode::FailFast);
        assert_eq!(
            BatchErrorMode::parse("unknown-mode"),
            BatchErrorMode::FailFast
        );

        let recoverable: anyhow::Error = UnifiedError::analysis_error("bad media").into();
        assert!(!BatchErrorMode::LogAndContinue.should_abort_error(&recoverable));
        assert!(BatchErrorMode::FailFast.should_abort_error(&recoverable));

        let fatal: anyhow::Error = UnifiedError::tool_not_found("ffmpeg").into();
        assert!(BatchErrorMode::LogAndContinue.should_abort_error(&fatal));

        let unknown = anyhow::anyhow!("unclassified failure");
        assert!(BatchErrorMode::LogAndContinue.should_abort_error(&unknown));

        let other: anyhow::Error = UnifiedError::Other(anyhow::anyhow!("unknown risk")).into();
        assert_eq!(
            other
                .downcast_ref::<UnifiedError>()
                .expect("unified error")
                .category(),
            ErrorCategory::Fatal
        );
        assert!(BatchErrorMode::LogAndContinue.should_abort_error(&other));
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
        assert!(matches!(err, UnifiedError::IoError(_)));
    }

    #[test]
    fn test_optimization_failure_semantics() {
        // An exhausted optimization search is a policy skip.
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
            ErrorCategory::Recoverable,
            "Quality verification failure must be reported as a failed file"
        );
        assert!(
            !quality_err.is_skip(),
            "Quality verification failure must not be hidden as a skip"
        );

        // Hard system failures MUST NOT be Optional Skips
        let fatal_err = UnifiedError::file_not_found("/missing");
        assert_eq!(fatal_err.category(), ErrorCategory::Fatal);
        assert!(!fatal_err.is_skip(), "Fatal errors must NOT be skips");

        let tool_err = UnifiedError::tool_not_found("ffmpeg");
        assert_eq!(tool_err.category(), ErrorCategory::Fatal);
        assert!(!tool_err.is_skip(), "Tool missing must NOT be skips");

        let io_err = UnifiedError::IoError(std::io::Error::other("disk crash"));
        assert_eq!(io_err.category(), ErrorCategory::Fatal);
        assert!(!io_err.is_skip(), "IO errors must NOT be skips");
    }
}
