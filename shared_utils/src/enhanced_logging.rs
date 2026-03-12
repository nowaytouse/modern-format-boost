//! Enhanced Logging System - 增强的日志系统
//!
//! ## 功能特性
//! - 完整的日志级别层次 (ERROR > WARN > INFO > DEBUG > TRACE)
//! - 24位真彩色终端输出
//! - 结构化日志到文件（包含emoji、完整调用栈）
//! - 终端简洁输出（仅关键摘要）
//! - 禁止静默上游工具日志
//!
//! ## 设计原则
//! - 终端：仅显示关键信息和进度
//! - 文件：记录完整详细信息
//! - 颜色：现代、美观、一致的24位真彩色
//! - 透明度：忠实反映运行时状态，快速识别bug

use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use tracing::Level;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

// ─── Color Palette (24-bit True Color) ─────────────────────────────────────

/// 24位真彩色定义
pub mod colors {
    /// 成功绿色 - RGB(76, 175, 80)
    pub const SUCCESS: &str = "\x1b[38;2;76;175;80m";
    /// 警告黄色 - RGB(255, 193, 7)
    pub const WARNING: &str = "\x1b[38;2;255;193;7m";
    /// 错误红色 - RGB(244, 67, 54)
    pub const ERROR: &str = "\x1b[38;2;244;67;54m";
    /// 信息蓝色 - RGB(33, 150, 243)
    pub const INFO: &str = "\x1b[38;2;33;150;243m";
    /// 调试青色 - RGB(0, 188, 212)
    pub const DEBUG: &str = "\x1b[38;2;0;188;212m";
    /// 追踪紫色 - RGB(156, 39, 176)
    pub const TRACE: &str = "\x1b[38;2;156;39;176m";
    /// 关键品红 - RGB(233, 30, 99)
    pub const CRITICAL: &str = "\x1b[38;2;233;30;99m";
    /// 值橙色 - RGB(255, 152, 0)
    pub const VALUE: &str = "\x1b[38;2;255;152;0m";
    /// 重置所有样式
    pub const RESET: &str = "\x1b[0m";
    /// 粗体
    pub const BOLD: &str = "\x1b[1m";
    /// 暗淡
    pub const DIM: &str = "\x1b[2m";
}

/// 终端颜色助手
pub struct TerminalColor;

impl TerminalColor {
    /// 应用颜色到文本
    pub fn colorize(text: &str, color: &str) -> String {
        format!("{}{}{}", color, text, colors::RESET)
    }

    /// 成功消息（绿色）
    pub fn success(text: &str) -> String {
        Self::colorize(text, colors::SUCCESS)
    }

    /// 警告消息（黄色）
    pub fn warning(text: &str) -> String {
        Self::colorize(text, colors::WARNING)
    }

    /// 错误消息（红色）
    pub fn error(text: &str) -> String {
        Self::colorize(text, colors::ERROR)
    }

    /// 信息消息（蓝色）
    pub fn info(text: &str) -> String {
        Self::colorize(text, colors::INFO)
    }

    /// 调试消息（青色）
    pub fn debug(text: &str) -> String {
        Self::colorize(text, colors::DEBUG)
    }

    /// 追踪消息（紫色）
    pub fn trace(text: &str) -> String {
        Self::colorize(text, colors::TRACE)
    }

    /// 关键消息（品红）
    pub fn critical(text: &str) -> String {
        Self::colorize(text, colors::CRITICAL)
    }

    /// 值高亮（橙色）
    pub fn value(text: &str) -> String {
        Self::colorize(text, colors::VALUE)
    }

    /// 移除所有ANSI颜色码（用于文件日志）
    pub fn strip_ansi(text: &str) -> String {
        // 简单的 ANSI 转义序列移除（不依赖 regex）
        let mut result = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // 跳过转义序列 ESC[ ... m
                if chars.next() == Some('[') {
                    // 跳过直到找到字母
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

/// 日志级别图标
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

/// 格式化日志级别标签（带颜色和图标）
pub fn format_level(level: Level) -> String {
    match level {
        Level::ERROR => TerminalColor::error(&format!("{} ERROR", icons::ERROR)),
        Level::WARN => TerminalColor::warning(&format!("{} WARN ", icons::WARN)),
        Level::INFO => TerminalColor::info(&format!("{} INFO ", icons::INFO)),
        Level::DEBUG => TerminalColor::debug(&format!("{} DEBUG", icons::DEBUG)),
        Level::TRACE => TerminalColor::trace(&format!("{} TRACE", icons::TRACE)),
    }
}

/// 格式化日志级别标签（纯文本，用于文件）
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

/// 增强的日志级别分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Critical, // 最高优先级：数据丢失、损坏
    Error,    // 错误：操作失败
    Warn,     // 警告：潜在问题
    Info,     // 信息：正常操作
    Debug,    // 调试：详细诊断信息
    Trace,    // 追踪：最详细的执行路径
}

impl From<Level> for LogLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            Level::INFO => LogLevel::Info,
            Level::DEBUG => LogLevel::Debug,
            Level::TRACE => LogLevel::Trace,
        }
    }
}

impl LogLevel {
    /// 检查是否应该记录此级别
    pub fn should_log(self, max_level: LogLevel) -> bool {
        self <= max_level
    }

    /// 转换为tracing Level
    pub fn to_tracing_level(self) -> Level {
        match self {
            LogLevel::Critical | LogLevel::Error => Level::ERROR,
            LogLevel::Warn => Level::WARN,
            LogLevel::Info => Level::INFO,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Trace => Level::TRACE,
        }
    }
}

// ─── Terminal vs File Log Routing ───────────────────────────────────────────

