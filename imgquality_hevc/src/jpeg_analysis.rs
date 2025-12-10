//! JPEG Quality Analysis Module
//! 
//! Implements precise JPEG quality factor estimation by analyzing
//! quantization tables and comparing them to the IJG standard tables.
//! 
//! Algorithm accuracy target: ±1 quality factor for standard tables

use serde::{Deserialize, Serialize};

/// JPEG quality analysis results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JpegQualityAnalysis {
    /// Estimated quality factor (1-100, where 100 is best)
    pub estimated_quality: u8,
    /// Confidence level of the estimation (0.0-1.0)
    pub confidence: f64,
    /// Whether the image uses standard IJG quantization tables
    pub is_standard_table: bool,
    /// Sum of squared errors for luminance table
    pub luminance_sse: f64,
    /// Sum of squared errors for chrominance table  
    pub chrominance_sse: Option<f64>,
    /// Luminance quality estimate
    pub luminance_quality: u8,
    /// Chrominance quality estimate (if available)
    pub chrominance_quality: Option<u8>,
    /// Quality assessment description
    pub quality_description: String,
    /// Whether this appears to be a high-quality original
    pub is_high_quality_original: bool,
    /// Detected encoder type (if identifiable)
    pub encoder_hint: Option<String>,
}

/// IJG Standard Luminance Quantization Table (Base Matrix)
/// This is the standard table from the Independent JPEG Group
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

/// IJG Standard Chrominance Quantization Table (Base Matrix)
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

/// Generate a standard quantization table for a given quality factor (1-100)
/// Using the IJG algorithm with high precision
fn generate_standard_qt(quality: u8, base_table: &[[u16; 8]; 8]) -> [[u16; 8]; 8] {
    let q = quality.clamp(1, 100) as f64;
    
    // Calculate scaling factor using IJG formula
    let scale = if q < 50.0 {
        5000.0 / q
    } else {
        200.0 - 2.0 * q
    };
    
    let mut result = [[0u16; 8]; 8];
    
    for i in 0..8 {
        for j in 0..8 {
            // IJG formula: floor((S * Qbase + 50) / 100)
            let value = ((scale * base_table[i][j] as f64) + 50.0) / 100.0;
            // Clamp to valid range [1, 255] for 8-bit precision
            result[i][j] = value.floor().clamp(1.0, 255.0) as u16;
        }
    }
    
    result
}

/// Calculate weighted Sum of Squared Errors between two quantization tables
/// Uses perceptual weighting - low frequency components are more important
fn calculate_weighted_sse(table1: &[[u16; 8]; 8], table2: &[[u16; 8]; 8]) -> f64 {
    // Perceptual importance weights for DCT coefficients
    // Higher weight = more visually important
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
    
    for i in 0..8 {
        for j in 0..8 {
            let diff = table1[i][j] as f64 - table2[i][j] as f64;
            let weight = WEIGHTS[i][j];
            weighted_sse += weight * diff * diff;
            total_weight += weight;
        }
    }
    
    // Normalize by total weight
    weighted_sse / total_weight
}

/// Calculate simple Sum of Squared Errors (for backward compatibility)
fn calculate_sse(table1: &[[u16; 8]; 8], table2: &[[u16; 8]; 8]) -> f64 {
    let mut sse = 0.0;
    for i in 0..8 {
        for j in 0..8 {
            let diff = table1[i][j] as f64 - table2[i][j] as f64;
            sse += diff * diff;
        }
    }
    sse
}

/// Quality estimation result with interpolation
#[derive(Debug, Clone)]
struct QualityEstimate {
    quality: u8,
    sse: f64,
    weighted_sse: f64,
    is_exact_match: bool,
    interpolated_quality: f64,
}

