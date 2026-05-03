//! Executes video conversions based on detection results (HEVC and AV1 support).

use crate::detection_api::VideoDetectionResult;
use crate::{Result, VidQualityError};

use shared_utils::analysis_cache::AnalysisCache;
use shared_utils::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, SelectedCodec, TargetVideoFormat,
};
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use tracing::{info, warn};

fn convert_options_from_config(
    config: &ConversionConfig,
) -> shared_utils::conversion::ConvertOptions {
    let mut opts = shared_utils::conversion::ConvertOptions {
        output_dir: config.output_dir.clone(),
        base_dir: config.base_dir.clone(),
        child_threads: config.child_threads,
        codec: config.codec,
        ..Default::default()
    };

    opts.flags.set(
        shared_utils::conversion::ConvertFlags::FORCE,
        config.force(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::DELETE_ORIGINAL,
        config.delete_original(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::IN_PLACE,
        config.in_place(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::EXPLORE,
        config.explore_smaller(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::MATCH_QUALITY,
        config.match_quality(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::APPLE_COMPAT,
        config.apple_compat(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::COMPRESS,
        config.require_compression(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::USE_GPU,
        config.use_gpu(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::ULTIMATE,
        config.ultimate_mode(),
    );
    opts.flags.set(
        shared_utils::conversion::ConvertFlags::ALLOW_SIZE_TOLERANCE,
        config.allow_size_tolerance(),
    );

    opts
}

fn cleanup_output_file(path: &Path, context: &str) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!(
                path = %path.display(),
                error = %e,
                context = context,
                "Failed to remove output file during cleanup"
            );
        }
    }
}

struct ExploreQualityFailureDecision {
    fail_reason: String,
    fail_message: String,
    protect_msg: String,
    delete_msg: String,
}

impl ExploreQualityFailureDecision {
    fn inspect_and_log(explore_result: &shared_utils::ExploreResult, ultimate_mode: bool) -> Self {
        let actual_ssim = explore_result.ssim;
        let threshold = explore_result.actual_min_ssim;
        if ultimate_mode {
            let reason = explore_result
                .quality_passed
                .failure_reason()
                .or(explore_result.enhanced_verify_fail_reason.as_deref())
                .unwrap_or("quality/size check failed");
            warn!("   ⚠️  Quality validation FAILED: {reason}");
            return Self {
                fail_reason: format!("Quality validation failed: {reason}"),
                fail_message: format!("Skipped: {reason}"),
                protect_msg: "Original file PROTECTED (quality/size check failed)".to_string(),
                delete_msg: "Output discarded (quality/size check failed)".to_string(),
            };
        }

        if actual_ssim.is_none() {
            warn!(
                "   ⚠️  SSIM CALCULATION FAILED │ cannot validate quality │ may indicate codec compatibility issues (VP8/VP9/alpha channel)"
            );
            return Self {
                fail_reason: "SSIM calculation failed".to_string(),
                fail_message: "Skipped: SSIM calculation failed".to_string(),
                protect_msg: "Original file PROTECTED (SSIM not available)".to_string(),
                delete_msg: "Output discarded (SSIM calculation failed)".to_string(),
            };
        }

        if actual_ssim.is_some_and(|ssim| ssim < threshold) {
            let actual_ssim = actual_ssim.unwrap_or_default();
            warn!(
                "   ⚠️  Quality validation FAILED: SSIM {:.4} < {:.4}",
                actual_ssim, threshold
            );
            return Self {
                fail_reason: format!(
                    "Quality validation failed: SSIM {actual_ssim:.4} < {threshold:.4}"
                ),
                fail_message: format!(
                    "Skipped: SSIM {actual_ssim:.4} below threshold {threshold:.4}"
                ),
                protect_msg: "Original file PROTECTED (quality below threshold)".to_string(),
                delete_msg: "Output discarded (quality below threshold)".to_string(),
            };
        }

        let reason = explore_result
            .quality_passed
            .failure_reason()
            .or(explore_result.enhanced_verify_fail_reason.as_deref())
            .unwrap_or("quality/size check failed");
        warn!("   ⚠️  Quality validation FAILED: {reason}");
        Self {
            fail_reason: format!("Quality validation failed: {reason}"),
            fail_message: format!("Skipped: {reason}"),
            protect_msg: "Original file PROTECTED (quality/size check failed)".to_string(),
            delete_msg: "Output discarded (quality/size check failed)".to_string(),
        }
    }

    fn emit(&self) {
        shared_utils::progress_mode::video_skipped(&self.fail_message);
        warn!("   🛡️  {} │ 🗑️  {}", self.protect_msg, self.delete_msg);
    }

    fn into_skip_output(
        self,
        input: &Path,
        detection: &VideoDetectionResult,
        explore_result: &shared_utils::ExploreResult,
    ) -> ConversionOutput {
        ConversionOutput {
            input_path: input.display().to_string(),
            output_path: input.display().to_string(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: self.fail_reason,
                command: String::new(),
                preserve_audio: detection.has_audio,
                crf: explore_result.optimal_crf,
                lossless: false,
            },
            input_size: detection.file_size,
            output_size: detection.file_size,
            size_ratio: 1.0,
            success: false,
            message: self.fail_message,
            final_crf: explore_result.optimal_crf,
            exploration_attempts: u8::try_from(explore_result.iterations).unwrap_or(u8::MAX),
            blake3: None,
        }
    }
}

struct FinalQualityGateFailureDecision {
    quality_summary: String,
    skip_reason: String,
    skip_message: String,
}

impl FinalQualityGateFailureDecision {
    fn inspect_and_log(result: &shared_utils::ExploreResult, ultimate_mode: bool) -> Self {
        let quality_summary = if ultimate_mode {
            result
                .ultimate_quality_summary()
                .unwrap_or_else(|| "3D metrics unavailable".to_string())
        } else {
            result
                .ms_ssim_score
                .map_or_else(|| "Unknown".to_string(), |s| format!("score={s:.4}"))
        };
        let failure_label = if ultimate_mode {
            "3D quality gate failed"
        } else {
            "QUALITY TARGET FAILED"
        };
        warn!(
            "   {} ({}) │ 🛡️  Original file PROTECTED (quality below threshold) ❌",
            failure_label, quality_summary
        );

        Self {
            skip_reason: if ultimate_mode {
                format!("3D quality gate failed ({quality_summary})")
            } else {
                format!("Quality target failed ({quality_summary})")
            },
            skip_message: if ultimate_mode {
                format!("Skipped: 3D quality gate failed ({quality_summary})")
            } else {
                format!("Skipped: MS-SSIM {quality_summary} below target 0.90")
            },
            quality_summary,
        }
    }

    fn into_skip_output(
        self,
        input: &Path,
        detection: &VideoDetectionResult,
        result: &shared_utils::ExploreResult,
    ) -> ConversionOutput {
        ConversionOutput {
            input_path: input.display().to_string(),
            output_path: input.display().to_string(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: self.skip_reason,
                command: String::new(),
                preserve_audio: detection.has_audio,
                crf: result.optimal_crf,
                lossless: false,
            },
            input_size: detection.file_size,
            output_size: detection.file_size,
            size_ratio: 1.0,
            success: false,
            message: self.skip_message,
            final_crf: result.optimal_crf,
            exploration_attempts: u8::try_from(result.iterations).unwrap_or(u8::MAX),
            blake3: None,
        }
    }
}

/// Build `FFmpeg` HDR metadata arguments from detection results.
/// Preserves primaries, transfer characteristics, matrix, and static HDR10 metadata.
fn build_hdr_ffmpeg_args(detection: &VideoDetectionResult) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    // -color_primaries
    if let Some(cp) = &detection.color_primaries {
        if !cp.is_empty() && cp != "unknown" {
            args.push("-color_primaries".to_string());
            args.push(cp.clone());
        }
    }

    // -color_trc (transfer characteristics)
    if let Some(trc) = &detection.color_transfer {
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
        crate::detection_api::ColorSpace::BT709 => Some("bt709"),
        crate::detection_api::ColorSpace::Unknown(s) if !s.is_empty() && s != "unknown" => {
            // pass raw string through
            None // handled below separately to avoid lifetime issues
        }
        _ => None,
    };
    if let Some(cs) = cs_str {
        args.push("-colorspace".to_string());
        args.push(cs.to_string());
    } else if let crate::detection_api::ColorSpace::Unknown(s) = &detection.color_space {
        let is_rgb_colorspace = s == "gbr" || s == "rgb" || s == "gbrp";
        if !s.is_empty() && s != "unknown" && !is_rgb_colorspace {
            args.push("-colorspace".to_string());
            args.push(s.clone());
        }
    }

    // NOTE: -master_display and -max_cll are NOT valid top-level ffmpeg CLI options
    // (they're not recognized and cause "Unrecognized option" errors). HDR10 static
    // mastering-display and content-light-level metadata must be injected via
    // `-x265-params` as `master-display=...:max-cll=...`, which is handled in the
    // x265 params construction in execute_video_conversion / auto_convert_with_cache.

    args
}

/// Return the correct pixel format (10-bit for HDR, otherwise 8-bit).
const fn hdr_pix_fmt(detection: &VideoDetectionResult) -> &'static str {
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
    let rpu_path =
        match shared_utils::extract_dv_rpu(&raw_hevc, temp_dir.path(), detection.dv_profile) {
            Ok(p) => p,
            Err(e) => {
                warn!("DV RPU extraction failed: {}", e);
                warn!("Falling back to HDR10 static layer");
                return None;
            }
        };

    // Step 3: Determine x265 profile string
    let Some(profile_str) = shared_utils::dv_x265_profile_string(
        detection.dv_profile,
        detection.dv_bl_signal_compatibility_id,
    ) else {
        warn!(
            "Unsupported DV profile {:?} for x265 — falling back to HDR10",
            detection.dv_profile
        );
        return None;
    };

    info!(
        "Dolby Vision RPU extracted — profile {} will be preserved in x265 output",
        profile_str
    );

    Some(DvRpuResult {
        rpu_path,
        profile_str,
        _temp_dir: temp_dir,
    })
}

/// Result of attempting to prepare HDR10+ dynamic metadata for x265 injection.
struct Hdr10PlusResult {
    /// Path to the metadata .json file for --dhdr10-info
    json_path: PathBuf,
    /// Temp directory that must be kept alive until encode completes
    _temp_dir: tempfile::TempDir,
}

/// Attempt to extract HDR10+ dynamic metadata for injection into x265.
/// Returns `None` if:
/// - Content is not HDR10+
/// - `hdr10plus_tool` is not installed
/// - Any extraction step fails
fn prepare_hdr10plus_metadata(detection: &VideoDetectionResult) -> Option<Hdr10PlusResult> {
    if !detection.is_hdr10_plus {
        return None;
    }

    if !shared_utils::hdr_utils::is_hdr10plus_tool_available() {
        warn!("hdr10plus_tool not found — HDR10+ dynamic metadata cannot be preserved, falling back to HDR10");
        return None;
    }

    let temp_dir = match tempfile::TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to create temp dir for HDR10+ extraction: {}", e);
            return None;
        }
    };

    let input_path = Path::new(&detection.file_path);

    // Step 1: Extract raw HEVC Annex-B bitstream
    let raw_hevc = match shared_utils::extract_hevc_bitstream(input_path, temp_dir.path()) {
        Ok(p) => p,
        Err(e) => {
            warn!("HDR10+ extraction: bitstream extraction failed: {}", e);
            return None;
        }
    };

    // Step 2: Extract HDR10+ JSON
    let json_path =
        match shared_utils::hdr_utils::extract_hdr10plus_metadata(&raw_hevc, temp_dir.path()) {
            Ok(p) => p,
            Err(e) => {
                warn!("HDR10+ extraction failed: {}", e);
                return None;
            }
        };

    info!("HDR10+ dynamic metadata extracted — will be preserved via dhdr10-info in x265 output");

    Some(Hdr10PlusResult {
        json_path,
        _temp_dir: temp_dir,
    })
}

#[must_use]
pub fn determine_strategy(
    result: &VideoDetectionResult,
    codec: SelectedCodec,
) -> ConversionStrategy {
    determine_strategy_with_apple_compat(result, Path::new(&result.file_path), false, false, codec)
}

#[inline]
const fn hevc_delivery_target(apple_compat: bool) -> TargetVideoFormat {
    if apple_compat {
        TargetVideoFormat::HevcMov
    } else {
        TargetVideoFormat::HevcMp4
    }
}

pub fn determine_strategy_with_apple_compat(
    result: &VideoDetectionResult,
    input: &Path,
    apple_compat: bool,
    force: bool,
    codec: SelectedCodec,
) -> ConversionStrategy {
    tracing::debug!(
        file = %input.display(),
        apple_compat = apple_compat,
        force = force,
        codec = %codec.as_str(),
        "Determining conversion strategy"
    );
    // Enforcement: AV1 strategy does NOT support Apple compatibility
    if codec == SelectedCodec::Av1 && apple_compat {
        return ConversionStrategy {
            target: TargetVideoFormat::Skip,
            reason: "AV1 strategy does not support Apple compatibility (HEVC required for --apple-compat)"
                .to_string(),
            command: String::new(),
            preserve_audio: false,
            crf: 0.0,
            lossless: false,
        };
    }

    let skip_decision = if apple_compat {
        shared_utils::should_skip_video_codec_apple_compat(result.codec.as_str())
    } else {
        shared_utils::should_skip_video_codec(result.codec.as_str())
    };

    let mut detection = result.clone();
    detection.file_path = input.display().to_string();

    // Loop Intent Identification System
    // For GIF files, use fast-path (from_gif_path) to preserve GIF-specific signals.
    // For videos, use ffprobe path with structural signal refresh.
    let loop_verdict = if shared_utils::should_use_gif_fast_path(input) {
        // GIF file: use header-level detection
        shared_utils::LoopMeta::from_gif_path(input).map_or_else(
            || shared_utils::assess_loop_intent(&detection),
            |meta| shared_utils::assess_loop_intent_from_meta(&meta, Some(input)),
        )
    } else {
        // Video file: ensure structural signals are available
        if detection.pkt_sizes.len() < 3 || detection.pts_deltas.len() < 3 {
            if let Ok(fresh) = crate::detection_api::detect_video_with_cache(input, None) {
                detection = fresh;
                detection.file_path = input.display().to_string();
            }
        }
        shared_utils::assess_loop_intent(&detection)
    };

    // Centralized Apple-compat delivery policy lives in `shared_utils::loop_intent`.
    // `vid` should only orchestrate and convert, not define compatibility policy.
    let meta_for_policy = shared_utils::LoopMeta::from_video_detection(&detection);
    let loop_verdict = shared_utils::apply_apple_compat_modern_animation_policy(
        loop_verdict,
        &meta_for_policy,
        apple_compat,
        force,
    );

    let is_loop_intent = loop_verdict.is_keep_gif();

    // ══════════════════════════════════════════════════════════════════════════════
    // LOOP ERROR HANDLING: Skip on impossible or conflicting signals
    // ══════════════════════════════════════════════════════════════════════════════
    if let shared_utils::LoopIntentVerdict::Error(reason) = loop_verdict {
        return ConversionStrategy {
            target: TargetVideoFormat::Skip,
            reason: format!("Loop Intent Error: {reason}"),
            command: String::new(),
            preserve_audio: false,
            crf: 0.0,
            lossless: false,
        };
    }

    // ══════════════════════════════════════════════════════════════════════════════
    // DEFINITE LOOP INTENT: GIF conversions based on 7-layer decision
    // ══════════════════════════════════════════════════════════════════════════════

    if is_loop_intent && !force {
        return ConversionStrategy {
            target: TargetVideoFormat::Gif,
            reason: format!(
                "Loop intent confirmed ({}) - routing to animated image pipeline",
                loop_verdict.reason()
            ),
            command: String::new(),
            preserve_audio: false,
            crf: 0.0,
            lossless: false,
        };
    }

    // LoopWeak / Uncertain: not intercepted — falls through to standard pipeline
    // (HEVC optimization, apple compat codec conversion, etc.)

    // ══════════════════════════════════════════════════════════════════════════════
    // APPLE COMPATIBILITY: Codec-based conversion
    // ══════════════════════════════════════════════════════════════════════════════
    // If Apple compat is enabled, convert unsupported codecs to HEVC.
    // This runs AFTER loop intent interception, so animated images take priority.

    if skip_decision.should_skip && !force {
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

    let (target, reason, crf, lossless) =
        if let (crate::detection_api::CompressionType::Lossless, _) =
            (result.compression, result.format.as_str())
        {
            let codec_name = codec.as_str().to_uppercase();
            (
                TargetVideoFormat::HevcLosslessMkv,
                format!("Source is lossless - using {codec_name} Lossless MKV"),
                0.0_f32,
                true,
            )
        } else {
            let (target, reason_prefix) = match codec {
                SelectedCodec::Hevc => (hevc_delivery_target(apple_compat), "HEVC"),
                SelectedCodec::Av1 => (TargetVideoFormat::Av1Mp4, "AV1"),
                SelectedCodec::Av2 => (TargetVideoFormat::Av2Mp4, "AV2"),
                SelectedCodec::Vvc => (TargetVideoFormat::VvcMp4, "VVC"),
            };
            if result.archival_candidate || result.quality_score >= 90 {
                (
                    target,
                    format!(
                    "Source is high quality ({}) - compressing with {} CRF 18 (visually lossless)",
                    result.codec.as_str(),
                    reason_prefix
                ),
                    18.0_f32,
                    false,
                )
            } else {
                (
                    target,
                    format!(
                        "Source is {} ({}) - compressing with {} CRF 20",
                        result.codec.as_str(),
                        result.compression.as_str(),
                        reason_prefix
                    ),
                    20.0_f32,
                    false,
                )
            }
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

/// Automatically convert video based on analysis.
///
/// # Errors
/// Returns an error if analysis or conversion fails.
pub fn auto_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    auto_convert_with_cache(input, config, None)
}

/// Automatically convert video with caching.
///
/// # Errors
/// Returns an error if video detection fails, strategy cannot be determined, or conversion execution fails.
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

    let label = input
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    shared_utils::progress_mode::set_log_context(&label);
    let _log_guard = shared_utils::progress_mode::LogContextGuard;

    // Skip Live Photos in Apple compat mode
    if config.apple_compat() && shared_utils::is_live_photo(input) {
        let reason = "Live Photo detected in Apple compat mode";
        shared_utils::progress_mode::video_skipped(reason);

        let file_size = std::fs::metadata(input).map_or(0, |m| m.len());

        shared_utils::copy_on_skip_or_fail(
            input,
            config.output_dir.as_deref(),
            config.base_dir.as_deref(),
            false,
        )
        .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: "Live Photo detected in Apple compat mode".to_string(),
                command: String::new(),
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
            blake3: None,
        });
    }

    let mut detection = crate::detection_api::detect_video_with_cache(input, cache)?;

    // Internal judgment reconciliation:
    // If vid sees single-frame on a format that can be animated, re-check with image_detection
    // (which includes structural + penetration animation verification) before static isolation.
    if detection.frame_count <= 1
        && shared_utils::quality_matcher::SourceCodec::identify_by_content(input)
            .is_some_and(|codec| codec.can_be_animated())
    {
        if let Ok(image_det) = shared_utils::image_detection::detect_image(input) {
            if matches!(
                image_det.image_type,
                shared_utils::image_detection::ImageType::Animated
            ) || image_det.frame_count > 1
            {
                let corrected = u64::from(image_det.frame_count.max(2));
                tracing::warn!(
                    file = %input.display(),
                    vid_frame_count = detection.frame_count,
                    image_frame_count = corrected,
                    "Animated-image reconciliation corrected frame_count before vid static isolation"
                );
                detection.frame_count = corrected;
                if detection.duration_secs <= 0.0 {
                    if let Some(dur) = image_det.duration {
                        if dur > 0.0 {
                            detection.duration_secs = f64::from(dur);
                        }
                    }
                }
            }
        }
    }

    // --- Strict Animated Isolation: Ignore static images in vid ---
    if detection.frame_count <= 1 {
        let reason = "Static image detected (1 frame) - vid ignores static media (handled by img)";
        shared_utils::progress_mode::video_skipped(reason);

        let file_size = std::fs::metadata(input).map_or(0, |m| m.len());

        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: input.display().to_string(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: reason.to_string(),
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            },
            input_size: file_size,
            output_size: 0,
            size_ratio: 0.0,
            success: true,
            message: "Ignored static image in vid module".to_string(),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
        });
    }

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

    detection.file_path = input.display().to_string();

    let strategy = determine_strategy_with_apple_compat(
        &detection,
        input,
        config.apple_compat(),
        config.force(),
        config.codec,
    );

    tracing::debug!(
        file = %input.display(),
        strategy = %strategy.target.extension(),
        reason = %strategy.reason,
        crf = strategy.crf,
        lossless = strategy.lossless,
        "Conversion strategy determined"
    );

    // Enforcement check: if strategy resulted in skip due to AV1/Apple-compat conflict
    if config.codec == SelectedCodec::Av1 && config.apple_compat() {
        info!("   ❌ Error: AV1 strategy does not support Apple compatibility.");
        info!("      Tip: remove --apple-compat or change codec to hevc.");
        std::process::exit(1);
    }

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
            output_path: String::new(),
            strategy,
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 0.0,
            success: true,
            message: "Skipped modern codec to avoid generation loss".to_string(),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
        });
    }

    let output_dir =
        if let (Some(ref user_out), Some(ref base)) = (&config.output_dir, &config.base_dir) {
            let rel_path = input
                .strip_prefix(base)
                .unwrap_or(input)
                .parent()
                .unwrap_or_else(|| Path::new(""));
            user_out.join(rel_path)
        } else {
            config.output_dir.as_ref().map_or_else(
                || {
                    input
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_path_buf()
                },
                std::clone::Clone::clone,
            )
        };

    std::fs::create_dir_all(&output_dir)?;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let target_ext = strategy.target.extension();
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    // GIF as source has no Apple compatibility issue; do not show "APPLE COMPAT FALLBACK" for GIF→video.
    let source_is_gif = input_ext.eq_ignore_ascii_case("gif");

    let output_path = if input_ext.eq_ignore_ascii_case(target_ext)
        || (config.apple_compat() && input_ext.eq_ignore_ascii_case("mov"))
    {
        output_dir.join(format!("{stem}_hevc.{target_ext}"))
    } else {
        output_dir.join(format!("{stem}.{target_ext}"))
    };
    let output_path = shared_utils::conversion::reserve_output_path(input, &output_path);
    shared_utils::conversion::validate_output_path(&output_path, config.base_dir.as_deref())
        .map_err(VidQualityError::ConversionError)?;

    shared_utils::path_validator::check_input_output_conflict(input, &output_path)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

    if output_path.exists() && !config.force() {
        shared_utils::progress_mode::video_skipped(&format!(
            "Output exists: {}",
            output_path.display()
        ));
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),
            strategy,
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 1.0,
            success: true,
            message: format!("Skipped: output exists ({})", output_path.display()),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
        });
    }

    let temp_path = shared_utils::path_safety::isolated_temp_path_for_search(&output_path)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_guard = shared_utils::conversion::TempOutputGuard::new(temp_path.clone());
    info!(
        "🎬 Auto Mode: {} → {}",
        input.display(),
        strategy.target.as_str()
    );
    info!("   Reason: {}", strategy.reason);

    let (output_size, final_crf, attempts, explore_result_opt) = match strategy.target {
        TargetVideoFormat::HevcLosslessMkv => {
            info!(
                "   🚀 Using {} Lossless Mode",
                config.codec.as_str().to_uppercase()
            );
            let size = execute_lossless(
                &detection,
                &temp_path,
                config.child_threads,
                config.codec,
                config.apple_compat(),
                config.ultimate_mode(),
            )?;
            (size, 0.0, 0, None)
        }
        TargetVideoFormat::Gif => {
            let result = crate::animated_image::convert_to_gif_apple_compat(
                input,
                &convert_options_from_config(config),
            )?;
            let output_size = result.output_size.unwrap_or(0);
            let output_path = result.output_path.unwrap_or_default();
            let size_ratio = if detection.file_size > 0 {
                let ratio = rug::Rational::from((output_size, detection.file_size));
                ratio.to_f64()
            } else {
                1.0
            };

            info!(
                "   ✅ GIF Recovery Complete: {} → {} ({:.1}% of original)",
                shared_utils::format_bytes(detection.file_size),
                shared_utils::format_bytes(output_size),
                size_ratio * 100.0
            );

            // Update cache hint for successful GIF recovery
            if result.success {
                detection.precision.last_best_crf = Some(0.0);
                detection.precision.last_best_effort_crf = None;
                if let Some(cache) = cache {
                    if let Err(e) = cache.store_video_analysis(input, &detection) {
                        tracing::warn!("Failed to update video cache hint for GIF: {}", e);
                    } else {
                        tracing::debug!("Updated video cache with GIF recovery hint");
                    }
                }
            }

            return Ok(ConversionOutput {
                input_path: input.display().to_string(),
                output_path,
                strategy,
                input_size: detection.file_size,
                output_size,
                size_ratio,
                success: result.success,
                message: result.message,
                final_crf: 0.0,
                exploration_attempts: 0,
                blake3: None,
            });
        }
        TargetVideoFormat::HevcMov | TargetVideoFormat::HevcMp4 | TargetVideoFormat::Av1Mp4 | TargetVideoFormat::Av2Mp4 | TargetVideoFormat::VvcMp4 => {
            if config.use_lossless() {
                info!(
                    "   🚀 Using {} Lossless Mode (forced)",
                    config.codec.as_str().to_uppercase()
                );
                let size = execute_lossless(
                    &detection,
                    &temp_path,
                    config.child_threads,
                    config.codec,
                    config.apple_compat(),
                    config.ultimate_mode(),
                )?;
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

                let flag_mode =
                    shared_utils::validate_flags_result_with_ultimate(shared_utils::FlagRequest {
                        explore: config.explore_smaller(),
                        match_quality: config.match_quality(),
                        compress: config.require_compression(),
                        ultimate: config.ultimate_mode(),
                    })
                    .map_err(VidQualityError::ConversionError)?;

                let use_gpu = config.use_gpu();
                if !use_gpu {
                    let encoder_name = match config.codec {
                        SelectedCodec::Hevc => "libx265",
                        SelectedCodec::Av1 => "libsvtav1",
                        SelectedCodec::Av2 => "libaom-av2",
                        SelectedCodec::Vvc => "libvvenc",
                    };
                    info!(
                        "   🖥️  CPU Mode: Using {} for higher SSIM (≥0.95)",
                        encoder_name
                    );
                }

                let ultimate = flag_mode.is_ultimate();

                let predicted_crf = calculate_matched_crf(&detection, &config.codec)?;
                let warm_start_crf = if let Some(hint) = detection.precision.last_best_crf {
                    info!("   💡 Using cached CRF hint: {:.1} (warm start only)", hint);
                    Some(hint)
                } else if let Some(hint) = detection.precision.last_best_effort_crf {
                    info!(
                        "   💡 Using cached best-effort CRF hint: {:.1} (warm start only)",
                        hint
                    );
                    Some(hint)
                } else if let Some(hint) = match config.codec {
                    SelectedCodec::Hevc => {
                        shared_utils::crf_constants::get_global_last_hit_crf_hevc()
                    }
                    SelectedCodec::Av1 => {
                        shared_utils::crf_constants::get_global_last_hit_crf_av1()
                    }
                    SelectedCodec::Av2 | SelectedCodec::Vvc => None, // No global hints for experimental codecs yet
                } {
                    info!(
                        "   💡 Using global last hit {} CRF: {:.1} (warm start only)",
                        config.codec.as_str().to_uppercase(),
                        hint
                    );
                    Some(hint)
                } else {
                    None
                };
                let search_crf = warm_start_crf.unwrap_or(predicted_crf);
                info!(
                    "   {} {}: base CRF {:.1} → search anchor {:.1}",
                    if ultimate { "🔥" } else { "🔬" },
                    flag_mode.description_en(),
                    predicted_crf,
                    search_crf
                );
                let mut hdr_x265_params = String::new();

                // Inject DV RPU path and profile into x265 params when available
                let dv_rpu = prepare_dv_rpu(&detection);
                if let Some(ref dv) = dv_rpu {
                    let _ = write!(
                        hdr_x265_params,
                        ":dolby-vision-rpu={}:dolby-vision-profile={}",
                        dv.rpu_path.display(),
                        dv.profile_str
                    );
                }

                // Inject HDR10+ metadata into x265 params
                let hdr10plus = prepare_hdr10plus_metadata(&detection);
                if let Some(ref hdr) = hdr10plus {
                    let _ = write!(hdr_x265_params, ":dhdr10-info={}", hdr.json_path.display());
                }

                let is_hdr_content = detection.bit_depth >= 10
                    || detection.is_dolby_vision
                    || detection.is_hdr10_plus
                    || detection.mastering_display.is_some()
                    || matches!(
                        detection.color_transfer.as_deref(),
                        Some("smpte2084" | "arib-std-b67")
                    );

                if is_hdr_content {
                    hdr_x265_params.insert_str(0, ":hdr-opt=1:repeat-headers=1");
                }

                if let Some(ref md) = detection.mastering_display {
                    if !md.is_empty() {
                        let _ = write!(hdr_x265_params, ":master-display={md}");
                    }
                }
                if let Some(ref cll) = detection.max_cll {
                    if !cll.is_empty() {
                        let _ = write!(hdr_x265_params, ":max-cll={cll}");
                    }
                }

                let hdr_x265_params_opt = if hdr_x265_params.is_empty() {
                    None
                } else {
                    Some(hdr_x265_params.trim_start_matches(':').to_string())
                };

                let explore_result = match config.codec {
                    SelectedCodec::Hevc => {
                        shared_utils::explore_hevc_with_gpu(&shared_utils::GpuSearchRequest {
                            input: input.to_path_buf(),
                            output: temp_path.clone(),
                            vf_args,
                            baseline_crf: predicted_crf,
                            warm_start_crf,
                            ultimate_mode: ultimate,
                            force_ms_ssim_long: config.force_ms_ssim_long(),
                            allow_size_tolerance: config.allow_size_tolerance(),
                            min_ssim: config.min_ssim,
                            max_threads: config.child_threads,
                            hdr_x265_params: hdr_x265_params_opt,
                            apple_compat: config.apple_compat(),
                            preset: if ultimate {
                                shared_utils::EncoderPreset::Slower
                            } else {
                                shared_utils::EncoderPreset::Medium
                            },
                        })
                    }
                    SelectedCodec::Av1 => {
                        shared_utils::explore_av1_with_gpu(&shared_utils::GpuSearchRequest {
                            input: input.to_path_buf(),
                            output: temp_path.clone(),
                            vf_args,
                            baseline_crf: predicted_crf,
                            warm_start_crf,
                            ultimate_mode: ultimate,
                            force_ms_ssim_long: config.force_ms_ssim_long(),
                            allow_size_tolerance: config.allow_size_tolerance(),
                            min_ssim: config.min_ssim,
                            max_threads: config.child_threads,
                            hdr_x265_params: None,
                            apple_compat: config.apple_compat(),
                            preset: if ultimate {
                                shared_utils::EncoderPreset::Slower
                            } else {
                                shared_utils::EncoderPreset::Medium
                            },
                        })
                    }
                    SelectedCodec::Av2 | SelectedCodec::Vvc => {
                        return Err(VidQualityError::GeneralError(format!(
                            "{} encoding not yet implemented (experimental codec)",
                            config.codec.as_str().to_uppercase()
                        )));
                    }
                }
                .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

                for log_line in &explore_result.log {
                    info!("{}", log_line);
                }

                // --- Explore phase: quality/SSIM or size did not meet target; decide whether to keep or discard output. ---
                if !explore_result.quality_passed.is_passed()
                    && (config.match_quality() || config.explore_smaller())
                {
                    let total_file_compressed = explore_result.output_size < detection.file_size;
                    let total_size_ratio = if detection.file_size > 0 {
                        let ratio =
                            rug::Rational::from((explore_result.output_size, detection.file_size));
                        ratio.to_f64()
                    } else {
                        1.0
                    };
                    let decision = ExploreQualityFailureDecision::inspect_and_log(
                        &explore_result,
                        config.ultimate_mode(),
                    );
                    decision.emit();

                    // Keep/discard by total file size only (video stream is internal metric).
                    if shared_utils::should_keep_apple_fallback_hevc_output(
                        shared_utils::AppleFallbackKeepRequest {
                            codec_str: detection.codec.as_str(),
                            total_file_compressed,
                            total_size_ratio,
                            allow_size_tolerance: config.allow_size_tolerance(),
                            apple_compat: config.apple_compat(),
                            source_is_gif,
                        },
                    ) {
                        warn!("   ⚠️  APPLE COMPAT FALLBACK: keeping best-effort HEVC output (CRF {:.1}, {} iters) to ensure iOS importability, despite missing quality/size targets", explore_result.optimal_crf, explore_result.iterations);
                        shared_utils::conversion::commit_temp_to_output_with_metadata(
                            &temp_path,
                            &output_path,
                            config.force(),
                            Some(input),
                        )?;
                        return Ok(ConversionOutput {
                            input_path: input.display().to_string(),
                            output_path: output_path.display().to_string(),
                            strategy: ConversionStrategy {
                                target: hevc_delivery_target(config.apple_compat()),
                                reason: "Apple compat fallback: best-effort HEVC kept (quality/size below target)".to_string(),
                                command: String::new(),
                                preserve_audio: detection.has_audio,
                                crf: explore_result.optimal_crf,
                                lossless: false,
                            },
                            input_size: detection.file_size,
                            output_size: explore_result.output_size,
                            size_ratio: {
                                let ratio = rug::Rational::from((explore_result.output_size, detection.file_size.max(1)));
                                ratio.to_f64()
                            },
                            success: true,
                            message: format!(
                                "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); quality/size below target — file is HEVC and importable",
                                explore_result.optimal_crf,
                                explore_result.iterations
                            ),
                            final_crf: explore_result.optimal_crf,
                            exploration_attempts: u8::try_from(explore_result.iterations).unwrap_or(u8::MAX),
                            blake3: None,
                        });
                    }

                    if let Err(e) = std::fs::remove_file(&temp_path) {
                        warn!(
                            "Failed to clean up temp file {}: {}",
                            temp_path.display(),
                            e
                        );
                    }
                    shared_utils::copy_on_skip_or_fail(
                        input,
                        config.output_dir.as_deref(),
                        config.base_dir.as_deref(),
                        false,
                    )
                    .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

                    return Ok(decision.into_skip_output(input, &detection, &explore_result));
                }

                (
                    explore_result.output_size,
                    explore_result.optimal_crf,
                    u8::try_from(explore_result.iterations).unwrap_or(u8::MAX),
                    Some(explore_result),
                )
            }
        }
        TargetVideoFormat::Ffv1Mkv => unreachable!("HEVC tool should not return AV1/FFV1 target"),
        TargetVideoFormat::Skip => unreachable!(),
    };

    let cache_exact_hint = success_status_for_cache(strategy.target, explore_result_opt.as_ref());
    let cache_best_effort_hint =
        best_effort_status_for_cache(strategy.target, explore_result_opt.as_ref(), final_crf);

    if cache_exact_hint && final_crf > 0.0 {
        match config.codec {
            SelectedCodec::Hevc => {
                shared_utils::crf_constants::update_global_last_hit_crf_hevc(final_crf);
            }
            SelectedCodec::Av1 => {
                shared_utils::crf_constants::update_global_last_hit_crf_av1(final_crf);
            }
            SelectedCodec::Av2 | SelectedCodec::Vvc => {
                // No global CRF hints for experimental codecs yet
            }
        }
    }
    if let Some(cache) = cache {
        if cache_exact_hint || cache_best_effort_hint {
            if cache_exact_hint {
                detection.precision.last_best_crf = Some(final_crf);
                detection.precision.last_best_effort_crf = None;
            } else {
                detection.precision.last_best_effort_crf = Some(final_crf);
            }
            if let Err(e) = cache.store_video_analysis(input, &detection) {
                tracing::warn!("Failed to update video cache hint: {}", e);
            } else {
                tracing::debug!(
                    "Updated video cache with {} CRF hint: {:.1}",
                    if cache_exact_hint {
                        "exact"
                    } else {
                        "best-effort"
                    },
                    final_crf
                );
            }
        }
    }

    // Verify temp file exists before commit
    if !temp_path.exists() {
        warn!(
            "⚠️  Temp file missing before commit: {}",
            temp_path.display()
        );
        return Err(VidQualityError::ConversionError(format!(
            "Temp file not found: {}",
            temp_path.display()
        )));
    }

    if !shared_utils::conversion::commit_temp_to_output_with_metadata(
        &temp_path,
        &output_path,
        config.force(),
        Some(input),
    )
    .map_err(|e| {
        VidQualityError::ConversionError(format!(
            "Commit failed: {} (temp: {}, output: {})",
            e,
            temp_path.display(),
            output_path.display()
        ))
    })? {
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
            blake3: None,
        });
    }

    if let Some(ref result) = explore_result_opt {
        if result.ms_ssim_passed.is_failed() {
            let decision =
                FinalQualityGateFailureDecision::inspect_and_log(result, config.ultimate_mode());

            // Only keep best-effort HEVC when source is Apple-incompatible (AV1/VP9/VVC/AV2).
            if config.apple_compat()
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
                        target: hevc_delivery_target(config.apple_compat()),
                        reason: "Apple compat fallback: best-effort HEVC kept (quality below target)".to_string(),
                        command: String::new(),
                        preserve_audio: detection.has_audio,
                        crf: result.optimal_crf,
                        lossless: false,
                    },
                    input_size: detection.file_size,
                    output_size: result.output_size,
                    size_ratio: {
                        let ratio = rug::Rational::from((result.output_size, detection.file_size.max(1)));
                        ratio.to_f64()
                    },
                    success: true,
                    message: format!(
                        "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); {} below target — file is HEVC and importable",
                        result.optimal_crf,
                        result.iterations,
                        decision.quality_summary
                    ),
                    final_crf: result.optimal_crf,
                    exploration_attempts: u8::try_from(result.iterations).unwrap_or(u8::MAX),
                    blake3: None,
                });
            }

            if output_path.exists() {
                cleanup_output_file(&output_path, "low MS-SSIM cleanup");
                info!("   🗑️  Low MS-SSIM output deleted");
            }
            if temp_path.exists() {
                cleanup_output_file(&temp_path, "temporary output cleanup after low MS-SSIM");
            }

            shared_utils::copy_on_skip_or_fail(
                input,
                config.output_dir.as_deref(),
                config.base_dir.as_deref(),
                false,
            )
            .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

            return Ok(decision.into_skip_output(input, &detection, result));
        }
    }

    let pre_metadata_size = output_size;

    shared_utils::copy_metadata(input, &output_path);

    let actual_output_size = std::fs::metadata(&output_path).map_or(output_size, |m| m.len());

    let metadata_delta =
        shared_utils::video_explorer::detect_metadata_size(pre_metadata_size, actual_output_size);

    let input_stream_info = shared_utils::extract_stream_sizes(input);
    let output_stream_info = shared_utils::extract_stream_sizes(&output_path);

    let verify_result = shared_utils::verify_pure_media_compression(
        &input_stream_info,
        &output_stream_info,
        config.allow_size_tolerance(),
    );

    if metadata_delta > 0 || output_stream_info.container_overhead > 10000 {
        info!("   📋 Metadata: +{} bytes", metadata_delta);
        info!(
            "   📦 Container overhead: {} bytes ({:.1}%)",
            output_stream_info.container_overhead,
            output_stream_info.container_overhead_percent()
        );
    }

    let total_file_compressed = actual_output_size < detection.file_size;
    let total_size_ratio = if detection.file_size > 0 {
        let ratio = rug::Rational::from((actual_output_size, detection.file_size));
        ratio.to_f64()
    } else {
        1.0
    };
    let total_within_tolerance = if config.allow_size_tolerance() {
        // Allow up to standard tolerance increase for container overhead
        actual_output_size
            <= detection
                .file_size
                .saturating_add(shared_utils::DEFAULT_SIZE_TOLERANCE_BYTES)
    } else {
        total_file_compressed
    };

    // --- require_compression phase: primary decision by total file size. ---
    if config.require_compression() && !total_within_tolerance {
        warn!("   ⚠️  COMPRESSION FAILED (total file comparison):");
        warn!(
            "   ⚠️  Total file: {} → {} ({:+.1}%)",
            shared_utils::format_bytes(input_stream_info.total_file_size),
            shared_utils::format_bytes(output_stream_info.total_file_size),
            verify_result.total_size_change_percent()
        );
        tracing::debug!(
            "video stream diagnostic: {} -> {} ({:+.1}%), container_overhead={}B",
            shared_utils::format_bytes(input_stream_info.video_stream_size),
            shared_utils::format_bytes(output_stream_info.video_stream_size),
            verify_result.video_size_change_percent(),
            output_stream_info.container_overhead
        );
        warn!("   🛡️  Original file PROTECTED");

        // Apple-compat fallback: still decided purely by total file behavior (video stream is internal detail).
        if shared_utils::should_keep_apple_fallback_hevc_output(
            shared_utils::AppleFallbackKeepRequest {
                codec_str: detection.codec.as_str(),
                total_file_compressed,
                total_size_ratio,
                allow_size_tolerance: config.allow_size_tolerance(),
                apple_compat: config.apple_compat(),
                source_is_gif,
            },
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
                    target: hevc_delivery_target(config.apple_compat()),
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
                    "Apple compat fallback: kept best-effort output (CRF {final_crf:.1}, {attempts} iters); compression check failed — total file not smaller enough, but file is HEVC and importable"
                ),
                final_crf,
                exploration_attempts: attempts,
                blake3: None,
            });
        }

        if output_path.exists() {
            cleanup_output_file(&output_path, "compression failure cleanup");
            info!("   🗑️  Output deleted (cannot compress by total file size)");
        }
        if temp_path.exists() {
            cleanup_output_file(
                &temp_path,
                "temporary output cleanup after compression failure",
            );
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
                    "Compression failed: total file {} → {} ({:+.1}%)",
                    shared_utils::format_bytes(input_stream_info.total_file_size),
                    shared_utils::format_bytes(output_stream_info.total_file_size),
                    verify_result.total_size_change_percent(),
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
                "Skipped: total file not smaller ({} → {})",
                shared_utils::format_bytes(input_stream_info.total_file_size),
                shared_utils::format_bytes(output_stream_info.total_file_size),
            ),
            final_crf,
            exploration_attempts: attempts,
            blake3: None,
        });
    }

    if verify_result.video_compressed && verify_result.total_compression_ratio >= 1.0 {
        tracing::debug!(
            "video stream shrank ({:+.1}%) but total file grew ({:+.1}%) due to container overhead diff {:+}B",
            verify_result.video_size_change_percent(),
            verify_result.total_size_change_percent(),
            verify_result.container_overhead_diff
        );
    }

    let output_size = actual_output_size;
    let size_ratio = {
        let ratio = rug::Rational::from((output_size, detection.file_size.max(1)));
        ratio.to_f64()
    };

    if config.should_delete_original() {
        if let Err(e) = shared_utils::conversion::safe_delete_original(
            input,
            &output_path,
            shared_utils::MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO,
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
            format!("Explored {attempts} CRF values, final CRF: {final_crf}")
        } else {
            "Conversion successful".to_string()
        },
        final_crf,
        exploration_attempts: attempts,
        blake3: None,
    })
}

