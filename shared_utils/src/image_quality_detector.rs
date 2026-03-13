//! 🔬 Image Quality Detector - Quality Dimensions (Routing Path Deprecated)
//!
//! This module provides **pixel-based quality dimensions** for quality judgment.
//! Main conversion routing uses **image_analyzer** (container/metadata-based); do not use
//! this module for routing decisions in img_hevc/img_av1.
//!
//! ## Retained for quality judgment (保留的维度)
//! The following dimensions remain available and are **not** deprecated:
//! - **ImageContentType** (Photo, Artwork, Screenshot, Icon, Animation, Graphic)
//! - **complexity**, **edge_density**, **color_diversity**, **texture_variance**
//! - **noise_level**, **sharpness**, **contrast**
//! - **compression_potential**, **confidence**
//!
//! Use these for quality analysis, tuning, or future quality gates; do not use `routing_decision`
//! or this module's output for main-path format routing.
//!
//! ## Deprecated (早期路由方案，已废弃)
//! - Using **analyze_image_quality** for conversion routing is deprecated.
//! - **RoutingDecision** (should_skip, primary_format, etc.) is deprecated for routing; main flow
//!   uses image_analyzer + image_recommender.

#![allow(deprecated)] // internal use of deprecated RoutingDecision / analyze_image_quality; deprecation applies to external callers

use crate::progress_mode::{has_log_file, write_to_log_at_level};
use tracing::Level;
use image::{open, GenericImageView};
use serde::{Deserialize, Serialize};
use crate::image_detection::PrecisionMetadata;
use std::path::Path;

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

    pub precision: PrecisionMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ImageContentType {
    pub name: String,
    pub adjust: i8,
    pub formats: Vec<String>,
    pub bonus: f64,
}

impl ImageContentType {
    pub fn quality_adjustment(&self) -> i8 {
        self.adjust
    }

    pub fn recommended_formats(&self) -> Vec<&str> {
        self.formats.iter().map(|s| s.as_str()).collect()
    }
}

#[derive(Debug, Deserialize)]
struct ClassifierRule {
    name: String,
    priority: i32,
    adjust: i8,
    formats: Vec<String>,
    bonus: f64,
    rules: RuleConditions,
}

#[derive(Debug, Deserialize)]
struct RuleConditions {
    is_animated: Option<bool>,
    has_alpha: Option<bool>,
    complexity: Option<ThresholdRange>,
    edge_density: Option<ThresholdRange>,
    color_diversity: Option<ThresholdRange>,
    texture_variance: Option<ThresholdRange>,
    noise_level: Option<ThresholdRange>,
    sharpness: Option<ThresholdRange>,
    contrast: Option<ThresholdRange>,
    aspect_ratio: Option<ThresholdRange>,
    width: Option<ThresholdRange>,
    height: Option<ThresholdRange>,
}

#[derive(Debug, Deserialize)]
struct ThresholdRange {
    min: Option<f64>,
    max: Option<f64>,
}

impl ThresholdRange {
    fn matches(&self, value: f64) -> bool {
        if let Some(min) = self.min {
            if value < min { return false; }
        }
        if let Some(max) = self.max {
            if value > max { return false; }
        }
        true
    }
}

static CLASSIFIER_RULES: std::sync::OnceLock<Vec<ClassifierRule>> = std::sync::OnceLock::new();

fn get_classifier_rules() -> &'static [ClassifierRule] {
    CLASSIFIER_RULES.get_or_init(|| {
        let json = include_str!("image_classifiers.json");
        let wrapper: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
        if let Some(rules_array) = wrapper.get("classifiers") {
            serde_json::from_value(rules_array.clone()).unwrap_or_default()
        } else {
            vec![]
        }
    })
}

