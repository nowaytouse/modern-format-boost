//! Animated Image → Video Conversion Module
//!
//! Handles conversion of animated images (GIF, WebP, AVIF, etc.) to video formats.
//! Migrated from img_hevc to vid_hevc for clearer separation of concerns:
//! - img_hevc: image analysis, format detection, quality estimation
//! - vid_hevc: all video encoding (including animated image → video)

use crate::{Result, VidQualityError};
use shared_utils::conversion::{ConversionResult, ConvertOptions};
use std::fs;
use std::path::Path;
use std::process::Command;

use shared_utils::conversion::{
    determine_output_path_with_base, is_already_processed, mark_as_processed,
};

fn get_output_path(
    input: &Path,
    extension: &str,
    options: &ConvertOptions,
) -> Result<std::path::PathBuf> {
    if let Some(ref base) = options.base_dir {
        determine_output_path_with_base(input, base, extension, &options.output_dir)
            .map_err(VidQualityError::ConversionError)
    } else {
        shared_utils::conversion::determine_output_path(input, extension, &options.output_dir)
            .map_err(VidQualityError::ConversionError)
    }
}

fn copy_original_on_skip(input: &Path, options: &ConvertOptions) -> Option<std::path::PathBuf> {
    shared_utils::copy_on_skip_or_fail(
        input,
        options.output_dir.as_deref(),
        options.base_dir.as_deref(),
        options.verbose,
    )
    .unwrap_or_default()
}

pub fn get_input_dimensions(input: &Path) -> Result<(u32, u32)> {
    shared_utils::conversion::get_input_dimensions(input).map_err(VidQualityError::ConversionError)
}

fn get_max_threads(options: &ConvertOptions) -> usize {
    if options.child_threads > 0 {
        options.child_threads
    } else {
        (std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            / 2)
        .clamp(1, 4)
    }
}

pub fn is_high_quality_animated(width: u32, height: u32) -> bool {
    let total_pixels = width as u64 * height as u64;
    width >= 1280 || height >= 720 || total_pixels >= 921600
}

fn skipped_already_processed(input: &Path) -> ConversionResult {
    ConversionResult {
        success: true,
        input_path: input.display().to_string(),
        output_path: None,
        input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
        output_size: None,
        size_reduction: None,
        message: "Skipped: Already processed".to_string(),
        skipped: true,
        skip_reason: Some("duplicate".to_string()),
    }
}

fn skipped_output_exists(input: &Path, output: &Path, input_size: u64) -> ConversionResult {
    ConversionResult {
        success: true,
        input_path: input.display().to_string(),
        output_path: Some(output.display().to_string()),
        input_size,
        output_size: fs::metadata(output).map(|m| m.len()).ok(),
        size_reduction: None,
        message: "Skipped: Output file exists".to_string(),
        skipped: true,
        skip_reason: Some("exists".to_string()),
    }
}

/// Returns true if the file is an animated image format but effectively static (0 or negligible duration).
/// Callers should skip video conversion and treat as static image (e.g. route to JXL in img_hevc).
fn is_static_animated_image(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "gif" | "webp" | "avif" | "heic" | "heif") {
        return false;
    }
    if let Ok(analysis) = shared_utils::image_analyzer::analyze_image(path) {
        if analysis.is_animated {
            let duration_secs = analysis.duration_secs.unwrap_or(1.0);
            if duration_secs < 0.01 {
                return true;
            }
        }
    }
    false
}

fn skipped_static_animated(input: &Path, input_size: u64) -> ConversionResult {
    ConversionResult {
        success: true,
        input_path: input.display().to_string(),
        output_path: None,
        input_size,
        output_size: None,
        size_reduction: None,
        message: "Skipped: Static image (1 frame), use image conversion path instead"
            .to_string(),
        skipped: true,
        skip_reason: Some("static_animated".to_string()),
    }
}

