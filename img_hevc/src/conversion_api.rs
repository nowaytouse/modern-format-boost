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
    HEVCMP4,
    NoConversion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionStrategy {
    pub target: TargetFormat,
    pub reason: String,
    pub command: String,
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
    if detection.format.is_modern_format() {
        return ConversionStrategy {
            target: TargetFormat::NoConversion,
            reason: format!(
                "Skipping modern format ({}) - already optimized, no conversion needed",
                detection.format.as_str()
            ),
            command: String::new(),
            expected_reduction: 0.0,
        };
    }

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
                command: format!(
                    "cjxl '{}' '{}' --lossless_jpeg=1",
                    input_path,
                    output_path.display()
                ),
                expected_reduction: 15.0,
            }
        }

        (ImageType::Static, CompressionType::Lossless, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("jxl");
            ConversionStrategy {
                target: TargetFormat::JXL,
                reason: "Static lossless image, recommend JXL for better compression".to_string(),
                command: format!(
                    "cjxl '{}' '{}' -d 0.0 -e 8",
                    input_path,
                    output_path.display()
                ),
                expected_reduction: 45.0,
            }
        }

        (ImageType::Animated, CompressionType::Lossless, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("mp4");
            let fps = detection.fps.unwrap_or(10.0);
            ConversionStrategy {
                target: TargetFormat::HEVCMP4,
                reason:
                    "Animated lossless image, recommend HEVC MP4 with CRF 0 (visually lossless)"
                        .to_string(),
                command: format!(
                    "ffmpeg -i '{}' -c:v libx265 -crf 0 -preset medium -tag:v hvc1 -r {} '{}'",
                    input_path,
                    fps,
                    output_path.display()
                ),
                expected_reduction: 30.0,
            }
        }

        (ImageType::Animated, CompressionType::Lossy, _) => ConversionStrategy {
            target: TargetFormat::NoConversion,
            reason: "Animated lossy image, skipping to avoid further quality loss".to_string(),
            command: String::new(),
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
                command: format!(
                    "avifenc '{}' '{}' -q {}",
                    input_path,
                    output_path.display(),
                    quality
                ),
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
        let _ = shared_utils::copy_on_skip_or_fail(
            input_path,
            config.output_dir.as_deref(),
            config.base_dir.as_deref(),
            false,
        );

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
        TargetFormat::HEVCMP4 => "mp4",
        TargetFormat::NoConversion => {
            return Err(ImgQualityError::ConversionError(
                "No conversion".to_string(),
            ))
        }
    };

    let file_stem = input_path.file_stem().ok_or_else(|| {
        ImgQualityError::ConversionError("Invalid file path: no file stem".to_string())
    })?;

    let output_path = if let Some(ref dir) = config.output_dir {
        dir.join(file_stem).with_extension(extension)
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
        TargetFormat::HEVCMP4 => convert_to_hevc_mp4(
            input_path,
            &output_path,
            detection.fps,
            detection.width,
            detection.height,
        ),
        TargetFormat::NoConversion => unreachable!(),
    };

    if let Err(e) = result {
        return Err(ImgQualityError::ConversionError(e.to_string()));
    }

    let output_size = std::fs::metadata(&output_path).ok().map(|m| m.len());
    let size_reduction = output_size.map(|s| 100.0 * (1.0 - s as f32 / detection.file_size as f32));


    if config.preserve_metadata {
        preserve_metadata(input_path, &output_path)?;
    }

    if config.preserve_timestamps {
        preserve_timestamps(input_path, &output_path)?;
    }

    if config.delete_original {
        if let Err(e) =
            shared_utils::conversion::safe_delete_original(input_path, &output_path, 100)
        {
            eprintln!("   ⚠️  Safe delete failed: {}", e);
        }
    }

    Ok(ConversionOutput {
        original_path: detection.file_path.clone(),
        output_path: output_path.display().to_string(),
        skipped: false,
        message: format!(
            "Conversion successful: size reduced {:.1}%",
            size_reduction.unwrap_or(0.0)
        ),
        original_size: detection.file_size,
        output_size,
        size_reduction,
    })
}

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        ImgQualityError::ConversionError(format!("Invalid UTF-8 in path: {:?}", path))
    })
}

fn convert_to_jxl(input: &Path, output: &Path, format: &DetectedFormat) -> Result<()> {
    let input_abs = std::fs::canonicalize(input).unwrap_or(input.to_path_buf());
    let input_str = path_to_str(&input_abs)?;
    let output_str = path_to_str(output)?;

    let args = if *format == DetectedFormat::JPEG {
        vec!["--lossless_jpeg=1", "--", input_str, output_str]
    } else {
        vec!["-d", "0.0", "-e", "7", "--", input_str, output_str]
    };

    let status = Command::new("cjxl").args(&args).output()?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    Ok(())
}

fn convert_to_avif(input: &Path, output: &Path, quality: Option<u8>) -> Result<()> {
    let q = quality.unwrap_or(85).to_string();

    let input_abs = std::fs::canonicalize(input).unwrap_or(input.to_path_buf());
    let output_abs = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(output)
    };

    let input_str = path_to_str(&input_abs)?;
    let output_str = path_to_str(&output_abs)?;

    let status = Command::new("avifenc")
        .args([input_str, output_str, "-q", &q])
        .output()?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    Ok(())
}

