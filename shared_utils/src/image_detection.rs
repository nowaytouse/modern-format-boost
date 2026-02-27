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
//!
//! ## Reliability and limitations
//!
//! - **PNG "lossy"** here means *palette-quantized* (e.g. pngquant, TinyPNG). 16-bit and
//!   truecolor PNG without tool signature are treated as lossless. Indexed PNG uses a
//!   **conservative threshold 0.58**: only scores ≥0.58 are marked lossy; gray zone
//!   [0.40, 0.58] is treated as lossless to reduce false positives (e.g. natural palette art).
//!   Heuristic score includes **palette-index frequency entropy** for indexed images and
//!   **per-channel RGB entropy** for others. Tool signatures include zTXt decompression.
//!   We do *not* detect "PNG exported from a lossy source" (e.g. JPEG→PNG screenshot).
//! - **WebP**: VP8L vs VP8 chunk; animated WebP traverses all ANMF frames (any VP8→lossy).
//! - **TIFF**: Compression tag (259) across ALL IFDs; JPEG (6,7)→lossy, others→lossless.
//!   Supports both standard TIFF and BigTIFF (0x002B). No tag → assumed lossless.
//! - **AVIF**: Multi-dimension (av1C chroma 4:2:0/4:2:2→lossy; 4:4:4 + colr Identity MC u16[8..9]/pixi/high bit depth→lossless; 4:4:4 ambiguous→Err). Err when av1C missing or 4:4:4 without definitive indicators.
//! - **HEIC**: Multi-dimension (hvcC chromaFormatIdc 4:2:0/4:2:2→lossy; Main/Main10/MSP→lossy; RExt/SCC + 4:4:4→lossless; RExt without 4:4:4→Err). Err when hvcC missing.
//! - **JXL**: Container jbrd box→lossless (naked codestream skips jbrd scan); codestream xyb_encoded→lossy/modular; Err only when no jbrd and header unparseable.
//! - **JPEG**: Always lossy; JXL transcoding does not require quality judgment.
//! - **EXR**: Parses compression attribute (NONE/RLE/ZIPS/ZIP/PIZ→lossless; PXR24/B44/B44A/DWAA/DWAB→lossy).
//! - **QOI, FLIF, PNM**: Treated as lossless. **JP2**: COD marker wavelet transform (9/7 irreversible→lossy, 5/3 reversible→lossless); fallback lossy if COD not found.
//! - **ICO**: Parses directory entries; embedded PNG checked for quantization (tRNS + indexed, tool signatures). BMP/DIB entries → lossless.
//! - **TGA, PSD, DDS**: Treated as lossless.
//! - **Format detection**: `mif1`/`msf1` major brand scans compatible_brands to disambiguate AVIF vs HEIC.
//!
//! ## Quality judgment reliability audit (conclusion)
//!
//! **Overall**: Format-by-format parsing + multi-dimension container/codestream logic; Err only when
//! key boxes/headers are missing (AVIF/HEIC/JXL). PNG uses a scored heuristic with conservative
//! gray zone; no format silently "guesses" lossy when uncertain — either deterministic or Err.
//!
//! | Format | Reliability | Deterministic? | When uncertain |
//! |--------|-------------|----------------|----------------|
//! | PNG    | Medium–High | No (score)     | Gray zone [0.40,0.58] → lossless; palette-index entropy + zTXt signatures. |
//! | WebP   | High        | Yes (VP8L/VP8)| Animated: traverses all ANMF frames. |
//! | TIFF   | High        | Yes (tag 259) | All IFDs + BigTIFF. No tag → lossless. |
//! | JPEG   | N/A         | Yes (always)  | Always lossy. |
//! | AVIF   | High        | Multi (av1C)  | Err if no av1C or ambiguous 4:4:4. colr MC u16 fix. |
//! | HEIC   | High        | Multi (hvcC)  | chromaFormatIdc + profile. Err if no hvcC or RExt w/o 4:4:4. |
//! | JXL    | High        | Multi (jbrd/xyb)| Container-only jbrd. Err if unparseable. |
//! | GIF    | Assumed     | N/A           | Treated as lossless. |
//! | EXR    | High        | Yes (attr)    | Parses compression attr. No attr → lossless. |
//! | QOI/FLIF/PNM | Assumed | N/A        | Treated as lossless. |
//! | JP2    | High        | Yes (COD wavelet)| Fallback lossy if COD not found. |
//! | ICO    | Medium      | Partial       | Embedded PNG checked for quantization. |
//! | TGA/PSD/DDS | Assumed | N/A         | Treated as lossless. |
//!
//! **Call chain**: `analyze_image` → format (HEIC/JXL/AVIF/…) → `detect_lossless` / `detect_compression` → `Result<CompressionType>`.  
//! **Error propagation**: AVIF/HEIC/JXL `Err` propagates via `?` in `analyze_heic_image`, `analyze_jxl_image`, and `detect_lossless`; conversion path fails loudly with path in message.

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
    // Additional formats — "can not use, but can't not have"
    QOI,
    JP2,
    ICO,
    TGA,
    EXR,
    FLIF,
    PSD,
    PNM,
    DDS,
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
            DetectedFormat::QOI => "QOI",
            DetectedFormat::JP2 => "JP2",
            DetectedFormat::ICO => "ICO",
            DetectedFormat::TGA => "TGA",
            DetectedFormat::EXR => "EXR",
            DetectedFormat::FLIF => "FLIF",
            DetectedFormat::PSD => "PSD",
            DetectedFormat::PNM => "PNM",
            DetectedFormat::DDS => "DDS",
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
        // AVIF brands (still + sequence) — check before HEIC since mif1 can be either
        if brand == b"avif" || brand == b"avis" || brand == b"MA1B" || brand == b"MA1A" {
            return Ok(DetectedFormat::AVIF);
        }
        // HEIC/HEVC-based brands (incl. sequence variants)
        if brand == b"heic" || brand == b"heix" || brand == b"heim" || brand == b"heis"
            || brand == b"hevc" || brand == b"hevx"
            || brand == b"hev1"
        {
            return Ok(DetectedFormat::HEIC);
        }
        if brand == b"heif" {
            return Ok(DetectedFormat::HEIF);
        }
        // Generic ISOBMFF brands — major brand is ambiguous, scan compatible_brands
        if brand == b"mif1" || brand == b"msf1" {
            return Ok(resolve_mif1_from_compatible_brands(path, brand));
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
        // BigTIFF: II + 0x002B or MM + 0x2B00
        || header.starts_with(&[0x49, 0x49, 0x2B, 0x00])
        || header.starts_with(&[0x4D, 0x4D, 0x00, 0x2B])
    {
        return Ok(DetectedFormat::TIFF);
    }

    if header.starts_with(b"BM") {
        return Ok(DetectedFormat::BMP);
    }

    // QOI: "qoif" magic
    if header.starts_with(b"qoif") {
        return Ok(DetectedFormat::QOI);
    }

    // JPEG 2000: 0x0000000C 6A502020 0D0A870A
    if header.len() >= 12
        && header[0..4] == [0x00, 0x00, 0x00, 0x0C]
        && header[4..8] == *b"jP  "
    {
        return Ok(DetectedFormat::JP2);
    }
    // JPEG 2000 codestream: FF 4F FF 51
    if header.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        return Ok(DetectedFormat::JP2);
    }

    // ICO: 00 00 01 00 (icon) or 00 00 02 00 (cursor)
    if header.starts_with(&[0x00, 0x00, 0x01, 0x00])
        || header.starts_with(&[0x00, 0x00, 0x02, 0x00])
    {
        return Ok(DetectedFormat::ICO);
    }

    // OpenEXR: 76 2F 31 01
    if header.starts_with(&[0x76, 0x2F, 0x31, 0x01]) {
        return Ok(DetectedFormat::EXR);
    }

    // FLIF: "FLIF"
    if header.starts_with(b"FLIF") {
        return Ok(DetectedFormat::FLIF);
    }

    // PSD: "8BPS"
    if header.starts_with(b"8BPS") {
        return Ok(DetectedFormat::PSD);
    }

    // PNM family: P1-P6 followed by whitespace
    if header.len() >= 2
        && header[0] == b'P'
        && header[1] >= b'1'
        && header[1] <= b'6'
        && (header.len() < 3 || header[2].is_ascii_whitespace())
    {
        return Ok(DetectedFormat::PNM);
    }

    // DDS: "DDS " (0x20534444)
    if header.starts_with(b"DDS ") {
        return Ok(DetectedFormat::DDS);
    }

    Ok(DetectedFormat::Unknown("Unknown format".to_string()))
}

