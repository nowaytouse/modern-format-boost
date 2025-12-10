//! Conversion API Module
//! 
//! Pure conversion layer - transforms images based on detection results.
//! Takes DetectionResult as input and performs smart conversions.

use crate::detection_api::{CompressionType, DetectedFormat, DetectionResult, ImageType};
use crate::{ImgQualityError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Target format for conversion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetFormat {
    /// JPEG XL - modern lossless/lossy format
    JXL,
    /// AVIF - AV1 based image format
    AVIF,
    /// AV1 MP4 - for animated images, Q=100 visually lossless
    AV1MP4,
    /// Keep original format
    NoConversion,
}

/// Conversion strategy for different image types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionStrategy {
    /// Target format
    pub target: TargetFormat,
    /// Reason for this conversion choice
    pub reason: String,
    /// Command to execute
    pub command: String,
    /// Expected size reduction percentage
    pub expected_reduction: f32,
}

/// Conversion options
#[derive(Debug, Clone, Default)]
pub struct ConversionConfig {
    /// Output directory (None = same as input)
    pub output_dir: Option<PathBuf>,
    /// Force conversion even if already processed
    pub force: bool,
    /// Delete original after successful conversion
    pub delete_original: bool,
    /// Preserve file timestamps
    pub preserve_timestamps: bool,
    /// Preserve metadata (EXIF, XMP, etc.)
    pub preserve_metadata: bool,
}

/// Conversion result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionOutput {
    /// Original file path
    pub original_path: String,
    /// Output file path
    pub output_path: String,
    /// Whether conversion was skipped
    pub skipped: bool,
    /// Skip reason or success message
    pub message: String,
    /// Original file size
    pub original_size: u64,
    /// Output file size (if converted)
    pub output_size: Option<u64>,
    /// Size reduction percentage
    pub size_reduction: Option<f32>,
}

