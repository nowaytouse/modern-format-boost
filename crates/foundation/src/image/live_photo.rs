//! Live Photo Detection Module
//!
//! Detects Apple Live Photos by checking for companion MOV files.

use std::path::Path;

const LIVE_STILL_EXTENSIONS: &[&str] = &["heic", "heif", "hif", "jpg", "jpeg"];

/// Check if a file is part of a Live Photo pair
///
/// Live Photos consist of:
/// - A HEIC/HEIF/HIF or JPEG image file (e.g., `IMG_1234.HEIC`)
/// - A companion MOV video file (e.g., `IMG_1234.MOV`)
///
/// This function checks if the given file has a companion file with the same
/// stem and the corresponding still/video extension, case-insensitively.
fn has_regular_companion(parent: &Path, stem: &str, extensions: &[&str]) -> bool {
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) => {
            crate::media_conversion_gate::probe_image_format_audit(
                "live_photo_parent_read_failed",
                parent,
                format!("failed to inspect Live Photo companion directory: {error}"),
            );
            return false;
        }
    };

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
            Ok(metadata) if metadata.file_type().is_file() => return true,
            Ok(_) => {}
            Err(error) => crate::media_conversion_gate::probe_image_format_audit(
                "live_photo_companion_metadata_failed",
                &path,
                format!("failed to inspect a matching Live Photo companion: {error}"),
            ),
        }
    }
    false
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

    if LIVE_STILL_EXTENSIONS.contains(&ext_lower.as_str()) {
        return has_regular_companion(parent, &stem, &["mov"]);
    }

    if ext_lower == "mov" {
        return has_regular_companion(parent, &stem, LIVE_STILL_EXTENSIONS);
    }

    false
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
}
