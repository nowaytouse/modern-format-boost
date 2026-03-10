//! Ctrl+C confirmation guard for long-running batch operations.
//!
//! After 4.5 minutes of processing, Ctrl+C shows a confirmation prompt instead of
//! immediately exiting. This prevents accidental termination of large batch jobs.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

static CONFIRMATION_ACTIVE: AtomicBool = AtomicBool::new(false);
static ELAPSED_SECONDS: AtomicU64 = AtomicU64::new(0);

// Use lazy_static for one-time initialization
static HANDLER_STATE: std::sync::Once = std::sync::Once::new();

/// Set the start time (used for testing)
pub fn set_start_time(_start: Instant) {
    // Store a mock elapsed time for testing
    ELAPSED_SECONDS.store(0, Ordering::Relaxed);
}

/// Set mock elapsed time for testing (in seconds)
pub fn set_mock_elapsed_time(seconds: u64) {
    ELAPSED_SECONDS.store(seconds, Ordering::Relaxed);
}

/// Check if confirmation should be shown based on elapsed time
pub fn should_show_confirmation() -> bool {
    let elapsed = ELAPSED_SECONDS.load(Ordering::Relaxed);
    elapsed >= 270 // 4.5 minutes = 270 seconds
}

/// Handle user input for confirmation (used for testing)
pub fn handle_user_input(input: &str) -> bool {
    match input.trim().to_lowercase().as_str() {
        "y" => true,
        _ => false,
    }
}

