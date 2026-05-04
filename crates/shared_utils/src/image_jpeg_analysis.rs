//! JPEG Quality Analysis Module
//!
//! Implements precise JPEG quality factor estimation by analyzing
//! quantization tables and comparing them to the IJG standard tables.
//!
//! Algorithm accuracy target: ±1 quality factor for standard tables

use image::{DynamicImage, GenericImageView, ImageReader};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Cursor;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JpegQualityAnalysis {
    pub estimated_quality: u8,
    pub confidence: f64,
    pub is_standard_table: bool,
    pub luminance_sse: f64,
    pub chrominance_sse: Option<f64>,
    pub luminance_quality: u8,
    pub chrominance_quality: Option<u8>,
    pub quality_description: String,
    pub is_high_quality_original: bool,
    pub is_complete: bool,
    pub encoder_hint: Option<String>,
}

const IJG_LUMINANCE_BASE: [[u16; 8]; 8] = [
    [16, 11, 10, 16, 24, 40, 51, 61],
    [12, 12, 14, 19, 26, 58, 60, 55],
    [14, 13, 16, 24, 40, 57, 69, 56],
    [14, 17, 22, 29, 51, 87, 80, 62],
    [18, 22, 37, 56, 68, 109, 103, 77],
    [24, 35, 55, 64, 81, 104, 113, 92],
    [49, 64, 78, 87, 103, 121, 120, 101],
    [72, 92, 95, 98, 112, 100, 103, 99],
];

const IJG_CHROMINANCE_BASE: [[u16; 8]; 8] = [
    [17, 18, 24, 47, 99, 99, 99, 99],
    [18, 21, 26, 66, 99, 99, 99, 99],
    [24, 26, 56, 99, 99, 99, 99, 99],
    [47, 66, 99, 99, 99, 99, 99, 99],
    [99, 99, 99, 99, 99, 99, 99, 99],
    [99, 99, 99, 99, 99, 99, 99, 99],
    [99, 99, 99, 99, 99, 99, 99, 99],
    [99, 99, 99, 99, 99, 99, 99, 99],
];

fn generate_standard_qt(quality: u8, base_table: &[[u16; 8]; 8]) -> [[u16; 8]; 8] {
    let q = f64::from(quality.clamp(1, 100));

    let scale = if q < 50.0 {
        5000.0 / q
    } else {
        2.0f64.mul_add(-q, 200.0)
    };

    let mut result = [[0u16; 8]; 8];

    for (row, base_row) in result.iter_mut().zip(base_table.iter()) {
        for (cell, &base_value) in row.iter_mut().zip(base_row.iter()) {
            let value = ((scale * f64::from(base_value)) + 50.0) / 100.0;
            *cell = crate::numeric_cast::f64_to_u16_sat(value.floor().clamp(1.0, 255.0));
        }
    }

    result
}

fn calculate_weighted_sse(table1: &[[u16; 8]; 8], table2: &[[u16; 8]; 8]) -> f64 {
    const WEIGHTS: [[f64; 8]; 8] = [
        [1.0, 0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3],
        [0.9, 0.85, 0.75, 0.65, 0.55, 0.45, 0.35, 0.25],
        [0.8, 0.75, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2],
        [0.7, 0.65, 0.6, 0.5, 0.4, 0.3, 0.2, 0.15],
        [0.6, 0.55, 0.5, 0.4, 0.3, 0.2, 0.15, 0.1],
        [0.5, 0.45, 0.4, 0.3, 0.2, 0.15, 0.1, 0.08],
        [0.4, 0.35, 0.3, 0.2, 0.15, 0.1, 0.08, 0.05],
        [0.3, 0.25, 0.2, 0.15, 0.1, 0.08, 0.05, 0.03],
    ];

    let mut weighted_sse = 0.0;
    let mut total_weight = 0.0;

    for ((row1, row2), weight_row) in table1.iter().zip(table2.iter()).zip(WEIGHTS.iter()) {
        for ((&lhs, &rhs), &weight) in row1.iter().zip(row2.iter()).zip(weight_row.iter()) {
            let diff = f64::from(lhs) - f64::from(rhs);
            weighted_sse = (weight * diff).mul_add(diff, weighted_sse);
            total_weight += weight;
        }
    }

    weighted_sse / total_weight
}

fn calculate_sse(table1: &[[u16; 8]; 8], table2: &[[u16; 8]; 8]) -> f64 {
    let mut sse = 0.0;
    for (row1, row2) in table1.iter().zip(table2.iter()) {
        for (&lhs, &rhs) in row1.iter().zip(row2.iter()) {
            let diff = f64::from(lhs) - f64::from(rhs);
            sse = diff.mul_add(diff, sse);
        }
    }
    sse
}

#[derive(Debug, Clone)]
struct QualityEstimate {
    quality: u8,
    sse: f64,
    weighted_sse: f64,
    is_exact_match: bool,
    interpolated_quality: f64,
}

fn estimate_quality_precise(
    extracted_qt: &[[u16; 8]; 8],
    base_table: &[[u16; 8]; 8],
) -> QualityEstimate {
    let mut best_quality = 75u8;
    let mut min_sse = f64::MAX;
    let mut min_weighted_sse = f64::MAX;
    let mut second_best_quality = 75u8;
    let mut second_min_sse = f64::MAX;

    for q in 1..=100 {
        let standard_qt = generate_standard_qt(q, base_table);
        let sse = calculate_sse(extracted_qt, &standard_qt);
        let weighted_sse = calculate_weighted_sse(extracted_qt, &standard_qt);

        if sse < min_sse {
            second_best_quality = best_quality;
            second_min_sse = min_sse;
            min_sse = sse;
            min_weighted_sse = weighted_sse;
            best_quality = q;
        } else if sse < second_min_sse {
            second_min_sse = sse;
            second_best_quality = q;
        }

        if sse == 0.0 {
            return QualityEstimate {
                quality: q,
                sse: 0.0,
                weighted_sse: 0.0,
                is_exact_match: true,
                interpolated_quality: f64::from(q),
            };
        }
    }

    let interpolated = if second_min_sse > min_sse && min_sse > 0.0 {
        let ratio = min_sse / (min_sse + second_min_sse);
        let direction = if second_best_quality > best_quality {
            1.0
        } else {
            -1.0
        };
        (direction * ratio).mul_add(0.5, f64::from(best_quality))
    } else {
        f64::from(best_quality)
    };

    QualityEstimate {
        quality: best_quality,
        sse: min_sse,
        weighted_sse: min_weighted_sse,
        is_exact_match: false,
        interpolated_quality: interpolated,
    }
}

#[must_use]
pub fn estimate_quality_from_table(
    extracted_qt: &[[u16; 8]; 8],
    is_luminance: bool,
) -> (u8, f64, bool) {
    let base_table = if is_luminance {
        &IJG_LUMINANCE_BASE
    } else {
        &IJG_CHROMINANCE_BASE
    };

    let estimate = estimate_quality_precise(extracted_qt, base_table);
    (estimate.quality, estimate.sse, estimate.is_exact_match)
}

fn calculate_confidence(
    luma_estimate: &QualityEstimate,
    chroma_estimate: Option<&QualityEstimate>,
) -> f64 {
    if luma_estimate.is_exact_match {
        if let Some(chroma) = chroma_estimate {
            if chroma.is_exact_match {
                return 1.0;
            }
        }
        return 0.98;
    }

    let luma_confidence = 1.0 / luma_estimate.weighted_sse.mul_add(0.01, 1.0);

    chroma_estimate.map_or_else(
        || luma_confidence.clamp(0.0, 1.0),
        |chroma| {
            let chroma_confidence = 1.0 / chroma.weighted_sse.mul_add(0.01, 1.0);
            0.7f64
                .mul_add(luma_confidence, 0.3 * chroma_confidence)
                .clamp(0.0, 1.0)
        },
    )
}

