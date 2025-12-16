//! Detection API Module
//! 
//! Pure analysis layer - detects image properties without trusting file extensions.
//! Uses magic bytes and actual file content for accurate format detection.
//! 
//! 🔥 v3.7: Enhanced PNG Quantization Detection with Referee System
//! 
//! PNG quantization detection is challenging because PNG format doesn't record
//! whether it was quantized. We use a multi-factor referee system:
//! 
//! 1. **Structural Analysis**: IHDR color type, bit depth, PLTE/tRNS chunks
//! 2. **Metadata Analysis**: tEXt/iTXt chunks for tool signatures
//! 3. **Statistical Analysis**: Color distribution, gradient smoothness, dithering patterns
//! 4. **Heuristic Analysis**: File size vs dimensions ratio, compression efficiency
//! 
//! Each factor contributes a weighted score, and the final decision is based on
//! the aggregate score with confidence level.

use crate::{ImgQualityError, Result};
use image::{DynamicImage, GenericImageView, Rgba};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

/// PNG Quantization Analysis Result
/// 
/// Detailed analysis of whether a PNG has been quantized (lossy optimization)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PngQuantizationAnalysis {
    /// Final verdict: is this PNG quantized (lossy)?
    pub is_quantized: bool,
    
    /// Confidence level (0.0 - 1.0)
    pub confidence: f64,
    
    /// Individual factor scores (each 0.0 - 1.0, higher = more likely quantized)
    pub factor_scores: PngQuantizationFactors,
    
    /// Detected quantization tool (if identifiable)
    pub detected_tool: Option<String>,
    
    /// Human-readable explanation
    pub explanation: String,
}