pub fn convert_to_hevc_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(skipped_already_processed(input));
    }

    if is_static_animated_image(input) {
        let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        if options.verbose {
            eprintln!(
                "   ⏭️  Detected static animated image (1 frame), skipping video conversion: {}",
                input.display()
            );
        }
        copy_original_on_skip(input, options);
        mark_as_processed(input);
        return Ok(skipped_static_animated(input, input_size));
    }

    const MIN_DURATION: f32 = shared_utils::image_analyzer::ANIMATED_MIN_DURATION_FOR_VIDEO_SECS;
    if let Ok(analysis) = shared_utils::image_analyzer::analyze_image(input) {
        if let Some(d) = analysis.duration_secs {
            if d > 0.01 && d < MIN_DURATION {
                let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
                if options.verbose {
                    eprintln!(
                        "   ⏭️  Short animation ({:.1}s < {:.1}s), skipping video conversion: {}",
                        d, MIN_DURATION, input.display()
                    );
                }
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: format!("Skipped: Short animation ({:.1}s < {:.1}s)", d, MIN_DURATION),
                    skipped: true,
                    skip_reason: Some("short_animation".to_string()),
                });
            }
        }
    }

    let input_size = fs::metadata(input)?.len();
    let ext = if options.apple_compat { "mov" } else { "mp4" };
    let output = get_output_path(input, ext, options)?;

    if output.exists() && !options.force {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, false);

    let max_threads = get_max_threads(options);
    let x265_params = format!("log-level=error:pools={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads")
        .arg(max_threads.to_string())
        .arg("-i")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg("-c:v")
        .arg("libx265")
        .arg("-crf")
        .arg("0")
        .arg("-preset")
        .arg("medium")
        .arg("-tag:v")
        .arg("hvc1")
        .arg("-x265-params")
        .arg(&x265_params);

    for arg in &vf_args {
        cmd.arg(arg);
    }

    cmd.arg(shared_utils::safe_path_arg(&temp_output).as_ref());
    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();
            if output_size == 0 {
                let _ = fs::remove_file(&temp_output);
                return Err(VidQualityError::ConversionError(
                    "HEVC output file is empty (encoding may have failed)".to_string(),
                ));
            }
            if get_input_dimensions(&temp_output).is_err() {
                let _ = fs::remove_file(&temp_output);
                return Err(VidQualityError::ConversionError(
                    "HEVC output file is not readable (invalid or corrupted)".to_string(),
                ));
            }

            if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
                return Ok(skipped_output_exists(input, &output, input_size));
            }

            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            shared_utils::copy_metadata(input, &output);
            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(
                    input,
                    &output,
                    shared_utils::conversion::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                ).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!(
                    "HEVC conversion successful: size reduced {:.1}%",
                    reduction_pct
                )
            } else {
                format!(
                    "HEVC conversion successful: size increased {:.1}%",
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
            let _ = fs::remove_file(&temp_output);
            Err(VidQualityError::ConversionError(format!(
                "ffmpeg failed: {}",
                stderr
            )))
        }
        Err(e) => {
            let _ = fs::remove_file(&temp_output);
            Err(VidQualityError::ConversionError(format!(
                "ffmpeg not found: {}",
                e
            )))
        }
    }
}