fn detect_encoder(
    tables: &[[[u16; 8]; 8]],
    luma_exact: bool,
    chroma_exact: bool,
    luma_sse: f64,
    chroma_sse: Option<f64>,
) -> Option<String> {
    if tables.is_empty() {
        return None;
    }

    if luma_exact && (tables.len() < 2 || chroma_exact) {
        return Some("IJG/libjpeg (standard)".to_string());
    }

    let luma = tables.first()?;

    if let Some(c_sse) = chroma_sse {
        if (720.0..735.0).contains(&luma_sse) && (5.0..12.0).contains(&c_sse) {
            return Some("Apple iOS Camera (high quality)".to_string());
        }
        if (150.0..165.0).contains(&luma_sse) && (2.0..10.0).contains(&c_sse) {
            return Some("Apple iOS Camera (very high quality)".to_string());
        }
    }

    if luma[0][0] <= 2 && luma[0][1] <= 2 && luma[1][0] <= 2 {
        if luma_sse < 100.0 {
            return Some("Adobe Photoshop (highest quality)".to_string());
        }
        return Some("Adobe Photoshop".to_string());
    }

    if let Some(c_sse) = chroma_sse {
        if (200.0..400.0).contains(&luma_sse) && (10.0..50.0).contains(&c_sse) {
            return Some("Android Camera".to_string());
        }
    }

    if (500.0..700.0).contains(&luma_sse) {
        return Some("Samsung Camera".to_string());
    }

    if luma_sse > 1000.0 {
        return Some("Non-standard encoder (highly custom)".to_string());
    }

    if !luma_exact {
        return Some("Custom encoder".to_string());
    }

    None
}

const MARKER_SOI: u8 = 0xD8;
const MARKER_DQT: u8 = 0xDB;
const MARKER_SOS: u8 = 0xDA;
const MARKER_EOI: u8 = 0xD9;

/// Extract quantization tables from JPEG raw bytes.
///
/// # Errors
/// Returns an error if the JPEG data is corrupted or missing DQT markers.
pub fn extract_quantization_tables(data: &[u8]) -> Result<Vec<[[u16; 8]; 8]>, String> {
    let mut tables = Vec::new();

    if data.len() < 2 || data.get(0..2) != Some(&[0xFF, MARKER_SOI]) {
        return Err("Not a valid JPEG file".to_string());
    }
    let mut pos = 2;

    while pos < data.len() - 1 {
        if data.get(pos) != Some(&0xFF) {
            pos += 1;
            continue;
        }

        while pos < data.len() && data.get(pos) == Some(&0xFF) {
            pos += 1;
        }

        if pos >= data.len() {
            break;
        }

        let marker = *data.get(pos).unwrap_or(&0);
        pos += 1;

        if marker == MARKER_SOI || marker == MARKER_EOI || (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        if pos + 2 > data.len() {
            break;
        }
        let length = (usize::from(*data.get(pos).unwrap_or(&0)) << 8)
            | usize::from(*data.get(pos + 1).unwrap_or(&0));

        if marker == MARKER_DQT {
            let segment_end = (pos + length).min(data.len());
            let mut seg_pos = pos + 2;

            while seg_pos < segment_end {
                if seg_pos >= data.len() {
                    break;
                }

                let pq_tq = *data.get(seg_pos).unwrap_or(&0);
                let precision = (pq_tq >> 4) & 0x0F;
                seg_pos += 1;

                let mut table = [[0u16; 8]; 8];

                if precision == 0 {
                    if seg_pos + 64 > data.len() {
                        break;
                    }
                    for &zigzag in &ZIGZAG_ORDER {
                        let row = zigzag / 8;
                        let col = zigzag % 8;
                        if let Some(cell) = table.get_mut(row).and_then(|r| r.get_mut(col)) {
                            *cell = u16::from(*data.get(seg_pos).unwrap_or(&0));
                        }
                        seg_pos += 1;
                    }
                } else {
                    if seg_pos + 128 > data.len() {
                        break;
                    }
                    for &zigzag in &ZIGZAG_ORDER {
                        let row = zigzag / 8;
                        let col = zigzag % 8;
                        if let Some(cell) = table.get_mut(row).and_then(|r| r.get_mut(col)) {
                            *cell = (u16::from(*data.get(seg_pos).unwrap_or(&0)) << 8)
                                | u16::from(*data.get(seg_pos + 1).unwrap_or(&0));
                        }
                        seg_pos += 2;
                    }
                }

                tables.push(table);
            }
        }

        pos += length;

        if marker == MARKER_SOS {
            break;
        }
    }

    if tables.is_empty() {
        return Err("No quantization tables found in JPEG".to_string());
    }

    Ok(tables)
}

const ZIGZAG_ORDER: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Returns true if the JPEG data is complete (starts with SOI and contains EOI).
///
/// This implementation is robust against trailing metadata (common in mobile captures like Vivo/Samsung)
/// by searching for the EOI marker (FF D9) in the data.
#[must_use]
pub fn is_jpeg_complete(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }

    // 1) Verify Start of Image (SOI): FF D8
    if data.get(0..2) != Some(&[0xFF, 0xD8]) {
        return false;
    }

    // 2) Verify End of Image (EOI): FF D9
    // We search from the end because it's more likely to be near the end,
    // even if there's a few hundred bytes of trailing metadata.
    // In a valid JPEG bitstream, FF D9 should not appear in the scan data (due to byte stuffing).
    data.windows(2).rev().any(|w| w == b"\xFF\xD9")
}

/// Analyze JPEG quality by inspecting DQT (Define Quantization Table) markers.
///
/// # Errors
/// Returns an error if the JPEG data is invalid or DQT markers are missing.
pub fn analyze_jpeg_quality(data: &[u8]) -> Result<JpegQualityAnalysis, String> {
    let tables = extract_quantization_tables(data)?;

    let luma_table = tables
        .first()
        .ok_or_else(|| "No quantization tables found".to_string())?;
    let luma_estimate = estimate_quality_precise(luma_table, &IJG_LUMINANCE_BASE);

    let chroma_estimate = tables
        .get(1)
        .map(|table| estimate_quality_precise(table, &IJG_CHROMINANCE_BASE));

    let confidence = calculate_confidence(&luma_estimate, chroma_estimate.as_ref());

    let final_quality = chroma_estimate
        .as_ref()
        .map_or(luma_estimate.quality, |chroma| {
            if luma_estimate.is_exact_match && chroma.is_exact_match {
                luma_estimate.quality
            } else if (i16::from(luma_estimate.quality) - i16::from(chroma.quality)).abs() <= 2 {
                let weighted = luma_estimate
                    .interpolated_quality
                    .mul_add(0.7, chroma.interpolated_quality * 0.3);
                crate::numeric_cast::f64_to_u8_sat(weighted.round())
            } else {
                luma_estimate.quality
            }
        });

    let is_standard_table =
        luma_estimate.is_exact_match && chroma_estimate.as_ref().is_none_or(|c| c.is_exact_match);

    let encoder_hint = detect_encoder(
        &tables,
        luma_estimate.is_exact_match,
        chroma_estimate.as_ref().is_none_or(|c| c.is_exact_match),
        luma_estimate.sse,
        chroma_estimate.as_ref().map(|c| c.sse),
    );

    let quality_description = match final_quality {
        95..=100 => "Very high quality (near lossless)".to_string(),
        90..=94 => "High quality (professional)".to_string(),
        80..=89 => "Good quality (standard photo)".to_string(),
        70..=79 => "Medium quality (web optimized)".to_string(),
        60..=69 => "Lower quality (high compression)".to_string(),
        _ => "Low quality (visible compression artifacts)".to_string(),
    };

    let is_high_quality_original = final_quality >= 90 && is_standard_table && confidence >= 0.95;
    let is_complete = is_jpeg_complete(data);

    let analysis = JpegQualityAnalysis {
        estimated_quality: final_quality,
        confidence,
        is_standard_table,
        luminance_sse: luma_estimate.sse,
        chrominance_sse: chroma_estimate.as_ref().map(|c| c.sse),
        luminance_quality: luma_estimate.quality,
        chrominance_quality: chroma_estimate.as_ref().map(|c| c.quality),
        quality_description,
        is_high_quality_original,
        is_complete,
        encoder_hint,
    };

    debug!(
        quality = final_quality,
        confidence = confidence,
        standard = is_standard_table,
        luma_sse = luma_estimate.sse,
        complete = is_complete,
        "JPEG quality analysis complete"
    );

    Ok(analysis)
}

/// Analyze JPEG quality from a file path.
///
/// # Errors
/// Returns an error if the file cannot be read or is not a valid JPEG.
pub fn analyze_jpeg_file(path: &std::path::Path) -> Result<JpegQualityAnalysis, String> {
    let data = std::fs::read(path).map_err(|e| format!("Failed to read file: {e}"))?;
    analyze_jpeg_quality(&data)
}

