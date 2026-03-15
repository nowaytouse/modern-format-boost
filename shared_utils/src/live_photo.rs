//! Live Photo Detection Module
//!
//! Detects Apple Live Photos by checking for companion MOV files

use std::path::Path;

/// Check if a file is part of a Live Photo pair
///
/// Live Photos consist of:
/// - A HEIC/HEIF image file (e.g., IMG_1234.HEIC)
/// - A companion MOV video file (e.g., IMG_1234.MOV)
///
/// This function checks if the given file has a companion file with the same
/// stem but different extension (.mov/.MOV for images, .heic/.HEIC for videos)
pub fn is_live_photo(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };

    let ext_lower = ext.to_lowercase();
    let stem = path.file_stem().and_then(|s| s.to_str());
    let parent = path.parent();

    let Some(stem) = stem else {
        return false;
    };
    let Some(parent) = parent else {
        return false;
    };

    // Check if this is a HEIC/HEIF file with a companion MOV
    if matches!(ext_lower.as_str(), "heic" | "heif" | "hif") {
        // Look for companion .mov or .MOV file
        let mov_path = parent.join(format!("{}.mov", stem));
        let mov_upper_path = parent.join(format!("{}.MOV", stem));

        if mov_path.exists() || mov_upper_path.exists() {
            return true;
        }
    }

    // Check if this is a MOV file with a companion HEIC/HEIF
    if ext_lower == "mov" {
        // Look for companion HEIC/HEIF files
        let heic_path = parent.join(format!("{}.heic", stem));
        let heic_upper_path = parent.join(format!("{}.HEIC", stem));
        let heif_path = parent.join(format!("{}.heif", stem));
        let heif_upper_path = parent.join(format!("{}.HEIF", stem));

        if heic_path.exists() || heic_upper_path.exists()
            || heif_path.exists() || heif_upper_path.exists() {
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
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create a Live Photo pair
        let heic_path = base_path.join("IMG_1234.HEIC");
        let mov_path = base_path.join("IMG_1234.MOV");

        File::create(&heic_path).unwrap();
        File::create(&mov_path).unwrap();

        // Both files should be detected as Live Photo
        assert!(is_live_photo(&heic_path));
        assert!(is_live_photo(&mov_path));

        // Single HEIC without MOV should not be Live Photo
        let single_heic = base_path.join("IMG_5678.HEIC");
        File::create(&single_heic).unwrap();
        assert!(!is_live_photo(&single_heic));

        // Single MOV without HEIC should not be Live Photo
        let single_mov = base_path.join("VID_9999.MOV");
        File::create(&single_mov).unwrap();
        assert!(!is_live_photo(&single_mov));
    }

    #[test]
    fn test_case_insensitive() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Test lowercase heic with uppercase MOV
        let heic_lower = base_path.join("IMG_0001.heic");
        let mov_upper = base_path.join("IMG_0001.MOV");

        File::create(&heic_lower).unwrap();
        File::create(&mov_upper).unwrap();

        assert!(is_live_photo(&heic_lower));
        assert!(is_live_photo(&mov_upper));
    }
}
