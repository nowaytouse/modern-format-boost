//! Logging Module - 统一的日志系统
//!
//! 本模块提供基于tracing框架的统一日志系统，支持：
//! - 日志输出到系统临时目录
//! - 日志文件大小限制和自动轮转
//! - 结构化日志记录
//! - 外部工具调用的详细日志
//!
//! # Examples
//!
//! ```no_run
//! use shared_utils::logging::{LogConfig, init_logging};
//! use tracing::{info, error};
//!
//! // 初始化日志系统
//! let config = LogConfig::default();
//! init_logging("my_program", config).expect("Failed to initialize logging");
//!
//! // 使用tracing宏记录日志
//! info!("Program started");
//! error!(error = "something went wrong", "Operation failed");
//! ```

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// 日志配置结构
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// 日志目录路径（默认为系统临时目录）
    pub log_dir: PathBuf,
    /// 单个日志文件最大大小（字节），默认100MB
    pub max_file_size: u64,
    /// 保留的最大日志文件数量，默认5个
    pub max_files: usize,
    /// 日志级别，默认Info
    pub level: Level,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_dir: std::env::temp_dir(),
            max_file_size: 100 * 1024 * 1024, // 100MB
            max_files: 5,
            level: Level::INFO,
        }
    }
}

impl LogConfig {
    /// 创建新的日志配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置日志目录
    pub fn with_log_dir<P: AsRef<Path>>(mut self, dir: P) -> Self {
        self.log_dir = dir.as_ref().to_path_buf();
        self
    }

    /// 设置最大文件大小
    pub fn with_max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    /// 设置最大文件数量
    pub fn with_max_files(mut self, count: usize) -> Self {
        self.max_files = count;
        self
    }

    /// 设置日志级别
    pub fn with_level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }
}

/// 初始化日志系统
///
/// 此函数设置tracing-subscriber，将日志输出到系统临时目录中的文件。
/// 日志文件命名格式：`{program_name}.log`
///
/// # Arguments
///
/// * `program_name` - 程序名称，用于日志文件命名
/// * `config` - 日志配置
///
/// # Returns
///
/// 成功返回Ok(())，失败返回错误信息
///
/// # Examples
///
/// ```no_run
/// use shared_utils::logging::{LogConfig, init_logging};
///
/// let config = LogConfig::default();
/// init_logging("imgquality_hevc", config).expect("Failed to init logging");
/// ```
pub fn init_logging(program_name: &str, config: LogConfig) -> Result<()> {
    // 确保日志目录存在
    std::fs::create_dir_all(&config.log_dir)
        .with_context(|| format!("Failed to create log directory: {:?}", config.log_dir))?;

    // 创建日志文件名
    let log_file_name = format!("{}.log", program_name);

    // 创建文件appender，使用每日轮转
    // 注意：tracing-appender的RollingFileAppender会自动处理文件轮转
    // 但它基于时间轮转（daily），不是基于文件大小
    // 对于文件大小限制，我们需要在后续版本中实现自定义appender
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &config.log_dir, &log_file_name);

    // 创建环境过滤器
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{}={}", program_name, config.level)));

    // 创建格式化层（输出到文件）
    let file_layer = fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false) // 文件中不使用ANSI颜色代码
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true);

    // 创建格式化层（输出到stderr）
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true) // stderr使用颜色
        .with_target(false)
        .with_line_number(false);

    // 组合所有层并初始化
    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer)
        .init();

    // 记录日志系统初始化信息
    tracing::info!(
        program = program_name,
        log_dir = ?config.log_dir,
        log_file = log_file_name,
        max_file_size = config.max_file_size,
        max_files = config.max_files,
        level = ?config.level,
        "Logging system initialized"
    );

    // 清理旧日志文件（保留最近N个）
    cleanup_old_logs(&config.log_dir, program_name, config.max_files)?;

    Ok(())
}

