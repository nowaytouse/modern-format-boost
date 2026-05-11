//! Modern UI/UX Module
//!
//! Provides modern terminal interactions and visual effects:
//! - Dynamic Spinner animations
//! - Gradient progress bars
//! - Real-time status updates
//! - Beautified result presentation

use console::style;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tracing::{error, warn};

/// ANSI color and styling constants for terminal output.
pub mod colors {
    /// Reset all terminal attributes.
    pub const RESET: &str = "\x1b[0m";
    /// Enable bold text.
    pub const BOLD: &str = "\x1b[1m";
    /// Enable dimmed text.
    pub const DIM: &str = "\x1b[2m";
    /// Enable italic text.
    pub const ITALIC: &str = "\x1b[3m";
    /// Enable underlined text.
    pub const UNDERLINE: &str = "\x1b[4m";

    /// Standard red foreground color.
    pub const RED: &str = "\x1b[31m";
    /// Standard green foreground color.
    pub const GREEN: &str = "\x1b[32m";
    /// Standard yellow foreground color.
    pub const YELLOW: &str = "\x1b[33m";
    /// Standard blue foreground color.
    pub const BLUE: &str = "\x1b[34m";
    /// Standard magenta foreground color.
    pub const MAGENTA: &str = "\x1b[35m";
    /// Standard cyan foreground color.
    pub const CYAN: &str = "\x1b[36m";
    /// Standard white foreground color.
    pub const WHITE: &str = "\x1b[37m";

    /// Bright red foreground color.
    pub const BRIGHT_RED: &str = "\x1b[91m";
    /// Bright green foreground color.
    pub const BRIGHT_GREEN: &str = "\x1b[92m";
    /// Bright yellow foreground color.
    pub const BRIGHT_YELLOW: &str = "\x1b[93m";
    /// Bright blue foreground color.
    pub const BRIGHT_BLUE: &str = "\x1b[94m";
    /// Bright magenta foreground color.
    pub const BRIGHT_MAGENTA: &str = "\x1b[95m";
    /// Bright cyan foreground color.
    pub const BRIGHT_CYAN: &str = "\x1b[96m";
    /// Bright white foreground color.
    pub const BRIGHT_WHITE: &str = "\x1b[97m";

    /// Black background color.
    pub const BG_BLACK: &str = "\x1b[40m";
    /// Red background color.
    pub const BG_RED: &str = "\x1b[41m";
    /// Green background color.
    pub const BG_GREEN: &str = "\x1b[42m";
    /// Yellow background color.
    pub const BG_YELLOW: &str = "\x1b[43m";
    /// Blue background color.
    pub const BG_BLUE: &str = "\x1b[44m";
    /// Magenta background color.
    pub const BG_MAGENTA: &str = "\x1b[45m";
    /// Cyan background color.
    pub const BG_CYAN: &str = "\x1b[46m";
    /// White background color.
    pub const BG_WHITE: &str = "\x1b[47m";

    /// Modern Format Boost blue — true-color RGB (67, 160, 255).
    pub const MFB_BLUE: &str = "\x1b[38;2;67;160;255m";
    /// Modern Format Boost purple — true-color RGB (187, 134, 252).
    pub const MFB_PURPLE: &str = "\x1b[38;2;187;134;252m";
    /// Modern Format Boost pink — true-color RGB (255, 121, 198).
    pub const MFB_PINK: &str = "\x1b[38;2;255;121;198m";
    /// Modern Format Boost green — true-color RGB (80, 250, 123).
    pub const MFB_GREEN: &str = "\x1b[38;2;80;250;123m";
    /// Modern Format Boost yellow — true-color RGB (241, 250, 140).
    pub const MFB_YELLOW: &str = "\x1b[38;2;241;250;140m";
    /// Modern Format Boost cyan — true-color RGB (139, 233, 253).
    pub const MFB_CYAN: &str = "\x1b[38;2;139;233;253m";
    /// Modern Format Boost orange — true-color RGB (255, 184, 108).
    pub const MFB_ORANGE: &str = "\x1b[38;2;255;184;108m";
    /// Modern Format Boost red — true-color RGB (255, 85, 85).
    pub const MFB_RED: &str = "\x1b[38;2;255;85;85m";

    /// Primary accent color for UI highlights.
    pub const ACCENT: &str = "\x1b[38;2;0;198;255m";
    /// Gradient color used for success messages.
    pub const SUCCESS_GRADIENT: &str = "\x1b[38;2;0;255;135m";
    /// Gradient color used for warning messages.
    pub const WARNING_GRADIENT: &str = "\x1b[38;2;255;220;0m";
    /// Gradient color used for error messages.
    pub const ERROR_GRADIENT: &str = "\x1b[38;2;255;75;43m";
    /// Muted/dimmed gray for secondary text.
    pub const MUTE: &str = "\x1b[38;2;100;100;100m";

    // --- Added from enhanced_logging.rs ---
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

    // --- Added from terminal_logging.rs ---
    pub const FG_BLACK: &str = "30";
    pub const FG_RED: &str = "31";
    pub const FG_GREEN: &str = "32";
    pub const FG_YELLOW: &str = "33";
    pub const FG_BLUE: &str = "34";
    pub const FG_MAGENTA: &str = "35";
    pub const FG_CYAN: &str = "36";
    pub const FG_WHITE: &str = "37";

    pub const FG_BRIGHT_BLACK: &str = "90";
    pub const FG_BRIGHT_RED: &str = "91";
    pub const FG_BRIGHT_GREEN: &str = "92";
    pub const FG_BRIGHT_YELLOW: &str = "93";
    pub const FG_BRIGHT_BLUE: &str = "94";
    pub const FG_BRIGHT_MAGENTA: &str = "95";
    pub const FG_BRIGHT_CYAN: &str = "96";
    pub const FG_BRIGHT_WHITE: &str = "97";
}

/// Helper functions for applying console-based styling.
pub mod styles {
    use console::Style;

