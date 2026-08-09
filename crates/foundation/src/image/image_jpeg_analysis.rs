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

const IJG_LUMINANCE_BASE: [[u16; 8]; 8] = crate::constants::JPEG_IJG_LUMINANCE_BASE;
const IJG_CHROMINANCE_BASE: [[u16; 8]; 8] = crate::constants::JPEG_IJG_CHROMINANCE_BASE;
const JPEG_MISSING_BYTE: u8 = 0xFF;

fn generate_standard_qt(quality: u8, base_table: &[[u16; 8]; 8]) -> [[u16; 8]; 8] {
    use crate::constants::{
        JPEG_IJG_ROUNDING_DIVISOR, JPEG_IJG_ROUNDING_OFFSET, JPEG_IJG_SCALE_FACTOR_HIGH_A,
        JPEG_IJG_SCALE_FACTOR_HIGH_B, JPEG_IJG_SCALE_FACTOR_LOW, JPEG_IJG_SCALE_THRESHOLD,
    };
    let q = f64::from(quality.clamp(1, 100));

    let scale = if q < JPEG_IJG_SCALE_THRESHOLD {
        JPEG_IJG_SCALE_FACTOR_LOW / q
    } else {
        JPEG_IJG_SCALE_FACTOR_HIGH_A.mul_add(-q, JPEG_IJG_SCALE_FACTOR_HIGH_B)
    };

    let mut result = [[0u16; 8]; 8];

    for (row, base_row) in result.iter_mut().zip(base_table.iter()) {
        for (cell, &base_value) in row.iter_mut().zip(base_row.iter()) {
            let value = f64::mul_add(scale, f64::from(base_value), JPEG_IJG_ROUNDING_OFFSET)
                / JPEG_IJG_ROUNDING_DIVISOR;
            let scaled = value
                .floor()
                .clamp(1.0, crate::constants::MAX_8BIT_VALUE_F64);
            *cell = crate::media_conversion_gate::delivery_jpeg_qt_cell_u16_or_one(scaled, quality);
        }
    }

    result
}

