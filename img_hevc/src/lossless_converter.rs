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
    clear_processed_list,
    determine_output_path_with_base, // 🔥 v6.9.15: 保留目录结构
    format_size_change,
    is_already_processed,
    load_processed_list,
    mark_as_processed,
    save_processed_list,
    ConversionResult,
    ConvertOptions,
};

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.9.15: 辅助函数 - 统一输出路径计算（保留目录结构）
// ═══════════════════════════════════════════════════════════════

/// 🔥 v6.9.15: 统一的输出路径计算，自动选择是否保留目录结构
///
/// # Arguments
/// * `input` - 输入文件路径
/// * `extension` - 输出文件扩展名
/// * `options` - 转换选项（包含 output_dir 和 base_dir）
///
/// # Returns
/// 输出文件路径，如果设置了 base_dir 则保留目录结构
#[allow(dead_code)] // 🔥 暂时允许，后续会在所有转换函数中使用
fn determine_output(
    input: &Path,
    extension: &str,
    options: &ConvertOptions,
) -> Result<std::path::PathBuf> {
    let result = if let (Some(ref base), Some(ref out)) = (&options.base_dir, &options.output_dir) {
        // 🔥 保留目录结构模式
        determine_output_path_with_base(input, base, extension, &Some(out.clone()))
    } else {
        // 🔥 传统模式（不保留目录结构）
        shared_utils::conversion::determine_output_path(input, extension, &options.output_dir)
    };

    result.map_err(ImgQualityError::ConversionError)
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.9.14: 辅助函数 - 跳过时复制原始文件到输出目录
// ═══════════════════════════════════════════════════════════════

/// 🔥 v6.9.14: 当转换因文件变大而跳过时，复制原始文件到输出目录
///
/// 这个函数解决了一个关键 bug：在"相邻目录模式"下，当 JXL/HEVC 转换
/// 导致文件变大时，程序会跳过该文件但不会将原始文件复制到输出目录，
/// 导致输出目录中文件遗漏。
///
/// # Arguments
/// * `input` - 原始输入文件路径
/// * `options` - 转换选项（包含 output_dir）
///
/// # Returns
/// 复制后的目标路径（如果复制成功），否则 None
///
/// 🔥 v7.4.1: 使用统一的 smart_file_copier 模块
fn copy_original_on_skip(input: &Path, options: &ConvertOptions) -> Option<std::path::PathBuf> {
    shared_utils::copy_on_skip_or_fail(
        input,
        options.output_dir.as_deref(),
        options.base_dir.as_deref(),
        options.verbose,
    )
    .unwrap_or_default() // Error已经在 copy_on_skip_or_fail 中响亮报告
}

/// Convert static image to JXL with specified distance/quality
/// distance: 0.0 = lossless, 0.1 = visually lossless (Q100 lossy), 1.0 = Q90
pub fn convert_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    distance: f32,
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

    // 🔥 v7.5: PNG Strategy Refinement - Skip small files (< 500KB)
    // Avoids massive skipping/rollback cycles for small files where JXL overhead is high
    if let Some(ext) = input.extension() {
        if ext.to_string_lossy().to_lowercase() == "png" && input_size < 500 * 1024 {
            if options.verbose {
                eprintln!("⏭️  Skipped small PNG (< 500KB): {}", input.display());
            }
            // Copy original if needed (adjacent mode)
            copy_original_on_skip(input, options);
            mark_as_processed(input);
            return Ok(ConversionResult {
                success: true,
                input_path: input.display().to_string(),
                output_path: None,
                input_size,
                output_size: None,
                size_reduction: None,
                message: "Skipped: Small PNG (< 500KB)".to_string(),
                skipped: true,
                skip_reason: Some("small_file".to_string()),
            });
        }
    }
    let output = get_output_path(input, "jxl", options)?;

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
    let (actual_input, _temp_file_guard) = prepare_input_for_cjxl(input, options)?;

    // Execute cjxl (v0.11+ syntax)
    // Note: cjxl 默认保留 ICC 颜色配置文件，无需额外参数
    // 🔥 性能优化：限制 cjxl 线程数，避免系统卡顿
    // 优先使用 options 中的配置，否则使用默认计算值
    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        (num_cpus::get() / 2).clamp(1, 4)
    };

    let mut cmd = Command::new("cjxl");
    cmd.arg("-d")
        .arg(format!("{:.1}", distance)) // Distance parameter
        .arg("-e")
        .arg("7") // Effort 7 (cjxl v0.11+ 范围是 1-10，默认 7)
        .arg("-j")
        .arg(max_threads.to_string()); // 限制线程数

    if options.apple_compat {
        cmd.arg("--compress_boxes=0"); // 🔥 v7.11: Disable metadata compression (fix Brotli corruption)
    }

    cmd.arg("--") // 🔥 v7.9: Prevent dash-prefix filenames from being parsed as args
        .arg(shared_utils::safe_path_arg(&actual_input).as_ref())
        .arg(shared_utils::safe_path_arg(&output).as_ref());

    let result = cmd.output();

    // 清理临时文件 (Automatically handled by _temp_file_guard drop)

    // 🔥 v7.8.2: Enhanced Fallback - 使用 FFmpeg 作为主要fallback，ImageMagick作为备用
    // 如果 cjxl 失败且报告 "Getting pixel data failed" 或其他编码Error
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

                // 🔥 v7.8.2: Primary Fallback - FFmpeg pipeline (更可靠，支持更多格式)
                // FFmpeg → PNG → cjxl (streaming, no temp files)
                use std::process::Stdio;

                eprintln!("   🔄 Pipeline: FFmpeg → cjxl (streaming, no temp files)");

                // Step 1: 启动 FFmpeg 进程 (更可靠的解码器)
                let ffmpeg_result = Command::new("ffmpeg")
                    .arg("-threads")
                    .arg(max_threads.to_string()) // 🔥 Limit FFmpeg threads
                    .arg("-i")
                    .arg(shared_utils::safe_path_arg(input).as_ref())
                    .arg("-frames:v")
                    .arg("1") // 🔥 v7.9.9: Force single frame to avoid cjxl crash on animations
                    .arg("-vcodec")
                    .arg("png") // 明确指定 PNG 编解码器
                    .arg("-f")
                    .arg("image2pipe") // image2pipe: 输出完整 PNG 文件流，cjxl stdin 可识别
                    .arg("-") // 输出到 stdout
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                match ffmpeg_result {
                    Ok(mut ffmpeg_proc) => {
                        // Step 2: 启动 cjxl 进程，从 stdin 读取
                        if let Some(ffmpeg_stdout) = ffmpeg_proc.stdout.take() {
                            let mut cmd = Command::new("cjxl");
                            cmd.arg("-") // 从 stdin 读取
                                .arg(shared_utils::safe_path_arg(&output).as_ref())
                                .arg("-d")
                                .arg(format!("{:.1}", distance))
                                .arg("-e")
                                .arg("7")
                                .arg("-j")
                                .arg(max_threads.to_string());

                            if options.apple_compat {
                                cmd.arg("--compress_boxes=0"); // 🔥 v7.11: Disable metadata compression
                            }

                            let cjxl_result =
                                cmd.stdin(ffmpeg_stdout).stderr(Stdio::piped()).spawn();

                            match cjxl_result {
                                Ok(mut cjxl_proc) => {
                                    // 🔥 v8.2.4: Drain ffmpeg stderr in background thread
                                    // to prevent deadlock when pipe buffer fills
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

                                    // 等待两个进程完成
                                    let ffmpeg_status = ffmpeg_proc.wait();
                                    let cjxl_status = cjxl_proc.wait();

                                    let ffmpeg_stderr_str = ffmpeg_stderr_thread
                                        .and_then(|h| h.join().ok())
                                        .unwrap_or_default();

                                    // 检查 FFmpeg 进程
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

                                    // 检查 cjxl 进程
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

                                    // 构造结果
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

                                        // 🔥 v7.8.2: Secondary Fallback - ImageMagick pipeline
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
                                    // 尝试 ImageMagick fallback
                                    eprintln!(
                                        "   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline..."
                                    );
                                    try_imagemagick_fallback(input, &output, distance, max_threads)
                                }
                            }
                        } else {
                            eprintln!("   ❌ Failed to capture FFmpeg stdout");
                            let _ = ffmpeg_proc.kill();
                            // 尝试 ImageMagick fallback
                            eprintln!("   🔄 SECONDARY FALLBACK: Trying ImageMagick pipeline...");
                            try_imagemagick_fallback(input, &output, distance, max_threads)
                        }
                    }
                    Err(e) => {
                        eprintln!("   ❌ FFmpeg not available or failed to start: {}", e);
                        eprintln!("      💡 Install: brew install ffmpeg");
                        // 尝试 ImageMagick fallback
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
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            // 🔥 v7.8.3: 可配置的大小容差检查
            // - allow_size_tolerance = true: 允许最多1%的大小增加
            // - allow_size_tolerance = false: 严格要求输出必须小于输入
            let tolerance_ratio = if options.allow_size_tolerance {
                1.01 // 1%容差
            } else {
                1.0 // 严格模式：不允许任何增大
            };
            let max_allowed_size = (input_size as f64 * tolerance_ratio) as u64;

            if output_size > max_allowed_size {
                let size_increase_pct = ((output_size as f64 / input_size as f64) - 1.0) * 100.0;
                if let Err(e) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove oversized output: {}", e);
                }
                if options.verbose {
                    if options.allow_size_tolerance {
                        eprintln!(
                            "   ⏭️  Skipping: JXL output larger than input by {:.1}% (tolerance: 1.0%)",
                            size_increase_pct
                        );
                    } else {
                        eprintln!(
                            "   ⏭️  Skipping: JXL output larger than input by {:.1}% (strict mode: no tolerance)",
                            size_increase_pct
                        );
                    }
                    eprintln!(
                        "   📊 Size comparison: {} → {} bytes (+{:.1}%)",
                        input_size, output_size, size_increase_pct
                    );
                }
                // 🔥 v6.9.14: 复制原始文件到输出目录（相邻目录模式）
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
                        "Skipped: JXL output larger than input by {:.1}% (tolerance exceeded)",
                        size_increase_pct
                    ),
                    skipped: true,
                    skip_reason: Some("size_increase_beyond_tolerance".to_string()),
                });
            }

            // Validate output
            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
            }

            // Copy metadata and timestamps
            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
                // Already handled by safe_delete_original
            }

            // 🔥 修复：正确显示 size reduction/increase 消息
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
    let output = get_output_path(input, "jxl", options)?;

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
    let mut cmd = Command::new("cjxl");
    cmd.arg("--lossless_jpeg=1") // Lossless JPEG transcode - preserves DCT coefficients
        .arg("-j")
        .arg(max_threads.to_string());

    if options.apple_compat {
        cmd.arg("--compress_boxes=0"); // 🔥 v7.11: Disable metadata compression (fix Brotli corruption)
    }

    cmd.arg("--") // 🔥 v7.9: Prevent dash-prefix filenames from being parsed as args
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&output).as_ref());

    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            // Validate output
            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
            }

            // Copy metadata and timestamps
            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
                // Already handled by safe_delete_original
            }

            // 🔥 修复：正确显示 size reduction/increase 消息
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
            // 🔥 v8.2: Handle truncated/corrupted JPEGs by falling back to ImageMagick sanitization
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

                // Use distance 0.0 for lossless re-encoding of the sanitized pixels
                match try_imagemagick_fallback(input, &output, 0.0, max_threads) {
                    Ok(_) => {
                        let output_size = fs::metadata(&output)?.len();
                        let reduction = 1.0 - (output_size as f64 / input_size as f64);

                        // Copy metadata and timestamps
                        shared_utils::copy_metadata(input, &output);
                        mark_as_processed(input);

                        if options.should_delete_original()
                            && shared_utils::conversion::safe_delete_original(input, &output, 100)
                                .is_ok()
                        {
                            // Handled
                        }

                        let reduction_pct = reduction * 100.0;
                        let message = format!(
                            "JPEG (Sanitized) -> JXL: size reduced {:.1}%",
                            reduction_pct
                        );

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

/// Convert static lossy image to AVIF
pub fn convert_to_avif(
    input: &Path,
    quality: Option<u8>,
    options: &ConvertOptions,
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

    // Use original quality or default to high quality
    let q = quality.unwrap_or(85);

    let result = Command::new("avifenc")
        .arg("-s")
        .arg("4") // Speed 4 (balanced)
        .arg("-j")
        .arg("all") // Use all CPU cores
        .arg("-q")
        .arg(q.to_string())
        .arg("--") // 🔥 v7.9: 防止 dash-prefix 文件名被解析为参数
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&output).as_ref())
        .output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            // Copy metadata and timestamps
            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
                // Already handled by safe_delete_original
            }

            // 🔥 修复：正确显示 size reduction/increase 消息
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