/// Determine optimal conversion strategy based on detection result
pub fn determine_strategy(detection: &DetectionResult) -> ConversionStrategy {
    match (&detection.image_type, &detection.compression, &detection.format) {
        // JPEG (static) -> JXL lossless transcode
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
        
        // Static lossless (PNG, GIF single frame, etc.) -> JXL
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
        
        // Animated lossless (GIF, APNG, animated WebP lossless) -> AV1 MP4 Q=100
        (ImageType::Animated, CompressionType::Lossless, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("mp4");
            let fps = detection.fps.unwrap_or(10.0);
            ConversionStrategy {
                target: TargetFormat::AV1MP4,
                reason: "Animated lossless image, recommend AV1 MP4 with CRF 0 (visually lossless)".to_string(),
                command: format!(
                    "ffmpeg -i '{}' -c:v libsvtav1 -crf 0 -preset 6 -r {} '{}'",
                    input_path,
                    fps,
                    output_path.display()
                ),
                expected_reduction: 30.0,
            }
        }
        
        // Animated lossy -> Skip (don't re-encode lossy animation)
        (ImageType::Animated, CompressionType::Lossy, _) => {
            ConversionStrategy {
                target: TargetFormat::NoConversion,
                reason: "Animated lossy image, skipping to avoid further quality loss".to_string(),
                command: String::new(),
                expected_reduction: 0.0,
            }
        }
        
        // Static lossy (non-JPEG) -> AVIF
        (ImageType::Static, CompressionType::Lossy, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("avif");
            let quality = detection.estimated_quality.unwrap_or(85);
            ConversionStrategy {
                target: TargetFormat::AVIF,
                reason: "Static lossy image (non-JPEG), recommend AVIF for better compression".to_string(),
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

/// Execute conversion based on strategy
pub fn execute_conversion(
    detection: &DetectionResult,
    strategy: &ConversionStrategy,
    config: &ConversionConfig,
) -> Result<ConversionOutput> {
    let input_path = Path::new(&detection.file_path);
    
    // Skip if no conversion needed
    if strategy.target == TargetFormat::NoConversion {
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
    
    // Determine output path
    let extension = match strategy.target {
        TargetFormat::JXL => "jxl",
        TargetFormat::AVIF => "avif",
        TargetFormat::AV1MP4 => "mp4",
        TargetFormat::NoConversion => return Err(ImgQualityError::ConversionError("No conversion".to_string())),
    };
    
    let output_path = if let Some(ref dir) = config.output_dir {
        dir.join(input_path.file_stem().unwrap()).with_extension(extension)
    } else {
        input_path.with_extension(extension)
    };
    
    // Check if output exists and not forcing
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
    
    // Build and execute command
    let result = match strategy.target {
        TargetFormat::JXL => convert_to_jxl(input_path, &output_path, &detection.format),
        TargetFormat::AVIF => convert_to_avif(input_path, &output_path, detection.estimated_quality),
        TargetFormat::AV1MP4 => convert_to_av1_mp4(input_path, &output_path, detection.fps),
        TargetFormat::NoConversion => unreachable!(),
    };
    
    if let Err(e) = result {
        return Err(ImgQualityError::ConversionError(e.to_string()));
    }
    
    // Get output file size
    let output_size = std::fs::metadata(&output_path).ok().map(|m| m.len());
    let size_reduction = output_size.map(|s| {
        100.0 * (1.0 - s as f32 / detection.file_size as f32)
    });
    
    // Preserve timestamps if requested
    if config.preserve_timestamps {
        preserve_timestamps(input_path, &output_path)?;
    }
    
    // Preserve metadata if requested
    if config.preserve_metadata {
        preserve_metadata(input_path, &output_path)?;
    }
    
    // Delete original if requested
    if config.delete_original {
        std::fs::remove_file(input_path)?;
    }
    
    Ok(ConversionOutput {
        original_path: detection.file_path.clone(),
        output_path: output_path.display().to_string(),
        skipped: false,
        message: format!("Conversion successful: size reduced {:.1}%", size_reduction.unwrap_or(0.0)),
        original_size: detection.file_size,
        output_size,
        size_reduction,
    })
}

/// Convert to JXL
fn convert_to_jxl(input: &Path, output: &Path, format: &DetectedFormat) -> Result<()> {
    let args = if *format == DetectedFormat::JPEG {
        // JPEG lossless transcode
        vec![
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--lossless_jpeg=1",
        ]
    } else {
        // Lossless modular encoding
        vec![
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "-d", "0.0",
            "-e", "7",  // cjxl v0.11+ 范围是 1-10，默认 7
        ]
    };
    
    let status = Command::new("cjxl")
        .args(&args)
        .output()?;
    
    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string()
        ));
    }
    
    Ok(())
}

/// Convert to AVIF
fn convert_to_avif(input: &Path, output: &Path, quality: Option<u8>) -> Result<()> {
    let q = quality.unwrap_or(85).to_string();
    
    let status = Command::new("avifenc")
        .args(&[
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "-q", &q,
        ])
        .output()?;
    
    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string()
        ));
    }
    
    Ok(())
}

/// Convert animated image to AV1 MP4 with CRF 0 (visually lossless)
/// 使用 SVT-AV1 编码器 (libsvtav1) - 比 libaom-av1 快 10-20 倍
fn convert_to_av1_mp4(input: &Path, output: &Path, fps: Option<f32>) -> Result<()> {
    let fps_str = fps.unwrap_or(10.0).to_string();
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let svt_params = format!("tune=0:film-grain=0:lp={}", max_threads);
    
    // SVT-AV1 with CRF 0 = 视觉无损最高质量
    let status = Command::new("ffmpeg")
        .args(&[
            "-y",
            "-threads", &max_threads.to_string(),
            "-i", input.to_str().unwrap(),
            "-c:v", "libsvtav1",  // 🔥 使用 SVT-AV1
            "-crf", "0",          // CRF 0 = 视觉无损最高质量
            "-preset", "6",       // 0-13, 6 是平衡点
            "-svtav1-params", &svt_params,
            "-r", &fps_str,
            "-pix_fmt", "yuv420p",
            output.to_str().unwrap(),
        ])
        .output()?;
    
    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string()
        ));
    }
    
    Ok(())
}

