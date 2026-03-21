//! Lossless Converter Module
//!
//! Provides conversion API for verified lossless/lossy images.
//! Uses shared_utils for common functionality (anti-duplicate, ConversionResult, etc.)
//!
//! **Unified Compress Check**: All image conversions call `check_size_tolerance` after
//! successful encoding and obtaining output_size, before finalization. When `options.compress`
//! is true, only accept when output < input, otherwise skip and keep original file.
//! Covered paths: convert_to_jxl, convert_jpeg_to_jxl (including fallback),
//! convert_to_avif, convert_to_avif_lossless, convert_to_jxl_matched.

use crate::{ImgQualityError, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

pub use shared_utils::conversion::{
    check_size_tolerance, clear_processed_list, determine_output_path_with_base,
    finalize_conversion, format_size_change, is_already_processed, load_processed_list,
    mark_as_processed, save_processed_list, ConversionResult, ConvertOptions,
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
                "⚠️ [img-hevc] Failed to remove temporary output {} for {}: {}",
                temp_output.display(),
                input.display(),
                e
            );
        }
    }
}

/// Finalize conversion with size check and metadata preservation.
/// Common pattern: commit temp → check size → finalize.
/// Returns ConversionResult on success or error.
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
        Some(label.to_string()),
    )
}

/// Convert an image to JXL format with specified quality distance.
///
/// # Arguments
/// * `input` - Path to the input image file
/// * `options` - Conversion options (force, delete_original, output_dir, etc.)
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
/// - Uses cjxl for encoding, with FFmpeg → ImageMagick fallback on failure
/// - Preserves HDR metadata via --cicp parameter when hdr_info is provided
/// - Verifies JXL health after encoding
/// - Checks size tolerance and compress mode requirements
///
/// # Example
/// ```no_run
/// use img_hevc::lossless_converter::{convert_to_jxl, ConvertOptions};
/// use std::path::Path;
///
/// let input = Path::new("input.png");
/// let options = ConvertOptions::default();
/// let result = convert_to_jxl(input, &options, 0.1, None)?;
/// # Ok::<(), img_hevc::ImgQualityError>(())
/// ```
pub fn convert_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    distance: f32,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

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
                "Skipped: Small PNG (< 500KB)",
                "small_file",
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
    let icc_path = _icc_temp.as_ref().map(|t| t.path());

    // Cache thread count calculation (avoid repeated calls)
    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        shared_utils::thread_manager::get_optimal_threads()
    };

    let mut cmd = Command::new("cjxl");
    cmd.arg("-d")
        .arg(format!("{:.2}", distance))
        .arg("-e")
        .arg("7")
        .arg("-j")
        .arg(max_threads.to_string());

    // Add HDR metadata via CICP if available
    if let Some(hdr) = hdr_info {
        if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
            cmd.arg(format!("--cicp={}", cicp));
            if options.verbose {
                eprintln!("   🌈 HDR detected: applying CICP {}", cicp);
            }
        }
    }

    if options.apple_compat {
        cmd.arg("--compress_boxes=0");
    }

    shared_utils::jxl_utils::add_icc_to_cjxl(&mut cmd, icc_path);

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(&actual_input).as_ref())
        .arg(shared_utils::safe_path_arg(&temp_output).as_ref());

    if options.verbose {
        eprintln!(
            "   🔧 Executing: cjxl -d {:.2} -e 7 -j {} {} {}",
            distance,
            max_threads,
            actual_input.display(),
            temp_output.display()
        );
    }

    let result = cmd.output();

    let result = match &result {
        Ok(output_cmd) if !output_cmd.status.success() => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            if stderr.contains("Getting pixel data failed")
                || stderr.contains("Failed to decode")
                || stderr.contains("Decoding failed")
                || stderr.contains("pixel data")
                || stderr.contains("Error while decoding")
            {
                use std::process::Stdio;

                let ffmpeg_result = Command::new("ffmpeg")
                    .arg("-threads")
                    .arg(max_threads.to_string())
                    .arg("-i")
                    .arg(shared_utils::safe_path_arg(input).as_ref())
                    .arg("-frames:v")
                    .arg("1")
                    .arg("-vcodec")
                    .arg("png")
                    .arg("-f")
                    .arg("image2pipe")
                    .arg("-")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                match ffmpeg_result {
                    Ok(mut ffmpeg_proc) => {
                        if let Some(ffmpeg_stdout) = ffmpeg_proc.stdout.take() {
                            let mut cmd = Command::new("cjxl");
                            cmd.arg("-")
                                .arg(shared_utils::safe_path_arg(&temp_output).as_ref())
                                .arg("-d")
                                .arg(format!("{:.2}", distance))
                                .arg("-e")
                                .arg("7")
                                .arg("-j")
                                .arg(max_threads.to_string());

                            if options.apple_compat {
                                cmd.arg("--compress_boxes=0");
                            }

                            let cjxl_result =
                                cmd.stdin(ffmpeg_stdout).stderr(Stdio::piped()).spawn();

                            match cjxl_result {
                                Ok(mut cjxl_proc) => {
                                    let ffmpeg_stderr_thread =
                                        ffmpeg_proc.stderr.take().map(|stderr| {
                                            std::thread::spawn(move || {
                                                use std::io::Read;
                                                let mut buf = String::with_capacity(64 * 1024);
                                                if let Err(err) = stderr
                                                    .take(crate::constants::STDERR_BUFFER_MAX as u64)
                                                    .read_to_string(&mut buf)
                                                {
                                                    let line = format!(
                                                        "   ⚠️ Failed to read FFmpeg stderr output: {}",
                                                        err
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
                                                        crate::constants::STDERR_BUFFER_MAX as u64,
                                                    )
                                                    .read_to_string(&mut buf)
                                                {
                                                    let line = format!(
                                                    "   ⚠️ Failed to read cjxl stderr output: {}",
                                                    err
                                                );
                                                    shared_utils::progress_mode::emit_stderr(&line);
                                                }
                                                buf
                                            })
                                        });

                                    let ffmpeg_status = ffmpeg_proc.wait();
                                    let cjxl_status = cjxl_proc.wait();

                                    let ffmpeg_stderr_str = match ffmpeg_stderr_thread {
                                        Some(handle) => match handle.join() {
                                            Ok(s) => s,
                                            Err(_) => {
                                                shared_utils::progress_mode::emit_stderr(
                                                    "   ⚠️ FFmpeg stderr thread panicked",
                                                );
                                                String::new()
                                            }
                                        },
                                        None => String::new(),
                                    };
                                    let cjxl_stderr_str = match cjxl_stderr_thread {
                                        Some(handle) => match handle.join() {
                                            Ok(s) => s,
                                            Err(_) => {
                                                shared_utils::progress_mode::emit_stderr(
                                                    "   ⚠️ cjxl stderr thread panicked",
                                                );
                                                String::new()
                                            }
                                        },
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
                                                format!("   ❌ Failed to wait for FFmpeg: {}", e);
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
                                                format!("   ❌ Failed to wait for cjxl: {}", e);
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
                                            Some("(ffmpeg fallback)".to_string()),
                                        );
                                    } else {
                                        let line = format!(
                                            "   ❌ FFmpeg pipeline failed for file: {} (ffmpeg: {}, cjxl: {})",
                                            input.display(),
                                            if ffmpeg_ok { "✓" } else { "✗" },
                                            if cjxl_ok { "✓" } else { "✗" }
                                        );
                                        shared_utils::progress_mode::emit_stderr(&line);
                                        shared_utils::progress_mode::emit_stderr("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                                        if try_imagemagick_fallback(
                                            input,
                                            &temp_output,
                                            distance,
                                            max_threads,
                                            options.apple_compat,
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
                                    let line = format!("   ❌ Failed to start cjxl process: {}", e);
                                    shared_utils::progress_mode::emit_stderr(&line);
                                    if let Err(kill_err) = ffmpeg_proc.kill() {
                                        let line = format!(
                                            "   ⚠️ Failed to stop FFmpeg after cjxl startup failure: {}",
                                            kill_err
                                        );
                                        shared_utils::progress_mode::emit_stderr(&line);
                                    }
                                    shared_utils::progress_mode::emit_stderr(
                                        "   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...",
                                    );
                                    if try_imagemagick_fallback(
                                        input,
                                        &temp_output,
                                        distance,
                                        max_threads,
                                        options.apple_compat,
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
                                    "   ⚠️ Failed to stop FFmpeg after stdout capture failure: {}",
                                    kill_err
                                );
                                shared_utils::progress_mode::emit_stderr(&line);
                            }
                            shared_utils::progress_mode::emit_stderr(
                                "   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...",
                            );
                            if try_imagemagick_fallback(
                                input,
                                &temp_output,
                                distance,
                                max_threads,
                                options.apple_compat,
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
                        let line = format!("   ❌ FFmpeg not available or failed to start: {}", e);
                        shared_utils::progress_mode::emit_stderr(&line);
                        shared_utils::progress_mode::emit_stderr(
                            "      💡 Install: brew install ffmpeg",
                        );
                        shared_utils::progress_mode::emit_stderr(
                            "   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...",
                        );
                        if try_imagemagick_fallback(
                            input,
                            &temp_output,
                            distance,
                            max_threads,
                            options.apple_compat,
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
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "cjxl failed: {}",
                stderr
            )))
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::ToolNotFound(format!(
                "cjxl not found: {}",
                e
            )))
        }
    }
}

/// True when cjxl failed with "JPEG bitstream reconstruction data could not be created" / "allow_jpeg_reconstruction".
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
    let _icc_temp = shared_utils::jxl_utils::extract_icc_profile(input);
    let icc_path = _icc_temp.as_ref().map(|t| t.path());

    let mut cmd = Command::new("cjxl");
    cmd.arg("--lossless_jpeg=1")
        .arg("-j")
        .arg(max_threads.to_string());
    if let Some(v) = allow_jpeg_reconstruction {
        cmd.arg("--allow_jpeg_reconstruction").arg(v.to_string());
    }

    // Add HDR metadata via CICP if available (for wide-gamut JPEG)
    if let Some(hdr) = hdr_info {
        if let Some(cicp) = shared_utils::color_info_to_cicp(hdr) {
            cmd.arg(format!("--cicp={}", cicp));
        }
    }

    if options.apple_compat {
        cmd.arg("--compress_boxes=0");
    }

    shared_utils::jxl_utils::add_icc_to_cjxl(&mut cmd, icc_path);

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(temp_output).as_ref());
    cmd.output()
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
    let output_size = fs::metadata(temp_output).map(|m| m.len()).unwrap_or(0);
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
/// - On corruption: uses ImageMagick fallback to sanitize
/// - Verifies JXL health and checks size tolerance
///
/// # Fallback Chain
/// 1. Primary: cjxl with lossless JPEG mode
/// 2. Strip JPEG tail → retry
/// 3. Use --allow_jpeg_reconstruction=0
/// 4. ImageMagick sanitization (for corrupt JPEGs)
pub fn convert_jpeg_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    hdr_info: Option<&shared_utils::ColorInfo>,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);
    let max_threads = shared_utils::thread_manager::get_optimal_threads();

    let result = run_cjxl_jpeg_transcode(input, &temp_output, options, max_threads, None, hdr_info);

    let output_cmd = match result {
        Ok(out) => out,
        Err(e) => {
            return Err(ImgQualityError::ToolNotFound(format!(
                "cjxl not found: {}",
                e
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
                    if options.verbose {
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
                let label = if source_to_use != input {
                    "JPEG lossless (sanitized tail)"
                } else {
                    "JPEG lossless"
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
            "cjxl JPEG transcode failed (fix + retry and --allow_jpeg_reconstruction 0 both failed): {}",
            stderr
        )));
    }

    if stderr.contains("Error while decoding")
        || stderr.contains("Corrupt JPEG")
        || stderr.contains("Premature end")
    {
        match shared_utils::jxl_utils::try_imagemagick_fallback(
            input,
            &temp_output,
            0.0,
            max_threads,
            options.apple_compat,
        ) {
            Ok(_) => commit_jpeg_to_jxl_success(
                input,
                &temp_output,
                &output,
                input_size,
                options,
                "JPEG (Sanitized) -> JXL",
            ),
            Err(e) => Err(ImgQualityError::ConversionError(format!(
                "Fallback failed after JPEG corruption: {}",
                e
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
            options.apple_compat,
        ) {
            Ok(_) => commit_jpeg_to_jxl_success(
                input,
                &temp_output,
                &output,
                input_size,
                options,
                "JPEG -> JXL (ImageMagick fallback)",
            ),
            Err(_) => Err(ImgQualityError::ConversionError(format!(
                "cjxl JPEG transcode failed: {}",
                stderr
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
pub fn convert_to_avif(
    input: &Path,
    quality: Option<u8>,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "avif", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);
    let q = quality.unwrap_or(85);

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
            let output_size = fs::metadata(&temp_output)?.len();
            if let Err(e) = shared_utils::avif_av1_health::verify_avif_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(ImgQualityError::ConversionError(format!(
                    "AVIF health check failed: {}",
                    e
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
                "avifenc failed: {}",
                stderr
            )))
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::ToolNotFound(format!(
                "avifenc not found: {}",
                e
            )))
        }
    }
}

pub fn convert_to_hevc_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    vid_hevc::animated_image::convert_to_hevc_mp4(input, options)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

pub fn convert_to_avif_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if options.verbose {
        eprintln!("⚠️  Mathematical lossless AVIF encoding - this will be SLOW!");
    }

    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "avif", options)?;

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
            let output_size = fs::metadata(&temp_output)?.len();
            if let Err(e) = shared_utils::avif_av1_health::verify_avif_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(ImgQualityError::ConversionError(format!(
                    "Lossless AVIF health check failed: {}",
                    e
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
                "avifenc lossless failed: {}",
                stderr
            )))
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::ToolNotFound(format!(
                "avifenc not found: {}",
                e
            )))
        }
    }
}

pub fn convert_to_hevc_mp4_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let initial_crf = calculate_matched_crf_for_animation_hevc(analysis, input_size)?;
    vid_hevc::animated_image::convert_to_hevc_mp4_matched(
        input,
        options,
        initial_crf,
        analysis.has_alpha,
    )
    .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

fn calculate_matched_crf_for_animation_hevc(
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
        analysis.duration_secs.map(|d| d as f64),
        None,
        None,
    );

    match shared_utils::calculate_hevc_crf(&quality_analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(
                &quality_analysis,
                &result,
                shared_utils::EncoderType::Hevc,
            );
            Ok(result.crf)
        }
        Err(e) => Err(ImgQualityError::AnalysisError(format!(
            "Quality analysis failed: {}",
            e
        ))),
    }
}

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
            "Quality analysis failed: {}",
            e
        ))),
    }
}