fn calculate_weighted_sse(table1: &[[u16; 8]; 8], table2: &[[u16; 8]; 8]) -> f64 {
    const WEIGHTS: [[f64; 8]; 8] = crate::constants::JPEG_SSE_WEIGHTS;

    let mut weighted_sse = 0.0_f64;
    let mut total_weight = 0.0_f64;

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
    let mut sse = 0.0_f64;
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

        if crate::numeric_cast::is_effectively_zero(
            sse,
            crate::numeric_cast::FloatContext::Accumulation,
        ) {
            return QualityEstimate {
                quality: q,
                sse: 0.0,
                weighted_sse: 0.0,
                is_exact_match: true,
                interpolated_quality: f64::from(q),
            };
        }
    }

    let interpolated = if second_min_sse > min_sse && min_sse > 0.0_f64 {
        let ratio = min_sse / (min_sse + second_min_sse);
        let direction = if second_best_quality > best_quality {
            1.0_f64
        } else {
            -1.0_f64
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
        if let Some(chroma) = chroma_estimate
            && chroma.is_exact_match
        {
            return 1.0;
        }
        return crate::constants::JPEG_CONFIDENCE_LUMA_ONLY;
    }

    let luma_confidence = 1.0_f64
        / luma_estimate
            .weighted_sse
            .mul_add(crate::constants::JPEG_CONFIDENCE_SSE_SCALE, 1.0);

    match chroma_estimate {
        None => luma_confidence.clamp(0.0, 1.0),
        Some(chroma) => {
            let chroma_confidence = 1.0_f64
                / chroma
                    .weighted_sse
                    .mul_add(crate::constants::JPEG_CONFIDENCE_SSE_SCALE, 1.0);
            crate::constants::JPEG_LUMA_WEIGHT
                .mul_add(
                    luma_confidence,
                    crate::constants::JPEG_CHROMA_WEIGHT * chroma_confidence,
                )
                .clamp(0.0, 1.0)
        }
    }
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
        use crate::constants::{
            JPEG_FINGERPRINT_APPLE_HIGH_CHROMA_MAX, JPEG_FINGERPRINT_APPLE_HIGH_CHROMA_MIN,
            JPEG_FINGERPRINT_APPLE_HIGH_LUMA_MAX, JPEG_FINGERPRINT_APPLE_HIGH_LUMA_MIN,
            JPEG_FINGERPRINT_APPLE_VERY_HIGH_CHROMA_MAX,
            JPEG_FINGERPRINT_APPLE_VERY_HIGH_CHROMA_MIN, JPEG_FINGERPRINT_APPLE_VERY_HIGH_LUMA_MAX,
            JPEG_FINGERPRINT_APPLE_VERY_HIGH_LUMA_MIN,
        };
        if (JPEG_FINGERPRINT_APPLE_HIGH_LUMA_MIN..JPEG_FINGERPRINT_APPLE_HIGH_LUMA_MAX)
            .contains(&luma_sse)
            && (JPEG_FINGERPRINT_APPLE_HIGH_CHROMA_MIN..JPEG_FINGERPRINT_APPLE_HIGH_CHROMA_MAX)
                .contains(&c_sse)
        {
            return Some("Apple iOS Camera (high quality)".to_string());
        }
        if (JPEG_FINGERPRINT_APPLE_VERY_HIGH_LUMA_MIN..JPEG_FINGERPRINT_APPLE_VERY_HIGH_LUMA_MAX)
            .contains(&luma_sse)
            && (JPEG_FINGERPRINT_APPLE_VERY_HIGH_CHROMA_MIN
                ..JPEG_FINGERPRINT_APPLE_VERY_HIGH_CHROMA_MAX)
                .contains(&c_sse)
        {
            return Some("Apple iOS Camera (very high quality)".to_string());
        }
    }

    if luma[0][0] <= 2 && luma[0][1] <= 2 && luma[1][0] <= 2 {
        if luma_sse < 100.0_f64 {
            return Some("Adobe Photoshop (highest quality)".to_string());
        }
        return Some("Adobe Photoshop".to_string());
    }

    if let Some(c_sse) = chroma_sse
        && (crate::constants::JPEG_FINGERPRINT_ANDROID_LUMA_MIN
            ..crate::constants::JPEG_FINGERPRINT_ANDROID_LUMA_MAX)
            .contains(&luma_sse)
        && (crate::constants::JPEG_FINGERPRINT_ANDROID_CHROMA_MIN
            ..crate::constants::JPEG_FINGERPRINT_ANDROID_CHROMA_MAX)
            .contains(&c_sse)
    {
        return Some("Android Camera".to_string());
    }

    if (crate::constants::JPEG_FINGERPRINT_SAMSUNG_LUMA_MIN
        ..crate::constants::JPEG_FINGERPRINT_SAMSUNG_LUMA_MAX)
        .contains(&luma_sse)
    {
        return Some("Samsung Camera".to_string());
    }

    if luma_sse > crate::constants::JPEG_FINGERPRINT_CUSTOM_THRESHOLD {
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
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            "JPEG AUDIT: Invalid signature | Forensic: Missing SOI marker (FFD8); refusing to \
             parse non-JPEG data",
        );
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

        let Some(marker) = data.get(pos).copied() else {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_JPEG,
                &format!(
                    "JPEG CORRUPTION AUDIT: Truncated marker at position {pos} | Forensic: \
                     Expected marker byte after 0xFF-padding; EOF reached"
                )
            );
            return Err(format!("Truncated JPEG at position {pos}: expected marker"));
        };
        pos += 1;

        if marker == MARKER_SOI || marker == MARKER_EOI || (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        if pos + 2 > data.len() {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_JPEG,
                &format!(
                    "JPEG CORRUPTION AUDIT: Truncated segment header at position {pos} | \
                     Forensic: Expected 2-byte length field; EOF reached"
                )
            );
            return Err(format!(
                "Truncated JPEG at position {pos}: expected segment length"
            ));
        }

        let Some(length_high) = data.get(pos).copied() else {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_JPEG,
                &format!(
                    "JPEG CORRUPTION AUDIT: Truncated length byte at position {pos} | Forensic: \
                     Failed to read segment length high byte"
                )
            );
            return Err("Failed to read segment length high byte".to_string());
        };
        let Some(length_low) = data.get(pos + 1).copied() else {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_JPEG,
                &format!(
                    "JPEG CORRUPTION AUDIT: Truncated length byte at position {} | Forensic: \
                     Failed to read segment length low byte",
                    pos + 1
                )
            );
            return Err("Failed to read segment length low byte".to_string());
        };
        let length = (usize::from(length_high) << 8_i32) | usize::from(length_low);

        if marker == MARKER_DQT {
            let segment_end = (pos + length).min(data.len());
            let mut seg_pos = pos + 2;

            while seg_pos < segment_end {
                if seg_pos >= data.len() {
                    break;
                }

                let Some(pq_tq) = data.get(seg_pos).copied() else {
                    crate::log_corruption!(
                        crate::infra::static_logs::messages::LABEL_JPEG,
                        &format!(
                            "JPEG CORRUPTION AUDIT: Truncated DQT segment at position {seg_pos} | \
                             Forensic: Unexpected EOF during quantization table property scan"
                        )
                    );
                    return Err(format!("Truncated DQT segment at position {seg_pos}"));
                };
                let precision = (pq_tq >> 4_i32) & 0x0F;
                seg_pos += 1;

                let mut table = [[0u16; 8]; 8];

                if precision == 0 {
                    if seg_pos + 64 > data.len() {
                        crate::media_conversion_gate::probe_image_format_batch_audit(
                            "probe_jpeg",
                            format!("DQT segment too short for 8-bit table at {seg_pos}"),
                        );
                        return Err(format!(
                            "DQT segment too short for 8-bit table at {seg_pos}"
                        ));
                    }
                    for &zigzag in &ZIGZAG_ORDER {
                        let row = zigzag / 8;
                        let col = zigzag % 8;
                        if let Some(cell) = table.get_mut(row).and_then(|r| r.get_mut(col)) {
                            *cell = u16::from(data[seg_pos]);
                        }
                        seg_pos += 1;
                    }
                } else {
                    if seg_pos + 128 > data.len() {
                        crate::media_conversion_gate::probe_image_format_batch_audit(
                            "probe_jpeg",
                            format!("DQT segment too short for 16-bit table at {seg_pos}"),
                        );
                        return Err(format!(
                            "DQT segment too short for 16-bit table at {seg_pos}"
                        ));
                    }
                    for &zigzag in &ZIGZAG_ORDER {
                        let row = zigzag / 8;
                        let col = zigzag % 8;
                        if let Some(cell) = table.get_mut(row).and_then(|r| r.get_mut(col)) {
                            *cell =
                                (u16::from(data[seg_pos]) << 8_i32) | u16::from(data[seg_pos + 1]);
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
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            "JPEG AUDIT: Empty DQT set | Forensic: SOS reached without any valid DQT segments \
             found; quality estimation impossible",
        );
        return Err("No quantization tables found in JPEG".to_string());
    }

    Ok(tables)
}

const ZIGZAG_ORDER: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Returns true if the JPEG data is complete (starts with SOI and contains
/// EOI).
///
/// This implementation is robust against trailing metadata (common in mobile
/// captures like Vivo/Samsung) by searching for the EOI marker (FF D9) in the
/// data.
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
    // To avoid false positives from thumbnails (which have their own SOI/EOI),
    // we ensure that the EOI marker follows the Start of Scan (SOS, FF DA) marker.
    // SOS marks the beginning of the actual image data scan.
    let last_sos = data.windows(2).rposition(|w| w == b"\xFF\xDA");
    let last_eoi = data.windows(2).rposition(|w| w == b"\xFF\xD9");

    match (last_sos, last_eoi) {
        (Some(sos), Some(eoi)) => eoi > sos,
        _ => false,
    }
}

/// Analyze JPEG quality by inspecting DQT (Define Quantization Table) markers.
///
/// # Errors
/// Returns an error if the JPEG data is invalid or DQT markers are missing.
///
/// # Panics
/// Panics if the generated standard quantization table contains out-of-range
/// values. This is considered a logic error as the IJG standard tables and
/// quality scaling should always result in valid 16-bit table entries.
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

    let final_quality = chroma_estimate.as_ref().map_or_else(
        || luma_estimate.quality,
        |chroma| {
            if luma_estimate.is_exact_match && chroma.is_exact_match {
                luma_estimate.quality
            } else if (i16::from(luma_estimate.quality) - i16::from(chroma.quality)).abs()
                <= i16::from(crate::constants::JPEG_QUALITY_MISMATCH_TOLERANCE)
            {
                let weighted = luma_estimate.interpolated_quality.mul_add(
                    crate::constants::JPEG_LUMA_WEIGHT,
                    chroma.interpolated_quality * crate::constants::JPEG_CHROMA_WEIGHT,
                );
                crate::media_conversion_gate::jpeg_weighted_quality_or_luma(
                    crate::numeric_cast::f64_to_u8_strict(
                        weighted.round(),
                        "jpeg_weighted_quality",
                    ),
                    luma_estimate.quality,
                )
            } else {
                luma_estimate.quality
            }
        },
    );

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
        crate::constants::QUALITY_LEVEL_ULTRA..=100 => {
            "Very high quality (near lossless)".to_string()
        }
        crate::constants::QUALITY_LEVEL_HIGH..=94 => "High quality (professional)".to_string(),
        crate::constants::QUALITY_LEVEL_GOOD..=89 => "Good quality (standard photo)".to_string(),
        crate::constants::QUALITY_LEVEL_MEDIUM..=79 => "Medium quality (web optimized)".to_string(),
        crate::constants::QUALITY_LEVEL_LOW..=69 => "Lower quality (high compression)".to_string(),
        _ => "Low quality (visible compression artifacts)".to_string(),
    };

    let is_high_quality_original = final_quality >= crate::constants::JPEG_HIGH_QUALITY_THRESHOLD
        && is_standard_table
        && confidence >= crate::constants::JPEG_CONFIDENCE_THRESHOLD_STANDARD;
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

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!(
            "JPEG quality analysis complete: quality={}, confidence={:.2}, standard={}, \
             luma_sse={:.2}, complete={}",
            final_quality, confidence, is_standard_table, luma_estimate.sse, is_complete
        )
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

/// Detect Google `UltraHDR` JPEG (gainmap embedded via MPF + XMP `hdrgm:`
/// namespace).
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

        let Some(marker) = data.get(pos + 1).copied() else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "Truncated JPEG at position {}: expected marker byte",
                    pos + 1
                ),
            );
            break;
        };
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
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("Truncated JPEG segment length at position {pos}"),
            );
            break;
        }
        let seg_len = usize::from(u16::from_be_bytes([data[pos], data[pos + 1]]));
        if seg_len < 2 || pos + seg_len > data.len() {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("Invalid segment length {seg_len} at position {pos}"),
            );
            break;
        }

        let Some(payload) = data.get(pos + 2..pos + seg_len) else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("Truncated segment payload at position {}", pos + 2),
            );
            break;
        };

        // APP2 (0xE2): check for XMP gainmap or MPF
        if marker == 0xE2 {
            if payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0") && payload.len() > 29 {
                let xmp_slice = if let Some(s) = payload.get(29..) {
                    s
                } else {
                    crate::media_conversion_gate::probe_image_format_batch_audit(
                        "probe_jpeg",
                        "APP1 XMP payload truncated at position 29",
                    );
                    &[]
                };
                let xmp = String::from_utf8_lossy(xmp_slice);
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
            let xmp =
                String::from_utf8_lossy(crate::media_conversion_gate::probe_jpeg_buffer_slice(
                    payload,
                    29..payload.len(),
                    "APP1 XMP payload truncated before namespace offset (XMP metadata will be \
                     lost)",
                ));
            if xmp.contains("hdrgm:") || xmp.contains("GainMap") || xmp.contains("gainmap") {
                has_gainmap_xmp = true;
            }
        }

        if has_gainmap_xmp && has_mpf {
            return true;
        }

        pos += seg_len;
    }

    // UltraHDR requires BOTH the XMP metadata parameters AND the MPF-linked
    // secondary image
    has_gainmap_xmp && has_mpf
}