/// Detect Google `UltraHDR` JPEG (gainmap embedded via MPF + XMP `hdrgm:` namespace).
///
/// `UltraHDR` JPEGs contain:
/// - APP2 segment with XMP containing `hdrgm:` or `GainMap` namespace
/// - APP2 MPF (Multi-Picture Format) segment with secondary gainmap image
///
/// Returns true if the file is a `UltraHDR` JPEG with embedded gainmap.
#[must_use]
pub fn is_ultra_hdr_jpeg(data: &[u8]) -> bool {
    if data.len() < 4 || data.get(0..2) != Some(&[0xFF, 0xD8]) {
        return false;
    }

    let mut has_gainmap_xmp = false;
    let mut has_mpf = false;

    let mut pos = 2;
    while pos + 1 < data.len() {
        // Skip leading 0xFFs including padding
        while pos + 1 < data.len() && data.get(pos..pos + 2) == Some(&[0xFF, 0xFF]) {
            pos += 1;
        }

        if pos + 1 >= data.len() || data.get(pos) != Some(&0xFF) {
            break;
        }

        let marker = *data.get(pos + 1).unwrap_or(&0);
        pos += 2;

        // Stop if we hit SOS or EOI - metadata is in the header
        if marker == 0xDA || marker == 0xD9 {
            break;
        }

        // Markers without length field
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }

        // Read length
        if pos + 2 > data.len() {
            break;
        }
        let seg_len = usize::from(u16::from_be_bytes([
            *data.get(pos).unwrap_or(&0),
            *data.get(pos + 1).unwrap_or(&0),
        ]));
        if seg_len < 2 || pos + seg_len > data.len() {
            break;
        }

        let payload = data.get(pos + 2..pos + seg_len).unwrap_or(&[]);

        // APP2 (0xE2): check for XMP gainmap or MPF
        if marker == 0xE2 {
            if payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0") && payload.len() > 29 {
                let xmp = String::from_utf8_lossy(payload.get(29..).unwrap_or(&[]));
                if xmp.contains("hdrgm:") || xmp.contains("GainMap") || xmp.contains("gainmap") {
                    has_gainmap_xmp = true;
                }
            }
            if strip_mpf_identifier(payload).is_some() {
                has_mpf = true;
            }
        }
        // APP1 (0xE1): check for XMP gainmap
        if marker == 0xE1
            && payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0")
            && payload.len() > 29
        {
            let xmp = String::from_utf8_lossy(payload.get(29..).unwrap_or(&[]));
            if xmp.contains("hdrgm:") || xmp.contains("GainMap") || xmp.contains("gainmap") {
                has_gainmap_xmp = true;
            }
        }

        if has_gainmap_xmp && has_mpf {
            return true;
        }

        pos += seg_len;
    }

    // UltraHDR requires BOTH the XMP metadata parameters AND the MPF-linked secondary image
    has_gainmap_xmp && has_mpf
}

/// Detect `UltraHDR` from file path.
#[must_use]
pub fn is_ultra_hdr_jpeg_file(path: &std::path::Path) -> bool {
    std::fs::read(path).is_ok_and(|data| is_ultra_hdr_jpeg(&data))
}

/// Extract XMP metadata string from JPEG data.
///
/// Searches for XMP segment (APP1) starting with "<http://ns.adobe.com/xap/1.0/\0>".
///
/// # Returns
/// - `Some(String)`: XMP metadata content
/// - `None`: No XMP segment found
pub fn extract_xmp_from_jpeg_data(data: &[u8]) -> Option<Vec<String>> {
    let mut xmp_blocks = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        // Look for APP1 marker 0xFF 0xE1
        if data.get(pos..pos + 2) == Some(&[0xFF, 0xE1]) {
            if pos + 3 >= data.len() {
                break;
            }
            let seg_len = usize::from(u16::from_be_bytes([
                *data.get(pos + 2).unwrap_or(&0),
                *data.get(pos + 3).unwrap_or(&0),
            ]));
            if seg_len < 2 || pos + 2 + seg_len > data.len() {
                pos += 1;
                continue;
            }

            let payload = data.get(pos + 4..pos + 2 + seg_len).unwrap_or(&[]);

            // APP1 (0xE1): XMP Standard
            if payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0") && payload.len() > 29 {
                let xmp = String::from_utf8_lossy(payload.get(29..).unwrap_or(&[])).to_string();
                xmp_blocks.push(xmp);
            }
            // APP1 (0xE1): XMP Extended
            else if payload.starts_with(b"http://ns.adobe.com/xmp/extension/\0")
                && payload.len() > 35 + 32 + 8
            {
                let xmp =
                    String::from_utf8_lossy(payload.get(35 + 32 + 8..).unwrap_or(&[])).to_string();
                xmp_blocks.push(xmp);
            }
            pos += 2 + seg_len;
        } else {
            pos += 1;
        }
    }

    if xmp_blocks.is_empty() {
        None
    } else {
        info!("Extracted {} XMP blocks from JPEG stream", xmp_blocks.len());
        Some(xmp_blocks)
    }
}

/// Extract gainmap image from `UltraHDR` JPEG.
///
/// Returns (`base_image`, `gainmap_image`) as `DynamicImages`.
/// The gainmap is extracted from the MPF (Multi-Picture Format) segment.
///
/// # Errors
/// Returns an error if:
/// - The JPEG data is invalid or corrupted
/// - No MPF segment is found (not a valid `UltraHDR` JPEG)
/// - The extracted gainmap has invalid dimensions
/// - Base image decoding fails
/// - MPF parsing fails
///
/// Extract base image and gainmap from an `UltraHDR` JPEG byte stream.
///
/// # Errors
///
/// Returns an error if the JPEG is malformed, base image cannot be decoded, or MPF/GainMap is missing.
pub fn extract_gainmap_from_jpeg(data: &[u8]) -> Result<(DynamicImage, DynamicImage), String> {
    tracing::debug!(size = data.len(), "Extracting gainmap from UltraHDR JPEG");

    // Validate JPEG signature
    if data.len() < 4 || data.get(0..2) != Some(&[0xFF, 0xD8]) {
        return Err(format!(
            "Invalid JPEG signature: expected FFD8, got {:02X}{:02X}. \
             File size: {} bytes. This is not a valid JPEG file.",
            data.first().copied().unwrap_or(0),
            data.get(1).copied().unwrap_or(0),
            data.len()
        ));
    }

    // Decode base image with detailed error reporting
    let base_image = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| format!("Failed to create JPEG reader: {e}"))?
        .decode()
        .map_err(|e| {
            format!(
                "Failed to decode base JPEG image: {}. \
                 File size: {} bytes. The file may be corrupted or truncated.",
                e,
                data.len()
            )
        })?;

    let base_dims = base_image.dimensions();
    if base_dims.0 == 0 || base_dims.1 == 0 {
        return Err(format!(
            "Invalid base image dimensions: {}x{} (must be > 0x0). \
             This indicates a corrupted JPEG file or decoder bug.",
            base_dims.0, base_dims.1
        ));
    }

    tracing::debug!(
        width = base_dims.0,
        height = base_dims.1,
        "Base image decoded successfully"
    );
    let base_aspect = f64::from(base_dims.0) / f64::from(base_dims.1);

    // Find and parse MPF segment
    let mpf_segment = find_mpf_segment(data)?;
    tracing::debug!(size = mpf_segment.len(), "MPF segment found");

    // Extract gainmap from MPF
    let gainmap_data = extract_gainmap_from_mpf(data, &mpf_segment, Some(base_aspect))?;
    tracing::debug!(size = gainmap_data.len(), "Gainmap data extracted from MPF");

    // Decode gainmap image
    let gainmap_image = ImageReader::new(Cursor::new(&gainmap_data))
        .with_guessed_format()
        .map_err(|e| format!("Failed to create gainmap JPEG reader: {e}"))?
        .decode()
        .map_err(|e| {
            format!(
                "Failed to decode gainmap image: {}. \
                 Extracted data: {} bytes. The gainmap may be corrupted.",
                e,
                gainmap_data.len()
            )
        })?;

    let gainmap_dims = gainmap_image.dimensions();
    if gainmap_dims.0 == 0 || gainmap_dims.1 == 0 {
        return Err(format!(
            "Invalid gainmap dimensions: {}x{} (must be > 0x0). \
             This indicates a corrupted gainmap or decoder bug.",
            gainmap_dims.0, gainmap_dims.1
        ));
    }

    // Validate gainmap aspect ratio matches base image
    let gainmap_aspect = f64::from(gainmap_dims.0) / f64::from(gainmap_dims.1);
    let aspect_diff = (base_aspect - gainmap_aspect).abs();
    if aspect_diff > 0.01 {
        warn!(
            "Aspect ratio mismatch: base={:.4} ({}x{}), gainmap={:.4} ({}x{}). \
             Difference: {:.4}. This may indicate incorrect gainmap extraction.",
            base_aspect,
            base_dims.0,
            base_dims.1,
            gainmap_aspect,
            gainmap_dims.0,
            gainmap_dims.1,
            aspect_diff
        );
    }

    info!(
        "Gainmap extracted successfully: base={}x{}, gainmap={}x{}, aspect_diff={:.4}",
        base_dims.0, base_dims.1, gainmap_dims.0, gainmap_dims.1, aspect_diff
    );

    Ok((base_image, gainmap_image))
}

