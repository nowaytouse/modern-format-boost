//! Video Conversion API Module - HEVC/H.265 Version
//!
//! Pure conversion layer - executes video conversions based on detection results.
//! - Auto Mode: HEVC Lossless for lossless sources, HEVC CRF for lossy sources
//! - Simple Mode: Always HEVC MP4
//! - Size Exploration: Tries higher CRF if output is larger than input

use crate::detection_api::{detect_video, CompressionType, VideoDetectionResult};
use crate::{Result, VidQualityError};

use shared_utils::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, TargetVideoFormat,
};
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

pub fn determine_strategy(result: &VideoDetectionResult) -> ConversionStrategy {
    determine_strategy_with_apple_compat(result, false)
}

pub fn determine_strategy_with_apple_compat(
    result: &VideoDetectionResult,
    apple_compat: bool,
) -> ConversionStrategy {
    let skip_decision = if apple_compat {
        shared_utils::should_skip_video_codec_apple_compat(result.codec.as_str())
    } else {
        shared_utils::should_skip_video_codec(result.codec.as_str())
    };

    if skip_decision.should_skip {
        return ConversionStrategy {
            target: TargetVideoFormat::Skip,
            reason: skip_decision.reason,
            command: String::new(),
            preserve_audio: false,
            crf: 0.0,
            lossless: false,
        };
    }

    if let crate::detection_api::DetectedCodec::Unknown(ref s) = result.codec {
        let unknown_skip = if apple_compat {
            shared_utils::should_skip_video_codec_apple_compat(s)
        } else {
            shared_utils::should_skip_video_codec(s)
        };
        if unknown_skip.should_skip {
            return ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: unknown_skip.reason,
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            };
        }
    }

    let (target, reason, crf, lossless) = match result.compression {
        CompressionType::Lossless => (
            TargetVideoFormat::HevcLosslessMkv,
            format!(
                "Source is {} (lossless) - converting to HEVC Lossless",
                result.codec.as_str()
            ),
            0.0_f32,
            true,
        ),
        CompressionType::VisuallyLossless => (
            TargetVideoFormat::HevcMp4,
            format!(
                "Source is {} (visually lossless) - compressing with HEVC CRF 18",
                result.codec.as_str()
            ),
            18.0_f32,
            false,
        ),
        _ => (
            TargetVideoFormat::HevcMp4,
            format!(
                "Source is {} ({}) - compressing with HEVC CRF 20",
                result.codec.as_str(),
                result.compression.as_str()
            ),
            20.0_f32,
            false,
        ),
    };

    ConversionStrategy {
        target,
        reason,
        command: String::new(),
        preserve_audio: result.has_audio,
        crf,
        lossless,
    }
}

pub fn simple_convert(input: &Path, output_dir: Option<&Path>) -> Result<ConversionOutput> {
    let detection = detect_video(input)?;

    let output_dir = output_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());

    std::fs::create_dir_all(&output_dir)?;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");

    let output_path = if input_ext.eq_ignore_ascii_case("mp4") {
        output_dir.join(format!("{}_hevc.mp4", stem))
    } else {
        output_dir.join(format!("{}.mp4", stem))
    };

    info!("🎬 Simple Mode: {} → HEVC MP4 (CRF 18)", input.display());

    let max_threads = shared_utils::thread_manager::get_balanced_thread_config(
        shared_utils::thread_manager::WorkloadType::Video,
    )
    .child_threads;

    let output_size = execute_hevc_conversion(&detection, &output_path, 18, max_threads)?;

    shared_utils::copy_metadata(input, &output_path);

    let size_ratio = output_size as f64 / detection.file_size as f64;

    info!("   ✅ Complete: {:.1}% of original", size_ratio * 100.0);

    Ok(ConversionOutput {
        input_path: input.display().to_string(),
        output_path: output_path.display().to_string(),
        strategy: ConversionStrategy {
            target: TargetVideoFormat::HevcMp4,
            reason: "Simple mode: HEVC High Quality".to_string(),
            command: String::new(),
            preserve_audio: detection.has_audio,
            crf: 18.0,
            lossless: false,
        },
        input_size: detection.file_size,
        output_size,
        size_ratio,
        success: true,
        message: "Simple conversion successful (HEVC CRF 18)".to_string(),
        final_crf: 18.0,
        exploration_attempts: 0,
    })
}

