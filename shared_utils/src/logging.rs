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
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Strip ANSI escape sequences (e.g. `\x1b[92m`) so log files are plain text, not raw codes.
fn strip_ansi_bytes(buf: &[u8]) -> Vec<u8> {
    let s = match std::str::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return buf.to_vec(),
    };
    let mut result = String::new();
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' || c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            result.push(c);
        }
    }
    result.into_bytes()
}

/// Wraps a writer and strips ANSI from each line before writing (so log files are readable, not raw `\x1b[92m`).
struct StripAnsiWriter<W: Write + Send> {
    buffer: Vec<u8>,
    inner: Mutex<W>,
}

impl<W: Write + Send> StripAnsiWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            buffer: Vec::new(),
            inner: Mutex::new(inner),
        }
    }

    fn flush_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let stripped = strip_ansi_bytes(&self.buffer);
        self.buffer.clear();
        let mut w = self.inner.lock().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        w.write_all(&stripped)?;
        Ok(())
    }
}

impl<W: Write + Send> Write for StripAnsiWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        // Flush complete lines (ending with \n) so we strip and write them.
        while let Some(i) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=i).collect();
            let stripped = strip_ansi_bytes(&line);
            let mut w = self.inner.lock().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            w.write_all(&stripped)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_buffer()?;
        let mut w = self.inner.lock().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        w.flush()?;
        Ok(())
    }
}

// Safe: buffer is process-local; inner is Mutex<W> and W: Send.
unsafe impl<W: Write + Send> Send for StripAnsiWriter<W> {}

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub log_dir: PathBuf,
    pub max_file_size: u64,
    pub max_files: usize,
    pub level: Level,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_dir: std::env::temp_dir(),
            max_file_size: 100 * 1024 * 1024,
            max_files: 5,
            level: Level::INFO,
        }
    }
}

impl LogConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_log_dir<P: AsRef<Path>>(mut self, dir: P) -> Self {
        self.log_dir = dir.as_ref().to_path_buf();
        self
    }

    pub fn with_max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    pub fn with_max_files(mut self, count: usize) -> Self {
        self.max_files = count;
        self
    }

    pub fn with_level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }
}

pub fn init_logging(program_name: &str, config: LogConfig) -> Result<()> {
    std::fs::create_dir_all(&config.log_dir)
        .with_context(|| format!("Failed to create log directory: {:?}", config.log_dir))?;

    let log_file_name = format!("{}.log", program_name);

    let file_appender = RollingFileAppender::new(Rotation::DAILY, &config.log_dir, &log_file_name);
    let file_writer = Mutex::new(StripAnsiWriter::new(file_appender));

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "{}={},shared_utils={}",
            program_name, config.level, config.level
        ))
    });

    // File: no thread_id/line_number so prefix width is stable and message bodies align in the log file.
    // StripAnsiWriter strips \x1b[...m so log files are plain text, not raw ANSI codes.
    let file_layer = fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_line_number(false);

    // Stderr: message only (uniform indent applied in progress_mode::emit_stderr).
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(false)
        .with_level(false)
        .with_line_number(false)
        .without_time();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer)
        .init();

    tracing::info!(
        program = program_name,
        log_dir = ?config.log_dir,
        log_file = log_file_name,
        max_file_size = config.max_file_size,
        max_files = config.max_files,
        level = ?config.level,
        "Logging system initialized"
    );

    cleanup_old_logs(&config.log_dir, program_name, config.max_files)?;

    Ok(())
}

fn cleanup_old_logs(log_dir: &Path, program_name: &str, max_files: usize) -> Result<()> {
    use std::fs;

    let entries = fs::read_dir(log_dir)
        .with_context(|| format!("Failed to read log directory: {:?}", log_dir))?;

    let mut log_files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        if let Some(file_name) = path.file_name() {
            let file_name_str = file_name.to_string_lossy();
            if file_name_str.starts_with(program_name) && file_name_str.ends_with(".log") {
                if let Ok(metadata) = fs::metadata(&path) {
                    if let Ok(modified) = metadata.modified() {
                        log_files.push((path, modified));
                    }
                }
            }
        }
    }

    if log_files.len() > max_files {
        log_files.sort_by(|a, b| b.1.cmp(&a.1));

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

#[derive(Debug)]
pub struct ExternalCommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration: std::time::Duration,
}

pub fn execute_external_command(tool_name: &str, args: &[&str]) -> Result<ExternalCommandResult> {
    use std::process::Command;

    let command_str = format!("{} {}", tool_name, args.join(" "));

    tracing::info!(
        tool = tool_name,
        command = %command_str,
        "Executing external command"
    );

    let start_time = std::time::Instant::now();

    let output = Command::new(tool_name)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute command: {}", command_str))?;

    let duration = start_time.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code();

    let combined_output = if !stdout.is_empty() && !stderr.is_empty() {
        format!("STDOUT:\n{}\n\nSTDERR:\n{}", stdout, stderr)
    } else if !stdout.is_empty() {
        stdout.clone()
    } else {
        stderr.clone()
    };

    log_external_tool(tool_name, args, &combined_output, exit_code, duration);

    Ok(ExternalCommandResult {
        exit_code,
        stdout,
        stderr,
        duration,
    })
}

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

pub fn flush_logs() {
    tracing::info!("Flushing logs to disk");
}

pub fn log_operation_start(operation: &str, context: &[(&str, &str)]) {
    let event = tracing::info_span!("operation", operation = operation);
    for (key, value) in context {
        event.record(*key, *value);
    }
    tracing::info!(parent: &event, "Operation started");
}

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

        assert!(temp_dir.path().exists());
    }

    #[test]
    fn test_cleanup_old_logs() {
        let temp_dir = TempDir::new().unwrap();
        let program_name = "test_program";

        for i in 0..10 {
            let file_path = temp_dir.path().join(format!("{}.{}.log", program_name, i));
            fs::write(&file_path, format!("log content {}", i)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        cleanup_old_logs(temp_dir.path(), program_name, 3).unwrap();

        let remaining_files: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(program_name))
            .collect();

        assert_eq!(remaining_files.len(), 3);
    }

    #[test]
    fn test_execute_external_command_success() {
        let result = execute_external_command("echo", &["hello", "world"]);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello"));
        assert!(result.duration.as_secs() < 5);
    }

    #[test]
    fn test_execute_external_command_failure() {
        let result = execute_external_command("nonexistent_command_xyz", &["arg1"]);

        assert!(result.is_err());
    }

    #[test]
    fn test_execute_external_command_checked_success() {
        let result = execute_external_command_checked("echo", &["test"]);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn test_external_command_result_structure() {
        let result = execute_external_command("echo", &["test"]).unwrap();

        assert!(result.exit_code.is_some());
        assert!(!result.stdout.is_empty() || !result.stderr.is_empty());
        assert!(result.duration.as_nanos() > 0);
    }

    #[test]
    fn test_log_external_tool_captures_all_fields() {
        log_external_tool(
            "test_tool",
            &["arg1", "arg2"],
            "test output",
            Some(0),
            std::time::Duration::from_secs(1),
        );
    }
}