/// Resolve mif1/msf1 major brand by scanning compatible_brands in the ftyp box.
/// AVIF spec allows mif1 as major brand; without this, such files get routed to
/// detect_heic_compression (hvcC lookup) which always fails → Err.
fn resolve_mif1_from_compatible_brands(path: &Path, major_brand: &[u8]) -> DetectedFormat {
    // Read enough for ftyp box (typically < 64 bytes, but can be larger)
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(_) => {
            // Fallback: mif1 without readable file → HEIC (legacy behavior)
            return DetectedFormat::HEIC;
        }
    };

    if data.len() < 16 || &data[4..8] != b"ftyp" {
        return DetectedFormat::HEIC;
    }

    let box_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let ftyp_end = box_size.min(data.len());

    // compatible_brands start at offset 16 (after size[4] + "ftyp"[4] + major_brand[4] + minor_version[4])
    if ftyp_end < 16 {
        return DetectedFormat::HEIC;
    }

    let compat_data = &data[16..ftyp_end];
    let mut has_heic = false;
    let mut has_heif = false;

    for chunk in compat_data.chunks_exact(4) {
        let cb: &[u8; 4] = chunk.try_into().unwrap();
        // AVIF-specific brands — highest priority
        if cb == b"avif" || cb == b"avis" || cb == b"MA1B" || cb == b"MA1A" {
            return DetectedFormat::AVIF;
        }
        if cb == b"heic" || cb == b"heix" || cb == b"hevc" || cb == b"hev1" {
            has_heic = true;
        }
        if cb == b"heif" {
            has_heif = true;
        }
    }

    if has_heic {
        DetectedFormat::HEIC
    } else if has_heif {
        DetectedFormat::HEIF
    } else {
        // mif1/msf1 with no recognizable compatible brands
        DetectedFormat::Unknown(format!(
            "ISOBMFF {} without recognizable compatible brands",
            String::from_utf8_lossy(major_brand)
        ))
    }
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
            let (is_animated, frame_count) = parse_apng_frames(&data);
            Ok((is_animated, frame_count, None))
        }
        _ => Ok((false, 1, None)),
    }
}

