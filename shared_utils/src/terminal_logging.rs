//! Terminal Logging Module - 终端日志模块
//!
//! 提供现代化、美观、颜色安全的终端日志输出
//!
//! ## 特性
//! - 自动颜色管理（防止溢出）
//! - 简洁的API
//! - ���一的视觉风格
//! - 调试级别控制


/// 颜色管理器 - 确保颜色正确关闭
pub struct ColorGuard {
    enabled: bool,
}

impl ColorGuard {
    /// 启用颜色
    pub fn enable() -> Self {
        Self { enabled: true }
    }

    /// 禁用颜色
    pub fn disable() -> Self {
        Self { enabled: false }
    }

    /// 应用颜色到文本
    pub fn colorize(&self, text: &str, ansi_code: &str) -> String {
        if self.enabled {
            format!("\x1b[{}m{}\x1b[0m", ansi_code, text)
        } else {
            text.to_string()
        }
    }
}

/// ANSI颜色代码
pub mod ansi {
    pub const RESET: &str = "0";
    pub const BOLD: &str = "1";
    pub const DIM: &str = "2";
    pub const ITALIC: &str = "3";
    pub const UNDERLINE: &str = "4";

    // 前景色
    pub const FG_BLACK: &str = "30";
    pub const FG_RED: &str = "31";
    pub const FG_GREEN: &str = "32";
    pub const FG_YELLOW: &str = "33";
    pub const FG_BLUE: &str = "34";
    pub const FG_MAGENTA: &str = "35";
    pub const FG_CYAN: &str = "36";
    pub const FG_WHITE: &str = "37";

    // 明亮前景色
    pub const FG_BRIGHT_BLACK: &str = "90";
    pub const FG_BRIGHT_RED: &str = "91";
    pub const FG_BRIGHT_GREEN: &str = "92";
    pub const FG_BRIGHT_YELLOW: &str = "93";
    pub const FG_BRIGHT_BLUE: &str = "94";
    pub const FG_BRIGHT_MAGENTA: &str = "95";
    pub const FG_BRIGHT_CYAN: &str = "96";
    pub const FG_BRIGHT_WHITE: &str = "97";

    // 背景色
    pub const BG_BLACK: &str = "40";
    pub const BG_RED: &str = "41";
    pub const BG_GREEN: &str = "42";
    pub const BG_YELLOW: &str = "43";
    pub const BG_BLUE: &str = "44";
    pub const BG_MAGENTA: &str = "45";
    pub const BG_CYAN: &str = "46";
    pub const BG_WHITE: &str = "47";
}

/// 终端日志助手
pub struct TerminalLogger {
    use_colors: bool,
    debug_mode: bool,
}

impl TerminalLogger {
    /// 创建新的终端日志器
    pub fn new(use_colors: bool, debug_mode: bool) -> Self {
        Self {
            use_colors,
            debug_mode,
        }
    }

    /// 应用颜色（如果启用）
    fn color(&self, text: &str, code: &str) -> String {
        if self.use_colors {
            format!("\x1b[{}m{}\x1b[0m", code, text)
        } else {
            text.to_string()
        }
    }

    /// 成功消息（绿色）
    pub fn success(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_GREEN)
    }

    /// 错误消息（红色）
    pub fn error(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_RED)
    }

    /// 警告消息（黄色）
    pub fn warning(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_YELLOW)
    }

    /// 信息消息（蓝色）
    pub fn info(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_BLUE)
    }

    /// 调试消息（青色）
    pub fn debug(&self, text: &str) -> String {
        self.color(text, ansi::FG_CYAN)
    }

    /// 关键消息（品红）
    pub fn critical(&self, text: &str) -> String {
        self.color(text, ansi::FG_BRIGHT_MAGENTA)
    }

    /// 值高亮（白色粗体）
    pub fn value(&self, text: &str) -> String {
        if self.use_colors {
            format!("\x1b[1;97m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }

    /// 打印成功消息
    pub fn print_success(&self, text: &str) {
        eprintln!("✅ {}", self.success(text));
    }

    /// 打印错误消息
    pub fn print_error(&self, text: &str) {
        eprintln!("❌ {}", self.error(text));
    }

    /// 打印警告消息
    pub fn print_warning(&self, text: &str) {
        eprintln!("⚠️  {}", self.warning(text));
    }

    /// 打印信息消息
    pub fn print_info(&self, text: &str) {
        eprintln!("ℹ️  {}", self.info(text));
    }

    /// 打印调试消息（仅在调试模式）
    pub fn print_debug(&self, text: &str) {
        if self.debug_mode {
            eprintln!("🔍 {}", self.debug(text));
        }
    }

    /// 打印关键消息
    pub fn print_critical(&self, text: &str) {
        eprintln!("🚨 {}", self.critical(text));
    }

    /// 打印阶段标题
    pub fn print_stage(&self, title: &str, description: &str) {
        eprintln!("▶ {}  {}", self.info(title), description);
    }

    /// 打印子阶段
    pub fn print_substage(&self, description: &str) {
        eprintln!("  └─ {}", description);
    }

    /// 打印分隔线
    pub fn print_separator(&self) {
        eprintln!("{}", "─".repeat(60));
    }

    /// 打印格式化的大小
    pub fn format_size(&self, bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!("{:.2} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.2} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.2} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} B", bytes)
        }
    }

    /// 打印大小变化
    pub fn print_size_change(&self, old: u64, new: u64) {
        let old_str = self.format_size(old);
        let new_str = self.format_size(new);
        let percent = if old > 0 {
            ((new as f64 - old as f64) / old as f64) * 100.0
        } else {
            0.0
        };

        let sign = if percent >= 0.0 { "+" } else { "" };
        let percent_str = format!("{}{:.1}%", sign, percent);

        let change_color = if percent < 0.0 {
            self.success(&percent_str)
        } else if percent > 5.0 {
            self.error(&percent_str)
        } else {
            self.warning(&percent_str)
        };

        eprintln!("{} → {} ({})", old_str, new_str, change_color);
    }
}

use std::sync::OnceLock;

/// 全局终端日志器实例
static GLOBAL_LOGGER: OnceLock<TerminalLogger> = OnceLock::new();

/// 初始化全局终端日志器
pub fn init_terminal_logger(use_colors: bool, debug_mode: bool) {
    let _ = GLOBAL_LOGGER.set(TerminalLogger::new(use_colors, debug_mode));
}

/// 获取全局终端日志器
pub fn terminal_logger() -> &'static TerminalLogger {
    GLOBAL_LOGGER
        .get()
        .expect("Terminal logger not initialized. Call init_terminal_logger first.")
}

// ─── 便捷宏 ─────────────────────────────────────────────────────────────────

/// 打印成功消息
#[macro_export]
macro_rules! log_success {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_success(&format!($($arg)*));
    }};
}

/// 打印错误消息
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_error(&format!($($arg)*));
    }};
}

/// 打印警告消息
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_warning(&format!($($arg)*));
    }};
}

/// 打印信息消息
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_info(&format!($($arg)*));
    }};
}

/// 打印调试消息
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_debug(&format!($($arg)*));
    }};
}

/// 打印关键消息
#[macro_export]
macro_rules! log_term_critical {
    ($($arg:tt)*) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_critical(&format!($($arg)*));
    }};
}

/// 打印阶段
#[macro_export]
macro_rules! log_stage {
    ($title:expr, $desc:expr) => {{
        use shared_utils::terminal_logging::terminal_logger;
        terminal_logger().print_stage($title, $desc);
    }};
}

/// 打印子阶段
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