    #[must_use]
    pub const fn success() -> Style {
        Style::new().for_stderr().color256(39).bold() // Premium Sky Blue
    }

    #[must_use]
    pub const fn error() -> Style {
        Style::new().for_stderr().color256(196).bold() // Premium Red (Vibrant)
    }

    #[must_use]
    pub const fn warning() -> Style {
        Style::new().for_stderr().color256(214) // Premium Orange/Yellow
    }

    #[must_use]
    pub const fn info() -> Style {
        Style::new().for_stderr().color256(45) // Bright Cyan
    }

    #[must_use]
    pub const fn highlight() -> Style {
        Style::new().for_stderr().color256(171).bold() // Premium Purple
    }

    #[must_use]
    pub const fn number() -> Style {
        Style::new().for_stderr().color256(45).bold() // Bright Cyan
    }

    #[must_use]
    pub const fn dim() -> Style {
        Style::new().for_stderr().dim()
    }
}

/// Helper functions for applying color gradient effects to text.
pub mod gradients {
    use super::colors::{MFB_BLUE, MFB_PURPLE, RESET};

    /// Wraps `text` in the MFB blue color code.
    #[must_use]
    pub fn blue_to_cyan(text: &str) -> String {
        format!("{MFB_BLUE}{text}{RESET}") // Simplified for now, real gradient logic would iterate chars
    }

    /// Wraps `text` in the MFB purple color code.
    #[must_use]
    pub fn purple_to_pink(text: &str) -> String {
        format!("{MFB_PURPLE}{text}{RESET}")
    }
}

// ─── Logging Infrastructure (Moved from terminal_logging.rs & enhanced_logging.rs) ───

/// Color Manager - Ensures colors are correctly closed
pub struct ColorGuard {
    enabled: bool,
}

impl ColorGuard {
    /// Enables colors
    #[must_use]
    pub const fn enable() -> Self {
        Self { enabled: true }
    }

    /// Disables colors
    #[must_use]
    pub const fn disable() -> Self {
        Self { enabled: false }
    }