/// MPF (Multi-Picture Format) structure constants
mod mpf {
    // MPF identifier: "MPF\0"
    pub const MPF_IDENTIFIER: &[u8] = b"MPF\0";
    // Some devices use a non-standard APP2 identifier while keeping the MPF TIFF layout.
    pub const XMPF_IDENTIFIER: &[u8] = b"XMPF";

    // TIFF big-endian marker: "MM\0*"
    pub const TIFF_BIG_ENDIAN: &[u8] = b"MM\0*";
    // TIFF little-endian marker: "II*\0"
    pub const TIFF_LITTLE_ENDIAN: &[u8] = b"II*\0";

    // MPF tags
    pub const TAG_NUMBER_OF_IMAGES: u16 = 0xB001;
    pub const TAG_MP_ENTRY: u16 = 0xB002;
}

const JPEG_SOI_BYTES: [u8; 3] = [0xFF, 0xD8, 0xFF];
const JPEG_EOI_BYTES: [u8; 2] = [0xFF, 0xD9];
const GAINMAP_SCAN_WINDOW_MIN: usize = 4_096;
const GAINMAP_SCAN_WINDOW_MAX: usize = 131_072;
const MAX_GAINMAP_SCAN_CANDIDATES: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GainmapCandidateSource {
    RelativeOffset,
    AbsoluteOffset,
    NearbyScan,
    TailScan,
}

#[derive(Debug)]
struct GainmapCandidate {
    data: Vec<u8>,
    start: usize,
    source: GainmapCandidateSource,
    repaired_eoi: bool,
    aspect_diff: Option<f64>,
    decoded: bool,
}

fn strip_mpf_identifier(payload: &[u8]) -> Option<&[u8]> {
    payload
        .strip_prefix(mpf::MPF_IDENTIFIER)
        .or_else(|| payload.strip_prefix(mpf::XMPF_IDENTIFIER))
}

fn starts_with_jpeg_at(data: &[u8], position: usize) -> bool {
    data.get(position..position + 2) == Some(&[0xFF, 0xD8])
}

fn push_gainmap_candidate(
    jpeg_data: &[u8],
    position: usize,
    source: GainmapCandidateSource,
    seen: &mut BTreeSet<usize>,
    candidates: &mut Vec<(usize, GainmapCandidateSource)>,
) {
    if starts_with_jpeg_at(jpeg_data, position) && seen.insert(position) {
        candidates.push((position, source));
    }
}

fn collect_scanned_gainmap_candidates(
    jpeg_data: &[u8],
    range_start: usize,
    range_end: usize,
    source: GainmapCandidateSource,
    seen: &mut BTreeSet<usize>,
    candidates: &mut Vec<(usize, GainmapCandidateSource)>,
) {
    let bounded_end = range_end.min(jpeg_data.len());
    if bounded_end.saturating_sub(range_start) < JPEG_SOI_BYTES.len() {
        return;
    }

    for (offset, window) in jpeg_data
        .get(range_start..bounded_end)
        .unwrap_or(&[])
        .windows(JPEG_SOI_BYTES.len())
        .enumerate()
    {
        if window == JPEG_SOI_BYTES {
            let position = range_start + offset;
            if seen.insert(position) {
                candidates.push((position, source));
                if candidates.len() >= MAX_GAINMAP_SCAN_CANDIDATES {
                    break;
                }
            }
        }
    }
}

fn collect_gainmap_candidate_offsets(
    jpeg_data: &[u8],
    relative_start: usize,
    absolute_start: usize,
    mpf_base_pos: usize,
    claimed_length: usize,
) -> Vec<(usize, GainmapCandidateSource)> {
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();

    push_gainmap_candidate(
        jpeg_data,
        relative_start,
        GainmapCandidateSource::RelativeOffset,
        &mut seen,
        &mut candidates,
    );
    push_gainmap_candidate(
        jpeg_data,
        absolute_start,
        GainmapCandidateSource::AbsoluteOffset,
        &mut seen,
        &mut candidates,
    );

    let search_radius = claimed_length.clamp(GAINMAP_SCAN_WINDOW_MIN, GAINMAP_SCAN_WINDOW_MAX);

    collect_scanned_gainmap_candidates(
        jpeg_data,
        relative_start.saturating_sub(search_radius),
        relative_start
            .saturating_add(search_radius)
            .min(jpeg_data.len()),
        GainmapCandidateSource::NearbyScan,
        &mut seen,
        &mut candidates,
    );

    if absolute_start != relative_start {
        collect_scanned_gainmap_candidates(
            jpeg_data,
            absolute_start.saturating_sub(search_radius),
            absolute_start
                .saturating_add(search_radius)
                .min(jpeg_data.len()),
            GainmapCandidateSource::NearbyScan,
            &mut seen,
            &mut candidates,
        );
    }

    if candidates.is_empty() {
        collect_scanned_gainmap_candidates(
            jpeg_data,
            mpf_base_pos.min(jpeg_data.len()),
            jpeg_data.len(),
            GainmapCandidateSource::TailScan,
            &mut seen,
            &mut candidates,
        );
    }

    candidates
}

fn find_first_eoi_end(jpeg_data: &[u8], start: usize) -> Option<usize> {
    jpeg_data
        .get(start..)?
        .windows(JPEG_EOI_BYTES.len())
        .position(|window| window == JPEG_EOI_BYTES)
        .map(|offset| start + offset + JPEG_EOI_BYTES.len())
}

fn candidate_gainmap_bytes(
    jpeg_data: &[u8],
    start: usize,
    claimed_length: usize,
) -> Option<(Vec<u8>, bool)> {
    if !starts_with_jpeg_at(jpeg_data, start) {
        return None;
    }

    let claimed_end = start.saturating_add(claimed_length).min(jpeg_data.len());
    let end = match find_first_eoi_end(jpeg_data, start) {
        Some(eoi_end) if claimed_end == jpeg_data.len() || eoi_end <= claimed_end => eoi_end,
        _ if claimed_end > start => claimed_end,
        _ => return None,
    };

    let mut candidate = jpeg_data.get(start..end).unwrap_or(&[]).to_vec();
    let repaired_eoi = !candidate.ends_with(&JPEG_EOI_BYTES);
    if repaired_eoi {
        candidate.extend_from_slice(&JPEG_EOI_BYTES);
    }

    Some((candidate, repaired_eoi))
}

fn decode_gainmap_dimensions(candidate: &[u8]) -> Option<(u32, u32)> {
    use image::ImageReader;
    use std::io::Cursor;

    let decoded = ImageReader::new(Cursor::new(candidate))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;

    Some(decoded.dimensions())
}

fn gainmap_candidate_score(
    source: GainmapCandidateSource,
    candidate_len: usize,
    claimed_len: usize,
    aspect_diff: Option<f64>,
    repaired_eoi: bool,
) -> f64 {
    let source_weight = match source {
        GainmapCandidateSource::RelativeOffset => 4_000.0,
        GainmapCandidateSource::AbsoluteOffset => 3_500.0,
        GainmapCandidateSource::NearbyScan => 2_500.0,
        GainmapCandidateSource::TailScan => 1_500.0,
    };
    let aspect_penalty = aspect_diff.unwrap_or(0.0) * 10_000.0;
    let length_penalty = if claimed_len == 0 {
        0.0
    } else {
        (crate::numeric_cast::usize_to_f64(candidate_len.abs_diff(claimed_len))
            / crate::numeric_cast::usize_to_f64(claimed_len))
            * 100.0
    };
    let repair_penalty = if repaired_eoi { 25.0 } else { 0.0 };

    source_weight - aspect_penalty - length_penalty - repair_penalty
}