pub fn detect_compression(format: &DetectedFormat, path: &Path) -> Result<CompressionType> {
    match format {
        DetectedFormat::PNG => detect_png_compression(path),

        DetectedFormat::BMP => Ok(CompressionType::Lossless),

        DetectedFormat::TIFF => detect_tiff_compression(path),

        // JPEG is always lossy. For JXL transcoding, quality judgment is optional (lossless re-encode);
        // we keep detection available so callers can use it if needed ("can add, can not use, but can't not have").
        DetectedFormat::JPEG => Ok(CompressionType::Lossy),

        DetectedFormat::GIF => Ok(CompressionType::Lossless),

        DetectedFormat::WebP => {
            crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
                .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;

            // For animated WebP, each ANMF frame can independently use VP8 or VP8L.
            // Any VP8 (lossy) frame → entire file is lossy.
            if crate::image_formats::webp::is_animated_from_bytes(&data) {
                return Ok(detect_webp_animation_compression(&data));
            }

            let is_lossless = crate::image_formats::webp::is_lossless_from_bytes(&data);
            Ok(if is_lossless {
                CompressionType::Lossless
            } else {
                CompressionType::Lossy
            })
        }

        DetectedFormat::HEIC | DetectedFormat::HEIF => detect_heic_compression(path),

        DetectedFormat::AVIF => detect_avif_compression(path),

        DetectedFormat::JXL => detect_jxl_compression(path),

        // Additional formats — "can not use, but can't not have"
        DetectedFormat::QOI | DetectedFormat::FLIF | DetectedFormat::PNM => {
            Ok(CompressionType::Lossless)
        }
        DetectedFormat::EXR => detect_exr_compression(path),
        DetectedFormat::JP2 => detect_jp2_compression(path),
        DetectedFormat::ICO => detect_ico_compression(path),
        DetectedFormat::TGA | DetectedFormat::PSD | DetectedFormat::DDS => {
            Ok(CompressionType::Lossless)
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

            // Entropy factor: for indexed PNG, use palette-index frequency distribution
            // which directly measures how evenly palette entries are used.
            // Quantized images tend to have uneven usage (few dominant colors),
            // while natural palette art uses entries more uniformly.
            let (entropy, max_entropy, entropy_ratio) = if png_info.palette_size.is_some() {
                let pe = calculate_palette_index_entropy(&img, png_info.palette_size.unwrap());
                (pe.0, pe.1, pe.2)
            } else {
                // Fallback to RGB channel entropy
                let e = calculate_rgb_entropy(&img);
                let ps = 256.0f64;
                let me = ps.log2();
                let ratio = if me > 0.0 { e / me } else { 0.0 };
                (e, me, ratio)
            };
            let palette_size = png_info.palette_size.unwrap_or(256) as f64;
            // Palette-index entropy ratio is the primary indicator for indexed PNG:
            // quantized images have uneven palette usage → low ratio.
            if palette_size >= 64.0 && entropy_ratio < 0.6 && pixel_count > 10_000 {
                // Strong quantization signal: large palette but uneven usage
                factors.entropy_anomaly = 0.5 + (0.6 - entropy_ratio) * 0.5;
                factors.entropy_anomaly = factors.entropy_anomaly.clamp(0.0, 0.75);
                if factors.entropy_anomaly > 0.4 {
                    explanations.push(format!(
                        "Low palette entropy ratio ({:.2}, max {:.2}) — quantization indicator",
                        entropy_ratio, 1.0
                    ));
                }
            } else if palette_size >= 128.0 && entropy < 5.0 && pixel_count > 10_000 {
                factors.entropy_anomaly = 0.5 + (5.0 - entropy) * 0.08;
                factors.entropy_anomaly = factors.entropy_anomaly.clamp(0.0, 0.7);
                if factors.entropy_anomaly > 0.4 {
                    explanations.push(format!(
                        "Low entropy ({:.2} vs max {:.2}) — quantization indicator",
                        entropy, max_entropy
                    ));
                }
            } else if palette_size >= 64.0 && entropy_ratio < 0.5 && pixel_count > 5_000 {
                factors.entropy_anomaly = 0.35;
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

    const LOSSY_THRESHOLD: f64 = 0.58;
    const GRAY_ZONE_LOW: f64 = 0.40;

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
            "         FINAL SCORE: {:.3} (threshold: {:.2} for lossy, gray zone: [{:.2}, {:.2}] → lossless)",
            final_score, LOSSY_THRESHOLD, GRAY_ZONE_LOW, LOSSY_THRESHOLD
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

    // Conservative strategy: only mark as lossy when confidence is high.
    // Gray zone [0.40, 0.58] without tool signature → treat as lossless to avoid
    // false positives (e.g. natural palette art misclassified as quantized).
    let (is_quantized, confidence) = if final_score >= 0.70 {
        (true, 0.9 + (final_score - 0.70) * 0.33)
    } else if final_score >= LOSSY_THRESHOLD {
        (true, 0.7 + (final_score - LOSSY_THRESHOLD) * 1.0)
    } else if final_score >= GRAY_ZONE_LOW {
        // Gray zone: no tool signature → lossless (conservative)
        (false, 0.5 + (LOSSY_THRESHOLD - final_score) * 1.0)
    } else if final_score >= 0.30 {
        (false, 0.5 + (LOSSY_THRESHOLD - final_score) * 1.0)
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

    // Walk PNG chunks properly (length + type + data + CRC) to avoid false positives
    // from matching chunk type bytes inside compressed pixel data.
    let mut palette_size: Option<usize> = None;
    let mut has_trns = false;
    let mut has_text_chunks = false;

    let mut pos = 8; // skip PNG signature
    while pos + 12 <= data.len() {
        let chunk_len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let chunk_type = &data[pos + 4..pos + 8];

        if chunk_type == b"PLTE" && color_type == 3 {
            palette_size = Some(chunk_len / 3);
        } else if chunk_type == b"tRNS" {
            has_trns = true;
        } else if chunk_type == b"tEXt" || chunk_type == b"iTXt" || chunk_type == b"zTXt" {
            has_text_chunks = true;
        } else if chunk_type == b"IEND" {
            break;
        }

        // next chunk: 4 (length) + 4 (type) + chunk_len (data) + 4 (CRC)
        pos += 12 + chunk_len;
    }

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

/// Scan only PNG text chunks (tEXt/iTXt/zTXt) for quantization tool signatures.
/// Previous version scanned the entire file as UTF-8, which could false-positive
/// on compressed pixel data containing signature bytes.
/// zTXt chunks are zlib-decompressed before matching.
fn detect_quantization_tool_signature(data: &[u8]) -> Option<String> {
    let signatures: &[(&str, &str)] = &[
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
        ("Squoosh", "Squoosh"),
        ("squoosh", "Squoosh"),
        ("sharp", "sharp"),
        ("libvips", "sharp/libvips"),
        ("pngcrush", "pngcrush"),
        ("PNGOUT", "PNGOUT"),
        ("pngout", "PNGOUT"),
        ("Fireworks", "Adobe Fireworks"),
        ("Adobe Fireworks", "Adobe Fireworks"),
        ("Sketch", "Sketch"),
        ("com.bohemiancoding", "Sketch"),
    ];

    let match_signatures = |text: &str| -> Option<String> {
        for &(pattern, tool_name) in signatures {
            if text.contains(pattern) {
                return Some(tool_name.to_string());
            }
        }
        None
    };

    // Walk PNG chunks and only inspect text chunk payloads
    let mut pos = 8; // skip PNG signature
    while pos + 12 <= data.len() {
        let chunk_len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let chunk_type = &data[pos + 4..pos + 8];
        let payload_start = pos + 8;
        let payload_end = payload_start + chunk_len;

        if payload_end <= data.len() {
            if chunk_type == b"tEXt" || chunk_type == b"iTXt" {
                let text = String::from_utf8_lossy(&data[payload_start..payload_end]);
                if let Some(tool) = match_signatures(&text) {
                    return Some(tool);
                }
            } else if chunk_type == b"zTXt" {
                // zTXt: keyword\0 + compression_method(1) + compressed_text
                // Decompress the value portion to match signatures
                let payload = &data[payload_start..payload_end];
                // Find null terminator after keyword
                if let Some(null_pos) = payload.iter().position(|&b| b == 0) {
                    // Check keyword itself (uncompressed)
                    let keyword = String::from_utf8_lossy(&payload[..null_pos]);
                    if let Some(tool) = match_signatures(&keyword) {
                        return Some(tool);
                    }
                    // Decompress value: skip keyword\0 + method byte (1)
                    if null_pos + 2 < payload.len() {
                        let compressed = &payload[null_pos + 2..];
                        if let Ok(decompressed) =
                            flate2::read::ZlibDecoder::new(compressed)
                                .bytes()
                                .collect::<std::result::Result<Vec<u8>, _>>()
                        {
                            let text = String::from_utf8_lossy(&decompressed);
                            if let Some(tool) = match_signatures(&text) {
                                return Some(tool);
                            }
                        }
                    }
                }
            }
        }

        if chunk_type == b"IEND" {
            break;
        }
        pos += 12 + chunk_len;
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
            // 8-neighbor check: cardinal + diagonal catches Floyd-Steinberg diagonal artifacts
            let neighbors = [
                rgba.get_pixel(x - 1, y),
                rgba.get_pixel(x + 1, y),
                rgba.get_pixel(x, y - 1),
                rgba.get_pixel(x, y + 1),
                rgba.get_pixel(x - 1, y - 1),
                rgba.get_pixel(x + 1, y - 1),
                rgba.get_pixel(x - 1, y + 1),
                rgba.get_pixel(x + 1, y + 1),
            ];

            let mut alternations = 0;
            for neighbor in &neighbors {
                let diff = color_difference(center, neighbor);
                if diff > 30.0 && diff < 100.0 {
                    alternations += 1;
                }
            }

            if alternations >= 3 {
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

/// Perceptually weighted color difference (Compuphase approximation).
/// Human vision: green > red > blue sensitivity. Equal-weight Euclidean RGB
/// under-weights green differences and over-weights blue, causing dithering
/// detection to miss green-channel artifacts and false-trigger on blue noise.
fn color_difference(a: &Rgba<u8>, b: &Rgba<u8>) -> f64 {
    let rmean = (a[0] as f64 + b[0] as f64) / 2.0;
    let dr = a[0] as f64 - b[0] as f64;
    let dg = a[1] as f64 - b[1] as f64;
    let db = a[2] as f64 - b[2] as f64;
    // Weights shift with mean red: redder pixels → more red weight, bluer → more blue weight
    let wr = 2.0 + rmean / 256.0;
    let wg = 4.0;
    let wb = 2.0 + (255.0 - rmean) / 256.0;
    (wr * dr * dr + wg * dg * dg + wb * db * db).sqrt()
}

/// Block-based random sampling — divides image into grid cells and randomly samples from each,
/// avoiding the systematic bias of stride sampling (which creates periodic blind spots on
/// structured images like game UI screenshots). Quantized images have concentrated color
/// distributions; stride sampling can miss local color clusters.
fn analyze_color_distribution(img: &DynamicImage, _palette_size: Option<usize>) -> (usize, usize) {
    let rgba = img.to_rgba8();
    let mut color_set: HashMap<[u8; 4], u32> = HashMap::new();

    let (width, height) = rgba.dimensions();
    let total_pixels = (width * height) as usize;

    // Target ~50k samples, distributed across a grid of blocks
    let target_samples: usize = 50_000;
    let grid_size: u32 = 16; // 16x16 = 256 blocks
    let block_w = (width / grid_size).max(1);
    let block_h = (height / grid_size).max(1);
    let samples_per_block = (target_samples / (grid_size * grid_size) as usize).max(1);

    // Simple LCG for deterministic pseudo-random sampling (no need for rand crate)
    let mut rng_state: u64 = 0x123456789ABCDEF0;
    let lcg_next = |state: &mut u64| -> u32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (*state >> 32) as u32
    };

    for by in 0..grid_size {
        for bx in 0..grid_size {
            let x0 = bx * block_w;
            let y0 = by * block_h;
            let x1 = ((bx + 1) * block_w).min(width);
            let y1 = ((by + 1) * block_h).min(height);
            let block_w_actual = x1 - x0;
            let block_h_actual = y1 - y0;
            let block_pixels = (block_w_actual * block_h_actual) as usize;
            if block_pixels == 0 {
                continue;
            }

            // Random sampling within this block
            let n_samples = samples_per_block.min(block_pixels);
            for _ in 0..n_samples {
                let rand_x = x0 + (lcg_next(&mut rng_state) % block_w_actual);
                let rand_y = y0 + (lcg_next(&mut rng_state) % block_h_actual);
                let pixel = rgba.get_pixel(rand_x, rand_y);
                let key = [pixel[0], pixel[1], pixel[2], pixel[3]];
                *color_set.entry(key).or_insert(0) += 1;
            }
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

    // Horizontal scan (existing)
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

    // Vertical scan — catches vertical gradients that horizontal scan misses
    for x in (0..width).step_by(4) {
        let mut prev_val = gray.get_pixel(x, 0)[0];
        let mut gradient_length = 0;
        let mut step_count = 0;

        for y in 1..height {
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

    // Diagonal scan (top-left to bottom-right) — catches diagonal gradients
    let diag_step = 8; // Sample every 8th diagonal to balance coverage vs performance
    for start_offset in (0..width.max(height)).step_by(diag_step) {
        // Diagonals starting from top edge
        if start_offset < width {
            let mut x = start_offset;
            let mut y = 0u32;
            let mut prev_val = gray.get_pixel(x, y)[0];
            let mut gradient_length = 0;
            let mut step_count = 0;

            while x < width && y < height {
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
                x += 1;
                y += 1;
            }
        }

        // Diagonals starting from left edge
        if start_offset > 0 && start_offset < height {
            let mut x = 0u32;
            let mut y = start_offset;
            let mut prev_val = gray.get_pixel(x, y)[0];
            let mut gradient_length = 0;
            let mut step_count = 0;

            while x < width && y < height {
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
                x += 1;
                y += 1;
            }
        }
    }

    // Diagonal scan (top-right to bottom-left) — catches opposite diagonal gradients
    for start_offset in (0..width.max(height)).step_by(diag_step) {
        // Diagonals starting from top edge
        if start_offset < width {
            let mut x = start_offset;
            let mut y = 0u32;
            let mut prev_val = gray.get_pixel(x, y)[0];
            let mut gradient_length = 0;
            let mut step_count = 0;

            while x > 0 && y < height {
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
                x = x.saturating_sub(1);
                y += 1;
            }
        }

        // Diagonals starting from right edge
        if start_offset > 0 && start_offset < height {
            let mut x = width - 1;
            let mut y = start_offset;
            let mut prev_val = gray.get_pixel(x, y)[0];
            let mut gradient_length = 0;
            let mut step_count = 0;

            while x > 0 && y < height {
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
                x = x.saturating_sub(1);
                y += 1;
            }
        }
    }

    if gradient_regions == 0 {
        return 0.0;
    }

    (banding_score / gradient_regions as f64).min(1.0)
}

fn estimate_uncompressed_size(info: &PngStructureInfo) -> u64 {
    let bits_per_sample: u64 = match info.color_type {
        0 => 1,                    // grayscale: 1 channel
        2 => 3,                    // RGB: 3 channels
        3 => 1,                    // indexed: 1 index per pixel
        4 => 2,                    // grayscale + alpha: 2 channels
        6 => 4,                    // RGBA: 4 channels
        _ => 4,
    };

    // bit_depth applies per sample; for sub-byte depths (1, 2, 4) pixels are packed
    let total_bits = info.width as u64 * info.height as u64 * bits_per_sample * info.bit_depth as u64;
    // Round up to bytes
    (total_bits + 7) / 8
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

/// Per-channel RGB entropy — avoids the grayscale projection problem where
/// perceptually distinct colors (e.g. red vs blue) map to similar luma values,
/// inflating entropy and masking quantization artifacts.
/// Returns the mean of R, G, B channel entropies.
/// Palette-index frequency entropy for indexed PNG.
///
/// Counts how many pixels use each palette index (0..palette_size), computes
/// Shannon entropy H = -Σ freq[i]*log2(freq[i]), and returns (H, max_H, ratio).
/// Quantized images have uneven palette usage (few dominant entries) → low ratio.
/// Natural palette art uses entries more uniformly → ratio close to 1.0.
fn calculate_palette_index_entropy(img: &DynamicImage, palette_size: usize) -> (f64, f64, f64) {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let total = (width as u64 * height as u64) as f64;
    if total == 0.0 || palette_size == 0 {
        return (0.0, 0.0, 0.0);
    }

    // Map each pixel to its nearest palette index by building a color→index lookup.
    // Since we don't have direct access to the raw index buffer through the `image` crate
    // (it decodes to RGBA), we approximate by quantizing to unique RGBA values and counting.
    let mut color_freq: HashMap<[u8; 4], u64> = HashMap::new();
    for pixel in rgba.pixels() {
        let key = [pixel[0], pixel[1], pixel[2], pixel[3]];
        *color_freq.entry(key).or_insert(0) += 1;
    }

    // Compute entropy over the frequency distribution of distinct colors
    let mut entropy = 0.0;
    for &count in color_freq.values() {
        if count > 0 {
            let p = count as f64 / total;
            entropy -= p * p.log2();
        }
    }

    let max_entropy = (palette_size as f64).log2();
    let ratio = if max_entropy > 0.0 {
        entropy / max_entropy
    } else {
        0.0
    };

    (entropy, max_entropy, ratio)
}

fn calculate_rgb_entropy(img: &DynamicImage) -> f64 {
    let rgba = img.to_rgba8();
    let mut hist_r = [0u64; 256];
    let mut hist_g = [0u64; 256];
    let mut hist_b = [0u64; 256];

    for pixel in rgba.pixels() {
        hist_r[pixel[0] as usize] += 1;
        hist_g[pixel[1] as usize] += 1;
        hist_b[pixel[2] as usize] += 1;
    }

    let total = rgba.pixels().count() as f64;

    fn channel_entropy(hist: &[u64; 256], total: f64) -> f64 {
        let mut h = 0.0;
        for &count in hist {
            if count > 0 {
                let p = count as f64 / total;
                h -= p * p.log2();
            }
        }
        h
    }

    let er = channel_entropy(&hist_r, total);
    let eg = channel_entropy(&hist_g, total);
    let eb = channel_entropy(&hist_b, total);

    (er + eg + eb) / 3.0
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
    } else if format == DetectedFormat::WebP && compression == CompressionType::Lossy {
        estimate_webp_quality(path).ok()
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

/// Estimate WebP VP8 quality by parsing the bitstream quantization index.
///
/// VP8 bitstream structure (after VP8 chunk header):
///   [3] frame_tag (frame type + version + show_frame + first_partition_size)
///   [3] start_code (0x9D 0x01 0x2A)
///   [2] width (14 bits) + horizontal_scale (2 bits)
///   [2] height (14 bits) + vertical_scale (2 bits)
///   First partition → quantization_index y_ac_qi (7 bits)
///
/// y_ac_qi range 0-127: lower = higher quality. Map to 0-100 quality scale.
fn estimate_webp_quality(path: &Path) -> Result<u8> {
    let data = std::fs::read(path)?;

    // Find VP8 chunk (lossy WebP)
    let mut pos = 12; // skip RIFF + size + WEBP
    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
        ]) as usize;
        let payload_start = pos + 8;
        let payload_end = (payload_start + chunk_size).min(data.len());

        if chunk_id == b"VP8 " && payload_end > payload_start + 10 {
            let vp8_data = &data[payload_start..payload_end];

            // Skip frame_tag (3 bytes) + start_code (3 bytes) + width/height (4 bytes) = 10 bytes
            if vp8_data.len() < 10 {
                return Err(ImgQualityError::AnalysisError("VP8 data too short".to_string()));
            }

            // Verify start code
            if vp8_data[3..6] != [0x9D, 0x01, 0x2A] {
                return Err(ImgQualityError::AnalysisError("Invalid VP8 start code".to_string()));
            }

            // First partition starts at byte 10
            // y_ac_qi is in the first byte of the quantization parameters
            if vp8_data.len() > 10 {
                let y_ac_qi = (vp8_data[10] & 0x7F) as u8; // 7 bits
                // Map 0-127 to 100-0 quality (lower qi = higher quality)
                let quality = ((127 - y_ac_qi) * 100 / 127).min(100);
                return Ok(quality);
            }
        }

        let padded = (chunk_size + 1) & !1;
        pos = payload_start + padded;
    }

    Err(ImgQualityError::AnalysisError("No VP8 chunk found".to_string()))
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

/// Parse APNG (Animated PNG) frame count from PNG data
/// Returns (is_animated, frame_count)
fn parse_apng_frames(data: &[u8]) -> (bool, u32) {
    // Look for acTL (Animation Control) chunk
    let mut pos = 8; // Skip PNG signature
    while pos + 12 <= data.len() {
        if pos + 4 > data.len() {
            break;
        }

        // Read chunk length (big-endian)
        let length = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        pos += 4;

        if pos + 4 > data.len() {
            break;
        }

        // Read chunk type
        let chunk_type = &data[pos..pos + 4];
        pos += 4;

        // Check if this is acTL chunk
        if chunk_type == b"acTL" {
            if pos + 4 <= data.len() {
                // Read num_frames (first 4 bytes of acTL data)
                let num_frames = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
                return (true, num_frames.max(1));
            }
            return (true, 2); // Fallback if we can't read frame count
        }

        // Skip chunk data and CRC
        pos += length as usize + 4;
    }

    (false, 1)
}

// ============================================================================
// 🔥 Enhanced Format-Specific Lossless Detection
// ============================================================================

/// Detect WebP animated compression by traversing all ANMF (animation frame) chunks.
///
/// WebP animation: RIFF header → VP8X → ANIM → ANMF* frames.
/// Each ANMF payload contains frame data starting with VP8/VP8L/VP8X sub-chunk.
/// Any VP8 (lossy) frame → Lossy. All VP8L → Lossless.
fn detect_webp_animation_compression(data: &[u8]) -> CompressionType {
    // WebP structure: RIFF[size]WEBP[chunks...]
    // Walk top-level chunks to find ANMF frames
    if data.len() < 12 {
        return CompressionType::Lossy;
    }

    let mut pos = 12; // skip RIFF + size + WEBP
    let mut found_any_frame = false;

    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
        ]) as usize;
        let payload_start = pos + 8;
        let payload_end = (payload_start + chunk_size).min(data.len());

        if chunk_id == b"ANMF" && payload_end > payload_start + 24 {
            found_any_frame = true;
            // ANMF payload: 24 bytes header, then frame data sub-chunk
            let frame_data = &data[payload_start + 24..payload_end];
            if frame_data.len() >= 4 {
                // Check sub-chunk type: VP8L = lossless, VP8 = lossy
                if &frame_data[0..4] == b"VP8 " {
                    return CompressionType::Lossy;
                }
                // VP8L is fine, continue checking other frames
            }
        }

        // Chunks are padded to even size
        let padded = (chunk_size + 1) & !1;
        pos = payload_start + padded;
    }

    if found_any_frame {
        CompressionType::Lossless
    } else {
        // No ANMF frames found in animated WebP — fallback
        if data.windows(4).any(|w| w == b"VP8L") {
            CompressionType::Lossless
        } else {
            CompressionType::Lossy
        }
    }
}

/// Detect TIFF compression type — traverses ALL IFDs. Supports both standard TIFF and BigTIFF.
///
/// Multi-page TIFF can have different compression per IFD (e.g. first page LZW, rest JPEG).
/// Any IFD with JPEG compression (6/7) → Lossy. All lossless → Lossless.
///
/// BigTIFF (0x002B magic) uses 8-byte offsets and 8-byte entry counts.
fn detect_tiff_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if data.len() < 8 {
        return Ok(CompressionType::Lossless);
    }

    let is_little_endian = &data[0..2] == b"II";
    if &data[0..2] != b"II" && &data[0..2] != b"MM" {
        return Ok(CompressionType::Lossless);
    }

    let version = if is_little_endian {
        u16::from_le_bytes([data[2], data[3]])
    } else {
        u16::from_be_bytes([data[2], data[3]])
    };
    let is_bigtiff = version == 0x002B;

    // Helper closures for endian-aware reads
    let read_u16 = |off: usize| -> u16 {
        if is_little_endian {
            u16::from_le_bytes([data[off], data[off + 1]])
        } else {
            u16::from_be_bytes([data[off], data[off + 1]])
        }
    };
    let read_u32 = |off: usize| -> u32 {
        if is_little_endian {
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        } else {
            u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        }
    };
    let read_u64 = |off: usize| -> u64 {
        if is_little_endian {
            u64::from_le_bytes([
                data[off], data[off+1], data[off+2], data[off+3],
                data[off+4], data[off+5], data[off+6], data[off+7],
            ])
        } else {
            u64::from_be_bytes([
                data[off], data[off+1], data[off+2], data[off+3],
                data[off+4], data[off+5], data[off+6], data[off+7],
            ])
        }
    };

    // Get first IFD offset
    let mut ifd_offset: u64 = if is_bigtiff {
        // BigTIFF: bytes 4-5 = offset size (always 8), 6-7 = reserved, 8-15 = first IFD offset
        if data.len() < 16 { return Ok(CompressionType::Lossless); }
        read_u64(8)
    } else {
        read_u32(4) as u64
    };

    // Traverse IFD chain (limit to 100 to prevent infinite loops on corrupt files)
    let mut ifd_count = 0u32;
    while ifd_offset != 0 && ifd_count < 100 {
        ifd_count += 1;
        let ifd_pos = ifd_offset as usize;

        // Read number of entries
        let (num_entries, entries_start, entry_size, next_offset_pos) = if is_bigtiff {
            if ifd_pos + 8 > data.len() { break; }
            let n = read_u64(ifd_pos) as usize;
            (n, ifd_pos + 8, 20usize, ifd_pos + 8 + n * 20)
        } else {
            if ifd_pos + 2 > data.len() { break; }
            let n = read_u16(ifd_pos) as usize;
            (n, ifd_pos + 2, 12usize, ifd_pos + 2 + n * 12)
        };

        // Scan entries for Compression tag (259)
        let mut pos = entries_start;
        for _ in 0..num_entries {
            if pos + entry_size > data.len() { break; }

            let tag = read_u16(pos);
            if tag == 259 {
                // Value offset depends on TIFF vs BigTIFF
                let compression = if is_bigtiff {
                    read_u16(pos + 12) // value/offset field starts at byte 12 in BigTIFF entry
                } else {
                    read_u16(pos + 8)
                };

                if std::env::var("IMGQUALITY_VERBOSE").is_ok() || std::env::var("IMGQUALITY_DEBUG").is_ok() {
                    eprintln!("   📊 TIFF IFD#{} Compression: {} ({})",
                        ifd_count, compression,
                        match compression {
                            1 => "No compression",
                            5 => "LZW",
                            6 | 7 => "JPEG (lossy)",
                            8 | 32946 => "Deflate",
                            32773 => "PackBits",
                            50001 => "WebP (lossy)",
                            _ => "Unknown"
                        }
                    );
                }

                // Any lossy IFD → entire file is lossy
                // JPEG (6, 7) and WebP (50001) are lossy
                if compression == 6 || compression == 7 || compression == 50001 {
                    return Ok(CompressionType::Lossy);
                }
            }

            pos += entry_size;
        }

        // Read next IFD offset
        if is_bigtiff {
            if next_offset_pos + 8 > data.len() { break; }
            ifd_offset = read_u64(next_offset_pos);
        } else {
            if next_offset_pos + 4 > data.len() { break; }
            ifd_offset = read_u32(next_offset_pos) as u64;
        }
    }

    // All IFDs scanned, no JPEG compression found
    Ok(CompressionType::Lossless)
}

/// Detect AVIF lossless encoding — multi-dimension analysis.
///
/// Dimensions checked (in priority order):
/// 1. **av1C chroma subsampling**: 4:2:0 / 4:2:2 → definitely lossy (AV1 lossless requires 4:4:4)
/// 2. **av1C 4:4:4 + colr Identity matrix (MC=0)** → lossless
/// 3. **av1C 4:4:4 + high_bitdepth / twelve_bit** → lossless (high-fidelity pipeline)
/// 4. **av1C seq_profile**: Profile 0 = 4:2:0 only → lossy; Profile 1 = 4:4:4 capable
/// 5. **pixi box**: bit depth ≥ 12 with 4:4:4 → strong lossless indicator
/// 6. **Fallback**: 4:4:4 without definitive indicators → **Err** (4:4:4 lossy exists, e.g. avifenc --yuv 444)
///
/// Returns Err when av1C box is missing, or 4:4:4 without definitive lossless indicators.
fn detect_avif_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;

    // Dimension 1-4: Parse av1C box
    if let Some(av1c_data) = find_box_data_recursive(&data, b"av1C") {
        if av1c_data.len() >= 4 {
            let byte1 = av1c_data[1];
            let byte2 = av1c_data[2];

            let seq_profile = (byte1 >> 5) & 0x07;
            let high_bitdepth = (byte2 >> 6) & 0x01;
            let twelve_bit = (byte2 >> 5) & 0x01;
            let monochrome = (byte2 >> 4) & 0x01;
            let chroma_subsampling_x = (byte2 >> 3) & 0x01;
            let chroma_subsampling_y = (byte2 >> 2) & 0x01;

            let is_444 = chroma_subsampling_x == 0 && chroma_subsampling_y == 0;
            let is_420 = chroma_subsampling_x == 1 && chroma_subsampling_y == 1;
            let is_422 = chroma_subsampling_x == 1 && chroma_subsampling_y == 0;

            if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                eprintln!(
                    "   📊 AVIF av1C: profile={}, high_bd={}, 12bit={}, mono={}, chroma={}",
                    seq_profile, high_bitdepth, twelve_bit, monochrome,
                    if is_444 { "4:4:4" } else if is_422 { "4:2:2" } else if is_420 { "4:2:0" } else { "mono" }
                );
            }

            // Dimension 1: AV1 lossless REQUIRES 4:4:4 — any subsampling is definitively lossy
            if is_420 || is_422 {
                if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                    eprintln!("   📊 AVIF: chroma subsampling detected — definitely lossy");
                }
                return Ok(CompressionType::Lossy);
            }

            // Monochrome without 4:4:4 is also lossy (unless truly grayscale lossless,
            // but monochrome + subsampling flags both 1 = lossy mono)
            if monochrome == 1 && !is_444 {
                return Ok(CompressionType::Lossy);
            }

            // From here: 4:4:4 (or monochrome with no subsampling)

            // Dimension 2: Check colr box for Identity matrix (MC=0) — definitive lossless
            // nclx payload: colour_type[4] + primaries[2] + transfer[2] + matrix_coefficients[2] + full_range[1]
            if let Some(colr_data) = find_box_data_recursive(&data, b"colr") {
                if colr_data.len() >= 11 && &colr_data[0..4] == b"nclx" {
                    let matrix_coefficients =
                        u16::from_be_bytes([colr_data[8], colr_data[9]]);
                    if matrix_coefficients == 0 {
                        // Identity matrix (IEC 61966-2-1 sRGB / GBR) = lossless
                        if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                            eprintln!("   📊 AVIF: 4:4:4 + Identity matrix (MC=0) — lossless");
                        }
                        return Ok(CompressionType::Lossless);
                    }
                }
            }

            // Dimension 3: high_bitdepth or twelve_bit with 4:4:4 → lossless pipeline
            if is_444 && (twelve_bit == 1 || (high_bitdepth == 1 && seq_profile >= 1)) {
                if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                    eprintln!("   📊 AVIF: 4:4:4 + high bit depth — lossless");
                }
                return Ok(CompressionType::Lossless);
            }

            // Dimension 4: Profile-based reasoning
            // Profile 0 can only do 4:2:0 in lossy mode; if we're here with 4:4:4,
            // profile must be 1 or 2 — 4:4:4 is extremely rare in lossy AVIF
            if is_444 && seq_profile == 0 {
                // Profile 0 + 4:4:4 is technically invalid for lossy; treat as lossless
                return Ok(CompressionType::Lossless);
            }

            // Dimension 5: pixi box bit depth
            if is_444 {
                if let Some(pixi_data) = find_box_data_recursive(&data, b"pixi") {
                    if !pixi_data.is_empty() {
                        let num_channels = pixi_data[0] as usize;
                        if num_channels > 0 && pixi_data.len() > num_channels {
                            let max_depth = pixi_data[1..=num_channels].iter().copied().max().unwrap_or(0);
                            if max_depth >= 12 {
                                return Ok(CompressionType::Lossless);
                            }
                        }
                    }
                }
            }

            // Dimension 6: Monochrome 4:4:4 (grayscale lossless) — check before Err fallback
            if is_444 && monochrome == 1 {
                if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                    eprintln!("   📊 AVIF: monochrome 4:4:4 — lossless grayscale");
                }
                return Ok(CompressionType::Lossless);
            }

            // Dimension 7: 4:4:4 without Identity matrix, without high bit depth, without Profile 0 —
            // could be high-quality lossy 4:4:4 (e.g. avifenc --yuv 444 -q 90). Refuse to guess.
            if is_444 {
                return Err(ImgQualityError::AnalysisError(format!(
                    "AVIF: 4:4:4 without definitive lossless indicators (no Identity MC, no high bit depth); \
                     refusing to guess — {}",
                    path.display()
                )));
            }
        }
    }

    // No av1C box found at all — truly cannot determine
    Err(ImgQualityError::AnalysisError(format!(
        "AVIF: no av1C box found in container; cannot determine compression — {}",
        path.display()
    )))
}

/// Detect HEIC/HEIF lossless encoding — multi-dimension analysis.
///
/// Dimensions checked (in priority order):
/// 1. **hvcC profile_idc**: Main(1)/Main10(2)/MainStillPicture(3) → definitely lossy (4:2:0 only)
/// 2. **hvcC RExt(4)/SCC(9)** → lossless capable; check chroma_format_idc
/// 3. **hvcC chroma_format_idc**: < 3 (not 4:4:4) → lossy; == 3 → lossless
/// 4. **hvcC general_profile_compatibility_flags**: bit 4 set → RExt compatible → lossless
/// 5. **pixi box**: high bit depth with compatible profile → lossless indicator
/// 6. **colr box**: Identity matrix (MC=0) → lossless
///
/// Only returns Err when hvcC box is missing entirely.
fn detect_heic_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;

    if let Some(hvcc_data) = find_box_data_recursive(&data, b"hvcC") {
        if hvcc_data.len() >= 23 {
            let profile_idc = hvcc_data[1] & 0x1F;

            // Bytes 2-5: general_profile_compatibility_flags (32 bits)
            let compat_flags = u32::from_be_bytes([hvcc_data[2], hvcc_data[3], hvcc_data[4], hvcc_data[5]]);

            // HEVCDecoderConfigurationRecord fixed fields:
            //   [16] chromaFormatIdc (low 2 bits)
            //   [17] bitDepthLumaMinus8 (low 3 bits)
            //   [18] bitDepthChromaMinus8 (low 3 bits)
            let chroma_format_idc = hvcc_data[16] & 0x03; // 0=mono, 1=4:2:0, 2=4:2:2, 3=4:4:4
            let bit_depth_luma = (hvcc_data[17] & 0x07) + 8;
            let bit_depth_chroma = (hvcc_data[18] & 0x07) + 8;

            if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                eprintln!(
                    "   📊 HEIC hvcC: profile_idc={}, compat_flags=0x{:08X}, chroma={}, luma_depth={}, chroma_depth={}",
                    profile_idc, compat_flags, chroma_format_idc, bit_depth_luma, bit_depth_chroma
                );
            }

            // Dimension 0: chromaFormatIdc — direct chroma subsampling (like AVIF av1C)
            // 4:2:0 (1) or 4:2:2 (2) → definitively lossy (HEVC lossless requires 4:4:4)
            if chroma_format_idc == 1 || chroma_format_idc == 2 {
                if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                    eprintln!(
                        "   📊 HEIC: chroma {} — definitely lossy",
                        if chroma_format_idc == 1 { "4:2:0" } else { "4:2:2" }
                    );
                }
                return Ok(CompressionType::Lossy);
            }

            // Dimension 1: Main/Main10/MainStillPicture → always 4:2:0 → always lossy
            if profile_idc == 1 || profile_idc == 2 || profile_idc == 3 {
                if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                    eprintln!(
                        "   📊 HEIC: Main profile ({}) — 4:2:0 only — definitely lossy",
                        match profile_idc { 1 => "Main", 2 => "Main10", 3 => "MainStillPicture", _ => "?" }
                    );
                }
                return Ok(CompressionType::Lossy);
            }

            // Dimension 2: RExt (4) or SCC (9) profiles can be lossless
            if profile_idc == 4 || profile_idc == 9 {
                // 4:4:4 from chromaFormatIdc is a strong lossless indicator for RExt/SCC
                let is_444 = chroma_format_idc == 3;

                // Check colr box for Identity matrix
                // nclx payload: colour_type[4] + primaries[2] + transfer[2] + matrix_coefficients[2] + full_range[1]
                if let Some(colr_data) = find_box_data_recursive(&data, b"colr") {
                    if colr_data.len() >= 11 && &colr_data[0..4] == b"nclx" {
                        let matrix_coefficients =
                            u16::from_be_bytes([colr_data[8], colr_data[9]]);
                        if matrix_coefficients == 0 {
                            if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                                eprintln!("   📊 HEIC: RExt + Identity matrix — lossless");
                            }
                            return Ok(CompressionType::Lossless);
                        }
                    }
                }

                // Check pixi box for high bit depth
                if let Some(pixi_data) = find_box_data_recursive(&data, b"pixi") {
                    if !pixi_data.is_empty() {
                        let num_ch = pixi_data[0] as usize;
                        if num_ch > 0 && pixi_data.len() > num_ch {
                            let max_depth = pixi_data[1..=num_ch].iter().copied().max().unwrap_or(0);
                            if max_depth >= 12 {
                                return Ok(CompressionType::Lossless);
                            }
                        }
                    }
                }

                // High bit depth from hvcC itself
                if is_444 && (bit_depth_luma >= 12 || bit_depth_chroma >= 12) {
                    return Ok(CompressionType::Lossless);
                }

                // RExt/SCC + 4:4:4 without other indicators — likely lossless but not certain
                if is_444 {
                    if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                        eprintln!("   📊 HEIC: RExt/SCC profile ({}) + 4:4:4 — likely lossless", profile_idc);
                    }
                    return Ok(CompressionType::Lossless);
                }

                // RExt/SCC without 4:4:4 — ambiguous (RExt can also do lossy 4:2:0)
                return Err(ImgQualityError::AnalysisError(format!(
                    "HEIC: RExt/SCC profile ({}) without 4:4:4 chroma; cannot determine — {}",
                    profile_idc, path.display()
                )));
            }

            // Dimension 4: Check profile compatibility flags — bit 4 = RExt compatible
            // RExt compatibility only indicates encoder capability, not actual lossless encoding.
            // Must verify chromaFormatIdc == 3 (4:4:4) before assuming lossless.
            if (compat_flags & (1 << (31 - 4))) != 0 {
                if chroma_format_idc == 3 {
                    if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                        eprintln!("   📊 HEIC: RExt compatibility flag + 4:4:4 — lossless");
                    }
                    return Ok(CompressionType::Lossless);
                } else {
                    // RExt compatible but not 4:4:4 — ambiguous
                    return Err(ImgQualityError::AnalysisError(format!(
                        "HEIC: RExt compatibility flag set but chroma {} (not 4:4:4); cannot determine — {}",
                        chroma_format_idc, path.display()
                    )));
                }
            }

            // Unknown profile but hvcC exists — profiles 5-8, 10+ are rare
            // Most are lossy variants; treat as lossy rather than Err
            if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                eprintln!("   📊 HEIC: unknown profile {} — treating as lossy", profile_idc);
            }
            return Ok(CompressionType::Lossy);
        }
    }

    // No hvcC box — truly cannot determine
    Err(ImgQualityError::AnalysisError(format!(
        "HEIC/HEIF: no hvcC box found in container; cannot determine compression — {}",
        path.display()
    )))
}