pub fn auto_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    let _label = input
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    shared_utils::progress_mode::set_log_context(&_label);
    let _log_guard = shared_utils::progress_mode::LogContextGuard;

    let detection = detect_video(input)?;
    let strategy = determine_strategy_with_apple_compat(&detection, config.apple_compat);

    if strategy.target == TargetVideoFormat::Skip {
        info!("🎬 Auto Mode: {} → SKIP", input.display());
        info!("   Reason: {}", strategy.reason);

        let _ = shared_utils::copy_on_skip_or_fail(
            input,
            config.output_dir.as_deref(),
            config.base_dir.as_deref(),
            false,
        );

        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: "".to_string(),
            strategy,
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 0.0,
            success: true,
            message: "Skipped modern codec to avoid generation loss".to_string(),
            final_crf: 0.0,
            exploration_attempts: 0,
        });
    }

    let output_dir =
        if let (Some(ref user_out), Some(ref base)) = (&config.output_dir, &config.base_dir) {
            let rel_path = input
                .strip_prefix(base)
                .unwrap_or(input)
                .parent()
                .unwrap_or(Path::new(""));
            user_out.join(rel_path)
        } else {
            config
                .output_dir
                .clone()
                .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf())
        };

    std::fs::create_dir_all(&output_dir)?;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let target_ext = if config.apple_compat && strategy.target == TargetVideoFormat::HevcMp4 {
        "mov"
    } else {
        strategy.target.extension()
    };
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");

    let output_path = if input_ext.eq_ignore_ascii_case(target_ext)
        || (config.apple_compat && input_ext.eq_ignore_ascii_case("mov"))
    {
        output_dir.join(format!("{}_hevc.{}", stem, target_ext))
    } else {
        output_dir.join(format!("{}.{}", stem, target_ext))
    };

    shared_utils::path_validator::check_input_output_conflict(input, &output_path)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

    if output_path.exists() && !config.force {
        info!("⏭️ Output exists, skipping: {}", output_path.display());
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),
            strategy: strategy.clone(),
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 1.0,
            success: true,
            message: format!("Skipped: output exists ({})", output_path.display()),
            final_crf: 0.0,
            exploration_attempts: 0,
        });
    }

    info!(
        "🎬 Auto Mode: {} → {}",
        input.display(),
        strategy.target.as_str()
    );
    info!("   Reason: {}", strategy.reason);

    let (output_size, final_crf, attempts, explore_result_opt) = match strategy.target {
        TargetVideoFormat::HevcLosslessMkv => {
            info!("   🚀 Using HEVC Lossless Mode");
            let size = execute_hevc_lossless(&detection, &output_path, config.child_threads)?;
            (size, 0.0, 0, None)
        }
        TargetVideoFormat::HevcMp4 => {
            if config.use_lossless {
                info!("   🚀 Using HEVC Lossless Mode (forced)");
                let size = execute_hevc_lossless(&detection, &output_path, config.child_threads)?;
                (size, 0.0, 0, None)
            } else {
                let vf_args = shared_utils::get_ffmpeg_dimension_args(
                    detection.width,
                    detection.height,
                    false,
                );
                let input_path = Path::new(&detection.file_path);

                let flag_mode = shared_utils::validate_flags_result_with_ultimate(
                    config.explore_smaller,
                    config.match_quality,
                    config.require_compression,
                    config.ultimate_mode,
                )
                .map_err(VidQualityError::ConversionError)?;

                let use_gpu = config.use_gpu;
                if !use_gpu {
                    info!("   🖥️  CPU Mode: Using libx265 for higher SSIM (≥0.98)");
                }

                let ultimate = flag_mode.is_ultimate();
                let initial_crf = calculate_matched_crf(&detection);
                info!(
                    "   {} {}: CRF {:.1}",
                    if ultimate { "🔥" } else { "🔬" },
                    flag_mode.description_cn(),
                    initial_crf
                );
                let explore_result = shared_utils::explore_hevc_with_gpu_coarse_full(
                    input_path,
                    &output_path,
                    vf_args,
                    initial_crf,
                    ultimate,
                    config.force_ms_ssim_long,
                    config.child_threads,
                )
                .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

                for log_line in &explore_result.log {
                    info!("{}", log_line);
                }

                if !explore_result.quality_passed
                    && (config.match_quality || config.explore_smaller)
                {
                    let actual_ssim = explore_result.ssim.unwrap_or(0.0);
                    let threshold = explore_result.actual_min_ssim;
                    let video_stream_compressed = explore_result.output_video_stream_size
                        < explore_result.input_video_stream_size;
                    let total_file_compressed = explore_result.output_size < detection.file_size;

                    warn!(
                        "   📊 DEBUG: input_stream={} bytes, output_stream={} bytes, compressed={}",
                        explore_result.input_video_stream_size,
                        explore_result.output_video_stream_size,
                        video_stream_compressed
                    );

                    let (fail_reason, fail_message, protect_msg, delete_msg) = if !video_stream_compressed {
                        let input_b = explore_result.input_video_stream_size as f64;
                        let output_b = explore_result.output_video_stream_size as f64;
                        let stream_change_pct = if input_b > 0.0 {
                            (output_b / input_b - 1.0) * 100.0
                        } else {
                            0.0
                        };
                        // Use KB + 1 decimal for streams < 1 MB so displayed sizes match the percentage (0.07→0.08 MB rounded looked like +14%).
                        let msg = if input_b < 1024.0 * 1024.0 {
                            format!(
                                "   ⚠️  VIDEO STREAM COMPRESSION FAILED: {:.1} KB → {:.1} KB ({:+.1}%)",
                                input_b / 1024.0,
                                output_b / 1024.0,
                                stream_change_pct
                            )
                        } else {
                            format!(
                                "   ⚠️  VIDEO STREAM COMPRESSION FAILED: {:.3} MB → {:.3} MB ({:+.1}%)",
                                input_b / 1024.0 / 1024.0,
                                output_b / 1024.0 / 1024.0,
                                stream_change_pct
                            )
                        };
                        warn!("{}", msg);
                        if total_file_compressed {
                            warn!("   ⚠️  Total file smaller but video stream larger (audio/container overhead)");
                        } else {
                            warn!("   ⚠️  Total file and video stream both larger than original");
                        }
                        warn!("   ⚠️  File may already be highly optimized");
                        (
                            format!(
                                "Video stream compression failed: {:+.1}%",
                                stream_change_pct
                            ),
                            format!("Skipped: video stream larger ({:+.1}%)", stream_change_pct),
                            "Original file PROTECTED (output did not compress)".to_string(),
                            "Output discarded (video stream larger than original)".to_string(),
                        )
                    } else if explore_result.ssim.is_none() {
                        warn!("   ⚠️  SSIM CALCULATION FAILED - cannot validate quality!");
                        warn!("   ⚠️  This may indicate codec compatibility issues (VP8/VP9/alpha channel)");
                        (
                            "SSIM calculation failed".to_string(),
                            "Skipped: SSIM calculation failed".to_string(),
                            "Original file PROTECTED (SSIM not available)".to_string(),
                            "Output discarded (SSIM calculation failed)".to_string(),
                        )
                    } else if actual_ssim < threshold {
                        warn!(
                            "   ⚠️  Quality validation FAILED: SSIM {:.4} < {:.4}",
                            actual_ssim, threshold
                        );
                        (
                            format!(
                                "Quality validation failed: SSIM {:.4} < {:.4}",
                                actual_ssim, threshold
                            ),
                            format!(
                                "Skipped: SSIM {:.4} below threshold {:.4}",
                                actual_ssim, threshold
                            ),
                            "Original file PROTECTED (quality below threshold)".to_string(),
                            "Output discarded (quality below threshold)".to_string(),
                        )
                    } else {
                        warn!("   ⚠️  Quality validation FAILED: unknown reason");
                        (
                            "Quality validation failed: unknown reason".to_string(),
                            "Skipped: quality validation failed".to_string(),
                            "Original file PROTECTED (quality/size check failed)".to_string(),
                            "Output discarded (quality/size check failed)".to_string(),
                        )
                    };
                    warn!("   🛡️  {}", protect_msg);

                    if config.apple_compat {
                        warn!("   ⚠️  APPLE COMPAT FALLBACK (not full success): quality/size below target");
                        warn!(
                            "   Keeping best-effort output: last attempt CRF {:.1} ({} iterations), file is HEVC and importable",
                            explore_result.optimal_crf,
                            explore_result.iterations
                        );
                        return Ok(ConversionOutput {
                            input_path: input.display().to_string(),
                            output_path: output_path.display().to_string(),
                            strategy: ConversionStrategy {
                                target: TargetVideoFormat::HevcMp4,
                                reason: "Apple compat fallback: best-effort HEVC kept (quality/size below target)".to_string(),
                                command: String::new(),
                                preserve_audio: detection.has_audio,
                                crf: explore_result.optimal_crf,
                                lossless: false,
                            },
                            input_size: detection.file_size,
                            output_size: explore_result.output_size,
                            size_ratio: explore_result.output_size as f64 / detection.file_size as f64,
                            success: true,
                            message: format!(
                                "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); quality/size below target — file is HEVC and importable",
                                explore_result.optimal_crf,
                                explore_result.iterations
                            ),
                            final_crf: explore_result.optimal_crf,
                            exploration_attempts: explore_result.iterations as u8,
                        });
                    }

                    if output_path.exists() {
                        let _ = std::fs::remove_file(&output_path);
                        info!("   🗑️  {}", delete_msg);
                    }

                    let _ = shared_utils::copy_on_skip_or_fail(
                        input,
                        config.output_dir.as_deref(),
                        config.base_dir.as_deref(),
                        false,
                    );

                    return Ok(ConversionOutput {
                        input_path: input.display().to_string(),
                        output_path: input.display().to_string(),
                        strategy: ConversionStrategy {
                            target: TargetVideoFormat::Skip,
                            reason: fail_reason,
                            command: String::new(),
                            preserve_audio: detection.has_audio,
                            crf: explore_result.optimal_crf,
                            lossless: false,
                        },
                        input_size: detection.file_size,
                        output_size: detection.file_size,
                        size_ratio: 1.0,
                        success: false,
                        message: fail_message,
                        final_crf: explore_result.optimal_crf,
                        exploration_attempts: explore_result.iterations as u8,
                    });
                }

                (
                    explore_result.output_size,
                    explore_result.optimal_crf,
                    explore_result.iterations as u8,
                    Some(explore_result),
                )
            }
        }
        TargetVideoFormat::Skip => unreachable!(),
        _ => unreachable!("HEVC tool should not return AV1/FFV1 target"),
    };

    if let Some(ref result) = explore_result_opt {
        if let Some(false) = result.ms_ssim_passed {
            let ms_ssim_score = result.ms_ssim_score.unwrap_or(0.0);
            warn!("   ❌ MS-SSIM TARGET FAILED: {:.4} < 0.90", ms_ssim_score);
            warn!("   🛡️  Original file PROTECTED (MS-SSIM quality too low)");

            if config.apple_compat {
                warn!("   ⚠️  APPLE COMPAT FALLBACK (not full success): MS-SSIM below target");
                warn!(
                    "   Keeping best-effort output: last attempt CRF {:.1} ({} iterations), file is HEVC and importable",
                    result.optimal_crf,
                    result.iterations
                );
                return Ok(ConversionOutput {
                    input_path: input.display().to_string(),
                    output_path: output_path.display().to_string(),
                    strategy: ConversionStrategy {
                        target: TargetVideoFormat::HevcMp4,
                        reason: "Apple compat fallback: best-effort HEVC kept (MS-SSIM below target)".to_string(),
                        command: String::new(),
                        preserve_audio: detection.has_audio,
                        crf: result.optimal_crf,
                        lossless: false,
                    },
                    input_size: detection.file_size,
                    output_size: result.output_size,
                    size_ratio: result.output_size as f64 / detection.file_size as f64,
                    success: true,
                    message: format!(
                        "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); MS-SSIM {:.4} < 0.90 — file is HEVC and importable",
                        result.optimal_crf,
                        result.iterations,
                        ms_ssim_score
                    ),
                    final_crf: result.optimal_crf,
                    exploration_attempts: result.iterations as u8,
                });
            }

            if output_path.exists() {
                let _ = std::fs::remove_file(&output_path);
                info!("   🗑️  Low MS-SSIM output deleted");
            }

            let _ = shared_utils::copy_on_skip_or_fail(
                input,
                config.output_dir.as_deref(),
                config.base_dir.as_deref(),
                false,
            );

            return Ok(ConversionOutput {
                input_path: input.display().to_string(),
                output_path: input.display().to_string(),
                strategy: ConversionStrategy {
                    target: TargetVideoFormat::Skip,
                    reason: format!("MS-SSIM target failed: {:.4} < 0.90", ms_ssim_score),
                    command: String::new(),
                    preserve_audio: detection.has_audio,
                    crf: result.optimal_crf,
                    lossless: false,
                },
                input_size: detection.file_size,
                output_size: detection.file_size,
                size_ratio: 1.0,
                success: false,
                message: format!("Skipped: MS-SSIM {:.4} below target 0.90", ms_ssim_score),
                final_crf: result.optimal_crf,
                exploration_attempts: result.iterations as u8,
            });
        }
    }

    let pre_metadata_size = output_size;

    shared_utils::copy_metadata(input, &output_path);

    let actual_output_size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(output_size);

    let metadata_delta =
        shared_utils::video_explorer::detect_metadata_size(pre_metadata_size, actual_output_size);

    let input_stream_info = shared_utils::extract_stream_sizes(input);
    let output_stream_info = shared_utils::extract_stream_sizes(&output_path);

    let verify_result =
        shared_utils::verify_pure_media_compression(&input_stream_info, &output_stream_info);

    if metadata_delta > 0 || output_stream_info.container_overhead > 10000 {
        info!("   📋 Metadata: +{} bytes", metadata_delta);
        info!(
            "   📦 Container overhead: {} bytes ({:.1}%)",
            output_stream_info.container_overhead,
            output_stream_info.container_overhead_percent()
        );
    }

    info!(
        "   🎬 Video stream: {} → {} ({:+.1}%)",
        shared_utils::format_bytes(input_stream_info.video_stream_size),
        shared_utils::format_bytes(output_stream_info.video_stream_size),
        verify_result.video_size_change_percent()
    );

    let can_compress = if config.allow_size_tolerance {
        verify_result.video_compression_ratio < 1.01
    } else {
        verify_result.video_compressed
    };

    if config.require_compression && !can_compress {
        warn!("   ⚠️  COMPRESSION FAILED (pure video stream comparison):");
        warn!(
            "   ⚠️  Video stream: {} bytes >= {} bytes",
            output_stream_info.video_stream_size, input_stream_info.video_stream_size
        );
        if verify_result.is_container_overhead_issue() {
            warn!("   ⚠️  Note: Container overhead caused total file to be larger");
        }
        warn!("   🛡️  Original file PROTECTED");

        if config.apple_compat {
            warn!("   ⚠️  APPLE COMPAT FALLBACK (not full success): compression check failed (video stream not smaller)");
            warn!(
                "   Keeping best-effort output: last attempt CRF {:.1} ({} iterations), file is HEVC and importable",
                final_crf,
                attempts
            );
            return Ok(ConversionOutput {
                input_path: input.display().to_string(),
                output_path: output_path.display().to_string(),
                strategy: ConversionStrategy {
                    target: TargetVideoFormat::HevcMp4,
                    reason: "Apple compat fallback: best-effort HEVC kept (compression check failed)".to_string(),
                    command: String::new(),
                    preserve_audio: detection.has_audio,
                    crf: final_crf,
                    lossless: false,
                },
                input_size: detection.file_size,
                output_size: actual_output_size,
                size_ratio: actual_output_size as f64 / detection.file_size as f64,
                success: true,
                message: format!(
                    "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); compression check failed — file is HEVC and importable",
                    final_crf,
                    attempts
                ),
                final_crf,
                exploration_attempts: attempts,
            });
        }

        if output_path.exists() {
            let _ = std::fs::remove_file(&output_path);
            info!("   🗑️  Output deleted (cannot compress)");
        }

        let _ = shared_utils::copy_on_skip_or_fail(
            input,
            config.output_dir.as_deref(),
            config.base_dir.as_deref(),
            false,
        );

        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: input.display().to_string(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: format!(
                    "Compression failed: video stream {} >= {}",
                    output_stream_info.video_stream_size, input_stream_info.video_stream_size
                ),
                command: String::new(),
                preserve_audio: detection.has_audio,
                crf: final_crf,
                lossless: false,
            },
            input_size: detection.file_size,
            output_size: detection.file_size,
            size_ratio: 1.0,
            success: false,
            message: format!(
                "Skipped: video stream {} >= {} (container overhead: {})",
                output_stream_info.video_stream_size,
                input_stream_info.video_stream_size,
                output_stream_info.container_overhead
            ),
            final_crf,
            exploration_attempts: attempts,
        });
    }

    if verify_result.video_compressed && verify_result.total_compression_ratio >= 1.0 {
        warn!(
            "   ⚠️  Video stream compressed ({:+.1}%) but total file larger ({:+.1}%)",
            verify_result.video_size_change_percent(),
            verify_result.total_size_change_percent()
        );
        warn!(
            "   ⚠️  Cause: Container overhead (+{} bytes)",
            verify_result.container_overhead_diff
        );
        info!("   ✅ Keeping output (video stream is smaller)");
    }

    let output_size = actual_output_size;
    let size_ratio = output_size as f64 / detection.file_size as f64;

    if config.should_delete_original() {
        if let Err(e) = shared_utils::conversion::safe_delete_original(input, &output_path, 1000) {
            warn!("   ⚠️  Safe delete failed: {}", e);
        } else {
            info!("   🗑️  Original deleted (integrity verified)");
        }
    }

    info!("   ✅ Complete: {:.1}% of original", size_ratio * 100.0);

    Ok(ConversionOutput {
        input_path: input.display().to_string(),
        output_path: output_path.display().to_string(),
        strategy: ConversionStrategy {
            target: strategy.target,
            reason: strategy.reason,
            command: String::new(),
            preserve_audio: detection.has_audio,
            crf: final_crf,
            lossless: strategy.lossless,
        },
        input_size: detection.file_size,
        output_size,
        size_ratio,
        success: true,
        message: if attempts > 0 {
            format!("Explored {} CRF values, final CRF: {}", attempts, final_crf)
        } else {
            "Conversion successful".to_string()
        },
        final_crf,
        exploration_attempts: attempts,
    })
}

