//! Lossless Converter Module
//!
//! Provides conversion API for verified lossless/lossy images
//! Uses `shared_utils` for common functionality (anti-duplicate, `ConversionResult`, etc.)

use crate::{ImgQualityError, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

pub use shared_utils::conversion::{
    check_size_tolerance, clear_processed_list, finalize_conversion, format_size_change,
    is_already_processed, load_processed_list, mark_as_processed, save_processed_list,
    ConversionResult, ConvertOptions,
};

fn copy_original_on_skip(input: &Path, options: &ConvertOptions) -> Option<std::path::PathBuf> {
    shared_utils::copy_on_skip_or_fail(
        input,
        options.output_dir.as_deref(),
        options.base_dir.as_deref(),
        options.verbose,
    )
    .unwrap_or_default()
}

fn cleanup_temp_output(temp_output: &Path, input: &Path) {
    if let Err(e) = fs::remove_file(temp_output) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "⚠️ [img-av1] Failed to remove temporary output {} for {}: {}",
                temp_output.display(),
                input.display(),
                e
            );
        }
    }
}

/// Finalize conversion with size check and metadata preservation.
/// Common pattern: commit temp → check size → finalize.
/// Returns `ConversionResult` on success or error.
fn finalize_with_size_check(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    output_size: u64,
    options: &ConvertOptions,
    format_label: &str,
    extra_info: Option<String>,
) -> Result<ConversionResult> {
    // Commit temp file to final output WITH METADATA PRESERVATION
    if !shared_utils::conversion::commit_temp_to_output_with_metadata(
        temp_output,
        output,
        options.force,
        Some(input),
    )? {
        return Ok(ConversionResult::skipped_exists(input, output));
    }

    // Check size tolerance (compress mode, oversized check)
    if let Some(skipped) = check_size_tolerance(
        input,
        output,
        input_size,
        output_size,
        options,
        format_label,
    ) {
        return Ok(skipped);
    }

    // Finalize with metadata preservation
    finalize_conversion(
        input,
        output,
        input_size,
        format_label,
        extra_info.as_deref(),
        options,
    )
    .map_err(ImgQualityError::IoError)
}