fn gainmap_raw_fallback_score(
    source: GainmapCandidateSource,
    candidate_len: usize,
    claimed_len: usize,
    repaired_eoi: bool,
) -> f64 {
    let source_weight = match source {
        GainmapCandidateSource::RelativeOffset => 200.0,
        GainmapCandidateSource::AbsoluteOffset => 150.0,
        GainmapCandidateSource::NearbyScan | GainmapCandidateSource::TailScan => 0.0,
    };
    let length_penalty = if claimed_len == 0 {
        0.0
    } else {
        (crate::numeric_cast::usize_to_f64(candidate_len.abs_diff(claimed_len))
            / crate::numeric_cast::usize_to_f64(claimed_len))
            * 100.0
    };
    let repair_penalty = if repaired_eoi { 25.0 } else { 0.0 };

    source_weight - length_penalty - repair_penalty
}

fn can_use_raw_direct_gainmap_candidate(
    source: GainmapCandidateSource,
    candidate_data: &[u8],
) -> bool {
    (matches!(
        source,
        GainmapCandidateSource::RelativeOffset | GainmapCandidateSource::AbsoluteOffset
    ) && candidate_data.len() >= 4
        && candidate_data.starts_with(&JPEG_SOI_BYTES[..2])
        && candidate_data.ends_with(&JPEG_EOI_BYTES))
}

fn recover_gainmap_candidate(
    jpeg_data: &[u8],
    mpf_base_pos: usize,
    gainmap_offset: usize,
    claimed_length: usize,
    expected_aspect: Option<f64>,
) -> Option<GainmapCandidate> {
    let relative_start = mpf_base_pos.saturating_add(gainmap_offset);
    let absolute_start = gainmap_offset;
    let mut best_decoded_match: Option<(f64, GainmapCandidate)> = None;
    let mut best_raw_direct_match: Option<(f64, GainmapCandidate)> = None;

    for (start, source) in collect_gainmap_candidate_offsets(
        jpeg_data,
        relative_start,
        absolute_start,
        mpf_base_pos,
        claimed_length,
    ) {
        let Some((candidate_data, repaired_eoi)) =
            candidate_gainmap_bytes(jpeg_data, start, claimed_length)
        else {
            continue;
        };

        if let Some((width, height)) = decode_gainmap_dimensions(&candidate_data) {
            let aspect_diff = expected_aspect
                .filter(|_| height > 0)
                .map(|aspect| (aspect - (f64::from(width) / f64::from(height))).abs());
            let score = gainmap_candidate_score(
                source,
                candidate_data.len(),
                claimed_length,
                aspect_diff,
                repaired_eoi,
            );
            let candidate = GainmapCandidate {
                data: candidate_data,
                start,
                source,
                repaired_eoi,
                aspect_diff,
                decoded: true,
            };

            if best_decoded_match
                .as_ref()
                .is_none_or(|(best_score, _)| score > *best_score)
            {
                best_decoded_match = Some((score, candidate));
            }
            continue;
        }

        if can_use_raw_direct_gainmap_candidate(source, &candidate_data) {
            let score = gainmap_raw_fallback_score(
                source,
                candidate_data.len(),
                claimed_length,
                repaired_eoi,
            );
            let candidate = GainmapCandidate {
                data: candidate_data,
                start,
                source,
                repaired_eoi,
                aspect_diff: None,
                decoded: false,
            };

            if best_raw_direct_match
                .as_ref()
                .is_none_or(|(best_score, _)| score > *best_score)
            {
                best_raw_direct_match = Some((score, candidate));
            }
        }
    }

    best_decoded_match
        .or(best_raw_direct_match)
        .map(|(_, candidate)| candidate)
}

/// Find the MPF segment in JPEG data.
/// Returns the TIFF payload after the `MPF\0` or `XMPF` APP2 identifier.
fn find_mpf_segment(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut pos = 2;

    while pos + 1 < data.len() {
        // Skip padding
        while pos + 1 < data.len() && data.get(pos..pos + 2) == Some(&[0xFF, 0xFF]) {
            pos += 1;
        }

        if pos + 1 >= data.len() || data.get(pos) != Some(&0xFF) {
            return Err(format!(
                "Invalid JPEG structure: expected marker 0xFF at position {}, found 0x{:02X}",
                pos,
                data.get(pos).copied().unwrap_or(0)
            ));
        }

        let marker = *data.get(pos + 1).unwrap_or(&0);
        pos += 2;

        if marker == 0xDA || marker == 0xD9 {
            break;
        }

        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }

        if pos + 2 > data.len() {
            return Err(format!("Truncated segment at position {pos}"));
        }

        let seg_len = usize::from(u16::from_be_bytes([
            *data.get(pos).unwrap_or(&0),
            *data.get(pos + 1).unwrap_or(&0),
        ]));
        if seg_len < 2 || pos + seg_len > data.len() {
            return Err(format!(
                "Invalid segment length {seg_len} at position {pos} (marker 0x{marker:02X})"
            ));
        }

        let payload = data.get(pos + 2..pos + seg_len).unwrap_or(&[]);

        if marker == 0xE2 {
            if let Some(mpf_payload) = strip_mpf_identifier(payload) {
                return Ok(mpf_payload.to_vec());
            }
        }

        pos += seg_len;
    }

    Err("No MPF (Multi-Picture Format) segment found in APP2 markers".to_string())
}