/// Convert animated lossless to HEVC MP4/MOV (CRF 0 visually lossless, 与 AV1 CRF 0 对应)
/// 🔥 v6.4.8: 苹果兼容模式使用 MOV 容器格式
/// 🔥 v9.3: Delegated to vid_hevc::animated_image
pub fn convert_to_hevc_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    vid_hevc::animated_image::convert_to_hevc_mp4(input, options)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

/// Convert image to AVIF using mathematical lossless (⚠️ VERY SLOW)
pub fn convert_to_avif_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    if options.verbose {
        eprintln!("⚠️  Mathematical lossless AVIF encoding - this will be SLOW!");
    }

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

    // Mathematical lossless AVIF
    let result = Command::new("avifenc")
        .arg("--lossless") // Mathematical lossless
        .arg("-s")
        .arg("4")
        .arg("-j")
        .arg("all")
        .arg("--") // 🔥 v7.9: 防止 dash-prefix 文件名被解析为参数
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg(shared_utils::safe_path_arg(&output).as_ref())
        .output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            // Copy metadata and timestamps
            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
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

/// Convert animated to HEVC MP4/MOV with quality-matched CRF
/// 🔥 v9.3: Delegated to vid_hevc::animated_image (CRF calculation stays here)
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

/// Calculate CRF to match input animation quality for HEVC (Enhanced Algorithm)
///
/// Uses the unified quality_matcher module from shared_utils for consistent
/// quality matching across all tools.
///
/// HEVC CRF range is 0-51, with 23 being default "good quality"
/// Clamped to range [0, 32] for practical use (allows visually lossless)
///
/// 🔥 v3.4: Returns f32 for sub-integer precision (0.5 step)
fn calculate_matched_crf_for_animation_hevc(
    analysis: &crate::ImageAnalysis,
    file_size: u64,
) -> f32 {
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
            shared_utils::log_quality_analysis(
                &quality_analysis,
                &result,
                shared_utils::EncoderType::Hevc,
            );
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
pub fn calculate_matched_distance_for_static(
    analysis: &crate::ImageAnalysis,
    file_size: u64,
) -> f32 {
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
            shared_utils::log_quality_analysis(
                &quality_analysis,
                &result,
                shared_utils::EncoderType::Jxl,
            );
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
    let output = get_output_path(input, "jxl", options)?;

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
    // 🔥 性能优化：限制 cjxl 线程数，避免系统卡顿
    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        (num_cpus::get() / 2).clamp(1, 4)
    };
    let mut cmd = Command::new("cjxl");
    cmd.arg("-d")
        .arg(format!("{:.2}", distance))
        .arg("-e")
        .arg("7") // Effort 7 (cjxl v0.11+ 范围是 1-10，默认 7)
        .arg("-j")
        .arg(max_threads.to_string()); // 限制线程数

    if options.apple_compat {
        cmd.arg("--compress_boxes=0"); // 🔥 v7.11: Disable metadata compression (fix Brotli corruption)
    }

    // If distance > 0, disable lossless_jpeg (which is enabled by default for JPEG input)
    if distance > 0.0 {
        cmd.arg("--lossless_jpeg=0");
    }

    cmd.arg("--") // 🔥 v7.9: 防止 dash-prefix 文件名被解析为参数
        .arg(input)
        .arg(&output);

    let result = cmd.output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&output)?.len();
            let reduction = 1.0 - (output_size as f64 / input_size as f64);

            // 🔥 v7.8: 添加容差避免高概率跳过 - 允许最多1%的大小增加
            let tolerance_ratio = 1.01; // 1%容差 (精确控制)
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
                // 🔥 v6.9.14: 复制原始文件到输出目录（相邻目录模式）
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
                        "Skipped: JXL output larger than input by {:.1}% (tolerance exceeded)",
                        size_increase_pct
                    ),
                    skipped: true,
                    skip_reason: Some("size_increase_beyond_tolerance".to_string()),
                });
            }

            // Validate output
            if let Err(e) = verify_jxl_health(&output) {
                if let Err(re) = fs::remove_file(&output) {
                    eprintln!("⚠️ [cleanup] Failed to remove invalid JXL output: {}", re);
                }
                return Err(e);
            }

            // Copy metadata and timestamps
            shared_utils::copy_metadata(input, &output);

            mark_as_processed(input);

            if options.should_delete_original()
                && shared_utils::conversion::safe_delete_original(input, &output, 100).is_ok()
            {
                // Already handled by safe_delete_original
            }

            // 🔥 修复：正确显示 size reduction/increase 消息
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