/// Detect `UltraHDR` from file path.
pub fn is_ultra_hdr_jpeg_file(path: &std::path::Path) -> std::io::Result<bool> {
    let data = std::fs::read(path).map_err(|err| {
        crate::media_conversion_gate::probe_layer_audit(
            "ultrahdr_jpeg_read_failed",
            path,
            format!("UltraHDR JPEG read failed: {err}"),
        );
        std::io::Error::new(
            err.kind(),
            format!("UltraHDR JPEG read failed for {}: {err}", path.display()),
        )
    })?;
    Ok(is_ultra_hdr_jpeg(&data))
}

/// Extract XMP metadata string from JPEG data.
///
/// Searches for XMP segment (APP1) starting with "<http://ns.adobe.com/xap/1.0/\0>".
///
/// # Returns
/// - `Some(String)`: XMP metadata content
/// - `None`: No XMP segment found
#[must_use]
pub fn extract_xmp_from_jpeg_data(data: &[u8]) -> Option<Vec<String>> {
    let mut xmp_blocks = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        // Look for APP1 marker 0xFF 0xE1
        if data.get(pos..pos + 2) == Some(&[0xFF, 0xE1]) {
            if pos + 3 >= data.len() {
                break;
            }
            let seg_len = if let Some((b1, b2)) = data.get(pos + 2).zip(data.get(pos + 3)) {
                usize::from(u16::from_be_bytes([*b1, *b2]))
            } else {
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_jpeg",
                    format!("JPEG segment length bytes missing at position {pos}"),
                );
                break;
            };
            if seg_len < 2 || pos + 2 + seg_len > data.len() {
                pos += 1;
                continue;
            }

            let payload = crate::media_conversion_gate::probe_jpeg_buffer_slice(
                data,
                (pos + 4)..(pos + 2 + seg_len),
                &format!("JPEG segment payload truncated at position {}", pos + 4),
            );

            // APP1 (0xE1): XMP Standard
            if payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0") && payload.len() > 29 {
                let xmp =
                    String::from_utf8_lossy(crate::media_conversion_gate::probe_jpeg_buffer_slice(
                        payload,
                        29..payload.len(),
                        "APP1 XMP standard payload truncated (standard XMP metadata will be lost)",
                    ))
                    .to_string();
                xmp_blocks.push(xmp);
            }
            // APP1 (0xE1): XMP Extended
            else if payload.starts_with(b"http://ns.adobe.com/xmp/extension/\0")
                && payload.len() > 35 + 32 + 8
            {
                let xmp =
                    String::from_utf8_lossy(crate::media_conversion_gate::probe_jpeg_buffer_slice(
                        payload,
                        (35 + 32 + 8)..payload.len(),
                        "APP1 XMP extended payload truncated (extended XMP metadata will be lost)",
                    ))
                    .to_string();
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
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_JPEG,
            &format!("Extracted {} XMP blocks from JPEG stream", xmp_blocks.len())
        );
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
/// Returns an error if the JPEG is malformed, base image cannot be decoded, or
/// MPF/GainMap is missing.
pub struct UltraHdrJpegPayload {
    pub base_image: DynamicImage,
    pub gainmap_image: DynamicImage,
    pub gainmap_jpeg: Vec<u8>,
}

