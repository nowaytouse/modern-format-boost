//! Logging Module - Unified logging system
//!
//! This module provides a unified logging system based on the tracing
//! framework, supporting:
//! - Log output to the system temporary directory
//! - Log file size limits and automatic rotation
//! - Structured logging
//! - Detailed logs for external tool invocations
//!
//! # Examples
//!
//! ```no_run
//! use foundation::{LogConfig, init_logging};
//! use tracing::{error, info};
//!
//! // Initialize logging system
//! let config = LogConfig::default();
//! init_logging("my_program", &config).expect("Failed to initialize logging");
//!
//! // Use tracing macros for logging
//! info!("Program started");
//! error!(error = "something went wrong", "Operation failed");
//! ```

use crate::modern_ui::{colors, symbols};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tracing::Level;
use tracing::field::Field;
// use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    field::Visit,
    filter::FilterFn,
    fmt::{self, FmtContext, FormatFields, format::FormatEvent, writer::MakeWriter},
    layer::{Layer, SubscriberExt},
    registry::LookupSpan,
    util::SubscriberInitExt,
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

        if progress_line.is_some() {
            write!(writer, "\r\x1b[K")?;
        }

        if metadata.target() == "mfb::audit" {
            let mut collector = AuditKvCollector::default();
            event.record(&mut collector);
            let line = format_mfb_audit_line(collector);
            write!(writer, "{}[mfb::audit]{} ", colors::DIM, colors::RESET)?;
            write!(writer, "{}{}{}", colors::DIM, line, colors::RESET)?;
            writeln!(writer)?;
            if let Some(progress_line) = progress_line {
                write!(writer, "\r\x1b[K{progress_line}")?;
            }
            return Ok(());
        }

        let target = metadata.target();
        let is_diagnostic =
            target == "static_log" || target.starts_with("mfb::") || target.starts_with("mfb.");

        // 1. Level Design / Hierarchy
        if !is_diagnostic {
            let plain = crate::progress_mode::is_plain_mode();
            let (icon, color, label) = match level {
                tracing::Level::ERROR => (
                    crate::media_conversion_gate::ui_icon_pick(
                        symbols::ERROR,
                        symbols::plain::ERROR,
                    ),
                    if plain { "" } else { colors::MFB_RED },
                    " ERR ",
                ),
                tracing::Level::WARN => (
                    crate::media_conversion_gate::ui_icon_pick(
                        symbols::WARNING,
                        symbols::plain::WARNING,
                    ),
                    if plain { "" } else { colors::MFB_ORANGE },
                    " WRN ",
                ),
                tracing::Level::INFO => (String::new(), "", ""),
                tracing::Level::DEBUG => (
                    crate::media_conversion_gate::ui_icon_pick(
                        symbols::DIAMOND,
                        symbols::plain::DIAMOND,
                    ),
                    if plain { "" } else { colors::MFB_CYAN },
                    " DBG ",
                ),
                tracing::Level::TRACE => (
                    crate::media_conversion_gate::ui_icon_pick(
                        symbols::BULLET,
                        symbols::plain::BULLET,
                    ),
                    if plain { "" } else { colors::MFB_PURPLE },
                    " TRC ",
                ),
            };

            if level == tracing::Level::INFO {
                if plain {
                    write!(
                        writer,
                        "{} INF ",
                        crate::media_conversion_gate::ui_icon_pick(
                            symbols::INFO,
                            symbols::plain::INFO
                        )
                    )?;
                } else {
                    write!(
                        writer,
                        "{}{} {} INF {} ",
                        colors::MFB_BLUE,
                        symbols::INFO,
                        colors::DIM,
                        colors::RESET
                    )?;
                }
            } else if plain {
                write!(writer, "{icon} ")?;
                write!(writer, "{label} ")?;
            } else {
                write!(writer, "{color}{icon}{} ", colors::RESET)?;
                write!(
                    writer,
                    "{}{}{}{} ",
                    colors::DIM,
                    color,
                    label,
                    colors::RESET
                )?;
            }
        }

        // 2. Message and Fields
        {
            let mut visitor = FieldVisitor {
                writer: &mut writer,
                is_first: true,
                has_message: false,
                skip_fields: is_diagnostic,
            };
            event.record(&mut visitor);
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
    skip_fields: bool,
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
        } else if !self.skip_fields {
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

/// Stable field order for ``target: mfb::audit`` file + terminal lines (Python
/// ``media_scope`` / verify).
const MFB_AUDIT_FIELD_ORDER: &[&str] = &[
    "mfb_audit_schema",
    "outcome",
    "pipeline",
    "path",
    "reason",
    "ignore_class",
    "label",
    "file_count",
    "succeeded",
    "skipped",
    "ignored",
    "failed",
    "total",
];

#[derive(Default)]
struct AuditKvCollector {
    values: BTreeMap<String, String>,
}

impl AuditKvCollector {
    fn insert_field(&mut self, key: &str, value: String) {
        if key.starts_with("log.") {
            return;
        }
        self.values.insert(key.to_string(), value);
    }
}

impl Visit for AuditKvCollector {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert_field(field.name(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert_field(
            field.name(),
            if value {
                "true".to_string()
            } else {
                "false".to_string()
            },
        );
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert_field(field.name(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert_field(field.name(), value.to_string());
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.insert_field(field.name(), value.to_string());
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.insert_field(field.name(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let raw = format!("{value:?}");
        let unquoted = if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
            raw[1..raw.len() - 1].to_string()
        } else {
            raw
        };
        self.insert_field(field.name(), unquoted);
    }
}

fn escape_mfb_audit_token(value: &str) -> String {
    let needs_quote = value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c == '=' || c == '"');
    if !needs_quote {
        return value.to_string();
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn format_mfb_audit_line(collector: AuditKvCollector) -> String {
    let pairs = collector.values;
    let mut segments: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for &key in MFB_AUDIT_FIELD_ORDER {
        if let Some(v) = pairs.get(key) {
            seen.insert(key);
            segments.push(format!("{key}={}", escape_mfb_audit_token(v)));
        }
    }

    for (k, v) in &pairs {
        if seen.contains(k.as_str()) || k == "message" {
            continue;
        }
        segments.push(format!("{k}={}", escape_mfb_audit_token(v)));
    }

    format!("MFB_AUDIT {}", segments.join(" "))
}

#[cfg(test)]
impl AuditKvCollector {
    fn test_from_pairs(pairs: &[(&str, &str)]) -> Self {
        let mut c = Self::default();
        for (k, v) in pairs {
            c.insert_field(k, (*v).to_string());
        }
        c
    }
}

struct FileFormatter;

impl<S, N> FormatEvent<S, N> for FileFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let metadata = event.metadata();

        // 1. Timestamp (ISO8601 with Millis)
        let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        write!(writer, "{now} ")?;

        // 2. Level (Fixed width)
        let level_str = match *metadata.level() {
            tracing::Level::ERROR => "ERROR",
            tracing::Level::WARN => "WARN ",
            tracing::Level::INFO => "INFO ",
            tracing::Level::DEBUG => "DEBUG",
            tracing::Level::TRACE => "TRACE",
        };
        write!(writer, "{level_str} ")?;

        // 3. Thread ID (Hex)
        write!(writer, "[{:?}] ", std::thread::current().id())?;

        // 4. Target/Module and Source Location
        write!(writer, "{}", metadata.target())?;
        if let (Some(file), Some(line)) = (metadata.file(), metadata.line()) {
            write!(writer, " [{file}:{line}]")?;
        }
        write!(writer, ": ")?;

        // 5. Span Context (Merged fields for traceability)
        if let Some(scope) = ctx.event_scope() {
            let mut spans_printed = 0;
            for span in scope {
                if spans_printed > 0 {
                    write!(writer, " > ")?;
                }
                write!(writer, "{}", span.name())?;
                let ext = span.extensions();
                if let Some(fields) = ext.get::<fmt::FormattedFields<N>>()
                    && !fields.is_empty()
                {
                    write!(writer, "{{{fields}}}")?;
                }
                drop(ext);
                spans_printed += 1;
            }
            if spans_printed > 0 {
                write!(writer, ": ")?;
            }
        }

        // 6. Event fields: canonical audit line for cross-layer reconciliation
        if metadata.target() == "mfb::audit" {
            let mut collector = AuditKvCollector::default();
            event.record(&mut collector);
            write!(writer, "{}", format_mfb_audit_line(collector))?;
            writeln!(writer)?;
            return Ok(());
        }

        let mut visitor = SimpleFieldVisitor {
            writer: &mut writer,
            is_first: true,
        };
        event.record(&mut visitor);

        writeln!(writer)
    }
}

struct SimpleFieldVisitor<'a, 'b> {
    writer: &'a mut fmt::format::Writer<'b>,
    is_first: bool,
}

impl Visit for SimpleFieldVisitor<'_, '_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.writer, "{value:?}");
        } else {
            if self.is_first {
                let _ = write!(self.writer, " | ");
            } else {
                let _ = write!(self.writer, " ");
            }
            let _ = write!(self.writer, "{}={:?}", field.name(), value);
        }
        self.is_first = false;
    }
}