pub fn calculate_matched_crf(detection: &VideoDetectionResult) -> f32 {
    let mut builder = shared_utils::VideoAnalysisBuilder::new()
        .basic(
            detection.codec.as_str(),
            detection.width,
            detection.height,
            detection.fps,
            detection.duration_secs,
        )
        .bit_depth(detection.bit_depth)
        .file_size(detection.file_size);

    if let Some(vbr) = detection.video_bitrate {
        builder = builder.video_bitrate(vbr);
    } else {
        builder = builder.video_bitrate(detection.bitrate);
    }

    if !detection.pix_fmt.is_empty() {
        builder = builder.pix_fmt(&detection.pix_fmt);
    }

    let (color_space_str, is_hdr) = match &detection.color_space {
        crate::detection_api::ColorSpace::BT709 => ("bt709", false),
        crate::detection_api::ColorSpace::BT2020 => ("bt2020nc", true),
        crate::detection_api::ColorSpace::SRGB => ("srgb", false),
        crate::detection_api::ColorSpace::AdobeRGB => ("adobergb", false),
        crate::detection_api::ColorSpace::Unknown(_) => ("", false),
    };
    if !color_space_str.is_empty() {
        builder = builder.color(color_space_str, is_hdr);
    }

    if detection.has_b_frames {
        builder = builder.gop(60, 2);
    }

    let analysis = builder.build();

    match shared_utils::calculate_hevc_crf(&analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(&analysis, &result, shared_utils::EncoderType::Hevc);
            result.crf
        }
        Err(e) => {
            warn!("   ⚠️  Quality analysis failed: {}", e);
            warn!("   ⚠️  Using conservative CRF 23.0");
            23.0
        }
    }
}