/// Extract the full `UltraHDR` JPEG payload, including the decoded base image,
/// decoded gainmap image, and the original embedded gainmap JPEG bytes.
///
/// # Errors
///
/// Returns an error if the JPEG is malformed, base image cannot be decoded, or
/// MPF/GainMap is missing.
pub fn extract_ultrahdr_jpeg_payload(data: &[u8]) -> Result<UltraHdrJpegPayload, String> {
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!(
            "Extracting gainmap from UltraHDR JPEG (size={} bytes)",
            data.len()
        )
    );

    // Validate JPEG signature
    if data.len() < 4 || data.get(0..2) != Some(&[0xFF, 0xD8]) {
        let b0 = crate::media_conversion_gate::probe_jpeg_byte_at(
            data,
            0,
            JPEG_MISSING_BYTE,
            "jpeg SOI b0",
        );
        let b1 = crate::media_conversion_gate::probe_jpeg_byte_at(
            data,
            1,
            JPEG_MISSING_BYTE,
            "jpeg SOI b1",
        );
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "JPEG AUDIT: Invalid signature during gainmap extraction | Forensic: Expected \
                 FFD8, got {b0:02X}{b1:02X}; len={}",
                data.len()
            ),
        );
        return Err(format!(
            "Invalid JPEG signature: expected FFD8, got {b0:02X}{b1:02X}. File size: {len} bytes. \
             This is not a valid JPEG file.",
            len = data.len()
        ));
    }

    let base_image = ImageReader::new(Cursor::new(data))
        .map_err(|e| {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "JPEG AUDIT: Failed to create base JPEG reader | Forensic: {}. File size={}; \
                     likely truncated or I/O error",
                    e,
                    data.len()
                ),
            );
            format!("Failed to create JPEG reader: {e}")
        })?
        .decode()
        .map_err(|e| {
            format!(
                "Failed to decode base JPEG image: {}. File size: {} bytes. The file may be \
                 corrupted or truncated.",
                e,
                data.len()
            )
        })?
        .0;

    let base_dims = base_image.dimensions();
    if base_dims.0 == 0 || base_dims.1 == 0 {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "JPEG AUDIT: Invalid base dimensions | Forensic: {}x{} (must be > 0x0); UltraHDR \
                 metadata cannot be mapped to empty canvas",
                base_dims.0, base_dims.1
            ),
        );
        return Err(format!(
            "Invalid base image dimensions: {}x{} (must be > 0x0). This indicates a corrupted \
             JPEG file or decoder bug.",
            base_dims.0, base_dims.1
        ));
    }

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!(
            "Base image decoded successfully ({}x{})",
            base_dims.0, base_dims.1
        )
    );
    let base_aspect = f64::from(base_dims.0) / f64::from(base_dims.1);

    let mpf_segment = find_mpf_segment(data)?;
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!("MPF segment found (size={} bytes)", mpf_segment.len())
    );

    let gainmap_data = extract_gainmap_from_mpf(data, &mpf_segment, Some(base_aspect))?;
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!(
            "Gainmap data extracted from MPF (size={} bytes)",
            gainmap_data.len()
        )
    );

    let gainmap_image = ImageReader::new(Cursor::new(&gainmap_data))
        .map_err(|e| {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "JPEG AUDIT: Failed to create gainmap JPEG reader | Forensic: {}. Extracted \
                     size={}; likely truncated or I/O error",
                    e,
                    gainmap_data.len()
                ),
            );
            format!("Failed to create gainmap JPEG reader: {e}")
        })?
        .decode()
        .map_err(|e| {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "JPEG AUDIT: Gainmap decoding failure | Forensic: {}. Extracted size={} \
                     bytes; potential bitstream corruption or unsupported codec sub-profile",
                    e,
                    gainmap_data.len()
                ),
            );
            format!(
                "Failed to decode gainmap image: {}. Extracted data: {} bytes. The gainmap may be \
                 corrupted.",
                e,
                gainmap_data.len()
            )
        })?
        .0;

    let gainmap_dims = gainmap_image.dimensions();
    if gainmap_dims.0 == 0 || gainmap_dims.1 == 0 {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "JPEG AUDIT: Invalid gainmap dimensions | Forensic: {}x{} (must be > 0x0); \
                 refusing to process empty gainmap canvas",
                gainmap_dims.0, gainmap_dims.1
            ),
        );
        return Err(format!(
            "Invalid gainmap dimensions: {}x{} (must be > 0x0). This indicates a corrupted \
             gainmap or decoder bug.",
            gainmap_dims.0, gainmap_dims.1
        ));
    }

    let gainmap_aspect = f64::from(gainmap_dims.0) / f64::from(gainmap_dims.1);
    let aspect_diff = (base_aspect - gainmap_aspect).abs();
    if aspect_diff > 0.01_f64 {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "Aspect ratio mismatch: base={:.4} ({}x{}), gainmap={:.4} ({}x{}). Difference: \
                 {:.4}. This may indicate incorrect gainmap extraction.",
                base_aspect,
                base_dims.0,
                base_dims.1,
                gainmap_aspect,
                gainmap_dims.0,
                gainmap_dims.1,
                aspect_diff
            ),
        );
    }

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!(
            "Gainmap extracted successfully: base={}x{}, gainmap={}x{}, aspect_diff={:.4}",
            base_dims.0, base_dims.1, gainmap_dims.0, gainmap_dims.1, aspect_diff
        )
    );

    Ok(UltraHdrJpegPayload {
        base_image,
        gainmap_image,
        gainmap_jpeg: gainmap_data,
    })
}

/// Extract base image and decoded gainmap from an `UltraHDR` JPEG byte stream.
///
/// # Errors
///
/// Returns an error if the JPEG is malformed, base image cannot be decoded, or
/// MPF/GainMap is missing.
pub fn extract_gainmap_from_jpeg(data: &[u8]) -> Result<(DynamicImage, DynamicImage), String> {
    let payload = extract_ultrahdr_jpeg_payload(data)?;
    Ok((payload.base_image, payload.gainmap_image))
}

/// MPF (Multi-Picture Format) structure constants
mod mpf {
    // MPF identifier: "MPF\0"
    pub(super) const MPF_IDENTIFIER: &[u8] = crate::constants::JPEG_MPF_IDENTIFIER;
    // Some devices use a non-standard APP2 identifier while keeping the MPF TIFF
    // layout.
    pub(super) const XMPF_IDENTIFIER: &[u8] = crate::constants::JPEG_XMPF_IDENTIFIER;

    // TIFF big-endian marker: "MM\0*"
    pub(super) const TIFF_BIG_ENDIAN: &[u8] = crate::constants::TIFF_BIG_ENDIAN;
    // TIFF little-endian marker: "II*\0"
    pub(super) const TIFF_LITTLE_ENDIAN: &[u8] = crate::constants::TIFF_LITTLE_ENDIAN;

    // MPF tags
    pub(super) const TAG_NUMBER_OF_IMAGES: u16 = crate::constants::JPEG_TAG_NUMBER_OF_IMAGES;
    pub(super) const TAG_MP_ENTRY: u16 = crate::constants::JPEG_TAG_MP_ENTRY;
}

const JPEG_SOI_BYTES: [u8; 3] = [0xFF, 0xD8, 0xFF];
const JPEG_EOI_BYTES: [u8; 2] = [0xFF, 0xD9];
const GAINMAP_SCAN_WINDOW_MIN: usize = crate::constants::JPEG_GAINMAP_SCAN_WINDOW_MIN;
const GAINMAP_SCAN_WINDOW_MAX: usize = crate::constants::JPEG_GAINMAP_SCAN_WINDOW_MAX;
const MAX_GAINMAP_SCAN_CANDIDATES: usize = crate::constants::JPEG_MAX_GAINMAP_SCAN_CANDIDATES;

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
    match payload.strip_prefix(mpf::MPF_IDENTIFIER) {
        Some(v) => Some(v),
        None => payload.strip_prefix(mpf::XMPF_IDENTIFIER),
    }
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

    for (offset, window) in crate::media_conversion_gate::probe_jpeg_buffer_slice(
        jpeg_data,
        range_start..bounded_end,
        "jpeg SOI scan range",
    )
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

    let mut candidate = crate::media_conversion_gate::probe_jpeg_buffer_slice(
        jpeg_data,
        start..end,
        "Gainmap candidate slice out of bounds (HDR gainmap detection will fail)",
    )
    .to_vec();
    let repaired_eoi = !candidate.ends_with(&JPEG_EOI_BYTES);
    if repaired_eoi {
        candidate.extend_from_slice(&JPEG_EOI_BYTES);
    }

    Some((candidate, repaired_eoi))
}

