//! 🔬 Image Quality Detector - Content Classification & Media Metrics
//!
//! This module provides **pixel-based image classification** and quality
//! dimensions. It is used to generate UI labels (e.g., PHOTO, SCREENSHOT) and
//! detailed quality metrics for logging. Routing and compression decisions are
//! handled by `image_analyzer/recommender`.
//!
//! ## Functions
//! - **Image Content Classification**: Categorizes images into logical types
//!   (Icon, Photo, etc.)
//! - **Quality Metrics**: Extracts complexity, edge density, color diversity,
//!   and more.
//! - **Media Information**: Provides a formatted summary of image
//!   characteristics.

use crate::image_detection::PrecisionMetadata;
use crate::progress_mode::has_log_file;
use image::{GenericImageView, open};
use serde::{Deserialize, Serialize};
use std::path::Path;

const TOKEN_PERCENT_INT: &str = "{:.0}";

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

impl ImageQualityAnalysis {
    /// Calculates a "Lossless Affinity Score" (0.0 to 1.0) indicating how
    /// likely the image is to be a clean, digital, or master-quality source
    /// that warrants lossless preservation.
    ///
    /// Combines complexity, noise, and content-type heuristics.
    #[must_use]
    pub fn lossless_affinity_score(&self) -> Option<f64> {
        let complexity_factor = 1.0 - self.complexity?;
        let noise_factor = 1.0 - self.noise_level?;
        let texture_factor = 1.0 - self.texture_variance?;
        let color_factor = self.color_diversity?; // high diversity is neutral/positive

        let mut score = complexity_factor.mul_add(
            crate::constants::AFFINITY_WEIGHT_COMPLEXITY,
            noise_factor.mul_add(
                crate::constants::AFFINITY_WEIGHT_NOISE,
                texture_factor.mul_add(
                    crate::constants::AFFINITY_WEIGHT_TEXTURE,
                    color_factor * crate::constants::AFFINITY_WEIGHT_COLOR,
                ),
            ),
        );

        // Content-type bonus (e.g. Screenshot, Icon)
        if crate::constants::HEURISTIC_LOSSLESS_CREDIBLE_TYPES
            .contains(&self.content_type.name.as_str())
        {
            score += crate::constants::AFFINITY_BONUS_CREDIBLE_TYPE;
        }

        // Alpha channel bonus (lossless preservation is critical for alpha)
        if self.has_alpha {
            score += crate::constants::AFFINITY_BONUS_ALPHA;
        }

        let score = score.clamp(0.0, 1.0);
        crate::algorithm_seal::quality_unit_probability(score)
    }