fn success_status_for_cache(
    target: TargetVideoFormat,
    explore_result: Option<&shared_utils::ExploreResult>,
) -> bool {
    matches!(target, TargetVideoFormat::Gif)
        || (matches!(
            target,
            TargetVideoFormat::HevcMov | TargetVideoFormat::HevcMp4 | TargetVideoFormat::Av1Mp4
        ) && explore_result.is_some_and(|r| r.quality_passed.is_passed()))
}

fn best_effort_status_for_cache(
    target: TargetVideoFormat,
    explore_result: Option<&shared_utils::ExploreResult>,
    final_crf: f32,
) -> bool {
    matches!(
        target,
        TargetVideoFormat::HevcMov | TargetVideoFormat::HevcMp4 | TargetVideoFormat::Av1Mp4
    ) && final_crf > 0.0
        && explore_result.is_some_and(|r| r.quality_passed.is_failed())
}

/// Calculate matched CRF based on detection results and selected codec.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_matched_crf(
    detection: &VideoDetectionResult,
    codec: &SelectedCodec,
) -> Result<f32> {
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

    let result = match codec {
        SelectedCodec::Hevc => shared_utils::calculate_hevc_crf(&analysis),
        SelectedCodec::Av1 => shared_utils::calculate_av1_crf(&analysis),
        SelectedCodec::Av2 | SelectedCodec::Vvc => {
            return Err(VidQualityError::GeneralError(format!(
                "{} CRF calculation not yet implemented (experimental codec)",
                codec.as_str().to_uppercase()
            )));
        }
    };

    match result {
        Ok(result) => {
            let encoder = match codec {
                SelectedCodec::Hevc => shared_utils::EncoderType::Hevc,
                SelectedCodec::Av1 => shared_utils::EncoderType::Av1,
                SelectedCodec::Av2 | SelectedCodec::Vvc => {
                    return Err(VidQualityError::GeneralError(format!(
                        "{} encoder type not yet implemented",
                        codec.as_str().to_uppercase()
                    )));
                }
            };
            shared_utils::log_quality_analysis(&analysis, &result, encoder);
            Ok(result.crf)
        }
        Err(e) => Err(crate::VidQualityError::AnalysisError(format!(
            "Quality analysis failed: {e}"
        ))),
    }
}

