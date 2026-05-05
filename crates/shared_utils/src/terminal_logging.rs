//! Terminal Logging Module
//!
//! Provides modern, aesthetic, and color-safe terminal log output
//!
//! ## Features
//! - Automatic color management (prevents overflow)
//! - Concise API
//! - Unified visual style
//! - Debug level control

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

/// ANSI color and style codes for terminal output.
pub mod ansi {
    /// Reset all styles and colors.
    pub const RESET: &str = "0";
    /// Bold text style.
    pub const BOLD: &str = "1";
    /// Dim/faint text style.
    pub const DIM: &str = "2";
    /// Italic text style.
    pub const ITALIC: &str = "3";
    /// Underline text style.
    pub const UNDERLINE: &str = "4";

    /// Foreground: Black
    pub const FG_BLACK: &str = "30";
    /// Foreground: Red
    pub const FG_RED: &str = "31";
    /// Foreground: Green
    pub const FG_GREEN: &str = "32";
    /// Foreground: Yellow
    pub const FG_YELLOW: &str = "33";
    /// Foreground: Blue
    pub const FG_BLUE: &str = "34";
    /// Foreground: Magenta
    pub const FG_MAGENTA: &str = "35";
    /// Foreground: Cyan
    pub const FG_CYAN: &str = "36";
    /// Foreground: White
    pub const FG_WHITE: &str = "37";

    /// Bright Foreground: Black
    pub const FG_BRIGHT_BLACK: &str = "90";
    /// Bright Foreground: Red
    pub const FG_BRIGHT_RED: &str = "91";
    /// Bright Foreground: Green
    pub const FG_BRIGHT_GREEN: &str = "92";
    /// Bright Foreground: Yellow
    pub const FG_BRIGHT_YELLOW: &str = "93";
    /// Bright Foreground: Blue
    pub const FG_BRIGHT_BLUE: &str = "94";
    /// Bright Foreground: Magenta
    pub const FG_BRIGHT_MAGENTA: &str = "95";
    /// Bright Foreground: Cyan
    pub const FG_BRIGHT_CYAN: &str = "96";
    /// Bright Foreground: White
    pub const FG_BRIGHT_WHITE: &str = "97";

    /// Background: Black
    pub const BG_BLACK: &str = "40";
    /// Background: Red
    pub const BG_RED: &str = "41";
    /// Background: Green
    pub const BG_GREEN: &str = "42";
    /// Background: Yellow
    pub const BG_YELLOW: &str = "43";
    /// Background: Blue
    pub const BG_BLUE: &str = "44";
    /// Background: Magenta
    pub const BG_MAGENTA: &str = "45";
    /// Background: Cyan
    pub const BG_CYAN: &str = "46";
    /// Background: White
    pub const BG_WHITE: &str = "47";
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
        self.color(text, ansi::FG_BRIGHT_GREEN)
    }

    /// Error message (Red)
    #[must_use]
    pub fn error(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_RED)
    }

    /// Warning message (Yellow)
    #[must_use]
    pub fn warning(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_YELLOW)
    }

    /// Info message (Blue)
    #[must_use]
    pub fn info(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_BLUE)
    }

    /// Debug message (Cyan)
    #[must_use]
    pub fn debug(&self, text: &str) -> String {
        self.color(text, ansi::FG_CYAN)
    }

    /// Critical message (Magenta)
    #[must_use]
    pub fn critical(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_MAGENTA)
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
        eprintln!("✅ {}", self.success(text));
    }

    /// Prints error message
    pub fn print_error(&self, text: &str) {
        eprintln!("❌ {}", self.error(text));
    }

    /// Prints warning message
    pub fn print_warning(&self, text: &str) {
        eprintln!("⚠️  {}", self.warning(text));
    }

    /// Prints info message
    pub fn print_info(&self, text: &str) {
        eprintln!("ℹ️  {}", self.info(text));
    }

    /// Prints debug message (Debug mode only)
    pub fn print_debug(&self, text: &str) {
        if self.debug_mode {
            eprintln!("🔍 {}", self.debug(text));
        }
    }

    /// Prints critical message
    pub fn print_critical(&self, text: &str) {
        eprintln!("🚨 {}", self.critical(text));
    }

    /// Prints stage title
    pub fn print_stage(&self, title: &str, description: &str) {
        eprintln!("▶ {}  {}", self.info(title), description);
    }

    /// Prints sub-stage
    pub fn print_substage(&self, description: &str) {
        eprintln!("  └─ {description}");
    }

    /// Prints separator line
    pub fn print_separator(&self) {
        eprintln!("{}", "─".repeat(60));
    }

    /// Prints formatted size
    #[must_use]
    pub fn format_size(&self, bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!(
                "{:.2} GB",
                crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(GB)
            )
        } else if bytes >= MB {
            format!(
                "{:.2} MB",
                crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(MB)
            )
        } else if bytes >= KB {
            format!(
                "{:.2} KB",
                crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(KB)
            )
        } else {
            format!("{bytes} B")
        }
    }

    /// Prints size change
    #[allow(
        clippy::missing_panics_doc,
        reason = "Explicit panic on data corruption is intended and documented inline."
    )]
    pub fn print_size_change(&self, old: u64, new: u64) {
        let old_str = self.format_size(old);
        let new_str = self.format_size(new);
        let percent = if old > 0 {
            let permille = u32::try_from((u128::from(new) * 10_000) / u128::from(old))
                .expect("Value overflowed or is missing, cannot process ratio");
            (f64::from(permille) / 100.0) - 100.0
        } else {
            0.0
        };

        let sign = if percent >= 0.0 { "+" } else { "" };
        let percent_str = format!("{sign}{percent:.1}%");

        let change_color = if percent < 0.0 {
            self.success(&percent_str)
        } else if percent > 5.0 {
            self.error(&percent_str)
        } else {
            self.warning(&percent_str)
        };

        eprintln!("{old_str} → {new_str} ({change_color})");
    }
}

