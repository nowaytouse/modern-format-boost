//! Video Conversion API Module
//!
//! Pure conversion layer - executes video conversions based on detection results.
//! - Auto Mode: FFV1 for lossless sources, AV1 for lossy sources
//! - Simple Mode: Always AV1 MP4
//! - Size Exploration: Tries higher CRF if output is larger than input

use crate::detection_api::{detect_video, CompressionType, VideoDetectionResult};
use crate::{Result, VidQualityError};

use shared_utils::analysis_cache::AnalysisCache;
use shared_utils::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, TargetVideoFormat,
};
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

/// Build the FFmpeg colour/HDR arguments that must be forwarded to every AV1 encode.
///
/// This preserves:
/// - color_primaries (e.g. bt2020)
/// - color_trc / color_transfer (e.g. smpte2084 for PQ, arib-std-b67 for HLG)
/// - colorspace (e.g. bt2020nc)
/// - mastering_display (HDR10 static mastering display metadata)
/// - max_cll (HDR10 content light level MaxCLL/MaxFALL)
///
/// Dolby Vision and HDR10+ layers are not currently preserved by libsvtav1 metadata pass-through.
fn build_hdr_ffmpeg_args(detection: &VideoDetectionResult) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    // -color_primaries
    if let Some(ref cp) = detection.color_primaries {
        if !cp.is_empty() && cp != "unknown" {
            args.push("-color_primaries".to_string());
            args.push(cp.clone());
        }
    }

    // -color_trc (transfer characteristics)
    if let Some(ref trc) = detection.color_transfer {
        if !trc.is_empty() && trc != "unknown" {
            args.push("-color_trc".to_string());
            args.push(trc.clone());
        }
    }

    // -colorspace (matrix coefficients)
    let cs_str = match &detection.color_space {
        crate::detection_api::ColorSpace::BT2020 => Some("bt2020nc"),
        crate::detection_api::ColorSpace::BT709  => Some("bt709"),
        crate::detection_api::ColorSpace::Unknown(s) if !s.is_empty() && s != "unknown" => {
            None
        }
        _ => None,
    };
    if let Some(cs) = cs_str {
        args.push("-colorspace".to_string());
        args.push(cs.to_string());
    } else if let crate::detection_api::ColorSpace::Unknown(ref s) = detection.color_space {
        let is_rgb_colorspace = s == "gbr" || s == "rgb" || s == "gbrp";
        if !s.is_empty() && s != "unknown" && !is_rgb_colorspace {
            args.push("-colorspace".to_string());
            args.push(s.clone());
        }
    }

    // -master_display (HDR10 mastering display metadata)
    if let Some(ref md) = detection.mastering_display {
        if !md.is_empty() {
            args.push("-master_display".to_string());
            args.push(md.clone());
        }
    }

    // -max_cll MaxCLL,MaxFALL (HDR10 content light level)
    if let Some(ref cll) = detection.max_cll {
        if !cll.is_empty() {
            args.push("-max_cll".to_string());
            args.push(cll.clone());
        }
    }

    args
}

