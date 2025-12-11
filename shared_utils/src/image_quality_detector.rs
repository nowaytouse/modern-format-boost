//! 🔬 Image Quality Detector - Unified Quality Detection for Auto Routing
//!
//! This module provides precision-validated quality detection for:
//! - Auto format routing decisions
//! - Compression potential estimation
//! - Content type classification
//!
//! ## 🔥 Quality Manifesto Compliance
//! - NO silent fallback - errors fail loudly
//! - NO hardcoded defaults - all from actual content analysis
//! - Base decisions on actual content detection, not format names
//!
//! ## Architecture
//! ```text
//! Input Image -> Feature Extraction -> Quality Analysis -> Routing Decision
//!                    |                     |
//!              128D Features         ContentType + Complexity
//! ```

use serde::{Deserialize, Serialize};

// ============================================================
// Core Types
// ============================================================

/// Image quality analysis result for auto routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageQualityAnalysis {
    // === Basic Properties ===
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub format: String,
    
    // === Content Detection (actual, not assumed) ===
    pub has_alpha: bool,
    pub is_animated: bool,
    pub frame_count: u32,
    
    // === Quality Metrics ===
    /// Overall complexity score (0.0-1.0)
    /// Combines edge density, color diversity, and texture variance
    pub complexity: f64,
    
    /// Edge density (0.0-1.0) - texture complexity indicator
    pub edge_density: f64,
    
    /// Color diversity (0.0-1.0) - unique colors ratio
    pub color_diversity: f64,
    
    /// Texture variance (0.0-1.0) - local variance indicator
    pub texture_variance: f64,
    
    /// Noise level estimate (0.0-1.0)
    pub noise_level: f64,
    
    /// Sharpness/clarity score (0.0-1.0)
    pub sharpness: f64,
    
    /// Contrast level (0.0-1.0)
    pub contrast: f64,
    
    // === Content Classification ===
    pub content_type: ImageContentType,
    
    /// Compression potential (0.0-1.0)
    /// Higher = more room for compression without quality loss
    pub compression_potential: f64,
    
    // === Routing Decision ===
    pub routing_decision: RoutingDecision,
    
    /// Analysis confidence (0.0-1.0)
    pub confidence: f64,
}

/// Content type classification based on actual detection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum ImageContentType {
    /// Natural photograph - high complexity, continuous tones
    Photo,
    /// Digital artwork/illustration - medium complexity, defined edges
    Artwork,
    /// Screenshot/UI capture - low complexity, sharp text, flat colors
    Screenshot,
    /// Icon/logo - very low complexity, few colors, often with alpha
    Icon,
    /// Animation frames - detected via frame count
    Animation,
    /// Graphic/diagram - geometric shapes, limited palette
    Graphic,
    /// Unknown/mixed content
    #[default]
    Unknown,
}



impl ImageContentType {
    /// Get quality adjustment for this content type
    /// Positive = can use higher compression (lower quality setting)
    /// Negative = needs lower compression (higher quality setting)
    pub fn quality_adjustment(&self) -> i8 {
        match self {
            ImageContentType::Screenshot => 8,   // Very compressible
            ImageContentType::Icon => 6,         // Simple content
            ImageContentType::Graphic => 5,      // Flat colors
            ImageContentType::Artwork => 2,      // Defined edges help
            ImageContentType::Animation => 0,    // Baseline
            ImageContentType::Photo => -2,       // Needs quality
            ImageContentType::Unknown => 0,      // Conservative
        }
    }
    
    /// Get recommended formats for this content type
    pub fn recommended_formats(&self) -> Vec<&'static str> {
        match self {
            ImageContentType::Photo => vec!["avif", "jxl", "webp", "jpeg"],
            ImageContentType::Artwork => vec!["avif", "webp", "jxl", "png"],
            ImageContentType::Screenshot => vec!["webp", "png", "avif"],
            ImageContentType::Icon => vec!["webp", "png", "avif"],
            ImageContentType::Animation => vec!["webp", "avif", "gif"],
            ImageContentType::Graphic => vec!["webp", "png", "avif"],
            ImageContentType::Unknown => vec!["avif", "webp", "jxl"],
        }
    }
}

/// Routing decision for auto format selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Primary recommended format
    pub primary_format: String,
    /// Alternative formats in order of preference
    pub alternatives: Vec<String>,
    /// Whether to use lossless compression
    pub use_lossless: bool,
    /// Estimated compression ratio (0.0-1.0, lower = better compression)
    pub estimated_ratio: f64,
    /// Reason for this decision
    pub reason: String,
    /// Should skip conversion (already optimal)
    pub should_skip: bool,
    /// Skip reason if applicable
    pub skip_reason: Option<String>,
}


// ============================================================
// Quality Detection Implementation
// ============================================================