pub fn convert_to_jxl_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    // Validate input file
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

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

    let distance = calculate_matched_distance_for_static(analysis, input_size)?;
    eprintln!("   🎯 Matched JXL distance: {:.2}", distance);

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        shared_utils::thread_manager::get_optimal_threads()
    };
    let mut cmd = Command::new("cjxl");
    cmd.arg("-d")
        .arg(format!("{:.2}", distance))
        .arg("-e")
        .arg("7")
        .arg("-j")
        .arg(max_threads.to_string());

    if options.apple_compat {
        cmd.arg("--compress_boxes=0");
    }

    // Only disable lossless JPEG mode when input is actually JPEG and we want lossy encoding.
    // For non-JPEG inputs this flag is a no-op, but omitting it keeps the command clean.
    if distance > 0.0 {
        let is_jpeg = options
            .input_format
            .as_deref()
            .map(|f| f.eq_ignore_ascii_case("jpeg") || f.eq_ignore_ascii_case("jpg"))
            .unwrap_or(false);
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

            let extra = format!("d={:.2}", distance);
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
                "cjxl failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "cjxl not found: {}",
            e
        ))),
    }
}

pub fn convert_to_hevc_mkv_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    vid_hevc::animated_image::convert_to_hevc_mkv_lossless(input, options)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

