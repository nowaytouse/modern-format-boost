//! Animated Image → Video Conversion Module
//!
//! Handles conversion of animated images (GIF, WebP, AVIF, etc.) to video formats.
//! Migrated from img_hevc to vid_hevc for clearer separation of concerns:
//! - img_av1: image analysis, format detection, quality estimation
//! - vid_av1: all video encoding (including animated image → video)

use crate::{Result, VidQualityError};
use shared_utils::conversion::{ConversionResult, ConvertOptions};
use std::fs;
use std::path::Path;
use std::process::Command;

use shared_utils::conversion::{
    determine_output_path_with_base, is_already_processed, mark_as_processed,
};

/// Extract frames from animated WebP using webpmux and create APNG with correct timing
fn extract_webp_to_apng(input: &Path, output_apng: &Path, verbose: bool) -> Result<()> {
    // Create temporary directory for frames
    let temp_dir = tempfile::Builder::new()
        .prefix("webp_frames_")
        .tempdir()
        .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp dir: {}", e)))?;
    let temp_dir_path = temp_dir.path();
    
    // Get WebP info to determine frame count and duration
    let webpmux_info = Command::new("webpmux")
        .arg("-info")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .output()
        .map_err(|e| VidQualityError::ConversionError(format!("webpmux not found: {}", e)))?;
    
    if !webpmux_info.status.success() {
        return Err(VidQualityError::ConversionError("webpmux -info failed".to_string()));
    }
    
    let info_str = String::from_utf8_lossy(&webpmux_info.stdout);
    
    // Parse frame count and duration
    let frame_count = info_str.lines()
        .find(|l| l.contains("Number of frames:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .ok_or_else(|| VidQualityError::ConversionError("Failed to parse frame count".to_string()))?;
    
    // Parse duration from first frame (assuming all frames have same duration)
    let frame_duration_ms = info_str.lines()
        .find(|l| l.contains("duration"))
        .and_then(|l| {
            l.split_whitespace()
                .find_map(|s| s.parse::<u32>().ok())
        })
        .ok_or_else(|| VidQualityError::ConversionError("Failed to parse frame duration from WebP".to_string()))?;
    
    let fps = 1000.0 / frame_duration_ms as f64;
    
    if verbose {
        eprintln!("   📊 WebP: {} frames, {}ms/frame, {:.2}fps", frame_count, frame_duration_ms, fps);
    }
    
    // Extract each frame using webpmux and convert to PNG
    for i in 1..=frame_count {
        let frame_webp_path = temp_dir_path.join(format!("frame_{:04}.webp", i));
        let frame_png_path = temp_dir_path.join(format!("frame_{:04}.png", i));
        
        // Extract frame as WebP
        let extract_result = Command::new("webpmux")
            .arg("-get")
            .arg("frame")
            .arg(i.to_string())
            .arg(shared_utils::safe_path_arg(input).as_ref())
            .arg("-o")
            .arg(&frame_webp_path)
            .output()
            .map_err(|e| VidQualityError::ConversionError(format!("webpmux extract failed: {}", e)))?;
        
        if !extract_result.status.success() {
            return Err(VidQualityError::ConversionError(format!("Failed to extract frame {}", i)));
        }
        
        // Convert WebP frame to PNG using FFmpeg
        let convert_result = Command::new("ffmpeg")
            .arg("-y")
            .arg("-i")
            .arg(&frame_webp_path)
            .arg(&frame_png_path)
            .output()
            .map_err(|e| VidQualityError::ConversionError(format!("FFmpeg WebP→PNG conversion failed: {}", e)))?;
        
        if !convert_result.status.success() {
            let stderr = String::from_utf8_lossy(&convert_result.stderr);
            return Err(VidQualityError::ConversionError(format!("Failed to convert frame {} to PNG: {}", i, stderr)));
        }
    }
    
    // Create APNG from PNG sequence using FFmpeg
    let pattern = temp_dir_path.join("frame_%04d.png");
    let ffmpeg_result = Command::new("ffmpeg")
        .arg("-y")
        .arg("-r")  // Use -r for input frame rate
        .arg(fps.to_string())
        .arg("-i")
        .arg(&pattern)
        .arg("-c:v")
        .arg("apng")  // Use apng codec, not png
        .arg("-f")
        .arg("apng")
        .arg("-plays")
        .arg("0") // Loop forever
        .arg(shared_utils::safe_path_arg(output_apng).as_ref())
        .output()
        .map_err(|e| VidQualityError::ConversionError(format!("FFmpeg APNG creation failed: {}", e)))?;
    
    if !ffmpeg_result.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_result.stderr);
        return Err(VidQualityError::ConversionError(format!("FFmpeg APNG creation failed: {}", stderr)));
    }
    
    if verbose {
        shared_utils::progress_mode::emit_stderr(&format!("   ✅ WebP → APNG conversion successful ({} frames, {:.2}fps)", frame_count, fps));
    }
    
    Ok(())
}

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