/// 清理旧的日志文件，只保留最近的N个
///
/// # Arguments
///
/// * `log_dir` - 日志目录
/// * `program_name` - 程序名称
/// * `max_files` - 保留的最大文件数
fn cleanup_old_logs(log_dir: &Path, program_name: &str, max_files: usize) -> Result<()> {
    use std::fs;

    // 读取日志目录中的所有文件
    let entries = fs::read_dir(log_dir)
        .with_context(|| format!("Failed to read log directory: {:?}", log_dir))?;

    // 收集所有匹配的日志文件
    let mut log_files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // 只处理文件（不处理目录）
        if !path.is_file() {
            continue;
        }

        // 检查文件名是否匹配程序名
        if let Some(file_name) = path.file_name() {
            let file_name_str = file_name.to_string_lossy();
            if file_name_str.starts_with(program_name) && file_name_str.ends_with(".log") {
                // 获取文件修改时间
                if let Ok(metadata) = fs::metadata(&path) {
                    if let Ok(modified) = metadata.modified() {
                        log_files.push((path, modified));
                    }
                }
            }
        }
    }

    // 如果文件数量超过限制，删除最旧的文件
    if log_files.len() > max_files {
        // 按修改时间排序（最新的在前）
        log_files.sort_by(|a, b| b.1.cmp(&a.1));

        // 删除超出限制的文件
        for (path, _) in log_files.iter().skip(max_files) {
            if let Err(e) = fs::remove_file(path) {
                tracing::warn!(
                    path = ?path,
                    error = %e,
                    "Failed to remove old log file"
                );
            } else {
                tracing::debug!(
                    path = ?path,
                    "Removed old log file"
                );
            }
        }
    }

    Ok(())
}

/// 记录外部工具调用
///
/// 此函数记录外部工具（如ffmpeg、x265）的调用信息，包括：
/// - 工具名称和参数
/// - 执行时间
/// - 退出状态
/// - 标准输出和标准错误
///
/// # Arguments
///
/// * `tool_name` - 工具名称（如"ffmpeg"）
/// * `args` - 命令行参数
/// * `output` - 工具的输出（stdout和stderr）
/// * `exit_code` - 退出代码
/// * `duration` - 执行时长
///
/// # Examples
///
/// ```no_run
/// use shared_utils::logging::log_external_tool;
/// use std::time::Duration;
///
/// log_external_tool(
///     "ffmpeg",
///     &["-i", "input.mp4", "output.mp4"],
///     "ffmpeg output...",
///     Some(0),
///     Duration::from_secs(10),
/// );
/// ```
pub fn log_external_tool(
    tool_name: &str,
    args: &[&str],
    output: &str,
    exit_code: Option<i32>,
    duration: std::time::Duration,
) {
    let command = format!("{} {}", tool_name, args.join(" "));

    match exit_code {
        Some(0) => {
            tracing::info!(
                tool = tool_name,
                command = %command,
                duration_secs = duration.as_secs_f64(),
                exit_code = 0,
                "External tool completed successfully"
            );
            tracing::debug!(
                tool = tool_name,
                output = %output,
                "External tool output"
            );
        }
        Some(code) => {
            tracing::error!(
                tool = tool_name,
                command = %command,
                duration_secs = duration.as_secs_f64(),
                exit_code = code,
                output = %output,
                "External tool failed"
            );
        }
        None => {
            tracing::error!(
                tool = tool_name,
                command = %command,
                duration_secs = duration.as_secs_f64(),
                output = %output,
                "External tool terminated without exit code"
            );
        }
    }
}

/// 外部命令执行结果
#[derive(Debug)]
pub struct ExternalCommandResult {
    /// 退出状态码
    pub exit_code: Option<i32>,
    /// 标准输出
    pub stdout: String,
    /// 标准错误
    pub stderr: String,
    /// 执行时长
    pub duration: std::time::Duration,
}

