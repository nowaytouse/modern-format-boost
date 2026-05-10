//! 🔬 Image Quality Detector - Content Classification & Media Metrics
//!
//! This module provides **pixel-based image classification** and quality dimensions.
//! It is used to generate UI labels (e.g., PHOTO, SCREENSHOT) and detailed quality metrics
//! for logging. Routing and compression decisions are handled by `image_analyzer/recommender`.
//!
//! ## Functions
//! - **Image Content Classification**: Categorizes images into logical types (Icon, Photo, etc.)
//! - **Quality Metrics**: Extracts complexity, edge density, color diversity, and more.
//! - **Media Information**: Provides a formatted summary of image characteristics.

use crate::image_detection::PrecisionMetadata;
use crate::progress_mode::{has_log_file, write_to_log_at_level};
use image::{GenericImageView, open};
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
    pub frame_count: Option<u32>,

    pub complexity: Option<f64>,
    pub edge_density: Option<f64>,
    pub color_diversity: Option<f64>,
    pub texture_variance: Option<f64>,
    pub noise_level: Option<f64>,
    pub sharpness: Option<f64>,
    pub contrast: Option<f64>,

    pub content_type: ImageContentType,
    pub confidence: Option<f64>,
    pub precision: PrecisionMetadata,

    /// Processing history for cache invalidation logic
    pub history: crate::types::ProcessHistory,

    /// Visual perception data (Auxiliary analysis)
    pub perception: crate::types::Visual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ImageContentType {
    pub name: String,
}

#[derive(Debug, Clone)]
struct ClassifierInput {
    pub complexity: Option<f64>,
    pub edge_density: Option<f64>,
    pub color_diversity: Option<f64>,
    pub texture_variance: Option<f64>,
    pub noise_level: Option<f64>,
    pub sharpness: Option<f64>,
    pub contrast: Option<f64>,
    pub has_alpha: bool,
    pub is_animated: bool,
    pub width: u32,
    pub height: u32,
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
    fn matches(&self, value_opt: Option<f64>) -> bool {
        let Some(value) = value_opt else {
            // If value is missing, it only matches if no constraints are set
            return self.min.is_none() && self.max.is_none();
        };

        if let Some(min) = self.min
            && value < min
        {
            return false;
        }
        if let Some(max) = self.max
            && value > max
        {
            return false;
        }
        true
    }
}

static CLASSIFIER_RULES: std::sync::OnceLock<Vec<ClassifierRule>> = std::sync::OnceLock::new();

/// Gets the classifier rules for image quality detection.
///
/// Loads and parses the embedded `image_classifiers.json` file
/// to provide classification rules for different image types.
///
/// # Returns
/// Static slice of classifier rules
fn get_classifier_rules() -> &'static [ClassifierRule] {
    CLASSIFIER_RULES.get_or_init(|| {
        let json = include_str!("image_classifiers.json");
        let wrapper: serde_json::Value =
            serde_json::from_str(json).expect("embedded image_classifiers.json is malformed");
        wrapper
            .get("classifiers")
            .map_or_else(Vec::new, |rules_array| {
                serde_json::from_value(rules_array.clone())
                    .expect("embedded image_classifiers.json 'classifiers' array is malformed")
            })
    })
}

/// Pixel-based quality analysis.
///
/// # Errors
/// Returns an error if analysis fails.
pub fn analyze_image_quality(
    width: u32,
    height: u32,
    rgba_data: &[u8],
    file_size: u64,
    format: &str,
    frame_count: Option<u32>,
    precision: PrecisionMetadata,
) -> Result<ImageQualityAnalysis, String> {
    let expected_size = crate::numeric_cast::u32_to_usize_sat(width)
        * crate::numeric_cast::u32_to_usize_sat(height)
        * 4;
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

    let pixels = u64::from(width) * u64::from(height);

    let edge_density = calculate_edge_density(rgba_data, width, height);

    let color_diversity = precision.palette_size.map_or_else(
        || calculate_color_diversity(rgba_data, width, height),
        |p_size| {
            Some(
                (crate::numeric_cast::usize_to_f64(p_size)
                    / crate::constants::PALETTE_MAX_DENSITY_F64)
                    .min(1.0),
            )
        },
    );

    let texture_variance = calculate_texture_variance(rgba_data, width, height);

    let noise_level = if precision.is_lossless_deterministic
        && precision
            .bit_depth
            .is_some_and(|bd| bd >= crate::constants::HDR_BIT_DEPTH_THRESHOLD)
    {
        Some(0.0_f64)
    } else {
        calculate_noise_level(rgba_data, width, height)
    };
    let sharpness = calculate_sharpness(rgba_data, width, height);
    let contrast = calculate_contrast(rgba_data, width, height);
    let has_alpha = detect_alpha_usage(rgba_data);

    let complexity =
        calculate_overall_complexity(edge_density, color_diversity, texture_variance, noise_level);

    let is_animated = frame_count.is_some_and(|n| n > 1);
    let content_type = classify_content_type(&ClassifierInput {
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
    });

    let confidence = Some(calculate_analysis_confidence(
        pixels,
        file_size,
        edge_density,
        color_diversity,
    ));

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
        perception: crate::types::Visual::default(),
    })
}

