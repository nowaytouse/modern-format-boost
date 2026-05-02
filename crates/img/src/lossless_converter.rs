//! Lossless Converter Module
//!
//! Provides conversion API for verified lossless/lossy images.
//! Uses `shared_utils` for common functionality (anti-duplicate, `ConversionResult`, etc.)
//!
//! **Unified Compress Check**: All image conversions call `check_size_tolerance` after
//! successful encoding and obtaining `output_size`, before finalization. When `options.compress`
//! is true, only accept when output < input, otherwise skip and keep original file.
//! Covered paths: `convert_to_jxl`, `convert_jpeg_to_jxl` (including fallback),
//! `convert_to_avif`, `convert_to_avif_lossless`, `convert_to_jxl_matched`.

use crate::{ImgQualityError, Result};
use rug::Rational;
use shared_utils::image_jpeg_analysis::is_jpeg_complete;
use std::fs;
use std::path::Path;

pub use shared_utils::conversion::{
    check_size_tolerance, clear_processed_list, determine_output_path_with_base,
    finalize_conversion, format_size_change, is_already_processed, load_processed_list,
    mark_as_processed, save_processed_list, ConversionResult, ConvertFlags, ConvertOptions,
};

fn copy_original_on_skip(input: &Path, options: &ConvertOptions) -> Option<std::path::PathBuf> {
    shared_utils::copy_on_skip_or_fail(
        input,
        options.output_dir.as_deref(),
        options.base_dir.as_deref(),
        options.verbose(),
    )
    .unwrap_or_default()
}

fn cleanup_temp_output(temp_output: &Path, _input: &Path) {
    let _ = shared_utils::io_utils::safe_remove_file(temp_output);
}

/// Finalize conversion with size check and metadata preservation.
/// Common pattern: commit temp → check size → finalize.
/// Returns `ConversionResult` on success or error.
/// # Errors
///
/// Returns an error if the temp file cannot be committed or metadata cannot be preserved.
fn finalize_with_size_check(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    output_size: u64,
    options: &ConvertOptions,
    format_label: &str,
    extra_info: Option<&str>,
) -> Result<ConversionResult> {
    let ratio = if input_size > 0 {
        let rat = rug::Rational::from((output_size, input_size));
        rat.to_f64()
    } else {
        1.0
    };
    let benefit = input_size.saturating_sub(output_size);

    tracing::debug!(
        input = ?input.file_name().unwrap_or_default(),
        input_size,
        output_size,
        ratio = %format!("{:.2}%", ratio * 100.0),
        benefit_bytes = benefit,
        format = %format_label,
        extra = ?extra_info,
        "Finalizing conversion"
    );
    // Commit temp file to final output WITH METADATA PRESERVATION
    if !shared_utils::conversion::commit_temp_to_output_with_metadata(
        temp_output,
        output,
        options.force(),
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
        extra_info,
        options,
    )
    .map_err(ImgQualityError::IoError)
}

/// Finalize a JXL produced by a fallback pipeline (ffmpeg or imagemagick).
/// Verifies health, then delegates to `finalize_with_size_check`.
fn finalize_fallback_jxl(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    options: &ConvertOptions,
    label: &str,
) -> Result<ConversionResult> {
    let output_size = fs::metadata(temp_output)?.len();
    if let Err(e) = verify_jxl_health(temp_output) {
        cleanup_temp_output(temp_output, input);
        return Err(e);
    }
    finalize_with_size_check(
        input,
        temp_output,
        output,
        input_size,
        output_size,
        options,
        "JXL",
        Some(label),
    )
}

