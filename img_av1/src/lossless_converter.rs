//! Lossless Converter Module
//!
//! Provides conversion API for verified lossless/lossy images
//! Uses shared_utils for common functionality (anti-duplicate, ConversionResult, etc.)

use crate::{ImgQualityError, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

pub use shared_utils::conversion::{
    clear_processed_list, format_size_change, is_already_processed, load_processed_list,
    mark_as_processed, save_processed_list, ConversionResult, ConvertOptions,
};

pub fn convert_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    distance: f32,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(&output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
    }

    let (actual_input, _temp_file_guard) = prepare_input_for_cjxl(input, options)?;

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        shared_utils::thread_manager::get_optimal_threads()
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
            if stderr.contains("Getting pixel data failed") || stderr.contains("Failed to decode") {
                eprintln!(
                    "   ⚠️  CJXL DECODE FAILED: {}",
                    stderr.lines().next().unwrap_or("Unknown error")
                );
                eprintln!("   🔧 FALLBACK: Using ImageMagick pipeline to re-encode PNG");
                eprintln!(
                    "   📋 Reason: PNG contains incompatible metadata/encoding (will be preserved)"
                );

                use std::process::Stdio;

                eprintln!("   🔄 Pipeline: magick → cjxl (streaming, no temp files)");

                let magick_result = Command::new("magick")
                    .arg("--")
                    .arg(shared_utils::safe_path_arg(input).as_ref())
                    .arg("-depth")
                    .arg("16")
                    .arg("png:-")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                match magick_result {
                    Ok(mut magick_proc) => {
                        if let Some(magick_stdout) = magick_proc.stdout.take() {
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
                                cmd.stdin(magick_stdout).stderr(Stdio::piped()).spawn();

                            match cjxl_result {
                                Ok(mut cjxl_proc) => {
                                    let magick_status = magick_proc.wait();
                                    let cjxl_status = cjxl_proc.wait();

                                    let magick_ok = match magick_status {
                                        Ok(status) if status.success() => true,
                                        Ok(status) => {
                                            eprintln!(
                                                "   ❌ ImageMagick failed with exit code: {:?}",
                                                status.code()
                                            );
                                            if let Some(mut stderr) = magick_proc.stderr {
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
                                            eprintln!(
                                                "   ❌ Failed to wait for ImageMagick: {}",
                                                e
                                            );
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

                                    if magick_ok && cjxl_ok {
                                        eprintln!("   🎉 FALLBACK SUCCESS: Pipeline completed successfully");
                                        Ok(std::process::Output {
                                            status: std::process::ExitStatus::default(),
                                            stdout: Vec::new(),
                                            stderr: Vec::new(),
                                        })
                                    } else {
                                        eprintln!("   ❌ FALLBACK FAILED: Pipeline error (magick: {}, cjxl: {})",
                                            if magick_ok { "✓" } else { "✗" },
                                            if cjxl_ok { "✓" } else { "✗" });
                                        result
                                    }
                                }
                                Err(e) => {
                                    eprintln!("   ❌ Failed to start cjxl process: {}", e);
                                    let _ = magick_proc.kill();
                                    result
                                }
                            }
                        } else {
                            eprintln!("   ❌ Failed to capture ImageMagick stdout");
                            let _ = magick_proc.kill();
                            result
                        }
                    }
                    Err(e) => {
                        eprintln!("   ❌ ImageMagick not available or failed to start: {}", e);
                        eprintln!("      💡 Install: brew install imagemagick");
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
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            let tolerance_ratio = if options.allow_size_tolerance { 1.01 } else { 1.0 };
            if output_size as f64 > input_size as f64 * tolerance_ratio {
                if let Err(e) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove oversized JXL output: {}", e);
                }
                eprintln!(
                    "   ⏭️  Rollback: JXL larger than original ({} → {} bytes, +{:.1}%)",
                    input_size,
                    output_size,
                    (output_size as f64 / input_size as f64 - 1.0) * 100.0
                );
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: format!(
                        "Skipped: JXL would be larger (+{:.1}%)",
                        (output_size as f64 / input_size as f64 - 1.0) * 100.0
                    ),
                    skipped: true,
                    skip_reason: Some("size_increase".to_string()),
                });
            }

            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
            }

            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!(
                    "JXL conversion successful: size reduced {:.1}%",
                    reduction_pct
                )
            } else {
                format!(
                    "JXL conversion successful: size increased {:.1}%",
                    -reduction_pct
                )
            };

            Ok(ConversionResult {
                success: true,
                input_path: input.display().to_string(),
                output_path: Some(output.display().to_string()),
                input_size,
                output_size: Some(output_size),
                size_reduction: Some(reduction_pct),
                message,
                skipped: false,
                skip_reason: None,
            })
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
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(&output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
    }

    let max_threads = shared_utils::thread_manager::get_ffmpeg_threads();
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
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
            }

            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!(
                    "JPEG lossless transcode successful: size reduced {:.1}%",
                    reduction_pct
                )
            } else {
                format!(
                    "JPEG lossless transcode successful: size increased {:.1}%",
                    -reduction_pct
                )
            };

            Ok(ConversionResult {
                success: true,
                input_path: input.display().to_string(),
                output_path: Some(output.display().to_string()),
                input_size,
                output_size: Some(output_size),
                size_reduction: Some(reduction_pct),
                message,
                skipped: false,
                skip_reason: None,
            })
        }
        Ok(output_cmd) => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "cjxl JPEG transcode failed: {}",
                stderr
            )))
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
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "avif", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(&output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
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
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!(
                    "AVIF conversion successful: size reduced {:.1}%",
                    reduction_pct
                )
            } else {
                format!(
                    "AVIF conversion successful: size increased {:.1}%",
                    -reduction_pct
                )
            };

            Ok(ConversionResult {
                success: true,
                input_path: input.display().to_string(),
                output_path: Some(output.display().to_string()),
                input_size,
                output_size: Some(output_size),
                size_reduction: Some(reduction_pct),
                message,
                skipped: false,
                skip_reason: None,
            })
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

