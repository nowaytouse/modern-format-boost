//! 🔬 Image Quality Detector - Content Classification & Media Metrics
//!
//! This module provides **pixel-based image classification** and quality dimensions.
//! It is used to generate UI labels (e.g., PHOTO, SCREENSHOT) and detailed quality metrics
//! for logging. Routing and compression decisions are handled by image_analyzer/recommender.
//!
//! ## Functions
//! - **Image Content Classification**: Categorizes images into logical types (Icon, Photo, etc.)
//! - **Quality Metrics**: Extracts complexity, edge density, color diversity, and more.
//! - **Media Information**: Provides a formatted summary of image characteristics.

use crate::image_detection::PrecisionMetadata;
use crate::progress_mode::{has_log_file, write_to_log_at_level};
use image::{open, GenericImageView};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::Level;

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
    pub confidence: f64,
    pub precision: PrecisionMetadata,

    /// Processing history for cache invalidation logic
    pub history: crate::types::ProcessHistory,

    /// Visual perception data (Auxiliary analysis)
    pub perception: crate::types::VisualPerception,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ImageContentType {
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct ClassifierRule {
    name: String,
    priority: i32,
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
            if value < min {
                return false;
            }
        }
        if let Some(max) = self.max {
            if value > max {
                return false;
            }
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

/// Pixel-based quality analysis.
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

    let color_diversity = if let Some(p_size) = precision.palette_size {
        (p_size as f64 / 256.0).min(1.0)
    } else {
        calculate_color_diversity(rgba_data, width, height)
    };

    let texture_variance = calculate_texture_variance(rgba_data, width, height);

    let noise_level =
        if precision.is_lossless_deterministic && (precision.bit_depth.unwrap_or(8) >= 10) {
            0.0
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
        confidence,
        precision,
        history: crate::common_utils::get_current_history(),
        perception: Default::default(),
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
    (edge_density * 0.35 + color_diversity * 0.25 + texture_variance * 0.25 + noise_level * 0.15)
        .clamp(0.0, 1.0)
}

#[allow(clippy::too_many_arguments)]
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

    let mut best_rule: Option<&ClassifierRule> = None;

    for rule in rules {
        let cond = &rule.rules;

        if let Some(v) = cond.is_animated {
            if v != is_animated {
                continue;
            }
        }
        if let Some(v) = cond.has_alpha {
            if v != has_alpha {
                continue;
            }
        }

        if let Some(r) = &cond.complexity {
            if !r.matches(complexity) {
                continue;
            }
        }
        if let Some(r) = &cond.edge_density {
            if !r.matches(edge_density) {
                continue;
            }
        }
        if let Some(r) = &cond.color_diversity {
            if !r.matches(color_diversity) {
                continue;
            }
        }
        if let Some(r) = &cond.texture_variance {
            if !r.matches(texture_variance) {
                continue;
            }
        }
        if let Some(r) = &cond.noise_level {
            if !r.matches(noise_level) {
                continue;
            }
        }
        if let Some(r) = &cond.sharpness {
            if !r.matches(sharpness) {
                continue;
            }
        }
        if let Some(r) = &cond.contrast {
            if !r.matches(contrast) {
                continue;
            }
        }
        if let Some(r) = &cond.aspect_ratio {
            if !r.matches(aspect_ratio) {
                continue;
            }
        }
        if let Some(r) = &cond.width {
            if !r.matches(width as f64) {
                continue;
            }
        }
        if let Some(r) = &cond.height {
            if !r.matches(height as f64) {
                continue;
            }
        }

        if best_rule.is_none() || rule.priority > best_rule.unwrap().priority {
            best_rule = Some(rule);
        }
    }

    if let Some(rule) = best_rule {
        ImageContentType {
            name: rule.name.clone(),
        }
    } else {
        ImageContentType {
            name: "UNKNOWN".to_string(),
        }
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

pub fn analyze_image_quality_from_path(path: &Path) -> Option<ImageQualityAnalysis> {
    analyze_image_quality_with_cache(path, None)
}

pub fn analyze_image_quality_with_cache(
    path: &Path,
    cache: Option<&crate::analysis_cache::AnalysisCache>,
) -> Option<ImageQualityAnalysis> {
    let is_jpeg_hint = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .map(|e| e == "jpg" || e == "jpeg")
        .unwrap_or(false);

    if is_jpeg_hint {
        return None;
    }

    if let Some(cache) = cache {
        match cache.get_quality_analysis(path) {
            Ok(Some(cached)) => return Some(cached),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "Failed to load cached image quality analysis"
                );
            }
        }
    }

    let analysis = analyze_image_quality_from_path_internal(path)?;
    if let Some(cache) = cache {
        if let Err(err) = cache.store_quality_analysis(path, &analysis) {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "Failed to store image quality analysis in cache"
            );
        }
    }
    Some(analysis)
}

fn analyze_image_quality_from_path_internal(path: &Path) -> Option<ImageQualityAnalysis> {
    let img = open(path).ok()?;
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();
    let file_size = std::fs::metadata(path).ok()?.len();
    let format = path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "unknown".to_string());
    analyze_image_quality(
        width,
        height,
        rgba.as_raw(),
        file_size,
        &format,
        1,
        PrecisionMetadata::default(),
    )
    .ok()
}

pub fn log_media_info_for_image_quality(analysis: &ImageQualityAnalysis, input_path: &Path) {
    if !has_log_file() {
        return;
    }
    write_to_log_at_level(
        Level::DEBUG,
        &format!("[Image quality] {}", input_path.display()),
    );
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
            "  content_type={} complexity={:.4} edge_density={:.4}",
            analysis.content_type.name, analysis.complexity, analysis.edge_density
        ),
    );
    write_to_log_at_level(Level::DEBUG, &format!("  color_diversity={:.4} texture_variance={:.4} noise={:.4} sharpness={:.4} contrast={:.4} confidence={:.4}", analysis.color_diversity, analysis.texture_variance, analysis.noise_level, analysis.sharpness, analysis.contrast, analysis.confidence));
    write_to_log_at_level(Level::DEBUG, "");
}