/// For GIF inputs: return true when the multi-dimensional meme-score indicates this GIF should be
/// kept as-is rather than converted to a video container.
///
/// Uses ffprobe to gather resolution / fps / frame-count / duration, then applies the weighted
/// scoring from `shared_utils::gif_meme_score`.  A score ≥ 0.50 → keep as GIF.
/// Returns false for all non-GIF paths so the caller proceeds with normal conversion.
fn is_gif_meme(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if ext != "gif" {
        return false;
    }
    let file_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if let Ok(probe) = shared_utils::probe_video(path) {
        if let Some(meta) = shared_utils::gif_meta_from_probe_with_path(&probe, file_size, path) {
            return shared_utils::should_keep_as_gif(&meta);
        }
    }
    false
}

/// Returns true if the file is an animated image format but effectively static (0 or negligible duration).
/// Callers should skip video conversion and treat as static image (e.g. route to JXL in img_av1).
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
        if let Some(duration_secs) = analysis.duration_secs {
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

pub fn convert_to_av1_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
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

    // GIF multi-dimensional meme-score: if the GIF looks like a meme/sticker, keep it as-is.
    if is_gif_meme(input) {
        let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        copy_original_on_skip(input, options);
        mark_as_processed(input);
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: "Skipped: GIF identified as meme/sticker (meme-score ≥ 0.50)".to_string(),
            skipped: true,
            skip_reason: Some("gif_meme".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    
    let input_ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    
    let ext = if options.apple_compat { "mov" } else { "mp4" };
    let output = get_output_path(input, ext, options)?;

    if output.exists() && !options.force {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    // Special handling for animated JXL: FFmpeg's jpegxl_anim decoder is incomplete
    // and cannot properly decode animated JXL files. We must use djxl to convert to APNG first.
    // Special handling for animated WebP: FFmpeg's WebP decoder is unreliable for animated WebP.
    // We must use webpmux to extract frames and create APNG with correct timing.
    let (actual_input, temp_apng_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) = 
        if input_ext == "jxl" {
            if options.verbose {
                eprintln!("   🔧 Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)");
            }
            
            // Check if djxl is available
            if which::which("djxl").is_err() {
                tracing::warn!(input = %input.display(), "djxl not found; cannot process animated JXL");
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: false,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: "Skipped: djxl not found (required for animated JXL)".to_string(),
                    skipped: true,
                    skip_reason: Some("djxl_not_found".to_string()),
                });
            }
            
            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp APNG: {}", e)))?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            
            // Convert JXL to APNG using djxl
            let djxl_result = Command::new("djxl")
                .arg(shared_utils::safe_path_arg(input).as_ref())
                .arg(shared_utils::safe_path_arg(&temp_apng_path).as_ref())
                .output();
            
            match djxl_result {
                Ok(output) if output.status.success() && temp_apng_path.exists() => {
                    if options.verbose {
                        shared_utils::progress_mode::emit_stderr("   ✅ JXL → APNG conversion successful");
                    }
                    (temp_apng_path, Some(temp_apng))
                }
                _ => {
                    tracing::warn!(input = %input.display(), "djxl conversion failed");
                    copy_original_on_skip(input, options);
                    mark_as_processed(input);
                    return Ok(ConversionResult {
                        success: false,
                        input_path: input.display().to_string(),
                        output_path: None,
                        input_size,
                        output_size: None,
                        size_reduction: None,
                        message: "JXL → APNG conversion failed (djxl error)".to_string(),
                        skipped: true,
                        skip_reason: Some("djxl_failed".to_string()),
                    });
                }
            }
        } else if input_ext == "webp" {
            if options.verbose {
                eprintln!("   🔧 Detected WebP format, extracting frames with webpmux");
            }
            
            // Check if webpmux is available
            if which::which("webpmux").is_err() {
                tracing::warn!(input = %input.display(), "webpmux not found; cannot process animated WebP");
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: false,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: "Skipped: webpmux not found (required for animated WebP)".to_string(),
                    skipped: true,
                    skip_reason: Some("webpmux_not_found".to_string()),
                });
            }
            
            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp APNG: {}", e)))?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            
            // Extract WebP frames and create APNG with correct timing
            match extract_webp_to_apng(input, &temp_apng_path, options.verbose) {
                Ok(_) => (temp_apng_path, Some(temp_apng)),
                Err(e) => {
                    tracing::warn!(input = %input.display(), error = %e, "WebP extraction failed");
                    copy_original_on_skip(input, options);
                    mark_as_processed(input);
                    return Ok(ConversionResult {
                        success: false,
                        input_path: input.display().to_string(),
                        output_path: None,
                        input_size,
                        output_size: None,
                        size_reduction: None,
                        message: format!("WebP extraction failed: {}", e),
                        skipped: true,
                        skip_reason: Some("webp_extraction_failed".to_string()),
                    });
                }
            }
        } else {
            (input.to_path_buf(), None)
        };

    let (width, height) = get_input_dimensions(&actual_input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, false);

    let max_threads = get_max_threads(options);
    let svtav1_params = format!("tune=0:film-grain=0:lp={}", max_threads);
    
    // Probe ORIGINAL input to get stream index for multi-stream files (animated AVIF/HEIC)
    // For JXL/WebP, actual_input is APNG (single stream), so we probe the original input
    let stream_idx = if let Ok(probe) = shared_utils::probe_video(input) {
        probe.stream_index
    } else {
        0 // Default to first stream
    };
    
    // For APNG (converted from JXL/WebP), stream_idx should be 0 since APNG is single-stream
    // For AVIF/HEIC with multiple streams, use the stream_idx from probe
    let effective_stream_idx = if input_ext == "jxl" || input_ext == "webp" {
        0 // APNG is always single-stream
    } else {
        stream_idx
    };
    
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads")
        .arg(max_threads.to_string())
        .arg("-i")
        .arg(shared_utils::safe_path_arg(&actual_input).as_ref())
        .arg("-map")
        .arg(format!("0:{}", effective_stream_idx))  // Select the correct stream
        // NO -r parameter: preserve original frame rate
        .arg("-c:v")
        .arg("libsvtav1")
        .arg("-crf")
        .arg("0")
        .arg("-preset")
        .arg("6")
        .arg("-tag:v")
        .arg("av01")
        .arg("-svtav1-params")
        .arg(&svtav1_params);

    for arg in &vf_args {
        cmd.arg(arg);
    }

    cmd.arg(shared_utils::safe_path_arg(&temp_output).as_ref());
    let result = cmd.output();

    // Clean up temporary APNG file if it was created
    drop(temp_apng_file);

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output).map(|m| m.len()).unwrap_or(0);
            if output_size == 0 || get_input_dimensions(&temp_output).is_err() {
                let _ = fs::remove_file(&temp_output);
                tracing::warn!(input = %input.display(), "AV1 output invalid (empty or unreadable); copying original");
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                let sz = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
                return Ok(ConversionResult {
                    success: false,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size: sz,
                    output_size: None,
                    size_reduction: None,
                    message: "AV1 output invalid; original copied".to_string(),
                    skipped: true,
                    skip_reason: Some("av1_invalid_output".to_string()),
                });
            }

            if !shared_utils::conversion::commit_temp_to_output(&temp_output, &output, options.force)? {
                return Ok(skipped_output_exists(input, &output, input_size));
            }

            let reduction = 1.0 - (output_size as f64 / input_size as f64);

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
                format!(
                    "AV1 conversion successful: size reduced \x1b[1;32m{:.1}%\x1b[0m",
                    reduction_pct
                )
            } else {
                format!(
                    "AV1 conversion successful: size increased \x1b[1;33m{:.1}%\x1b[0m",
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
            tracing::warn!(input = %input.display(), stderr = %stderr, "ffmpeg AV1 encode failed; copying original");
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            let sz = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            Ok(ConversionResult {
                success: false,
                input_path: input.display().to_string(),
                output_path: None,
                input_size: sz,
                output_size: None,
                size_reduction: None,
                message: format!("AV1 encode failed; original copied (ffmpeg: {})", stderr.lines().last().unwrap_or("")),
                skipped: true,
                skip_reason: Some("av1_encode_failed".to_string()),
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&temp_output);
            tracing::warn!(input = %input.display(), err = %e, "ffmpeg not found; copying original");
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            let sz = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            Ok(ConversionResult {
                success: false,
                input_path: input.display().to_string(),
                output_path: None,
                input_size: sz,
                output_size: None,
                size_reduction: None,
                message: format!("AV1 encode failed (ffmpeg not found: {}); original copied", e),
                skipped: true,
                skip_reason: Some("av1_encode_failed".to_string()),
            })
        }
    }
}