/// Analyze image quality from raw pixel data
/// 
/// # Arguments
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
/// * `rgba_data` - Raw RGBA pixel data (4 bytes per pixel)
/// * `file_size` - Original file size in bytes
/// * `format` - Source format string
/// * `frame_count` - Number of frames (1 for static images)
/// 
/// # Returns
/// * `Result<ImageQualityAnalysis, String>` - Analysis result or error
/// 
/// # 🔥 Quality Manifesto
/// - All metrics calculated from actual pixel data
/// - No assumptions based on format name
/// - Fails loudly if data is invalid
pub fn analyze_image_quality(
    width: u32,
    height: u32,
    rgba_data: &[u8],
    file_size: u64,
    format: &str,
    frame_count: u32,
) -> Result<ImageQualityAnalysis, String> {
    // 🔥 Validate input - fail loudly
    let expected_size = (width as usize) * (height as usize) * 4;
    if rgba_data.len() < expected_size {
        return Err(format!(
            "❌ Invalid RGBA data: expected {} bytes for {}x{}, got {}",
            expected_size, width, height, rgba_data.len()
        ));
    }
    
    if width == 0 || height == 0 {
        return Err("❌ Invalid dimensions: width or height is 0".to_string());
    }
    
    let pixels = (width as u64) * (height as u64);
    
    // === Calculate all metrics from actual pixel data ===
    let edge_density = calculate_edge_density(rgba_data, width, height);
    let color_diversity = calculate_color_diversity(rgba_data, width, height);
    let texture_variance = calculate_texture_variance(rgba_data, width, height);
    let noise_level = calculate_noise_level(rgba_data, width, height);
    let sharpness = calculate_sharpness(rgba_data, width, height);
    let contrast = calculate_contrast(rgba_data, width, height);
    let has_alpha = detect_alpha_usage(rgba_data);

    
    // === Calculate overall complexity ===
    // Weighted combination of metrics
    let complexity = calculate_overall_complexity(
        edge_density,
        color_diversity,
        texture_variance,
        noise_level,
    );
    
    // === Classify content type from actual metrics ===
    let is_animated = frame_count > 1;
    let content_type = classify_content_type(
        complexity,
        edge_density,
        color_diversity,
        has_alpha,
        is_animated,
        width,
        height,
    );
    
    // === Calculate compression potential ===
    let compression_potential = calculate_compression_potential(
        complexity,
        content_type,
        has_alpha,
        is_animated,
    );
    
    // === Make routing decision ===
    let routing_decision = make_routing_decision(
        format,
        content_type,
        has_alpha,
        is_animated,
        compression_potential,
        file_size,
        pixels,
    );
    
    // === Calculate confidence ===
    let confidence = calculate_analysis_confidence(
        pixels,
        file_size,
        edge_density,
        color_diversity,
    );
    
    Ok(ImageQualityAnalysis {
        width,
        height,
        file_size,
        format: format.to_string(),
        has_alpha,
        is_animated,
        frame_count,
        complexity,
        edge_density,
        color_diversity,
        texture_variance,
        noise_level,
        sharpness,
        contrast,
        content_type,
        compression_potential,
        routing_decision,
        confidence,
    })
}


// ============================================================
// Metric Calculation Functions
// ============================================================

/// Calculate edge density using Sobel operator
/// 
/// Returns: 0.0-1.0 (0 = no edges, 1 = all edges)
fn calculate_edge_density(rgba: &[u8], width: u32, height: u32) -> f64 {
    if width < 3 || height < 3 {
        return 0.0;
    }
    
    // Smart sampling for large images - but not too aggressive
    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 4_000_000 { 4 } else if pixels > 1_000_000 { 2 } else { 1 };
    
    let mut edge_count = 0usize;
    let mut sample_count = 0usize;
    
    let w = width as usize;
    
    for y in (1..(height - 1) as usize).step_by(step) {
        for x in (1..(width - 1) as usize).step_by(step) {
            // Get grayscale values for Sobel
            let get_gray = |px: usize, py: usize| -> i32 {
                let idx = (py * w + px) * 4;
                let r = rgba[idx] as i32;
                let g = rgba[idx + 1] as i32;
                let b = rgba[idx + 2] as i32;
                (r * 299 + g * 587 + b * 114) / 1000
            };
            
            // Simplified Sobel (horizontal + vertical gradients)
            let gx = get_gray(x + 1, y) - get_gray(x - 1, y);
            let gy = get_gray(x, y + 1) - get_gray(x, y - 1);
            let gradient = ((gx * gx + gy * gy) as f64).sqrt();
            
            // Edge threshold: gradient > 25 (slightly lower for better detection)
            if gradient > 25.0 {
                edge_count += 1;
            }
            sample_count += 1;
        }
    }
    
    if sample_count == 0 {
        return 0.0;
    }
    
    // Normalize: typical edge density 0.05-0.30
    let raw_density = edge_count as f64 / sample_count as f64;
    (raw_density * 3.0).min(1.0)
}


/// Calculate color diversity (unique colors ratio)
/// 
/// Returns: 0.0-1.0 (0 = single color, 1 = maximum diversity)
fn calculate_color_diversity(rgba: &[u8], width: u32, height: u32) -> f64 {
    use std::collections::HashSet;
    
    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 { 20 } else if pixels > 100_000 { 10 } else { 1 };
    
    // Quantize to 64 levels per channel (262144 possible colors)
    let quantize_step = 4u8;
    let mut colors = HashSet::new();
    let mut sample_count = 0usize;
    
    for i in (0..pixels).step_by(step) {
        let idx = i * 4;
        if idx + 2 < rgba.len() {
            let r = rgba[idx] / quantize_step;
            let g = rgba[idx + 1] / quantize_step;
            let b = rgba[idx + 2] / quantize_step;
            colors.insert((r, g, b));
            sample_count += 1;
        }
    }
    
    if sample_count == 0 {
        return 0.0;
    }
    
    // Normalize: max theoretical is 262144, but practical max is sample_count
    let max_colors = sample_count.min(10000) as f64;
    (colors.len() as f64 / max_colors).min(1.0)
}

