//! macOS-specific UI enhancements (legacy AppleScript dialogs removed).
//!
//! Provides non-intrusive terminal interaction helpers.

/// This replaces the previous GUI dialog and manual wait.
/// The process will now exit automatically after printing the completion message.
pub fn wait_for_exit_confirmation() {
    println!("\n   ✅ Task finished. Auto-exiting...");
}