/// Routing output from pixel-based analysis. **Deprecated for routing**: main flow uses
/// image_analyzer + image_recommender. Kept only as part of [ImageQualityAnalysis] for
/// dimension compatibility; use the other fields (content_type, complexity, compression_potential)
/// for quality judgment.
#[deprecated(
    since = "8.8.0",
    note = "Routing uses image_analyzer + image_recommender. Dimensions in ImageQualityAnalysis are retained for quality judgment."
)]
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

/// Pixel-based quality analysis. **Use for routing is deprecated**; main flow uses image_analyzer.
/// The returned dimensions (content_type, complexity, compression_potential, edge_density, etc.)
/// are retained for quality judgment.
#[deprecated(
    since = "8.8.0",
    note = "Main conversion routing uses image_analyzer. Use this only for quality dimension extraction (content_type, complexity, compression_potential) for quality judgment."
)]
pub fn analyze_image_quality(
    width: u32,
    height: u32,
    rgba_data: &[u8],
    file_size: u64,
    format: &str,
    frame_count: u32,
    precision: PrecisionMetadata,
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
    
    // Color diversity override: if we have exact palette size (PNG-8, GIF), use it deterministicly.
    let color_diversity = if let Some(p_size) = precision.palette_size {
        (p_size as f64 / 256.0).min(1.0)
    } else {
        calculate_color_diversity(rgba_data, width, height)
    };

    let texture_variance = calculate_texture_variance(rgba_data, width, height);
    
    // Noise level override: for deterministic lossless or high-depth formats, noise is often intentional/fine-grain.
    let noise_level = if precision.is_lossless_deterministic && (precision.bit_depth.unwrap_or(8) >= 10) {
        0.0 // True high-fidelity lossless doesn't have "compression noise"
    } else {
        calculate_noise_level(rgba_data, width, height)
    };
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
        texture_variance,
        noise_level,
        sharpness,
        contrast,
        has_alpha,
        is_animated,
        width,
        height,
    );

    let compression_potential =
        calculate_compression_potential(complexity, &content_type, has_alpha, is_animated);

    let routing_decision = make_routing_decision(
        format,
        &content_type,
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
        precision,
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
    texture_variance: f64,
    noise_level: f64,
    sharpness: f64,
    contrast: f64,
    has_alpha: bool,
    is_animated: bool,
    width: u32,
    height: u32,
) -> ImageContentType {
    let aspect_ratio = width as f64 / height.max(1) as f64;
    let rules = get_classifier_rules();
    
    // Find the matching rule with highest priority
    let mut best_rule: Option<&ClassifierRule> = None;

    for rule in rules {
        let cond = &rule.rules;
        
        if let Some(v) = cond.is_animated { if v != is_animated { continue; } }
        if let Some(v) = cond.has_alpha { if v != has_alpha { continue; } }
        
        if let Some(r) = &cond.complexity { if !r.matches(complexity) { continue; } }
        if let Some(r) = &cond.edge_density { if !r.matches(edge_density) { continue; } }
        if let Some(r) = &cond.color_diversity { if !r.matches(color_diversity) { continue; } }
        if let Some(r) = &cond.texture_variance { if !r.matches(texture_variance) { continue; } }
        if let Some(r) = &cond.noise_level { if !r.matches(noise_level) { continue; } }
        if let Some(r) = &cond.sharpness { if !r.matches(sharpness) { continue; } }
        if let Some(r) = &cond.contrast { if !r.matches(contrast) { continue; } }
        if let Some(r) = &cond.aspect_ratio { if !r.matches(aspect_ratio) { continue; } }
        if let Some(r) = &cond.width { if !r.matches(width as f64) { continue; } }
        if let Some(r) = &cond.height { if !r.matches(height as f64) { continue; } }

        if best_rule.is_none() || rule.priority > best_rule.unwrap().priority {
            best_rule = Some(rule);
        }
    }

    if let Some(rule) = best_rule {
        ImageContentType {
            name: rule.name.clone(),
            adjust: rule.adjust,
            formats: rule.formats.clone(),
            bonus: rule.bonus,
        }
    } else {
        ImageContentType {
            name: "UNKNOWN".to_string(),
            adjust: 0,
            formats: vec!["avif".to_string(), "webp".to_string(), "jxl".to_string()],
            bonus: 0.0,
        }
    }
}