pub fn convert_to_hevc_mp4_matched(
    input: &Path,
    options: &ConvertOptions,
    initial_crf: f32,
    has_alpha: bool,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(skipped_already_processed(input));
    }

    if is_static_animated_image(input) {
        let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        copy_original_on_skip(input, options);
        mark_as_processed(input);
        return Ok(skipped_static_animated(input, input_size));
    }
    const MIN_DURATION: f32 = shared_utils::image_analyzer::ANIMATED_MIN_DURATION_FOR_VIDEO_SECS;
    if let Ok(analysis) = shared_utils::image_analyzer::analyze_image(input) {
        if let Some(d) = analysis.duration_secs {
            if d > 0.01 && d < MIN_DURATION {
                let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
                if options.verbose {
                    eprintln!(
                        "   ⏭️  Short animation ({:.1}s < {:.1}s), skipping video conversion: {}",
                        d, MIN_DURATION, input.display()
                    );
                }
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: format!("Skipped: Short animation ({:.1}s < {:.1}s)", d, MIN_DURATION),
                    skipped: true,
                    skip_reason: Some("short_animation".to_string()),
                });
            }
        }
    }

    let input_size = fs::metadata(input)?.len();
    let ext = if options.apple_compat { "mov" } else { "mp4" };
    let output = get_output_path(input, ext, options)?;

    if output.exists() && !options.force {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, has_alpha);

    let flag_mode = options
        .flag_mode()
        .map_err(VidQualityError::ConversionError)?;

    let use_gpu = options.use_gpu;
    if !use_gpu && options.verbose {
        eprintln!("   🖥️  CPU Mode: Using libx265 for higher SSIM (≥0.98)");
    }

    if options.verbose {
        eprintln!(
            "   {} Mode: CRF {:.1} (based on input analysis)",
            flag_mode.description_en(),
            initial_crf
        );
    }

    let (_max_crf, _min_ssim) = shared_utils::video_explorer::calculate_smart_thresholds(
        initial_crf,
        shared_utils::VideoEncoder::Hevc,
    );

    let explore_result = if flag_mode.is_ultimate() {
        shared_utils::explore_hevc_with_gpu_coarse_ultimate(
            input,
            &temp_output,
            vf_args,
            initial_crf,
            true,
            options.child_threads,
        )
    } else {
        shared_utils::explore_hevc_with_gpu_coarse(
            input,
            &temp_output,
            vf_args,
            initial_crf,
            options.child_threads,
        )
    }
    .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

    for log in &explore_result.log {
        eprintln!("{}", log);
    }

    let tolerance_ratio = if options.allow_size_tolerance {
        1.01
    } else {
        1.0
    };
    let max_allowed_size = (input_size as f64 * tolerance_ratio) as u64;

    if explore_result.output_size > max_allowed_size {
        let size_increase_pct =
            ((explore_result.output_size as f64 / input_size as f64) - 1.0) * 100.0;
        if let Err(e) = fs::remove_file(&temp_output) {
            eprintln!("⚠️ [cleanup] Failed to remove oversized HEVC output: {}", e);
        }
        if options.allow_size_tolerance {
            eprintln!(
                "   ⏭️  Skipping: HEVC output larger than input by {:.1}% (tolerance: 1.0%)",
                size_increase_pct
            );
        } else {
            eprintln!(
                "   ⏭️  Skipping: HEVC output larger than input by {:.1}% (strict mode: no tolerance)",
                size_increase_pct
            );
        }
        eprintln!(
            "   📊 Size comparison: {} → {} bytes (+{:.1}%)",
            input_size, explore_result.output_size, size_increase_pct
        );
        copy_original_on_skip(input, options);
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!(
                "Skipped: HEVC output larger than input by {:.1}% ({}x{}, tolerance exceeded)",
                size_increase_pct, width, height
            ),
            skipped: true,
            skip_reason: Some("size_increase_beyond_tolerance".to_string()),
        });
    }

    if !explore_result.quality_passed {
        let actual_ssim = explore_result.ssim.unwrap_or(0.0);
        let threshold = explore_result.actual_min_ssim;

        let video_stream_compressed =
            explore_result.output_video_stream_size < explore_result.input_video_stream_size;

        let (protect_msg, delete_msg) = if !video_stream_compressed {
            let input_stream_kb = explore_result.input_video_stream_size as f64 / 1024.0;
            let output_stream_kb = explore_result.output_video_stream_size as f64 / 1024.0;
            let stream_change_pct = if explore_result.input_video_stream_size > 0 {
                (output_stream_kb / input_stream_kb - 1.0) * 100.0
            } else {
                0.0
            };
            tracing::warn!(input = %input.display(), "Video stream compression failed: {:.1}KB → {:.1}KB", input_stream_kb, output_stream_kb);
            eprintln!(
                "   ⚠️  VIDEO STREAM COMPRESSION FAILED: {:.1} KB → {:.1} KB ({:+.1}%)",
                input_stream_kb, output_stream_kb, stream_change_pct
            );
            eprintln!("   ⚠️  File may already be highly optimized");
            (
                "Original file PROTECTED (output did not compress)".to_string(),
                "Output discarded (video stream larger than original)".to_string(),
            )
        } else if explore_result.ssim.is_none() {
            tracing::warn!(input = %input.display(), "SSIM calculation failed — cannot validate quality");
            eprintln!("   ⚠️  SSIM CALCULATION FAILED - cannot validate quality!");
            eprintln!("   ⚠️  This may indicate codec compatibility issues");
            (
                "Original file PROTECTED (SSIM not available)".to_string(),
                "Output discarded (SSIM calculation failed)".to_string(),
            )
        } else if actual_ssim < threshold {
            tracing::warn!(input = %input.display(), ssim = actual_ssim, threshold, "Quality validation failed");
            eprintln!(
                "   ⚠️  Quality validation FAILED: SSIM {:.4} < {:.4}",
                actual_ssim, threshold
            );
            (
                "Original file PROTECTED (quality below threshold)".to_string(),
                "Output discarded (quality below threshold)".to_string(),
            )
        } else {
            let reason = explore_result
                .enhanced_verify_fail_reason
                .as_deref()
                .unwrap_or("unknown reason");
            tracing::warn!(input = %input.display(), reason, "Quality validation failed");
            eprintln!("   ⚠️  Quality validation FAILED: {}", reason);
            (
                "Original file PROTECTED (quality/size check failed)".to_string(),
                "Output discarded (quality/size check failed)".to_string(),
            )
        };
        eprintln!("   🛡️  {}", protect_msg);

        // GIF/animated image has no Apple compatibility issue; exclude from Apple compat fallback. On fail: discard output, copy original only.
        if let Err(e) = fs::remove_file(&temp_output) {
            eprintln!("⚠️ [cleanup] Failed to remove output: {}", e);
        } else {
            eprintln!("   🗑️  {}", delete_msg);
        }

        let _ = shared_utils::copy_on_skip_or_fail(
            input,
            options.output_dir.as_deref(),
            options.base_dir.as_deref(),
            false,
        );

        return Ok(ConversionResult {
            success: false,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!(
                "Skipped: SSIM {:.4} below threshold {:.4}",
                actual_ssim, threshold
            ),
            skipped: true,
            skip_reason: Some("quality_failed".to_string()),
        });
    }

    if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    shared_utils::copy_metadata(input, &output);
    mark_as_processed(input);

    if options.should_delete_original()
        && shared_utils::conversion::safe_delete_original(
                    input,
                    &output,
                    shared_utils::conversion::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                ).is_ok()
    {}

    let reduction_pct = -explore_result.size_change_pct;
    let explored_msg = if (explore_result.optimal_crf - initial_crf).abs() > 0.1 {
        format!(" (explored from CRF {:.1})", initial_crf)
    } else {
        String::new()
    };

    let ssim_msg = explore_result
        .ssim
        .map(|s| format!(", SSIM: {:.4}", s))
        .unwrap_or_default();

    let message = format!(
        "HEVC (CRF {:.1}{}, {} iter{}): -{:.1}%",
        explore_result.optimal_crf,
        explored_msg,
        explore_result.iterations,
        ssim_msg,
        reduction_pct
    );

    Ok(ConversionResult {
        success: true,
        input_path: input.display().to_string(),
        output_path: Some(output.display().to_string()),
        input_size,
        output_size: Some(explore_result.output_size),
        size_reduction: Some(reduction_pct),
        message,
        skipped: false,
        skip_reason: None,
    })
}

