//! v0.11.3: Progress Mode - controls progress bar display
//!
//! Avoids progress output clutter when processing in parallel.
//! Stderr output is routed through tracing when a subscriber is set
//! (`init_logging`).

use crate::modern_ui::{colors, symbols};
use std::cell::RefCell;
use std::fmt::Write;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write as _};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::vec::Vec;
use tracing;
use tracing::Level;

// ── Per-thread log context (file name or ID) for concurrent processing ───────
// When set, every log_eprintln! / verbose_eprintln! line is prefixed so
// interleaved output from multiple files can be attributed correctly.

thread_local! {
    static LOG_PREFIX: RefCell<String> = const { RefCell::new(String::new()) };
}

static RUN_LOG_IO_FAILURE_REPORTED: AtomicBool = AtomicBool::new(false);

fn report_run_log_io_failure(context: &str, detail: &str) {
    crate::media_conversion_gate::delivery_progress_batch_audit(
        "delivery_progress",
        format!("Run log output degraded (context={context}, detail={detail})"),
    );

    if !RUN_LOG_IO_FAILURE_REPORTED.swap(true, Ordering::Relaxed) {
        let icon =
            crate::media_conversion_gate::ui_icon_pick(symbols::WARNING, symbols::plain::WARNING);
        let _ = writeln!(std::io::stderr(), "{icon} [Run Log] {context}: {detail}");
    }
}

/// Format duration as detailed string with progressive spacing strategy.
///
/// Examples: "01Y   01M   01W   01D   01h 00m00s000ms" or "01M   01W   01D   01h 00m00s000ms" or "01W   01D   01h 00m00s000ms" or "01D   01h 00m00s000ms" or "01h 00m00s000ms" or "00m00s000ms" or "00s000ms"
#[must_use]
pub fn format_duration_compact(duration: Duration) -> String {
    let total_millis = duration.as_millis();
    let years = total_millis / (365 * 86400 * 1000);
    let months = (total_millis % (365 * 86400 * 1000)) / (30 * 86400 * 1000);
    let weeks = (total_millis % (30 * 86400 * 1000)) / (7 * 86400 * 1000);
    let days = (total_millis % (7 * 86400 * 1000)) / (86400 * 1000);
    let hours = (total_millis % (86400 * 1000)) / (3600 * 1000);
    let minutes = (total_millis % (3600 * 1000)) / (60 * 1000);
    let seconds = (total_millis % (60 * 1000)) / 1000;
    let millis = total_millis % 1000;

    let mut parts = Vec::new();

    if years > 0 {
        parts.push(format!("{years:02}Y"));
    }
    if months > 0 || years > 0 {
        parts.push(format!("{months:02}M"));
    }
    if weeks > 0 || months > 0 || years > 0 {
        parts.push(format!("{weeks:02}W"));
    }
    if days > 0 || weeks > 0 || months > 0 || years > 0 {
        parts.push(format!("{days:02}D"));
    }
    if hours > 0 || days > 0 || weeks > 0 || months > 0 || years > 0 {
        parts.push(format!("{hours:02}h"));
    }
    if minutes > 0 || hours > 0 || days > 0 || weeks > 0 || months > 0 || years > 0 {
        parts.push(format!("{minutes:02}m"));
    }

    // Seconds: only show when there are no hours-or-larger components
    // (avoids "1h01m40s" when "1h01m" is cleaner at hour-level precision)
    let has_hours_plus = hours > 0 || days > 0 || weeks > 0 || months > 0 || years > 0;
    if !has_hours_plus && (total_millis >= 1000 || seconds > 0) {
        parts.push(format!("{seconds:02}s"));
    }

    // Milliseconds: only show when there are no seconds-or-larger components
    // (i.e., sub-second precision is useful), or when ms is non-zero and
    // there are no minutes-or-larger components (show "5s372ms" but not
    // "30s000ms").
    let has_large_unit =
        minutes > 0 || hours > 0 || days > 0 || weeks > 0 || months > 0 || years > 0;
    if !has_large_unit && millis > 0 {
        parts.push(format!("{millis:03}ms"));
    } else if total_millis == 0 {
        // Zero duration: show "000ms"
        parts.push("000ms".to_string());
    }

    if parts.is_empty() {
        return "00s".to_string();
    }

    // Strip leading zeros from the first (most-significant) part so we get
    // "1m30s" rather than "01m30s" while sub-units stay zero-padded ("01m", "30s").
    if let Some(first) = parts.first_mut() {
        // Find where digits end and the unit suffix begins
        let suffix_start = match first.find(|c: char| c.is_alphabetic()) {
            Some(pos) => pos,
            None => first.len(),
        };
        let digits = &first[..suffix_start];
        let suffix = &first[suffix_start..];
        let trimmed = digits.trim_start_matches('0');
        let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
        *first = format!("{trimmed}{suffix}");
    }

    // Progressive spacing for large compound durations
    if years > 0 || months > 0 || weeks > 0 || days > 0 || hours > 0 {
        let mut result = String::new();
        for (i, part) in parts.iter().enumerate() {
            result.push_str(part);
            let spacing = if i == 0 && years > 0
                || i == 1 && (months > 0 || years > 0)
                || i == 2 && (weeks > 0 || months > 0 || years > 0)
                || i == 3 && (days > 0 || weeks > 0 || months > 0 || years > 0)
            {
                "   " // 3 spaces after large units
            } else if i == 4 && (hours > 0 || days > 0 || weeks > 0 || months > 0 || years > 0) {
                "  " // 2 spaces after hours
            } else {
                "" // no extra spacing for minutes/seconds/ms
            };
            result.push_str(spacing);
        }
        result
    } else {
        parts.join("")
    }
}

/// Width of the tag column so all message bodies align (e.g. [file.jpeg]).
/// 28 chars fits filenames up to ~24 chars + brackets + space separator.
const LOG_TAG_WIDTH: usize = crate::constants::LOG_TAG_WIDTH_DEFAULT;

/// Max visible chars for the filename displayed inside \[brackets\].
/// With `LOG_TAG_WIDTH=28`, tag=\[prefix\] uses prefix+2 bytes, max prefix =
/// 25.
const LOG_PREFIX_MAX_DISPLAY: usize = crate::constants::LOG_PREFIX_MAX_DISPLAY;

/// Prefix for periodic statistics lines — emoji instead of \[Info\] to avoid
/// confusion with log severity levels. Followed by a fixed-width space pad so
/// the message body aligns with regular file-tag lines.
/// Stats column prefix (plain-aware).
#[inline]
fn stats_line_prefix() -> String {
    crate::media_conversion_gate::ui_icon_pick("📊", "[#]")
}