/// Convert to JXL using specific distance.
///
/// # Errors
/// Returns an error if cjxl execution fails.
pub fn convert_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    distance: f32,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();

    if let Some(ext) = input.extension() {
        if ext.to_string_lossy().to_lowercase() == "png"
            && input_size < crate::constants::SMALL_PNG_THRESHOLD_BYTES
        {
            if options.verbose {
                eprintln!("⏭️  Skipped small PNG (< 500KB): {}", input.display());
            }
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            return Ok(ConversionResult::skipped_custom(
                input,
                input_size,
                "PNG",
                "Size < 500KB threshold",
            ));
        }
    }
    let output = get_output_path(input, "jxl", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let (actual_input, _temp_file_guard) = prepare_input_for_cjxl(input, options, hdr_info)?;

    // Extract ICC Profile from original input for preservation
    let _icc_temp = shared_utils::jxl_utils::extract_icc_profile(input);
    let icc_path = _icc_temp.as_ref().map(tempfile::NamedTempFile::path);

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        shared_utils::thread_manager::get_optimal_threads()
    };
    let mut cmd = Command::new("cjxl");
    cmd.arg("-d")
        .arg(format!("{distance:.2}"))
        .arg("-e")
        .arg("7")
        .arg("-j")
        .arg(max_threads.to_string());

    if options.apple_compat {
        cmd.arg("--compress_boxes=0");
    }

    // Add HDR metadata via CICP if available
    if let Some(hdr) = hdr_info {
        if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
            cmd.arg(format!("--cicp={cicp}"));
        }
    }

    shared_utils::jxl_utils::add_icc_to_cjxl(&mut cmd, icc_path);

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(&actual_input).as_ref())
        .arg(shared_utils::safe_path_arg(&temp_output).as_ref());

    let cmd_result = cmd.output();

    let result = match &cmd_result {
        Ok(output_cmd) if !output_cmd.status.success() => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            if shared_utils::jxl_utils::is_icc_rounding_error(&stderr) {
                // Robustness: cjxl rejected the ICC profile (likely Capture One D50 rounding
                // deviation). Re-extract with D50 patch applied and retry once.
                use console::style;
                eprintln!(
                    "   {} {}",
                    style("🔧 ICC PATCH:").yellow().bold(),
                    style("ICC D50 rounding error detected, retrying with patched profile")
                        .yellow()
                );
                cleanup_temp_output(&temp_output, input);
                let _patched_icc = shared_utils::jxl_utils::extract_icc_with_d50_patch(input);
                let patched_icc_path = _patched_icc.as_ref().map(tempfile::NamedTempFile::path);
                let mut retry_cmd = Command::new("cjxl");
                retry_cmd
                    .arg("-d")
                    .arg(format!("{distance:.2}"))
                    .arg("-e")
                    .arg("7")
                    .arg("-j")
                    .arg(max_threads.to_string());
                if options.apple_compat {
                    retry_cmd.arg("--compress_boxes=0");
                }
                if let Some(hdr) = hdr_info {
                    if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
                        retry_cmd.arg(format!("--cicp={cicp}"));
                    }
                }
                shared_utils::jxl_utils::add_icc_to_cjxl(&mut retry_cmd, patched_icc_path);
                retry_cmd
                    .arg("--")
                    .arg(shared_utils::safe_path_arg(&actual_input).as_ref())
                    .arg(shared_utils::safe_path_arg(&temp_output).as_ref());
                retry_cmd.output().map_or(cmd_result, |o| {
                    if o.status.success() {
                        eprintln!(
                            "   {} {}",
                            style("✅").green(),
                            style("ICC patch retry succeeded").green().bold()
                        );
                    } else {
                        eprintln!(
                            "   {} ICC patch retry also failed: {}",
                            style("⚠️").yellow(),
                            String::from_utf8_lossy(&o.stderr)
                                .lines()
                                .next()
                                .unwrap_or("unknown")
                        );
                    }
                    Ok(o)
                })
            } else if stderr.contains("Getting pixel data failed")
                || stderr.contains("Failed to decode")
            {
                tracing::warn!(
                    input = %input.display(),
                    cjxl_stderr = %stderr.trim(),
                    "cjxl decode failed — falling back to ImageMagick pipeline"
                );
                if shared_utils::jxl_utils::try_imagemagick_fallback(
                    input,
                    &temp_output,
                    distance,
                    max_threads,
                    options.apple_compat,
                )
                .is_ok()
                {
                    // ImageMagick fallback succeeded — finalize directly
                    let output_size = fs::metadata(&temp_output)?.len();
                    if let Err(e) = verify_jxl_health(&temp_output) {
                        cleanup_temp_output(&temp_output, input);
                        return Err(e);
                    }
                    if !shared_utils::conversion::commit_temp_to_output_with_metadata(
                        &temp_output,
                        &output,
                        options.force,
                        Some(input),
                    )? {
                        return Ok(ConversionResult::skipped_exists(input, &output));
                    }
                    if let Some(skipped) = check_size_tolerance(
                        input,
                        &output,
                        input_size,
                        output_size,
                        options,
                        "JXL",
                    ) {
                        return Ok(skipped);
                    }
                    return finalize_conversion(
                        input,
                        &output,
                        input_size,
                        "JXL",
                        Some("(imagemagick fallback)"),
                        options,
                    )
                    .map_err(ImgQualityError::IoError);
                }
                cmd_result
            } else {
                cmd_result
            }
        }
        _ => cmd_result,
    };

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if let Err(e) = verify_jxl_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(e);
            }

            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                output_size,
                options,
                "JXL",
                None,
            )
        }
        Ok(output_cmd) => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "cjxl failed: {stderr}"
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "cjxl not found: {e}"
        ))),
    }
}

/// True when cjxl failed with "JPEG bitstream reconstruction data could not be created" / "`allow_jpeg_reconstruction`".
fn is_jpeg_reconstruction_cjxl_error(stderr: &str) -> bool {
    stderr.contains("allow_jpeg_reconstruction")
        || stderr.contains("bitstream reconstruction data could not be created")
        || stderr.contains("too much tail data")
}

