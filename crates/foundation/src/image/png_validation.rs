//! PNG genuineness validation and heuristic policy.
//!
//! Genuine PNG media receive full exemption: always treated as lossless, encoded
//! to JXL at effort 10 with no file-size gate. PNG quantization heuristics are
//! retained but disabled by default (`MFB_ENABLE_PNG_HEURISTIC`).

use crate::unified_error::{ImgQualityError, Result};
use std::path::Path;

use super::format_detect::{FormatKind, detect_true_format, validate_format_forensic};

/// When set to `1`/`true`/`yes`, re-enable PNG quantization heuristics for
/// content-level lossy detection (256-color art, palette logos, etc.).
pub const ENV_ENABLE_PNG_HEURISTIC: &str = "MFB_ENABLE_PNG_HEURISTIC";

/// Effort value mandated for genuine PNG → lossless JXL encoding.
pub const PNG_LOSSLESS_JXL_EFFORT: u8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PngValidationOutcome {
    Confirmed,
    Rejected,
    ToolUnavailable,
}

#[must_use]
pub fn png_heuristic_enabled() -> bool {
    match std::env::var(ENV_ENABLE_PNG_HEURISTIC) {
        Ok(value) => matches!(
            value.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            tracing::debug!("PNG heuristic env var error: {}", e);
            false
        }
    }
}

/// Hierarchical PNG validation: pngcheck → libpng/image decode → magic bytes.
///
/// Returns `false` when magic indicates PNG but authoritative validation rejects
/// the file. Returns `true` only for structurally valid PNG media.
pub fn is_true_png(path: &Path) -> Result<bool> {
    if detect_true_format(path)? != FormatKind::Png {
        return Ok(false);
    }

    match try_pngcheck_validation(path) {
        PngValidationOutcome::Confirmed => Ok(true),
        PngValidationOutcome::Rejected => Ok(false),
        PngValidationOutcome::ToolUnavailable => match png_libpng_decode_probe(path) {
            Ok(true) => Ok(true),
            Ok(false) => Ok(false),
            Err(err) => {
                tracing::warn!(
                    target: "png_validation",
                    path = %path.display(),
                    error = %err,
                    "PNG decode probe failed; falling back to magic-bytes admission"
                );
                Ok(true)
            }
        },
    }
}

/// Validate a magic-identified PNG with the shared PNG audit tool (`pngcheck`).
pub fn validate_png_forensic(path: &Path) -> Result<super::format_detect::ForensicFormatCheck> {
    validate_format_forensic(path, FormatKind::Png)
}

fn try_pngcheck_validation(path: &Path) -> PngValidationOutcome {
    if super::format_detect::forensic_tool_for_format(FormatKind::Png).is_none() {
        return PngValidationOutcome::ToolUnavailable;
    }
    if crate::common_utils::resolve_tool_path(crate::constants::TOOL_PNGCHECK).is_none() {
        return PngValidationOutcome::ToolUnavailable;
    }
    match validate_format_forensic(path, FormatKind::Png) {
        Ok(check) => {
            tracing::debug!(
                target: "png_validation",
                path = %path.display(),
                tool = %check.tool,
                "PNG confirmed via authoritative validator"
            );
            PngValidationOutcome::Confirmed
        }
        Err(err) => {
            let message = err.to_string();
            if message.contains("requires '") && message.contains("' on PATH") {
                PngValidationOutcome::ToolUnavailable
            } else {
                tracing::debug!(
                    target: "png_validation",
                    path = %path.display(),
                    error = %message,
                    "PNG rejected by authoritative validator"
                );
                PngValidationOutcome::Rejected
            }
        }
    }
}

fn png_libpng_decode_probe(path: &Path) -> Result<bool> {
    image::open(path).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "PNG decode probe failed for {}: {err}",
            path.display()
        ))
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use tempfile::NamedTempFile;

    const ONE_BY_ONE_RGBA_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    #[serial]
    fn png_heuristic_disabled_by_default() {
        unsafe {
            std::env::remove_var(ENV_ENABLE_PNG_HEURISTIC);
        }
        assert!(!png_heuristic_enabled());
    }

    #[test]
    #[serial]
    fn png_heuristic_enabled_when_env_set() {
        unsafe {
            std::env::set_var(ENV_ENABLE_PNG_HEURISTIC, "1");
        }
        assert!(png_heuristic_enabled());
        unsafe {
            std::env::remove_var(ENV_ENABLE_PNG_HEURISTIC);
        }
    }

    #[test]
    #[serial]
    fn test_png_heuristic_enabled_all_values() {
        for (val, expected) in [
            ("1", true),
            ("true", true),
            ("TRUE", true),
            ("yes", true),
            ("YES", true),
            ("on", true),
            ("ON", true),
            ("0", false),
            ("false", false),
            ("no", false),
            ("off", false),
            ("", false),
        ] {
            unsafe {
                std::env::set_var(ENV_ENABLE_PNG_HEURISTIC, val);
            }
            assert_eq!(png_heuristic_enabled(), expected, "val={val}");
        }
        unsafe {
            std::env::remove_var(ENV_ENABLE_PNG_HEURISTIC);
        }
        assert!(!png_heuristic_enabled());
    }

    #[test]
    fn true_png_accepts_structurally_valid_bytes() -> Result<()> {
        let mut file = NamedTempFile::new().expect("temp png");
        file.write_all(ONE_BY_ONE_RGBA_PNG).expect("write png");
        assert!(is_true_png(file.path())?);
        Ok(())
    }

    #[test]
    fn true_png_rejects_non_png_magic() -> Result<()> {
        let mut file = NamedTempFile::new().expect("temp fake png");
        file.write_all(b"not a png file").expect("write junk");
        assert!(!is_true_png(file.path())?);
        Ok(())
    }
}
