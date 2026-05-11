//! External Tools Detection Module
//!
//! Checks for required external tools (ffmpeg, cjxl, exiftool, etc.)
//! Provides helpful installation instructions when tools are missing.

use std::fmt::Write;
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

/// Minimum version requirements for external tools to ensure compatibility with modern features.
const MIN_VERSIONS: &[(&str, &str)] = &[
    ("ffmpeg", "6.1"),
    ("exiftool", "12.70"),
    ("magick", "7.1.1"),
    ("cjxl", "0.9.0"),
];

/// Ensure that the specified tools are available in the system PATH and meet version requirements.
///
/// # Errors
/// Returns an error message if any of the specified tools are missing or out of date.
pub fn require(tool_names: &[&str]) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut outdated = Vec::new();

    for name in tool_names {
        if !check_tool(name) && !check_tool_alt(name) {
            missing.push(*name);
            continue;
        }

        // Version locking: Check if the tool meets the minimum version requirement
        if let Some(&(target_name, min_ver)) = MIN_VERSIONS.iter().find(|(n, _)| n == name)
            && let Some(current_ver_full) = get_tool_version(target_name)
            && !is_version_at_least(&current_ver_full, min_ver)
        {
            outdated.push(format!(
                "{target_name} (found {current_ver_full}, required ≥{min_ver})"
            ));
        }
    }

    if !missing.is_empty() || !outdated.is_empty() {
        let mut err_msg = String::new();
        if !missing.is_empty() {
            let _ = write!(err_msg, "Required tools missing: {}. ", missing.join(", "));
        }
        if !outdated.is_empty() {
            let _ = write!(
                err_msg,
                "Tools out of date: {}. Please upgrade to the latest versions.",
                outdated.join(", ")
            );
        }
        return Err(err_msg);
    }

    Ok(())
}

/// Robust version comparison helper.
/// Extracts the first semantic version string (e.g. "7.1.1") and compares it.
fn is_version_at_least(current_full: &str, required: &str) -> bool {
    let extract_version = |s: &str| -> String {
        let mut result = String::new();
        let mut started = false;
        for c in s.chars() {
            if c.is_ascii_digit() {
                result.push(c);
                started = true;
            } else if started && c == '.' {
                result.push(c);
            } else if started {
                break;
            }
        }
        result
    };

    let current = extract_version(current_full);
    let current_parts: Vec<u32> = current.split('.').filter_map(|s| s.parse().ok()).collect();
    let required_parts: Vec<u32> = required.split('.').filter_map(|s| s.parse().ok()).collect();

    for (c, r) in current_parts.iter().zip(required_parts.iter()) {
        if c > r {
            return true;
        }
        if c < r {
            return false;
        }
    }
    current_parts.len() >= required_parts.len()
}

#[must_use]
pub fn is_available(name: &str) -> bool {
    check_tool(name) || check_tool_alt(name)
}