fn run_cjxl_jpeg_transcode(
    input: &Path,
    temp_output: &Path,
    options: &ConvertOptions,
    allow_jpeg_reconstruction: Option<u8>,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> std::io::Result<std::process::Output> {
    let _icc_temp = shared_utils::jxl_utils::extract_icc_profile(input);
    let icc_path = _icc_temp.as_ref().map(tempfile::NamedTempFile::path);

    let max_threads = shared_utils::thread_manager::get_ffmpeg_threads();
    let mut cmd = Command::new("cjxl");
    cmd.arg("--lossless_jpeg=1")
        .arg("-j")
        .arg(max_threads.to_string());
    if let Some(v) = allow_jpeg_reconstruction {
        cmd.arg("--allow_jpeg_reconstruction").arg(v.to_string());
    }
    if options.apple_compat {
        cmd.arg("--compress_boxes=0");
    }

    if let Some(hdr) = hdr_info {
        if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
            cmd.arg(format!("--cicp={cicp}"));
        }
    }

    shared_utils::jxl_utils::add_icc_to_cjxl(&mut cmd, icc_path);

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(temp_output).as_ref());
    cmd.output()
}

/// Transcode JPEG to JXL losslessly (reconstructible).
///
/// # Errors
/// Returns an error if transcoding fails.
pub fn convert_jpeg_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let result = run_cjxl_jpeg_transcode(input, &temp_output, options, None, hdr_info);

    let output_cmd = match result {
        Ok(out) => out,
        Err(e) => {
            return Err(ImgQualityError::ToolNotFound(format!(
                "cjxl not found: {e}"
            )));
        }
    };

    if output_cmd.status.success() {
        if let Err(e) = verify_jxl_health(&temp_output) {
            cleanup_temp_output(&temp_output, input);
            return Err(e);
        }
        let output_size = fs::metadata(&temp_output).map(|m| m.len()).unwrap_or(0);
        return finalize_with_size_check(
            input,
            &temp_output,
            &output,
            input_size,
            output_size,
            options,
            "JPEG lossless transcode",
            None,
        );
    }

    let stderr = String::from_utf8_lossy(&output_cmd.stderr);
    cleanup_temp_output(&temp_output, input);

    if is_jpeg_reconstruction_cjxl_error(&stderr) {
        // 1) Fix: strip trailing data after JPEG EOI so cjxl can use bitstream reconstruction
        let (source_to_use, _guard): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
            match shared_utils::jxl_utils::strip_jpeg_tail_to_temp(input) {
                Ok(Some((cleaned, guard))) => (cleaned, Some(guard)),
                _ => (input.to_path_buf(), None),
            };

        // 2) Retry with original cjxl flags (no --allow_jpeg_reconstruction 0) on fixed or original
        let retry_original =
            run_cjxl_jpeg_transcode(&source_to_use, &temp_output, options, None, hdr_info);
        if let Ok(out) = retry_original {
            if out.status.success() {
                if let Err(e) = verify_jxl_health(&temp_output) {
                    cleanup_temp_output(&temp_output, input);
                    return Err(e);
                }
                if !shared_utils::conversion::commit_temp_to_output_with_metadata(
                    &temp_output,
                    &output,
                    options.force,
                    Some(input),
                )? {
                    return Ok(ConversionResult::skipped_exists(input, &output));
                }
                let label = if source_to_use == input {
                    "JPEG lossless transcode"
                } else {
                    "JPEG lossless transcode (sanitized tail)"
                };
                return finalize_conversion(input, &output, input_size, label, None, options)
                    .map_err(ImgQualityError::IoError);
            }
        }
        cleanup_temp_output(&temp_output, input);

        // 3) Fallback: --allow_jpeg_reconstruction 0
        let retry_no_recon =
            run_cjxl_jpeg_transcode(&source_to_use, &temp_output, options, Some(0), hdr_info);
        if let Ok(out) = retry_no_recon {
            if out.status.success() {
                if let Err(e) = verify_jxl_health(&temp_output) {
                    cleanup_temp_output(&temp_output, input);
                    return Err(e);
                }
                if !shared_utils::conversion::commit_temp_to_output_with_metadata(
                    &temp_output,
                    &output,
                    options.force,
                    Some(input),
                )? {
                    return Ok(ConversionResult::skipped_exists(input, &output));
                }
                return finalize_conversion(
                    input,
                    &output,
                    input_size,
                    "JPEG lossless transcode (--allow_jpeg_reconstruction 0)",
                    None,
                    options,
                )
                .map_err(ImgQualityError::IoError);
            }
        }
        cleanup_temp_output(&temp_output, input);
    }

    Err(ImgQualityError::ConversionError(format!(
        "cjxl JPEG transcode failed: {stderr}"
    )))
}