fn decode_gainmap_dimensions(candidate: &[u8]) -> Option<(u32, u32)> {
    use image::GenericImageView;

    let decoded = match image::load_from_memory(candidate) {
        Ok(d) => d,
        Err(e) => {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_JPEG,
                &format!("Gainmap candidate decoding failed: {e}")
            );
            return None;
        }
    };

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
        GainmapCandidateSource::RelativeOffset => {
            crate::constants::JPEG_GAINMAP_SCORE_RELATIVE_OFFSET
        }
        GainmapCandidateSource::AbsoluteOffset => {
            crate::constants::JPEG_GAINMAP_SCORE_ABSOLUTE_OFFSET
        }
        GainmapCandidateSource::NearbyScan => crate::constants::JPEG_GAINMAP_SCORE_NEARBY_SCAN,
        GainmapCandidateSource::TailScan => crate::constants::JPEG_GAINMAP_SCORE_TAIL_SCAN,
    };
    let aspect_penalty = match aspect_diff {
        None => {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                "Gainmap candidate aspect ratio missing; using moderate penalty fallback",
            );
            2_000.0_f64
        }
        Some(d) => d * 10_000.0_f64,
    };
    let length_penalty = if claimed_len == 0 {
        0.0_f64
    } else {
        (crate::numeric_cast::usize_to_f64(candidate_len.abs_diff(claimed_len))
            / crate::numeric_cast::usize_to_f64(claimed_len))
            * 100.0_f64
    };
    let repair_penalty = if repaired_eoi { 25.0_f64 } else { 0.0_f64 };

    source_weight - aspect_penalty - length_penalty - repair_penalty
}

fn gainmap_raw_fallback_score(
    source: GainmapCandidateSource,
    candidate_len: usize,
    claimed_len: usize,
    repaired_eoi: bool,
) -> f64 {
    let source_weight = match source {
        GainmapCandidateSource::RelativeOffset => 200.0_f64,
        GainmapCandidateSource::AbsoluteOffset => 150.0_f64,
        GainmapCandidateSource::NearbyScan | GainmapCandidateSource::TailScan => 0.0_f64,
    };
    let length_penalty = if claimed_len == 0 {
        0.0_f64
    } else {
        (crate::numeric_cast::usize_to_f64(candidate_len.abs_diff(claimed_len))
            / crate::numeric_cast::usize_to_f64(claimed_len))
            * 100.0_f64
    };
    let repair_penalty = if repaired_eoi { 25.0_f64 } else { 0.0_f64 };

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
            let found = crate::media_conversion_gate::probe_jpeg_byte_at(
                data,
                pos,
                JPEG_MISSING_BYTE,
                "jpeg marker",
            );
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "JPEG AUDIT: Segment sync failure | Forensic: Expected marker 0xFF at \
                     position {pos}, found {found:02X}; bitstream may be misaligned"
                ),
            );
            return Err(format!(
                "Invalid JPEG structure: expected marker 0xFF at position {pos}, found {found:02X}",
            ));
        }

        let Some(marker) = data.get(pos + 1).copied() else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "Truncated JPEG at position {}: expected marker byte",
                    pos + 1
                ),
            );
            break;
        };
        pos += 2;

        if marker == 0xDA || marker == 0xD9 {
            break;
        }

        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }

        if pos + 2 > data.len() {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_JPEG,
                &format!(
                    "JPEG CORRUPTION AUDIT: Truncated segment header | Forensic: Expected length \
                     field at position {pos}; EOF reached"
                )
            );
            return Err(format!("Truncated segment at position {pos}"));
        }

        let Some(length_high) = data.get(pos).copied() else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("Truncated segment at position {pos}: missing length high byte"),
            );
            break;
        };
        let Some(length_low) = data.get(pos + 1).copied() else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "Truncated segment at position {}: missing length low byte",
                    pos + 1
                ),
            );
            break;
        };
        let seg_len = usize::from(u16::from_be_bytes([length_high, length_low]));
        if seg_len < 2 || pos + seg_len > data.len() {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_JPEG,
                &format!(
                    "JPEG CORRUPTION AUDIT: Invalid segment length | Forensic: len={seg_len} at \
                     position {pos} (marker 0x{marker:02X}) exceeds EOF; bitstream is truncated"
                )
            );
            return Err(format!(
                "Invalid segment length {seg_len} at position {pos} (marker 0x{marker:02X})"
            ));
        }

        let seg_len = usize::from(u16::from_be_bytes([data[pos], data[pos + 1]]));
        if seg_len < 2 || pos + seg_len > data.len() {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("Invalid APP2 segment length {seg_len} at position {pos}"),
            );
            return Err(format!(
                "Invalid segment length {seg_len} at position {pos} (marker 0x{marker:02X})"
            ));
        }

        let payload = data.get(pos + 2..pos + seg_len).ok_or_else(|| {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("Failed to extract APP2 payload at position {}", pos + 2),
            );
            format!("Failed to extract APP2 payload at position {}", pos + 2)
        })?;

        if marker == 0xE2
            && let Some(mpf_payload) = strip_mpf_identifier(payload)
        {
            return Ok(mpf_payload.to_vec());
        }

        pos += seg_len;
    }

    Err("No MPF (Multi-Picture Format) segment found in APP2 markers".to_string())
}

