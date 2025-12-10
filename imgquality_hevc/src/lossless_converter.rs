//! Lossless Converter Module
//! 
//! Provides conversion API for verified lossless/lossy images
//! Uses shared_utils for common functionality (anti-duplicate, ConversionResult, etc.)

use crate::{ImgQualityError, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

// 🔥 模块化：从 shared_utils 导入通用功能
pub use shared_utils::conversion::{
    ConversionResult, ConvertOptions,
    is_already_processed, mark_as_processed, clear_processed_list,
    load_processed_list, save_processed_list,
    format_size_change,
};

/// Convert static image to JXL with specified distance/quality
/// distance: 0.0 = lossless, 0.1 = visually lossless (Q100 lossy), 1.0 = Q90
pub fn convert_to_jxl(input: &Path, options: &ConvertOptions, distance: f32) -> Result<ConversionResult> {
    // Anti-duplicate check
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
    let output = get_output_path(input, "jxl", &options.output_dir)?;
    
    // Ensure output directory exists
    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }
    
    // Check if output already exists
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
    
    // Execute cjxl (v0.11+ syntax)
    // Note: cjxl 默认保留 ICC 颜色配置文件，无需额外参数
    // 🔥 性能优化：限制 cjxl 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let result = Command::new("cjxl")
        .arg(input)
        .arg(&output)
        .arg("-d").arg(format!("{:.1}", distance))  // Distance parameter
        .arg("-e").arg("7")    // Effort 7 (cjxl v0.11+ 范围是 1-10，默认 7)
        .arg("-j").arg(max_threads.to_string())  // 限制线程数
        .output();
    
    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);
            
            // 🔥 智能回退：如果转换后文件变大，删除输出并跳过
            // 这对于小型PNG或已高度优化的图片很常见
            if output_size > input_size {
                let _ = fs::remove_file(&output);
                eprintln!("   ⏭️  Rollback: JXL larger than original ({} → {} bytes, +{:.1}%)", 
                    input_size, output_size, (output_size as f64 / input_size as f64 - 1.0) * 100.0);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: format!("Skipped: JXL would be larger (+{:.1}%)", (output_size as f64 / input_size as f64 - 1.0) * 100.0),
                    skipped: true,
                    skip_reason: Some("size_increase".to_string()),
                });
            }
            
            // Validate output
            if let Err(e) = verify_jxl_health(&output) {
                 let _ = fs::remove_file(&output);
                 return Err(e);
            }

            // Copy metadata and timestamps
            copy_metadata(input, &output);
            
            mark_as_processed(input);
            
            if options.should_delete_original() {
                fs::remove_file(input)?;
            }
            
            // 🔥 修复：正确显示 size reduction/increase 消息
            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("JXL conversion successful: size reduced {:.1}%", reduction_pct)
            } else {
                format!("JXL conversion successful: size increased {:.1}%", -reduction_pct)
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
            Err(ImgQualityError::ConversionError(format!("cjxl failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("cjxl not found: {}", e)))
        }
    }
}

/// Convert JPEG to JXL using lossless JPEG transcode (preserves DCT coefficients)
/// This is the BEST option for JPEG files - no quality loss at all
pub fn convert_jpeg_to_jxl(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    // Anti-duplicate check
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
    let output = get_output_path(input, "jxl", &options.output_dir)?;
    
    // Check if output already exists
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
    
    // Execute cjxl with --lossless_jpeg=1 for lossless JPEG transcode
    // Note: cjxl 默认保留 ICC 颜色配置文件，无需额外参数
    // 🔥 性能优化：限制 cjxl 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let result = Command::new("cjxl")
        .arg(input)
        .arg(&output)
        .arg("--lossless_jpeg=1")  // Lossless JPEG transcode - preserves DCT coefficients
        .arg("-j").arg(max_threads.to_string())  // 限制线程数
        .output();
    
    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);
            
            // Validate output
            if let Err(e) = verify_jxl_health(&output) {
                 let _ = fs::remove_file(&output);
                 return Err(e);
            }

            // Copy metadata and timestamps
            copy_metadata(input, &output);
            
            mark_as_processed(input);
            
            if options.should_delete_original() {
                fs::remove_file(input)?;
            }
            
            // 🔥 修复：正确显示 size reduction/increase 消息
            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("JPEG lossless transcode successful: size reduced {:.1}%", reduction_pct)
            } else {
                format!("JPEG lossless transcode successful: size increased {:.1}%", -reduction_pct)
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
            Err(ImgQualityError::ConversionError(format!("cjxl JPEG transcode failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("cjxl not found: {}", e)))
        }
    }
}

/// Convert static lossy image to AVIF
pub fn convert_to_avif(input: &Path, quality: Option<u8>, options: &ConvertOptions) -> Result<ConversionResult> {
    // Anti-duplicate check
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
    let output = get_output_path(input, "avif", &options.output_dir)?;
    
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
    
    // Use original quality or default to high quality
    let q = quality.unwrap_or(85);
    
    let result = Command::new("avifenc")
        .arg("-s").arg("4")       // Speed 4 (balanced)
        .arg("-j").arg("all")     // Use all CPU cores
        .arg("-q").arg(q.to_string())
        .arg(input)
        .arg(&output)
        .output();
    
    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            // Copy metadata and timestamps
            copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original() {
                fs::remove_file(input)?;
            }

            // 🔥 修复：正确显示 size reduction/increase 消息
            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("AVIF conversion successful: size reduced {:.1}%", reduction_pct)
            } else {
                format!("AVIF conversion successful: size increased {:.1}%", -reduction_pct)
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
            Err(ImgQualityError::ConversionError(format!("avifenc failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("avifenc not found: {}", e)))
        }
    }
}

/// Convert animated lossless to HEVC MP4 (CRF 0 visually lossless, 与 AV1 CRF 0 对应)
pub fn convert_to_hevc_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    // Anti-duplicate check
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
    let output = get_output_path(input, "mp4", &options.output_dir)?;
    
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
    
    // 🔥 健壮性：获取输入尺寸并生成视频滤镜链
    // 解决 "Picture height must be an integer multiple of the specified chroma subsampling" 错误
    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, false);
    
    // HEVC with CRF 0 for visually lossless (与 AV1 CRF 0 对应)
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let x265_params = format!("log-level=error:pools={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")  // Overwrite
        .arg("-threads").arg(max_threads.to_string())  // 限制线程数
        .arg("-i").arg(input)
        .arg("-c:v").arg("libx265")
        .arg("-crf").arg("0")    // Visually lossless (与 AV1 CRF 0 对应)
        .arg("-preset").arg("medium")
        .arg("-tag:v").arg("hvc1")  // Apple 兼容性
        .arg("-x265-params").arg(&x265_params);
    
    // 添加视频滤镜（尺寸修正 + 像素格式）
    for arg in &vf_args {
        cmd.arg(arg);
    }
    
    cmd.arg(&output);
    let result = cmd.output();
    
    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);
            
            // Copy metadata and timestamps
            copy_metadata(input, &output);
            
            mark_as_processed(input);
            
            if options.should_delete_original() {
                fs::remove_file(input)?;
            }
            
            // 🔥 修复：正确显示 size reduction/increase 消息
            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("HEVC conversion successful: size reduced {:.1}%", reduction_pct)
            } else {
                format!("HEVC conversion successful: size increased {:.1}%", -reduction_pct)
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
            Err(ImgQualityError::ConversionError(format!("ffmpeg failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("ffmpeg not found: {}", e)))
        }
    }
}

/// Convert image to AVIF using mathematical lossless (⚠️ VERY SLOW)
pub fn convert_to_avif_lossless(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
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
    let output = get_output_path(input, "avif", &options.output_dir)?;
    
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
    
    // Mathematical lossless AVIF
    let result = Command::new("avifenc")
        .arg("--lossless")  // Mathematical lossless
        .arg("-s").arg("4")
        .arg("-j").arg("all")
        .arg(input)
        .arg(&output)
        .output();
    
    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);
            
            // Copy metadata and timestamps
            copy_metadata(input, &output);
            
            mark_as_processed(input);
            
            if options.should_delete_original() {
                fs::remove_file(input)?;
            }
            
            // 🔥 修复：正确显示 size reduction/increase 消息
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
            Err(ImgQualityError::ConversionError(format!("avifenc lossless failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("avifenc not found: {}", e)))
        }
    }
}

/// Convert animated to HEVC MP4 with quality-matched CRF
/// 
/// This function calculates an appropriate CRF based on the input file's
/// characteristics to match the input quality level.
pub fn convert_to_hevc_mp4_matched(
    input: &Path, 
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    // Anti-duplicate check
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
    let output = get_output_path(input, "mp4", &options.output_dir)?;
    
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
    
    // Calculate matched CRF based on input characteristics (HEVC CRF range: 0-32)
    let crf = calculate_matched_crf_for_animation_hevc(analysis, input_size);
    eprintln!("   🎯 Matched HEVC CRF: {} (based on input quality analysis)", crf);
    
    // 🔥 健壮性：获取输入尺寸并生成视频滤镜链
    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, analysis.has_alpha);
    
    // HEVC with calculated CRF
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let x265_params = format!("log-level=error:pools={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")  // Overwrite
        .arg("-threads").arg(max_threads.to_string())  // 限制线程数
        .arg("-i").arg(input)
        .arg("-c:v").arg("libx265")
        .arg("-crf").arg(crf.to_string())
        .arg("-preset").arg("medium")
        .arg("-tag:v").arg("hvc1")  // Apple 兼容性
        .arg("-x265-params").arg(&x265_params);
    
    // 添加视频滤镜（尺寸修正 + 像素格式）
    for arg in &vf_args {
        cmd.arg(arg);
    }
    
    cmd.arg(&output);
    let result = cmd.output();
    
    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);
            
            // Copy metadata and timestamps
            copy_metadata(input, &output);
            
            mark_as_processed(input);
            
            if options.should_delete_original() {
                fs::remove_file(input)?;
            }
            
            // 🔥 修复：正确显示 size reduction/increase 消息
            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("Quality-matched HEVC (CRF {}): size reduced {:.1}%", crf, reduction_pct)
            } else {
                format!("Quality-matched HEVC (CRF {}): size increased {:.1}%", crf, -reduction_pct)
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
            Err(ImgQualityError::ConversionError(format!("ffmpeg failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("ffmpeg not found: {}", e)))
        }
    }
}

/// Calculate CRF to match input animation quality for HEVC (Enhanced Algorithm)
/// 
/// This function uses a more precise algorithm that considers:
/// 1. Bytes per pixel per second (bpps) - primary quality indicator
/// 2. Source format efficiency - GIF vs WebP vs APNG
/// 3. Color depth and palette size
/// 4. Resolution scaling
/// 
/// The formula converts bpps to an equivalent CRF using:
/// CRF ≈ 51 - 10 * log2(bpps * efficiency_factor * 1000)
/// 
/// Clamped to range [18, 32] for HEVC (x265)
fn calculate_matched_crf_for_animation_hevc(analysis: &crate::ImageAnalysis, file_size: u64) -> u8 {
    let pixels = (analysis.width as u64) * (analysis.height as u64);
    
    // 🔥 质量宣言：动画时长未知时使用保守策略
    // 如果无法获取时长，使用保守的 CRF 值（较高质量）
    // 而不是静默使用可能导致质量损失的默认值
    let duration = match analysis.duration_secs {
        Some(d) if d > 0.0 => d as f64,
        Some(_) | None => {
            eprintln!("   ⚠️  动画时长未知，使用保守 CRF 策略（偏向高质量）");
            // 使用保守估计：假设较短时长，这会导致较低的 bpps，从而选择较低的 CRF（更高质量）
            1.0
        }
    }.max(0.1);
    
    // Calculate bytes per pixel per second
    let bytes_per_second = file_size as f64 / duration;
    let bpps = bytes_per_second / pixels as f64;
    
    // Format efficiency factor
    // GIF is very inefficient (256 colors, LZW), WebP is better, APNG is in between
    let format_factor = match analysis.format.to_lowercase().as_str() {
        "gif" => 2.5,      // GIF is very inefficient, high bpps doesn't mean high quality
        "apng" | "png" => 1.5,  // APNG is moderately efficient
        "webp" => 1.0,     // WebP animated is efficient
        _ => 1.2,
    };
    
    // Color depth factor
    // 8-bit (256 colors) animations have inherently lower quality ceiling
    let color_factor = if analysis.color_depth <= 8 {
        1.3  // Limited palette, don't need as high quality
    } else {
        1.0
    };
    
    // Resolution factor (higher res needs more bits for same perceived quality)
    let resolution_factor = if pixels > 2_000_000 {
        0.8   // 1080p+ needs more bits
    } else if pixels > 500_000 {
        0.9   // 720p
    } else {
        1.0   // SD
    };
    
    // Alpha channel factor (alpha adds complexity)
    let alpha_factor = if analysis.has_alpha { 0.9 } else { 1.0 };
    
    // Effective bpps after adjustments
    let effective_bpps = bpps / format_factor * color_factor * resolution_factor * alpha_factor;
    
    // Convert bpps to CRF using logarithmic formula
    // HEVC CRF range is 0-51, with 23 being default "good quality"
    // CRF = 51 - 10 * log2(effective_bpps * 1000)
    let crf_float = if effective_bpps > 0.0 {
        51.0 - 10.0 * (effective_bpps * 1000.0).log2()
    } else {
        23.0
    };
    
    // Clamp to reasonable range [0, 32] for HEVC (与 AV1 CRF 0 对应，允许视觉无损)
    let crf = (crf_float.round() as i32).clamp(0, 32) as u8;
    
    eprintln!("   📊 Quality Analysis:");
    eprintln!("      Raw bpps: {:.4} bytes/pixel/second", bpps);
    eprintln!("      Format: {} (factor: {:.2})", analysis.format, format_factor);
    eprintln!("      Color depth: {}-bit (factor: {:.2})", analysis.color_depth, color_factor);
    eprintln!("      Resolution: {}x{} (factor: {:.2})", analysis.width, analysis.height, resolution_factor);
    eprintln!("      Alpha: {} (factor: {:.2})", analysis.has_alpha, alpha_factor);
    eprintln!("      Effective bpps: {:.4}", effective_bpps);
    eprintln!("      Calculated CRF: {}", crf);
    
    crf
}

/// Calculate JXL distance to match input image quality (for lossy static images)
/// 
/// This function analyzes the input image and calculates an appropriate JXL distance
/// that matches the perceived quality of the original.
/// 
/// JXL distance: 0.0 = lossless, 1.0 = Q90, 2.0 = Q80, etc.
/// Formula: distance ≈ (100 - estimated_quality) / 10
/// 
/// For images without JPEG quality info, we estimate based on:
/// - Compression ratio
/// - File size per pixel
/// - Format efficiency
pub fn calculate_matched_distance_for_static(analysis: &crate::ImageAnalysis, file_size: u64) -> f32 {
    let pixels = (analysis.width as u64) * (analysis.height as u64);
    
    // If we have JPEG quality analysis, use it directly
    if let Some(ref jpeg) = analysis.jpeg_analysis {
        let quality = jpeg.estimated_quality as f32;
        // JXL distance formula: distance = (100 - quality) / 10
        // Q100 → d=0.0, Q90 → d=1.0, Q80 → d=2.0, Q70 → d=3.0
        let distance = (100.0 - quality) / 10.0;
        let clamped = distance.clamp(0.0, 5.0);
        
        eprintln!("   📊 Quality Analysis (JPEG):");
        eprintln!("      JPEG Quality: Q{}", jpeg.estimated_quality);
        eprintln!("      Confidence: {:.1}%", jpeg.confidence * 100.0);
        eprintln!("      Calculated JXL distance: {:.2}", clamped);
        
        return clamped;
    }
    
    // For non-JPEG lossy images, estimate based on bytes per pixel
    let bytes_per_pixel = file_size as f64 / pixels as f64;
    
    // Format efficiency factor
    let format_factor = match analysis.format.to_lowercase().as_str() {
        "webp" => 0.8,      // WebP is efficient
        "avif" | "heic" | "heif" => 0.7,  // AVIF/HEIC are very efficient
        "png" => 1.5,       // PNG is less efficient for photos
        "bmp" | "tiff" => 2.0,  // Uncompressed/lightly compressed
        _ => 1.0,
    };
    
    // Color depth factor
    let depth_factor = match analysis.color_depth {
        8 => 1.0,
        16 => 2.0,
        _ => 1.0,
    };
    
    // Alpha channel factor
    let alpha_factor = if analysis.has_alpha { 1.33 } else { 1.0 };
    
    // Effective bytes per pixel
    let effective_bpp = bytes_per_pixel / format_factor / depth_factor / alpha_factor;
    
    // Estimate quality from effective bpp
    // High bpp (>1.0) suggests high quality, low bpp (<0.3) suggests low quality
    // bpp=2.0 → Q95 → d=0.5
    // bpp=1.0 → Q90 → d=1.0
    // bpp=0.5 → Q85 → d=1.5
    // bpp=0.3 → Q80 → d=2.0
    // bpp=0.1 → Q70 → d=3.0
    let estimated_quality = if effective_bpp > 0.0 {
        // Q = 70 + 15 * log2(effective_bpp * 5)
        70.0 + 15.0 * (effective_bpp * 5.0).log2()
    } else {
        75.0
    };
    
    let clamped_quality = estimated_quality.clamp(50.0, 100.0);
    let distance = ((100.0 - clamped_quality) / 10.0) as f32;
    let clamped_distance = distance.clamp(0.0, 5.0);
    
    eprintln!("   📊 Quality Analysis (Non-JPEG):");
    eprintln!("      Bytes per pixel: {:.4}", bytes_per_pixel);
    eprintln!("      Format: {} (factor: {:.2})", analysis.format, format_factor);
    eprintln!("      Effective bpp: {:.4}", effective_bpp);
    eprintln!("      Estimated quality: Q{:.0}", clamped_quality);
    eprintln!("      Calculated JXL distance: {:.2}", clamped_distance);
    
    clamped_distance
}

/// Convert static lossy image to JXL with quality-matched distance
pub fn convert_to_jxl_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<ConversionResult> {
    // Anti-duplicate check
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
    let output = get_output_path(input, "jxl", &options.output_dir)?;
    
    // Ensure output directory exists
    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }
    
    // Check if output already exists
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
    
    // Calculate matched distance
    let distance = calculate_matched_distance_for_static(analysis, input_size);
    eprintln!("   🎯 Matched JXL distance: {:.2}", distance);
    
    // Execute cjxl with calculated distance
    // Note: For JPEG input with non-zero distance, we need to disable lossless_jpeg
    // Note: cjxl 默认保留 ICC 颜色配置文件，无需额外参数
    // 🔥 性能优化：限制 cjxl 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let mut cmd = Command::new("cjxl");
    cmd.arg(input)
        .arg(&output)
        .arg("-d").arg(format!("{:.2}", distance))
        .arg("-e").arg("7")    // Effort 7 (cjxl v0.11+ 范围是 1-10，默认 7)
        .arg("-j").arg(max_threads.to_string());  // 限制线程数
    
    // If distance > 0, disable lossless_jpeg (which is enabled by default for JPEG input)
    if distance > 0.0 {
        cmd.arg("--lossless_jpeg=0");
    }
    
    let result = cmd.output();
    
    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);
            
            // 🔥 智能回退：如果转换后文件变大，删除输出并跳过
            if output_size > input_size {
                let _ = fs::remove_file(&output);
                eprintln!("   ⏭️  Rollback: JXL larger than original ({} → {} bytes, +{:.1}%)", 
                    input_size, output_size, (output_size as f64 / input_size as f64 - 1.0) * 100.0);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: format!("Skipped: JXL would be larger (+{:.1}%)", (output_size as f64 / input_size as f64 - 1.0) * 100.0),
                    skipped: true,
                    skip_reason: Some("size_increase".to_string()),
                });
            }
            
            // Validate output
            if let Err(e) = verify_jxl_health(&output) {
                let _ = fs::remove_file(&output);
                return Err(e);
            }

            // Copy metadata and timestamps
            copy_metadata(input, &output);
            
            mark_as_processed(input);
            
            if options.should_delete_original() {
                fs::remove_file(input)?;
            }
            
            // 🔥 修复：正确显示 size reduction/increase 消息
            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("Quality-matched JXL (d={:.2}): size reduced {:.1}%", distance, reduction_pct)
            } else {
                format!("Quality-matched JXL (d={:.2}): size increased {:.1}%", distance, -reduction_pct)
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
            Err(ImgQualityError::ConversionError(format!("cjxl failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("cjxl not found: {}", e)))
        }
    }
}

/// Convert animated to HEVC MKV using mathematical lossless (⚠️ SLOW, huge files)
pub fn convert_to_hevc_mkv_lossless(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    eprintln!("⚠️  Mathematical lossless HEVC encoding - this will be SLOW and produce large files!");
    
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
    let output = get_output_path(input, "mkv", &options.output_dir)?;  // MKV for lossless
    
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
    
    // 🔥 健壮性：获取输入尺寸并生成视频滤镜链
    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, false);
    
    // Mathematical lossless HEVC
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let x265_params = format!("lossless=1:log-level=error:pools={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads").arg(max_threads.to_string())  // 限制线程数
        .arg("-i").arg(input)
        .arg("-c:v").arg("libx265")
        .arg("-x265-params").arg(&x265_params)  // lossless=1 for mathematical lossless
        .arg("-preset").arg("medium")
        .arg("-tag:v").arg("hvc1");
    
    // 添加视频滤镜（尺寸修正 + 像素格式）
    for arg in &vf_args {
        cmd.arg(arg);
    }
    
    cmd.arg(&output);
    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            // Copy metadata and timestamps
            copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original() {
                fs::remove_file(input)?;
            }

            // 🔥 修复：正确显示 size reduction/increase 消息
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
            Err(ImgQualityError::ConversionError(format!("ffmpeg lossless failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("ffmpeg not found: {}", e)))
        }
    }
}

// MacOS specialized timestamp setter (creation time + date added)


// Helper to copy metadata and timestamps from source to destination
// Maximum metadata preservation: centralized via shared_utils::metadata
fn copy_metadata(src: &Path, dst: &Path) {
    // shared_utils::preserve_metadata handles ALL layers:
    // 1. Internal (Exif/IPTC via ExifTool)
    // 2. Network (WhereFroms check)
    // 3. System (ACL, Flags, Xattr, Timestamps via copyfile)
    if let Err(e) = shared_utils::preserve_metadata(src, dst) {
        eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }
}


/// Wrapper for shared_utils::determine_output_path with imgquality error type
fn get_output_path(input: &Path, extension: &str, output_dir: &Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    shared_utils::conversion::determine_output_path(input, extension, output_dir)
        .map_err(|e| ImgQualityError::ConversionError(e))
}

/// 获取输入文件的尺寸（宽度和高度）
/// 
/// 使用 ffprobe 获取视频/动画的尺寸，或使用 image crate 获取静态图片的尺寸
/// 
/// 🔥 遵循质量宣言：失败就响亮报错，绝不静默降级！
fn get_input_dimensions(input: &Path) -> Result<(u32, u32)> {
    // 首先尝试使用 ffprobe（适用于视频和动画）
    if let Ok(probe) = shared_utils::probe_video(input) {
        if probe.width > 0 && probe.height > 0 {
            return Ok((probe.width, probe.height));
        }
    }
    
    // 回退到 image crate（适用于静态图片）
    match image::image_dimensions(input) {
        Ok((w, h)) => Ok((w, h)),
        Err(e) => {
            // 🔥 响亮报错！绝不静默降级！
            Err(ImgQualityError::ConversionError(format!(
                "❌ 无法获取文件尺寸: {}\n\
                 错误: {}\n\
                 💡 可能原因:\n\
                 - 文件损坏或格式不支持\n\
                 - ffprobe 未安装或不可用\n\
                 - 文件不是有效的图像/视频格式\n\
                 请检查文件完整性或安装 ffprobe: brew install ffmpeg",
                input.display(), e
            )))
        }
    }
}

/// Verify that JXL file is valid using signature and jxlinfo (if available)
fn verify_jxl_health(path: &Path) -> Result<()> {
    // Check file signature
    let mut file = fs::File::open(path)?;
    let mut sig = [0u8; 2];
    use std::io::Read;
    file.read_exact(&mut sig)?;

    // JXL signature: 0xFF 0x0A (bare JXL) or 0x00 0x00 (ISOBMFF container)
    if sig != [0xFF, 0x0A] && sig != [0x00, 0x00] {
        return Err(ImgQualityError::ConversionError(
            "Invalid JXL file signature".to_string(),
        ));
    }
    
    // 🔥 使用 jxlinfo 进行更可靠的验证（如果可用）
    // jxlinfo 比 djxl 更适合验证，因为它只读取元数据，不需要完整解码
    if which::which("jxlinfo").is_ok() {
        let result = Command::new("jxlinfo")
            .arg(path)
            .output();

        if let Ok(output) = result {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ImgQualityError::ConversionError(
                    format!("JXL health check failed (jxlinfo): {}", stderr.trim()),
                ));
            }
        }
    }
    // 如果 jxlinfo 不可用，签名检查已经足够（cjxl 输出通常是有效的）
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    
    #[test]
    fn test_get_output_path() {
        let input = Path::new("/path/to/image.png");
        let output = get_output_path(input, "jxl", &None).unwrap();
        assert_eq!(output, Path::new("/path/to/image.jxl"));
    }
    
    #[test]
    fn test_get_output_path_with_dir() {
        let input = Path::new("/path/to/image.png");
        let output_dir = Some(PathBuf::from("/output"));
        let output = get_output_path(input, "avif", &output_dir).unwrap();
        assert_eq!(output, Path::new("/output/image.avif"));
    }
    
    #[test]
    fn test_get_output_path_same_file_error() {
        // 测试输入输出相同时应该报错
        let input = Path::new("/path/to/image.jxl");
        let result = get_output_path(input, "jxl", &None);
        assert!(result.is_err());
    }
}