pub fn convert_to_hevc_mkv_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    eprintln!(
        "⚠️  Mathematical lossless HEVC encoding - this will be SLOW and produce large files!"
    );

    if !options.force && is_already_processed(input) {
        return Ok(skipped_already_processed(input));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mkv", options)?;

    if output.exists() && !options.force {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, false);

    let max_threads = get_max_threads(options);
    let x265_params = format!("lossless=1:log-level=error:pools={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads")
        .arg(max_threads.to_string())
        .arg("-i")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg("-c:v")
        .arg("libx265")
        .arg("-x265-params")
        .arg(&x265_params)
        .arg("-preset")
        .arg("medium")
        .arg("-tag:v")
        .arg("hvc1");

    for arg in &vf_args {
        cmd.arg(arg);
    }

    cmd.arg(shared_utils::safe_path_arg(&temp_output).as_ref());
    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
                return Ok(skipped_output_exists(input, &output, input_size));
            }

            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            shared_utils::copy_metadata(input, &output);
            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(
                    input,
                    &output,
                    shared_utils::conversion::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                ).is_ok()
            {
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("Lossless HEVC: size reduced {:.1}%", reduction_pct)
            } else {
                format!("Lossless HEVC: size increased {:.1}%", -reduction_pct)
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
            let _ = fs::remove_file(&temp_output);
            Err(VidQualityError::ConversionError(format!(
                "ffmpeg lossless failed: {}",
                stderr
            )))
        }
        Err(e) => {
            let _ = fs::remove_file(&temp_output);
            Err(VidQualityError::ConversionError(format!(
                "ffmpeg not found: {}",
                e
            )))
        }
    }
}