/// Detect JXL (JPEG XL) lossless encoding
///
/// JXL can use modular mode (lossless) or VarDCT mode (lossy).
/// - Container: look for "jbrd" box (JPEG bitstream reconstruction = lossless recompression).
/// - Naked codestream: look for "jbrd" in raw bytes (e.g. container-style fragment) or treat as lossy (conservative).

/// Detect ICO compression by inspecting embedded image entries.
///
/// ICO directory: header[6] + entries[16 each]. Each entry has an offset to image data.
/// If image data starts with PNG magic → recursively check PNG quantization.
/// Any quantized PNG entry → Lossy. Otherwise → Lossless.
fn detect_ico_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 64 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    // ICO header: reserved(2) + type(2) + count(2) = 6 bytes
    if data.len() < 6 {
        return Ok(CompressionType::Lossless);
    }

    let image_count = u16::from_le_bytes([data[4], data[5]]) as usize;
    let png_magic: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    // Each directory entry is 16 bytes, starting at offset 6
    for i in 0..image_count {
        let entry_offset = 6 + i * 16;
        if entry_offset + 16 > data.len() {
            break;
        }

        // Bytes 8-11: size of image data, bytes 12-15: offset of image data
        let img_size = u32::from_le_bytes([
            data[entry_offset + 8], data[entry_offset + 9],
            data[entry_offset + 10], data[entry_offset + 11],
        ]) as usize;
        let img_offset = u32::from_le_bytes([
            data[entry_offset + 12], data[entry_offset + 13],
            data[entry_offset + 14], data[entry_offset + 15],
        ]) as usize;

        if img_offset + 8 > data.len() {
            continue;
        }
        let img_end = (img_offset + img_size).min(data.len());

        // Check if this entry is an embedded PNG
        if data[img_offset..].starts_with(png_magic) && img_end > img_offset + 33 {
            let png_data = &data[img_offset..img_end];

            // Run full PNG quantization analysis on embedded PNG
            // Write to temp file since analyze_png_quantization needs a Path
            use std::io::Write;
            if let Ok(mut temp_file) = tempfile::NamedTempFile::new() {
                if temp_file.write_all(png_data).is_ok() {
                    if let Ok(analysis) = analyze_png_quantization(temp_file.path()) {
                        if analysis.is_quantized {
                            if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                                eprintln!(
                                    "   📊 ICO: embedded PNG quantized (confidence {:.1}%)",
                                    analysis.confidence * 100.0
                                );
                            }
                            return Ok(CompressionType::Lossy);
                        }
                    }
                }
            }
        }
        // BMP/DIB entries are always lossless — no action needed
    }

    Ok(CompressionType::Lossless)
}