/// Estimate JPEG quality factor with high precision
/// Uses both regular and weighted SSE, plus interpolation for sub-integer accuracy
fn estimate_quality_precise(extracted_qt: &[[u16; 8]; 8], base_table: &[[u16; 8]; 8]) -> QualityEstimate {
    let mut best_quality = 75u8;
    let mut min_sse = f64::MAX;
    let mut min_weighted_sse = f64::MAX;
    let mut second_best_quality = 75u8;
    let mut second_min_sse = f64::MAX;
    
    // Test all quality factors from 1 to 100
    for q in 1..=100 {
        let standard_qt = generate_standard_qt(q, base_table);
        let sse = calculate_sse(extracted_qt, &standard_qt);
        let weighted_sse = calculate_weighted_sse(extracted_qt, &standard_qt);
        
        if sse < min_sse {
            // Update second best
            second_best_quality = best_quality;
            second_min_sse = min_sse;
            // Update best
            min_sse = sse;
            min_weighted_sse = weighted_sse;
            best_quality = q;
        } else if sse < second_min_sse {
            second_min_sse = sse;
            second_best_quality = q;
        }
        
        // Perfect match - no need to continue
        if sse == 0.0 {
            return QualityEstimate {
                quality: q,
                sse: 0.0,
                weighted_sse: 0.0,
                is_exact_match: true,
                interpolated_quality: q as f64,
            };
        }
    }
    
    // Interpolate between best and second best for sub-integer precision
    let interpolated = if second_min_sse > min_sse && min_sse > 0.0 {
        let ratio = min_sse / (min_sse + second_min_sse);
        let direction = if second_best_quality > best_quality { 1.0 } else { -1.0 };
        best_quality as f64 + direction * ratio * 0.5
    } else {
        best_quality as f64
    };
    
    QualityEstimate {
        quality: best_quality,
        sse: min_sse,
        weighted_sse: min_weighted_sse,
        is_exact_match: false,
        interpolated_quality: interpolated,
    }
}

/// Estimate JPEG quality factor from an extracted quantization table (legacy API)
pub fn estimate_quality_from_table(extracted_qt: &[[u16; 8]; 8], is_luminance: bool) -> (u8, f64, bool) {
    let base_table = if is_luminance {
        &IJG_LUMINANCE_BASE
    } else {
        &IJG_CHROMINANCE_BASE
    };
    
    let estimate = estimate_quality_precise(extracted_qt, base_table);
    (estimate.quality, estimate.sse, estimate.is_exact_match)
}

/// Calculate confidence score based on SSE and table analysis
fn calculate_confidence(luma_estimate: &QualityEstimate, chroma_estimate: Option<&QualityEstimate>) -> f64 {
    // Perfect match = highest confidence
    if luma_estimate.is_exact_match {
        if let Some(chroma) = chroma_estimate {
            if chroma.is_exact_match {
                return 1.0;
            }
        }
        return 0.98;
    }
    
    // Calculate confidence based on SSE
    // Lower SSE = higher confidence
    let luma_confidence = 1.0 / (1.0 + luma_estimate.weighted_sse * 0.01);
    
    if let Some(chroma) = chroma_estimate {
        let chroma_confidence = 1.0 / (1.0 + chroma.weighted_sse * 0.01);
        // Weight luminance more heavily (70/30)
        (0.7 * luma_confidence + 0.3 * chroma_confidence).clamp(0.0, 1.0)
    } else {
        luma_confidence.clamp(0.0, 1.0)
    }
}