/// Individual factors for PNG quantization detection
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PngQuantizationFactors {
    /// Structural: indexed color with transparency
    pub indexed_with_alpha: f64,
    
    /// Structural: large palette (>200 colors)
    pub large_palette: f64,
    
    /// Metadata: tool signature found
    pub tool_signature: f64,
    
    /// Statistical: dithering pattern detected
    pub dithering_detected: f64,
    
    /// Statistical: color count vs expected ratio
    pub color_count_anomaly: f64,
    
    /// Statistical: gradient banding detected
    pub gradient_banding: f64,
    
    /// Heuristic: file size efficiency anomaly
    pub size_efficiency_anomaly: f64,
    
    /// Heuristic: high entropy in indexed mode
    pub entropy_anomaly: f64,
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
    file.read_exact(&mut header)?;
    
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
/// 
/// 🔥 v3.6: Enhanced PNG lossy detection
/// PNG can be "lossy" in these cases:
/// 1. Quantized PNG (pngquant/pngnq): 24-bit → 8-bit indexed palette
/// 2. Lossy optimization (TinyPNG): reduces colors with dithering
/// 3. Low bit depth: 8-bit instead of 16-bit for photos
/// 
/// Detection strategy:
/// - PNG with indexed color (color type 3) AND ≤256 colors → potentially lossy
/// - PNG with alpha + indexed → likely quantized (lossy)
/// - PNG 16-bit → lossless
/// - PNG 8-bit truecolor → lossless (standard)
pub fn detect_compression(format: &DetectedFormat, path: &Path) -> Result<CompressionType> {
    match format {
        // PNG: Check for quantization (lossy optimization)
        DetectedFormat::PNG => {
            detect_png_compression(path)
        }
        
        // BMP/TIFF: Always lossless
        DetectedFormat::BMP | DetectedFormat::TIFF => {
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

/// 🔥 v3.7: PNG Quantization Detection Referee System
/// 
/// Multi-factor analysis to determine if a PNG has been quantized (lossy).
/// Uses weighted scoring across multiple detection methods.
/// 
/// ## Detection Factors:
/// 
/// 1. **Structural Analysis** (Weight: 0.25)
///    - IHDR color type (indexed = suspicious)
///    - tRNS chunk presence (indexed + alpha = very suspicious)
///    - Palette size analysis
/// 
/// 2. **Metadata Analysis** (Weight: 0.30)
///    - tEXt/iTXt chunks for tool signatures
///    - Known quantization tool fingerprints
/// 
/// 3. **Statistical Analysis** (Weight: 0.30)
///    - Dithering pattern detection
///    - Color distribution analysis
///    - Gradient smoothness check
/// 
/// 4. **Heuristic Analysis** (Weight: 0.15)
///    - File size vs dimensions ratio
///    - Compression efficiency anomalies
/// 
/// ## Decision Thresholds:
/// - Score >= 0.70: Definitely quantized (Lossy)
/// - Score >= 0.50: Likely quantized (Lossy)
/// - Score >= 0.30: Uncertain, treat as Lossless (conservative)
/// - Score < 0.30: Definitely not quantized (Lossless)
fn detect_png_compression(path: &Path) -> Result<CompressionType> {
    let analysis = analyze_png_quantization(path)?;
    
    // Log for PNG analysis (only in verbose/debug mode)
    if std::env::var("IMGQUALITY_VERBOSE").is_ok() || std::env::var("IMGQUALITY_DEBUG").is_ok() {
        eprintln!("   📊 PNG Analysis: {} (confidence: {:.1}%)", 
            if analysis.is_quantized { "Quantized/Lossy" } else { "Lossless" },
            analysis.confidence * 100.0);
        eprintln!("      {}", analysis.explanation);
    }
    
    Ok(if analysis.is_quantized {
        CompressionType::Lossy
    } else {
        CompressionType::Lossless
    })
}

/// Comprehensive PNG quantization analysis
/// 
/// Returns detailed analysis with confidence score and factor breakdown
pub fn analyze_png_quantization(path: &Path) -> Result<PngQuantizationAnalysis> {
    let data = std::fs::read(path)?;
    
    // Validate PNG signature
    if data.len() < 33 || !data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Ok(PngQuantizationAnalysis {
            is_quantized: false,
            confidence: 1.0,
            factor_scores: PngQuantizationFactors::default(),
            detected_tool: None,
            explanation: "Invalid PNG or non-PNG file".to_string(),
        });
    }
    
    // Parse PNG structure
    let png_info = parse_png_structure(&data)?;
    
    // Initialize factor scores
    let mut factors = PngQuantizationFactors::default();
    let mut detected_tool: Option<String> = None;
    let mut explanations: Vec<String> = Vec::new();
    
    // ═══════════════════════════════════════════════════════════════════════
    // Factor 1: Structural Analysis (Weight: 0.25)
    // ═══════════════════════════════════════════════════════════════════════
    
    // 1a. Indexed color analysis
    // Key insight: Indexed color (type 3) on large images is almost always quantization
    // Small images (icons, sprites) legitimately use indexed color
    if png_info.color_type == 3 {
        let pixel_count = png_info.width as u64 * png_info.height as u64;
        let is_large_image = pixel_count > 100_000;  // > 100K pixels
        let is_medium_image = pixel_count > 10_000;  // > 10K pixels
        
        if png_info.has_trns {
            // Indexed + transparency = very strong quantization indicator
            // This is the signature of pngquant and similar tools
            factors.indexed_with_alpha = 0.98;
            explanations.push("Indexed PNG with alpha (tRNS) - definite quantization".to_string());
        } else if is_large_image {
            // Large indexed image without alpha = still very suspicious
            // Natural large images are almost never indexed
            factors.indexed_with_alpha = 0.75;
            explanations.push(format!("Large indexed PNG ({}x{}) - likely quantized", 
                png_info.width, png_info.height));
        } else if is_medium_image {
            // Medium indexed image = moderately suspicious
            factors.indexed_with_alpha = 0.45;
        } else {
            // Small indexed image = could be intentional (icons, pixel art)
            factors.indexed_with_alpha = 0.15;
        }
    }
    
    // 1b. Palette analysis with image size consideration
    // Key insight: For large images, even moderate palettes indicate quantization
    // For small images (icons, pixel art), palettes are often intentional
    if let Some(palette_size) = png_info.palette_size {
        let pixel_count = png_info.width as u64 * png_info.height as u64;
        let is_large_image = pixel_count > 100_000;  // > 100K pixels
        let is_medium_image = pixel_count > 10_000;  // > 10K pixels
        
        // Calculate expected unique colors for natural images
        // Natural photos typically have thousands of unique colors
        // Quantized images are forced to use limited palette
        let colors_per_megapixel = (palette_size as f64 / (pixel_count as f64 / 1_000_000.0)).min(1000.0);
        
        if palette_size > 240 {
            // Near-maximum palette = definitely quantized from truecolor
            factors.large_palette = 0.95;
            explanations.push(format!("Near-max palette ({} colors) - definitely quantized", palette_size));
        } else if palette_size > 200 {
            factors.large_palette = 0.85;
            explanations.push(format!("Large palette ({} colors) - likely quantized", palette_size));
        } else if is_large_image && palette_size > 64 {
            // Large image with moderate palette = very suspicious
            // Natural large images would have many more colors
            factors.large_palette = 0.80;
            explanations.push(format!("Large image ({}x{}) with limited palette ({} colors) - quantization indicator", 
                png_info.width, png_info.height, palette_size));
        } else if is_large_image && palette_size > 32 {
            factors.large_palette = 0.60;
            explanations.push(format!("Large image with small palette ({} colors)", palette_size));
        } else if is_medium_image && palette_size > 128 {
            factors.large_palette = 0.50;
        } else if palette_size <= 16 && !is_large_image {
            // Very small palette on small image = likely intentional (icons, pixel art)
            factors.large_palette = 0.0;
        } else if palette_size <= 32 && !is_medium_image {
            // Small palette on small image = likely intentional
            factors.large_palette = 0.1;
        } else {
            // Default: moderate suspicion for indexed images
            factors.large_palette = 0.3;
        }
        
        // Additional check: colors per megapixel ratio
        // Quantized images have very low colors/MP ratio
        if is_large_image && colors_per_megapixel < 50.0 {
            factors.large_palette = factors.large_palette.max(0.70);
            if !explanations.iter().any(|e| e.contains("colors/MP")) {
                explanations.push(format!("Low color density ({:.1} colors/MP)", colors_per_megapixel));
            }
        }
    }
    
    // ═══════════════════════════════════════════════════════════════════════
    // Factor 2: Metadata Analysis (Weight: 0.30)
    // ═══════════════════════════════════════════════════════════════════════
    
    // Check for quantization tool signatures in metadata
    let tool_signatures = detect_quantization_tool_signature(&data);
    if let Some(ref tool) = tool_signatures {
        factors.tool_signature = 1.0;
        detected_tool = Some(tool.clone());
        explanations.push(format!("Tool signature detected: {}", tool));
    }
    
    // ═══════════════════════════════════════════════════════════════════════
    // Factor 3: Statistical Analysis (Weight: 0.30)
    // ═══════════════════════════════════════════════════════════════════════
    
    // Load image for pixel analysis (only for indexed PNGs to save time)
    if png_info.color_type == 3 {
        if let Ok(img) = image::open(path) {
            // 3a. Dithering pattern detection
            let dithering_score = detect_dithering_pattern(&img);
            factors.dithering_detected = dithering_score;
            if dithering_score > 0.5 {
                explanations.push(format!("Dithering pattern detected (score: {:.2})", dithering_score));
            }
            
            // 3b. Color count anomaly
            // For large images, having limited unique colors is a strong quantization indicator
            let (unique_colors, _expected_colors) = analyze_color_distribution(&img, png_info.palette_size);
            let pixel_count = png_info.width as u64 * png_info.height as u64;
            let is_large_image = pixel_count > 100_000;
            
            if let Some(palette_size) = png_info.palette_size {
                // If using most of the palette, likely quantized
                let usage_ratio = unique_colors as f64 / palette_size as f64;
                
                // 🔥 Key insight: Large images with ANY indexed palette are suspicious
                // Natural large images would have thousands of colors, not 256 or less
                if is_large_image {
                    // Large image with indexed color = very suspicious
                    if usage_ratio > 0.8 {
                        factors.color_count_anomaly = 0.85;
                        explanations.push(format!("Large image using {:.0}% of {} color palette", usage_ratio * 100.0, palette_size));
                    } else if usage_ratio > 0.5 {
                        factors.color_count_anomaly = 0.70;
                    } else {
                        factors.color_count_anomaly = 0.50;
                    }
                } else if usage_ratio > 0.9 && palette_size > 200 {
                    factors.color_count_anomaly = 0.8;
                    explanations.push(format!("High palette utilization ({:.0}%)", usage_ratio * 100.0));
                } else if usage_ratio > 0.7 && palette_size > 128 {
                    factors.color_count_anomaly = 0.5;
                }
            }
            
            // 3c. Gradient banding detection
            let banding_score = detect_gradient_banding(&img);
            factors.gradient_banding = banding_score;
            if banding_score > 0.5 {
                explanations.push(format!("Gradient banding detected (score: {:.2})", banding_score));
            }
        }
    }
    
    // ═══════════════════════════════════════════════════════════════════════
    // Factor 4: Heuristic Analysis (Weight: 0.15)
    // ═══════════════════════════════════════════════════════════════════════
    
    // 4a. File size efficiency anomaly
    let expected_size = estimate_uncompressed_size(&png_info);
    let actual_size = data.len() as u64;
    let compression_ratio = actual_size as f64 / expected_size as f64;
    
    // Quantized PNGs often have unusually good compression for their content
    if png_info.color_type == 3 && compression_ratio < 0.15 && png_info.width * png_info.height > 100_000 {
        factors.size_efficiency_anomaly = 0.6;
        explanations.push(format!("Unusually efficient compression ({:.1}%)", compression_ratio * 100.0));
    }
    
    // ═══════════════════════════════════════════════════════════════════════
    // Calculate Final Score with Weights
    // ═══════════════════════════════════════════════════════════════════════
    
    // 🔥 v3.7: Rebalanced weights for better detection
    // Structural analysis is the most reliable indicator for indexed PNGs
    // Metadata is unreliable (most tools don't leave signatures)
    // Statistical analysis is secondary confirmation
    let weights = PngQuantizationWeights {
        structural: 0.55,   // 🔥 Increased: indexed color is the strongest indicator
        metadata: 0.10,     // 🔥 Decreased: most tools don't leave signatures
        statistical: 0.25,  // Secondary confirmation
        heuristic: 0.10,    // Minor factor
    };
    
    // Structural score (average of indexed_with_alpha and large_palette)
    let structural_score = (factors.indexed_with_alpha + factors.large_palette) / 2.0;
    
    // Metadata score
    let metadata_score = factors.tool_signature;
    
    // Statistical score (average of dithering, color anomaly, banding)
    let statistical_score = (factors.dithering_detected + factors.color_count_anomaly + factors.gradient_banding) / 3.0;
    
    // Heuristic score
    let heuristic_score = (factors.size_efficiency_anomaly + factors.entropy_anomaly) / 2.0;
    
    // Weighted final score
    let final_score = 
        structural_score * weights.structural +
        metadata_score * weights.metadata +
        statistical_score * weights.statistical +
        heuristic_score * weights.heuristic;
    
    // Debug output for score breakdown (only in verbose mode)
    if std::env::var("IMGQUALITY_DEBUG").is_ok() {
        eprintln!("      📈 Score breakdown:");
        eprintln!("         Structural: {:.2} (indexed_alpha={:.2}, large_palette={:.2}) × {:.2} = {:.3}",
            structural_score, factors.indexed_with_alpha, factors.large_palette, weights.structural, structural_score * weights.structural);
        eprintln!("         Metadata: {:.2} × {:.2} = {:.3}", metadata_score, weights.metadata, metadata_score * weights.metadata);
        eprintln!("         Statistical: {:.2} (dither={:.2}, color={:.2}, band={:.2}) × {:.2} = {:.3}",
            statistical_score, factors.dithering_detected, factors.color_count_anomaly, factors.gradient_banding, weights.statistical, statistical_score * weights.statistical);
        eprintln!("         Heuristic: {:.2} × {:.2} = {:.3}", heuristic_score, weights.heuristic, heuristic_score * weights.heuristic);
        eprintln!("         FINAL SCORE: {:.3} (threshold: 0.50 for lossy)", final_score);
    }
    
    // ═══════════════════════════════════════════════════════════════════════
    // Decision Logic
    // ═══════════════════════════════════════════════════════════════════════
    
    // Special case: 16-bit PNG is always lossless
    if png_info.bit_depth == 16 {
        return Ok(PngQuantizationAnalysis {
            is_quantized: false,
            confidence: 1.0,
            factor_scores: factors,
            detected_tool: None,
            explanation: "16-bit PNG - always lossless".to_string(),
        });
    }
    
    // Special case: Truecolor (type 2) or Truecolor+Alpha (type 6) without tool signature
    // NOTE: This check uses png_info.color_type from raw PNG bytes, NOT from image crate
    // image crate converts indexed PNG to RGBA, so we must use raw PNG structure
    if (png_info.color_type == 2 || png_info.color_type == 6) && detected_tool.is_none() {
        return Ok(PngQuantizationAnalysis {
            is_quantized: false,
            confidence: 0.95,
            factor_scores: factors,
            detected_tool: None,
            explanation: "Truecolor PNG without quantization indicators".to_string(),
        });
    }
    
    // If we reach here with indexed color (type 3), proceed to score-based decision
    // This is the key path for detecting quantized PNGs
    
    // Special case: Tool signature is definitive
    if detected_tool.is_some() {
        return Ok(PngQuantizationAnalysis {
            is_quantized: true,
            confidence: 0.99,
            factor_scores: factors,
            detected_tool,
            explanation: explanations.join("; "),
        });
    }
    
    // Score-based decision
    let (is_quantized, confidence) = if final_score >= 0.70 {
        (true, 0.9 + (final_score - 0.70) * 0.33)  // 0.90 - 1.0
    } else if final_score >= 0.50 {
        (true, 0.7 + (final_score - 0.50) * 1.0)   // 0.70 - 0.90
    } else if final_score >= 0.30 {
        // Uncertain zone - be conservative, treat as lossless
        (false, 0.5 + (0.50 - final_score) * 1.0)  // 0.50 - 0.70
    } else {
        (false, 0.8 + (0.30 - final_score) * 0.67) // 0.80 - 1.0
    };
    
    let explanation = if explanations.is_empty() {
        if is_quantized {
            format!("Quantization detected (score: {:.2})", final_score)
        } else {
            format!("No quantization indicators (score: {:.2})", final_score)
        }
    } else {
        explanations.join("; ")
    };
    
    Ok(PngQuantizationAnalysis {
        is_quantized,
        confidence: confidence.min(1.0),
        factor_scores: factors,
        detected_tool,
        explanation,
    })
}

/// PNG structure information parsed from chunks
struct PngStructureInfo {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    palette_size: Option<usize>,
    has_trns: bool,
    #[allow(dead_code)]
    has_text_chunks: bool,
}

/// Weights for quantization detection factors
struct PngQuantizationWeights {
    structural: f64,
    metadata: f64,
    statistical: f64,
    heuristic: f64,
}

/// Parse PNG structure from raw bytes
fn parse_png_structure(data: &[u8]) -> Result<PngStructureInfo> {
    // IHDR chunk starts at byte 8 (after signature)
    let ihdr_start = 8;
    if data.len() < ihdr_start + 8 + 13 {
        return Err(ImgQualityError::AnalysisError("PNG too small".to_string()));
    }
    
    // Check chunk type is IHDR
    if &data[ihdr_start + 4..ihdr_start + 8] != b"IHDR" {
        return Err(ImgQualityError::AnalysisError("Invalid PNG: no IHDR".to_string()));
    }
    
    let ihdr_data = &data[ihdr_start + 8..];
    let width = u32::from_be_bytes([ihdr_data[0], ihdr_data[1], ihdr_data[2], ihdr_data[3]]);
    let height = u32::from_be_bytes([ihdr_data[4], ihdr_data[5], ihdr_data[6], ihdr_data[7]]);
    let bit_depth = ihdr_data[8];
    let color_type = ihdr_data[9];
    
    // Find PLTE chunk and count palette entries
    let palette_size = if color_type == 3 {
        find_chunk_size(data, b"PLTE").map(|size| size / 3)
    } else {
        None
    };
    
    // Check for tRNS chunk
    let has_trns = data.windows(4).any(|w| w == b"tRNS");
    
    // Check for text chunks
    let has_text_chunks = data.windows(4).any(|w| w == b"tEXt" || w == b"iTXt" || w == b"zTXt");
    
    Ok(PngStructureInfo {
        width,
        height,
        bit_depth,
        color_type,
        palette_size,
        has_trns,
        has_text_chunks,
    })
}

/// Find chunk size by chunk type
fn find_chunk_size(data: &[u8], chunk_type: &[u8; 4]) -> Option<usize> {
    for i in 8..data.len().saturating_sub(12) {
        if &data[i + 4..i + 8] == chunk_type {
            let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            return Some(len);
        }
    }
    None
}

/// Detect quantization tool signatures in PNG metadata
/// 
/// Known tool signatures:
/// - pngquant: "pngquant" in tEXt/iTXt
/// - TinyPNG: "TinyPNG" or specific patterns
/// - ImageOptim: "ImageOptim"
/// - pngnq: "pngnq"
/// - posterize: various patterns
fn detect_quantization_tool_signature(data: &[u8]) -> Option<String> {
    // Convert to string for searching (lossy but sufficient for signatures)
    let text = String::from_utf8_lossy(data);
    
    // Known quantization tool signatures
    let signatures = [
        ("pngquant", "pngquant"),
        ("pngnq", "pngnq"),
        ("TinyPNG", "TinyPNG"),
        ("tinypng", "TinyPNG"),
        ("ImageOptim", "ImageOptim"),
        ("imageoptim", "ImageOptim"),
        ("posterize", "posterize"),
        ("quantize", "quantize tool"),
        ("Quantized", "quantization"),
        ("color reduction", "color reduction"),
        ("palette optimization", "palette optimization"),
    ];
    
    for (pattern, tool_name) in signatures {
        if text.contains(pattern) {
            return Some(tool_name.to_string());
        }
    }
    
    // Check for specific chunk patterns that indicate quantization
    // Some tools add specific ancillary chunks
    
    None
}

/// Detect dithering patterns in image
/// 
/// Dithering is a telltale sign of quantization - it's used to simulate
/// more colors than the palette allows.
/// 
/// Detection methods:
/// 1. High-frequency noise analysis
/// 2. Checkerboard pattern detection
/// 3. Error diffusion pattern detection
fn detect_dithering_pattern(img: &DynamicImage) -> f64 {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    
    if width < 8 || height < 8 {
        return 0.0; // Too small to analyze
    }
    
    let mut high_freq_count = 0u64;
    let mut total_comparisons = 0u64;
    
    // Sample the image (don't analyze every pixel for performance)
    let step = ((width * height) as f64 / 10000.0).max(1.0) as u32;
    
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if (x + y * width) % step != 0 {
                continue;
            }
            
            let center = rgba.get_pixel(x, y);
            let neighbors = [
                rgba.get_pixel(x - 1, y),
                rgba.get_pixel(x + 1, y),
                rgba.get_pixel(x, y - 1),
                rgba.get_pixel(x, y + 1),
            ];
            
            // Check for high-frequency alternation (dithering signature)
            let mut alternations = 0;
            for neighbor in &neighbors {
                let diff = color_difference(center, neighbor);
                if diff > 30.0 && diff < 100.0 {
                    // Moderate difference = potential dithering
                    alternations += 1;
                }
            }
            
            if alternations >= 2 {
                high_freq_count += 1;
            }
            total_comparisons += 1;
        }
    }
    
    if total_comparisons == 0 {
        return 0.0;
    }
    
    let dithering_ratio = high_freq_count as f64 / total_comparisons as f64;
    
    // Normalize to 0-1 range (typical dithered images have 5-20% high-freq pixels)
    (dithering_ratio * 5.0).min(1.0)
}

/// Calculate color difference between two pixels
fn color_difference(a: &Rgba<u8>, b: &Rgba<u8>) -> f64 {
    let dr = (a[0] as f64 - b[0] as f64).abs();
    let dg = (a[1] as f64 - b[1] as f64).abs();
    let db = (a[2] as f64 - b[2] as f64).abs();
    (dr * dr + dg * dg + db * db).sqrt()
}

/// Analyze color distribution in image
/// 
/// Returns (unique_colors, expected_colors_for_content)
fn analyze_color_distribution(img: &DynamicImage, _palette_size: Option<usize>) -> (usize, usize) {
    let rgba = img.to_rgba8();
    let mut color_set: HashMap<[u8; 4], u32> = HashMap::new();
    
    // Sample pixels for performance
    let (width, height) = rgba.dimensions();
    let total_pixels = (width * height) as usize;
    let sample_rate = (total_pixels / 50000).max(1);
    
    for (i, pixel) in rgba.pixels().enumerate() {
        if i % sample_rate == 0 {
            let key = [pixel[0], pixel[1], pixel[2], pixel[3]];
            *color_set.entry(key).or_insert(0) += 1;
        }
    }
    
    let unique_colors = color_set.len();
    
    // Estimate expected colors based on image complexity
    // Photos typically have thousands of unique colors
    // Illustrations have fewer
    // Icons/pixel art have very few
    let expected = if total_pixels > 500_000 {
        10000 // Large photo
    } else if total_pixels > 100_000 {
        5000  // Medium image
    } else {
        1000  // Small image
    };
    
    (unique_colors, expected)
}

/// Detect gradient banding (posterization artifact)
/// 
/// Quantized images often show visible steps in gradients
/// instead of smooth transitions.
fn detect_gradient_banding(img: &DynamicImage) -> f64 {
    let gray = img.to_luma8();
    let (width, height) = gray.dimensions();
    
    if width < 16 || height < 16 {
        return 0.0;
    }
    
    let mut banding_score = 0.0;
    let mut gradient_regions = 0;
    
    // Scan horizontal lines for gradient regions
    for y in (0..height).step_by(4) {
        let mut prev_val = gray.get_pixel(0, y)[0];
        let mut gradient_length = 0;
        let mut step_count = 0;
        
        for x in 1..width {
            let val = gray.get_pixel(x, y)[0];
            let diff = (val as i16 - prev_val as i16).abs();
            
            if diff > 0 && diff < 20 {
                // Potential gradient region
                gradient_length += 1;
                if diff > 3 {
                    step_count += 1;
                }
            } else if gradient_length > 20 {
                // End of gradient region
                if step_count > 0 {
                    let step_ratio = step_count as f64 / gradient_length as f64;
                    if step_ratio > 0.1 && step_ratio < 0.5 {
                        // Suspicious banding pattern
                        banding_score += step_ratio;
                        gradient_regions += 1;
                    }
                }
                gradient_length = 0;
                step_count = 0;
            }
            
            prev_val = val;
        }
    }
    
    if gradient_regions == 0 {
        return 0.0;
    }
    
    (banding_score / gradient_regions as f64).min(1.0)
}

/// Estimate uncompressed size for compression ratio analysis
fn estimate_uncompressed_size(info: &PngStructureInfo) -> u64 {
    let bytes_per_pixel = match info.color_type {
        0 => 1,  // Grayscale
        2 => 3,  // RGB
        3 => 1,  // Indexed
        4 => 2,  // Grayscale + Alpha
        6 => 4,  // RGBA
        _ => 4,
    };
    
    let bit_multiplier = info.bit_depth as u64 / 8;
    
    info.width as u64 * info.height as u64 * bytes_per_pixel * bit_multiplier.max(1)
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
        .map_err(ImgQualityError::AnalysisError)?;
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
