//! External Tools Detection Module
//!
//! Checks for required external tools (ffmpeg, cjxl, exiftool, etc.)
//! Provides helpful installation instructions when tools are missing.

use std::fmt::Write;
use std::process::{Command, Output};

const FFMPEG_MIN_RELEASE: &str = "6.1";
// Minimum library versions declared by the upstream FFmpeg n6.1 headers.
const FFMPEG_MIN_LIBRARY_VERSIONS: &[(&str, [u32; 3])] = &[
    ("libavutil", [58, 29, 100]),
    ("libavcodec", [60, 31, 102]),
    ("libavformat", [60, 16, 100]),
];

#[derive(Debug, Clone)]
pub struct ToolCheck {
    pub name: &'static str,
    pub available: bool,
    pub version: Option<String>,
    pub install_hint: &'static str,
}

fn tool_output_success(tool: &str, probe: &str, output: std::io::Result<Output>) -> bool {
    match output {
        Ok(output) => output.status.success(),
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_probe",
                format!("tool availability probe failed for {tool} via {probe}: {e}"),
            );
            false
        }
    }
}

#[must_use]
pub fn check_tool(name: &str) -> bool {
    let path = crate::common_utils::resolve_tool_path_or_audit(name);
    let probe_arg = if name == crate::constants::TOOL_JXLINFO {
        "--help"
    } else {
        "--version"
    };
    tool_output_success(name, probe_arg, Command::new(&path).arg(probe_arg).output())
}

#[must_use]
pub fn check_tool_alt(name: &str) -> bool {
    let path = crate::common_utils::resolve_tool_path_or_audit(name);
    tool_output_success(
        name,
        "-version",
        Command::new(&path).arg("-version").output(),
    )
}

fn version_probe_arg(name: &str) -> &'static str {
    match name {
        "exiftool" => "-ver",
        "ffmpeg" | "ffprobe" => "-version",
        _ => "--version",
    }
}

fn get_tool_version_output(name: &str) -> Option<Output> {
    let path = crate::common_utils::resolve_tool_path_or_audit(name);
    let probe_arg = version_probe_arg(name);
    let result = match name {
        "exiftool" | "ffmpeg" | "ffprobe" => Command::new(&path).arg(probe_arg).output(),
        _ => Command::new(&path)
            .arg(probe_arg)
            .output()
            .or_else(|_| Command::new(&path).arg("-version").output()),
    };

    match result {
        Ok(output) => Some(output),
        Err(error) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_version",
                format!("tool version probe failed for {name} via {probe_arg}: {error}"),
            );
            None
        }
    }
}

#[must_use]
pub fn get_tool_version(name: &str) -> Option<String> {
    let output = get_tool_version_output(name)?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        first_version_line(&stdout)
    } else {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "tool_version",
            format!(
                "tool version probe returned non-zero status for {name}: {stderr}",
                stderr = String::from_utf8_lossy(&output.stderr).trim()
            ),
        );
        None
    }
}

