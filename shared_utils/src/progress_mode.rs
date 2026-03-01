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

// ── Per-thread log context (file name or ID) for concurrent processing ───────
// When set, every log_eprintln! / verbose_eprintln! line is prefixed so interleaved
// output from multiple files can be attributed correctly.

thread_local! {
    static LOG_PREFIX: RefCell<String> = RefCell::new(String::new());
}

const LOG_PREFIX_MAX_LEN: usize = 28;

/// Width of the tag column so all message bodies align (e.g. [file.jpeg], [Info]).
const LOG_TAG_WIDTH: usize = 24;

/// Log level tag for status/summary lines written to the run log (user-facing, not a severity).
const RUN_LOG_STATUS_TAG: &str = "[Info]";

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

/// Pad tag (e.g. `[file.jpeg]` or `[Info]`) to LOG_TAG_WIDTH for aligned message body.
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
    crate::logging::register_run_log_forwarder(Box::new(|s| write_to_log(s)));
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
    if has_log_file() {
        return Ok(());
    }
    let dir = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("logs");
    let _ = std::fs::create_dir_all(&dir);
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
        "{} Run log attached program=\"{}\" run_log=\"{}\" (all stderr and tracing written here)",
        pad_tag(RUN_LOG_STATUS_TAG),
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
    let h = elapsed_secs / 3600;
    let m = (elapsed_secs % 3600) / 60;
    let s = elapsed_secs % 60;
    let line = format!(
        "  Running: {:02}:{:02}:{:02}  {}/{}  {}",
        h, m, s, current, total, message
    );
    write_to_log_at_level(Level::DEBUG, &line);
}

/// Write a line to the log file (no-op if no log file is configured).
/// Does NOT write to stderr — use log_eprintln! or verbose_eprintln! for dual output.
/// Flushes after each write so log output is immediate (no loss on crash/kill).
pub fn write_to_log(line: &str) {
    if let Ok(mut guard) = LOG_FILE_WRITER.lock() {
        if let Some(ref mut w) = *guard {
            let _ = writeln!(w, "{}", line);
            let _ = w.flush();
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

/// Emit a line to stderr via tracing (and to run log when a log file is configured).
/// Run log always receives the line when configured so the file is complete and unfiltered.
/// Applies a uniform 2-space indent so multi-line blocks (e.g. precheck report) stay aligned.
/// When stderr is not a TTY (e.g. redirect/script), ANSI is stripped so output is plain text.
#[inline]
pub fn emit_stderr(line: &str) {
    if has_log_file() {
        write_to_log(line);
    }
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
                $crate::progress_mode::write_to_log_at_level(tracing::Level::DEBUG, &_line);
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
    let preprocess_ok = PREPROCESSING_COUNT.load(Ordering::Relaxed);
    let fallback_ok = FALLBACK_SUCCESS_COUNT.load(Ordering::Relaxed);
    let msg = format_xmp_jxl_images_line(xmp_ok, xmp_done, xmp_failed, jxl_ok, img_ok, img_fail, preprocess_ok, fallback_ok);
    let line = format!("{}{}", pad_tag(RUN_LOG_STATUS_TAG), msg);
    emit_stderr(&line);
}

fn format_xmp_jxl_images_line(
    xmp_ok: u64,
    xmp_done: bool,
    xmp_failed: u64,
    jxl_ok: u64,
    img_ok: u64,
    img_fail: u64,
    preprocess_ok: u64,
    fallback_ok: u64,
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
    let images_ok = img_ok + jxl_ok;
    let mut parts = vec![xmp_part];
    if images_ok > 0 || img_fail > 0 {
        let img_part = if img_fail > 0 {
            format!("Images: {} OK, {} failed", images_ok, img_fail)
        } else {
            format!("Images: {} OK", images_ok)
        };
        parts.push(img_part);
    }
    if preprocess_ok > 0 {
        parts.push(format!("Pre-processing: {} done", preprocess_ok));
    }
    if fallback_ok > 0 {
        parts.push(format!("Fallback: {} done", fallback_ok));
    }
    parts.join("   ")
}

/// Call when an XMP sidecar is found and a merge is about to be attempted.
pub fn xmp_merge_attempt() {
    XMP_ATTEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Call on successful merge. Prints a milestone line every N merges (no \r interleaving).
/// Same line shows XMP count and Images OK/failed (JXL merged into Images) when non-zero.
pub fn xmp_merge_success() {
    let success = XMP_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if xmp_milestone_interval(success) {
        let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
        let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
        emit_combined_status_line(img_ok, img_fail);
    }
}

/// Format a status line with the standard [Info] tag padding (for run log alignment).
pub fn format_status_line(msg: &str) -> String {
    format!("{}{}", pad_tag(RUN_LOG_STATUS_TAG), msg)
}

/// Call on failed merge. Logs the error on its own line.
pub fn xmp_merge_failure(msg: &str) {
    let line = format!("{}{}", pad_tag(RUN_LOG_STATUS_TAG), format!("⚠️  XMP merge failed: {}", msg));
    emit_stderr(&line);
}

/// Call after all processing is done to print the final summary.
/// Same line shows XMP summary, Images OK/failed, and Pre-processing count when non-zero.
pub fn xmp_merge_finalize() {
    let total = XMP_ATTEMPT_COUNT.load(Ordering::Relaxed);
    let jxl_ok = JXL_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_ok = IMAGE_SUCCESS_COUNT.load(Ordering::Relaxed);
    let img_fail = IMAGE_FAIL_COUNT.load(Ordering::Relaxed);
    let preprocess_ok = PREPROCESSING_COUNT.load(Ordering::Relaxed);
    if total > 0 {
        let success = XMP_SUCCESS_COUNT.load(Ordering::Relaxed);
        let failed = total.saturating_sub(success);
        let fallback_ok = FALLBACK_SUCCESS_COUNT.load(Ordering::Relaxed);
        let msg = format_xmp_jxl_images_line(success, true, failed, jxl_ok, img_ok, img_fail, preprocess_ok, fallback_ok);
        let line = format!("{}{}", pad_tag(RUN_LOG_STATUS_TAG), msg);
        emit_stderr(&line);
    } else {
        let fallback_ok = FALLBACK_SUCCESS_COUNT.load(Ordering::Relaxed);
        if jxl_ok > 0 || img_ok > 0 || img_fail > 0 || preprocess_ok > 0 || fallback_ok > 0 {
        let mut parts = Vec::new();
        let images_ok = img_ok + jxl_ok;
        if images_ok > 0 || img_fail > 0 {
            let img_part = if img_fail > 0 {
                format!("Images: {} OK, {} failed", images_ok, img_fail)
            } else {
                format!("Images: {} OK", images_ok)
            };
            parts.push(img_part);
        }
        if preprocess_ok > 0 {
            parts.push(format!("Pre-processing: {} done", preprocess_ok));
        }
        if fallback_ok > 0 {
            parts.push(format!("Fallback: {} done", fallback_ok));
        }
        let line = format!("{}{}", pad_tag(RUN_LOG_STATUS_TAG), parts.join("   "));
        emit_stderr(&line);
        }
    }
}