    /// Applies color to text
    #[must_use]
    pub fn colorize(&self, text: &str, ansi_code: &str) -> String {
        if self.enabled {
            format!("\x1b[{ansi_code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// Terminal Log Helper
pub struct TerminalLogger {
    use_colors: bool,
    debug_mode: bool,
}

impl TerminalLogger {
    /// Creates a new terminal logger
    #[must_use]
    pub const fn new(use_colors: bool, debug_mode: bool) -> Self {
        Self {
            use_colors,
            debug_mode,
        }
    }

    /// Applies color (if enabled)
    fn color(&self, text: &str, code: &str) -> String {
        if self.use_colors {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// Success message (Green)
    #[must_use]
    pub fn success(&self, text: &str) -> String {
        self.color(text, colors::FG_BRIGHT_GREEN)
    }

    /// Error message (Red)
    #[must_use]
    pub fn error(&self, text: &str) -> String {
        self.color(text, colors::FG_BRIGHT_RED)
    }

    /// Warning message (Yellow)
    #[must_use]
    pub fn warning(&self, text: &str) -> String {
        self.color(text, colors::FG_BRIGHT_YELLOW)
    }

    /// Info message (Blue)
    #[must_use]
    pub fn info(&self, text: &str) -> String {
        self.color(text, colors::FG_BRIGHT_BLUE)
    }

    /// Debug message (Cyan)
    #[must_use]
    pub fn debug(&self, text: &str) -> String {
        self.color(text, colors::FG_CYAN)
    }

    /// Critical message (Magenta)
    #[must_use]
    pub fn critical(&self, text: &str) -> String {
        self.color(text, colors::FG_BRIGHT_MAGENTA)
    }

    /// Value highlight (Bold White)
    #[must_use]
    pub fn value(&self, text: &str) -> String {
        if self.use_colors {
            format!("\x1b[1;97m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// Prints success message
    pub fn print_success(&self, text: &str) {
        crate::progress_mode::emit_stderr(&format!("✅ {}", self.success(text)));
    }

    /// Prints error message
    pub fn print_error(&self, text: &str) {
        crate::progress_mode::emit_stderr(&format!("❌ {}", self.error(text)));
    }

    /// Prints warning message
    pub fn print_warning(&self, text: &str) {
        crate::progress_mode::emit_stderr(&format!("⚠️  {}", self.warning(text)));
    }

    /// Prints info message
    pub fn print_info(&self, text: &str) {
        crate::progress_mode::emit_stderr(&format!("ℹ️  {}", self.info(text)));
    }

    /// Prints debug message (Debug mode only)
    pub fn print_debug(&self, text: &str) {
        if self.debug_mode {
            crate::progress_mode::emit_stderr(&format!("🔍 {}", self.debug(text)));
        }
    }

    /// Prints critical message
    pub fn print_critical(&self, text: &str) {
        crate::progress_mode::emit_stderr(&format!("🚨 {}", self.critical(text)));
    }

    /// Prints stage title
    pub fn print_stage(&self, title: &str, description: &str) {
        crate::progress_mode::emit_stderr(&format!("▶ {}  {}", self.info(title), description));
    }

    /// Prints sub-stage
    pub fn print_substage(&self, description: &str) {
        crate::progress_mode::emit_stderr(&format!("  └─ {description}"));
    }

    /// Prints separator line
    pub fn print_separator(&self) {
        crate::progress_mode::emit_stderr(&"─".repeat(60));
    }

    /// Prints formatted size
    #[must_use]
    pub fn format_size(&self, bytes: u64) -> String {
        format_size(bytes)
    }

    /// Prints size change
    pub fn print_size_change(&self, old: u64, new: u64) {
        let old_str = self.format_size(old);
        let new_str = self.format_size(new);
        let percent = if old > 0 {
            // Compute permille as u128 to avoid overflow, then narrow to u32.
            // If new >> old such that result > u32::MAX, the ratio is astronomically
            // large; cap at u32::MAX and warn rather than panicking or hiding it.
            let permille_u128 = (u128::from(new) * 10_000) / u128::from(old);
            let permille = u32::try_from(permille_u128).unwrap_or_else(|_| {
                tracing::warn!(
                    old_bytes = old,
                    new_bytes = new,
                    permille_raw = permille_u128,
                    "print_size_change: size ratio overflows u32; capping display at u32::MAX"
                );
                u32::MAX
            });
            (f64::from(permille) / 100.0_f64) - 100.0_f64
        } else {
            0.0_f64
        };

        let sign = if percent >= 0.0_f64 { "+" } else { "" };
        let percent_str = format!("{sign}{percent:.1}%");

        let change_color = if percent < 0.0_f64 {
            self.success(&percent_str)
        } else if percent > 5.0_f64 {
            self.error(&percent_str)
        } else {
            self.warning(&percent_str)
        };

        crate::progress_mode::emit_stderr(&format!("{old_str} → {new_str} ({change_color})"));
    }
}

/// Global terminal logger instance
static GLOBAL_LOGGER: OnceLock<TerminalLogger> = OnceLock::new();
static FALLBACK_LOGGER: TerminalLogger = TerminalLogger::new(false, false);
static FALLBACK_LOGGER_WARNING: OnceLock<()> = OnceLock::new();

/// Initialize global terminal logger
pub fn init_terminal_logger(use_colors: bool, debug_mode: bool) {
    if GLOBAL_LOGGER
        .set(TerminalLogger::new(use_colors, debug_mode))
        .is_err()
    {
        crate::progress_mode::emit_stderr(
            "⚠️ [Terminal UI] logger init requested more than once; keeping first instance",
        );
    }
}

/// Get global terminal logger instance
pub fn terminal_logger() -> &'static TerminalLogger {
    GLOBAL_LOGGER.get().unwrap_or_else(|| {
        if FALLBACK_LOGGER_WARNING.set(()).is_ok() {
            crate::progress_mode::emit_stderr(
                "⚠️ [Terminal UI] logger used before initialization; falling back to a plain logger",
            );
        }
        &FALLBACK_LOGGER
    })
}

/// Terminal Color Helper (from `enhanced_logging`)
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
        strip_ansi(text)
    }
}

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

impl From<tracing::Level> for LogLevel {
    fn from(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::TRACE => Self::Trace,
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
    pub const fn to_tracing_level(self) -> tracing::Level {
        match self {
            Self::Critical | Self::Error => tracing::Level::ERROR,
            Self::Warn => tracing::Level::WARN,
            Self::Info => tracing::Level::INFO,
            Self::Debug => tracing::Level::DEBUG,
            Self::Trace => tracing::Level::TRACE,
        }
    }
}

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
        crate::progress_mode::emit_stderr(&colored_msg);
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

/// Formats log level tag (with color and icon)
#[must_use]
pub fn format_level(level: tracing::Level) -> String {
    match level {
        tracing::Level::ERROR => TerminalColor::error(&format!("{} ERROR", symbols::CROSS)),
        tracing::Level::WARN => TerminalColor::warning(&format!("{} WARN ", symbols::WARNING)),
        tracing::Level::INFO => TerminalColor::info(&format!("{} INFO ", symbols::INFO)),
        tracing::Level::DEBUG => TerminalColor::debug(&format!("{} DEBUG", symbols::SEARCH)),
        tracing::Level::TRACE => TerminalColor::trace(&format!("{} TRACE", "🔬")),
    }
}

/// Formats log level tag (plain text, for file)
#[must_use]
pub fn format_level_plain(level: tracing::Level) -> String {
    match level {
        tracing::Level::ERROR => format!("[ERROR] {}", symbols::CROSS),
        tracing::Level::WARN => format!("[WARN]  {}", symbols::WARNING),
        tracing::Level::INFO => format!("[INFO]  {}", symbols::INFO),
        tracing::Level::DEBUG => format!("[DEBUG] {}", symbols::SEARCH),
        tracing::Level::TRACE => "[TRACE] 🔬".to_string(),
    }
}

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
        crate::progress_mode::emit_stderr(&format!(
            "\x1b[38;2;33;150;243m▶ {}\x1b[0m Executing: \x1b[38;2;255;152;0m{command}\x1b[0m",
            self.tool_name
        ));
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
        crate::progress_mode::emit_stderr(&format!(
            "⚠️  {} error: \x1b[38;2;244;67;54m{error}\x1b[0m",
            self.tool_name
        ));
    }

    /// Logs tool exit code
    pub fn log_exit(&self, exit_code: i32) {
        if exit_code == 0_i32 {
            tracing::debug!("[{}] exited with code 0", self.tool_name);
        } else {
            crate::progress_mode::emit_stderr(&format!(
                "\x1b[38;2;233;30;99m🚨 CRITICAL\x1b[0m [{}] exited with non-zero code: {exit_code}",
                self.tool_name
            ));
            tracing::error!(
                "[{}] exited with non-zero code: {exit_code}",
                self.tool_name
            );
        }
    }
}

/// Initialize the enhanced logging system.
///
/// # Errors
///
/// Returns an error if an IO error or configuration error occurs during initialization.
pub fn init_enhanced_logging(
    program_name: &str,
    log_level: LogLevel,
    log_file_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    use tracing_subscriber::{
        EnvFilter,
        fmt::{self, format::FmtSpan},
        layer::SubscriberExt,
        util::SubscriberInitExt,
    };

    let filter = EnvFilter::builder()
        .with_default_directive(log_level.to_tracing_level().into())
        .from_env_lossy();

    let fmt_layer = fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .with_writer(io::stderr)
        .with_ansi(true);

    if let Some(path) = log_file_path {
        let file_appender = tracing_appender::rolling::never(
            path.parent().unwrap_or_else(|| std::path::Path::new(".")),
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

    crate::progress_mode::emit_stderr(&format!(
        "🚀 {program_name} logging initialized at level {log_level:?}"
    ));
    tracing::info!(
        "{} logging initialized at level {:?}",
        program_name,
        log_level
    );

    Ok(())
}

/// Unicode symbols and icon-like characters used throughout the CLI output.
pub mod symbols {
    /// Check mark symbol.
    pub const CHECK: &str = "✅";
    /// Cross / failure symbol.
    pub const CROSS: &str = "❌";
    /// Right-pointing arrow.
    pub const ARROW_RIGHT: &str = "➜";
    /// Down-pointing arrow.
    pub const ARROW_DOWN: &str = "▼";
    /// Bullet point.
    pub const BULLET: &str = "•";
    /// Star symbol.
    pub const STAR: &str = "⭐";
    /// Sparkle symbol.
    pub const SPARKLE: &str = "✨";
    /// Fire symbol.
    pub const FIRE: &str = "🔥";
    /// Rocket symbol.
    pub const ROCKET: &str = "🚀";
    /// Search / magnifying glass symbol.
    pub const SEARCH: &str = "🔍";
    /// Chart / graph symbol.
    pub const CHART: &str = "📊";
    /// Folder symbol.
    pub const FOLDER: &str = "📂";
    /// Video symbol.
    pub const VIDEO: &str = "🎬";
    /// Image symbol.
    pub const IMAGE: &str = "🖼️";
    /// Compress / squeeze symbol.
    pub const COMPRESS: &str = "🗜️";
    /// Quality indicator symbol.
    pub const QUALITY: &str = "💎";
    /// GPU indicator symbol.
    pub const GPU: &str = "⚡";
    /// CPU indicator symbol.
    pub const CPU: &str = "💻";
    /// Clock / time indicator symbol.
    pub const CLOCK: &str = "⏱️";
    /// Save / disk symbol.
    pub const SAVE: &str = "💾";
    /// Warning indicator symbol.
    pub const WARNING: &str = "⚠️";
    /// Error indicator symbol.
    pub const ERROR: &str = "🚫";
    /// Success indicator symbol.
    pub const SUCCESS: &str = "✅";
    /// Information indicator symbol.
    pub const INFO: &str = "💡";
    /// Diamond / bullet point symbol.
    pub const DIAMOND: &str = "◆";
    /// Medal / achievement symbol.
    pub const MEDAL: &str = "🏅";
    /// Shield / security symbol.
    pub const SHIELD: &str = "🛡️";
    /// Link / chain symbol.
    pub const LINK: &str = "🔗";
    /// Bug / error symbol.
    pub const BUG: &str = "🐛";
    /// Stop / halt symbol.
    pub const STOP: &str = "🛑";
}

/// Constants that define the appearance of progress bars and spinners.
pub mod progress_style {
    /// Characters used to represent filled, boundary, and empty portions of a progress bar.
    pub const PROGRESS_CHARS: &str = "=#-";

    /// Default width of the progress bar in characters.
    pub const BAR_WIDTH: usize = crate::constants::UI_BAR_WIDTH;

    /// Character displayed at the left edge of the progress bar.
    pub const BAR_LEFT: &str = "[";
    /// Character displayed at the right edge of the progress bar.
    pub const BAR_RIGHT: &str = "]";

    /// Characters used for the animated spinner frames (dash, forward slash, pipe, backslash).
    pub const SPINNER_CHARS: &str = "-/|\\";

    /// Template string for batch-processing progress (used with `indicatif`).
    pub const BATCH_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} [{bar:35.green/black}] {percent:>3}% * {pos}/{len} * {elapsed_precise} (ETA: {eta}) * {msg}";

    /// Template string for explore-mode progress (used with `indicatif`).
    pub const EXPLORE_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} [{bar:35.green/black}] {percent:>3}% * {elapsed} * {msg}";

    /// Template string for compact progress display (used with `indicatif`).
    pub const COMPACT_TEMPLATE: &str =
        "{prefix:.cyan} [{bar:30.green/black}] {percent:>3}% ({pos}/{len}) {msg:.dim}";

    /// Template string for spinner-only display (used with `indicatif`).
    pub const SPINNER_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} * {elapsed} * {msg}";
}

const SPINNER_FRAMES: &[&str] = &["-", "/", "|", "\\"];
const SPINNER_DOTS: &[&str] = &["*", ".", "o", "O"];

static SPINNER_FRAME: AtomicU64 = AtomicU64::new(0);

/// Returns the next spinner animation frame (rotating through dash, slash, pipe, backslash).
pub fn spinner_frame() -> &'static str {
    let frame =
        usize::try_from(SPINNER_FRAME.fetch_add(1, Ordering::Relaxed)).unwrap_or_else(|_| {
            warn!("☢️ [ANOMALY] Spinner frame counter overflowed; resetting to 0");
            SPINNER_FRAME.store(0, Ordering::Relaxed);
            0
        });
    SPINNER_FRAMES
        .get(frame % SPINNER_FRAMES.len())
        .copied()
        .unwrap_or("-")
}

/// Returns the next spinner dots animation frame (rotating through asterisk, dot, small o, capital O).
pub fn spinner_dots() -> &'static str {
    let frame =
        usize::try_from(SPINNER_FRAME.fetch_add(1, Ordering::Relaxed)).unwrap_or_else(|_| {
            warn!("☢️ [ANOMALY] Spinner frame counter overflowed; resetting to 0");
            SPINNER_FRAME.store(0, Ordering::Relaxed);
            0
        });
    SPINNER_DOTS
        .get(frame % SPINNER_DOTS.len())
        .copied()
        .unwrap_or("*")
}

/// Visual style variants for progress bar rendering.
#[derive(Clone, Copy)]
pub enum ProgressStyle {
    /// Classic Unicode block-progress using filled/empty block characters.
    Classic,
    /// Minimalist ASCII-style using equals signs and dashes.
    Modern,
    /// Shaded Unicode blocks with a gradient-like edge effect.
    Gradient,
    /// Fine-grained Unicode blocks with smooth fill transitions.
    Blocks,
}

/// Renders a text-based progress bar of the given `width`.
///
/// `progress` is clamped to the range `0.0..=1.0`, where `1.0` represents 100%.
/// The visual appearance is determined by the `style` variant.
/// # Errors
/// Returns an error if the progress bar cannot be rendered due to invalid parameters.
pub fn render_progress_bar(
    progress: f64,
    width: usize,
    style: ProgressStyle,
) -> crate::unified_error::Result<String> {
    let progress = progress.clamp(0.0, 1.0);
    let filled = crate::numeric_cast::f64_to_usize_strict(
        (progress * crate::numeric_cast::usize_to_f64(width)).round(),
        "progress_filled",
    )
    .ok_or_else(|| {
        crate::unified_error::ImgQualityError::NumericError(
            "Failed to calculate progress bar filled width".into(),
        )
    })?;
    let empty = width.saturating_sub(filled);

    match style {
        ProgressStyle::Classic => Ok(format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))),
        ProgressStyle::Modern => Ok(format!("{}{}", "=".repeat(filled), "-".repeat(empty))),
        ProgressStyle::Gradient => {
            let mut bar = String::new();
            for i in 0..width {
                if i < filled {
                    bar.push('▓');
                } else if i == filled && progress > 0.0 {
                    bar.push('▒');
                } else {
                    bar.push('░');
                }
            }
            Ok(bar)
        }
        ProgressStyle::Blocks => {
            let mut bar = String::new();
            for i in 0..width {
                let pos =
                    crate::numeric_cast::usize_to_f64(i) / crate::numeric_cast::usize_to_f64(width);
                if pos < progress - 0.1 {
                    bar.push('█');
                } else if pos < progress - 0.05 {
                    bar.push('▓');
                } else if pos < progress {
                    bar.push('▒');
                } else {
                    bar.push('░');
                }
            }
            Ok(bar)
        }
    }
}