fn calculate_compression_potential(
    complexity: f64,
    content_type: &ImageContentType,
    has_alpha: bool,
    is_animated: bool,
) -> f64 {
    let mut potential = 1.0 - complexity;

    potential += content_type.bonus;

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
    content_type: &ImageContentType,
    has_alpha: bool,
    is_animated: bool,
    compression_potential: f64,
    _file_size: u64,
    _pixels: u64,
) -> RoutingDecision {
    let format_lower = source_format.to_lowercase();

    // Animated modern formats should NOT be skipped here — they need to flow through
    // the animated routing pipeline (HEVC MP4 / GIF / AV1 MP4).
    let modern_lossy = ["avif", "jxl", "heic", "heif"];
    let is_modern_lossy = modern_lossy.iter().any(|f| format_lower.contains(f));

    if is_modern_lossy && !is_animated {
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
        || format_lower == "png" && has_alpha && content_type.name == "ICON";

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
        "{} content detected (complexity: {:.2}). {} recommended for {}",
        content_type.name,
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

/// Load image from path, run pixel-based quality analysis. Returns `None` if the file cannot be
/// decoded (e.g. HEIC/JXL without in-process decoder). Used for quality-dimension logging and
/// quality judgment; not used for routing.
pub fn analyze_image_quality_from_path(path: &Path) -> Option<ImageQualityAnalysis> {
    let img = open(path).ok()?;
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();
    let file_size = std::fs::metadata(path).ok()?.len();
    let format = path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "unknown".to_string());
    analyze_image_quality(width, height, rgba.as_raw(), file_size, &format, 1, PrecisionMetadata::default()).ok()
}

/// Format [ImageQualityAnalysis] as multi-line media info. **Log file only** — does not write to
/// terminal. Call when a log file is configured (e.g. alongside image conversion runs).
pub fn log_media_info_for_image_quality(analysis: &ImageQualityAnalysis, input_path: &Path) {
    if !has_log_file() {
        return;
    }
    write_to_log_at_level(Level::DEBUG, &format!("[Image quality] {}", input_path.display()));
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  size={}x{} format={} file_size={}",
            analysis.width, analysis.height, analysis.format, analysis.file_size
        ),
    );
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  content_type={} complexity={:.4} edge_density={:.4} compression_potential={:.4}",
            analysis.content_type.name,
            analysis.complexity,
            analysis.edge_density,
            analysis.compression_potential
        ),
    );
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  color_diversity={:.4} texture_variance={:.4} noise={:.4} sharpness={:.4} contrast={:.4} confidence={:.4}",
            analysis.color_diversity,
            analysis.texture_variance,
            analysis.noise_level,
            analysis.sharpness,
            analysis.contrast,
            analysis.confidence
        ),
    );
    write_to_log_at_level(Level::DEBUG, "");
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
        let result = analyze_image_quality(100, 100, &data, 10000, "png", 1, PrecisionMetadata::default()).unwrap();

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
        let result = analyze_image_quality(200, 200, &data, 50000, "png", 1, PrecisionMetadata::default()).unwrap();

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
        let result = analyze_image_quality(200, 200, &data, 50000, "png", 1, PrecisionMetadata::default()).unwrap();

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
        let result = analyze_image_quality(200, 200, &data, 100000, "jpeg", 1, PrecisionMetadata::default()).unwrap();

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
        let result_alpha = analyze_image_quality(100, 100, &data_alpha, 10000, "png", 1, PrecisionMetadata::default()).unwrap();
        assert!(result_alpha.has_alpha, "Should detect alpha usage");

        let data_no_alpha = create_solid_color(100, 100, 128, 128, 128, 255);
        let result_no_alpha =
            analyze_image_quality(100, 100, &data_no_alpha, 10000, "png", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            !result_no_alpha.has_alpha,
            "Should not detect alpha when all 255"
        );
    }

    #[test]
    fn test_animation_detection() {
        let data = create_solid_color(100, 100, 128, 128, 128, 255);

        let static_result = analyze_image_quality(100, 100, &data, 10000, "png", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            !static_result.is_animated,
            "frame_count=1 should not be animated"
        );
        assert_ne!(static_result.content_type, ImageContentType::Animation);

        let animated_result = analyze_image_quality(100, 100, &data, 50000, "gif", 10, PrecisionMetadata::default()).unwrap();
        assert!(
            animated_result.is_animated,
            "frame_count=10 should be animated"
        );
        assert_eq!(animated_result.content_type, ImageContentType::Animation);
    }

    #[test]
    fn test_classify_icon() {
        let data = create_solid_color(64, 64, 100, 150, 200, 200);
        let result = analyze_image_quality(64, 64, &data, 5000, "png", 1, PrecisionMetadata::default()).unwrap();

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

        let result = analyze_image_quality(1920, 1080, &data, 500000, "png", 1, PrecisionMetadata::default()).unwrap();

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
        let result = analyze_image_quality(1920, 1080, &data, 2000000, "jpeg", 1, PrecisionMetadata::default()).unwrap();

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
        let result = analyze_image_quality(800, 600, &data, 100000, "png", 1, PrecisionMetadata::default()).unwrap();

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

        let avif_result = analyze_image_quality(500, 500, &data, 50000, "avif", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            avif_result.routing_decision.should_skip,
            "AVIF should be skipped"
        );

        let jxl_result = analyze_image_quality(500, 500, &data, 50000, "jxl", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            jxl_result.routing_decision.should_skip,
            "JXL should be skipped"
        );

        let heic_result = analyze_image_quality(500, 500, &data, 50000, "heic", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            heic_result.routing_decision.should_skip,
            "HEIC should be skipped"
        );

        let png_result = analyze_image_quality(500, 500, &data, 50000, "png", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            !png_result.routing_decision.should_skip,
            "PNG should not be skipped"
        );

        let jpeg_result = analyze_image_quality(500, 500, &data, 50000, "jpeg", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            !jpeg_result.routing_decision.should_skip,
            "JPEG should not be skipped"
        );
    }

    #[test]
    fn test_format_recommendations_by_content() {
        let photo_data = create_noisy(1000, 1000, 11111);
        let photo_result =
            analyze_image_quality(1000, 1000, &photo_data, 500000, "jpeg", 1, PrecisionMetadata::default()).unwrap();

        let photo_formats = photo_result.content_type.recommended_formats();
        assert!(
            photo_formats.contains(&"avif") || photo_formats.contains(&"jxl"),
            "Photo should recommend AVIF or JXL"
        );

        let anim_data = create_gradient(500, 500);
        let anim_result = analyze_image_quality(500, 500, &anim_data, 100000, "gif", 5, PrecisionMetadata::default()).unwrap();

        let anim_formats = anim_result.content_type.recommended_formats();
        assert!(
            anim_formats.contains(&"webp"),
            "Animation should recommend WebP"
        );
    }

    #[test]
    fn test_strict_solid_complexity() {
        let data = create_solid_color(500, 500, 100, 100, 100, 255);
        let result = analyze_image_quality(500, 500, &data, 10000, "png", 1, PrecisionMetadata::default()).unwrap();

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
        let result = analyze_image_quality(500, 500, &data, 100000, "png", 1, PrecisionMetadata::default()).unwrap();

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
        let result = analyze_image_quality(500, 500, &data, 50000, "png", 1, PrecisionMetadata::default()).unwrap();

        assert!(
            result.edge_density > 0.1,
            "STRICT: Checkerboard edge density must be > 0.1, got {}",
            result.edge_density
        );
    }

    #[test]
    fn test_strict_compression_potential() {
        let simple = create_solid_color(500, 500, 200, 200, 200, 255);
        let simple_result = analyze_image_quality(500, 500, &simple, 10000, "png", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            simple_result.compression_potential > 0.7,
            "STRICT: Simple content compression potential must be > 0.7, got {}",
            simple_result.compression_potential
        );

        let complex = create_noisy(500, 500, 77777);
        let complex_result = analyze_image_quality(500, 500, &complex, 100000, "jpeg", 1, PrecisionMetadata::default()).unwrap();
        assert!(
            complex_result.compression_potential < 0.5,
            "STRICT: Complex content compression potential must be < 0.5, got {}",
            complex_result.compression_potential
        );
    }

    #[test]
    fn test_edge_minimum_size() {
        let data = create_solid_color(10, 10, 128, 128, 128, 255);
        let result = analyze_image_quality(10, 10, &data, 400, "png", 1, PrecisionMetadata::default());
        assert!(result.is_ok(), "Should handle minimum size images");
    }

    #[test]
    fn test_edge_large_image() {
        let data = create_gradient(3840, 2160);
        let result = analyze_image_quality(3840, 2160, &data, 5000000, "png", 1, PrecisionMetadata::default()).unwrap();

        assert!(
            result.confidence > 0.7,
            "Large image should have high confidence"
        );
    }

    #[test]
    fn test_edge_invalid_dimensions() {
        let data = vec![0u8; 100];
        let result = analyze_image_quality(0, 100, &data, 100, "png", 1, PrecisionMetadata::default());
        assert!(result.is_err(), "Should fail on zero width");

        let result2 = analyze_image_quality(100, 0, &data, 100, "png", 1, PrecisionMetadata::default());
        assert!(result2.is_err(), "Should fail on zero height");
    }

    #[test]
    fn test_edge_insufficient_data() {
        let data = vec![0u8; 100];
        let result = analyze_image_quality(100, 100, &data, 100, "png", 1, PrecisionMetadata::default());
        assert!(result.is_err(), "Should fail on insufficient data");
    }

    #[test]
    fn test_metric_edge_density_isolation() {
        let high_edges = create_checkerboard(200, 200, 5);
        let low_edges = create_solid_color(200, 200, 128, 128, 128, 255);

        let high_result = analyze_image_quality(200, 200, &high_edges, 50000, "png", 1, PrecisionMetadata::default()).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_edges, 50000, "png", 1, PrecisionMetadata::default()).unwrap();

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

        let high_result = analyze_image_quality(200, 200, &high_div, 50000, "png", 1, PrecisionMetadata::default()).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_div, 50000, "png", 1, PrecisionMetadata::default()).unwrap();

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

        let high_result = analyze_image_quality(200, 200, &high_noise, 50000, "png", 1, PrecisionMetadata::default()).unwrap();
        let low_result = analyze_image_quality(200, 200, &low_noise, 50000, "png", 1, PrecisionMetadata::default()).unwrap();

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

        let sharp_result = analyze_image_quality(200, 200, &sharp, 50000, "png", 1, PrecisionMetadata::default()).unwrap();
        let smooth_result = analyze_image_quality(200, 200, &smooth, 50000, "png", 1, PrecisionMetadata::default()).unwrap();

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

        let result1 = analyze_image_quality(300, 300, &data, 50000, "png", 1, PrecisionMetadata::default()).unwrap();
        let result2 = analyze_image_quality(300, 300, &data, 50000, "png", 1, PrecisionMetadata::default()).unwrap();

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

        let small_result = analyze_image_quality(100, 100, &small, 10000, "png", 1, PrecisionMetadata::default()).unwrap();
        let large_result = analyze_image_quality(400, 400, &large, 160000, "png", 1, PrecisionMetadata::default()).unwrap();

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
