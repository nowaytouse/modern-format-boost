//! Ctrl+C confirmation guard for long-running batch operations.
//!
//! After 10 seconds of processing, Ctrl+C shows a confirmation prompt instead
//! of immediately exiting. This prevents accidental termination of batch jobs.
//!
//! # Design
//! - Signal handler is minimal: only sets an atomic flag and wakes a watcher
//!   thread
//! - A dedicated watcher thread owns all blocking I/O and the timeout logic
//! - No stdin read, no heap allocation, no mutex in the signal handler
//! - Re-entrant signals during the prompt window are ignored gracefully
//! - `SIGTERM` is treated identically to `SIGINT` for clean shutdown

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Global lock for all terminal UI output to prevent interleaving and
/// interference.
pub static TERMINAL_LOCK: Mutex<()> = Mutex::new(());

// ─── Shared state ────────────────────────────────────────────────────────────

/// Set to true when a Ctrl+C signal has been received.
static SIGNAL_RECEIVED: AtomicBool = AtomicBool::new(false);

/// Set to true while the confirmation prompt is being shown (re-entrant guard).
static PROMPT_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set to true after `init()` has been called, so double-init is harmless.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// Thin wrapper so we can lazily encode a real Instant via OnceLock.
static START_INSTANT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

// ─── Public API ──────────────────────────────────────────────────────────────

/// Returns true if the Ctrl+C confirmation prompt is currently active.
pub fn is_prompt_active() -> bool {
    PROMPT_ACTIVE.load(Ordering::Acquire)
}

/// Blocks the current thread while the Ctrl+C confirmation prompt is active.
/// Call this in tight loops or before emitting logs to pause execution.
pub fn wait_if_prompt_active() {
    if is_prompt_active() {
        while PROMPT_ACTIVE.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(
                crate::constants::CTRLC_WATCHER_SLEEP_MS,
            ));
        }
        // Small delay after prompt dismissal to ensure logs format cleanly
        std::thread::sleep(Duration::from_millis(
            crate::constants::CTRLC_WATCHER_RESUME_SLEEP_MS,
        ));
    }
}

/// Initialize the Ctrl+C guard. Safe to call multiple times (idempotent).
///
/// Spawns a background daemon thread that watches for Ctrl+C signals and
/// presents a confirmation prompt after 10 seconds. The thread exits when
/// the process exits (it is daemonized via `thread::Builder::spawn`).
pub fn init() {
    // Idempotent: only install once.
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    let start = Instant::now();
    let _ = START_INSTANT.set(start);

    // Install a minimal signal handler — only sets the atomic flag.
    // All blocking work happens in the watcher thread below.
    let signal_received = Arc::new(AtomicBool::new(false));
    let signal_received_clone = Arc::clone(&signal_received);

    let handler_result = ctrlc::set_handler(move || {
        // Re-entrant guard: if the prompt is already showing, a second Ctrl+C
        // means the user REALLY wants to exit now.
        if PROMPT_ACTIVE.load(Ordering::Acquire) {
            std::process::exit(crate::constants::EXIT_CODE_SIGINT);
        }
        // Set the shared flag and the global flag.
        signal_received_clone.store(true, Ordering::Release);
        SIGNAL_RECEIVED.store(true, Ordering::Release);
    });

    if let Err(e) = handler_result {
        // Best-effort: if we cannot install the handler (e.g. another crate
        // already did), log a warning but continue — the program still works,
        // Ctrl+C will just exit immediately via the OS default.
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_runtime",
            format!("ctrlc_guard: could not install Ctrl+C handler: {e}"),
        );
        return;
    }

    // Spawn the watcher thread (daemonized so it doesn't block process exit).
    if let Err(err) = std::thread::Builder::new()
        .name("ctrlc-watcher".into())
        .spawn(move || watcher_thread(&signal_received))
    {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_runtime",
            format!("ctrlc_guard: failed to spawn Ctrl+C watcher thread: {err}"),
        );
    }
}

// ─── Watcher thread ──────────────────────────────────────────────────────────

fn watcher_thread(signal_flag: &Arc<AtomicBool>) {
    loop {
        // Poll at intervals — very cheap, avoids condvar complexity.
        std::thread::sleep(Duration::from_millis(
            crate::constants::CTRLC_WATCHER_POLL_MS,
        ));

        if !signal_flag.swap(false, Ordering::AcqRel) {
            continue; // No signal yet.
        }

        let elapsed_secs = crate::media_conversion_gate::runtime_elapsed_secs_or_zero(
            START_INSTANT.get().map(std::time::Instant::elapsed),
            "ctrlc_guard watcher",
        );

        if elapsed_secs < crate::constants::CTRLC_CONFIRM_THRESHOLD_SECS {
            // Under threshold → exit immediately (user made a deliberate Ctrl+C).
            crate::ui_stderr::line(
                crate::modern_ui::symbols::WARNING,
                crate::modern_ui::symbols::plain::WARNING,
                "\n  Interrupted by user.",
            );
            std::process::exit(crate::constants::EXIT_CODE_SIGINT);
        }

        // 10 seconds+: show confirmation prompt.
        show_confirmation_prompt(elapsed_secs);

        // Clear the flag again in case multiple signals arrived while prompting.
        signal_flag.store(false, Ordering::Release);
        SIGNAL_RECEIVED.store(false, Ordering::Release);
    }
}

// ─── Confirmation prompt
// ──────────────────────────────────────────────────────

