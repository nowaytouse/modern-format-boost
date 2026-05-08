use crate::image_analyzer::{ImageAnalysis, JxlIndicator};
use crate::image_detection::{CompressionType, DetectedFormat, DetectionResult};
use crate::media_index_types::MediaIndexRow;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeRecommendation {
    pub current_format: String,
    pub recommended_format: String,
    pub reason: String,
    pub expected_size_reduction: f64,
    pub quality_preservation: String,
    pub command: String,
}

#[must_use]
pub fn get_recommendation(analysis: &ImageAnalysis) -> UpgradeRecommendation {
    let indicator = &analysis.jxl_indicator;
    format_recommendation(indicator, &analysis.format, analysis.is_lossless)
}

/// 🚀 New Entry Point: Subscribes to `MediaIndexRow` (Database-driven decision)
///
/// # Errors
/// Returns an error if the recommendation cannot be generated.
pub fn get_recommendation_from_row(
    row: &MediaIndexRow,
) -> Result<UpgradeRecommendation, serde_json::Error> {
    let features: DetectionResult = serde_json::from_str(&row.raw_features_json)?;
    let is_lossless = features.compression == CompressionType::Lossless;

    let indicator = jxl_indicator_from_features(&features, &row.rel_path);

    Ok(format_recommendation(&indicator, &row.format, is_lossless))
}

fn format_recommendation(
    indicator: &JxlIndicator,
    format: &str,
    is_lossless: bool,
) -> UpgradeRecommendation {
    if indicator.should_convert {
        UpgradeRecommendation {
            current_format: format.to_string(),
            recommended_format: "JXL".to_string(),
            reason: indicator.reason.clone(),
            expected_size_reduction: if is_lossless { 45.0 } else { 20.0 },
            quality_preservation: if is_lossless {
                "Mathematically Lossless".to_string()
            } else {
                "Lossless JPEG Transcode".to_string()
            },
            command: indicator.command.clone(),
        }
    } else {
        UpgradeRecommendation {
            current_format: format.to_string(),
            recommended_format: format.to_string(),
            reason: indicator.reason.clone(),
            expected_size_reduction: 0.0,
            quality_preservation: "N/A".to_string(),
            command: String::new(),
        }
    }
}

/// Build a `JxlIndicator` from indexed DB features, mirroring `image_analyzer::generate_jxl_indicator`.
/// This is the production code path used when the analyzer output is not in scope (DB-driven flow).
fn jxl_indicator_from_features(features: &DetectionResult, rel_path: &str) -> JxlIndicator {
    let output_path = format!("{rel_path}.jxl");
    let is_lossless = features.compression == CompressionType::Lossless;
    let default_effort = crate::constants::JXL_DEFAULT_EFFORT;

    match features.format {
        DetectedFormat::PNG | DetectedFormat::GIF | DetectedFormat::TIFF => JxlIndicator {
            should_convert: true,
            reason: "Lossless image; strongly recommend converting to JXL".to_string(),
            command: format!(
                "cjxl '{rel_path}' '{output_path}' -d 0.0 --modular=1 -e {default_effort}"
            ),
            benefit: "30-60% size reduction while preserving full quality".to_string(),
        },
        DetectedFormat::JPEG => JxlIndicator {
            should_convert: true,
            reason: "JPEG can be losslessly transcoded to JXL".to_string(),
            command: format!(
                "cjxl '{rel_path}' '{output_path}' --lossless_jpeg=1 -e {default_effort}"
            ),
            benefit: "Keeps original JPEG DCT coefficients, reversible".to_string(),
        },
        DetectedFormat::WebP if is_lossless => JxlIndicator {
            should_convert: true,
            reason: "Lossless WebP; recommend converting to JXL".to_string(),
            command: format!(
                "cjxl '{rel_path}' '{output_path}' -d 0.0 --modular=1 -e {default_effort}"
            ),
            benefit: "JXL is typically more efficient than lossless WebP".to_string(),
        },
        _ => JxlIndicator {
            should_convert: false,
            reason: "Already efficient or unsupported conversion".to_string(),
            command: String::new(),
            benefit: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_analyzer::{ImageFeatures, JxlIndicator};
    use crate::image_detection::PrecisionMetadata;
    use crate::types::{ProcessHistory, VisualPerception};
    use std::collections::HashMap;

    #[test]
    fn test_png_recommendation() {
        let analysis = ImageAnalysis {
            file_path: "test.png".to_string(),
            format: "PNG".to_string(),
            width: 1920,
            height: 1080,
            file_size: 1_000_000,
            color_depth: Some(8),
            color_space: "sRGB".to_string(),
            has_alpha: false,
            is_animated: false,
            duration_secs: None,
            is_lossless: true,
            jpeg_analysis: None,
            heic_analysis: None,
            features: ImageFeatures {
                entropy: 7.5,
                compression_ratio: 0.5,
            },
            jxl_indicator: JxlIndicator {
                should_convert: true,
                reason: "Lossless image; strongly recommend converting to JXL".to_string(),
                command: "cjxl 'test.png' 'test.jxl' -d 0.0 -e 7".to_string(),
                benefit: "May reduce size by 30–60%".to_string(),
            },
            psnr: None,
            ssim: None,
            metadata: HashMap::new(),
            hdr_info: None,
            precision: PrecisionMetadata::default(),
            history: ProcessHistory::default(),
            perception: VisualPerception::default(),
            analysis_error: None,
            cache_version: 0,
        };

        let rec = get_recommendation(&analysis);
        assert_eq!(rec.recommended_format, "JXL");
        assert_eq!(rec.quality_preservation, "Mathematically Lossless");
    }
}