/// Detect OpenEXR compression type by parsing the header attributes.
///
/// EXR header: magic (76 2F 31 01) + version (4 bytes) + attributes until empty name.
/// Each attribute: null-terminated name + null-terminated type + size (u32 LE) + value.
/// The "compression" attribute value byte:
///   0=NONE, 1=RLE, 2=ZIPS, 3=ZIP, 4=PIZ → lossless
///   5=PXR24, 6=B44, 7=B44A, 8=DWAA, 9=DWAB → lossy
///
/// EXR 2.0 multi-part: version bit 9 = 1. Each part has independent header with its own
/// compression. Parts separated by empty name; all parts end with two consecutive empty names.
/// Any lossy part → Lossy overall.
fn detect_exr_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    // Magic (4) + version (4) = 8 bytes minimum before attributes
    if data.len() < 12 || !data.starts_with(&[0x76, 0x2F, 0x31, 0x01]) {
        return Ok(CompressionType::Lossless); // fallback
    }

    // Check version field for multi-part flag (bit 9)
    let version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let is_multipart = (version & (1 << 9)) != 0;

    let mut pos = 8; // skip magic + version
    let mut found_any_compression = false;
    let mut part_count = 0;

    // Scan all parts (single-part = 1 iteration, multi-part = multiple)
    loop {
        part_count += 1;

        // Scan attributes in this part: each is name\0 + type\0 + size(u32 LE) + value
        // Empty name terminates the part header
        while pos < data.len() {
            // Read attribute name (null-terminated)
            let name_start = pos;
            while pos < data.len() && data[pos] != 0 {
                pos += 1;
            }
            if pos >= data.len() {
                break;
            }
            let name = &data[name_start..pos];
            pos += 1; // skip null terminator

            // Empty name = end of this part's header
            if name.is_empty() {
                break;
            }

            // Read type name (null-terminated)
            while pos < data.len() && data[pos] != 0 {
                pos += 1;
            }
            if pos >= data.len() {
                break;
            }
            pos += 1; // skip null terminator

            // Read value size (u32 LE)
            if pos + 4 > data.len() {
                break;
            }
            let value_size =
                u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
            pos += 4;

            if name == b"compression" && value_size >= 1 && pos < data.len() {
                let compression = data[pos];
                found_any_compression = true;

                if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                    eprintln!(
                        "   📊 EXR part#{} compression: {} ({})",
                        part_count,
                        compression,
                        match compression {
                            0 => "NONE",
                            1 => "RLE",
                            2 => "ZIPS",
                            3 => "ZIP",
                            4 => "PIZ",
                            5 => "PXR24",
                            6 => "B44",
                            7 => "B44A",
                            8 => "DWAA",
                            9 => "DWAB",
                            _ => "Unknown",
                        }
                    );
                }

                // Any lossy part → entire file is lossy
                if compression >= 5 {
                    return Ok(CompressionType::Lossy);
                }
            }

            // Skip value
            pos += value_size;
        }

        // If not multi-part, we're done after first part
        if !is_multipart {
            break;
        }

        // Multi-part: check for second consecutive empty name (end of all parts)
        if pos < data.len() && data[pos] == 0 {
            // Two consecutive empty names → end of multi-part file
            break;
        }

        // Continue to next part
        if pos >= data.len() {
            break;
        }
    }

    // All parts scanned
    if found_any_compression {
        Ok(CompressionType::Lossless)
    } else {
        // No compression attribute found — default lossless (NONE is the default in EXR spec)
        Ok(CompressionType::Lossless)
    }
}