/// Calculates the edge density of an image.
///
/// Analyzes the image to detect edges and calculates the density
/// of edge pixels as a measure of image complexity and detail.
///
/// # Arguments
/// * `rgba` - RGBA image data as bytes
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
///
/// # Returns
/// Edge density value (0.0 to 1.0)
fn calculate_edge_density(rgba: &[u8], width: u32, height: u32) -> Option<f64> {
    if width < 3 || height < 3 {
        return None;
    }

    let pixels = crate::numeric_cast::u32_to_usize_sat(width)
        * crate::numeric_cast::u32_to_usize_sat(height);
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_SAMPLING_PIXELS_ULTRA_LARGE
    {
        crate::constants::IMAGE_SAMPLING_STEP_ULTRA_LARGE
    } else if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::IMAGE_SAMPLING_STEP_LARGE
    } else {
        crate::constants::IMAGE_SAMPLING_STEP_NORMAL
    };

    let mut edge_count = 0usize;
    let mut sample_count = 0usize;

    let w = crate::numeric_cast::u32_to_usize_sat(width);

    for y in (1..crate::numeric_cast::u32_to_usize_sat(height.saturating_sub(1))).step_by(step) {
        for x in (1..crate::numeric_cast::u32_to_usize_sat(width.saturating_sub(1))).step_by(step) {
            let get_gray = |px: usize, py: usize| -> i32 {
                let idx = (py * w + px) * 4;
                let r = i32::from(rgba[idx]);
                let g = i32::from(rgba[idx + 1]);
                let b = i32::from(rgba[idx + 2]);
                (r * crate::constants::LUMA_COEFF_R
                    + g * crate::constants::LUMA_COEFF_G
                    + b * crate::constants::LUMA_COEFF_B)
                    / crate::constants::LUMA_DIVISOR
            };

            let gx = get_gray(x + 1, y) - get_gray(x - 1, y);
            let gy = get_gray(x, y + 1) - get_gray(x, y - 1);
            let gradient = f64::from(gx * gx + gy * gy).sqrt();

            if gradient > crate::constants::IMAGE_EDGE_DENSITY_THRESHOLD {
                edge_count += 1;
            }
            sample_count += 1;
        }
    }

    if sample_count == 0 {
        return None;
    }

    let raw_density = crate::numeric_cast::usize_to_f64(edge_count)
        / crate::numeric_cast::usize_to_f64(sample_count);
    Some((raw_density * crate::constants::IMAGE_EDGE_DENSITY_MULTIPLIER).min(1.0))
}

/// Calculates the color diversity of an image.
///
/// Analyzes the image to measure the variety and distribution of colors.
/// Higher diversity indicates more complex and potentially higher quality images.
///
/// # Arguments
/// * `rgba` - RGBA image data as bytes
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
///
/// # Returns
/// Color diversity value (0.0 to 1.0)
fn calculate_color_diversity(rgba: &[u8], width: u32, height: u32) -> Option<f64> {
    use std::collections::HashSet;

    let pixels = crate::numeric_cast::u32_to_usize_sat(width)
        * crate::numeric_cast::u32_to_usize_sat(height);
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::COLOR_DIVERSITY_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_sat(crate::constants::IMAGE_SIZE_THRESHOLD_LARGE)
    {
        crate::constants::COLOR_DIVERSITY_STEP_MEDIUM
    } else {
        crate::constants::COLOR_DIVERSITY_STEP_NORMAL
    };

    let quantize_step = crate::constants::COLOR_DIVERSITY_QUANTIZE_STEP;
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
        return None;
    }

    let max_colors = crate::numeric_cast::usize_to_f64(
        sample_count.min(crate::constants::COLOR_DIVERSITY_MAX_SAMPLES),
    );
    Some((crate::numeric_cast::usize_to_f64(colors.len()) / max_colors).min(1.0))
}

