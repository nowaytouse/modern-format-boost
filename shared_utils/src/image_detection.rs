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
use image::{DynamicImage, GenericImageView, ImageReader, Rgba};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Open an image with relaxed memory limits to handle very large JPEGs.
/// Increases max_alloc from default ~512MB to 2GB for legitimate large images.
/// Still protects against malicious images (2GB is reasonable for 100MP+ images).
pub fn open_image_with_limits(path: &Path) -> std::result::Result<DynamicImage, image::ImageError> {
    use image::Limits;
    let mut limits = Limits::default();
    limits.max_alloc = Some(2 * 1024 * 1024 * 1024); // 2GB (reasonable for 100MP images)

    let mut reader = ImageReader::open(path)?;
    reader = reader.with_guessed_format()?;
    reader.limits(limits);
    reader.decode()
}

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrecisionMetadata {
    pub bit_depth: Option<u8>,
    pub palette_size: Option<usize>,
    pub color_type: Option<u8>, // Format-specific (e.g. PNG color type)
    pub is_lossless_deterministic: bool,
    pub quality_estimate: Option<u8>,
    pub chroma_subsampling: Option<String>,
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

    pub color_frequency_distribution: f64,
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

    pub precision: PrecisionMetadata,
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
        if brand == b"heic"
            || brand == b"heix"
            || brand == b"heim"
            || brand == b"heis"
            || brand == b"hevc"
            || brand == b"hevx"
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
    if header.len() >= 12 && header[0..4] == [0x00, 0x00, 0x00, 0x0C] && header[4..8] == *b"jP  " {
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
    // 🚀 Stage 1: Native Fast-Path for Simple Formats
    // GIF, WebP, and PNG have simple, deterministic byte-level frame structures.
    // We can rely on our native parsers for these to save the ffprobe overhead.
    match format {
        DetectedFormat::GIF => {
            crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
                .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let frame_count = crate::image_formats::gif::count_frames_from_bytes(&data);
            return Ok((frame_count > 1, frame_count, None));
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
            return Ok((is_animated, frame_count, None));
        }
        DetectedFormat::PNG => {
            crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
                .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let (is_animated, frame_count) = parse_apng_frames(&data);
            return Ok((is_animated, frame_count, None));
        }
        _ => {} // Fall through for ISOBMFF and unknown formats
    }

    // 🚀 Stage 2: libavformat / ffprobe for Complex Containers
    // Third-party libraries like libavformat have years of fuzzing, fixes, and edge-case
    // coverage for complex ISOBMFF containers (AVIF, HEIC). Hand-written box-level parsing
    // is prone to false positives (e.g., seeing 'avis' brand but missing 'hdlr' or 'iloc' links)
    // and false negatives. We trust ffprobe natively here.
    let mut fps = None;
    if crate::ffprobe::is_ffprobe_available() {
        if let Ok(probe) = crate::ffprobe::probe_video(path) {
            let probe_frames = probe.frame_count as u32;
            if probe.frame_rate > 0.0 {
                fps = Some(probe.frame_rate as f32);
            }

            if probe_frames > 1 {
                return Ok((true, probe_frames, fps));
            } else if probe_frames == 1 {
                return Ok((false, 1, fps));
            } else if probe.duration > 0.1 && probe.format_name.contains("video") {
                return Ok((true, 0, fps));
            }
        }

        // If metadata probe fails to find frame count (common for AVIF/JXL sequences),
        // we explicitly count the packets. This demuxes the file and is 100% accurate.
        if matches!(format, DetectedFormat::AVIF | DetectedFormat::JXL) {
            if let Some(explicit_count) = crate::ffprobe::get_frame_count(path) {
                if explicit_count > 1 {
                    return Ok((true, explicit_count as u32, fps));
                } else {
                    return Ok((false, 1, fps));
                }
            }
        }
    }

    // 🛡️ Stage 3: Ultimate Fallback (if ffprobe is missing or fails entirely)
    let mut is_animated = false;
    let mut frame_count = 1;

    match format {
        DetectedFormat::AVIF => {
            is_animated = is_isobmff_animated_sequence(path);
            if is_animated {
                frame_count = 0;
            }
        }
        DetectedFormat::JXL => {
            is_animated = is_jxl_animated_via_ffprobe(path);
            if is_animated {
                frame_count = 0;
            }
        }
        _ => {}
    }

    Ok((is_animated, frame_count, fps))
}

/// Parse GIF palette size from Global Color Table (GCT) and Local Color Table (LCT)
pub fn parse_gif_precision_metadata(path: &Path) -> Result<PrecisionMetadata> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 13];
    file.read_exact(&mut header)?;

    if !header.starts_with(b"GIF87a") && !header.starts_with(b"GIF89a") {
        return Err(ImgQualityError::AnalysisError(
            "Not a valid GIF file".to_string(),
        ));
    }

    // Logical Screen Descriptor (bytes 6-12)
    // Byte 10: GCT flag (bit 7), Color Resolution (bits 4-6), Sort flag (bit 3), GCT size (bits 0-2)
    let packed = header[10];
    let has_gct = (packed & 0x80) != 0;
    let gct_size_exp = packed & 0x07;
    let gct_colors = if has_gct { 1 << (gct_size_exp + 1) } else { 0 };

    let mut metadata = PrecisionMetadata {
        bit_depth: Some(8),
        palette_size: Some(gct_colors),
        is_lossless_deterministic: true,
        ..Default::default()
    };

    // If GCT exists, we skip it and look for LCT in image descriptors
    if has_gct {
        let gct_byte_size = 3 * gct_colors;
        let _pos = 13 + gct_byte_size;

        // We can't easily scan the whole file for all LCTs without a full decoder,
        // but typically GCT is the primary source. If we need perfect accuracy,
        // we'd need to parse all blocks. For now, GCT is a huge improvement over heuristics.
        let mut data = Vec::new();
        let mut file = File::open(path)?;
        file.read_to_end(&mut data)?;

        // Basic scan for Image Descriptor (0x2C) to find LCT
        let mut max_palette = gct_colors;
        let mut current_pos = 13 + gct_byte_size;

        while current_pos + 10 < data.len() {
            match data[current_pos] {
                0x2C => {
                    // Image Descriptor
                    let packed_img = data[current_pos + 9];
                    let has_lct = (packed_img & 0x80) != 0;
                    if has_lct {
                        let lct_size_exp = packed_img & 0x07;
                        let lct_colors = 1 << (lct_size_exp + 1);
                        max_palette = max_palette.max(lct_colors);
                        current_pos += 10 + (3 * lct_colors);
                    } else {
                        current_pos += 10;
                    }
                    // Skip image data blocks
                    while current_pos < data.len() && data[current_pos] != 0 {
                        let block_size = data[current_pos] as usize;
                        current_pos += block_size + 1;
                    }
                    current_pos += 1;
                }
                0x21 => {
                    // Extension
                    if current_pos + 2 >= data.len() {
                        break;
                    }
                    current_pos += 2;
                    while current_pos < data.len() && data[current_pos] != 0 {
                        let block_size = data[current_pos] as usize;
                        current_pos += block_size + 1;
                    }
                    current_pos += 1;
                }
                0x3B => break, // Trailer
                _ => current_pos += 1,
            }
        }
        metadata.palette_size = Some(max_palette);
    }

    Ok(metadata)
}

