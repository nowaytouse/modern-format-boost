//! Live Photo Detection Module
//!
//! Detects Apple Live Photos from same-stem still/MOV companions and verifies
//! their Apple Content Identifier whenever both files expose one.

use crate::builder_base::ToolBuilder;
use anyhow::Context;
use std::path::{Path, PathBuf};

const LIVE_STILL_EXTENSIONS: &[&str] = &["heic", "heif", "hif", "jpg", "jpeg"];

/// Check if a file is part of a Live Photo pair
///
/// Live Photos consist of:
/// - A HEIC/HEIF/HIF or JPEG image file (e.g., `IMG_1234.HEIC`)
/// - A companion MOV video file (e.g., `IMG_1234.MOV`)
///
/// Same-stem discovery is followed by Apple Content Identifier verification.
/// A proven mismatch is not a pair; missing or unreadable identity metadata is
/// retained conservatively so metadata loss cannot make a real pair unsafe.
fn regular_companions(parent: &Path, stem: &str, extensions: &[&str]) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) => {
            crate::media_conversion_gate::probe_image_format_audit(
                "live_photo_parent_read_failed",
                parent,
                format!("failed to inspect Live Photo companion directory: {error}"),
            );
            return Vec::new();
        }
    };

    let mut companions = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                crate::media_conversion_gate::probe_image_format_audit(
                    "live_photo_directory_entry_failed",
                    parent,
                    format!("failed to read a Live Photo companion directory entry: {error}"),
                );
                continue;
            }
        };
        let path = entry.path();
        let same_stem = path
            .file_stem()
            .is_some_and(|candidate| candidate == std::ffi::OsStr::new(stem));
        let matching_extension = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                extensions
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            });
        if !same_stem || !matching_extension {
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => companions.push(path),
            Ok(_) => {}
            Err(error) => crate::media_conversion_gate::probe_image_format_audit(
                "live_photo_companion_metadata_failed",
                &path,
                format!("failed to inspect a matching Live Photo companion: {error}"),
            ),
        }
    }
    companions.sort_unstable();
    companions
}

fn parse_content_identifier_json(stdout: &[u8]) -> anyhow::Result<Option<String>> {
    let records: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_slice(stdout).context("invalid ExifTool Live Photo identity JSON")?;
    anyhow::ensure!(
        records.len() == 1,
        "ExifTool returned {} Live Photo identity records for one file",
        records.len()
    );

    let mut identifier: Option<String> = None;
    for (key, value) in &records[0] {
        if key != "ContentIdentifier" && !key.ends_with(":ContentIdentifier") {
            continue;
        }
        let candidate = value
            .as_str()
            .context("ExifTool returned a non-string Apple Content Identifier")?
            .trim();
        if candidate.is_empty() {
            continue;
        }
        if let Some(existing) = identifier.as_deref() {
            anyhow::ensure!(
                existing.eq_ignore_ascii_case(candidate),
                "file contains conflicting Apple Content Identifiers"
            );
        } else {
            identifier = Some(candidate.to_string());
        }
    }
    Ok(identifier)
}

fn content_identifier(path: &Path) -> anyhow::Result<Option<String>> {
    let mut builder = crate::ExiftoolBuilder::new();
    builder
        .arg("-j")
        .arg("-G1")
        .arg("-Apple:ContentIdentifier")
        .arg("-Keys:ContentIdentifier")
        .input(path);
    let output = builder
        .build()
        .output()
        .with_context(|| format!("failed to read Live Photo identity from {}", path.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "ExifTool could not read Live Photo identity from {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    parse_content_identifier_json(&output.stdout)
}

fn content_identifiers_agree(left: Option<&str>, right: Option<&str>) -> Option<bool> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.trim().eq_ignore_ascii_case(right.trim())),
        _ => None,
    }
}

fn same_stem_pair_is_live(path: &Path, companion: &Path) -> bool {
    match (content_identifier(path), content_identifier(companion)) {
        (Ok(left), Ok(right)) => match content_identifiers_agree(left.as_deref(), right.as_deref()) {
            Some(true) => true,
            Some(false) => {
                crate::media_conversion_gate::probe_image_format_audit(
                    "live_photo_identifier_mismatch",
                    path,
                    format!(
                        "same-stem companion {} has a different Apple Content Identifier; treating the files independently",
                        companion.display()
                    ),
                );
                false
            }
            None => {
                crate::media_conversion_gate::probe_image_format_audit(
                    "live_photo_identifier_unavailable",
                    path,
                    format!(
                        "Apple Content Identifier is absent from one or both same-stem files; conservatively retaining the pair with {}",
                        companion.display()
                    ),
                );
                true
            }
        },
        (Err(error), _) | (_, Err(error)) => {
            crate::media_conversion_gate::probe_image_format_audit(
                "live_photo_identifier_probe_failed",
                path,
                format!(
                    "Apple Content Identifier could not be verified; conservatively retaining same-stem pair with {}: {error}",
                    companion.display()
                ),
            );
            true
        }
    }
}