/// Extract gainmap image data from MPF segment.
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
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            "Invalid MPF endianness marker",
        );
        let b0 = crate::media_conversion_gate::probe_jpeg_byte_at(
            mpf_data,
            0,
            JPEG_MISSING_BYTE,
            "mpf endian b0",
        );
        let b1 = crate::media_conversion_gate::probe_jpeg_byte_at(
            mpf_data,
            1,
            JPEG_MISSING_BYTE,
            "mpf endian b1",
        );
        let b2 = crate::media_conversion_gate::probe_jpeg_byte_at(
            mpf_data,
            2,
            JPEG_MISSING_BYTE,
            "mpf endian b2",
        );
        let b3 = crate::media_conversion_gate::probe_jpeg_byte_at(
            mpf_data,
            3,
            JPEG_MISSING_BYTE,
            "mpf endian b3",
        );
        return Err(format!(
            "Invalid MPF endianness marker: expected 'MM\\0*' or 'II*\\0', got {b0:02X} {b1:02X} \
             {b2:02X} {b3:02X}",
        ));
    };

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!(
            "MPF endianness: {}",
            if is_big_endian {
                "big-endian (MM)"
            } else {
                "little-endian (II)"
            }
        )
    );

    // Read first IFD offset (4 bytes after endianness marker)
    let first_ifd_offset = read_u32(
        crate::media_conversion_gate::probe_jpeg_buffer_slice(
            mpf_data,
            4..8,
            "mpf first IFD offset",
        ),
        is_big_endian,
    )?;
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!("First IFD offset: {first_ifd_offset}")
    );

    // Navigate to first IFD
    // first_ifd_offset is u32 from read_u32(); usize::try_from only fails if u32 >
    // usize::MAX, which cannot happen on 64-bit targets. Return Err rather than
    // panic on 32-bit overflow.
    let first_ifd_start = usize::try_from(first_ifd_offset).map_err(|_| {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "JPEG AUDIT: MPF offset overflow | Forensic: IFD offset {first_ifd_offset} \
                 exceeds usize; refusing to parse malformed bitstream"
            ),
        );
        format!("MPF first IFD offset {first_ifd_offset} exceeds usize — file is anomalous")
    })?;
    if first_ifd_start + 2 > mpf_data.len() {
        crate::log_corruption!(
            crate::infra::static_logs::messages::LABEL_JPEG,
            &format!(
                "JPEG CORRUPTION AUDIT: Invalid MPF IFD offset | Forensic: Offset {} exceeds MPF \
                 segment size {}; bitstream is truncated",
                first_ifd_offset,
                mpf_data.len()
            )
        );
        return Err(format!(
            "Invalid first IFD offset {}: exceeds MPF data size {}",
            first_ifd_offset,
            mpf_data.len()
        ));
    }

    // Read number of entries in IFD
    let num_entries = read_u16(
        mpf_data.get(first_ifd_start..).ok_or_else(|| {
            crate::log_corruption!(
                crate::infra::static_logs::messages::LABEL_JPEG,
                &format!(
                    "JPEG CORRUPTION AUDIT: MPF IFD range error | Forensic: start={} out of range \
                     len={}; refusing to probe further",
                    first_ifd_start,
                    mpf_data.len()
                )
            );
            format!(
                "MPF IFD offset {first_ifd_start} out of range {}",
                mpf_data.len()
            )
        })?,
        is_big_endian,
    )?;
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!("IFD entries: {num_entries}")
    );

    // Find MPEntry tag
    let mut mp_entry_offset: Option<u32> = None;
    let mut num_images: Option<u32> = None;

    let Ok(ifd_start) = usize::try_from(first_ifd_offset) else {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!("first_ifd_offset {first_ifd_offset} overflows usize"),
        );
        return Err(format!(
            "first_ifd_offset {first_ifd_offset} overflows usize"
        ));
    };
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

        let tag = if let Some(p) = mpf_data.get(entry_offset..entry_offset + 2) {
            read_u16(p, is_big_endian)?
        } else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("IFD entry {i} tag at {entry_offset} truncated"),
            );
            return Err(format!("IFD entry {i} tag at {entry_offset} truncated"));
        };
        let _data_type = if let Some(p) = mpf_data.get(entry_offset + 2..entry_offset + 4) {
            read_u16(p, is_big_endian)?
        } else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("IFD entry {i} type at {} truncated", entry_offset + 2),
            );
            return Err(format!(
                "IFD entry {i} type at {} truncated",
                entry_offset + 2
            ));
        };
        let num_components = if let Some(p) = mpf_data.get(entry_offset + 4..entry_offset + 8) {
            read_u32(p, is_big_endian)?
        } else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!("IFD entry {} count at {} truncated", i, entry_offset + 4),
            );
            return Err(format!(
                "IFD entry {} count at {} truncated",
                i,
                entry_offset + 4
            ));
        };
        let value_offset = if let Some(p) = mpf_data.get(entry_offset + 8..entry_offset + 12) {
            read_u32(p, is_big_endian)?
        } else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "IFD entry {} value/offset at {} truncated",
                    i,
                    entry_offset + 8
                ),
            );
            return Err(format!(
                "IFD entry {} value/offset at {} truncated",
                i,
                entry_offset + 8
            ));
        };

        match tag {
            mpf::TAG_NUMBER_OF_IMAGES => {
                // For NumberOfImages (tag 0xB001), it's a LONG (type 4, size 4).
                // If count is 1, the value is in value_offset.
                if num_components == 1 {
                    num_images = Some(value_offset);
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_JPEG,
                        &format!("NumberOfImages: {value_offset}")
                    );
                } else {
                    crate::media_conversion_gate::probe_image_format_batch_audit(
                        "probe_jpeg",
                        format!("NumberOfImages has unexpected component count: {num_components}"),
                    );
                    num_images = Some(num_components); // Fallback
                }
            }
            mpf::TAG_MP_ENTRY => {
                mp_entry_offset = Some(value_offset);
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_JPEG,
                    &format!("MPEntry offset: {value_offset}, count: {num_components}")
                );
            }
            _ => {}
        }
    }

    let mp_entry_offset = mp_entry_offset.ok_or_else(|| {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            "JPEG AUDIT: Missing MPEntry tag | Forensic: Tag 0xB002 missing from MPF IFD; \
             UltraHDR gainmap resolution impossible",
        );
        "MPEntry tag (0xB002) not found in IFD. This is not a valid MPF structure.".to_string()
    })?;

    let num_images = num_images.ok_or_else(|| {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            "JPEG AUDIT: Missing NumberOfImages tag | Forensic: Tag 0xB001 missing from MPF IFD; \
             refusing to guess asset count",
        );
        "NumberOfImages tag (0xB001) not found in IFD.".to_string()
    })?;

    if num_images < 2 {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "JPEG AUDIT: Insufficient MPF images | Forensic: Found {num_images} image(s); \
                 UltraHDR requires at least 2 (base + gainmap)"
            ),
        );
        return Err(format!(
            "MPF contains only {num_images} image(s). UltraHDR requires at least 2 images (base + \
             gainmap)."
        ));
    }

    // Navigate to MP Entry array
    let Ok(mp_entry_array_offset) = usize::try_from(mp_entry_offset) else {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!("mp_entry_offset {mp_entry_offset} overflows usize"),
        );
        return Err(format!("mp_entry_offset {mp_entry_offset} overflows usize"));
    };
    if mp_entry_array_offset + 16 > mpf_data.len() {
        crate::log_corruption!(
            crate::infra::static_logs::messages::LABEL_JPEG,
            &format!(
                "JPEG CORRUPTION AUDIT: MPF entry array out of bounds | Forensic: Offset {} \
                 exceeds segment size {}; bitstream is truncated",
                mp_entry_array_offset,
                mpf_data.len()
            )
        );
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
        crate::log_corruption!(
            crate::infra::static_logs::messages::LABEL_JPEG,
            &format!(
                "JPEG CORRUPTION AUDIT: Gainmap MP entry out of bounds | Forensic: Offset {} \
                 exceeds segment size {}; bitstream is truncated",
                gainmap_entry_offset,
                mpf_data.len()
            )
        );
        return Err(format!(
            "Gainmap entry offset {} exceeds MPF data size {}",
            gainmap_entry_offset,
            mpf_data.len()
        ));
    }

    let attributes = if let Some(p) = mpf_data.get(gainmap_entry_offset..gainmap_entry_offset + 4) {
        read_u32(p, is_big_endian)?
    } else {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!("Gainmap entry attributes truncated at {gainmap_entry_offset}"),
        );
        return Err("Gainmap entry attributes truncated".to_string());
    };
    let gainmap_length =
        if let Some(p) = mpf_data.get(gainmap_entry_offset + 4..gainmap_entry_offset + 8) {
            read_u32(p, is_big_endian)?
        } else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "Gainmap entry length truncated at {}",
                    gainmap_entry_offset + 4
                ),
            );
            return Err("Gainmap entry length truncated".to_string());
        };
    let gainmap_offset =
        if let Some(p) = mpf_data.get(gainmap_entry_offset + 8..gainmap_entry_offset + 12) {
            read_u32(p, is_big_endian)?
        } else {
            crate::media_conversion_gate::probe_image_format_batch_audit(
                "probe_jpeg",
                format!(
                    "Gainmap entry offset truncated at {}",
                    gainmap_entry_offset + 8
                ),
            );
            return Err("Gainmap entry offset truncated".to_string());
        };

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_JPEG,
        &format!(
            "Gainmap entry: attributes=0x{attributes:08X}, length={gainmap_length}, \
             offset={gainmap_offset}"
        )
    );

    // Validate gainmap length
    if gainmap_length == 0 {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            "JPEG AUDIT: Zero-length gainmap | Forensic: MP entry reports 0 bytes; refusing to \
             forge empty output",
        );
        return Err("Gainmap length is 0. Invalid MPF structure.".to_string());
    }

    let gainmap_len_usize =
        crate::numeric_cast::u32_to_usize_strict(gainmap_length, "gainmap_length")
            .ok_or_else(|| format!("gainmap_length {gainmap_length} overflows usize"))?;
    if gainmap_len_usize > jpeg_data.len() {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "Gainmap length {} exceeds JPEG file size {}; attempting recovery from available \
                 bytes",
                gainmap_length,
                jpeg_data.len()
            ),
        );
    }

    let mpf_base_pos = find_mpf_base_position(jpeg_data)?;
    let gainmap_offset_usize = usize::try_from(gainmap_offset)
        .map_err(|_| format!("gainmap_offset {gainmap_offset} overflows usize"))?;
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
            "Gainmap data at calculated position {} (offset {}, base {}) with length {} could not \
             be recovered from JPEG file size {}",
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
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "Recovered gainmap using MPF fallback candidate: start={}, source={:?}, \
                 repaired_eoi={}, decoded={}, aspect_diff={}",
                gainmap_candidate.start,
                gainmap_candidate.source,
                gainmap_candidate.repaired_eoi,
                gainmap_candidate.decoded,
                crate::media_conversion_gate::ui_f64_or_na(
                    gainmap_candidate.aspect_diff,
                    "jpeg_gainmap_aspect_diff",
                    4,
                )
            ),
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

        // Guarded by while pos + 1 < jpeg_data.len()
        let marker = jpeg_data[pos + 1];
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

        // Guarded by pos + 2 > jpeg_data.len()
        let seg_len = usize::from(u16::from_be_bytes([jpeg_data[pos], jpeg_data[pos + 1]]));
        if seg_len < 2 || pos + seg_len > jpeg_data.len() {
            break;
        }

        let payload = crate::media_conversion_gate::probe_jpeg_buffer_slice(
            jpeg_data,
            (pos + 2)..(pos + seg_len),
            "jpeg segment payload",
        );

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
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "JPEG AUDIT: Insufficient bytes for u16 read | Forensic: Expected 2 bytes, found \
                 {}; bitstream is truncated or misaligned",
                data.len()
            ),
        );
        return Err(format!(
            "Insufficient data for u16 read: {} bytes",
            data.len()
        ));
    }
    // Guarded by data.len() < 2
    Ok(if big_endian {
        u16::from_be_bytes([data[0], data[1]])
    } else {
        u16::from_le_bytes([data[0], data[1]])
    })
}