/// Convert to AVIF using specific quality.
///
/// # Errors
/// Returns an error if avifenc execution fails.
pub fn convert_to_avif(
    input: &Path,
    quality: Option<u8>,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();

    if let Some(ext) = input.extension() {
        if ext.to_string_lossy().to_lowercase() == "png"
            && input_size < crate::constants::SMALL_PNG_THRESHOLD_BYTES
        {
            if options.verbose {
                eprintln!("⏭️  Skipped small PNG (< 500KB): {}", input.display());
            }
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            return Ok(ConversionResult::skipped_custom(
                input,
                input_size,
                "PNG",
                "Size < 500KB threshold",
            ));
        }
    }
    let output = get_output_path(input, "avif", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);
    let q = quality.ok_or_else(|| {
        ImgQualityError::AnalysisError("Missing quality for AVIF conversion".to_string())
    })?;

    let result = Command::new("avifenc")
        .arg("-s")
        .arg("4")
        .arg("-j")
        .arg("all")
        .arg("-q")
        .arg(q.to_string())
        .arg("--")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&temp_output).as_ref())
        .output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            if let Err(e) = shared_utils::avif_av1_health::verify_avif_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(ImgQualityError::ConversionError(e));
            }
            let output_size = fs::metadata(&temp_output).map(|m| m.len()).unwrap_or(0);
            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                output_size,
                options,
                "AVIF",
                None,
            )
        }
        Ok(output_cmd) => {
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "avifenc failed: {stderr}"
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "avifenc not found: {e}"
        ))),
    }
}

/// Convert to AV1 MP4 (lossy).
///
/// # Errors
/// Returns an error if encoding fails.
pub fn convert_to_av1_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    vid_av1::animated_image::convert_to_av1_mp4(input, options)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

/// Convert to AVIF losslessly.
///
/// # Errors
/// Returns an error if encoding fails.
pub fn convert_to_avif_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    eprintln!("⚠️  Mathematical lossless AVIF encoding - this will be SLOW!");

    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();

    if let Some(ext) = input.extension() {
        if ext.to_string_lossy().to_lowercase() == "png"
            && input_size < crate::constants::SMALL_PNG_THRESHOLD_BYTES
        {
            if options.verbose {
                eprintln!("⏭️  Skipped small PNG (< 500KB): {}", input.display());
            }
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            return Ok(ConversionResult::skipped_custom(
                input,
                input_size,
                "PNG",
                "Size < 500KB threshold",
            ));
        }
    }
    let output = get_output_path(input, "avif", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let result = Command::new("avifenc")
        .arg("--lossless")
        .arg("-s")
        .arg("4")
        .arg("-j")
        .arg("all")
        .arg("--")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&temp_output).as_ref())
        .output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            if let Err(e) = shared_utils::avif_av1_health::verify_avif_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(ImgQualityError::ConversionError(e));
            }
            let output_size = fs::metadata(&temp_output).map(|m| m.len()).unwrap_or(0);
            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                output_size,
                options,
                "Lossless AVIF",
                None,
            )
        }
        Ok(output_cmd) => {
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "avifenc lossless failed: {stderr}"
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "avifenc not found: {e}"
        ))),
    }
}

