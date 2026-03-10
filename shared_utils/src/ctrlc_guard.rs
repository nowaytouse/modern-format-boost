//! Ctrl+C confirmation guard for long-running batch operations.
//!
//! After 4.5 minutes of processing, Ctrl+C shows a confirmation prompt instead of
//! immediately exiting. This prevents accidental termination of large batch jobs.
//!
//! # Design
//! - Signal handler is minimal: only sets an atomic flag and wakes a watcher thread
//! - A dedicated watcher thread owns all blocking I/O and the timeout logic
//! - No stdin read, no heap allocation, no mutex in the signal handler
//! - Re-entrant signals during the prompt window are ignored gracefully
//! - `SIGTERM` is treated identically to `SIGINT` for clean shutdown

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ─── Shared state ────────────────────────────────────────────────────────────

/// Set to true when a Ctrl+C signal has been received.
static SIGNAL_RECEIVED: AtomicBool = AtomicBool::new(false);

/// Set to true while the confirmation prompt is being shown (re-entrant guard).
static PROMPT_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set to true after `init()` has been called, so double-init is harmless.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Start-of-batch wall-clock epoch (seconds since boot, via Instant).
/// We store the *value* of the Instant as elapsed nanos relative to an
/// internal epoch — using a u64 allows Ordering::Relaxed atomic access.
static START_EPOCH_NANOS: AtomicU64 = AtomicU64::new(0);

// Thin wrapper so we can lazily encode a real Instant via OnceLock.
static START_INSTANT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

// ─── Public API ──────────────────────────────────────────────────────────────

/// Initialize the Ctrl+C guard. Safe to call multiple times (idempotent).
///
/// Spawns a background daemon thread that watches for Ctrl+C signals and
/// presents a confirmation prompt after 4.5 minutes. The thread exits when
/// the process exits (it is daemonized via `thread::Builder::spawn`).
pub fn init() {
    // Idempotent: only install once.
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    let start = Instant::now();
    let _ = START_INSTANT.set(start);
    START_EPOCH_NANOS.store(0, Ordering::Relaxed); // relative to START_INSTANT

    // Install a minimal signal handler — only sets the atomic flag.
    // All blocking work happens in the watcher thread below.
    let signal_received = Arc::new(AtomicBool::new(false));
    let signal_received_clone = Arc::clone(&signal_received);

    let handler_result = ctrlc::set_handler(move || {
        // Re-entrant guard: ignore extra signals while the prompt is showing.
        if PROMPT_ACTIVE.load(Ordering::Acquire) {
            return;
        }
        // Set the shared flag and the global flag.
        signal_received_clone.store(true, Ordering::Release);
        SIGNAL_RECEIVED.store(true, Ordering::Release);
    });

    if let Err(e) = handler_result {
        // Best-effort: if we cannot install the handler (e.g. another crate
        // already did), log a warning but continue — the program still works,
        // Ctrl+C will just exit immediately via the OS default.
        eprintln!("  ⚠️  ctrlc_guard: could not install Ctrl+C handler: {e}");
        return;
    }

    // Spawn the watcher thread (daemonized so it doesn't block process exit).
    let _ = std::thread::Builder::new()
        .name("ctrlc-watcher".into())
        .spawn(move || watcher_thread(signal_received));
}

// ─── Watcher thread ──────────────────────────────────────────────────────────

fn watcher_thread(signal_flag: Arc<AtomicBool>) {
    loop {
        // Poll at 100 ms intervals — very cheap, avoids condvar complexity.
        std::thread::sleep(Duration::from_millis(100));

        if !signal_flag.swap(false, Ordering::AcqRel) {
            continue; // No signal yet.
        }

        let elapsed_secs = START_INSTANT
            .get()
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        if elapsed_secs < 270 {
            // Under 4.5 minutes → exit immediately (user made a deliberate Ctrl+C).
            eprintln!("\n  ⚠️  Interrupted by user.");
            std::process::exit(130);
        }

        // 4.5 minutes+: show confirmation prompt.
        show_confirmation_prompt(elapsed_secs);

        // Clear the flag again in case multiple signals arrived while prompting.
        signal_flag.store(false, Ordering::Release);
        SIGNAL_RECEIVED.store(false, Ordering::Release);
    }
}

// ─── Confirmation prompt ──────────────────────────────────────────────────────

fn show_confirmation_prompt(elapsed_secs: u64) {
    // Guard: only one prompt at a time.
    if PROMPT_ACTIVE.swap(true, Ordering::AcqRel) {
        return;
    }

    let elapsed_str = format_duration(elapsed_secs);
    let stderr = io::stderr();

    {
        let mut out = stderr.lock();
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  \x1b[1;33m⚠️  Ctrl+C detected\x1b[0m after \x1b[1m{elapsed_str}\x1b[0m of processing."
        );
        let _ = writeln!(
            out,
            "  \x1b[2mPress Enter to exit, or wait 10 s to resume automatically.\x1b[0m"
        );
        let _ = write!(out, "  \x1b[1mConfirm exit? [y/N]\x1b[0m (auto-resume in 10 s): ");
        let _ = out.flush();
    }

    // Read from stdin with a 10-second timeout using a background thread + channel.
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    let _reader = std::thread::Builder::new()
        .name("ctrlc-stdin-reader".into())
        .spawn(move || {
            let mut line = String::new();
            if io::stdin().read_line(&mut line).is_ok() {
                let _ = tx.send(line.trim().to_ascii_lowercase());
            }
            // If the channel send fails, the receiver already timed out — that's fine.
        });

    let should_exit = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(answer) => matches!(answer.as_str(), "y" | "yes"),
        // Timeout or stdin closed → resume.
        Err(_) => false,
    };

    {
        let mut out = io::stderr().lock();
        let _ = writeln!(out);
        if should_exit {
            let _ = writeln!(
                out,
                "  \x1b[1;31m⚠️  Interrupted by user after {elapsed_str}.\x1b[0m"
            );
        } else {
            let _ = writeln!(out, "  \x1b[1;32m▶  Resuming…\x1b[0m");
            let _ = writeln!(out);
        }
        let _ = out.flush();
    }

    PROMPT_ACTIVE.store(false, Ordering::Release);

    if should_exit {
        std::process::exit(130);
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

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