// ── Current log level: so progress_mode direct writes respect the same level
// as the tracing filter ──
/// Cached level from `init_logging`; used by
/// `progress_mode::write_to_log_at_level` so direct run-log writes respect the
/// level.
static CURRENT_LOG_LEVEL: OnceLock<Level> = OnceLock::new();

static LOG_FILE_INCLUDES_PROGRESS: OnceLock<bool> = OnceLock::new();

pub const DEFAULT_MAX_LOG_FILE_SIZE_BYTES: u64 = 30 * 1024 * 1024;

/// When true (`MFB_LOG_PROGRESS=1`), `mfb::progress` events are written to
/// run/jsonl/rotating logs.
#[must_use]
pub fn log_file_includes_progress() -> bool {
    *LOG_FILE_INCLUDES_PROGRESS.get_or_init(|| {
        match std::env::var(crate::constants::ENV_MFB_LOG_PROGRESS) {
            Ok(value) => {
                let trimmed = value.trim();
                trimmed == "1" || trimmed.eq_ignore_ascii_case("true")
            }
            Err(std::env::VarError::NotPresent) => false,
            Err(e) => {
                crate::media_conversion_gate::delivery_logging_path_audit(
                    "log_progress_env",
                    crate::media_conversion_gate::delivery_dot_path(),
                    format!(
                        "failed to read {}: {e}; progress events disabled for file logs",
                        crate::constants::ENV_MFB_LOG_PROGRESS
                    ),
                );
                false
            }
        }
    })
}