/// Returns true if the ISOBMFF file (AVIF/HEIC/HEIF) is an image sequence (animated).
/// Checks major_brand and compatible_brands for known sequence brand codes.
pub fn is_isobmff_animated_sequence(path: &Path) -> bool {
    // Sequence brands: avis=AVIF sequence, msf1=multi-sample ftyp (used by animated HEIC/AVIF)
    const SEQUENCE_BRANDS: &[&[u8]] = &[b"avis", b"msf1"];

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut header = [0u8; 32];
    if std::io::Read::read_exact(&mut file, &mut header).is_err() {
        return false;
    }

    if header[4..8] != *b"ftyp" {
        return false;
    }

    let major_brand = &header[8..12];
    for seq_brand in SEQUENCE_BRANDS {
        if major_brand == *seq_brand {
            return true;
        }
    }

    // Scan compatible_brands (each 4 bytes, starting at offset 16)
    let ftyp_box_size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if !(16..=4096).contains(&ftyp_box_size) {
        return false;
    }
    let compat_size = ftyp_box_size - 16;
    if compat_size == 0 {
        return false;
    }

    let mut compat_data = vec![0u8; compat_size];
    if std::io::Read::read_exact(&mut file, &mut compat_data).is_err() {
        return false;
    }

    for cb in compat_data.chunks_exact(4) {
        for seq_brand in SEQUENCE_BRANDS {
            if cb == *seq_brand {
                return true;
            }
        }
    }

    false
}

/// Returns true if the JXL file contains animation.
/// JXL stores animation natively in its container; we use ffprobe to check duration > 0.
/// Falls back to jxlinfo "animation" keyword detection if ffprobe is unavailable.
fn is_jxl_animated_via_ffprobe(path: &Path) -> bool {
    use std::process::Command;

    // FFmpeg's jpegxl_anim decoder is incomplete and cannot properly detect JXL animation.
    // We need to convert to APNG first, then check frame count.

    // Check if djxl is available
    if which::which("djxl").is_err() {
        // Fallback: try jxlinfo
        if let Ok(output) = Command::new("jxlinfo")
            .arg(crate::safe_path_arg(path).as_ref())
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
                return stdout.contains("animation");
            }
        }
        return false;
    }

    // Create temporary APNG file
    let temp_apng = match tempfile::Builder::new().suffix(".apng").tempfile() {
        Ok(f) => f,
        Err(_) => return false,
    };
    let temp_apng_path = temp_apng.path();

    // Convert JXL to APNG using djxl
    let djxl_result = Command::new("djxl")
        .arg(crate::safe_path_arg(path).as_ref())
        .arg(crate::safe_path_arg(temp_apng_path).as_ref())
        .output();

    if djxl_result.is_err() || !temp_apng_path.exists() {
        return false;
    }

    // Check frame count using ffprobe with -count_frames
    if let Ok(output) = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "json",
            "--",
        ])
        .arg(crate::safe_path_arg(temp_apng_path).as_ref())
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                if let Some(stream) = json["streams"].as_array().and_then(|s| s.first()) {
                    if let Some(nb_frames_str) = stream["nb_read_frames"].as_str() {
                        if let Ok(nb_frames) = nb_frames_str.parse::<u32>() {
                            return nb_frames > 1;
                        }
                    }
                }
            }
        }
    }

    // temp_apng will be automatically cleaned up when dropped
    false
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

            if crate::image_formats::webp::is_animated_from_bytes(&data) {
                return detect_webp_animation_compression(&data);
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
        crate::progress_mode::emit_stderr(&format!(
            "   📊 PNG Analysis: {} (confidence: {:.1}%)\n      {}",
            if analysis.is_quantized {
                "Quantized/Lossy"
            } else {
                "Lossless"
            },
            analysis.confidence * 100.0,
            analysis.explanation
        ));
    }

    Ok(if analysis.is_quantized {
        CompressionType::Lossy
    } else {
        CompressionType::Lossless
    })
}

