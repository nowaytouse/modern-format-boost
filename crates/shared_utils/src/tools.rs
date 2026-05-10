//! External Tools Detection Module
//!
//! Checks for required external tools (ffmpeg, cjxl, exiftool, etc.)
//! Provides helpful installation instructions when tools are missing.

use std::process::Command;

#[derive(Debug, Clone)]
pub struct ToolCheck {
    pub name: &'static str,
    pub available: bool,
    pub version: Option<String>,
    pub install_hint: &'static str,
}

#[must_use]
pub fn check_tool(name: &str) -> bool {
    let path = crate::common_utils::resolve_tool_path(name)
        .unwrap_or_else(|| std::path::PathBuf::from(name));
    Command::new(&path)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[must_use]
pub fn check_tool_alt(name: &str) -> bool {
    let path = crate::common_utils::resolve_tool_path(name)
        .unwrap_or_else(|| std::path::PathBuf::from(name));
    Command::new(&path)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[must_use]
pub fn get_tool_version(name: &str) -> Option<String> {
    let path = crate::common_utils::resolve_tool_path(name)
        .unwrap_or_else(|| std::path::PathBuf::from(name));
    let output = Command::new(&path)
        .arg("--version")
        .output()
        .or_else(|_| Command::new(&path).arg("-version").output())
        .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.lines().next().map(std::string::ToString::to_string)
    } else {
        None
    }
}

#[must_use]
pub fn check_image() -> Vec<ToolCheck> {
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

#[must_use]
pub fn check_video() -> Vec<ToolCheck> {
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
            name: "vmaf",
            available: check_tool("vmaf"),
            version: get_tool_version("vmaf"),
            install_hint: "brew install vmaf",
        },
        ToolCheck {
            name: "dovi_tool",
            available: check_tool("dovi_tool"),
            version: get_tool_version("dovi_tool"),
            install_hint: "cargo install dovi_tool",
        },
    ]
}

/// Ensure that the specified tools are available in the system PATH.
///
/// # Errors
/// Returns an error message if any of the specified tools are missing.
pub fn require(tool_names: &[&str]) -> Result<(), String> {
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
            "The following required tools are missing: {}. Please install them to continue.",
            missing.join(", ")
        ))
    }
}

#[must_use]
pub fn is_available(name: &str) -> bool {
    check_tool(name) || check_tool_alt(name)
}
