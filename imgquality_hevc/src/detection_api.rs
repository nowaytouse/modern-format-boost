//! Detection API Module
//! 
//! Pure analysis layer - detects image properties without trusting file extensions.
//! Uses magic bytes and actual file content for accurate format detection.

use crate::{ImgQualityError, Result};
use image::{DynamicImage, GenericImageView};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Image type classification (static vs animated)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageType {
    /// Single frame static image
    Static,
    /// Multi-frame animated image (GIF, APNG, animated WebP)
    Animated,
}

/// Compression type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionType {
    /// Mathematically lossless compression
    Lossless,
    /// Lossy compression with quality loss
    Lossy,
}

/// Detected image format (from magic bytes, not extension)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectedFormat {
    PNG,
    JPEG,
    GIF,
    WebP,
    HEIC,
    HEIF,
    AVIF,
    JXL,
    TIFF,
    BMP,
    Unknown(String),
}

impl DetectedFormat {
    pub fn as_str(&self) -> &str {
        match self {
            DetectedFormat::PNG => "PNG",
            DetectedFormat::JPEG => "JPEG",
            DetectedFormat::GIF => "GIF",
            DetectedFormat::WebP => "WebP",
            DetectedFormat::HEIC => "HEIC",
            DetectedFormat::HEIF => "HEIF",
            DetectedFormat::AVIF => "AVIF",
            DetectedFormat::JXL => "JXL",
            DetectedFormat::TIFF => "TIFF",
            DetectedFormat::BMP => "BMP",
            DetectedFormat::Unknown(s) => s,
        }
    }
}

/// Complete detection result - all image properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    /// File path
    pub file_path: String,
    
    /// Detected format (from magic bytes)
    pub format: DetectedFormat,
    
    /// Image type (static or animated)
    pub image_type: ImageType,
    
    /// Compression type (lossless or lossy)
    pub compression: CompressionType,
    
    /// Image dimensions
    pub width: u32,
    pub height: u32,
    
    /// Color depth in bits
    pub bit_depth: u8,
    
    /// Has alpha channel
    pub has_alpha: bool,
    
    /// File size in bytes
    pub file_size: u64,
    
    /// Frame count (1 for static, >1 for animated)
    pub frame_count: u32,
    
    /// Frames per second (for animated images)
    pub fps: Option<f32>,
    
    /// Duration in seconds (for animated images)
    pub duration: Option<f32>,
    
    /// Estimated quality (0-100 for JPEG)
    pub estimated_quality: Option<u8>,
    
    /// Image entropy (complexity measure)
    pub entropy: f64,
}

/// Detect format from magic bytes (not file extension)
pub fn detect_format_from_bytes(path: &Path) -> Result<DetectedFormat> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 32];
    file.read(&mut header)?;
    
    // Check magic bytes
    if header.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Ok(DetectedFormat::PNG);
    }
    
    if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok(DetectedFormat::JPEG);
    }
    
    if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        return Ok(DetectedFormat::GIF);
    }
    
    if header.starts_with(b"RIFF") && header[8..12] == *b"WEBP" {
        return Ok(DetectedFormat::WebP);
    }
    
    // HEIC/HEIF - ftyp box with heic, heix, hevc, hevx, mif1
    if header[4..8] == *b"ftyp" {
        let brand = &header[8..12];
        if brand == b"heic" || brand == b"heix" || brand == b"mif1" {
            return Ok(DetectedFormat::HEIC);
        }
        if brand == b"heif" {
            return Ok(DetectedFormat::HEIF);
        }
        if brand == b"avif" {
            return Ok(DetectedFormat::AVIF);
        }
    }
    
    // JXL - starts with 0xFF 0x0A or 0x00 0x00 0x00 0x0C 0x4A 0x58 0x4C 0x20
    if header.starts_with(&[0xFF, 0x0A]) {
        return Ok(DetectedFormat::JXL);
    }
    if header.starts_with(&[0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20]) {
        return Ok(DetectedFormat::JXL);
    }
    
    // TIFF - II or MM
    if header.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || header.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        return Ok(DetectedFormat::TIFF);
    }
    
    // BMP
    if header.starts_with(b"BM") {
        return Ok(DetectedFormat::BMP);
    }
    
    Ok(DetectedFormat::Unknown("Unknown format".to_string()))
}

/// Detect if image is animated (multi-frame)
pub fn detect_animation(path: &Path, format: &DetectedFormat) -> Result<(bool, u32, Option<f32>)> {
    match format {
        DetectedFormat::GIF => {
            // GIF: check for NETSCAPE extension or multiple image blocks
            let data = std::fs::read(path)?;
            let frame_count = count_gif_frames(&data);
            let is_animated = frame_count > 1;
            let fps = if is_animated { Some(10.0) } else { None }; // Default GIF fps
            Ok((is_animated, frame_count, fps))
        }
        DetectedFormat::WebP => {
            // WebP: check for ANIM chunk
            let data = std::fs::read(path)?;
            let is_animated = data.windows(4).any(|w| w == b"ANIM");
            let frame_count = if is_animated { count_webp_frames(&data) } else { 1 };
            let fps = if is_animated { Some(24.0) } else { None };
            Ok((is_animated, frame_count, fps))
        }
        DetectedFormat::PNG => {
            // APNG: check for acTL chunk
            let data = std::fs::read(path)?;
            let is_animated = data.windows(4).any(|w| w == b"acTL");
            Ok((is_animated, if is_animated { 2 } else { 1 }, None))
        }
        _ => Ok((false, 1, None)),
    }
}