/// Renders a progress bar with color-coded segments based on completion percentage.
///
/// The bar color transitions from red (low) to yellow, cyan, and green (high).
/// # Errors
/// Returns an error if the colored progress bar cannot be rendered due to invalid parameters.
pub fn render_colored_progress(
    progress: f64,
    width: usize,
) -> crate::unified_error::Result<String> {
    use colors::{BRIGHT_CYAN, BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, RESET};

    let bar = render_progress_bar(progress, width, ProgressStyle::Modern)?;
    let Some(pct) = crate::numeric_cast::f64_to_u32_strict(progress * 100.0, "progress_pct") else {
        // Critical error: Progress calculation failed strictly.
        // Since the function returns (), we cannot propagate an error.
        // Defaulting to 0 for UI display as a fallback, but this is not ideal and logs the issue.
        // This indicates a problem with the input 'progress' value (NaN, Inf, negative, or out of range).
        error!(
            "Strict progress calculation failed for pct. Progress value was likely invalid. Defaulting to 0% for UI display."
        );
        return Err(crate::unified_error::ImgQualityError::NumericError(
            "Progress calculation failed".to_string(),
        ));
    };

    let color = if pct >= crate::constants::UI_PROGRESS_BAR_HIGH_THRESHOLD {
        BRIGHT_GREEN
    } else if pct >= crate::constants::UI_PROGRESS_BAR_MEDIUM_THRESHOLD {
        BRIGHT_CYAN
    } else if pct >= crate::constants::UI_PROGRESS_BAR_LOW_THRESHOLD {
        BRIGHT_YELLOW
    } else {
        BRIGHT_RED
    };

    Ok(format!("{color}{bar}{RESET}"))
}

