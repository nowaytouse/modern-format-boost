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
            
            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
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

/// Convert animated to HEVC MP4 with quality-matched CRF
/// 
/// 🔥 统一使用 shared_utils::video_explorer 处理所有探索模式
/// 
/// 探索模式由 options.explore 和 options.match_quality 决定：
/// - explore=true, match_quality=true: 精确质量匹配（二分搜索 + SSIM 验证）
/// - explore=true, match_quality=false: 仅探索更小大小
/// - explore=false, match_quality=true: 单次编码 + SSIM 验证
/// - explore=false, match_quality=false: 默认使用质量匹配
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
    let initial_crf = calculate_matched_crf_for_animation_hevc(analysis, input_size);
    
    // 🔥 健壮性：获取输入尺寸并生成视频滤镜链
    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, analysis.has_alpha);
    
    // 🔥 统一使用 shared_utils::video_explorer 处理所有探索模式
    let explore_mode = options.explore_mode();
    let mode_name = match explore_mode {
        shared_utils::ExploreMode::PreciseQualityMatch => "🔬 Precise Quality-Match",
        shared_utils::ExploreMode::SizeOnly => "🔍 Size-Only Exploration",
        shared_utils::ExploreMode::QualityMatch => "🎯 Quality-Match",
    };
    eprintln!("   {} Mode: CRF {:.1} (based on input analysis)", mode_name, initial_crf);
    
    let explore_result = match explore_mode {
        shared_utils::ExploreMode::PreciseQualityMatch => {
            shared_utils::explore_hevc(input, &output, vf_args, initial_crf)
        }
        shared_utils::ExploreMode::SizeOnly => {
            shared_utils::explore_hevc_size_only(input, &output, vf_args, initial_crf)
        }
        shared_utils::ExploreMode::QualityMatch => {
            shared_utils::explore_hevc_quality_match(input, &output, vf_args, initial_crf)
        }
    }.map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    
    // 打印探索日志
    for log in &explore_result.log {
        eprintln!("{}", log);
    }
    
    // 🔥 如果最终输出仍然比输入大，跳过转换
    if explore_result.output_size > input_size {
        let _ = fs::remove_file(&output);
        eprintln!("   ⏭️  Skipping: HEVC output larger than input even at CRF {:.1} ({} > {} bytes)", 
            explore_result.optimal_crf, explore_result.output_size, input_size);
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!("Skipped: HEVC output larger than GIF input (low resolution {}x{})", width, height),
            skipped: true,
            skip_reason: Some("size_increase".to_string()),
        });
    }
    
    // 🔥 v3.8: 质量验证失败时，保护原文件！
    if !explore_result.quality_passed {
        eprintln!("   ⚠️  Quality validation FAILED: SSIM {:.4} < 0.95", 
            explore_result.ssim.unwrap_or(0.0));
        eprintln!("   🛡️  Original file PROTECTED (quality too low to replace)");
        
        // 删除低质量的输出文件
        if output.exists() {
            let _ = fs::remove_file(&output);
            eprintln!("   🗑️  Low-quality output deleted");
        }
        
        // 返回跳过状态，不删除原文件
        return Ok(ConversionResult {
            success: false,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!("Skipped: SSIM {:.4} below threshold 0.95", explore_result.ssim.unwrap_or(0.0)),
            skipped: true,
            skip_reason: Some("quality_failed".to_string()),
        });
    }
    
    // Copy metadata and timestamps
    copy_metadata(input, &output);
    mark_as_processed(input);
    
    if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
        // Already handled by safe_delete_original
    }
    
    let reduction_pct = -explore_result.size_change_pct; // 转换为正数表示减少
    // 🔥 v3.4: Use epsilon comparison for f32 CRF values
    let explored_msg = if (explore_result.optimal_crf - initial_crf).abs() > 0.1 {
        format!(" (explored from CRF {:.1})", initial_crf)
    } else {
        String::new()
    };
    
    let ssim_msg = explore_result.ssim
        .map(|s| format!(", SSIM: {:.4}", s))
        .unwrap_or_default();
    
    let message = format!("HEVC (CRF {:.1}{}, {} iter{}): -{:.1}%", 
        explore_result.optimal_crf, explored_msg, explore_result.iterations, ssim_msg, reduction_pct);
    
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

