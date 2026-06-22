//! Live Photo Detection Module
//!
//! Detects Apple Live Photos by checking for companion MOV files

use std::path::Path;

/// Check if a file is part of a Live Photo pair
///
/// Live Photos consist of:
/// - A HEIC/HEIF image file (e.g., `IMG_1234.HEIC`)
/// - A companion MOV video file (e.g., `IMG_1234.MOV`)
///
/// This function checks if the given file has a companion file with the same
/// stem but different extension (.mov/.MOV for images, .heic/.HEIC for videos)
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

    // Check if this is a HEIC/HEIF file with a companion MOV
    if matches!(ext_lower.as_str(), "heic" | "heif" | "hif") {
        // Look for companion .mov or .MOV file
        let mov_path = parent.join(format!("{stem}.mov"));
        let mov_upper_path = parent.join(format!("{stem}.MOV"));

        if mov_path.exists() || mov_upper_path.exists() {
            return true;
        }
    }

    // Check if this is a MOV file with a companion HEIC/HEIF
    if ext_lower == "mov" {
        // Look for companion HEIC/HEIF files
        let heic_path = parent.join(format!("{stem}.heic"));
        let heic_upper_path = parent.join(format!("{stem}.HEIC"));
        let heif_path = parent.join(format!("{stem}.heif"));
        let heif_upper_path = parent.join(format!("{stem}.HEIF"));

        if heic_path.exists()
            || heic_upper_path.exists()
            || heif_path.exists()
            || heif_upper_path.exists()
        {
            return true;
        }
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
}