fn execute_hevc_conversion(
    detection: &VideoDetectionResult,
    output: &Path,
    crf: u8,
    max_threads: usize,
) -> Result<u64> {
    let x265_params = format!("log-level=error:pools={}", max_threads);

    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);

    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(),
        max_threads.to_string(),
        "-i".to_string(),
        detection.file_path.clone(),
        "-c:v".to_string(),
        "libx265".to_string(),
        "-crf".to_string(),
        crf.to_string(),
        "-preset".to_string(),
        "medium".to_string(),
        "-tag:v".to_string(),
        "hvc1".to_string(),
        "-x265-params".to_string(),
        x265_params,
    ];

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.has_audio {
        args.extend(vec![
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            "320k".to_string(),
        ]);
    } else {
        args.push("-an".to_string());
    }

    args.push(output.display().to_string());

    let result = Command::new("ffmpeg").args(&args).output()?;

    if !result.status.success() {
        return Err(VidQualityError::FFmpegError(
            String::from_utf8_lossy(&result.stderr).to_string(),
        ));
    }

    Ok(std::fs::metadata(output)?.len())
}

fn execute_hevc_lossless(
    detection: &VideoDetectionResult,
    output: &Path,
    max_threads: usize,
) -> Result<u64> {
    warn!("⚠️  HEVC Lossless encoding - this will be slow and produce large files!");

    let x265_params = format!("lossless=1:log-level=error:pools={}", max_threads);

    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);

    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(),
        max_threads.to_string(),
        "-i".to_string(),
        detection.file_path.clone(),
        "-c:v".to_string(),
        "libx265".to_string(),
        "-x265-params".to_string(),
        x265_params,
        "-preset".to_string(),
        "medium".to_string(),
        "-tag:v".to_string(),
        "hvc1".to_string(),
    ];

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.has_audio {
        args.extend(vec!["-c:a".to_string(), "flac".to_string()]);
    } else {
        args.push("-an".to_string());
    }

    args.push(output.display().to_string());

    let result = Command::new("ffmpeg").args(&args).output()?;

    if !result.status.success() {
        return Err(VidQualityError::FFmpegError(
            String::from_utf8_lossy(&result.stderr).to_string(),
        ));
    }

    Ok(std::fs::metadata(output)?.len())
}


