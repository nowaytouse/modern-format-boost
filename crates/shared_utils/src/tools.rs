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

    // Tool-specific version flags (some tools don't follow --version convention)
    let output = match name {
        "exiftool" => Command::new(&path).arg("-ver").output().ok(),
        _ => Command::new(&path)
            .arg("--version")
            .output()
            .or_else(|_| Command::new(&path).arg("-version").output())
            .ok(),
    };

    let output = output?;

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
    // If all compared parts are equal, current version is sufficient if:
    // - It has equal or more parts than required, OR
    // - All remaining required parts are 0 (treat missing parts as 0)
    let cur_len = current_parts.len();
    let req_len = required_parts.len();
    if cur_len >= req_len {
        return true;
    }
    // Current is shorter: check if remaining required parts are all 0
    required_parts.iter().skip(cur_len).all(|r| *r == 0)
}

#[must_use]
pub fn is_available(name: &str) -> bool {
    check_tool(name) || check_tool_alt(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_version_at_least_basic() {
        assert!(is_version_at_least("0.10.0", "0.9.0"));
        assert!(is_version_at_least("0.9.1", "0.9.0"));
        assert!(is_version_at_least("1.0.0", "0.9.0"));
        assert!(!is_version_at_least("0.8.0", "0.9.0"));
        assert!(!is_version_at_least("0.9.0", "0.10.0"));
    }

    #[test]
    fn test_is_version_at_least_unequal_parts() {
        // Shorter current version should be treated as having trailing zeros
        assert!(is_version_at_least("0.9", "0.9.0")); // BUG FIX: was false, should be true
        assert!(is_version_at_least("0.9", "0.9"));
        assert!(is_version_at_least("1.0", "1.0.0"));

        // Shorter current version with non-zero remaining required parts should fail
        assert!(!is_version_at_least("0.9", "0.9.1"));
        assert!(!is_version_at_least("0.9", "0.10.0"));

        // Longer current version should pass
        assert!(is_version_at_least("0.9.1", "0.9"));
        assert!(is_version_at_least("0.10.0", "0.9"));
    }

    #[test]
    fn test_is_version_at_least_from_tool_output() {
        // Real-world cjxl version output formats
        assert!(is_version_at_least("cjxl 0.10.0 8b1d1d7", "0.9.0"));
        assert!(is_version_at_least("cjxl 0.9.1 a1b2c3d", "0.9.0"));
        assert!(!is_version_at_least("cjxl 0.8.3 xxxxxxx", "0.9.0"));

        // ffmpeg version formats
        assert!(is_version_at_least("ffmpeg version 6.1.1", "6.1"));
        assert!(is_version_at_least("ffmpeg version 7.0", "6.1"));
        assert!(!is_version_at_least("ffmpeg version 5.0", "6.1"));

        // exiftool version formats (uses -ver flag, returns plain version like "13.55")
        assert!(is_version_at_least("13.55", "12.70"));
        assert!(is_version_at_least("12.70", "12.70"));
        assert!(!is_version_at_least("11.85", "12.70"));
    }

    /// Integration test: Actually invoke tools and verify version detection works
    /// This catches issues like exiftool's non-standard --version behavior
    #[test]
    fn test_get_tool_version_integration() {
        // Test exiftool - this was the original bug (returned "NAME" from --version)
        let exif_ver = get_tool_version("exiftool");
        assert!(exif_ver.is_some(), "exiftool version should be detected");
        let exif_ver = exif_ver.unwrap();
        assert!(
            !exif_ver.contains("NAME"),
            "exiftool version should not contain 'NAME' (wrong flag used?), got: {exif_ver}"
        );
        // Should look like a version number (starts with digits.digits)
        assert!(
            exif_ver.chars().next().unwrap().is_ascii_digit(),
            "exiftool version should start with digit, got: {exif_ver}"
        );

        // Test cjxl
        if let Some(ver) = get_tool_version("cjxl") {
            assert!(
                ver.contains(char::is_numeric),
                "cjxl version should contain numbers, got: {ver}"
            );
        }

        // Test ffmpeg
        if let Some(ver) = get_tool_version("ffmpeg") {
            assert!(
                ver.contains(char::is_numeric),
                "ffmpeg version should contain numbers, got: {ver}"
            );
        }
    }
}