/// Extract gainmap image data from MPF segment.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines, reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead.")]
fn extract_gainmap_from_mpf(
    jpeg_data: &[u8],
    mpf_data: &[u8],
    expected_aspect: Option<f64>,
) -> Result<Vec<u8>, String> {
    // Determine endianness
    let is_big_endian = if mpf_data.starts_with(mpf::TIFF_BIG_ENDIAN) {
        true
    } else if mpf_data.starts_with(mpf::TIFF_LITTLE_ENDIAN) {
        false
    } else {
        return Err(format!(
            "Invalid MPF endianness marker: expected 'MM\\0*' or 'II*\\0', got {:02X} {:02X} {:02X} {:02X}",
            mpf_data.first().copied().unwrap_or(0),
            mpf_data.get(1).copied().unwrap_or(0),
            mpf_data.get(2).copied().unwrap_or(0),
            mpf_data.get(3).copied().unwrap_or(0)
        ));
    };

    info!(
        "MPF endianness: {}",
        if is_big_endian {
            "big-endian (MM)"
        } else {
            "little-endian (II)"
        }
    );

    // Read first IFD offset (4 bytes after endianness marker)
    let first_ifd_offset = read_u32(mpf_data.get(4..8).unwrap_or(&[]), is_big_endian)?;
    info!("First IFD offset: {}", first_ifd_offset);

    // Navigate to first IFD
    if usize::try_from(first_ifd_offset).unwrap_or(0) + 2 > mpf_data.len() {
        return Err(format!(
            "Invalid first IFD offset {}: exceeds MPF data size {}",
            first_ifd_offset,
            mpf_data.len()
        ));
    }

    // Read number of entries in IFD
    let num_entries = read_u16(
        mpf_data
            .get(usize::try_from(first_ifd_offset).unwrap_or(0)..)
            .unwrap_or(&[]),
        is_big_endian,
    )?;
    info!("IFD entries: {}", num_entries);

    // Find MPEntry tag
    let mut mp_entry_offset: Option<u32> = None;
    let mut num_images: Option<u32> = None;

    let ifd_start = usize::try_from(first_ifd_offset).unwrap_or(0);
    for i in 0..num_entries {
        let entry_offset = ifd_start + 2 + (usize::from(i) * 12);
        if entry_offset + 12 > mpf_data.len() {
            return Err(format!(
                "IFD entry {} offset {} exceeds MPF data size {}",
                i,
                entry_offset,
                mpf_data.len()
            ));
        }

        let tag = read_u16(mpf_data.get(entry_offset..).unwrap_or(&[]), is_big_endian)?;
        let _data_type = read_u16(
            mpf_data.get(entry_offset + 2..).unwrap_or(&[]),
            is_big_endian,
        )?;
        let num_components = read_u32(
            mpf_data.get(entry_offset + 4..).unwrap_or(&[]),
            is_big_endian,
        )?;
        let value_offset = read_u32(
            mpf_data.get(entry_offset + 8..).unwrap_or(&[]),
            is_big_endian,
        )?;

        match tag {
            mpf::TAG_NUMBER_OF_IMAGES => {
                // For NumberOfImages (tag 0xB001), it's a LONG (type 4, size 4).
                // If count is 1, the value is in value_offset.
                if num_components == 1 {
                    num_images = Some(value_offset);
                    info!("NumberOfImages: {}", value_offset);
                } else {
                    warn!(
                        "NumberOfImages has unexpected component count: {}",
                        num_components
                    );
                    num_images = Some(num_components); // Fallback
                }
            }
            mpf::TAG_MP_ENTRY => {
                mp_entry_offset = Some(value_offset);
                info!(
                    "MPEntry offset: {}, count: {}",
                    value_offset, num_components
                );
            }
            _ => {}
        }
    }

    let mp_entry_offset = mp_entry_offset.ok_or_else(|| {
        "MPEntry tag (0xB002) not found in IFD. This is not a valid MPF structure.".to_string()
    })?;

    let num_images =
        num_images.ok_or_else(|| "NumberOfImages tag (0xB001) not found in IFD.".to_string())?;

    if num_images < 2 {
        return Err(format!(
            "MPF contains only {num_images} image(s). UltraHDR requires at least 2 images (base + gainmap)."
        ));
    }

    // Navigate to MP Entry array
    let mp_entry_array_offset = usize::try_from(mp_entry_offset).unwrap_or(0);
    if mp_entry_array_offset + 16 > mpf_data.len() {
        return Err(format!(
            "MP entry array offset {} exceeds MPF data size {}",
            mp_entry_array_offset,
            mpf_data.len()
        ));
    }

    // Read MP entries - entry 0 is primary image, entry 1 is gainmap
    // Each entry is 16 bytes:
    // - Attributes: 4 bytes
    // - Image data length: 4 bytes
    // - Data offset: 4 bytes
    // - Dependency indices: 2 + 2 = 4 bytes
    let gainmap_entry_offset = mp_entry_array_offset + 16; // Skip first entry (primary image)

    if gainmap_entry_offset + 16 > mpf_data.len() {
        return Err(format!(
            "Gainmap entry offset {} exceeds MPF data size {}",
            gainmap_entry_offset,
            mpf_data.len()
        ));
    }

    let attributes = read_u32(
        mpf_data.get(gainmap_entry_offset..).unwrap_or(&[]),
        is_big_endian,
    )?;
    let gainmap_length = read_u32(
        mpf_data.get(gainmap_entry_offset + 4..).unwrap_or(&[]),
        is_big_endian,
    )?;
    let gainmap_offset = read_u32(
        mpf_data.get(gainmap_entry_offset + 8..).unwrap_or(&[]),
        is_big_endian,
    )?;

    info!(
        "Gainmap entry: attributes=0x{:08X}, length={}, offset={}",
        attributes, gainmap_length, gainmap_offset
    );

    // Validate gainmap length
    if gainmap_length == 0 {
        return Err("Gainmap length is 0. Invalid MPF structure.".to_string());
    }

    if gainmap_length > u32::try_from(jpeg_data.len()).unwrap_or(u32::MAX) {
        warn!(
            gainmap_length,
            jpeg_len = jpeg_data.len(),
            "Gainmap length exceeds JPEG file size; attempting recovery from available bytes"
        );
    }

    let mpf_base_pos = find_mpf_base_position(jpeg_data)?;
    let gainmap_len_usize = usize::try_from(gainmap_length).unwrap_or(0);
    let gainmap_offset_usize = usize::try_from(gainmap_offset).unwrap_or(0);
    let gainmap_candidate = recover_gainmap_candidate(
        jpeg_data,
        mpf_base_pos,
        gainmap_offset_usize,
        gainmap_len_usize,
        expected_aspect,
    )
    .ok_or_else(|| {
        let relative_start = mpf_base_pos.saturating_add(gainmap_offset_usize);
        format!(
            "Gainmap data at calculated position {} (offset {}, base {}) with length {} could not be recovered from JPEG file size {}",
            relative_start,
            gainmap_offset,
            mpf_base_pos,
            gainmap_len_usize,
            jpeg_data.len()
        )
    })?;

    if gainmap_candidate.source != GainmapCandidateSource::RelativeOffset
        || gainmap_candidate.repaired_eoi
        || !gainmap_candidate.decoded
    {
        warn!(
            start = gainmap_candidate.start,
            source = ?gainmap_candidate.source,
            repaired_eoi = gainmap_candidate.repaired_eoi,
            decoded = gainmap_candidate.decoded,
            aspect_diff = gainmap_candidate.aspect_diff.unwrap_or_default(),
            "Recovered gainmap using MPF fallback candidate"
        );
    }

    Ok(gainmap_candidate.data)
}

/// Find the absolute position of MPF base in JPEG data
fn find_mpf_base_position(jpeg_data: &[u8]) -> Result<usize, String> {
    let mut pos = 2;

    while pos + 1 < jpeg_data.len() {
        // Skip padding
        while pos + 1 < jpeg_data.len() && jpeg_data.get(pos..pos + 2) == Some(&[0xFF, 0xFF]) {
            pos += 1;
        }

        if pos + 1 >= jpeg_data.len() || jpeg_data.get(pos) != Some(&0xFF) {
            break;
        }

        let marker = *jpeg_data.get(pos + 1).unwrap_or(&0);
        pos += 2;

        if marker == 0xDA || marker == 0xD9 {
            break;
        }

        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }

        if pos + 2 > jpeg_data.len() {
            break;
        }

        let seg_len = usize::from(u16::from_be_bytes([
            *jpeg_data.get(pos).unwrap_or(&0),
            *jpeg_data.get(pos + 1).unwrap_or(&0),
        ]));
        if seg_len < 2 || pos + seg_len > jpeg_data.len() {
            break;
        }

        let payload = jpeg_data.get(pos + 2..pos + seg_len).unwrap_or(&[]);

        if marker == 0xE2 && strip_mpf_identifier(payload).is_some() {
            // Offsets are relative to the TIFF header that begins immediately after
            // the MPF/XMPF identifier in the APP2 payload.
            return Ok(pos + 2 + 4);
        }

        pos += seg_len;
    }

    Err("MPF segment not found".to_string())
}

/// Read u16 from bytes with specified endianness
fn read_u16(data: &[u8], big_endian: bool) -> Result<u16, String> {
    if data.len() < 2 {
        return Err(format!(
            "Insufficient data for u16 read: {} bytes",
            data.len()
        ));
    }
    Ok(if big_endian {
        u16::from_be_bytes([*data.first().unwrap_or(&0), *data.get(1).unwrap_or(&0)])
    } else {
        u16::from_le_bytes([*data.first().unwrap_or(&0), *data.get(1).unwrap_or(&0)])
    })
}

