//! Logging Module - Unified logging system
//!
//! This module provides a unified logging system based on the tracing framework, supporting:
//! - Log output to the system temporary directory
//! - Log file size limits and automatic rotation
//! - Structured logging
//! - Detailed logs for external tool invocations
//!
//! # Examples
//!
//! ```no_run
//! use shared_utils::logging::{LogConfig, init_logging};
//! use tracing::{info, error};
//!
//! // Initialize logging system
//! let config = LogConfig::default();
//! init_logging("my_program", config).expect("Failed to initialize logging");
//!
//! // Use tracing macros for logging
//! info!("Program started");
//! error!(error = "something went wrong", "Operation failed");
//! ```

use crate::modern_ui::{colors, symbols};
use anyhow::{Context, Result};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tracing::field::Field;
use tracing::Level;
// use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    field::Visit,
    filter::FilterFn,
    fmt::{self, format::FormatEvent, writer::MakeWriter, FmtContext, FormatFields},
    layer::{Layer, SubscriberExt},
    registry::LookupSpan,
    util::SubscriberInitExt,
    EnvFilter,
};

struct ModernFormatter;

impl<S, N> FormatEvent<S, N> for ModernFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let metadata = event.metadata();
        let level = *metadata.level();
        let progress_line = crate::progress::active_progress_line();

        // Pause output if the Ctrl+C confirmation prompt is currently waiting for input
        crate::ctrlc_guard::wait_if_prompt_active();

        if progress_line.is_some() {
            write!(writer, "\r\x1b[K")?;
        }

        // 1. Level Design / Hierarchy
        let (icon, color, label) = match level {
            tracing::Level::ERROR => (symbols::ERROR, colors::MFB_RED, " ERR "),
            tracing::Level::WARN => (symbols::WARNING, colors::MFB_ORANGE, " WRN "),
            tracing::Level::INFO => ("", "", ""), // Info often has its own icons in the message
            tracing::Level::DEBUG => (symbols::DIAMOND, colors::MFB_CYAN, " DBG "),
            tracing::Level::TRACE => (symbols::BULLET, colors::MFB_PURPLE, " TRC "),
        };

        if level != tracing::Level::INFO {
            write!(writer, "{}{}{} ", color, icon, colors::RESET)?;
            write!(
                writer,
                "{}{}{}{} ",
                colors::DIM,
                color,
                label,
                colors::RESET
            )?;
        }

        // 2. Message and Fields
        {
            let mut visitor = FieldVisitor {
                writer: &mut writer,
                is_first: true,
                has_message: false,
            };
            event.record(&mut visitor);
        }

        // 3. Milestone Stats: Only append to WARN and ERROR for context (skip info for clean output)
        if level <= tracing::Level::WARN {
            let stats = crate::progress_mode::get_current_stats_string();
            // Align stats for tracing logs
            write!(writer, "  {stats}")?;
        }

        writeln!(writer)?;

        if let Some(progress_line) = progress_line {
            write!(writer, "\r\x1b[K{progress_line}")?;
        }

        Ok(())
    }
}

struct FieldVisitor<'a, 'b> {
    writer: &'a mut fmt::format::Writer<'b>,
    is_first: bool,
    has_message: bool,
}

impl Visit for FieldVisitor<'_, '_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // Filter out tracing internal metadata fields
        let field_name = field.name();
        if field_name.starts_with("log.") {
            return;
        }

        if field_name == "message" {
            let msg = format!("{value:?}");
            // Strip quotes from Debug format of string
            let msg = msg.trim_start_matches('"').trim_end_matches('"');
            let _ = write!(self.writer, "{msg}");
            self.has_message = true;
        } else {
            if !self.is_first || self.has_message {
                let _ = write!(self.writer, " ");
            }

            let _ = write!(
                self.writer,
                "{}{}={}{:?}{}",
                colors::DIM,
                field.name(),
                colors::RESET,
                value,
                colors::RESET
            );
            self.is_first = false;
        }
    }
}

// ── Current log level: so progress_mode direct writes respect the same level as the tracing filter ──
/// Cached level from `init_logging`; used by `progress_mode::write_to_log_at_level` so direct run-log writes respect the level.
static CURRENT_LOG_LEVEL: OnceLock<Level> = OnceLock::new();

/// Returns true if an event at this level should be logged. Uses tracing order: TRACE > DEBUG > INFO > WARN > ERROR (more verbose = greater).
///
/// So config INFO passes INFO, WARN, ERROR; config TRACE passes all.
pub fn should_log(level: Level) -> bool {
    match CURRENT_LOG_LEVEL.get() {
        Some(&current) => level <= current,
        None => true, // init not called yet, log everything
    }
}