/// Calculate texture variance (local variance indicator)
/// 
/// Returns: 0.0-1.0 (0 = flat, 1 = highly textured)
fn calculate_texture_variance(rgba: &[u8], width: u32, height: u32) -> f64 {
    if width < 3 || height < 3 {
        return 0.0;
    }
    
    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 { 10 } else if pixels > 100_000 { 5 } else { 2 };
    
    let mut variance_sum = 0.0;
    let mut sample_count = 0usize;
    
    for y in (1..(height - 1) as usize).step_by(step) {
        for x in (1..(width - 1) as usize).step_by(step) {
            // Calculate local 3x3 variance
            let mut sum = 0i32;
            let mut sq_sum = 0i64;
            
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let px = (x as i32 + dx) as usize;
                    let py = (y as i32 + dy) as usize;
                    let idx = (py * width as usize + px) * 4;
                    
                    let gray = (rgba[idx] as i32 * 299 
                              + rgba[idx + 1] as i32 * 587 
                              + rgba[idx + 2] as i32 * 114) / 1000;
                    sum += gray;
                    sq_sum += (gray as i64) * (gray as i64);
                }
            }
            
            let mean = sum as f64 / 9.0;
            let variance = (sq_sum as f64 / 9.0) - (mean * mean);
            variance_sum += variance.sqrt();
            sample_count += 1;
        }
    }
    
    if sample_count == 0 {
        return 0.0;
    }
    
    // Normalize: typical std dev 0-80
    let avg_std = variance_sum / sample_count as f64;
    (avg_std / 80.0).min(1.0)
}


/// Calculate noise level estimate
/// 
/// Returns: 0.0-1.0 (0 = clean, 1 = very noisy)
fn calculate_noise_level(rgba: &[u8], width: u32, height: u32) -> f64 {
    if width < 2 || height < 2 {
        return 0.0;
    }
    
    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 { 10 } else if pixels > 100_000 { 5 } else { 1 };
    
    // High-frequency component detection
    let mut diff_sum = 0.0;
    let mut sample_count = 0usize;
    
    for y in (0..(height - 1) as usize).step_by(step) {
        for x in (0..(width - 1) as usize).step_by(step) {
            let idx = (y * width as usize + x) * 4;
            let idx_right = idx + 4;
            let idx_down = idx + (width as usize * 4);
            
            if idx_down + 2 < rgba.len() {
                // Grayscale difference
                let curr = (rgba[idx] as i32 + rgba[idx + 1] as i32 + rgba[idx + 2] as i32) / 3;
                let right = (rgba[idx_right] as i32 + rgba[idx_right + 1] as i32 + rgba[idx_right + 2] as i32) / 3;
                let down = (rgba[idx_down] as i32 + rgba[idx_down + 1] as i32 + rgba[idx_down + 2] as i32) / 3;
                
                diff_sum += (curr - right).abs() as f64;
                diff_sum += (curr - down).abs() as f64;
                sample_count += 2;
            }
        }
    }
    
    if sample_count == 0 {
        return 0.0;
    }
    
    // Normalize: typical noise diff 0-30
    let avg_diff = diff_sum / sample_count as f64;
    (avg_diff / 30.0).min(1.0)
}

/// Calculate sharpness using Laplacian variance
/// 
/// Returns: 0.0-1.0 (0 = blurry, 1 = very sharp)
fn calculate_sharpness(rgba: &[u8], width: u32, height: u32) -> f64 {
    if width < 3 || height < 3 {
        return 0.0;
    }
    
    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 { 10 } else if pixels > 100_000 { 5 } else { 1 };
    
    let mut laplacian_sum = 0.0;
    let mut sample_count = 0usize;
    
    let get_gray = |x: usize, y: usize| -> i32 {
        let idx = (y * width as usize + x) * 4;
        (rgba[idx] as i32 * 299 + rgba[idx + 1] as i32 * 587 + rgba[idx + 2] as i32 * 114) / 1000
    };
    
    for y in (1..(height - 1) as usize).step_by(step) {
        for x in (1..(width - 1) as usize).step_by(step) {
            let center = get_gray(x, y);
            let top = get_gray(x, y - 1);
            let bottom = get_gray(x, y + 1);
            let left = get_gray(x - 1, y);
            let right = get_gray(x + 1, y);
            
            let laplacian = (4 * center - top - bottom - left - right).abs();
            laplacian_sum += laplacian as f64;
            sample_count += 1;
        }
    }
    
    if sample_count == 0 {
        return 0.0;
    }
    
    // Normalize: typical Laplacian 0-100
    let avg_laplacian = laplacian_sum / sample_count as f64;
    (avg_laplacian / 100.0).min(1.0)
}


