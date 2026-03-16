//! Video Conversion API Module - HEVC/H.265 Version
//!
//! Pure conversion layer - executes video conversions based on detection results.
//! - Auto Mode: HEVC Lossless for lossless sources, HEVC CRF for lossy sources
//! - Simple Mode: Always HEVC MP4
//! - Size Exploration: Tries higher CRF if output is larger than input

use crate::detection_api::{CompressionType, VideoDetectionResult};
use crate::{Result, VidQualityError};

use shared_utils::analysis_cache::AnalysisCache;
use shared_utils::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, TargetVideoFormat,
};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use tracing::{info, warn};

/// Build the FFmpeg colour/HDR arguments that must be forwarded to every HEVC encode.
///
/// This preserves:
/// - color_primaries (e.g. bt2020)
/// - color_trc / color_transfer (e.g. smpte2084 for PQ, arib-std-b67 for HLG)
/// - colorspace (e.g. bt2020nc)
/// - mastering_display (HDR10 static mastering display metadata)
/// - max_cll (HDR10 content light level MaxCLL/MaxFALL)
///
/// Dolby Vision and HDR10+ cannot be remuxed losslessly through libx265, so they are preserved
/// as HDR10 (their static layer) by forwarding all static metadata — the dynamic layer is
/// stripped, which is unavoidable without specialised DV tooling.
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
    // Derive from color_space field; normalise bt2020ncl → bt2020nc for ffmpeg
    // Skip RGB/GBR colorspace: HEVC doesn't support it, and we're converting to YUV in filter chain
    let cs_str = match &detection.color_space {
        crate::detection_api::ColorSpace::BT2020 => Some("bt2020nc"),
        crate::detection_api::ColorSpace::BT709  => Some("bt709"),
        crate::detection_api::ColorSpace::Unknown(s) if !s.is_empty() && s != "unknown" => {
            // pass raw string through
            None // handled below separately to avoid lifetime issues
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

/// Result of attempting to prepare DV RPU data for x265 injection.
struct DvRpuResult {
    /// Path to the RPU .bin file for --dolby-vision-rpu
    rpu_path: PathBuf,
    /// x265 dolby-vision-profile string (e.g. "8.1")
    profile_str: String,
    /// Temp directory that must be kept alive until encode completes
    _temp_dir: tempfile::TempDir,
}

/// Attempt to extract Dolby Vision RPU data for injection into x265.
/// Returns `None` if:
/// - Content is not Dolby Vision
/// - `dovi_tool` is not installed
/// - Any extraction step fails (graceful fallback to HDR10)
fn prepare_dv_rpu(detection: &VideoDetectionResult) -> Option<DvRpuResult> {
    if !detection.is_dolby_vision {
        return None;
    }

    if !shared_utils::is_dovi_tool_available() {
        warn!("dovi_tool not found — Dolby Vision RPU cannot be preserved, falling back to HDR10");
        warn!("Install with: cargo install dovi_tool");
        return None;
    }

    let temp_dir = match tempfile::TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to create temp dir for DV RPU extraction: {}", e);
            return None;
        }
    };

    let input_path = Path::new(&detection.file_path);

    // Step 1: Extract raw HEVC Annex-B bitstream
    let raw_hevc = match shared_utils::extract_hevc_bitstream(input_path, temp_dir.path()) {
        Ok(p) => p,
        Err(e) => {
            warn!("DV RPU extraction: bitstream extraction failed: {}", e);
            warn!("Falling back to HDR10 static layer");
            return None;
        }
    };

    // Step 2: Extract RPU (and convert Profile 7 → 8.1 if needed)
    let rpu_path = match shared_utils::extract_dv_rpu(&raw_hevc, temp_dir.path(), detection.dv_profile) {
        Ok(p) => p,
        Err(e) => {
            warn!("DV RPU extraction failed: {}", e);
            warn!("Falling back to HDR10 static layer");
            return None;
        }
    };

    // Step 3: Determine x265 profile string
    let profile_str = match shared_utils::dv_x265_profile_string(
        detection.dv_profile,
        detection.dv_bl_signal_compatibility_id,
    ) {
        Some(s) => s,
        None => {
            warn!(
                "Unsupported DV profile {:?} for x265 — falling back to HDR10",
                detection.dv_profile
            );
            return None;
        }
    };

    info!("Dolby Vision RPU extracted — profile {} will be preserved in x265 output", profile_str);

    Some(DvRpuResult {
        rpu_path,
        profile_str,
        _temp_dir: temp_dir,
    })
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
    // Validate input file (check symlinks, file type, readability)
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(VidQualityError::ConversionError(e));
    }

    let detection = crate::detection_api::detect_video_with_cache(input, None)?;

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

    let temp_path = shared_utils::conversion::temp_path_for_output(&output_path);
    let _temp_guard = shared_utils::conversion::TempOutputGuard::new(temp_path.clone());
    let output_size = execute_hevc_conversion(&detection, &temp_path, 18, max_threads)?;

    if !shared_utils::conversion::commit_temp_to_output_with_metadata(&temp_path, &output_path, true, Some(input))
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
    auto_convert_with_cache(input, config, None)
}