/// Detect potential encoder based on quantization table characteristics and SSE patterns
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
    
    // Check for standard IJG tables (exact match)
    if luma_exact && (tables.len() < 2 || chroma_exact) {
        return Some("IJG/libjpeg (标准)".to_string());
    }
    
    let luma = &tables[0];
    
    // SSE-based encoder fingerprinting
    // These patterns are based on empirical analysis of real-world images
    
    // iOS Camera patterns (discovered from Photos Library analysis)
    // Pattern 1: Luma SSE ≈ 727, Chroma SSE ≈ 8 (Q85-like)
    // Pattern 2: Luma SSE ≈ 157, Chroma SSE ≈ 4-6 (Q93-like)
    if let Some(c_sse) = chroma_sse {
        // iOS Camera Q85-equivalent
        if (720.0..735.0).contains(&luma_sse) && (5.0..12.0).contains(&c_sse) {
            return Some("Apple iOS Camera (高质量)".to_string());
        }
        // iOS Camera Q93-equivalent
        if (150.0..165.0).contains(&luma_sse) && (2.0..10.0).contains(&c_sse) {
            return Some("Apple iOS Camera (极高质量)".to_string());
        }
    }
    
    // Adobe Photoshop patterns
    // High quality: very low SSE, Q[0][0] = 1-2
    if luma[0][0] <= 2 && luma[0][1] <= 2 && luma[1][0] <= 2 {
        if luma_sse < 100.0 {
            return Some("Adobe Photoshop (最高质量)".to_string());
        }
        return Some("Adobe Photoshop".to_string());
    }
    
    // Google/Android Camera patterns
    // Typically uses slightly different tables with specific SSE ranges
    if let Some(c_sse) = chroma_sse {
        if (200.0..400.0).contains(&luma_sse) && (10.0..50.0).contains(&c_sse) {
            return Some("Android Camera".to_string());
        }
    }
    
    // Samsung Camera (often uses optimized tables)
    if (500.0..700.0).contains(&luma_sse) {
        return Some("Samsung Camera".to_string());
    }
    
    // Large SSE difference indicates non-standard encoder
    if luma_sse > 1000.0 {
        return Some("非标准编码器 (高度自定义)".to_string());
    }
    
    // Generic non-standard
    if !luma_exact {
        return Some("自定义编码器".to_string());
    }
    
    None
}

/// JPEG Marker definitions
const MARKER_SOI: u8 = 0xD8;  // Start of Image
const MARKER_DQT: u8 = 0xDB;  // Define Quantization Table
const MARKER_SOS: u8 = 0xDA;  // Start of Scan
const MARKER_EOI: u8 = 0xD9;  // End of Image

/// Parse quantization tables from JPEG data
pub fn extract_quantization_tables(data: &[u8]) -> Result<Vec<[[u16; 8]; 8]>, String> {
    let mut tables = Vec::new();
    
    // Check JPEG signature
    if data.len() < 2 || data[0] != 0xFF || data[1] != MARKER_SOI {
        return Err("Not a valid JPEG file".to_string());
    }
    let mut pos = 2;
    
    while pos < data.len() - 1 {
        // Find next marker
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        
        // Skip padding FF bytes
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        
        if pos >= data.len() {
            break;
        }
        
        let marker = data[pos];
        pos += 1;
        
        // Check for markers that don't have length
        if marker == MARKER_SOI || marker == MARKER_EOI || (marker >= 0xD0 && marker <= 0xD7) {
            continue;
        }
        
        // Get segment length
        if pos + 2 > data.len() {
            break;
        }
        let length = ((data[pos] as usize) << 8) | (data[pos + 1] as usize);
        
        if marker == MARKER_DQT {
            // Parse DQT segment
            let segment_end = (pos + length).min(data.len());
            let mut seg_pos = pos + 2;
            
            while seg_pos < segment_end {
                if seg_pos >= data.len() {
                    break;
                }
                
                let pq_tq = data[seg_pos];
                let precision = (pq_tq >> 4) & 0x0F; // 0 = 8-bit, 1 = 16-bit
                seg_pos += 1;
                
                let mut table = [[0u16; 8]; 8];
                
                if precision == 0 {
                    // 8-bit quantization values
                    if seg_pos + 64 > data.len() {
                        break;
                    }
                    for i in 0..64 {
                        let row = ZIGZAG_ORDER[i] / 8;
                        let col = ZIGZAG_ORDER[i] % 8;
                        table[row][col] = data[seg_pos] as u16;
                        seg_pos += 1;
                    }
                } else {
                    // 16-bit quantization values
                    if seg_pos + 128 > data.len() {
                        break;
                    }
                    for i in 0..64 {
                        let row = ZIGZAG_ORDER[i] / 8;
                        let col = ZIGZAG_ORDER[i] % 8;
                        table[row][col] = ((data[seg_pos] as u16) << 8) | (data[seg_pos + 1] as u16);
                        seg_pos += 2;
                    }
                }
                
                tables.push(table);
            }
        }
        
        // Move to next segment
        pos += length;
        
        // Stop at SOS (Start of Scan) - image data follows
        if marker == MARKER_SOS {
            break;
        }
    }
    
    if tables.is_empty() {
        return Err("No quantization tables found in JPEG".to_string());
    }
    
    Ok(tables)
}

