//! Enhanced Logging System
//!
//! ## Features
//! - Full log level hierarchy (ERROR > WARN > INFO > DEBUG > TRACE)
//! - 24-bit `TrueColor` terminal output
//! - Structured logging to file (includes emojis, full call stack)
//! - Concise terminal output (key summaries only)
//! - Prevents silencing of upstream tool logs
//!
//! ## Design Principles
//! - Terminal: Displays only key information and progress
//! - File: Records full detailed information
//! - Color: Modern, aesthetic, and consistent 24-bit `TrueColor`
//! - Transparency: Faithfully reflects runtime state for quick bug identification

use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use tracing::Level;
use tracing_subscriber::{
    EnvFilter,
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

// ─── Color Palette (24-bit True Color) ─────────────────────────────────────

/// 24-bit `TrueColor` definition
pub mod colors {
    /// Success Green - `RGB(76, 175, 80)`
    pub const SUCCESS: &str = "\x1b[38;2;76;175;80m";
    /// Warning Yellow - `RGB(255, 193, 7)`
    pub const WARNING: &str = "\x1b[38;2;255;193;7m";
    /// Error Red - `RGB(244, 67, 54)`
    pub const ERROR: &str = "\x1b[38;2;244;67;54m";
    /// Info Blue - `RGB(33, 150, 243)`
    pub const INFO: &str = "\x1b[38;2;33;150;243m";
    /// Debug Cyan - `RGB(0, 188, 212)`
    pub const DEBUG: &str = "\x1b[38;2;0;188;212m";
    /// Trace Purple - `RGB(156, 39, 176)`
    pub const TRACE: &str = "\x1b[38;2;156;39;176m";
    /// Critical Magenta - `RGB(233, 30, 99)`
    pub const CRITICAL: &str = "\x1b[38;2;233;30;99m";
    /// Value Orange - `RGB(255, 152, 0)`
    pub const VALUE: &str = "\x1b[38;2;255;152;0m";
    /// Reset all styles
    pub const RESET: &str = "\x1b[0m";
    /// Bold
    pub const BOLD: &str = "\x1b[1m";
    /// Dim
    pub const DIM: &str = "\x1b[2m";
}

/// Terminal Color Helper
pub struct TerminalColor;

impl TerminalColor {
    /// Applies color to text
    #[must_use]
    pub fn colorize(text: &str, color: &str) -> String {
        format!("{color}{text}{}", colors::RESET)
    }

    /// Success message (Green)
    #[must_use]
    pub fn success(text: &str) -> String {
        Self::colorize(text, colors::SUCCESS)
    }

    /// Warning message (Yellow)
    #[must_use]
    pub fn warning(text: &str) -> String {
        Self::colorize(text, colors::WARNING)
    }

    /// Error message (Red)
    #[must_use]
    pub fn error(text: &str) -> String {
        Self::colorize(text, colors::ERROR)
    }

    /// Info message (Blue)
    #[must_use]
    pub fn info(text: &str) -> String {
        Self::colorize(text, colors::INFO)
    }

    /// Debug message (Cyan)
    #[must_use]
    pub fn debug(text: &str) -> String {
        Self::colorize(text, colors::DEBUG)
    }

    /// Trace message (Purple)
    #[must_use]
    pub fn trace(text: &str) -> String {
        Self::colorize(text, colors::TRACE)
    }

    /// Critical message (Magenta)
    #[must_use]
    pub fn critical(text: &str) -> String {
        Self::colorize(text, colors::CRITICAL)
    }

    /// Value highlight (Orange)
    #[must_use]
    pub fn value(text: &str) -> String {
        Self::colorize(text, colors::VALUE)
    }

    /// Removes all ANSI color codes (for file logs)
    #[must_use]
    pub fn strip_ansi(text: &str) -> String {
        // Simple ANSI escape sequence removal (no regex)
        let mut result = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Skip escape sequence ESC[ ... m
                if chars.next() == Some('[') {
                    // Skip until letter is found
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if next.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    }
}

// ─── Log Level Icons & Formatting ───────────────────────────────────────────

/// Log level icons
pub mod icons {
    pub const ERROR: &str = "❌";
    pub const WARN: &str = "⚠️ ";
    pub const INFO: &str = "ℹ️ ";
    pub const DEBUG: &str = "🔍";
    pub const TRACE: &str = "🔬";
    pub const CRITICAL: &str = "🚨";
    pub const SUCCESS: &str = "✅";
    pub const START: &str = "🚀";
    pub const END: &str = "🏁";
}

/// Formats log level tag (with color and icon)
#[must_use]
pub fn format_level(level: Level) -> String {
    match level {
        Level::ERROR => TerminalColor::error(&format!("{} ERROR", icons::ERROR)),
        Level::WARN => TerminalColor::warning(&format!("{} WARN ", icons::WARN)),
        Level::INFO => TerminalColor::info(&format!("{} INFO ", icons::INFO)),
        Level::DEBUG => TerminalColor::debug(&format!("{} DEBUG", icons::DEBUG)),
        Level::TRACE => TerminalColor::trace(&format!("{} TRACE", icons::TRACE)),
    }
}

/// Formats log level tag (plain text, for file)
#[must_use]
pub fn format_level_plain(level: Level) -> String {
    match level {
        Level::ERROR => format!("[ERROR] {}", icons::ERROR),
        Level::WARN => format!("[WARN]  {}", icons::WARN),
        Level::INFO => format!("[INFO]  {}", icons::INFO),
        Level::DEBUG => format!("[DEBUG] {}", icons::DEBUG),
        Level::TRACE => format!("[TRACE] {}", icons::TRACE),
    }
}

// ─── Enhanced Log Levels ────────────────────────────────────────────────────

/// Enhanced log level classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Critical, // Highest priority: Data loss, corruption
    Error,    // Error: Operation failed
    Warn,     // Warning: Potential issues
    Info,     // Information: Normal operation
    Debug,    // Debug: Detailed diagnostic info
    Trace,    // Trace: Most detailed execution path
}

impl From<Level> for LogLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::ERROR => Self::Error,
            Level::WARN => Self::Warn,
            Level::INFO => Self::Info,
            Level::DEBUG => Self::Debug,
            Level::TRACE => Self::Trace,
        }
    }
}