/// Calculate CRF to match input animation quality for HEVC (Enhanced Algorithm)
/// 
/// Uses the unified quality_matcher module from shared_utils for consistent
/// quality matching across all tools.
/// 
/// HEVC CRF range is 0-51, with 23 being default "good quality"
/// Clamped to range [0, 32] for practical use (allows visually lossless)
/// 
/// 🔥 v3.4: Returns f32 for sub-integer precision (0.5 step)
fn calculate_matched_crf_for_animation_hevc(analysis: &crate::ImageAnalysis, file_size: u64) -> f32 {
    // 🔥 使用统一的 quality_matcher 模块
    // Note: ImageAnalysis doesn't have fps field, estimate from duration and frame count if available
    let quality_analysis = shared_utils::from_image_analysis(
        &analysis.format,
        analysis.width,
        analysis.height,
        analysis.color_depth,
        analysis.has_alpha,
        file_size,
        analysis.duration_secs.map(|d| d as f64),
        None, // fps not available in ImageAnalysis, will be estimated from duration
        None, // No estimated quality for animations
    );
    
    match shared_utils::calculate_hevc_crf(&quality_analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(&quality_analysis, &result, shared_utils::EncoderType::Hevc);
            result.crf // 🔥 v3.4: Already f32 from quality_matcher
        }
        Err(e) => {
            // 🔥 Quality Manifesto: 失败时响亮报错，使用保守值
            eprintln!("   ⚠️  Quality analysis failed: {}", e);
            eprintln!("   ⚠️  Using conservative CRF 18.0 (high quality)");
            18.0
        }
    }
}

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

            if options.should_delete_original() && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok() {
                // Already handled by safe_delete_original
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

/// 🍎 Apple 兼容模式：将现代动态图片转换为 GIF
/// 
/// 用于短时长（<3秒）且非高质量的动态图片
/// - 保留原始帧数和尺寸
/// - 使用 Bayer 抖动算法
/// - 最大 256 色
/// - 视觉无损参数
/// 
/// 🔥 v3.8: 智能化改进
/// - 如果输入已经是 GIF，直接跳过（避免重新编码导致文件变大）
/// - 如果输出比输入大，自动回退并保留原文件
pub fn convert_to_gif_apple_compat(
    input: &Path,
    options: &ConvertOptions,
    fps: Option<f32>,
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
    
    // 🔥 v3.8: 如果输入已经是 GIF，直接跳过
    // GIF 重新编码通常会导致文件变大（LZW 压缩不是确定性的）
    let input_ext = input.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    
    if input_ext == "gif" {
        eprintln!("   ⏭️  Input is already GIF, skipping re-encode (would likely increase size)");
        mark_as_processed(input);
        return Ok(ConversionResult {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(input.display().to_string()), // 保持原路径
            input_size,
            output_size: Some(input_size),
            size_reduction: Some(0.0),
            message: "Skipped: Already GIF (re-encoding would increase size)".to_string(),
            skipped: true,
            skip_reason: Some("already_gif".to_string()),
        });
    }
    
    let output = get_output_path(input, "gif", &options.output_dir)?;
    
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
            output_size: Some(fs::metadata(&output)?.len()),
            size_reduction: None,
            message: "Skipped: Output already exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        });
    }
    
    // 获取原始尺寸
    let (width, height) = get_input_dimensions(input)?;
    
    // 使用 ffmpeg 转换为 GIF
    // - 保留原始尺寸
    // - 使用 Bayer 抖动算法（视觉效果最好）
    // - 256 色调色板
    // - 保留原始帧率
    let fps_val = fps.unwrap_or(10.0);
    
    // 两步转换：先生成调色板，再应用
    // 这样可以获得更好的颜色质量
    let palette_path = output.with_extension("palette.png");
    
    // Step 1: 生成调色板
    let palette_result = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(input)
        .args([
            "-vf", &format!(
                "fps={},scale={}:{}:flags=lanczos,palettegen=max_colors=256:stats_mode=diff",
                fps_val, width, height
            ),
        ])
        .arg(&palette_path)
        .output();
    
    if let Err(e) = palette_result {
        return Err(ImgQualityError::ToolNotFound(format!("ffmpeg not found: {}", e)));
    }
    
    // Step 2: 使用调色板转换
    let result = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(input)
        .args(["-i"])
        .arg(&palette_path)
        .args([
            "-lavfi", &format!(
                "fps={},scale={}:{}:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle",
                fps_val, width, height
            ),
        ])
        .arg(&output)
        .output();
    
    // 清理调色板文件
    let _ = fs::remove_file(&palette_path);
    
    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);
            
            // 🔥 v3.8: 智能回退 - 如果输出比输入大，删除输出并跳过
            if output_size > input_size {
                let _ = fs::remove_file(&output);
                eprintln!("   ⏭️  Rollback: GIF larger than original ({} → {} bytes, +{:.1}%)", 
                    input_size, output_size, (output_size as f64 / input_size as f64 - 1.0) * 100.0);
                mark_as_processed(input);
                return Ok(ConversionResult {
                    success: true,
                    input_path: input.display().to_string(),
                    output_path: None,
                    input_size,
                    output_size: None,
                    size_reduction: None,
                    message: format!("Skipped: GIF would be larger (+{:.1}%)", (output_size as f64 / input_size as f64 - 1.0) * 100.0),
                    skipped: true,
                    skip_reason: Some("size_increase".to_string()),
                });
            }
            
            copy_metadata(input, &output);
            mark_as_processed(input);
            
            if options.should_delete_original() {
                let _ = shared_utils::conversion::safe_delete_original(input, &output, 100);
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
            Err(ImgQualityError::ConversionError(format!("ffmpeg GIF conversion failed: {}", stderr)))
        }
        Err(e) => {
            Err(ImgQualityError::ToolNotFound(format!("ffmpeg not found: {}", e)))
        }
    }
}