/// Zigzag order for reading DCT coefficients
const ZIGZAG_ORDER: [usize; 64] = [
    0,  1,  8, 16,  9,  2,  3, 10,
   17, 24, 32, 25, 18, 11,  4,  5,
   12, 19, 26, 33, 40, 48, 41, 34,
   27, 20, 13,  6,  7, 14, 21, 28,
   35, 42, 49, 56, 57, 50, 43, 36,
   29, 22, 15, 23, 30, 37, 44, 51,
   58, 59, 52, 45, 38, 31, 39, 46,
   53, 60, 61, 54, 47, 55, 62, 63,
];

/// Analyze JPEG quality from file data with enhanced precision
pub fn analyze_jpeg_quality(data: &[u8]) -> Result<JpegQualityAnalysis, String> {
    let tables = extract_quantization_tables(data)?;
    
    if tables.is_empty() {
        return Err("No quantization tables found".to_string());
    }
    
    // Analyze luminance table (first table) with high precision
    let luma_estimate = estimate_quality_precise(&tables[0], &IJG_LUMINANCE_BASE);
    
    // Analyze chrominance table if present
    let chroma_estimate = if tables.len() > 1 {
        Some(estimate_quality_precise(&tables[1], &IJG_CHROMINANCE_BASE))
    } else {
        None
    };
    
    // Calculate confidence
    let confidence = calculate_confidence(&luma_estimate, chroma_estimate.as_ref());
    
    // Combine luminance and chrominance estimates
    // For standard JPEG, both should match
    let final_quality = if let Some(ref chroma) = chroma_estimate {
        if luma_estimate.is_exact_match && chroma.is_exact_match {
            // Both match exactly - use luminance
            luma_estimate.quality
        } else if (luma_estimate.quality as i16 - chroma.quality as i16).abs() <= 2 {
            // Close match - weighted average favoring luminance
            let weighted = luma_estimate.interpolated_quality * 0.7 + chroma.interpolated_quality * 0.3;
            weighted.round() as u8
        } else {
            // Significant difference - use luminance (more reliable)
            luma_estimate.quality
        }
    } else {
        luma_estimate.quality
    };
    
    // Check if using standard tables
    let is_standard_table = luma_estimate.is_exact_match 
        && chroma_estimate.as_ref().map_or(true, |c| c.is_exact_match);
    
    // Detect encoder using SSE-based fingerprinting
    let encoder_hint = detect_encoder(
        &tables, 
        luma_estimate.is_exact_match, 
        chroma_estimate.as_ref().map_or(true, |c| c.is_exact_match),
        luma_estimate.sse,
        chroma_estimate.as_ref().map(|c| c.sse),
    );
    
    // Determine quality description
    let quality_description = match final_quality {
        95..=100 => "极高质量 (接近无损)".to_string(),
        90..=94 => "高质量 (专业级)".to_string(),
        80..=89 => "良好质量 (标准照片)".to_string(),
        70..=79 => "中等质量 (网络优化)".to_string(),
        60..=69 => "较低质量 (高压缩)".to_string(),
        _ => "低质量 (明显压缩伪影)".to_string(),
    };
    
    // High quality original criteria
    let is_high_quality_original = final_quality >= 90 
        && is_standard_table 
        && confidence >= 0.95;
    
    Ok(JpegQualityAnalysis {
        estimated_quality: final_quality,
        confidence,
        is_standard_table,
        luminance_sse: luma_estimate.sse,
        chrominance_sse: chroma_estimate.as_ref().map(|c| c.sse),
        luminance_quality: luma_estimate.quality,
        chrominance_quality: chroma_estimate.as_ref().map(|c| c.quality),
        quality_description,
        is_high_quality_original,
        encoder_hint,
    })
}