pub fn convert_to_av1_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mp4", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(&output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
    }

    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, false);

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        shared_utils::thread_manager::get_optimal_threads()
    };
    let svt_params = format!("tune=0:film-grain=0:lp={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads")
        .arg(max_threads.to_string())
        .arg("-i")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg("-c:v")
        .arg("libsvtav1")
        .arg("-crf")
        .arg("0")
        .arg("-preset")
        .arg("6")
        .arg("-svtav1-params")
        .arg(&svt_params);

    for arg in &vf_args {
        cmd.arg(arg);
    }

    cmd.arg(shared_utils::safe_path_arg(&output).as_ref());
    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!(
                    "AV1 conversion successful: size reduced {:.1}%",
                    reduction_pct
                )
            } else {
                format!(
                    "AV1 conversion successful: size increased {:.1}%",
                    -reduction_pct
                )
            };

            Ok(ConversionResult {
                success: true,
                input_path: input.display().to_string(),
                output_path: Some(output.display().to_string()),
                input_size,
                output_size: Some(output_size),
                size_reduction: Some(reduction_pct),
                message,
                skipped: false,
                skip_reason: None,
            })
        }
        Ok(output_cmd) => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "ffmpeg failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "ffmpeg not found: {}",
            e
        ))),
    }
}

pub fn convert_to_avif_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    eprintln!("⚠️  Mathematical lossless AVIF encoding - this will be SLOW!");

    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "avif", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(&output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
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
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("Lossless AVIF: size reduced {:.1}%", reduction_pct)
            } else {
                format!("Lossless AVIF: size increased {:.1}%", -reduction_pct)
            };

            Ok(ConversionResult {
                success: true,
                input_path: input.display().to_string(),
                output_path: Some(output.display().to_string()),
                input_size,
                output_size: Some(output_size),
                size_reduction: Some(reduction_pct),
                message,
                skipped: false,
                skip_reason: None,
            })
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