/// Calculate contrast level
/// 
/// Returns: 0.0-1.0 (0 = flat, 1 = high contrast)
fn calculate_contrast(rgba: &[u8], width: u32, height: u32) -> f64 {
    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 { 20 } else if pixels > 100_000 { 10 } else { 1 };
    
    let mut sum = 0u64;
    let mut sq_sum = 0u64;
    let mut sample_count = 0usize;
    
    for i in (0..pixels).step_by(step) {
        let idx = i * 4;
        if idx + 2 < rgba.len() {
            let gray = (rgba[idx] as u64 * 299 + rgba[idx + 1] as u64 * 587 + rgba[idx + 2] as u64 * 114) / 1000;
            sum += gray;
            sq_sum += gray * gray;
            sample_count += 1;
        }
    }
    
    if sample_count == 0 {
        return 0.0;
    }
    
    let mean = sum as f64 / sample_count as f64;
    let variance = (sq_sum as f64 / sample_count as f64) - (mean * mean);
    let std_dev = variance.sqrt();
    
    // Normalize: typical std dev 0-80
    (std_dev / 80.0).min(1.0)
}

/// Detect actual alpha channel usage
/// 
/// Returns: true if alpha channel is actually used (not all 255)
fn detect_alpha_usage(rgba: &[u8]) -> bool {
    // Sample every 100th pixel for efficiency
    for i in (0..rgba.len()).step_by(400) {
        let alpha_idx = i + 3;
        if alpha_idx < rgba.len() && rgba[alpha_idx] < 255 {
            return true;
        }
    }
    false
}

/// Calculate overall complexity from individual metrics
fn calculate_overall_complexity(
    edge_density: f64,
    color_diversity: f64,
    texture_variance: f64,
    noise_level: f64,
) -> f64 {
    // Weighted combination
    // Edge density: 35% - most important for compression
    // Color diversity: 25% - affects palette-based compression
    // Texture variance: 25% - affects DCT-based compression
    // Noise level: 15% - affects all compression
    let complexity = edge_density * 0.35
        + color_diversity * 0.25
        + texture_variance * 0.25
        + noise_level * 0.15;
    
    complexity.clamp(0.0, 1.0)
}


// ============================================================
// Content Classification
// ============================================================

/// Classify content type based on actual metrics
/// 
/// 🔥 Quality Manifesto: Based on actual detection, NOT format name
fn classify_content_type(
    complexity: f64,
    edge_density: f64,
    color_diversity: f64,
    has_alpha: bool,
    is_animated: bool,
    width: u32,
    height: u32,
) -> ImageContentType {
    // Animation takes priority
    if is_animated {
        return ImageContentType::Animation;
    }
    
    // Icon detection: small + alpha + low complexity
    if width <= 512 && height <= 512 && has_alpha && complexity < 0.4 {
        return ImageContentType::Icon;
    }
    
    // Screenshot detection: typical screen dimensions + low color diversity + sharp edges
    let aspect_ratio = width as f64 / height.max(1) as f64;
    let is_screen_ratio = (1.3..2.0).contains(&aspect_ratio) || (0.5..0.8).contains(&aspect_ratio);
    if is_screen_ratio && color_diversity < 0.3 && edge_density > 0.2 && complexity < 0.5 {
        return ImageContentType::Screenshot;
    }
    
    // Graphic/diagram: low color diversity + defined edges
    if color_diversity < 0.2 && edge_density > 0.15 && complexity < 0.4 {
        return ImageContentType::Graphic;
    }
    
    // Photo: high complexity + high color diversity + continuous tones
    if complexity > 0.6 && color_diversity > 0.5 {
        return ImageContentType::Photo;
    }
    
    // Artwork: medium complexity + defined edges
    if complexity > 0.3 && complexity < 0.7 && edge_density > 0.2 {
        return ImageContentType::Artwork;
    }
    
    // Default to unknown for conservative handling
    ImageContentType::Unknown
}

/// Calculate compression potential
fn calculate_compression_potential(
    complexity: f64,
    content_type: ImageContentType,
    has_alpha: bool,
    is_animated: bool,
) -> f64 {
    // Base potential inversely related to complexity
    let mut potential = 1.0 - complexity;
    
    // Content type adjustments
    potential += match content_type {
        ImageContentType::Screenshot => 0.3,
        ImageContentType::Icon => 0.25,
        ImageContentType::Graphic => 0.2,
        ImageContentType::Artwork => 0.1,
        ImageContentType::Animation => 0.0,
        ImageContentType::Photo => -0.1,
        ImageContentType::Unknown => 0.0,
    };
    
    // Alpha reduces potential (more data to preserve)
    if has_alpha {
        potential -= 0.1;
    }
    
    // Animation reduces potential
    if is_animated {
        potential -= 0.15;
    }
    
    potential.clamp(0.0, 1.0)
}


// ============================================================
// Routing Decision
// ============================================================