/// Detect JPEG 2000 lossless vs lossy by parsing COD and COC markers.
///
/// COD (Coding style Default, FF 52) contains default SPcod parameters; the last byte
/// is the wavelet transform type:
///   - 0 = 9/7 irreversible (lossy)
///   - 1 = 5/3 reversible (lossless)
///
/// COC (Component-specific coding style, FF 53) can override COD for specific components.
/// For multi-component images (e.g. DICOM-JP2), if COD=9/7 but COC overrides to 5/3 for
/// a component, we need to check all components. Any lossy component → Lossy overall.
///
/// For JP2 container: find the codestream inside "jp2c" box, then scan for COD/COC.
/// For raw codestream (FF 4F FF 51): scan directly.
fn detect_jp2_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if data.len() < 4 {
        return Ok(CompressionType::Lossy);
    }

    // Determine where the codestream starts
    let cs_start = if data.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        // Raw codestream
        0
    } else {
        // JP2 container — find jp2c box
        find_jp2c_offset(&data).unwrap_or(0)
    };

    // Scan for COD and COC markers in the codestream header area
    // COD/COC must appear before the first tile-part, so limit scan to first 4KB of codestream
    let scan_end = (cs_start + 4096).min(data.len());
    let cs = &data[cs_start..scan_end];

    let (cod_wavelet, coc_wavelets) = find_jp2_wavelets(cs);

    // Check COD default wavelet
    if let Some(wavelet) = cod_wavelet {
        if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
            eprintln!(
                "   📊 JP2 COD wavelet: {} ({})",
                wavelet,
                if wavelet == 1 { "5/3 reversible — lossless" } else { "9/7 irreversible — lossy" }
            );
        }
        // If COD is lossy and no COC overrides, it's lossy
        if wavelet == 0 && coc_wavelets.is_empty() {
            return Ok(CompressionType::Lossy);
        }
    }

    // Check COC component-specific wavelets
    for (component, wavelet) in &coc_wavelets {
        if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
            eprintln!(
                "   📊 JP2 COC component {} wavelet: {} ({})",
                component,
                wavelet,
                if *wavelet == 1 { "5/3 reversible — lossless" } else { "9/7 irreversible — lossy" }
            );
        }
        // Any lossy component → entire file is lossy
        if *wavelet == 0 {
            return Ok(CompressionType::Lossy);
        }
    }

    // All components are lossless (or only COD found and it's lossless)
    if cod_wavelet == Some(1) || !coc_wavelets.is_empty() {
        return Ok(CompressionType::Lossless);
    }

    // Couldn't find COD — default to lossy (safer assumption for JP2)
    Ok(CompressionType::Lossy)
}

