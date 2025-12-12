use crate::analyzer::ImageAnalysis;
use serde::{Deserialize, Serialize};

/// Simple upgrade recommendation
/// Note: Most of the intelligence is now in the JxlIndicator from analyzer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeRecommendation {
    pub current_format: String,
    pub recommended_format: String,
    pub reason: String,
    pub expected_size_reduction: f64,
    pub quality_preservation: String,
    pub command: String,
}

/// Get simple upgrade recommendation based on analysis
/// The real logic is now in analyzer.rs via JxlIndicator
pub fn get_recommendation(analysis: &ImageAnalysis) -> UpgradeRecommendation {
    let indicator = &analysis.jxl_indicator;
    
    if indicator.should_convert {
        UpgradeRecommendation {
            current_format: analysis.format.clone(),
            recommended_format: "JXL".to_string(),
            reason: indicator.reason.clone(),
            expected_size_reduction: if analysis.is_lossless { 45.0 } else { 20.0 },
            quality_preservation: if analysis.is_lossless {
                "Mathematically Lossless".to_string()
            } else {
                "Lossless JPEG Transcode".to_string()
            },
            command: indicator.command.clone(),
        }
    } else {
        UpgradeRecommendation {
            current_format: analysis.format.clone(),
            recommended_format: analysis.format.clone(),
            reason: indicator.reason.clone(),
            expected_size_reduction: 0.0,
            quality_preservation: "N/A".to_string(),
            command: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::analyzer::{ImageFeatures, JxlIndicator};

    #[test]
    fn test_png_recommendation() {
        let analysis = ImageAnalysis {
            file_path: "test.png".to_string(),
            format: "PNG".to_string(),
            width: 1920,
            height: 1080,
            file_size: 1_000_000,
            color_depth: 8,
            color_space: "sRGB".to_string(),
            has_alpha: false,
            is_animated: false,
            duration_secs: None,  // 静态图像无时长
            is_lossless: true,
            jpeg_analysis: None,
            heic_analysis: None,
            features: ImageFeatures {
                entropy: 7.5,
                compression_ratio: 0.5,
            },
            jxl_indicator: JxlIndicator {
                should_convert: true,
                reason: "无损图像，强烈建议转换为JXL格式".to_string(),
                command: "cjxl 'test.png' 'test.jxl' -d 0.0 -e 8".to_string(),
                benefit: "可减少30-60%体积".to_string(),
            },
            psnr: None,
            ssim: None,
            metadata: HashMap::new(),
        };

        let rec = get_recommendation(&analysis);
        assert_eq!(rec.recommended_format, "JXL");
        assert_eq!(rec.quality_preservation, "Mathematically Lossless");
    }
}