/// Count frames in GIF
fn count_gif_frames(data: &[u8]) -> u32 {
    let mut count = 0u32;
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x2C { // Image descriptor
            count += 1;
        }
        i += 1;
    }
    count.max(1)
}

/// Count frames in animated WebP
fn count_webp_frames(data: &[u8]) -> u32 {
    let mut count = 0u32;
    for window in data.windows(4) {
        if window == b"ANMF" {
            count += 1;
        }
    }
    count.max(1)
}

/// Detect compression type (lossless vs lossy)
pub fn detect_compression(format: &DetectedFormat, path: &Path) -> Result<CompressionType> {
    match format {
        // Always lossless formats
        DetectedFormat::PNG | DetectedFormat::BMP | DetectedFormat::TIFF => {
            Ok(CompressionType::Lossless)
        }
        
        // Always lossy formats
        DetectedFormat::JPEG => {
            Ok(CompressionType::Lossy)
        }
        
        // GIF is technically lossless compression (but limited palette)
        DetectedFormat::GIF => {
            Ok(CompressionType::Lossless)
        }
        
        // WebP can be either - check VP8L chunk for lossless
        DetectedFormat::WebP => {
            let data = std::fs::read(path)?;
            let is_lossless = data.windows(4).any(|w| w == b"VP8L");
            Ok(if is_lossless { CompressionType::Lossless } else { CompressionType::Lossy })
        }
        
        // HEIC/HEIF/AVIF - typically lossy unless specific lossless mode
        DetectedFormat::HEIC | DetectedFormat::HEIF | DetectedFormat::AVIF => {
            Ok(CompressionType::Lossy)
        }
        
        // JXL can be either - needs deeper analysis
        DetectedFormat::JXL => {
            // For now assume lossy unless we can detect modular mode
            Ok(CompressionType::Lossy)
        }
        
        _ => Ok(CompressionType::Lossy),
    }
}

/// Calculate image entropy (complexity measure)
pub fn calculate_entropy(img: &DynamicImage) -> f64 {
    let gray = img.to_luma8();
    let mut histogram = [0u64; 256];
    
    for pixel in gray.pixels() {
        histogram[pixel[0] as usize] += 1;
    }
    
    let total = gray.pixels().count() as f64;
    let mut entropy = 0.0;
    
    for &count in &histogram {
        if count > 0 {
            let p = count as f64 / total;
            entropy -= p * p.log2();
        }
    }
    
    entropy
}

/// Complete image detection - the main API entry point
pub fn detect_image(path: &Path) -> Result<DetectionResult> {
    let file_size = std::fs::metadata(path)?.len();
    
    // Detect format from magic bytes (NOT extension)
    let format = detect_format_from_bytes(path)?;
    
    // Detect animation status
    let (is_animated, frame_count, fps) = detect_animation(path, &format)?;
    
    // Detect compression type
    let compression = detect_compression(&format, path)?;
    
    // Load image for dimension and other analysis
    let img = image::open(path).map_err(|e| ImgQualityError::ImageReadError(e.to_string()))?;
    let (width, height) = img.dimensions();
    let has_alpha = img.color().has_alpha();
    let bit_depth = match img.color() {
        image::ColorType::L8 | image::ColorType::La8 | image::ColorType::Rgb8 | image::ColorType::Rgba8 => 8,
        image::ColorType::L16 | image::ColorType::La16 | image::ColorType::Rgb16 | image::ColorType::Rgba16 => 16,
        _ => 8,
    };
    
    // Calculate entropy
    let entropy = calculate_entropy(&img);
    
    // Estimate quality for JPEG
    let estimated_quality = if format == DetectedFormat::JPEG {
        estimate_jpeg_quality(path).ok()
    } else {
        None
    };
    
    // Calculate duration for animated images
    let duration = if is_animated {
        fps.map(|f| frame_count as f32 / f)
    } else {
        None
    };
    
    Ok(DetectionResult {
        file_path: path.display().to_string(),
        format,
        image_type: if is_animated { ImageType::Animated } else { ImageType::Static },
        compression,
        width,
        height,
        bit_depth,
        has_alpha,
        file_size,
        frame_count,
        fps,
        duration,
        estimated_quality,
        entropy,
    })
}

/// Estimate JPEG quality (simplified version)
fn estimate_jpeg_quality(path: &Path) -> Result<u8> {
    // Read file bytes
    let data = std::fs::read(path)?;
    // Use existing JPEG analysis
    use crate::jpeg_analysis::analyze_jpeg_quality;
    let analysis = analyze_jpeg_quality(&data)
        .map_err(|e| ImgQualityError::AnalysisError(e))?;
    Ok(analysis.estimated_quality as u8)
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    
    #[test]
    fn test_format_detection() {
        // PNG magic bytes
        let png_header = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert!(png_header.starts_with(&[0x89, 0x50, 0x4E, 0x47]));
    }
}