    /// Sanitize pixel-analysis metrics before routing or logging.
    pub fn seal_algorithm_outputs(&mut self) {
        self.complexity = self
            .complexity
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.edge_density = self
            .edge_density
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.color_diversity = self
            .color_diversity
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.texture_variance = self
            .texture_variance
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.noise_level = self
            .noise_level
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.sharpness = self
            .sharpness
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.contrast = self
            .contrast
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.confidence = self
            .confidence
            .and_then(crate::algorithm_seal::quality_unit_probability);
    }
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
        let json = include_str!("../../../dev/src/config/image_classifiers.json");
        let wrapper: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(e) => unreachable!(
                "CRITICAL: embedded image_classifiers.json is malformed (error: {})",
                e
            ),
        };
        match wrapper.get("classifiers") {
            Some(rules_array) => match serde_json::from_value(rules_array.clone()) {
                Ok(v) => v,
                Err(e) => unreachable!(
                    "CRITICAL: embedded image_classifiers.json 'classifiers' array is malformed \
                     (error: {})",
                    e
                ),
            },
            None => Vec::new(),
        }
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
    crate::log_detail!(
        &crate::infra::static_logs::messages::MSG_IQD_PIXEL_FEATURE
            .replacen("{}", &width.to_string(), 1)
            .replacen("{}", &height.to_string(), 1)
            .replacen("{}", format, 1)
    );

    let expected_size = crate::numeric_cast::u32_to_usize_strict(width, "width")
        .ok_or_else(|| "Width too large for processing".to_string())?
        .checked_mul(
            crate::numeric_cast::u32_to_usize_strict(height, "height")
                .ok_or_else(|| "Height too large for processing".to_string())?,
        )
        .ok_or_else(|| "Image dimensions overflow processing limits".to_string())?
        .checked_mul(4)
        .ok_or_else(|| "Image data size overflows processing limits".to_string())?;
    if rgba_data.len() < expected_size {
        return Err(crate::infra::static_logs::messages::MSG_IQD_INVALID_RGBA
            .replacen("{}", &expected_size.to_string(), 1)
            .replacen("{}", &width.to_string(), 1)
            .replacen("{}", &height.to_string(), 1)
            .replacen("{}", &rgba_data.len().to_string(), 1));
    }

    if width == 0 || height == 0 {
        return Err(crate::infra::static_logs::messages::MSG_IQD_INVALID_DIM.to_string());
    }

    let pixels = u64::from(width) * u64::from(height);

    let edge_density = calculate_edge_density(rgba_data, width, height);

    let color_diversity = match precision.palette_size {
        Some(p_size) => {
            Some(crate::media_conversion_gate::probe_palette_color_diversity_ratio(p_size))
        }
        None => calculate_color_diversity(rgba_data, width, height),
    };

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

    let confidence =
        calculate_analysis_confidence(pixels, file_size, edge_density, color_diversity);

    crate::log_success!(
        crate::infra::static_logs::messages::LABEL_DETECTION,
        &crate::infra::static_logs::messages::MSG_IQD_CLASSIFIED
            .replacen("{}", &content_type.name, 1)
            .replacen(TOKEN_PERCENT_INT, &format!("{:.0}", confidence * 100.0), 1)
    );

    let mut analysis = ImageQualityAnalysis {
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
        confidence: Some(confidence),
        precision,
        history: crate::common_utils::get_current_history(),
        perception: crate::types::Visual::default(),
    };
    analysis.seal_algorithm_outputs();
    Ok(analysis)
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

    let pixels = crate::numeric_cast::u32_to_usize_strict(width, "width")?
        .checked_mul(crate::numeric_cast::u32_to_usize_strict(height, "height")?)?;
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

    let w = crate::numeric_cast::u32_to_usize_strict(width, "width")?;

    for y in (1..crate::numeric_cast::u32_to_usize_strict(height.saturating_sub(1), "height")?)
        .step_by(step)
    {
        for x in (1..crate::numeric_cast::u32_to_usize_strict(width.saturating_sub(1), "width")?)
            .step_by(step)
        {
            let get_gray = |px: usize, py: usize| -> Option<i32> {
                let idx = (py * w + px) * 4;
                let r = i32::from(*rgba.get(idx)?);
                let g = i32::from(*rgba.get(idx + 1)?);
                let b = i32::from(*rgba.get(idx + 2)?);
                Some(
                    (r * crate::constants::LUMA_COEFF_R
                        + g * crate::constants::LUMA_COEFF_G
                        + b * crate::constants::LUMA_COEFF_B)
                        / crate::constants::LUMA_DIVISOR,
                )
            };

            let Some(gray_right) = get_gray(x + 1, y) else {
                continue;
            };
            let Some(gray_left) = get_gray(x - 1, y) else {
                continue;
            };
            let Some(gray_below) = get_gray(x, y + 1) else {
                continue;
            };
            let Some(gray_above) = get_gray(x, y - 1) else {
                continue;
            };

            let gx = gray_right - gray_left;
            let gy = gray_below - gray_above;
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
/// Higher diversity indicates more complex and potentially higher quality
/// images.
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

    let pixels = crate::numeric_cast::u32_to_usize_strict(width, "width")?
        .checked_mul(crate::numeric_cast::u32_to_usize_strict(height, "height")?)?;
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::COLOR_DIVERSITY_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_strict(
            crate::constants::IMAGE_SIZE_THRESHOLD_LARGE,
            "threshold_large",
        )?
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

    let pixels = crate::numeric_cast::u32_to_usize_strict(width, "width")?
        .checked_mul(crate::numeric_cast::u32_to_usize_strict(height, "height")?)?;
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::TEXTURE_VARIANCE_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_strict(
            crate::constants::IMAGE_SIZE_THRESHOLD_LARGE,
            "threshold_large",
        )?
    {
        crate::constants::TEXTURE_VARIANCE_STEP_MEDIUM
    } else {
        crate::constants::TEXTURE_VARIANCE_STEP_NORMAL
    };

    let mut variance_sum = 0.0_f64;
    let mut sample_count = 0usize;

    for y in (1..crate::numeric_cast::u32_to_usize_strict(height.saturating_sub(1), "height")?)
        .step_by(step)
    {
        for x in (1..crate::numeric_cast::u32_to_usize_strict(width.saturating_sub(1), "width")?)
            .step_by(step)
        {
            let mut sum = 0i32;
            let mut sq_sum = 0i64;

            for dy in -1i32..=1_i32 {
                for dx in -1i32..=1_i32 {
                    let px = crate::numeric_cast::i32_to_usize_strict(
                        crate::numeric_cast::usize_to_i32_strict(x, "x")? + dx,
                        "px",
                    )?;
                    let py = crate::numeric_cast::i32_to_usize_strict(
                        crate::numeric_cast::usize_to_i32_strict(y, "y")? + dy,
                        "py",
                    )?;
                    let idx =
                        (py * crate::numeric_cast::u32_to_usize_strict(width, "width")? + px) * 4;

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

    let pixels = crate::numeric_cast::u32_to_usize_strict(width, "width")?
        .checked_mul(crate::numeric_cast::u32_to_usize_strict(height, "height")?)?;
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::NOISE_LEVEL_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_strict(
            crate::constants::IMAGE_SIZE_THRESHOLD_LARGE,
            "threshold_large",
        )?
    {
        crate::constants::NOISE_LEVEL_STEP_MEDIUM
    } else {
        crate::constants::IMAGE_SAMPLING_STEP_NORMAL
    };

    let mut diff_sum = 0.0_f64;
    let mut sample_count = 0usize;

    for y in (0..crate::numeric_cast::u32_to_usize_strict(height.saturating_sub(1), "height")?)
        .step_by(step)
    {
        for x in (0..crate::numeric_cast::u32_to_usize_strict(width.saturating_sub(1), "width")?)
            .step_by(step)
        {
            let idx = (y * crate::numeric_cast::u32_to_usize_strict(width, "width")? + x) * 4;
            let idx_right = idx + 4;
            let idx_down = idx + (crate::numeric_cast::u32_to_usize_strict(width, "width")? * 4);

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

    let pixels = crate::numeric_cast::u32_to_usize_strict(width, "width")?
        .checked_mul(crate::numeric_cast::u32_to_usize_strict(height, "height")?)?;
    let step = if crate::numeric_cast::usize_to_u64(pixels)
        > crate::constants::SHARPNESS_SAMPLING_PIXELS_LARGE
    {
        crate::constants::SHARPNESS_SAMPLING_STEP_LARGE
    } else if pixels
        > crate::numeric_cast::u64_to_usize_strict(
            crate::constants::IMAGE_SIZE_THRESHOLD_LARGE,
            "threshold_large",
        )?
    {
        crate::constants::SHARPNESS_SAMPLING_STEP_MEDIUM
    } else {
        crate::constants::SHARPNESS_SAMPLING_STEP_NORMAL
    };

    let mut laplacian_sum = 0.0_f64;
    let mut sample_count = 0usize;

    let get_gray = |x: usize, y: usize| -> Option<i32> {
        let idx = (y * crate::numeric_cast::u32_to_usize_strict(width, "width")? + x) * 4;
        Some(
            (i32::from(rgba[idx]) * crate::constants::LUMA_COEFF_R
                + i32::from(rgba[idx + 1]) * crate::constants::LUMA_COEFF_G
                + i32::from(rgba[idx + 2]) * crate::constants::LUMA_COEFF_B)
                / crate::constants::LUMA_DIVISOR,
        )
    };

    for y in (1..crate::numeric_cast::u32_to_usize_strict(height.saturating_sub(1), "height")?)
        .step_by(step)
    {
        for x in (1..crate::numeric_cast::u32_to_usize_strict(width.saturating_sub(1), "width")?)
            .step_by(step)
        {
            let center = get_gray(x, y)?;
            let top = get_gray(x, y - 1)?;
            let bottom = get_gray(x, y + 1)?;
            let left = get_gray(x - 1, y)?;
            let right = get_gray(x + 1, y)?;

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
    let pixels = crate::numeric_cast::u32_to_usize_strict(width, "width")?
        .checked_mul(crate::numeric_cast::u32_to_usize_strict(height, "height")?);
    let step = if let Some(p) = pixels
        && crate::numeric_cast::usize_to_u64(p)
            > crate::constants::IMAGE_CONFIDENCE_PIXELS_LARGE_THRESHOLD
    {
        crate::constants::CONTRAST_SAMPLING_STEP_LARGE
    } else if let Some(p) = pixels
        && p > crate::numeric_cast::u64_to_usize_strict(
            crate::constants::IMAGE_SIZE_THRESHOLD_LARGE,
            "large_img_threshold",
        )?
    {
        crate::constants::CONTRAST_SAMPLING_STEP_MEDIUM
    } else {
        crate::constants::CONTRAST_SAMPLING_STEP_NORMAL
    };

    let mut sum = 0u64;
    let mut sq_sum = 0u64;
    let mut sample_count = 0usize;

    let p = pixels?;
    for i in (0..p).step_by(step) {
        let idx = i * 4;
        if idx + 2 < rgba.len() {
            let gray = (u64::from(rgba[idx])
                * crate::numeric_cast::i32_to_u64_strict(
                    crate::constants::LUMA_COEFF_R,
                    "luma_coeff_r",
                )?
                + u64::from(rgba[idx + 1])
                    * crate::numeric_cast::i32_to_u64_strict(
                        crate::constants::LUMA_COEFF_G,
                        "luma_coeff_g",
                    )?
                + u64::from(rgba[idx + 2])
                    * crate::numeric_cast::i32_to_u64_strict(
                        crate::constants::LUMA_COEFF_B,
                        "luma_coeff_b",
                    )?)
                / crate::numeric_cast::i32_to_u64_strict(
                    crate::constants::LUMA_DIVISOR,
                    "luma_divisor",
                )?;
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

    match best_rule {
        Some(rule) => ImageContentType {
            name: rule.name.clone(),
        },
        None => ImageContentType {
            name: crate::media_conversion_gate::probe_classifier_content_name_or_unknown(
                "image_quality_classifier",
            ),
        },
    }
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

fn is_jpeg_content(path: &Path) -> bool {
    matches!(
        crate::image::format_detect::detect_true_format(path),
        Ok(crate::image::format_detect::FormatKind::Jpeg)
    )
}

#[must_use]
pub fn analyze_image_quality_with_cache(
    path: &Path,
    cache: Option<&crate::analysis_cache::AnalysisCache>,
) -> Option<ImageQualityAnalysis> {
    if !crate::algorithm_runtime::image_quality_heuristic_enabled() {
        return None;
    }

    if is_jpeg_content(path) {
        return None;
    }

    if let Some(cache) = cache {
        match cache.get_quality_analysis(path) {
            Ok(Some(mut cached)) => {
                cached.seal_algorithm_outputs();
                return Some(cached);
            }
            Ok(None) => {}
            Err(err) => {
                crate::media_conversion_gate::image_quality_cache_load_failed_audit(path, err);
            }
        }
    }

    let analysis = analyze_image_quality_from_path_internal(path)?;
    if let Some(cache) = cache
        && let Err(err) = cache.store_quality_analysis(path, &analysis)
    {
        crate::media_conversion_gate::image_quality_cache_store_failed_audit(
            path,
            "quality-ingest",
            err,
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
            crate::media_conversion_gate::probe_quality_layer_audit(
                "image_quality_open_failed",
                path,
                format!("failed to open for quality analysis: {e}"),
            );
            return None;
        }
    };
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();
    let file_size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            crate::media_conversion_gate::probe_quality_layer_audit(
                "image_quality_metadata_failed",
                path,
                format!("failed to read metadata for quality analysis: {e}"),
            );
            return None;
        }
    };
    let format = crate::media_conversion_gate::path_extension_uppercase_or_unknown(
        path,
        "image_quality_format_label",
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
            crate::media_conversion_gate::probe_quality_layer_audit(
                "image_quality_calc_failed",
                path,
                format!("quality analysis calculation failed: {e}"),
            );
            None
        }
    }
}

pub fn log_media_info_for_image_quality(analysis: &ImageQualityAnalysis, input_path: &Path) {
    if !has_log_file() {
        return;
    }
    let label = crate::infra::static_logs::messages::LABEL_QUALITY;
    crate::log_summary_header!(
        &crate::media_conversion_gate::ui_visual_artifact_audit_title(input_path)
    );
    crate::log_report_stat!(
        label,
        format!(
            "Geometry: {}x{} | Format: {} | Footprint: {}",
            analysis.width,
            analysis.height,
            analysis.format,
            crate::format_bytes(analysis.file_size)
        )
    );
    crate::log_report_stat!(
        label,
        format!(
            "Class: {} | Complexity: {} | Edge Density: {}",
            analysis.content_type.name,
            crate::media_conversion_gate::ui_f64_or_na(
                analysis.complexity,
                "image_quality_complexity",
                4,
            ),
            crate::media_conversion_gate::ui_f64_or_na(
                analysis.edge_density,
                "image_quality_edge_density",
                4,
            )
        )
    );
    crate::log_report_stat!(
        label,
        format!(
            "Diversity: {} | Texture: {} | Noise: {} | Sharpness: {} | Contrast: {} | Confidence: \
             {:.1}%",
            crate::media_conversion_gate::ui_f64_or_na(
                analysis.color_diversity,
                "image_quality_color_diversity",
                4,
            ),
            crate::media_conversion_gate::ui_f64_or_na(
                analysis.texture_variance,
                "image_quality_texture_variance",
                4,
            ),
            crate::media_conversion_gate::ui_f64_or_na(
                analysis.noise_level,
                "image_quality_noise_level",
                4,
            ),
            crate::media_conversion_gate::ui_f64_or_na(
                analysis.sharpness,
                "image_quality_sharpness",
                4,
            ),
            crate::media_conversion_gate::ui_f64_or_na(
                analysis.contrast,
                "image_quality_contrast",
                4,
            ),
            crate::media_conversion_gate::ui_f64_percent_or_na(
                analysis.confidence,
                "image_quality_confidence",
            )
        )
    );
    tracing::debug!("");
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::Write;

    proptest! {
        #[test]
        fn test_complexity_range(
            edge in 0.0_f64..2.0f64,
            div in 0.0_f64..2.0f64,
            tex in 0.0_f64..2.0f64,
            noise in 0.0_f64..2.0f64
        ) {
            let score = calculate_overall_complexity(Some(edge), Some(div), Some(tex), Some(noise))
                .unwrap_or_else(|| {
                    unreachable!(
                        "CRITICAL: Valid inputs should yield a score in calculate_overall_complexity (edge={:?}, div={:?}, tex={:?}, noise={:?})",
                        edge,
                        div,
                        tex,
                        noise
                    )
                });
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

    #[test]
    fn jpeg_quality_bypass_uses_content_not_suffix() {
        let mut spoof = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "jpeg_quality_content_test",
            None,
            Some(".jpg"),
        )
        .expect("create spoof JPEG path");
        spoof
            .write_all(b"\x89PNG\r\n\x1a\n")
            .expect("write PNG signature");
        assert!(!is_jpeg_content(spoof.path()));

        let mut jpeg = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "jpeg_quality_content_test",
            None,
            Some(".bin"),
        )
        .expect("create JPEG content path");
        jpeg.write_all(&[0xFF, 0xD8, 0xFF])
            .expect("write JPEG signature");
        assert!(is_jpeg_content(jpeg.path()));
    }

    #[test]
    fn test_lossless_affinity_score() {
        use super::{ImageContentType, ImageQualityAnalysis};
        use crate::image_detection::PrecisionMetadata;

        // 1. High-integrity digital source (Screenshot)
        let screenshot_confidence = 0.95_f64;
        let screenshot = ImageQualityAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            has_alpha: true,
            is_animated: false,
            frame_count: Some(1),
            complexity: Some(0.1), // Very simple
            edge_density: Some(0.05),
            color_diversity: Some(0.2),
            texture_variance: Some(0.05),
            noise_level: Some(0.01), // Very clean
            sharpness: Some(0.9),
            contrast: Some(0.8),
            content_type: ImageContentType {
                name: "SCREENSHOT".to_string(),
            },
            confidence: Some(screenshot_confidence),
            precision: PrecisionMetadata::default(),
            history: crate::types::ProcessHistory::default(),
            perception: crate::types::Visual::default(),
        };

        let score = screenshot.lossless_affinity_score().unwrap();
        // Base affinity (approx): 0.9*0.4 + 0.99*0.3 + 0.95*0.2 + 0.2*0.1 = 0.36 +
        // 0.297 + 0.19 + 0.02 = 0.867
        // + Credible bonus (0.15) = 1.017 -> clamp 1.0
        assert!(score >= 0.95);

        // 2. Noisy Photo
        let photo = ImageQualityAnalysis {
            width: 1920,
            height: 1080,
            file_size: 2_000_000,
            format: "JPEG".to_string(),
            has_alpha: false,
            is_animated: false,
            frame_count: Some(1),
            complexity: Some(0.7), // Complex
            edge_density: Some(0.4),
            color_diversity: Some(0.8),
            texture_variance: Some(0.6),
            noise_level: Some(0.5), // Noisy
            sharpness: Some(0.5),
            contrast: Some(0.5),
            content_type: ImageContentType {
                name: "PHOTO".to_string(),
            },
            confidence: Some(0.55_f64),
            precision: PrecisionMetadata::default(),
            history: crate::types::ProcessHistory::default(),
            perception: crate::types::Visual::default(),
        };

        let score_photo = photo.lossless_affinity_score().unwrap();
        // Base affinity (approx): 0.3*0.4 + 0.5*0.3 + 0.4*0.2 + 0.8*0.1 = 0.12 + 0.15 +
        // 0.08 + 0.08 = 0.43
        assert!(score_photo < 0.6);
    }

    #[test]
    fn test_calculate_edge_density_minimal() {
        // 4x4 black image - no edges
        let rgba = vec![0u8; 4 * 4 * 4];
        let density = calculate_edge_density(&rgba, 4, 4).unwrap();
        assert!(density.abs() < f64::EPSILON);

        // 4x4 image with a sharp edge
        let mut rgba_edge = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..2 {
                let idx = (y * 4 + x) * 4;
                rgba_edge[idx] = 255;
                rgba_edge[idx + 1] = 255;
                rgba_edge[idx + 2] = 255;
            }
        }
        let density_edge = calculate_edge_density(&rgba_edge, 4, 4).unwrap();
        assert!(density_edge > 0.0);
    }

    #[test]
    fn test_calculate_color_diversity_minimal() {
        // Single color
        let rgba = vec![128u8; 10 * 10 * 4];
        let div = calculate_color_diversity(&rgba, 10, 10).unwrap();
        assert!(div < 0.1);

        // Multiple colors
        let mut rgba_multi = vec![0u8; 10 * 10 * 4];
        for i in 0..100 {
            rgba_multi[i * 4] = u8::try_from(i % 255)
                .expect("Forensic Analysis: i % 255 must fit in u8 for test pattern generation");
            rgba_multi[i * 4 + 1] = u8::try_from((i * 2) % 255).expect(
                "Forensic Analysis: (i * 2) % 255 must fit in u8 for test pattern generation",
            );
            rgba_multi[i * 4 + 2] = u8::try_from((i * 3) % 255).expect(
                "Forensic Analysis: (i * 3) % 255 must fit in u8 for test pattern generation",
            );
        }
        let div_multi = calculate_color_diversity(&rgba_multi, 10, 10).unwrap();
        assert!(div_multi > div);
    }

    #[test]
    fn test_detect_alpha_usage() {
        let mut rgba = vec![255u8; 10 * 10 * 4];
        assert!(!detect_alpha_usage(&rgba));

        rgba[3] = 128; // First pixel alpha
        assert!(detect_alpha_usage(&rgba));
    }

    #[test]
    fn test_classify_content_type_basic() {
        let screenshot_input = ClassifierInput {
            complexity: Some(0.1),
            edge_density: Some(0.25), // Increased to match SCREENSHOT rule
            color_diversity: Some(0.1),
            texture_variance: Some(0.05),
            noise_level: Some(0.01),
            sharpness: Some(0.9),
            contrast: Some(0.8),
            has_alpha: false,
            is_animated: false,
            width: 1920,
            height: 1080,
        };
        let content_type = classify_content_type(&screenshot_input);
        assert_eq!(content_type.name, "SCREENSHOT");

        let photo_input = ClassifierInput {
            complexity: Some(0.7),
            edge_density: Some(0.5),
            color_diversity: Some(0.8),
            texture_variance: Some(0.6),
            noise_level: Some(0.4),
            sharpness: Some(0.5),
            contrast: Some(0.5),
            has_alpha: false,
            is_animated: false,
            width: 1920,
            height: 1080,
        };
        let photo_type = classify_content_type(&photo_input);
        assert_eq!(photo_type.name, "GAME_CAPTURE");
    }

    #[test]
    fn test_alpha_channel_increases_affinity_score() {
        let base = ImageQualityAnalysis {
            width: 100,
            height: 100,
            file_size: 10_000,
            format: "PNG".to_string(),
            has_alpha: false,
            is_animated: false,
            frame_count: Some(1),
            complexity: Some(0.1),
            edge_density: Some(0.1),
            color_diversity: Some(0.5),
            texture_variance: Some(0.1),
            noise_level: Some(0.1),
            sharpness: Some(0.5),
            contrast: Some(0.5),
            content_type: ImageContentType {
                name: "PHOTO".to_string(),
            },
            confidence: Some(0.72_f64),
            precision: PrecisionMetadata::default(),
            history: crate::types::ProcessHistory::default(),
            perception: crate::types::Visual::default(),
        };

        let mut with_alpha = base.clone();
        with_alpha.has_alpha = true;

        let score_no_alpha = base.lossless_affinity_score().unwrap();
        let score_with_alpha = with_alpha.lossless_affinity_score().unwrap();
        assert!(
            score_with_alpha > score_no_alpha,
            "Alpha channel must increase affinity score: {score_with_alpha} vs {score_no_alpha}"
        );
    }

    #[test]
    fn test_threshold_range_boundary_exact() {
        let range = ThresholdRange {
            min: Some(0.5),
            max: Some(1.0),
        };
        assert!(
            range.matches(Some(0.5)),
            "Value exactly at min should match (inclusive lower bound)"
        );
        assert!(
            range.matches(Some(1.0)),
            "Value exactly at max should match (inclusive upper bound)"
        );
        assert!(range.matches(Some(0.75)), "Value in range should match");
        assert!(
            !range.matches(Some(0.3)),
            "Value below min should not match"
        );
        assert!(
            !range.matches(Some(1.5)),
            "Value above max should not match"
        );
        assert!(
            !range.matches(Some(0.4999)),
            "Value just below min should not match"
        );
        assert!(
            !range.matches(Some(1.0001)),
            "Value just above max should not match"
        );
    }
}
