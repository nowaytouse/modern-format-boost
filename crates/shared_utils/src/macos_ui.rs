use std::process::Command;

/* 
/// Shows a GUI confirmation dialog on macOS. (DISABLED)
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
*/

/// This replaces the previous GUI dialog and manual wait.
/// The process will now exit automatically after printing the completion message.
pub fn wait_for_exit_confirmation() {
    println!("\n   ✅ Task finished. Auto-exiting...");
}