/// Calculates the texture variance of an image.
///
/// Analyzes local pixel variations to measure texture complexity.
/// Higher variance indicates more detailed and textured images.
///
/// # Arguments
/// * `rgba` - RGBA image data as bytes
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
///
/// # Returns
/// Texture variance value (0.0 to 1.0)
fn calculate_texture_variance(rgba: &[u8], width: u32, height: u32) -> Option<f64> {
    if width < 3 || height < 3 {
        return None;
    }

    let pixels = crate::numeric_cast::u32_to_usize_sat(width)
        * crate::numeric_cast::u32_to_usize_sat(height);
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::TEXTURE_VARIANCE_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_sat(crate::constants::IMAGE_SIZE_THRESHOLD_LARGE)
    {
        crate::constants::TEXTURE_VARIANCE_STEP_MEDIUM
    } else {
        crate::constants::TEXTURE_VARIANCE_STEP_NORMAL
    };

    let mut variance_sum = 0.0_f64;
    let mut sample_count = 0usize;

    for y in (1..crate::numeric_cast::u32_to_usize_sat(height.saturating_sub(1))).step_by(step) {
        for x in (1..crate::numeric_cast::u32_to_usize_sat(width.saturating_sub(1))).step_by(step) {
            let mut sum = 0i32;
            let mut sq_sum = 0i64;

            for dy in -1i32..=1_i32 {
                for dx in -1i32..=1_i32 {
                    let px = crate::numeric_cast::i32_to_usize_sat(
                        crate::numeric_cast::usize_to_i32_sat(x) + dx,
                    );
                    let py = crate::numeric_cast::i32_to_usize_sat(
                        crate::numeric_cast::usize_to_i32_sat(y) + dy,
                    );
                    let idx = (py * crate::numeric_cast::u32_to_usize_sat(width) + px) * 4;

                    let gray = (i32::from(rgba[idx]) * crate::constants::LUMA_COEFF_R
                        + i32::from(rgba[idx + 1]) * crate::constants::LUMA_COEFF_G
                        + i32::from(rgba[idx + 2]) * crate::constants::LUMA_COEFF_B)
                        / crate::constants::LUMA_DIVISOR;
                    sum += gray;
                    sq_sum += i64::from(gray) * i64::from(gray);
                }
            }

            let mean = f64::from(sum) / crate::constants::KERNEL_SIZE_3X3;
            let variance = mean.mul_add(
                -mean,
                crate::numeric_cast::i64_to_f64(sq_sum) / crate::constants::KERNEL_SIZE_3X3,
            );
            variance_sum += variance.sqrt();
            sample_count += 1;
        }
    }

    if sample_count == 0 {
        return None;
    }

    let avg_std = variance_sum / crate::numeric_cast::usize_to_f64(sample_count);
    Some((avg_std / crate::constants::IMAGE_TEXTURE_VAR_NORMALIZATION).min(1.0))
}