/// Tracks the state of a CRF exploration progress, displaying real-time updates.
pub struct ExploreProgressState {
    /// Current stage label (e.g., "Exploring", "Finalizing").
    pub stage: String,
    /// Current CRF (Constant Rate Factor) value being tested.
    pub crf: f32,
    /// Percentage change in file size relative to the original.
    pub size_pct: f64,
    /// Structural Similarity Index Measure, if computed.
    pub ssim: Option<f64>,
    /// Number of iterations completed so far.
    pub iteration: u32,
    /// The best CRF found so far (one that reduced file size).
    pub best_crf: Option<f32>,
    /// The instant when this exploration started.
    pub start_time: Instant,
}

impl ExploreProgressState {
    /// Creates a new `ExploreProgressState` with the given `stage` label.
    #[must_use]
    pub fn new(stage: &str) -> Self {
        Self {
            stage: stage.to_string(),
            crf: 0.0,
            size_pct: 0.0,
            ssim: None,
            iteration: 0,
            best_crf: None,
            start_time: Instant::now(),
        }
    }

    /// Updates the state with new CRF, size percentage, and optional SSIM values,
    /// then prints the updated progress to stderr.
    pub fn update(&mut self, crf: f32, size_pct: f64, ssim: Option<f64>) {
        self.crf = crf;
        self.size_pct = size_pct;
        self.ssim = ssim;
        self.iteration += 1;

        if size_pct < 0.0_f64 {
            self.best_crf = Some(crf);
        }

        self.display();
    }

    /// Displays the current progress inline on stderr, overwriting the previous line.
    pub fn display(&self) {
        use colors::{BRIGHT_GREEN, BRIGHT_YELLOW, CYAN, DIM, RESET};
        use symbols::{BULLET, SAVE, WARNING};

        // Pause output if the Ctrl+C confirmation prompt is currently waiting for input
        crate::ctrlc_guard::wait_if_prompt_active();

        let elapsed = self.start_time.elapsed().as_secs_f64();

        let (_size_icon, size_color) = if self.size_pct < 0.0_f64 {
            (SAVE, BRIGHT_GREEN)
        } else {
            (WARNING, BRIGHT_YELLOW)
        };

        let ssim_str = self
            .ssim
            .map(|s| format!(" {DIM}SSIM {s:.4}{RESET}"))
            .unwrap_or_default();

        let best_str = self
            .best_crf
            .map(|b| format!(" {DIM}Best: {b:.1}{RESET}"))
            .unwrap_or_default();

        eprint!(
            "\r\x1b[K{} {}{}{} {} CRF {:.1} {} {}{:+.1}%{}{}{} {} {}{:.1}s{}",
            spinner_frame(),
            CYAN,
            self.stage,
            RESET,
            BULLET,
            self.crf,
            BULLET,
            size_color,
            self.size_pct,
            RESET,
            ssim_str,
            best_str,
            BULLET,
            DIM,
            elapsed,
            RESET
        );
        let _ = io::stderr().flush();
    }

