//! Ctrl+C confirmation guard for long-running batch operations.
//!
//! After 4.5 minutes of processing, Ctrl+C shows a confirmation prompt instead of
//! immediately exiting. This prevents accidental termination of large batch jobs.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

static START_TIME: AtomicU64 = AtomicU64::new(0);
static CONFIRMATION_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Initialize the Ctrl+C guard. Call this at the start of batch processing.
pub fn init() {
    let start = Instant::now();
    START_TIME.store(start.elapsed().as_secs(), Ordering::Relaxed);

    let confirmation_active = Arc::new(AtomicBool::new(false));
    let confirmation_active_clone = confirmation_active.clone();

    ctrlc::set_handler(move || {
        // If confirmation is already active, ignore re-entrant signals
        if confirmation_active_clone.load(Ordering::Relaxed) {
            return;
        }

        let elapsed = Instant::now().duration_since(start).as_secs();

        // Under 4.5 minutes: exit immediately
        if elapsed < 270 {
            eprintln!("\n⚠️  Interrupted by user.");
            std::process::exit(130);
        }

        // 4.5+ minutes: ask for confirmation
        confirmation_active_clone.store(true, Ordering::Relaxed);
        CONFIRMATION_ACTIVE.store(true, Ordering::Relaxed);

        let elapsed_str = format_duration(elapsed);
        eprint!("\n⚠️  Ctrl+C detected after {} of processing.\n", elapsed_str);
        eprint!("   Confirm exit? [y/N] (auto-resume in 8s): ");
        let _ = io::stderr().flush();

        // Read with timeout using a separate thread
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_ok() {
                let _ = tx.send(input.trim().to_lowercase());
            }
        });

        // Wait up to 8 seconds for input
        match rx.recv_timeout(Duration::from_secs(8)) {
            Ok(answer) if answer == "y" => {
                eprintln!("\n⚠️  Interrupted by user after {}.", elapsed_str);
                std::process::exit(130);
            }
            _ => {
                // Timeout or any other input: resume
                eprintln!("\n▶  Resuming...\n");
            }
        }

        confirmation_active_clone.store(false, Ordering::Relaxed);
        CONFIRMATION_ACTIVE.store(false, Ordering::Relaxed);
    })
    .expect("Failed to set Ctrl+C handler");
}

fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;

    if h > 0 {
        format!("{:02}h{:02}m{:02}s", h, m, s)
    } else if m > 0 {
        format!("{:02}m{:02}s", m, s)
    } else {
        format!("{:02}s", s)
    }
}