impl LogLevel {
    /// Checks if this level should be logged
    #[must_use]
    pub fn should_log(self, max_level: Self) -> bool {
        self <= max_level
    }

    /// Converts to tracing Level
    #[must_use]
    pub const fn to_tracing_level(self) -> Level {
        match self {
            Self::Critical | Self::Error => Level::ERROR,
            Self::Warn => Level::WARN,
            Self::Info => Level::INFO,
            Self::Debug => Level::DEBUG,
            Self::Trace => Level::TRACE,
        }
    }
}

// ─── Terminal vs File Log Routing ───────────────────────────────────────────

/// Log output target
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTarget {
    /// Terminal output (concise, colorized)
    Terminal,
    /// File output (complete, plain text)
    File,
    /// Output to both
    Both,
}

/// Log Router - Determines output target based on content
pub struct LogRouter {
    /// Current log level
    max_level: LogLevel,
    /// Whether file logging is enabled
    file_enabled: bool,
    /// File log writer (if enabled)
    file_writer: Option<Mutex<Box<dyn Write + Send>>>,
}

impl LogRouter {
    /// Creates a new log router
    #[must_use]
    pub fn new(max_level: LogLevel) -> Self {
        Self {
            max_level,
            file_enabled: false,
            file_writer: None,
        }
    }

    /// Sets the file log writer
    pub fn set_file_writer(&mut self, writer: Box<dyn Write + Send>) {
        self.file_writer = Some(Mutex::new(writer));
        self.file_enabled = true;
    }

    /// Routes log messages
    pub fn route(&self, level: LogLevel, _message: &str) -> LogTarget {
        // Always output to terminal (filtered by level)
        // Detailed debug info only goes to file
        if level <= LogLevel::Info {
            LogTarget::Both
        } else {
            LogTarget::File
        }
    }

    /// Writes log
    pub fn log(&self, level: LogLevel, message: &str, context: &str) {
        if !level.should_log(self.max_level) {
            return;
        }

        let target = self.route(level, message);

        match target {
            LogTarget::Terminal | LogTarget::Both => {
                self.write_terminal(level, message, context);
            }
            LogTarget::File => {}
        }

        if (target == LogTarget::File || target == LogTarget::Both) && self.file_enabled {
            self.write_file(level, message, context);
        }
    }

    /// Writes to terminal (colorized, concise)
    fn write_terminal(&self, level: LogLevel, message: &str, context: &str) {
        let _ = self; // Acknowledge self to resolve unused_self if it's part of a trait or intended for future use
        let level_str = format_level(level.to_tracing_level());
        let colored_msg = format!("{level_str} {context} {message}");
        eprintln!("{colored_msg}");
    }

    /// Writes to file (plain text, complete)
    fn write_file(&self, level: LogLevel, message: &str, context: &str) {
        if let Some(ref writer) = self.file_writer {
            let plain_level = format_level_plain(level.to_tracing_level());
            let plain_msg = format!("{plain_level} {context} {message}");
            if let Ok(mut w) = writer.lock() {
                let _ = writeln!(w, "{plain_msg}");
                let _ = w.flush();
            }
        }
    }
}

