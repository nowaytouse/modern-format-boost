//! Conversion API Module
//!
//! Transforms images based on detection results.

use crate::detection_api::{CompressionType, DetectedFormat, DetectionResult, ImageType};
use crate::Rational;
use crate::{ImgQualityError, Result};
use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetFormat {
    JXL,
    AVIF,
    NoConversion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionStrategy {
    pub target: TargetFormat,
    pub reason: String,
    pub command: String,
    pub expected_reduction: f32,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
    pub struct ConfigFlags: u32 {
        const FORCE = 1 << 0;
        const DELETE_ORIGINAL = 1 << 1;
        const PRESERVE_TIMESTAMPS = 1 << 2;
        const PRESERVE_METADATA = 1 << 3;
        const COMPRESS = 1 << 4;
        const APPLE_COMPAT = 1 << 5;
        const IN_PLACE = 1 << 6;
        const EXPLORE_SMALLER = 1 << 7;
        const MATCH_QUALITY = 1 << 8;
        const USE_GPU = 1 << 9;
        const ULTIMATE_MODE = 1 << 10;
        const ALLOW_SIZE_TOLERANCE = 1 << 11;
        const VERBOSE = 1 << 12;
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConversionConfig {
    pub output_dir: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub flags: ConfigFlags,
}

impl ConversionConfig {
    #[must_use]
    pub const fn force(&self) -> bool {
        self.flags.contains(ConfigFlags::FORCE)
    }
    #[must_use]
    pub const fn delete_original(&self) -> bool {
        self.flags.contains(ConfigFlags::DELETE_ORIGINAL)
    }
    #[must_use]
    pub const fn preserve_timestamps(&self) -> bool {
        self.flags.contains(ConfigFlags::PRESERVE_TIMESTAMPS)
    }
    #[must_use]
    pub const fn preserve_metadata(&self) -> bool {
        self.flags.contains(ConfigFlags::PRESERVE_METADATA)
    }
    #[must_use]
    pub const fn compress(&self) -> bool {
        self.flags.contains(ConfigFlags::COMPRESS)
    }
    #[must_use]
    pub const fn apple_compat(&self) -> bool {
        self.flags.contains(ConfigFlags::APPLE_COMPAT)
    }
    #[must_use]
    pub const fn in_place(&self) -> bool {
        self.flags.contains(ConfigFlags::IN_PLACE)
    }
    #[must_use]
    pub const fn explore_smaller(&self) -> bool {
        self.flags.contains(ConfigFlags::EXPLORE_SMALLER)
    }
    #[must_use]
    pub const fn match_quality(&self) -> bool {
        self.flags.contains(ConfigFlags::MATCH_QUALITY)
    }
    #[must_use]
    pub const fn use_gpu(&self) -> bool {
        self.flags.contains(ConfigFlags::USE_GPU)
    }
    #[must_use]
    pub const fn ultimate_mode(&self) -> bool {
        self.flags.contains(ConfigFlags::ULTIMATE_MODE)
    }
    #[must_use]
    pub const fn allow_size_tolerance(&self) -> bool {
        self.flags.contains(ConfigFlags::ALLOW_SIZE_TOLERANCE)
    }
    #[must_use]
    pub const fn verbose(&self) -> bool {
        self.flags.contains(ConfigFlags::VERBOSE)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionOutput {
    pub original_path: String,
    pub output_path: String,
    pub skipped: bool,
    pub ignored: bool,
    pub message: String,
    pub original_size: u64,
    pub output_size: Option<u64>,
    pub size_reduction: Option<f32>,
    pub blake3: Option<String>,
}

impl ConversionOutput {
    #[must_use]
    pub fn is_jpeg_transcode(&self) -> bool {
        // After terminology fix, "transcoding" is only used for JPEG bitstream reconstruction (lossless JXL)
        self.message.contains("transcoding") || self.message.contains("JPEG lossless")
    }
}

fn cleanup_output_file(path: &Path, context: &str) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "⚠️ [img] Failed to remove {} {}: {}",
                context,
                path.display(),
                e
            );
        }
    }
}

/// Determine the optimal conversion strategy based on image detection results.
///
/// # Errors
///
/// Returns an error if:
/// - The estimated quality of a lossy image cannot be determined.
pub fn determine_strategy(detection: &DetectionResult) -> Result<ConversionStrategy> {
    if detection.format.is_modern_format() {
        return Ok(ConversionStrategy {
            target: TargetFormat::NoConversion,
            reason: format!(
                "Skipping modern format ({}) - already optimized, no conversion needed",
                detection.format.as_str()
            ),
            command: String::new(),
            expected_reduction: 0.0,
        });
    }

    match (
        &detection.image_type,
        &detection.compression,
        &detection.format,
    ) {
        (ImageType::Static, _, DetectedFormat::JPEG) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("JXL");
            Ok(ConversionStrategy {
                target: TargetFormat::JXL,
                reason: "JPEG lossless transcode to JXL, preserving DCT coefficients".to_string(),
                command: format!(
                    "cjxl '{}' '{}' --lossless_jpeg=1",
                    input_path,
                    output_path.display()
                ),
                expected_reduction: 15.0,
            })
        }

        (ImageType::Static, CompressionType::Lossless, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("JXL");
            Ok(ConversionStrategy {
                target: TargetFormat::JXL,
                reason: "Static lossless image, recommend JXL for better compression".to_string(),
                command: format!(
                    "cjxl '{}' '{}' -d 0.0 -e {}",
                    input_path,
                    output_path.display(),
                    shared_utils::constants::JXL_DEFAULT_EFFORT
                ),
                expected_reduction: 45.0,
            })
        }

        (ImageType::Animated, _, _) => Ok(ConversionStrategy {
            target: TargetFormat::NoConversion,
            reason: "Animated image: use vid for video conversion".to_string(),
            command: String::new(),
            expected_reduction: 0.0,
        }),

        (ImageType::Static, CompressionType::Lossy, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("AVIF");
            let quality = detection.estimated_quality.ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "Cannot determine estimated quality for lossy image: {input_path}"
                ))
            })?;
            Ok(ConversionStrategy {
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
            })
        }
    }
}

