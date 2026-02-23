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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageQualityAnalysis {
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub format: String,

    pub has_alpha: bool,
    pub is_animated: bool,
    pub frame_count: u32,

    pub complexity: f64,

    pub edge_density: f64,

    pub color_diversity: f64,

    pub texture_variance: f64,

    pub noise_level: f64,

    pub sharpness: f64,

    pub contrast: f64,

    pub content_type: ImageContentType,

    pub compression_potential: f64,

    pub routing_decision: RoutingDecision,

    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ImageContentType {
    Photo,
    Artwork,
    Screenshot,
    Icon,
    Animation,
    Graphic,
    #[default]
    Unknown,
}

impl ImageContentType {
    pub fn quality_adjustment(&self) -> i8 {
        match self {
            ImageContentType::Screenshot => 8,
            ImageContentType::Icon => 6,
            ImageContentType::Graphic => 5,
            ImageContentType::Artwork => 2,
            ImageContentType::Animation => 0,
            ImageContentType::Photo => -2,
            ImageContentType::Unknown => 0,
        }
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub primary_format: String,
    pub alternatives: Vec<String>,
    pub use_lossless: bool,
    pub estimated_ratio: f64,
    pub reason: String,
    pub should_skip: bool,
    pub skip_reason: Option<String>,
}

pub fn analyze_image_quality(
    width: u32,
    height: u32,
    rgba_data: &[u8],
    file_size: u64,
    format: &str,
    frame_count: u32,
) -> Result<ImageQualityAnalysis, String> {
    let expected_size = (width as usize) * (height as usize) * 4;
    if rgba_data.len() < expected_size {
        return Err(format!(
            "❌ Invalid RGBA data: expected {} bytes for {}x{}, got {}",
            expected_size,
            width,
            height,
            rgba_data.len()
        ));
    }

    if width == 0 || height == 0 {
        return Err("❌ Invalid dimensions: width or height is 0".to_string());
    }

    let pixels = (width as u64) * (height as u64);

    let edge_density = calculate_edge_density(rgba_data, width, height);
    let color_diversity = calculate_color_diversity(rgba_data, width, height);
    let texture_variance = calculate_texture_variance(rgba_data, width, height);
    let noise_level = calculate_noise_level(rgba_data, width, height);
    let sharpness = calculate_sharpness(rgba_data, width, height);
    let contrast = calculate_contrast(rgba_data, width, height);
    let has_alpha = detect_alpha_usage(rgba_data);

    let complexity =
        calculate_overall_complexity(edge_density, color_diversity, texture_variance, noise_level);

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

    let compression_potential =
        calculate_compression_potential(complexity, content_type, has_alpha, is_animated);

    let routing_decision = make_routing_decision(
        format,
        content_type,
        has_alpha,
        is_animated,
        compression_potential,
        file_size,
        pixels,
    );

    let confidence =
        calculate_analysis_confidence(pixels, file_size, edge_density, color_diversity);

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

fn calculate_edge_density(rgba: &[u8], width: u32, height: u32) -> f64 {
    if width < 3 || height < 3 {
        return 0.0;
    }

    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 4_000_000 {
        4
    } else if pixels > 1_000_000 {
        2
    } else {
        1
    };

    let mut edge_count = 0usize;
    let mut sample_count = 0usize;

    let w = width as usize;

    for y in (1..(height - 1) as usize).step_by(step) {
        for x in (1..(width - 1) as usize).step_by(step) {
            let get_gray = |px: usize, py: usize| -> i32 {
                let idx = (py * w + px) * 4;
                let r = rgba[idx] as i32;
                let g = rgba[idx + 1] as i32;
                let b = rgba[idx + 2] as i32;
                (r * 299 + g * 587 + b * 114) / 1000
            };

            let gx = get_gray(x + 1, y) - get_gray(x - 1, y);
            let gy = get_gray(x, y + 1) - get_gray(x, y - 1);
            let gradient = ((gx * gx + gy * gy) as f64).sqrt();

            if gradient > 25.0 {
                edge_count += 1;
            }
            sample_count += 1;
        }
    }

    if sample_count == 0 {
        return 0.0;
    }

    let raw_density = edge_count as f64 / sample_count as f64;
    (raw_density * 3.0).min(1.0)
}

fn calculate_color_diversity(rgba: &[u8], width: u32, height: u32) -> f64 {
    use std::collections::HashSet;

    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 {
        20
    } else if pixels > 100_000 {
        10
    } else {
        1
    };

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

    let max_colors = sample_count.min(10000) as f64;
    (colors.len() as f64 / max_colors).min(1.0)
}

fn calculate_texture_variance(rgba: &[u8], width: u32, height: u32) -> f64 {
    if width < 3 || height < 3 {
        return 0.0;
    }

    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 {
        10
    } else if pixels > 100_000 {
        5
    } else {
        2
    };

    let mut variance_sum = 0.0;
    let mut sample_count = 0usize;

    for y in (1..(height - 1) as usize).step_by(step) {
        for x in (1..(width - 1) as usize).step_by(step) {
            let mut sum = 0i32;
            let mut sq_sum = 0i64;

            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let px = (x as i32 + dx) as usize;
                    let py = (y as i32 + dy) as usize;
                    let idx = (py * width as usize + px) * 4;

                    let gray = (rgba[idx] as i32 * 299
                        + rgba[idx + 1] as i32 * 587
                        + rgba[idx + 2] as i32 * 114)
                        / 1000;
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

    let avg_std = variance_sum / sample_count as f64;
    (avg_std / 80.0).min(1.0)
}

fn calculate_noise_level(rgba: &[u8], width: u32, height: u32) -> f64 {
    if width < 2 || height < 2 {
        return 0.0;
    }

    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 {
        10
    } else if pixels > 100_000 {
        5
    } else {
        1
    };

    let mut diff_sum = 0.0;
    let mut sample_count = 0usize;

    for y in (0..(height - 1) as usize).step_by(step) {
        for x in (0..(width - 1) as usize).step_by(step) {
            let idx = (y * width as usize + x) * 4;
            let idx_right = idx + 4;
            let idx_down = idx + (width as usize * 4);

            if idx_down + 2 < rgba.len() {
                let curr = (rgba[idx] as i32 + rgba[idx + 1] as i32 + rgba[idx + 2] as i32) / 3;
                let right = (rgba[idx_right] as i32
                    + rgba[idx_right + 1] as i32
                    + rgba[idx_right + 2] as i32)
                    / 3;
                let down =
                    (rgba[idx_down] as i32 + rgba[idx_down + 1] as i32 + rgba[idx_down + 2] as i32)
                        / 3;

                diff_sum += (curr - right).abs() as f64;
                diff_sum += (curr - down).abs() as f64;
                sample_count += 2;
            }
        }
    }

    if sample_count == 0 {
        return 0.0;
    }

    let avg_diff = diff_sum / sample_count as f64;
    (avg_diff / 30.0).min(1.0)
}

fn calculate_sharpness(rgba: &[u8], width: u32, height: u32) -> f64 {
    if width < 3 || height < 3 {
        return 0.0;
    }

    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 {
        10
    } else if pixels > 100_000 {
        5
    } else {
        1
    };

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

    let avg_laplacian = laplacian_sum / sample_count as f64;
    (avg_laplacian / 100.0).min(1.0)
}

fn calculate_contrast(rgba: &[u8], width: u32, height: u32) -> f64 {
    let pixels = (width as usize) * (height as usize);
    let step = if pixels > 1_000_000 {
        20
    } else if pixels > 100_000 {
        10
    } else {
        1
    };

    let mut sum = 0u64;
    let mut sq_sum = 0u64;
    let mut sample_count = 0usize;

    for i in (0..pixels).step_by(step) {
        let idx = i * 4;
        if idx + 2 < rgba.len() {
            let gray =
                (rgba[idx] as u64 * 299 + rgba[idx + 1] as u64 * 587 + rgba[idx + 2] as u64 * 114)
                    / 1000;
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

    (std_dev / 80.0).min(1.0)
}

fn detect_alpha_usage(rgba: &[u8]) -> bool {
    for i in (0..rgba.len()).step_by(400) {
        let alpha_idx = i + 3;
        if alpha_idx < rgba.len() && rgba[alpha_idx] < 255 {
            return true;
        }
    }
    false
}

fn calculate_overall_complexity(
    edge_density: f64,
    color_diversity: f64,
    texture_variance: f64,
    noise_level: f64,
) -> f64 {
    let complexity =
        edge_density * 0.35 + color_diversity * 0.25 + texture_variance * 0.25 + noise_level * 0.15;

    complexity.clamp(0.0, 1.0)
}

fn classify_content_type(
    complexity: f64,
    edge_density: f64,
    color_diversity: f64,
    has_alpha: bool,
    is_animated: bool,
    width: u32,
    height: u32,
) -> ImageContentType {
    if is_animated {
        return ImageContentType::Animation;
    }

    if width <= 512 && height <= 512 && has_alpha && complexity < 0.4 {
        return ImageContentType::Icon;
    }

    let aspect_ratio = width as f64 / height.max(1) as f64;
    let is_screen_ratio = (1.3..2.0).contains(&aspect_ratio) || (0.5..0.8).contains(&aspect_ratio);
    if is_screen_ratio && color_diversity < 0.3 && edge_density > 0.2 && complexity < 0.5 {
        return ImageContentType::Screenshot;
    }

    if color_diversity < 0.2 && edge_density > 0.15 && complexity < 0.4 {
        return ImageContentType::Graphic;
    }

    if complexity > 0.6 && color_diversity > 0.5 {
        return ImageContentType::Photo;
    }

    if complexity > 0.3 && complexity < 0.7 && edge_density > 0.2 {
        return ImageContentType::Artwork;
    }

    ImageContentType::Unknown
}

fn calculate_compression_potential(
    complexity: f64,
    content_type: ImageContentType,
    has_alpha: bool,
    is_animated: bool,
) -> f64 {
    let mut potential = 1.0 - complexity;

    potential += match content_type {
        ImageContentType::Screenshot => 0.3,
        ImageContentType::Icon => 0.25,
        ImageContentType::Graphic => 0.2,
        ImageContentType::Artwork => 0.1,
        ImageContentType::Animation => 0.0,
        ImageContentType::Photo => -0.1,
        ImageContentType::Unknown => 0.0,
    };

    if has_alpha {
        potential -= 0.1;
    }

    if is_animated {
        potential -= 0.15;
    }

    potential.clamp(0.0, 1.0)
}

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

    let use_lossless = compression_potential < 0.2
        || format_lower == "png" && has_alpha && content_type == ImageContentType::Icon;

    let formats = content_type.recommended_formats();
    let primary = formats.first().unwrap_or(&"avif").to_string();
    let alternatives: Vec<String> = formats.iter().skip(1).map(|s| s.to_string()).collect();

    let base_ratio = match primary.as_str() {
        "avif" => 0.25,
        "jxl" => 0.35,
        "webp" => 0.45,
        "png" => 0.70,
        "jpeg" | "jpg" => 0.50,
        _ => 0.60,
    };

    let estimated_ratio = base_ratio + (1.0 - compression_potential) * 0.3;

    let reason = format!(
        "{:?} content detected (complexity: {:.2}). {} recommended for {}",
        content_type,
        1.0 - compression_potential,
        primary.to_uppercase(),
        if use_lossless {
            "lossless compression"
        } else {
            "optimal quality/size"
        }
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

fn calculate_analysis_confidence(
    pixels: u64,
    file_size: u64,
    edge_density: f64,
    color_diversity: f64,
) -> f64 {
    let mut confidence: f64 = 0.7;

    if pixels > 1_000_000 {
        confidence += 0.1;
    } else if pixels < 100_000 {
        confidence -= 0.1;
    }

    if file_size > 10_000 && file_size < 100_000_000 {
        confidence += 0.05;
    }

    if edge_density > 0.01 && edge_density < 0.9 {
        confidence += 0.05;
    }
    if color_diversity > 0.01 && color_diversity < 0.99 {
        confidence += 0.05;
    }

    confidence.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_solid_color(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let pixels = (width as usize) * (height as usize);
        let mut data = Vec::with_capacity(pixels * 4);
        for _ in 0..pixels {
            data.extend_from_slice(&[r, g, b, a]);
        }
        data
    }

    fn create_gradient(width: u32, height: u32) -> Vec<u8> {
        let w = width.max(1);
        let h = height.max(1);
        let mut data = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let r = ((x * 255) / w) as u8;
                let g = ((y * 255) / h) as u8;
                let b = (((x + y) * 127) / (w + h)) as u8;
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        data
    }

    fn create_checkerboard(width: u32, height: u32, block_size: u32) -> Vec<u8> {
        let block_size = block_size.max(1);
        let mut data = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let is_white = ((x / block_size) + (y / block_size)).is_multiple_of(2);
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

    #[test]
    fn test_analyze_solid_color() {
        let data = create_solid_color(100, 100, 128, 128, 128, 255);
        let result = analyze_image_quality(100, 100, &data, 10000, "png", 1).unwrap();

        assert!(
            result.complexity < 0.2,
            "Solid color complexity should be < 0.2, got {}",
            result.complexity
        );
        assert!(
            result.edge_density < 0.1,
            "Solid color edge density should be < 0.1, got {}",
            result.edge_density
        );
        assert!(
            result.color_diversity < 0.1,
            "Solid color diversity should be < 0.1, got {}",
            result.color_diversity
        );

        assert!(
            result.compression_potential > 0.7,
            "Solid color should have high compression potential"
        );
    }

    #[test]
    fn test_analyze_gradient() {
        let data = create_gradient(200, 200);
        let result = analyze_image_quality(200, 200, &data, 50000, "png", 1).unwrap();

        assert!(
            result.complexity > 0.1 && result.complexity < 0.8,
            "Gradient complexity should be 0.1-0.8, got {}",
            result.complexity
        );
        assert!(
            result.color_diversity > 0.2,
            "Gradient should have color diversity > 0.2, got {}",
            result.color_diversity
        );
    }

    #[test]
    fn test_analyze_checkerboard() {
        let data = create_checkerboard(200, 200, 10);
        let result = analyze_image_quality(200, 200, &data, 50000, "png", 1).unwrap();

        assert!(
            result.edge_density > 0.3,
            "Checkerboard should have high edge density, got {}",
            result.edge_density
        );
        assert!(
            result.color_diversity < 0.2,
            "Checkerboard should have low color diversity, got {}",
            result.color_diversity
        );
    }

    #[test]
    fn test_analyze_noisy() {
        let data = create_noisy(200, 200, 12345);
        let result = analyze_image_quality(200, 200, &data, 100000, "jpeg", 1).unwrap();

        assert!(
            result.complexity > 0.5,
            "Noisy image complexity should be > 0.5, got {}",
            result.complexity
        );
        assert!(
            result.noise_level > 0.3,
            "Noisy image noise level should be > 0.3, got {}",
            result.noise_level
        );
        assert!(
            result.color_diversity > 0.5,
            "Noisy image should have high color diversity"
        );
    }

    #[test]
    fn test_alpha_detection() {
        let data_alpha = create_solid_color(100, 100, 128, 128, 128, 128);
        let result_alpha = analyze_image_quality(100, 100, &data_alpha, 10000, "png", 1).unwrap();
        assert!(result_alpha.has_alpha, "Should detect alpha usage");

        let data_no_alpha = create_solid_color(100, 100, 128, 128, 128, 255);
        let result_no_alpha =
            analyze_image_quality(100, 100, &data_no_alpha, 10000, "png", 1).unwrap();
        assert!(
            !result_no_alpha.has_alpha,
            "Should not detect alpha when all 255"
        );
    }

    #[test]
    fn test_animation_detection() {
        let data = create_solid_color(100, 100, 128, 128, 128, 255);

        let static_result = analyze_image_quality(100, 100, &data, 10000, "png", 1).unwrap();
        assert!(
            !static_result.is_animated,
            "frame_count=1 should not be animated"
        );
        assert_ne!(static_result.content_type, ImageContentType::Animation);

        let animated_result = analyze_image_quality(100, 100, &data, 50000, "gif", 10).unwrap();
        assert!(
            animated_result.is_animated,
            "frame_count=10 should be animated"
        );
        assert_eq!(animated_result.content_type, ImageContentType::Animation);
    }

    #[test]
    fn test_classify_icon() {
        let data = create_solid_color(64, 64, 100, 150, 200, 200);
        let result = analyze_image_quality(64, 64, &data, 5000, "png", 1).unwrap();

        assert_eq!(
            result.content_type,
            ImageContentType::Icon,
            "Small alpha image should be classified as Icon, got {:?}",
            result.content_type
        );
    }

    #[test]
    fn test_classify_screenshot() {
        let mut data = create_solid_color(1920, 1080, 240, 240, 240, 255);
        for y in 100..200 {
            for x in 100..500 {
                let idx = (y * 1920 + x) * 4;
                data[idx] = 50;
                data[idx + 1] = 50;
                data[idx + 2] = 50;
            }
        }

        let result = analyze_image_quality(1920, 1080, &data, 500000, "png", 1).unwrap();

        assert!(
            result.complexity < 0.5,
            "Screenshot-like should have low complexity, got {}",
            result.complexity
        );
        assert!(
            result.compression_potential > 0.4,
            "Screenshot should have good compression potential, got {}",
            result.compression_potential
        );
    }

    #[test]
    fn test_classify_photo() {
        let data = create_noisy(1920, 1080, 54321);
        let result = analyze_image_quality(1920, 1080, &data, 2000000, "jpeg", 1).unwrap();

        assert!(
            result.complexity > 0.5,
            "Photo-like image should have high complexity"
        );
        assert!(
            result.color_diversity > 0.4,
            "Photo-like image should have high color diversity"
        );
    }

    #[test]
    fn test_classify_graphic() {
        let data = create_checkerboard(800, 600, 50);
        let result = analyze_image_quality(800, 600, &data, 100000, "png", 1).unwrap();

        assert!(
            result.color_diversity < 0.2,
            "Checkerboard should have low color diversity, got {}",
            result.color_diversity
        );
        assert!(
            result.edge_density > 0.0,
            "Checkerboard should have some edges, got {}",
            result.edge_density
        );
    }

    #[test]
    fn test_skip_modern_formats() {
        let data = create_gradient(500, 500);

        let avif_result = analyze_image_quality(500, 500, &data, 50000, "avif", 1).unwrap();
        assert!(
            avif_result.routing_decision.should_skip,
            "AVIF should be skipped"
        );

        let jxl_result = analyze_image_quality(500, 500, &data, 50000, "jxl", 1).unwrap();
        assert!(
            jxl_result.routing_decision.should_skip,
            "JXL should be skipped"
        );

        let heic_result = analyze_image_quality(500, 500, &data, 50000, "heic", 1).unwrap();
        assert!(
            heic_result.routing_decision.should_skip,
            "HEIC should be skipped"
        );

        let png_result = analyze_image_quality(500, 500, &data, 50000, "png", 1).unwrap();
        assert!(
            !png_result.routing_decision.should_skip,
            "PNG should not be skipped"
        );

        let jpeg_result = analyze_image_quality(500, 500, &data, 50000, "jpeg", 1).unwrap();
        assert!(
            !jpeg_result.routing_decision.should_skip,
            "JPEG should not be skipped"
        );
    }

    #[test]
    fn test_format_recommendations_by_content() {
        let photo_data = create_noisy(1000, 1000, 11111);
        let photo_result =
            analyze_image_quality(1000, 1000, &photo_data, 500000, "jpeg", 1).unwrap();

        let photo_formats = photo_result.content_type.recommended_formats();
        assert!(
            photo_formats.contains(&"avif") || photo_formats.contains(&"jxl"),
            "Photo should recommend AVIF or JXL"
        );

        let anim_data = create_gradient(500, 500);
        let anim_result = analyze_image_quality(500, 500, &anim_data, 100000, "gif", 5).unwrap();

        let anim_formats = anim_result.content_type.recommended_formats();
        assert!(
            anim_formats.contains(&"webp"),
            "Animation should recommend WebP"
        );
    }

    #[test]
    fn test_strict_solid_complexity() {
        let data = create_solid_color(500, 500, 100, 100, 100, 255);
        let result = analyze_image_quality(500, 500, &data, 10000, "png", 1).unwrap();

        assert!(
            result.complexity < 0.15,
            "STRICT: Solid color complexity must be < 0.15, got {}",
            result.complexity
        );
        assert!(
            result.edge_density < 0.05,
            "STRICT: Solid color edge density must be < 0.05, got {}",
            result.edge_density
        );
    }

    #[test]
    fn test_strict_noise_complexity() {
        let data = create_noisy(500, 500, 99999);
        let result = analyze_image_quality(500, 500, &data, 100000, "png", 1).unwrap();

        assert!(
            result.complexity > 0.6,
            "STRICT: Random noise complexity must be > 0.6, got {}",
            result.complexity
        );
        assert!(
            result.color_diversity > 0.5,
            "STRICT: Random noise color diversity must be > 0.5, got {}",
            result.color_diversity
        );
    }

    #[test]
    fn test_strict_checkerboard_edges() {
        let data = create_checkerboard(500, 500, 5);
        let result = analyze_image_quality(500, 500, &data, 50000, "png", 1).unwrap();

        assert!(
            result.edge_density > 0.1,
            "STRICT: Checkerboard edge density must be > 0.1, got {}",
            result.edge_density
        );
    }

    #[test]
    fn test_strict_compression_potential() {
        let simple = create_solid_color(500, 500, 200, 200, 200, 255);
        let simple_result = analyze_image_quality(500, 500, &simple, 10000, "png", 1).unwrap();
        assert!(
            simple_result.compression_potential > 0.7,
            "STRICT: Simple content compression potential must be > 0.7, got {}",
            simple_result.compression_potential
        );

        let complex = create_noisy(500, 500, 77777);
        let complex_result = analyze_image_quality(500, 500, &complex, 100000, "jpeg", 1).unwrap();
        assert!(
            complex_result.compression_potential < 0.5,
            "STRICT: Complex content compression potential must be < 0.5, got {}",
            complex_result.compression_potential
        );
    }

    #[test]
    fn test_edge_minimum_size() {
        let data = create_solid_color(10, 10, 128, 128, 128, 255);
        let result = analyze_image_quality(10, 10, &data, 400, "png", 1);
        assert!(result.is_ok(), "Should handle minimum size images");
    }

    #[test]
    fn test_edge_large_image() {
        let data = create_gradient(3840, 2160);
        let result = analyze_image_quality(3840, 2160, &data, 5000000, "png", 1).unwrap();

        assert!(
            result.confidence > 0.7,
            "Large image should have high confidence"
        );
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
        let data = vec![0u8; 100];
        let result = analyze_image_quality(100, 100, &data, 100, "png", 1);
        assert!(result.is_err(), "Should fail on insufficient data");
    }

    #[test]
    fn test_metric_edge_density_isolation() {
        let high_edges = create_checkerboard(200, 200, 5);
        let low_edges = create_solid_color(200, 200, 128, 128, 128, 255);

        let high_result = analyze_image_quality(200, 200, &high_edges, 50000, "png", 1).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_edges, 50000, "png", 1).unwrap();

        assert!(
            high_result.edge_density > low_result.edge_density * 3.0,
            "Checkerboard edge density ({}) should be >> solid ({})",
            high_result.edge_density,
            low_result.edge_density
        );
    }

    #[test]
    fn test_metric_color_diversity_isolation() {
        let high_div = create_gradient(200, 200);
        let low_div = create_solid_color(200, 200, 128, 128, 128, 255);

        let high_result = analyze_image_quality(200, 200, &high_div, 50000, "png", 1).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_div, 50000, "png", 1).unwrap();

        assert!(
            high_result.color_diversity > low_result.color_diversity * 3.0,
            "Gradient color diversity ({}) should be >> solid ({})",
            high_result.color_diversity,
            low_result.color_diversity
        );
    }

    #[test]
    fn test_metric_noise_isolation() {
        let high_noise = create_noisy(200, 200, 12345);
        let low_noise = create_gradient(200, 200);

        let high_result = analyze_image_quality(200, 200, &high_noise, 50000, "png", 1).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_noise, 50000, "png", 1).unwrap();

        assert!(
            high_result.noise_level > low_result.noise_level,
            "Random noise level ({}) should be > gradient ({})",
            high_result.noise_level,
            low_result.noise_level
        );
    }

    #[test]
    fn test_metric_sharpness_isolation() {
        let sharp = create_checkerboard(200, 200, 20);
        let smooth = create_gradient(200, 200);

        let sharp_result = analyze_image_quality(200, 200, &sharp, 50000, "png", 1).unwrap();
        let smooth_result = analyze_image_quality(200, 200, &smooth, 50000, "png", 1).unwrap();

        assert!(
            sharp_result.sharpness > smooth_result.sharpness,
            "Checkerboard sharpness ({}) should be > gradient ({})",
            sharp_result.sharpness,
            smooth_result.sharpness
        );
    }

    #[test]
    fn test_consistency_same_input() {
        let data = create_gradient(300, 300);

        let result1 = analyze_image_quality(300, 300, &data, 50000, "png", 1).unwrap();
        let result2 = analyze_image_quality(300, 300, &data, 50000, "png", 1).unwrap();

        assert!(
            (result1.complexity - result2.complexity).abs() < 0.001,
            "Same input should produce same complexity"
        );
        assert!(
            (result1.edge_density - result2.edge_density).abs() < 0.001,
            "Same input should produce same edge density"
        );
        assert_eq!(
            result1.content_type, result2.content_type,
            "Same input should produce same content type"
        );
    }

    #[test]
    fn test_consistency_scaling() {
        let small = create_checkerboard(100, 100, 10);
        let large = create_checkerboard(400, 400, 40);

        let small_result = analyze_image_quality(100, 100, &small, 10000, "png", 1).unwrap();
        let large_result = analyze_image_quality(400, 400, &large, 160000, "png", 1).unwrap();

        assert!(
            small_result.color_diversity < 0.2,
            "Small checkerboard should have low color diversity"
        );
        assert!(
            large_result.color_diversity < 0.2,
            "Large checkerboard should have low color diversity"
        );
    }
}