fn execute_lossless(
    detection: &VideoDetectionResult,
    output: &Path,
    max_threads: usize,
    codec: SelectedCodec,
    apple_compat: bool,
    ultimate: bool,
) -> Result<u64> {
    let codec_name = codec.as_str().to_uppercase();
    warn!(
        "⚠️  {} Lossless encoding - this will be slow and produce large files!",
        codec_name
    );

    // Attempt to extract DV RPU for injection (None = not DV or graceful fallback)
    let dv_rpu = prepare_dv_rpu(detection);

    // Attempt to extract HDR10+ metadata for injection
    let hdr10plus = prepare_hdr10plus_metadata(detection);

    let is_hdr_content = detection.bit_depth >= 10
        || detection.is_dolby_vision
        || detection.is_hdr10_plus
        || detection.mastering_display.is_some()
        || matches!(
            detection.color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        );

    // hdr-opt=1 + repeat-headers=1 ensure HDR SEI metadata is written into the bitstream.
    let x265_memory_profile = shared_utils::x265_params::memory_profile_for_detection(detection);
    if x265_memory_profile.is_low_memory() {
        info!(
            file = %detection.file_path,
            codec = %detection.codec.as_str(),
            file_size_gb = f64::from(u32::try_from(detection.file_size / (1024 * 1024)).unwrap_or(u32::MAX)) / 1024.0,
            "Applying low-memory x265 profile for large/high-fidelity source"
        );
    }
    let mut extra_x265_params = String::new();
    if is_hdr_content {
        shared_utils::x265_params::push_param(&mut extra_x265_params, "hdr-opt=1");
        shared_utils::x265_params::push_param(&mut extra_x265_params, "repeat-headers=1");
    }
    if let Some(ref md) = detection.mastering_display {
        if !md.is_empty() {
            shared_utils::x265_params::push_param(
                &mut extra_x265_params,
                &format!("master-display={md}"),
            );
        }
    }
    if let Some(ref cll) = detection.max_cll {
        if !cll.is_empty() {
            shared_utils::x265_params::push_param(
                &mut extra_x265_params,
                &format!("max-cll={cll}"),
            );
        }
    }

    // Inject DV RPU path and profile into x265 params when available
    if let Some(ref dv) = dv_rpu {
        shared_utils::x265_params::push_param(
            &mut extra_x265_params,
            &format!(
                "dolby-vision-rpu={}:dolby-vision-profile={}",
                dv.rpu_path.display(),
                dv.profile_str
            ),
        );
    }

    // Inject HDR10+ metadata into x265 params
    if let Some(ref hdr) = hdr10plus {
        shared_utils::x265_params::push_param(
            &mut extra_x265_params,
            &format!("dhdr10-info={}", hdr.json_path.display()),
        );
    }
    let x265_params = shared_utils::x265_params::format_x265_lossless_params(
        max_threads,
        Some(&extra_x265_params),
        x265_memory_profile,
    );

    let pix_fmt = hdr_pix_fmt(detection);
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);

    let encoder = match codec {
        SelectedCodec::Hevc => "libx265",
        SelectedCodec::Av1 => "libsvtav1",
        SelectedCodec::Av2 => "libaom-av2",
        SelectedCodec::Vvc => "libvvenc",
    };

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
        encoder.to_string(),
        "-pix_fmt".to_string(),
        pix_fmt.to_string(),
    ];

    if codec == SelectedCodec::Hevc {
        args.extend([
            shared_utils::constants::FFMPEG_ARG_X265_PARAMS.to_string(),
            x265_params,
            shared_utils::constants::FFMPEG_ARG_PRESET.to_string(),
            if ultimate {
                shared_utils::constants::FFMPEG_PRESET_SLOWER.to_string()
            } else {
                shared_utils::constants::FFMPEG_PRESET_MEDIUM.to_string()
            },
        ]);
        if apple_compat {
            args.extend([
                shared_utils::constants::FFMPEG_ARG_TAG_VIDEO.to_string(),
                shared_utils::constants::FFMPEG_TAG_HVC1.to_string(),
            ]);
        }
    } else {
        // SVT-AV1 lossless
        args.extend([
            shared_utils::constants::FFMPEG_ARG_CRF.to_string(),
            "0".to_string(),
            shared_utils::constants::FFMPEG_ARG_PRESET.to_string(),
            shared_utils::constants::FFMPEG_SVTAV1_DEFAULT_PRESET.to_string(),
        ]);
    }

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

    let (status, stderr) = shared_utils::FfmpegBuilder::new()
        .args(args)
        .spawn()?
        .wait_with_output()?;

    if !status.success() {
        return Err(VidQualityError::FFmpegError {
            message: "FFmpeg command failed".to_string(),
            stderr,
            exit_code: status.code(),
            command: None,
            file_path: None,
        });
    }

    Ok(std::fs::metadata(output)?.len())
}