/// Convert `HEIC` with Gainmap to `HDR` `JXL`.
///
/// # Errors
///
/// Returns an error if:
/// - The input file is invalid or a duplicate.
/// - The `HDR` synthesis process fails.
/// - The output file cannot be written or finalized.
pub fn convert_heic_gainmap_to_jxl(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if output.exists() && !options.force() {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    // Use 16-bit PNG for PQ HDR encoding with cjxl
    let intermediate_format = shared_utils::HdrIntermediateFormat::Png16;

    // Call the synthesis logic from shared_utils
    shared_utils::hdr_synthesis::convert_heic_with_gainmap_to_jxl_hdr(
        input,
        &temp_output,
        options.apple_compat(),
        intermediate_format,
        options.ultimate(),
    )
    .map_err(|e| {
        let msg = format!("☢️ HDR Synthesis Failure: {e}");
        ImgQualityError::ConversionError(msg)
    })?;

    let output_size = fs::metadata(&temp_output)
        .map_err(|e| {
            ImgQualityError::ConversionError(format!(
                "☢️ Failed to retrieve HDR synthesis output metadata: {e}"
            ))
        })?
        .len();

    // Verify health
    if let Err(e) = verify_jxl_health(&temp_output) {
        cleanup_temp_output(&temp_output, input);
        return Err(ImgQualityError::ConversionError(format!(
            "⛔️ Synthetic HDR JXL health check failed: {e}"
        )));
    }

    finalize_with_size_check(
        input,
        &temp_output,
        &output,
        input_size,
        output_size,
        options,
        "JXL (HDR Synthesis 🌈)",
        None,
    )
    .map_err(|e| {
        ImgQualityError::ConversionError(format!("☢️ HDR Synthesis Finalization Error: {e}"))
    })
}

/// Convert `UltraHDR JPEG` with gainmap metadata to synthesized HDR `JXL`.
///
/// # Errors
///
/// Returns an error if extraction, synthesis, or finalization fails.
pub fn convert_ultrahdr_jpeg_to_jxl(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if output.exists() && !options.force() {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    // Synthesize into an isolated temp path so final commit/metadata handling stays atomic.
    shared_utils::hdr_synthesis::convert_ultrahdr_jpeg_to_jxl_hdr(
        input,
        &temp_output,
        options.apple_compat(),
        shared_utils::hdr_synthesis::HdrIntermediateFormat::Png16,
        options.ultimate(),
    )
    .map_err(|e| {
        let msg = format!("☢️ UltraHDR Synthesis Failure: {e}");
        ImgQualityError::ConversionError(msg)
    })?;

    let output_size = fs::metadata(&temp_output)
        .map_err(|e| {
            ImgQualityError::ConversionError(format!(
                "☢️ Failed to retrieve synthesized JXL metadata: {e}"
            ))
        })?
        .len();

    // Verify health
    if let Err(e) = verify_jxl_health(&temp_output) {
        cleanup_temp_output(&temp_output, input);
        return Err(ImgQualityError::ConversionError(format!(
            "⛔️ Synthesized UltraHDR JXL health check failed: {e}"
        )));
    }

    finalize_with_size_check(
        input,
        &temp_output,
        &output,
        input_size,
        output_size,
        options,
        "JXL (UltraHDR Synthesis ☀️)",
        Some("Native HDR"),
    )
    .map_err(|e| {
        ImgQualityError::ConversionError(format!("☢️ UltraHDR Synthesis Finalization Error: {e}"))
    })
}

/// Convert an image to JXL format with specified quality distance.
///
/// # Arguments
/// * `input` - Path to the input image file
/// * `options` - Conversion options (force, `delete_original`, `output_dir`, etc.)
/// * `distance` - JXL quality distance (0.0 = lossless, higher = more lossy)
/// * `hdr_info` - Optional HDR metadata for preserving color information
///
/// # Returns
/// * `Ok(ConversionResult)` - Conversion result with file sizes and status
/// * `Err(ImgQualityError)` - Conversion failed
///
/// # Behavior
/// - Validates input file (checks symlinks, file type, readability)
/// - Skips small PNG files (< 500KB) to avoid overhead
/// - Uses cjxl for encoding, with `FFmpeg` → `ImageMagick` fallback on failure
/// - Preserves HDR metadata via --cicp parameter when `hdr_info` is provided
/// - Verifies JXL health after encoding
/// - Checks size tolerance and compress mode requirements
///
/// # Example
/// ```no_run
/// use img::lossless_converter::{convert_to_jxl, ConvertOptions};
/// use std::path::Path;
///
/// let input = Path::new("input.png");
/// let options = ConvertOptions::default();
/// let result = convert_to_jxl(input, &options, 0.1, None)?;
/// # Ok::<(), img::ImgQualityError>(())
/// ```
/// Convert to JXL using specific distance.
///
/// # Errors
///
/// Returns an error if:
/// - Input validation fails.
/// - `cjxl` execution fails and all fallbacks (`FFmpeg`, `ImageMagick`) also fail.
/// - The output file cannot be written or verified.
pub fn convert_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    distance: f32,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> Result<ConversionResult> {
    use std::process::Stdio;
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();

    if let Some(ext) = input.extension() {
        if ext.to_string_lossy().to_lowercase() == "png"
            && input_size < crate::constants::SMALL_PNG_THRESHOLD_BYTES
        {
            if options.verbose() {
                eprintln!("⏭️  Skipped small PNG (< 500KB): {}", input.display());
            }
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            return Ok(ConversionResult::skipped_custom(
                input,
                input_size,
                "Skipped: Small PNG (< 500KB)",
                "small_file",
            ));
        }
    }
    let output = get_output_path(input, "jxl", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force() {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    let (actual_input, _temp_file_guard) = prepare_input_for_cjxl(input, options, hdr_info)?;

    // Extract ICC Profile from original input for preservation
    let icc_temp = shared_utils::jxl_utils::extract_icc_profile(input);
    let icc_path = icc_temp.as_ref().map(tempfile::NamedTempFile::path);

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        shared_utils::thread_manager::get_optimal_threads()
    };

    tracing::debug!(
        file = %input.display(),
        threads = max_threads,
        icc_preserved = icc_path.is_some(),
        hdr = hdr_info.is_some(),
        "Encoding JXL: calculating parameters"
    );

    let actual_dist = shared_utils::constants::jxl_distance_for_mode(distance, options.ultimate());
    let is_extreme_explore = options.ultimate() && options.explore();
    let actual_eff = jxl_screening_effort(options.ultimate(), options.explore());

    let mut builder = shared_utils::CjxlBuilder::new();
    builder
        .input(&actual_input)
        .output(&temp_output)
        .distance(actual_dist)
        .effort(actual_eff)
        .threads(max_threads)
        .apple_compat(options.apple_compat());

    // Add HDR metadata via CICP if available
    if let Some(hdr) = hdr_info {
        if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
            builder.cicp(&cicp);
            if options.verbose() {
                eprintln!("   🌈 HDR detected: applying CICP {cicp}");
            }
        }
    }

    if let Some(icc) = icc_path {
        builder.icc_profile(icc);
    }

    if options.verbose() {
        eprintln!(
            "   🔧 Executing: cjxl -d {:.2} -e {} -j {} {} {}",
            actual_dist,
            actual_eff,
            max_threads,
            actual_input.display(),
            temp_output.display()
        );
    }

    let result = builder.build().output();

    let result = match &result {
        Ok(output_cmd) if !output_cmd.status.success() => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            if shared_utils::jxl_utils::is_icc_rounding_error(&stderr) {
                // Robustness: cjxl rejected the ICC profile (likely Capture One D50 rounding
                // deviation). Re-extract with D50 patch applied and retry once.
                use console::style;
                shared_utils::progress_mode::emit_stderr(&format!(
                    "   {} {}",
                    "🔧 ICC PATCH:",
                    "ICC D50 rounding error detected, retrying with patched profile"
                ));
                let patched_icc = shared_utils::jxl_utils::extract_icc_with_d50_patch(input);
                let patched_icc_path = patched_icc.as_ref().map(tempfile::NamedTempFile::path);
                let mut builder = shared_utils::CjxlBuilder::new();
                builder
                    .input(&actual_input)
                    .output(&temp_output)
                    .distance(actual_dist)
                    .effort(actual_eff)
                    .threads(max_threads)
                    .apple_compat(options.apple_compat());

                if let Some(hdr) = hdr_info {
                    if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
                        builder.cicp(cicp);
                    }
                }
                if let Some(icc) = patched_icc_path {
                    builder.icc_profile(icc);
                }

                let retry_out = builder.build().output();
                if let Ok(ref o) = retry_out {
                    if o.status.success() {
                        shared_utils::progress_mode::emit_stderr("   ✅ ICC patch retry succeeded");
                    } else {
                        let line = format!(
                            "   ⚠️ ICC patch retry also failed: {}",
                            String::from_utf8_lossy(&o.stderr)
                                .lines()
                                .next()
                                .unwrap_or("unknown")
                        );
                        shared_utils::progress_mode::emit_stderr(&line);
                    }
                }
                // drop style to satisfy unused import lint when feature is off
                let _ = style("");
                retry_out.or(result)
            } else if stderr.contains("Getting pixel data failed")
                || stderr.contains("Failed to decode")
                || stderr.contains("Decoding failed")
                || stderr.contains("pixel data")
                || stderr.contains("Error while decoding")
                || stderr.contains("libpng warning")
                || shared_utils::jxl_utils::is_grayscale_icc_cjxl_error(&stderr)
            {
                // Check if this is a grayscale ICC profile mismatch error
                // If so, use ImageMagick fallback which has proper retry logic with -strip
                if shared_utils::jxl_utils::is_grayscale_icc_cjxl_error(&stderr) {
                    tracing::warn!(
                        input = %input.display(),
                        "Grayscale ICC profile mismatch detected — using ImageMagick fallback"
                    );

                    if try_imagemagick_fallback_with_effort(
                        input,
                        &temp_output,
                        actual_dist,
                        max_threads,
                        options.apple_compat(),
                        actual_eff,
                    )
                    .is_ok()
                    {
                        // ImageMagick fallback succeeded — finalize directly
                        let _output_size = fs::metadata(&temp_output)?.len();
                        if let Err(e) = verify_jxl_health(&temp_output) {
                            cleanup_temp_output(&temp_output, input);
                            return Err(e);
                        }
                        if !shared_utils::conversion::commit_temp_to_output_with_metadata(
                            &temp_output,
                            &output,
                            options.force(),
                            Some(input),
                        )? {
                            return Ok(ConversionResult::skipped_exists(input, &output));
                        }
                        return finalize_conversion(
                            input,
                            &output,
                            input_size,
                            "JXL",
                            Some("(grayscale ICC fix)"),
                            options,
                        )
                        .map_err(ImgQualityError::IoError);
                    }
                }

                // Not a grayscale ICC error, or ImageMagick fallback failed
                // Try FFmpeg pipeline as before
                tracing::warn!(
                    input = %input.display(),
                    cjxl_stderr = %stderr.trim(),
                    "cjxl decode failed — falling back to FFmpeg+cjxl pipeline"
                );

                let is_high_bit_depth = hdr_info
                    .is_some_and(|info| info.bit_depth.is_some_and(|d| d > 8) || info.is_hdr());
                let pix_fmt = if is_high_bit_depth {
                    shared_utils::PixFmt::Rgb48le
                } else {
                    shared_utils::PixFmt::Rgb24
                };

                let mut ffmpeg_builder = shared_utils::FfmpegBuilder::new();
                ffmpeg_builder
                    .threads(max_threads)
                    .input(input)
                    .frames_v(1)
                    .pix_fmt(pix_fmt)
                    .vcodec(shared_utils::VideoCodec::Png)
                    .format("image2pipe")
                    .output_pipe(); // output to pipe

                let ffmpeg_result = ffmpeg_builder
                    .build()
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                match ffmpeg_result {
                    Ok(mut ffmpeg_proc) => {
                        if let Some(ffmpeg_stdout) = ffmpeg_proc.stdout.take() {
                            let mut cjxl_builder = shared_utils::CjxlBuilder::new();
                            cjxl_builder
                                .use_stdin(true)
                                .output(&temp_output)
                                .distance(actual_dist)
                                .effort(actual_eff)
                                .threads(max_threads)
                                .apple_compat(options.apple_compat());

                            let cjxl_result = cjxl_builder
                                .build()
                                .stdin(ffmpeg_stdout)
                                .stderr(Stdio::piped())
                                .spawn();

                            match cjxl_result {
                                Ok(mut cjxl_proc) => {
                                    let ffmpeg_stderr_thread =
                                        ffmpeg_proc.stderr.take().map(|stderr| {
                                            std::thread::spawn(move || {
                                                use std::io::Read;
                                                let mut buf = String::with_capacity(64 * 1024);
                                                if let Err(err) = stderr
                                                    .take(shared_utils::numeric_cast::usize_to_u64(crate::constants::STDERR_BUFFER_MAX))
                                                    .read_to_string(&mut buf)
                                                {
                                                    let line = format!(
                                                        "   ⚠️ Failed to read FFmpeg stderr output: {err}"
                                                    );
                                                    shared_utils::progress_mode::emit_stderr(&line);
                                                }
                                                buf
                                            })
                                        });

                                    // Drain cjxl stderr in background so cjxl does not block when pipe buffer fills.
                                    let cjxl_stderr_thread =
                                        cjxl_proc.stderr.take().map(|stderr| {
                                            std::thread::spawn(move || {
                                                use std::io::Read;
                                                let mut buf = String::with_capacity(64 * 1024);
                                                if let Err(err) = stderr
                                                    .take(
                                                        shared_utils::numeric_cast::usize_to_u64(crate::constants::STDERR_BUFFER_MAX),
                                                    )
                                                    .read_to_string(&mut buf)
                                                {
                                                    shared_utils::log_rare_error!(
                                                        "Stderr Pipe",
                                                        "Failed to read cjxl stderr: {err}"
                                                    );
                                                }
                                                buf
                                            })
                                        });

                                    let ffmpeg_status = ffmpeg_proc.wait();
                                    let cjxl_status = cjxl_proc.wait();

                                    let ffmpeg_stderr_str = match ffmpeg_stderr_thread {
                                        Some(handle) => {
                                            if let Ok(s) = handle.join() {
                                                s
                                            } else {
                                                shared_utils::log_rare_error!(
                                                    "Background Thread",
                                                    "FFmpeg stderr thread panicked"
                                                );
                                                String::new()
                                            }
                                        }
                                        None => String::new(),
                                    };
                                    let cjxl_stderr_str = match cjxl_stderr_thread {
                                        Some(handle) => {
                                            if let Ok(s) = handle.join() {
                                                s
                                            } else {
                                                shared_utils::progress_mode::emit_stderr(
                                                    "   ⚠️ cjxl stderr thread panicked",
                                                );
                                                String::new()
                                            }
                                        }
                                        None => String::new(),
                                    };

                                    let ffmpeg_ok = match ffmpeg_status {
                                        Ok(status) if status.success() => true,
                                        Ok(status) => {
                                            let line = format!(
                                                "   ❌ FFmpeg failed with exit code: {:?}",
                                                status.code()
                                            );
                                            shared_utils::progress_mode::emit_stderr(&line);
                                            if !ffmpeg_stderr_str.is_empty() {
                                                let line2 = format!(
                                                    "      Error: {}",
                                                    ffmpeg_stderr_str
                                                        .lines()
                                                        .next()
                                                        .unwrap_or("Unknown")
                                                );
                                                shared_utils::progress_mode::emit_stderr(&line2);
                                            }
                                            false
                                        }
                                        Err(e) => {
                                            let line =
                                                format!("   ❌ Failed to wait for FFmpeg: {e}");
                                            shared_utils::progress_mode::emit_stderr(&line);
                                            false
                                        }
                                    };

                                    let cjxl_ok = match cjxl_status {
                                        Ok(status) if status.success() => true,
                                        Ok(status) => {
                                            let line = format!(
                                                "   ❌ cjxl failed with exit code: {:?}",
                                                status.code()
                                            );
                                            shared_utils::progress_mode::emit_stderr(&line);
                                            if !cjxl_stderr_str.is_empty() {
                                                let line2 = format!(
                                                    "      Error: {}",
                                                    cjxl_stderr_str
                                                        .lines()
                                                        .next()
                                                        .unwrap_or("Unknown")
                                                );
                                                shared_utils::progress_mode::emit_stderr(&line2);
                                            }
                                            false
                                        }
                                        Err(e) => {
                                            let line =
                                                format!("   ❌ Failed to wait for cjxl: {e}");
                                            shared_utils::progress_mode::emit_stderr(&line);
                                            false
                                        }
                                    };

                                    if ffmpeg_ok && cjxl_ok {
                                        shared_utils::progress_mode::fallback_success();
                                        // Early-return: finalize directly instead of faking an Output
                                        let output_size = fs::metadata(&temp_output)?.len();
                                        if let Err(e) = verify_jxl_health(&temp_output) {
                                            cleanup_temp_output(&temp_output, input);
                                            return Err(e);
                                        }
                                        return finalize_with_size_check(
                                            input,
                                            &temp_output,
                                            &output,
                                            input_size,
                                            output_size,
                                            options,
                                            "JXL",
                                            Some("(ffmpeg fallback)"),
                                        );
                                    }

                                    let line = format!(
                                        "   ❌ FFmpeg pipeline failed for file: {} (ffmpeg: {}, cjxl: {})",
                                        input.display(),
                                        if ffmpeg_ok { "✓" } else { "✗" },
                                        if cjxl_ok { "✓" } else { "✗" }
                                    );
                                    shared_utils::progress_mode::emit_stderr(&line);
                                    shared_utils::progress_mode::emit_stderr("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                                    if try_imagemagick_fallback_with_effort(
                                        input,
                                        &temp_output,
                                        actual_dist,
                                        max_threads,
                                        options.apple_compat(),
                                        actual_eff,
                                    )
                                    .is_ok()
                                    {
                                        return finalize_fallback_jxl(
                                            input,
                                            &temp_output,
                                            &output,
                                            input_size,
                                            options,
                                            "(imagemagick fallback)",
                                        );
                                    }
                                    result
                                }
                                Err(e) => {
                                    let line = format!("   ❌ Failed to start cjxl process: {e}");
                                    shared_utils::progress_mode::emit_stderr(&line);
                                    if let Err(kill_err) = ffmpeg_proc.kill() {
                                        let line = format!(
                                            "   ⚠️ Failed to stop FFmpeg after cjxl startup failure: {kill_err}"
                                        );
                                        shared_utils::progress_mode::emit_stderr(&line);
                                    }
                                    shared_utils::progress_mode::emit_stderr(
                                        "   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...",
                                    );
                                    if try_imagemagick_fallback_with_effort(
                                        input,
                                        &temp_output,
                                        actual_dist,
                                        max_threads,
                                        options.apple_compat(),
                                        actual_eff,
                                    )
                                    .is_ok()
                                    {
                                        return finalize_fallback_jxl(
                                            input,
                                            &temp_output,
                                            &output,
                                            input_size,
                                            options,
                                            "(imagemagick fallback)",
                                        );
                                    }
                                    result
                                }
                            }
                        } else {
                            shared_utils::progress_mode::emit_stderr(
                                "   ❌ Failed to capture FFmpeg stdout",
                            );
                            if let Err(kill_err) = ffmpeg_proc.kill() {
                                let line = format!(
                                    "   ⚠️ Failed to stop FFmpeg after stdout capture failure: {kill_err}"
                                );
                                shared_utils::progress_mode::emit_stderr(&line);
                            }
                            shared_utils::progress_mode::emit_stderr(
                                "   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...",
                            );
                            if try_imagemagick_fallback_with_effort(
                                input,
                                &temp_output,
                                actual_dist,
                                max_threads,
                                options.apple_compat(),
                                actual_eff,
                            )
                            .is_ok()
                            {
                                return finalize_fallback_jxl(
                                    input,
                                    &temp_output,
                                    &output,
                                    input_size,
                                    options,
                                    "(imagemagick fallback)",
                                );
                            }
                            result
                        }
                    }
                    Err(e) => {
                        let line = format!("   ❌ FFmpeg not available or failed to start: {e}");
                        shared_utils::progress_mode::emit_stderr(&line);
                        shared_utils::progress_mode::emit_stderr(
                            "      💡 Install: brew install ffmpeg",
                        );
                        shared_utils::progress_mode::emit_stderr(
                            "   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...",
                        );
                        if try_imagemagick_fallback_with_effort(
                            input,
                            &temp_output,
                            actual_dist,
                            max_threads,
                            options.apple_compat(),
                            actual_eff,
                        )
                        .is_ok()
                        {
                            return finalize_fallback_jxl(
                                input,
                                &temp_output,
                                &output,
                                input_size,
                                options,
                                "(imagemagick fallback)",
                            );
                        }
                        result
                    }
                }
            } else {
                result
            }
        }
        _ => result,
    };

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if let Err(e) = verify_jxl_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(e);
            }

            let mut final_output_size = output_size;
            let mut extra_info = None;

            if is_extreme_explore {
                if let Some(explore_result) = try_explore_ultimate_jxl_distance(
                    input,
                    &actual_input,
                    &temp_output,
                    input_size,
                    output_size,
                    max_threads,
                    options,
                    icc_path,
                    hdr_info,
                )? {
                    final_output_size = explore_result.output_size;
                    extra_info = Some(format!(
                        "(screened e7, finalized e10 d={})",
                        shared_utils::jxl_explorer::format_distance_for_log(
                            explore_result.accepted_distance
                        )
                    ));
                }
            }

            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                final_output_size,
                options,
                "JXL",
                extra_info.as_deref(),
            )
        }
        Ok(output_cmd) => {
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "cjxl failed: {stderr}"
            )))
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::ToolNotFound(format!(
                "cjxl not found: {e}"
            )))
        }
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
    max_threads: usize,
    allow_jpeg_reconstruction: Option<u8>,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> std::io::Result<std::process::Output> {
    let icc_temp = shared_utils::jxl_utils::extract_icc_profile(input);
    let icc_path = icc_temp.as_ref().map(tempfile::NamedTempFile::path);

    let mut builder = shared_utils::CjxlBuilder::new();
    builder
        .input(input)
        .output(temp_output)
        .lossless_jpeg(true)
        .effort(shared_utils::constants::jxl_effort_for_mode(
            options.ultimate(),
        ))
        .threads(max_threads)
        .apple_compat(options.apple_compat());

    if let Some(v) = allow_jpeg_reconstruction {
        builder.allow_jpeg_reconstruction(v != 0);
    }

    // Add HDR metadata via CICP if available (for wide-gamut JPEG)
    if let Some(hdr) = hdr_info {
        if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
            builder.cicp(cicp);
        }
    }

    if let Some(icc) = icc_path {
        builder.icc_profile(icc);
    }

    builder.build().output()
}