fn show_confirmation_prompt(elapsed_secs: u64) {
    // Enforcement: set PROMPT_ACTIVE *before* any lock attempts.
    // This ensures concurrent threads (e.g. main thread loggers) see the flag and
    // pause/sleep, allowing the watcher thread to acquire TERMINAL_LOCK without
    // starvation.
    if PROMPT_ACTIVE.swap(true, Ordering::AcqRel) {
        return;
    }

    let elapsed_str = format_duration(elapsed_secs);
    let mut should_exit = false;

    // ─── Phase 1: Print the prompt ───
    let mut prompt_warnings = Vec::new();
    {
        let _guard = crate::media_conversion_gate::mutex_guard_or_recover(
            "ctrlc_terminal_lock_prompt",
            TERMINAL_LOCK.lock(),
        );
        let mut out = io::stderr().lock();
        if let Err(err) = write!(out, "\x1b[?25h") {
            prompt_warnings.push(format!(
                "[WARN] Could not show cursor for ctrl-c prompt: {err}"
            ));
        }
        if let Err(err) = writeln!(out) {
            prompt_warnings.push(format!(
                "[WARN] Could not write ctrl-c prompt newline: {err}"
            ));
        }
        if let Err(err) = writeln!(
            out,
            "  \x1b[1;33m⚠️  Ctrl+C detected\x1b[0m after \x1b[1m{elapsed_str}\x1b[0m of \
             processing."
        ) {
            prompt_warnings.push(format!(
                "[WARN] Could not write ctrl-c prompt header: {err}"
            ));
        }
        if let Err(err) = writeln!(
            out,
            "  \x1b[2mPress Enter to exit, or wait 10 s to resume automatically.\x1b[0m"
        ) {
            prompt_warnings.push(format!(
                "[WARN] Could not write ctrl-c prompt instruction: {err}"
            ));
        }
        if let Err(err) = write!(
            out,
            "  \x1b[1mConfirm exit? [y/N]\x1b[0m (auto-resume in 10 s): "
        ) {
            prompt_warnings.push(format!(
                "[WARN] Could not write ctrl-c prompt confirmation: {err}"
            ));
        }
        if let Err(err) = out.flush() {
            prompt_warnings.push(format!(
                "[WARN] Could not flush ctrl-c confirmation prompt: {err}"
            ));
        }
    }
    for warning in prompt_warnings {
        eprintln!("{warning}");
    }

    // ─── Phase 2: Read from stdin with a 10-second timeout ───
    #[cfg(unix)]
    {
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = match i32::try_from(crate::constants::CTRLC_PROMPT_TIMEOUT_MS) {
            Ok(value) => value,
            Err(err) => {
                crate::media_conversion_gate::delivery_numeric_fallback_audit(
                    "ctrlc_timeout",
                    format!(
                        "[FALLBACK] CTRLC_PROMPT_TIMEOUT_MS exceeds i32 ({err}); using 10000ms \
                         poll timeout"
                    ),
                );
                10_000
            }
        };
        // SAFETY: poll() is called with a single pollfd and a valid timeout.
        let res = unsafe { libc::poll(&raw mut pfd, 1, timeout_ms) };
        if res > 0 && (pfd.revents & libc::POLLIN) != 0 {
            let mut line = String::new();
            match io::stdin().read_line(&mut line) {
                Ok(_) => {
                    let answer = line.trim().to_ascii_lowercase();
                    should_exit = matches!(answer.as_str(), "y" | "yes");
                }
                Err(err) => {
                    eprintln!("[WARN] Could not read ctrl-c confirmation prompt: {err}");
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(_) => {
                let answer = line.trim().to_ascii_lowercase();
                should_exit = matches!(answer.as_str(), "y" | "yes");
            }
            Err(err) => {
                eprintln!("[WARN] Could not read ctrl-c confirmation prompt: {err}");
            }
        }
    }

    // ─── Phase 3: Print result ───
    let mut result_warnings = Vec::new();
    {
        let _guard = crate::media_conversion_gate::mutex_guard_or_recover(
            "ctrlc_terminal_lock_result",
            TERMINAL_LOCK.lock(),
        );
        let mut out = io::stderr().lock();
        if let Err(err) = writeln!(out) {
            result_warnings.push(format!(
                "[WARN] Could not write ctrl-c result newline: {err}"
            ));
        }
        if should_exit {
            if let Err(err) = writeln!(
                out,
                "  \x1b[1;31m⚠️  Interrupted by user after {elapsed_str}.\x1b[0m"
            ) {
                result_warnings.push(format!(
                    "[WARN] Could not write ctrl-c interrupted result: {err}"
                ));
            }
        } else {
            if let Err(err) = writeln!(out, "  \x1b[1;32m▶  Resuming…\x1b[0m") {
                result_warnings.push(format!(
                    "[WARN] Could not write ctrl-c resume result: {err}"
                ));
            }
            if let Err(err) = writeln!(out) {
                result_warnings.push(format!(
                    "[WARN] Could not write ctrl-c resume newline: {err}"
                ));
            }
            if let Err(err) = write!(out, "\x1b[?25l") {
                result_warnings.push(format!(
                    "[WARN] Could not hide cursor after ctrl-c prompt: {err}"
                ));
            }
        }
        if let Err(err) = out.flush() {
            result_warnings.push(format!(
                "[WARN] Could not flush ctrl-c result prompt: {err}"
            ));
        }
    }
    for warning in result_warnings {
        eprintln!("{warning}");
    }

    PROMPT_ACTIVE.store(false, Ordering::Release);

    if should_exit {
        std::process::exit(crate::constants::EXIT_CODE_SIGINT);
    }
}

// ─── Helpers
// ──────────────────────────────────────────────────────────────────

fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;

    if h > 0 {
        format!("{h:02}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m:02}m {s:02}s")
    } else {
        format!("{s:02}s")
    }
}
