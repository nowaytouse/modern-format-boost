//! Lossless Converter Module
//!
//! Provides conversion API for verified lossless/lossy images.
//! Uses shared_utils for common functionality (anti-duplicate, ConversionResult, etc.)
//!
//! **Compress 判断统一**: 所有图片转换在编码成功、取得 output_size 后，在 finalize 前均调用
//! `check_size_tolerance`；当 `options.compress` 为 true 时，仅当 output < input 才接受，
//! 否则跳过并保留原文件。覆盖路径：convert_to_jxl、convert_jpeg_to_jxl（含 fallback）、
//! convert_to_avif、convert_to_avif_lossless、convert_to_jxl_matched。

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

pub fn convert_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    distance: f32,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();

    if let Some(ext) = input.extension() {
        if ext.to_string_lossy().to_lowercase() == "png" && input_size < 500 * 1024 {
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
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let (actual_input, _temp_file_guard) = prepare_input_for_cjxl(input, options)?;

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

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(&actual_input).as_ref())
        .arg(shared_utils::safe_path_arg(&temp_output).as_ref());

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
                use console::style;
                eprintln!(
                    "   {} {}",
                    style("⚠️  CJXL ENCODING FAILED:").yellow().bold(),
                    stderr.lines().next().unwrap_or("Unknown error")
                );
                eprintln!(
                    "   {} {}",
                    style("🔄 FALLBACK:").cyan(),
                    style("Using FFmpeg → CJXL pipeline (more reliable for large images)").dim()
                );
                eprintln!(
                    "   📋 Reason: Image format/size incompatible with installed CJXL version (metadata will be preserved)"
                );

                use std::process::Stdio;

                eprintln!("   🔄 Pipeline: FFmpeg → cjxl (streaming, no temp files)");

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
                                .arg(format!("{:.1}", distance))
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
                                                let mut buf = String::new();
                                                let mut reader = stderr;
                                                let _ = reader.read_to_string(&mut buf);
                                                buf
                                            })
                                        });

                                    // Drain cjxl stderr in background so cjxl does not block when pipe buffer fills.
                                    let cjxl_stderr_thread = cjxl_proc.stderr.take().map(|mut stderr| {
                                        std::thread::spawn(move || {
                                            let mut buf = String::new();
                                            let _ = std::io::Read::read_to_string(&mut stderr, &mut buf);
                                            buf
                                        })
                                    });

                                    let ffmpeg_status = ffmpeg_proc.wait();
                                    let cjxl_status = cjxl_proc.wait();

                                    let ffmpeg_stderr_str = ffmpeg_stderr_thread
                                        .and_then(|h| h.join().ok())
                                        .unwrap_or_default();
                                    let cjxl_stderr_str = cjxl_stderr_thread
                                        .and_then(|h| h.join().ok())
                                        .unwrap_or_default();

                                    let ffmpeg_ok = match ffmpeg_status {
                                        Ok(status) if status.success() => true,
                                        Ok(status) => {
                                            eprintln!(
                                                "   ❌ FFmpeg failed with exit code: {:?}",
                                                status.code()
                                            );
                                            if !ffmpeg_stderr_str.is_empty() {
                                                eprintln!(
                                                    "      Error: {}",
                                                    ffmpeg_stderr_str
                                                        .lines()
                                                        .next()
                                                        .unwrap_or("Unknown")
                                                );
                                            }
                                            false
                                        }
                                        Err(e) => {
                                            eprintln!("   ❌ Failed to wait for FFmpeg: {}", e);
                                            false
                                        }
                                    };

                                    let cjxl_ok = match cjxl_status {
                                        Ok(status) if status.success() => true,
                                        Ok(status) => {
                                            eprintln!(
                                                "   ❌ cjxl failed with exit code: {:?}",
                                                status.code()
                                            );
                                            if !cjxl_stderr_str.is_empty() {
                                                eprintln!(
                                                    "      Error: {}",
                                                    cjxl_stderr_str
                                                        .lines()
                                                        .next()
                                                        .unwrap_or("Unknown")
                                                );
                                            }
                                            false
                                        }
                                        Err(e) => {
                                            eprintln!("   ❌ Failed to wait for cjxl: {}", e);
                                            false
                                        }
                                    };

                                    if ffmpeg_ok && cjxl_ok {
                                        eprintln!("   🎉 FALLBACK SUCCESS: FFmpeg pipeline completed successfully");
                                        Ok(std::process::Output {
                                            status: std::process::ExitStatus::default(),
                                            stdout: Vec::new(),
                                            stderr: Vec::new(),
                                        })
                                    } else {
                                        eprintln!(
                                            "   ❌ FFmpeg pipeline failed for file: {} (ffmpeg: {}, cjxl: {})",
                                            input.display(),
                                            if ffmpeg_ok { "✓" } else { "✗" },
                                            if cjxl_ok { "✓" } else { "✗" }
                                        );

                                        eprintln!("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                                        try_imagemagick_fallback(
                                            input,
                                            &temp_output,
                                            distance,
                                            max_threads,
                                        )
                                    }
                                }
                                Err(e) => {
                                    eprintln!("   ❌ Failed to start cjxl process: {}", e);
                                    let _ = ffmpeg_proc.kill();
                                    eprintln!(
                                        "   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline..."
                                    );
                                    try_imagemagick_fallback(input, &temp_output, distance, max_threads)
                                }
                            }
                        } else {
                            eprintln!("   ❌ Failed to capture FFmpeg stdout");
                            let _ = ffmpeg_proc.kill();
                            eprintln!("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                            try_imagemagick_fallback(input, &temp_output, distance, max_threads)
                        }
                    }
                    Err(e) => {
                        eprintln!("   ❌ FFmpeg not available or failed to start: {}", e);
                        eprintln!("      💡 Install: brew install ffmpeg");
                        eprintln!("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                        try_imagemagick_fallback(input, &temp_output, distance, max_threads)
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
                let _ = fs::remove_file(&temp_output);
                return Err(e);
            }

            if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
                return Ok(ConversionResult::skipped_exists(input, &output));
            }

            if let Some(skipped) =
                check_size_tolerance(input, &output, input_size, output_size, options, "JXL")
            {
                return Ok(skipped);
            }

            finalize_conversion(input, &output, input_size, "JXL", None, options)
                .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
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
) -> std::io::Result<std::process::Output> {
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
        let _ = fs::remove_file(temp_output);
        return Err(e);
    }
    if !shared_utils::conversion::commit_temp_to_output(temp_output, output, options.force)? {
        return Ok(ConversionResult::skipped_exists(input, output));
    }
    let output_size = fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    if let Some(skipped) =
        check_size_tolerance(input, output, input_size, output_size, options, label)
    {
        return Ok(skipped);
    }
    finalize_conversion(input, output, input_size, label, None, options)
        .map_err(ImgQualityError::IoError)
}