pub fn convert_to_av1_mp4_matched(
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

    // GIF multi-dimensional meme-score: if the GIF looks like a meme/sticker, keep it as-is.
    if is_gif_meme(input) {
        let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        copy_original_on_skip(input, options);
        mark_as_processed(input);
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: "Skipped: GIF identified as meme/sticker (meme-score ≥ 0.50)".to_string(),
            skipped: true,
            skip_reason: Some("gif_meme".to_string()),
        });
    }

    let input_size = fs::metadata(input)?.len();
    
    let input_ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    
    let ext = if options.apple_compat { "mov" } else { "mp4" };
    let output = get_output_path(input, ext, options)?;

    if output.exists() && !options.force {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::conversion::temp_path_for_output(&output);

    // Special handling for animated JXL/WebP: pre-convert to APNG
    let (actual_input, temp_apng_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) = 
        if input_ext == "jxl" {
            if options.verbose {
                eprintln!("   🔧 Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)");
            }
            if which::which("djxl").is_err() {
                tracing::warn!(input = %input.display(), "djxl not found; cannot process animated JXL");
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: false,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: "Skipped: djxl not found (required for animated JXL)".to_string(),
                    skipped: true,
                    skip_reason: Some("djxl_not_found".to_string()),
                });
            }
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp APNG: {}", e)))?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            let djxl_result = Command::new("djxl")
                .arg(shared_utils::safe_path_arg(input).as_ref())
                .arg(shared_utils::safe_path_arg(&temp_apng_path).as_ref())
                .output();
            match djxl_result {
                Ok(output) if output.status.success() && temp_apng_path.exists() => {
                    if options.verbose {
                        shared_utils::progress_mode::emit_stderr("   ✅ JXL → APNG conversion successful");
                    }
                    (temp_apng_path, Some(temp_apng))
                }
                _ => {
                    tracing::warn!(input = %input.display(), "djxl conversion failed");
                    copy_original_on_skip(input, options);
                    mark_as_processed(input);
                    return Ok(ConversionResult {
                        success: false,
                        input_path: input.display().to_string(),
                        output_path: None,
                        input_size,
                        output_size: None,
                        size_reduction: None,
                        message: "JXL → APNG conversion failed (djxl error)".to_string(),
                        skipped: true,
                        skip_reason: Some("djxl_failed".to_string()),
                    });
                }
            }
        } else if input_ext == "webp" {
            if options.verbose {
                eprintln!("   🔧 Detected WebP format, extracting frames with webpmux");
            }
            
            // Check if webpmux is available
            if which::which("webpmux").is_err() {
                tracing::warn!(input = %input.display(), "webpmux not found; cannot process animated WebP");
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: false,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: "Skipped: webpmux not found (required for animated WebP)".to_string(),
                    skipped: true,
                    skip_reason: Some("webpmux_not_found".to_string()),
                });
            }
            
            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp APNG: {}", e)))?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            
            // Extract WebP frames and create APNG with correct timing
            match extract_webp_to_apng(input, &temp_apng_path, options.verbose) {
                Ok(_) => (temp_apng_path, Some(temp_apng)),
                Err(e) => {
                    tracing::warn!(input = %input.display(), error = %e, "WebP extraction failed");
                    copy_original_on_skip(input, options);
                    mark_as_processed(input);
                    return Ok(ConversionResult {
                        success: false,
                        input_path: input.display().to_string(),
                        output_path: None,
                        input_size,
                        output_size: None,
                        size_reduction: None,
                        message: format!("WebP extraction failed: {}", e),
                        skipped: true,
                        skip_reason: Some("webp_extraction_failed".to_string()),
                    });
                }
            }
        } else {
            (input.to_path_buf(), None)
        };

    // For multi-stream AVIF/HEIC, convert the correct stream to APNG
    // This ensures explore functions work with the correct stream
    let (final_input, temp_stream_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) = 
        if (input_ext == "avif" || input_ext == "heic" || input_ext == "heif") && temp_apng_file.is_none() {
            if let Ok(probe) = shared_utils::probe_video(input) {
                // Check if there are multiple video streams
                let stream_count_output = Command::new("ffprobe")
                    .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=index", "-of", "csv=p=0"])
                    .arg(shared_utils::safe_path_arg(input).as_ref())
                    .output();
                
                let has_multiple_streams = stream_count_output
                    .map(|o| String::from_utf8_lossy(&o.stdout).lines().count() > 1)
                    .unwrap_or(false);
                
                if has_multiple_streams && probe.stream_index > 0 {
                    if options.verbose {
                        eprintln!("   🔧 Multi-stream {} detected, converting stream {} to APNG ({} frames)", 
                            input_ext.to_uppercase(), probe.stream_index, probe.frame_count);
                    }
                    
                    // Create temporary APNG file
                    let temp_stream = tempfile::Builder::new()
                        .suffix(".apng")
                        .tempfile()
                        .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp APNG: {}", e)))?;
                    let temp_stream_path = temp_stream.path().to_path_buf();
                    
                    // Convert the correct stream to APNG using FFmpeg
                    let extract_result = Command::new("ffmpeg")
                        .arg("-y")
                        .arg("-i")
                        .arg(shared_utils::safe_path_arg(input).as_ref())
                        .arg("-map")
                        .arg(format!("0:{}", probe.stream_index))
                        .arg("-c:v")
                        .arg("apng")
                        .arg("-f")
                        .arg("apng")
                        .arg("-plays")
                        .arg("0")
                        .arg(shared_utils::safe_path_arg(&temp_stream_path).as_ref())
                        .output();
                    
                    match extract_result {
                        Ok(output) if output.status.success() && temp_stream_path.exists() => {
                            if options.verbose {
                                shared_utils::progress_mode::emit_stderr("   ✅ Stream → APNG conversion successful");
                            }
                            (temp_stream_path, Some(temp_stream))
                        }
                        _ => {
                            if options.verbose {
                                eprintln!("   ⚠️  Stream conversion failed, using original file");
                            }
                            (actual_input, None)
                        }
                    }
                } else {
                    (actual_input, None)
                }
            } else {
                (actual_input, None)
            }
        } else {
            (actual_input, None)
        };

    let (width, height) = get_input_dimensions(&final_input)?;
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
        shared_utils::VideoEncoder::Av1,
    );

    let explore_result = if flag_mode.is_ultimate() {
        shared_utils::explore_av1_with_gpu_coarse_ultimate(
            &final_input,
            &temp_output,
            vf_args,
            initial_crf,
            true,
            options.allow_size_tolerance,
            options.child_threads,
        )
    } else {
        shared_utils::explore_av1_with_gpu_coarse(
            &final_input,
            &temp_output,
            vf_args,
            initial_crf,
            options.allow_size_tolerance,
            options.child_threads,
        )
    }
    .map_err(|e: anyhow::Error| VidQualityError::ConversionError(e.to_string()))?;

    // Clean up temporary files
    drop(temp_apng_file);
    drop(temp_stream_file);

    for log in &explore_result.log {
        eprintln!("{}", log);
    }

    let tolerance_ratio = if options.allow_size_tolerance {
        1.01
    } else {
        1.0
    };
    let max_allowed_size = (input_size as f64 * tolerance_ratio) as u64;

    // apple_compat mode: compatibility takes priority over file size.
    // The whole point is to make the output playable on Apple devices — keeping a
    // non-playable original is worse than a slightly larger AV1 file.
    let size_guard_active = !options.apple_compat;

    if size_guard_active && explore_result.output_size > max_allowed_size {
        let size_increase_pct =
            ((explore_result.output_size as f64 / input_size as f64) - 1.0) * 100.0;
        if let Err(e) = fs::remove_file(&temp_output) {
            eprintln!("⚠️ [cleanup] Failed to remove oversized AV1 output: {}", e);
        }
        if options.allow_size_tolerance {
            eprintln!(
                "   ⏭️  Skipping: AV1 output larger than input by {:.1}% (tolerance: 1.0%)",
                size_increase_pct
            );
        } else {
            eprintln!(
                "   ⏭️  Skipping: AV1 output larger than input by {:.1}% (strict mode: no tolerance)",
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
                "Skipped: AV1 output larger than input by {:.1}% ({}x{}, tolerance exceeded)",
                size_increase_pct, width, height
            ),
            skipped: true,
            skip_reason: Some("size_increase_beyond_tolerance".to_string()),
        });
    }

    // apple_compat: if quality_passed=false only because the file couldn't be compressed
    // (not because of actual quality degradation), still accept the AV1 output.
    // A larger-but-playable AV1 is always better than a non-playable original (e.g. AVIF).
    let quality_or_compat_ok = explore_result.quality_passed
        || (options.apple_compat && explore_result.ssim.is_some_and(|s| s >= 0.90));

    if !quality_or_compat_ok {
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
                "   ⚠️  VIDEO STREAM COMPRESSION FAILED: {:.1} KB → {:.1} KB ({:+.1}%) │ File may already be highly optimized",
                input_stream_kb, output_stream_kb, stream_change_pct
            );
            (
                "Original file PROTECTED (output did not compress)".to_string(),
                "Output discarded (video stream larger than original)".to_string(),
            )
        } else if explore_result.ssim.is_none() {
            tracing::warn!(input = %input.display(), "SSIM calculation failed — cannot validate quality");
            eprintln!("   ⚠️  SSIM CALCULATION FAILED │ cannot validate quality │ may indicate codec compatibility issues");
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
        eprintln!("   ⚠️  {} │ 🛡️  {} │ 🗑️  {}", 
            if !video_stream_compressed { "VIDEO STREAM COMPRESSION FAILED" } 
            else if explore_result.ssim.is_none() { "SSIM CALCULATION FAILED" }
            else { "QUALITY VALIDATION FAILED" },
            protect_msg, delete_msg);

        let _ = shared_utils::copy_on_skip_or_fail(
            input,
            options.output_dir.as_deref(),
            options.base_dir.as_deref(),
            false,
        );
        mark_as_processed(input);

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

    if options.should_delete_original() {
        let _ = shared_utils::conversion::safe_delete_original(
            input,
            &output,
            shared_utils::conversion::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
        );
    }

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
        "AV1 (CRF {:.1}{}, {} iter{}): -{:.1}%",
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

pub fn convert_to_av1_mkv_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    eprintln!(
        "⚠️  Mathematical lossless AV1 encoding - this will be SLOW and produce large files!"
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
    let svtav1_params = format!("lossless=1:tune=0:film-grain=0:lp={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads")
        .arg(max_threads.to_string())
        .arg("-i")
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg("-c:v")
        .arg("libsvtav1")
        .arg("-svtav1-params")
        .arg(&svtav1_params)
        .arg("-preset")
        .arg("6")
        .arg("-tag:v")
        .arg("av01");

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

            if options.should_delete_original() {
                let _ = shared_utils::conversion::safe_delete_original(
                    input,
                    &output,
                    shared_utils::conversion::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                );
            }

            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("Lossless AV1: size reduced \x1b[1;32m{:.1}%\x1b[0m", reduction_pct)
            } else {
                format!("Lossless AV1: size increased \x1b[1;33m{:.1}%\x1b[0m", -reduction_pct)
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
            tracing::warn!(input = %input.display(), stderr = %stderr, "ffmpeg lossless AV1 failed; copying original");
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            let sz = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            Ok(ConversionResult {
                success: false,
                input_path: input.display().to_string(),
                output_path: None,
                input_size: sz,
                output_size: None,
                size_reduction: None,
                message: format!("Lossless AV1 failed; original copied ({})", stderr.lines().last().unwrap_or("")),
                skipped: true,
                skip_reason: Some("av1_lossless_failed".to_string()),
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&temp_output);
            tracing::warn!(input = %input.display(), err = %e, "ffmpeg not found for lossless AV1; copying original");
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            let sz = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            Ok(ConversionResult {
                success: false,
                input_path: input.display().to_string(),
                output_path: None,
                input_size: sz,
                output_size: None,
                size_reduction: None,
                message: format!("Lossless AV1 failed (ffmpeg not found: {}); original copied", e),
                skipped: true,
                skip_reason: Some("av1_lossless_failed".to_string()),
            })
        }
    }
}