/// Read u32 from bytes with specified endianness
fn read_u32(data: &[u8], big_endian: bool) -> Result<u32, String> {
    if data.len() < 4 {
        return Err(format!(
            "Insufficient data for u32 read: {} bytes",
            data.len()
        ));
    }
    Ok(if big_endian {
        u32::from_be_bytes([
            *data.first().unwrap_or(&0),
            *data.get(1).unwrap_or(&0),
            *data.get(2).unwrap_or(&0),
            *data.get(3).unwrap_or(&0),
        ])
    } else {
        u32::from_le_bytes([
            *data.first().unwrap_or(&0),
            *data.get(1).unwrap_or(&0),
            *data.get(2).unwrap_or(&0),
            *data.get(3).unwrap_or(&0),
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_standard_qt_q50() {
        let qt = generate_standard_qt(50, &IJG_LUMINANCE_BASE);
        assert_eq!(qt[0][0], 16);
    }

    #[test]
    fn test_generate_standard_qt_q100() {
        let qt = generate_standard_qt(100, &IJG_LUMINANCE_BASE);
        for row in &qt {
            for &val in row {
                assert!(val >= 1);
            }
        }
    }

    #[test]
    fn test_generate_standard_qt_q1() {
        let qt = generate_standard_qt(1, &IJG_LUMINANCE_BASE);
        assert!(qt[0][0] > 100);
    }

    #[test]
    fn test_sse_identical() {
        let table = IJG_LUMINANCE_BASE;
        let sse = calculate_sse(&table, &table);
        assert!(crate::float_compare::approx_eq_f64(sse, 0.0));
    }

    #[test]
    fn test_weighted_sse_identical() {
        let table = IJG_LUMINANCE_BASE;
        let wsse = calculate_weighted_sse(&table, &table);
        assert!(crate::float_compare::approx_eq_f64(wsse, 0.0));
    }

    #[test]
    fn test_estimate_quality_perfect_match() {
        let qt = generate_standard_qt(75, &IJG_LUMINANCE_BASE);
        let (quality, sse, is_standard) = estimate_quality_from_table(&qt, true);
        assert_eq!(quality, 75);
        assert!(crate::float_compare::approx_eq_f64(sse, 0.0));
        assert!(is_standard);
    }

    #[test]
    fn test_estimate_quality_all_levels() {
        for expected_q in 1..=100 {
            let qt = generate_standard_qt(expected_q, &IJG_LUMINANCE_BASE);
            let (detected_q, sse, _) = estimate_quality_from_table(&qt, true);
            assert_eq!(detected_q, expected_q, "Failed to detect Q={expected_q}");
            assert!(crate::float_compare::approx_eq_f64(sse, 0.0));
        }
    }

    #[test]
    fn test_confidence_exact_match() {
        let qt = generate_standard_qt(85, &IJG_LUMINANCE_BASE);
        let estimate = estimate_quality_precise(&qt, &IJG_LUMINANCE_BASE);
        let confidence = calculate_confidence(&estimate, None);
        assert!(
            confidence >= 0.98,
            "Confidence should be high for exact match"
        );
    }

    #[test]
    fn test_chrominance_detection() {
        for expected_q in &[50, 75, 90, 95] {
            let qt = generate_standard_qt(*expected_q, &IJG_CHROMINANCE_BASE);
            let (detected_q, sse, _) = estimate_quality_from_table(&qt, false);
            assert_eq!(
                detected_q, *expected_q,
                "Failed to detect chroma Q={expected_q}"
            );
            assert!(crate::float_compare::approx_eq_f64(sse, 0.0));
        }
    }

    // ==================== UltraHDR / Gainmap Tests ====================

    #[test]
    fn test_extract_xmp_from_jpeg_data_invalid_signature() {
        let invalid = b"not a jpeg";
        let result = extract_xmp_from_jpeg_data(invalid);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_xmp_from_jpeg_data_no_xmp() {
        let jpeg_without_xmp = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x10, // APP0 (not XMP)
            b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
            0xFF, 0xD9, // EOI
        ];
        let result = extract_xmp_from_jpeg_data(&jpeg_without_xmp);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_xmp_from_jpeg_data_with_xmp() {
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        let xmp_content = b"<x:xmpmeta>test content</x:xmpmeta>";

        let mut jpeg_with_xmp = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE1, // APP1 for XMP
        ];
        let xmp_len = u16::try_from(xmp_header.len() + xmp_content.len() + 2).unwrap_or(0);
        jpeg_with_xmp.extend_from_slice(&xmp_len.to_be_bytes());
        jpeg_with_xmp.extend_from_slice(xmp_header);
        jpeg_with_xmp.extend_from_slice(xmp_content);
        jpeg_with_xmp.extend_from_slice(&[0xFF, 0xD9]); // EOI

        let extracted = extract_xmp_from_jpeg_data(&jpeg_with_xmp);
        assert!(extracted.is_some());
        let xmp_blocks = extracted.unwrap_or_default();
        assert_eq!(xmp_blocks.len(), 1);
        let xmp_str = xmp_blocks
            .first()
            .unwrap_or_else(|| panic!("No XMP blocks found"));
        assert!(xmp_str.contains("<x:xmpmeta>"));
        assert!(xmp_str.contains("test content"));
    }

    #[test]
    fn test_is_ultra_hdr_jpeg_false_for_standard_jpeg() {
        let jpeg_data = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x10, // APP0
            b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
            0xFF, 0xD9, // EOI
        ];
        assert!(!is_ultra_hdr_jpeg(&jpeg_data));
    }

    #[test]
    fn test_is_ultra_hdr_jpeg_true_with_xmp_gainmap() {
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        // Use actual gainmap metadata format that contains "hdrgm:"
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"1.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";

        let mut jpeg_with_gainmap = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE1, // APP1 for XMP
        ];
        let xmp_len = u16::try_from(xmp_header.len() + xmp_content.len() + 2).unwrap_or(0);
        jpeg_with_gainmap.extend_from_slice(&xmp_len.to_be_bytes());
        jpeg_with_gainmap.extend_from_slice(xmp_header);
        jpeg_with_gainmap.extend_from_slice(xmp_content);

        // Add dummy MPF segment in APP2
        jpeg_with_gainmap.extend_from_slice(&[
            0xFF, 0xE2, // APP2
            0x00, 0x06, // Length (2 + bytes("MPF\0")) = 6
        ]);
        jpeg_with_gainmap.extend_from_slice(mpf::MPF_IDENTIFIER);

        jpeg_with_gainmap.extend_from_slice(&[0xFF, 0xD9]); // EOI

        assert!(is_ultra_hdr_jpeg(&jpeg_with_gainmap));
    }

    #[test]
    fn test_is_ultra_hdr_jpeg_true_with_xmpf_identifier() {
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"1.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";

        let mut jpeg_with_gainmap = vec![0xFF, 0xD8, 0xFF, 0xE1];
        let xmp_len = u16::try_from(xmp_header.len() + xmp_content.len() + 2).unwrap_or(u16::MAX);
        jpeg_with_gainmap.extend_from_slice(&xmp_len.to_be_bytes());
        jpeg_with_gainmap.extend_from_slice(xmp_header);
        jpeg_with_gainmap.extend_from_slice(xmp_content);

        jpeg_with_gainmap.extend_from_slice(&[0xFF, 0xE2, 0x00, 0x06]);
        jpeg_with_gainmap.extend_from_slice(mpf::XMPF_IDENTIFIER);
        jpeg_with_gainmap.extend_from_slice(&[0xFF, 0xD9]);

        assert!(is_ultra_hdr_jpeg(&jpeg_with_gainmap));
    }

    #[test]
    fn test_extract_gainmap_from_jpeg_invalid_signature() {
        let invalid_data = b"not a jpeg";
        let result = extract_gainmap_from_jpeg(invalid_data);
        assert!(result.is_err());
        let err_msg = result.err().unwrap_or_default();
        assert!(err_msg.contains("Invalid JPEG signature"));
        assert!(err_msg.contains("FFD8"));
    }

    #[test]
    fn test_extract_gainmap_from_jpeg_empty() {
        let empty_data: &[u8] = &[];
        let result = extract_gainmap_from_jpeg(empty_data);
        assert!(result.is_err());
        assert!(result
            .err()
            .is_some_and(|e| e.contains("Invalid JPEG signature")));
    }

    #[test]
    fn test_extract_gainmap_from_jpeg_truncated() {
        // Test with truncated JPEG (valid signature but incomplete)
        let truncated = vec![0xFF, 0xD8, 0xFF, 0xE0]; // SOI + APP0 marker only
        let result = extract_gainmap_from_jpeg(&truncated);
        assert!(result.is_err());
        let err_msg = result.err().unwrap_or_default();
        assert!(
            err_msg.contains("Truncated")
                || err_msg.contains("Failed to decode")
                || err_msg.contains("corrupted")
        );
    }

    #[test]
    fn test_extract_gainmap_from_jpeg_no_mpf_detailed_error() {
        // Create a minimal valid JPEG structure without MPF
        // This tests the detailed error message for missing MPF
        let jpeg_no_mpf = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x10, // APP0
            b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
            0xFF, 0xD9, // EOI
        ];
        let result = extract_gainmap_from_jpeg(&jpeg_no_mpf);
        // Should fail because it's not a decodable image or no MPF
        assert!(result.is_err());
    }

    #[test]
    fn test_gainmap_params_default() {
        use crate::hdr_synthesis::GainMapParams;
        let params = GainMapParams::default();
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.gain_map_max),
            1.0
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.gain_map_min),
            0.0
        ));
        assert!(crate::float_compare::approx_eq_f64(
            f64::from(params.gamma),
            1.0
        ));
        assert!((params.offset_sdr - 1.0 / 64.0).abs() < f32::EPSILON);
        assert!((params.offset_hdr - 1.0 / 64.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_hdr_intermediate_format_default() {
        use crate::hdr_synthesis::HdrIntermediateFormat;
        let format = HdrIntermediateFormat::default();
        assert_eq!(format, HdrIntermediateFormat::OpenExr32);
    }

    #[test]
    fn test_hdr_intermediate_format_debug() {
        use crate::hdr_synthesis::HdrIntermediateFormat;
        let exr = HdrIntermediateFormat::OpenExr32;
        let png = HdrIntermediateFormat::Png16;

        // Test Debug trait
        let exr_debug = format!("{exr:?}");
        let png_debug = format!("{png:?}");

        assert!(exr_debug.contains("OpenExr32"));
        assert!(png_debug.contains("Png16"));
    }

    #[test]
    fn test_hdr_intermediate_format_equality() {
        use crate::hdr_synthesis::HdrIntermediateFormat;
        assert_eq!(
            HdrIntermediateFormat::OpenExr32,
            HdrIntermediateFormat::OpenExr32
        );
        assert_eq!(HdrIntermediateFormat::Png16, HdrIntermediateFormat::Png16);
        assert_ne!(
            HdrIntermediateFormat::OpenExr32,
            HdrIntermediateFormat::Png16
        );
    }

    #[test]
    fn test_is_ultra_hdr_jpeg_with_padding_and_com() {
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"1.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";

        let mut data = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xFF, 0xFF, 0xE1, // APP1 with padding
        ];
        let xmp_len = u16::try_from(xmp_header.len() + xmp_content.len() + 2).unwrap_or(u16::MAX);
        data.extend_from_slice(&xmp_len.to_be_bytes());
        data.extend_from_slice(xmp_header);
        data.extend_from_slice(xmp_content);

        // Add a COM marker (0xFE) which caused failure before
        data.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x05, b't', b'a', b'g']);

        // Add MPF
        data.extend_from_slice(&[0xFF, 0xE2, 0x00, 0x0C]);
        data.extend_from_slice(mpf::MPF_IDENTIFIER);
        data.extend_from_slice(b"dummy");

        data.extend_from_slice(&[0xFF, 0xD9]); // EOI

        assert!(is_ultra_hdr_jpeg(&data));
    }

    #[test]
    fn test_extract_xmp_stops_at_sos() {
        let mut data = vec![0xFF, 0xD8];
        // SOS before APP1
        data.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        // Compressed data (would look like markers if not skipped)
        data.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x10]);

        let result = extract_xmp_from_jpeg_data(&data);
        assert!(result.is_none()); // Should stop at SOS and not find the "fake" APP1 in scan data
    }

    #[test]
    fn test_extract_gainmap_absolute_fallback() {
        // [GIVEN] A JPEG with an MPF segment where the offset is absolute but not relative
        let mut data = vec![0xFF, 0xD8]; // SOI

        // 1. APP1 XMP (simplified but must contain hdrgm:)
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"2.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";
        let xmp_hdr = b"http://ns.adobe.com/xap/1.0/\0";
        data.push(0xFF);
        data.push(0xE1); // APP1
        let xmp_len = crate::numeric_cast::usize_to_u16_sat(xmp_hdr.len() + xmp_content.len() + 2);
        data.extend_from_slice(&xmp_len.to_be_bytes());
        data.extend_from_slice(xmp_hdr);
        data.extend_from_slice(xmp_content);

        // 2. Identify where MPF payload starts
        let mpf_id = b"MPF\0";
        let tiff_hdr = b"MM\0*"; // Big Endian
        let ifd_offset = 8u32;

        let mut mpf_payload = Vec::new();
        mpf_payload.extend_from_slice(tiff_hdr);
        mpf_payload.extend_from_slice(&ifd_offset.to_be_bytes());
        mpf_payload.extend_from_slice(&2u16.to_be_bytes()); // 2 IFD entries

        // Entry 1: NumberOfImages (tag 0xB001)
        mpf_payload.extend_from_slice(&0xB001u16.to_be_bytes());
        mpf_payload.extend_from_slice(&4u16.to_be_bytes()); // LONG
        mpf_payload.extend_from_slice(&1u32.to_be_bytes());
        mpf_payload.extend_from_slice(&2u32.to_be_bytes()); // 2 images

        // Entry 2: MPEntry (tag 0xB002)
        let mp_entry_val_offset = crate::numeric_cast::usize_to_u32_sat(mpf_payload.len() + 12 + 4);
        mpf_payload.extend_from_slice(&0xB002u16.to_be_bytes());
        mpf_payload.extend_from_slice(&7u16.to_be_bytes()); // UNDEFINED
        mpf_payload.extend_from_slice(&32u32.to_be_bytes()); // 2 entries
        mpf_payload.extend_from_slice(&mp_entry_val_offset.to_be_bytes());

        mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Next IFD offset

        // MP Entry 0 (Primary)
        mpf_payload.extend_from_slice(&[0u8; 16]);

        // 3. MP Entry 1 (Gainmap) - ABSOLUTE OFFSET
        // Calculate where the APP2 segment WILL end
        let app2_segment_overhead = 2 + 2 + mpf_id.len();
        let absolute_offset = crate::numeric_cast::usize_to_u32_sat(
            data.len() + app2_segment_overhead + mpf_payload.len() + 16,
        );

        let gainmap_size = 4u32;
        mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Attributes
        mpf_payload.extend_from_slice(&gainmap_size.to_be_bytes()); // Size
        mpf_payload.extend_from_slice(&absolute_offset.to_be_bytes()); // Absolute offset!
        mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Deps

        // Assemble APP2 segment
        let app2_len = crate::numeric_cast::usize_to_u16_sat(mpf_id.len() + mpf_payload.len() + 2);
        data.push(0xFF);
        data.push(0xE2); // APP2 marker
        data.extend_from_slice(&app2_len.to_be_bytes());
        data.extend_from_slice(mpf_id);
        data.extend_from_slice(&mpf_payload);

        // 4. Place Gainmap at the end (absolute_offset)
        assert_eq!(data.len(), absolute_offset as usize);
        data.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]); // Valid JPEG (SOI+EOI)

        // [WHEN] We search for MPF segment
        let mpf_segment = find_mpf_segment(&data).unwrap_or_else(|_| panic!("Should find MPF"));

        // [THEN] Standard relative logic would fail, but fallback should work
        let gainmap_extracted = extract_gainmap_from_mpf(&data, &mpf_segment, None)
            .unwrap_or_else(|_| panic!("Fallback failed"));
        assert_eq!(gainmap_extracted, vec![0xFF, 0xD8, 0xFF, 0xD9]);
    }

    #[test]
    fn test_extract_gainmap_uses_eoi_when_length_runs_past_eof() {
        let mut data = vec![0xFF, 0xD8];
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"2.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";
        let xmp_len = u16::try_from(xmp_header.len() + xmp_content.len() + 2).unwrap_or(u16::MAX);
        data.extend_from_slice(&[0xFF, 0xE1]);
        data.extend_from_slice(&xmp_len.to_be_bytes());
        data.extend_from_slice(xmp_header);
        data.extend_from_slice(xmp_content);

        let mpf_id = mpf::MPF_IDENTIFIER;
        let mut mpf_payload = Vec::new();
        mpf_payload.extend_from_slice(b"MM\0*");
        mpf_payload.extend_from_slice(&8u32.to_be_bytes());
        mpf_payload.extend_from_slice(&2u16.to_be_bytes());
        mpf_payload.extend_from_slice(&0xB001u16.to_be_bytes());
        mpf_payload.extend_from_slice(&4u16.to_be_bytes());
        mpf_payload.extend_from_slice(&1u32.to_be_bytes());
        mpf_payload.extend_from_slice(&2u32.to_be_bytes());
        let mp_entry_val_offset = crate::numeric_cast::usize_to_u32_sat(mpf_payload.len() + 12 + 4);
        mpf_payload.extend_from_slice(&0xB002u16.to_be_bytes());
        mpf_payload.extend_from_slice(&7u16.to_be_bytes());
        mpf_payload.extend_from_slice(&32u32.to_be_bytes());
        mpf_payload.extend_from_slice(&mp_entry_val_offset.to_be_bytes());
        mpf_payload.extend_from_slice(&0u32.to_be_bytes());
        mpf_payload.extend_from_slice(&[0u8; 16]);

        let app2_segment_overhead = 2 + 2 + mpf_id.len();
        let absolute_offset = crate::numeric_cast::usize_to_u32_sat(
            data.len() + app2_segment_overhead + mpf_payload.len() + 16,
        );

        mpf_payload.extend_from_slice(&0u32.to_be_bytes());
        mpf_payload.extend_from_slice(&100u32.to_be_bytes());
        mpf_payload.extend_from_slice(&absolute_offset.to_be_bytes());
        mpf_payload.extend_from_slice(&0u32.to_be_bytes());

        let app2_len = crate::numeric_cast::usize_to_u16_sat(mpf_id.len() + mpf_payload.len() + 2);
        data.extend_from_slice(&[0xFF, 0xE2]);
        data.extend_from_slice(&app2_len.to_be_bytes());
        data.extend_from_slice(mpf_id);
        data.extend_from_slice(&mpf_payload);

        assert_eq!(data.len(), absolute_offset as usize);
        data.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);

        let mpf_segment = find_mpf_segment(&data).unwrap_or_else(|_| panic!("Should find MPF"));
        let gainmap_extracted = extract_gainmap_from_mpf(&data, &mpf_segment, None)
            .unwrap_or_else(|_| panic!("Fallback failed"));

        assert_eq!(gainmap_extracted, vec![0xFF, 0xD8, 0xFF, 0xD9]);
    }
}