/// Truncate at a UTF-8 char boundary so we never split a multi-byte character.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Pad a file-context tag (e.g. `[file.jpeg]`) to `LOG_TAG_WIDTH` chars for
/// aligned message body. Always produces exactly `LOG_TAG_WIDTH` chars, or tag
/// + one space if tag is already wide.
fn pad_tag(tag: &str) -> String {
    if tag.len() >= LOG_TAG_WIDTH {
        format!("{tag} ")
    } else {
        format!("{}{}", tag, " ".repeat(LOG_TAG_WIDTH - tag.len()))
    }
}
/// Format a statistics summary line (plain, no leading blank line) for
/// the final summary emitted after all processing is done.
fn fmt_stats_line_final(msg: &str) -> String {
    format!("    {} {}", stats_line_prefix(), msg)
}

/// Set the current thread's log prefix (e.g. file name or short ID). Cleared on
/// drop of `LogContextGuard`.
///
/// Truncates long names to `LOG_PREFIX_MAX_DISPLAY` chars, preserving the file
/// extension:   "`Image_103999006594198.jpeg`" → "`Image_103999006…jpeg`"
///   "`Cache_4ac28036da7d11be.jpg`" → "`Cache_4ac28036da7…jpg`"
pub fn set_log_context(prefix: &str) {
    let s = if prefix.chars().count() > LOG_PREFIX_MAX_DISPLAY {
        match prefix.rfind('.') {
            None => {
                let head = truncate_to_char_boundary(prefix, LOG_PREFIX_MAX_DISPLAY - 1);
                format!("{head}…")
            }
            Some(dot_pos) => {
                let ext = &prefix[dot_pos..]; // e.g. ".jpeg"
                let ext_chars = ext.chars().count();
                if ext_chars < LOG_PREFIX_MAX_DISPLAY - 2 {
                    let stem_max_chars = LOG_PREFIX_MAX_DISPLAY - ext_chars - 1;
                    let stem = truncate_to_char_boundary(prefix, stem_max_chars);
                    format!("{stem}…{ext}")
                } else {
                    let head = truncate_to_char_boundary(prefix, LOG_PREFIX_MAX_DISPLAY - 1);
                    format!("{head}…")
                }
            }
        }
    } else {
        prefix.to_string()
    };
    LOG_PREFIX.with(|p| *p.borrow_mut() = s);
}

/// Clear the current thread's log prefix.
pub fn clear_log_context() {
    LOG_PREFIX.with(|p| p.borrow_mut().clear());
}

/// File-type marker for log lines (plain-aware).
fn file_type_emoji(filename: &str) -> String {
    crate::media_conversion_gate::ui_log_file_type_icon_prefix(filename)
}

/// Format a log line with optional tag, emoji prefix, and padded indent so
/// message bodies align. When a filename prefix is set, prepends a file-type
/// emoji (🖼️ image / 🎞️ GIF / 🎬 video).
#[must_use]
pub fn format_log_line(line: &str) -> String {
    LOG_PREFIX.with(|p| {
        let prefix = p.borrow();
        if prefix.is_empty() {
            format!("{}{}", " ".repeat(LOG_TAG_WIDTH), line)
        } else {
            let emoji = file_type_emoji(&prefix);
            format!("{}{}{}", emoji, pad_tag(&format!("[{prefix}]")), line)
        }
    })
}

/// Guard that clears log context when dropped. Use at the start of per-file
/// processing.
pub struct LogContextGuard;

impl Drop for LogContextGuard {
    fn drop(&mut self) {
        clear_log_context();
    }
}

// ── File log writer
// ──────────────────────────────────────────────────────────── When a log file
// path is configured, ALL messages (both regular and verbose) are written to it
// in full detail, regardless of the terminal verbose setting.
//
// **If the log file is renamed/moved while the process is running:** on Unix we
// keep writing to the same open file descriptor (same inode). Data is not lost,
// but the content keeps going to the renamed file; the original path may show a
// new empty file if one was recreated. So avoid renaming the run log file until
// the process exits.
//
// **File lock:** we take an advisory exclusive lock (flock LOCK_EX) on open so
// other processes that respect the lock cannot truncate or overwrite the log.

#[cfg(unix)]
fn flock_log_exclusive(file: &File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

struct RunLogFileWriter {
    base_path: std::path::PathBuf,
    max_file_size: u64,
    current_size: u64,
    current_seq: usize,
    writer: BufWriter<File>,
}

impl RunLogFileWriter {
    fn new(path: &std::path::Path, max_file_size: u64) -> std::io::Result<Self> {
        if max_file_size == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "run log max_file_size must be greater than zero",
            ));
        }
        let (writer, current_size) = Self::open_writer(path)?;
        Ok(Self {
            base_path: path.to_path_buf(),
            max_file_size,
            current_size,
            current_seq: 0,
            writer,
        })
    }

    fn open_writer(path: &std::path::Path) -> std::io::Result<(BufWriter<File>, u64)> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        #[cfg(unix)]
        flock_log_exclusive(&file)?;
        let current_size = file.metadata()?.len();
        Ok((BufWriter::with_capacity(64 * 1024, file), current_size))
    }

    fn path_for_seq(&self, seq: usize) -> std::io::Result<std::path::PathBuf> {
        if seq == 0 {
            return Ok(self.base_path.clone());
        }
        let parent = self.base_path.parent().ok_or_else(|| {
            std::io::Error::other(format!(
                "run log path has no parent: {}",
                self.base_path.display()
            ))
        })?;
        let stem = crate::media_conversion_gate::path_file_stem_os_or_delivery_err(
            &self.base_path,
            "run log rotation",
        )
        .map_err(std::io::Error::other)?;
        let mut name = format!("{}.{}", stem.to_string_lossy(), seq);
        if let Some(ext) = self.base_path.extension() {
            name.push('.');
            name.push_str(&ext.to_string_lossy());
        }
        Ok(parent.join(name))
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        self.writer.flush()?;
        self.current_seq += 1;
        let next_path = self.path_for_seq(self.current_seq)?;
        let (writer, current_size) = Self::open_writer(&next_path)?;
        self.writer = writer;
        self.current_size = current_size;
        Ok(())
    }

    fn write_capped(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.max_file_size == u64::MAX {
            self.writer.write_all(bytes)?;
            self.current_size += crate::numeric_cast::usize_to_u64(bytes.len());
            return Ok(());
        }

        let mut remaining = bytes;
        while !remaining.is_empty() {
            if self.current_size >= self.max_file_size {
                self.rotate()?;
            }

            let remaining_cap = self.max_file_size.saturating_sub(self.current_size);
            let Some(remaining_cap_usize) =
                crate::numeric_cast::u64_to_usize_strict(remaining_cap, "run_log_remaining_cap")
            else {
                return Err(std::io::Error::other(format!(
                    "run log remaining capacity does not fit usize: {remaining_cap}"
                )));
            };
            let write_len = remaining.len().min(remaining_cap_usize);
            if write_len == 0 {
                self.rotate()?;
                continue;
            }
            self.writer.write_all(&remaining[..write_len])?;
            self.current_size += crate::numeric_cast::usize_to_u64(write_len);
            remaining = &remaining[write_len..];
        }
        Ok(())
    }

    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        self.write_capped(line.as_bytes())?;
        self.write_capped(b"\n")
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