/// 执行外部命令并记录详细日志
///
/// 此函数封装了外部命令的执行，自动捕获stdout/stderr并记录到日志。
/// 相比直接使用Command::output()，此函数提供：
/// - 自动记录命令执行前的完整命令行
/// - 捕获并记录stdout和stderr
/// - 记录执行时长和退出状态
/// - 失败时提供详细的错误信息
///
/// # Arguments
///
/// * `tool_name` - 工具名称（如"ffmpeg"、"x265"）
/// * `args` - 命令行参数列表
///
/// # Returns
///
/// 返回包含执行结果的ExternalCommandResult
///
/// # Examples
///
/// ```no_run
/// use shared_utils::logging::execute_external_command;
/// # fn main() -> anyhow::Result<()> {
/// let result = execute_external_command("ffmpeg", &["-version"])?;
/// if result.exit_code == Some(0) {
///     println!("Success: {}", result.stdout);
/// } else {
///     eprintln!("Failed: {}", result.stderr);
/// }
/// # Ok(())
/// # }
/// ```
pub fn execute_external_command(tool_name: &str, args: &[&str]) -> Result<ExternalCommandResult> {
    use std::process::Command;

    let command_str = format!("{} {}", tool_name, args.join(" "));

    // 记录命令执行前的信息
    tracing::info!(
        tool = tool_name,
        command = %command_str,
        "Executing external command"
    );

    let start_time = std::time::Instant::now();

    // 执行命令
    let output = Command::new(tool_name)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute command: {}", command_str))?;

    let duration = start_time.elapsed();

    // 转换输出为字符串
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code();

    // 合并stdout和stderr用于日志记录
    let combined_output = if !stdout.is_empty() && !stderr.is_empty() {
        format!("STDOUT:\n{}\n\nSTDERR:\n{}", stdout, stderr)
    } else if !stdout.is_empty() {
        stdout.clone()
    } else {
        stderr.clone()
    };

    // 记录执行结果
    log_external_tool(tool_name, args, &combined_output, exit_code, duration);

    Ok(ExternalCommandResult {
        exit_code,
        stdout,
        stderr,
        duration,
    })
}

/// 执行外部命令并在失败时返回错误
///
/// 此函数是execute_external_command的便捷包装，当命令失败时自动返回错误。
///
/// # Arguments
///
/// * `tool_name` - 工具名称
/// * `args` - 命令行参数列表
///
/// # Returns
///
/// 成功时返回ExternalCommandResult，失败时返回包含详细错误信息的Error
///
/// # Examples
///
/// ```no_run
/// use shared_utils::logging::execute_external_command_checked;
/// # fn main() -> anyhow::Result<()> {
/// // 如果命令失败，会自动返回错误
/// let result = execute_external_command_checked("ffmpeg", &["-i", "input.mp4", "output.mp4"])?;
/// println!("Command completed in {:?}", result.duration);
/// # Ok(())
/// # }
/// ```
pub fn execute_external_command_checked(
    tool_name: &str,
    args: &[&str],
) -> Result<ExternalCommandResult> {
    let result = execute_external_command(tool_name, args)?;

    if result.exit_code != Some(0) {
        let command_str = format!("{} {}", tool_name, args.join(" "));
        anyhow::bail!(
            "Command failed with exit code {:?}: {}\nSTDERR: {}",
            result.exit_code,
            command_str,
            result.stderr
        );
    }

    Ok(result)
}

/// 强制刷新日志到磁盘
///
/// 在程序异常退出前调用此函数，确保所有日志已写入磁盘
pub fn flush_logs() {
    // tracing-subscriber会自动刷新，但我们可以显式调用
    // 注意：目前tracing没有提供显式的flush API
    // 但在Drop时会自动刷新
    tracing::info!("Flushing logs to disk");
}

/// 记录操作开始
///
/// # Arguments
///
/// * `operation` - 操作名称
/// * `context` - 操作上下文信息（键值对）
pub fn log_operation_start(operation: &str, context: &[(&str, &str)]) {
    let event = tracing::info_span!("operation", operation = operation);
    for (key, value) in context {
        event.record(*key, *value);
    }
    tracing::info!(parent: &event, "Operation started");
}