    /// Prints a final summary of the exploration results, including quality rating.
    pub fn finish(&self, final_crf: f32, final_size_pct: f64, final_ssim: Option<f64>) {
        use colors::{BRIGHT_GREEN, BRIGHT_YELLOW, RESET};
        use symbols::{CHECK, SAVE, SUCCESS, WARNING};

        let elapsed = self.start_time.elapsed().as_secs_f64();

        crate::progress_mode::emit_stderr("\r\x1b[K");

        let (ssim_str, ssim_rating) = match final_ssim {
            Some(s) if s >= crate::constants::UI_QUALITY_EXCELLENT_THRESHOLD => {
                (format!("SSIM {s:.4}"), format!("{SUCCESS} Excellent"))
            }
            Some(s) if s >= crate::constants::UI_QUALITY_VERY_GOOD_THRESHOLD => {
                (format!("SSIM {s:.4}"), format!("{SUCCESS} Very Good"))
            }
            Some(s) if s >= crate::constants::UI_QUALITY_GOOD_THRESHOLD => {
                (format!("SSIM {s:.4}"), format!("{CHECK}  Good"))
            }
            Some(s) => (format!("SSIM {s:.4}"), format!("{WARNING}  Fair")),
            None => (String::new(), String::new()),
        };

        let size_str = if final_size_pct < 0.0_f64 {
            format!("{BRIGHT_GREEN}{final_size_pct:+.1}%{RESET} {SAVE}")
        } else {
            format!("{BRIGHT_YELLOW}{final_size_pct:+.1}%{RESET}")
        };

        crate::progress_mode::emit_stderr(&format!(
            "{} {}Result:{} CRF {:.1} {} {} {} {} {} {} iter {} {:.1}s",
            symbols::SUCCESS,
            colors::BOLD,
            colors::RESET,
            final_crf,
            symbols::BULLET,
            size_str,
            symbols::BULLET,
            ssim_str,
            ssim_rating,
            symbols::BULLET,
            self.iteration,
            elapsed
        ));
    }
}

/// Prints a boxed result with a decorative border, centered title, and content lines.
pub fn print_result_box(title: &str, lines: &[&str]) {
    use colors::{BOLD, MFB_BLUE, RESET};

    let max_width = lines
        .iter()
        .map(|l| strip_ansi(l).len())
        .max()
        .unwrap_or_else(|| {
            // If no lines, use title length or default
            strip_ansi(title).len().max(40)
        })
        .max(strip_ansi(title).len())
        .max(50);

    let box_width = max_width + 4;
    let theme_color = MFB_BLUE;
    let accent_color = crate::modern_ui::colors::ACCENT;

    crate::progress_mode::emit_stderr(&format!(
        "{}╭{}╮{}",
        theme_color,
        "─".repeat(box_width),
        RESET
    ));

    let title_stripped = strip_ansi(title);
    let title_padding_total = box_width - title_stripped.len() - 2;
    let title_padding_left = title_padding_total / 2;
    let title_padding_right = title_padding_total - title_padding_left;

    crate::progress_mode::emit_stderr(&format!(
        "{}│{} {}{}{}{} {}{}│{}",
        theme_color,
        " ".repeat(title_padding_left),
        BOLD,
        accent_color,
        title,
        RESET,
        " ".repeat(title_padding_right),
        theme_color,
        RESET
    ));

    crate::progress_mode::emit_stderr(&format!(
        "{}├{}┤{}",
        theme_color,
        "─".repeat(box_width),
        RESET
    ));

    for line in lines {
        let line_stripped = strip_ansi(line);
        let padding = box_width - line_stripped.len() - 2;
        crate::progress_mode::emit_stderr(&format!(
            "{}│{} {}{} {}│{}",
            theme_color,
            RESET,
            line,
            " ".repeat(padding),
            theme_color,
            RESET
        ));
    }

    crate::progress_mode::emit_stderr(&format!(
        "{}╰{}╯{}",
        theme_color,
        "─".repeat(box_width),
        RESET
    ));
}