fn first_version_line(output: &str) -> Option<String> {
    output.lines().next().map(std::string::ToString::to_string)
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
            name: "jxlinfo",
            available: check_tool("jxlinfo"),
            version: get_tool_version("cjxl"),
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

/// Minimum version requirements for external tools to ensure compatibility with
/// modern features.
const MIN_VERSIONS: &[(&str, &str)] = &[
    ("ffmpeg", FFMPEG_MIN_RELEASE),
    ("exiftool", "12.70"),
    ("magick", "7.1.1"),
    ("cjxl", "0.9.0"),
];

/// Ensure that the specified tools are available in the system PATH and meet
/// version requirements.
///
/// # Errors
/// Returns an error message if any of the specified tools are missing or out of
/// date.
pub fn require(tool_names: &[&str]) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut outdated = Vec::new();

    for name in tool_names {
        if !check_tool(name) && !check_tool_alt(name) {
            missing.push(*name);
            continue;
        }

        // Version locking: Check if the tool meets the minimum version requirement
        if let Some(&(target_name, min_ver)) = MIN_VERSIONS.iter().find(|(n, _)| n == name) {
            match get_tool_version_output(target_name) {
                Some(output) if output.status.success() => {
                    let full_output = String::from_utf8_lossy(&output.stdout);
                    let current_ver = full_output
                        .lines()
                        .next()
                        .map(std::string::ToString::to_string);
                    match current_ver {
                        Some(ref version)
                            if version_meets_minimum(
                                target_name,
                                version,
                                &full_output,
                                min_ver,
                            ) => {}
                        Some(version) => outdated.push(format!(
                            "{target_name} (found {version}, required ≥{min_ver})"
                        )),
                        None => {
                            outdated.push(format!("{target_name} (version output was empty)"));
                        }
                    }
                }
                Some(output) => {
                    crate::media_conversion_gate::delivery_runtime_batch_audit(
                        "tool_version",
                        format!(
                            "tool version probe returned non-zero status for {target_name}: {stderr}",
                            stderr = String::from_utf8_lossy(&output.stderr).trim()
                        ),
                    );
                    outdated.push(format!("{target_name} (version probe failed)"));
                }
                None => {
                    outdated.push(format!("{target_name} (version probe failed)"));
                }
            }
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
                "Tool version requirements not verified: {}. Upgrade outdated tools or repair the reported version-probe evidence.",
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
    if current_full.starts_with("ffmpeg version N-") {
        return false;
    }

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
                // FFmpeg snapshots use a dotted qualifier, e.g. 8.0.git.
                // Strip its separator, not a malformed trailing version dot.
                if c.is_ascii_alphabetic() && result.ends_with('.') {
                    result.truncate(result.len() - 1);
                }
                break;
            }
        }
        result
    };

    let current = extract_version(current_full);
    let Some(current_parts) = parse_version_parts(&current, "current") else {
        return false;
    };
    let Some(required_parts) = parse_version_parts(required, "required") else {
        return false;
    };

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

fn version_meets_minimum(tool: &str, first_line: &str, full_output: &str, minimum: &str) -> bool {
    if tool == "ffmpeg" && first_line.starts_with("ffmpeg version N-") {
        if minimum != FFMPEG_MIN_RELEASE {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_version",
                format!("no verified FFmpeg snapshot API baseline for minimum {minimum}"),
            );
            return false;
        }
        return ffmpeg_snapshot_meets_minimum(full_output);
    }
    is_version_at_least(first_line, minimum)
}

fn ffmpeg_snapshot_meets_minimum(full_output: &str) -> bool {
    for (library, minimum) in FFMPEG_MIN_LIBRARY_VERSIONS {
        let mut evidence = full_output.lines().filter_map(|line| {
            line.strip_prefix(library)
                .filter(|remainder| remainder.starts_with(char::is_whitespace))
        });
        let Some(remainder) = evidence.next() else {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_version",
                format!("FFmpeg snapshot is missing {library} version evidence"),
            );
            return false;
        };
        if evidence.next().is_some() {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_version",
                format!("FFmpeg snapshot has duplicate {library} version evidence"),
            );
            return false;
        }
        match parse_ffmpeg_library_version(remainder) {
            Some((compiled, runtime)) if compiled == runtime && compiled >= *minimum => {}
            Some((compiled, runtime)) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "tool_version",
                    format!(
                        "FFmpeg snapshot {library} compiled/runtime versions {compiled:?} / {runtime:?} must match and meet n6.1 API baseline {minimum:?}"
                    ),
                );
                return false;
            }
            None => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "tool_version",
                    format!("FFmpeg snapshot has malformed {library} version evidence"),
                );
                return false;
            }
        }
    }

    true
}