/// Return the correct pixel format for encoding:
/// - If source is 10-bit (yuv420p10le, yuv422p10le, etc.) use yuv420p10le so that
///   the HDR signal range / precision is preserved in the output stream.
/// - Otherwise default to yuv420p (8-bit SDR).
fn hdr_pix_fmt(detection: &VideoDetectionResult) -> &'static str {
    if detection.bit_depth >= 10 {
        "yuv420p10le"
    } else {
        "yuv420p"
    }
}

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
        let unknown_skip = shared_utils::should_skip_video_codec(s);
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
            TargetVideoFormat::Av1Mp4,
            format!(
                "Source is {} (lossless) - converting to AV1 Lossless",
                result.codec.as_str()
            ),
            0.0,
            true,
        ),
        CompressionType::VisuallyLossless => (
            TargetVideoFormat::Av1Mp4,
            format!(
                "Source is {} (visually lossless) - compressing with AV1 CRF 0",
                result.codec.as_str()
            ),
            0.0,
            false,
        ),
        _ => (
            TargetVideoFormat::Av1Mp4,
            format!(
                "Source is {} ({}) - compressing with AV1 CRF 0",
                result.codec.as_str(),
                result.compression.as_str()
            ),
            0.0,
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
        output_dir.join(format!("{}_av1.mp4", stem))
    } else {
        output_dir.join(format!("{}.mp4", stem))
    };

    info!("🎬 Simple Mode: {} → AV1 MP4 (LOSSLESS)", input.display());

    let thread_config = shared_utils::thread_manager::get_balanced_thread_config(
        shared_utils::thread_manager::WorkloadType::Video,
    );
    let temp_path = shared_utils::conversion::temp_path_for_output(&output_path);
    let _temp_guard = shared_utils::conversion::TempOutputGuard::new(temp_path.clone());
    let output_size = execute_av1_lossless(&detection, &temp_path, thread_config.child_threads)?;

    if !shared_utils::conversion::commit_temp_to_output(&temp_path, &output_path, true)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?
    {
        return Err(VidQualityError::ConversionError("Failed to commit temporary file to output".to_string()));
    }

    shared_utils::copy_metadata(input, &output_path);

    let size_ratio = output_size as f64 / detection.file_size as f64;

    info!("   ✅ Complete: {:.1}% of original", size_ratio * 100.0);

    Ok(ConversionOutput {
        input_path: input.display().to_string(),
        output_path: output_path.display().to_string(),
        strategy: ConversionStrategy {
            target: TargetVideoFormat::Av1Mp4,
            reason: "Simple mode: Always AV1 Lossless".to_string(),
            command: String::new(),
            preserve_audio: detection.has_audio,
            crf: 0.0,
            lossless: true,
        },
        input_size: detection.file_size,
        output_size,
        size_ratio,
        success: true,
        message: "Simple conversion successful (Lossless)".to_string(),
        final_crf: 0.0,
        exploration_attempts: 0,
    })
}

pub fn auto_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    auto_convert_with_cache(input, config, None)
}

