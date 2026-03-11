//! HEIC/HEIF Format Analysis Module
//!
//! Uses libheif-rs to decode and analyze HEIC/HEIF images

use crate::img_errors::{ImgQualityError, Result};
use image::DynamicImage;
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeicAnalysis {
    pub bit_depth: u8,
    pub codec: String,
    pub is_lossless: bool,
    pub has_alpha: bool,
    pub has_auxiliary: bool,
    pub image_count: usize,
    pub is_hdr: bool,
    pub is_dolby_vision: bool,
}

pub fn analyze_heic_file(path: &Path) -> Result<(DynamicImage, HeicAnalysis)> {
    let lib_heif = LibHeif::new();

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
    
    // Deterministic lossless detection from headers
    let is_lossless = crate::image_detection::detect_heic_compression(path)
        .map(|comp| comp == crate::image_detection::CompressionType::Lossless)
        .unwrap_or(false);

    let image_count = ctx.image_ids().len();

    let has_auxiliary = handle.number_of_depth_images() > 0;

    // Detect HDR and Dolby Vision
    let (is_hdr, is_dolby_vision) = detect_heic_hdr_dv(path);

    let decoded_image = lib_heif
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to decode HEIC: {}", e)))?;

    let planes = decoded_image.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| ImgQualityError::ImageReadError("No RGB plane found".to_string()))?;

    let img = image::RgbImage::from_raw(width, height, plane.data.to_vec())
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| ImgQualityError::ImageReadError("Failed to create RGB image".to_string()))?;

    let codec = "HEVC".to_string();

    let analysis = HeicAnalysis {
        bit_depth,
        codec,
        is_lossless,
        has_alpha,
        has_auxiliary,
        image_count,
        is_hdr,
        is_dolby_vision,
    };

    Ok((img, analysis))
}

pub fn is_heic_file(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_lowercase();
        if matches!(ext.as_str(), "heic" | "heif" | "hif") {
            return true;
        }
    }

    if let Ok(mut file) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buffer = [0u8; 12];
        if file.read_exact(&mut buffer).is_ok()
            && &buffer[4..8] == b"ftyp" {
                let brand = &buffer[8..12];
                if matches!(
                    brand,
                    b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1"
                ) {
                    return true;
                }
            }
    }

    false
}

/// Detect HDR and Dolby Vision in HEIC files by reading raw file bytes
/// Returns (is_hdr, is_dolby_vision)
fn detect_heic_hdr_dv(path: &Path) -> (bool, bool) {
    use std::fs::File;
    use std::io::Read;

    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return (false, false),
    };

    // Read first 64KB for box scanning
    let mut buffer = vec![0u8; 65536];
    let bytes_read = file.read(&mut buffer).unwrap_or(0);
    if bytes_read < 12 {
        return (false, false);
    }
    buffer.truncate(bytes_read);

    let mut is_hdr = false;
    let mut is_dolby_vision = false;

    // Scan for HEVC configuration boxes (hvcC) and color info (colr, nclx)
    let mut pos = 0;
    while pos + 8 <= buffer.len() {
        if pos + 4 > buffer.len() {
            break;
        }

        let box_size = u32::from_be_bytes([
            buffer[pos],
            buffer[pos + 1],
            buffer[pos + 2],
            buffer[pos + 3],
        ]) as usize;

        if box_size < 8 || pos + box_size > buffer.len() {
            pos += 1;
            continue;
        }

        let box_type = &buffer[pos + 4..pos + 8];

        // Check for hvcC (HEVC configuration)
        if box_type == b"hvcC" && pos + 23 <= buffer.len() {
            // Check for HDR transfer characteristics in hvcC
            // Byte 22+ contains VPS/SPS/PPS NAL units
            if pos + 30 < buffer.len() {
                // Look for transfer_characteristics indicating HDR
                // PQ (SMPTE 2084) = 16, HLG = 18
                for &b in buffer.iter().take(std::cmp::min(pos + box_size, buffer.len() - 1)).skip(pos + 22) {
                    if b == 16 || b == 18 {
                        is_hdr = true;
                        break;
                    }
                }
            }
        }

        // Check for Dolby Vision configuration (dvcC or dvvC)
        if (box_type == b"dvcC" || box_type == b"dvvC") && box_size >= 24 {
            is_dolby_vision = true;
            is_hdr = true; // Dolby Vision implies HDR
        }

        // Check for colr/nclx boxes with HDR color space
        if box_type == b"colr" && box_size >= 18
            && pos + 12 < buffer.len() {
                let color_type = &buffer[pos + 8..pos + 12];
                if color_type == b"nclx" && pos + 18 <= buffer.len() {
                    // Check color primaries (BT.2020 = 9)
                    let primaries = u16::from_be_bytes([buffer[pos + 12], buffer[pos + 13]]);
                    // Check transfer characteristics (PQ = 16, HLG = 18)
                    let transfer = u16::from_be_bytes([buffer[pos + 14], buffer[pos + 15]]);

                    if primaries == 9 && (transfer == 16 || transfer == 18) {
                        is_hdr = true;
                    }
                }
            }

        pos += box_size;
    }

    (is_hdr, is_dolby_vision)
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
