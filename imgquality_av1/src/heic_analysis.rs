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
    
    let ctx = HeifContext::read_from_file(path.to_string_lossy().as_ref())
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to read HEIC: {}", e)))?;
    
    let handle = ctx.primary_image_handle()
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to get primary image: {}", e)))?;
    
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
    let decoded_image = lib_heif.decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to decode HEIC: {}", e)))?;
    
    let planes = decoded_image.planes();
    let plane = planes.interleaved
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

/// Check if file is HEIC/HEIF format
pub fn is_heic_file(path: &Path) -> bool {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    
    matches!(ext.as_deref(), Some("heic") | Some("heif") | Some("hif"))
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