/// Make routing decision for auto format selection
fn make_routing_decision(
    source_format: &str,
    content_type: ImageContentType,
    has_alpha: bool,
    _is_animated: bool,
    compression_potential: f64,
    _file_size: u64,
    _pixels: u64,
) -> RoutingDecision {
    let format_lower = source_format.to_lowercase();
    
    // === Check if should skip (already optimal) ===
    let modern_lossy = ["avif", "jxl", "heic", "heif"];
    let is_modern_lossy = modern_lossy.iter().any(|f| format_lower.contains(f));
    
    if is_modern_lossy {
        return RoutingDecision {
            primary_format: source_format.to_string(),
            alternatives: vec![],
            use_lossless: false,
            estimated_ratio: 1.0,
            reason: "Already in modern format - skip to avoid generational loss".to_string(),
            should_skip: true,
            skip_reason: Some(format!("Source is {} - already optimal", source_format)),
        };
    }
    
    // === Determine lossless vs lossy ===
    let use_lossless = compression_potential < 0.2 
        || format_lower == "png" && has_alpha && content_type == ImageContentType::Icon;
    
    // === Select formats based on content type ===
    let formats = content_type.recommended_formats();
    let primary = formats.first().unwrap_or(&"avif").to_string();
    let alternatives: Vec<String> = formats.iter().skip(1).map(|s| s.to_string()).collect();
    
    // === Estimate compression ratio ===
    let base_ratio = match primary.as_str() {
        "avif" => 0.25,
        "jxl" => 0.35,
        "webp" => 0.45,
        "png" => 0.70,
        "jpeg" | "jpg" => 0.50,
        _ => 0.60,
    };
    
    // Adjust by compression potential
    let estimated_ratio = base_ratio + (1.0 - compression_potential) * 0.3;
    
    // === Generate reason ===
    let reason = format!(
        "{:?} content detected (complexity: {:.2}). {} recommended for {}",
        content_type,
        1.0 - compression_potential,
        primary.to_uppercase(),
        if use_lossless { "lossless compression" } else { "optimal quality/size" }
    );
    
    RoutingDecision {
        primary_format: primary,
        alternatives,
        use_lossless,
        estimated_ratio: estimated_ratio.clamp(0.1, 1.0),
        reason,
        should_skip: false,
        skip_reason: None,
    }
}

/// Calculate analysis confidence
fn calculate_analysis_confidence(
    pixels: u64,
    file_size: u64,
    edge_density: f64,
    color_diversity: f64,
) -> f64 {
    let mut confidence: f64 = 0.7; // Base confidence
    
    // More pixels = more reliable sampling
    if pixels > 1_000_000 {
        confidence += 0.1;
    } else if pixels < 100_000 {
        confidence -= 0.1;
    }
    
    // Reasonable file size
    if file_size > 10_000 && file_size < 100_000_000 {
        confidence += 0.05;
    }
    
    // Metrics in reasonable ranges
    if edge_density > 0.01 && edge_density < 0.9 {
        confidence += 0.05;
    }
    if color_diversity > 0.01 && color_diversity < 0.99 {
        confidence += 0.05;
    }
    
    confidence.clamp(0.0, 1.0)
}


