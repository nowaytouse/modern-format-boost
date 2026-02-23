//! Lossless Converter Module
//!
//! Provides conversion API for verified lossless/lossy images
//! Uses shared_utils for common functionality (anti-duplicate, ConversionResult, etc.)

use crate::{ImgQualityError, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

pub use shared_utils::conversion::{
    check_size_tolerance, clear_processed_list, finalize_conversion, format_size_change,
    is_already_processed, load_processed_list, mark_as_processed, save_processed_list,
    ConversionResult, ConvertOptions,
};

pub fn convert_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    distance: f32,
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
        .arg(shared_utils::safe_path_arg(&output).as_ref());

    let cmd_result = cmd.output();

    let result = match &cmd_result {
        Ok(output_cmd) if !output_cmd.status.success() => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            if stderr.contains("Getting pixel data failed") || stderr.contains("Failed to decode") {
                eprintln!(
                    "   ⚠️  CJXL DECODE FAILED: {}",
                    stderr.lines().next().unwrap_or("Unknown error")
                );
                eprintln!("   🔧 FALLBACK: Using ImageMagick pipeline to re-encode PNG");

                match shared_utils::jxl_utils::try_imagemagick_fallback(input, &output, distance, max_threads) {
                    Ok(out) => Ok(out),
                    Err(_) => cmd_result,
                }
            } else {
                cmd_result
            }
        }
        _ => cmd_result,
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

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
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
            let _ = fs::remove_file(&output);
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
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "avif", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

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
            let _ = fs::remove_file(&output);
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
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mp4", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
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
            finalize_conversion(input, &output, input_size, "AV1", None, options)
                .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
            let _ = fs::remove_file(&output);
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
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "avif", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

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
            let _ = fs::remove_file(&output);
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
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mp4", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
    }

    let initial_crf = calculate_matched_crf_for_animation(analysis, input_size);

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

    let explore_result = match shared_utils::explore_precise_quality_match_with_compression(
        input,
        &output,
        shared_utils::VideoEncoder::Av1,
        vf_args,
        initial_crf,
        50.0,
        0.91,
        options.child_threads,
    ) {
        Ok(r) => r,
        Err(e) => {
            let _ = fs::remove_file(&output);
            return Err(ImgQualityError::ConversionError(e.to_string()));
        }
    };

    for log in &explore_result.log {
        eprintln!("{}", log);
    }

    let extra = format!("CRF {:.1}", explore_result.optimal_crf);
    finalize_conversion(input, &output, input_size, "Quality-matched AV1", Some(&extra), options)
        .map_err(ImgQualityError::IoError)
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

            if let Some(skipped) = check_size_tolerance(input, &output, input_size, output_size, options, "JXL") {
                return Ok(skipped);
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

pub fn convert_to_av1_mp4_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    eprintln!("⚠️  Mathematical lossless AV1 encoding - this will be VERY SLOW!");

    if !options.force && is_already_processed(input) {
        return Ok(ConversionResult::skipped_duplicate(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mp4", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult::skipped_exists(input, &output));
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
            finalize_conversion(input, &output, input_size, "Lossless AV1", None, options)
                .map_err(ImgQualityError::IoError)
        }
        Ok(output_cmd) => {
            let _ = fs::remove_file(&output);
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


fn verify_jxl_health(path: &Path) -> Result<()> {
    shared_utils::jxl_utils::verify_jxl_health(path)
        .map_err(ImgQualityError::ConversionError)
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
            // SOI marker only; detect_real_extension may have already done a fuller magic-byte check.
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
    shared_utils::conversion::get_input_dimensions(input)
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
}
