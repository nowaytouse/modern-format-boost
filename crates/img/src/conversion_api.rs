//! Conversion API Module
//!
//! Transforms images based on detection results.

use crate::Rational;
use crate::detection_api::{CompressionType, DetectedFormat, DetectionResult, ImageType};
use crate::{ImgQualityError, Result};
use bitflags::bitflags;
use foundation::ToolBuilder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetFormat {
    JXL,
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
        const ALLOW_EXPERT_OPTIONS = 1 << 13;
        const ARCHIVE_MODE = 1 << 14;
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
    fn size_policy(&self) -> foundation::exploration_policy::SizePolicy {
        foundation::exploration_policy::SizePolicy::strict_or_allow_growth(
            foundation::media_conversion_gate::effective_allow_size_tolerance(
                self.allow_size_tolerance(),
            ),
            foundation::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
        )
    }

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
    #[must_use]
    pub const fn allow_expert_options(&self) -> bool {
        self.flags.contains(ConfigFlags::ALLOW_EXPERT_OPTIONS)
    }
    #[must_use]
    pub const fn archive_mode(&self) -> bool {
        self.flags.contains(ConfigFlags::ARCHIVE_MODE)
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
        // JPEG lossless transcode to JXL preserves DCT coefficients (true transcoding)
        self.message.contains("transcoding") || self.message.contains("JPEG lossless")
    }
}

fn cleanup_output_file(path: &Path, context: &str) {
    foundation::media_conversion_gate::delivery_remove_file_or_audit(context, path);
}

/// Determine the optimal conversion strategy based on image detection results.
///
/// # Errors
///
/// Returns an error if:
/// - The estimated quality of a lossy image cannot be determined.
pub fn determine_strategy(detection: &DetectionResult) -> Result<ConversionStrategy> {
    if matches!(&detection.image_type, ImageType::Animated) {
        return Ok(ConversionStrategy {
            target: TargetFormat::NoConversion,
            reason: "Animated image: outside img static-conversion domain".to_string(),
            command: String::new(),
            expected_reduction: 0.0,
        });
    }

    if detection.format == DetectedFormat::JXL
        || (detection.format.is_modern_format()
            && detection.compression != CompressionType::Lossless)
    {
        return Ok(ConversionStrategy {
            target: TargetFormat::NoConversion,
            reason: format!(
                "Retaining modern format ({}) to avoid generational loss",
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
                expected_reduction: foundation::constants::EXPECTED_REDUCTION_JXL_LOSSLESS_JPEG,
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
                    foundation::constants::JXL_DEFAULT_EFFORT
                ),
                expected_reduction: foundation::constants::EXPECTED_REDUCTION_JXL_LOSSLESS_STATIC,
            })
        }

        (ImageType::Static, CompressionType::Lossy, _) => {
            let input_path = &detection.file_path;
            let output_path = Path::new(input_path).with_extension("JXL");

            Ok(ConversionStrategy {
                target: TargetFormat::JXL,
                reason: "Static legacy lossy image, encode as near-lossless JXL".to_string(),
                command: format!(
                    "cjxl '{}' '{}' -d {}",
                    input_path,
                    output_path.display(),
                    foundation::constants::JXL_ULTIMATE_DISTANCE
                ),
                expected_reduction: foundation::constants::EXPECTED_REDUCTION_JXL_LOSSY_STATIC,
            })
        }

        // Animated inputs return above; this arm keeps the tuple match
        // exhaustive without duplicating the animated strategy construction.
        (ImageType::Animated, _, _) => {
            unreachable!("animated strategy was handled before the static match")
        }

        // Unproven compression semantics must not unlock a lossy re-encode:
        // a possibly-lossless source could suffer a second-generation loss.
        // Same for jbrd JXL sources — they belong to the reversible-JPEG
        // route, not a lossy AVIF strategy.
        (ImageType::Static, CompressionType::Unknown | CompressionType::JpegReconstruction, _) => {
            Ok(ConversionStrategy {
                target: TargetFormat::NoConversion,
                reason: format!(
                    "Compression semantics unproven for {}; refusing lossy re-encode (fail-closed)",
                    detection.format.as_str()
                ),
                command: String::new(),
                expected_reduction: 0.0,
            })
        }
    }
}