/// Prints a green success banner with sparkle decorations.
pub fn print_success_banner(msg: &str) {
    use colors::{BOLD, MFB_GREEN, RESET};
    use symbols::SPARKLE;
    crate::progress_mode::emit_stderr(&format!(
        "\n   {BOLD}{MFB_GREEN}{SPARKLE} {msg}  {SPARKLE}{RESET}{RESET}"
    ));
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::new();
    let mut in_escape = false;

    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Prints a stage heading with a diamond prefix and bold title.
pub fn print_stage(_icon: &str, title: &str) {
    use colors::{BOLD, MFB_BLUE, RESET};
    crate::progress_mode::emit_stderr(&format!(
        "{} {} {}{}{}",
        MFB_BLUE,
        symbols::DIAMOND,
        BOLD,
        title,
        RESET
    ));
    let _ = io::stderr().flush();
}

/// Prints an indented sub-stage heading with a bullet prefix.
pub fn print_substage(title: &str) {
    use colors::RESET;
    crate::progress_mode::emit_stderr(&format!(
        "   {} {} {}{}",
        colors::DIM,
        symbols::BULLET,
        RESET,
        title
    ));
}

/// Prints a success message in bright green with a check indicator.
pub fn print_success(msg: &str) {
    use colors::{BRIGHT_GREEN, RESET};
    crate::log_info!(crate::static_logs::messages::LABEL_SYSTEM, msg);
    crate::progress_mode::emit_stderr(&format!(
        "{}{} {}{}",
        BRIGHT_GREEN,
        symbols::SUCCESS,
        msg,
        RESET
    ));
}

/// Prints a warning message in bright yellow with a warning indicator.
pub fn print_warning(msg: &str) {
    use colors::{BRIGHT_YELLOW, RESET};
    crate::log_anomaly!(crate::static_logs::messages::LABEL_SYSTEM, msg);
    crate::progress_mode::emit_stderr(&format!(
        "{}{} {}{}",
        BRIGHT_YELLOW,
        symbols::WARNING,
        msg,
        RESET
    ));
}

/// Prints an error message in bright red with an error indicator.
pub fn print_error(msg: &str) {
    use colors::{BRIGHT_RED, RESET};
    crate::log_anomaly!(crate::static_logs::messages::LABEL_SYSTEM, msg);
    crate::progress_mode::emit_stderr(&format!(
        "{}{} {}{}",
        BRIGHT_RED,
        symbols::ERROR,
        msg,
        RESET
    ));
}

/// Prints an informational message in bright cyan with an info indicator.
pub fn print_info(msg: &str) {
    use colors::{BRIGHT_CYAN, RESET};
    crate::log_info!(crate::static_logs::messages::LABEL_SYSTEM, msg);
    crate::progress_mode::emit_stderr(&format!(
        "{}{} {}{}",
        BRIGHT_CYAN,
        symbols::INFO,
        msg,
        RESET
    ));
}

/// # Errors
/// Returns an error if the duration formatting fails due to numeric overflow.
pub fn format_duration(secs: f64) -> crate::unified_error::Result<String> {
    if secs >= crate::constants::SECS_PER_HOUR_F64 {
        let h = crate::numeric_cast::f64_to_u32_strict(
            (secs / crate::constants::SECS_PER_HOUR_F64).floor(),
            "duration_h",
        )
        .ok_or_else(|| {
            crate::unified_error::ImgQualityError::NumericError("Duration hours overflow".into())
        })?;
        let m = crate::numeric_cast::f64_to_u32_strict(
            ((secs % crate::constants::SECS_PER_HOUR_F64) / crate::constants::SECS_PER_MIN_F64)
                .floor(),
            "duration_m_large",
        )
        .ok_or_else(|| {
            crate::unified_error::ImgQualityError::NumericError(
                "Duration minutes overflow (large)".into(),
            )
        })?;
        let s = crate::numeric_cast::f64_to_u32_strict(
            (secs % crate::constants::SECS_PER_MIN_F64).floor(),
            "duration_s_large",
        )
        .ok_or_else(|| {
            crate::unified_error::ImgQualityError::NumericError(
                "Duration seconds overflow (large)".into(),
            )
        })?;
        Ok(format!("{}", style(format!("{h}h {m:02}m {s:02}s")).cyan()))
    } else if secs >= crate::constants::SECS_PER_MIN_F64 {
        let m = crate::numeric_cast::f64_to_u32_strict(
            (secs / crate::constants::SECS_PER_MIN_F64).floor(),
            "duration_m_small",
        )
        .ok_or_else(|| {
            crate::unified_error::ImgQualityError::NumericError(
                "Duration minutes overflow (small)".into(),
            )
        })?;
        let s = crate::numeric_cast::f64_to_u32_strict(
            (secs % crate::constants::SECS_PER_MIN_F64).floor(),
            "duration_s_small",
        )
        .ok_or_else(|| {
            crate::unified_error::ImgQualityError::NumericError(
                "Duration seconds overflow (small)".into(),
            )
        })?;
        Ok(format!("{}", style(format!("{m}m {s:02}s")).cyan()))
    } else {
        Ok(format!("{}", style(format!("{secs:.1}s")).cyan()))
    }
}

#[must_use]
pub fn fmt_crf(crf: f32) -> String {
    format!("{}", style(format!("CRF {crf:.1}")).color256(39).bold())
}

#[must_use]
pub fn fmt_ssim(ssim: f64) -> String {
    let (color_ssim, grade) = if ssim >= crate::constants::SSIM_GRADE_EXCELLENT {
        (style(format!("{ssim:.4}")).color256(118).bold(), "🟢") // Premium Neon Green
    } else if ssim >= crate::constants::SSIM_GRADE_VERY_GOOD {
        (style(format!("{ssim:.4}")).color256(154), "🟡")
    } else if ssim >= crate::constants::SSIM_GRADE_GOOD {
        (style(format!("{ssim:.4}")).color256(220), "🟠")
    } else {
        (style(format!("{ssim:.4}")).color256(196), "🔴")
    };
    format!("SSIM {color_ssim} {grade}")
}

#[must_use]
pub fn fmt_size_pct(pct: f64) -> String {
    if pct < 0.0 {
        format!("{}", style(format!("{pct:+.1}%")).color256(118).bold())
    } else if pct < crate::constants::UI_SIZE_REDUCTION_THRESHOLD {
        format!("{}", style(format!("{pct:+.1}%")).color256(220))
    } else {
        format!("{}", style(format!("{pct:+.1}%")).color256(196))
    }
}

#[must_use]
pub const fn fmt_compress_status(compressed: bool) -> &'static str {
    if compressed { "✅" } else { "❌" }
}

#[must_use]
pub fn fmt_iterations(iter: u32, max: u32) -> String {
    let ratio = f64::from(iter) / f64::from(max);
    if ratio <= crate::constants::UI_ITERATION_RATIO_OK {
        format!("{}", style(format!("{iter}/{max}")).green())
    } else if ratio <= crate::constants::UI_ITERATION_RATIO_WARN {
        format!("{}", style(format!("{iter}/{max}")).yellow())
    } else {
        format!("{}", style(format!("{iter}/{max}")).red())
    }
}