pub fn convert_jpeg_to_jxl(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
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

    let result = run_cjxl_jpeg_transcode(input, &temp_output, options, max_threads, None);

    let output_cmd = match result {
        Ok(out) => out,
        Err(e) => {
            return Err(ImgQualityError::ToolNotFound(format!("cjxl not found: {}", e)));
        }
    };

    if output_cmd.status.success() {
        return commit_jpeg_to_jxl_success(
            input,
            &temp_output,
            &output,
            input_size,
            options,
            "JPEG lossless transcode",
        );
    }

    let stderr = String::from_utf8_lossy(&output_cmd.stderr);
    let _ = fs::remove_file(&temp_output);

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
        let retry_original = run_cjxl_jpeg_transcode(&source_to_use, &temp_output, options, max_threads, None);
        if let Ok(out) = retry_original {
            if out.status.success() {
                let label = if source_to_use != input {
                    "JPEG lossless transcode (sanitized tail)"
                } else {
                    "JPEG lossless transcode"
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
        let _ = fs::remove_file(&temp_output);

        // 3) Fallback: --allow_jpeg_reconstruction 0 (no bitstream reconstruction, often larger)
        let retry_no_recon = run_cjxl_jpeg_transcode(&source_to_use, &temp_output, options, max_threads, Some(0));
        if let Ok(out) = retry_no_recon {
            if out.status.success() {
                return commit_jpeg_to_jxl_success(
                    input,
                    &temp_output,
                    &output,
                    input_size,
                    options,
                    "JPEG lossless transcode (--allow_jpeg_reconstruction 0)",
                );
            }
        }
        let _ = fs::remove_file(&temp_output);
        return Err(ImgQualityError::ConversionError(format!(
            "cjxl JPEG transcode failed (fix + retry and --allow_jpeg_reconstruction 0 both failed): {}",
            stderr
        )));
    }

    if stderr.contains("Error while decoding")
        || stderr.contains("Corrupt JPEG")
        || stderr.contains("Premature end")
    {
        use console::style;
        eprintln!(
            "   {} {}",
            style("⚠️  JPEG TRANSCODE FAILED:").yellow().bold(),
            style("Detected corrupted/truncated JPEG structure").yellow()
        );
        eprintln!(
            "   {} {}",
            style("🔄 FALLBACK:").cyan(),
            style("Using ImageMagick → cjxl pipeline to sanitize and re-encode").dim()
        );

        match try_imagemagick_fallback(input, &temp_output, 0.0, max_threads) {
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
        Err(ImgQualityError::ConversionError(format!(
            "cjxl JPEG transcode failed: {}",
            stderr
        )))
    }
}

pub fn convert_to_avif(
    input: &Path,
    quality: Option<u8>,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
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
                let _ = fs::remove_file(&temp_output);
                return Err(ImgQualityError::ConversionError(format!(
                    "AVIF health check failed: {}", e
                )));
            }
            if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
                return Ok(ConversionResult::skipped_exists(input, &output));
            }
            if let Some(skipped) =
                check_size_tolerance(input, &output, input_size, output_size, options, "AVIF")
            {
                return Ok(skipped);
            }
            finalize_conversion(input, &output, input_size, "AVIF", None, options)
                .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
            let _ = fs::remove_file(&temp_output);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "avifenc failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "avifenc not found: {}",
            e
        ))),
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
                let _ = fs::remove_file(&temp_output);
                return Err(ImgQualityError::ConversionError(format!(
                    "Lossless AVIF health check failed: {}", e
                )));
            }
            if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
                return Ok(ConversionResult::skipped_exists(input, &output));
            }
            if let Some(skipped) =
                check_size_tolerance(input, &output, input_size, output_size, options, "Lossless AVIF")
            {
                return Ok(skipped);
            }
            finalize_conversion(input, &output, input_size, "Lossless AVIF", None, options)
                .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
            let _ = fs::remove_file(&temp_output);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "avifenc lossless failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "avifenc not found: {}",
            e
        ))),
    }
}