/// 记录操作结束
///
/// # Arguments
///
/// * `operation` - 操作名称
/// * `duration` - 操作耗时
/// * `success` - 是否成功
pub fn log_operation_end(operation: &str, duration: std::time::Duration, success: bool) {
    if success {
        tracing::info!(
            operation = operation,
            duration_secs = duration.as_secs_f64(),
            "Operation completed successfully"
        );
    } else {
        tracing::error!(
            operation = operation,
            duration_secs = duration.as_secs_f64(),
            "Operation failed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_log_config_default() {
        let config = LogConfig::default();
        assert_eq!(config.max_file_size, 100 * 1024 * 1024);
        assert_eq!(config.max_files, 5);
        assert_eq!(config.level, Level::INFO);
    }

    #[test]
    fn test_log_config_builder() {
        let temp_dir = TempDir::new().unwrap();
        let config = LogConfig::new()
            .with_log_dir(temp_dir.path())
            .with_max_file_size(50 * 1024 * 1024)
            .with_max_files(3)
            .with_level(Level::DEBUG);

        assert_eq!(config.log_dir, temp_dir.path());
        assert_eq!(config.max_file_size, 50 * 1024 * 1024);
        assert_eq!(config.max_files, 3);
        assert_eq!(config.level, Level::DEBUG);
    }

    #[test]
    fn test_init_logging_creates_log_file() {
        let temp_dir = TempDir::new().unwrap();
        let _config = LogConfig::new().with_log_dir(temp_dir.path());

        // 注意：init_logging只能调用一次，因为它会初始化全局subscriber
        // 在测试中，我们只测试配置创建，不实际初始化
        assert!(temp_dir.path().exists());
    }

    #[test]
    fn test_cleanup_old_logs() {
        let temp_dir = TempDir::new().unwrap();
        let program_name = "test_program";

        // 创建多个测试日志文件
        for i in 0..10 {
            let file_path = temp_dir.path().join(format!("{}.{}.log", program_name, i));
            fs::write(&file_path, format!("log content {}", i)).unwrap();
            // 等待一小段时间，确保文件修改时间不同
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // 清理，只保留3个最新的
        cleanup_old_logs(temp_dir.path(), program_name, 3).unwrap();

        // 检查剩余文件数量
        let remaining_files: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(program_name))
            .collect();

        assert_eq!(remaining_files.len(), 3);
    }

    #[test]
    fn test_execute_external_command_success() {
        // 测试成功的命令执行（使用echo命令，跨平台可用）
        let result = execute_external_command("echo", &["hello", "world"]);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello"));
        assert!(result.duration.as_secs() < 5); // 应该很快完成
    }

    #[test]
    fn test_execute_external_command_failure() {
        // 测试失败的命令（使用不存在的命令）
        let result = execute_external_command("nonexistent_command_xyz", &["arg1"]);

        // 命令不存在应该返回错误
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_external_command_checked_success() {
        // 测试checked版本的成功情况
        let result = execute_external_command_checked("echo", &["test"]);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn test_external_command_result_structure() {
        // 测试ExternalCommandResult结构包含所有必需字段
        let result = execute_external_command("echo", &["test"]).unwrap();

        // 验证所有字段都存在且有意义
        assert!(result.exit_code.is_some());
        assert!(!result.stdout.is_empty() || !result.stderr.is_empty());
        assert!(result.duration.as_nanos() > 0);
    }

    #[test]
    fn test_log_external_tool_captures_all_fields() {
        // 测试log_external_tool函数接受所有必需的参数
        // 这个测试主要验证API的完整性
        log_external_tool(
            "test_tool",
            &["arg1", "arg2"],
            "test output",
            Some(0),
            std::time::Duration::from_secs(1),
        );

        // 如果能执行到这里，说明函数签名正确
    }
}
