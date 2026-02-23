//! v7.3.2: Progress Mode - controls progress bar display
//!
//! Avoids progress output clutter when processing in parallel.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

// ── Per-thread log context (file name or ID) for concurrent processing ───────
// When set, every log_eprintln! / verbose_eprintln! line is prefixed so interleaved
// output from multiple files can be attributed correctly.

thread_local! {
    static LOG_PREFIX: RefCell<String> = RefCell::new(String::new());
}

const LOG_PREFIX_MAX_LEN: usize = 40;

/// Set the current thread's log prefix (e.g. file name or short ID). Cleared on drop of LogContextGuard.
pub fn set_log_context(prefix: &str) {
    let s = if prefix.len() > LOG_PREFIX_MAX_LEN {
        format!("{}…", &prefix[..LOG_PREFIX_MAX_LEN.saturating_sub(1)])
    } else {
        prefix.to_string()
    };
    LOG_PREFIX.with(|p| *p.borrow_mut() = s);
}

/// Clear the current thread's log prefix.
pub fn clear_log_context() {
    LOG_PREFIX.with(|p| p.borrow_mut().clear());
}

/// Prefix the given line with current context if set. Used by log macros.
pub fn format_log_line(line: &str) -> String {
    LOG_PREFIX.with(|p| {
        let prefix = p.borrow();
        if prefix.is_empty() {
            line.to_string()
        } else {
            format!("[{}] {}", prefix, line)
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

static LOG_FILE_WRITER: Mutex<Option<BufWriter<File>>> = Mutex::new(None);

/// Open (or create) the log file. Call once at startup.
pub fn set_log_file(path: &std::path::Path) -> std::io::Result<()> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    if let Ok(mut guard) = LOG_FILE_WRITER.lock() {
        *guard = Some(BufWriter::with_capacity(64 * 1024, file));
    }
    Ok(())
}

/// Returns true if a log file has been configured.
pub fn has_log_file() -> bool {
    LOG_FILE_WRITER
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false)
}

/// Write a line to the log file (no-op if no log file is configured).
/// Does NOT write to stderr — use log_eprintln! or verbose_eprintln! for dual output.
pub fn write_to_log(line: &str) {
    if let Ok(mut guard) = LOG_FILE_WRITER.lock() {
        if let Some(ref mut w) = *guard {
            let _ = writeln!(w, "{}", line);
        }
    }
}

/// Flush the log file buffer. Call at program exit.
pub fn flush_log_file() {
    if let Ok(mut guard) = LOG_FILE_WRITER.lock() {
        if let Some(ref mut w) = *guard {
            let _ = w.flush();
        }
    }
}

/// Print a milestone progress line every N events (avoids \r interleaving in concurrent output).
fn xmp_milestone_interval(count: u64) -> bool {
    if count <= 10 {
        count % 5 == 0
    } else if count <= 100 {
        count % 20 == 0
    } else {
        count % 100 == 0
    }
}

static QUIET_MODE: AtomicBool = AtomicBool::new(false);

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
            eprintln!($($arg)*);
        }
    };
}

pub fn create_conditional_progress(total: u64, prefix: &str) -> indicatif::ProgressBar {
    if is_quiet_mode() {
        indicatif::ProgressBar::hidden()
    } else {
        crate::create_progress_bar(total, prefix)
    }
}

// ── Verbose mode ──────────────────────────────────────────────────────────────
// Default OFF: noisy success messages (XMP merge, JXL info) are hidden.
// Call `set_verbose_mode(true)` at startup when --verbose is passed.

static VERBOSE_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose_mode(v: bool) {
    VERBOSE_MODE.store(v, Ordering::Relaxed);
}

pub fn is_verbose_mode() -> bool {
    VERBOSE_MODE.load(Ordering::Relaxed)
}

/// Print to stderr only when verbose mode is enabled.
/// Always writes to the log file when one is configured.
/// When set via set_log_context(), the line is prefixed with [prefix] for concurrent file processing.
#[macro_export]
macro_rules! verbose_eprintln {
    () => {{
        $crate::progress_mode::write_to_log("");
        if $crate::progress_mode::is_verbose_mode() {
            eprintln!();
        }
    }};
    ($($arg:tt)*) => {{
        if $crate::progress_mode::has_log_file() || $crate::progress_mode::is_verbose_mode() {
            let _msg = format!($($arg)*);
            let _line = $crate::progress_mode::format_log_line(&_msg);
            $crate::progress_mode::write_to_log(&_line);
            if $crate::progress_mode::is_verbose_mode() {
                eprintln!("{}", _line);
            }
        }
    }};
}

/// Print to both stderr and the log file (if configured).
/// When set via set_log_context(), the line is prefixed with [prefix] for concurrent file processing.
#[macro_export]
macro_rules! log_eprintln {
    () => {{
        $crate::progress_mode::write_to_log("");
        eprintln!();
    }};
    ($($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        let _line = $crate::progress_mode::format_log_line(&_msg);
        $crate::progress_mode::write_to_log(&_line);
        eprintln!("{}", _line);
    }};
}

// ── XMP merge live counter ─────────────────────────────────────────────────
// Tracks XMP sidecar merge attempts/successes and prints a live \r status line.

static XMP_ATTEMPT_COUNT: AtomicU64 = AtomicU64::new(0);
static XMP_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);

/// Call when an XMP sidecar is found and a merge is about to be attempted.
pub fn xmp_merge_attempt() {
    XMP_ATTEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call on successful merge. Prints a milestone line every N merges (no \r interleaving).
pub fn xmp_merge_success() {
    let success = XMP_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if xmp_milestone_interval(success) {
        let total = XMP_ATTEMPT_COUNT.load(Ordering::Relaxed);
        let msg = format!("[XMP] XMP merge: {} OK/{}", success, total);
        write_to_log(&msg);
        eprintln!("{}", msg);
    }
}

/// Call on failed merge. Logs the error on its own line.
pub fn xmp_merge_failure(msg: &str) {
    let line = format!("[XMP] ⚠️  XMP merge failed: {}", msg);
    write_to_log(&line);
    eprintln!("{}", line);
}

/// Call after all processing is done to print the final summary.
pub fn xmp_merge_finalize() {
    let total = XMP_ATTEMPT_COUNT.load(Ordering::Relaxed);
    if total > 0 {
        let success = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
        let msg = format!("[XMP] XMP merge done: {} OK/{}", success, total);
        write_to_log(&msg);
        eprintln!("{}", msg);
    }
}