fn validate_conversion_preflight(
    detection: &DetectionResult,
    strategy: &ConversionStrategy,
    config: &ConversionConfig,
    input_path: &Path,
) -> Result<Option<ConversionOutput>> {
    if strategy.target == TargetFormat::NoConversion {
        if matches!(&detection.image_type, ImageType::Animated) {
            return Ok(Some(ConversionOutput {
                original_path: detection.file_path.clone(),
                output_path: String::new(),
                skipped: false,
                ignored: true,
                message: strategy.reason.clone(),
                original_size: detection.file_size,
                output_size: None,
                size_reduction: None,
                blake3: None,
            }));
        }

        foundation::copy_on_skip_or_fail(
            input_path,
            config.output_dir.as_deref(),
            config.base_dir.as_deref(),
            false,
        )
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

        return Ok(Some(ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: detection.file_path.clone(),
            skipped: true,
            ignored: false,
            message: strategy.reason.clone(),
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        }));
    }

    if strategy.target == TargetFormat::JXL && detection.image_type != ImageType::Static {
        let reason = format!(
            "Non-static source refused for JXL output: {}",
            detection.file_path
        );
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "non_static_jxl_refused",
            std::path::Path::new(&detection.file_path),
            &reason,
        );
        return Ok(Some(ConversionOutput {
            original_path: detection.file_path.clone(),
            output_path: String::new(),
            skipped: false,
            ignored: true,
            message: reason,
            original_size: detection.file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        }));
    }

    Ok(None)
}

/// Execute the selected conversion strategy.
///
/// # Errors
/// Returns an error if the conversion process fails (e.g., tool execution error).
///
/// # Panics
/// Panics if an animated image state transition is detected within a static-only processing branch,
/// indicating a critical breach of upstream validation logic.
pub fn execute_conversion(
    detection: &DetectionResult,
    strategy: &ConversionStrategy,
    config: &ConversionConfig,
) -> Result<ConversionOutput> {
    let input_path = Path::new(&detection.file_path);

    if let Some(early_output) =
        validate_conversion_preflight(detection, strategy, config, input_path)?
    {
        return Ok(early_output);
    }

    let extension = match strategy.target {
        TargetFormat::JXL => "JXL",
        TargetFormat::NoConversion => {
            return Err(ImgQualityError::ConversionError(
                "No conversion".to_string(),
            ));
        }
    };

    let output_path = resolve_output_path(input_path, config.output_dir.as_deref(), extension)?;
    foundation::conversion::validate_output_path(&output_path, config.base_dir.as_deref())
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

    let temp_path = foundation::path_safety::isolated_temp_path_for_search(&output_path)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    let result = match strategy.target {
        TargetFormat::JXL => convert_to_jxl(
            input_path,
            &temp_path,
            &detection.format,
            detection.compression,
            config,
        ),
        TargetFormat::NoConversion => {
            return Err(ImgQualityError::ConversionError(
                "NoConversion should have been handled earlier".to_string(),
            ));
        }
    };

    if let Err(e) = result {
        cleanup_output_file(&temp_path, "temporary output after conversion failure");
        return Err(ImgQualityError::ConversionError(e.to_string()));
    }

    finalize_conversion_output(detection, config, input_path, &output_path, &temp_path)
}

