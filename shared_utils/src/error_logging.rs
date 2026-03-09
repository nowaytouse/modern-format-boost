//! Enhanced Error Logging - Critical and rare error detection system

use crate::colors::*;

/// Error severity levels for enhanced visibility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSeverity {
    /// Critical errors that may cause data loss or corruption
    Critical,
    /// Rare/unexpected errors from upstream tools
    Rare,
    /// Metadata loss or corruption
    MetadataLoss,
    /// Pipeline broken or interrupted
    PipelineBroken,
    /// Upstream tool unexpected behavior
    UpstreamError,
    /// Standard error
    Standard,
}

impl ErrorSeverity {
    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Critical => "🚨 CRITICAL",
            Self::Rare => "⚠️  RARE ERROR",
            Self::MetadataLoss => "📋 METADATA LOSS",
            Self::PipelineBroken => "🔧 PIPELINE BROKEN",
            Self::UpstreamError => "🔺 UPSTREAM ERROR",
            Self::Standard => "❌",
        }
    }

    pub fn color(&self) -> &'static str {
        match self {
            Self::Critical => RED_BOLD,
            Self::Rare => YELLOW_BOLD,
            Self::MetadataLoss => MAGENTA_BOLD,
            Self::PipelineBroken => CYAN_BOLD,
            Self::UpstreamError => YELLOW_BOLD,
            Self::Standard => RED,
        }
    }
}

/// Log an enhanced error with severity classification
pub fn log_enhanced_error(severity: ErrorSeverity, context: &str, error: &str) {
    let prefix = severity.prefix();
    let color = severity.color();
    let reset = RESET;

    // Terminal output with color
    eprintln!("{}{}{} {}: {}", color, prefix, reset, context, error);

    // File log output (no color)
    if crate::progress_mode::has_log_file() {
        let line = format!("{} {}: {}", prefix, context, error);
        crate::progress_mode::write_to_log(&line);
    }
}

/// Detect and classify error patterns
pub fn classify_error(error_msg: &str) -> ErrorSeverity {
    let lower = error_msg.to_lowercase();

    // Critical patterns
    if lower.contains("data loss") || lower.contains("corruption") || lower.contains("truncated") {
        return ErrorSeverity::Critical;
    }

    // Metadata loss patterns
    if lower.contains("metadata") && (lower.contains("lost") || lower.contains("missing") || lower.contains("stripped")) {
        return ErrorSeverity::MetadataLoss;
    }

    // Pipeline broken patterns
    if lower.contains("broken pipe") || lower.contains("connection reset") || lower.contains("unexpected eof") {
        return ErrorSeverity::PipelineBroken;
    }

    // Rare upstream errors
    if lower.contains("assertion failed") || lower.contains("segmentation fault") ||
       lower.contains("bus error") || lower.contains("illegal instruction") {
        return ErrorSeverity::Rare;
    }

    // FFprobe/FFmpeg specific rare errors
    if lower.contains("could find no file") || lower.contains("pattern_type") ||
       lower.contains("invalid data found") && !lower.contains("expected") {
        return ErrorSeverity::Rare;
    }

    // ImageMagick/cjxl rare errors
    if lower.contains("magick") && (lower.contains("unable to") || lower.contains("failed to")) ||
       lower.contains("cjxl") && lower.contains("exit code") && !lower.contains("exit code: 0") {
        return ErrorSeverity::UpstreamError;
    }

    ErrorSeverity::Standard
}

#[macro_export]
macro_rules! log_critical {
    ($context:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::Critical,
            $context,
            &msg
        );
    }};
}

#[macro_export]
macro_rules! log_rare_error {
    ($context:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::Rare,
            $context,
            &msg
        );
    }};
}

#[macro_export]
macro_rules! log_metadata_loss {
    ($context:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::MetadataLoss,
            $context,
            &msg
        );
    }};
}

#[macro_export]
macro_rules! log_pipeline_broken {
    ($context:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::PipelineBroken,
            $context,
            &msg
        );
    }};
}

#[macro_export]
macro_rules! log_upstream_error {
    ($context:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::UpstreamError,
            $context,
            &msg
        );
    }};
}

/// Auto-classify and log error with appropriate severity
#[macro_export]
macro_rules! log_auto_error {
    ($context:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        let severity = $crate::error_logging::classify_error(&msg);
        $crate::error_logging::log_enhanced_error(severity, $context, &msg);
    }};
}