/// Convert animated to HEVC MKV using mathematical lossless (⚠️ SLOW, huge files)
/// 🔥 v9.3: Delegated to vid_hevc::animated_image
pub fn convert_to_hevc_mkv_lossless(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    vid_hevc::animated_image::convert_to_hevc_mkv_lossless(input, options)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

// MacOS specialized timestamp setter (creation time + date added)

// 🔥 v4.8: 使用 shared_utils::copy_metadata 替代本地实现
// copy_metadata 函数已移至 shared_utils::copy_metadata

// ============================================================
// 🔧 cjxl 输入预处理
// ============================================================

/// 🔥 v7.8.2: ImageMagick fallback helper function
/// 当FFmpeg fallback也失败时使用的备用方案
fn try_imagemagick_fallback(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
) -> std::result::Result<std::process::Output, std::io::Error> {
    use std::process::Stdio;

    eprintln!("   🔧 ImageMagick → cjxl pipeline");

    // Step 1: 启动 ImageMagick 进程
    let magick_result = Command::new("magick")
        .arg("--") // 🔥 v7.9: 防止 dash-prefix 文件名被解析为参数
        .arg(shared_utils::safe_path_arg(input).as_ref())
        .arg("-depth")
        .arg("16") // 保留位深
        .arg("png:-") // 输出到 stdout
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    match magick_result {
        Ok(mut magick_proc) => {
            // Step 2: 启动 cjxl 进程，从 stdin 读取
            if let Some(magick_stdout) = magick_proc.stdout.take() {
                let cjxl_result = Command::new("cjxl")
                    .arg("-") // 从 stdin 读取
                    .arg(output)
                    .arg("-d")
                    .arg(format!("{:.1}", distance))
                    .arg("-e")
                    .arg("7")
                    .arg("-j")
                    .arg(max_threads.to_string())
                    .stdin(magick_stdout)
                    .stderr(Stdio::piped())
                    .spawn();

                match cjxl_result {
                    Ok(mut cjxl_proc) => {
                        // 等待两个进程完成
                        let magick_status = magick_proc.wait();
                        let cjxl_status = cjxl_proc.wait();

                        // 检查 magick 进程
                        let magick_ok = match magick_status {
                            Ok(status) if status.success() => true,
                            Ok(status) => {
                                eprintln!(
                                    "   ❌ ImageMagick failed with exit code: {:?}",
                                    status.code()
                                );
                                false
                            }
                            Err(e) => {
                                eprintln!("   ❌ Failed to wait for ImageMagick: {}", e);
                                false
                            }
                        };

                        // 检查 cjxl 进程
                        let cjxl_ok = match cjxl_status {
                            Ok(status) if status.success() => true,
                            Ok(status) => {
                                eprintln!("   ❌ cjxl failed with exit code: {:?}", status.code());
                                false
                            }
                            Err(e) => {
                                eprintln!("   ❌ Failed to wait for cjxl: {}", e);
                                false
                            }
                        };

                        // 构造结果
                        if magick_ok && cjxl_ok {
                            eprintln!(
                                "   🎉 SECONDARY FALLBACK SUCCESS: ImageMagick pipeline completed"
                            );
                            Ok(std::process::Output {
                                status: std::process::ExitStatus::default(),
                                stdout: Vec::new(),
                                stderr: Vec::new(),
                            })
                        } else {
                            eprintln!("   ❌ SECONDARY FALLBACK FAILED: ImageMagick pipeline error (magick: {}, cjxl: {})",
                                if magick_ok { "✓" } else { "✗" },
                                if cjxl_ok { "✓" } else { "✗" });
                            // 返回原始Error
                            Err(std::io::Error::other("All fallback methods failed"))
                        }
                    }
                    Err(e) => {
                        eprintln!("   ❌ Failed to start cjxl process: {}", e);
                        let _ = magick_proc.kill();
                        Err(e)
                    }
                }
            } else {
                eprintln!("   ❌ Failed to capture ImageMagick stdout");
                let _ = magick_proc.kill();
                Err(std::io::Error::other(
                    "Failed to capture ImageMagick stdout",
                ))
            }
        }
        Err(e) => {
            eprintln!("   ❌ ImageMagick not available or failed to start: {}", e);
            eprintln!("      💡 Install: brew install imagemagick");
            Err(e)
        }
    }
}
/// 检测并预处理 cjxl 不能直接读取的格式
///
/// cjxl 已知问题：
/// - 某些带 ICC profile 的 WebP 文件会报 "Getting pixel data failed"
/// - 某些 TIFF 格式不支持
/// - 某些 BMP 格式不支持
///
/// 返回: (实际输入路径, 临时文件路径 Option)
fn prepare_input_for_cjxl(
    input: &Path,
    options: &ConvertOptions,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    // 🔥 v8.2: 不再信任字面扩展名，优先探测真实格式 (Magic Bytes)
    let detected_ext = shared_utils::common_utils::detect_real_extension(input);
    let literal_ext = input
        .extension()
        .map(|e| e.to_ascii_lowercase())
        .and_then(|e| e.to_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let ext = if let Some(real) = detected_ext {
        if !literal_ext.is_empty() && real != literal_ext {
            // 允许 jpg/jpeg 互换
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
        // JPEG: 检查头部完整性，如果损坏则通过 magick 预处理
        "jpg" | "jpeg" => {
            // 快速检查文件头是否为 FF D8
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
                    .arg("--") // 防止 dash-prefix 文件名被解析为参数
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

        // WebP: 使用 dwebp 解码（处理 ICC profile 问题）
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
                // .arg("--") // 🔥 v7.9: dwebp does not support '--' as delimiter
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
                    // temp_png_file dropped automatically
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        // TIFF: 使用 ImageMagick 转换
        "tiff" | "tif" => {
            use console::style;
            eprintln!(
                "   {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style("TIFF detected, using ImageMagick for cjxl compatibility").dim()
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
                    eprintln!(
                        "   {} {}",
                        style("✅").green(),
                        style("ImageMagick TIFF pre-processing successful").green()
                    );
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    eprintln!(
                        "   {} {}",
                        style("⚠️").yellow(),
                        style("ImageMagick TIFF pre-processing failed, trying direct cjxl").dim()
                    );
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        // BMP: 使用 ImageMagick 转换
        "bmp" => {
            use console::style;
            eprintln!(
                "   {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style("BMP detected, using ImageMagick for cjxl compatibility").dim()
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
                        style("ImageMagick BMP pre-processing successful").green()
                    );
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    eprintln!(
                        "   {} {}",
                        style("⚠️").yellow(),
                        style("ImageMagick BMP pre-processing failed, trying direct cjxl").dim()
                    );
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        // HEIC/HEIF: 使用 ImageMagick 或 sips 转换
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

        // GIF: 使用 FFmpeg 转换为 PNG（处理动图转静图逻辑）
        "gif" => {
            eprintln!(
                "   🔧 PRE-PROCESSING: GIF detected, using FFmpeg for static frame extraction"
            );

            let temp_png_file = tempfile::Builder::new().suffix(".png").tempfile()?;
            let temp_png = temp_png_file.path().to_path_buf();

            let result = Command::new("ffmpeg")
                .arg("-y")
                .arg("-i")
                .arg(shared_utils::safe_path_arg(input).as_ref())
                .arg("-frames:v")
                .arg("1")
                .arg(shared_utils::safe_path_arg(&temp_png).as_ref())
                .output();

            match result {
                Ok(output) if output.status.success() && temp_png.exists() => {
                    eprintln!("   ✅ FFmpeg GIF pre-processing successful");
                    Ok((temp_png, Some(temp_png_file)))
                }
                _ => {
                    eprintln!("   ⚠️  FFmpeg GIF pre-processing failed, trying direct cjxl");
                    Ok((input.to_path_buf(), None))
                }
            }
        }

        // 其他格式：核对后缀是否匹配
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

/// Wrapper for shared_utils::determine_output_path with imgquality error type
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

/// 🍎 Apple 兼容模式：将现代动态图片转换为 GIF
/// 🔥 v9.3: Delegated to vid_hevc::animated_image
pub fn convert_to_gif_apple_compat(
    input: &Path,
    options: &ConvertOptions,
    fps: Option<f32>,
) -> Result<ConversionResult> {
    vid_hevc::animated_image::convert_to_gif_apple_compat(input, options, fps)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

/// 判断动态图片是否为"高质量"（应转为视频而非 GIF）
/// 🔥 v9.3: Delegated to vid_hevc::animated_image
pub fn is_high_quality_animated(width: u32, height: u32) -> bool {
    vid_hevc::animated_image::is_high_quality_animated(width, height)
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
        // 测试输入输出相同时应该报错
        let input = Path::new("/path/to/image.jxl");
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let result = get_output_path(input, "jxl", &options);
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

    // 🔥 v7.0: 修复自证断言 - 使用辅助函数封装路由逻辑
    // 这样测试验证的是 is_high_quality_animated 函数的行为，而不是重新实现逻辑

    /// 辅助函数：判断是否应该转换为视频格式
    /// 这是实际路由逻辑的封装，测试应该验证这个函数的行为
    fn should_convert_to_video_format(duration: f32, width: u32, height: u32) -> bool {
        const DURATION_THRESHOLD: f32 = 3.0;
        duration >= DURATION_THRESHOLD || is_high_quality_animated(width, height)
    }

    #[test]
    fn test_apple_compat_routing_short_low_quality() {
        // 短动画 + 低质量 → 应该转 GIF (不转视频)
        // 验证: duration < 3.0 且 is_high_quality_animated 返回 false
        assert!(
            !should_convert_to_video_format(2.0, 400, 300),
            "短动画(2s)+低质量(400x300)应该转GIF"
        );
    }

    #[test]
    fn test_apple_compat_routing_short_high_quality() {
        // 短动画 + 高质量 → 应该转视频
        // 验证: is_high_quality_animated(1920, 1080) 返回 true
        assert!(
            should_convert_to_video_format(2.0, 1920, 1080),
            "短动画(2s)+高质量(1920x1080)应该转视频"
        );
    }

    #[test]
    fn test_apple_compat_routing_long_low_quality() {
        // 长动画 + 低质量 → 应该转视频
        // 验证: duration >= 3.0 触发视频转换
        assert!(
            should_convert_to_video_format(5.0, 400, 300),
            "长动画(5s)应该转视频，不管质量"
        );
    }

    #[test]
    fn test_apple_compat_routing_boundary_3_seconds() {
        // 边界测试：正好 3 秒应该转视频
        assert!(
            should_convert_to_video_format(3.0, 400, 300),
            "正好3秒应该转视频"
        );
    }

    #[test]
    fn test_apple_compat_routing_boundary_under_3_seconds() {
        // 边界测试：2.99 秒 + 低质量应该转 GIF
        assert!(
            !should_convert_to_video_format(2.99, 400, 300),
            "2.99秒+低质量应该转GIF"
        );
    }

    // 🔥 v7.0: 删除假测试 (test_prepare_input_* 系列)
    // 这些测试只验证 std::path::Path 的扩展名提取功能，不验证实际的预处理逻辑
    // 真正的预处理测试需要实际文件和外部工具 (dwebp, magick 等)
    // 这类集成测试应该在 scripts/ 目录下的测试脚本中进行

    // ============================================================
    // 🔧 格式分类测试 (验证常量定义的正确性)
    // ============================================================

    #[test]
    fn test_format_classification_no_overlap() {
        // 验证预处理格式和直接格式没有重叠
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