pub fn convert_to_av1_mp4_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mp4", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(&output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
    }

    let initial_crf = calculate_matched_crf_for_animation(analysis, input_size) as f32;

    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, analysis.has_alpha);

    let flag_mode = options
        .flag_mode()
        .map_err(ImgQualityError::ConversionError)?;

    eprintln!(
        "   {} Mode: CRF {:.1} (based on input analysis)",
        flag_mode.description_cn(),
        initial_crf
    );

    let explore_result = shared_utils::explore_precise_quality_match_with_compression(
        input,
        &output,
        shared_utils::VideoEncoder::Av1,
        vf_args,
        initial_crf,
        50.0,
        0.91,
        options.child_threads,
    )
    .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    for log in &explore_result.log {
        eprintln!("{}", log);
    }

    let output_size = explore_result.output_size;
    let reduction = 1.0 - (output_size as f64 / input_size as f64);

    shared_utils::copy_metadata(input, &output);

    mark_as_processed(input);

    if options.should_delete_original()
        && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
    {
    }

    let reduction_pct = reduction * 100.0;
    let message = if reduction >= 0.0 {
        format!(
            "Quality-matched AV1 (CRF {:.1}): size reduced {:.1}%",
            explore_result.optimal_crf, reduction_pct
        )
    } else {
        format!(
            "Quality-matched AV1 (CRF {:.1}): size increased {:.1}%",
            explore_result.optimal_crf, -reduction_pct
        )
    };

    Ok(ConversionResult {
        success: true,
        input_path: input.display().to_string(),
        output_path: Some(output.display().to_string()),
        input_size,
        output_size: Some(output_size),
        size_reduction: Some(reduction_pct),
        message,
        skipped: false,
        skip_reason: None,
    })
}

fn calculate_matched_crf_for_animation(analysis: &crate::ImageAnalysis, file_size: u64) -> f32 {
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

    match shared_utils::calculate_av1_crf(&quality_analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(
                &quality_analysis,
                &result,
                shared_utils::EncoderType::Av1,
            );
            result.crf
        }
        Err(e) => {
            eprintln!("   ⚠️  Quality analysis failed: {}", e);
            eprintln!("   ⚠️  Using conservative CRF 23.0 (high quality)");
            23.0
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
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "jxl", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(&output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
    }

    let distance = calculate_matched_distance_for_static(analysis, input_size);
    eprintln!("   🎯 Matched JXL distance: {:.2}", distance);

    let max_threads = shared_utils::thread_manager::get_optimal_threads();
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
        .arg(shared_utils::safe_path_arg(&output).as_ref());

    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            let tolerance_ratio = if options.allow_size_tolerance { 1.01 } else { 1.0 };
            if output_size as f64 > input_size as f64 * tolerance_ratio {
                if let Err(e) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove oversized JXL output: {}", e);
                }
                eprintln!(
                    "   ⏭️  Rollback: JXL larger than original ({} → {} bytes, +{:.1}%)",
                    input_size,
                    output_size,
                    (output_size as f64 / input_size as f64 - 1.0) * 100.0
                );
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: format!(
                        "Skipped: JXL would be larger (+{:.1}%)",
                        (output_size as f64 / input_size as f64 - 1.0) * 100.0
                    ),
                    skipped: true,
                    skip_reason: Some("size_increase".to_string()),
                });
            }

            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
            }

            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!(
                    "Quality-matched JXL (d={:.2}): size reduced {:.1}%",
                    distance, reduction_pct
                )
            } else {
                format!(
                    "Quality-matched JXL (d={:.2}): size increased {:.1}%",
                    distance, -reduction_pct
                )
            };

            Ok(ConversionResult {
                success: true,
                input_path: input.display().to_string(),
                output_path: Some(output.display().to_string()),
                input_size,
                output_size: Some(output_size),
                size_reduction: Some(reduction_pct),
                message,
                skipped: false,
                skip_reason: None,
            })
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