pub fn convert_to_gif_apple_compat(
    input: &Path,
    options: &ConvertOptions,
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

    // Special handling for animated JXL: FFmpeg's jpegxl_anim decoder is incomplete
    // and cannot properly decode animated JXL files. We must use djxl to convert to APNG first.
    // Special handling for animated WebP: FFmpeg's WebP decoder is unreliable for animated WebP.
    // We must use webpmux to extract frames and create APNG with correct timing.
    let (actual_input, temp_apng_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) = 
        if input_ext == "jxl" {
            if options.verbose {
                eprintln!("   🔧 Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)");
            }
            
            // Check if djxl is available
            if which::which("djxl").is_err() {
                tracing::warn!(input = %input.display(), "djxl not found; cannot process animated JXL");
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: false,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: "Skipped: djxl not found (required for animated JXL)".to_string(),
                    skipped: true,
                    skip_reason: Some("djxl_not_found".to_string()),
                });
            }
            
            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp APNG: {}", e)))?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            
            // Convert JXL to APNG using djxl
            let djxl_result = Command::new("djxl")
                .arg(shared_utils::safe_path_arg(input).as_ref())
                .arg(shared_utils::safe_path_arg(&temp_apng_path).as_ref())
                .output();
            
            match djxl_result {
                Ok(output) if output.status.success() && temp_apng_path.exists() => {
                    if options.verbose {
                        shared_utils::progress_mode::emit_stderr("   ✅ JXL → APNG conversion successful");
                    }
                    (temp_apng_path, Some(temp_apng))
                }
                _ => {
                    tracing::warn!(input = %input.display(), "djxl conversion failed");
                    copy_original_on_skip(input, options);
                    mark_as_processed(input);
                    return Ok(ConversionResult {
                        success: false,
                        input_path: input.display().to_string(),
                        output_path: None,
                        input_size,
                        output_size: None,
                        size_reduction: None,
                        message: "JXL → APNG conversion failed (djxl error)".to_string(),
                        skipped: true,
                        skip_reason: Some("djxl_failed".to_string()),
                    });
                }
            }
        } else if input_ext == "webp" {
            if options.verbose {
                eprintln!("   🔧 Detected WebP format, extracting frames with webpmux");
            }
            
            // Check if webpmux is available
            if which::which("webpmux").is_err() {
                tracing::warn!(input = %input.display(), "webpmux not found; cannot process animated WebP");
                copy_original_on_skip(input, options);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: false,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: "Skipped: webpmux not found (required for animated WebP)".to_string(),
                    skipped: true,
                    skip_reason: Some("webpmux_not_found".to_string()),
                });
            }
            
            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp APNG: {}", e)))?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            
            // Extract WebP frames and create APNG with correct timing
            match extract_webp_to_apng(input, &temp_apng_path, options.verbose) {
                Ok(_) => (temp_apng_path, Some(temp_apng)),
                Err(e) => {
                    tracing::warn!(input = %input.display(), error = %e, "WebP extraction failed");
                    copy_original_on_skip(input, options);
                    mark_as_processed(input);
                    return Ok(ConversionResult {
                        success: false,
                        input_path: input.display().to_string(),
                        output_path: None,
                        input_size,
                        output_size: None,
                        size_reduction: None,
                        message: format!("WebP extraction failed: {}", e),
                        skipped: true,
                        skip_reason: Some("webp_extraction_failed".to_string()),
                    });
                }
            }
        } else {
            (input.to_path_buf(), None)
        };

    let (width, height) = get_input_dimensions(&actual_input)?;
    
    // Probe ORIGINAL input to get stream index for multi-stream files (animated AVIF/HEIC)
    // For JXL/WebP, actual_input is APNG (single stream), so we probe the original input
    let stream_idx = if let Ok(probe) = shared_utils::probe_video(input) {
        probe.stream_index
    } else {
        0
    };
    
    // For APNG (converted from JXL/WebP), stream_idx should be 0 since APNG is single-stream
    // For AVIF/HEIC with multiple streams, use the stream_idx from probe
    let effective_stream_idx = if input_ext == "jxl" || input_ext == "webp" {
        0 // APNG is always single-stream
    } else {
        stream_idx
    };
    
    // Check if file has multiple video streams
    let has_multiple_streams = if let Ok(output) = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(&actual_input)
        .output()
    {
        String::from_utf8_lossy(&output.stdout).lines().count() > 1
    } else {
        false
    };

    // Use FFmpeg high-quality single-pass palette method for all formats
    // This ensures consistent quality across all animated formats (AVIF/WebP/JXL/HEIC/etc)
    // Note: JXL is pre-converted to APNG above due to FFmpeg's incomplete jpegxl_anim decoder
    let ffmpeg_ok = {
        let filter = if has_multiple_streams {
            // Multi-stream: specify stream in filter
            format!(
                "[0:{}]scale={}:{}:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=256[p];[s1][p]paletteuse=dither=bayer",
                effective_stream_idx, width, height
            )
        } else {
            // Single-stream: simple filter
            format!(
                "scale={}:{}:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=256[p];[s1][p]paletteuse=dither=bayer",
                width, height
            )
        };
        
        let res = Command::new("ffmpeg")
            .arg("-y")
            .arg("-i")
            .arg(shared_utils::safe_path_arg(&actual_input).as_ref())
            .arg("-filter_complex")
            .arg(&filter)
            .arg(shared_utils::safe_path_arg(&temp_output).as_ref())
            .output();
        matches!(res, Ok(o) if o.status.success() && temp_output.exists())
    };

    // Clean up temporary APNG file if it was created
    drop(temp_apng_file);

    if !ffmpeg_ok {
        // FFmpeg conversion failed — copy original so data is not lost
        let _ = fs::remove_file(&temp_output);
        tracing::warn!(input = %input.display(), "GIF conversion failed (FFmpeg unavailable or failed); copying original");
        copy_original_on_skip(input, options);
        mark_as_processed(input);
        let input_size_fb = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        return Ok(ConversionResult {
            success: false,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: input_size_fb,
            output_size: None,
            size_reduction: None,
            message: "GIF conversion failed (FFmpeg unavailable or failed); original copied".to_string(),
            skipped: true,
            skip_reason: Some("gif_encode_failed".to_string()),
        });
    }

    // Validate output
    let output_size = fs::metadata(&temp_output)
        .map(|m| m.len())
        .unwrap_or(0);
    if output_size == 0 || get_input_dimensions(&temp_output).is_err() {
        let _ = fs::remove_file(&temp_output);
        tracing::warn!(input = %input.display(), "GIF output invalid (empty or unreadable); copying original");
        copy_original_on_skip(input, options);
        mark_as_processed(input);
        return Ok(ConversionResult {
            success: false,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: "GIF output invalid; original copied".to_string(),
            skipped: true,
            skip_reason: Some("gif_invalid_output".to_string()),
        });
    }

    let reduction = 1.0 - (output_size as f64 / input_size as f64);

    let tolerance_ratio = if options.allow_size_tolerance {
        1.01
    } else {
        1.0
    };
    let max_allowed_size = (input_size as f64 * tolerance_ratio) as u64;

    // apple_compat: compatibility takes priority — a playable GIF is always
    // better than a non-playable original (e.g. animated AVIF).
    let size_guard_active = !options.apple_compat;

    if size_guard_active && output_size > max_allowed_size {
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
        format!("GIF (Apple Compat): size reduced \x1b[1;32m{:.1}%\x1b[0m", reduction_pct)
    } else {
        format!("GIF (Apple Compat): size increased \x1b[1;33m{:.1}%\x1b[0m", -reduction_pct)
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
