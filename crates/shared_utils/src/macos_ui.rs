//! macOS-specific UI enhancements (legacy AppleScript dialogs removed).
//!
//! Provides non-intrusive terminal interaction helpers.

/// Pauses execution and waits for user to press Enter before exiting.
/// This replaces the previous GUI dialog which was considered intrusive.
pub fn wait_for_exit_confirmation() {
    println!("\n   💡 Task finished. Press Enter to exit.");
    let mut unused = String::new();
    let _ = std::io::stdin().read_line(&mut unused);
}
