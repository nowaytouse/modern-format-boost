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

use crate::img_errors::{ImgQualityError, Result};
use image::{DynamicImage, GenericImageView, Rgba};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageType {
    Static,
    Animated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionType {
    Lossless,
    Lossy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PngQuantizationAnalysis {
    pub is_quantized: bool,

    pub confidence: f64,

    pub factor_scores: PngQuantizationFactors,

    pub detected_tool: Option<String>,

    pub explanation: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PngQuantizationFactors {
    pub indexed_with_alpha: f64,

    pub large_palette: f64,

    pub tool_signature: f64,

    pub dithering_detected: f64,

    pub color_count_anomaly: f64,

    pub gradient_banding: f64,

    pub size_efficiency_anomaly: f64,

    pub entropy_anomaly: f64,
}

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

    pub fn is_modern_format(&self) -> bool {
        matches!(
            self,
            DetectedFormat::HEIC
                | DetectedFormat::HEIF
                | DetectedFormat::AVIF
                | DetectedFormat::JXL
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    pub file_path: String,

    pub format: DetectedFormat,

    pub image_type: ImageType,

    pub compression: CompressionType,

    pub width: u32,
    pub height: u32,

    pub bit_depth: u8,

    pub has_alpha: bool,

    pub file_size: u64,

    pub frame_count: u32,

    pub fps: Option<f32>,

    pub duration: Option<f32>,

    pub estimated_quality: Option<u8>,

    pub entropy: f64,
}

pub fn detect_format_from_bytes(path: &Path) -> Result<DetectedFormat> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 32];
    file.read_exact(&mut header)?;

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

    if header.starts_with(&[0xFF, 0x0A]) {
        return Ok(DetectedFormat::JXL);
    }
    if header.starts_with(&[0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20]) {
        return Ok(DetectedFormat::JXL);
    }

    if header.starts_with(&[0x49, 0x49, 0x2A, 0x00])
        || header.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
    {
        return Ok(DetectedFormat::TIFF);
    }

    if header.starts_with(b"BM") {
        return Ok(DetectedFormat::BMP);
    }

    Ok(DetectedFormat::Unknown("Unknown format".to_string()))
}

pub fn detect_animation(path: &Path, format: &DetectedFormat) -> Result<(bool, u32, Option<f32>)> {
    match format {
        DetectedFormat::GIF => {
            crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
                .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let frame_count = crate::image_formats::gif::count_frames_from_bytes(&data);
            let is_animated = frame_count > 1;
            let fps = if is_animated { Some(10.0) } else { None };
            Ok((is_animated, frame_count, fps))
        }
        DetectedFormat::WebP => {
            crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
                .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let is_animated = crate::image_formats::webp::is_animated_from_bytes(&data);
            let frame_count = if is_animated {
                crate::image_formats::webp::count_frames_from_bytes(&data)
            } else {
                1
            };
            let fps = if is_animated { Some(24.0) } else { None };
            Ok((is_animated, frame_count, fps))
        }
        DetectedFormat::PNG => {
            crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
                .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let is_animated = data.windows(4).any(|w| w == b"acTL");
            Ok((is_animated, if is_animated { 2 } else { 1 }, None))
        }
        _ => Ok((false, 1, None)),
    }
}

pub fn detect_compression(format: &DetectedFormat, path: &Path) -> Result<CompressionType> {
    match format {
        DetectedFormat::PNG => detect_png_compression(path),

        DetectedFormat::BMP | DetectedFormat::TIFF => Ok(CompressionType::Lossless),

        DetectedFormat::JPEG => Ok(CompressionType::Lossy),

        DetectedFormat::GIF => Ok(CompressionType::Lossless),

        DetectedFormat::WebP => {
            crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
                .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let is_lossless = crate::image_formats::webp::is_lossless_from_bytes(&data);
            Ok(if is_lossless {
                CompressionType::Lossless
            } else {
                CompressionType::Lossy
            })
        }

        DetectedFormat::HEIC | DetectedFormat::HEIF | DetectedFormat::AVIF => {
            Ok(CompressionType::Lossy)
        }

        DetectedFormat::JXL => {
            Ok(CompressionType::Lossy)
        }

        _ => Ok(CompressionType::Lossy),
    }
}

fn detect_png_compression(path: &Path) -> Result<CompressionType> {
    let analysis = analyze_png_quantization(path)?;

    if std::env::var("IMGQUALITY_VERBOSE").is_ok() || std::env::var("IMGQUALITY_DEBUG").is_ok() {
        eprintln!(
            "   📊 PNG Analysis: {} (confidence: {:.1}%)",
            if analysis.is_quantized {
                "Quantized/Lossy"
            } else {
                "Lossless"
            },
            analysis.confidence * 100.0
        );
        eprintln!("      {}", analysis.explanation);
    }

    Ok(if analysis.is_quantized {
        CompressionType::Lossy
    } else {
        CompressionType::Lossless
    })
}

pub fn analyze_png_quantization(path: &Path) -> Result<PngQuantizationAnalysis> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
    let data = std::fs::read(path)?;

    if data.len() < 33 || !data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Ok(PngQuantizationAnalysis {
            is_quantized: false,
            confidence: 1.0,
            factor_scores: PngQuantizationFactors::default(),
            detected_tool: None,
            explanation: "Invalid PNG or non-PNG file".to_string(),
        });
    }

    let png_info = parse_png_structure(&data)?;

    let mut factors = PngQuantizationFactors::default();
    let mut detected_tool: Option<String> = None;
    let mut explanations: Vec<String> = Vec::new();


    if png_info.color_type == 3 {
        let pixel_count = png_info.width as u64 * png_info.height as u64;
        let is_large_image = pixel_count > 100_000;
        let is_medium_image = pixel_count > 10_000;

        if png_info.has_trns {
            factors.indexed_with_alpha = 0.98;
            explanations.push("Indexed PNG with alpha (tRNS) - definite quantization".to_string());
        } else if is_large_image {
            factors.indexed_with_alpha = 0.75;
            explanations.push(format!(
                "Large indexed PNG ({}x{}) - likely quantized",
                png_info.width, png_info.height
            ));
        } else if is_medium_image {
            factors.indexed_with_alpha = 0.45;
        } else {
            factors.indexed_with_alpha = 0.15;
        }
    }

    if let Some(palette_size) = png_info.palette_size {
        let pixel_count = png_info.width as u64 * png_info.height as u64;
        let is_large_image = pixel_count > 100_000;
        let is_medium_image = pixel_count > 10_000;

        let colors_per_megapixel =
            (palette_size as f64 / (pixel_count as f64 / 1_000_000.0)).min(1000.0);

        if palette_size > 240 {
            factors.large_palette = 0.95;
            explanations.push(format!(
                "Near-max palette ({} colors) - definitely quantized",
                palette_size
            ));
        } else if palette_size > 200 {
            factors.large_palette = 0.85;
            explanations.push(format!(
                "Large palette ({} colors) - likely quantized",
                palette_size
            ));
        } else if is_large_image && palette_size > 64 {
            factors.large_palette = 0.80;
            explanations.push(format!(
                "Large image ({}x{}) with limited palette ({} colors) - quantization indicator",
                png_info.width, png_info.height, palette_size
            ));
        } else if is_large_image && palette_size > 32 {
            factors.large_palette = 0.60;
            explanations.push(format!(
                "Large image with small palette ({} colors)",
                palette_size
            ));
        } else if is_medium_image && palette_size > 128 {
            factors.large_palette = 0.50;
        } else if palette_size <= 16 && !is_large_image {
            factors.large_palette = 0.0;
        } else if palette_size <= 32 && !is_medium_image {
            factors.large_palette = 0.1;
        } else {
            factors.large_palette = 0.3;
        }

        if is_large_image && colors_per_megapixel < 50.0 {
            factors.large_palette = factors.large_palette.max(0.70);
            if !explanations.iter().any(|e| e.contains("colors/MP")) {
                explanations.push(format!(
                    "Low color density ({:.1} colors/MP)",
                    colors_per_megapixel
                ));
            }
        }
    }


    let tool_signatures = detect_quantization_tool_signature(&data);
    if let Some(ref tool) = tool_signatures {
        factors.tool_signature = 1.0;
        detected_tool = Some(tool.clone());
        explanations.push(format!("Tool signature detected: {}", tool));
    }


    if png_info.color_type == 3 {
        if let Ok(img) = image::open(path) {
            let dithering_score = detect_dithering_pattern(&img);
            factors.dithering_detected = dithering_score;
            if dithering_score > 0.5 {
                explanations.push(format!(
                    "Dithering pattern detected (score: {:.2})",
                    dithering_score
                ));
            }

            let (unique_colors, _expected_colors) =
                analyze_color_distribution(&img, png_info.palette_size);
            let pixel_count = png_info.width as u64 * png_info.height as u64;
            let is_large_image = pixel_count > 100_000;

            if let Some(palette_size) = png_info.palette_size {
                let usage_ratio = unique_colors as f64 / palette_size as f64;

                if is_large_image {
                    if usage_ratio > 0.8 {
                        factors.color_count_anomaly = 0.85;
                        explanations.push(format!(
                            "Large image using {:.0}% of {} color palette",
                            usage_ratio * 100.0,
                            palette_size
                        ));
                    } else if usage_ratio > 0.5 {
                        factors.color_count_anomaly = 0.70;
                    } else {
                        factors.color_count_anomaly = 0.50;
                    }
                } else if usage_ratio > 0.9 && palette_size > 200 {
                    factors.color_count_anomaly = 0.8;
                    explanations.push(format!(
                        "High palette utilization ({:.0}%)",
                        usage_ratio * 100.0
                    ));
                } else if usage_ratio > 0.7 && palette_size > 128 {
                    factors.color_count_anomaly = 0.5;
                }
            }

            let banding_score = detect_gradient_banding(&img);
            factors.gradient_banding = banding_score;
            if banding_score > 0.5 {
                explanations.push(format!(
                    "Gradient banding detected (score: {:.2})",
                    banding_score
                ));
            }
        }
    }


    let expected_size = estimate_uncompressed_size(&png_info);
    let actual_size = data.len() as u64;
    let compression_ratio = actual_size as f64 / expected_size as f64;

    if png_info.color_type == 3
        && compression_ratio < 0.15
        && png_info.width * png_info.height > 100_000
    {
        factors.size_efficiency_anomaly = 0.6;
        explanations.push(format!(
            "Unusually efficient compression ({:.1}%)",
            compression_ratio * 100.0
        ));
    }


    let weights = PngQuantizationWeights {
        structural: 0.55,
        metadata: 0.10,
        statistical: 0.25,
        heuristic: 0.10,
    };

    let structural_score = (factors.indexed_with_alpha + factors.large_palette) / 2.0;

    let metadata_score = factors.tool_signature;

    let statistical_score =
        (factors.dithering_detected + factors.color_count_anomaly + factors.gradient_banding) / 3.0;

    let heuristic_score = (factors.size_efficiency_anomaly + factors.entropy_anomaly) / 2.0;

    let final_score = structural_score * weights.structural
        + metadata_score * weights.metadata
        + statistical_score * weights.statistical
        + heuristic_score * weights.heuristic;

    if std::env::var("IMGQUALITY_DEBUG").is_ok() {
        eprintln!("      📈 Score breakdown:");
        eprintln!(
            "         Structural: {:.2} (indexed_alpha={:.2}, large_palette={:.2}) × {:.2} = {:.3}",
            structural_score,
            factors.indexed_with_alpha,
            factors.large_palette,
            weights.structural,
            structural_score * weights.structural
        );
        eprintln!(
            "         Metadata: {:.2} × {:.2} = {:.3}",
            metadata_score,
            weights.metadata,
            metadata_score * weights.metadata
        );
        eprintln!(
            "         Statistical: {:.2} (dither={:.2}, color={:.2}, band={:.2}) × {:.2} = {:.3}",
            statistical_score,
            factors.dithering_detected,
            factors.color_count_anomaly,
            factors.gradient_banding,
            weights.statistical,
            statistical_score * weights.statistical
        );
        eprintln!(
            "         Heuristic: {:.2} × {:.2} = {:.3}",
            heuristic_score,
            weights.heuristic,
            heuristic_score * weights.heuristic
        );
        eprintln!(
            "         FINAL SCORE: {:.3} (threshold: 0.50 for lossy)",
            final_score
        );
    }


    if png_info.bit_depth == 16 {
        return Ok(PngQuantizationAnalysis {
            is_quantized: false,
            confidence: 1.0,
            factor_scores: factors,
            detected_tool: None,
            explanation: "16-bit PNG - always lossless".to_string(),
        });
    }

    if (png_info.color_type == 2 || png_info.color_type == 6) && detected_tool.is_none() {
        return Ok(PngQuantizationAnalysis {
            is_quantized: false,
            confidence: 0.95,
            factor_scores: factors,
            detected_tool: None,
            explanation: "Truecolor PNG without quantization indicators".to_string(),
        });
    }


    if detected_tool.is_some() {
        return Ok(PngQuantizationAnalysis {
            is_quantized: true,
            confidence: 0.99,
            factor_scores: factors,
            detected_tool,
            explanation: explanations.join("; "),
        });
    }

    let (is_quantized, confidence) = if final_score >= 0.70 {
        (true, 0.9 + (final_score - 0.70) * 0.33)
    } else if final_score >= 0.50 {
        (true, 0.7 + (final_score - 0.50) * 1.0)
    } else if final_score >= 0.30 {
        (false, 0.5 + (0.50 - final_score) * 1.0)
    } else {
        (false, 0.8 + (0.30 - final_score) * 0.67)
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

struct PngQuantizationWeights {
    structural: f64,
    metadata: f64,
    statistical: f64,
    heuristic: f64,
}

fn parse_png_structure(data: &[u8]) -> Result<PngStructureInfo> {
    let ihdr_start = 8;
    if data.len() < ihdr_start + 8 + 13 {
        return Err(ImgQualityError::AnalysisError("PNG too small".to_string()));
    }

    if &data[ihdr_start + 4..ihdr_start + 8] != b"IHDR" {
        return Err(ImgQualityError::AnalysisError(
            "Invalid PNG: no IHDR".to_string(),
        ));
    }

    let ihdr_data = &data[ihdr_start + 8..];
    let width = u32::from_be_bytes([ihdr_data[0], ihdr_data[1], ihdr_data[2], ihdr_data[3]]);
    let height = u32::from_be_bytes([ihdr_data[4], ihdr_data[5], ihdr_data[6], ihdr_data[7]]);
    let bit_depth = ihdr_data[8];
    let color_type = ihdr_data[9];

    let palette_size = if color_type == 3 {
        find_chunk_size(data, b"PLTE").map(|size| size / 3)
    } else {
        None
    };

    let has_trns = data.windows(4).any(|w| w == b"tRNS");

    let has_text_chunks = data
        .windows(4)
        .any(|w| w == b"tEXt" || w == b"iTXt" || w == b"zTXt");

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

fn find_chunk_size(data: &[u8], chunk_type: &[u8; 4]) -> Option<usize> {
    for i in 8..data.len().saturating_sub(12) {
        if &data[i + 4..i + 8] == chunk_type {
            let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            return Some(len);
        }
    }
    None
}

fn detect_quantization_tool_signature(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);

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


    None
}

fn detect_dithering_pattern(img: &DynamicImage) -> f64 {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    if width < 8 || height < 8 {
        return 0.0;
    }

    let mut high_freq_count = 0u64;
    let mut total_comparisons = 0u64;

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

            let mut alternations = 0;
            for neighbor in &neighbors {
                let diff = color_difference(center, neighbor);
                if diff > 30.0 && diff < 100.0 {
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

    (dithering_ratio * 5.0).min(1.0)
}

fn color_difference(a: &Rgba<u8>, b: &Rgba<u8>) -> f64 {
    let dr = (a[0] as f64 - b[0] as f64).abs();
    let dg = (a[1] as f64 - b[1] as f64).abs();
    let db = (a[2] as f64 - b[2] as f64).abs();
    (dr * dr + dg * dg + db * db).sqrt()
}

fn analyze_color_distribution(img: &DynamicImage, _palette_size: Option<usize>) -> (usize, usize) {
    let rgba = img.to_rgba8();
    let mut color_set: HashMap<[u8; 4], u32> = HashMap::new();

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

    let expected = if total_pixels > 500_000 {
        10000
    } else if total_pixels > 100_000 {
        5000
    } else {
        1000
    };

    (unique_colors, expected)
}

fn detect_gradient_banding(img: &DynamicImage) -> f64 {
    let gray = img.to_luma8();
    let (width, height) = gray.dimensions();

    if width < 16 || height < 16 {
        return 0.0;
    }

    let mut banding_score = 0.0;
    let mut gradient_regions = 0;

    for y in (0..height).step_by(4) {
        let mut prev_val = gray.get_pixel(0, y)[0];
        let mut gradient_length = 0;
        let mut step_count = 0;

        for x in 1..width {
            let val = gray.get_pixel(x, y)[0];
            let diff = (val as i16 - prev_val as i16).abs();

            if diff > 0 && diff < 20 {
                gradient_length += 1;
                if diff > 3 {
                    step_count += 1;
                }
            } else if gradient_length > 20 {
                if step_count > 0 {
                    let step_ratio = step_count as f64 / gradient_length as f64;
                    if step_ratio > 0.1 && step_ratio < 0.5 {
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

fn estimate_uncompressed_size(info: &PngStructureInfo) -> u64 {
    let bytes_per_pixel = match info.color_type {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => 4,
    };

    let bit_multiplier = info.bit_depth as u64 / 8;

    info.width as u64 * info.height as u64 * bytes_per_pixel * bit_multiplier.max(1)
}

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

pub fn detect_image(path: &Path) -> Result<DetectionResult> {
    let file_size = std::fs::metadata(path)?.len();

    let format = detect_format_from_bytes(path)?;

    let (is_animated, frame_count, fps) = detect_animation(path, &format)?;

    let compression = detect_compression(&format, path)?;

    let img = image::open(path).map_err(|e| ImgQualityError::ImageReadError(e.to_string()))?;
    let (width, height) = img.dimensions();
    let has_alpha = img.color().has_alpha();
    let bit_depth = match img.color() {
        image::ColorType::L8
        | image::ColorType::La8
        | image::ColorType::Rgb8
        | image::ColorType::Rgba8 => 8,
        image::ColorType::L16
        | image::ColorType::La16
        | image::ColorType::Rgb16
        | image::ColorType::Rgba16 => 16,
        _ => 8,
    };

    let entropy = calculate_entropy(&img);

    let estimated_quality = if format == DetectedFormat::JPEG {
        estimate_jpeg_quality(path).ok()
    } else {
        None
    };

    let duration = if is_animated {
        fps.map(|f| frame_count as f32 / f)
    } else {
        None
    };

    Ok(DetectionResult {
        file_path: path.display().to_string(),
        format,
        image_type: if is_animated {
            ImageType::Animated
        } else {
            ImageType::Static
        },
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

fn estimate_jpeg_quality(path: &Path) -> Result<u8> {
    let data = std::fs::read(path)?;
    use crate::image_jpeg_analysis::analyze_jpeg_quality;
    let analysis = analyze_jpeg_quality(&data).map_err(ImgQualityError::AnalysisError)?;
    Ok(analysis.estimated_quality as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;


    #[test]
    fn test_detect_png_format() {
        let png_magic: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        let mut data = png_magic.to_vec();
        data.extend_from_slice(&[0u8; 24]);
        file.write_all(&data).expect("写入失败");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "PNG 格式检测应该成功");
        assert_eq!(result.unwrap(), DetectedFormat::PNG, "应该检测为 PNG 格式");
    }

    #[test]
    fn test_detect_jpeg_format() {
        let jpeg_magic: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0];
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        let mut data = jpeg_magic.to_vec();
        data.extend_from_slice(&[0u8; 28]);
        file.write_all(&data).expect("写入失败");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "JPEG 格式检测应该成功");
        assert_eq!(
            result.unwrap(),
            DetectedFormat::JPEG,
            "应该检测为 JPEG 格式"
        );
    }

    #[test]
    fn test_detect_gif_format() {
        let gif_magic: &[u8] = b"GIF89a";
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        let mut data = gif_magic.to_vec();
        data.extend_from_slice(&[0u8; 26]);
        file.write_all(&data).expect("写入失败");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "GIF 格式检测应该成功");
        assert_eq!(result.unwrap(), DetectedFormat::GIF, "应该检测为 GIF 格式");
    }

    #[test]
    fn test_detect_webp_format() {
        let mut webp_data = b"RIFF".to_vec();
        webp_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        webp_data.extend_from_slice(b"WEBP");
        webp_data.extend_from_slice(&[0u8; 20]);

        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        file.write_all(&webp_data).expect("写入失败");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "WebP 格式检测应该成功");
        assert_eq!(
            result.unwrap(),
            DetectedFormat::WebP,
            "应该检测为 WebP 格式"
        );
    }

    #[test]
    fn test_detect_unknown_format() {
        let random_data: &[u8] = &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05];
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        let mut data = random_data.to_vec();
        data.extend_from_slice(&[0u8; 26]);
        file.write_all(&data).expect("写入失败");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "未知格式检测应该成功（返回 Unknown）");
        match result.unwrap() {
            DetectedFormat::Unknown(_) => (),
            other => panic!("应该检测为 Unknown 格式，实际为 {:?}", other),
        }
    }

    #[test]
    fn test_detect_nonexistent_file() {
        let result = detect_format_from_bytes(std::path::Path::new("/nonexistent/file.png"));
        assert!(result.is_err(), "不存在的文件应该返回错误");
    }
}