// ─── Convenience Macros ─────────────────────────────────────────────────────

/// Logs critical error
#[macro_export]
macro_rules! log_enhanced_critical {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        let colored = format!("\x1b[38;2;233;30;99m🚨 CRITICAL\x1b[0m {msg}");
        eprintln!("{colored}");
        tracing::error!("{msg}");
    }};
}

/// Logs success
#[macro_export]
macro_rules! log_enhanced_success {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        eprintln!("✅ {msg}");
        tracing::info!("{msg}");
    }};
}

/// Logs operation start
#[macro_export]
macro_rules! log_enhanced_start {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        eprintln!("🚀 {msg}");
        tracing::info!("{msg}");
    }};
}

/// Logs operation end
#[macro_export]
macro_rules! log_enhanced_end {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        eprintln!("🏁 {msg}");
        tracing::info!("{msg}");
    }};
}

// ─── Upstream Tool Logging ──────────────────────────────────────────────────

/// Upstream tool logger - **Never silences upstream tool output**
pub struct UpstreamToolLogger {
    tool_name: String,
}

impl UpstreamToolLogger {
    /// Creates an upstream tool logger
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
        }
    }

    /// Logs tool command
    pub fn log_command(&self, command: &str) {
        eprintln!(
            "\x1b[38;2;33;150;243m▶ {}\x1b[0m Executing: \x1b[38;2;255;152;0m{command}\x1b[0m",
            self.tool_name
        );
        tracing::info!("[{}] Executing: {command}", self.tool_name);
    }

    /// Logs tool output
    pub fn log_output(&self, output: &str) {
        // Detailed output recorded to file, not shown in terminal
        tracing::debug!("[{}] stdout: {output}", self.tool_name);
    }

    /// Logs tool error
    pub fn log_error(&self, error: &str) {
        tracing::warn!("[{}] stderr: {error}", self.tool_name);
        // Errors always shown in terminal
        eprintln!(
            "⚠️  {} error: \x1b[38;2;244;67;54m{error}\x1b[0m",
            self.tool_name
        );
    }

    /// Logs tool exit code
    pub fn log_exit(&self, exit_code: i32) {
        if exit_code == 0_i32 {
            tracing::debug!("[{}] exited with code 0", self.tool_name);
        } else {
            eprintln!(
                "\x1b[38;2;233;30;99m🚨 CRITICAL\x1b[0m [{}] exited with non-zero code: {exit_code}",
                self.tool_name
            );
            tracing::error!(
                "[{}] exited with non-zero code: {exit_code}",
                self.tool_name
            );
        }
    }
}

// ─── Integration with tracing ───────────────────────────────────────────────

/// Initialize the enhanced logging system.
///
/// # Errors
///
/// Returns an error if an IO error or configuration error occurs during initialization.
pub fn init_enhanced_logging(
    program_name: &str,
    log_level: LogLevel,
    log_file_path: Option<&Path>,
) -> anyhow::Result<()> {
    let filter = EnvFilter::builder()
        .with_default_directive(log_level.to_tracing_level().into())
        .from_env_lossy();

    let fmt_layer = fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .with_writer(io::stderr)
        .with_ansi(true);

    if let Some(path) = log_file_path {
        let file_appender = tracing_appender::rolling::never(
            path.parent().unwrap_or_else(|| Path::new(".")),
            path.file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("app.log")),
        );

        let file_layer = fmt::layer()
            .with_writer(file_appender)
            .with_ansi(false) // Do not use colors in files
            .with_span_events(FmtSpan::FULL);

        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .init();
    }

    eprintln!("🚀 {program_name} logging initialized at level {log_level:?}");
    tracing::info!(
        "{} logging initialized at level {:?}",
        program_name,
        log_level
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_color() {
        let msg = TerminalColor::success("Test message");
        assert!(msg.contains("\x1b[")); // Should contain ANSI codes
    }

    #[test]
    fn test_strip_ansi() {
        let colored = TerminalColor::error("Error message");
        let plain = TerminalColor::strip_ansi(&colored);
        assert!(!plain.contains("\x1b[")); // Should not contain ANSI codes
        assert!(plain.contains("Error message"));
    }

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Trace);
    }

    #[test]
    fn test_upstream_tool_logger() {
        let logger = UpstreamToolLogger::new("ffmpeg");
        logger.log_command("ffmpeg -i input.mp4 output.mp4");
        logger.log_exit(0);
    }

    #[test]
    fn test_strip_ansi_simple() {
        let colored = "\x1b[31mError\x1b[0m message";
        let plain = TerminalColor::strip_ansi(colored);
        assert!(!plain.contains('\x1b'));
        assert!(plain.contains("Error message"));
    }
}