// ── Run log forwarder: when progress_mode sets a run log file, tracing events are also written there ──

/// Store the "Logging system initialized" line so `progress_mode` can write it to the run log when it opens (run log is set after init).
fn store_init_message_for_run_log(msg: String) {
    let mut guard = INIT_MESSAGE_FOR_RUN_LOG.lock().unwrap_or_else(|err| {
        eprintln!("⚠️ [Logging] init-message mutex was poisoned; recovering state");
        err.into_inner()
    });
    *guard = Some(msg);
}

/// Take the stored init message and clear it so it is written to the run log exactly once.
pub fn take_init_message_for_run_log() -> Option<String> {
    let mut guard = INIT_MESSAGE_FOR_RUN_LOG.lock().unwrap_or_else(|err| {
        eprintln!("⚠️ [Logging] init-message mutex was poisoned during take; recovering state");
        err.into_inner()
    });
    guard.take()
}

static INIT_MESSAGE_FOR_RUN_LOG: Mutex<Option<String>> = Mutex::new(None);

/// Register a callback so that when tracing events are formatted, each line is also written to the run log.
///
/// Called by `progress_mode::set_log_file` so the run log gets complete output (all tracing + progress).
pub fn register_run_log_forwarder(f: Box<dyn Fn(&str) + Send>) {
    let mut guard = RUN_LOG_FORWARDER.lock().unwrap_or_else(|err| {
        eprintln!("⚠️ [Logging] run-log forwarder mutex was poisoned; recovering state");
        err.into_inner()
    });
    *guard = Some(f);
}

static RUN_LOG_FORWARDER: Mutex<Option<Box<dyn Fn(&str) + Send>>> = Mutex::new(None);
static RUN_LOG_BUFFER: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Writer used by the run-log layer: buffers output and forwards each complete line to the run log when a forwarder is set.
struct RunLogWriter;

impl Write for RunLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut buffer = RUN_LOG_BUFFER
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        buffer.extend_from_slice(buf);
        while let Some(i) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=i).collect();
            let line_str = String::from_utf8_lossy(&line);
            let stripped = strip_ansi_str(line_str.trim_end_matches('\n'));
            let guard = RUN_LOG_FORWARDER.lock().unwrap_or_else(|err| {
                eprintln!("⚠️ [Logging] run-log forwarder mutex was poisoned during write; recovering state");
                err.into_inner()
            });
            if let Some(ref f) = *guard {
                f(&stripped);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut buffer = RUN_LOG_BUFFER
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        if !buffer.is_empty() {
            let line = String::from_utf8_lossy(&buffer);
            let stripped = strip_ansi_str(line.trim_end_matches('\n'));
            if !stripped.is_empty() {
                let guard = RUN_LOG_FORWARDER.lock().unwrap_or_else(|err| {
                    eprintln!("⚠️ [Logging] run-log forwarder mutex was poisoned during flush; recovering state");
                    err.into_inner()
                });
                if let Some(ref f) = *guard {
                    f(&stripped);
                }
            }
            buffer.clear();
        }
        Ok(())
    }
}

struct RunLogMaker;

impl<'a> MakeWriter<'a> for RunLogMaker {
    type Writer = RunLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RunLogWriter
    }
}

/// Strip ANSI escape sequences from a string.
///
/// Handles all CSI sequences (`ESC [ <params> <final>` where final is `0x40–0x7E`),
/// including SGR colour codes (`ESC[…m`), cursor-movement codes, and others.
/// Non-escape characters (including multi-byte UTF-8) are passed through unchanged.
#[must_use]
pub fn strip_ansi_str(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // Consume ESC [ <params> <final_byte>, where final is 0x40..=0x7E
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (0x40..=0x7e).contains(&b) {
                    break;
                }
            }
        } else if let Some(ch) = s[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }
    out
}