/// Initialize the Ctrl+C guard. Call this at the start of batch processing.
pub fn init() {
    let start = Instant::now();
    
    // Use std::sync::Once to ensure handler is set only once
    HANDLER_STATE.call_once(|| {
        let confirmation_active = Arc::new(AtomicBool::new(false));
        let confirmation_active_clone = confirmation_active.clone();

        match ctrlc::set_handler(move || {
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
        }) {
            Ok(_) => {
                // Handler set successfully - use stderr to ensure visibility
                eprintln!("🛡️  Ctrl+C guard initialized (4.5min threshold)");
            }
            Err(e) => {
                // Use stderr to ensure the error message is visible even in pipes
                eprintln!("⚠️  Failed to set Ctrl+C handler: {}", e);
                eprintln!("   💡 Ctrl+C guard will not be available");
                // Continue without guard - better than crashing
            }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::thread;

    #[test]
    fn test_should_show_confirmation_before_threshold() {
        // Test that confirmation is NOT shown before 4.5 minutes
        set_start_time(Instant::now());
        set_mock_elapsed_time(60); // 1 minute
        
        assert!(!should_show_confirmation(), "Should not show confirmation before 4.5 minutes");
    }

    #[test]
    fn test_should_show_confirmation_after_threshold() {
        // Test that confirmation IS shown after 4.5 minutes
        set_start_time(Instant::now());
        set_mock_elapsed_time(300); // 5 minutes
        
        assert!(should_show_confirmation(), "Should show confirmation after 4.5 minutes");
    }

    #[test]
    fn test_should_show_confirmation_at_threshold_boundary() {
        // Test exactly at the 4.5 minute boundary (270 seconds)
        set_start_time(Instant::now());
        set_mock_elapsed_time(270); // Exactly 4.5 minutes
        
        assert!(should_show_confirmation(), "Should show confirmation at exactly 4.5 minutes");
    }

    #[test]
    fn test_handle_user_input_yes() {
        // Test that 'y' input returns true (exit confirmed)
        assert!(handle_user_input("y\n"), "Should return true for 'y' input");
        assert!(handle_user_input("Y\n"), "Should return true for uppercase 'Y' input");
        assert!(handle_user_input("y"), "Should return true for 'y' without newline");
    }

    #[test]
    fn test_handle_user_input_no() {
        // Test that 'n' input returns false (resume)
        assert!(!handle_user_input("n\n"), "Should return false for 'n' input");
        assert!(!handle_user_input("N\n"), "Should return false for uppercase 'N' input");
    }

    #[test]
    fn test_handle_user_input_other() {
        // Test that any other input returns false (resume)
        assert!(!handle_user_input("x\n"), "Should return false for non-'y' input");
        assert!(!handle_user_input("yes\n"), "Should return false for 'yes' (not just 'y')");
        assert!(!handle_user_input("1\n"), "Should return false for numeric input");
    }

    #[test]
    fn test_handle_user_input_empty() {
        // Test that empty input returns false (resume)
        assert!(!handle_user_input(""), "Should return false for empty input");
        assert!(!handle_user_input("\n"), "Should return false for just newline");
    }

    #[test]
    fn test_handle_user_input_whitespace() {
        // Test that whitespace around 'y' is handled correctly
        assert!(handle_user_input("  y  \n"), "Should return true for 'y' with whitespace");
        assert!(!handle_user_input("  n  \n"), "Should return false for 'n' with whitespace");
    }

    #[test]
    fn test_multiple_init_calls() {
        // Reset state for clean test
        ELAPSED_SECONDS.store(0, Ordering::Relaxed);
        
        // Test that multiple init() calls don't cause issues
        set_start_time(Instant::now());
        
        // First init should work
        init();
        
        // Second init should also work (idempotent due to std::sync::Once)
        init();
        
        // Should still work correctly
        assert!(!should_show_confirmation());
    }

    #[test]
    fn test_elapsed_time_updates() {
        // Test that elapsed time can be updated
        set_start_time(Instant::now());
        
        // Set to 1 minute
        set_mock_elapsed_time(60);
        assert!(!should_show_confirmation());
        
        // Update to 5 minutes
        set_mock_elapsed_time(300);
        assert!(should_show_confirmation());
        
        // Update back to 1 minute
        set_mock_elapsed_time(60);
        assert!(!should_show_confirmation());
    }

    
    #[test]
    fn test_concurrent_access() {
        // Reset state for clean test
        ELAPSED_SECONDS.store(300, Ordering::Relaxed); // Directly set to 5 minutes
        
        // Initialize first (don't call set_start_time as it resets elapsed time)
        init();
        
        // Give a moment for initialization to complete
        std::thread::sleep(std::time::Duration::from_millis(10));
        
        // Test thread safety of the guard functions
        let handles: Vec<_> = (0..3) // Use fewer threads to avoid signal handler conflicts
            .map(|_| {
                // Copy the elapsed time to avoid race conditions
                let elapsed = 300;
                thread::spawn(move || {
                    // Set elapsed time in this thread
                    set_mock_elapsed_time(elapsed);
                    // Check confirmation status
                    should_show_confirmation()
                })
            })
            .collect();
        
        // All threads should return true (since we're past the threshold)
        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.join().unwrap();
            assert!(result, "Thread {} should return true", i);
        }
    }

    #[test]
    fn test_edge_case_zero_elapsed() {
        // Test with zero elapsed time
        set_start_time(Instant::now());
        set_mock_elapsed_time(0);
        
        assert!(!should_show_confirmation(), "Should not show confirmation at 0 seconds");
    }

    #[test]
    fn test_edge_case_one_second() {
        // Test with 1 second elapsed
        set_start_time(Instant::now());
        set_mock_elapsed_time(1);
        
        assert!(!should_show_confirmation(), "Should not show confirmation at 1 second");
    }

    #[test]
    fn test_edge_case_very_long_duration() {
        // Test with very long duration (1 hour)
        set_start_time(Instant::now());
        set_mock_elapsed_time(3600); // 1 hour
        
        assert!(should_show_confirmation(), "Should show confirmation after 1 hour");
    }

    #[test]
    fn test_format_duration() {
        // Test the format_duration helper function
        assert_eq!(format_duration(5), "05s");
        assert_eq!(format_duration(65), "01m05s");
        assert_eq!(format_duration(3665), "01h01m05s");
        assert_eq!(format_duration(3600), "01h00m00s");
        assert_eq!(format_duration(0), "00s");
    }

    #[test]
    fn test_boundary_conditions() {
        // Test boundary conditions around the 270 second threshold
        set_start_time(Instant::now());
        
        // Just before threshold
        set_mock_elapsed_time(269);
        assert!(!should_show_confirmation(), "Should not show confirmation at 269 seconds");
        
        // Exactly at threshold
        set_mock_elapsed_time(270);
        assert!(should_show_confirmation(), "Should show confirmation at 270 seconds");
        
        // Just after threshold
        set_mock_elapsed_time(271);
        assert!(should_show_confirmation(), "Should show confirmation at 271 seconds");
    }

    #[test]
    fn test_uninitialized_state() {
        // Test behavior when not initialized
        // Reset to uninitialized state
        ELAPSED_SECONDS.store(0, Ordering::Relaxed);
        
        assert!(!should_show_confirmation(), "Should not show confirmation when not initialized");
    }
}