fn commit_jpeg_to_jxl_success(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    options: &ConvertOptions,
    label: &str,
) -> Result<ConversionResult> {
    if let Err(e) = verify_jxl_health(temp_output) {
        cleanup_temp_output(temp_output, input);
        return Err(e);
    }
    let output_size = fs::metadata(temp_output).map_or(0, |m| m.len());
    finalize_with_size_check(
        input,
        temp_output,
        output,
        input_size,
        output_size,
        options,
        label,
        None,
    )
}

/// Convert a JPEG image to JXL format using lossless JPEG transcoding.
///
/// # Arguments
/// * `input` - Path to the input JPEG file
/// * `options` - Conversion options
///
/// # Returns
/// * `Ok(ConversionResult)` - Conversion result
/// * `Err(ImgQualityError)` - Conversion failed
///
/// # Behavior
/// - Uses `cjxl --lossless_jpeg=1` for bitstream reconstruction
/// - On reconstruction failure: strips JPEG tail and retries
/// - On corruption: uses `ImageMagick` fallback to sanitize
/// - Verifies JXL health and checks size tolerance
///
/// # Fallback Chain
/// 1. Primary: cjxl with lossless JPEG mode
/// 2. Strip JPEG tail → retry
/// 3. Use --`allow_jpeg_reconstruction=0`
/// 4. `ImageMagick` sanitization (for corrupt JPEGs)
///
/// Transcode JPEG to JXL losslessly (reconstructible).
///
/// # Errors
/// Returns an error if transcoding fails.
pub fn convert_jpeg_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    // Check for corruption early
    if !is_jpeg_complete(&std::fs::read(input).unwrap_or_default()) {
        return Err(ImgQualityError::ConversionError(
            "JPEG is truncated or missing EOI".to_string(),
        ));
    }

    // Check for UltraHDR JPEG and skip conversion
    if shared_utils::image_jpeg_analysis::is_ultra_hdr_jpeg_file(input) {
        shared_utils::progress_mode::emit_stderr(&format!(
            "   🌈 UltraHDR detected: {} - skipping JXL encoding (tool limitation) and copying original",
            input.file_name().unwrap_or_default().to_string_lossy()
        ));

        let input_size = fs::metadata(input)?.len();
        copy_original_on_skip(input, options);
        mark_as_processed(input);
        return Ok(ConversionResult::skipped_custom(
            input,
            input_size,
            "UltraHDR JPEG",
            "Skipped due to cjxl gainmap incompatibility",
        ));
    }

    // Standard JPEG conversion (non-UltraHDR)
    tracing::trace!(
        "UltraHDR not detected for {}: performing standard JPEG transcoding",
        input.file_name().unwrap_or_default().to_string_lossy()
    );

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if output.exists() && !options.force() {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    let max_threads = shared_utils::thread_manager::get_optimal_threads();

    let result = run_cjxl_jpeg_transcode(input, &temp_output, options, max_threads, None, hdr_info);

    let output_cmd = match result {
        Ok(out) => out,
        Err(e) => {
            return Err(ImgQualityError::ToolNotFound(format!(
                "cjxl not found: {e}"
            )));
        }
    };

    if output_cmd.status.success() {
        return commit_jpeg_to_jxl_success(
            input,
            &temp_output,
            &output,
            input_size,
            options,
            "JPEG lossless",
        );
    }

    let stderr = String::from_utf8_lossy(&output_cmd.stderr);
    cleanup_temp_output(&temp_output, input);

    if is_jpeg_reconstruction_cjxl_error(&stderr) {
        // 1) Fix: strip trailing data after JPEG EOI so cjxl can use bitstream reconstruction
        let (source_to_use, _guard): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
            match shared_utils::jxl_utils::strip_jpeg_tail_to_temp(input) {
                Ok(Some((cleaned, guard))) => {
                    if options.verbose() {
                        eprintln!("   🔧 Stripped JPEG tail; retrying with original cjxl flags");
                    }
                    (cleaned, Some(guard))
                }
                _ => (input.to_path_buf(), None),
            };

        // 2) Retry with original cjxl flags (no --allow_jpeg_reconstruction 0) on fixed or original
        let retry_original = run_cjxl_jpeg_transcode(
            &source_to_use,
            &temp_output,
            options,
            max_threads,
            None,
            hdr_info,
        );
        if let Ok(out) = retry_original {
            if out.status.success() {
                let label = if source_to_use == input {
                    "JPEG lossless"
                } else {
                    "JPEG lossless (sanitized tail)"
                };
                return commit_jpeg_to_jxl_success(
                    input,
                    &temp_output,
                    &output,
                    input_size,
                    options,
                    label,
                );
            }
        }
        cleanup_temp_output(&temp_output, input);

        // 3) Fallback: --allow_jpeg_reconstruction 0 (no bitstream reconstruction, often larger)
        let retry_no_recon = run_cjxl_jpeg_transcode(
            &source_to_use,
            &temp_output,
            options,
            max_threads,
            Some(0),
            hdr_info,
        );
        if let Ok(out) = retry_no_recon {
            if out.status.success() {
                return commit_jpeg_to_jxl_success(
                    input,
                    &temp_output,
                    &output,
                    input_size,
                    options,
                    "JPEG lossless (--allow_jpeg_reconstruction 0)",
                );
            }
        }
        cleanup_temp_output(&temp_output, input);
        return Err(ImgQualityError::ConversionError(format!(
            "cjxl JPEG transcode failed (fix + retry and --allow_jpeg_reconstruction 0 both failed): {stderr}"
        )));
    }

    if stderr.contains("Error while decoding")
        || stderr.contains("Corrupt JPEG")
        || stderr.contains("Premature end")
    {
        // For truncated JPEGs, the ImageMagick fallback often "repairs" them but results in
        // large JXL files that we eventually discard. We skip fallback if it's incomplete.
        if !is_jpeg_complete(&std::fs::read(input).unwrap_or_default()) {
            shared_utils::progress_mode::emit_stderr(
                "   ⚠️  [Corruption] JPEG file is truncated or missing EOI, skipping expensive fallback.",
            );
            return Err(ImgQualityError::ConversionError(format!(
                "JPEG is truncated or missing EOI, and cjxl bitstream reconstruction failed: {stderr}"
            )));
        }

        match shared_utils::jxl_utils::try_imagemagick_fallback(
            input,
            &temp_output,
            0.0,
            max_threads,
            options.apple_compat(),
            options.ultimate(),
        ) {
            Ok(()) => commit_jpeg_to_jxl_success(
                input,
                &temp_output,
                &output,
                input_size,
                options,
                "JPEG (Sanitized) -> JXL",
            ),
            Err(e) => Err(ImgQualityError::ConversionError(format!(
                "Fallback failed after JPEG corruption: {e}"
            ))),
        }
    } else {
        shared_utils::progress_mode::emit_stderr(
            "   🔄 JPEG transcode failed, trying ImageMagick pipeline...",
        );
        match shared_utils::jxl_utils::try_imagemagick_fallback(
            input,
            &temp_output,
            0.0,
            max_threads,
            options.apple_compat(),
            options.ultimate(),
        ) {
            Ok(()) => commit_jpeg_to_jxl_success(
                input,
                &temp_output,
                &output,
                input_size,
                options,
                "JPEG -> JXL (ImageMagick fallback)",
            ),
            Err(_) => Err(ImgQualityError::ConversionError(format!(
                "cjxl JPEG transcode failed: {stderr}"
            ))),
        }
    }
}

