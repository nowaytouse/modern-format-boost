//! v7.3.2: Progress Mode - controls progress bar display
//!
//! Avoids progress output clutter when processing in parallel.
//! Stderr output is routed through tracing when a subscriber is set (init_logging).

use std::cell::RefCell;
use tracing;
use tracing::Level;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use std::vec::Vec;
use crate::modern_ui::{colors, symbols};

// ── Per-thread log context (file name or ID) for concurrent processing ───────
// When set, every log_eprintln! / verbose_eprintln! line is prefixed so interleaved
// output from multiple files can be attributed correctly.

thread_local! {
    static LOG_PREFIX: RefCell<String> = const { RefCell::new(String::new()) };
}

static RUN_LOG_IO_FAILURE_REPORTED: AtomicBool = AtomicBool::new(false);

fn report_run_log_io_failure(context: &str, detail: &str) {
    tracing::warn!(
        context = context,
        detail = detail,
        "Run log output degraded"
    );

    if !RUN_LOG_IO_FAILURE_REPORTED.swap(true, Ordering::Relaxed) {
        let _ = writeln!(std::io::stderr(), "⚠️ [Run Log] {}: {}", context, detail);
    }
}

/// Format duration as detailed string with progressive spacing strategy
/// Examples: "01Y   01M   01W   01D   01h 00m00s000ms" or "01M   01W   01D   01h 00m00s000ms" or "01W   01D   01h 00m00s000ms" or "01D   01h 00m00s000ms" or "01h 00m00s000ms" or "00m00s000ms" or "00s000ms"
pub fn format_duration_compact(duration: Duration) -> String {
    let total_millis = duration.as_millis();
    let years   = total_millis / (365 * 86400 * 1000);
    let months  = (total_millis % (365 * 86400 * 1000)) / (30 * 86400 * 1000);
    let weeks   = (total_millis % (30 * 86400 * 1000)) / (7 * 86400 * 1000);
    let days    = (total_millis % (7 * 86400 * 1000)) / (86400 * 1000);
    let hours   = (total_millis % (86400 * 1000)) / (3600 * 1000);
    let minutes = (total_millis % (3600 * 1000)) / (60 * 1000);
    let seconds = (total_millis % (60 * 1000)) / 1000;
    let millis  = total_millis % 1000;

    let mut parts = Vec::new();

    if years   > 0 { parts.push(format!("{:02}Y", years)); }
    if months  > 0 || years > 0 { parts.push(format!("{:02}M", months)); }
    if weeks   > 0 || months > 0 || years > 0 { parts.push(format!("{:02}W", weeks)); }
    if days    > 0 || weeks > 0 || months > 0 || years > 0 { parts.push(format!("{:02}D", days)); }
    if hours   > 0 || days > 0 || weeks > 0 || months > 0 || years > 0 { parts.push(format!("{:02}h", hours)); }
    if minutes > 0 || hours > 0 || days > 0 || weeks > 0 || months > 0 || years > 0 { parts.push(format!("{:02}m", minutes)); }

    // Seconds: only show when there are no hours-or-larger components
    // (avoids "1h01m40s" when "1h01m" is cleaner at hour-level precision)
    let has_hours_plus = hours > 0 || days > 0 || weeks > 0 || months > 0 || years > 0;
    if !has_hours_plus && (total_millis >= 1000 || seconds > 0) {
        parts.push(format!("{:02}s", seconds));
    }

    // Milliseconds: only show when there are no seconds-or-larger components
    // (i.e., sub-second precision is useful), or when ms is non-zero and
    // there are no minutes-or-larger components (show "5s372ms" but not "30s000ms").
    let has_large_unit = minutes > 0 || hours > 0 || days > 0 || weeks > 0 || months > 0 || years > 0;
    if !has_large_unit && millis > 0 {
        parts.push(format!("{:03}ms", millis));
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
        let suffix_start = first.find(|c: char| c.is_alphabetic()).unwrap_or(first.len());
        let digits = &first[..suffix_start];
        let suffix = &first[suffix_start..];
        let trimmed = digits.trim_start_matches('0');
        let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
        *first = format!("{}{}", trimmed, suffix);
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
                "  "  // 2 spaces after hours
            } else {
                ""    // no extra spacing for minutes/seconds/ms
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
const LOG_TAG_WIDTH: usize = 28;

/// Max visible chars for the filename displayed inside [brackets].
/// With LOG_TAG_WIDTH=28, tag=[prefix] uses prefix+2 bytes, max prefix = 25.
const LOG_PREFIX_MAX_DISPLAY: usize = 25;

/// Prefix for periodic statistics lines — emoji instead of [Info] to avoid
/// confusion with log severity levels. Followed by a fixed-width space pad so
/// the message body aligns with regular file-tag lines.
/// Display width: 1 emoji (2 cells on most terminals) + spaces to reach LOG_TAG_WIDTH.
const STATS_PREFIX: &str = "📊 ";

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

/// Pad a file-context tag (e.g. `[file.jpeg]`) to LOG_TAG_WIDTH chars for aligned message body.
/// Always produces exactly LOG_TAG_WIDTH chars, or tag + one space if tag is already wide.
fn pad_tag(tag: &str) -> String {
    if tag.len() >= LOG_TAG_WIDTH {
        format!("{} ", tag)
    } else {
        format!("{}{}", tag, " ".repeat(LOG_TAG_WIDTH - tag.len()))
    }
}
/// Format a statistics summary line (plain, no leading blank line) for
/// the final summary emitted after all processing is done.
fn fmt_stats_line_final(msg: &str) -> String {
    format!("    {} {}", STATS_PREFIX.trim(), msg)
}

/// Set the current thread's log prefix (e.g. file name or short ID). Cleared on drop of LogContextGuard.
/// Truncates long names to LOG_PREFIX_MAX_DISPLAY chars, preserving the file extension:
///   "Image_103999006594198.jpeg" → "Image_103999006…jpeg"
///   "Cache_4ac28036da7d11be.jpg" → "Cache_4ac28036da7…jpg"
pub fn set_log_context(prefix: &str) {
    let s = if prefix.chars().count() > LOG_PREFIX_MAX_DISPLAY {
        if let Some(dot_pos) = prefix.rfind('.') {
            let ext = &prefix[dot_pos..]; // e.g. ".jpeg"
            let ext_chars = ext.chars().count();
            if ext_chars < LOG_PREFIX_MAX_DISPLAY - 2 {
                let stem_max_chars = LOG_PREFIX_MAX_DISPLAY - ext_chars - 1;
                let stem = truncate_to_char_boundary(prefix, stem_max_chars);
                format!("{}…{}", stem, ext)
            } else {
                let head = truncate_to_char_boundary(prefix, LOG_PREFIX_MAX_DISPLAY - 1);
                format!("{}…", head)
            }
        } else {
            let head = truncate_to_char_boundary(prefix, LOG_PREFIX_MAX_DISPLAY - 1);
            format!("{}…", head)
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

/// Detect file type emoji based on extension.
/// Returns 🖼️  for still images, 🎞️  for GIF/animated, 🎬 for videos, empty string for unknown.
fn file_type_emoji(filename: &str) -> &'static str {
    if let Some(ext_start) = filename.rfind('.') {
        let ext = &filename[ext_start + 1..].to_lowercase();
        match ext.as_str() {
            // Animated / GIF
            "gif" => "🎞️ ",
            // Still images
            "jpg" | "jpeg" | "png" | "webp" | "avif" | "heic" | "heif" | "jxl"
            | "bmp" | "tiff" | "tif" | "ico" | "svg" | "psd" | "raw" | "cr2" | "nef"
            | "arw" | "dng" | "orf" | "rw2" | "exr" | "qoi" | "flif" | "jp2" | "j2k" => "🖼️  ",
            // Videos
            "mp4" | "mov" | "avi" | "mkv" | "webm" | "flv" | "wmv" | "m4v" | "mpg"
            | "mpeg" | "3gp" | "ogv" | "ts" | "mts" | "m2ts" => "🎬 ",
            _ => "",
        }
    } else {
        ""
    }
}

/// Format a log line with optional tag, emoji prefix, and padded indent so message bodies align.
/// When a filename prefix is set, prepends a file-type emoji (🖼️ image / 🎞️ GIF / 🎬 video).
pub fn format_log_line(line: &str) -> String {
    LOG_PREFIX.with(|p| {
        let prefix = p.borrow();
        if prefix.is_empty() {
            format!("{}{}", " ".repeat(LOG_TAG_WIDTH), line)
        } else {
            let emoji = file_type_emoji(&prefix);
            format!("{}{}{}", emoji, pad_tag(&format!("[{}]", prefix)), line)
        }
    })
}

/// Guard that clears log context when dropped. Use at the start of per-file processing.
pub struct LogContextGuard;

impl Drop for LogContextGuard {
    fn drop(&mut self) {
        clear_log_context();
    }
}

// ── File log writer ────────────────────────────────────────────────────────────
// When a log file path is configured, ALL messages (both regular and verbose) are
// written to it in full detail, regardless of the terminal verbose setting.
//
// **If the log file is renamed/moved while the process is running:** on Unix we keep
// writing to the same open file descriptor (same inode). Data is not lost, but the
// content keeps going to the renamed file; the original path may show a new empty
// file if one was recreated. So avoid renaming the run log file until the process exits.
//
// **File lock:** we take an advisory exclusive lock (flock LOCK_EX) on open so other
// processes that respect the lock cannot truncate or overwrite the log.

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

static LOG_FILE_WRITER: Mutex<Option<BufWriter<File>>> = Mutex::new(None);

/// Open (or create) the log file and take an advisory exclusive lock so it is not truncated by others.
/// Call once at startup. Registers a forwarder so tracing events are also written to this run log.
pub fn set_log_file(path: &std::path::Path) -> std::io::Result<()> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    #[cfg(unix)]
    flock_log_exclusive(&file)?;
    if let Ok(mut guard) = LOG_FILE_WRITER.lock() {
        *guard = Some(BufWriter::with_capacity(64 * 1024, file));
    }
    crate::logging::register_run_log_forwarder(Box::new(write_to_log));
    Ok(())
}

/// Returns true if a log file has been configured.
pub fn has_log_file() -> bool {
    LOG_FILE_WRITER.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// If no log file is configured, open a default run log under `./logs/`
/// with a timestamp in the filename so each run gets a unique file
/// (e.g. `./logs/img_hevc_run_2026-02-28_14-30-00.log`). That directory is gitignored.
/// Call at Run startup so quality and progress are always written without requiring `--log-file`.
pub fn set_default_run_log_file(binary_name: &str) -> std::io::Result<()> {
    if binary_name.contains("vid") {
        IS_VIDEO_MODE.store(true, Ordering::Relaxed);
    }
    if has_log_file() {
        return Ok(());
    }
    let dir = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("logs");
    std::fs::create_dir_all(&dir)?;
    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let path = dir.join(format!("{}_run_{}.log", binary_name, timestamp));
    set_log_file(&path)?;
    write_run_log_session_header(binary_name, &path);
    Ok(())
}

/// Write a session header line to the run log so the file clearly records that full output is being captured.
/// Call after set_log_file (or from set_default_run_log_file). If init_logging already emitted a line, it is written here so the run log has it too.
/// Respects log level (INFO): only written when level is INFO or more verbose.
pub fn write_run_log_session_header(program_name: &str, run_log_path: &std::path::Path) {
    if !has_log_file() {
        return;
    }
    if let Some(ref init_line) = crate::logging::take_init_message_for_run_log() {
        write_to_log_at_level(Level::INFO, init_line);
    }
    let line = format!(
        "  [stats] Run log attached program=\"{}\" run_log=\"{}\" (all stderr and tracing written here)",
        program_name,
        run_log_path.display()
    );
    write_to_log_at_level(Level::INFO, &line);
}

/// Write one progress line to the run log so the log has the same "Running: HH:MM:SS  N/total  message" as the terminal.
/// Call whenever the progress bar is updated (e.g. after set_position/set_message) so the run log is complete.
/// Respects log level (DEBUG): only written when level is DEBUG or TRACE.
pub fn write_progress_line_to_run_log(elapsed_secs: u64, current: u64, total: u64, message: &str) {
    if !has_log_file() {
        return;
    }
    let duration = Duration::from_secs(elapsed_secs);
    let compact_time = format_duration_compact(duration);
    let line = format!(
        "  Running: {}  {}/{}  {}",
        compact_time, current, total, message
    );
    write_to_log_at_level(Level::DEBUG, &line);
}

/// Write a line to the log file (no-op if no log file is configured).
/// Does NOT write to stderr — use log_eprintln! or verbose_eprintln! for dual output.
/// Strips ANSI escape codes so file logs are plain text.
/// Flushes after each write so log output is immediate (no loss on crash/kill).
pub fn write_to_log(line: &str) {
    // Ensure every line written to the run log has milestone stats appended (unless it already does)
    let line_with_stats = append_stats_to_line(line);
    let plain = crate::logging::strip_ansi_str(&line_with_stats);
    match LOG_FILE_WRITER.lock() {
        Ok(mut guard) => {
            if let Some(ref mut w) = *guard {
                if let Err(err) = writeln!(w, "{}", plain) {
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

/// Write a line to the run log only when the configured log level allows this level (so level has real effect).
/// Use for status/info (Level::Info), progress (Level::Debug), verbose (Level::Trace). Errors use write_to_log.
pub fn write_to_log_at_level(level: Level, line: &str) {
    if crate::logging::should_log(level) {
        write_to_log(line);
    }
}

/// Write conversion failure to the run log file immediately (so failures are in the log, not only stderr).
/// Call this whenever a single-file conversion returns Err, so the log file has the full error for later inspection.
/// Uses Level::Error so it is always written when level is WARN or ERROR (and any level includes errors).
pub fn log_conversion_failure(path: &std::path::Path, error: &str) {
    if has_log_file() {
        let line = format!("❌ Conversion failed {}: {}", path.display(), error);
        write_to_log_at_level(Level::ERROR, &line);
    }
}

/// Uniform indent for all stderr lines so logs are visually aligned (2 spaces).
const STDERR_INDENT: &str = "  ";

/// Returns true when stderr is connected to a real terminal (TTY) OR if FORCE_COLOR is set.
/// Cached after the first call — TTY state does not change during a run.
#[inline]
fn stderr_is_tty() -> bool {
    use std::sync::OnceLock;
    static IS_TTY: OnceLock<bool> = OnceLock::new();
    *IS_TTY.get_or_init(|| {
        if std::env::var("FORCE_COLOR").is_ok() {
            true
        } else {
            // Use the `console` crate's detection which correctly handles
            // NO_COLOR, TERM=dumb, CI env vars, and is_terminal() semantics.
            console::Term::stderr().is_term()
        }
    })
}

/// Emit a line to stderr (and to run log when configured).
///
/// * When stderr **is a TTY**: ANSI colour codes are forwarded as-is.
/// * When stderr **is not a TTY** (pipe/redirect/script): ANSI is stripped so
///   captured output is plain, readable text.
/// * The run-log always receives the plain (stripped) version.
#[inline]
pub fn emit_stderr(line: &str) {
    // Pause output if the Ctrl+C confirmation prompt is currently waiting for input
    crate::ctrlc_guard::wait_if_prompt_active();

    let mut active_ansi = String::new();
    let mut is_first = true;

    // Process each line separately to ensure milestone stats are appended correctly
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

        // File log always receives the plain line.
        if has_log_file() {
            write_to_log(&line_with_stats);
        }
        
        use std::io::Write;
        let out = if stderr_is_tty() {
            // TTY: keep colours.
            format!("{}{}", STDERR_INDENT, line_with_stats)
        } else {
            // Non-TTY: strip ANSI so piped / redirected output is clean.
            format!("{}{}", STDERR_INDENT, crate::logging::strip_ansi_str(&line_with_stats))
        };
        if let Err(err) = writeln!(std::io::stderr(), "{}", out) {
            tracing::warn!(
                error = %err,
                "Failed to write progress output to stderr"
            );
        }
    }
}

/// Flush the log file buffer. Call at program exit.
pub fn flush_log_file() {
    match LOG_FILE_WRITER.lock() {
        Ok(mut guard) => {
            if let Some(ref mut w) = *guard {
                if let Err(err) = w.flush() {
                    report_run_log_io_failure("failed to flush run log at shutdown", &err.to_string());
                }
            }
        }
        Err(err) => {
            report_run_log_io_failure("run log writer mutex poisoned during shutdown", &err.to_string());
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
    ($($arg:tt)*) => {
        if !$crate::progress_mode::is_quiet_mode() {
            $crate::progress_mode::emit_stderr(&format!($($arg)*));
        }
    };
}

// ── Verbose mode (single source of truth for process-wide verbose logging) ───
// Default OFF: noisy success messages (XMP merge, JXL info) are hidden.
// CLI should call `set_verbose_mode(true)` at startup when --verbose is passed;
// all verbose output uses `is_verbose_mode()` / `verbose_eprintln!` — no per-config flag.

static VERBOSE_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose_mode(v: bool) {
    VERBOSE_MODE.store(v, Ordering::Relaxed);
}

pub fn tracing_level_debug() -> Level {
    Level::DEBUG
}

pub fn is_verbose_mode() -> bool {
    VERBOSE_MODE.load(Ordering::Relaxed)
}

/// Print to stderr only when verbose mode is enabled.
/// Run log gets the line only when level allows (DEBUG: written at DEBUG/TRACE).
/// When set via set_log_context(), the line is prefixed with [prefix] for concurrent file processing.
#[macro_export]
macro_rules! verbose_eprintln {
    () => {{
        if $crate::progress_mode::has_log_file() && !$crate::progress_mode::is_verbose_mode() {
            $crate::progress_mode::write_to_log_at_level(tracing::Level::DEBUG, "");
        }
        if $crate::progress_mode::is_verbose_mode() {
            $crate::progress_mode::emit_stderr("");
        }
    }};
    ($($arg:tt)*) => {{
        if $crate::progress_mode::has_log_file() || $crate::progress_mode::is_verbose_mode() {
            let _msg = format!($($arg)*);
            let _line = $crate::progress_mode::format_log_line(&_msg);
            if $crate::progress_mode::has_log_file() && !$crate::progress_mode::is_verbose_mode() {
                let _line_with_stats = $crate::progress_mode::append_stats_to_line(&_line);
                $crate::progress_mode::write_to_log_at_level($crate::progress_mode::tracing_level_debug(), &_line_with_stats);
            } else if $crate::progress_mode::is_verbose_mode() {
                $crate::progress_mode::emit_stderr(&_line);
            }
        }
    }};
}

/// Print to both stderr and the run log file (if configured). Run log gets full TRACE-level detail.
/// When set via set_log_context(), the line is prefixed with [prefix] for concurrent file processing.
#[macro_export]
macro_rules! log_eprintln {
    () => {{
        $crate::progress_mode::emit_stderr("");
    }};
    ($($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        let _line = $crate::progress_mode::format_log_line(&_msg);
        $crate::progress_mode::emit_stderr(&_line);
    }};
}

// ── XMP merge + JXL + Images live counter ────────────────────────────────────
// Tracks XMP sidecar merge, JXL success, and image conversion success/failure; same line.

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

/// Call when a pre-processing step completes successfully (e.g. GIF→FFmpeg static frame). No per-line log; count is shown in the combined status line.
pub fn preprocessing_success() {
    PREPROCESSING_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call when a fallback pipeline completes successfully (e.g. ImageMagick→cjxl, FFmpeg→cjxl). No per-line log; count is shown in the combined status line.
pub fn fallback_success() {
    FALLBACK_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call when a JXL conversion completes successfully (e.g. from finalize_conversion).
pub fn jxl_success() {
    JXL_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call on successful image conversion. Prints milestone line on EVERY success (persistent display).
/// Same line shows XMP count and Images OK/failed (JXL merged into Images) when non-zero.
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
/// Logs a prominent message to stderr and increments the skip counter.
pub fn image_skipped(reason: &str) {
    let _img_skip = IMAGE_SKIP_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let line = format!("{}⏭️  {}  {}{}{}", colors::MFB_YELLOW, "[SKIP]", colors::RESET, colors::DIM, reason);
    log_eprintln!("{}", line);
    // Force a status line update so the skip count is visible immediately
    emit_combined_status_line(IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed), IMAGE_FAIL_COUNT.load(Ordering::Relaxed));
}

pub fn video_processed_success() {
    VIDEO_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn video_processed_failure() {
    VIDEO_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call when a video is skipped.
/// Logs a prominent message to stderr and increments the skip counter.
pub fn video_skipped(reason: &str) {
    let _ = VIDEO_SKIP_COUNT.fetch_add(1, Ordering::Relaxed);
    let line = format!("{}⏭️  {}  {}{}{}", colors::MFB_YELLOW, "[SKIP]", colors::RESET, colors::DIM, reason);
    log_eprintln!("{}", line);
}

/// Helper that appends milestone stats (XMP, Img, etc.) to a log line with aligned padding.
/// Skips if the line already contains stats or is empty.
pub fn append_stats_to_line(line: &str) -> String {
    let mut trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return line.to_string();
    }
    
    // Strip ANSI for accurate length calculation and duplicate check
    let plain = crate::logging::strip_ansi_str(trimmed);
    
    // Check if it already has stats (avoids double appending)
    // We check for "│ 📊" which is the delimiter used in get_current_stats_string()
    if plain.contains("│ 📊") {
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
    let padding_len = if visible_len < target_len { target_len - visible_len } else { 1 };
    
    // Put \x1b[0m before padding to prevent color bleed to stats
    format!("{}\x1b[0m{}{}", trimmed, " ".repeat(padding_len), stats_string)
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
        format_video_stats_line(vid_ok, vid_fail, vid_skip, xmp_ok, xmp_failed, preprocess_ok, fallback_ok)
    } else {
        format_xmp_jxl_images_line(xmp_ok, xmp_done, xmp_failed, jxl_ok, img_ok, img_fail, img_skip, preprocess_ok, fallback_ok)
    };
    
    // Very minimalist separator for video
    let separator = if is_video {
        format!("{}│{}", colors::DIM, colors::RESET)
    } else {
        format!("{}│{} {}", colors::DIM, colors::RESET, symbols::CHART)
    };
    
    format!(" {} {}", separator, msg)
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
    let mut parts = Vec::new();

    // XMP Stats: X: 12✓ (Only show if used for video)
    if xmp_ok > 0 || xmp_failed > 0 {
        let xmp_msg = if xmp_failed > 0 {
            format!("{}X:{}{}✓{}{}{}✗{}", colors::MFB_BLUE, colors::MFB_GREEN, xmp_ok, colors::DIM, colors::MFB_RED, xmp_failed, colors::RESET)
        } else {
            format!("{}X:{}{}✓{}", colors::MFB_BLUE, colors::MFB_GREEN, xmp_ok, colors::RESET)
        };
        parts.push(xmp_msg);
    }

    // Video Stats: V: 12✓
    let vid_msg = if vid_fail > 0 || vid_skip > 0 {
        let mut v_stat = format!("{}V:{}{}✓", colors::MFB_PURPLE, colors::MFB_GREEN, vid_ok);
        if vid_skip > 0 {
            v_stat.push_str(&format!("{}{}{}s", colors::DIM, colors::MFB_YELLOW, vid_skip));
        }
        if vid_fail > 0 {
            v_stat.push_str(&format!("{}{}{}✗", colors::DIM, colors::MFB_RED, vid_fail));
        }
        v_stat.push_str(colors::RESET);
        v_stat
    } else {
        format!("{}V:{}{}✓{}", colors::MFB_PURPLE, colors::MFB_GREEN, vid_ok, colors::RESET)
    };
    parts.push(vid_msg);

    // Preprocessing: P: 1✓ (Only show if > 0 for video)
    if preprocess_ok > 0 {
        parts.push(format!("{}P:{}{}✓{}", colors::MFB_CYAN, colors::MFB_GREEN, preprocess_ok, colors::RESET));
    }

    parts.join(&format!("{} ", colors::DIM))
}

fn emit_combined_status_line(_img_ok: u64, _img_fail: u64) {
    // Deprecated: UI now relies on inline stats via get_current_stats_string() in ConversionResult
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
    let images_ok = img_ok + jxl_ok;
    let mut parts = Vec::new();

    // XMP Stats: X: 12✓
    let xmp_msg = if xmp_failed > 0 {
        format!("{}X:{}{}✓{}{}{}✗{}", colors::MFB_BLUE, colors::MFB_GREEN, xmp_ok, colors::DIM, colors::MFB_RED, xmp_failed, colors::RESET)
    } else {
        format!("{}X:{}{}✓{}", colors::MFB_BLUE, colors::MFB_GREEN, xmp_ok, colors::RESET)
    };
    parts.push(xmp_msg);

    // Image Stats: I: 123✓
    let img_msg = if img_fail > 0 || img_skip > 0 {
        let mut i_stat = format!("{}I:{}{}✓", colors::MFB_PURPLE, colors::MFB_GREEN, images_ok);
        if img_skip > 0 {
            i_stat.push_str(&format!("{}{}{}s", colors::DIM, colors::MFB_YELLOW, img_skip));
        }
        if img_fail > 0 {
            i_stat.push_str(&format!("{}{}{}✗", colors::DIM, colors::MFB_RED, img_fail));
        }
        i_stat.push_str(colors::RESET);
        i_stat
    } else {
        format!("{}I:{}{}✓{}", colors::MFB_PURPLE, colors::MFB_GREEN, images_ok, colors::RESET)
    };
    parts.push(img_msg);

    // Preprocessing: P: 1✓
    parts.push(format!("{}P:{}{}✓{}", colors::MFB_CYAN, colors::MFB_GREEN, preprocess_ok, colors::RESET));

    parts.join(&format!("{} ", colors::DIM))
}

/// Call when an XMP sidecar is found and a merge is about to be attempted.
pub fn xmp_merge_attempt() {
    XMP_ATTEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call on successful merge. Prints milestone line on EVERY merge (persistent display).
/// Same line shows XMP count and Images OK/failed (JXL merged into Images) when non-zero.
pub fn xmp_merge_success() {
    let _success = XMP_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    // Always emit status line on every XMP merge for persistent display
    let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    emit_combined_status_line(img_ok, img_fail);
}

/// Format a statistics status line with the 📊 emoji prefix (for run log alignment).
pub fn format_status_line(msg: &str) -> String {
    fmt_stats_line_final(msg)
}

/// Call on failed merge. Logs the error on its own line.
pub fn xmp_merge_failure(msg: &str) {
    let line = format!("{}⚠️  XMP merge failed: {}", fmt_stats_line_final(""), msg);
    emit_stderr(&line);
}

/// Call after all processing is done to print the final summary.
/// Same line shows XMP summary, Images OK/failed, and Pre-processing count when non-zero.
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
                    format!("XMP: {} OK, {} failed", success, failed)
                } else {
                    format!("XMP: {} OK", success)
                });
            }
            if vid_ok > 0 || vid_fail > 0 || vid_skip > 0 {
                let mut vid_part = if vid_fail > 0 {
                    format!("Videos: {} OK, {} failed", vid_ok, vid_fail)
                } else {
                    format!("Videos: {} OK", vid_ok)
                };
                if vid_skip > 0 {
                    vid_part.push_str(&format!(" ({} skipped)", vid_skip));
                }
                parts.push(vid_part);
            }
            if preprocess_ok > 0 {
                parts.push(format!("Pre-processing: {} done", preprocess_ok));
            }
            if fallback_ok > 0 {
                parts.push(format!("Fallback: {} done", fallback_ok));
            }
            let line = fmt_stats_line_final(&parts.join("   "));
            emit_stderr(&line);
        }
        return;
    }

    if xmp_total > 0 {
        let success = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
        let failed = xmp_total.saturating_sub(success);
        let msg = format_xmp_jxl_images_line(success, true, failed, jxl_ok, img_ok, img_fail, img_skip, preprocess_ok, fallback_ok);
        let line = fmt_stats_line_final(&msg);
        emit_stderr(&line);
    } else {
        if jxl_ok > 0 || img_ok > 0 || img_fail > 0 || img_skip > 0 || preprocess_ok > 0 || fallback_ok > 0 {
            let mut parts = Vec::new();
            let images_ok = img_ok + jxl_ok;
            if images_ok > 0 || img_fail > 0 || img_skip > 0 {
                let mut img_part = if img_fail > 0 {
                    format!("Images: {} OK, {} failed", images_ok, img_fail)
                } else {
                    format!("Images: {} OK", images_ok)
                };
                if img_skip > 0 {
                    img_part.push_str(&format!(" ({} skipped)", img_skip));
                }
                parts.push(img_part);
            }
            if preprocess_ok > 0 {
                parts.push(format!("Pre-processing: {} done", preprocess_ok));
            }
            if fallback_ok > 0 {
                parts.push(format!("Fallback: {} done", fallback_ok));
            }
            let line = fmt_stats_line_final(&parts.join("   "));
            emit_stderr(&line);
        }
    }
}