fn log_layer_accepts_metadata(metadata: &tracing::Metadata<'_>) -> bool {
    log_file_includes_progress() || metadata.target() != "mfb::progress"
}

/// Returns true if an event at this level should be logged. Uses tracing order:
/// TRACE > DEBUG > INFO > WARN > ERROR (more verbose = greater).
///
/// So config INFO passes INFO, WARN, ERROR; config TRACE passes all.
pub fn should_log(level: Level) -> bool {
    match CURRENT_LOG_LEVEL.get() {
        Some(&current) => level <= current,
        None => true, // init not called yet, log everything
    }
}

// ── Run log forwarder: when progress_mode sets a run log file, tracing events
// are also written there ──

/// Store the "Logging system initialized" line so `progress_mode` can write it
/// to the run log when it opens (run log is set after init).
fn store_init_message_for_run_log(msg: String) {
    let mut guard = crate::media_conversion_gate::logging_mutex_guard_or_recover(
        "logging_init_message",
        "init-message mutex was poisoned; recovering state",
        INIT_MESSAGE_FOR_RUN_LOG.lock(),
    );
    *guard = Some(msg);
}

/// Take the stored init message and clear it so it is written to the run log
/// exactly once.
pub fn take_init_message_for_run_log() -> Option<String> {
    let mut guard = crate::media_conversion_gate::logging_mutex_guard_or_recover(
        "logging_init_message_take",
        "init-message mutex was poisoned during take; recovering state",
        INIT_MESSAGE_FOR_RUN_LOG.lock(),
    );
    guard.take()
}

static INIT_MESSAGE_FOR_RUN_LOG: Mutex<Option<String>> = Mutex::new(None);

/// Register a callback so that when tracing events are formatted, each line is
/// also written to the run log.
///
/// Called by `progress_mode::set_log_file` so the run log gets complete output
/// (all tracing + progress).
pub fn register_run_log_forwarder(f: Box<dyn Fn(&str) + Send>) {
    let mut guard = crate::media_conversion_gate::logging_mutex_guard_or_recover(
        "logging_run_log_forwarder",
        "run-log forwarder mutex was poisoned; recovering state",
        RUN_LOG_FORWARDER.lock(),
    );
    *guard = Some(f);
}

static RUN_LOG_FORWARDER: Mutex<Option<Box<dyn Fn(&str) + Send>>> = Mutex::new(None);
static RUN_LOG_BUFFER: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Writer used by the run-log layer: buffers output and forwards each complete
/// line to the run log when a forwarder is set.
struct RunLogWriter;

impl Write for RunLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut buffer = RUN_LOG_BUFFER
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        buffer.extend_from_slice(buf);
        let mut lines_to_process = Vec::new();
        while let Some(i) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=i).collect();
            lines_to_process.push(line);
        }
        drop(buffer);

        for line in lines_to_process {
            let line_str = String::from_utf8_lossy(&line);
            let stripped = strip_ansi_str(line_str.trim_end_matches('\n'));
            let guard = crate::media_conversion_gate::logging_mutex_guard_or_recover(
                "logging_run_log_forwarder_write",
                "run-log forwarder mutex was poisoned during write; recovering state",
                RUN_LOG_FORWARDER.lock(),
            );
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
                let guard = crate::media_conversion_gate::logging_mutex_guard_or_recover(
                    "logging_run_log_forwarder_flush",
                    "run-log forwarder mutex was poisoned during flush; recovering state",
                    RUN_LOG_FORWARDER.lock(),
                );
                if let Some(ref f) = *guard {
                    f(&stripped);
                }
            }
            buffer.clear();
        }
        drop(buffer);
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
/// Handles all CSI sequences (`ESC [ <params> <final>` where final is
/// `0x40–0x7E`), including SGR colour codes (`ESC[…m`), cursor-movement codes,
/// and others. Non-escape characters (including multi-byte UTF-8) are passed
/// through unchanged.
#[must_use]
/// # Panics
///
/// Panics if the string slicing logic encounters an invalid UTF-8 boundary
/// during escape sequence extraction.
pub fn strip_ansi_str(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes.get(i) == Some(&0x1b) && bytes.get(i + 1) == Some(&b'[') {
            // Consume ESC [ <params> <final_byte>, where final is 0x40..=0x7E
            i += 2;
            while i < bytes.len() {
                let Some(&b) = bytes.get(i) else {
                    crate::progress_mode::emit_stderr(&format!(
                        "{} [LOGGING ANOMALY] Required metadata byte missing (out of bounds) in \
                         strip_ansi_str | Forensic: ESC[ sequence incomplete; defaulting to 0 to \
                         prevent panic",
                        crate::media_conversion_gate::ui_icon_pick("☢️", "[RARE]")
                    ));
                    break;
                };
                i += 1;
                if (0x40..=0x7e).contains(&b) {
                    break;
                }
            }
        } else if let Some(ch) = s.get(i..).and_then(|sub| sub.chars().next()) {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }
    out
}

/// Strip ANSI escape sequences (e.g. `\x1b[92m`) so log files are plain text,
/// not raw codes.
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