/// 日志输出目标
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTarget {
    /// 终端输出（简洁、��色）
    Terminal,
    /// 文件输出（完整、纯文本）
    File,
    /// 两者都输出
    Both,
}

/// 日志路由器 - 根据内容决定输出目标
pub struct LogRouter {
    /// 当前日志级别
    max_level: LogLevel,
    /// 是否启用文件日志
    file_enabled: bool,
    /// 文件日志写入器（如果启用）
    file_writer: Option<Mutex<Box<dyn Write + Send>>>,
}

impl LogRouter {
    /// 创建新的日志路由器
    pub fn new(max_level: LogLevel) -> Self {
        Self {
            max_level,
            file_enabled: false,
            file_writer: None,
        }
    }

    /// 设置文件日志写入器
    pub fn set_file_writer(&mut self, writer: Box<dyn Write + Send>) {
        self.file_writer = Some(Mutex::new(writer));
        self.file_enabled = true;
    }

    /// 路由日志消息
    pub fn route(&self, level: LogLevel, _message: &str) -> LogTarget {
        // 始终输出到终端（根据级别过滤）
        // 详细调试信息仅输出到文件
        if level <= LogLevel::Info {
            LogTarget::Both
        } else {
            LogTarget::File
        }
    }

    /// 写入日志
    pub fn log(&self, level: LogLevel, message: &str, context: &str) {
        if !level.should_log(self.max_level) {
            return;
        }

        let target = self.route(level, message);

        match target {
            LogTarget::Terminal | LogTarget::Both => {
                self.write_terminal(level, message, context);
            }
            _ => {}
        }

        if (target == LogTarget::File || target == LogTarget::Both) && self.file_enabled {
            self.write_file(level, message, context);
        }
    }

    /// 写入终端（彩色、简洁）
    fn write_terminal(&self, level: LogLevel, message: &str, context: &str) {
        let level_str = format_level(level.to_tracing_level());
        let colored_msg = format!("{} {} {}", level_str, context, message);
        eprintln!("{}", colored_msg);
    }

    /// 写入文件（纯文本、完整）
    fn write_file(&self, level: LogLevel, message: &str, context: &str) {
        if let Some(ref writer) = self.file_writer {
            let plain_level = format_level_plain(level.to_tracing_level());
            let plain_msg = format!("{} {} {}", plain_level, context, message);
            if let Ok(mut w) = writer.lock() {
                let _ = writeln!(w, "{}", plain_msg);
                let _ = w.flush();
            }
        }
    }
}

// ─── Convenience Macros ─────────────────────────────────────────────────────

/// 记录关键错误
#[macro_export]
macro_rules! log_enhanced_critical {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        let colored = format!("\x1b[38;2;233;30;99m🚨 CRITICAL\x1b[0m {}", msg);
        eprintln!("{}", colored);
        tracing::error!("{}", msg);
    }};
}

/// 记录成功
#[macro_export]
macro_rules! log_enhanced_success {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        eprintln!("✅ {}", msg);
        tracing::info!("{}", msg);
    }};
}

/// 记录操作开始
#[macro_export]
macro_rules! log_enhanced_start {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        eprintln!("🚀 {}", msg);
        tracing::info!("{}", msg);
    }};
}

/// 记录操作结束
#[macro_export]
macro_rules! log_enhanced_end {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        eprintln!("🏁 {}", msg);
        tracing::info!("{}", msg);
    }};
}

// ─── Upstream Tool Logging ──────────────────────────────────────────────────

/// 上游工具日志记录器 - **绝不静默上游工具输出**
pub struct UpstreamToolLogger {
    tool_name: String,
}

impl UpstreamToolLogger {
    /// 创建上游工具日志记录器
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
        }
    }

    /// 记录工具命令
    pub fn log_command(&self, command: &str) {
        eprintln!(
            "\x1b[38;2;33;150;243m▶ {}\x1b[0m Executing: \x1b[38;2;255;152;0m{}\x1b[0m",
            self.tool_name, command
        );
        tracing::info!("[{}] Executing: {}", self.tool_name, command);
    }

    /// 记录工具输出
    pub fn log_output(&self, output: &str) {
        // 详细输出记录到文件，不在终端显示
        tracing::debug!("[{}] stdout: {}", self.tool_name, output);
    }

    /// 记录工具错误
    pub fn log_error(&self, error: &str) {
        tracing::warn!("[{}] stderr: {}", self.tool_name, error);
        // 错误始终显示在终端
        eprintln!(
            "⚠️  {} error: \x1b[38;2;244;67;54m{}\x1b[0m",
            self.tool_name, error
        );
    }

    /// 记录工具退出码
    pub fn log_exit(&self, exit_code: i32) {
        if exit_code == 0 {
            tracing::debug!("[{}] exited with code 0", self.tool_name);
        } else {
            eprintln!(
                "\x1b[38;2;233;30;99m🚨 CRITICAL\x1b[0m [{}] exited with non-zero code: {}",
                self.tool_name, exit_code
            );
            tracing::error!("[{}] exited with non-zero code: {}", self.tool_name, exit_code);
        }
    }
}

// ─── Integration with tracing ───────────────────────────────────────────────

/// 初始化增强的日志系统
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
            path.file_name().unwrap_or_else(|| std::ffi::OsStr::new("app.log")),
        );

        let file_layer = fmt::layer()
            .with_writer(file_appender)
            .with_ansi(false) // 文件中不使用颜色
            .with_span_events(FmtSpan::FULL);

        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry().with(filter).with(fmt_layer).init();
    }

    eprintln!(
        "🚀 {} logging initialized at level {:?}",
        program_name, log_level
    );
    tracing::info!("{} logging initialized at level {:?}", program_name, log_level);

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
        assert!(!plain.contains("\x1b"));
        assert!(plain.contains("Error message"));
    }
}
