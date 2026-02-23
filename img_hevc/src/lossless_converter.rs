//! Lossless Converter Module
//!
//! Provides conversion API for verified lossless/lossy images
//! Uses shared_utils for common functionality (anti-duplicate, ConversionResult, etc.)

use crate::{ImgQualityError, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

pub use shared_utils::conversion::{
    check_size_tolerance,
    clear_processed_list,
    determine_output_path_with_base,
    finalize_conversion,
    format_size_change,
    is_already_processed,
    load_processed_list,
    mark_as_processed,
    save_processed_list,
    ConversionResult,
    ConvertOptions,
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
                input, input_size, "Skipped: Small PNG (< 500KB)", "small_file",
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

    let (actual_input, _temp_file_guard) = prepare_input_for_cjxl(input, options)?;

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) / 2).clamp(1, 4)
    };

    let mut cmd = Command::new("cjxl");
    cmd.arg("-d")
        .arg(format!("{:.1}", distance))
        .arg("-e")
        .arg("7")
        .arg("-j")
        .arg(max_threads.to_string());

    if options.apple_compat {
        cmd.arg("--compress_boxes=0");
    }

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(&actual_input).as_ref())
        .arg(shared_utils::safe_path_arg(&output).as_ref());

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
                                .arg(shared_utils::safe_path_arg(&output).as_ref())
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

                                    let ffmpeg_status = ffmpeg_proc.wait();
                                    let cjxl_status = cjxl_proc.wait();

                                    let ffmpeg_stderr_str = ffmpeg_stderr_thread
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
                                            if let Some(mut stderr) = cjxl_proc.stderr {
                                                use std::io::Read;
                                                let mut err = String::new();
                                                if stderr.read_to_string(&mut err).is_ok()
                                                    && !err.is_empty()
                                                {
                                                    eprintln!(
                                                        "      Error: {}",
                                                        err.lines().next().unwrap_or("Unknown")
                                                    );
                                                }
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
                                            "   ❌ FFmpeg pipeline failed (ffmpeg: {}, cjxl: {})",
                                            if ffmpeg_ok { "✓" } else { "✗" },
                                            if cjxl_ok { "✓" } else { "✗" }
                                        );

                                        eprintln!("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                                        try_imagemagick_fallback(
                                            input,
                                            &output,
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
                                    try_imagemagick_fallback(input, &output, distance, max_threads)
                                }
                            }
                        } else {
                            eprintln!("   ❌ Failed to capture FFmpeg stdout");
                            let _ = ffmpeg_proc.kill();
                            eprintln!("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                            try_imagemagick_fallback(input, &output, distance, max_threads)
                        }
                    }
                    Err(e) => {
                        eprintln!("   ❌ FFmpeg not available or failed to start: {}", e);
                        eprintln!("      💡 Install: brew install ffmpeg");
                        eprintln!("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                        try_imagemagick_fallback(input, &output, distance, max_threads)
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
            let output_size = fs::metadata(&output)?.len();

            if let Some(skipped) = check_size_tolerance(input, &output, input_size, output_size, options, "JXL") {
                return Ok(skipped);
            }

            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
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

pub fn convert_jpeg_to_jxl(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let max_threads = (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) / 2).clamp(1, 4);
    let mut cmd = Command::new("cjxl");
    cmd.arg("--lossless_jpeg=1")
        .arg("-j")
        .arg(max_threads.to_string());

    if options.apple_compat {
        cmd.arg("--compress_boxes=0");
    }

    cmd.arg("--")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&output).as_ref());

    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
            }

            finalize_conversion(input, &output, input_size, "JPEG lossless transcode", None, options)
                .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
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

                match try_imagemagick_fallback(input, &output, 0.0, max_threads) {
                    Ok(_) => {
                        finalize_conversion(input, &output, input_size, "JPEG (Sanitized) -> JXL", None, options)
                            .map_err(ImgQualityError::IoError)
                    }
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
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "cjxl not found: {}",
            e
        ))),
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
        .arg(shared_utils::safe_path_arg(&output).as_ref())
        .output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            finalize_conversion(input, &output, input_size, "AVIF", None, options)
                .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
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

    let result = Command::new("avifenc")
        .arg("--lossless")
        .arg("-s")
        .arg("4")
        .arg("-j")
        .arg("all")
        .arg("--")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&output).as_ref())
        .output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            finalize_conversion(input, &output, input_size, "Lossless AVIF", None, options)
                .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
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

    let distance = calculate_matched_distance_for_static(analysis, input_size);
    eprintln!("   🎯 Matched JXL distance: {:.2}", distance);

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) / 2).clamp(1, 4)
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
        .arg(input)
        .arg(&output);

    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();

            let tolerance_ratio = 1.01;
            let max_allowed_size = (input_size as f64 * tolerance_ratio) as u64;

            if output_size > max_allowed_size {
                let size_increase_pct = ((output_size as f64 / input_size as f64) - 1.0) * 100.0;
                if let Err(e) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove oversized JXL output: {}", e);
                }
                eprintln!(
                    "   ⏭️  Skipping: JXL output larger than input by {:.1}% (tolerance: 1.0%)",
                    size_increase_pct
                );
                eprintln!(
                    "   📊 Size comparison: {} → {} bytes (+{:.1}%)",
                    input_size, output_size, size_increase_pct
                );
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult::skipped_size_increase(input, input_size, output_size));
            }

            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
            }

            let extra = format!("d={:.2}", distance);
            finalize_conversion(input, &output, input_size, "Quality-matched JXL", Some(&extra), options)
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
    shared_utils::jxl_utils::convert_to_temp_png(input, tool, args_before_input, args_after_input, label)
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
                    style("⚠️  [智能修正] 扩展名不匹配:").yellow().bold(),
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

        "webp" => {
            convert_to_temp_png(
                input, "dwebp", &[],
                &["-o", "__OUTPUT__"],
                "WebP detected, using dwebp for ICC profile compatibility",
            )
        }

        "tiff" | "tif" => {
            convert_to_temp_png(
                input, "magick", &["--"],
                &["-depth", "16", "__OUTPUT__"],
                "TIFF detected, using ImageMagick for cjxl compatibility",
            )
        }

        "bmp" => {
            convert_to_temp_png(
                input, "magick", &["--"],
                &["__OUTPUT__"],
                "BMP detected, using ImageMagick for cjxl compatibility",
            )
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

        "gif" => {
            convert_to_temp_png(
                input, "ffmpeg", &["-y", "-i"],
                &["-frames:v", "1", "__OUTPUT__"],
                "GIF detected, using FFmpeg for static frame extraction",
            )
        }

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
    shared_utils::jxl_utils::verify_jxl_health(path)
        .map_err(ImgQualityError::ConversionError)
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
            "短动画(2s)+低质量(400x300)应该转GIF"
        );
    }

    #[test]
    fn test_apple_compat_routing_short_high_quality() {
        assert!(
            should_convert_to_video_format(2.0, 1920, 1080),
            "短动画(2s)+高质量(1920x1080)应该转视频"
        );
    }

    #[test]
    fn test_apple_compat_routing_long_low_quality() {
        assert!(
            should_convert_to_video_format(5.0, 400, 300),
            "长动画(5s)应该转视频，不管质量"
        );
    }

    #[test]
    fn test_apple_compat_routing_boundary_3_seconds() {
        assert!(
            should_convert_to_video_format(3.0, 400, 300),
            "正好3秒应该转视频"
        );
    }

    #[test]
    fn test_apple_compat_routing_boundary_under_3_seconds() {
        assert!(
            !should_convert_to_video_format(2.99, 400, 300),
            "2.99秒+低质量应该转GIF"
        );
    }


    #[test]
    fn test_format_classification_no_overlap() {
        let preprocess_formats = ["webp", "tiff", "tif", "bmp", "heic", "heif"];
        let direct_formats = ["png", "jpg", "jpeg", "gif", "jxl", "avif"];

        for fmt in &preprocess_formats {
            assert!(
                !direct_formats.contains(fmt),
                "格式 '{}' 同时出现在预处理和直接格式列表中，这是配置Error",
                fmt
            );
        }
    }
}