/// Preserve file timestamps (modification time, access time)
fn preserve_timestamps(source: &Path, dest: &Path) -> Result<()> {
    let status = Command::new("touch")
        .args(&["-r", source.to_str().unwrap(), dest.to_str().unwrap()])
        .output()?;
    
    if !status.status.success() {
        // Non-fatal, just log
        eprintln!("⚠️ Warning: Failed to preserve timestamps");
    }
    
    Ok(())
}

/// Preserve metadata using exiftool
fn preserve_metadata(source: &Path, dest: &Path) -> Result<()> {
    // Check if exiftool is available
    if which::which("exiftool").is_err() {
        return Ok(()); // Skip if not available
    }
    
    let status = Command::new("exiftool")
        .args(&[
            "-overwrite_original",
            "-TagsFromFile", source.to_str().unwrap(),
            "-All:All",
            dest.to_str().unwrap(),
        ])
        .output()?;
    
    if !status.status.success() {
        // Non-fatal, just log
        eprintln!("⚠️ Warning: Failed to preserve metadata");
    }
    
    Ok(())
}

/// High-level smart conversion function
pub fn smart_convert(path: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    use crate::detection_api::detect_image;
    
    // Step 1: Detect image properties
    let detection = detect_image(path)?;
    
    // Step 2: Determine strategy
    let strategy = determine_strategy(&detection);
    
    // Step 3: Execute conversion
    execute_conversion(&detection, &strategy, config)
}

/// Simple mode conversion - Always use JXL for static, AV1 MP4 for animated
/// 
/// Strategy:
/// - Any static image → JXL mathematical lossless
/// - Any animated image → AV1 MP4 CRF 0 (visually lossless)
pub fn simple_convert(path: &Path, output_dir: Option<&Path>) -> Result<ConversionOutput> {
    use crate::detection_api::detect_image;
    
    let detection = detect_image(path)?;
    let input_path = Path::new(&detection.file_path);
    
    // Determine output path
    let (extension, is_animated) = match detection.image_type {
        ImageType::Static => ("jxl", false),
        ImageType::Animated => ("mp4", true),
    };
    
    let output_path = if let Some(dir) = output_dir {
        std::fs::create_dir_all(dir)?;
        dir.join(input_path.file_stem().unwrap()).with_extension(extension)
    } else {
        input_path.with_extension(extension)
    };
    
    // Skip if output exists
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
    
    // Execute conversion
    let result = if is_animated {
        // Animated → AV1 MP4 CRF 0
        convert_to_av1_mp4(input_path, &output_path, detection.fps)
    } else {
        // Static → JXL lossless
        convert_to_jxl_lossless(input_path, &output_path, &detection.format)
    };
    
    if let Err(e) = result {
        return Err(ImgQualityError::ConversionError(e.to_string()));
    }
    
    // Get output size
    let output_size = std::fs::metadata(&output_path).ok().map(|m| m.len());
    let size_reduction = output_size.map(|s| {
        100.0 * (1.0 - s as f32 / detection.file_size as f32)
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

/// JXL lossless conversion (always mathematical lossless)
fn convert_to_jxl_lossless(input: &Path, output: &Path, format: &DetectedFormat) -> Result<()> {
    let args = if *format == DetectedFormat::JPEG {
        // JPEG: use lossless_jpeg transcode
        vec![
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--lossless_jpeg=1",
        ]
    } else {
        // Non-JPEG: use -d 0.0 for mathematical lossless
        // cjxl v0.11+: --modular=1 强制使用 modular 模式，-e 范围 1-10
        vec![
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "-d", "0.0",
            "--modular=1",
            "-e", "9",
        ]
    };
    
    let status = Command::new("cjxl")
        .args(&args)
        .output()?;
    
    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string()
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
        assert_eq!(strategy.target, TargetFormat::AV1MP4);
    }
}
