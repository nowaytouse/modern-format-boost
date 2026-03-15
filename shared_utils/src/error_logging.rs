//! Enhanced Error Logging - Severity-classified error detection system
//!
//! Provides color-coded, severity-classified error output so rare and critical bugs
//! are immediately visible in both terminal and file logs.

/// Error severity levels for enhanced visibility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSeverity {
    /// Data loss, corruption, or truncation — highest priority
    Critical,
    /// Unexpected behavior with no obvious cause — needs investigation
    Rare,
    /// Metadata stripped, lost, or unreadable
    MetadataLoss,
    /// Broken pipe, EOF mid-stream, process terminated unexpectedly
    PipelineBroken,
    /// FFmpeg/cjxl/ImageMagick returned unexpected exit codes or output
    UpstreamError,
    /// Ordinary recoverable error
    Standard,
}

impl ErrorSeverity {
    /// Short label used in log lines (no color — for file logs)
    pub fn label(&self) -> &'static str {
        match self {
            Self::Critical      => "[CRITICAL]",
            Self::Rare          => "[RARE ERROR]",
            Self::MetadataLoss  => "[METADATA LOSS]",
            Self::PipelineBroken => "[PIPELINE BROKEN]",
            Self::UpstreamError => "[UPSTREAM ERROR]",
            Self::Standard      => "[ERROR]",
        }
    }

    /// Colored label for terminal output
    pub fn label_colored(&self) -> String {
        match self {
            Self::Critical      => "\x1b[1;31m🚨 CRITICAL\x1b[0m".to_string(),
            Self::Rare          => "\x1b[1;33m⚠️  RARE ERROR\x1b[0m".to_string(),
            Self::MetadataLoss  => "\x1b[1;35m📋 METADATA LOSS\x1b[0m".to_string(),
            Self::PipelineBroken => "\x1b[1;36m🔧 PIPELINE BROKEN\x1b[0m".to_string(),
            Self::UpstreamError => "\x1b[33m🔺 UPSTREAM ERROR\x1b[0m".to_string(),
            Self::Standard      => "\x1b[31m❌ ERROR\x1b[0m".to_string(),
        }
    }
}

/// Emit an enhanced error to both terminal (colored) and file log (plain).
///
/// Terminal: `  🚨 CRITICAL  <context>: <detail>`
/// File:     `  [CRITICAL] <context>: <detail>`
pub fn log_enhanced_error(severity: ErrorSeverity, context: &str, detail: &str) {
    // Terminal: colored, indented
    let colored = format!("  {}  \x1b[1m{}\x1b[0m: {}", severity.label_colored(),
        context, detail);
    crate::progress_mode::emit_stderr(&colored);

    // File log: plain text with label
    if crate::progress_mode::has_log_file() {
        let plain = format!("  {}  {}: {}", severity.label(), context, detail);
        crate::progress_mode::write_to_log(&plain);
    }
}

/// Auto-classify an error message by pattern matching.
pub fn classify_error(msg: &str) -> ErrorSeverity {
    let lower = msg.to_lowercase();

    if lower.contains("data loss") || lower.contains("corrupt") || lower.contains("truncat") {
        return ErrorSeverity::Critical;
    }
    if lower.contains("metadata") && (lower.contains("lost") || lower.contains("missing") || lower.contains("strip")) {
        return ErrorSeverity::MetadataLoss;
    }
    if lower.contains("broken pipe") || lower.contains("unexpected eof") || lower.contains("connection reset") {
        return ErrorSeverity::PipelineBroken;
    }
    if lower.contains("assertion failed") || lower.contains("segmentation fault") || lower.contains("bus error") {
        return ErrorSeverity::Rare;
    }
    if lower.contains("could find no file") || lower.contains("pattern_type") {
        return ErrorSeverity::Rare;
    }
    if (lower.contains("cjxl") || lower.contains("magick") || lower.contains("ffmpeg") || lower.contains("ffprobe"))
        && (lower.contains("exit code") || lower.contains("failed") || lower.contains("error"))
    {
        return ErrorSeverity::UpstreamError;
    }
    ErrorSeverity::Standard
}

// ── Convenience macros ────────────────────────────────────────────────────────

#[macro_export]
macro_rules! log_critical {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::Critical,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_rare_error {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::Rare,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_metadata_loss {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::MetadataLoss,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_pipeline_broken {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::PipelineBroken,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_upstream_error {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::ErrorSeverity::UpstreamError,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_auto_error {
    ($ctx:expr, $($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        $crate::error_logging::log_enhanced_error(
            $crate::error_logging::classify_error(&_msg),
            $ctx, &_msg,
        )
    }};
}