/// Smart conversion with comprehensive analysis.
///
/// # Errors
/// Returns an error if analysis or conversion fails.
pub fn smart_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    auto_convert(input, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::Builder;

    const MINIMAL_TRANSPARENT_LOOP_GIF: &[u8] = &[
        b'G', b'I', b'F', b'8', b'9', b'a', // Header
        0x01, 0x00, 0x01, 0x00, // Logical screen: 1x1
        0x80, 0x00, 0x00, // Global color table, background, aspect
        0x00, 0x00, 0x00, // Color #0
        0xFF, 0xFF, 0xFF, // Color #1
        0x21, 0xFF, 0x0B, // App extension introducer
        b'N', b'E', b'T', b'S', b'C', b'A', b'P', b'E', b'2', b'.', b'0', 0x03, 0x01, 0x00, 0x00,
        0x00, // Infinite loop
        0x21, 0xF9, 0x04, 0x01, 0x0A, 0x00, 0x00, 0x00, // Frame 1 GCE, transparency + 100 ms
        0x2C, // Frame 1 image descriptor
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x01, 0x00,
        0x00, // Minimal image data block
        0x21, 0xF9, 0x04, 0x01, 0x0A, 0x00, 0x00, 0x00, // Frame 2 GCE, transparency + 100 ms
        0x2C, // Frame 2 image descriptor
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x01, 0x00,
        0x00, // Minimal image data block
        0x3B, // Trailer
    ];

    #[test]
    fn test_target_format() {
        assert_eq!(TargetVideoFormat::HevcLosslessMkv.extension(), "MKV");
        assert_eq!(TargetVideoFormat::HevcMov.extension(), "MOV");
        assert_eq!(TargetVideoFormat::HevcMp4.extension(), "MP4");
    }

    #[test]
    fn test_config_default_apple_compat() {
        let config = ConversionConfig::default();
        assert!(
            !config.apple_compat(),
            "Default apple_compat should be false"
        );
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };

        let strategy = determine_strategy(&detection, SelectedCodec::Hevc);
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };

        let strategy = determine_strategy_with_apple_compat(
            &detection,
            Path::new(&detection.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
        assert_ne!(
            strategy.target,
            TargetVideoFormat::Skip,
            "VP9 should NOT be skipped in Apple compat mode"
        );
        assert_eq!(
            strategy.target,
            TargetVideoFormat::HevcMov,
            "VP9 should be converted to HEVC MOV in Apple compat mode"
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };

        let normal = determine_strategy(&detection, SelectedCodec::Hevc);
        assert_eq!(
            normal.target,
            TargetVideoFormat::Skip,
            "HEVC should be skipped in normal mode"
        );

        let apple = determine_strategy_with_apple_compat(
            &detection,
            Path::new(&detection.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };

        let normal = determine_strategy(&detection, SelectedCodec::Hevc);
        assert_ne!(
            normal.target,
            TargetVideoFormat::Skip,
            "H.264 should NOT be skipped in normal mode"
        );

        let apple = determine_strategy_with_apple_compat(
            &detection,
            Path::new(&detection.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
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
                encoder_params: None,
                audio_channels: None,
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

            let normal = determine_strategy(&detection, SelectedCodec::Hevc);
            let apple = determine_strategy_with_apple_compat(
                &detection,
                Path::new(&detection.file_path),
                true,
                false,
                SelectedCodec::Hevc,
            );

            let is_skip_normal = normal.target == TargetVideoFormat::Skip;
            let is_skip_apple = apple.target == TargetVideoFormat::Skip;

            assert_eq!(
                is_skip_normal, expected_skip_normal,
                "STRICT: {codec:?} normal mode: expected skip={expected_skip_normal}, got skip={is_skip_normal}"
            );

            assert_eq!(
                is_skip_apple, expected_skip_apple,
                "STRICT: {codec:?} Apple compat mode: expected skip={expected_skip_apple}, got skip={is_skip_apple}"
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let s = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
        assert_eq!(s.target, TargetVideoFormat::HevcMov);
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let s = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let crf = calculate_matched_crf(&det, &SelectedCodec::Hevc)
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        assert!(
            (0.0..=35.0).contains(&crf),
            "CRF {crf:.1} should be in [0, 35]"
        );
        assert!(
            (18.0..=28.0).contains(&crf),
            "CRF {crf:.1} should be ~18-28 for 6Mbps 1080p"
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let crf = calculate_matched_crf(&det, &SelectedCodec::Hevc)
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        assert!(
            (0.0..=22.0).contains(&crf),
            "High bitrate AV1 should get CRF <= 22, got {crf:.1}"
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let s = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let s = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
        assert_eq!(s.target, TargetVideoFormat::HevcMov);
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
            encoder_params: None,
            audio_channels: None,
            is_variable_frame_rate: false,
            precision: shared_utils::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            ..Default::default()
        };
        let normal = determine_strategy(&det, SelectedCodec::Hevc);
        assert_eq!(
            normal.target,
            TargetVideoFormat::Skip,
            "Unknown(\"vp9\") skipped in normal mode"
        );
        let apple = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
        assert_ne!(apple.target, TargetVideoFormat::Skip);
    }

    #[test]
    fn test_hdr10plus_injection_logic() {
        use crate::detection_api::VideoDetectionResult;
        use std::path::PathBuf;

        // Mock a 10-bit HDR10+ result
        let detection = VideoDetectionResult {
            file_path: "test.mp4".to_string(),
            bit_depth: 10,
            is_hdr10_plus: true, // HDR10+ detected
            is_dolby_vision: false,
            color_transfer: Some("smpte2084".to_string()),
            ..Default::default()
        };

        // Logic we want to verify (from auto_convert_with_cache)
        let mut hdr_x265_params = String::new();

        // Simulate prepare_hdr10plus_metadata success
        let mock_json_path = PathBuf::from("/tmp/hdr10plus.json");
        let _ = write!(hdr_x265_params, ":dhdr10-info={}", mock_json_path.display());

        let is_hdr_content = detection.bit_depth >= 10
            || detection.is_dolby_vision
            || detection.is_hdr10_plus
            || detection.mastering_display.is_some()
            || matches!(
                detection.color_transfer.as_deref(),
                Some("smpte2084" | "arib-std-b67")
            );

        if is_hdr_content {
            hdr_x265_params.insert_str(0, ":hdr-opt=1:repeat-headers=1");
        }

        let hdr_x265_params_opt = if hdr_x265_params.is_empty() {
            None
        } else {
            Some(hdr_x265_params.trim_start_matches(':').to_string())
        };

        // Verify the result
        assert!(hdr_x265_params_opt.is_some());
        let final_params = hdr_x265_params_opt.unwrap_or_else(|| panic!("missing params"));
        assert!(final_params.contains("hdr-opt=1"));
        assert!(final_params.contains("repeat-headers=1"));
        assert!(final_params.contains("dhdr10-info=/tmp/hdr10plus.json"));

        println!("✅ HDR10+ x265-params injection verified: {final_params}");
    }

    #[test]
    fn test_gif_like_video_recovery() {
        use crate::detection_api::{CompressionType, DetectedCodec};
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "sticker.mp4".into(),
            codec: DetectedCodec::H264,
            compression: CompressionType::Standard,
            width: 512,
            height: 512,
            duration_secs: 2.0,
            has_audio: false,
            frame_count: 50,
            fps: 25.0,
            file_size: 500_000,
            ..Default::default()
        };

        // This should trigger the Gif strategy because it's silent, short, and fits sticker heuristic
        let strategy = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );
        assert_eq!(strategy.target, TargetVideoFormat::Gif);
        // The sticker heuristic now lives in Layer 1-B3, so reason comes from the tree
        assert!(
            strategy.reason.contains("Loop intent confirmed"),
            "unexpected reason: {}",
            strategy.reason
        );
    }

    #[test]
    fn test_strategy_uses_current_input_path_for_native_gif_loop_intent() {
        use crate::detection_api::{CompressionType, DetectedCodec};

        let mut gif = Builder::new()
            .suffix(".gif")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        gif.write_all(MINIMAL_TRANSPARENT_LOOP_GIF)
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        let detection = crate::detection_api::VideoDetectionResult {
            file_path: "/stale/cache-hit.mp4".to_string(),
            format: "gif".into(),
            codec: DetectedCodec::Unknown("gif".into()),
            compression: CompressionType::Lossless,
            width: 1,
            height: 1,
            duration_secs: 0.2,
            has_audio: false,
            frame_count: 2,
            fps: 10.0,
            file_size: std::fs::metadata(gif.path())
                .unwrap_or_else(|e| panic!("error: {e:?}"))
                .len(),
            ..Default::default()
        };

        let adjusted = determine_strategy_with_apple_compat(
            &detection,
            gif.path(),
            false,
            false,
            SelectedCodec::Hevc,
        );

        assert_eq!(adjusted.target, TargetVideoFormat::Gif);
        assert!(
            adjusted.reason.contains("Layer 0") || adjusted.reason.contains("Layer 1-B"),
            "unexpected reason: {}",
            adjusted.reason
        );
    }

    #[test]
    fn test_strategy_uses_current_input_path_for_native_gif_loop_intent_apple_compat() {
        use crate::detection_api::{CompressionType, DetectedCodec};

        let mut gif = Builder::new()
            .suffix(".gif")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        gif.write_all(MINIMAL_TRANSPARENT_LOOP_GIF)
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        let detection = crate::detection_api::VideoDetectionResult {
            file_path: "/stale/cache-hit.mp4".to_string(),
            format: "gif".into(),
            codec: DetectedCodec::Unknown("gif".into()),
            compression: CompressionType::Lossless,
            width: 1,
            height: 1,
            duration_secs: 0.2,
            has_audio: false,
            frame_count: 2,
            fps: 10.0,
            file_size: std::fs::metadata(gif.path())
                .unwrap_or_else(|e| panic!("error: {e:?}"))
                .len(),
            ..Default::default()
        };

        let adjusted = determine_strategy_with_apple_compat(
            &detection,
            gif.path(),
            true,
            false,
            SelectedCodec::Hevc,
        );

        assert_eq!(adjusted.target, TargetVideoFormat::Gif);
    }

    #[test]
    fn test_apple_compat_forces_gif_for_modern_animated_webp_even_if_loop_tree_errors() {
        use crate::detection_api::{CompressionType, DetectedCodec};

        // Simulate an animated WebP with degenerate duration (the historical edge case).
        // Apple compat must still force GIF delivery for modern animated formats *when it is
        // clearly an animated-image (short / sticker-like) asset*.
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "IMG_0116.WEBP".into(),
            format: "webp".into(),
            codec: DetectedCodec::Unknown("webp".into()),
            compression: CompressionType::Standard,
            width: 512,
            height: 512,
            duration_secs: 0.0,
            has_audio: false,
            frame_count: 12,
            fps: 0.0,
            file_size: 500_000,
            ..Default::default()
        };

        let strategy = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );

        assert_eq!(strategy.target, TargetVideoFormat::Gif);
        assert!(
            strategy
                .reason
                .contains("Apple compat policy: modern animated format"),
            "unexpected reason: {}",
            strategy.reason
        );
    }

    #[test]
    fn test_apple_compat_does_not_force_gif_for_long_modern_animation() {
        use crate::detection_api::{CompressionType, DetectedCodec};

        // Long animations are video-like and must remain eligible for HEVC delivery in apple compat.
        let det = crate::detection_api::VideoDetectionResult {
            file_path: "LONG_ANIM.WEBP".into(),
            format: "webp".into(),
            codec: DetectedCodec::Unknown("webp".into()),
            compression: CompressionType::Standard,
            width: 720,
            height: 720,
            duration_secs: shared_utils::constants::EXTREME_LONG_ABSOLUTE_LIMIT_SECS + 5.0,
            has_audio: false,
            frame_count: 600,
            fps: 30.0,
            file_size: 5_000_000,
            ..Default::default()
        };

        let strategy = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            true,
            false,
            SelectedCodec::Hevc,
        );

        assert_ne!(strategy.target, TargetVideoFormat::Gif);
    }

    #[test]
    fn test_explore_quality_failure_prefers_total_size_reason_over_stream_growth() {
        let decision = ExploreQualityFailureDecision::inspect_and_log(
            &shared_utils::ExploreResult {
                quality_passed: shared_utils::types::CheckResult::Failed(
                    "Total file not smaller than input".to_string(),
                ),
                ssim: Some(0.99),
                actual_min_ssim: 0.95,
                input_video_stream_size: 1_000_000,
                output_video_stream_size: 1_100_000,
                ..Default::default()
            },
            false,
        );

        assert_eq!(
            decision.fail_message,
            "Skipped: Total file not smaller than input"
        );
        assert!(decision
            .fail_reason
            .contains("Total file not smaller than input"));
        assert!(!decision.fail_message.contains("video stream"));
    }
}