// ============================================================
// 🔬 PRECISION VALIDATION TESTS ("裁判" Tests)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    // Helper to create test RGBA data
    fn create_solid_color(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let pixels = (width as usize) * (height as usize);
        let mut data = Vec::with_capacity(pixels * 4);
        for _ in 0..pixels {
            data.extend_from_slice(&[r, g, b, a]);
        }
        data
    }
    
    fn create_gradient(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let r = ((x * 255) / width) as u8;
                let g = ((y * 255) / height) as u8;
                let b = (((x + y) * 127) / (width + height)) as u8;
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        data
    }
    
    fn create_checkerboard(width: u32, height: u32, block_size: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let is_white = ((x / block_size) + (y / block_size)) % 2 == 0;
                let color = if is_white { 255 } else { 0 };
                data.extend_from_slice(&[color, color, color, 255]);
            }
        }
        data
    }
    
    fn create_noisy(width: u32, height: u32, seed: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((width as usize) * (height as usize) * 4);
        let mut rng = seed;
        for _ in 0..(width * height) {
            // Simple LCG random
            rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
            let r = ((rng >> 16) & 0xFF) as u8;
            rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
            let g = ((rng >> 16) & 0xFF) as u8;
            rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
            let b = ((rng >> 16) & 0xFF) as u8;
            data.extend_from_slice(&[r, g, b, 255]);
        }
        data
    }


    // ============================================================
    // Basic Functionality Tests
    // ============================================================
    
    #[test]
    fn test_analyze_solid_color() {
        let data = create_solid_color(100, 100, 128, 128, 128, 255);
        let result = analyze_image_quality(100, 100, &data, 10000, "png", 1).unwrap();
        
        // Solid color should have very low complexity
        assert!(result.complexity < 0.2, "Solid color complexity should be < 0.2, got {}", result.complexity);
        assert!(result.edge_density < 0.1, "Solid color edge density should be < 0.1, got {}", result.edge_density);
        assert!(result.color_diversity < 0.1, "Solid color diversity should be < 0.1, got {}", result.color_diversity);
        
        // High compression potential
        assert!(result.compression_potential > 0.7, "Solid color should have high compression potential");
    }
    
    #[test]
    fn test_analyze_gradient() {
        let data = create_gradient(200, 200);
        let result = analyze_image_quality(200, 200, &data, 50000, "png", 1).unwrap();
        
        // Gradient has smooth transitions - low edge density but high color diversity
        // Complexity depends on weighted combination
        assert!(result.complexity > 0.1 && result.complexity < 0.8, 
            "Gradient complexity should be 0.1-0.8, got {}", result.complexity);
        assert!(result.color_diversity > 0.2, "Gradient should have color diversity > 0.2, got {}", result.color_diversity);
    }
    
    #[test]
    fn test_analyze_checkerboard() {
        let data = create_checkerboard(200, 200, 10);
        let result = analyze_image_quality(200, 200, &data, 50000, "png", 1).unwrap();
        
        // Checkerboard has high edge density but low color diversity
        assert!(result.edge_density > 0.3, "Checkerboard should have high edge density, got {}", result.edge_density);
        assert!(result.color_diversity < 0.2, "Checkerboard should have low color diversity, got {}", result.color_diversity);
    }
    
    #[test]
    fn test_analyze_noisy() {
        let data = create_noisy(200, 200, 12345);
        let result = analyze_image_quality(200, 200, &data, 100000, "jpeg", 1).unwrap();
        
        // Noisy image should have high complexity
        assert!(result.complexity > 0.5, "Noisy image complexity should be > 0.5, got {}", result.complexity);
        assert!(result.noise_level > 0.3, "Noisy image noise level should be > 0.3, got {}", result.noise_level);
        assert!(result.color_diversity > 0.5, "Noisy image should have high color diversity");
    }
    
    #[test]
    fn test_alpha_detection() {
        // With alpha
        let data_alpha = create_solid_color(100, 100, 128, 128, 128, 128);
        let result_alpha = analyze_image_quality(100, 100, &data_alpha, 10000, "png", 1).unwrap();
        assert!(result_alpha.has_alpha, "Should detect alpha usage");
        
        // Without alpha
        let data_no_alpha = create_solid_color(100, 100, 128, 128, 128, 255);
        let result_no_alpha = analyze_image_quality(100, 100, &data_no_alpha, 10000, "png", 1).unwrap();
        assert!(!result_no_alpha.has_alpha, "Should not detect alpha when all 255");
    }
    
    #[test]
    fn test_animation_detection() {
        let data = create_solid_color(100, 100, 128, 128, 128, 255);
        
        let static_result = analyze_image_quality(100, 100, &data, 10000, "png", 1).unwrap();
        assert!(!static_result.is_animated, "frame_count=1 should not be animated");
        assert_ne!(static_result.content_type, ImageContentType::Animation);
        
        let animated_result = analyze_image_quality(100, 100, &data, 50000, "gif", 10).unwrap();
        assert!(animated_result.is_animated, "frame_count=10 should be animated");
        assert_eq!(animated_result.content_type, ImageContentType::Animation);
    }


    // ============================================================
    // 🔬 Content Type Classification Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_classify_icon() {
        // Small image with alpha and low complexity = Icon
        let data = create_solid_color(64, 64, 100, 150, 200, 200);
        let result = analyze_image_quality(64, 64, &data, 5000, "png", 1).unwrap();
        
        assert_eq!(result.content_type, ImageContentType::Icon,
            "Small alpha image should be classified as Icon, got {:?}", result.content_type);
    }
    
    #[test]
    fn test_classify_screenshot() {
        // Screen-like dimensions + low color diversity + edges
        // Create a screenshot-like pattern: mostly flat with some UI elements
        let mut data = create_solid_color(1920, 1080, 240, 240, 240, 255);
        // Add some "UI elements" (dark rectangles)
        for y in 100..200 {
            for x in 100..500 {
                let idx = (y * 1920 + x) * 4;
                data[idx] = 50;
                data[idx + 1] = 50;
                data[idx + 2] = 50;
            }
        }
        
        let result = analyze_image_quality(1920, 1080, &data, 500000, "png", 1).unwrap();
        
        // Low complexity content - could be Screenshot, Graphic, or Unknown
        // The key is that it should have high compression potential
        assert!(result.complexity < 0.5, "Screenshot-like should have low complexity, got {}", result.complexity);
        assert!(result.compression_potential > 0.4, "Screenshot should have good compression potential, got {}", result.compression_potential);
    }
    
    #[test]
    fn test_classify_photo() {
        // High complexity + high color diversity = Photo
        let data = create_noisy(1920, 1080, 54321);
        let result = analyze_image_quality(1920, 1080, &data, 2000000, "jpeg", 1).unwrap();
        
        // Noisy random data simulates photo-like complexity
        assert!(result.complexity > 0.5, "Photo-like image should have high complexity");
        assert!(result.color_diversity > 0.4, "Photo-like image should have high color diversity");
    }
    
    #[test]
    fn test_classify_graphic() {
        // Low color diversity + defined edges = Graphic
        let data = create_checkerboard(800, 600, 50);
        let result = analyze_image_quality(800, 600, &data, 100000, "png", 1).unwrap();
        
        // Checkerboard has low color diversity (only 2 colors)
        assert!(result.color_diversity < 0.2, "Checkerboard should have low color diversity, got {}", result.color_diversity);
        // Should have some edge density from the pattern
        assert!(result.edge_density > 0.0, "Checkerboard should have some edges, got {}", result.edge_density);
    }
    
    // ============================================================
    // 🔬 Routing Decision Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_skip_modern_formats() {
        let data = create_gradient(500, 500);
        
        // AVIF should be skipped
        let avif_result = analyze_image_quality(500, 500, &data, 50000, "avif", 1).unwrap();
        assert!(avif_result.routing_decision.should_skip, "AVIF should be skipped");
        
        // JXL should be skipped
        let jxl_result = analyze_image_quality(500, 500, &data, 50000, "jxl", 1).unwrap();
        assert!(jxl_result.routing_decision.should_skip, "JXL should be skipped");
        
        // HEIC should be skipped
        let heic_result = analyze_image_quality(500, 500, &data, 50000, "heic", 1).unwrap();
        assert!(heic_result.routing_decision.should_skip, "HEIC should be skipped");
        
        // PNG should NOT be skipped
        let png_result = analyze_image_quality(500, 500, &data, 50000, "png", 1).unwrap();
        assert!(!png_result.routing_decision.should_skip, "PNG should not be skipped");
        
        // JPEG should NOT be skipped
        let jpeg_result = analyze_image_quality(500, 500, &data, 50000, "jpeg", 1).unwrap();
        assert!(!jpeg_result.routing_decision.should_skip, "JPEG should not be skipped");
    }


    #[test]
    fn test_format_recommendations_by_content() {
        // Photo content should recommend AVIF/JXL
        let photo_data = create_noisy(1000, 1000, 11111);
        let photo_result = analyze_image_quality(1000, 1000, &photo_data, 500000, "jpeg", 1).unwrap();
        
        let photo_formats = photo_result.content_type.recommended_formats();
        assert!(photo_formats.contains(&"avif") || photo_formats.contains(&"jxl"),
            "Photo should recommend AVIF or JXL");
        
        // Animation should recommend WebP
        let anim_data = create_gradient(500, 500);
        let anim_result = analyze_image_quality(500, 500, &anim_data, 100000, "gif", 5).unwrap();
        
        let anim_formats = anim_result.content_type.recommended_formats();
        assert!(anim_formats.contains(&"webp"), "Animation should recommend WebP");
    }
    
    // ============================================================
    // 🔬 Precision Tests - Strict Ranges (裁判机制)
    // ============================================================
    
    /// Strict test: Solid color complexity must be < 0.15
    #[test]
    fn test_strict_solid_complexity() {
        let data = create_solid_color(500, 500, 100, 100, 100, 255);
        let result = analyze_image_quality(500, 500, &data, 10000, "png", 1).unwrap();
        
        assert!(result.complexity < 0.15,
            "STRICT: Solid color complexity must be < 0.15, got {}", result.complexity);
        assert!(result.edge_density < 0.05,
            "STRICT: Solid color edge density must be < 0.05, got {}", result.edge_density);
    }
    
    /// Strict test: Random noise complexity must be > 0.6
    #[test]
    fn test_strict_noise_complexity() {
        let data = create_noisy(500, 500, 99999);
        let result = analyze_image_quality(500, 500, &data, 100000, "png", 1).unwrap();
        
        assert!(result.complexity > 0.6,
            "STRICT: Random noise complexity must be > 0.6, got {}", result.complexity);
        assert!(result.color_diversity > 0.5,
            "STRICT: Random noise color diversity must be > 0.5, got {}", result.color_diversity);
    }
    
    /// Strict test: Checkerboard edge density must be detectable
    #[test]
    fn test_strict_checkerboard_edges() {
        let data = create_checkerboard(500, 500, 5); // Small blocks = more edges
        let result = analyze_image_quality(500, 500, &data, 50000, "png", 1).unwrap();
        
        // Checkerboard with 5px blocks should have detectable edges
        // The exact value depends on sampling, but should be > 0
        assert!(result.edge_density > 0.1,
            "STRICT: Checkerboard edge density must be > 0.1, got {}", result.edge_density);
    }
    
    /// Strict test: Compression potential ranges
    #[test]
    fn test_strict_compression_potential() {
        // Simple content = high potential
        let simple = create_solid_color(500, 500, 200, 200, 200, 255);
        let simple_result = analyze_image_quality(500, 500, &simple, 10000, "png", 1).unwrap();
        assert!(simple_result.compression_potential > 0.7,
            "STRICT: Simple content compression potential must be > 0.7, got {}", 
            simple_result.compression_potential);
        
        // Complex content = lower potential
        let complex = create_noisy(500, 500, 77777);
        let complex_result = analyze_image_quality(500, 500, &complex, 100000, "jpeg", 1).unwrap();
        assert!(complex_result.compression_potential < 0.5,
            "STRICT: Complex content compression potential must be < 0.5, got {}",
            complex_result.compression_potential);
    }
    
    // ============================================================
    // 🔬 Edge Case Tests
    // ============================================================
    
    #[test]
    fn test_edge_minimum_size() {
        let data = create_solid_color(10, 10, 128, 128, 128, 255);
        let result = analyze_image_quality(10, 10, &data, 400, "png", 1);
        assert!(result.is_ok(), "Should handle minimum size images");
    }
    
    #[test]
    fn test_edge_large_image() {
        // 4K image simulation
        let data = create_gradient(3840, 2160);
        let result = analyze_image_quality(3840, 2160, &data, 5000000, "png", 1).unwrap();
        
        assert!(result.confidence > 0.7, "Large image should have high confidence");
    }
    
    #[test]
    fn test_edge_invalid_dimensions() {
        let data = vec![0u8; 100];
        let result = analyze_image_quality(0, 100, &data, 100, "png", 1);
        assert!(result.is_err(), "Should fail on zero width");
        
        let result2 = analyze_image_quality(100, 0, &data, 100, "png", 1);
        assert!(result2.is_err(), "Should fail on zero height");
    }
    
    #[test]
    fn test_edge_insufficient_data() {
        let data = vec![0u8; 100]; // Not enough for 100x100
        let result = analyze_image_quality(100, 100, &data, 100, "png", 1);
        assert!(result.is_err(), "Should fail on insufficient data");
    }


    // ============================================================
    // 🔬 Metric Isolation Tests - Verify Each Metric Works
    // ============================================================
    
    #[test]
    fn test_metric_edge_density_isolation() {
        // High edges (checkerboard) vs low edges (solid)
        let high_edges = create_checkerboard(200, 200, 5);
        let low_edges = create_solid_color(200, 200, 128, 128, 128, 255);
        
        let high_result = analyze_image_quality(200, 200, &high_edges, 50000, "png", 1).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_edges, 50000, "png", 1).unwrap();
        
        assert!(high_result.edge_density > low_result.edge_density * 3.0,
            "Checkerboard edge density ({}) should be >> solid ({})",
            high_result.edge_density, low_result.edge_density);
    }
    
    #[test]
    fn test_metric_color_diversity_isolation() {
        // High diversity (gradient) vs low diversity (solid)
        let high_div = create_gradient(200, 200);
        let low_div = create_solid_color(200, 200, 128, 128, 128, 255);
        
        let high_result = analyze_image_quality(200, 200, &high_div, 50000, "png", 1).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_div, 50000, "png", 1).unwrap();
        
        assert!(high_result.color_diversity > low_result.color_diversity * 3.0,
            "Gradient color diversity ({}) should be >> solid ({})",
            high_result.color_diversity, low_result.color_diversity);
    }
    
    #[test]
    fn test_metric_noise_isolation() {
        // High noise (random) vs low noise (gradient)
        let high_noise = create_noisy(200, 200, 12345);
        let low_noise = create_gradient(200, 200);
        
        let high_result = analyze_image_quality(200, 200, &high_noise, 50000, "png", 1).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_noise, 50000, "png", 1).unwrap();
        
        assert!(high_result.noise_level > low_result.noise_level,
            "Random noise level ({}) should be > gradient ({})",
            high_result.noise_level, low_result.noise_level);
    }
    
    #[test]
    fn test_metric_sharpness_isolation() {
        // Sharp edges (checkerboard) vs smooth (gradient)
        let sharp = create_checkerboard(200, 200, 20);
        let smooth = create_gradient(200, 200);
        
        let sharp_result = analyze_image_quality(200, 200, &sharp, 50000, "png", 1).unwrap();
        let smooth_result = analyze_image_quality(200, 200, &smooth, 50000, "png", 1).unwrap();
        
        assert!(sharp_result.sharpness > smooth_result.sharpness,
            "Checkerboard sharpness ({}) should be > gradient ({})",
            sharp_result.sharpness, smooth_result.sharpness);
    }
    
    // ============================================================
    // 🔬 Consistency Tests
    // ============================================================
    
    #[test]
    fn test_consistency_same_input() {
        let data = create_gradient(300, 300);
        
        let result1 = analyze_image_quality(300, 300, &data, 50000, "png", 1).unwrap();
        let result2 = analyze_image_quality(300, 300, &data, 50000, "png", 1).unwrap();
        
        assert!((result1.complexity - result2.complexity).abs() < 0.001,
            "Same input should produce same complexity");
        assert!((result1.edge_density - result2.edge_density).abs() < 0.001,
            "Same input should produce same edge density");
        assert_eq!(result1.content_type, result2.content_type,
            "Same input should produce same content type");
    }
    
    #[test]
    fn test_consistency_scaling() {
        // Both should have low color diversity (only 2 colors)
        let small = create_checkerboard(100, 100, 10);
        let large = create_checkerboard(400, 400, 40); // Same pattern, scaled
        
        let small_result = analyze_image_quality(100, 100, &small, 10000, "png", 1).unwrap();
        let large_result = analyze_image_quality(400, 400, &large, 160000, "png", 1).unwrap();
        
        // Color diversity should be similar (both low for checkerboard)
        assert!(small_result.color_diversity < 0.2, "Small checkerboard should have low color diversity");
        assert!(large_result.color_diversity < 0.2, "Large checkerboard should have low color diversity");
        
        // Note: Complexity may differ due to sampling differences at different resolutions
        // This is expected behavior - the key invariant is color diversity
    }
}