/// Convert to AV1 MP4 with matched quality based on local analysis.
///
/// # Errors
/// Returns an error if matching or encoding fails.
pub fn convert_to_av1_mp4_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    let input_size = fs::metadata(input)
        .map(|m| m.len())
        .map_err(ImgQualityError::IoError)?;
    let initial_crf = calculate_matched_crf_for_animation(analysis, input_size)?;
    vid_av1::animated_image::convert_to_av1_mp4_matched(
        input,
        options,
        initial_crf,
        analysis.has_alpha,
    )
    .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

fn calculate_matched_crf_for_animation(
    analysis: &crate::ImageAnalysis,
    file_size: u64,
) -> Result<f32> {
    let quality_analysis = shared_utils::from_image_analysis(
        &analysis.format,
        analysis.width,
        analysis.height,
        analysis.color_depth,
        analysis.has_alpha,
        file_size,
        analysis.duration_secs.map(f64::from),
        None,
        None,
    );

    match shared_utils::calculate_av1_crf(&quality_analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(
                &quality_analysis,
                &result,
                shared_utils::EncoderType::Av1,
            );
            Ok(result.crf)
        }
        Err(e) => Err(ImgQualityError::AnalysisError(format!(
            "Quality analysis failed for animation: {e}"
        ))),
    }
}

/// Calculate matched JXL distance based on image complexity and file size.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_matched_distance_for_static(
    analysis: &crate::ImageAnalysis,
    file_size: u64,
) -> Result<f32> {
    let estimated_quality = analysis.jpeg_analysis.as_ref().map(|j| j.estimated_quality);

    let quality_analysis = shared_utils::from_image_analysis(
        &analysis.format,
        analysis.width,
        analysis.height,
        analysis.color_depth,
        analysis.has_alpha,
        file_size,
        None,
        None,
        estimated_quality,
    );

    match shared_utils::calculate_jxl_distance(&quality_analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(
                &quality_analysis,
                &result,
                shared_utils::EncoderType::Jxl,
            );
            Ok(result.distance)
        }
        Err(e) => Err(ImgQualityError::AnalysisError(format!(
            "Quality analysis failed for static: {e}"
        ))),
    }
}

/// Convert to JXL with matched distance.
///
/// # Errors
/// Returns an error if matching or encoding fails.
pub fn convert_to_jxl_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();

    if let Some(ext) = input.extension() {
        if ext.to_string_lossy().to_lowercase() == "png"
            && input_size < crate::constants::SMALL_PNG_THRESHOLD_BYTES
        {
            if options.verbose {
                eprintln!("⏭️  Skipped small PNG (< 500KB): {}", input.display());
            }
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            return Ok(ConversionResult::skipped_custom(
                input,
                input_size,
                "PNG",
                "Size < 500KB threshold",
            ));
        }
    }
    let output = get_output_path(input, "jxl", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let distance = calculate_matched_distance_for_static(analysis, input_size)?;
    eprintln!("   🎯 Matched JXL distance: {distance:.2}");

    let max_threads = shared_utils::thread_manager::get_optimal_threads();
    let mut cmd = Command::new("cjxl");
    cmd.arg("-d")
        .arg(format!("{distance:.2}"))
        .arg("-e")
        .arg("7")
        .arg("-j")
        .arg(max_threads.to_string());

    if options.apple_compat {
        cmd.arg("--compress_boxes=0");
    }

    // `analysis` passed in doesn't have hdr_info in signature, we get it from analysis
    if let Some(ref hdr) = analysis.hdr_info {
        if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
            cmd.arg(format!("--cicp={cicp}"));
        }
    }

    // Only disable lossless JPEG mode when input is actually JPEG and we want lossy encoding.
    if distance > 0.0 {
        let is_jpeg = options
            .input_format
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("jpeg") || f.eq_ignore_ascii_case("jpg"));
        if is_jpeg {
            cmd.arg("--lossless_jpeg=0");
        }
    }

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&temp_output).as_ref());

    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if let Err(e) = verify_jxl_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(e);
            }

            let extra = format!("d={distance:.2}");
            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                output_size,
                options,
                "Quality-matched JXL",
                Some(extra),
            )
        }
        Ok(output_cmd) => {
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "cjxl failed: {stderr}"
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "cjxl not found: {e}"
        ))),
    }
}