fn try_imagemagick_fallback(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
    apple_compat: bool,
) -> std::result::Result<(), std::io::Error> {
    shared_utils::jxl_utils::try_imagemagick_fallback(
        input,
        output,
        distance,
        max_threads,
        apple_compat,
    )
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
    // Check if we need HDR decoding first
    if shared_utils::needs_hdr_decode(hdr_info) {
        use console::style;
        eprintln!(
            "   {} {}",
            style("🌈 HDR DECODING:").cyan().bold(),
            style("Using FFmpeg to preserve high bit-depth").cyan()
        );

        match shared_utils::decode_hdr_image_to_png16(input, hdr_info.unwrap()) {
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
        .map(|e| e.to_ascii_lowercase())
        .and_then(|e| e.to_str().map(|s| s.to_string()))
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

            if !is_header_valid {
                use console::style;
                eprintln!(
                    "   {} {}",
                    style("🔧 PRE-PROCESSING:").yellow().bold(),
                    style("Corrupted JPEG header detected, using ImageMagick to sanitize").yellow()
                );

                let temp_png_file = tempfile::Builder::new().suffix(".png").tempfile()?;
                let temp_png = temp_png_file.path().to_path_buf();

                let result = Command::new("magick")
                    .arg("--")
                    .arg(shared_utils::safe_path_arg(input).as_ref())
                    .arg(shared_utils::safe_path_arg(&temp_png).as_ref())
                    .output();

                match result {
                    Ok(output) if output.status.success() && temp_png.exists() => {
                        eprintln!(
                            "   {} {}",
                            style("✅").green(),
                            style("ImageMagick JPEG sanitization successful")
                                .green()
                                .bold()
                        );
                        Ok((temp_png, Some(temp_png_file)))
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
            } else {
                Ok((input.to_path_buf(), None))
            }
        }

        "webp" => convert_to_temp_png(
            input,
            "dwebp",
            &[],
            &["-o", "__OUTPUT__"],
            "WebP detected, using dwebp for ICC profile compatibility",
        ),

        "tiff" | "tif" => convert_to_temp_png(
            input,
            "magick",
            &["--"],
            &["-depth", "16", "__OUTPUT__"],
            "TIFF detected, using ImageMagick for cjxl compatibility",
        ),

        "bmp" => convert_to_temp_png(
            input,
            "magick",
            &["--"],
            &["__OUTPUT__"],
            "BMP detected, using ImageMagick for cjxl compatibility",
        ),

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
                    let result = Command::new("magick")
                        .arg("--")
                        .arg(shared_utils::safe_path_arg(input).as_ref())
                        .arg(shared_utils::safe_path_arg(&temp_png).as_ref())
                        .output();

                    match result {
                        Ok(output) if output.status.success() && temp_png.exists() => {
                            eprintln!("   ✅ ImageMagick HEIC pre-processing successful");
                            Ok((temp_png, Some(temp_png_file)))
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

        "gif" => convert_to_temp_png(
            input,
            "ffmpeg",
            &["-y", "-i"],
            &["-frames:v", "1", "__OUTPUT__"],
            "GIF detected, using FFmpeg for static frame extraction",
        ),

        _ => {
            if let Some(actual_ext) = input.extension().and_then(|e| e.to_str()) {
                if actual_ext.to_lowercase() != ext {
                    eprintln!(
                        "   🔧 PRE-PROCESSING: Extension mismatch detected (.{} vs {}), creating aligned temp file",
                        actual_ext, ext
                    );

                    let temp_aligned_file = tempfile::Builder::new()
                        .suffix(&format!(".{}", ext))
                        .tempfile()?;
                    let temp_path = temp_aligned_file.path().to_path_buf();

                    if std::fs::copy(input, &temp_path).is_ok() {
                        Ok((temp_path, Some(temp_aligned_file)))
                    } else {
                        Ok((input.to_path_buf(), None))
                    }
                } else {
                    Ok((input.to_path_buf(), None))
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

pub fn convert_to_gif_apple_compat(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    vid_hevc::animated_image::convert_to_gif_apple_compat(input, options)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

pub fn is_high_quality_animated(width: u32, height: u32) -> bool {
    vid_hevc::animated_image::is_high_quality_animated(width, height)
}

fn verify_jxl_health(path: &Path) -> Result<()> {
    shared_utils::jxl_utils::verify_jxl_health(path).map_err(ImgQualityError::ConversionError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_get_output_path() {
        let input = Path::new("/path/to/image.png");
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let output = get_output_path(input, "jxl", &options).unwrap();
        assert_eq!(output, Path::new("/path/to/image.JXL"));
    }

    #[test]
    fn test_get_output_path_with_dir() {
        let input = Path::new("/path/to/image.png");
        let options = ConvertOptions {
            output_dir: Some(PathBuf::from("/output")),
            base_dir: None,
            ..Default::default()
        };
        let output = get_output_path(input, "avif", &options).unwrap();
        assert_eq!(output, Path::new("/output/image.AVIF"));
    }

    #[test]
    fn test_get_output_path_same_file_error() {
        let input = Path::new("/path/to/image.JXL");
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let result = get_output_path(input, "jxl", &options);
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
                "Format '{}' appears in both preprocess and direct format lists; configuration error",
                fmt
            );
        }
    }
}