pub fn analyze_png_quantization(path: &Path) -> Result<PngQuantizationAnalysis> {
    let file = std::fs::File::open(path).map_err(ImgQualityError::IoError)?;
    let mut reader = std::io::BufReader::new(file);
    analyze_png_quantization_from_reader(&mut reader, Some(path))
}

pub fn analyze_png_quantization_from_bytes(data: &[u8]) -> Result<PngQuantizationAnalysis> {
    let mut cursor = std::io::Cursor::new(data);
    analyze_png_quantization_from_reader(&mut cursor, None)
}

pub fn analyze_png_quantization_from_reader<R: std::io::Read + std::io::Seek>(
    mut reader: R,
    path: Option<&Path>,
) -> Result<PngQuantizationAnalysis> {
    let png_info = parse_png_structure(&mut reader)?;

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

        // Palette density: entries per sqrt(pixel_count).
        // Small image + small palette = normal (icon, pixel art).
        // Large image + small palette = quantization indicator.
        let palette_density = palette_size as f64 / (pixel_count as f64).sqrt();

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
        } else if palette_size <= 16 && palette_density > 0.5 {
            // Small palette on small image — likely intentional (pixel art, icon)
            factors.large_palette = 0.0;
        } else if palette_size <= 32 && palette_density > 0.3 {
            factors.large_palette = 0.1;
        } else {
            factors.large_palette = if palette_density < 0.5 { 0.3 } else { 0.15 };
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

    if let Some(ref tool) = png_info.detected_tool {
        factors.tool_signature = 1.0;
        detected_tool = Some(tool.clone());
        explanations.push(format!("Tool signature detected: {}", tool));
    }

    if png_info.color_type == 3 {
        if let Some(p) = path {
            if let Ok(img) = open_image_with_limits(p) {
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

                let freq_score = detect_color_frequency_distribution(&img);
                factors.color_frequency_distribution = freq_score;
                if freq_score > 0.5 {
                    explanations.push(format!(
                        "Color frequency concentrated (score: {:.2}) — quantization indicator",
                        freq_score
                    ));
                }

                let (entropy, max_entropy, entropy_ratio) =
                    if let Some(p_size) = png_info.palette_size {
                        let pe = calculate_palette_index_entropy(&img, p_size);
                        (pe.0, pe.1, pe.2)
                    } else {
                        let e = calculate_rgb_entropy(&img);
                        let ps = 256.0f64;
                        let me = ps.log2();
                        let ratio = if me > 0.0 { e / me } else { 0.0 };
                        (e, me, ratio)
                    };
                let palette_size = png_info.palette_size.unwrap_or(256) as f64;
                if palette_size >= 64.0 && entropy_ratio < 0.6 && pixel_count > 10_000 {
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
    }

    let expected_size = estimate_uncompressed_size(&png_info);
    let actual_size = reader.seek(std::io::SeekFrom::End(0)).unwrap_or(0);
    let compression_ratio = if expected_size > 0 {
        actual_size as f64 / expected_size as f64
    } else {
        1.0
    };

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
        structural: 0.50,
        metadata: 0.15,
        statistical: 0.25,
        heuristic: 0.10,
    };

    let structural_score = (factors.indexed_with_alpha + factors.large_palette) / 2.0;

    let metadata_score = factors.tool_signature;

    let statistical_score = (factors.dithering_detected
        + factors.color_count_anomaly
        + factors.gradient_banding
        + factors.color_frequency_distribution)
        / 4.0;

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
            "         Statistical: {:.2} (dither={:.2}, color={:.2}, band={:.2}, freq={:.2}) × {:.2} = {:.3}",
            statistical_score,
            factors.dithering_detected,
            factors.color_count_anomaly,
            factors.gradient_banding,
            factors.color_frequency_distribution,
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
        // Analyze truecolor for quantization signals (conservative: need 2+ strong signals)
        if let Some(p) = path {
            if let Ok(img) = open_image_with_limits(p) {
                let pixel_count = png_info.width as u64 * png_info.height as u64;

                // Signal 1: color frequency concentration
                let freq_signal = detect_color_frequency_distribution(&img);

                // Signal 2: per-channel entropy
                let rgb_entropy = calculate_rgb_entropy(&img);
                let max_entropy = 8.0f64;
                let entropy_ratio = rgb_entropy / max_entropy;
                let entropy_signal = if entropy_ratio < 0.55 && pixel_count > 10_000 {
                    0.70
                } else if entropy_ratio < 0.65 && pixel_count > 10_000 {
                    0.40
                } else {
                    0.0
                };

                // Signal 3: gradient banding
                let banding_signal = detect_gradient_banding(&img);

                let strong_signals = [freq_signal, entropy_signal, banding_signal]
                    .iter()
                    .filter(|&&s| s >= 0.50)
                    .count();

                if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                    eprintln!(
                        "      🎨 Truecolor analysis: freq={:.2}, entropy={:.2} (raw={:.2}), band={:.2}, strong={}",
                        freq_signal, entropy_signal, rgb_entropy, banding_signal, strong_signals
                    );
                }

                if strong_signals >= 2 {
                    let tc_score = (freq_signal + entropy_signal + banding_signal) / 3.0;
                    return Ok(PngQuantizationAnalysis {
                        is_quantized: true,
                        confidence: 0.70 + tc_score * 0.15,
                        factor_scores: factors,
                        detected_tool: None,
                        explanation: format!(
                            "Truecolor quantization detected (freq={:.2}, entropy={:.2}, band={:.2})",
                            freq_signal, entropy_signal, banding_signal
                        ),
                    });
                } else if strong_signals == 1 {
                    return Ok(PngQuantizationAnalysis {
                        is_quantized: false,
                        confidence: 0.65,
                        factor_scores: factors,
                        detected_tool: None,
                        explanation:
                            "Truecolor PNG — weak quantization signal, treating as lossless"
                                .to_string(),
                    });
                }
            }
        }
        return Ok(PngQuantizationAnalysis {
            is_quantized: false,
            confidence: 0.90,
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

pub struct PngStructureInfo {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
    pub palette_size: Option<usize>,
    pub has_trns: bool,
    pub has_text_chunks: bool,
    pub detected_tool: Option<String>,
}

struct PngQuantizationWeights {
    structural: f64,
    metadata: f64,
    statistical: f64,
    heuristic: f64,
}

pub fn parse_png_structure<R: std::io::Read + std::io::Seek>(
    mut reader: R,
) -> Result<PngStructureInfo> {
    use std::io::SeekFrom;

    fn skip_bytes<R: std::io::Seek>(reader: &mut R, bytes: u64, context: &str) -> Result<()> {
        let offset = i64::try_from(bytes).map_err(|_| {
            ImgQualityError::AnalysisError(format!(
                "PNG chunk too large to seek while parsing {}",
                context
            ))
        })?;
        reader.seek(SeekFrom::Current(offset)).map_err(|e| {
            ImgQualityError::AnalysisError(format!("Failed to seek past {}: {}", context, e))
        })?;
        Ok(())
    }

    let mut sig = [0u8; 8];
    reader
        .read_exact(&mut sig)
        .map_err(|_| ImgQualityError::AnalysisError("PNG too small".to_string()))?;
    if sig != [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return Err(ImgQualityError::AnalysisError(
            "Invalid PNG signature".to_string(),
        ));
    }

    // Read IHDR
    let mut ihdr_header = [0u8; 8];
    reader
        .read_exact(&mut ihdr_header)
        .map_err(|_| ImgQualityError::AnalysisError("Missing IHDR".to_string()))?;
    let mut ihdr_data = [0u8; 13];
    reader
        .read_exact(&mut ihdr_data)
        .map_err(|_| ImgQualityError::AnalysisError("IHDR data truncated".to_string()))?;

    let width = u32::from_be_bytes([ihdr_data[0], ihdr_data[1], ihdr_data[2], ihdr_data[3]]);
    let height = u32::from_be_bytes([ihdr_data[4], ihdr_data[5], ihdr_data[6], ihdr_data[7]]);
    let bit_depth = ihdr_data[8];
    let color_type = ihdr_data[9];
    skip_bytes(&mut reader, 4, "IHDR CRC")?;

    let mut palette_size: Option<usize> = None;
    let mut has_trns = false;
    let mut has_text_chunks = false;
    let mut detected_tool: Option<String> = None;

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

    let mut buf = [0u8; 8];
    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(ImgQualityError::AnalysisError(format!(
                    "Failed to read PNG chunk header: {}",
                    e
                )));
            }
        }
        let chunk_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
        let chunk_type = &buf[4..8];

        match chunk_type {
            b"PLTE" if color_type == 3 => {
                palette_size = Some((chunk_len as usize) / 3);
                skip_bytes(&mut reader, chunk_len + 4, "PLTE chunk")?;
            }
            b"tRNS" => {
                has_trns = true;
                skip_bytes(&mut reader, chunk_len + 4, "tRNS chunk")?;
            }
            b"tEXt" | b"iTXt" | b"zTXt" if detected_tool.is_none() => {
                has_text_chunks = true;
                let mut payload = vec![0u8; chunk_len as usize];
                reader.read_exact(&mut payload).map_err(|e| {
                    ImgQualityError::AnalysisError(format!(
                        "Failed to read PNG text chunk payload: {}",
                        e
                    ))
                })?;
                if chunk_type == b"zTXt" {
                    // zTXt: keyword\0 + compression_method(1) + compressed_text
                    if let Some(null_pos) = payload.iter().position(|&b| b == 0) {
                        let keyword = String::from_utf8_lossy(&payload[..null_pos]);
                        for &(pattern, tool_name) in signatures {
                            if keyword.contains(pattern) {
                                detected_tool = Some(tool_name.to_string());
                                break;
                            }
                        }
                        if detected_tool.is_none() && null_pos + 2 < payload.len() {
                            let compressed = &payload[null_pos + 2..];
                            let mut decompressed = Vec::new();
                            if flate2::read::ZlibDecoder::new(compressed)
                                .read_to_end(&mut decompressed)
                                .is_ok()
                            {
                                let text = String::from_utf8_lossy(&decompressed);
                                for &(pattern, tool_name) in signatures {
                                    if text.contains(pattern) {
                                        detected_tool = Some(tool_name.to_string());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                if chunk_type != b"zTXt" {
                    let text = String::from_utf8_lossy(&payload);
                    for &(pattern, tool_name) in signatures {
                        if text.contains(pattern) {
                            detected_tool = Some(tool_name.to_string());
                            break;
                        }
                    }
                }
                skip_bytes(&mut reader, 4, "text chunk CRC")?;
            }
            b"IEND" => break,
            _ => {
                skip_bytes(&mut reader, chunk_len + 4, "PNG chunk")?;
            }
        }
    }

    Ok(PngStructureInfo {
        width,
        height,
        bit_depth,
        color_type,
        palette_size,
        has_trns,
        has_text_chunks,
        detected_tool,
    })
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

    let floyd_steinberg_score = (dithering_ratio * 5.0).min(1.0);

    // Bayer/ordered dithering: 2x2 checkerboard — diagonal pairs similar, cross pairs differ
    let mut bayer_count = 0u64;
    let mut bayer_total = 0u64;
    for y in (1..height.saturating_sub(1)).step_by(2) {
        for x in (1..width.saturating_sub(1)).step_by(2) {
            if (x + y * width) % step != 0 {
                continue;
            }
            let c00 = rgba.get_pixel(x, y);
            let c10 = rgba.get_pixel(x + 1, y);
            let c01 = rgba.get_pixel(x, y + 1);
            let c11 = rgba.get_pixel(x + 1, y + 1);
            let diag_diff = color_difference(c00, c11) + color_difference(c10, c01);
            let cross_diff = color_difference(c00, c10) + color_difference(c00, c01);
            if cross_diff > 40.0 && diag_diff < cross_diff * 0.5 {
                bayer_count += 1;
            }
            bayer_total += 1;
        }
    }
    let bayer_score = if bayer_total > 0 {
        ((bayer_count as f64 / bayer_total as f64) * 4.0).min(1.0)
    } else {
        0.0
    };

    floyd_steinberg_score.max(bayer_score)
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

/// Color frequency concentration — quantized images have a few dominant colors
/// covering most pixels. Natural palette art distributes more evenly.
/// Returns score in [0.0, 1.0] where high = likely quantized.
fn detect_color_frequency_distribution(img: &DynamicImage) -> f64 {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let total_pixels = (width as usize) * (height as usize);
    if total_pixels < 100 {
        return 0.0;
    }

    // Block-random sampling: divide image into a grid of blocks, sample one pixel
    // per block at a deterministic-but-spread position. Avoids stride bias where
    // step-based sampling always hits the same spatial columns/rows.
    let target_samples: usize = 50_000.min(total_pixels);
    let block_size = ((total_pixels as f64 / target_samples as f64).max(1.0)) as usize;
    let blocks_x = (width as usize).div_ceil(block_size);
    let blocks_y = (height as usize).div_ceil(block_size);

    let mut color_freq: std::collections::HashMap<[u8; 4], u32> = std::collections::HashMap::new();
    let mut sampled = 0u64;

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            // Pick a pixel near the center of each block (deterministic, no RNG needed)
            let px = ((bx * block_size + block_size / 2) as u32).min(width - 1);
            let py = ((by * block_size + block_size / 2) as u32).min(height - 1);
            let pixel = rgba.get_pixel(px, py);
            let key = [pixel[0], pixel[1], pixel[2], pixel[3]];
            *color_freq.entry(key).or_insert(0) += 1;
            sampled += 1;
        }
    }

    if sampled == 0 || color_freq.len() < 2 {
        return 0.0;
    }

    let mut freqs: Vec<u32> = color_freq.values().copied().collect();
    freqs.sort_unstable_by(|a, b| b.cmp(a));

    // How many distinct colors cover 85% of sampled pixels?
    let target = (sampled as f64 * 0.85) as u64;
    let mut cumulative = 0u64;
    let mut colors_for_85pct = 0usize;
    for &f in &freqs {
        cumulative += f as u64;
        colors_for_85pct += 1;
        if cumulative >= target {
            break;
        }
    }

    // Low ratio = few colors dominate = quantized
    let coverage_ratio = colors_for_85pct as f64 / freqs.len() as f64;

    if coverage_ratio < 0.05 {
        0.85
    } else if coverage_ratio < 0.10 {
        0.70
    } else if coverage_ratio < 0.20 {
        0.50
    } else if coverage_ratio < 0.35 {
        0.25
    } else {
        0.0
    }
}

fn detect_gradient_banding(img: &DynamicImage) -> f64 {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    if width < 16 || height < 16 {
        return 0.0;
    }

    // Per-channel detection weighted by human visual sensitivity (G > R > B).
    // Grayscale projection loses hue info — red vs blue map to similar luma,
    // causing missed banding in single-channel gradients.
    let channel_weights = [0.30f64, 0.59, 0.11]; // R, G, B
    let mut total_score = 0.0f64;

    for (ch, &weight) in channel_weights.iter().enumerate() {
        let mut banding_score = 0.0f64;
        let mut gradient_regions = 0u32;

        for y in (0..height).step_by(4) {
            let mut prev_val = rgba.get_pixel(0, y)[ch] as i16;
            let mut gradient_length = 0u32;
            let mut step_count = 0u32;
            let mut last_step_x = 0u32;

            for x in 1..width {
                let val = rgba.get_pixel(x, y)[ch] as i16;
                let diff = (val - prev_val).abs();

                if diff > 0 && diff < 20 {
                    gradient_length += 1;
                    // Require step width > 3px to reduce false positives on natural gradients
                    if diff > 3 && x - last_step_x > 3 {
                        step_count += 1;
                        last_step_x = x;
                    }
                } else if gradient_length > 20 {
                    if step_count > 0 {
                        let step_ratio = step_count as f64 / gradient_length as f64;
                        if step_ratio > 0.08 && step_ratio < 0.5 {
                            banding_score += step_ratio;
                            gradient_regions += 1;
                        }
                    }
                    gradient_length = 0;
                    step_count = 0;
                    last_step_x = x;
                }
                prev_val = val;
            }
        }

        let ch_score = if gradient_regions > 0 {
            (banding_score / gradient_regions as f64).min(1.0)
        } else {
            0.0
        };
        total_score += ch_score * weight;
    }

    // Diagonal scan on luma for efficiency — catches diagonal gradients
    let gray = img.to_luma8();
    let mut diag_banding = 0.0f64;
    let mut diag_regions = 0u32;
    let diag_step: usize = 8;

    for start_offset in (0..width.max(height)).step_by(diag_step) {
        // Top-left to bottom-right diagonals from top edge
        if start_offset < width {
            let mut x = start_offset;
            let mut y = 0u32;
            let mut prev_val = gray.get_pixel(x, y)[0] as i16;
            let mut grad_len = 0u32;
            let mut steps = 0u32;

            while {
                x += 1;
                y += 1;
                x < width && y < height
            } {
                let val = gray.get_pixel(x, y)[0] as i16;
                let diff = (val - prev_val).abs();
                if diff > 0 && diff < 20 {
                    grad_len += 1;
                    if diff > 3 {
                        steps += 1;
                    }
                } else if grad_len > 20 && steps > 0 {
                    let r = steps as f64 / grad_len as f64;
                    if r > 0.08 && r < 0.5 {
                        diag_banding += r;
                        diag_regions += 1;
                    }
                    grad_len = 0;
                    steps = 0;
                } else {
                    grad_len = 0;
                    steps = 0;
                }
                prev_val = val;
            }
        }

        // Top-right to bottom-left diagonals from top edge
        if start_offset < width && start_offset > 0 {
            let mut x = start_offset;
            let mut y = 0u32;
            let mut prev_val = gray.get_pixel(x, y)[0] as i16;
            let mut grad_len = 0u32;
            let mut steps = 0u32;

            while x > 0 && y + 1 < height {
                x -= 1;
                y += 1;
                let val = gray.get_pixel(x, y)[0] as i16;
                let diff = (val - prev_val).abs();
                if diff > 0 && diff < 20 {
                    grad_len += 1;
                    if diff > 3 {
                        steps += 1;
                    }
                } else if grad_len > 20 && steps > 0 {
                    let r = steps as f64 / grad_len as f64;
                    if r > 0.08 && r < 0.5 {
                        diag_banding += r;
                        diag_regions += 1;
                    }
                    grad_len = 0;
                    steps = 0;
                } else {
                    grad_len = 0;
                    steps = 0;
                }
                prev_val = val;
            }
        }
    }

    let diag_score = if diag_regions > 0 {
        (diag_banding / diag_regions as f64).min(1.0)
    } else {
        0.0
    };

    // Combine: per-channel horizontal (70%) + diagonal luma (30%)
    (total_score * 0.70 + diag_score * 0.30).min(1.0)
}

fn estimate_uncompressed_size(info: &PngStructureInfo) -> u64 {
    let bits_per_sample: u64 = match info.color_type {
        0 => 1, // grayscale: 1 channel
        2 => 3, // RGB: 3 channels
        3 => 1, // indexed: 1 index per pixel
        4 => 2, // grayscale + alpha: 2 channels
        6 => 4, // RGBA: 4 channels
        _ => 4,
    };

    // bit_depth applies per sample; for sub-byte depths (1, 2, 4) pixels are packed
    let total_bits =
        info.width as u64 * info.height as u64 * bits_per_sample * info.bit_depth as u64;
    // Round up to bytes
    total_bits.div_ceil(8)
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

    let img =
        open_image_with_limits(path).map_err(|e| ImgQualityError::ImageReadError(e.to_string()))?;
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

    #[allow(clippy::field_reassign_with_default)]
    let mut precision = PrecisionMetadata::default();

    match format {
        DetectedFormat::PNG => {
            let data = std::fs::read(path)?;
            let mut cursor = std::io::Cursor::new(&data);
            if let Ok(info) = parse_png_structure(&mut cursor) {
                precision.bit_depth = Some(info.bit_depth);
                precision.palette_size = info.palette_size;
                precision.color_type = Some(info.color_type);
                precision.is_lossless_deterministic = true;
            }
        }
        DetectedFormat::GIF => {
            if let Ok(gif_prec) = parse_gif_precision_metadata(path) {
                precision = gif_prec;
            }
        }
        DetectedFormat::TIFF => {
            if let Ok(comp) = detect_tiff_compression(path) {
                precision.is_lossless_deterministic = comp == CompressionType::Lossless;
                // TIFF bit depth is usually in Tag 258, but Image crate handles basic ones.
                // For now, we flag deterministic lossless.
            }
        }
        DetectedFormat::WebP => {
            let data = std::fs::read(path)?;
            if crate::image_formats::webp::is_animated_from_bytes(&data) {
                if let Ok(comp) = detect_webp_animation_compression(&data) {
                    precision.is_lossless_deterministic = comp == CompressionType::Lossless;
                }
            } else {
                precision.is_lossless_deterministic =
                    crate::image_formats::webp::is_lossless_from_bytes(&data);
                if !precision.is_lossless_deterministic {
                    precision.quality_estimate = estimate_webp_quality(path).ok();
                }
            }
        }
        DetectedFormat::JPEG => {
            precision.is_lossless_deterministic = false;
            precision.quality_estimate = estimate_jpeg_quality(path).ok();
        }
        DetectedFormat::HEIC | DetectedFormat::HEIF => {
            if let Ok(comp) = detect_heic_compression(path) {
                precision.is_lossless_deterministic = comp == CompressionType::Lossless;
            }
        }
        DetectedFormat::AVIF => {
            if let Ok(comp) = detect_avif_compression(path) {
                precision.is_lossless_deterministic = comp == CompressionType::Lossless;
            }
        }
        _ => {}
    }

    let mut estimated_quality = if format == DetectedFormat::JPEG
        || (format == DetectedFormat::WebP && compression == CompressionType::Lossy)
    {
        precision.quality_estimate
    } else {
        None
    };

    if estimated_quality.is_none() && compression == CompressionType::Lossy {
        estimated_quality = Some(estimate_lossy_quality_fallback(
            path,
            &format,
            width,
            height,
            file_size,
            frame_count,
            entropy,
        )?);
    }

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
        precision,
    })
}

fn estimate_lossy_quality_fallback(
    path: &Path,
    format: &DetectedFormat,
    width: u32,
    height: u32,
    file_size: u64,
    frame_count: u32,
    entropy: f64,
) -> Result<u8> {
    let pixels = (width as u64) * (height as u64);
    if pixels == 0 || file_size == 0 {
        crate::progress_mode::emit_stderr(&format!(
            "   \x1b[1;31m🚨 [CRITICAL FALLBACK]\x1b[0m \x1b[31mQuality detection failed and heuristic fallback is impossible.\x1b[0m\n\
               \x1b[31m      File: {}\x1b[0m\n\
               \x1b[31m      Refusing to invent a hardcoded quality value.\x1b[0m",
            path.display()
        ));
        return Err(ImgQualityError::AnalysisError(format!(
            "Cannot estimate quality for lossy {}: invalid dimensions ({width}x{height}) or empty file",
            format.as_str()
        )));
    }

    // Heuristic v2: Multi-factor quality estimation
    let raw_bpp = (file_size * 8) as f64 / pixels as f64 / (frame_count.max(1) as f64);

    // Format efficiency multiplier (relative to JPEG)
    // AVIF/HEIC ~ 3.0x, WebP ~ 1.5x
    let efficiency_factor = match format {
        DetectedFormat::AVIF | DetectedFormat::HEIC | DetectedFormat::HEIF => 3.0,
        DetectedFormat::WebP => 1.5,
        _ => 1.0,
    };

    // Entropy compensation:
    // High entropy (>7.5) means complex texture, needs more BPP for same quality
    // Low entropy (<4.0) means flat colors, quality is higher even with low BPP
    let entropy_adj = (7.5 / entropy.max(1.0)).sqrt().clamp(0.7, 1.3);

    let effective_bpp = raw_bpp * efficiency_factor * entropy_adj;
    let bpp_quality =
        (70.0 + 15.0 * (effective_bpp * 5.0).max(0.001).log2()).clamp(10.0, 100.0) as u8;

    crate::progress_mode::emit_stderr(&format!(
        "   \x1b[1;33m⚠️  [QUALITY FALLBACK]\x1b[0m \x1b[33mExact detection unavailable for {} codec.\x1b[0m\n\
           \x1b[33m      File: {}\x1b[0m\n\
           \x1b[33m      Heuristic: BPP={:.3}, Eff={:.1}x, Entropy={:.2} -> \x1b[1;32mEstimated Q: {}\x1b[0m",
        format.as_str(),
        path.display(),
        raw_bpp,
        efficiency_factor,
        entropy,
        bpp_quality
    ));

    Ok(bpp_quality)
}

fn estimate_jpeg_quality(path: &Path) -> Result<u8> {
    let data = std::fs::read(path)?;
    use crate::image_jpeg_analysis::analyze_jpeg_quality;
    let analysis = analyze_jpeg_quality(&data).map_err(ImgQualityError::AnalysisError)?;
    Ok(analysis.estimated_quality as u8)
}

/// Estimate WebP VP8 quality by parsing the bitstream quantization index.
fn estimate_webp_quality(path: &Path) -> Result<u8> {
    crate::image_formats::webp::estimate_quality(path)
}

/// Parse APNG (Animated PNG) frame count from PNG data
/// Returns (is_animated, frame_count)
pub(crate) fn parse_apng_frames(data: &[u8]) -> (bool, u32) {
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
                let num_frames =
                    u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
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
fn detect_webp_animation_compression(data: &[u8]) -> Result<CompressionType> {
    if crate::image_formats::webp::detect_webp_animation_is_lossless(data)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect TIFF compression type — traverses ALL IFDs. Supports both standard TIFF and BigTIFF.
fn detect_tiff_compression(path: &Path) -> Result<CompressionType> {
    if crate::image_formats::tiff::is_lossless(path)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect AVIF lossless encoding — multi-dimension analysis.
fn detect_avif_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if crate::image_formats::avif::is_lossless_from_bytes(&data, path)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect HEIC/HEIF lossless encoding — multi-dimension analysis.
fn detect_heic_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if crate::image_heic_analysis::detect_heic_is_lossless(&data, path)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect ICO compression by inspecting embedded image entries.
///
/// ICO directory: header[6] + entries[16 each]. Each entry has an offset to image data.
/// If image data starts with PNG magic → recursively check PNG quantization.
/// Any quantized PNG entry → Lossy. Otherwise → Lossless.
fn detect_ico_compression(path: &Path) -> Result<CompressionType> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(ImgQualityError::IoError)?;

    // ICO header: reserved(2) + type(2) + count(2) = 6 bytes
    let mut header = [0u8; 6];
    if file.read_exact(&mut header).is_err() {
        return Ok(CompressionType::Lossless);
    }

    let image_count = u16::from_le_bytes([header[4], header[5]]) as usize;
    let png_magic: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    // Each directory entry is 16 bytes, starting at offset 6
    for i in 0..image_count {
        let entry_offset = 6 + (i as u64) * 16;
        file.seek(SeekFrom::Start(entry_offset))
            .map_err(ImgQualityError::IoError)?;

        let mut entry = [0u8; 16];
        if file.read_exact(&mut entry).is_err() {
            break;
        }

        // Bytes 8-11: size of image data, bytes 12-15: offset of image data
        let img_size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as u64;
        let img_offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as u64;

        // Peak into image data for PNG magic
        file.seek(SeekFrom::Start(img_offset))
            .map_err(ImgQualityError::IoError)?;
        let mut magic_peek = [0u8; 8];
        if file.read_exact(&mut magic_peek).is_ok() && magic_peek == png_magic {
            // Seek back to start of image data for full analysis
            file.seek(SeekFrom::Start(img_offset))
                .map_err(ImgQualityError::IoError)?;
            let mut img_reader = (&file).take(img_size);
            // Since analyze_png_quantization_from_reader needs Seek, and take() doesn't provide it easily,
            // we read the PNG part into memory. BUT: PNGs inside ICO are usually small (max 512KB for 256x256).
            // This is infinitely safer than loading the whole 64MB ICO.
            let mut png_data = Vec::with_capacity(img_size as usize);
            img_reader
                .read_to_end(&mut png_data)
                .map_err(ImgQualityError::IoError)?;

            if let Ok(analysis) = analyze_png_quantization_from_bytes(&png_data) {
                if analysis.is_quantized {
                    return Ok(CompressionType::Lossy);
                }
            }
        }
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
        // Fallback to lossless for corrupted/invalid EXR files (safe default)
        return Ok(CompressionType::Lossless);
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
                u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as usize;
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
                if wavelet == 1 {
                    "5/3 reversible — lossless"
                } else {
                    "9/7 irreversible — lossy"
                }
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
                if *wavelet == 1 {
                    "5/3 reversible — lossless"
                } else {
                    "9/7 irreversible — lossy"
                }
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
        let size =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let box_type = &data[pos + 4..pos + 8];

        if box_type == b"jp2c" {
            return Some(pos + 8);
        }

        if size == 0 {
            break;
        } else if size == 1 {
            if pos + 16 > data.len() {
                break;
            }
            let ext = u64::from_be_bytes([
                data[pos + 8],
                data[pos + 9],
                data[pos + 10],
                data[pos + 11],
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
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
        if pos + 4 > cs.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([cs[pos + 2], cs[pos + 3]]) as usize;
        pos += 2 + seg_len;
    }

    (cod_wavelet, coc_wavelets)
}

/// Detect JXL (JPEG XL) lossless encoding — multi-dimension analysis.
fn detect_jxl_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if crate::image_formats::jxl::is_lossless_from_bytes(&data, path)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_detect_png_format() {
        let png_magic: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        let mut data = png_magic.to_vec();
        data.extend_from_slice(&[0u8; 24]);
        file.write_all(&data).expect("Failed to write");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "PNG format detection should succeed");
        assert_eq!(
            result.unwrap(),
            DetectedFormat::PNG,
            "Should be detected as PNG format"
        );
    }

    #[test]
    fn test_detect_jpeg_format() {
        let jpeg_magic: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0];
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        let mut data = jpeg_magic.to_vec();
        data.extend_from_slice(&[0u8; 28]);
        file.write_all(&data).expect("Failed to write");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "JPEG format detection should succeed");
        assert_eq!(
            result.unwrap(),
            DetectedFormat::JPEG,
            "Should be detected as JPEG format"
        );
    }

    #[test]
    fn test_detect_gif_format() {
        let gif_magic: &[u8] = b"GIF89a";
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        let mut data = gif_magic.to_vec();
        data.extend_from_slice(&[0u8; 26]);
        file.write_all(&data).expect("Failed to write");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "GIF format detection should succeed");
        assert_eq!(
            result.unwrap(),
            DetectedFormat::GIF,
            "Should be detected as GIF format"
        );
    }

    #[test]
    fn test_detect_webp_format() {
        let mut webp_data = b"RIFF".to_vec();
        webp_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        webp_data.extend_from_slice(b"WEBP");
        webp_data.extend_from_slice(&[0u8; 20]);

        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        file.write_all(&webp_data).expect("Failed to write");

        let result = detect_format_from_bytes(file.path());
        assert!(result.is_ok(), "WebP format detection should succeed");
        assert_eq!(
            result.unwrap(),
            DetectedFormat::WebP,
            "Should be detected as WebP format"
        );
    }

    #[test]
    fn test_detect_unknown_format() {
        let random_data: &[u8] = &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05];
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        let mut data = random_data.to_vec();
        data.extend_from_slice(&[0u8; 26]);
        file.write_all(&data).expect("Failed to write");

        let result = detect_format_from_bytes(file.path());
        assert!(
            result.is_ok(),
            "Unknown format detection should succeed (return Unknown)"
        );
        match result.unwrap() {
            DetectedFormat::Unknown(_) => (),
            other => panic!("Should be detected as Unknown format, actual {:?}", other),
        }
    }

    #[test]
    fn test_detect_nonexistent_file() {
        let result = detect_format_from_bytes(std::path::Path::new("/nonexistent/file.png"));
        assert!(result.is_err(), "Non-existent file should return error");
    }

    #[test]
    fn test_parse_png_structure_rejects_truncated_text_chunk() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        data.extend_from_slice(&13u32.to_be_bytes());
        data.extend_from_slice(b"IHDR");
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&[8, 2, 0, 0, 0]);
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&4u32.to_be_bytes());
        data.extend_from_slice(b"tEXt");
        data.extend_from_slice(b"ab");

        match parse_png_structure(std::io::Cursor::new(data)) {
            Ok(_) => panic!("truncated text chunk should fail loudly"),
            Err(err) => assert!(err.to_string().contains("text chunk payload")),
        }
    }

    #[test]
    fn test_estimate_lossy_quality_fallback_rejects_invalid_dimensions() {
        let err = estimate_lossy_quality_fallback(
            std::path::Path::new("/tmp/fake-lossy.webp"),
            &DetectedFormat::WebP,
            0,
            1080,
            12345,
            1,
            5.0,
        )
        .expect_err("invalid dimensions should not produce a hardcoded fallback quality");

        match err {
            ImgQualityError::AnalysisError(message) => {
                assert!(message.contains("Cannot estimate quality"));
                assert!(message.contains("invalid dimensions"));
            }
            other => panic!("expected AnalysisError, got {:?}", other),
        }
    }
}