/// Convert to AV1 MP4 losslessly.
///
/// # Errors
/// Returns an error if encoding fails.
pub fn convert_to_av1_mp4_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    vid_av1::animated_image::convert_to_av1_mkv_lossless(input, options)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

fn verify_jxl_health(path: &Path) -> Result<()> {
    shared_utils::jxl_utils::verify_jxl_health(path).map_err(ImgQualityError::ConversionError)
}

fn convert_to_temp_png(
    input: &Path,
    tool: &str,
    args_before_input: &[&str],
    args_after_input: &[&str],
    label: &str,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    shared_utils::jxl_utils::convert_to_temp_png(
        input,
        tool,
        args_before_input,
        args_after_input,
        label,
    )
    .map_err(ImgQualityError::IoError)
}
fn prepare_input_for_cjxl(
    input: &Path,
    options: &ConvertOptions,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    // Ensure we have color info for bit depth detection if not provided
    let local_hdr_info;
    let hdr_info = if let Some(info) = hdr_info {
        info
    } else {
        local_hdr_info = shared_utils::ffprobe_json::extract_color_info(input);
        &local_hdr_info
    };

    // Determine target bit depth (match source if > 8-bit, else 8-bit)
    let is_float = hdr_info.is_float;
    let is_high_bit_depth = hdr_info.bit_depth.is_some_and(|d| d > 8) || hdr_info.is_hdr() || is_float;
    let depth_str = if is_float { "32" } else if is_high_bit_depth { "16" } else { "8" };
    let intermediate_suffix = if is_float { ".exr" } else { ".png" };

    if is_float && options.verbose {
        use console::style;
        eprintln!(
            "   {} Source is 32-bit float (HDR), using OpenEXR intermediate format",
            style("🚀").cyan().bold()
        );
    } else if is_high_bit_depth && options.verbose {
        use console::style;
        eprintln!(
            "   {} Source is {}-bit, using 16-bit intermediate format",
            style("💎").cyan(),
            hdr_info.bit_depth.unwrap_or(10)
        );
    }

    // Check if we need HDR decoding (explicitly requested or high bit depth)
    if shared_utils::needs_hdr_decode(Some(hdr_info)) {
        use console::style;
        eprintln!(
            "   {} {}",
            style("🌈 HDR DECODING:").cyan().bold(),
            style("Using FFmpeg to preserve high bit-depth").cyan()
        );

        match shared_utils::decode_hdr_image_to_png16(input, hdr_info) {
            Ok((png16_path, temp_file)) => {
                eprintln!(
                    "   {} {}",
                    style("✅").green(),
                    style("HDR decode successful (16-bit PNG)").green().bold()
                );
                return Ok((png16_path, Some(temp_file)));
            }
            Err(e) => {
                eprintln!(
                    "   {} HDR decode failed: {}, falling back to standard decode",
                    style("⚠️").yellow(),
                    e
                );
            }
        }
    }

    let detected_ext = shared_utils::common_utils::detect_real_extension(input);
    let literal_ext = input
        .extension()
        .map(std::ffi::OsStr::to_ascii_lowercase)
        .and_then(|e| e.to_str().map(std::string::ToString::to_string))
        .unwrap_or_default();

    let ext = if let Some(real) = detected_ext {
        if !literal_ext.is_empty()
            && real != literal_ext
            && !((real == "jpg" && literal_ext == "jpeg")
                || (real == "jpeg" && literal_ext == "jpg"))
        {
            use console::style;
            eprintln!(
                "   {} '{}' (disguised as .{}) -> actually {}, will process as actual format",
                style("⚠️  [Smart fix] Extension mismatch:").yellow().bold(),
                input.display(),
                literal_ext,
                real.to_uppercase()
            );
        }
        real.to_string()
    } else if let Some(ref format) = options.input_format {
        format.to_lowercase()
    } else {
        literal_ext
    };

    match ext.as_str() {
        "jpg" | "jpeg" => {
            // SOI marker only; detect_real_extension may have already done a fuller magic-byte check.
            let is_header_valid = std::fs::File::open(input)
                .and_then(|mut f| {
                    use std::io::Read;
                    let mut buf = [0u8; 2];
                    f.read_exact(&mut buf)?;
                    Ok(buf == [0xFF, 0xD8])
                })
                .unwrap_or(false);

            if is_header_valid {
                Ok((input.to_path_buf(), None))
            } else {
                use console::style;
                eprintln!(
                    "   {} {}",
                    style("🔧 PRE-PROCESSING:").yellow().bold(),
                    style("Corrupted JPEG header detected, using ImageMagick to sanitize").yellow()
                );

                let temp_file = tempfile::Builder::new().suffix(intermediate_suffix).tempfile()?;
                let temp_path = temp_file.path().to_path_buf();

                let mut cmd = Command::new("magick");
                cmd.arg("--")
                    .arg(shared_utils::safe_path_arg(input).as_ref());

                if is_float {
                    cmd.arg("-format").arg("exr");
                }
                
                cmd.arg("-depth")
                    .arg(depth_str)
                    .arg(shared_utils::safe_path_arg(&temp_path).as_ref());

                let result = cmd.output();

                match result {
                    Ok(output) if output.status.success() && temp_path.exists() => {
                        let label = if is_float { "OpenEXR" } else { "ImageMagick PNG" };
                        eprintln!(
                            "   {} {} {} sanitization successful",
                            style("✅").green(),
                            style(label).green().bold(),
                            style("JPEG").dim()
                        );
                        Ok((temp_path, Some(temp_file)))
                    }
                    _ => {
                        eprintln!(
                            "   {} {}",
                            style("⚠️").red(),
                            style("ImageMagick sanitization failed, trying direct input").dim()
                        );
                        Ok((input.to_path_buf(), None))
                    }
                }
            }
        }

        "webp" => convert_to_temp_png(
            input,
            "dwebp",
            &[],
            &["-o", "__OUTPUT__"],
            "WebP detected, using dwebp for ICC profile compatibility",
        ),

        "tiff" | "tif" => {
            let label = if is_float { "32-bit float OpenEXR" } else { &format!("{depth_str}-bit PNG") };
            let temp_file = tempfile::Builder::new().suffix(intermediate_suffix).tempfile().map_err(ImgQualityError::IoError)?;
            let temp_path = temp_file.path().to_path_buf();
            
            let mut cmd = Command::new("magick");
            cmd.arg("--")
                .arg(shared_utils::safe_path_arg(input).as_ref());
            
            if is_float {
                cmd.arg("-format").arg("exr");
            }
            
            cmd.arg("-depth")
                .arg(depth_str)
                .arg(shared_utils::safe_path_arg(&temp_path).as_ref());
            
            let out = cmd.output().map_err(ImgQualityError::IoError)?;
            if out.status.success() && temp_path.exists() {
                if options.verbose {
                    eprintln!("   ✅ TIFF detected, using ImageMagick to emit {label}");
                }
                Ok((temp_path, Some(temp_file)))
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(ImgQualityError::ConversionError(format!("magick TIFF conversion failed: {err}")))
            }
        }

        "bmp" => {
            let label = if is_float { "32-bit float OpenEXR" } else { &format!("{depth_str}-bit PNG") };
            let temp_file = tempfile::Builder::new().suffix(intermediate_suffix).tempfile().map_err(ImgQualityError::IoError)?;
            let temp_path = temp_file.path().to_path_buf();
            
            let mut cmd = Command::new("magick");
            cmd.arg("--")
                .arg(shared_utils::safe_path_arg(input).as_ref());
            
            if is_float {
                cmd.arg("-format").arg("exr");
            }
            
            cmd.arg("-depth")
                .arg(depth_str)
                .arg(shared_utils::safe_path_arg(&temp_path).as_ref());
            
            let out = cmd.output().map_err(ImgQualityError::IoError)?;
            if out.status.success() && temp_path.exists() {
                if options.verbose {
                    eprintln!("   ✅ BMP detected, using ImageMagick to emit {label}");
                }
                Ok((temp_path, Some(temp_file)))
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(ImgQualityError::ConversionError(format!("magick BMP conversion failed: {err}")))
            }
        }

        "heic" | "heif" => {
            use console::style;
            eprintln!(
                "   {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style("HEIC/HEIF detected, using sips/ImageMagick for cjxl compatibility").dim()
            );

            let temp_png_file = tempfile::Builder::new().suffix(".png").tempfile()?;
            let temp_png = temp_png_file.path().to_path_buf();

            eprintln!("   🍎 Trying macOS sips first...");
            let result = Command::new("sips")
                .arg("-s")
                .arg("format")
                .arg("png")
                .arg(shared_utils::safe_path_arg(input).as_ref())
                .arg("--out")
                .arg(shared_utils::safe_path_arg(&temp_png).as_ref())
                .output();

            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!("   ✅ sips HEIC pre-processing successful");
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    eprintln!("   ⚠️  sips failed, trying ImageMagick...");
                    let temp_file = tempfile::Builder::new().suffix(intermediate_suffix).tempfile()?;
                    let temp_path = temp_file.path().to_path_buf();
                    
                    let mut cmd = Command::new("magick");
                    cmd.arg("--")
                        .arg(shared_utils::safe_path_arg(input).as_ref());
                    
                    if is_float {
                        cmd.arg("-format").arg("exr");
                    }
                    
                    cmd.arg("-depth")
                        .arg(depth_str)
                        .arg(shared_utils::safe_path_arg(&temp_path).as_ref());

                    match cmd.output() {
                        Ok(output) if output.status.success() && temp_path.exists() => {
                            eprintln!("   ✅ ImageMagick HEIC pre-processing successful");
                            Ok((temp_path, Some(temp_file)))
                        }
                        _ => {
                            eprintln!(
                                "   ⚠️  Both sips and ImageMagick failed, trying direct cjxl"
                            );
                            Ok((input.to_path_buf(), None))
                        }
                    }
                }
            }
        }

        _ => Ok((input.to_path_buf(), None)),
    }
}