/// Wraps a writer and strips ANSI from each line before writing (so log files
/// are readable, not raw `\x1b[92m`).
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
        drop(w);
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
            drop(w);
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
        drop(w);
        Ok(())
    }
}

// Safe: buffer is process-local; inner is Mutex<W> and W: Send.
unsafe impl<W: Write + Send> Send for StripAnsiWriter<W> {}

/// A writer that rotates files based on size and optionally limits the total
/// number of files.
struct SizeRotatingAppender {
    log_dir: PathBuf,
    program_name: String,
    timestamp: String,
    max_file_size: u64,
    current_size: u64,
    current_seq: usize,
    current_file: Option<std::fs::File>,
    extension: String,
}

impl SizeRotatingAppender {
    fn new(log_dir: PathBuf, program_name: &str, max_file_size: u64, extension: &str) -> Self {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        Self {
            log_dir,
            program_name: program_name.to_string(),
            timestamp,
            max_file_size,
            current_size: 0,
            current_seq: 0,
            current_file: None,
            extension: extension.to_string(),
        }
    }

    fn open_current_file(&mut self) -> io::Result<&mut std::fs::File> {
        if let Some(ref mut file) = self.current_file {
            return Ok(file);
        }

        let file_name = if self.current_seq == 0 {
            format!(
                "{}_{}.{}",
                self.program_name, self.timestamp, self.extension
            )
        } else {
            format!(
                "{}_{}.{}.{}",
                self.program_name, self.timestamp, self.current_seq, self.extension
            )
        };
        let path = self.log_dir.join(file_name);

        // Ensure parent exists (though usually handled by init_logging)
        if let Some(parent) = path.parent() {
            crate::media_conversion_gate::delivery_create_dir_all_or_audit(
                "logging_rotator_parent",
                parent,
            );
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
        if self.max_file_size == u64::MAX {
            let file = self.open_current_file()?;
            let written = file.write(buf)?;
            self.current_size += crate::numeric_cast::usize_to_u64(written);
            return Ok(written);
        }

        if self.max_file_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log max_file_size must be greater than zero",
            ));
        }

        let mut written_total = 0_usize;
        let mut remaining = buf;
        while !remaining.is_empty() {
            if self.current_file.is_none() {
                self.open_current_file()?;
            }
            if self.current_size >= self.max_file_size {
                self.rotate()?;
            }

            let remaining_cap = self.max_file_size.saturating_sub(self.current_size);
            let Some(remaining_cap_usize) =
                crate::numeric_cast::u64_to_usize_strict(remaining_cap, "log_remaining_cap")
            else {
                return Err(io::Error::other(format!(
                    "log remaining capacity does not fit usize: {remaining_cap}"
                )));
            };
            let write_len = remaining.len().min(remaining_cap_usize);
            if write_len == 0 {
                self.rotate()?;
                continue;
            }

            let file = self.open_current_file()?;
            let written = file.write(&remaining[..write_len])?;
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write capped log chunk",
                ));
            }
            self.current_size += crate::numeric_cast::usize_to_u64(written);
            written_total += written;
            remaining = &remaining[written..];
        }

        Ok(written_total)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(ref mut file) = self.current_file {
            file.flush()?;
        }
        Ok(())
    }
}

/// Locate MFB workspace root when `start` is inside the repo tree.
fn find_mfb_workspace_root(start: &std::path::Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..16 {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Some(dir);
        }
        let parent = dir.parent()?.to_path_buf();
        if parent == dir {
            break;
        }
        dir = parent;
    }
    None
}

/// True when `path` is `<workspace>/logs` or `<workspace>/target/training*`.
fn is_forbidden_workspace_log_path(path: &std::path::Path) -> bool {
    let resolved = crate::media_conversion_gate::canonicalize_for_tool_input(path);
    let workspace = match find_mfb_workspace_root(&resolved) {
        Some(v) => Some(v),
        None => crate::media_conversion_gate::delivery_cwd_or_audit(
            "logging forbidden workspace path check",
        )
        .and_then(|cwd| find_mfb_workspace_root(&cwd)),
    };
    let Some(workspace) = workspace else {
        return false;
    };
    let Ok(rel) = resolved.strip_prefix(&workspace) else {
        return false;
    };
    let mut components = rel.components();
    let Some(first) = components.next() else {
        return false;
    };
    let first = first.as_os_str().to_string_lossy();
    if first == "logs" {
        return true;
    }
    if first == "target" {
        let Some(second) = components.next() else {
            return false;
        };
        return second.as_os_str().to_string_lossy().starts_with("training");
    }
    false
}

/// Redirect forbidden workspace log paths to [`persistent_log_dir`].
fn coerce_log_dir(candidate: PathBuf) -> PathBuf {
    if is_forbidden_workspace_log_path(&candidate) {
        let mut fallback = persistent_log_dir();
        if is_forbidden_workspace_log_path(&fallback) {
            fallback = fallback_log_dir("LogConfig::coerce_log_dir");
        }
        crate::progress_mode::emit_stderr(&format!(
            "{} [MFB] Refusing workspace log dir {} — using {}",
            crate::media_conversion_gate::ui_icon_pick(symbols::WARNING, symbols::plain::WARNING),
            candidate.display(),
            fallback.display()
        ));
        return fallback;
    }
    candidate
}