static LOG_FILE_WRITER: Mutex<Option<RunLogFileWriter>> = Mutex::new(None);

fn lock_log_writer() -> std::sync::MutexGuard<'static, Option<RunLogFileWriter>> {
    crate::media_conversion_gate::logging_mutex_guard_or_recover(
        "progress_run_log_writer",
        "[Run Log] log writer mutex was poisoned; recovering state",
        LOG_FILE_WRITER.lock(),
    )
}

/// Open (or create) the log file and take an advisory exclusive lock so it is
/// not truncated by others.
///
/// Call once at startup. Registers a forwarder so tracing events are also
/// written to this run log. Set the log file for the current process.
///
/// # Errors
/// Returns an I/O error if the file cannot be created.
pub fn set_log_file(path: &std::path::Path) -> std::io::Result<()> {
    *lock_log_writer() = Some(RunLogFileWriter::new(
        path,
        crate::logging::DEFAULT_MAX_LOG_FILE_SIZE_BYTES,
    )?);
    crate::logging::register_run_log_forwarder(Box::new(write_to_log));
    Ok(())
}

/// Returns true if a log file has been configured.
#[must_use]
pub fn has_log_file() -> bool {
    return lock_log_writer().is_some();
}

/// If no log file is configured, open a default run log under the unified log
/// directory.
///
/// Timestamped filename ensures each run gets a unique file
/// (e.g. `~/.modern_format_boost/logs/img_run_20260526_143000.log`).
/// Call at Run startup so quality and progress are always written without
/// requiring `--log-file`. Set the default log file for the current process.
///
/// # Errors
/// Returns an I/O error if the file cannot be created.
pub fn set_default_run_log_file(binary_name: &str) -> std::io::Result<()> {
    if binary_name.contains("vid") {
        IS_VIDEO_MODE.store(true, Ordering::Relaxed);
    }
    if has_log_file() {
        return Ok(());
    }

    // Use the same unified log directory as LogConfig
    let dir = crate::logging::LogConfig::unified_log_dir();

    std::fs::create_dir_all(&dir)?;
    let session_id = std::env::var("MFB_SESSION_ID")
        .unwrap_or_else(|_| chrono::Local::now().format("%Y%m%d_%H%M%S").to_string());
    let path = dir.join(format!("{binary_name}_run_{session_id}.log"));
    set_log_file(&path)?;
    write_run_log_session_header(binary_name, &path);
    Ok(())
}

/// Write a session header line to the run log so the file clearly records that
/// full output is being captured.
///
/// Call after `set_log_file` (or from `set_default_run_log_file`). If
/// `init_logging` already emitted a line, it is written here so the run log has
/// it too. Respects log level (INFO): only written when level is INFO or more
/// verbose.
pub fn write_run_log_session_header(program_name: &str, run_log_path: &std::path::Path) {
    if let Some(ref init_line) = crate::logging::take_init_message_for_run_log() {
        tracing::info!("{}", init_line);
    }
    tracing::info!(
        program = program_name,
        run_log = %run_log_path.display(),
        "Run log attached (all stderr and tracing written here)"
    );
}

/// Write one progress line to the run log so the log has the same "Running:
/// HH:MM:SS  N/total  message" as the terminal.
///
/// Written only when `MFB_LOG_PROGRESS=1` (see
/// [`crate::logging::log_file_includes_progress`]).
pub fn write_progress_line_to_run_log(elapsed_secs: u64, current: u64, total: u64, message: &str) {
    if !crate::logging::log_file_includes_progress() {
        return;
    }
    tracing::debug!(
        target: "mfb::progress",
        elapsed_secs,
        current,
        total,
        message = %message,
        "progress tick"
    );
}

/// Write a line to the log file (no-op if no log file is configured).
///
/// Does NOT write to stderr — use `log_eprintln`! or `verbose_eprintln`! for
/// dual output. Strips ANSI escape codes so file logs are plain text.
/// Flushes after each write so log output is immediate (no loss on crash/kill).
pub fn write_to_log(line: &str) {
    // Ensure every line written to the run log has milestone stats appended (unless
    // it already does)
    let line_with_stats = append_stats_to_line(line);
    let plain = crate::logging::strip_ansi_str(&line_with_stats);
    match LOG_FILE_WRITER.lock() {
        Ok(mut guard) => {
            if let Some(ref mut w) = *guard {
                if let Err(err) = w.write_line(&plain) {
                    report_run_log_io_failure("failed to write run log line", &err.to_string());
                    return;
                }
                if let Err(err) = w.flush() {
                    report_run_log_io_failure("failed to flush run log line", &err.to_string());
                }
            }
        }
        Err(err) => {
            report_run_log_io_failure("run log writer mutex poisoned", &err.to_string());
        }
    }
}

/// Write conversion failure to the run log file immediately (so failures are in
/// the log, not only stderr).
///
/// Call this whenever a single-file conversion returns Err, so the log file has
/// the full error for later inspection. Uses `Level::Error` so it is always
/// written when level is WARN or ERROR (and any level includes errors).
pub fn log_conversion_failure(path: &std::path::Path, error: &str) {
    tracing::error!(
        file = %path.display(),
        error = %error,
        "Conversion failed"
    );
}

/// Uniform indent for all stderr lines so logs are visually aligned (2 spaces).
const STDERR_INDENT: &str = "  ";