/// Strip ANSI escape sequences (e.g. `\x1b[92m`) so log files are plain text, not raw codes.
fn strip_ansi_bytes(buf: &[u8]) -> Vec<u8> {
    let Ok(s) = std::str::from_utf8(buf) else {
        return buf.to_vec();
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
    const fn new(inner: W) -> Self {
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
        let mut w = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
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
            let mut w = self
                .inner
                .lock()
                .map_err(|e| io::Error::other(e.to_string()))?;
            w.write_all(&stripped)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_buffer()?;
        let mut w = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        w.flush()?;
        Ok(())
    }
}

// Safe: buffer is process-local; inner is Mutex<W> and W: Send.
unsafe impl<W: Write + Send> Send for StripAnsiWriter<W> {}

/// A writer that rotates files based on size and optionally limits the total number of files.
struct SizeRotatingAppender {
    log_dir: PathBuf,
    program_name: String,
    timestamp: String,
    max_file_size: u64,
    current_size: u64,
    current_seq: usize,
    current_file: Option<std::fs::File>,
}

impl SizeRotatingAppender {
    fn new(log_dir: PathBuf, program_name: &str, max_file_size: u64) -> Self {
        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        Self {
            log_dir,
            program_name: program_name.to_string(),
            timestamp,
            max_file_size,
            current_size: 0,
            current_seq: 0,
            current_file: None,
        }
    }

    fn open_current_file(&mut self) -> io::Result<&mut std::fs::File> {
        if let Some(ref mut file) = self.current_file {
            return Ok(file);
        }

        let file_name = if self.current_seq == 0 {
            format!("{}_{}.log", self.program_name, self.timestamp)
        } else {
            format!("{}_{}.{}.log", self.program_name, self.timestamp, self.current_seq)
        };
        let path = self.log_dir.join(file_name);
        
        // Ensure parent exists (though usually handled by init_logging)
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        
        let metadata = file.metadata()?;
        self.current_size = metadata.len();
        
        Ok(self.current_file.insert(file))
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.current_file = None;
        self.current_seq += 1;
        self.current_size = 0;
        self.open_current_file()?;
        Ok(())
    }
}

impl Write for SizeRotatingAppender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.max_file_size != u64::MAX && self.current_size + buf.len() as u64 > self.max_file_size {
            self.rotate()?;
        }
        
        let file = self.open_current_file()?;
        let written = file.write(buf)?;
        self.current_size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(ref mut file) = self.current_file {
            file.flush()?;
        }
        Ok(())
    }
}

/// Logging configuration. Default: TRACE level, no file count or size limit, system temp dir.
#[derive(Debug, Clone)]
pub struct LogConfig {
    pub log_dir: PathBuf,
    /// Max size per log file (bytes). Default `u64::MAX` = no limit.
    pub max_file_size: u64,
    /// Max number of log files to keep in `log_dir`; older ones are deleted. Default `usize::MAX` = no limit.
    pub max_files: usize,
    /// Minimum level (TRACE = most comprehensive).
    pub level: Level,
}

impl Default for LogConfig {
    fn default() -> Self {
        let log_dir = std::env::var("MFB_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        Self {
            log_dir,
            // 50 MiB default limit for "easy to open" logs
            max_file_size: 50 * 1024 * 1024,
            max_files: 10,
            level: Level::TRACE,
        }
    }
}

impl LogConfig {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_log_dir<P: AsRef<Path>>(mut self, dir: P) -> Self {
        self.log_dir = dir.as_ref().to_path_buf();
        self
    }

    #[must_use]
    pub const fn with_max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    #[must_use]
    pub const fn with_max_files(mut self, count: usize) -> Self {
        self.max_files = count;
        self
    }

    #[must_use]
    pub const fn with_level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }
}

