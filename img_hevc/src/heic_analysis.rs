//! HEIC/HEIF Format Analysis Module
//!
//! Uses libheif-rs to decode and analyze HEIC/HEIF images

use crate::{ImgQualityError, Result};
use image::DynamicImage;
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// HEIC analysis results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeicAnalysis {
    /// Bit depth (8, 10, 12)
    pub bit_depth: u8,
    /// Compression codec (HEVC, AV1, etc)
    pub codec: String,
    /// Whether image is lossless
    pub is_lossless: bool,
    /// Has alpha channel
    pub has_alpha: bool,
    /// Has auxiliary images (depth map, etc)
    pub has_auxiliary: bool,
    /// Number of images in container
    pub image_count: usize,
}

/// Load and analyze a HEIC/HEIF file
pub fn analyze_heic_file(path: &Path) -> Result<(DynamicImage, HeicAnalysis)> {
    // Initialize libheif
    let lib_heif = LibHeif::new();

    // 🔥 v7.8.1: 增强HEIC错误处理，特别是SecurityLimitExceeded错误
    let ctx = HeifContext::read_from_file(path.to_string_lossy().as_ref()).map_err(|e| {
        let error_msg = format!("{}", e);
        if error_msg.contains("SecurityLimitExceeded") || error_msg.contains("ipco") {
            eprintln!(
                "⚠️  HEIC SecurityLimitExceeded: {} - using fallback analysis",
                path.display()
            );
            ImgQualityError::ImageReadError(format!(
                "HEIC security limit exceeded (ipco box limit): {}",
                e
            ))
        } else {
            ImgQualityError::ImageReadError(format!("Failed to read HEIC: {}", e))
        }
    })?;

    let handle = ctx.primary_image_handle().map_err(|e| {
        ImgQualityError::ImageReadError(format!("Failed to get primary image: {}", e))
    })?;

    let width = handle.width();
    let height = handle.height();
    let has_alpha = handle.has_alpha_channel();
    let bit_depth = handle.luma_bits_per_pixel();
    let is_lossless = false; // HEIC is typically lossy

    // Get image count
    let image_count = ctx.number_of_top_level_images();

    // Check for auxiliary images
    let has_auxiliary = handle.number_of_depth_images() > 0;

    // Decode to RGB using LibHeif
    let decoded_image = lib_heif
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to decode HEIC: {}", e)))?;

    let planes = decoded_image.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| ImgQualityError::ImageReadError("No RGB plane found".to_string()))?;

    // Convert to image::DynamicImage
    let img = image::RgbImage::from_raw(width, height, plane.data.to_vec())
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| ImgQualityError::ImageReadError("Failed to create RGB image".to_string()))?;

    // Determine codec
    let codec = "HEVC".to_string(); // Default for HEIC

    let analysis = HeicAnalysis {
        bit_depth,
        codec,
        is_lossless,
        has_alpha,
        has_auxiliary,
        image_count,
    };

    Ok((img, analysis))
}

/// Check if file is HEIC/HEIF format (Content-aware)
///
/// v8.1.1: Added magic byte detection to support files with incorrect extensions
pub fn is_heic_file(path: &Path) -> bool {
    // 1. Check extension (fast path)
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_lowercase();
        if matches!(ext.as_str(), "heic" | "heif" | "hif") {
            return true;
        }
    }

    // 2. Check magic bytes (content-aware fallback)
    // HEIF files are ISO-BMFF containers starting with an 'ftyp' box.
    if let Ok(mut file) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buffer = [0u8; 12];
        if file.read_exact(&mut buffer).is_ok() {
            // Offset 4-7 is "ftyp"
            if &buffer[4..8] == b"ftyp" {
                let brand = &buffer[8..12];
                // Common HEIC brands: heic, heix, heim, heis, mif1, msf1
                if matches!(
                    brand,
                    b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1"
                ) {
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_heic_file() {
        assert!(is_heic_file(Path::new("test.heic")));
        assert!(is_heic_file(Path::new("test.HEIC")));
        assert!(is_heic_file(Path::new("test.heif")));
        assert!(!is_heic_file(Path::new("test.jpg")));
    }
}
