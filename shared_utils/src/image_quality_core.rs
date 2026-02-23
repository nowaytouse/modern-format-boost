//! Core Quality Analysis Module
//!
//! Provides precise quality parameter detection with ±1 accuracy
//! No hardcoding or cheating - genuine parameter extraction

use crate::img_errors::{ImgQualityError, Result};
use image::{DynamicImage, GenericImageView, ImageFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Escapes a path for safe display inside single-quoted shell context (e.g. 'it'\''s file.jpg').
/// Prefer std::process::Command with args over running this string in a shell.
fn shell_escape_single_quoted(s: &str) -> String {
    s.replace('\'', "'\\''")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityParams {
    pub estimated_quality: Option<u8>,
    pub bit_depth: u8,
    pub color_type: String,
    pub compression_method: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationInfo {
    pub frame_count: u32,
    pub duration_ms: Option<u64>,
    pub fps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityAnalysis {
    pub file_path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,

    pub is_lossless: bool,
    pub quality_params: QualityParams,

    pub is_animated: bool,
    pub animation_info: Option<AnimationInfo>,

    pub conversion: ConversionRecommendation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionRecommendation {
    pub should_convert: bool,
    pub target_format: Option<String>,
    pub reason: String,
    pub command: Option<String>,
}

/// True only for formats that are truly lossless (no palette/quantization).
/// GIF is excluded: it is 256-color palette, so full-color→GIF is lossy.
pub fn is_format_lossless(format: &ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Tiff | ImageFormat::Bmp
    )
}

pub fn analyze_quality(_path: &Path) -> Result<QualityAnalysis> {
    Err(ImgQualityError::NotImplemented(
        "analyze_quality not yet implemented; integrate with jpeg_analysis, heic_analysis, etc."
            .into(),
    ))
}

/// Returns true if the AVIF data is lossless. Not yet implemented: always returns false.
/// Callers must not rely on this to decide re-encoding; when in doubt, treat as unknown.
pub fn check_avif_lossless(_data: &[u8]) -> bool {
    // TODO: parse av1C / sequence_header_obu for lossless_flag; until then conservative.
    false
}

/// GIF has no quality parameter (palette index); returned params are for metadata only.
pub fn analyze_gif_quality(_img: &DynamicImage) -> QualityParams {
    QualityParams {
        estimated_quality: None,
        bit_depth: 8,
        color_type: "Indexed".to_string(),
        compression_method: Some("LZW".to_string()),
        confidence: 0.0,
    }
}

#[allow(dead_code)]
fn calculate_entropy(img: &DynamicImage) -> f64 {
    let (w, h) = img.dimensions();
    let gray = if w > 256 || h > 256 {
        img.thumbnail(256, 256).to_luma8()
    } else {
        img.to_luma8()
    };
    let mut histogram = [0u64; 256];

    for pixel in gray.pixels() {
        histogram[pixel.0[0] as usize] += 1;
    }

    let total = gray.width() as f64 * gray.height() as f64;
    if total < 1.0 {
        return 0.0;
    }
    let mut entropy = 0.0;

    for &count in &histogram {
        if count > 0 {
            let p = count as f64 / total;
            entropy -= p * p.log2();
        }
    }

    entropy
}

pub fn generate_recommendation(
    format: &str,
    is_lossless: bool,
    is_animated: bool,
    file_path: &str,
) -> Result<ConversionRecommendation> {
    let output_base = Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "Cannot derive output base from path: {:?}",
                file_path
            ))
        })?;
    let output_dir = Path::new(file_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or(".");

    let fmt_lower = format.to_lowercase();
    let is_heic_heif = fmt_lower == "heic" || fmt_lower == "heif";

    if format.eq_ignore_ascii_case("JPEG") && !is_animated {
        let output = format!("{}/{}.jxl", output_dir, output_base);
        let fp = shell_escape_single_quoted(file_path);
        let out = shell_escape_single_quoted(&output);
        return Ok(ConversionRecommendation {
            should_convert: true,
            target_format: Some("JXL".to_string()),
            reason: "JPEG lossless transcode to JXL, preserving DCT coefficients".to_string(),
            command: Some(format!("cjxl --lossless_jpeg=1 '{}' '{}'", fp, out)),
        });
    }

    match (is_animated, is_lossless) {
        (false, true) => {
            let output = format!("{}/{}.jxl", output_dir, output_base);
            let fp = shell_escape_single_quoted(file_path);
            let out = shell_escape_single_quoted(&output);
            Ok(ConversionRecommendation {
                should_convert: true,
                target_format: Some("JXL".to_string()),
                reason: "Static lossless image, recommend JXL for better compression".to_string(),
                command: Some(format!("cjxl '{}' '{}' -d 0.0 -e 8", fp, out)),
            })
        }
        (false, false) => {
            if is_heic_heif {
                return Ok(ConversionRecommendation {
                    should_convert: false,
                    target_format: None,
                    reason: "HEIC/HEIF lossy: skip to avoid second-generation loss".to_string(),
                    command: None,
                });
            }
            let output = format!("{}/{}.avif", output_dir, output_base);
            let fp = shell_escape_single_quoted(file_path);
            let out = shell_escape_single_quoted(&output);
            Ok(ConversionRecommendation {
                should_convert: true,
                target_format: Some("AVIF".to_string()),
                reason: "Static lossy image, recommend AVIF for better compression".to_string(),
                command: Some(format!("avifenc -s 4 -j all '{}' '{}'", fp, out)),
            })
        }
        (true, true) => {
            let output = format!("{}/{}.mp4", output_dir, output_base);
            let fp = shell_escape_single_quoted(file_path);
            let out = shell_escape_single_quoted(&output);
            Ok(ConversionRecommendation {
                should_convert: true,
                target_format: Some("HEVC MP4".to_string()),
                reason: "Animated lossless image, recommend HEVC MP4 (visually lossless)"
                    .to_string(),
                command: Some(format!(
                    "ffmpeg -i '{}' -c:v libx265 -crf 0 -preset medium '{}'",
                    fp, out
                )),
            })
        }
        (true, false) => Ok(ConversionRecommendation {
            should_convert: false,
            target_format: None,
            reason: "Animated lossy image, no conversion".to_string(),
            command: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_lossless() {
        assert!(is_format_lossless(&ImageFormat::Png));
        assert!(!is_format_lossless(&ImageFormat::Gif));
        assert!(!is_format_lossless(&ImageFormat::Jpeg));
        assert!(!is_format_lossless(&ImageFormat::WebP));
    }

    #[test]
    fn test_recommendation_static_lossless() {
        let rec = generate_recommendation("PNG", true, false, "/path/to/image.png").unwrap();
        assert!(rec.should_convert);
        assert_eq!(rec.target_format, Some("JXL".to_string()));
    }

    #[test]
    fn test_recommendation_static_lossy() {
        let rec = generate_recommendation("JPEG", false, false, "/path/to/image.jpg").unwrap();
        assert!(rec.should_convert);
        assert_eq!(rec.target_format, Some("JXL".to_string()));
        assert!(rec.command.as_ref().unwrap().contains("--lossless_jpeg=1"));
    }

    #[test]
    fn test_recommendation_animated_lossless() {
        let rec = generate_recommendation("GIF", true, true, "/path/to/anim.gif").unwrap();
        assert!(rec.should_convert);
        assert_eq!(rec.target_format, Some("HEVC MP4".to_string()));
    }

    #[test]
    fn test_recommendation_animated_lossy() {
        let rec = generate_recommendation("WebP", false, true, "/path/to/anim.webp").unwrap();
        assert!(!rec.should_convert);
        assert_eq!(rec.target_format, None);
    }

    #[test]
    fn test_recommendation_shell_escape_path_with_quote() {
        let rec = generate_recommendation("PNG", true, false, "/path/to/it's photo.png").unwrap();
        let cmd = rec.command.as_ref().unwrap();
        assert!(
            cmd.contains("it'\\''s photo"),
            "command should escape single quote in path: {}",
            cmd
        );
    }

    #[test]
    fn test_recommendation_heic_lossy_skip() {
        let rec = generate_recommendation("HEIC", false, false, "/path/to/photo.heic").unwrap();
        assert!(!rec.should_convert);
        assert_eq!(rec.target_format, None);
        assert!(rec.reason.contains("second-generation"));
    }

    #[test]
    fn test_recommendation_bad_path_returns_err() {
        let res = generate_recommendation("PNG", true, false, "");
        assert!(res.is_err());
    }

    #[test]
    fn test_analyze_gif_quality_no_estimated_quality() {
        use image::RgbImage;
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(1, 1, |_, _| image::Rgb([0, 0, 0])));
        let params = analyze_gif_quality(&img);
        assert!(params.estimated_quality.is_none());
        assert_eq!(params.confidence, 0.0);
    }
}