fn finalize_conversion_output(
    detection: &DetectionResult,
    config: &ConversionConfig,
    input_path: &Path,
    output_path: &Path,
    temp_path: &Path,
) -> Result<ConversionOutput> {
    let committed = if detection.format == DetectedFormat::JPEG {
        foundation::conversion::commit_reconstructible_jxl_to_output_with_metadata(
            temp_path,
            output_path,
            config.force(),
            Some(input_path),
        )
    } else {
        foundation::conversion::commit_temp_to_output_with_metadata(
            temp_path,
            output_path,
            config.force(),
            Some(input_path),
        )
    }
    .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    if !committed {
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

    let output_size = Some(
        foundation::io_utils::metadata_with_retry(output_path)
            .map_err(|e| {
                ImgQualityError::ConversionError(format!(
                    "Failed to read committed output metadata for {}: {e}",
                    output_path.display()
                ))
            })?
            .len(),
    );
    let size_reduction = output_size.map(|s| {
        if detection.file_size == 0 {
            0.0
        } else {
            foundation::numeric_cast::f64_to_f32_lossy({
                let ratio = Rational::from((s, detection.file_size.max(1)));
                ratio.to_f64().mul_add(-100.0, 100.0)
            })
        }
    });

    // Compress mode uses the same pure-media policy as the encoder gate.
    if config.compress() {
        let input_payload = foundation::image::static_payload::measure(input_path)
            .map_err(|error| ImgQualityError::ConversionError(error.to_string()))?;
        let output_payload = foundation::image::static_payload::measure(output_path)
            .map_err(|error| ImgQualityError::ConversionError(error.to_string()))?;
        if !config.size_policy().fits(output_payload, input_payload) {
            cleanup_output_file(output_path, "oversized output in compress mode");
            foundation::copy_on_skip_or_fail(
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
                message: "Skipped: encoded image payload is outside the active size policy"
                    .to_string(),
                original_size: detection.file_size,
                output_size: None,
                size_reduction: None,
                blake3: None,
            });
        }
    }

    let jpeg_integrity = if detection.format == DetectedFormat::JPEG {
        match foundation::fast_img::verify_final_jxl_delivery_integrity(input_path, output_path) {
            Ok(integrity) => Some(integrity),
            Err(error) => {
                cleanup_output_file(output_path, "failed final JPEG reconstruction proof");
                return Err(ImgQualityError::ConversionError(format!(
                    "Final JPEG reconstruction proof failed for {}: {error}",
                    output_path.display()
                )));
            }
        }
    } else {
        None
    };

    if config.delete_original() {
        if let Some(integrity) = jpeg_integrity.as_ref() {
            foundation::fast_img::safe_delete_jpeg_source(input_path, output_path, integrity)?;
        } else {
            foundation::conversion::safe_delete_original(
                input_path,
                output_path,
                foundation::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
            )
            .map_err(|error| ImgQualityError::ConversionError(error.to_string()))?;
        }
    }

    let action = if detection.format == DetectedFormat::JPEG {
        "transcoding"
    } else {
        "encoding"
    };

    let reduction =
        foundation::numeric_cast::option_f32_strict(size_reduction, "size_reduction_report")
            .ok_or_else(|| {
                ImgQualityError::ConversionError("Missing size reduction report".to_string())
            })?;

    let ok = foundation::modern_ui::symbols::pick(
        foundation::modern_ui::symbols::SUCCESS,
        foundation::modern_ui::symbols::plain::SUCCESS,
    );
    let message = if reduction >= 0.0 {
        format!("{ok} JXL {action}: -{reduction:.1}%")
    } else {
        let out_val = i128::from(
            foundation::numeric_cast::option_u64_strict(output_size, "output_size_report")
                .ok_or_else(|| {
                    ImgQualityError::ConversionError("Missing output size report".to_string())
                })?,
        );
        let src_val = i128::from(detection.file_size);
        let diff_bytes = out_val - src_val;

        let size_diff =
            foundation::media_conversion_gate::size_delta_report_label(diff_bytes, output_path);
        format!("{ok} JXL {action}: {size_diff}")
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
    foundation::media_conversion_gate::canonicalize_for_tool_input(input)
}

/// Resolve output path: if `output_dir` is set, join dir + stem + extension; else same dir as input with new extension.
fn resolve_output_path(
    input: &Path,
    output_dir: Option<&Path>,
    extension: &str,
) -> Result<PathBuf> {
    let file_stem = foundation::media_conversion_gate::path_file_stem_os_or_delivery_err(
        input,
        "img_resolve_output",
    )
    .map_err(ImgQualityError::ConversionError)?;
    let output = match output_dir {
        Some(dir) => dir.join(file_stem).with_extension(extension),
        None => input.with_extension(extension),
    };
    foundation::conversion::validate_output_path(&output, None)
        .map_err(ImgQualityError::ConversionError)?;
    Ok(output)
}

fn convert_to_jxl(
    input: &Path,
    output: &Path,
    format: &DetectedFormat,
    compression: CompressionType,
    config: &ConversionConfig,
) -> Result<()> {
    let input_abs = canonicalize_input(input);
    let output_abs = resolve_output_absolute(output);
    let max_threads = foundation::thread_manager::get_balanced_thread_config(
        foundation::thread_manager::WorkloadType::Image,
    )
    .child_threads;

    let mut builder = foundation::CjxlBuilder::new();
    builder
        .input(&input_abs)
        .output(&output_abs)
        .effort(foundation::jxl_effort_policy::encoder_effort_for_mode(
            config.ultimate_mode(),
            config.archive_mode(),
        ))
        .threads(max_threads);

    if *format == DetectedFormat::JPEG {
        builder.lossless_jpeg(true);
    } else {
        match compression {
            CompressionType::Lossless => {
                builder.distance(0.0);
            }
            CompressionType::Lossy => {
                builder.distance(foundation::constants::JXL_ULTIMATE_DISTANCE);
            }
            CompressionType::Unknown | CompressionType::JpegReconstruction => {
                return Err(ImgQualityError::ConversionError(format!(
                    "JXL conversion requires proven source compression semantics: {}",
                    input.display()
                )));
            }
        }
    }

    if config.apple_compat() {
        builder.apple_compat(true);
    }

    let mut command = builder.build();
    let status = foundation::process_runner::run_command_with_liveness_timeout(
        &mut command,
        std::time::Duration::from_secs(120),
        foundation::process_runner::image_process_hard_timeout(),
        "JXL image conversion",
    )?;

    if !status.status.success() {
        return Err(ImgQualityError::ConversionError(
            String::from_utf8_lossy(&status.stderr).to_string(),
        ));
    }

    // Verify output file
    let output_size = foundation::io_utils::metadata_with_retry(output)
        .map_err(|e| ImgQualityError::ConversionError(format!("Failed to read JXL output: {e}")))?
        .len();
    if output_size == 0 {
        cleanup_output_file(output, "empty JXL output");
        return Err(ImgQualityError::ConversionError(
            "JXL output file is empty (encoding may have failed)".to_string(),
        ));
    }

    // Verify JXL file integrity
    if let Err(e) = foundation::jxl_utils::verify_jxl_health(output) {
        cleanup_output_file(output, "unhealthy JXL output");
        return Err(ImgQualityError::ConversionError(format!(
            "JXL health check failed: {e}"
        )));
    }
    if *format == DetectedFormat::JPEG
        && let Err(error) = foundation::fast_img::verify_jxl_roundtrip_integrity(input, output)
    {
        cleanup_output_file(output, "non-reconstructible JPEG JXL output");
        return Err(ImgQualityError::ConversionError(format!(
            "JPEG reconstruction proof failed before output commit: {error}"
        )));
    }

    // Compress mode: compare encoded image payload only.
    if config.compress() {
        let input_payload = foundation::image::static_payload::measure(input)
            .map_err(|error| ImgQualityError::ConversionError(error.to_string()))?;
        let output_payload = foundation::image::static_payload::jxl(output)
            .map_err(|error| ImgQualityError::ConversionError(error.to_string()))?;
        if !config.size_policy().fits(output_payload, input_payload) {
            cleanup_output_file(output, "non-compressing JXL output");
            return Err(ImgQualityError::ConversionError(format!(
                "Compress mode: JXL payload ({output_payload} bytes) is outside the active \
                 size policy for source payload ({input_payload} bytes)"
            )));
        }
    }

    Ok(())
}

/// Make output path absolute for tools that require it (e.g. avifenc).
fn resolve_output_absolute(output: &Path) -> PathBuf {
    foundation::media_conversion_gate::delivery_absolute_output_path_or_dot(
        output,
        "img resolve_output_absolute",
    )
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
            bit_depth: Some(8),
            has_alpha: false,
            file_size: 100_000,
            frame_count: Some(1),
            fps: None,
            duration: None,
            estimated_quality: Some(85),
            entropy: Some(7.0),
            precision: foundation::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::JXL);
        assert!(strategy.command.contains("--lossless_jpeg=1"));
        Ok(())
    }

    #[test]
    fn test_animated_image_uses_no_static_conversion() -> Result<()> {
        let detection = DetectionResult {
            file_path: "/test/animation.gif".to_string(),
            format: DetectedFormat::GIF,
            image_type: ImageType::Animated,
            compression: CompressionType::Lossless,
            width: 640,
            height: 480,
            bit_depth: Some(8),
            has_alpha: false,
            file_size: 500_000,
            frame_count: Some(30),
            fps: Some(10.0),
            duration: Some(3.0),
            estimated_quality: None,
            entropy: Some(5.0),
            precision: foundation::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::NoConversion);
        Ok(())
    }

    #[test]
    fn test_animated_modern_format_is_domain_ignore() -> Result<()> {
        let detection = DetectionResult {
            file_path: "/test/animation.jxl".to_string(),
            format: DetectedFormat::JXL,
            image_type: ImageType::Animated,
            compression: CompressionType::Lossless,
            width: 640,
            height: 480,
            bit_depth: Some(8),
            has_alpha: false,
            file_size: 500_000,
            frame_count: Some(30),
            fps: Some(10.0),
            duration: Some(3.0),
            estimated_quality: None,
            entropy: Some(5.0),
            precision: foundation::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::NoConversion);
        assert!(strategy.reason.contains("outside img"));

        let output = execute_conversion(&detection, &strategy, &ConversionConfig::default())?;
        assert!(output.ignored);
        assert!(!output.skipped);
        assert_eq!(output.output_path, "");
        Ok(())
    }

    #[test]
    fn test_modern_format_no_conversion() -> Result<()> {
        let detection = DetectionResult {
            file_path: "/test/image.jxl".to_string(),
            format: DetectedFormat::JXL,
            image_type: ImageType::Static,
            compression: CompressionType::Lossless,
            width: 1920,
            height: 1080,
            bit_depth: Some(10),
            has_alpha: true,
            file_size: 100_000,
            frame_count: None,
            fps: None,
            duration: None,
            estimated_quality: None,
            entropy: None,
            precision: foundation::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::NoConversion);
        assert!(strategy.reason.contains("generational loss"));
        Ok(())
    }

    #[test]
    fn test_modern_lossy_sources_are_retained() -> Result<()> {
        for format in [
            DetectedFormat::WebP,
            DetectedFormat::AVIF,
            DetectedFormat::HEIC,
            DetectedFormat::HEIF,
            DetectedFormat::JP2,
        ] {
            let detection = DetectionResult {
                file_path: format!("/test/image.{}", format.as_str().to_ascii_lowercase()),
                format,
                image_type: ImageType::Static,
                compression: CompressionType::Lossy,
                width: 100,
                height: 100,
                bit_depth: Some(8),
                has_alpha: false,
                file_size: 1000,
                frame_count: Some(1),
                fps: None,
                duration: None,
                estimated_quality: Some(90),
                entropy: None,
                precision: foundation::image_detection::PrecisionMetadata::default(),
            };

            let strategy = determine_strategy(&detection)?;
            assert_eq!(strategy.target, TargetFormat::NoConversion);
        }
        Ok(())
    }

    #[test]
    fn test_png_strategy() -> Result<()> {
        let detection = DetectionResult {
            file_path: "/test/image.png".to_string(),
            format: DetectedFormat::PNG,
            image_type: ImageType::Static,
            compression: CompressionType::Lossless,
            width: 100,
            height: 100,
            bit_depth: Some(8),
            has_alpha: false,
            file_size: 1000,
            frame_count: None,
            fps: None,
            duration: None,
            estimated_quality: None,
            entropy: None,
            precision: foundation::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::JXL);
        assert!(strategy.command.contains("-d 0.0"));
        Ok(())
    }

    #[test]
    fn test_lossy_legacy_strategy_stays_jxl() -> Result<()> {
        let detection = DetectionResult {
            file_path: "/test/image.tiff".to_string(),
            format: DetectedFormat::TIFF,
            image_type: ImageType::Static,
            compression: CompressionType::Lossy,
            width: 100,
            height: 100,
            bit_depth: Some(8),
            has_alpha: false,
            file_size: 1000,
            frame_count: None,
            fps: None,
            duration: None,
            estimated_quality: Some(90),
            entropy: None,
            precision: foundation::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::JXL);
        assert!(strategy.command.contains("cjxl"));
        assert!(strategy.command.contains("-d"));
        Ok(())
    }

    #[test]
    fn test_lossless_modern_source_upgrades_to_jxl() -> Result<()> {
        let detection = DetectionResult {
            file_path: "/test/image.avif".to_string(),
            format: DetectedFormat::AVIF,
            image_type: ImageType::Static,
            compression: CompressionType::Lossless,
            width: 100,
            height: 100,
            bit_depth: Some(10),
            has_alpha: false,
            file_size: 1000,
            frame_count: Some(1),
            fps: None,
            duration: None,
            estimated_quality: None,
            entropy: None,
            precision: foundation::image_detection::PrecisionMetadata::default(),
        };

        let strategy = determine_strategy(&detection)?;
        assert_eq!(strategy.target, TargetFormat::JXL);
        assert!(strategy.command.contains("-d 0.0"));
        Ok(())
    }
}
