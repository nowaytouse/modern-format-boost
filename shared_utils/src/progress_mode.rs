//! v7.3.2: Progress Mode - controls progress bar display
//!
//! Avoids progress output clutter when processing in parallel.
//! Stderr output is routed through tracing when a subscriber is set (init_logging).

use std::cell::RefCell;
use tracing;
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

const LOG_PREFIX_MAX_LEN: usize = 28;

/// Width of the tag column so all message bodies align (e.g. [file.jpeg], [XMP]).
const LOG_TAG_WIDTH: usize = 24;

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

/// Pad tag (e.g. `[file.jpeg]` or `[XMP]`) to LOG_TAG_WIDTH for aligned message body.
fn pad_tag(tag: &str) -> String {
    let w = LOG_TAG_WIDTH;
    if tag.len() >= w {
        format!("{} ", tag)
    } else {
        format!("{}{}", tag, " ".repeat(w - tag.len()))
    }
}

/// Set the current thread's log prefix (e.g. file name or short ID). Cleared on drop of LogContextGuard.
pub fn set_log_context(prefix: &str) {
    let s = if prefix.len() > LOG_PREFIX_MAX_LEN {
        let head = truncate_to_char_boundary(prefix, LOG_PREFIX_MAX_LEN.saturating_sub(1));
        format!("{}…", head)
    } else {
        prefix.to_string()
    };
    LOG_PREFIX.with(|p| *p.borrow_mut() = s);
}

/// Clear the current thread's log prefix.
pub fn clear_log_context() {
    LOG_PREFIX.with(|p| p.borrow_mut().clear());
}