/// Calculates the noise level of an image.
///
/// Analyzes pixel variations to estimate the amount of noise present.
/// Lower noise levels generally indicate higher image quality.
///
/// # Arguments
/// * `rgba` - RGBA image data as bytes
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
///
/// # Returns
/// Noise level value (0.0 to 1.0, lower is better)
fn calculate_noise_level(rgba: &[u8], width: u32, height: u32) -> Option<f64> {
    if width < 2 || height < 2 {
        return None;
    }

    let pixels = crate::numeric_cast::u32_to_usize_sat(width)
        * crate::numeric_cast::u32_to_usize_sat(height);
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::NOISE_LEVEL_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_sat(crate::constants::IMAGE_SIZE_THRESHOLD_LARGE)
    {
        crate::constants::NOISE_LEVEL_STEP_MEDIUM
    } else {
        crate::constants::IMAGE_SAMPLING_STEP_NORMAL
    };

    let mut diff_sum = 0.0_f64;
    let mut sample_count = 0usize;

    for y in (0..crate::numeric_cast::u32_to_usize_sat(height.saturating_sub(1))).step_by(step) {
        for x in (0..crate::numeric_cast::u32_to_usize_sat(width.saturating_sub(1))).step_by(step) {
            let idx = (y * crate::numeric_cast::u32_to_usize_sat(width) + x) * 4;
            let idx_right = idx + 4;
            let idx_down = idx + (crate::numeric_cast::u32_to_usize_sat(width) * 4);

            if idx_down + 2 < rgba.len() {
                let curr =
                    (i32::from(rgba[idx]) + i32::from(rgba[idx + 1]) + i32::from(rgba[idx + 2]))
                        / 3_i32;
                let right = (i32::from(rgba[idx_right])
                    + i32::from(rgba[idx_right + 1])
                    + i32::from(rgba[idx_right + 2]))
                    / 3_i32;
                let down = (i32::from(rgba[idx_down])
                    + i32::from(rgba[idx_down + 1])
                    + i32::from(rgba[idx_down + 2]))
                    / 3_i32;

                diff_sum += f64::from((curr - right).abs());
                diff_sum += f64::from((curr - down).abs());
                sample_count += 2;
            }
        }
    }

    if sample_count == 0 {
        return None;
    }

    let avg_diff = diff_sum / crate::numeric_cast::usize_to_f64(sample_count);
    Some((avg_diff / crate::constants::IMAGE_NOISE_NORMALIZATION).min(1.0))
}