pub fn smart_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    auto_convert(input, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_format() {
        assert_eq!(TargetVideoFormat::HevcLosslessMkv.extension(), "mkv");
        assert_eq!(TargetVideoFormat::HevcMp4.extension(), "mp4");
    }


    #[test]
    fn test_config_default_apple_compat() {
        let config = ConversionConfig::default();
        assert!(!config.apple_compat, "Default apple_compat should be false");
    }

    #[test]
    fn test_strategy_normal_mode_skips_vp9() {
        let detection = crate::detection_api::VideoDetectionResult {
            file_path: "/test/video.webm".to_string(),
            format: "webm".to_string(),
            codec: crate::detection_api::DetectedCodec::VP9,
            codec_long: "Google VP9".to_string(),
            compression: crate::detection_api::CompressionType::Standard,
            width: 1920,
            height: 1080,
            frame_count: 1800,
            fps: 30.0,
            duration_secs: 60.0,
            bit_depth: 8,
            pix_fmt: "yuv420p".to_string(),
            file_size: 50_000_000,
            bitrate: 6_666_666,
            has_audio: true,
            audio_codec: Some("opus".to_string()),
            quality_score: 75,
            archival_candidate: false,
            color_space: crate::detection_api::ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.1,
        };

        let strategy = determine_strategy(&detection);
        assert_eq!(
            strategy.target,
            TargetVideoFormat::Skip,
            "VP9 should be skipped in normal mode"
        );
        assert!(
            strategy.reason.contains("VP9"),
            "Skip reason should mention VP9"
        );
    }

    #[test]
    fn test_strategy_apple_compat_converts_vp9() {
        let detection = crate::detection_api::VideoDetectionResult {
            file_path: "/test/video.webm".to_string(),
            format: "webm".to_string(),
            codec: crate::detection_api::DetectedCodec::VP9,
            codec_long: "Google VP9".to_string(),
            compression: crate::detection_api::CompressionType::Standard,
            width: 1920,
            height: 1080,
            frame_count: 1800,
            fps: 30.0,
            duration_secs: 60.0,
            bit_depth: 8,
            pix_fmt: "yuv420p".to_string(),
            file_size: 50_000_000,
            bitrate: 6_666_666,
            has_audio: true,
            audio_codec: Some("opus".to_string()),
            quality_score: 75,
            archival_candidate: false,
            color_space: crate::detection_api::ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.1,
        };

        let strategy = determine_strategy_with_apple_compat(&detection, true);
        assert_ne!(
            strategy.target,
            TargetVideoFormat::Skip,
            "VP9 should NOT be skipped in Apple compat mode"
        );
        assert_eq!(
            strategy.target,
            TargetVideoFormat::HevcMp4,
            "VP9 should be converted to HEVC MP4 in Apple compat mode"
        );
    }

    #[test]
    fn test_strategy_hevc_skipped_both_modes() {
        let detection = crate::detection_api::VideoDetectionResult {
            file_path: "/test/video.mp4".to_string(),
            format: "mp4".to_string(),
            codec: crate::detection_api::DetectedCodec::H265,
            codec_long: "HEVC".to_string(),
            compression: crate::detection_api::CompressionType::Standard,
            width: 1920,
            height: 1080,
            frame_count: 1800,
            fps: 30.0,
            duration_secs: 60.0,
            bit_depth: 8,
            pix_fmt: "yuv420p".to_string(),
            file_size: 50_000_000,
            bitrate: 6_666_666,
            has_audio: true,
            audio_codec: Some("aac".to_string()),
            quality_score: 80,
            archival_candidate: false,
            color_space: crate::detection_api::ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.1,
        };

        let normal = determine_strategy(&detection);
        assert_eq!(
            normal.target,
            TargetVideoFormat::Skip,
            "HEVC should be skipped in normal mode"
        );

        let apple = determine_strategy_with_apple_compat(&detection, true);
        assert_eq!(
            apple.target,
            TargetVideoFormat::Skip,
            "HEVC should be skipped in Apple compat mode too"
        );
    }

    #[test]
    fn test_strategy_h264_converted_both_modes() {
        let detection = crate::detection_api::VideoDetectionResult {
            file_path: "/test/video.mp4".to_string(),
            format: "mp4".to_string(),
            codec: crate::detection_api::DetectedCodec::H264,
            codec_long: "H.264/AVC".to_string(),
            compression: crate::detection_api::CompressionType::Standard,
            width: 1920,
            height: 1080,
            frame_count: 1800,
            fps: 30.0,
            duration_secs: 60.0,
            bit_depth: 8,
            pix_fmt: "yuv420p".to_string(),
            file_size: 50_000_000,
            bitrate: 6_666_666,
            has_audio: true,
            audio_codec: Some("aac".to_string()),
            quality_score: 70,
            archival_candidate: false,
            color_space: crate::detection_api::ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.1,
        };

        let normal = determine_strategy(&detection);
        assert_ne!(
            normal.target,
            TargetVideoFormat::Skip,
            "H.264 should NOT be skipped in normal mode"
        );

        let apple = determine_strategy_with_apple_compat(&detection, true);
        assert_ne!(
            apple.target,
            TargetVideoFormat::Skip,
            "H.264 should NOT be skipped in Apple compat mode"
        );
    }

    #[test]
    fn test_strict_apple_compat_routing() {
        use crate::detection_api::{ColorSpace, CompressionType, DetectedCodec};

        let make_detection = |codec: DetectedCodec| -> crate::detection_api::VideoDetectionResult {
            crate::detection_api::VideoDetectionResult {
                file_path: "/test/video.mp4".to_string(),
                format: "mp4".to_string(),
                codec,
                codec_long: "Test".to_string(),
                compression: CompressionType::Standard,
                width: 1920,
                height: 1080,
                frame_count: 1800,
                fps: 30.0,
                duration_secs: 60.0,
                bit_depth: 8,
                pix_fmt: "yuv420p".to_string(),
                file_size: 50_000_000,
                bitrate: 6_666_666,
                has_audio: false,
                audio_codec: None,
                quality_score: 70,
                archival_candidate: false,
                color_space: ColorSpace::BT709,
                video_bitrate: Some(6_000_000),
                has_b_frames: true,
                profile: None,
                bits_per_pixel: 0.1,
            }
        };

        let test_cases = [
            (DetectedCodec::H264, false, false),
            (DetectedCodec::H265, true, true),
            (DetectedCodec::VP9, true, false),
            (DetectedCodec::AV1, true, false),
        ];

        for (codec, expected_skip_normal, expected_skip_apple) in test_cases {
            let detection = make_detection(codec.clone());

            let normal = determine_strategy(&detection);
            let apple = determine_strategy_with_apple_compat(&detection, true);

            let is_skip_normal = normal.target == TargetVideoFormat::Skip;
            let is_skip_apple = apple.target == TargetVideoFormat::Skip;

            assert_eq!(
                is_skip_normal, expected_skip_normal,
                "STRICT: {:?} normal mode: expected skip={}, got skip={}",
                codec, expected_skip_normal, is_skip_normal
            );

            assert_eq!(
                is_skip_apple, expected_skip_apple,
                "STRICT: {:?} Apple compat mode: expected skip={}, got skip={}",
                codec, expected_skip_apple, is_skip_apple
            );
        }
    }


    #[test]
    fn test_apple_compat_av1_to_hevc() {
        use crate::detection_api::{ColorSpace, CompressionType, DetectedCodec};
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "/t.mp4".into(),
            format: "mp4".into(),
            codec: DetectedCodec::AV1,
            codec_long: "AV1".into(),
            compression: CompressionType::Standard,
            width: 1920,
            height: 1080,
            frame_count: 1800,
            fps: 30.0,
            duration_secs: 60.0,
            bit_depth: 8,
            pix_fmt: "yuv420p".into(),
            file_size: 50_000_000,
            bitrate: 6_666_666,
            has_audio: true,
            audio_codec: Some("opus".into()),
            quality_score: 85,
            archival_candidate: false,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.1,
        };
        let s = determine_strategy_with_apple_compat(&det, true);
        assert_eq!(s.target, TargetVideoFormat::HevcMp4);
        assert!(!s.lossless);
    }

    #[test]
    fn test_apple_compat_vvc_to_hevc() {
        use crate::detection_api::{ColorSpace, CompressionType, DetectedCodec};
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "/t.mp4".into(),
            format: "mp4".into(),
            codec: DetectedCodec::VVC,
            codec_long: "VVC".into(),
            compression: CompressionType::Standard,
            width: 3840,
            height: 2160,
            frame_count: 3600,
            fps: 60.0,
            duration_secs: 60.0,
            bit_depth: 10,
            pix_fmt: "yuv420p10le".into(),
            file_size: 100_000_000,
            bitrate: 13_333_333,
            has_audio: true,
            audio_codec: Some("aac".into()),
            quality_score: 90,
            archival_candidate: false,
            color_space: ColorSpace::BT2020,
            video_bitrate: Some(12_000_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.04,
        };
        let s = determine_strategy_with_apple_compat(&det, true);
        assert_ne!(
            s.target,
            TargetVideoFormat::Skip,
            "VVC should convert in Apple compat mode"
        );
    }


    #[test]
    fn test_apple_compat_crf_precision_vp9() {
        use crate::detection_api::{ColorSpace, CompressionType, DetectedCodec};
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "/t.webm".into(),
            format: "webm".into(),
            codec: DetectedCodec::VP9,
            codec_long: "VP9".into(),
            compression: CompressionType::Standard,
            width: 1920,
            height: 1080,
            frame_count: 1800,
            fps: 30.0,
            duration_secs: 60.0,
            bit_depth: 8,
            pix_fmt: "yuv420p".into(),
            file_size: 50_000_000,
            bitrate: 6_666_666,
            has_audio: false,
            audio_codec: None,
            quality_score: 75,
            archival_candidate: false,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.1,
        };
        let crf = calculate_matched_crf(&det);
        assert!(
            (0.0..=35.0).contains(&crf),
            "CRF {:.1} should be in [0, 35]",
            crf
        );
        assert!(
            (18.0..=28.0).contains(&crf),
            "CRF {:.1} should be ~18-28 for 6Mbps 1080p",
            crf
        );
    }

    #[test]
    fn test_apple_compat_crf_precision_av1_high_bitrate() {
        use crate::detection_api::{ColorSpace, CompressionType, DetectedCodec};
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "/t.mp4".into(),
            format: "mp4".into(),
            codec: DetectedCodec::AV1,
            codec_long: "AV1".into(),
            compression: CompressionType::VisuallyLossless,
            width: 3840,
            height: 2160,
            frame_count: 3600,
            fps: 60.0,
            duration_secs: 60.0,
            bit_depth: 10,
            pix_fmt: "yuv420p10le".into(),
            file_size: 500_000_000,
            bitrate: 66_666_666,
            has_audio: true,
            audio_codec: Some("opus".into()),
            quality_score: 95,
            archival_candidate: true,
            color_space: ColorSpace::BT2020,
            video_bitrate: Some(60_000_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.15,
        };
        let crf = calculate_matched_crf(&det);
        assert!(
            (0.0..=22.0).contains(&crf),
            "High bitrate AV1 should get CRF <= 22, got {:.1}",
            crf
        );
    }


    #[test]
    fn test_apple_compat_lossless_source() {
        use crate::detection_api::{ColorSpace, CompressionType, DetectedCodec};
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "/t.mkv".into(),
            format: "mkv".into(),
            codec: DetectedCodec::FFV1,
            codec_long: "FFV1".into(),
            compression: CompressionType::Lossless,
            width: 1920,
            height: 1080,
            frame_count: 900,
            fps: 30.0,
            duration_secs: 30.0,
            bit_depth: 10,
            pix_fmt: "yuv444p10le".into(),
            file_size: 2_000_000_000,
            bitrate: 533_333_333,
            has_audio: false,
            audio_codec: None,
            quality_score: 100,
            archival_candidate: true,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(533_333_333),
            has_b_frames: false,
            profile: None,
            bits_per_pixel: 8.5,
        };
        let s = determine_strategy_with_apple_compat(&det, true);
        assert_eq!(
            s.target,
            TargetVideoFormat::HevcLosslessMkv,
            "Lossless source should use HEVC Lossless"
        );
        assert!(s.lossless);
    }

    #[test]
    fn test_apple_compat_visually_lossless() {
        use crate::detection_api::{ColorSpace, CompressionType, DetectedCodec};
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "/t.mov".into(),
            format: "mov".into(),
            codec: DetectedCodec::ProRes,
            codec_long: "ProRes".into(),
            compression: CompressionType::VisuallyLossless,
            width: 1920,
            height: 1080,
            frame_count: 1800,
            fps: 30.0,
            duration_secs: 60.0,
            bit_depth: 10,
            pix_fmt: "yuv422p10le".into(),
            file_size: 1_000_000_000,
            bitrate: 133_333_333,
            has_audio: true,
            audio_codec: Some("pcm_s24le".into()),
            quality_score: 98,
            archival_candidate: true,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(130_000_000),
            has_b_frames: false,
            profile: None,
            bits_per_pixel: 2.1,
        };
        let s = determine_strategy_with_apple_compat(&det, true);
        assert_eq!(s.target, TargetVideoFormat::HevcMp4);
        assert!(
            (s.crf - 18.0).abs() < 0.1,
            "Visually lossless should use CRF 18, got {:.1}",
            s.crf
        );
    }

    #[test]
    fn test_apple_compat_unknown_codec_parsing() {
        use crate::detection_api::{ColorSpace, CompressionType, DetectedCodec};
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "/t.webm".into(),
            format: "webm".into(),
            codec: DetectedCodec::Unknown("vp9".into()),
            codec_long: "VP9".into(),
            compression: CompressionType::Standard,
            width: 1280,
            height: 720,
            frame_count: 900,
            fps: 30.0,
            duration_secs: 30.0,
            bit_depth: 8,
            pix_fmt: "yuv420p".into(),
            file_size: 10_000_000,
            bitrate: 2_666_666,
            has_audio: false,
            audio_codec: None,
            quality_score: 70,
            archival_candidate: false,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(2_500_000),
            has_b_frames: true,
            profile: None,
            bits_per_pixel: 0.09,
        };
        let normal = determine_strategy(&det);
        assert_eq!(normal.target, TargetVideoFormat::Skip);
        let apple = determine_strategy_with_apple_compat(&det, true);
        assert_ne!(apple.target, TargetVideoFormat::Skip);
    }
}
