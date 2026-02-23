//! Conversion API Module
//!
//! Pure conversion layer - transforms images based on detection results.
//! Takes DetectionResult as input and performs smart conversions.

use crate::detection_api::{CompressionType, DetectedFormat, DetectionResult, ImageType};
use crate::{ImgQualityError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetFormat {
    JXL,
    AVIF,
    AV1MP4,
    NoConversion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionStrategy {
    pub target: TargetFormat,
    pub reason: String,
    /// Illustrative command (matches actual cjxl/avifenc/ffmpeg args where possible). None when no conversion.
    pub command: Option<String>,
    pub expected_reduction: f32,
}

#[derive(Debug, Clone, Default)]
pub struct ConversionConfig {
    pub output_dir: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub force: bool,
    pub delete_original: bool,
    pub preserve_timestamps: bool,
    pub preserve_metadata: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionOutput {
    pub original_path: String,
    pub output_path: String,
    pub skipped: bool,
    pub message: String,
    pub original_size: u64,
    pub output_size: Option<u64>,
    pub size_reduction: Option<f32>,
}

pub fn determine_strategy(detection: &DetectionResult) -> ConversionStrategy {
    match (
        &detection.image_type,
        &detection.compression,
        &detection.format,
    ) {
        (ImageType::Static, _, DetectedFormat::JPEG) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("jxl");
            ConversionStrategy {
                target: TargetFormat::JXL,
                reason: "JPEG lossless transcode to JXL, preserving DCT coefficients".to_string(),
                command: Some(format!(
                    "cjxl --lossless_jpeg=1 -- '{}' '{}'",
                    input_path,
                    output_path.display()
                )),
                expected_reduction: 15.0,
            }
        }

        (ImageType::Static, CompressionType::Lossless, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("jxl");
            ConversionStrategy {
                target: TargetFormat::JXL,
                reason: "Static lossless image, recommend JXL for better compression".to_string(),
                command: Some(format!(
                    "cjxl -d 0.0 -e 7 -- '{}' '{}'",
                    input_path,
                    output_path.display()
                )),
                expected_reduction: 45.0,
            }
        }

        (ImageType::Animated, CompressionType::Lossless, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("mp4");
            let fps = detection.fps.unwrap_or(10.0);
            ConversionStrategy {
                target: TargetFormat::AV1MP4,
                reason: "Animated lossless image, recommend AV1 MP4 with CRF 0 (visually lossless)"
                    .to_string(),
                command: Some(format!(
                    "ffmpeg -y -i '{}' -c:v libsvtav1 -crf 0 -preset 6 -r {} -pix_fmt yuv420p '{}'",
                    input_path,
                    fps,
                    output_path.display()
                )),
                expected_reduction: 30.0,
            }
        }

        (ImageType::Animated, CompressionType::Lossy, _) => ConversionStrategy {
            target: TargetFormat::NoConversion,
            reason: "Animated lossy image, skipping to avoid further quality loss".to_string(),
            command: None,
            expected_reduction: 0.0,
        },

        (ImageType::Static, CompressionType::Lossy, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("avif");
            let quality = detection.estimated_quality.unwrap_or(85);
            ConversionStrategy {
                target: TargetFormat::AVIF,
                reason: "Static lossy image (non-JPEG), recommend AVIF for better compression"
                    .to_string(),
                command: Some(format!(
                    "avifenc '{}' '{}' -q {}",
                    input_path,
                    output_path.display(),
                    quality
                )),
                expected_reduction: 25.0,
            }
        }
    }
}

pub fn execute_conversion(
    detection: &DetectionResult,
    strategy: &ConversionStrategy,
    config: &ConversionConfig,
) -> Result<ConversionOutput> {
    let input_path = Path::new(&detection.file_path);

    if strategy.target == TargetFormat::NoConversion {
        if let Some(ref out_dir) = config.output_dir {
            let _ = shared_utils::copy_on_skip_or_fail(
                input_path,
                Some(out_dir),
                config.base_dir.as_deref(),
                false,
            );
        }

        return Ok(ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: detection.file_path.clone(),
            skipped: true,
            message: strategy.reason.clone(),
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
        });
    }

    let extension = match strategy.target {
        TargetFormat::JXL => "jxl",
        TargetFormat::AVIF => "avif",
        TargetFormat::AV1MP4 => "mp4",
        TargetFormat::NoConversion => unreachable!("NoConversion handled by early return above"),
    };

    let stem = input_path.file_stem().ok_or_else(|| {
        ImgQualityError::ConversionError(format!(
            "Invalid input path (no file stem): {}",
            input_path.display()
        ))
    })?;
    let output_path = if let Some(ref dir) = config.output_dir {
        dir.join(stem).with_extension(extension)
    } else {
        input_path.with_extension(extension)
    };

    if output_path.exists() && !config.force {
        return Ok(ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: output_path.display().to_string(),
            skipped: true,
            message: "Skipped: Output file already exists".to_string(),
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
        });
    }

    let result = match strategy.target {
        TargetFormat::JXL => convert_to_jxl(input_path, &output_path, &detection.format),
        TargetFormat::AVIF => {
            convert_to_avif(input_path, &output_path, detection.estimated_quality)
        }
        TargetFormat::AV1MP4 => convert_to_av1_mp4(input_path, &output_path, detection.fps),
        TargetFormat::NoConversion => unreachable!("handled above"),
    };

    if let Err(e) = result {
        let _ = std::fs::remove_file(&output_path);
        return Err(ImgQualityError::ConversionError(e.to_string()));
    }

    let output_size = std::fs::metadata(&output_path).ok().map(|m| m.len());
    let size_reduction = output_size.map(|s| {
        if detection.file_size == 0 {
            0.0
        } else {
            100.0 * (1.0 - s as f32 / detection.file_size as f32)
        }
    });

    if config.preserve_metadata || config.preserve_timestamps {
        shared_utils::copy_metadata(input_path, &output_path);
    }

    if config.delete_original {
        if let Err(e) =
            shared_utils::conversion::safe_delete_original(input_path, &output_path, 100)
        {
            eprintln!("   ⚠️  Safe delete failed: {}", e);
        }
    }

    let reduction = size_reduction.unwrap_or(0.0);
    let message = if reduction >= 0.0 {
        format!("Conversion successful: size reduced {:.1}%", reduction)
    } else {
        format!("Conversion successful: size increased {:.1}%", -reduction)
    };

    Ok(ConversionOutput {
        original_path: detection.file_path.clone(),
        output_path: output_path.display().to_string(),
        skipped: false,
        message,
        original_size: detection.file_size,
        output_size,
        size_reduction,
    })
}