fn convert_to_hevc_mp4(
    input: &Path,
    output: &Path,
    fps: Option<f32>,
    width: u32,
    height: u32,
) -> Result<()> {
    use shared_utils::ffmpeg_process::FfmpegProcess;

    let fps_str = fps.unwrap_or(10.0).to_string();

    let vf_args = build_even_dimension_filter(width, height);

    let max_threads = shared_utils::thread_manager::get_ffmpeg_threads();
    let x265_params = format!("log-level=error:pools={}", max_threads);

    let input_abs = std::fs::canonicalize(input).unwrap_or(input.to_path_buf());

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads")
        .arg(max_threads.to_string())
        .arg("-i")
        .arg(shared_utils::safe_path_arg(&input_abs).as_ref())
        .arg("-c:v")
        .arg("libx265")
        .arg("-crf")
        .arg("0")
        .arg("-preset")
        .arg("medium")
        .arg("-tag:v")
        .arg("hvc1")
        .arg("-x265-params")
        .arg(&x265_params)
        .arg("-r")
        .arg(&fps_str);

    if !vf_args.is_empty() {
        cmd.arg("-vf").arg(&vf_args);
    }
    cmd.arg("-pix_fmt").arg("yuv420p");

    let output_abs = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(output)
    };
    cmd.arg(&output_abs);

    let process = FfmpegProcess::spawn(&mut cmd)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    let (status, stderr) = process
        .wait_with_output()
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    if !status.success() {
        return Err(ImgQualityError::ConversionError(stderr));
    }

    Ok(())
}

fn build_even_dimension_filter(width: u32, height: u32) -> String {
    let need_pad = !width.is_multiple_of(2) || !height.is_multiple_of(2);
    if need_pad {
        let new_width = if !width.is_multiple_of(2) {
            width + 1
        } else {
            width
        };
        let new_height = if !height.is_multiple_of(2) {
            height + 1
        } else {
            height
        };
        format!("pad={}:{}:0:0:black", new_width, new_height)
    } else {
        String::new()
    }
}

fn preserve_timestamps(source: &Path, dest: &Path) -> Result<()> {
    let source_str = path_to_str(source)?;
    let dest_str = path_to_str(dest)?;

    let status = Command::new("touch")
        .args(["-r", source_str, dest_str])
        .output()?;

    if !status.status.success() {
        eprintln!("⚠️ Warning: Failed to preserve timestamps");
    }

    Ok(())
}

fn preserve_metadata(source: &Path, dest: &Path) -> Result<()> {
    shared_utils::metadata::copy_metadata(source, dest);
    Ok(())
}

pub fn smart_convert(path: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    use crate::detection_api::detect_image;

    let detection = detect_image(path)?;

    let strategy = determine_strategy(&detection);

    execute_conversion(&detection, &strategy, config)
}

pub fn simple_convert(path: &Path, output_dir: Option<&Path>) -> Result<ConversionOutput> {
    use crate::detection_api::detect_image;

    let detection = detect_image(path)?;
    let input_path = Path::new(&detection.file_path);

    let (extension, is_animated) = match detection.image_type {
        ImageType::Static => ("jxl", false),
        ImageType::Animated => ("mp4", true),
    };

    let file_stem = input_path.file_stem().ok_or_else(|| {
        ImgQualityError::ConversionError("Invalid file path: no file stem".to_string())
    })?;

    let output_path = if let Some(dir) = output_dir {
        std::fs::create_dir_all(dir)?;
        dir.join(file_stem).with_extension(extension)
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
        convert_to_hevc_mp4(
            input_path,
            &output_path,
            detection.fps,
            detection.width,
            detection.height,
        )
    } else {
        convert_to_jxl_lossless(input_path, &output_path, &detection.format)
    };

    if let Err(e) = result {
        return Err(ImgQualityError::ConversionError(e.to_string()));
    }

    let output_size = std::fs::metadata(&output_path).ok().map(|m| m.len());
    let size_reduction = output_size.map(|s| 100.0 * (1.0 - s as f32 / detection.file_size as f32));

    Ok(ConversionOutput {
        original_path: detection.file_path.clone(),
        output_path: output_path.display().to_string(),
        skipped: false,
        message: if is_animated {
            "Animated → HEVC MP4 (high quality)".to_string()
        } else {
            "Static → JXL (mathematical lossless)".to_string()
        },
        original_size: detection.file_size,
        output_size,
        size_reduction,
    })
}

fn convert_to_jxl_lossless(input: &Path, output: &Path, format: &DetectedFormat) -> Result<()> {
    let input_abs = std::fs::canonicalize(input).unwrap_or(input.to_path_buf());
    let input_str = path_to_str(&input_abs)?;
    let output_str = path_to_str(output)?;

    let args = if *format == DetectedFormat::JPEG {
        vec!["--lossless_jpeg=1", "--", input_str, output_str]
    } else {
        vec![
            "-d",
            "0.0",
            "--modular=1",
            "-e",
            "9",
            "--",
            input_str,
            output_str,
        ]
    };

    let status = Command::new("cjxl").args(&args).output()?;

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
        assert!(strategy.command.contains("--lossless_jpeg=1"));
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
        assert_eq!(strategy.target, TargetFormat::HEVCMP4);
    }
}