pub fn auto_convert_with_cache(
    input: &Path,
    config: &ConversionConfig,
    cache: Option<&AnalysisCache>,
) -> Result<ConversionOutput> {
    // Pause if the user is being prompted to exit via Ctrl+C
    shared_utils::ctrlc_guard::wait_if_prompt_active();

    // Validate input file (check symlinks, file type, readability)
    if let Err(e) = shared_utils::conversion::validate_input_file(input) {
        return Err(VidQualityError::ConversionError(e));
    }

    let _label = input
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    shared_utils::progress_mode::set_log_context(&_label);
    let _log_guard = shared_utils::progress_mode::LogContextGuard;

    // Skip Live Photos in Apple compat mode
    if config.apple_compat && shared_utils::is_live_photo(input) {
        let reason = "Live Photo detected in Apple compat mode";
        shared_utils::progress_mode::video_skipped(reason);

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

    let mut detection = crate::detection_api::detect_video_with_cache(input, cache)?;

    // Warn about dynamic HDR metadata that will be stripped during re-encode
    if detection.is_dolby_vision {
        if shared_utils::is_dovi_tool_available() {
            info!("Dolby Vision detected: RPU will be preserved via dovi_tool");
        } else {
            warn!("Dolby Vision detected: dovi_tool not found, falling back to HDR10 static layer");
            warn!("Install dovi_tool to preserve DV metadata: cargo install dovi_tool");
        }
    }
    if detection.is_hdr10_plus {
        warn!("HDR10+ detected: dynamic metadata will be stripped to HDR10 static layer");
    }

    let strategy = determine_strategy_with_apple_compat(&detection, config.apple_compat);

    if strategy.target == TargetVideoFormat::Skip {
        shared_utils::progress_mode::video_skipped(&strategy.reason);

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
    // GIF as source has no Apple compatibility issue; do not show "APPLE COMPAT FALLBACK" for GIF→video.
    let source_is_gif = input_ext.eq_ignore_ascii_case("gif");

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
        shared_utils::progress_mode::video_skipped(&format!("Output exists: {}", output_path.display()));
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

    let (output_size, final_crf, attempts, explore_result_opt) = match strategy.target {
        TargetVideoFormat::HevcLosslessMkv => {
            info!("   🚀 Using HEVC Lossless Mode");
            let size =
                execute_hevc_lossless(&detection, &temp_path, config.child_threads)?;
            (size, 0.0, 0, None)
        }
        TargetVideoFormat::HevcMp4 => {
            if config.use_lossless {
                info!("   🚀 Using HEVC Lossless Mode (forced)");
                let size =
                    execute_hevc_lossless(&detection, &temp_path, config.child_threads)?;
                (size, 0.0, 0, None)
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
                    info!("   🖥️  CPU Mode: Using libx265 for higher SSIM (≥0.98)");
                }

                let ultimate = flag_mode.is_ultimate();
                
                // 🚀 CRF Hint Logic: Use cached best CRF if available for "warm start"
                let initial_crf = if let Some(hint) = detection.precision.last_best_crf {
                    info!("   💡 Using cached CRF hint: {:.1} (warm start)", hint);
                    hint
                } else if let Some(hint) = shared_utils::crf_constants::get_global_last_hit_crf_hevc() {
                    info!("   💡 Using global last hit CRF: {:.1} (warm start)", hint);
                    hint
                } else {
                    calculate_matched_crf(&detection)?
                };
                info!(
                    "   {} {}: CRF {:.1}",
                    if ultimate { "🔥" } else { "🔬" },
                    flag_mode.description_en(),
                    initial_crf
                );
                let explore_result = if ultimate {
                    shared_utils::explore_hevc_with_gpu_coarse_ultimate(
                        input_path,
                        &temp_path,
                        vf_args,
                        initial_crf,
                        ultimate,
                        config.allow_size_tolerance,
                        config.child_threads,
                    )
                } else {
                    shared_utils::explore_hevc_with_gpu_coarse_full(
                        input_path,
                        &temp_path,
                        vf_args,
                        initial_crf,
                        ultimate,
                        config.force_ms_ssim_long,
                        config.allow_size_tolerance,
                        config.min_ssim,
                        config.child_threads,
                    )
                }
                .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

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
                            let _ = std::fs::remove_file(&temp_path);
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
                    let total_size_ratio = if detection.file_size > 0 {
                        explore_result.output_size as f64 / detection.file_size as f64
                    } else {
                        1.0
                    };

                    tracing::debug!(
                        "stream_size: input={} output={} compressed={}",
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
                            
                            let base_msg = if input_b < 1024.0 * 1024.0 {
                                format!(
                                "⚠️  VIDEO STREAM COMPRESSION FAILED: {:.1} KB → {:.1} KB ({:+.1}%)",
                                input_b / 1024.0,
                                output_b / 1024.0,
                                stream_change_pct
                            )
                            } else {
                                format!(
                                "⚠️  VIDEO STREAM COMPRESSION FAILED: {:.3} MB → {:.3} MB ({:+.1}%)",
                                input_b / 1024.0 / 1024.0,
                                output_b / 1024.0 / 1024.0,
                                stream_change_pct
                            )
                            };
                            
                            // Create beautiful single-line format with visual separators
                            let additional_info = if total_file_compressed {
                                "│ Total file smaller but video stream larger"
                            } else {
                                "│ Total file and video stream both larger"
                            };
                            
                            let final_msg = format!("{} {} │ File may already be highly optimized", base_msg, additional_info);
                            tracing::debug!("   {}", final_msg);
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
                            warn!("   ⚠️  SSIM CALCULATION FAILED │ cannot validate quality │ may indicate codec compatibility issues (VP8/VP9/alpha channel)");
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
                    // Combine protection message with the main warning in a beautiful single line
                    shared_utils::progress_mode::video_skipped(&fail_message);
                    warn!("   🛡️  {} │ 🗑️  {}", protect_msg, delete_msg);

                    // Keep/discard by total file size only (video stream is internal metric).
                    if shared_utils::should_keep_apple_fallback_hevc_output(
                        detection.codec.as_str(),
                        total_file_compressed,
                        total_size_ratio,
                        config.allow_size_tolerance,
                        config.apple_compat,
                        source_is_gif,
                    ) {
                        warn!("   ⚠️  APPLE COMPAT FALLBACK: keeping best-effort HEVC output (CRF {:.1}, {} iters) to ensure iOS importability, despite missing quality/size targets", explore_result.optimal_crf, explore_result.iterations);
                        shared_utils::conversion::commit_temp_to_output_with_metadata(
                            &temp_path,
                            &output_path,
                            config.force,
                            Some(input),
                        )?;
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

                    if let Err(e) = std::fs::remove_file(&temp_path) {
                        warn!("Failed to clean up temp file {}: {}", temp_path.display(), e);
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

    // 💾 Update cache with the new best CRF
    if success_status_for_cache(strategy.target, &explore_result_opt) && final_crf > 0.0 {
        shared_utils::crf_constants::update_global_last_hit_crf_hevc(final_crf);
    }
    if let Some(cache) = cache {
        if success_status_for_cache(strategy.target, &explore_result_opt) {
            detection.precision.last_best_crf = Some(final_crf);
            if let Err(e) = cache.store_video_analysis(input, &detection) {
                tracing::debug!("Failed to update video cache: {}", e);
            } else {
                tracing::debug!("Updated video cache with best CRF: {:.1}", final_crf);
            }
        }
    }

    // 💾 Update cache with the new best CRF
    if success_status_for_cache(strategy.target, &explore_result_opt) && final_crf > 0.0 {
        shared_utils::crf_constants::update_global_last_hit_crf_hevc(final_crf);
    }
    if let Some(cache) = cache {
        if success_status_for_cache(strategy.target, &explore_result_opt) {
            detection.precision.last_best_crf = Some(final_crf);
            if let Err(e) = cache.store_video_analysis(input, &detection) {
                tracing::debug!("Failed to update video cache: {}", e);
            } else {
                tracing::debug!("Updated video cache with best CRF: {:.1}", final_crf);
            }
        }
    }

    // Verify temp file exists before commit
    if !temp_path.exists() {
        warn!("⚠️  Temp file missing before commit: {}", temp_path.display());
        return Err(VidQualityError::ConversionError(
            format!("Temp file not found: {}", temp_path.display())
        ));
    }

    if !shared_utils::conversion::commit_temp_to_output_with_metadata(&temp_path, &output_path, config.force, Some(input))
        .map_err(|e| VidQualityError::ConversionError(format!("Commit failed: {} (temp: {}, output: {})", e, temp_path.display(), output_path.display())))?
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

    if let Some(ref result) = explore_result_opt {
        if let Some(false) = result.ms_ssim_passed {
            let score_str = result.ms_ssim_score
                .map(|s| format!("{:.4}", s))
                .unwrap_or_else(|| "Unknown".to_string());
            // Note: In Ultimate Mode, ms_ssim_score stores VMAF-Y (0-1 scale).
            // The quality gate can fail even with high VMAF if CAMBI or PSNR-UV fail.
            // In Normal Mode, ms_ssim_score stores actual MS-SSIM or SSIM-All score.
            warn!("   QUALITY TARGET FAILED (score: {}) │ 🛡️  Original file PROTECTED (quality below threshold) ❌", score_str);

            // Only keep best-effort HEVC when source is Apple-incompatible (AV1/VP9/VVC/AV2).
            if config.apple_compat
                && !source_is_gif
                && shared_utils::is_apple_incompatible_video_codec(detection.codec.as_str())
            {
                warn!("   ⚠️  APPLE COMPAT FALLBACK (not full success): quality below target");
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
                        reason: "Apple compat fallback: best-effort HEVC kept (quality below target)".to_string(),
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
                        "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); quality score {} below target — file is HEVC and importable",
                        result.optimal_crf,
                        result.iterations,
                        score_str
                    ),
                    final_crf: result.optimal_crf,
                    exploration_attempts: result.iterations as u8,
                });
            }

            if output_path.exists() {
                let _ = std::fs::remove_file(&output_path);
                info!("   🗑️  Low MS-SSIM output deleted");
            }
            if temp_path.exists() {
                let _ = std::fs::remove_file(&temp_path);
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
                    reason: format!("Quality target failed (score: {})", score_str),
                    command: String::new(),
                    preserve_audio: detection.has_audio,
                    crf: result.optimal_crf,
                    lossless: false,
                },
                input_size: detection.file_size,
                output_size: detection.file_size,
                size_ratio: 1.0,
                success: false,
                message: format!("Skipped: MS-SSIM {} below target 0.90", score_str),
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
        shared_utils::verify_pure_media_compression(&input_stream_info, &output_stream_info, config.allow_size_tolerance);

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

    let video_smaller = verify_result.video_compressed;
    let total_file_compressed = actual_output_size < detection.file_size;
    let total_size_ratio = if detection.file_size > 0 {
        actual_output_size as f64 / detection.file_size as f64
    } else {
        1.0
    };
    let total_within_tolerance = if config.allow_size_tolerance {
        // Allow up to 1MB increase for container overhead
        actual_output_size <= detection.file_size.saturating_add(1_048_576)
    } else {
        total_file_compressed
    };

    // --- require_compression phase: primary decision by total file size, with video stream as diagnostic. ---
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
                "   ⚠️  Video stream not compressed ({:+.1}%)",
                verify_result.video_size_change_percent()
            );
        }
        warn!("   🛡️  Original file PROTECTED");

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
                size_ratio: total_size_ratio,
                success: true,
                message: format!(
                    "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); compression check failed — total file not smaller enough, but file is HEVC and importable",
                    final_crf,
                    attempts
                ),
                final_crf,
                exploration_attempts: attempts,
            });
        }

        if output_path.exists() {
            let _ = std::fs::remove_file(&output_path);
            info!("   🗑️  Output deleted (cannot compress by total file size)");
        }
        if temp_path.exists() {
            let _ = std::fs::remove_file(&temp_path);
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
                lossless: false,
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

    let output_size = actual_output_size;
    let size_ratio = output_size as f64 / detection.file_size as f64;

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

fn success_status_for_cache(target: TargetVideoFormat, explore_result: &Option<shared_utils::ExploreResult>) -> bool {
    matches!(target, TargetVideoFormat::HevcMp4) && explore_result.as_ref().map(|r| r.quality_passed).unwrap_or(false)
}

pub fn calculate_matched_crf(detection: &VideoDetectionResult) -> Result<f32> {
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
            Ok(result.crf)
        }
        Err(e) => Err(crate::VidQualityError::AnalysisError(format!(
            "Quality analysis failed: {}",
            e
        ))),
    }
}

fn execute_hevc_conversion(
    detection: &VideoDetectionResult,
    output: &Path,
    crf: u8,
    max_threads: usize,
) -> Result<u64> {
    // Attempt to extract DV RPU for injection (None = not DV or graceful fallback)
    let dv_rpu = prepare_dv_rpu(detection);

    // For HDR content (10-bit) we need additional x265 params to signal HDR correctly.
    // hdr-opt=1 enables SEI HDR metadata writing; repeat-headers=1 ensures SPS/PPS on
    // every keyframe so players always have the colour info available.
    let is_hdr_content = detection.bit_depth >= 10
        || detection.is_dolby_vision
        || detection.is_hdr10_plus
        || detection.mastering_display.is_some()
        || matches!(
            detection.color_transfer.as_deref(),
            Some("smpte2084") | Some("arib-std-b67")
        );

    let mut x265_params = if is_hdr_content {
        format!(
            "log-level=error:pools={}:hdr-opt=1:repeat-headers=1",
            max_threads
        )
    } else {
        format!("log-level=error:pools={}", max_threads)
    };

    // Inject DV RPU path and profile into x265 params when available
    if let Some(ref dv) = dv_rpu {
        x265_params.push_str(&format!(
            ":dolby-vision-rpu={}:dolby-vision-profile={}",
            dv.rpu_path.display(),
            dv.profile_str
        ));
    }

    let pix_fmt = hdr_pix_fmt(detection);
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
        "libx265".to_string(),
        "-crf".to_string(),
        crf.to_string(),
        "-preset".to_string(),
        "medium".to_string(),
        "-pix_fmt".to_string(),
        pix_fmt.to_string(),
        "-tag:v".to_string(),
        "hvc1".to_string(),
        "-x265-params".to_string(),
        x265_params,
    ];

    // Preserve variable frame rate (VFR) for iPhone slow-motion videos
    if detection.is_variable_frame_rate {
        args.extend(["-vsync".to_string(), "vfr".to_string()]);
    }

    // Append HDR colour metadata args (color_primaries, color_trc, colorspace,
    // master_display, max_cll)
    args.extend(build_hdr_ffmpeg_args(detection));

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.has_audio {
        args.extend(shared_utils::audio_args_for_container(
            detection.audio_codec.as_deref(),
            "mp4",
        ));
    } else {
        args.push("-an".to_string());
    }

    // Subtitles: copy when format is MP4-compatible
    args.extend(shared_utils::subtitle_args_for_container(
        detection.has_subtitles,
        detection.subtitle_codec.as_deref(),
        "mp4",
    ));

    args.push(output_arg);

    let result = Command::new("ffmpeg").args(&args).output()?;

    if !result.status.success() {
        return Err(VidQualityError::FFmpegError {
            message: "FFmpeg command failed".to_string(),
            stderr: String::from_utf8_lossy(&result.stderr).to_string(),
            exit_code: result.status.code(),
            command: None,
            file_path: None,
        });
    }

    Ok(std::fs::metadata(output)?.len())
}