/// Find the offset of the jp2c (contiguous codestream) box payload in a JP2 container.
fn find_jp2c_offset(data: &[u8]) -> Option<usize> {
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
        ]) as usize;
        let box_type = &data[pos + 4..pos + 8];

        if box_type == b"jp2c" {
            return Some(pos + 8);
        }

        if size == 0 {
            break;
        } else if size == 1 {
            if pos + 16 > data.len() { break; }
            let ext = u64::from_be_bytes([
                data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11],
                data[pos + 12], data[pos + 13], data[pos + 14], data[pos + 15],
            ]) as usize;
            pos += ext;
        } else if size < 8 {
            break;
        } else {
            pos += size;
        }
    }
    None
}

/// Scan JPEG 2000 codestream for COD and COC markers, extract wavelet transform types.
/// Returns (COD wavelet, Vec<(component_index, COC wavelet)>).
/// COD: Some(0) for 9/7 irreversible (lossy), Some(1) for 5/3 reversible (lossless).
/// COC: component-specific overrides.
fn find_jp2_wavelets(cs: &[u8]) -> (Option<u8>, Vec<(u16, u8)>) {
    let mut cod_wavelet: Option<u8> = None;
    let mut coc_wavelets: Vec<(u16, u8)> = Vec::new();

    // Walk markers: each marker is FF xx, followed by 2-byte length (except SOC=FF4F, SOD=FF93)
    let mut pos = 0;
    while pos + 2 <= cs.len() {
        if cs[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let marker = cs[pos + 1];

        // SOC (FF 4F) — no length field
        if marker == 0x4F {
            pos += 2;
            continue;
        }
        // SOD (FF 93) — start of data, stop scanning
        if marker == 0x93 {
            break;
        }

        // COD marker (FF 52)
        if marker == 0x52 && pos + 4 <= cs.len() {
            let seg_len = u16::from_be_bytes([cs[pos + 2], cs[pos + 3]]) as usize;
            // COD segment: Scod(1) + SGcod(4) + SPcod(variable)
            // SPcod starts at offset 5 within segment data
            // SPcod layout: NL(1) + cb_width(1) + cb_height(1) + cb_style(1) + transform(1)
            // So transform byte is at segment_data[5 + 4] = segment_data[9]
            // segment_data starts at pos+4, so transform is at pos+4+9 = pos+13
            let transform_offset = pos + 4 + 9;
            if transform_offset < cs.len() && seg_len >= 10 {
                let wavelet = cs[transform_offset];
                if wavelet <= 1 {
                    cod_wavelet = Some(wavelet);
                }
            }
        }

        // COC marker (FF 53) — component-specific coding style
        if marker == 0x53 && pos + 4 <= cs.len() {
            let seg_len = u16::from_be_bytes([cs[pos + 2], cs[pos + 3]]) as usize;
            // COC segment: Ccoc(1 or 2 bytes) + Scoc(1) + SPcoc(variable)
            // For images with < 257 components, Ccoc is 1 byte; otherwise 2 bytes
            // We'll assume 1 byte for simplicity (most common case)
            // SPcoc layout is same as SPcod: NL(1) + cb_width(1) + cb_height(1) + cb_style(1) + transform(1)
            let component_offset = pos + 4;
            let spcoc_offset = component_offset + 1; // Ccoc (1 byte) + Scoc (1 byte) = 2 bytes before SPcoc
            let transform_offset = spcoc_offset + 1 + 4; // SPcoc[4] = transform

            if component_offset < cs.len() && transform_offset < cs.len() && seg_len >= 7 {
                let component = cs[component_offset] as u16;
                let wavelet = cs[transform_offset];
                if wavelet <= 1 {
                    coc_wavelets.push((component, wavelet));
                }
            }
        }

        // Skip marker segment
        if pos + 4 > cs.len() { break; }
        let seg_len = u16::from_be_bytes([cs[pos + 2], cs[pos + 3]]) as usize;
        pos += 2 + seg_len;
    }

    (cod_wavelet, coc_wavelets)
}

/// Minimal bit reader for parsing JXL codestream headers.
struct JxlBitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u8, // 0-7, LSB first
}

impl<'a> JxlBitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, byte_pos: 0, bit_pos: 0 }
    }

    fn read_bits(&mut self, n: u8) -> Option<u32> {
        if n == 0 { return Some(0); }
        let mut result: u32 = 0;
        for i in 0..n {
            if self.byte_pos >= self.data.len() { return None; }
            let bit = (self.data[self.byte_pos] >> self.bit_pos) & 1;
            result |= (bit as u32) << i;
            self.bit_pos += 1;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
                self.byte_pos += 1;
            }
        }
        Some(result)
    }

    fn read_bool(&mut self) -> Option<bool> {
        self.read_bits(1).map(|v| v == 1)
    }

    /// JXL U32: 2 selector bits, then variable additional bits per distribution.
    fn read_u32(&mut self, dists: [(u32, u8); 4]) -> Option<u32> {
        let sel = self.read_bits(2)? as usize;
        let (base, extra_bits) = dists[sel];
        let extra = self.read_bits(extra_bits)?;
        Some(base + extra)
    }
}