/// Returns true when stderr is connected to a real terminal (TTY) OR if
/// `FORCE_COLOR` is set. Cached after the first call — TTY state does not
/// change during a run.
#[inline]
fn stderr_is_tty() -> bool {
    use std::sync::OnceLock;
    static IS_TTY: OnceLock<bool> = OnceLock::new();
    *IS_TTY.get_or_init(|| {
        if std::env::var(crate::constants::ENV_FORCE_COLOR).is_ok() {
            true
        } else {
            // Use the `console` crate's detection which correctly handles
            // NO_COLOR, TERM=dumb, CI env vars, and is_terminal() semantics.
            console::Term::stderr().is_term()
        }
    })
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
/// Emit a line to stderr (and to run log when configured).
///
/// * When stderr **is a TTY**: ANSI colour codes are forwarded as-is.
/// * When stderr **is not a TTY** (pipe/redirect/script): ANSI is stripped so
///   captured output is plain, readable text.
/// * The run-log always receives the plain (stripped) version.
#[inline]
pub fn emit_stderr(line: &str) {
    use std::io::Write;
    // Pause output if the Ctrl+C confirmation prompt is currently waiting for input
    crate::ctrlc_guard::wait_if_prompt_active();

    // ── Pre-process All Lines ──
    let mut processed_lines = Vec::new();
    let mut active_ansi = String::new();
    let mut is_first = true;

    for subline in line.lines() {
        if subline.trim().is_empty() {
            continue;
        }

        let mut curr_line = subline.to_string();
        if !active_ansi.is_empty() && !curr_line.starts_with("\x1b[") {
            curr_line.insert_str(0, &active_ansi);
        }

        // scan curr_line for the last color code to update active_ansi
        let mut temp = &curr_line[..];
        while let Some(idx) = temp.find("\x1b[") {
            temp = &temp[idx..];
            if let Some(m_idx) = temp.find('m') {
                let code = &temp[..=m_idx];
                if code == "\x1b[0m" {
                    active_ansi.clear();
                } else {
                    active_ansi = code.to_string();
                }
                temp = &temp[m_idx + 1..];
            } else {
                break;
            }
        }

        // Add milestone stats only to the first line of a multi-line message
        let line_with_stats = if is_first {
            append_stats_to_line(&curr_line)
        } else {
            curr_line
        };
        is_first = false;

        let out = if stderr_is_tty() && !is_plain_mode() {
            // TTY: keep colours.
            format!("{STDERR_INDENT}{line_with_stats}")
        } else {
            // Non-TTY: strip ANSI so piped / redirected output is clean.
            format!(
                "{}{}",
                STDERR_INDENT,
                crate::logging::strip_ansi_str(&line_with_stats)
            )
        };
        processed_lines.push(out);
    }

    if processed_lines.is_empty() {
        return;
    }

    // ── Unified Output Block ──
    // Clear once, print all, restore once.
    {
        let _terminal_guard = crate::media_conversion_gate::delivery_terminal_lock_guard(
            "progress_mode_emit_stderr_batch",
        );
        if stderr_is_tty() {
            if let Some(progress_line) = crate::progress::active_progress_line() {
                // Clear progress line, print all lines, then re-show progress
                let mut output = String::from("\r\x1b[2K");
                for p_line in processed_lines {
                    output.push_str(&p_line);
                    output.push('\n');
                }
                output.push('\r');
                output.push_str(&progress_line);

                if let Err(_err) = write!(std::io::stderr(), "{output}") {
                    crate::media_conversion_gate::delivery_progress_batch_audit(
                        "delivery_progress",
                        "Failed to write progress output to stderr",
                    );
                }
            } else {
                let mut output = String::new();
                for p_line in processed_lines {
                    output.push_str(&p_line);
                    output.push('\n');
                }
                if let Err(_err) = write!(std::io::stderr(), "{output}") {
                    crate::media_conversion_gate::delivery_progress_batch_audit(
                        "delivery_progress",
                        "Failed to write progress output to stderr",
                    );
                }
            }
        } else {
            // Non-TTY: just print with newlines
            let mut output = String::new();
            for p_line in processed_lines {
                output.push_str(&p_line);
                output.push('\n');
            }
            if let Err(_err) = write!(std::io::stderr(), "{output}") {
                crate::media_conversion_gate::delivery_progress_batch_audit(
                    "delivery_progress",
                    "Failed to write progress output to stderr",
                );
            }
        }
        let _ = std::io::stderr().flush();
    }
}

/// Flush the log file buffer. Call at program exit.
pub fn flush_log_file() {
    match LOG_FILE_WRITER.lock() {
        Ok(mut guard) => {
            if let Some(ref mut w) = *guard
                && let Err(err) = w.flush()
            {
                report_run_log_io_failure("failed to flush run log at shutdown", &err.to_string());
            }
        }
        Err(err) => {
            report_run_log_io_failure(
                "run log writer mutex poisoned during shutdown",
                &err.to_string(),
            );
        }
    }
}

static QUIET_MODE: AtomicBool = AtomicBool::new(false);
static IS_VIDEO_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_is_video_mode(val: bool) {
    IS_VIDEO_MODE.store(val, Ordering::Relaxed);
}

pub fn is_video_mode() -> bool {
    IS_VIDEO_MODE.load(Ordering::Relaxed)
}

pub fn enable_quiet_mode() {
    QUIET_MODE.store(true, Ordering::Relaxed);
}

pub fn disable_quiet_mode() {
    QUIET_MODE.store(false, Ordering::Relaxed);
}

pub fn is_quiet_mode() -> bool {
    QUIET_MODE.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! quiet_eprintln {
    ($($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        tracing::info!(target: "mfb::ui", "{}", _msg);
        if !$crate::progress_mode::is_quiet_mode() {
            $crate::progress_mode::emit_stderr(&_msg);
        }
    }};
}

// ── Verbose mode (single source of truth for process-wide verbose logging) ───
// Default ON: full stderr detail for forensic sessions; disable with
// --no-verbose if added. CLI calls `set_verbose_mode` from `--verbose` (default
// true on img/vid run). All verbose output uses `is_verbose_mode()` /
// `verbose_eprintln!` — no per-config flag.

static VERBOSE_MODE: AtomicBool = AtomicBool::new(true);

static PLAIN_MODE: AtomicBool = AtomicBool::new(false);

/// Returns true when the process should avoid emoji and decorative ANSI on
/// stderr.
#[must_use]
pub fn is_plain_mode() -> bool {
    PLAIN_MODE.load(Ordering::Relaxed)
}

pub fn set_plain_mode(v: bool) {
    PLAIN_MODE.store(v, Ordering::Relaxed);
}

/// Apply CLI `--plain` plus `MODERN_FORMAT_PLAIN_UI` / `NO_COLOR` before any UI
/// output.
pub fn configure_terminal_ux(cli_plain: bool) {
    let env_plain = match std::env::var(crate::constants::ENV_PLAIN_UI) {
        Ok(value) => matches!(value.as_str(), "1" | "true" | "yes" | "on"),
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "terminal_ux",
                format!(
                    "failed to read {}: {e}; plain mode env flag disabled",
                    crate::constants::ENV_PLAIN_UI
                ),
            );
            false
        }
    };
    let no_color = std::env::var_os("NO_COLOR").is_some();
    set_plain_mode(cli_plain || env_plain || no_color);
}