use std::sync::OnceLock;

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
        eprintln!("⚠️ [Terminal Logger] init requested more than once; keeping first instance");
    }
}

/// Get global terminal logger instance
pub fn terminal_logger() -> &'static TerminalLogger {
    GLOBAL_LOGGER.get().unwrap_or_else(|| {
        if FALLBACK_LOGGER_WARNING.set(()).is_ok() {
            eprintln!(
                "⚠️ [Terminal Logger] used before initialization; falling back to a plain logger"
            );
        }
        &FALLBACK_LOGGER
    })
}

// ─── Convenience Macros ─────────────────────────────────────────────────────

/// Prints success message
#[macro_export]
macro_rules! log_success {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_success(&format!($($arg)*));
    }};
}

/// Prints error message
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_error(&format!($($arg)*));
    }};
}

/// Prints warning message
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_warning(&format!($($arg)*));
    }};
}

/// Prints info message
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_info(&format!($($arg)*));
    }};
}

/// Prints debug message
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_debug(&format!($($arg)*));
    }};
}

/// Prints critical message
#[macro_export]
macro_rules! log_term_critical {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_critical(&format!($($arg)*));
    }};
}

/// Prints phase header
#[macro_export]
macro_rules! log_stage {
    ($title:expr, $desc:expr) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_stage($title, $desc);
    }};
}

/// Prints sub-phase header
#[macro_export]
macro_rules! log_substage {
    ($desc:expr) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_substage($desc);
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_guard_enabled() {
        let guard = ColorGuard::enable();
        let colored = guard.colorize("test", ansi::FG_RED);
        assert!(colored.contains("\x1b["));
    }

    #[test]
    fn test_color_guard_disabled() {
        let guard = ColorGuard::disable();
        let colored = guard.colorize("test", ansi::FG_RED);
        assert!(!colored.contains("\x1b["));
        assert_eq!(colored, "test");
    }

    #[test]
    fn test_terminal_logger_format_size() {
        let logger = TerminalLogger::new(false, false);
        assert_eq!(logger.format_size(1024), "1.00 KB");
        assert_eq!(logger.format_size(1024 * 1024), "1.00 MB");
        assert_eq!(logger.format_size(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_terminal_logger_colors() {
        let logger = TerminalLogger::new(true, false);
        let success = logger.success("test");
        assert!(success.contains("\x1b["));
    }

    #[test]
    fn test_terminal_logger_no_colors() {
        let logger = TerminalLogger::new(false, false);
        let success = logger.success("test");
        assert!(!success.contains("\x1b["));
        assert_eq!(success, "test");
    }
}