pub fn auto_convert_with_cache(
    input: &Path,
    config: &ConversionConfig,
    cache: Option<&AnalysisCache>,
) -> Result<ConversionOutput> {
    // Pause if the user is being prompted to exit via Ctrl+C
    shared_utils::ctrlc_guard::wait_if_prompt_active();

    // Skip Live Photos in Apple compat mode
    if config.apple_compat && shared_utils::is_live_photo(input) {
        info!("🎬 Auto Mode: {} → SKIP (Live Photo)", input.display());
        info!("   Reason: Live Photo detected in Apple compat mode");

        let file_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);

        shared_utils::copy_on_skip_or_fail(
            input,
            config.output_dir.as_deref(),
            config.base_dir.as_deref(),
            false,
        )
        .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: "".to_string(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: "Live Photo detected in Apple compat mode".to_string(),
                command: "".to_string(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            },
            input_size: file_size,
            output_size: 0,
            size_ratio: 0.0,
            success: true,
            message: "Skipped Live Photo in Apple compat mode".to_string(),
            final_crf: 0.0,
            exploration_attempts: 0,
        });
    }

    let detection = detect_video(input)?;

    // Warn about dynamic HDR metadata that will be stripped during re-encode
    if detection.is_dolby_vision {
        warn!("Dolby Vision detected: Metadata will be stripped to HDR10 static layer");
        warn!("(SVT-AV1 does not support Dolby Vision RPU pass-through)");
    }
    if detection.is_hdr10_plus {
        warn!("HDR10+ detected: dynamic metadata will be stripped to HDR10 static layer");
    }

    let mut detection = detection;
    let mut explore_result_opt: Option<shared_utils::ExploreResult> = None;

    let strategy = determine_strategy_with_apple_compat(&detection, config.apple_compat);

    if strategy.target == TargetVideoFormat::Skip {
        info!("🎬 Auto Mode: {} → SKIP", input.display());
        info!("   Reason: {}", strategy.reason);

        if let Some(ref out_dir) = config.output_dir {
            shared_utils::copy_on_skip_or_fail(
                input,
                Some(out_dir),
                config.base_dir.as_deref(),
                false,
            )
            .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;
        }

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

    let output_dir = config
        .output_dir
        .clone()
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());

    std::fs::create_dir_all(&output_dir)?;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let target_ext = strategy.target.extension();
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    // GIF as source has no Apple compatibility issue; do not show "APPLE COMPAT FALLBACK" for GIF→video.
    let source_is_gif = input_ext.eq_ignore_ascii_case("gif");

    let output_path = if input_ext.eq_ignore_ascii_case(target_ext) {
        output_dir.join(format!("{}_av1.{}", stem, target_ext))
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

    let temp_path = shared_utils::conversion::temp_path_for_output(&output_path);
    let _temp_guard = shared_utils::conversion::TempOutputGuard::new(temp_path.clone());
    info!(
        "🎬 Auto Mode: {} → {}",
        input.display(),
        strategy.target.as_str()
    );
    info!("   Reason: {}", strategy.reason);

    let (output_size, final_crf, attempts) = match strategy.target {
        TargetVideoFormat::Ffv1Mkv => {
            let size =
                execute_ffv1_conversion(&detection, &temp_path, config.child_threads)?;
            (size, 0.0, 0)
        }
        TargetVideoFormat::Av1Mp4 => {
            if strategy.lossless || config.use_lossless {
                if config.use_lossless && !strategy.lossless {
                    info!("   🚀 Using AV1 Mathematical Lossless Mode (forced)");
                } else {
                    info!("   🚀 Using AV1 Mathematical Lossless Mode");
                }
                let size =
                    execute_av1_lossless(&detection, &temp_path, config.child_threads)?;
                (size, 0.0, 0)
            } else {
                let vf_args = shared_utils::get_ffmpeg_dimension_args(
                    detection.width,
                    detection.height,
                    false,
                );
                let input_path = Path::new(&detection.file_path);

                // Log media info to log file only (for SSIM/quality context); not shown on terminal.
                if let Ok(quality_analysis) =
                    shared_utils::analyze_video_quality_from_detection(&detection)
                {
                    shared_utils::log_media_info_for_quality(&quality_analysis, input_path);
                }

                let flag_mode = shared_utils::validate_flags_result_with_ultimate(
                    config.explore_smaller,
                    config.match_quality,
                    config.require_compression,
                    config.ultimate_mode,
                )
                .map_err(VidQualityError::ConversionError)?;

                let use_gpu = config.use_gpu;
                if !use_gpu {
                    info!("   🖥️  CPU Mode: Using libaom for maximum SSIM (≥0.98)");
                }

                let ultimate = flag_mode.is_ultimate();
                
                // 🚀 CRF Hint Logic: Use cached best CRF if available for "warm start"
                let initial_crf = if let Some(hint) = detection.precision.last_best_crf {
                    info!("   💡 Using cached CRF hint: {:.1} (warm start)", hint);
                    hint
                } else if let Some(hint) = shared_utils::crf_constants::get_global_last_hit_crf_av1() {
                    info!("   💡 Using global last hit CRF: {:.1} (warm start)", hint);
                    hint
                } else {
                    calculate_matched_crf(&detection)? as f32
                };
                info!(
                    "   {} {}: CRF {:.1}",
                    if ultimate { "🔥" } else { "🔬" },
                    flag_mode.description_en(),
                    initial_crf
                );
                let explore_result = if ultimate {
                    shared_utils::explore_av1_with_gpu_coarse_ultimate(
                        input_path,
                        &temp_path,
                        vf_args,
                        initial_crf as f32,
                        ultimate,
                        config.allow_size_tolerance,
                        config.child_threads,
                    )
                } else {
                    shared_utils::explore_av1_with_gpu_coarse_full(
                        input_path,
                        &temp_path,
                        vf_args,
                        initial_crf as f32,
                        ultimate,
                        config.force_ms_ssim_long,
                        config.allow_size_tolerance,
                        config.min_ssim,
                        config.child_threads,
                    )
                }
                .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

                explore_result_opt = Some(explore_result.clone());

                for log_line in &explore_result.log {
                    info!("{}", log_line);
                }

                // --- Explore phase: quality/SSIM or size did not meet target; decide whether to keep or discard output. ---
                if !explore_result.quality_passed
                    && (config.match_quality || config.explore_smaller)
                {
                    let actual_ssim = match explore_result.ssim {
                        Some(s) => s,
                        None => {
                            warn!("   ⚠️  SSIM not measured, cannot verify quality");
                            return Err(VidQualityError::GeneralError(
                                "Quality verification failed: SSIM not measured".to_string()
                            ));
                        }
                    };
                    let threshold = explore_result.actual_min_ssim;
                    let video_stream_compressed = explore_result.output_video_stream_size
                        < explore_result.input_video_stream_size ||
                        (config.allow_size_tolerance && 
                         (explore_result.output_video_stream_size as i64 - explore_result.input_video_stream_size as i64) < 1024 * 1024);
                    let total_file_compressed = explore_result.output_size < detection.file_size;
                    let _total_size_ratio = if detection.file_size > 0 {
                        explore_result.output_size as f64 / detection.file_size as f64
                    } else {
                        1.0
                    };

                    warn!(
                        "   📊 DEBUG: input_stream={} bytes, output_stream={} bytes, compressed={}",
                        explore_result.input_video_stream_size,
                        explore_result.output_video_stream_size,
                        video_stream_compressed
                    );

                    let (fail_reason, fail_message, protect_msg, delete_msg) =
                        if !video_stream_compressed {
                            let input_b = explore_result.input_video_stream_size as f64;
                            let output_b = explore_result.output_video_stream_size as f64;
                            let stream_change_pct = if input_b > 0.0 {
                                (output_b / input_b - 1.0) * 100.0
                            } else {
                                0.0
                            };
                            
                            use shared_utils::modern_ui::colors::*;
                            
                            // Use KB + 1 decimal for streams < 1 MB so displayed sizes match the percentage (0.07→0.08 MB rounded looked like +14%).
                            let base_msg = if input_b < 1024.0 * 1024.0 {
                                format!(
                                "{}⚠️  VIDEO STREAM COMPRESSION FAILED:{} {:.1} KB → {:.1} KB ({:+.1}%)",
                                BRIGHT_YELLOW,
                                RESET,
                                input_b / 1024.0,
                                output_b / 1024.0,
                                stream_change_pct
                            )
                            } else {
                                format!(
                                "{}⚠️  VIDEO STREAM COMPRESSION FAILED:{} {:.3} MB → {:.3} MB ({:+.1}%)",
                                BRIGHT_YELLOW,
                                RESET,
                                input_b / 1024.0 / 1024.0,
                                output_b / 1024.0 / 1024.0,
                                stream_change_pct
                            )
                            };
                            
                            // Create beautiful single-line format with visual separators
                            let additional_info = if total_file_compressed {
                                format!("{}│{} Total file smaller but video stream larger", DIM, RESET)
                            } else {
                                format!("{}│{} Total file and video stream both larger", DIM, RESET)
                            };
                            
                            let final_msg = format!("{} {} {}│{} File may already be highly optimized", base_msg, additional_info, DIM, RESET);
                            warn!("   {}", final_msg);
                            (
                                format!(
                                    "Video stream compression failed: {:+.1}%",
                                    stream_change_pct
                                ),
                                format!(
                                    "Skipped: video stream larger ({:+.1}%)",
                                    stream_change_pct
                                ),
                                "Original file PROTECTED (output did not compress)".to_string(),
                                "Output discarded (video stream larger than original)".to_string(),
                            )
                        } else if explore_result.ssim.is_none() {
                            warn!("   ⚠️  SSIM CALCULATION FAILED │ cannot validate quality │ may indicate codec compatibility issues");
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
                            let reason = explore_result
                                .enhanced_verify_fail_reason
                                .as_deref()
                                .unwrap_or("unknown reason");
                            warn!("   ⚠️  Quality validation FAILED: {}", reason);
                            (
                                format!("Quality validation failed: {}", reason),
                                format!("Skipped: {}", reason),
                                "Original file PROTECTED (quality/size check failed)".to_string(),
                                "Output discarded (quality/size check failed)".to_string(),
                            )
                        };
                    warn!("   ⚠️  {} │ 🛡️  {} │ 🗑️  {}", fail_message, protect_msg, delete_msg);

                    // Keep/discard by total file size only (video stream is internal metric).
                    if shared_utils::should_keep_apple_fallback_hevc_output(
                        detection.codec.as_str(),
                        total_file_compressed,
                        _total_size_ratio,
                        config.allow_size_tolerance,
                        config.apple_compat,
                        source_is_gif,
                    ) {
                        warn!("   ⚠️  APPLE COMPAT FALLBACK (not full success): quality/size below target");
                        warn!(
                            "   Keeping best-effort output: last attempt CRF {:.1} ({} iterations), file is AV1 and importable",
                            explore_result.optimal_crf,
                            explore_result.iterations
                        );
                        shared_utils::conversion::commit_temp_to_output(
                            &temp_path,
                            &output_path,
                            config.force,
                        )
                        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;
                        return Ok(ConversionOutput {
                            input_path: input.display().to_string(),
                            output_path: output_path.display().to_string(),
                            strategy: ConversionStrategy {
                                target: TargetVideoFormat::Av1Mp4,
                                reason: "Apple compat fallback: best-effort AV1 kept (quality/size below target)".to_string(),
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
                                "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); quality/size below target — file is AV1 and importable",
                                explore_result.optimal_crf,
                                explore_result.iterations
                            ),
                            final_crf: explore_result.optimal_crf,
                            exploration_attempts: explore_result.iterations as u8,
                        });
                    }

                    if let Err(e) = std::fs::remove_file(&temp_path) {
                        warn!("Failed to clean up temp file {}: {}", temp_path.display(), e);
                    }
                    info!("   🗑️  {}", delete_msg);

                    shared_utils::copy_on_skip_or_fail(
                        input,
                        config.output_dir.as_deref(),
                        config.base_dir.as_deref(),
                        false,
                    )
                    .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

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

                if let Some(false) = explore_result.ms_ssim_passed {
                    let ms_ssim_score = match explore_result.ms_ssim_score {
                        Some(score) => score,
                        None => {
                            warn!("   ⚠️  MS-SSIM marked as failed but score not available");
                            return Err(VidQualityError::GeneralError(
                                "MS-SSIM verification failed: score not measured".to_string()
                            ));
                        }
                    };
                    warn!("   QUALITY TARGET FAILED (score: {:.4}) │ 🛡️  Original file PROTECTED (quality below threshold) ❌", ms_ssim_score);

                    // Only keep best-effort AV1 when source is Apple-incompatible.
                    if config.apple_compat
                        && !source_is_gif
                        && shared_utils::is_apple_incompatible_video_codec(detection.codec.as_str())
                    {
                        warn!("   ⚠️  APPLE COMPAT FALLBACK (not full success): quality below target");
                        warn!(
                            "   Keeping best-effort output: last attempt CRF {:.1} ({} iterations), file is AV1 and importable",
                            explore_result.optimal_crf,
                            explore_result.iterations
                        );
                        return Ok(ConversionOutput {
                            input_path: input.display().to_string(),
                            output_path: output_path.display().to_string(),
                            strategy: ConversionStrategy {
                                target: TargetVideoFormat::Av1Mp4,
                                reason: "Apple compat fallback: best-effort AV1 kept (quality below target)".to_string(),
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
                                "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); quality score {:.4} below target — file is AV1 and importable",
                                explore_result.optimal_crf,
                                explore_result.iterations,
                                ms_ssim_score
                            ),
                            final_crf: explore_result.optimal_crf,
                            exploration_attempts: explore_result.iterations as u8,
                        });
                    }

                    if output_path.exists() {
                        let _ = std::fs::remove_file(&output_path);
                        info!("   🗑️  Low MS-SSIM output deleted");
                    }

                    shared_utils::copy_on_skip_or_fail(
                        input,
                        config.output_dir.as_deref(),
                        config.base_dir.as_deref(),
                        false,
                    )
                    .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

                    return Ok(ConversionOutput {
                        input_path: input.display().to_string(),
                        output_path: input.display().to_string(),
                        strategy: ConversionStrategy {
                            target: TargetVideoFormat::Skip,
                            reason: format!("Quality target failed (score: {:.4})", ms_ssim_score),
                            command: String::new(),
                            preserve_audio: detection.has_audio,
                            crf: explore_result.optimal_crf,
                            lossless: false,
                        },
                        input_size: detection.file_size,
                        output_size: detection.file_size,
                        size_ratio: 1.0,
                        success: false,
                        message: format!("Skipped: MS-SSIM {:.4} below target {:.2}", ms_ssim_score, explore_result.actual_min_ssim),
                        final_crf: explore_result.optimal_crf,
                        exploration_attempts: explore_result.iterations as u8,
                    });
                }

                (
                    explore_result.output_size,
                    explore_result.optimal_crf,
                    explore_result.iterations as u8,
                )
            }
        }
        TargetVideoFormat::Skip => unreachable!(),
        _ => unreachable!("AV1 tool should not return HEVC target"),
    };

    // In AV1 conversion logic, the explore_result wasn't explicitly captured in a way that's shared across all branches in current code, 
    // but the final_crf and output_size were. For caching we want the full result if possible.
    // ... (This might need a bit more refactor if we want the full ExploreResult here)
    // For now, if we have final_crf and output_size, we can still update.
    
    // 💾 Update cache with the new best CRF
    if success_status_for_cache(strategy.target, &explore_result_opt) && final_crf > 0.0 {
        shared_utils::crf_constants::update_global_last_hit_crf_av1(final_crf);
    }
    if let Some(cache) = cache {
        if success_status_for_cache(strategy.target, &explore_result_opt) && final_crf > 0.0 {
            detection.precision.last_best_crf = Some(final_crf);
            if let Err(e) = cache.store_video_analysis(input, &detection) {
                tracing::debug!("Failed to update video cache: {}", e);
            } else {
                tracing::debug!("Updated video cache with best CRF: {:.1}", final_crf);
            }
        }
    }

    if !shared_utils::conversion::commit_temp_to_output(&temp_path, &output_path, config.force)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?
    {
        info!("⏭️ Output was created concurrently, skipping overwrite");
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),
            strategy: strategy.clone(),
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 1.0,
            success: true,
            message: "Skipped: output was created concurrently".to_string(),
            final_crf: 0.0,
            exploration_attempts: 0,
        });
    }

    shared_utils::copy_metadata(input, &output_path);

    let actual_output_size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(output_size);

    let input_stream_info = shared_utils::extract_stream_sizes(input);
    let output_stream_info = shared_utils::extract_stream_sizes(&output_path);
    let verify_result =
        shared_utils::verify_pure_media_compression(&input_stream_info, &output_stream_info, config.allow_size_tolerance);

    if output_stream_info.container_overhead > 10000 {
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

    let video_smaller = verify_result.video_compressed;
    let total_file_compressed = actual_output_size < detection.file_size;
    let total_size_ratio = if detection.file_size > 0 {
        actual_output_size as f64 / detection.file_size as f64
    } else {
        1.0
    };
    let total_within_tolerance = if config.allow_size_tolerance {
        total_size_ratio < 1.01
    } else {
        total_file_compressed
    };

    if config.require_compression && !total_within_tolerance {
        warn!("   ⚠️  COMPRESSION FAILED (total file comparison):");
        warn!(
            "   ⚠️  Total file: {} → {} ({:+.1}%)",
            shared_utils::format_bytes(input_stream_info.total_file_size),
            shared_utils::format_bytes(output_stream_info.total_file_size),
            verify_result.total_size_change_percent()
        );
        if video_smaller {
            warn!(
                "   ⚠️  Note: video stream compressed ({:+.1}%) but container/metadata overhead erased the gain",
                verify_result.video_size_change_percent()
            );
        } else {
            warn!(
                "   ⚠️  Video stream not compressed ({:+.1}%) │ 🛡️  Original file PROTECTED",
                verify_result.video_size_change_percent()
            );
        }

        // Apple-compat fallback: still decided purely by total file behavior (video stream is internal detail).
        if shared_utils::should_keep_apple_fallback_hevc_output(
            detection.codec.as_str(),
            total_file_compressed,
            total_size_ratio,
            config.allow_size_tolerance,
            config.apple_compat,
            source_is_gif,
        ) {
            warn!("   ⚠️  APPLE COMPAT FALLBACK (not full success): compression check failed (total file not smaller enough)");
            warn!(
                "   Keeping best-effort output: last attempt CRF {:.1} ({} iterations), file is AV1 and importable",
                final_crf, attempts
            );
            return Ok(ConversionOutput {
                input_path: input.display().to_string(),
                output_path: output_path.display().to_string(),
                strategy: ConversionStrategy {
                    target: strategy.target,
                    reason: "Apple compat fallback: best-effort AV1 kept (compression check failed)".to_string(),
                    command: String::new(),
                    preserve_audio: detection.has_audio,
                    crf: final_crf,
                    lossless: strategy.lossless,
                },
                input_size: detection.file_size,
                output_size: actual_output_size,
                size_ratio: total_size_ratio,
                success: true,
                message: format!(
                    "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); compression check failed — total file not smaller enough, but file is AV1 and importable",
                    final_crf, attempts
                ),
                final_crf,
                exploration_attempts: attempts,
            });
        }

        if output_path.exists() {
            let _ = std::fs::remove_file(&output_path);
            info!("   🗑️  Output deleted (cannot compress by total file size)");
        }
        shared_utils::copy_on_skip_or_fail(
            input,
            config.output_dir.as_deref(),
            config.base_dir.as_deref(),
            false,
        )
        .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: input.display().to_string(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: format!(
                    "Compression failed: total file {} → {} (video stream {} → {})",
                    shared_utils::format_bytes(input_stream_info.total_file_size),
                    shared_utils::format_bytes(output_stream_info.total_file_size),
                    shared_utils::format_bytes(input_stream_info.video_stream_size),
                    shared_utils::format_bytes(output_stream_info.video_stream_size),
                ),
                command: String::new(),
                preserve_audio: detection.has_audio,
                crf: final_crf,
                lossless: strategy.lossless,
            },
            input_size: detection.file_size,
            output_size: detection.file_size,
            size_ratio: 1.0,
            success: false,
            message: format!(
                "Skipped: total file not smaller (video stream {} → {}, container overhead: {})",
                shared_utils::format_bytes(input_stream_info.video_stream_size),
                shared_utils::format_bytes(output_stream_info.video_stream_size),
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
    }

    let size_ratio = actual_output_size as f64 / detection.file_size as f64;

    if config.should_delete_original() {
        if let Err(e) = shared_utils::conversion::safe_delete_original(
                input,
                &output_path,
                shared_utils::conversion::MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO,
            ) {
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
        output_size: actual_output_size,
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

fn success_status_for_cache(target: TargetVideoFormat, explore_result: &Option<shared_utils::ExploreResult>) -> bool {
    matches!(target, TargetVideoFormat::Av1Mp4) && explore_result.as_ref().map(|r| r.quality_passed).unwrap_or(false)
}

pub fn calculate_matched_crf(detection: &VideoDetectionResult) -> Result<u8> {
    let analysis = shared_utils::from_video_detection(
        &detection.file_path,
        detection.codec.as_str(),
        detection.width,
        detection.height,
        detection.bitrate,
        detection.fps,
        detection.duration_secs,
        detection.has_b_frames,
        detection.bit_depth,
        detection.file_size,
    );

    match shared_utils::calculate_av1_crf(&analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(&analysis, &result, shared_utils::EncoderType::Av1);
            Ok(result.crf.round() as u8)
        }
        Err(e) => Err(crate::VidQualityError::AnalysisError(format!(
            "Quality analysis failed: {}",
            e
        ))),
    }
}

fn execute_ffv1_conversion(
    detection: &VideoDetectionResult,
    output: &Path,
    max_threads: usize,
) -> Result<u64> {
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    let input_arg = shared_utils::safe_path_arg(Path::new(&detection.file_path))
        .as_ref()
        .to_string();
    let output_arg = shared_utils::safe_path_arg(output).as_ref().to_string();

    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(),
        max_threads.to_string(),
        "-i".to_string(),
        input_arg,
        "-c:v".to_string(),
        "ffv1".to_string(),
        "-level".to_string(),
        "3".to_string(),
        "-coder".to_string(),
        "1".to_string(),
        "-context".to_string(),
        "1".to_string(),
        "-g".to_string(),
        "1".to_string(),
        "-slices".to_string(),
        max_threads.to_string(),
        "-slicecrc".to_string(),
        "1".to_string(),
    ];

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.has_audio {
        args.extend(vec!["-c:a".to_string(), "flac".to_string()]);
    } else {
        args.push("-an".to_string());
    }

    args.push(output_arg);

    let result = Command::new("ffmpeg").args(&args).output()?;

    if !result.status.success() {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::FFmpegError {
            message: "FFmpeg command failed".to_string(),
            stderr: String::from_utf8_lossy(&result.stderr).to_string(),
            exit_code: result.status.code(),
            command: None,
            file_path: None,
        });
    }

    let size = std::fs::metadata(output).map_err(|e| {
        VidQualityError::ConversionError(format!("Failed to read FFV1 output: {}", e))
    })?;
    let size = size.len();
    if size == 0 {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::ConversionError(
            "FFV1 output file is empty (encoding may have failed)".to_string(),
        ));
    }
    if shared_utils::conversion::get_input_dimensions(output).is_err() {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::ConversionError(
            "FFV1 output file is not readable (invalid or corrupted)".to_string(),
        ));
    }

    Ok(size)
}