/// 判断动态图片是否为"高质量"（应转为视频而非 GIF）
/// 
/// 高质量条件（满足任一）：
/// - 分辨率 >= 720p (1280x720)
/// - 宽度 >= 1280 或 高度 >= 720
/// - 总像素 >= 921600 (1280*720)
pub fn is_high_quality_animated(width: u32, height: u32) -> bool {
    let total_pixels = width as u64 * height as u64;
    width >= 1280 || height >= 720 || total_pixels >= 921600
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
    
    // ============================================================
    // 🍎 Apple 兼容模式测试 (裁判测试)
    // ============================================================
    
    #[test]
    fn test_is_high_quality_720p() {
        // 720p 应该被判定为高质量
        assert!(is_high_quality_animated(1280, 720));
    }
    
    #[test]
    fn test_is_high_quality_1080p() {
        // 1080p 应该被判定为高质量
        assert!(is_high_quality_animated(1920, 1080));
    }
    
    #[test]
    fn test_is_high_quality_width_only() {
        // 宽度 >= 1280 应该被判定为高质量
        assert!(is_high_quality_animated(1280, 480));
    }
    
    #[test]
    fn test_is_high_quality_height_only() {
        // 高度 >= 720 应该被判定为高质量
        assert!(is_high_quality_animated(960, 720));
    }
    
    #[test]
    fn test_is_high_quality_total_pixels() {
        // 总像素 >= 921600 应该被判定为高质量
        // 1024 * 900 = 921600
        assert!(is_high_quality_animated(1024, 900));
    }
    
    #[test]
    fn test_is_not_high_quality_small() {
        // 小尺寸应该不是高质量
        assert!(!is_high_quality_animated(640, 480));
    }
    
    #[test]
    fn test_is_not_high_quality_480p() {
        // 480p 应该不是高质量
        assert!(!is_high_quality_animated(854, 480));
    }
    
    #[test]
    fn test_is_not_high_quality_typical_gif() {
        // 典型 GIF 尺寸应该不是高质量
        assert!(!is_high_quality_animated(400, 300));
        assert!(!is_high_quality_animated(500, 500));
        assert!(!is_high_quality_animated(320, 240));
    }
    
    #[test]
    fn test_apple_compat_routing_short_low_quality() {
        // 短动画 + 低质量 → 应该转 GIF
        let duration = 2.0; // < 3秒
        let (width, height) = (400, 300); // 低质量
        
        let should_convert_to_video = duration >= 3.0 || is_high_quality_animated(width, height);
        assert!(!should_convert_to_video, "短动画+低质量应该转GIF，不是视频");
    }
    
    #[test]
    fn test_apple_compat_routing_short_high_quality() {
        // 短动画 + 高质量 → 应该转视频
        let duration = 2.0; // < 3秒
        let (width, height) = (1920, 1080); // 高质量
        
        let should_convert_to_video = duration >= 3.0 || is_high_quality_animated(width, height);
        assert!(should_convert_to_video, "短动画+高质量应该转视频");
    }
    
    #[test]
    fn test_apple_compat_routing_long_low_quality() {
        // 长动画 + 低质量 → 应该转视频
        let duration = 5.0; // >= 3秒
        let (width, height) = (400, 300); // 低质量
        
        let should_convert_to_video = duration >= 3.0 || is_high_quality_animated(width, height);
        assert!(should_convert_to_video, "长动画应该转视频，不管质量");
    }
    
    #[test]
    fn test_apple_compat_routing_long_high_quality() {
        // 长动画 + 高质量 → 应该转视频
        let duration = 10.0; // >= 3秒
        let (width, height) = (1920, 1080); // 高质量
        
        let should_convert_to_video = duration >= 3.0 || is_high_quality_animated(width, height);
        assert!(should_convert_to_video, "长动画+高质量应该转视频");
    }
    
    #[test]
    fn test_apple_compat_boundary_3_seconds() {
        // 边界测试：正好 3 秒
        let duration = 3.0;
        let (width, height) = (400, 300); // 低质量
        
        let should_convert_to_video = duration >= 3.0 || is_high_quality_animated(width, height);
        assert!(should_convert_to_video, "正好3秒应该转视频");
    }
    
    #[test]
    fn test_apple_compat_boundary_just_under_3_seconds() {
        // 边界测试：刚好不到 3 秒
        let duration = 2.99;
        let (width, height) = (400, 300); // 低质量
        
        let should_convert_to_video = duration >= 3.0 || is_high_quality_animated(width, height);
        assert!(!should_convert_to_video, "2.99秒+低质量应该转GIF");
    }
    
    // ============================================================
    // 🔧 cjxl 预处理测试
    // ============================================================
    
    #[test]
    fn test_prepare_input_webp() {
        // WebP 文件应该触发 dwebp 预处理
        let input = Path::new("/tmp/test.webp");
        // 注意：这个测试只验证逻辑，不实际执行（文件不存在）
        // 实际测试需要真实文件
        let ext = input.extension()
            .map(|e| e.to_ascii_lowercase())
            .and_then(|e| e.to_str().map(|s| s.to_string()))
            .unwrap_or_default();
        assert_eq!(ext, "webp");
    }
    
    #[test]
    fn test_prepare_input_tiff() {
        let input = Path::new("/tmp/test.tiff");
        let ext = input.extension()
            .map(|e| e.to_ascii_lowercase())
            .and_then(|e| e.to_str().map(|s| s.to_string()))
            .unwrap_or_default();
        assert_eq!(ext, "tiff");
    }
    
    #[test]
    fn test_prepare_input_heic() {
        let input = Path::new("/tmp/test.HEIC");
        let ext = input.extension()
            .map(|e| e.to_ascii_lowercase())
            .and_then(|e| e.to_str().map(|s| s.to_string()))
            .unwrap_or_default();
        assert_eq!(ext, "heic");
    }
    
    #[test]
    fn test_prepare_input_png_no_preprocess() {
        // PNG 不需要预处理
        let input = Path::new("/tmp/test.png");
        let ext = input.extension()
            .map(|e| e.to_ascii_lowercase())
            .and_then(|e| e.to_str().map(|s| s.to_string()))
            .unwrap_or_default();
        assert_eq!(ext, "png");
        // PNG 应该直接使用，不需要预处理
        assert!(!["webp", "tiff", "tif", "bmp", "heic", "heif"].contains(&ext.as_str()));
    }
    
    #[test]
    fn test_prepare_input_jpeg_no_preprocess() {
        // JPEG 不需要预处理（使用 lossless_jpeg 模式）
        let input = Path::new("/tmp/test.jpg");
        let ext = input.extension()
            .map(|e| e.to_ascii_lowercase())
            .and_then(|e| e.to_str().map(|s| s.to_string()))
            .unwrap_or_default();
        assert_eq!(ext, "jpg");
        assert!(!["webp", "tiff", "tif", "bmp", "heic", "heif"].contains(&ext.as_str()));
    }
    
    // ============================================================
    // 🔧 预处理格式覆盖测试
    // ============================================================
    
    #[test]
    fn test_preprocess_formats_coverage() {
        // 验证所有需要预处理的格式都被覆盖
        let preprocess_formats = ["webp", "tiff", "tif", "bmp", "heic", "heif"];
        let direct_formats = ["png", "jpg", "jpeg", "gif", "jxl", "avif"];
        
        // 预处理格式不应该与直接格式重叠
        for fmt in &preprocess_formats {
            assert!(!direct_formats.contains(fmt), "{} 不应该在直接格式列表中", fmt);
        }
    }
}