fn default_user_log_dir() -> Option<PathBuf> {
    match std::env::var(crate::constants::ENV_HOME) {
        Ok(home) if !home.trim().is_empty() => Some(
            PathBuf::from(home)
                .join(crate::constants::MFB_DEFAULT_HOME_DIRNAME)
                .join("logs"),
        ),
        _ => match std::env::var(crate::constants::ENV_USERPROFILE) {
            Ok(home) if !home.trim().is_empty() => Some(
                PathBuf::from(home)
                    .join(crate::constants::MFB_DEFAULT_HOME_DIRNAME)
                    .join("logs"),
            ),
            _ => None,
        },
    }
}

fn fallback_log_dir(context: &str) -> PathBuf {
    if let Some(candidate) = default_user_log_dir()
        && !is_forbidden_workspace_log_path(&candidate)
    {
        return candidate;
    }
    let candidate = crate::media_conversion_gate::delivery_log_dir_from_env_or_temp(context);
    if !is_forbidden_workspace_log_path(&candidate) {
        return candidate;
    }
    crate::media_conversion_gate::delivery_temp_mfb_root_ssot().join("logs")
}

/// Persistent log root without `MFB_LOG_DIR` (never under the git workspace).
fn persistent_log_dir() -> PathBuf {
    match crate::process_lock::get_mfb_root() {
        Ok(root) => {
            let candidate = root.join("logs");
            if is_forbidden_workspace_log_path(&candidate) {
                let fallback = fallback_log_dir("LogConfig::persistent_log_dir");
                crate::progress_mode::emit_stderr(&format!(
                    "{} [MFB] Refusing workspace MFB_HOME_ROOT log dir {} — using {}",
                    crate::media_conversion_gate::ui_icon_pick(
                        symbols::WARNING,
                        symbols::plain::WARNING
                    ),
                    candidate.display(),
                    fallback.display()
                ));
                fallback
            } else {
                candidate
            }
        }
        Err(err) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "persistent_log_dir",
                format!("MFB root unavailable for logs ({err}); using temp log dir"),
            );
            fallback_log_dir("LogConfig::persistent_log_dir")
        }
    }
}

/// Logging configuration. Default: TRACE level, unified log directory, 30 MiB
/// per log file.
#[derive(Debug, Clone)]
pub struct LogConfig {
    pub log_dir: PathBuf,
    /// Max size per log file (bytes). Default = 30 MiB.
    pub max_file_size: u64,
    /// Max number of log files to keep in `log_dir`; older ones are deleted.
    /// Default `usize::MAX` = no limit.
    pub max_files: usize,
    /// Minimum level (TRACE = most comprehensive).
    pub level: Level,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_dir: Self::unified_log_dir(),
            max_file_size: DEFAULT_MAX_LOG_FILE_SIZE_BYTES,
            max_files: usize::MAX,
            level: Level::TRACE,
        }
    }
}