/// Initialize the logging system.
///
/// # Errors
/// Returns an error if initialization fails.
pub fn init_logging(program_name: &str, config: LogConfig) -> Result<()> {
    if std::env::var("FORCE_COLOR").is_ok() {
        console::set_colors_enabled(true);
        console::set_colors_enabled_stderr(true);
    }

    if CURRENT_LOG_LEVEL.set(config.level).is_err() {
        eprintln!("⚠️ [Logging] log level was already initialized earlier; keeping previous level");
    }
    std::fs::create_dir_all(&config.log_dir).with_context(|| {
        format!(
            "Failed to create log directory: {}",
            config.log_dir.display()
        )
    })?;

    let file_appender = SizeRotatingAppender::new(config.log_dir.clone(), program_name, config.max_file_size);
    let file_writer = Mutex::new(StripAnsiWriter::new(file_appender));

    // Registry: config.level has real effect (TRACE = all; INFO = info+; etc.). RUST_LOG overrides when set.
    let registry_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "{}={},shared_utils={}",
            program_name, config.level, config.level
        ))
    });

    // Temp file + run log: all events that pass the registry filter (level and targets).
    // StripAnsiWriter strips \x1b[...m so log files are plain text.
    let file_layer = fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_line_number(false);

    // Run log: same as file_layer — no target filter; when forwarder is set, receives every tracing event.
    let run_log_layer = fmt::layer()
        .with_writer(RunLogMaker)
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_line_number(false);

    // Stderr (terminal): filtered for display — exclude DEBUG level, no level/target in message.
    let stderr_layer = fmt::layer()
        .with_writer(io::stderr)
        .event_format(ModernFormatter)
        .with_filter(FilterFn::new(|m: &tracing::Metadata| {
            // Only show INFO, WARN, ERROR in terminal (no DEBUG or TRACE)
            m.level() <= &tracing::Level::INFO
        }));

    tracing_subscriber::registry()
        .with(registry_filter)
        .with(file_layer)
        .with(run_log_layer)
        .with(stderr_layer)
        .init();

    let log_file_name_display = format!("{program_name}_{}.log", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    let init_msg = format!(
        "Logging system initialized program=\"{}\" log_dir=\"{}\" log_file_pattern=\"{}\" max_file_size={} max_files={} level={:?}",
        program_name, config.log_dir.display(), log_file_name_display, config.max_file_size, config.max_files, config.level
    );
    // Note: We don't call append_stats_to_line here to avoid potential circular dependency during init.
    // The run log writer will handle it if we pass it through.

    tracing::debug!("{}", init_msg);
    store_init_message_for_run_log(init_msg);

    // Only prune old logs when an explicit limit is set (default usize::MAX = no limit).
    if config.max_files != usize::MAX {
        cleanup_old_logs(&config.log_dir, program_name, config.max_files)?;
    }

    Ok(())
}

fn cleanup_old_logs(log_dir: &Path, program_name: &str, max_files: usize) -> Result<()> {
    use std::fs;

    let entries = fs::read_dir(log_dir)
        .with_context(|| format!("Failed to read log directory: {}", log_dir.display()))?;

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
                match fs::metadata(&path) {
                    Ok(metadata) => match metadata.modified() {
                        Ok(modified) => log_files.push((path, modified)),
                        Err(err) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %err,
                                "Failed to read log file modification time during cleanup"
                            );
                        }
                    },
                    Err(err) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %err,
                            "Failed to read log file metadata during cleanup"
                        );
                    }
                }
            }
        }
    }

    if log_files.len() > max_files {
        log_files.sort_by_key(|f| std::cmp::Reverse(f.1));

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
            tracing::debug!(
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
                "☢️ External tool TERMINATED by signal (OOM kill or Crash). Check system logs (dmesg/Console.app) for details."
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

/// Execute an external tool and return its result.
///
/// # Errors
/// Returns an error if the tool fails to start.
pub fn execute_external_command(tool_name: &str, args: &[&str]) -> Result<ExternalCommandResult> {
    use std::process::Command;

    let command_str = format!("{} {}", tool_name, args.join(" "));

    tracing::debug!(
        tool = tool_name,
        command = %command_str,
        "Executing external command"
    );

    let start_time = std::time::Instant::now();

    let output = Command::new(tool_name)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute command: {command_str}"))?;

    let duration = start_time.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code();

    let combined_output = if !stdout.is_empty() && !stderr.is_empty() {
        format!("STDOUT:\n{stdout}\n\nSTDERR:\n{stderr}")
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

/// Execute an external command and ensure it succeeds.
///
/// # Errors
/// Returns an error if the command fails to start or exits with a non-zero status.
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
        assert_eq!(config.max_file_size, 50 * 1024 * 1024);
        assert_eq!(config.max_files, 10);
        assert_eq!(config.level, Level::TRACE);
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
            let file_path = temp_dir.path().join(format!("{program_name}.{i}.log"));
            fs::write(&file_path, format!("log content {i}")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        cleanup_old_logs(temp_dir.path(), program_name, 3).unwrap();

        let remaining_files: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
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
        assert!(
            result.duration.as_secs() <= 10,
            "echo should complete within 10s"
        );
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

    #[test]
    fn test_size_based_rotation() {
        let temp_dir = TempDir::new().unwrap();
        let program_name = "test_rotate_program";
        let max_size = 500; // 500 bytes limit
        
        let mut appender = SizeRotatingAppender::new(temp_dir.path().to_path_buf(), program_name, max_size);
        
        // Write enough to trigger rotation
        for i in 0..20 {
            let msg = format!("Log entry number {i} filling space\n");
            appender.write_all(msg.as_bytes()).unwrap();
        }
        appender.flush().unwrap();
        
        let files: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();
            
        assert!(files.len() >= 2);
    }
}