/// Format a log line with optional tag and padded indent so message bodies align.
pub fn format_log_line(line: &str) -> String {
    LOG_PREFIX.with(|p| {
        let prefix = p.borrow();
        if prefix.is_empty() {
            format!("{}{}", " ".repeat(LOG_TAG_WIDTH), line)
        } else {
            format!("{}{}", pad_tag(&format!("[{}]", prefix)), line)
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
    LOG_FILE_WRITER.lock().map(|g| g.is_some()).unwrap_or(false)
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

/// Uniform indent for all stderr lines so logs are visually aligned (2 spaces).
const STDERR_INDENT: &str = "  ";

/// Emit a line to stderr via tracing (so it appears when a tracing subscriber is initialized).
/// Applies a uniform 2-space indent so multi-line blocks (e.g. precheck report) stay aligned.
/// When stderr is not a TTY (e.g. redirect/script), ANSI is stripped so output is plain text.
#[inline]
pub fn emit_stderr(line: &str) {
    use std::borrow::Cow;
    use std::io::IsTerminal;
    let msg: Cow<str> = if std::io::stderr().is_terminal() {
        Cow::Borrowed(line)
    } else {
        Cow::Owned(crate::logging::strip_ansi_str(line))
    };
    if msg.is_empty() {
        tracing::info!("");
    } else {
        tracing::info!("{}{}", STDERR_INDENT, msg);
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

fn image_milestone_interval(total_processed: u64) -> bool {
    if total_processed == 0 {
        return false;
    }
    if total_processed <= 100 {
        total_processed % 50 == 0
    } else if total_processed <= 1000 {
        total_processed % 200 == 0
    } else {
        total_processed % 500 == 0
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
            $crate::progress_mode::emit_stderr(&format!($($arg)*));
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

// ── Verbose mode (single source of truth for process-wide verbose logging) ───
// Default OFF: noisy success messages (XMP merge, JXL info) are hidden.
// CLI should call `set_verbose_mode(true)` at startup when --verbose is passed;
// all verbose output uses `is_verbose_mode()` / `verbose_eprintln!` — no per-config flag.

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
            $crate::progress_mode::emit_stderr("");
        }
    }};
    ($($arg:tt)*) => {{
        if $crate::progress_mode::has_log_file() || $crate::progress_mode::is_verbose_mode() {
            let _msg = format!($($arg)*);
            let _line = $crate::progress_mode::format_log_line(&_msg);
            $crate::progress_mode::write_to_log(&_line);
            if $crate::progress_mode::is_verbose_mode() {
                $crate::progress_mode::emit_stderr(&_line);
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
        $crate::progress_mode::emit_stderr("");
    }};
    ($($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        let _line = $crate::progress_mode::format_log_line(&_msg);
        $crate::progress_mode::write_to_log(&_line);
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

/// Call when a JXL conversion completes successfully (e.g. from finalize_conversion).
pub fn jxl_success() {
    JXL_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call when an image conversion completes successfully (not skipped).
/// May emit a combined status line (XMP + JXL + Images) at image milestones.
pub fn image_processed_success() {
    let img_ok = IMAGE_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    if image_milestone_interval(img_ok + img_fail) {
        emit_combined_status_line(img_ok, img_fail);
    }
}

/// Call when an image conversion fails.
/// May emit a combined status line (XMP + JXL + Images) at image milestones.
pub fn image_processed_failure() {
    let _ = IMAGE_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
    let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    if image_milestone_interval(img_ok + img_fail) {
        emit_combined_status_line(img_ok, img_fail);
    }
}

fn emit_combined_status_line(img_ok: u64, img_fail: u64) {
    let xmp_ok = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
    let xmp_total = XMP_ATTEMPT_COUNT.load(Ordering::Relaxed);
    let xmp_done = false;
    let xmp_failed = xmp_total.saturating_sub(xmp_ok);
    let jxl_ok = JXL_SUCCESS_COUNT.load(Ordering::Relaxed);
    let msg = format_xmp_jxl_images_line(xmp_ok, xmp_done, xmp_failed, jxl_ok, img_ok, img_fail);
    let line = format!("{}{}", pad_tag("[XMP]"), msg);
    write_to_log(&line);
    emit_stderr(&line);
}

fn format_xmp_jxl_images_line(
    xmp_ok: u64,
    xmp_done: bool,
    xmp_failed: u64,
    jxl_ok: u64,
    img_ok: u64,
    img_fail: u64,
) -> String {
    let xmp_part = if xmp_done {
        if xmp_failed > 0 {
            format!("XMP merge done: {} OK, {} failed", xmp_ok, xmp_failed)
        } else {
            format!("XMP merge done: {} OK", xmp_ok)
        }
    } else {
        format!("XMP merge: {} OK", xmp_ok)
    };
    let with_jxl = if jxl_ok > 0 {
        format!("{}   JXL: {} OK", xmp_part, jxl_ok)
    } else {
        xmp_part
    };
    if img_ok > 0 || img_fail > 0 {
        let img_part = if img_fail > 0 {
            format!("Images: {} OK, {} failed", img_ok, img_fail)
        } else {
            format!("Images: {} OK", img_ok)
        };
        format!("{}   {}", with_jxl, img_part)
    } else {
        with_jxl
    }
}

/// Call when an XMP sidecar is found and a merge is about to be attempted.
pub fn xmp_merge_attempt() {
    XMP_ATTEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call on successful merge. Prints a milestone line every N merges (no \r interleaving).
/// Same line shows XMP count, JXL count, and Images OK/failed when non-zero.
pub fn xmp_merge_success() {
    let success = XMP_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if xmp_milestone_interval(success) {
        let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
        let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
        emit_combined_status_line(img_ok, img_fail);
    }
}

/// Call on failed merge. Logs the error on its own line.
pub fn xmp_merge_failure(msg: &str) {
    let line = format!(
        "{}{}",
        pad_tag("[XMP]"),
        format!("⚠️  XMP merge failed: {}", msg)
    );
    write_to_log(&line);
    emit_stderr(&line);
}

/// Call after all processing is done to print the final summary.
/// Same line shows XMP summary, JXL count, and Images OK/failed when non-zero.
pub fn xmp_merge_finalize() {
    let total = XMP_ATTEMPT_COUNT.load(Ordering::Relaxed);
    let jxl_ok = JXL_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    if total > 0 {
        let success = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
        let failed = total.saturating_sub(success);
        let msg = format_xmp_jxl_images_line(success, true, failed, jxl_ok, img_ok, img_fail);
        let line = format!("{}{}", pad_tag("[XMP]"), msg);
        write_to_log(&line);
        emit_stderr(&line);
    } else if jxl_ok > 0 || img_ok > 0 || img_fail > 0 {
        let mut parts: Vec<String> = Vec::new();
        if jxl_ok > 0 {
            parts.push(format!("JXL: {} OK", jxl_ok));
        }
        if img_ok > 0 || img_fail > 0 {
            let img_part = if img_fail > 0 {
                format!("Images: {} OK, {} failed", img_ok, img_fail)
            } else {
                format!("Images: {} OK", img_ok)
            };
            parts.push(img_part);
        }
        let msg = parts.join("   ");
        let line = format!("{}{}", pad_tag("[XMP]"), msg);
        write_to_log(&line);
        emit_stderr(&line);
    }
}