fn execute_av1_lossless(
    detection: &VideoDetectionResult,
    output: &Path,
    max_threads: usize,
) -> Result<u64> {
    warn!("⚠️  Mathematical lossless AV1 encoding (SVT-AV1) - this will be SLOW!");

    let svt_params = format!("lossless=1:lp={}", max_threads);

    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    let input_arg = shared_utils::safe_path_arg(Path::new(&detection.file_path))
        .as_ref()
        .to_string();
    let output_arg = shared_utils::safe_path_arg(output).as_ref().to_string();

    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(),
        max_threads.to_string(),
        "-i".to_string(),
        input_arg,
        "-c:v".to_string(),
        "libsvtav1".to_string(),
        "-crf".to_string(),
        "0".to_string(),
        "-preset".to_string(),
        "4".to_string(),
        "-svtav1-params".to_string(),
        svt_params,
        "-pix_fmt".to_string(),
        hdr_pix_fmt(detection).to_string(),
    ];

    args.extend(build_hdr_ffmpeg_args(detection));

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.has_audio {
        args.extend(vec!["-c:a".to_string(), "flac".to_string()]);
    } else {
        args.push("-an".to_string());
    }

    args.push(output_arg);

    let result = Command::new("ffmpeg").args(&args).output()?;

    if !result.status.success() {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::FFmpegError {
            message: "FFmpeg command failed".to_string(),
            stderr: String::from_utf8_lossy(&result.stderr).to_string(),
            exit_code: result.status.code(),
            command: None,
            file_path: None,
        });
    }

    let size = std::fs::metadata(output).map_err(|e| {
        VidQualityError::ConversionError(format!("Failed to read AV1 output: {}", e))
    })?;
    let size = size.len();
    if size == 0 {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::ConversionError(
            "AV1 output file is empty (encoding may have failed)".to_string(),
        ));
    }
    if shared_utils::conversion::get_input_dimensions(output).is_err() {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::ConversionError(
            "AV1 output file is not readable (invalid or corrupted)".to_string(),
        ));
    }

    Ok(size)
}

pub fn smart_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    auto_convert(input, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_format() {
        assert_eq!(TargetVideoFormat::Ffv1Mkv.extension(), "mkv");
        assert_eq!(TargetVideoFormat::Av1Mp4.extension(), "mp4");
    }
}