pub fn set_verbose_mode(v: bool) {
    VERBOSE_MODE.store(v, Ordering::Relaxed);
}

/// One-time audit hint: DB `final_verdict` may be `TelemetryOnly` (U5).
pub fn maybe_log_inference_analytics_hint(verbose: bool) {
    if !verbose {
        return;
    }
    tracing::info!(
        target: "mfb::audit",
        inference_log_final_verdict_column = crate::constants::INFERENCE_TELEMETRY_ONLY_VERDICT,
        analytics_view = "loop_inference_log_effective",
        "Do not treat inference_log.final_verdict as user-facing truth; use *_inference_log_effective SQL views or signal_snapshot fields"
    );
}

#[must_use]
pub const fn tracing_level_debug() -> Level {
    Level::DEBUG
}

pub fn is_verbose_mode() -> bool {
    VERBOSE_MODE.load(Ordering::Relaxed)
}

/// Print to stderr only when verbose mode is enabled.
///
/// Run log gets the line only when level allows (DEBUG: written at
/// DEBUG/TRACE). When set via `set_log_context()`, the line is prefixed with
/// `[prefix]` for concurrent file processing.
#[macro_export]
macro_rules! verbose_eprintln {
    () => {{
        tracing::debug!(target: "mfb::ui", "");
        if $crate::progress_mode::is_verbose_mode() {
            $crate::progress_mode::emit_stderr("");
        }
    }};
    ($($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        let _line = $crate::progress_mode::format_log_line(&_msg);
        tracing::debug!(target: "mfb::ui", "{}", _line);
        if $crate::progress_mode::is_verbose_mode() {
            $crate::progress_mode::emit_stderr(&_line);
        }
    }};
}

/// Print to both stderr and the run log file (if configured). Run log gets full
/// TRACE-level detail. When set via `set_log_context()`, the line is prefixed
/// with `[prefix]` for concurrent file processing.
#[macro_export]
macro_rules! log_eprintln {
    () => {{
        tracing::info!(target: "mfb::ui", "");
        $crate::progress_mode::emit_stderr("");
    }};
    ($($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        let _line = $crate::progress_mode::format_log_line(&_msg);
        tracing::info!(target: "mfb::ui", "{}", _line);
        $crate::progress_mode::emit_stderr(&_line);
    }};
}

// ── XMP merge + JXL + Images live counter ────────────────────────────────────
// Tracks XMP sidecar merge, JXL success, and image conversion success/failure;
// same line.

static XMP_ATTEMPT_COUNT: AtomicU64 = AtomicU64::new(0);
static XMP_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);
static JXL_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);
static IMAGE_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);
static IMAGE_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
static IMAGE_SKIP_COUNT: AtomicU64 = AtomicU64::new(0);
static VIDEO_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);
static VIDEO_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
static VIDEO_SKIP_COUNT: AtomicU64 = AtomicU64::new(0);
static PREPROCESSING_COUNT: AtomicU64 = AtomicU64::new(0);
static FALLBACK_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);

/// Resets all global session statistics to zero.
/// Use this when starting a new batch or session to ensure progress counters
/// are accurate.
pub fn reset_session_stats() {
    XMP_ATTEMPT_COUNT.store(0, Ordering::Relaxed);
    XMP_SUCCESS_COUNT.store(0, Ordering::Relaxed);
    JXL_SUCCESS_COUNT.store(0, Ordering::Relaxed);
    IMAGE_SUCCESS_COUNT.store(0, Ordering::Relaxed);
    IMAGE_FAIL_COUNT.store(0, Ordering::Relaxed);
    IMAGE_SKIP_COUNT.store(0, Ordering::Relaxed);
    VIDEO_SUCCESS_COUNT.store(0, Ordering::Relaxed);
    VIDEO_FAIL_COUNT.store(0, Ordering::Relaxed);
    VIDEO_SKIP_COUNT.store(0, Ordering::Relaxed);
    PREPROCESSING_COUNT.store(0, Ordering::Relaxed);
    FALLBACK_SUCCESS_COUNT.store(0, Ordering::Relaxed);
}