fn execute_hevc_lossless(
    detection: &VideoDetectionResult,
    output: &Path,
    max_threads: usize,
) -> Result<u64> {
    warn!("⚠️  HEVC Lossless encoding - this will be slow and produce large files!");

    // Attempt to extract DV RPU for injection (None = not DV or graceful fallback)
    let dv_rpu = prepare_dv_rpu(detection);

    let is_hdr_content = detection.bit_depth >= 10
        || detection.is_dolby_vision
        || detection.is_hdr10_plus
        || detection.mastering_display.is_some()
        || matches!(
            detection.color_transfer.as_deref(),
            Some("smpte2084") | Some("arib-std-b67")
        );

    // hdr-opt=1 + repeat-headers=1 ensure HDR SEI metadata is written into the bitstream.
    let mut x265_params = if is_hdr_content {
        format!(
            "lossless=1:log-level=error:pools={}:hdr-opt=1:repeat-headers=1",
            max_threads
        )
    } else {
        format!("lossless=1:log-level=error:pools={}", max_threads)
    };

    // Inject DV RPU path and profile into x265 params when available
    if let Some(ref dv) = dv_rpu {
        x265_params.push_str(&format!(
            ":dolby-vision-rpu={}:dolby-vision-profile={}",
            dv.rpu_path.display(),
            dv.profile_str
        ));
    }

    let pix_fmt = hdr_pix_fmt(detection);
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
        "libx265".to_string(),
        "-pix_fmt".to_string(),
        pix_fmt.to_string(),
        "-x265-params".to_string(),
        x265_params,
        "-preset".to_string(),
        "medium".to_string(),
        "-tag:v".to_string(),
        "hvc1".to_string(),
    ];

    // Forward all HDR colour metadata
    args.extend(build_hdr_ffmpeg_args(detection));

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.has_audio {
        // MKV supports all codecs — always copy
        args.extend(shared_utils::audio_args_for_container(
            detection.audio_codec.as_deref(),
            "mkv",
        ));
    } else {
        args.push("-an".to_string());
    }

    // Subtitles: MKV supports all subtitle formats — always copy
    args.extend(shared_utils::subtitle_args_for_container(
        detection.has_subtitles,
        detection.subtitle_codec.as_deref(),
        "mkv",
    ));

    args.push(output_arg);

    let result = Command::new("ffmpeg").args(&args).output()?;

    if !result.status.success() {
        return Err(VidQualityError::FFmpegError {
            message: "FFmpeg command failed".to_string(),
            stderr: String::from_utf8_lossy(&result.stderr).to_string(),
            exit_code: result.status.code(),
            command: None,
            file_path: None,
        });
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };

        let strategy = determine_strategy(&detection);
        assert_eq!(
            strategy.target,
            TargetVideoFormat::Skip,
            "VP9 skipped in normal mode (modern format; use Apple-compat to convert)"
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
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
                color_primaries: None,
                color_transfer: None,
                mastering_display: None,
                max_cll: None,
                is_dolby_vision: false,
                dv_profile: None,
                dv_bl_signal_compatibility_id: None,
                is_hdr10_plus: false,
                has_subtitles: false,
                subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,                audio_channels: None,
                is_variable_frame_rate: false,
                precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
                tags: std::collections::HashMap::new(),
            ..Default::default()
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let crf = calculate_matched_crf(&det).unwrap();
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let crf = calculate_matched_crf(&det).unwrap();
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
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
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            is_dolby_vision: false,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            is_hdr10_plus: false,
            has_subtitles: false,
            subtitle_codec: None,
            max_b_frames: 0,
            encoder_params: None,            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let normal = determine_strategy(&det);
        assert_eq!(normal.target, TargetVideoFormat::Skip, "Unknown(\"vp9\") skipped in normal mode");
        let apple = determine_strategy_with_apple_compat(&det, true);
        assert_ne!(apple.target, TargetVideoFormat::Skip);
    }
}
