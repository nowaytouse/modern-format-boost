//! macOS-specific UI enhancements using `AppleScript`.
//!
//! Provides GUI dialogs and alerts to improve the desktop experience
//! on macOS, such as exit confirmation windows.

use std::process::Command;

/// Shows a GUI confirmation dialog on macOS.
/// Returns true if the user clicked "OK", false if "Cancel" or if an error occurred.
#[must_use]
pub fn show_exit_confirmation() -> bool {
    #[cfg(target_os = "macos")]
    {
        // Use AppleScript to show a caution dialog with OK/Cancel buttons.
        let script = r#"
            tell application "System Events"
                activate
                set theResponse to button returned of (display dialog "⚠️ Modern Format Boost has finished its task.\n\nDo you want to close this session?" buttons {"❌ Keep Open", "✅ Exit Now"} default button "✅ Exit Now" with icon caution with title "Modern Format Boost")
                return theResponse
            end tell
        "#;

        let output = Command::new("osascript").arg("-e").arg(script).output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                stdout == "✅ Exit Now"
            }
            _ => false,
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Pauses execution and waits for a GUI confirmation on macOS,
/// or falls back to a terminal prompt on other platforms.
pub fn wait_for_exit_confirmation() {
    if cfg!(target_os = "macos") {
        if !show_exit_confirmation() {
            // If user wants to keep it open, we just print a message and do a terminal pause
            println!("\n   💡 Session kept open. Press Enter to close.");
            let mut unused = String::new();
            let _ = std::io::stdin().read_line(&mut unused);
        }
    } else {
        // Fallback for non-macOS: simple terminal pause
        println!("\n   💡 Task finished. Press Enter to exit.");
        let mut unused = String::new();
        let _ = std::io::stdin().read_line(&mut unused);
    }
}