/// Try to parse JXL codestream header and extract `xyb_encoded` flag.
/// Returns Some(true) for lossy (XYB), Some(false) for lossless (modular), None on parse failure.
fn parse_jxl_xyb_encoded(codestream: &[u8]) -> Option<bool> {
    // Skip signature if present
    let start = if codestream.len() >= 2
        && codestream[0] == 0xFF && codestream[1] == 0x0A
    { 2 } else { 0 };

    if start >= codestream.len() { return None; }
    let mut r = JxlBitReader::new(&codestream[start..]);

    // --- SizeHeader ---
    let small = r.read_bool()?;
    if small {
        let _ysize_div8_m1 = r.read_bits(5)?;
        let ratio = r.read_bits(3)?;
        if ratio == 0 { r.read_bits(5)?; }
    } else {
        // ysize_minus1: U32(u(9), u(13), u(18), u(30))
        r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
        let ratio = r.read_bits(3)?;
        if ratio == 0 {
            r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
        }
    }

    // --- ImageMetadata ---
    let all_default = r.read_bool()?;
    if all_default {
        // all_default=true → xyb_encoded defaults to true → lossy
        return Some(true);
    }

    let extra_fields = r.read_bool()?;
    if extra_fields {
        // orientation - 1: u(3)
        r.read_bits(3)?;

        // have_intrinsic_size
        let have_intrinsic = r.read_bool()?;
        if have_intrinsic {
            // Skip another SizeHeader
            let small2 = r.read_bool()?;
            if small2 {
                r.read_bits(5)?;
                let ratio2 = r.read_bits(3)?;
                if ratio2 == 0 { r.read_bits(5)?; }
            } else {
                r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                let ratio2 = r.read_bits(3)?;
                if ratio2 == 0 {
                    r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                }
            }
        }

        // have_preview
        let have_preview = r.read_bool()?;
        if have_preview {
            // PreviewHeader: div8/div16 bools then size
            let div8 = r.read_bool()?;
            if div8 {
                r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
            } else {
                let div16 = r.read_bool()?;
                if div16 {
                    // nothing extra
                } else {
                    r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                    r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                }
            }
        }

        // have_animation
        let have_animation = r.read_bool()?;
        if have_animation {
            // AnimationHeader: tps_numerator, tps_denominator, num_loops, have_timecodes
            r.read_u32([(100, 0), (1000, 0), (0, 10), (0, 30)])?;
            r.read_u32([(1, 0), (1001, 0), (0, 10), (0, 30)])?;
            r.read_u32([(0, 0), (0, 3), (0, 16), (0, 32)])?;
            r.read_bool()?; // have_timecodes
        }
    }

    // bit_depth
    let float_sample = r.read_bool()?;
    if float_sample {
        r.read_u32([(32, 0), (16, 0), (24, 0), (1, 6)])?; // bits_per_sample
        r.read_bits(4)?; // exp_bits + 1
    } else {
        r.read_u32([(8, 0), (10, 0), (12, 0), (1, 6)])?; // bits_per_sample
    }

    // modular_16_bit_buffer_sufficient: derived, not read

    // num_extra_channels
    let num_extra = r.read_u32([(0, 0), (1, 0), (2, 0), (3, 12)])?;

    // Skip ExtraChannelInfo for each extra channel
    for _ in 0..num_extra {
        let ec_default = r.read_bool()?;
        if !ec_default {
            // d_alpha: Bool, type: enum, ...
            // This is complex; bail out rather than risk misparse
            return None;
        }
    }

    // xyb_encoded: Bool — THIS IS WHAT WE NEED
    let xyb_encoded = r.read_bool()?;
    Some(xyb_encoded)
}

/// Detect JXL (JPEG XL) lossless encoding — multi-dimension analysis.
///
/// Dimensions checked (in priority order):
/// 1. **jbrd box** (container) or jbrd bytes (naked): JPEG reconstruction → lossless
/// 2. **Codestream header `xyb_encoded`**: false → modular (lossless), true → VarDCT (lossy)
/// 3. **Codestream `all_default` metadata**: true → xyb_encoded defaults true → lossy
/// 4. **jxlc/jxlp boxes**: extract codestream from container, then parse header
///
/// Only returns Err when codestream is unparseable AND no jbrd found.
fn detect_jxl_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if data.len() < 4 {
        return Err(ImgQualityError::AnalysisError(format!(
            "JXL: file too short — {}", path.display()
        )));
    }

    let is_naked = data[0] == 0xFF && data[1] == 0x0A;

    // Dimension 1: jbrd = JPEG bitstream reconstruction = lossless
    // Only check jbrd as a proper BMFF box in container mode.
    // Naked codestream (FF 0A) has no BMFF structure — scanning raw bytes for "jbrd"
    // would be a false-positive collision risk, so skip directly to xyb_encoded parsing.
    if !is_naked {
        if find_any_box_recursive(&data, b"jbrd") {
            if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                eprintln!("   📊 JXL: jbrd box in container — lossless");
            }
            return Ok(CompressionType::Lossless);
        }
    }

    // Dimension 2-4: Parse codestream header for xyb_encoded
    let codestream: Option<&[u8]> = if is_naked {
        Some(&data)
    } else {
        // Container: find jxlc (complete) or first jxlp (partial) box
        find_box_data_recursive(&data, b"jxlc")
            .or_else(|| find_box_data_recursive(&data, b"jxlp"))
    };

    if let Some(cs) = codestream {
        match parse_jxl_xyb_encoded(cs) {
            Some(true) => {
                if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                    eprintln!("   📊 JXL: xyb_encoded=true — lossy (VarDCT)");
                }
                return Ok(CompressionType::Lossy);
            }
            Some(false) => {
                if std::env::var("IMGQUALITY_VERBOSE").is_ok() {
                    eprintln!("   📊 JXL: xyb_encoded=false — lossless (Modular)");
                }
                return Ok(CompressionType::Lossless);
            }
            None => {
                if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                    eprintln!("   📊 JXL: codestream header parse failed, falling back");
                }
            }
        }
    }

    Err(ImgQualityError::AnalysisError(format!(
        "JXL: no jbrd and codestream header unparseable — cannot determine — {}",
        path.display()
    )))
}

/// Recursively find a box by type and return its payload (excluding size + type). Used by AVIF/HEIC/JXL.
fn find_box_data_recursive<'a>(data: &'a [u8], box_type: &[u8; 4]) -> Option<&'a [u8]> {
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let current_type = &data[pos + 4..pos + 8];
        let (payload_start, next_pos) = if size == 0 {
            break;
        } else if size == 1 {
            if pos + 16 > data.len() {
                pos += 8;
                continue;
            }
            let ext = u64::from_be_bytes([
                data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11],
                data[pos + 12], data[pos + 13], data[pos + 14], data[pos + 15],
            ]) as usize;
            (pos + 16, (pos + ext).min(data.len()))
        } else if size < 8 {
            pos += 8;
            continue;
        } else {
            (pos + 8, (pos + size).min(data.len()))
        };
        if current_type == box_type {
            if next_pos <= data.len() && payload_start < next_pos {
                return Some(&data[payload_start..next_pos]);
            }
            return None;
        }
        if next_pos > payload_start {
            let sub = &data[payload_start..next_pos];
            if let Some(payload) = find_box_data_recursive(sub, box_type) {
                return Some(payload);
            }
        }
        pos = next_pos;
    }
    None
}

/// Recursively search for a box type in ISO BMFF data (e.g. "jbrd" inside "JXL " container).
fn find_any_box_recursive(data: &[u8], box_type: &[u8; 4]) -> bool {
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let current_type = &data[pos + 4..pos + 8];
        if current_type == box_type {
            return true;
        }
        let (payload_start, next_pos) = if size == 0 {
            break;
        } else if size == 1 {
            if pos + 16 > data.len() {
                pos += 8;
                continue;
            }
            let ext = u64::from_be_bytes([
                data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11],
                data[pos + 12], data[pos + 13], data[pos + 14], data[pos + 15],
            ]) as usize;
            (pos + 16, (pos + ext).min(data.len()))
        } else if size < 8 {
            pos += 8;
            continue;
        } else {
            (pos + 8, (pos + size).min(data.len()))
        };
        if next_pos > payload_start && find_any_box_recursive(&data[payload_start..next_pos], box_type) {
            return true;
        }
        pos = next_pos;
    }
    false
}

/// Helper function to find a box in ISO Base Media File Format (used by AVIF/HEIF). Top-level only.
#[allow(dead_code)]
fn find_box_data<'a>(data: &'a [u8], box_type: &[u8; 4]) -> Option<&'a [u8]> {
    let mut pos = 0;

    while pos + 8 <= data.len() {
        // Read box size (big-endian)
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;

        // Read box type
        let current_type = &data[pos + 4..pos + 8];

        if current_type == box_type {
            // Found the box, return its data (excluding size and type; for size==1, excluding 8-byte extended size too)
            let (data_start, data_end) = if size == 0 {
                (pos + 8, data.len())
            } else if size == 1 {
                if pos + 16 > data.len() {
                    return None;
                }
                let extended_size = u64::from_be_bytes([
                    data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11],
                    data[pos + 12], data[pos + 13], data[pos + 14], data[pos + 15],
                ]) as usize;
                (pos + 16, (pos + extended_size).min(data.len()))
            } else {
                (pos + 8, (pos + size).min(data.len()))
            };

            if data_end <= data.len() && data_start < data_end {
                return Some(&data[data_start..data_end]);
            }
            return None;
        }

        // Move to next box
        if size == 0 {
            break; // Box extends to end of file
        } else if size == 1 {
            // Extended size
            if pos + 16 > data.len() {
                break;
            }
            let extended_size = u64::from_be_bytes([
                data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11],
                data[pos + 12], data[pos + 13], data[pos + 14], data[pos + 15],
            ]) as usize;
            pos += extended_size;
        } else if size < 8 {
            break; // Invalid box size
        } else {
            pos += size;
        }
    }

    None
}