/// Calculates the sharpness of an image.
///
/// Analyzes edge strength and focus to estimate image sharpness.
/// Higher sharpness values indicate clearer, more focused images.
///
/// # Arguments
/// * `rgba` - RGBA image data as bytes
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
///
/// # Returns
/// Sharpness value (0.0 to 1.0, higher is better)
fn calculate_sharpness(rgba: &[u8], width: u32, height: u32) -> Option<f64> {
    if width < 3 || height < 3 {
        return None;
    }

    let pixels = crate::numeric_cast::u32_to_usize_sat(width)
        * crate::numeric_cast::u32_to_usize_sat(height);
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::SHARPNESS_SAMPLING_PIXELS_LARGE
    {
        crate::constants::SHARPNESS_SAMPLING_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_sat(crate::constants::IMAGE_SIZE_THRESHOLD_LARGE)
    {
        crate::constants::SHARPNESS_SAMPLING_STEP_MEDIUM
    } else {
        crate::constants::SHARPNESS_SAMPLING_STEP_NORMAL
    };

    let mut laplacian_sum = 0.0_f64;
    let mut sample_count = 0usize;

    let get_gray = |x: usize, y: usize| -> i32 {
        let idx = (y * crate::numeric_cast::u32_to_usize_sat(width) + x) * 4;
        (i32::from(rgba[idx]) * crate::constants::LUMA_COEFF_R
            + i32::from(rgba[idx + 1]) * crate::constants::LUMA_COEFF_G
            + i32::from(rgba[idx + 2]) * crate::constants::LUMA_COEFF_B)
            / crate::constants::LUMA_DIVISOR
    };

    for y in (1..crate::numeric_cast::u32_to_usize_sat(height.saturating_sub(1))).step_by(step) {
        for x in (1..crate::numeric_cast::u32_to_usize_sat(width.saturating_sub(1))).step_by(step) {
            let center = get_gray(x, y);
            let top = get_gray(x, y - 1);
            let bottom = get_gray(x, y + 1);
            let left = get_gray(x - 1, y);
            let right = get_gray(x + 1, y);

            let laplacian =
                (crate::constants::IMAGE_LAPLACIAN_CENTER * center - top - bottom - left - right)
                    .abs();
            laplacian_sum += f64::from(laplacian);
            sample_count += 1;
        }
    }

    if sample_count == 0 {
        return None;
    }

    let avg_laplacian = laplacian_sum / crate::numeric_cast::usize_to_f64(sample_count);
    Some((avg_laplacian / crate::constants::IMAGE_SHARPNESS_NORMALIZATION).min(1.0))
}

/// Calculates the contrast of an image.
///
/// Analyzes the distribution of pixel intensities to estimate contrast.
/// Higher contrast values indicate better dynamic range and image quality.
///
/// # Arguments
/// * `rgba` - RGBA image data as bytes
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
///
/// # Returns
/// Contrast value (0.0 to 1.0, higher is better)
fn calculate_contrast(rgba: &[u8], width: u32, height: u32) -> Option<f64> {
    let pixels = crate::numeric_cast::u32_to_usize_sat(width)
        * crate::numeric_cast::u32_to_usize_sat(height);
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::CONTRAST_SAMPLING_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_sat(crate::constants::IMAGE_SIZE_THRESHOLD_LARGE)
    {
        crate::constants::CONTRAST_SAMPLING_STEP_MEDIUM
    } else {
        crate::constants::CONTRAST_SAMPLING_STEP_NORMAL
    };

    let mut sum = 0u64;
    let mut sq_sum = 0u64;
    let mut sample_count = 0usize;

    for i in (0..pixels).step_by(step) {
        let idx = i * 4;
        if idx + 2 < rgba.len() {
            let gray = (u64::from(rgba[idx])
                * crate::numeric_cast::i32_to_u64_sat(crate::constants::LUMA_COEFF_R)
                + u64::from(rgba[idx + 1])
                    * crate::numeric_cast::i32_to_u64_sat(crate::constants::LUMA_COEFF_G)
                + u64::from(rgba[idx + 2])
                    * crate::numeric_cast::i32_to_u64_sat(crate::constants::LUMA_COEFF_B))
                / crate::numeric_cast::i32_to_u64_sat(crate::constants::LUMA_DIVISOR);
            sum += gray;
            sq_sum += gray * gray;
            sample_count += 1;
        }
    }

    if sample_count == 0 {
        return None;
    }

    let mean =
        crate::numeric_cast::u64_to_f64(sum) / crate::numeric_cast::usize_to_f64(sample_count);
    let mean_sq =
        crate::numeric_cast::u64_to_f64(sq_sum) / crate::numeric_cast::usize_to_f64(sample_count);
    let variance = mean.mul_add(-mean, mean_sq);
    Some((variance.sqrt() / crate::constants::IMAGE_CONTRAST_NORMALIZATION).min(1.0))
}

/// Detects if an image uses the alpha channel.
///
/// Samples the alpha channel to determine if the image has
/// any transparency (alpha values less than 255).
///
/// # Arguments
/// * `rgba` - RGBA image data as bytes
///
/// # Returns
/// `true` if alpha channel is used, `false` otherwise
fn detect_alpha_usage(rgba: &[u8]) -> bool {
    for i in (0..rgba.len()).step_by(crate::constants::IMAGE_ALPHA_SAMPLING_STEP) {
        let alpha_idx = i + crate::constants::RGBA_ALPHA_OFFSET;
        if alpha_idx < rgba.len() && rgba[alpha_idx] < crate::constants::ALPHA_OPAQUE {
            return true;
        }
    }
    false
}

/// Calculates the overall image complexity from individual metrics.
///
/// Combines edge density, color diversity, texture variance, and noise level
/// using weighted averages to produce a single complexity score.
///
/// # Arguments
/// * `edge_density` - Edge density metric (0.0 to 1.0)
/// * `color_diversity` - Color diversity metric (0.0 to 1.0)
/// * `texture_variance` - Texture variance metric (0.0 to 1.0)
/// * `noise_level` - Noise level metric (0.0 to 1.0)
///
/// # Returns
/// Overall complexity score (0.0 to 1.0)
pub(crate) fn calculate_overall_complexity(
    edge_density: Option<f64>,
    color_diversity: Option<f64>,
    texture_variance: Option<f64>,
    noise_level: Option<f64>,
) -> Option<f64> {
    let edge = edge_density?;
    let color = color_diversity?;
    let texture = texture_variance?;
    let noise = noise_level?;

    debug_assert!(
        (crate::constants::IMAGE_COMPLEXITY_WEIGHT_NOISE
            + crate::constants::IMAGE_COMPLEXITY_WEIGHT_TEXTURE
            + crate::constants::IMAGE_COMPLEXITY_WEIGHT_EDGE
            + crate::constants::IMAGE_COMPLEXITY_WEIGHT_COLOR
            - 1.0)
            .abs()
            < 1e-6_f64,
        "Image complexity weights must sum to 1.0"
    );

    Some(
        noise
            .mul_add(
                crate::constants::IMAGE_COMPLEXITY_WEIGHT_NOISE,
                texture.mul_add(
                    crate::constants::IMAGE_COMPLEXITY_WEIGHT_TEXTURE,
                    edge.mul_add(
                        crate::constants::IMAGE_COMPLEXITY_WEIGHT_EDGE,
                        color * crate::constants::IMAGE_COMPLEXITY_WEIGHT_COLOR,
                    ),
                ),
            )
            .clamp(0.0, 1.0),
    )
}

/// Classifies the content type of an image based on quality metrics.
///
/// Uses a rule-based classifier system to determine if the image is
/// likely to be a photo, screenshot, graphic, or other content type.
///
/// # Arguments
/// * `input` - Classifier input with all image quality metrics
///
/// # Returns
/// Classified image content type
fn classify_content_type(input: &ClassifierInput) -> ImageContentType {
    let &ClassifierInput {
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
    } = input;
    let aspect_ratio = f64::from(width) / f64::from(height.max(1));
    let rules = get_classifier_rules();

    let mut best_rule: Option<&ClassifierRule> = None;

    for rule in rules {
        let cond = &rule.rules;

        if let Some(v) = cond.is_animated
            && v != is_animated
        {
            continue;
        }
        if let Some(v) = cond.has_alpha
            && v != has_alpha
        {
            continue;
        }

        if let Some(r) = &cond.complexity
            && !r.matches(complexity)
        {
            continue;
        }
        if let Some(r) = &cond.edge_density
            && !r.matches(edge_density)
        {
            continue;
        }
        if let Some(r) = &cond.color_diversity
            && !r.matches(color_diversity)
        {
            continue;
        }
        if let Some(r) = &cond.texture_variance
            && !r.matches(texture_variance)
        {
            continue;
        }
        if let Some(r) = &cond.noise_level
            && !r.matches(noise_level)
        {
            continue;
        }
        if let Some(r) = &cond.sharpness
            && !r.matches(sharpness)
        {
            continue;
        }
        if let Some(r) = &cond.contrast
            && !r.matches(contrast)
        {
            continue;
        }
        if let Some(r) = &cond.aspect_ratio
            && !r.matches(Some(aspect_ratio))
        {
            continue;
        }
        if let Some(r) = &cond.width
            && !r.matches(Some(f64::from(width)))
        {
            continue;
        }
        if let Some(r) = &cond.height
            && !r.matches(Some(f64::from(height)))
        {
            continue;
        }

        if best_rule.is_none_or(|best| rule.priority > best.priority) {
            best_rule = Some(rule);
        }
    }

    best_rule.map_or_else(
        || ImageContentType {
            name: "UNKNOWN".to_string(),
        },
        |rule| ImageContentType {
            name: rule.name.clone(),
        },
    )
}

/// Calculates the confidence level for image quality analysis.
///
/// Determines how reliable the quality metrics are based on image size,
/// file size, and metric consistency. Larger images with consistent
/// metrics get higher confidence scores.
///
/// # Arguments
/// * `pixels` - Total number of pixels in the image
/// * `file_size` - File size in bytes
/// * `edge_density` - Edge density metric
/// * `color_diversity` - Color diversity metric
///
/// # Returns
/// Confidence level (0.0 to 1.0)
pub(crate) fn calculate_analysis_confidence(
    pixels: u64,
    file_size: u64,
    edge_density: Option<f64>,
    color_diversity: Option<f64>,
) -> f64 {
    let mut confidence: f64 = crate::constants::IMAGE_CONFIDENCE_BASE;

    if pixels > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD {
        confidence += crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_BONUS;
    } else if pixels < crate::constants::IMAGE_CONFIDENCE_PIXELS_SMALL_THRESHOLD {
        confidence -= crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_BONUS;
    }
    if file_size > crate::constants::IMAGE_CONFIDENCE_SIZE_MIN
        && file_size < crate::constants::IMAGE_CONFIDENCE_SIZE_MAX
    {
        confidence += crate::constants::IMAGE_CONFIDENCE_INCREMENT;
    }

    if let Some(edge) = edge_density {
        if edge > crate::constants::IMAGE_CONFIDENCE_MIN_EDGE_DENSITY
            && edge < crate::constants::IMAGE_CONFIDENCE_MAX_EDGE_DENSITY
        {
            confidence += crate::constants::IMAGE_CONFIDENCE_INCREMENT;
        }
    } else {
        confidence -= crate::constants::IMAGE_CONFIDENCE_INCREMENT;
    }

    if let Some(div) = color_diversity {
        if div > crate::constants::IMAGE_CONFIDENCE_MIN_COLOR_DIVERSITY
            && div < crate::constants::IMAGE_CONFIDENCE_MAX_COLOR_DIVERSITY
        {
            confidence += crate::constants::IMAGE_CONFIDENCE_INCREMENT;
        }
    } else {
        confidence -= crate::constants::IMAGE_CONFIDENCE_INCREMENT;
    }

    confidence.clamp(0.0, 1.0)
}

#[must_use]
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
        .is_some_and(|e| e == "jpg" || e == "jpeg");

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
    if let Some(cache) = cache
        && let Err(err) = cache.store_quality_analysis(path, &analysis)
    {
        tracing::warn!(
            path = %path.display(),
            error = %err,
            "Failed to store image quality analysis in cache"
        );
    }
    Some(analysis)
}