fn parse_ffmpeg_library_version(remainder: &str) -> Option<([u32; 3], [u32; 3])> {
    let (runtime, linked) = match remainder.split_once('/') {
        Some(versions) => versions,
        None => return None,
    };
    let runtime = match parse_ffmpeg_version_components(runtime) {
        Some(version) => version,
        None => return None,
    };
    let linked = match parse_ffmpeg_version_components(linked) {
        Some(version) => version,
        None => return None,
    };
    Some((runtime, linked))
}

fn parse_ffmpeg_version_components(value: &str) -> Option<[u32; 3]> {
    let compact = value
        .split('.')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(".");
    match parse_version_parts(&compact, "FFmpeg library")?.try_into() {
        Ok(version) => Some(version),
        Err(parts) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_version",
                format!("FFmpeg library version must have three components: {parts:?}"),
            );
            None
        }
    }
}

fn parse_version_parts(version: &str, label: &str) -> Option<Vec<u32>> {
    let mut parts = Vec::new();
    for part in version.split('.') {
        match part.parse::<u32>() {
            Ok(parsed) => parts.push(parsed),
            Err(e) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "tool_version",
                    format!(
                        "failed to parse {label} version component '{part}' from '{version}': {e}"
                    ),
                );
                return None;
            }
        }
    }
    Some(parts)
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
    fn ffmpeg_git_snapshot_versions_keep_their_release_components() {
        assert!(is_version_at_least(
            "ffmpeg version 8.0.git Copyright (c) 2000-2026 the FFmpeg developers",
            "6.1"
        ));
        assert!(is_version_at_least("ffmpeg version 6.1.git", "6.1"));
        assert!(!is_version_at_least("ffmpeg version 6.0.git", "6.1"));
        assert!(!is_version_at_least("ffmpeg version 6.1..git", "6.1"));
        assert!(!is_version_at_least("ffmpeg version 6.1.", "6.1"));
        assert!(!is_version_at_least("ffmpeg version 6.1. Copyright", "6.1"));
        assert!(!is_version_at_least(
            "ffmpeg version N-127165-g12c589a37d",
            "6.1"
        ));
    }

    #[test]
    fn ffmpeg_n_snapshot_uses_complete_library_api_versions() {
        let current_snapshot = concat!(
            "ffmpeg version N-127165-g12c589a37d Copyright (c) FFmpeg developers\n",
            "libavutil      61. 10.100 / 61. 10.100\n",
            "libavcodec     63. 16.100 / 63. 16.100\n",
            "libavformat    63.  7.100 / 63.  7.100\n",
        );
        assert!(version_meets_minimum(
            "ffmpeg",
            "ffmpeg version N-127165-g12c589a37d",
            current_snapshot,
            "6.1"
        ));
        assert!(!version_meets_minimum(
            "ffmpeg",
            "ffmpeg version N-127165-g12c589a37d",
            current_snapshot,
            "999.0"
        ));
        let exact_baseline = concat!(
            "ffmpeg version N-1-gabcdef0123\n",
            "libavutil      58. 29.100 / 58. 29.100\n",
            "libavcodec     60. 31.102 / 60. 31.102\n",
            "libavformat    60. 16.100 / 60. 16.100\n",
        );
        assert!(ffmpeg_snapshot_meets_minimum(exact_baseline));

        let old_snapshot = concat!(
            "ffmpeg version N-999999-gabcdef0123 Copyright (c) FFmpeg developers\n",
            "libavutil      58. 28.100 / 58. 28.100\n",
            "libavcodec     60. 30.102 / 60. 30.102\n",
            "libavformat    60. 15.100 / 60. 15.100\n",
        );
        assert!(!version_meets_minimum(
            "ffmpeg",
            "ffmpeg version N-999999-gabcdef0123",
            old_snapshot,
            "6.1"
        ));
    }

    #[test]
    fn ffmpeg_n_snapshot_requires_complete_consistent_library_evidence() {
        let missing_library = concat!(
            "ffmpeg version N-127165-g12c589a37d Copyright (c) FFmpeg developers\n",
            "libavutil      63.  8.100 / 63.  8.100\n",
            "libavcodec     63. 16.100 / 63. 16.100\n",
        );
        assert!(!ffmpeg_snapshot_meets_minimum(missing_library));

        let conflicting_library = concat!(
            "ffmpeg version N-127165-g12c589a37d Copyright (c) FFmpeg developers\n",
            "libavutil      63.  8.100 / 63.  8.100\n",
            "libavcodec     63. 16.100 / 63. 15.100\n",
            "libavformat    63.  6.103 / 63.  6.103\n",
        );
        assert!(!ffmpeg_snapshot_meets_minimum(conflicting_library));

        let malformed_library = concat!(
            "ffmpeg version N-127165-g12c589a37d Copyright (c) FFmpeg developers\n",
            "libavutil      unknown / unknown\n",
            "libavcodec     63. 16.100 / 63. 16.100\n",
            "libavformat    63.  6.103 / 63.  6.103\n",
        );
        assert!(!ffmpeg_snapshot_meets_minimum(malformed_library));
        for malformed in [
            "60..31.102 / 60.31.102",
            "60.31.102. / 60.31.102",
            "60.31 / 60.31.102",
            "60.31.102 / 60.31.102 / 60.31.102",
            "6 0.31.102 / 60.31.102",
        ] {
            assert!(
                parse_ffmpeg_library_version(malformed).is_none(),
                "{malformed}"
            );
        }
        let duplicate_instead_of_missing = concat!(
            "ffmpeg version N-127165-g12c589a37d\n",
            "libavutil      61.10.100 / 61.10.100\n",
            "libavutil      61.10.100 / 61.10.100\n",
            "libavcodec     63.16.100 / 63.16.100\n",
        );
        assert!(!ffmpeg_snapshot_meets_minimum(duplicate_instead_of_missing));
        let valid = format!("{missing_library}libavformat    63.  6.103 / 63.  6.103\n");
        assert!(ffmpeg_snapshot_meets_minimum(&valid));
        assert!(!ffmpeg_snapshot_meets_minimum(&format!(
            "{valid}libavformat    63.  6.103 / 63.  6.103\n"
        )));
    }

    #[test]
    fn ffprobe_uses_its_supported_version_flag() {
        assert_eq!(version_probe_arg("ffprobe"), "-version");
        assert_eq!(version_probe_arg("ffmpeg"), "-version");
        assert_eq!(version_probe_arg("cjxl"), "--version");
        let version =
            get_tool_version("ffprobe").expect("ffprobe must report its installed version");
        assert!(version.starts_with("ffprobe version "), "{version}");
        require(&["ffmpeg"]).expect("the installed FFmpeg must meet the version gate");
    }

    #[test]
    fn public_version_display_remains_single_line() {
        let full_output = concat!(
            "ffmpeg version N-127165-g12c589a37d Copyright (c) FFmpeg developers\n",
            "libavcodec     63. 16.100 / 63. 16.100\n",
        );
        assert_eq!(
            first_version_line(full_output),
            Some("ffmpeg version N-127165-g12c589a37d Copyright (c) FFmpeg developers".into())
        );
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
        assert!(is_version_at_least("ffmpeg version 9.0.1", "6.1"));
        assert!(!is_version_at_least("ffmpeg version 5.0", "6.1"));

        // exiftool version formats (uses -ver flag, returns plain version like "13.55")
        assert!(is_version_at_least("13.55", "12.70"));
        assert!(is_version_at_least("12.70", "12.70"));
        assert!(!is_version_at_least("11.85", "12.70"));
    }

    /// Integration test: Actually invoke tools and verify version detection
    /// works This catches issues like exiftool's non-standard --version
    /// behavior
    #[test]
    fn test_get_tool_version_integration() {
        if !crate::common_utils::is_command_available("exiftool") {
            eprintln!("skipping test_get_tool_version_integration: exiftool unavailable");
            return;
        }

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