/// Call when a pre-processing step completes successfully (e.g. GIF→FFmpeg
/// static frame). No per-line log; count is shown in the combined status line.
pub fn preprocessing_success() {
    PREPROCESSING_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call when a fallback pipeline completes successfully (e.g. ImageMagick→cjxl,
/// FFmpeg→cjxl). No per-line log; count is shown in the combined status line.
pub fn fallback_success() {
    FALLBACK_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call when a JXL conversion completes successfully (e.g. from
/// `finalize_task`).
pub fn jxl_success() {
    JXL_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call on successful image conversion. Prints milestone line on EVERY success
/// (persistent display). Same line shows XMP count and Images OK/failed (JXL
/// merged into Images) when non-zero.
pub fn image_processed_success() {
    let img_ok = IMAGE_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    // Always emit status line on every image success for persistent display
    emit_combined_status_line(img_ok, img_fail);
}

pub fn image_processed_failure() {
    let _ = IMAGE_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call when an image is skipped (e.g. source is already lossy modern format).
/// Writes structured skip record to the log file and a prominent stderr line.
pub fn image_skipped(path: &std::path::Path, reason: &str) {
    let _img_skip = IMAGE_SKIP_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    crate::infra::static_logs::log_skip_at_with_pipeline(
        &format!(
            "{} Skip Audit",
            crate::media_conversion_gate::ui_icon_pick("📋", "[SKIP]")
        ),
        "img",
        Some(path),
        reason,
    );
    let line = format!(
        "{}{}  {}{}{} — {}",
        colors::MFB_YELLOW,
        crate::media_conversion_gate::ui_icon_pick("⏭️  ", "[SKIP] "),
        colors::RESET,
        colors::DIM,
        path.display(),
        reason
    );
    log_eprintln!("{}", line);
    emit_combined_status_line(
        IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed),
        IMAGE_FAIL_COUNT.load(Ordering::Relaxed),
    );
}

/// Call when an image is completely ignored (e.g. animated media in static-only
/// tool). Writes structured ignore record to the log file and a prominent
/// stderr line.
pub fn image_ignored(path: &std::path::Path, reason: &str, ignore_class: Option<&str>) {
    crate::infra::static_logs::log_ignore_at_with_pipeline(
        &format!(
            "{} Ignore Audit",
            crate::media_conversion_gate::ui_icon_pick("🙈", "[SKIP]")
        ),
        "img",
        Some(path),
        reason,
        ignore_class,
    );
    let line = format!(
        "{}{}  {}{}{} — {}",
        colors::MFB_YELLOW,
        crate::media_conversion_gate::ui_icon_pick("⏭️  ", "[IGNORE] "),
        colors::RESET,
        colors::DIM,
        path.display(),
        reason
    );
    log_eprintln!("{}", line);
    emit_combined_status_line(
        IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed),
        IMAGE_FAIL_COUNT.load(Ordering::Relaxed),
    );
}

pub fn video_processed_success() {
    VIDEO_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn video_processed_failure() {
    VIDEO_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call when a video is skipped.
/// Writes structured skip record to the log file and a prominent stderr line.
pub fn video_skipped(path: &std::path::Path, reason: &str) {
    let _ = VIDEO_SKIP_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::infra::static_logs::log_skip_at_with_pipeline(
        &format!(
            "{} Skip Audit",
            crate::media_conversion_gate::ui_icon_pick("📋", "[SKIP]")
        ),
        "vid",
        Some(path),
        reason,
    );
    let line = format!(
        "{}{}  {}{}{} — {}",
        colors::MFB_YELLOW,
        crate::media_conversion_gate::ui_icon_pick("⏭️  ", "[SKIP] "),
        colors::RESET,
        colors::DIM,
        path.display(),
        reason
    );
    log_eprintln!("{}", line);
}

/// Call when a video asset is ignored by the video pipeline (e.g. static
/// single-frame).
pub fn video_ignored(path: &std::path::Path, reason: &str, ignore_class: Option<&str>) {
    crate::infra::static_logs::log_ignore_at_with_pipeline(
        &format!(
            "{} Ignore Audit",
            crate::media_conversion_gate::ui_icon_pick("🙈", "[SKIP]")
        ),
        "vid",
        Some(path),
        reason,
        ignore_class,
    );
    let line = format!(
        "{}{}  {}{}{} — {}",
        colors::MFB_YELLOW,
        crate::media_conversion_gate::ui_icon_pick("⏭️  ", "[IGNORE] "),
        colors::RESET,
        colors::DIM,
        path.display(),
        reason
    );
    log_eprintln!("{}", line);
}

/// Helper that appends milestone stats (XMP, Img, etc.) to a log line with
/// aligned padding. Skips if the line already contains stats or is empty.
#[must_use]
pub fn append_stats_to_line(line: &str) -> String {
    let mut trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return line.to_string();
    }

    // Strip ANSI for accurate length calculation and duplicate check
    let plain = crate::logging::strip_ansi_str(trimmed);

    // Check if it already has stats (avoids double appending)
    let stats_marker =
        crate::media_conversion_gate::ui_icon_pick(symbols::CHART, symbols::plain::CHART);
    if plain.contains(&format!("│ {stats_marker}")) || plain.contains("│ 📊") {
        return trimmed.to_string();
    }

    // 🚀 Terminology/UI refinement: Only append stats if something has actually
    // happened and the message is a "Result" line (e.g. marked with ✅, ❌, ⏭️)
    // or a Progress Bar. This avoids cluttering initialization logs and general
    // info messages.
    let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    let img_skip = IMAGE_SKIP_COUNT.load(Ordering::Relaxed);
    let xmp_ok = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
    let preprocess_ok = PREPROCESSING_COUNT.load(Ordering::Relaxed);
    let vid_ok = VIDEO_SUCCESS_COUNT.load(Ordering::Relaxed);
    let vid_fail = VIDEO_FAIL_COUNT.load(Ordering::Relaxed);
    let vid_skip = VIDEO_SKIP_COUNT.load(Ordering::Relaxed);

    if xmp_ok == 0
        && img_ok == 0
        && img_fail == 0
        && img_skip == 0
        && vid_ok == 0
        && vid_fail == 0
        && vid_skip == 0
        && preprocess_ok == 0
    {
        return trimmed.to_string();
    }

    // Only append to "Result" lines that indicate failure/warning, or to the
    // progress bar itself. Skip appending to success (✅) and skip (⏭️) lines
    // in the terminal as they are redundant with the progress bar's live
    // counter.
    let is_important_result = plain.contains("❌")
        || plain.contains("[ERR]")
        || plain.contains("[X]")
        || plain.contains("⚡")
        || plain.contains("[FAST]")
        || plain.contains("☢️")
        || plain.contains("[RARE]")
        || plain.contains("⛔️")
        || plain.contains("⚠️")
        || plain.contains("[WARN]")
        || plain.contains("[!]");
    let is_progress = plain.contains("▕") && plain.contains('▏');

    if !is_important_result && !is_progress {
        return trimmed.to_string();
    }

    let stats_string = get_current_stats_string();

    // Check if it ends with \x1b[0m
    let has_reset = trimmed.ends_with("\x1b[0m");
    if has_reset {
        trimmed = &trimmed[..trimmed.len() - 4];
    }

    let visible_len = plain.chars().count();

    // Align stats to column 65 (Standard for this project)
    let target_len = 65;
    let padding_len = if visible_len < target_len {
        target_len - visible_len
    } else {
        1
    };

    // Put \x1b[0m before padding to prevent color bleed to stats
    format!(
        "{}\x1b[0m{}{}",
        trimmed,
        " ".repeat(padding_len),
        stats_string
    )
}

pub fn get_current_stats_string() -> String {
    let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    let img_skip = IMAGE_SKIP_COUNT.load(Ordering::Relaxed);
    let xmp_ok = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
    let xmp_total = XMP_ATTEMPT_COUNT.load(Ordering::Relaxed);
    let xmp_done = false;
    let xmp_failed = xmp_total.saturating_sub(xmp_ok);
    let jxl_ok = JXL_SUCCESS_COUNT.load(Ordering::Relaxed);
    let preprocess_ok = PREPROCESSING_COUNT.load(Ordering::Relaxed);
    let fallback_ok = FALLBACK_SUCCESS_COUNT.load(Ordering::Relaxed);
    let vid_ok = VIDEO_SUCCESS_COUNT.load(Ordering::Relaxed);
    let vid_fail = VIDEO_FAIL_COUNT.load(Ordering::Relaxed);
    let vid_skip = VIDEO_SKIP_COUNT.load(Ordering::Relaxed);

    let is_video = IS_VIDEO_MODE.load(Ordering::Relaxed);

    let msg = if is_video {
        format_video_stats_line(
            vid_ok,
            vid_fail,
            vid_skip,
            xmp_ok,
            xmp_failed,
            preprocess_ok,
            fallback_ok,
        )
    } else {
        format_xmp_jxl_images_line(
            xmp_ok,
            xmp_done,
            xmp_failed,
            jxl_ok,
            img_ok,
            img_fail,
            img_skip,
            preprocess_ok,
            fallback_ok,
        )
    };

    // Very minimalist separator for video
    let separator = if is_video {
        format!("{}│{}", colors::DIM, colors::RESET)
    } else {
        format!(
            "{}│{} {}",
            colors::DIM,
            colors::RESET,
            crate::media_conversion_gate::ui_icon_pick(symbols::CHART, symbols::plain::CHART)
        )
    };

    format!(" {separator} {msg}")
}

fn format_video_stats_line(
    vid_ok: u64,
    vid_fail: u64,
    vid_skip: u64,
    xmp_ok: u64,
    xmp_failed: u64,
    preprocess_ok: u64,
    _fallback_ok: u64,
) -> String {
    let ok_mark = crate::media_conversion_gate::ui_icon_pick("✓", "+");
    let fail_mark = crate::media_conversion_gate::ui_icon_pick("✗", "x");
    let mut parts = Vec::new();

    // XMP Stats: X: 12✓ (Only show if used for video)
    if xmp_ok > 0 || xmp_failed > 0 {
        let xmp_msg = if xmp_failed > 0 {
            format!(
                "{}X:{}{}{}{}{}{}{}{}",
                colors::MFB_BLUE,
                colors::MFB_GREEN,
                xmp_ok,
                ok_mark,
                colors::DIM,
                colors::MFB_RED,
                xmp_failed,
                fail_mark,
                colors::RESET
            )
        } else {
            format!(
                "{}X:{}{}{}{}",
                colors::MFB_BLUE,
                colors::MFB_GREEN,
                xmp_ok,
                ok_mark,
                colors::RESET
            )
        };
        parts.push(xmp_msg);
    }

    // Video Stats: V: 12✓ (Only show if > 0 or has failures/skips)
    if vid_ok > 0 || vid_fail > 0 || vid_skip > 0 {
        let mut v_stat = format!(
            "{}V:{}{}{}{}",
            colors::MFB_PURPLE,
            colors::MFB_GREEN,
            vid_ok,
            ok_mark,
            colors::RESET
        );
        if vid_skip > 0 {
            let _ = write!(v_stat, "{}{}{}s", colors::DIM, colors::MFB_YELLOW, vid_skip);
        }
        if vid_fail > 0 {
            let _ = write!(
                v_stat,
                "{}{}{}{}",
                colors::DIM,
                colors::MFB_RED,
                vid_fail,
                fail_mark
            );
        }
        parts.push(v_stat);
    } else {
        // All zeros - don't show V:0✓
        return String::new();
    }

    // Preprocessing: P: 1✓ (Only show if > 0 for video)
    if preprocess_ok > 0 {
        parts.push(format!(
            "{}P:{}{}{}{}",
            colors::MFB_CYAN,
            colors::MFB_GREEN,
            preprocess_ok,
            ok_mark,
            colors::RESET
        ));
    }

    parts.join(&format!("{} ", colors::DIM))
}

const fn emit_combined_status_line(_img_ok: u64, _img_fail: u64) {
    // Deprecated: UI now relies on inline stats via get_current_stats_string()
    // in TaskResult
}

fn format_xmp_jxl_images_line(
    xmp_ok: u64,
    _xmp_done: bool,
    xmp_failed: u64,
    jxl_ok: u64,
    img_ok: u64,
    img_fail: u64,
    img_skip: u64,
    preprocess_ok: u64,
    _fallback_ok: u64,
) -> String {
    let ok_mark = crate::media_conversion_gate::ui_icon_pick("✓", "+");
    let fail_mark = crate::media_conversion_gate::ui_icon_pick("✗", "x");
    let images_ok = img_ok + jxl_ok;
    let mut parts = Vec::new();

    // XMP Stats: X: 12✓
    let xmp_msg = if xmp_failed > 0 {
        format!(
            "{}X:{}{}{}{}{}{}{}{}",
            colors::MFB_BLUE,
            colors::MFB_GREEN,
            xmp_ok,
            ok_mark,
            colors::DIM,
            colors::MFB_RED,
            xmp_failed,
            fail_mark,
            colors::RESET
        )
    } else {
        format!(
            "{}X:{}{}{}{}",
            colors::MFB_BLUE,
            colors::MFB_GREEN,
            xmp_ok,
            ok_mark,
            colors::RESET
        )
    };
    parts.push(xmp_msg);

    // Image Stats: I: 123✓
    let img_msg = if img_fail > 0 || img_skip > 0 {
        let mut i_stat = format!(
            "{}I:{}{}{}{}",
            colors::MFB_PURPLE,
            colors::MFB_GREEN,
            images_ok,
            ok_mark,
            colors::RESET
        );
        if img_skip > 0 {
            let _ = write!(i_stat, "{}{}{}s", colors::DIM, colors::MFB_YELLOW, img_skip);
        }
        if img_fail > 0 {
            let _ = write!(
                i_stat,
                "{}{}{}{}",
                colors::DIM,
                colors::MFB_RED,
                img_fail,
                fail_mark
            );
        }
        i_stat
    } else {
        format!(
            "{}I:{}{}{}{}",
            colors::MFB_PURPLE,
            colors::MFB_GREEN,
            images_ok,
            ok_mark,
            colors::RESET
        )
    };
    parts.push(img_msg);

    // Preprocessing: P: 1✓
    parts.push(format!(
        "{}P:{}{}{}{}",
        colors::MFB_CYAN,
        colors::MFB_GREEN,
        preprocess_ok,
        ok_mark,
        colors::RESET
    ));

    parts.join(&format!("{} ", colors::DIM))
}

/// Call when an XMP sidecar is found and a merge is about to be attempted.
pub fn xmp_merge_attempt() {
    XMP_ATTEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call on successful merge. Prints milestone line on EVERY merge (persistent
/// display). Same line shows XMP count and Images OK/failed (JXL merged into
/// Images) when non-zero.
pub fn xmp_merge_success() {
    let _success = XMP_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    // Always emit status line on every XMP merge for persistent display
    let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    emit_combined_status_line(img_ok, img_fail);
}

/// Format a statistics status line with the 📊 emoji prefix (for run log
/// alignment).
#[must_use]
pub fn format_status_line(msg: &str) -> String {
    fmt_stats_line_final(msg)
}

/// Call on failed merge. Logs the error on its own line.
pub fn xmp_merge_failure(msg: &str) {
    let stats = fmt_stats_line_final("");
    crate::ui_stderr::line(
        symbols::WARNING,
        symbols::plain::WARNING,
        format!("{stats}XMP merge failed: {msg}"),
    );
}

/// Call after all processing is done to print the final summary.
/// Same line shows XMP summary, Images OK/failed, and Pre-processing count when
/// non-zero.
pub fn xmp_merge_finalize() {
    let is_video = IS_VIDEO_MODE.load(Ordering::Relaxed);
    let xmp_total = XMP_ATTEMPT_COUNT.load(Ordering::Relaxed);
    let jxl_ok = JXL_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    let img_skip = IMAGE_SKIP_COUNT.load(Ordering::Relaxed);
    let vid_skip = VIDEO_SKIP_COUNT.load(Ordering::Relaxed);
    let preprocess_ok = PREPROCESSING_COUNT.load(Ordering::Relaxed);
    let fallback_ok = FALLBACK_SUCCESS_COUNT.load(Ordering::Relaxed);
    let vid_ok = VIDEO_SUCCESS_COUNT.load(Ordering::Relaxed);
    let vid_fail = VIDEO_FAIL_COUNT.load(Ordering::Relaxed);

    if is_video {
        if vid_ok > 0 || vid_fail > 0 || xmp_total > 0 || preprocess_ok > 0 || fallback_ok > 0 {
            let mut parts = Vec::new();
            if xmp_total > 0 {
                let success = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
                let failed = xmp_total.saturating_sub(success);
                parts.push(if failed > 0 {
                    format!("XMP: {success} OK, {failed} failed")
                } else {
                    format!("XMP: {success} OK")
                });
            }
            if vid_ok > 0 || vid_fail > 0 || vid_skip > 0 {
                let mut vid_part = if vid_fail > 0 {
                    format!("Videos: {vid_ok} OK, {vid_fail} failed")
                } else {
                    format!("Videos: {vid_ok} OK")
                };
                if vid_skip > 0 {
                    let _ = write!(vid_part, " ({vid_skip} skipped)");
                }
                parts.push(vid_part);
            }
            if preprocess_ok > 0 {
                parts.push(format!("Pre-processing: {preprocess_ok} done"));
            }
            if fallback_ok > 0 {
                parts.push(format!("Fallback: {fallback_ok} done"));
            }
            let line = fmt_stats_line_final(&parts.join("   "));
            emit_stderr(&line);
        }
        return;
    }

    if xmp_total > 0 {
        let success = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
        let failed = xmp_total.saturating_sub(success);
        let msg = format_xmp_jxl_images_line(
            success,
            true,
            failed,
            jxl_ok,
            img_ok,
            img_fail,
            img_skip,
            preprocess_ok,
            fallback_ok,
        );
        let line = fmt_stats_line_final(&msg);
        emit_stderr(&line);
    } else if jxl_ok > 0
        || img_ok > 0
        || img_fail > 0
        || img_skip > 0
        || preprocess_ok > 0
        || fallback_ok > 0
    {
        let mut parts = Vec::new();
        let images_ok = img_ok + jxl_ok;
        if images_ok > 0 || img_fail > 0 || img_skip > 0 {
            let mut img_part = if img_fail > 0 {
                format!("Images: {images_ok} OK, {img_fail} failed")
            } else {
                format!("Images: {images_ok} OK")
            };
            if img_skip > 0 {
                let _ = write!(img_part, " ({img_skip} skipped)");
            }
            parts.push(img_part);
        }
        if preprocess_ok > 0 {
            parts.push(format!("Pre-processing: {preprocess_ok} done"));
        }
        if fallback_ok > 0 {
            parts.push(format!("Fallback: {fallback_ok} done"));
        }
        let line = fmt_stats_line_final(&parts.join("   "));
        emit_stderr(&line);
    }
}

#[cfg(test)]
mod terminal_ux_tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn configure_terminal_ux_respects_no_color() {
        set_plain_mode(false);
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        configure_terminal_ux(false);
        assert!(is_plain_mode());
        unsafe {
            std::env::remove_var("NO_COLOR");
        }
        set_plain_mode(false);
    }

    #[test]
    #[serial_test::serial]
    fn configure_terminal_ux_cli_plain() {
        set_plain_mode(false);
        configure_terminal_ux(true);
        assert!(is_plain_mode());
        set_plain_mode(false);
    }

    #[test]
    #[serial_test::serial]
    fn test_set_default_run_log_file_with_session_id() {
        let original_writer = lock_log_writer().take();
        let temp_dir = tempfile::tempdir().expect("log temp dir");
        let log_dir = temp_dir.path().join("logs");

        let test_session_id = "test_session_12345";
        let _session_guard = crate::common_utils::EnvGuard::set("MFB_SESSION_ID", test_session_id);
        let _log_dir_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_LOG_DIR,
            log_dir.to_str().expect("utf-8 log dir"),
        );

        let result = set_default_run_log_file("test_bin");
        assert!(result.is_ok());

        let expected_path = log_dir.join(format!("test_bin_run_{test_session_id}.log"));
        assert!(
            expected_path.is_file(),
            "Expected log file was not created: {expected_path:?}"
        );

        *lock_log_writer() = None; // Close file handle
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "progress_mode_test_log_cleanup",
            &expected_path,
        );

        *lock_log_writer() = original_writer;
    }

    #[test]
    #[serial_test::serial]
    fn default_run_log_writer_rotates_before_exceeding_thirty_mib() {
        let original_writer = lock_log_writer().take();
        let temp_dir = tempfile::tempdir().expect("log temp dir");
        let log_path = temp_dir.path().join("run.log");

        set_log_file(&log_path).expect("open run log");
        let chunk = "x".repeat(1024 * 1024);
        for _ in 0..31 {
            write_to_log(&chunk);
        }
        flush_log_file();
        *lock_log_writer() = None;

        let cap = 30 * 1024 * 1024_u64;
        let files: Vec<_> = std::fs::read_dir(temp_dir.path())
            .expect("read run log dir")
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("run"))
            .collect();

        assert!(
            files.len() >= 2,
            "expected rotated run logs, got {}",
            files.len()
        );
        for file in files {
            let len = file
                .metadata()
                .unwrap_or_else(|err| panic!("metadata for {:?}: {err:?}", file.path()))
                .len();
            assert!(
                len <= cap,
                "run log file exceeded cap: {:?} len={len} cap={cap}",
                file.path()
            );
        }

        *lock_log_writer() = original_writer;
    }
}