/// Convert an image to AVIF format with specified quality.
///
/// # Arguments
/// * `input` - Path to the input image file
/// * `quality` - AVIF quality (0-100, None = 85)
/// * `options` - Conversion options
///
/// # Returns
/// * `Ok(ConversionResult)` - Conversion result
/// * `Err(ImgQualityError)` - Conversion failed
///
/// # Behavior
/// - Uses avifenc with speed 4 and all threads
/// - Verifies AVIF health after encoding
/// - Checks size tolerance and compress mode
///
/// Convert to AVIF using specific quality.
///
/// # Errors
/// Returns an error if avifenc execution fails.
pub fn convert_to_avif(
    input: &Path,
    quality: Option<u8>,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "avif", options)?;

    if output.exists() && !options.force() {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    let q = quality.unwrap_or(85);

    let mut builder = shared_utils::AvifencBuilder::new();
    builder
        .speed(4)
        .jobs("all")
        .quality(q, q)
        .input(input)
        .output(&temp_output);

    let result = builder.build().output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();
            if let Err(e) = shared_utils::avif_av1_health::verify_avif_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(ImgQualityError::ConversionError(format!(
                    "AVIF health check failed: {e}"
                )));
            }
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
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::ToolNotFound(format!(
                "avifenc not found: {e}"
            )))
        }
    }
}