fn get_output_path(
    input: &Path,
    extension: &str,
    options: &ConvertOptions,
) -> Result<std::path::PathBuf> {
    if let Some(ref base) = options.base_dir {
        shared_utils::conversion::determine_output_path_with_base(
            input,
            base,
            extension,
            &options.output_dir,
        )
        .map_err(ImgQualityError::ConversionError)
    } else {
        shared_utils::conversion::determine_output_path(input, extension, &options.output_dir)
            .map_err(ImgQualityError::ConversionError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_get_output_path() {
        let tmp = tempdir().expect("create temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
        let input_dir = root.join("path").join("to");
        std::fs::create_dir_all(&input_dir).expect("create input dir");
        let input = input_dir.join("image.png");
        std::fs::write(&input, b"png").expect("create input file");
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let output = get_output_path(&input, "jxl", &options).unwrap();
        assert_eq!(output, input_dir.join("image.JXL"));
    }

    #[test]
    fn test_get_output_path_with_dir() {
        let tmp = tempdir().expect("create temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
        let input_dir = root.join("path").join("to");
        std::fs::create_dir_all(&input_dir).expect("create input dir");
        let input = input_dir.join("image.png");
        std::fs::write(&input, b"png").expect("create input file");
        let output_dir = root.join("output");
        let options = ConvertOptions {
            output_dir: Some(output_dir.clone()),
            base_dir: None,
            ..Default::default()
        };
        let output = get_output_path(&input, "avif", &options).unwrap();
        assert_eq!(output, output_dir.join("image.AVIF"));
    }

    #[test]
    fn test_get_output_path_same_file_error() {
        let tmp = tempdir().expect("create temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
        let input = root.join("image.JXL");
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let result = get_output_path(&input, "jxl", &options);
        assert!(result.is_err());
    }
}