/// Used by execute_conversion (effort 7, no --modular). For simple_convert use convert_to_jxl_lossless.
fn convert_to_jxl(input: &Path, output: &Path, format: &DetectedFormat) -> Result<()> {
    let mut cmd = Command::new("cjxl");
    if *format == DetectedFormat::JPEG {
        cmd.args(["--lossless_jpeg=1", "--"]);
    } else {
        cmd.args(["-d", "0.0", "-e", "7", "--"]);
    }
    let status = cmd
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(output).as_ref())
        .output()?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    Ok(())
}

fn convert_to_avif(input: &Path, output: &Path, quality: Option<u8>) -> Result<()> {
    let q = quality.unwrap_or(85).to_string();

    let status = Command::new("avifenc")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(output).as_ref())
        .args(["-q", &q])
        .output()?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    Ok(())
}

fn convert_to_av1_mp4(input: &Path, output: &Path, fps: Option<f32>) -> Result<()> {
    let fps_str = fps.unwrap_or(10.0).to_string();
    let max_threads = shared_utils::thread_manager::get_ffmpeg_threads();
    let svt_params = format!("tune=0:film-grain=0:lp={}", max_threads);

    let safe_input = shared_utils::safe_path_arg(input);
    let safe_output = shared_utils::safe_path_arg(output);
    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-threads")
        .arg(max_threads.to_string())
        .arg("-i")
        .arg(safe_input.as_ref())
        .args([
            "-c:v",
            "libsvtav1",
            "-crf",
            "0",
            "-preset",
            "6",
            "-svtav1-params",
            &svt_params,
            "-r",
            &fps_str,
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(safe_output.as_ref())
        .output()?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    Ok(())
}

pub fn smart_convert(path: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    use crate::detection_api::detect_image;

    let detection = detect_image(path)?;

    let strategy = determine_strategy(&detection);

    execute_conversion(&detection, &strategy, config)
}

/// Simple conversion: respects compression type to avoid re-encoding lossy as lossless (second-generation loss).
pub fn simple_convert(path: &Path, output_dir: Option<&Path>) -> Result<ConversionOutput> {
    use crate::detection_api::detect_image;

    let detection = detect_image(path)?;
    let input_path = Path::new(&detection.file_path);

    let (extension, is_animated) = match (&detection.image_type, &detection.compression) {
        (ImageType::Static, CompressionType::Lossless) => ("jxl", false),
        (ImageType::Static, CompressionType::Lossy) => {
            return Ok(ConversionOutput {
                original_path: detection.file_path.clone(),
                output_path: detection.file_path.clone(),
                skipped: true,
                message: "Static lossy image: skipping to avoid second-generation loss".to_string(),
                original_size: detection.file_size,
                output_size: None,
                size_reduction: None,
            });
        }
        (ImageType::Animated, CompressionType::Lossless) => ("mp4", true),
        (ImageType::Animated, CompressionType::Lossy) => {
            return Ok(ConversionOutput {
                original_path: detection.file_path.clone(),
                output_path: detection.file_path.clone(),
                skipped: true,
                message: "Animated lossy image: skipping to avoid further quality loss".to_string(),
                original_size: detection.file_size,
                output_size: None,
                size_reduction: None,
            });
        }
    };

    let stem = input_path.file_stem().ok_or_else(|| {
        ImgQualityError::ConversionError(format!(
            "Invalid input path (no file stem): {}",
            input_path.display()
        ))
    })?;
    let output_path = if let Some(dir) = output_dir {
        std::fs::create_dir_all(dir)?;
        dir.join(stem).with_extension(extension)
    } else {
        input_path.with_extension(extension)
    };

    if output_path.exists() {
        return Ok(ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: output_path.display().to_string(),
            skipped: true,
            message: "Output file already exists".to_string(),
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
        });
    }

    let result = if is_animated {
        convert_to_av1_mp4(input_path, &output_path, detection.fps)
    } else {
        convert_to_jxl_lossless(input_path, &output_path, &detection.format)
    };

    if let Err(e) = result {
        let _ = std::fs::remove_file(&output_path);
        return Err(ImgQualityError::ConversionError(e.to_string()));
    }

    let output_size = std::fs::metadata(&output_path).ok().map(|m| m.len());
    let size_reduction = output_size.map(|s| {
        if detection.file_size == 0 {
            0.0
        } else {
            100.0 * (1.0 - s as f32 / detection.file_size as f32)
        }
    });

    Ok(ConversionOutput {
        original_path: detection.file_path.clone(),
        output_path: output_path.display().to_string(),
        skipped: false,
        message: if is_animated {
            "Animated → AV1 MP4 (visually lossless)".to_string()
        } else {
            "Static → JXL (mathematical lossless)".to_string()
        },
        original_size: detection.file_size,
        output_size,
        size_reduction,
    })
}

/// Used by simple_convert: effort 9 and --modular for better compression. execute_conversion uses convert_to_jxl (effort 7).
fn convert_to_jxl_lossless(input: &Path, output: &Path, format: &DetectedFormat) -> Result<()> {
    let mut cmd = Command::new("cjxl");
    if *format == DetectedFormat::JPEG {
        cmd.args(["--lossless_jpeg=1", "--"]);
    } else {
        cmd.args(["-d", "0.0", "--modular=1", "-e", "9", "--"]);
    }
    let status = cmd
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(output).as_ref())
        .output()?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jpeg_strategy() {
        let detection = DetectionResult {
            file_path: "/test/image.jpg".to_string(),
            format: DetectedFormat::JPEG,
            image_type: ImageType::Static,
            compression: CompressionType::Lossy,
            width: 1920,
            height: 1080,
            bit_depth: 8,
            has_alpha: false,
            file_size: 100000,
            frame_count: 1,
            fps: None,
            duration: None,
            estimated_quality: Some(85),
            entropy: 7.0,
        };

        let strategy = determine_strategy(&detection);
        assert_eq!(strategy.target, TargetFormat::JXL);
        assert!(strategy.command.as_ref().map_or(false, |c| c.contains("--lossless_jpeg=1")));
    }

    #[test]
    fn test_gif_animated_strategy() {
        let detection = DetectionResult {
            file_path: "/test/animation.gif".to_string(),
            format: DetectedFormat::GIF,
            image_type: ImageType::Animated,
            compression: CompressionType::Lossless,
            width: 640,
            height: 480,
            bit_depth: 8,
            has_alpha: false,
            file_size: 500000,
            frame_count: 30,
            fps: Some(10.0),
            duration: Some(3.0),
            estimated_quality: None,
            entropy: 5.0,
        };

        let strategy = determine_strategy(&detection);
        assert_eq!(strategy.target, TargetFormat::AV1MP4);
    }

    #[test]
    fn test_no_conversion_has_no_command() {
        let detection = DetectionResult {
            file_path: "/test/anim.webp".to_string(),
            format: DetectedFormat::WebP,
            image_type: ImageType::Animated,
            compression: CompressionType::Lossy,
            width: 100,
            height: 100,
            bit_depth: 8,
            has_alpha: false,
            file_size: 5000,
            frame_count: 10,
            fps: Some(10.0),
            duration: Some(1.0),
            estimated_quality: Some(80),
            entropy: 5.0,
        };
        let strategy = determine_strategy(&detection);
        assert_eq!(strategy.target, TargetFormat::NoConversion);
        assert!(strategy.command.is_none());
    }

    #[test]
    fn test_execute_conversion_skips_when_output_exists() {
        let temp = std::env::temp_dir().join("img_av1_conv_test");
        let _ = std::fs::create_dir_all(&temp);
        let output_path = temp.join("input.jxl");
        let _ = std::fs::write(&output_path, b"existing");
        let detection = DetectionResult {
            file_path: temp.join("input.png").display().to_string(),
            format: DetectedFormat::PNG,
            image_type: ImageType::Static,
            compression: CompressionType::Lossless,
            width: 10,
            height: 10,
            bit_depth: 8,
            has_alpha: false,
            file_size: 100,
            frame_count: 1,
            fps: None,
            duration: None,
            estimated_quality: None,
            entropy: 4.0,
        };
        let strategy = determine_strategy(&detection);
        let config = ConversionConfig {
            output_dir: Some(temp.clone()),
            ..Default::default()
        };
        let out = execute_conversion(&detection, &strategy, &config).unwrap();
        assert!(out.skipped);
        assert!(out.message.contains("already exists"));
        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_simple_convert_skips_static_lossy() {
        let detection = DetectionResult {
            file_path: "/any/lossy.webp".to_string(),
            format: DetectedFormat::WebP,
            image_type: ImageType::Static,
            compression: CompressionType::Lossy,
            width: 100,
            height: 100,
            bit_depth: 8,
            has_alpha: false,
            file_size: 1000,
            frame_count: 1,
            fps: None,
            duration: None,
            estimated_quality: Some(80),
            entropy: 5.0,
        };
        let strategy = determine_strategy(&detection);
        assert_eq!(strategy.target, TargetFormat::AVIF);
        let rec = ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: detection.file_path.clone(),
            skipped: true,
            message: "Static lossy image: skipping to avoid second-generation loss".to_string(),
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
        };
        assert!(rec.skipped);
        assert!(rec.message.contains("lossy"));
    }
}