/// Analyzes image quality from a file path.
///
/// Internal function that loads an image from disk and performs
/// quality analysis using the core analysis engine.
///
/// # Arguments
/// * `path` - Path to the image file
///
/// # Returns
/// Image quality analysis results, or None if loading fails
fn analyze_image_quality_from_path_internal(path: &Path) -> Option<ImageQualityAnalysis> {
    let img = match open(path) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "Failed to open image for quality analysis");
            return None;
        }
    };
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();
    let file_size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "Failed to get metadata for quality analysis");
            return None;
        }
    };
    let format = path.extension().map_or_else(
        || "unknown".to_string(),
        |e| e.to_string_lossy().to_uppercase(),
    );
    match analyze_image_quality(
        width,
        height,
        rgba.as_raw(),
        file_size,
        &format,
        Some(1),
        PrecisionMetadata::default(),
    ) {
        Ok(res) => Some(res),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "Image quality analysis calculation failed");
            None
        }
    }
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
            "  content_type={} complexity={} edge_density={}",
            analysis.content_type.name,
            analysis
                .complexity
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.4}")),
            analysis
                .edge_density
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.4}"))
        ),
    );
    write_to_log_at_level(
        Level::DEBUG,
        &format!(
            "  color_diversity={} texture_variance={} noise={} sharpness={} contrast={} confidence={:.4}",
            analysis
                .color_diversity
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.4}")),
            analysis
                .texture_variance
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.4}")),
            analysis
                .noise_level
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.4}")),
            analysis
                .sharpness
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.4}")),
            analysis
                .contrast
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.4}")),
            analysis
                .confidence
                .map_or_else(|| "N/A".to_string(), |v| format!("{v:.4}"))
        ),
    );
    write_to_log_at_level(Level::DEBUG, "");
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_complexity_range(
            edge in 0.0_f64..2.0f64,
            div in 0.0_f64..2.0f64,
            tex in 0.0_f64..2.0f64,
            noise in 0.0_f64..2.0f64
        ) {
            let score = calculate_overall_complexity(Some(edge), Some(div), Some(tex), Some(noise))
                .expect("Valid inputs should yield a score");
            prop_assert!(
                (0.0_f64..=1.0_f64).contains(&score),
                "Complexity score must be in [0, 1] (got {})",
                score
            );
        }

        #[test]
        fn test_confidence_range(
            pixels in 0..10_000_000u64,
            size in 0..500_000_000u64,
            edge in 0.0_f64..1.5f64,
            div in 0.0_f64..1.5f64
        ) {
            let conf = calculate_analysis_confidence(pixels, size, Some(edge), Some(div));
            prop_assert!((0.0_f64..=1.0_f64).contains(&conf), "Confidence must be in [0, 1] (got {:?})", conf);
        }
    }
}