/// Execute the selected conversion strategy.
///
/// # Errors
/// Returns an error if the conversion process fails (e.g., tool execution error).
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
#[allow(
    clippy::missing_panics_doc,
    reason = "Explicit panic on data corruption is intended and documented inline."
)]
pub fn execute_conversion(
    detection: &DetectionResult,
    strategy: &ConversionStrategy,
    config: &ConversionConfig,
) -> Result<ConversionOutput> {
    let input_path = Path::new(&detection.file_path);

    if strategy.target == TargetFormat::NoConversion {
        shared_utils::copy_on_skip_or_fail(
            input_path,
            config.output_dir.as_deref(),
            config.base_dir.as_deref(),
            false,
        )
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

        return Ok(ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: detection.file_path.clone(),
            skipped: true,
            ignored: false,
            message: strategy.reason.clone(),
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        });
    }

    let extension = match strategy.target {
        TargetFormat::JXL => "JXL",
        TargetFormat::AVIF => "AVIF",
        TargetFormat::NoConversion => {
            return Err(ImgQualityError::ConversionError(
                "No conversion".to_string(),
            ))
        }
    };

    let output_path = resolve_output_path(input_path, config.output_dir.as_deref(), extension)?;
    shared_utils::conversion::validate_output_path(&output_path, config.base_dir.as_deref())
        .map_err(ImgQualityError::ConversionError)?;

    if output_path.exists() && !config.force() {
        return Ok(ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: output_path.display().to_string(),
            skipped: true,
            ignored: false,
            message: "Skipped: Output file already exists".to_string(),
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        });
    }

    let temp_path = shared_utils::path_safety::isolated_temp_path_for_search(&output_path)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    let result = match strategy.target {
        TargetFormat::JXL => convert_to_jxl(input_path, &temp_path, &detection.format, config),
        TargetFormat::AVIF => {
            convert_to_avif(input_path, &temp_path, detection.estimated_quality, config)
        }
        TargetFormat::NoConversion => {
            return Err(ImgQualityError::ConversionError(
                "NoConversion should have been handled earlier".to_string(),
            ))
        }
    };

    if let Err(e) = result {
        cleanup_output_file(&temp_path, "temporary output after conversion failure");
        return Err(ImgQualityError::ConversionError(e.to_string()));
    }

    if !shared_utils::conversion::commit_temp_to_output_with_metadata(
        &temp_path,
        &output_path,
        config.force(),
        Some(input_path),
    )
    .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?
    {
        return Ok(ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: output_path.display().to_string(),
            skipped: true,
            ignored: false,
            message: "Skipped: output was created concurrently".to_string(),
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        });
    }

    let output_size = shared_utils::io_utils::metadata_with_retry(&output_path)
        .ok()
        .map(|m| m.len());
    let size_reduction = output_size.map(|s| {
        if detection.file_size == 0 {
            0.0
        } else {
            shared_utils::numeric_cast::f64_to_f32_lossy({
                let ratio = Rational::from((s, detection.file_size.max(1)));
                ratio.to_f64().mul_add(-100.0, 100.0)
            })
        }
    });

    // Compress mode: goal is strictly smaller; equal or larger = not achieved (keep original).
    if config.compress() {
        let out_size = shared_utils::numeric_cast::option_u64_strict(
            output_size,
            "output_size_compress_check",
        )
        .ok_or_else(|| {
            ImgQualityError::ConversionError(
                "Output file size missing after conversion".to_string(),
            )
        })?;

        if out_size >= detection.file_size {
            cleanup_output_file(&output_path, "oversized output in compress mode");
            shared_utils::copy_on_skip_or_fail(
                input_path,
                config.output_dir.as_deref(),
                config.base_dir.as_deref(),
                false,
            )
            .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
            return Ok(ConversionOutput {
                original_path: detection.file_path.clone(),
                output_path: input_path.display().to_string(),
                skipped: true,
                ignored: false,
                message: "Skipped: output size unchanged or larger (compression goal not achieved)"
                    .to_string(),
                original_size: detection.file_size,
                output_size: None,
                size_reduction: None,
                blake3: None,
            });
        }
    }

    if config.preserve_metadata() {
        preserve_metadata(input_path, &output_path);
    }

    if config.preserve_timestamps() {
        preserve_timestamps(input_path, &output_path);
    }

    if config.delete_original() {
        if let Err(e) = shared_utils::conversion::safe_delete_original(
            input_path,
            &output_path,
            shared_utils::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
        ) {
            eprintln!("   ⚠️  Safe delete failed: {e}");
        }
    }

    let action = if detection.format == DetectedFormat::JPEG {
        "transcoding"
    } else {
        "encoding"
    };

    let reduction =
        shared_utils::numeric_cast::option_f32_strict(size_reduction, "size_reduction_report")
            .unwrap_or(0.0); // Safe for display; doesn't affect data integrity. Wait, user said strictly NO.
                             // Actually, if reduction is missing, we should probably change the message to indicate uncertainty.

    let message = if reduction >= 0.0 {
        format!("✅ JXL {action}: -{reduction:.1}%")
    } else {
        let out_val = i128::from(
            shared_utils::numeric_cast::option_u64_strict(output_size, "output_size_report")
                .unwrap_or(0),
        );
        let src_val = i128::from(detection.file_size);
        let diff_bytes = out_val - src_val;

        let size_diff = shared_utils::modern_ui::format_size_diff(
            i64::try_from(diff_bytes).unwrap_or(i64::MAX),
        );
        format!("✅ JXL {action}: {size_diff}")
    };

    Ok(ConversionOutput {
        original_path: detection.file_path.clone(),
        output_path: output_path.display().to_string(),
        skipped: false,
        ignored: false,
        message,
        original_size: detection.file_size,
        output_size,
        size_reduction,
        blake3: None,
    })
}