pub fn convert_to_hevc_mp4_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let initial_crf = calculate_matched_crf_for_animation_hevc(analysis, input_size);
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
) -> f32 {
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
            result.crf
        }
        Err(e) => {
            eprintln!("   ⚠️  Quality analysis failed: {}", e);
            eprintln!("   ⚠️  Using conservative CRF 18.0 (high quality)");
            18.0
        }
    }
}

pub fn calculate_matched_distance_for_static(
    analysis: &crate::ImageAnalysis,
    file_size: u64,
) -> f32 {
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
            result.distance
        }
        Err(e) => {
            eprintln!("   ⚠️  Quality analysis failed: {}", e);
            eprintln!("   ⚠️  Using conservative distance 1.0 (Q90 equivalent)");
            1.0
        }
    }
}

pub fn convert_to_jxl_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let distance = calculate_matched_distance_for_static(analysis, input_size);
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

    if distance > 0.0 {
        cmd.arg("--lossless_jpeg=0");
    }

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&temp_output).as_ref());

    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if let Err(e) = verify_jxl_health(&temp_output) {
                let _ = fs::remove_file(&temp_output);
                return Err(e);
            }

            if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
                return Ok(ConversionResult::skipped_exists(input, &output));
            }

            if let Some(skipped) =
                check_size_tolerance(input, &output, input_size, output_size, options, "Quality-matched JXL")
            {
                return Ok(skipped);
            }

            let extra = format!("d={:.2}", distance);
            finalize_conversion(
                input,
                &output,
                input_size,
                "Quality-matched JXL",
                Some(&extra),
                options,
            )
            .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
            let _ = fs::remove_file(&temp_output);
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
) -> std::result::Result<std::process::Output, std::io::Error> {
    shared_utils::jxl_utils::try_imagemagick_fallback(input, output, distance, max_threads)
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
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    let detected_ext = shared_utils::common_utils::detect_real_extension(input);
    let literal_ext = input
        .extension()
        .map(|e| e.to_ascii_lowercase())
        .and_then(|e| e.to_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let ext = if let Some(real) = detected_ext {
        if !literal_ext.is_empty() && real != literal_ext {
            if !((real == "jpg" && literal_ext == "jpeg")
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

pub fn convert_to_gif_apple_compat(
    input: &Path,
    options: &ConvertOptions,
    fps: Option<f32>,
) -> Result<ConversionResult> {
    vid_hevc::animated_image::convert_to_gif_apple_compat(input, options, fps)
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
        assert_eq!(output, Path::new("/path/to/image.jxl"));
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
        assert_eq!(output, Path::new("/output/image.avif"));
    }

    #[test]
    fn test_get_output_path_same_file_error() {
        let input = Path::new("/path/to/image.jxl");
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