/// Convert to AVIF losslessly.
///
/// # Errors
/// Returns an error if encoding fails.
pub fn convert_to_avif_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if options.verbose() {
        eprintln!("⚠️  Mathematical lossless AVIF encoding - this will be SLOW!");
    }

    if !options.force() && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "avif", options)?;

    if output.exists() && !options.force() {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    let mut builder = shared_utils::AvifencBuilder::new();
    builder
        .lossless(true)
        .speed(4)
        .jobs("all")
        .input(input)
        .output(&temp_output);

    let result = builder.build().output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();
            if let Err(e) = shared_utils::avif_av1_health::verify_avif_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(ImgQualityError::ConversionError(format!(
                    "Lossless AVIF health check failed: {e}"
                )));
            }
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
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::ToolNotFound(format!(
                "avifenc not found: {e}"
            )))
        }
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
            "Quality analysis failed: {e}"
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
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force() {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    let distance = calculate_matched_distance_for_static(analysis, input_size)?;
    eprintln!("   🎯 Matched JXL distance: {distance:.2}");

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        shared_utils::thread_manager::get_optimal_threads()
    };

    let actual_dist = shared_utils::constants::jxl_distance_for_mode(distance, options.ultimate());
    let actual_eff = shared_utils::constants::jxl_effort_for_mode(options.ultimate());

    let mut builder = shared_utils::CjxlBuilder::new();
    builder
        .input(input)
        .output(&temp_output)
        .distance(actual_dist)
        .effort(actual_eff)
        .threads(max_threads)
        .apple_compat(options.apple_compat());

    tracing::debug!(
        file = %input.display(),
        distance = distance,
        threads = max_threads,
        "Encoding quality-matched JXL: starting cjxl"
    );

    if distance > 0.0 {
        let is_jpeg = options
            .input_format
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("jpeg") || f.eq_ignore_ascii_case("jpg"));
        if is_jpeg {
            builder.lossless_jpeg(false);
        }
    }

    let result = builder.build().output();

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
                Some(&extra),
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

const fn jxl_screening_effort(ultimate: bool, explore: bool) -> u8 {
    if ultimate && explore {
        shared_utils::constants::JXL_DEFAULT_EFFORT
    } else {
        shared_utils::constants::jxl_effort_for_mode(ultimate)
    }
}

fn try_imagemagick_fallback_with_effort(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
    apple_compat: bool,
    effort: u8,
) -> std::result::Result<(), std::io::Error> {
    shared_utils::jxl_utils::try_imagemagick_fallback_with_effort(
        input,
        output,
        distance,
        effort,
        max_threads,
        apple_compat,
    )
}

fn encode_direct_jxl_probe_with_effort(
    input: &Path,
    output: &Path,
    distance: f32,
    effort: u8,
    max_threads: usize,
    apple_compat: bool,
    icc_path: Option<&Path>,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> std::result::Result<(), String> {
    let mut builder = shared_utils::CjxlBuilder::new();
    builder
        .input(input)
        .output(output)
        .distance(distance)
        .effort(effort)
        .threads(max_threads)
        .apple_compat(apple_compat);

    if let Some(hdr) = hdr_info {
        if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
            builder.cicp(&cicp);
        }
    }

    if let Some(icc) = icc_path {
        builder.icc_profile(icc);
    }

    let output = builder
        .build()
        .output()
        .map_err(|e| format!("Failed to run cjxl probe: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err("cjxl probe failed without stderr output".to_string())
        } else {
            Err(stderr)
        }
    }
}

fn encode_jxl_probe_to_output(
    input: &Path,
    actual_input: &Path,
    output: &Path,
    distance: f32,
    effort: u8,
    max_threads: usize,
    apple_compat: bool,
    icc_path: Option<&Path>,
    hdr_info: Option<&shared_utils::ColorInfo>,
    stage_label: &str,
) -> std::result::Result<u64, String> {
    let _ = shared_utils::io_utils::safe_remove_file(output);

    let mut direct_encode = |candidate_distance| {
        encode_direct_jxl_probe_with_effort(
            actual_input,
            output,
            candidate_distance,
            effort,
            max_threads,
            apple_compat,
            icc_path,
            hdr_info,
        )?;
        verify_jxl_health(output)
            .map_err(|err| format!("Health check failed after direct cjxl probe: {err}"))?;
        fs::metadata(output)
            .map(|meta| meta.len())
            .map_err(|e| e.to_string())
    };

    let mut fallback_encode = |candidate_distance| {
        let _ = shared_utils::io_utils::safe_remove_file(output);
        shared_utils::progress_mode::emit_stderr(&format!(
            "   🔄 {stage_label} d={}: cjxl failed, trying ImageMagick fallback at e{effort}",
            shared_utils::jxl_explorer::format_distance_for_log(candidate_distance)
        ));
        try_imagemagick_fallback_with_effort(
            input,
            output,
            candidate_distance,
            max_threads,
            apple_compat,
            effort,
        )
        .map_err(|e| e.to_string())?;
        verify_jxl_health(output).map_err(|err| {
            format!("Health check failed after ImageMagick exploration probe: {err}")
        })?;
        fs::metadata(output)
            .map(|meta| meta.len())
            .map_err(|e| e.to_string())
    };

    run_jxl_exploration_probe_with(distance, &mut direct_encode, &mut fallback_encode)
}

fn run_jxl_exploration_probe_with<Direct, Fallback>(
    distance: f32,
    direct_encode: &mut Direct,
    fallback_encode: &mut Fallback,
) -> std::result::Result<u64, String>
where
    Direct: FnMut(f32) -> std::result::Result<u64, String>,
    Fallback: FnMut(f32) -> std::result::Result<u64, String>,
{
    match direct_encode(distance) {
        Ok(size) => Ok(size),
        Err(direct_err) => fallback_encode(distance).map_err(|fallback_err| {
            format!(
                "JXL exploration probe failed at d={}: direct cjxl: {direct_err}; ImageMagick fallback: {fallback_err}",
                shared_utils::jxl_explorer::format_distance_for_log(distance)
            )
        }),
    }
}

fn compare_jxl_finalists(
    input_size: u64,
    left_distance: f32,
    left_size: u64,
    right_distance: f32,
    right_size: u64,
) -> std::cmp::Ordering {
    let left_smaller_than_input = left_size < input_size;
    let right_smaller_than_input = right_size < input_size;

    match (left_smaller_than_input, right_smaller_than_input) {
        (true, false) => return std::cmp::Ordering::Less,
        (false, true) => return std::cmp::Ordering::Greater,
        _ => {}
    }

    left_distance
        .total_cmp(&right_distance)
        .then_with(|| left_size.cmp(&right_size))
}

fn describe_jxl_finalist_pass(
    finalist: &shared_utils::jxl_explorer::JxlScreenedCandidate,
    screening: &shared_utils::jxl_explorer::JxlScreeningResult,
    input_size: u64,
) -> String {
    let distance = shared_utils::jxl_explorer::format_distance_for_log(finalist.distance);
    let ratio_pct = if input_size == 0 {
        100.0
    } else {
        let ratio = Rational::from((finalist.output_size, input_size));
        ratio.to_f64() * 100.0
    };
    let origin = if finalist.ladder_phase {
        "screened"
    } else {
        "refined"
    };
    let role = if finalist.distance <= shared_utils::constants::JXL_EXPLORE_FLOOR + f32::EPSILON {
        "rechecking the required floor"
    } else if (finalist.distance - screening.best_distance).abs() < f32::EPSILON {
        "rechecking the screened leader"
    } else if ratio_pct <= 105.0 {
        "verifying a break-even candidate"
    } else {
        "sampling a shortlist branch"
    };

    format!("{role}: d={distance} from the {origin} pass ({ratio_pct:.1}% of input at e7)")
}