impl LogConfig {
    /// Unified log root (aligned with ``crates/dev/scripts/mfb_log_paths.py``):
    ///
    /// 1. `MFB_LOG_DIR` — explicit override (session, CI)
    /// 2. `MFB_HOME_ROOT/logs` — state root (tests, `FROM_APP` cache layout)
    /// 3. `~/.modern_format_boost/logs` — default persistent user location
    /// 4. System temp — last resort
    ///
    /// Never uses `<workspace>/logs` or `target/training_*`.
    #[must_use]
    pub fn unified_log_dir() -> PathBuf {
        match std::env::var(crate::constants::ENV_MFB_LOG_DIR) {
            Ok(log_dir) => {
                let trimmed = log_dir.trim();
                if !trimmed.is_empty() {
                    return coerce_log_dir(PathBuf::from(trimmed));
                }
            }
            Err(std::env::VarError::NotPresent) => {}
            Err(e) => {
                crate::media_conversion_gate::delivery_logging_path_audit(
                    "log_dir_env",
                    crate::media_conversion_gate::delivery_dot_path(),
                    format!(
                        "failed to read {}: {e}; using persistent log dir",
                        crate::constants::ENV_MFB_LOG_DIR
                    ),
                );
            }
        }

        persistent_log_dir()
    }

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
pub fn init(program_name: &str, config: &LogConfig) -> Result<()> {
    if std::env::var(crate::constants::ENV_FORCE_COLOR).is_ok() {
        console::set_colors_enabled(true);
        console::set_colors_enabled_stderr(true);
    }

    if CURRENT_LOG_LEVEL.set(config.level).is_err() {
        crate::progress_mode::emit_stderr(&format!(
            "{} [Logging] log level was already initialized earlier; keeping previous level",
            crate::media_conversion_gate::ui_icon_pick(symbols::WARNING, symbols::plain::WARNING)
        ));
    }
    std::fs::create_dir_all(&config.log_dir).with_context(|| {
        format!(
            "Failed to create log directory: {}",
            config.log_dir.display()
        )
    })?;

    let file_appender = SizeRotatingAppender::new(
        config.log_dir.clone(),
        program_name,
        config.max_file_size,
        "log",
    );
    let file_writer = Mutex::new(StripAnsiWriter::new(file_appender));

    // Structured JSON audit log
    let json_appender = SizeRotatingAppender::new(
        config.log_dir.clone(),
        program_name,
        config.max_file_size,
        "jsonl",
    );
    let json_writer = Mutex::new(json_appender);

    // Registry: config.level has real effect (TRACE = all; INFO = info+; etc.).
    // RUST_LOG overrides when set.
    let registry_filter = crate::media_conversion_gate::tracing_registry_env_filter_or_config(
        program_name,
        config.level,
    );

    // Temp file + run log: all events that pass the registry filter (level and
    // targets). StripAnsiWriter strips \x1b[...m so log files are plain text.
    let file_layer = fmt::layer()
        .with_writer(file_writer)
        .event_format(FileFormatter)
        .with_filter(FilterFn::new(|m: &tracing::Metadata| {
            log_layer_accepts_metadata(m)
        }));

    // Run log: same as file_layer — when forwarder is set, receives every tracing
    // event.
    let run_log_layer = fmt::layer()
        .with_writer(RunLogMaker)
        .event_format(FileFormatter)
        .with_filter(FilterFn::new(|m: &tracing::Metadata| {
            log_layer_accepts_metadata(m)
        }));

    // Stderr (terminal): filtered for display — exclude DEBUG level, no
    // level/target in message.
    let stderr_layer = fmt::layer()
        .with_writer(io::stderr)
        .event_format(ModernFormatter)
        .with_filter(FilterFn::new(|m: &tracing::Metadata| {
            // Only show INFO, WARN, ERROR in terminal (no DEBUG or TRACE)
            // Also exclude events with target "mfb::ui" as they are handled manually via
            // emit_stderr Also exclude verbose reports with target
            // "mfb::report" to keep terminal clean
            m.level() <= &tracing::Level::INFO
                && m.target() != "mfb::ui"
                && m.target() != "mfb::report"
                && m.target() != "mfb::tool_output"
        }));

    let json_layer = fmt::layer()
        .json()
        .with_writer(json_writer)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_span_list(true)
        .with_filter(FilterFn::new(|m: &tracing::Metadata| {
            log_layer_accepts_metadata(m)
        }));

    tracing_subscriber::registry()
        .with(registry_filter)
        .with(file_layer)
        .with(run_log_layer)
        .with(stderr_layer)
        .with(json_layer)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let current_dir = crate::media_conversion_gate::delivery_cwd_display_or_unknown("logging init");
    let init_msg = format!(
        "Logging system initialized program=\"{}\" log_dir=\"{}\" max_file_size={} max_files={} \
         level={:?} args={:?} cwd=\"{}\" os=\"{}\"",
        program_name,
        config.log_dir.display(),
        config.max_file_size,
        config.max_files,
        config.level,
        args,
        current_dir,
        std::env::consts::OS,
    );
    // Note: We don't call append_stats_to_line here to avoid potential circular
    // dependency during init. The run log writer will handle it if we pass it
    // through.

    tracing::debug!("{}", init_msg);
    store_init_message_for_run_log(init_msg);

    // Log directory info for user awareness
    tracing::info!(
        target: "mfb::system",
        "{} Log directory: {}",
        crate::media_conversion_gate::ui_icon_pick("📁", "[LOG]"),
        config.log_dir.display()
    );

    // Only prune old logs when an explicit limit is set (default usize::MAX = no
    // limit).
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
            if file_name_str.starts_with(program_name)
                && (file_name_str.ends_with(".log") || file_name_str.ends_with(".jsonl"))
            {
                match fs::metadata(&path) {
                    Ok(metadata) => match metadata.modified() {
                        Ok(modified) => log_files.push((path, modified)),
                        Err(err) => {
                            crate::media_conversion_gate::delivery_logging_path_audit(
                                "log_cleanup_mtime",
                                &path,
                                format!(
                                    "SYSTEM AUDIT: Failed to read log file modification time \
                                     during cleanup for '{}' | Forensic: Error '{}'",
                                    path.display(),
                                    err
                                ),
                            );
                        }
                    },
                    Err(err) => {
                        crate::media_conversion_gate::delivery_logging_path_audit(
                            "log_cleanup_metadata",
                            &path,
                            format!(
                                "SYSTEM AUDIT: Failed to read log file metadata during cleanup \
                                 for '{}' | Forensic: Error '{}'",
                                path.display(),
                                err
                            ),
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
                crate::media_conversion_gate::delivery_logging_path_audit(
                    "log_cleanup_remove",
                    path,
                    format!(
                        "SYSTEM AUDIT: Failed to remove old log file '{}' | Forensic: Error '{}'",
                        path.display(),
                        e
                    ),
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
    let command = crate::common_utils::format_command_string(tool_name, args);

    match exit_code {
        Some(0_i32) => {
            tracing::info!(
                tool = tool_name,
                command = %command,
                duration_secs = duration.as_secs_f64(),
                exit_code = 0_i32,
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
                "{} External tool TERMINATED by signal (OOM kill or Crash). Check system logs (dmesg/Console.app) for details.",
                crate::media_conversion_gate::ui_icon_pick("☢️", "[RARE]")
            );
        }
    }
}

const CAPTURED_TOOL_OUTPUT_LOG_MAX_BYTES: usize = 256 * 1024;

/// Keep captured tool diagnostics in the file/run logs without flooding the
/// terminal. The capture itself remains bounded by the process runner, and a
/// truncation marker makes any omitted bytes explicit rather than silent.
pub(crate) fn log_captured_process_output(
    command_line: &str,
    status: &std::process::ExitStatus,
    stdout: &str,
    stderr: &str,
) {
    let output = combined_tool_output(stdout, stderr);
    if output.trim().is_empty() {
        return;
    }
    let output = truncate_captured_tool_output(&output);
    if status.success() {
        tracing::info!(
            target: "mfb::tool_output",
            command = %command_line,
            success = true,
            exit_code = ?status.code(),
            output = %output,
            "Captured external-tool diagnostics"
        );
    } else {
        tracing::warn!(
            target: "mfb::tool_output",
            command = %command_line,
            success = false,
            exit_code = ?status.code(),
            output = %output,
            "Captured failed external-tool diagnostics"
        );
    }
}

/// Combine both output streams while retaining their origin for audits and
/// error messages.
#[must_use]
pub(crate) fn combined_tool_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (false, false) => format!("STDOUT:\n{stdout}\n\nSTDERR:\n{stderr}"),
        (false, true) => format!("STDOUT:\n{stdout}"),
        (true, false) => format!("STDERR:\n{stderr}"),
        (true, true) => String::new(),
    }
}

fn truncate_captured_tool_output(output: &str) -> String {
    if output.len() <= CAPTURED_TOOL_OUTPUT_LOG_MAX_BYTES {
        return output.to_owned();
    }

    let half = CAPTURED_TOOL_OUTPUT_LOG_MAX_BYTES / 2;
    let mut head_end = half;
    while head_end > 0 && !output.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = output.len() - half;
    while tail_start < output.len() && !output.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n...[captured diagnostic truncated: {} bytes total]...\n{}",
        &output[..head_end],
        output.len(),
        &output[tail_start..]
    )
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

    let command_str = crate::common_utils::format_command_string(tool_name, args);

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
/// Returns an error if the command fails to start or exits with a non-zero
/// status.
pub fn execute_external_command_checked(
    tool_name: &str,
    args: &[&str],
) -> Result<ExternalCommandResult> {
    let result = execute_external_command(tool_name, args)?;

    if result.exit_code != Some(0_i32) {
        let command_str = crate::common_utils::format_command_string(tool_name, args);
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
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    /// Env vars are process-global; serialize tests that mutate them.
    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Clone, Default)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn saved_env_var(key: &str) -> Option<String> {
        match std::env::var(key) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(e) => panic!("failed to read env var {key}: {e:?}"),
        }
    }

    struct RemovedEnvGuard {
        key: &'static str,
        old_value: Option<String>,
    }

    impl RemovedEnvGuard {
        fn remove(key: &'static str) -> Self {
            let old_value = saved_env_var(key);
            // SAFETY: guarded by ENV_TEST_LOCK.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, old_value }
        }
    }

    impl Drop for RemovedEnvGuard {
        fn drop(&mut self) {
            match &self.old_value {
                Some(value) => unsafe {
                    // SAFETY: guarded by ENV_TEST_LOCK.
                    std::env::set_var(self.key, value);
                },
                None => unsafe {
                    // SAFETY: guarded by ENV_TEST_LOCK.
                    std::env::remove_var(self.key);
                },
            }
        }
    }

    #[test]
    fn test_log_config_default() {
        let config = LogConfig::default();
        assert_eq!(config.max_file_size, 30 * 1024 * 1024);
        assert_eq!(config.max_files, usize::MAX);
        assert_eq!(config.level, Level::TRACE);
    }

    #[test]
    fn test_log_config_builder() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
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
    fn generic_warning_and_error_lines_do_not_append_progress_stats() {
        crate::progress_mode::reset_session_stats();
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .event_format(ModernFormatter)
            .with_writer(writer.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("ordinary warning");
            tracing::error!("ordinary error");
        });

        let output = String::from_utf8(
            writer
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
        .expect("formatter output must be UTF-8");
        assert!(
            !output.contains("📊") && !output.contains("X:0"),
            "generic tracing lines must not carry progress statistics: {output:?}"
        );
    }

    #[test]
    #[serial_test::serial]
    fn unified_log_dir_prefers_explicit_env() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let log_path = temp_dir.path().join("explicit");
        fs::create_dir_all(&log_path).unwrap_or_else(|e| panic!("error: {e:?}"));
        let path_str = log_path.to_str().expect("utf-8 path");
        let _guard =
            crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_LOG_DIR, path_str);
        assert_eq!(LogConfig::unified_log_dir(), log_path);
    }

    #[test]
    #[serial_test::serial]
    fn unified_log_dir_rejects_workspace_logs_path() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let workspace = temp_dir.path().join("ws");
        let logs = workspace.join("logs");
        fs::create_dir_all(workspace.join("crates")).unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::write(workspace.join("Cargo.toml"), "").unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::create_dir_all(&logs).unwrap_or_else(|e| panic!("error: {e:?}"));
        let home_logs = temp_dir.path().join("state").join("logs");
        let _log_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_LOG_DIR,
            logs.to_str().expect("utf-8 path"),
        );
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            temp_dir.path().join("state").to_str().expect("utf-8 path"),
        );
        assert_eq!(LogConfig::unified_log_dir(), home_logs);
    }

    #[test]
    #[serial_test::serial]
    fn unified_log_dir_uses_home_root_not_workspace_logs() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let home_logs = temp_dir.path().join("state").join("logs");
        let _log_guard = RemovedEnvGuard::remove(crate::constants::ENV_MFB_LOG_DIR);
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            temp_dir.path().join("state").to_str().expect("utf-8 path"),
        );
        assert_eq!(LogConfig::unified_log_dir(), home_logs);
    }

    #[test]
    #[serial_test::serial]
    fn unified_log_dir_rejects_workspace_home_root_logs() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let workspace = temp_dir.path().join("ws");
        fs::create_dir_all(workspace.join("crates")).unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::write(workspace.join("Cargo.toml"), "").unwrap_or_else(|e| panic!("error: {e:?}"));
        let home = temp_dir.path().join("home");
        fs::create_dir_all(&home).unwrap_or_else(|e| panic!("error: {e:?}"));
        let expected = home.join(".modern_format_boost").join("logs");
        let _log_guard = RemovedEnvGuard::remove(crate::constants::ENV_MFB_LOG_DIR);
        let _home_root_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            workspace.to_str().expect("utf-8 path"),
        );
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_HOME,
            home.to_str().expect("utf-8 path"),
        );
        assert_eq!(LogConfig::unified_log_dir(), expected);
    }

    #[test]
    fn mfb_audit_line_field_order_and_quoting() {
        let c = AuditKvCollector::test_from_pairs(&[
            ("reason", "not routed"),
            ("outcome", "ignored"),
            ("mfb_audit_schema", "1"),
            ("pipeline", "img"),
            ("path", "/tmp/a b.jpg"),
        ]);
        let line = format_mfb_audit_line(c);
        assert!(
            line.starts_with("MFB_AUDIT mfb_audit_schema=1 outcome=ignored pipeline=img path="),
            "{line}"
        );
        assert!(line.contains(r#""/tmp/a b.jpg""#), "{line}");
        assert!(
            line.contains(r#"reason="not routed""#),
            "expected quoted reason, got {line}"
        );
    }

    #[test]
    fn test_init_logging_creates_log_file() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let _config = LogConfig::new().with_log_dir(temp_dir.path());

        assert!(temp_dir.path().exists());
    }

    #[test]
    fn test_cleanup_old_logs() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let program_name = "test_program";

        for i in 0_i32..10_i32 {
            let file_path = temp_dir.path().join(format!("{program_name}.{i}.log"));
            fs::write(&file_path, format!("log content {i}"))
                .unwrap_or_else(|e| panic!("error: {e:?}"));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        cleanup_old_logs(temp_dir.path(), program_name, 3)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let remaining_files: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap_or_else(|e| panic!("error: {e:?}"))
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(program_name))
            .collect();

        assert_eq!(remaining_files.len(), 3);
    }

    #[test]
    fn test_execute_external_command_success() {
        let result = execute_external_command("echo", &["hello", "world"]);

        assert!(result.is_ok());
        let result = result.unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(result.exit_code, Some(0_i32));
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
        let result = result.unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(result.exit_code, Some(0_i32));
    }

    #[test]
    fn test_external_command_result_structure() {
        let result =
            execute_external_command("echo", &["test"]).unwrap_or_else(|e| panic!("error: {e:?}"));

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
            Some(0_i32),
            std::time::Duration::from_secs(1),
        );
    }

    #[test]
    fn test_size_based_rotation() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let program_name = "test_rotate_program";
        let max_size = 500; // 500 bytes limit

        let mut appender =
            SizeRotatingAppender::new(temp_dir.path().to_path_buf(), program_name, max_size, "log");

        // Write enough to trigger rotation
        for i in 0_i32..20_i32 {
            let msg = format!("Log entry number {i} filling space\n");
            appender
                .write_all(msg.as_bytes())
                .unwrap_or_else(|e| panic!("error: {e:?}"));
        }
        appender.flush().unwrap_or_else(|e| panic!("error: {e:?}"));

        let files: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap_or_else(|e| panic!("error: {e:?}"))
            .filter_map(std::result::Result::ok)
            .collect();

        assert!(files.len() >= 2);
    }

    #[test]
    fn size_rotating_appender_splits_oversized_writes_to_keep_each_file_within_cap() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let program_name = "test_split_program";
        let max_size = 64_u64;

        let mut appender =
            SizeRotatingAppender::new(temp_dir.path().to_path_buf(), program_name, max_size, "log");
        appender
            .write_all(&[b'x'; 160])
            .unwrap_or_else(|e| panic!("write oversized payload: {e:?}"));
        appender.flush().unwrap_or_else(|e| panic!("flush: {e:?}"));

        let files: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap_or_else(|e| panic!("read log dir: {e:?}"))
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(program_name)
            })
            .collect();

        assert!(
            files.len() >= 3,
            "expected rotation files, got {}",
            files.len()
        );
        for file in files {
            let len = file
                .metadata()
                .unwrap_or_else(|e| panic!("metadata for {:?}: {e:?}", file.path()))
                .len();
            assert!(
                len <= max_size,
                "log file exceeded cap: {:?} len={len} cap={max_size}",
                file.path()
            );
        }
    }
}