/// Check whether a path belongs to a same-stem Apple still/MOV pair.
#[must_use]
pub fn is_live(path: &Path) -> bool {
    let ext_lower = crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path);
    if ext_lower.is_empty() {
        return false;
    }

    let stem = crate::media_conversion_gate::path_file_stem_lossy_or_empty(path);
    let Some(parent) = path.parent() else {
        return false;
    };
    if stem.is_empty() {
        return false;
    }

    let companions = if LIVE_STILL_EXTENSIONS.contains(&ext_lower.as_str()) {
        regular_companions(parent, &stem, &["mov"])
    } else if ext_lower == "mov" {
        regular_companions(parent, &stem, LIVE_STILL_EXTENSIONS)
    } else {
        Vec::new()
    };

    companions
        .iter()
        .any(|companion| same_stem_pair_is_live(path, companion))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    #[test]
    fn test_live_photo_detection() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        let base_path = temp_dir.path();

        // Create a Live Photo pair
        let heic_path = base_path.join("IMG_1234.HEIC");
        let mov_path = base_path.join("IMG_1234.MOV");

        File::create(&heic_path).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        File::create(&mov_path).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });

        // Both files should be detected as Live Photo
        assert!(is_live(&heic_path));
        assert!(is_live(&mov_path));

        // Single HEIC without MOV should not be Live Photo
        let single_heic = base_path.join("IMG_5678.HEIC");
        File::create(&single_heic).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        assert!(!is_live(&single_heic));

        // Single MOV without HEIC should not be Live Photo
        let single_mov = base_path.join("VID_9999.MOV");
        File::create(&single_mov).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        assert!(!is_live(&single_mov));
    }

    #[test]
    fn test_case_insensitive() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        let base_path = temp_dir.path();

        // Test lowercase heic with uppercase MOV
        let heic_lower = base_path.join("IMG_0001.heic");
        let mov_upper = base_path.join("IMG_0001.MOV");

        File::create(&heic_lower).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        File::create(&mov_upper).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });

        assert!(is_live(&heic_lower));
        assert!(is_live(&mov_upper));
    }

    #[test]
    fn jpeg_and_mov_pair_is_a_live_photo() -> std::io::Result<()> {
        let temp_dir = TempDir::new()?;
        let jpeg = temp_dir.path().join("IMG_0002.JPG");
        let mov = temp_dir.path().join("IMG_0002.mov");
        File::create(&jpeg)?;
        File::create(&mov)?;

        assert!(is_live(&jpeg));
        assert!(is_live(&mov));
        Ok(())
    }

    #[test]
    fn content_identifier_parser_accepts_both_apple_groups_and_rejects_conflicts() {
        let apple = br#"[{"SourceFile":"still.heic","Apple:ContentIdentifier":" AABB-CCDD "}]"#;
        let quicktime =
            br#"[{"SourceFile":"motion.mov","Keys:ContentIdentifier":"aabb-ccdd"}]"#;
        assert_eq!(
            parse_content_identifier_json(apple).expect("Apple identifier JSON"),
            Some("AABB-CCDD".to_string())
        );
        assert_eq!(
            parse_content_identifier_json(quicktime).expect("QuickTime identifier JSON"),
            Some("aabb-ccdd".to_string())
        );
        assert_eq!(
            content_identifiers_agree(Some("AABB-CCDD"), Some("aabb-ccdd")),
            Some(true)
        );
        assert_eq!(
            content_identifiers_agree(Some("AABB-CCDD"), Some("different")),
            Some(false)
        );
        assert_eq!(content_identifiers_agree(Some("AABB-CCDD"), None), None);

        let conflicting = br#"[{"Apple:ContentIdentifier":"one","Keys:ContentIdentifier":"two"}]"#;
        assert!(parse_content_identifier_json(conflicting).is_err());
    }
}