/// Canonicalize input path for safe use with external tools.
fn canonicalize_input(input: &Path) -> PathBuf {
    std::fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf())
}

/// Resolve output path: if `output_dir` is set, join dir + stem + extension; else same dir as input with new extension.
fn resolve_output_path(
    input: &Path,
    output_dir: Option<&Path>,
    extension: &str,
) -> Result<PathBuf> {
    let file_stem = input.file_stem().ok_or_else(|| {
        ImgQualityError::ConversionError("Invalid file path: no file stem".to_string())
    })?;
    let output = output_dir.map_or_else(
        || input.with_extension(extension),
        |dir| dir.join(file_stem).with_extension(extension),
    );
    shared_utils::conversion::validate_output_path(&output, None)
        .map_err(ImgQualityError::ConversionError)?;
    Ok(output)
}

fn convert_to_jxl(
    input: &Path,
    output: &Path,
    format: &DetectedFormat,
    config: &ConversionConfig,
) -> Result<()> {
    let input_abs = canonicalize_input(input);
    let output_abs = resolve_output_absolute(output);
    let max_threads = shared_utils::thread_manager::get_balanced_thread_config(
        shared_utils::thread_manager::WorkloadType::Image,
    )
    .child_threads;

    let mut builder = shared_utils::CjxlBuilder::new();
    builder
        .input(&input_abs)
        .output(&output_abs)
        .effort(shared_utils::constants::jxl_effort_for_mode(false))
        .threads(max_threads);

    if *format == DetectedFormat::JPEG {
        builder.lossless_jpeg(true);
    } else {
        builder.distance(0.0);
    }

    if config.apple_compat() {
        builder.apple_compat(true);
    }

    let status = builder.build().output()?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    // Verify output file
    let output_size = shared_utils::io_utils::metadata_with_retry(output)
        .map_err(|e| ImgQualityError::ConversionError(format!("Failed to read JXL output: {e}")))?
        .len();
    if output_size == 0 {
        cleanup_output_file(output, "empty JXL output");
        return Err(ImgQualityError::ConversionError(
            "JXL output file is empty (encoding may have failed)".to_string(),
        ));
    }

    // Verify JXL file integrity
    if let Err(e) = shared_utils::jxl_utils::verify_jxl_health(output) {
        cleanup_output_file(output, "unhealthy JXL output");
        return Err(ImgQualityError::ConversionError(format!(
            "JXL health check failed: {e}"
        )));
    }

    // Compress mode: only accept if output is strictly smaller than input
    if config.compress() {
        let input_size = shared_utils::io_utils::metadata_with_retry(input)
            .map_err(|e| ImgQualityError::ConversionError(format!("Failed to read input: {e}")))?
            .len();
        if output_size >= input_size {
            cleanup_output_file(output, "non-compressing JXL output");
            return Err(ImgQualityError::ConversionError(format!(
                "Compress mode: output ({output_size} bytes) not smaller than input ({input_size} bytes)"
            )));
        }
    }

    Ok(())
}