/// Read u32 from bytes with specified endianness
fn read_u32(data: &[u8], big_endian: bool) -> Result<u32, String> {
    if data.len() < 4 {
        crate::media_conversion_gate::probe_image_format_batch_audit(
            "probe_jpeg",
            format!(
                "JPEG AUDIT: Insufficient bytes for u32 read | Forensic: Expected 4 bytes, found \
                 {}; bitstream is truncated or misaligned",
                data.len()
            ),
        );
        return Err(format!(
            "Insufficient data for u32 read: {} bytes",
            data.len()
        ));
    }
    // Guarded by data.len() < 4
    Ok(if big_endian {
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    } else {
        u32::from_le_bytes([data[0], data[1], data[2], data[3]])
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
            confidence >= crate::constants::JPEG_CONFIDENCE_THRESHOLD_STRICT,
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
    fn test_extract_xmp_from_jpeg_data_with_xmp() -> anyhow::Result<()> {
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        let xmp_content = b"<x:xmpmeta>test content</x:xmpmeta>";

        let mut jpeg_with_xmp = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE1, // APP1 for XMP
        ];
        let xmp_len = u16::try_from(xmp_header.len() + xmp_content.len() + 2)
            .map_err(|_| anyhow::anyhow!("XMP segment length overflow"))?;
        jpeg_with_xmp.extend_from_slice(&xmp_len.to_be_bytes());
        jpeg_with_xmp.extend_from_slice(xmp_header);
        jpeg_with_xmp.extend_from_slice(xmp_content);
        jpeg_with_xmp.extend_from_slice(&[0xFF, 0xD9]); // EOI

        let extracted = extract_xmp_from_jpeg_data(&jpeg_with_xmp);
        assert!(extracted.is_some());
        let xmp_blocks = extracted.expect("XMP block should be extracted");
        assert_eq!(xmp_blocks.len(), 1);
        let xmp_str = xmp_blocks.first().expect("extracted XMP has one block");
        assert!(xmp_str.contains("test content"));
        Ok(())
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
    fn ultra_hdr_file_read_errors_are_not_false_negatives() {
        let err = is_ultra_hdr_jpeg_file(std::path::Path::new("missing.jpg")).unwrap_err();

        assert!(
            err.to_string().contains("UltraHDR JPEG read failed"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn test_is_ultra_hdr_jpeg_true_with_xmp_gainmap() -> anyhow::Result<()> {
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        // Use actual gainmap metadata format that contains "hdrgm:"
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"1.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";

        let mut jpeg_with_gainmap = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE1, // APP1 for XMP
        ];
        let xmp_len = u16::try_from(xmp_header.len() + xmp_content.len() + 2)
            .map_err(|_| anyhow::anyhow!("XMP segment length overflow"))?;
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
        Ok(())
    }

    #[test]
    fn test_is_ultra_hdr_jpeg_true_with_xmpf_identifier() {
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"1.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";

        let mut jpeg_with_gainmap = vec![0xFF, 0xD8, 0xFF, 0xE1];
        let xmp_len = crate::numeric_cast::usize_to_u16_strict(
            xmp_header.len() + xmp_content.len() + 2,
            "xmp_len",
        )
        .expect("XMP segment length should fit u16 in test");
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
        let err_msg = result.expect_err("invalid JPEG should return an error");
        assert!(err_msg.contains("Invalid JPEG signature"));
        assert!(err_msg.contains("FFD8"));
    }

    #[test]
    fn test_extract_gainmap_from_jpeg_empty() {
        let empty_data: &[u8] = &[];
        let result = extract_gainmap_from_jpeg(empty_data);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .is_some_and(|e| e.contains("Invalid JPEG signature"))
        );
    }

    #[test]
    fn test_extract_gainmap_from_jpeg_truncated() {
        // Test with truncated JPEG (valid signature but incomplete)
        let truncated = vec![0xFF, 0xD8, 0xFF, 0xE0]; // SOI + APP0 marker only
        let result = extract_gainmap_from_jpeg(&truncated);
        assert!(result.is_err());
        let err_msg = result.expect_err("truncated JPEG should return an error");
        assert!(
            err_msg.contains("Truncated")
                || err_msg.contains("Failed to decode")
                || err_msg.contains("Failed to create")
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
        use crate::hdr::GainMapParams;
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
        assert!(
            (params.offset_sdr - crate::constants::GAINMAP_OFFSET_DEFAULT).abs() < f32::EPSILON
        );
        assert!(
            (params.offset_hdr - crate::constants::GAINMAP_OFFSET_DEFAULT).abs() < f32::EPSILON
        );
    }

    #[test]
    fn test_hdr_intermediate_format_default() {
        use crate::hdr::IntermediateFormat;
        let format = IntermediateFormat::default();
        assert_eq!(format, IntermediateFormat::OpenExr32);
    }

    #[test]
    fn test_hdr_intermediate_format_debug() {
        use crate::hdr::IntermediateFormat;
        let exr = IntermediateFormat::OpenExr32;
        let png = IntermediateFormat::Png16;

        // Test Debug trait
        let exr_debug = format!("{exr:?}");
        let png_debug = format!("{png:?}");

        assert!(exr_debug.contains("OpenExr32"));
        assert!(png_debug.contains("Png16"));
    }

    #[test]
    fn test_hdr_intermediate_format_equality() {
        use crate::hdr::IntermediateFormat;
        assert_eq!(IntermediateFormat::OpenExr32, IntermediateFormat::OpenExr32);
        assert_eq!(IntermediateFormat::Png16, IntermediateFormat::Png16);
        assert_ne!(IntermediateFormat::OpenExr32, IntermediateFormat::Png16);
    }

    #[test]
    fn test_extract_quantization_tables_malformed() {
        // Too short
        assert!(extract_quantization_tables(&[0xFF, 0xD8]).is_err());
        // Invalid SOI
        assert!(extract_quantization_tables(&[0xFF, 0xD9, 0x00, 0x00]).is_err());
        // Truncated segment
        let data = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x05, 0x00, 0x01, 0x02];
        assert!(extract_quantization_tables(&data).is_err());
    }

    #[test]
    fn test_is_jpeg_complete_variants() {
        // Minimal valid (requires SOS for forensic scan tracking)
        assert!(is_jpeg_complete(&[
            0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x00, 0xFF, 0xD9
        ]));
        // With trailing junk
        assert!(is_jpeg_complete(&[
            0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x00, 0xFF, 0xD9, 0x00, 0x11, 0x22
        ]));
        // Missing SOI
        assert!(!is_jpeg_complete(&[0x00, 0x00, 0xFF, 0xD9]));
        // Missing EOI
        assert!(!is_jpeg_complete(&[0xFF, 0xD8, 0x00, 0x11]));
        // EOI inside data (should still be true if found after SOS)
        assert!(is_jpeg_complete(&[
            0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x00, 0xAA, 0xFF, 0xD9, 0xBB
        ]));
    }

    #[test]
    fn test_is_ultra_hdr_jpeg_with_padding_and_com() {
        let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"1.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";

        let mut data = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xFF, 0xFF, 0xE1, // APP1 with padding
        ];
        let xmp_len = crate::numeric_cast::usize_to_u16_strict(
            xmp_header.len() + xmp_content.len() + 2,
            "xmp_len",
        )
        .expect("XMP segment length should fit u16 in test");
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
        // [GIVEN] A JPEG with an MPF segment where the offset is absolute but not
        // relative
        let mut data = vec![0xFF, 0xD8]; // SOI

        // 1. APP1 XMP (simplified but must contain hdrgm:)
        let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"2.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";
        let xmp_hdr = b"http://ns.adobe.com/xap/1.0/\0";
        data.push(0xFF);
        data.push(0xE1); // APP1
        let xmp_len = crate::numeric_cast::usize_to_u16_strict(
            xmp_hdr.len() + xmp_content.len() + 2,
            "xmp_len",
        )
        .expect("marker length overflow in test");
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
        let mp_entry_val_offset = crate::numeric_cast::usize_to_u32_strict(
            mpf_payload.len() + 12 + 4,
            "mp_entry_val_offset",
        )
        .expect("offset overflow in test");
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
        let absolute_offset = crate::numeric_cast::usize_to_u32_strict(
            data.len() + app2_segment_overhead + mpf_payload.len() + 16,
            "absolute_offset",
        )
        .expect("offset overflow in test");

        let gainmap_size = 4u32;
        mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Attributes
        mpf_payload.extend_from_slice(&gainmap_size.to_be_bytes()); // Size
        mpf_payload.extend_from_slice(&absolute_offset.to_be_bytes()); // Absolute offset!
        mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Deps

        // Assemble APP2 segment
        let app2_len = crate::numeric_cast::usize_to_u16_strict(
            mpf_id.len() + mpf_payload.len() + 2,
            "app2_len",
        )
        .expect("marker length overflow in test");
        data.push(0xFF);
        data.push(0xE2); // APP2 marker
        data.extend_from_slice(&app2_len.to_be_bytes());
        data.extend_from_slice(mpf_id);
        data.extend_from_slice(&mpf_payload);

        // 4. Place Gainmap at the end (absolute_offset)
        assert_eq!(
            data.len(),
            crate::numeric_cast::u32_to_usize_strict(absolute_offset, "absolute_offset")
                .expect("absolute offset fits usize in test")
        );
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
        let xmp_len = crate::numeric_cast::usize_to_u16_strict(
            xmp_header.len() + xmp_content.len() + 2,
            "xmp_len",
        )
        .expect("XMP segment length should fit u16 in test");
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
        let mp_entry_val_offset = crate::numeric_cast::usize_to_u32_strict(
            mpf_payload.len() + 12 + 4,
            "mp_entry_val_offset",
        )
        .expect("offset overflow in test");
        mpf_payload.extend_from_slice(&0xB002u16.to_be_bytes());
        mpf_payload.extend_from_slice(&7u16.to_be_bytes());
        mpf_payload.extend_from_slice(&32u32.to_be_bytes());
        mpf_payload.extend_from_slice(&mp_entry_val_offset.to_be_bytes());
        mpf_payload.extend_from_slice(&0u32.to_be_bytes());
        mpf_payload.extend_from_slice(&[0u8; 16]);

        let app2_segment_overhead = 2 + 2 + mpf_id.len();
        let absolute_offset = crate::numeric_cast::usize_to_u32_strict(
            data.len() + app2_segment_overhead + mpf_payload.len() + 16,
            "absolute_offset",
        )
        .expect("offset overflow in test");

        mpf_payload.extend_from_slice(&0u32.to_be_bytes());
        mpf_payload.extend_from_slice(&100u32.to_be_bytes());
        mpf_payload.extend_from_slice(&absolute_offset.to_be_bytes());
        mpf_payload.extend_from_slice(&0u32.to_be_bytes());

        let app2_len = crate::numeric_cast::usize_to_u16_strict(
            mpf_id.len() + mpf_payload.len() + 2,
            "app2_len",
        )
        .expect("marker length overflow in test");
        data.extend_from_slice(&[0xFF, 0xE2]);
        data.extend_from_slice(&app2_len.to_be_bytes());
        data.extend_from_slice(mpf_id);
        data.extend_from_slice(&mpf_payload);

        assert_eq!(
            data.len(),
            crate::numeric_cast::u32_to_usize_strict(absolute_offset, "absolute_offset")
                .expect("absolute offset fits usize in test")
        );
        data.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);

        let mpf_segment = find_mpf_segment(&data).unwrap_or_else(|_| panic!("Should find MPF"));
        let gainmap_extracted = extract_gainmap_from_mpf(&data, &mpf_segment, None)
            .unwrap_or_else(|_| panic!("Fallback failed"));

        assert_eq!(gainmap_extracted, vec![0xFF, 0xD8, 0xFF, 0xD9]);
    }
}