pub fn convert_to_av1_mp4_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    eprintln!("⚠️  Mathematical lossless AV1 encoding - this will be VERY SLOW!");

    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mp4", options)?;

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(&output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
    }

    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, false);

    let max_threads = shared_utils::thread_manager::get_optimal_threads();
    let svt_params = format!("lossless=1:lp={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads")
        .arg(max_threads.to_string())
        .arg("-i")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg("-c:v")
        .arg("libsvtav1")
        .arg("-crf")
        .arg("0")
        .arg("-preset")
        .arg("4")
        .arg("-svtav1-params")
        .arg(&svt_params);

    for arg in &vf_args {
        cmd.arg(arg);
    }

    cmd.arg(shared_utils::safe_path_arg(&output).as_ref());
    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("Lossless AV1: size reduced {:.1}%", reduction_pct)
            } else {
                format!("Lossless AV1: size increased {:.1}%", -reduction_pct)
            };

            Ok(ConversionResult {
                success: true,
                input_path: input.display().to_string(),
                output_path: Some(output.display().to_string()),
                input_size,
                output_size: Some(output_size),
                size_reduction: Some(reduction_pct),
                message,
                skipped: false,
                skip_reason: None,
            })
        }
        Ok(output_cmd) => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "ffmpeg lossless failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ImgQualityError::ToolNotFound(format!(
            "ffmpeg not found: {}",
            e
        ))),
    }
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
                eprintln!(
                    "   ⚠️  EXTENSION MISMATCH: {} is actually {}, adjusting pre-processing...",
                    input.display(),
                    real
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
            use console::style;
            eprintln!(
                "   {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style("WebP detected, using dwebp for ICC profile compatibility").dim()
            );

            let temp_png_file = tempfile::Builder::new().suffix(".png").tempfile()?;
            let temp_png = temp_png_file.path().to_path_buf();

            let result = Command::new("dwebp")
                .arg(shared_utils::safe_path_arg(input).as_ref())
                .arg("-o")
                .arg(shared_utils::safe_path_arg(&temp_png).as_ref())
                .output();

            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!(
                        "   {} {}",
                        style("✅").green(),
                        style("dwebp pre-processing successful").green()
                    );
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    eprintln!(
                        "   {} {}",
                        style("⚠️").yellow(),
                        style("dwebp pre-processing failed, trying direct cjxl").dim()
                    );
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        "tiff" | "tif" => {
            eprintln!(
                "   🔧 PRE-PROCESSING: TIFF detected, using ImageMagick for cjxl compatibility"
            );

            let temp_png_file = tempfile::Builder::new().suffix(".png").tempfile()?;
            let temp_png = temp_png_file.path().to_path_buf();

            let result = Command::new("magick")
                .arg("--")
                .arg(shared_utils::safe_path_arg(input).as_ref())
                .arg("-depth")
                .arg("16")
                .arg(shared_utils::safe_path_arg(&temp_png).as_ref())
                .output();

            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!("   ✅ ImageMagick TIFF pre-processing successful");
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    eprintln!("   ⚠️  ImageMagick TIFF pre-processing failed, trying direct cjxl");
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        "bmp" => {
            eprintln!(
                "   🔧 PRE-PROCESSING: BMP detected, using ImageMagick for cjxl compatibility"
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
                    eprintln!("   ✅ ImageMagick BMP pre-processing successful");
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    eprintln!("   ⚠️  ImageMagick BMP pre-processing failed, trying direct cjxl");
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        "heic" | "heif" => {
            eprintln!("   🔧 PRE-PROCESSING: HEIC/HEIF detected, using sips/ImageMagick for cjxl compatibility");

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

fn get_input_dimensions(input: &Path) -> Result<(u32, u32)> {
    if let Ok(probe) = shared_utils::probe_video(input) {
        if probe.width > 0 && probe.height > 0 {
            return Ok((probe.width, probe.height));
        }
    }

    if let Ok((w, h)) = image::image_dimensions(input) {
        return Ok((w, h));
    }

    {
        use std::process::Command;
        let safe_path = shared_utils::safe_path_arg(input);
        let output = Command::new("magick")
            .args(["identify", "-format", "%w %h\n"])
            .arg(safe_path.as_ref())
            .output()
            .or_else(|_| {
                Command::new("identify")
                    .args(["-format", "%w %h\n"])
                    .arg(safe_path.as_ref())
                    .output()
            });
        if let Ok(out) = output {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = s.lines().next() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                            if w > 0 && h > 0 {
                                return Ok((w, h));
                            }
                        }
                    }
                }
            }
        }
    }

    Err(ImgQualityError::ConversionError(format!(
        "❌ 无法获取文件尺寸: {}\n\
         💡 ffprobe, image crate, ImageMagick identify 均失败\n\
         请检查文件是否完整，或安装 ffmpeg/ImageMagick",
        input.display(),
    )))
}

fn verify_jxl_health(path: &Path) -> Result<()> {
    let mut file = fs::File::open(path)?;
    let mut sig = [0u8; 2];
    use std::io::Read;
    file.read_exact(&mut sig)?;

    if sig != [0xFF, 0x0A] && sig != [0x00, 0x00] {
        return Err(ImgQualityError::ConversionError(
            "Invalid JXL file signature".to_string(),
        ));
    }

    if which::which("jxlinfo").is_ok() {
        let result = Command::new("jxlinfo")
            .arg(shared_utils::safe_path_arg(path).as_ref())
            .output();

        if let Ok(output) = result {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ImgQualityError::ConversionError(format!(
                    "JXL health check failed (jxlinfo): {}",
                    stderr.trim()
                )));
            }
        }
    }

    Ok(())
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
}