fn try_explore_ultimate_jxl_distance(
    input: &Path,
    actual_input: &Path,
    temp_output: &Path,
    input_size: u64,
    initial_output_size: u64,
    max_threads: usize,
    options: &ConvertOptions,
    icc_path: Option<&Path>,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> Result<Option<shared_utils::jxl_explorer::JxlExploreResult>> {
    const MAX_CONTINUED_ITERATIONS: u32 = 20;
    shared_utils::progress_mode::emit_stderr(
        "   🔬 Ultimate JXL exploration: screening with e7, promoting a shortlist, finalizing with e10",
    );

    let screening_effort = jxl_screening_effort(true, true);
    let final_effort = shared_utils::constants::jxl_effort_for_mode(true);
    let screening = shared_utils::jxl_explorer::screen_jxl_candidates(
        input_size,
        initial_output_size,
        |distance| {
            let candidate_output =
                shared_utils::path_safety::isolated_temp_path_for_search(temp_output)
                    .map_err(|e| e.to_string())?;
            let result = encode_jxl_probe_to_output(
                input,
                actual_input,
                &candidate_output,
                distance,
                screening_effort,
                max_threads,
                options.apple_compat(),
                icc_path,
                hdr_info,
                "Screening probe",
            );
            let _ = shared_utils::io_utils::safe_remove_file(&candidate_output);
            result
        },
    );

    let Some(screening) = (match screening {
        Ok(result) => result,
        Err(err) => {
            shared_utils::progress_mode::emit_stderr(&format!(
                "   ⚠️ Ultimate JXL exploration aborted during e7 screening; keeping baseline encode: {err}"
            ));
            return Ok(None);
        }
    }) else {
        return Ok(None);
    };

    for line in &screening.log {
        shared_utils::progress_mode::emit_stderr(&format!("   {line}"));
    }

    let mut best_final: Option<(usize, u64, std::path::PathBuf)> = None;

    for (finalist_idx, finalist) in screening.finalists.iter().enumerate() {
        shared_utils::progress_mode::emit_stderr(&format!(
            "   🧪 e{} pass {}/{}: {}",
            final_effort,
            finalist_idx + 1,
            screening.finalists.len(),
            describe_jxl_finalist_pass(finalist, &screening, input_size)
        ));

        let candidate_output =
            shared_utils::path_safety::isolated_temp_path_for_search(temp_output)
                .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

        match encode_jxl_probe_to_output(
            input,
            actual_input,
            &candidate_output,
            finalist.distance,
            final_effort,
            max_threads,
            options.apple_compat(),
            icc_path,
            hdr_info,
            "Finalist encode",
        ) {
            Ok(size) => {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "      ↳ e{} result: {:.1}% of input",
                    final_effort,
                    if input_size == 0 {
                        100.0
                    } else {
                        let ratio = Rational::from((size, input_size.max(1)));
                        ratio.to_f64() * 100.0
                    }
                ));
                let replace_best = best_final.as_ref().is_none_or(|(best_idx, best_size, _)| {
                    compare_jxl_finalists(
                        input_size,
                        finalist.distance,
                        size,
                        screening.finalists.get(*best_idx).map_or(0.01, |f| f.distance),
                        *best_size,
                    ) == std::cmp::Ordering::Less
                });

                if replace_best {
                    if let Some((_, _, old_path)) =
                        best_final.replace((finalist_idx, size, candidate_output.clone()))
                    {
                        let _ = shared_utils::io_utils::safe_remove_file(&old_path);
                    }
                } else {
                    let _ = shared_utils::io_utils::safe_remove_file(&candidate_output);
                }
            }
            Err(err) => {
                let _ = shared_utils::io_utils::safe_remove_file(&candidate_output);
                shared_utils::progress_mode::emit_stderr(&format!(
                    "   ⚠️ e{} pass failed for d={}: {err}",
                    final_effort,
                    shared_utils::jxl_explorer::format_distance_for_log(finalist.distance)
                ));
            }
        }
    }

    let Some((best_idx, best_size, best_path)) = best_final else {
        shared_utils::progress_mode::emit_stderr(
            "   ⚠️ No e10 finalist succeeded; keeping the e7 screening baseline",
        );
        return Ok(None);
    };

    if best_size >= input_size {
        let _ = shared_utils::io_utils::safe_remove_file(&best_path);
        shared_utils::progress_mode::emit_stderr(&format!(
            "   ⚠️ All e10 finalists exceed input size (best={:.1}% of input); skipping JXL",
            if input_size == 0 {
                100.0
            } else {
                let ratio = Rational::from((best_size, input_size));
                ratio.to_f64() * 100.0
            }
        ));
        return Ok(None);
    }

    let best_candidate = screening.finalists.get(best_idx).ok_or_else(|| {
        ImgQualityError::ConversionError("Failed to find best JXL candidate in finalists".to_string())
    })?;
    let _ = shared_utils::io_utils::safe_remove_file(temp_output);
    shared_utils::io_utils::robust_move(&best_path, temp_output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    // ── Phase: Continued Downward Exploration at e10 ────────────────────
    // e10 has greater compression potential than e7. After finalists settle,
    // continue stepping distance downward (higher quality) to see if e10
    // can produce even smaller files at lower distances.
    // STRICT RULE: stop immediately on first size increase.
    let mut accepted_distance = best_candidate.distance;
    let mut accepted_size = best_size;
    {
        let floor = shared_utils::constants::JXL_EXPLORE_FLOOR;
        // Adaptive step: use 1/10th of current distance, clamped to sane bounds
        let step = (accepted_distance / 10.0).clamp(
            shared_utils::constants::JXL_EXPLORE_BINARY_SEARCH_PRECISION,
            0.01,
        );
        let mut test_distance = accepted_distance - step;
        let mut continued_iterations = 0u32;

        if test_distance >= floor {
            shared_utils::progress_mode::emit_stderr(&format!(
                "   🔬 Continued e{} exploration: stepping down from d={} (step={})",
                final_effort,
                shared_utils::jxl_explorer::format_distance_for_log(accepted_distance),
                shared_utils::jxl_explorer::format_distance_for_log(step),
            ));
        }

        while test_distance >= floor && continued_iterations < MAX_CONTINUED_ITERATIONS {
            // Canonicalize to avoid float drift
            let candidate_distance =
                shared_utils::jxl_explorer::clamp_explore_distance(test_distance);

            let candidate_output =
                shared_utils::path_safety::isolated_temp_path_for_search(temp_output)
                    .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

            match encode_jxl_probe_to_output(
                input,
                actual_input,
                &candidate_output,
                candidate_distance,
                final_effort,
                max_threads,
                options.apple_compat(),
                icc_path,
                hdr_info,
                "Continued exploration",
            ) {
                Ok(size) => {
                    continued_iterations += 1;
                    let pct = if input_size == 0 {
                        100.0
                    } else {
                        let ratio = Rational::from((size, input_size.max(1)));
                        ratio.to_f64() * 100.0
                    };

                    if size < accepted_size {
                        // Progress: smaller file at lower distance
                        shared_utils::progress_mode::emit_stderr(&format!(
                            "      ✓ d={} -> {:.1}% of input (gain)",
                            shared_utils::jxl_explorer::format_distance_for_log(candidate_distance),
                            pct
                        ));
                        accepted_distance = candidate_distance;
                        accepted_size = size;
                        // Move the new best to temp_output
                        let _ = shared_utils::io_utils::safe_remove_file(temp_output);
                        shared_utils::io_utils::robust_move(&candidate_output, temp_output)
                            .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
                        test_distance -= step;
                    } else {
                        // Size increased or stayed the same — stop immediately
                        shared_utils::progress_mode::emit_stderr(&format!(
                            "      ✗ d={} -> {:.1}% of input (size increased, stopping)",
                            shared_utils::jxl_explorer::format_distance_for_log(candidate_distance),
                            pct
                        ));
                        let _ = shared_utils::io_utils::safe_remove_file(&candidate_output);
                        break;
                    }
                }
                Err(err) => {
                    let _ = shared_utils::io_utils::safe_remove_file(&candidate_output);
                    shared_utils::progress_mode::emit_stderr(&format!(
                        "      ⚠️ Continued exploration probe failed at d={}: {err}",
                        shared_utils::jxl_explorer::format_distance_for_log(candidate_distance)
                    ));
                    break;
                }
            }
        }

        if continued_iterations > 0 && accepted_distance < best_candidate.distance {
            shared_utils::progress_mode::emit_stderr(&format!(
                "   🎯 Continued exploration improved: d={} -> d={} ({} probes)",
                shared_utils::jxl_explorer::format_distance_for_log(best_candidate.distance),
                shared_utils::jxl_explorer::format_distance_for_log(accepted_distance),
                continued_iterations
            ));
        }
    }

    let mut log = screening.log.clone();
    log.push(format!(
        "Accepted e10 finalist d={} -> {:.1}% of input",
        shared_utils::jxl_explorer::format_distance_for_log(accepted_distance),
        if input_size == 0 {
            100.0
        } else {
            let ratio = Rational::from((accepted_size, input_size));
            ratio.to_f64() * 100.0
        }
    ));

    let result = shared_utils::jxl_explorer::JxlExploreResult {
        accepted_distance,
        output_size: accepted_size,
        iterations: screening.iterations,
        ladder_phase: best_candidate.ladder_phase,
        screened_best_distance: screening.best_distance,
        screened_best_size: screening.best_output_size,
        promoted_distances: screening
            .finalists
            .iter()
            .map(|candidate| candidate.distance)
            .collect(),
        log,
        initial_ratio: screening.initial_ratio,
        pressure_stops: screening.pressure_stops,
        profile_label: screening.profile_label,
        target_distance: screening.target_distance,
    };

    shared_utils::progress_mode::emit_stderr(&format!(
        "   ✅ Ultimate JXL exploration accepted d={} after e7 screening / e10 finalization ({:.1}% of input)",
        shared_utils::jxl_explorer::format_distance_for_log(result.accepted_distance),
        if input_size == 0 {
            100.0
        } else {
            let ratio = Rational::from((result.output_size, input_size));
            ratio.to_f64() * 100.0
        }
    ));
    shared_utils::progress_mode::emit_stderr(&format!(
        "   TELEMETRY: outcome_distance={} outcome_pct={:.1} profile={} pressure_stops={:.4}",
        shared_utils::jxl_explorer::format_distance_for_log(result.accepted_distance),
        if input_size == 0 {
            100.0
        } else {
            let ratio = Rational::from((result.output_size, input_size));
            ratio.to_f64() * 100.0
        },
        result.profile_label,
        result.pressure_stops
    ));
    Ok(Some(result))
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
    let mut is_float = hdr_info.is_float;

    // Safety Fallback: Use extension as a hint if ffprobe failed to detect float
    if !is_float {
        if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_lowercase();
            if ext_lower == "exr" || ext_lower == "hdr" {
                is_float = true;
                if options.verbose() {
                    tracing::warn!(input = %input.display(), "ffprobe float detection failed, using extension hint (EXR/HDR)");
                }
            }
        }
    }

    let is_high_bit_depth =
        hdr_info.bit_depth.is_some_and(|d| d > 8) || hdr_info.is_hdr() || is_float;

    // Safety Fallback: Use extension as a hint for high-bit integer if ffprobe failed
    let mut bit_depth = hdr_info.bit_depth;
    if bit_depth.is_none() && !is_float {
        if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_lowercase();
            if ext_lower == "tif" || ext_lower == "tiff" || ext_lower == "dng" {
                bit_depth = Some(16); // Safe assumption for these pro formats
                if options.verbose() {
                    tracing::warn!(input = %input.display(), "ffprobe bit-depth detection failed, using extension hint (16-bit TIFF/DNG)");
                }
            }
        }
    }

    let depth_str = if is_float {
        "32"
    } else if is_high_bit_depth || bit_depth.is_some_and(|d| d > 8) {
        "16"
    } else {
        "8"
    };
    let intermediate_suffix = if is_float { ".exr" } else { ".png" };

    if is_float && options.verbose() {
        use console::style;
        eprintln!(
            "   {} Source is 32-bit float (HDR), using OpenEXR intermediate format",
            style("🚀").cyan().bold()
        );
    } else if is_high_bit_depth && options.verbose() {
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
                // Fall through to standard decoding
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

                let temp_file = tempfile::Builder::new()
                    .suffix(intermediate_suffix)
                    .tempfile()?;
                let temp_path = temp_file.path().to_path_buf();

                let mut builder = shared_utils::MagickBuilder::new();
                builder.input(input).output(&temp_path);

                if is_float {
                    builder.format("exr");
                }

                if let Ok(depth) = depth_str.parse::<u8>() {
                    builder.depth(depth);
                }

                let result = builder.build().output();

                match result {
                    Ok(output) if output.status.success() && temp_path.exists() => {
                        let label = if is_float {
                            "OpenEXR"
                        } else {
                            "ImageMagick PNG"
                        };
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

        "webp" => {
            use console::style;
            eprintln!(
                "   {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style("WebP detected, using dwebp for ICC profile compatibility").dim()
            );

            let temp_png_file = tempfile::Builder::new().suffix(".png").tempfile()?;
            let temp_png = temp_png_file.path().to_path_buf();

            let mut builder = shared_utils::image_builders::DwebpBuilder::new();
            builder.input(input).output(&temp_png);

            let result = builder.build().output();

            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    shared_utils::progress_mode::preprocessing_success();
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    let line = format!(
                        "   {} {} {}",
                        style("🔧 PRE-PROCESSING:").cyan().bold(),
                        style("WebP").dim(),
                        style("→ ⚠️ failed, trying direct cjxl").yellow()
                    );
                    shared_utils::progress_mode::emit_stderr(&line);
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        "tiff" | "tif" => {
            let label = if is_float {
                "32-bit float OpenEXR"
            } else {
                &format!("{depth_str}-bit PNG")
            };

            let temp_file = tempfile::Builder::new()
                .suffix(intermediate_suffix)
                .tempfile()
                .map_err(ImgQualityError::IoError)?;
            let temp_path = temp_file.path().to_path_buf();

            let mut builder = shared_utils::MagickBuilder::new();
            builder.input(input).output(&temp_path);
            if is_float {
                builder.format("exr");
            }
            if let Ok(depth) = depth_str.parse::<u8>() {
                builder.depth(depth);
            }
            let out = builder.build().output().map_err(ImgQualityError::IoError)?;
            if out.status.success() && temp_path.exists() {
                if options.verbose() {
                    eprintln!("   ✅ TIFF detected, using ImageMagick to emit {label}");
                }
                Ok((temp_path, Some(temp_file)))
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(ImgQualityError::ConversionError(format!(
                    "magick TIFF conversion failed: {err}"
                )))
            }
        }

        "bmp" => {
            let label = if is_float {
                "32-bit float OpenEXR"
            } else {
                &format!("{depth_str}-bit PNG")
            };

            let temp_file = tempfile::Builder::new()
                .suffix(intermediate_suffix)
                .tempfile()
                .map_err(ImgQualityError::IoError)?;
            let temp_path = temp_file.path().to_path_buf();

            let mut builder = shared_utils::MagickBuilder::new();
            builder.input(input).output(&temp_path);
            if is_float {
                builder.format("exr");
            }
            if let Ok(depth) = depth_str.parse::<u8>() {
                builder.depth(depth);
            }
            let out = builder.build().output().map_err(ImgQualityError::IoError)?;
            if out.status.success() && temp_path.exists() {
                if options.verbose() {
                    eprintln!("   ✅ BMP detected, using ImageMagick to emit {label}");
                }
                Ok((temp_path, Some(temp_file)))
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(ImgQualityError::ConversionError(format!(
                    "magick BMP conversion failed: {err}"
                )))
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
            let mut builder = shared_utils::SipsBuilder::new();
            builder.format("png").input(input).output(&temp_png);

            let result = builder.build().output();

            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!("   ✅ sips HEIC pre-processing successful");
                    // sips doesn't easily support forcing 16-bit depth for PNG
                    // If we need 16-bit, we might want to sanitize with magick afterwards if depth was lost,
                    // but for now we trust sips for standard HEIC.
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    eprintln!("   ⚠️  sips failed, trying ImageMagick...");
                    let temp_file = tempfile::Builder::new()
                        .suffix(intermediate_suffix)
                        .tempfile()?;
                    let temp_path = temp_file.path().to_path_buf();

                    let mut builder = shared_utils::MagickBuilder::new();
                    builder.input(input).output(&temp_path);

                    if is_float {
                        builder.format("exr");
                    }

                    if let Ok(depth) = depth_str.parse::<u8>() {
                        builder.depth(depth);
                    }

                    match builder.build().output() {
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

        "gif" => {
            use console::style;
            eprintln!(
                "   {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style("GIF detected, using FFmpeg for static frame extraction").dim()
            );

            let temp_png_file = tempfile::Builder::new().suffix(".png").tempfile()?;
            let temp_png = temp_png_file.path().to_path_buf();

            let mut builder = shared_utils::FfmpegBuilder::new();
            builder
                .overwrite()
                .input(input)
                .frames_v(1)
                .output(&temp_png);

            let result = builder.build().output();

            match result {
                Ok(out) if out.status.success() && temp_png.exists() => {
                    shared_utils::progress_mode::preprocessing_success();
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    let line = format!(
                        "   {} {} {}",
                        style("🔧 PRE-PROCESSING:").cyan().bold(),
                        style("GIF").dim(),
                        style("→ ⚠️ failed, trying direct cjxl").yellow()
                    );
                    shared_utils::progress_mode::emit_stderr(&line);
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        _ => {
            if let Some(actual_ext) = input.extension().and_then(|e| e.to_str()) {
                if actual_ext.to_lowercase() == ext {
                    Ok((input.to_path_buf(), None))
                } else {
                    eprintln!(
                        "   🔧 PRE-PROCESSING: Extension mismatch detected (.{actual_ext} vs {ext}), creating aligned temp file"
                    );

                    let temp_aligned_file = tempfile::Builder::new()
                        .suffix(&format!(".{ext}"))
                        .tempfile()?;
                    let temp_path = temp_aligned_file.path().to_path_buf();

                    if std::fs::copy(input, &temp_path).is_ok() {
                        Ok((temp_path, Some(temp_aligned_file)))
                    } else {
                        Ok((input.to_path_buf(), None))
                    }
                }
            } else {
                Ok((input.to_path_buf(), None))
            }
        }
    }
}

fn get_output_path(
    input: &Path,
    extension: &str,
    options: &ConvertOptions,
) -> Result<std::path::PathBuf> {
    let output = if let Some(ref base) = options.base_dir {
        shared_utils::conversion::determine_output_path_with_base(
            input,
            base,
            extension,
            &options.output_dir,
        )
        .map_err(ImgQualityError::ConversionError)?
    } else {
        shared_utils::conversion::determine_output_path(input, extension, &options.output_dir)
            .map_err(ImgQualityError::ConversionError)?
    };

    // Validate output path (check path traversal, symlinks)
    shared_utils::conversion::validate_output_path(&output, options.base_dir.as_deref())
        .map_err(ImgQualityError::ConversionError)?;

    Ok(output)
}

fn verify_jxl_health(path: &Path) -> Result<()> {
    shared_utils::jxl_utils::verify_jxl_health(path).map_err(ImgQualityError::ConversionError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use tempfile::tempdir;
    use vid::animated_image::is_high_quality_animated;

    #[test]
    fn test_get_output_path() {
        let tmp = tempdir().unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let root = std::fs::canonicalize(tmp.path()).unwrap_or_else(|e| panic!("canonicalize: {e:?}"));
        let input_dir = root.join("path").join("to");
        std::fs::create_dir_all(&input_dir).unwrap_or_else(|e| panic!("create input dir: {e:?}"));
        let input = input_dir.join("image.png");
        std::fs::write(&input, b"png").unwrap_or_else(|e| panic!("write input file: {e:?}"));
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let output = get_output_path(&input, "jxl", &options).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(output, input_dir.join("image.JXL"));
    }

    #[test]
    fn test_get_output_path_with_dir() {
        let tmp = tempdir().unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let root = std::fs::canonicalize(tmp.path()).unwrap_or_else(|e| panic!("canonicalize: {e:?}"));
        let input_dir = root.join("path").join("to");
        std::fs::create_dir_all(&input_dir).unwrap_or_else(|e| panic!("create input dir: {e:?}"));
        let input = input_dir.join("image.png");
        std::fs::write(&input, b"png").unwrap_or_else(|e| panic!("write input file: {e:?}"));
        let output_dir = root.join("output");
        let options = ConvertOptions {
            output_dir: Some(output_dir.clone()),
            base_dir: None,
            ..Default::default()
        };
        let output = get_output_path(&input, "avif", &options).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(output, output_dir.join("image.AVIF"));
    }

    #[test]
    fn test_get_output_path_same_file_error() {
        let tmp = tempdir().unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let root = std::fs::canonicalize(tmp.path()).unwrap_or_else(|e| panic!("canonicalize: {e:?}"));
        let input = root.join("image.JXL");
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let result = get_output_path(&input, "jxl", &options);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_high_quality_720p() {
        assert!(is_high_quality_animated(1280, 720));
    }

    #[test]
    fn test_is_high_quality_1080p() {
        assert!(is_high_quality_animated(1920, 1080));
    }

    #[test]
    fn test_is_high_quality_width_only() {
        assert!(is_high_quality_animated(1280, 480));
    }

    #[test]
    fn test_is_high_quality_height_only() {
        assert!(is_high_quality_animated(960, 720));
    }

    #[test]
    fn test_is_high_quality_total_pixels() {
        assert!(is_high_quality_animated(1024, 900));
    }

    #[test]
    fn test_is_not_high_quality_small() {
        assert!(!is_high_quality_animated(640, 480));
    }

    #[test]
    fn test_is_not_high_quality_480p() {
        assert!(!is_high_quality_animated(854, 480));
    }

    #[test]
    fn test_is_not_high_quality_typical_gif() {
        assert!(!is_high_quality_animated(400, 300));
        assert!(!is_high_quality_animated(500, 500));
        assert!(!is_high_quality_animated(320, 240));
    }

    fn should_convert_to_video_format(duration: f32, width: u32, height: u32) -> bool {
        const DURATION_THRESHOLD: f32 = 3.0;
        duration >= DURATION_THRESHOLD || is_high_quality_animated(width, height)
    }

    #[test]
    fn test_apple_compat_routing_short_low_quality() {
        assert!(
            !should_convert_to_video_format(2.0, 400, 300),
            "Short animation (2s) + low quality (400x300) should convert to GIF"
        );
    }

    #[test]
    fn test_apple_compat_routing_short_high_quality() {
        assert!(
            should_convert_to_video_format(2.0, 1920, 1080),
            "Short animation (2s) + high quality (1920x1080) should convert to video"
        );
    }

    #[test]
    fn test_apple_compat_routing_long_low_quality() {
        assert!(
            should_convert_to_video_format(5.0, 400, 300),
            "Long animation (5s) should convert to video regardless of quality"
        );
    }

    #[test]
    fn test_apple_compat_routing_boundary_3_seconds() {
        assert!(
            should_convert_to_video_format(3.0, 400, 300),
            "Exactly 3 seconds should convert to video"
        );
    }

    #[test]
    fn test_apple_compat_routing_boundary_under_3_seconds() {
        assert!(
            !should_convert_to_video_format(2.99, 400, 300),
            "2.99s + low quality should convert to GIF"
        );
    }

    #[test]
    fn test_format_classification_no_overlap() {
        let preprocess_formats = ["webp", "tiff", "tif", "bmp", "heic", "heif"];
        let direct_formats = ["png", "jpg", "jpeg", "gif", "jxl", "avif"];

        for fmt in &preprocess_formats {
            assert!(
                !direct_formats.contains(fmt),
                "Format '{fmt}' appears in both preprocess and direct format lists; configuration error"
            );
        }
    }

    #[test]
    fn test_jxl_exploration_probe_uses_imagemagick_fallback() {
        let direct_calls = Cell::new(0);
        let fallback_calls = Cell::new(0);

        let mut direct = |distance: f32| {
            direct_calls.set(direct_calls.get() + 1);
            assert!((distance - 0.2).abs() < f32::EPSILON);
            Err("direct cjxl failed".to_string())
        };
        let mut fallback = |distance: f32| {
            fallback_calls.set(fallback_calls.get() + 1);
            assert!((distance - 0.2).abs() < f32::EPSILON);
            Ok(88)
        };

        let size = run_jxl_exploration_probe_with(0.2, &mut direct, &mut fallback)
            .unwrap_or_else(|e| panic!("fallback should recover the exploration probe: {e:?}"));

        assert_eq!(size, 88);
        assert_eq!(direct_calls.get(), 1);
        assert_eq!(fallback_calls.get(), 1);
    }

    #[test]
    fn test_jxl_exploration_probe_skips_fallback_after_direct_success() {
        let direct_calls = Cell::new(0);
        let fallback_calls = Cell::new(0);

        let mut direct = |_distance: f32| {
            direct_calls.set(direct_calls.get() + 1);
            Ok(77)
        };
        let mut fallback = |_distance: f32| {
            fallback_calls.set(fallback_calls.get() + 1);
            Ok(55)
        };

        let size = run_jxl_exploration_probe_with(0.1, &mut direct, &mut fallback)
            .unwrap_or_else(|e| panic!("direct cjxl probe should win: {e:?}"));

        assert_eq!(size, 77);
        assert_eq!(direct_calls.get(), 1);
        assert_eq!(fallback_calls.get(), 0);
    }

    #[test]
    fn test_jxl_screening_effort_only_drops_to_e7_for_ultimate_explore() {
        assert_eq!(jxl_screening_effort(true, true), 7);
        assert_eq!(jxl_screening_effort(true, false), 10);
        assert_eq!(jxl_screening_effort(false, true), 7);
        assert_eq!(jxl_screening_effort(false, false), 7);
    }

    #[test]
    fn test_jxl_final_round_prefers_lower_distance_once_size_beats_source() {
        assert_eq!(
            compare_jxl_finalists(9_000_000, 0.01, 8_800_000, 0.1, 7_500_000),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_jxl_final_round_requires_beating_source_before_quality_preference() {
        assert_eq!(
            compare_jxl_finalists(9_000_000, 0.01, 9_200_000, 0.1, 8_900_000),
            std::cmp::Ordering::Greater
        );
    }
}
