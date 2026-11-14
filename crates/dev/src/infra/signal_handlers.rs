//! Signal handlers for graceful shutdown.
//! Mirrors `install_runtime_signal_handlers()` from drag_and_drop_processor.py.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

static ACTIVE_CHILD: AtomicBool = AtomicBool::new(false);
static SIGTERM_RECEIVED: AtomicBool = AtomicBool::new(false);

/// Check if a child process is currently active.
pub fn is_child_active() -> bool {
    ACTIVE_CHILD.load(Ordering::SeqCst)
}

/// Check if SIGTERM was received.
pub fn sigterm_received() -> bool {
    SIGTERM_RECEIVED.load(Ordering::SeqCst)
}

/// Set child process active state.
pub fn set_child_active(active: bool) {
    ACTIVE_CHILD.store(active, Ordering::SeqCst);
}

/// Install signal handlers for graceful shutdown.
/// Mirrors Python's SIGINT/SIGTERM handling.
pub fn install_signal_handlers() -> anyhow::Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
        if is_child_active() {
            eprintln!("Waiting for child process to complete...");
        } else {
            eprintln!("\nInterrupted");
            std::process::exit(130);
        }
    })?;

    #[cfg(unix)]
    unsafe {
        extern "C" fn handle_sigterm(_: libc::c_int) {
            SIGTERM_RECEIVED.store(true, Ordering::SeqCst);
            if is_child_active() {
                eprintln!("Waiting for child process to complete...");
            } else {
                std::process::exit(143);
            }
        }
        libc::signal(
            libc::SIGTERM,
            handle_sigterm as *const () as libc::sighandler_t,
        );
    }

    Ok(())
}