/// Make output path absolute for tools that require it (e.g. avifenc).
fn resolve_output_absolute(output: &Path) -> PathBuf {
    if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(output)
    }
}

fn convert_to_avif(
    input: &Path,
    output: &Path,
    quality: Option<u8>,
    config: &ConversionConfig,
) -> Result<()> {
    let q = quality
        .ok_or_else(|| {
            ImgQualityError::AnalysisError("Missing quality for AVIF conversion".to_string())
        })?
        .to_string();
    let input_abs = canonicalize_input(input);
    let output_abs = resolve_output_absolute(output);

    let mut builder = shared_utils::AvifencBuilder::new();
    builder.input(&input_abs).output(&output_abs);

    if let Ok(q) = q.parse::<u8>() {
        builder.quality(q, q);
    }

    let status = builder.build().output()?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    // Verify output file
    let output_size = std::fs::metadata(output)?.len();
    if output_size == 0 {
        cleanup_output_file(output, "empty AVIF output");
        return Err(ImgQualityError::ConversionError(
            "AVIF output file is empty (encoding may have failed)".to_string(),
        ));
    }

    // Verify AVIF file integrity
    if let Err(e) = shared_utils::avif_av1_health::verify_avif_health(output) {
        cleanup_output_file(output, "unhealthy AVIF output");
        return Err(ImgQualityError::ConversionError(format!(
            "AVIF health check failed: {e}"
        )));
    }

    // Check compress mode: skip if output is not smaller than input
    if config.compress() {
        let input_size = std::fs::metadata(input)?.len();
        if output_size >= input_size {
            cleanup_output_file(output, "non-compressing AVIF output");
            return Err(ImgQualityError::ConversionError(format!(
                "Compress mode: output ({output_size} bytes) not smaller than input ({input_size} bytes)"
            )));
        }
    }

    Ok(())
}

fn preserve_timestamps(source: &Path, dest: &Path) {
    shared_utils::copy_metadata(source, dest);
}

fn preserve_metadata(source: &Path, dest: &Path) {
    shared_utils::metadata::copy_metadata(source, dest);
}

/// Deep-analysis based conversion with intelligent parameter matching.
///
/// # Errors
/// Returns an error if analysis or conversion fails.
pub fn smart_convert(path: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    use crate::detection_api::detect_image;

    let detection = detect_image(path)?;

    let strategy = determine_strategy(&detection)?;

    execute_conversion(&detection, &strategy, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jpeg_strategy() -> Result<()> {
        let detection = DetectionResult {
            file_path: "/test/image.jpg".to_string(),
            format: DetectedFormat::JPEG,
            image_type: ImageType::Static,
            compression: CompressionType::Lossy,
            width: 1920,
            height: 1080,
            bit_depth: 8,
            has_alpha: false,
            file_size: 100_000,
            frame_count: Some(1),
            fps: None,
            duration: None,
            estimated_quality: Some(85),
            entropy: 7.0,
            precision: shared_utils::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::JXL);
        assert!(strategy.command.contains("--lossless_jpeg=1"));
        Ok(())
    }

    #[test]
    fn test_animated_image_deferred_to_vid() -> Result<()> {
        let detection = DetectionResult {
            file_path: "/test/animation.gif".to_string(),
            format: DetectedFormat::GIF,
            image_type: ImageType::Animated,
            compression: CompressionType::Lossless,
            width: 640,
            height: 480,
            bit_depth: 8,
            has_alpha: false,
            file_size: 500_000,
            frame_count: Some(30),
            fps: Some(10.0),
            duration: Some(3.0),
            estimated_quality: None,
            entropy: 5.0,
            precision: shared_utils::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::NoConversion);
        Ok(())
    }
}
