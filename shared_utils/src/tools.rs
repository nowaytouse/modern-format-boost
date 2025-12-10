//! External Tools Detection Module
//!
//! Checks for required external tools (ffmpeg, cjxl, exiftool, etc.)
//! Provides helpful installation instructions when tools are missing.

use std::process::Command;

/// Tool availability result
#[derive(Debug, Clone)]
pub struct ToolCheck {
    pub name: &'static str,
    pub available: bool,
    pub version: Option<String>,
    pub install_hint: &'static str,
}

/// Check if a tool is available in PATH
pub fn check_tool(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if a tool is available (alternative with -version flag)
pub fn check_tool_alt(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get tool version string
pub fn get_tool_version(name: &str) -> Option<String> {
    let output = Command::new(name)
        .arg("--version")
        .output()
        .or_else(|_| Command::new(name).arg("-version").output())
        .ok()?;
    
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Get first line of version output
        stdout.lines().next().map(|s| s.to_string())
    } else {
        None
    }
}

/// Check all required tools for image processing
pub fn check_image_tools() -> Vec<ToolCheck> {
    vec![
        ToolCheck {
            name: "cjxl",
            available: check_tool("cjxl"),
            version: get_tool_version("cjxl"),
            install_hint: "brew install jpeg-xl",
        },
        ToolCheck {
            name: "djxl",
            available: check_tool("djxl"),
            version: get_tool_version("djxl"),
            install_hint: "brew install jpeg-xl",
        },
        ToolCheck {
            name: "exiftool",
            available: check_tool_alt("exiftool"),
            version: get_tool_version("exiftool"),
            install_hint: "brew install exiftool",
        },
        ToolCheck {
            name: "ffmpeg",
            available: check_tool_alt("ffmpeg"),
            version: get_tool_version("ffmpeg"),
            install_hint: "brew install ffmpeg",
        },
        ToolCheck {
            name: "ffprobe",
            available: check_tool_alt("ffprobe"),
            version: get_tool_version("ffprobe"),
            install_hint: "brew install ffmpeg",
        },
    ]
}

/// Check all required tools for video processing
pub fn check_video_tools() -> Vec<ToolCheck> {
    vec![
        ToolCheck {
            name: "ffmpeg",
            available: check_tool_alt("ffmpeg"),
            version: get_tool_version("ffmpeg"),
            install_hint: "brew install ffmpeg",
        },
        ToolCheck {
            name: "ffprobe",
            available: check_tool_alt("ffprobe"),
            version: get_tool_version("ffprobe"),
            install_hint: "brew install ffmpeg",
        },
        ToolCheck {
            name: "exiftool",
            available: check_tool_alt("exiftool"),
            version: get_tool_version("exiftool"),
            install_hint: "brew install exiftool",
        },
    ]
}

/// Print tool availability report
pub fn print_tool_report(tools: &[ToolCheck]) {
    println!("🔧 External Tools Check");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    let mut all_available = true;
    for tool in tools {
        if tool.available {
            let version = tool.version.as_deref().unwrap_or("unknown version");
            println!("   ✅ {} - {}", tool.name, version);
        } else {
            println!("   ❌ {} - NOT FOUND", tool.name);
            println!("      💡 Install with: {}", tool.install_hint);
            all_available = false;
        }
    }
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    if all_available {
        println!("   ✅ All required tools are available!");
    } else {
        println!("   ⚠️  Some tools are missing. Please install them before proceeding.");
    }
}

/// Check required tools and exit if any are missing
pub fn require_tools(tool_names: &[&str]) -> Result<(), String> {
    let mut missing = Vec::new();
    
    for name in tool_names {
        if !check_tool(name) && !check_tool_alt(name) {
            missing.push(*name);
        }
    }
    
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "❌ Missing required tools: {}\n💡 Install with: brew install {}",
            missing.join(", "),
            missing.join(" ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_tool() {
        // These should exist on most systems
        assert!(check_tool("ls") || check_tool_alt("ls"));
    }
}
