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
    
    // 🔥 预处理：检测 cjxl 不能直接读取的格式，先转换为中间格式
    let (actual_input, temp_file) = prepare_input_for_cjxl(input)?;
    
    // Execute cjxl (v0.11+ syntax)
    // Note: cjxl 默认保留 ICC 颜色配置文件，无需额外参数
    // 🔥 性能优化：限制 cjxl 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let result = Command::new("cjxl")
        .arg(&actual_input)
        .arg(&output)
        .arg("-d").arg(format!("{:.1}", distance))  // Distance parameter
        .arg("-e").arg("7")    // Effort 7 (cjxl v0.11+ 范围是 1-10，默认 7)
        .arg("-j").arg(max_threads.to_string())  // 限制线程数
        .output();
    
    // 清理临时文件
    if let Some(ref temp) = temp_file {
        let _ = fs::remove_file(temp);
    }
    
    // 🔥 Fallback: 如果 cjxl 失败且报告 "Getting pixel data failed"，使用 ImageMagick 重新编码后再试
    let result = match &result {
        Ok(output_cmd) if !output_cmd.status.success() => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            if stderr.contains("Getting pixel data failed") || stderr.contains("Failed to decode") {
                eprintln!("   ⚠️  CJXL DECODE FAILED: {}", stderr.trim());
                eprintln!("   🔧 FALLBACK: Using ImageMagick to re-encode PNG for compatibility");
                eprintln!("   📋 Reason: PNG contains metadata/encoding that cjxl cannot handle");
                
                // 使用 ImageMagick 重新编码为 PNG
                let fallback_png = std::env::temp_dir().join(format!(
                    "mfb_fallback_{}_{}.png",
                    std::process::id(),
                    input.file_stem().unwrap_or_default().to_string_lossy()
                ));
                
                eprintln!("   🔄 Re-encoding with ImageMagick: {} → temp PNG", 
                    input.file_name().unwrap_or_default().to_string_lossy());
                
                let magick_result = Command::new("magick")
                    .arg(input)
                    .arg("-depth").arg("16")  // 保留位深
                    .arg(&fallback_png)
                    .output();
                
                if let Ok(magick_out) = magick_result {
                    if magick_out.status.success() && fallback_png.exists() {
                        eprintln!("   ✅ ImageMagick re-encode successful, retrying cjxl...");
                        
                        // 使用重新编码的 PNG 再次调用 cjxl
                        let retry_result = Command::new("cjxl")
                            .arg(&fallback_png)
                            .arg(&output)
                            .arg("-d").arg(format!("{:.1}", distance))
                            .arg("-e").arg("7")
                            .arg("-j").arg(max_threads.to_string())
                            .output();
                        
                        let _ = fs::remove_file(&fallback_png);
                        
                        // 检查重试结果
                        match &retry_result {
                            Ok(retry_out) if retry_out.status.success() => {
                                eprintln!("   🎉 FALLBACK SUCCESS: cjxl conversion completed via ImageMagick");
                            }
                            Ok(retry_out) => {
                                eprintln!("   ❌ FALLBACK FAILED: cjxl still failed after re-encode");
                                eprintln!("   📝 Error: {}", String::from_utf8_lossy(&retry_out.stderr));
                            }
                            Err(e) => {
                                eprintln!("   ❌ FALLBACK ERROR: {}", e);
                            }
                        }
                        
                        retry_result
                    } else {
                        eprintln!("   ❌ ImageMagick re-encode failed, using original error");
                        let _ = fs::remove_file(&fallback_png);
                        result
                    }
                } else {
                    eprintln!("   ❌ ImageMagick not available, using original error");
                    result
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
            
            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
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
            
            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
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

            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
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

/// Convert animated lossless to AV1 MP4 (Q=100 visual lossless)
pub fn convert_to_av1_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
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
    
    // AV1 with CRF 0 for visually lossless (使用 SVT-AV1 编码器)
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let svt_params = format!("tune=0:film-grain=0:lp={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")  // Overwrite
        .arg("-threads").arg(max_threads.to_string())  // 限制线程数
        .arg("-i").arg(input)
        .arg("-c:v").arg("libsvtav1")  // 🔥 使用 SVT-AV1 (比 libaom-av1 快 10-20 倍)
        .arg("-crf").arg("0")    // CRF 0 = 视觉无损最高质量
        .arg("-preset").arg("6")  // 0-13, 6 是平衡点
        .arg("-svtav1-params").arg(&svt_params);  // 限制 SVT-AV1 线程数
    
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
            
            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
            }
            
            // 🔥 修复：正确显示 size reduction/increase 消息
            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("AV1 conversion successful: size reduced {:.1}%", reduction_pct)
            } else {
                format!("AV1 conversion successful: size increased {:.1}%", -reduction_pct)
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
            
            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
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

/// Convert animated to AV1 MP4 with quality-matched CRF
/// 
/// This function calculates an appropriate CRF based on the input file's
/// characteristics to match the input quality level.
pub fn convert_to_av1_mp4_matched(
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
    
    // Calculate matched CRF based on input characteristics
    // For animated images, we estimate quality based on:
    // - File size per frame
    // - Resolution
    // - Duration
    let crf = calculate_matched_crf_for_animation(analysis, input_size);
    eprintln!("   🎯 Matched CRF: {} (based on input quality analysis)", crf);
    
    // 🔥 健壮性：获取输入尺寸并生成视频滤镜链
    // 解决 "Picture height must be an integer multiple of the specified chroma subsampling" 错误
    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, analysis.has_alpha);
    
    // AV1 with calculated CRF (使用 SVT-AV1 编码器)
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let svt_params = format!("tune=0:film-grain=0:lp={}", max_threads);
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")  // Overwrite
        .arg("-threads").arg(max_threads.to_string())  // 限制线程数
        .arg("-i").arg(input)
        .arg("-c:v").arg("libsvtav1")  // 🔥 使用 SVT-AV1 (比 libaom-av1 快 10-20 倍)
        .arg("-crf").arg(crf.to_string())
        .arg("-preset").arg("6")  // 0-13, 6 是平衡点
        .arg("-svtav1-params").arg(&svt_params);  // 限制 SVT-AV1 线程数
    
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
            
            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
            }
            
            // 🔥 修复：正确显示 size reduction/increase 消息
            let reduction_pct = reduction * 100.0;
            let message = if reduction >= 0.0 {
                format!("Quality-matched AV1 (CRF {}): size reduced {:.1}%", crf, reduction_pct)
            } else {
                format!("Quality-matched AV1 (CRF {}): size increased {:.1}%", crf, -reduction_pct)
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

/// Calculate CRF to match input animation quality (Enhanced Algorithm)
/// Calculate CRF to match input animation quality for AV1 (Enhanced Algorithm)
/// 
/// Uses the unified quality_matcher module from shared_utils for consistent
/// quality matching across all tools.
/// 
/// AV1 CRF range is 0-63, with 23 being default "good quality"
/// Clamped to range [18, 35] for practical use
/// 
/// v3.4: Returns f32 for sub-integer precision (0.5 step)
fn calculate_matched_crf_for_animation(analysis: &crate::ImageAnalysis, file_size: u64) -> f32 {
    // 🔥 使用统一的 quality_matcher 模块
    // Note: ImageAnalysis doesn't have fps field, will be estimated from duration
    let quality_analysis = shared_utils::from_image_analysis(
        &analysis.format,
        analysis.width,
        analysis.height,
        analysis.color_depth,
        analysis.has_alpha,
        file_size,
        analysis.duration_secs.map(|d| d as f64),
        None, // fps not available in ImageAnalysis
        None, // No estimated quality for animations
    );
    
    match shared_utils::calculate_av1_crf(&quality_analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(&quality_analysis, &result, shared_utils::EncoderType::Av1);
            result.crf // 🔥 v3.4: Already f32 from quality_matcher
        }
        Err(e) => {
            // 🔥 Quality Manifesto: 失败时响亮报错，使用保守值
            eprintln!("   ⚠️  Quality analysis failed: {}", e);
            eprintln!("   ⚠️  Using conservative CRF 23.0 (high quality)");
            23.0
        }
    }
}

/// Calculate JXL distance to match input image quality (for lossy static images)
/// 
/// This function analyzes the input image and calculates an appropriate JXL distance
/// that matches the perceived quality of the original.
/// 
/// JXL distance: 0.0 = lossless, 1.0 = Q90, 2.0 = Q80, etc.
/// Formula: distance ≈ (100 - estimated_quality) / 10
/// Calculate JXL distance to match input image quality (for lossy static images)
/// 
/// Uses the unified quality_matcher module from shared_utils for consistent
/// quality matching across all tools.
/// 
/// JXL distance: 0.0 = lossless, 1.0 = Q90, 2.0 = Q80, etc.
/// Clamped to range [0.0, 5.0] for practical use
pub fn calculate_matched_distance_for_static(analysis: &crate::ImageAnalysis, file_size: u64) -> f32 {
    // 🔥 使用统一的 quality_matcher 模块
    let estimated_quality = analysis.jpeg_analysis.as_ref().map(|j| j.estimated_quality);
    
    let quality_analysis = shared_utils::from_image_analysis(
        &analysis.format,
        analysis.width,
        analysis.height,
        analysis.color_depth,
        analysis.has_alpha,
        file_size,
        None, // Static image, no duration
        None, // Static image, no fps
        estimated_quality,
    );
    
    match shared_utils::calculate_jxl_distance(&quality_analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(&quality_analysis, &result, shared_utils::EncoderType::Jxl);
            result.distance
        }
        Err(e) => {
            // 🔥 Quality Manifesto: 失败时响亮报错，使用保守值
            eprintln!("   ⚠️  Quality analysis failed: {}", e);
            eprintln!("   ⚠️  Using conservative distance 1.0 (Q90 equivalent)");
            1.0
        }
    }
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
            
            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
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

/// Convert animated to AV1 MP4 using mathematical lossless (⚠️ VERY SLOW)
pub fn convert_to_av1_mp4_lossless(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
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
    
    // Mathematical lossless AV1 (使用 SVT-AV1 编码器)
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let svt_params = format!("lossless=1:lp={}", max_threads);  // 数学无损 + 限制线程数
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-threads").arg(max_threads.to_string())  // 限制线程数
        .arg("-i").arg(input)
        .arg("-c:v").arg("libsvtav1")  // 🔥 使用 SVT-AV1 (比 libaom-av1 快 10-20 倍)
        .arg("-crf").arg("0")
        .arg("-preset").arg("4")  // 无损模式用更慢的 preset 保证质量
        .arg("-svtav1-params").arg(&svt_params);  // 数学无损
    
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

            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
            }

            // 🔥 修复：正确显示 size reduction/increase 消息
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

// ============================================================
// 🔧 cjxl 输入预处理
// ============================================================

/// 检测并预处理 cjxl 不能直接读取的格式
/// 
/// cjxl 已知问题：
/// - 某些带 ICC profile 的 WebP 文件会报 "Getting pixel data failed"
/// - 某些 TIFF 格式不支持
/// - 某些 BMP 格式不支持
/// 
/// 返回: (实际输入路径, 临时文件路径 Option)
fn prepare_input_for_cjxl(input: &Path) -> Result<(std::path::PathBuf, Option<std::path::PathBuf>)> {
    let ext = input.extension()
        .map(|e| e.to_ascii_lowercase())
        .and_then(|e| e.to_str().map(|s| s.to_string()))
        .unwrap_or_default();
    
    match ext.as_str() {
        // WebP: 使用 dwebp 解码（处理 ICC profile 问题）
        "webp" => {
            eprintln!("   🔧 PRE-PROCESSING: WebP detected, using dwebp for ICC profile compatibility");
            
            let temp_png = std::env::temp_dir().join(format!(
                "mfb_cjxl_{}_{}.png",
                std::process::id(),
                input.file_stem().unwrap_or_default().to_string_lossy()
            ));
            
            let result = Command::new("dwebp")
                .arg(input)
                .arg("-o")
                .arg(&temp_png)
                .output();
            
            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!("   ✅ dwebp pre-processing successful");
                    Ok((temp_png.clone(), Some(temp_png)))
                }
                _ => {
                    eprintln!("   ⚠️  dwebp pre-processing failed, trying direct cjxl");
                    let _ = fs::remove_file(&temp_png);
                    Ok((input.to_path_buf(), None))
                }
            }
        }
        
        // TIFF: 使用 ImageMagick 转换
        "tiff" | "tif" => {
            eprintln!("   🔧 PRE-PROCESSING: TIFF detected, using ImageMagick for cjxl compatibility");
            
            let temp_png = std::env::temp_dir().join(format!(
                "mfb_cjxl_{}_{}.png",
                std::process::id(),
                input.file_stem().unwrap_or_default().to_string_lossy()
            ));
            
            let result = Command::new("magick")
                .arg(input)
                .arg("-depth").arg("16")  // 保留位深
                .arg(&temp_png)
                .output();
            
            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!("   ✅ ImageMagick TIFF pre-processing successful");
                    Ok((temp_png.clone(), Some(temp_png)))
                }
                _ => {
                    eprintln!("   ⚠️  ImageMagick TIFF pre-processing failed, trying direct cjxl");
                    let _ = fs::remove_file(&temp_png);
                    Ok((input.to_path_buf(), None))
                }
            }
        }
        
        // BMP: 使用 ImageMagick 转换
        "bmp" => {
            eprintln!("   🔧 PRE-PROCESSING: BMP detected, using ImageMagick for cjxl compatibility");
            
            let temp_png = std::env::temp_dir().join(format!(
                "mfb_cjxl_{}_{}.png",
                std::process::id(),
                input.file_stem().unwrap_or_default().to_string_lossy()
            ));
            
            let result = Command::new("magick")
                .arg(input)
                .arg(&temp_png)
                .output();
            
            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!("   ✅ ImageMagick BMP pre-processing successful");
                    Ok((temp_png.clone(), Some(temp_png)))
                }
                _ => {
                    eprintln!("   ⚠️  ImageMagick BMP pre-processing failed, trying direct cjxl");
                    let _ = fs::remove_file(&temp_png);
                    Ok((input.to_path_buf(), None))
                }
            }
        }
        
        // HEIC/HEIF: 使用 ImageMagick 或 sips 转换
        "heic" | "heif" => {
            eprintln!("   🔧 PRE-PROCESSING: HEIC/HEIF detected, using sips/ImageMagick for cjxl compatibility");
            
            let temp_png = std::env::temp_dir().join(format!(
                "mfb_cjxl_{}_{}.png",
                std::process::id(),
                input.file_stem().unwrap_or_default().to_string_lossy()
            ));
            
            // 优先使用 sips (macOS 原生)
            eprintln!("   🍎 Trying macOS sips first...");
            let result = Command::new("sips")
                .arg("-s").arg("format").arg("png")
                .arg(input)
                .arg("--out").arg(&temp_png)
                .output();
            
            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!("   ✅ sips HEIC pre-processing successful");
                    Ok((temp_png.clone(), Some(temp_png)))
                }
                _ => {
                    eprintln!("   ⚠️  sips failed, trying ImageMagick...");
                    // 尝试 ImageMagick
                    let result = Command::new("magick")
                        .arg(input)
                        .arg(&temp_png)
                        .output();
                    
                    match result {
                        Ok(output) if output.status.success() && temp_png.exists() => {
                            eprintln!("   ✅ ImageMagick HEIC pre-processing successful");
                            Ok((temp_png.clone(), Some(temp_png)))
                        }
                        _ => {
                            eprintln!("   ⚠️  Both sips and ImageMagick failed, trying direct cjxl");
                            let _ = fs::remove_file(&temp_png);
                            Ok((input.to_path_buf(), None))
                        }
                    }
                }
            }
        }
        
        // 其他格式：直接使用
        _ => Ok((input.to_path_buf(), None)),
    }
}

/// Wrapper for shared_utils::determine_output_path with imgquality error type
fn get_output_path(input: &Path, extension: &str, output_dir: &Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    shared_utils::conversion::determine_output_path(input, extension, output_dir)
        .map_err(ImgQualityError::ConversionError)
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