#[must_use]
pub fn fmt_search_result(crf: f32, size_pct: f64, ssim: Option<f64>, compressed: bool) -> String {
    let status = fmt_compress_status(compressed);
    let size_str = fmt_size_pct(size_pct);

    ssim.map_or_else(
        || {
            format!(
                "   {} {} | {} {}",
                if compressed {
                    style("✓").green()
                } else {
                    style("✗").red()
                },
                fmt_crf(crf),
                size_str,
                status
            )
        },
        |s| {
            let ssim_str = fmt_ssim(s);
            format!(
                "   {} {} | {} | {}",
                if compressed {
                    style("✓").green()
                } else {
                    style("✗").red()
                },
                fmt_crf(crf),
                size_str,
                ssim_str
            )
        },
    )
}

pub fn fmt_final_result(crf: f32, size_pct: f64, ssim: Option<f64>, iterations: u32) -> String {
    let ssim_str = ssim.map_or_else(|| "---".to_string(), fmt_ssim);
    format!(
        "{} {} | {} | {} | {} iterations",
        style("RESULT:").green().bold(),
        fmt_crf(crf),
        fmt_size_pct(size_pct),
        ssim_str,
        style(iterations).cyan()
    )
}

pub fn print_header(title: &str) {
    crate::progress_mode::emit_stderr(&format!(
        "{}",
        style(format!("═══ {title} ═══")).cyan().bold()
    ));
}

pub fn print_separator() {
    crate::progress_mode::emit_stderr(&format!(
        "{}",
        style("─────────────────────────────────────────────").dim()
    ));
}

/// Formats a byte count into a human-readable string with appropriate units (B, KB, MB, GB).
#[must_use]
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    let (value, unit) = if bytes >= GB {
        (
            crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(GB),
            "GB",
        )
    } else if bytes >= MB {
        (
            crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(MB),
            "MB",
        )
    } else if bytes >= KB {
        (
            crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(KB),
            "KB",
        )
    } else {
        (crate::numeric_cast::u64_to_f64(bytes), "B")
    };
    format!("{}", style(format!("{value:.2} {unit}")).blue())
}

/// Formats a size-change percentage with color coding and decorative icons.
#[must_use]
pub fn format_size_change(pct: f64) -> String {
    use colors::{BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, RESET};

    if pct < -50.0 {
        format!("{}{:+.1}%{} {}", BRIGHT_GREEN, pct, RESET, symbols::SPARKLE)
    } else if pct < 0.0 {
        format!("{}{:+.1}%{} {}", BRIGHT_GREEN, pct, RESET, symbols::SAVE)
    } else if pct < 10.0 {
        format!("{BRIGHT_YELLOW}{pct:+.1}%{RESET}")
    } else {
        format!("{}{:+.1}%{} {}", BRIGHT_RED, pct, RESET, symbols::WARNING)
    }
}

/// Formats a signed byte difference into a human-readable string with a sign prefix.
#[must_use]
pub fn format_size_diff(diff_bytes: i64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    let abs_diff = diff_bytes.unsigned_abs();
    let sign = if diff_bytes >= 0 { "+" } else { "-" };

    if abs_diff >= MB {
        format!(
            "{}{:.1} MB",
            sign,
            crate::numeric_cast::u64_to_f64(abs_diff) / crate::numeric_cast::u64_to_f64(MB)
        )
    } else if abs_diff >= KB {
        format!(
            "{}{:.1} KB",
            sign,
            crate::numeric_cast::u64_to_f64(abs_diff) / crate::numeric_cast::u64_to_f64(KB)
        )
    } else {
        format!("{sign}{abs_diff} B")
    }
}

// ─── Macros ───

/// Prints critical message
#[macro_export]
macro_rules! log_term_critical {
    ($($arg:tt)*) => {{
        use $crate::modern_ui::terminal_logger;
        terminal_logger().print_critical(&format!($($arg)*));
    }};
}

/// Prints phase header
#[macro_export]
macro_rules! log_stage {
    ($title:expr, $desc:expr) => {{
        use $crate::modern_ui::terminal_logger;
        terminal_logger().print_stage($title, $desc);
    }};
}

/// Prints sub-phase header
#[macro_export]
macro_rules! log_substage {
    ($desc:expr) => {{
        use $crate::modern_ui::terminal_logger;
        terminal_logger().print_substage($desc);
    }};
}

/// Logs critical error
#[macro_export]
macro_rules! log_enhanced_critical {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        let colored = format!("\x1b[38;2;233;30;99m🚨 CRITICAL\x1b[0m {msg}");
        $crate::progress_mode::emit_stderr(&colored);
        tracing::error!("{msg}");
    }};
}

/// Logs success
#[macro_export]
macro_rules! log_enhanced_success {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::progress_mode::emit_stderr(&format!("✅ {msg}"));
        tracing::info!("{msg}");
    }};
}

/// Logs operation start
#[macro_export]
macro_rules! log_enhanced_start {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::progress_mode::emit_stderr(&format!("🚀 {msg}"));
        tracing::info!("{msg}");
    }};
}

/// Logs operation end
#[macro_export]
macro_rules! log_enhanced_end {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::progress_mode::emit_stderr(&format!("🏁 {msg}"));
        tracing::info!("{msg}");
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar() {
        let bar = render_progress_bar(0.5, 20, ProgressStyle::Modern).unwrap();
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(strip_ansi(&format_size(500)), "500.00 B");
        assert_eq!(strip_ansi(&format_size(1500)), "1.46 KB");
        assert_eq!(strip_ansi(&format_size(1_500_000)), "1.43 MB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(strip_ansi(&format_duration(5.5).unwrap()), "5.5s");
        assert_eq!(strip_ansi(&format_duration(65.0).unwrap()), "1m 05s");
        assert_eq!(strip_ansi(&format_duration(3665.0).unwrap()), "1h 01m 05s");
    }

    #[test]
    fn test_strip_ansi() {
        let s = "\x1b[31mRed\x1b[0m Text";
        assert_eq!(strip_ansi(s), "Red Text");
    }
}