pub fn convert_to_gif_apple_compat(
    input: &Path,
    options: &ConvertOptions,
    fps: Option<f32>,
) -> Result<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Ok(skipped_already_processed(input));
    }

    if is_static_animated_image(input) {
        let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        if options.verbose {
            eprintln!(
                "   ⏭️  Detected static animated image (1 frame), skipping GIF conversion: {}",
                input.display()
            );
        }
        copy_original_on_skip(input, options);
        mark_as_processed(input);
        return Ok(skipped_static_animated(input, input_size));
    }

    let input_size = fs::metadata(input)?.len();

    let input_ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if input_ext == "gif" {
        eprintln!("   ⏭️  Input is already GIF, skipping re-encode (would likely increase size)");
        mark_as_processed(input);
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(input.display().to_string()),
            input_size,
            output_size: Some(input_size),
            size_reduction: Some(0.0),
            message: "Skipped: Already GIF (re-encoding would increase size)".to_string(),
            skipped: true,
            skip_reason: Some("already_gif".to_string()),
        });
    }

    let output = get_output_path(input, "gif", options)?;

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if output.exists() && !options.force {
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: Some(fs::metadata(&output)?.len()),
            size_reduction: None,
            message: "Skipped: Output already exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    let (width, height) = get_input_dimensions(input)?;
    let fps_val = fps.unwrap_or(10.0);

    let palette_path = output.with_extension("palette.png");

    struct PaletteGuard<'a> {
        path: &'a Path,
    }
    impl<'a> Drop for PaletteGuard<'a> {
        fn drop(&mut self) {
            if self.path.exists() {
                if let Err(e) = fs::remove_file(self.path) {
                    eprintln!("⚠️ [cleanup] Failed to remove temp palette file: {}", e);
                }
            }
        }
    }
    let _palette_guard = PaletteGuard {
        path: &palette_path,
    };

    let palette_result = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg("-vf")
        .arg(format!(
            "fps={},scale={}:{}:flags=lanczos,palettegen=max_colors=256:stats_mode=diff",
            fps_val, width, height
        ))
        .arg(shared_utils::safe_path_arg(&palette_path).as_ref())
        .output();

    if let Err(e) = palette_result {
        return Err(VidQualityError::ConversionError(format!(
            "ffmpeg not found: {}",
            e
        )));
    }

    let result = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg("-i")
        .arg(shared_utils::safe_path_arg(&palette_path).as_ref())
        .arg("-lavfi")
        .arg(format!(
            "fps={},scale={}:{}:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle",
            fps_val, width, height
        ))
        .arg(shared_utils::safe_path_arg(&temp_output).as_ref())
        .output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();
            if output_size == 0 {
                let _ = fs::remove_file(&temp_output);
                return Err(VidQualityError::ConversionError(
                    "GIF output file is empty (encoding may have failed)".to_string(),
                ));
            }
            if get_input_dimensions(&temp_output).is_err() {
                let _ = fs::remove_file(&temp_output);
                return Err(VidQualityError::ConversionError(
                    "GIF output file is not readable (invalid or corrupted)".to_string(),
                ));
            }
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            let tolerance_ratio = if options.allow_size_tolerance {
                1.01
            } else {
                1.0
            };
            let max_allowed_size = (input_size as f64 * tolerance_ratio) as u64;

            if output_size > max_allowed_size {
                let size_increase_pct = ((output_size as f64 / input_size as f64) - 1.0) * 100.0;
                if let Err(e) = fs::remove_file(&temp_output) {
                    eprintln!("⚠️ [cleanup] Failed to remove oversized GIF output: {}", e);
                }
                if options.allow_size_tolerance {
                    eprintln!(
                        "   ⏭️  Skipping: GIF output larger than input by {:.1}% (tolerance: 1.0%)",
                        size_increase_pct
                    );
                } else {
                    eprintln!(
                        "   ⏭️  Skipping: GIF output larger than input by {:.1}% (strict mode: no tolerance)",
                        size_increase_pct
                    );
                }
                eprintln!(
                    "   📊 Size comparison: {} → {} bytes (+{:.1}%)",
                    input_size, output_size, size_increase_pct
                );
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: format!(
                        "Skipped: GIF output larger than input by {:.1}% (tolerance exceeded)",
                        size_increase_pct
                    ),
                    skipped: true,
                    skip_reason: Some("size_increase_beyond_tolerance".to_string()),
                });
            }

            if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: Some(output.display().to_string()),
                    input_size,
                    output_size: Some(fs::metadata(&output)?.len()),
                    size_reduction: None,
                    message: "Skipped: Output already exists".to_string(),
                    skipped: true,
                    skip_reason: Some("exists".to_string()),
                });
            }

            shared_utils::copy_metadata(input, &output);
            mark_as_processed(input);

            if options.should_delete_original() {
                let _ = shared_utils::conversion::safe_delete_original(
                    input,
                    &output,
                    shared_utils::conversion::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                );
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("GIF (Apple Compat): size reduced {:.1}%", reduction_pct)
            } else {
                format!("GIF (Apple Compat): size increased {:.1}%", -reduction_pct)
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
            let _ = fs::remove_file(&temp_output);
            Err(VidQualityError::ConversionError(format!(
                "ffmpeg GIF conversion failed: {}",
                stderr
            )))
        }
        Err(e) => {
            let _ = fs::remove_file(&temp_output);
            Err(VidQualityError::ConversionError(format!(
                "ffmpeg not found: {}",
                e
            )))
        }
    }
}