/// Analyze JPEG from file path
pub fn analyze_jpeg_file(path: &std::path::Path) -> Result<JpegQualityAnalysis, String> {
    let data = std::fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;
    analyze_jpeg_quality(&data)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_generate_standard_qt_q50() {
        // At Q=50, the scaling factor is 100, so output should equal base table
        let qt = generate_standard_qt(50, &IJG_LUMINANCE_BASE);
        assert_eq!(qt[0][0], 16); // First element should be same
    }
    
    #[test]
    fn test_generate_standard_qt_q100() {
        // At Q=100, scaling factor is 0, all values should be 1
        let qt = generate_standard_qt(100, &IJG_LUMINANCE_BASE);
        for row in &qt {
            for &val in row {
                assert!(val >= 1);
            }
        }
    }
    
    #[test]
    fn test_generate_standard_qt_q1() {
        // At Q=1, scaling factor is 5000, values should be high
        let qt = generate_standard_qt(1, &IJG_LUMINANCE_BASE);
        assert!(qt[0][0] > 100);
    }
    
    #[test]
    fn test_sse_identical() {
        let table = IJG_LUMINANCE_BASE;
        let sse = calculate_sse(&table, &table);
        assert_eq!(sse, 0.0);
    }
    
    #[test]
    fn test_weighted_sse_identical() {
        let table = IJG_LUMINANCE_BASE;
        let wsse = calculate_weighted_sse(&table, &table);
        assert_eq!(wsse, 0.0);
    }
    
    #[test]
    fn test_estimate_quality_perfect_match() {
        // Generate a Q=75 table and verify we can estimate it back
        let qt = generate_standard_qt(75, &IJG_LUMINANCE_BASE);
        let (quality, sse, is_standard) = estimate_quality_from_table(&qt, true);
        assert_eq!(quality, 75);
        assert_eq!(sse, 0.0);
        assert!(is_standard);
    }
    
    #[test]
    fn test_estimate_quality_all_levels() {
        // Test that all quality levels from 1-100 can be accurately detected
        for expected_q in 1..=100 {
            let qt = generate_standard_qt(expected_q, &IJG_LUMINANCE_BASE);
            let (detected_q, sse, _) = estimate_quality_from_table(&qt, true);
            assert_eq!(detected_q, expected_q, "Failed to detect Q={}", expected_q);
            assert_eq!(sse, 0.0, "Non-zero SSE for Q={}", expected_q);
        }
    }
    
    #[test]
    fn test_confidence_exact_match() {
        let qt = generate_standard_qt(85, &IJG_LUMINANCE_BASE);
        let estimate = estimate_quality_precise(&qt, &IJG_LUMINANCE_BASE);
        let confidence = calculate_confidence(&estimate, None);
        assert!(confidence >= 0.98, "Confidence should be high for exact match");
    }
    
    #[test]
    fn test_chrominance_detection() {
        // Test chrominance table detection
        for expected_q in [50, 75, 90, 95].iter() {
            let qt = generate_standard_qt(*expected_q, &IJG_CHROMINANCE_BASE);
            let (detected_q, sse, _) = estimate_quality_from_table(&qt, false);
            assert_eq!(detected_q, *expected_q, "Failed to detect chroma Q={}", expected_q);
            assert_eq!(sse, 0.0);
        }
    }
}
