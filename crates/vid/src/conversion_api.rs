//! Executes video conversions based on detection results (HEVC and AV1 support).

use crate::detection_api::Detection;
use crate::{Rational, Result, VidQualityError};

#[cfg(test)]
use foundation::MediaPrecision;
use foundation::analysis_cache::AnalysisCache;
use foundation::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, SelectedCodec, TargetVideoFormat,
};
use foundation::{log_detail, log_success};
use std::path::Path;
use std::path::PathBuf;
// no-op (removed tracing::info)

fn convert_options_from_config(
    config: &ConversionConfig,
) -> foundation::conversion::ConvertOptions {
    foundation::delivery_codec_strategy::build_video_convert_options(
        &foundation::delivery_codec_strategy::RunDeliveryFlags::from_conversion_config(config),
    )
}

/// True when the asset is an animated **raster** (not a video container) that needs
/// `animated_image::convert_to_mp4_matched` preprocessing (JXL→APNG, WebP mux, etc.).
fn should_delegate_to_animated_mp4_matched(input: &Path, detection: &Detection) -> Result<bool> {
    match foundation::quality_matcher::SourceCodec::identify_by_content(input).map_err(|err| {
        VidQualityError::ConversionError(format!(
            "Failed to identify source codec for {}: {err}",
            input.display()
        ))
    })? {
        Some(codec) if codec.is_video() => return Ok(false),
        Some(codec) if codec.can_be_animated() => return Ok(true),
        _ => {}
    }
    let fmt = detection.format.to_ascii_lowercase();
    Ok(matches!(
        fmt.as_str(),
        "gif" | "webp" | "jxl" | "avif" | "heic" | "heif" | "apng" | "png" | "bmp"
    ))
}

fn animated_raster_has_alpha(input: &Path) -> bool {
    let ext = foundation::media_conversion_gate::path_extension_lowercase_or_empty(
        input,
        "animated_raster_alpha",
    );
    matches!(
        ext.as_str(),
        "webp" | "gif" | "jxl" | "apng" | "png" | "avif"
    )
}

fn concurrent_output_skip_conversion_output(
    input: &Path,
    detection: &Detection,
    strategy: ConversionStrategy,
) -> ConversionOutput {
    ConversionOutput {
        input_path: input.display().to_string(),
        output_path: String::new(),
        strategy,
        input_size: detection.file_size,
        output_size: 0,
        size_ratio: 1.0,
        success: true,
        message: "Skipped: output was created concurrently".to_string(),
        final_crf: 0.0,
        exploration_attempts: 0,
        blake3: None,
        ignored: false,
    }
}

fn task_result_to_conversion_output(
    input: &Path,
    detection: &Detection,
    strategy: ConversionStrategy,
    result: foundation::conversion::TaskResult,
    final_crf: f32,
    exploration_attempts: u8,
    cache: Option<&AnalysisCache>,
) -> Result<ConversionOutput> {
    if result.ignored {
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),
            strategy,
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 0.0,
            success: false,
            message: result.message,
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: result.blake3,
            ignored: true,
        });
    }

    if !result.success {
        let output_size = result.output_size.unwrap_or(detection.file_size);
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: result
                .output_path
                .unwrap_or_else(|| input.display().to_string()),
            strategy,
            input_size: detection.file_size,
            output_size,
            size_ratio: if detection.file_size == 0 {
                1.0
            } else {
                Rational::from((output_size, detection.file_size)).to_f64()
            },
            success: false,
            message: result.message,
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: result.blake3,
            ignored: false,
        });
    }

    if result.skipped {
        let output_path = result.output_path.unwrap_or_else(|| {
            foundation::media_conversion_gate::delivery_pipeline_batch_audit(
                "animated_task_skip_missing_output_path",
                format!(
                    "{}: skip result missing output_path; reporting empty",
                    input.display()
                ),
            );
            String::new()
        });
        let (output_size, size_ratio) = if let Some(size) = result.output_size {
            let ratio = if detection.file_size > 0 {
                Rational::from((size, detection.file_size)).to_f64()
            } else {
                1.0_f64
            };
            (size, ratio)
        } else {
            foundation::media_conversion_gate::delivery_pipeline_batch_audit(
                "animated_task_skip_missing_output_size",
                format!(
                    "{}: skip result missing output_size; size_ratio=1.0 (not fabricated 0/input)",
                    input.display()
                ),
            );
            (0, 1.0_f64)
        };
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
            blake3: result.blake3,
            ignored: false,
        });
    }

    let output_path = result.output_path.ok_or_else(|| {
        VidQualityError::ConversionError(
            "animated raster conversion succeeded but output path is missing".to_string(),
        )
    })?;
    let output_size = result.output_size.ok_or_else(|| {
        VidQualityError::ConversionError(
            "animated raster conversion succeeded but output size is missing".to_string(),
        )
    })?;
    let size_ratio = if detection.file_size > 0 {
        Rational::from((output_size, detection.file_size)).to_f64()
    } else {
        1.0_f64
    };

    if result.success
        && let Some(cache) = cache
    {
        let mut det = detection.clone();
        det.precision.last_best_crf = Some(final_crf);
        det.precision.last_best_effort_crf = None;
        if let Err(e) = cache.store_video_analysis(input, &det) {
            foundation::media_conversion_gate::video_cache_store_failed_audit(
                input,
                "animated-mp4-matched-crf-hint",
                e,
            );
        }
    }

    Ok(ConversionOutput {
        input_path: input.display().to_string(),
        output_path,
        strategy,
        input_size: detection.file_size,
        output_size,
        size_ratio,
        success: result.success,
        message: result.message,
        final_crf,
        exploration_attempts,
        blake3: result.blake3,
        ignored: false,
    })
}

fn cleanup_output_file(path: &Path, context: &str) {
    foundation::media_conversion_gate::delivery_remove_file_or_audit(context, path);
}

struct ExploreGateRejectionDecision {
    failed: bool,
    reason: String,
    message: String,
    protect_msg: String,
    delete_msg: String,
    label: &'static str,
}

impl ExploreGateRejectionDecision {
    fn inspect_and_log(input: &Path, explore_result: &foundation::ExploreResult) -> Self {
        let ultimate_contract = explore_result.uses_ultimate_quality_contract();
        let actual_ssim = explore_result.ssim;
        let threshold = explore_result.actual_min_ssim;
        if ultimate_contract {
            let reason = foundation::media_conversion_gate::explore_quality_fail_reason(
                explore_result.quality_passed.failure_reason(),
                explore_result.enhanced_verify_fail_reason.as_deref(),
                input,
            );
            foundation::media_conversion_gate::explore_quality_gate_audit(
                "explore_quality_ultimate",
                input,
                &reason,
            );
            return Self {
                failed: true,
                reason: format!("Quality validation failed: {reason}"),
                message: format!("Failed: {reason}"),
                protect_msg: foundation::infra::static_logs::messages::PROTECT_QUALITY_SIZE
                    .to_string(),
                delete_msg: foundation::infra::static_logs::messages::DISCARD_QUALITY_SIZE
                    .to_string(),
                label: foundation::infra::static_logs::messages::LABEL_QUALITY_FAIL,
            };
        }

        if !actual_ssim.is_some_and(f64::is_finite) {
            foundation::media_conversion_gate::explore_quality_gate_audit(
                "explore_quality_ssim_missing",
                input,
                foundation::infra::static_logs::messages::MSG_SSIM_NA_DETAIL,
            );
            return Self {
                failed: true,
                reason: "SSIM calculation failed".to_string(),
                message: "Failed: SSIM calculation failed".to_string(),
                protect_msg: foundation::infra::static_logs::messages::PROTECT_SSIM_NA.to_string(),
                delete_msg: foundation::infra::static_logs::messages::DISCARD_SSIM_FAIL.to_string(),
                label: foundation::infra::static_logs::messages::LABEL_SSIM_CALC_FAILED,
            };
        }

        if let Some(actual_ssim) = actual_ssim.filter(|ssim| *ssim < threshold) {
            foundation::media_conversion_gate::explore_quality_gate_audit(
                "explore_quality_ssim_low",
                input,
                format!("SSIM {actual_ssim:.4} < {threshold:.4}"),
            );
            return Self {
                failed: true,
                reason: format!(
                    "Quality validation failed: SSIM {actual_ssim:.4} < {threshold:.4}"
                ),
                message: format!("Failed: SSIM {actual_ssim:.4} below threshold {threshold:.4}"),
                protect_msg: foundation::infra::static_logs::messages::PROTECT_QUALITY_LOW
                    .to_string(),
                delete_msg: foundation::infra::static_logs::messages::DISCARD_QUALITY_LOW
                    .to_string(),
                label: foundation::infra::static_logs::messages::LABEL_QUALITY_FAIL,
            };
        }

        let reason = foundation::media_conversion_gate::explore_quality_fail_reason(
            explore_result.quality_passed.failure_reason(),
            explore_result.enhanced_verify_fail_reason.as_deref(),
            input,
        );
        foundation::media_conversion_gate::explore_quality_gate_audit(
            "explore_quality_size",
            input,
            &reason,
        );
        Self {
            failed: false,
            reason: format!("Optimization target not met: {reason}"),
            message: format!("Skipped: {reason}"),
            protect_msg: foundation::infra::static_logs::messages::PROTECT_QUALITY_SIZE.to_string(),
            delete_msg: foundation::infra::static_logs::messages::DISCARD_QUALITY_SIZE.to_string(),
            label: foundation::infra::static_logs::messages::LABEL_QUALITY_FAIL,
        }
    }

    fn emit(&self, input: &Path) {
        if !self.failed {
            foundation::progress_mode::video_skipped(input, &self.message);
        }
        foundation::media_conversion_gate::explore_quality_skip_summary_audit(
            self.label,
            &self.protect_msg,
            &self.delete_msg,
        );
    }

    fn into_output(
        self,
        input: &Path,
        detection: &Detection,
        explore_result: &foundation::ExploreResult,
    ) -> Result<ConversionOutput> {
        Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: input.display().to_string(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: self.reason,
                command: String::new(),
                preserve_audio: detection.flags.streams.has_audio,
                crf: explore_result.optimal_crf,
                lossless: false,
            },
            input_size: detection.file_size,
            output_size: detection.file_size,
            size_ratio: 1.0,
            success: !self.failed,
            message: self.message,
            final_crf: explore_result.optimal_crf,
            exploration_attempts: u8::try_from(explore_result.iterations).map_err(|_| {
                foundation::unified_error::UnifiedError::IterationLimitExceeded(
                    foundation::IterationError {
                        current: explore_result.iterations,
                        max: foundation::constants::EXPLORATION_ITERATION_LIMIT,
                        context: format!("Exploration attempts overflow for {}", input.display()),
                    },
                )
            })?,
            blake3: None,
            ignored: false,
        })
    }
}

struct FinalQualityGateFailureDecision {
    quality_summary: String,
    reason: String,
    message: String,
}

impl FinalQualityGateFailureDecision {
    fn inspect_and_log(input: &Path, result: &foundation::ExploreResult) -> Self {
        let ultimate_contract = result.uses_ultimate_quality_contract();
        let quality_summary = if ultimate_contract {
            foundation::media_conversion_gate::explore_ultimate_summary_display(
                result.ultimate_quality_summary(),
                &format!("video final gate {}", input.display()),
            )
        } else {
            foundation::media_conversion_gate::explore_ms_ssim_score_prefixed(
                result.ms_ssim_score,
                &format!("video final gate {}", input.display()),
            )
        };
        let failure_label = if ultimate_contract {
            foundation::infra::static_logs::messages::MSG_3D_GATE_FAILED
        } else {
            foundation::infra::static_logs::messages::MSG_QUALITY_TARGET_FAILED
        };
        foundation::media_conversion_gate::explore_quality_gate_audit(
            "explore_quality_final_gate",
            input,
            format!(
                "{failure_label} ({quality_summary}) │ original protected (quality below threshold)"
            ),
        );

        Self {
            reason: if ultimate_contract {
                format!("3D quality gate failed ({quality_summary})")
            } else {
                format!("Quality target failed ({quality_summary})")
            },
            message: if ultimate_contract {
                format!("Failed: 3D quality gate failed ({quality_summary})")
            } else {
                format!(
                    "Failed: MS-SSIM {quality_summary} below target {:.2}",
                    foundation::constants::VIDEO_QUALITY_GATE_THRESHOLD
                )
            },
            quality_summary,
        }
    }

    fn into_failed_output(
        self,
        input: &Path,
        detection: &Detection,
        result: &foundation::ExploreResult,
    ) -> Result<ConversionOutput> {
        Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: input.display().to_string(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: self.reason,
                command: String::new(),
                preserve_audio: detection.flags.streams.has_audio,
                crf: result.optimal_crf,
                lossless: false,
            },
            input_size: detection.file_size,
            output_size: detection.file_size,
            size_ratio: 1.0,
            success: false,
            message: self.message,
            final_crf: result.optimal_crf,
            exploration_attempts: u8::try_from(result.iterations).map_err(|_| {
                foundation::unified_error::UnifiedError::IterationLimitExceeded(
                    foundation::IterationError {
                        current: result.iterations,
                        max: foundation::constants::EXPLORATION_ITERATION_LIMIT,
                        context: format!("Exploration attempts overflow for {}", input.display()),
                    },
                )
            })?,
            blake3: None,
            ignored: false,
        })
    }
}

/// Build `FFmpeg` HDR metadata arguments from detection results.
/// Preserves primaries, transfer characteristics, matrix, and static HDR10 metadata.
fn build_hdr_ffmpeg_args(detection: &Detection) -> Vec<String> {
    foundation::build_yuv_output_ffmpeg_color_args(
        detection.color_space.yuv_output_colorspace(),
        detection.color_transfer.as_deref(),
        detection.color_primaries.as_deref(),
    )
}

#[cfg(test)]
fn requires_high_bit_depth_encode(detection: &Detection) -> bool {
    detection.should_preserve_high_bit_depth()
}

fn build_hevc_x265_extra_params(
    detection: &Detection,
    output_pix_fmt: &str,
    dv_rpu: Option<&DvRpuResult>,
    hdr10plus: Option<&Hdr10PlusResult>,
) -> String {
    let mut extra_x265_params = String::new();

    if let Some(dv) = dv_rpu {
        foundation::x265_params::push_param(
            &mut extra_x265_params,
            &format!(
                "dolby-vision-rpu={}:dolby-vision-profile={}",
                dv.rpu_path.display(),
                dv.profile_str
            ),
        );
    }

    foundation::append_x265_hdr10_params(
        &mut extra_x265_params,
        Some(detection.color_space.as_str()),
        detection.color_transfer.as_deref(),
        detection.color_primaries.as_deref(),
        detection.mastering_display.as_deref(),
        detection.max_cll.as_deref(),
        detection.flags.hdr.is_hdr10_plus,
        output_pix_fmt,
        hdr10plus.map(|hdr| hdr.json_path.as_path()),
    );

    extra_x265_params
}

/// Return the correct pixel format (10-bit for HDR/high-precision preservation,
/// otherwise 8-bit).
fn hdr_pix_fmt(detection: &Detection) -> &'static str {
    foundation::hevc_yuv420_output_pix_fmt(detection)
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
fn prepare_dv_rpu(detection: &Detection) -> Option<DvRpuResult> {
    if !detection.flags.hdr.is_dolby_vision {
        return None;
    }

    if !foundation::is_dovi_tool_available() {
        foundation::log_static!(
            warn,
            foundation::infra::static_logs::messages::DOVI_TOOL_MISSING
        );
        foundation::log_static!(
            warn,
            foundation::infra::static_logs::messages::DOVI_TOOL_INSTALL
        );
        return None;
    }

    let temp_dir = match foundation::media_conversion_gate::delivery_temp_dir_in_scratch_or_err(
        "dv_rpu_temp_dir",
        "mfb-dv-",
    ) {
        Ok(d) => d,
        Err(e) => {
            foundation::media_conversion_gate::hdr_metadata_fallback_audit(
                "dv_rpu_temp_dir",
                Path::new(&detection.file_path),
                format!("failed to create temp dir: {e}"),
            );
            return None;
        }
    };

    let input_path = Path::new(&detection.file_path);

    // Step 1: Extract raw HEVC Annex-B bitstream
    let raw_hevc = match foundation::extract_hevc_bitstream(input_path, temp_dir.path()) {
        Ok(p) => p,
        Err(e) => {
            foundation::media_conversion_gate::hdr_metadata_fallback_audit(
                "dv_rpu_bitstream",
                input_path,
                format!("bitstream extraction failed: {e}"),
            );
            foundation::log_detail!(
                "HDR Recovery: Downgrading to HDR10 static layer (dynamic metadata extraction failed)"
            );
            return None;
        }
    };

    // Step 2: Extract RPU (and convert Profile 7 → 8.1 if needed)
    let rpu_path =
        match foundation::extract_dv_rpu(&raw_hevc, temp_dir.path(), detection.dv_profile) {
            Ok(p) => p,
            Err(e) => {
                foundation::media_conversion_gate::hdr_metadata_fallback_audit(
                    "dv_rpu_extract",
                    input_path,
                    e.to_string(),
                );
                foundation::log_detail!(
                    foundation::infra::static_logs::messages::HDR_RECOVERY_DOWNGRADE
                );
                return None;
            }
        };

    // Step 3: Determine x265 profile string
    let Some(profile_str) = foundation::dv_x265_profile_string(
        detection.dv_profile,
        detection.dv_bl_signal_compatibility_id,
    ) else {
        foundation::media_conversion_gate::hdr_metadata_fallback_audit(
            "dv_profile_unsupported",
            input_path,
            format!(
                "unsupported DV profile {:?} for x265; falling back to HDR10",
                detection.dv_profile
            ),
        );
        return None;
    };

    foundation::log_success!(
        foundation::infra::static_logs::messages::LABEL_METADATA,
        &format!(
            "HDR Audit: Dolby Vision RPU successfully harvested (Profile {profile_str} preserved)"
        )
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

fn hdr10plus_fail_closed_error(detection: &Detection, detail: impl AsRef<str>) -> VidQualityError {
    VidQualityError::ConversionError(format!(
        "HDR10+ dynamic metadata extraction failed closed for {}: {}. Static HDR10 fallback requires explicit opt-in.",
        detection.file_path,
        detail.as_ref()
    ))
}

fn hdr10plus_tool_missing_decision(detection: &Detection) -> Result<()> {
    Err(hdr10plus_fail_closed_error(
        detection,
        "hdr10plus_tool unavailable; fail closed",
    ))
}

/// Attempt to extract HDR10+ dynamic metadata for injection into x265.
/// Returns `Ok(None)` only if content is not HDR10+ or static HDR10 fallback was explicitly allowed.
fn prepare_hdr10plus_metadata(
    detection: &Detection,
    allow_static_fallback: bool,
) -> Result<Option<Hdr10PlusResult>> {
    if !detection.flags.hdr.is_hdr10_plus {
        return Ok(None);
    }

    if !foundation::hdr::is_hdr10plus_tool_available() {
        foundation::log_static!(
            warn,
            foundation::infra::static_logs::messages::HDR10PLUS_TOOL_MISSING
        );
        if allow_static_fallback {
            foundation::media_conversion_gate::hdr_metadata_fallback_audit(
                "hdr10plus_tool_missing_static_fallback_opt_in",
                Path::new(&detection.file_path),
                "caller explicitly allowed static HDR10 fallback",
            );
            return Ok(None);
        }
        return hdr10plus_tool_missing_decision(detection).map(|()| None);
    }

    let temp_dir = match foundation::media_conversion_gate::delivery_temp_dir_in_scratch_or_err(
        "hdr10plus_temp_dir",
        "mfb-hdr10p-",
    ) {
        Ok(d) => d,
        Err(e) => {
            foundation::media_conversion_gate::hdr_metadata_fallback_audit(
                "hdr10plus_temp_dir",
                Path::new(&detection.file_path),
                format!("failed to create temp dir: {e}"),
            );
            if allow_static_fallback {
                return Ok(None);
            }
            return Err(hdr10plus_fail_closed_error(
                detection,
                format!("failed to create temp dir: {e}"),
            ));
        }
    };

    let input_path = Path::new(&detection.file_path);

    // Step 1: Extract raw HEVC Annex-B bitstream
    let raw_hevc = match foundation::extract_hevc_bitstream(input_path, temp_dir.path()) {
        Ok(p) => p,
        Err(e) => {
            foundation::media_conversion_gate::hdr_metadata_fallback_audit(
                "hdr10plus_bitstream",
                input_path,
                format!("bitstream extraction failed: {e}"),
            );
            if allow_static_fallback {
                return Ok(None);
            }
            return Err(hdr10plus_fail_closed_error(
                detection,
                format!("bitstream extraction failed: {e}"),
            ));
        }
    };

    // Step 2: Extract HDR10+ JSON
    let json_path = match foundation::hdr::extract_hdr10plus_metadata(&raw_hevc, temp_dir.path()) {
        Ok(p) => p,
        Err(e) => {
            foundation::media_conversion_gate::hdr_metadata_fallback_audit(
                "hdr10plus_extract",
                input_path,
                e.to_string(),
            );
            if allow_static_fallback {
                return Ok(None);
            }
            return Err(hdr10plus_fail_closed_error(detection, e.to_string()));
        }
    };

    foundation::log_success!(
        foundation::infra::static_logs::messages::LABEL_METADATA,
        foundation::infra::static_logs::messages::MSG_HDR10PLUS_HARVEST_SUCCESS
    );

    Ok(Some(Hdr10PlusResult {
        json_path,
        _temp_dir: temp_dir,
    }))
}

#[inline]
const fn hevc_delivery_target(apple_compat: bool) -> TargetVideoFormat {
    if apple_compat {
        TargetVideoFormat::HevcMov
    } else {
        TargetVideoFormat::HevcMp4
    }
}

#[must_use]
pub fn determine_strategy_with_apple_compat(
    result: &Detection,
    input: &Path,
    apple_compat: bool,
    force: bool,
    codec: SelectedCodec,
) -> ConversionStrategy {
    log_detail!(&format!(
        "Pipeline Decision: Analyzing optimal strategy for {} (AppleCompat={}, Force={}, Codec={})",
        input.display(),
        apple_compat,
        force,
        codec.as_str()
    ));
    if let Err(reason) = codec.validate_delivery_flags(apple_compat) {
        return ConversionStrategy {
            target: TargetVideoFormat::Skip,
            reason: reason.to_string(),
            command: String::new(),
            preserve_audio: false,
            crf: 0.0,
            lossless: false,
        };
    }

    let skip_decision = if apple_compat {
        foundation::should_skip_video_codec_apple_compat(result.codec.as_str())
    } else {
        foundation::should_skip_video_codec(result.codec.as_str())
    };

    let mut detection = result.clone();
    detection.file_path = input.display().to_string();

    // Loop Intent Identification System
    // For GIF files, use fast-path (from_gif_path) to preserve GIF-specific signals.
    // For videos, use ffprobe path with structural signal refresh.
    let loop_verdict = if foundation::should_use_gif_fast_path(input) {
        // GIF file: use header-level detection
        match foundation::LoopMeta::from_gif_path(input) {
            None => {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "gif_loop_meta",
                    input,
                    "GIF loop-meta fast path unavailable; using full detection assess",
                );
                foundation::assess_loop_intent(&detection)
            }
            Some(meta) => foundation::assess_loop_intent_from_meta(&meta, Some(input)),
        }
    } else {
        // Video file: ensure structural signals are available
        if detection.pkt_sizes.len() < 3 || detection.pts_deltas.len() < 3 {
            match crate::detection_api::detect_video_with_cache(input, None) {
                Ok(fresh) => {
                    detection = fresh;
                    detection.file_path = input.display().to_string();
                    foundation::assess_loop_intent(&detection)
                }
                Err(err) => {
                    foundation::media_conversion_gate::probe_layer_batch_audit(
                        "loop_intent_signal_refresh_failed",
                        format!(
                            "loop-intent structural refresh failed for {}: {err}",
                            input.display()
                        ),
                    );
                    foundation::LoopIntentVerdict::Error(format!(
                        "structural signal refresh failed for {}: {err}",
                        input.display()
                    ))
                }
            }
        } else {
            foundation::assess_loop_intent(&detection)
        }
    };

    // Centralized Apple-compat delivery policy lives in `foundation::loop_intent`.
    // `vid` should only orchestrate and convert, not define compatibility policy.
    let meta_for_policy = foundation::LoopMeta::from_video_detection(&detection);
    let loop_verdict = foundation::apply_apple_compat_modern_animation_policy(
        loop_verdict,
        &meta_for_policy,
        apple_compat,
        force,
    );

    let is_loop_intent = loop_verdict.is_keep_gif();

    // ══════════════════════════════════════════════════════════════════════════════
    // LOOP ERROR HANDLING: Skip on impossible or conflicting signals
    // ══════════════════════════════════════════════════════════════════════════════
    if let foundation::LoopIntentVerdict::Error(reason) = loop_verdict {
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
            foundation::should_skip_video_codec_apple_compat(s)
        } else {
            foundation::should_skip_video_codec(s)
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

    let (target, reason, crf, lossless) = if let (
        crate::detection_api::CompressionType::Lossless,
        _,
    ) = (result.compression, result.format.as_str())
    {
        if !codec.supports_lossless_archival_mkv() {
            return ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: format!(
                    "Source is lossless but --codec {} cannot use archival MKV; re-run with --codec hevc",
                    codec.as_str()
                ),
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            };
        }
        let target = codec.delivery_target(apple_compat, true);
        (
            target,
            format!(
                "Source is lossless - using {} Lossless MKV (delivery archival container)",
                codec.delivery_label_prefix()
            ),
            0.0_f32,
            true,
        )
    } else {
        let target = codec.delivery_target(apple_compat, false);
        let reason_prefix = codec.delivery_label_prefix();
        if result.flags.content.archival_candidate
            || result.quality_score >= foundation::constants::QUALITY_SCORE_HIGH_THRESHOLD
        {
            (
                target,
                format!(
                    "Source is high quality ({}) - compressing with {} CRF {} (visually lossless)",
                    result.codec.as_str(),
                    reason_prefix,
                    foundation::constants::CRF_TARGET_VISUALLY_LOSSLESS
                ),
                foundation::constants::CRF_TARGET_VISUALLY_LOSSLESS,
                false,
            )
        } else {
            (
                target,
                format!(
                    "Source is {} ({}) - compressing with {} CRF {}",
                    result.codec.as_str(),
                    result.compression.as_str(),
                    reason_prefix,
                    foundation::constants::CRF_TARGET_STANDARD
                ),
                foundation::constants::CRF_TARGET_STANDARD,
                false,
            )
        }
    };

    ConversionStrategy {
        target,
        reason,
        command: String::new(),
        preserve_audio: result.flags.streams.has_audio,
        crf,
        lossless,
    }
}

/// Automatically convert video with caching.
///
/// # Errors
/// Returns an error if video detection fails, strategy cannot be determined, or conversion execution fails.
///
/// # Panics
///
/// Panics if internal string formatting fails during x265 parameter construction.
/// This is considered a logic error as standard formatting to a string should be infallible.
pub fn auto_convert_with_cache(
    input: &Path,
    config: &ConversionConfig,
    cache: Option<&AnalysisCache>,
) -> Result<ConversionOutput> {
    let span = tracing::info_span!("video_conversion", file = %input.display());
    let _enter = span.enter();

    // Pause if the user is being prompted to exit via Ctrl+C
    foundation::ctrlc_guard::wait_if_prompt_active();

    // Validate input file (check symlinks, file type, readability)
    if let Err(e) = foundation::conversion::validate_input_file(input) {
        return Err(VidQualityError::ConversionError(e));
    }

    let label = foundation::media_conversion_gate::path_file_name_for_log(input);
    foundation::progress_mode::set_log_context(&label);
    let _log_guard = foundation::progress_mode::LogContextGuard;

    // Skip Live Photos in Apple compat mode
    if config.apple_compat() && foundation::live_photo::is_live(input) {
        let reason = "Live Photo Audit: Asset skipped in Apple Compatibility mode to preserve paired HEIC/MOV association and avoid sidecar fragmentation.";
        foundation::progress_mode::video_skipped(input, reason);

        let file_size = std::fs::metadata(input)
            .map_err(|e| {
                VidQualityError::ConversionError(format!(
                    "Failed to read Live Photo metadata for {}: {e}",
                    input.display()
                ))
            })?
            .len();

        foundation::copy_on_skip_or_fail(
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
            message: format!("Skipped Live Photo in Apple compat mode: {reason}"),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
            ignored: false,
        });
    }
    let mut detection = crate::detection_api::detect_video_with_cache(input, cache)?;

    let mut static_image_forced = false;
    let detected_input_format = foundation::image_detection::detect_format_from_bytes(input)
        .map_err(|err| {
            foundation::media_conversion_gate::probe_image_format_audit(
                "vid_conversion_format_detect_failed",
                input,
                format!("vid conversion refused routing guess after format detection error: {err}"),
            );
            VidQualityError::ConversionError(format!(
                "Failed to detect true input format for {}: {err}",
                input.display()
            ))
        })?;

    // Safety override: if an animation-capable image is proven static, force
    // single-frame so vid ignores it without producing or copying output.
    match &detected_input_format {
        foundation::image_detection::DetectedFormat::PNG
        | foundation::image_detection::DetectedFormat::JPEG
        | foundation::image_detection::DetectedFormat::GIF
        | foundation::image_detection::DetectedFormat::WebP
        | foundation::image_detection::DetectedFormat::AVIF
        | foundation::image_detection::DetectedFormat::JXL
        | foundation::image_detection::DetectedFormat::TIFF
        | foundation::image_detection::DetectedFormat::BMP
        | foundation::image_detection::DetectedFormat::ICO
        | foundation::image_detection::DetectedFormat::EXR
        | foundation::image_detection::DetectedFormat::QOI
        | foundation::image_detection::DetectedFormat::JP2
        | foundation::image_detection::DetectedFormat::HEIC
        | foundation::image_detection::DetectedFormat::HEIF => {
            match foundation::image_detection::detect_animation(input, &detected_input_format) {
                Ok((false, native_frames, _)) => match native_frames {
                    Some(n) if n <= 1 => {
                        log_info!(
                            foundation::infra::static_logs::messages::LABEL_VID_SAFEGUARD,
                            &format!(
                                "Static image safeguard: measured frame_count={n} for {}; vid will ignore",
                                input.display()
                            )
                        );
                        detection.frame_count = Some(u64::from(n));
                        detection.duration_secs = None;
                        static_image_forced = true;
                    }
                    None => {
                        log_info!(
                            foundation::infra::static_logs::messages::LABEL_VID_SAFEGUARD,
                            &format!(
                                "Static image safeguard: animation probe static, frame_count absent for {}; vid will ignore without forging fc=1",
                                input.display()
                            )
                        );
                        detection.frame_count = None;
                        detection.duration_secs = None;
                        static_image_forced = true;
                    }
                    Some(n) => {
                        foundation::media_conversion_gate::probe_layer_audit(
                            "vid_static_safeguard_implausible_frames",
                            input,
                            format!(
                                "Static safeguard skipped: implausible measured frame_count={n}"
                            ),
                        );
                    }
                },
                Ok((true, _, _)) => {}
                Err(err) => {
                    foundation::media_conversion_gate::probe_image_format_audit(
                        "vid_static_safeguard_animation_detect_failed",
                        input,
                        format!(
                            "vid conversion refused static-image safeguard guess after animation detection error: {err}"
                        ),
                    );
                    return Err(VidQualityError::ConversionError(format!(
                        "Failed to verify animation state for {}: {err}",
                        input.display()
                    )));
                }
            }
        }
        _ => {}
    }

    // Internal judgment reconciliation:
    // If vid sees single-frame on a format that can be animated, re-check with image_detection
    // (which includes structural + penetration animation verification) before static isolation.
    let content_codec = foundation::quality_matcher::SourceCodec::identify_by_content(input)
        .map_err(|err| {
            VidQualityError::ConversionError(format!(
                "Failed to identify source codec for {} during animation reconciliation: {err}",
                input.display()
            ))
        })?;
    let content_codec_can_be_animated = content_codec.is_some_and(|codec| codec.can_be_animated());
    if !static_image_forced
        && detection.frame_count.is_none_or(|fc| fc <= 1)
        && content_codec_can_be_animated
    {
        if matches!(
            &detected_input_format,
            foundation::image_detection::DetectedFormat::PNG
                | foundation::image_detection::DetectedFormat::JPEG
                | foundation::image_detection::DetectedFormat::GIF
                | foundation::image_detection::DetectedFormat::WebP
        ) {
            let image_det = foundation::image_detection::detect_image(input).map_err(|err| {
                foundation::media_conversion_gate::probe_image_format_audit(
                    "vid_reconciliation_image_detect_failed",
                    input,
                    format!(
                        "vid conversion refused animated/static reconciliation guess after image detection error: {err}"
                    ),
                );
                VidQualityError::ConversionError(format!(
                    "Failed to reconcile animated image metadata for {}: {err}",
                    input.display()
                ))
            })?;
            let image_is_animated = matches!(
                image_det.image_type,
                foundation::image_detection::ImageType::Animated
            );
            let decoded_frame_count =
                if image_is_animated && image_det.frame_count.is_none_or(|count| count <= 1) {
                    match foundation::media_penetration::detect_real_frame_count(
                        input,
                        image_det.frame_count.map(u64::from),
                    ) {
                        foundation::media_penetration::PenetrationResult::Verified(count) => {
                            Some(count)
                        }
                        foundation::media_penetration::PenetrationResult::Failed
                        | foundation::media_penetration::PenetrationResult::Skipped => None,
                    }
                } else {
                    image_det.frame_count.map(u64::from)
                };

            if let Some(corrected) = decoded_frame_count.filter(|count| *count > 1) {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "media_reconciliation_frame_count",
                    input,
                    format!(
                        "structure mismatch (vid {}, image penetration {corrected}); applying frame_count correction",
                        foundation::media_conversion_gate::delivery_frame_count_label_u64(
                            detection.frame_count,
                            &format!("vid reconciliation {}", input.display()),
                        ),
                    ),
                );
                detection.frame_count = Some(corrected);

                if detection.duration_secs.is_none_or(|d| d <= 0.0_f64)
                    && let Some(dur) = image_det.duration
                    && dur > 0.0
                {
                    detection.duration_secs = Some(f64::from(dur));
                }
            } else if image_is_animated {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "animated_frame_count_unverified",
                    input,
                    format!(
                        "animated image evidence but penetration could not verify frame count; leaving vid metadata (vid saw {})",
                        foundation::media_conversion_gate::delivery_frame_count_label_u64(
                            detection.frame_count,
                            &format!("vid animated evidence {}", input.display()),
                        )
                    ),
                );
            }
        } else {
            foundation::log_detail!(&format!(
                "[VID-RECONCILIATION] Bypassing deep pixel decode for modern format {detected_input_format:?} (not natively supported by standard image decoder)",
            ));
        }
    }

    match foundation::promote_animated_container_for_vid(input, &mut detection) {
        Ok(true) => {
            foundation::log_detail!(&format!(
                "Animated container promoted for vid after ffprobe/frame_count gap: {}",
                input.display()
            ));
        }
        Ok(false) => {}
        Err(err) => {
            let message = format!(
                "animated container promotion failed for {}: {err}",
                input.display()
            );
            foundation::media_conversion_gate::probe_layer_batch_audit(
                "animated_container_promotion_failed",
                &message,
            );
            return Err(VidQualityError::ConversionError(message));
        }
    }

    // Short clips may over-report frame_count on first pass; refresh sparse structural signals
    // before static isolation (matches `determine_strategy_with_apple_compat` refresh path).
    if detection.pkt_sizes.len() < 3 || detection.pts_deltas.len() < 3 {
        detection = crate::detection_api::detect_video_with_cache(input, cache).map_err(|err| {
            let message = format!(
                "static isolation structural refresh failed for {}: {err}",
                input.display()
            );
            foundation::media_conversion_gate::probe_layer_batch_audit(
                "static_isolation_signal_refresh_failed",
                &message,
            );
            VidQualityError::ConversionError(message)
        })?;
        detection.file_path = input.display().to_string();
    }

    // --- Strict Animated Isolation: Ignore static images in vid ---
    if detection.frame_count.is_none_or(|fc| fc <= 1) {
        let (reason, ignore_class) = if detection.frame_count == Some(1) {
            (
                "Static image detected (1 frame) - vid ignores static media",
                Some(foundation::infra::static_logs::audit_ignore_class::VID_STATIC_SINGLE_FRAME),
            )
        } else {
            (
                "Unknown or zero frame count - vid ignores potentially non-animated media",
                Some(foundation::infra::static_logs::audit_ignore_class::VID_STATIC_UNKNOWN_FRAMES),
            )
        };

        let file_size = std::fs::metadata(input)
            .map_err(|e| {
                VidQualityError::ConversionError(format!("Failed to read metadata for size: {e}"))
            })?
            .len();
        foundation::progress_mode::video_ignored(input, reason, ignore_class);
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Ignored,
                reason: reason.to_string(),
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            },
            input_size: file_size,
            output_size: 0,
            size_ratio: 0.0,
            success: false,
            message: format!("IGNORED: {reason}"),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
            ignored: true,
        });
    }

    // Warn about dynamic HDR metadata that will be stripped during re-encode
    if detection.flags.hdr.is_dolby_vision {
        if foundation::is_dovi_tool_available() {
            foundation::log_detail!(
                foundation::infra::static_logs::messages::DOLBY_VISION_DETECTED
            );
        } else {
            foundation::log_static!(
                warn,
                foundation::infra::static_logs::messages::DOVI_TOOL_MISSING
            );
            foundation::log_hint!(
                "Install dovi_tool to preserve DV metadata: cargo install dovi_tool",
            );
        }
    }
    if detection.flags.hdr.is_hdr10_plus {
        foundation::log_static!(
            warn,
            foundation::infra::static_logs::messages::HDR10PLUS_TOOL_MISSING
        );
    }

    detection.file_path = input.display().to_string();

    let strategy = determine_strategy_with_apple_compat(
        &detection,
        input,
        config.apple_compat(),
        config.force(),
        config.codec,
    );

    log_detail!(&format!(
        "Conversion strategy determined for {}: Target={}, Reason={}, CRF={:.1}, Lossless={}",
        input.display(),
        strategy.target.extension(),
        strategy.reason,
        strategy.crf,
        strategy.lossless
    ));

    if let Err(e) = config.codec.validate_delivery_flags(config.apple_compat()) {
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "av1_apple_compat_conflict",
            input,
            e.to_string(),
        );
        foundation::log_hint!("remove --apple-compat or change codec to hevc.");
        return Err(VidQualityError::GeneralError(e.to_string()));
    }

    if strategy.target == TargetVideoFormat::Skip {
        foundation::progress_mode::video_skipped(input, &strategy.reason);

        foundation::copy_on_skip_or_fail(
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
message: "Generation Loss Guard: Skipping modern format conversion to preserve source bitstream integrity".to_string(),
final_crf: 0.0,
exploration_attempts: 0,
blake3: None,
ignored: false,
});
    }

    let output_dir = foundation::media_conversion_gate::resolve_output_dir_for_delivery(
        input,
        config.base_dir.as_deref(),
        config.output_dir.as_deref(),
    );

    std::fs::create_dir_all(&output_dir)?;

    let stem = foundation::media_conversion_gate::output_stem_for_delivery(input);
    let target_ext = strategy.target.extension();
    let input_ext = foundation::media_conversion_gate::path_extension_label(input);
    // GIF as source has no Apple compatibility issue; use the already-sniffed codec, not its suffix.
    let source_is_gif = content_codec == Some(foundation::quality_matcher::SourceCodec::Gif);

    let output_path = if input_ext.eq_ignore_ascii_case(target_ext)
        || (config.apple_compat() && input_ext.eq_ignore_ascii_case("mov"))
    {
        output_dir.join(format!("{stem}_hevc.{target_ext}"))
    } else {
        output_dir.join(format!("{stem}.{target_ext}"))
    };
    let output_path = foundation::conversion::reserve_output_path(input, &output_path);
    foundation::conversion::validate_output_path(&output_path, config.base_dir.as_deref())
        .map_err(VidQualityError::ConversionError)?;

    foundation::path_validator::check_input_output_conflict(input, &output_path)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

    if output_path.exists() && !config.force() {
        let skip_reason = format!("Output exists: {}", output_path.display());
        foundation::progress_mode::video_skipped(input, &skip_reason);
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),
            strategy,
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 1.0,
            success: true,
            message: format!(
                " Pipeline Guard: Output path occupied ({}); skipping redundant processing",
                output_path.display()
            ),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
            ignored: false,
        });
    }

    let temp_path = foundation::path_safety::isolated_temp_path_for_search(&output_path)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_guard = foundation::conversion::TempOutputGuard::new(temp_path.clone());

    foundation::infra::static_logs::log_stage(
        "🎬",
        "Auto Mode",
        &format!("{} → {}", input.display(), strategy.target.as_str()),
    );
    foundation::log_detail!(&format!(
        "{} {}",
        foundation::infra::static_logs::messages::REASON_STR,
        strategy.reason
    ));

    let (output_size, final_crf, attempts, explore_result_opt) = match strategy.target {
        TargetVideoFormat::Ignored => {
            return Err(VidQualityError::GeneralError(
                "Unexpected Ignored target reached in conversion flow".to_string(),
            ));
        }
        TargetVideoFormat::HevcLosslessMkv => {
            foundation::infra::static_logs::log_stage(
                "",
                "Lossless Mode",
                &format!(
                    "Using {} Lossless Mode",
                    config.codec.as_str().to_uppercase()
                ),
            );
            if should_delegate_to_animated_mp4_matched(input, &detection)? {
                foundation::media_conversion_gate::delivery_pipeline_batch_audit(
                    "animated_mkv_lossless_route",
                    format!(
                        "{}: routing animated raster to convert_to_mkv_lossless (JXL/WebP preprocess + HEVC lossless MKV)",
                        input.display()
                    ),
                );
                let convert_options = convert_options_from_config(config);
                let task = crate::animated_image::convert_to_mkv_lossless(input, &convert_options)?;
                return task_result_to_conversion_output(
                    input, &detection, strategy, task, 0.0, 0, cache,
                );
            }
            let size = execute_lossless(
                &detection,
                &temp_path,
                config.child_threads,
                config.codec,
                config.apple_compat(),
                foundation::delivery_codec_strategy::EncoderModeFlags::empty()
                    | if config.ultimate_mode() {
                        foundation::delivery_codec_strategy::EncoderModeFlags::ULTIMATE
                    } else {
                        foundation::delivery_codec_strategy::EncoderModeFlags::empty()
                    }
                    | if config.archive_mode() {
                        foundation::delivery_codec_strategy::EncoderModeFlags::ARCHIVE
                    } else {
                        foundation::delivery_codec_strategy::EncoderModeFlags::empty()
                    },
                config.allow_hdr10plus_static_fallback(),
            )?;
            (size, 0.0, 0, None)
        }
        TargetVideoFormat::Gif => {
            let result = crate::animated_image::convert_to_gif_apple_compat(
                input,
                &convert_options_from_config(config),
            )?;

            if result.outcome() != foundation::conversion::Outcome::Converted {
                return task_result_to_conversion_output(
                    input, &detection, strategy, result, 0.0, 0, cache,
                );
            }

            let output_size = result.output_size.ok_or_else(|| {
                VidQualityError::ConversionError(
                    "GIF conversion success but missing output size".to_string(),
                )
            })?;
            let output_path = result.output_path.ok_or_else(|| {
                VidQualityError::ConversionError(
                    "GIF conversion success but missing output path".to_string(),
                )
            })?;
            let size_ratio = if detection.file_size > 0 {
                let ratio = Rational::from((output_size, detection.file_size));
                ratio.to_f64()
            } else {
                1.0_f64
            };

            log_success!(&format!(
                "GIF Recovery: Restoration finalized ({} → {} | Ratio: {:.1}%)",
                foundation::format_bytes(detection.file_size),
                foundation::format_bytes(output_size),
                size_ratio * 100.0_f64
            ));

            // Update cache hint for successful GIF recovery (no fabricated CRF; GIF path has no CRF grid)
            if result.success {
                detection.precision.last_best_crf = None;
                detection.precision.last_best_effort_crf = None;
                if let Some(cache) = cache {
                    if let Err(e) = cache.store_video_analysis(input, &detection) {
                        foundation::media_conversion_gate::video_cache_store_failed_audit(
                            input,
                            "gif-recovery-hint",
                            e,
                        );
                    } else {
                        log_detail!(
                            "Cache Sync: Persisted GIF recovery telemetry to analysis repository"
                        );
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
                ignored: false,
            });
        }
        TargetVideoFormat::HevcMov
        | TargetVideoFormat::HevcMp4
        | TargetVideoFormat::Av1Mp4
        | TargetVideoFormat::Av2Mp4
        | TargetVideoFormat::VvcMp4 => {
            if should_delegate_to_animated_mp4_matched(input, &detection)? {
                let convert_options = convert_options_from_config(config);
                if !config.explore_smaller() && !config.match_quality() {
                    foundation::media_conversion_gate::delivery_pipeline_batch_audit(
                        "animated_mp4_crf0_route",
                        format!(
                            "{}: routing animated raster to convert_to_mp4 (CRF 0 deconstruct, no GPU explore)",
                            input.display()
                        ),
                    );
                    let task = crate::animated_image::convert_to_mp4(input, &convert_options)?;
                    return task_result_to_conversion_output(
                        input, &detection, strategy, task, 0.0, 0, cache,
                    );
                }
                foundation::media_conversion_gate::delivery_pipeline_batch_audit(
                    "animated_mp4_matched_route",
                    format!(
                        "{}: routing animated raster to convert_to_mp4_matched (JXL/WebP/AVIF preprocess + GPU explore)",
                        input.display()
                    ),
                );
                let initial_crf = calculate_matched_crf(&detection, &config.codec)?;
                let task = crate::animated_image::convert_to_mp4_matched(
                    input,
                    &convert_options,
                    initial_crf,
                    animated_raster_has_alpha(input),
                )?;
                let (final_crf, exploration_attempts) = if task.skipped || !task.success {
                    (0.0_f32, 0_u8)
                } else {
                    let final_crf = task.explore_final_crf.ok_or_else(|| {
                        VidQualityError::ConversionError(format!(
                            "{}: animated GPU explore succeeded but explore_final_crf is missing",
                            input.display()
                        ))
                    })?;
                    let iterations = task.explore_iterations.ok_or_else(|| {
                        VidQualityError::ConversionError(format!(
                            "{}: animated GPU explore succeeded but explore_iterations is missing",
                            input.display()
                        ))
                    })?;
                    let exploration_attempts = u8::try_from(iterations).map_err(|_| {
                        VidQualityError::ConversionError(format!(
                            "Exploration iteration limit exceeded ({iterations} > 255) for {}",
                            input.display()
                        ))
                    })?;
                    (final_crf, exploration_attempts)
                };
                return task_result_to_conversion_output(
                    input,
                    &detection,
                    strategy,
                    task,
                    final_crf,
                    exploration_attempts,
                    cache,
                );
            }

            let vf_args = foundation::get_ffmpeg_dimension_args(
                detection.width.ok_or_else(|| {
                    anyhow::anyhow!("ffprobe returned no width for {}", detection.file_path)
                })?,
                detection.height.ok_or_else(|| {
                    anyhow::anyhow!("ffprobe returned no height for {}", detection.file_path)
                })?,
                false,
            );
            let input_path = Path::new(&detection.file_path);

            // Log media info to log file only (for SSIM/quality context); not shown on terminal.
            match foundation::analyze_video_quality_from_detection(&detection) {
                Ok(quality_analysis) => {
                    foundation::log_media_info_for_quality(&quality_analysis, input_path);
                }
                Err(err) => {
                    foundation::media_conversion_gate::probe_layer_batch_audit(
                        "video_quality_log_analysis_failed",
                        format!(
                            "quality log analysis failed for {}: {err}",
                            input_path.display()
                        ),
                    );
                }
            }

            let flag_mode =
                foundation::validate_flags_result_with_ultimate(foundation::FlagRequest {
                    base: foundation::FlagBase {
                        explore: config.explore_smaller(),
                        match_quality: config.match_quality(),
                        compress: config.require_compression(),
                    },
                    tier: foundation::FlagTier {
                        ultimate: config.ultimate_mode(),
                    },
                })
                .map_err(VidQualityError::ConversionError)?;

            let ultimate = flag_mode.is_ultimate();

            let use_gpu = config.use_gpu();
            if !use_gpu {
                let encoder_name = config.codec.cpu_encoder_name();
                if ultimate {
                    foundation::log_detail!(&format!(
                        "Compute Strategy: Utilizing {encoder_name} (CPU) with 3D quality gate (VMAF/CAMBI/PSNR-UV)",
                    ));
                } else {
                    foundation::log_detail!(&format!(
                        "Compute Strategy: Utilizing {} (CPU) for maximum structural fidelity (SSIM ≥ {:.2})",
                        encoder_name,
                        foundation::constants::UI_QUALITY_GOOD_THRESHOLD
                    ));
                }
            }

            let predicted_crf = calculate_matched_crf(&detection, &config.codec)?;
            let warm_start_crf = if let Some(hint) = detection.precision.last_best_crf {
                foundation::log_hint!(&format!(
                    "Warm Start: Utilizing cached CRF hint ({hint:.1}) for accelerated anchor search"
                ));
                Some(hint)
            } else if let Some(hint) = detection.precision.last_best_effort_crf {
                foundation::log_hint!(&format!(
                    "Warm Start: Utilizing best-effort CRF hint ({hint:.1}) for accelerated anchor search"
                ));
                Some(hint)
            } else if let Some(hint) = config.codec.warm_start_crf_hint() {
                foundation::log_hint!(&format!(
                    "Warm Start: Utilizing global {} success CRF ({hint:.1}) for anchor convergence",
                    config.codec.as_str().to_uppercase()
                ));
                Some(foundation::numeric_cast::f64_to_f32_lossy(hint))
            } else {
                None
            };
            let search_crf = foundation::media_conversion_gate::warm_start_crf_or_predicted(
                warm_start_crf,
                predicted_crf,
                input,
                config.codec.as_str(),
            );
            foundation::log_detail!(&format!(
                "{} {}: base CRF {:.1} → search anchor {:.1}",
                if ultimate { "[Ultimate]" } else { "[Precise]" },
                flag_mode.description_en(),
                predicted_crf,
                search_crf
            ));
            let dv_rpu = prepare_dv_rpu(&detection);
            let hdr10plus =
                prepare_hdr10plus_metadata(&detection, config.allow_hdr10plus_static_fallback())?;
            let hdr_x265_params = build_hevc_x265_extra_params(
                &detection,
                hdr_pix_fmt(&detection),
                dv_rpu.as_ref(),
                hdr10plus.as_ref(),
            );

            let hdr_x265_params_opt = if hdr_x265_params.is_empty() {
                None
            } else {
                Some(hdr_x265_params)
            };

            let explore_preset = if config.archive_mode() {
                foundation::EncoderPreset::Veryslow
            } else if ultimate {
                foundation::EncoderPreset::Slower
            } else {
                foundation::EncoderPreset::Medium
            };
            let explore_result = config
                .codec
                .explore_with_gpu(&foundation::GpuSearchRequest {
                    input: input.to_path_buf(),
                    output: temp_path.clone(),
                    vf_args,
                    baseline_crf: search_crf,
                    warm_start_crf,
                    flags: foundation::delivery_codec_strategy::gpu_search_flags_for_codec(
                        config.codec,
                        foundation::GpuSearchFeatures {
                            ultimate_mode: ultimate,
                            apple_compat: config.apple_compat(),
                            archive_mode: config.archive_mode(),
                        },
                        foundation::GpuSearchValidation {
                            force_ms_ssim_long: config.force_ms_ssim_long(),
                            allow_size_tolerance: config.allow_size_tolerance(),
                        },
                    ),
                    min_ssim: config.min_ssim,
                    max_threads: config.child_threads,
                    hdr_x265_params: if config.codec == SelectedCodec::Hevc {
                        hdr_x265_params_opt
                    } else {
                        None
                    },
                    preset: explore_preset,
                })
                .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

            for log_line in &explore_result.log {
                foundation::log_detail!(log_line);
            }

            // --- Explore phase: quality/SSIM or size did not meet target; decide whether to keep or discard output. ---
            if !explore_result.pipeline_acceptable(config.match_quality(), config.explore_smaller())
                && (config.match_quality() || config.explore_smaller())
            {
                let pure_media_compressed =
                    explore_result.output_pure_media_size < explore_result.input_pure_media_size;
                let pure_media_size_ratio = if explore_result.input_pure_media_size > 0 {
                    let ratio = Rational::from((
                        explore_result.output_pure_media_size,
                        explore_result.input_pure_media_size,
                    ));
                    ratio.to_f64()
                } else {
                    1.0_f64
                };
                let decision =
                    ExploreGateRejectionDecision::inspect_and_log(input, &explore_result);
                decision.emit(input);

                // Keep/discard by exact video + audio packet payload; total size is reporting only.
                if foundation::should_keep_apple_fallback_hevc_output(
                    foundation::AppleFallbackKeepRequest {
                        codec_str: detection.codec.as_str(),
                        pure_media_size_ratio,
                        flags: foundation::AppleFallbackFlags {
                            outcome: foundation::AppleOutcomeFlags {
                                pure_media_compressed,
                                allow_size_tolerance: config.allow_size_tolerance(),
                            },
                            context: foundation::AppleContextFlags {
                                apple_compat: config.apple_compat(),
                                source_is_gif,
                                ultimate_explore: explore_result.ultimate_mode,
                            },
                        },
                    },
                ) {
                    foundation::media_conversion_gate::apple_compat_fallback_audit(
                        "apple_compat_hevc_quality",
                        input,
                        format!(
                            "keeping best-effort HEVC (CRF {:.1}, {} iters) despite missing quality/size targets",
                            explore_result.optimal_crf, explore_result.iterations
                        ),
                    );
                    if !foundation::conversion::commit_temp_to_output_with_metadata(
                        &temp_path,
                        &output_path,
                        config.force(),
                        Some(input),
                    )? {
                        return Ok(concurrent_output_skip_conversion_output(
                            input,
                            &detection,
                            ConversionStrategy {
                                target: hevc_delivery_target(config.apple_compat()),
                                reason: "Apple compat fallback: best-effort HEVC kept (quality/size below target)".to_string(),
                                command: String::new(),
                                preserve_audio: detection.flags.streams.has_audio,
                                crf: explore_result.optimal_crf,
                                lossless: false,
                            },
                        ));
                    }
                    return Ok(ConversionOutput {
input_path: input.display().to_string(),
output_path: output_path.display().to_string(),
strategy: ConversionStrategy {
target: hevc_delivery_target(config.apple_compat()),
reason: "Apple compat fallback: best-effort HEVC kept (quality/size below target)".to_string(),
command: String::new(),
preserve_audio: detection.flags.streams.has_audio,
crf: explore_result.optimal_crf,
lossless: false,
},
input_size: detection.file_size,
output_size: explore_result.output_size,
size_ratio: {
let ratio = Rational::from((explore_result.output_size, detection.file_size.max(1)));
ratio.to_f64()
},
success: true,
message: format!(
"Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); quality/size below target — file is HEVC and importable",
explore_result.optimal_crf,
explore_result.iterations
),
final_crf: explore_result.optimal_crf,
exploration_attempts: u8::try_from(explore_result.iterations).map_err(|_| {
VidQualityError::ConversionError(format!(
"Exploration iteration limit exceeded ({} > 255) for {}",
explore_result.iterations,
input.display()
))
})?,
blake3: None,
ignored: false,
});
                }

                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "explore_gate_rejected_temp",
                    &temp_path,
                );
                foundation::copy_on_skip_or_fail(
                    input,
                    config.output_dir.as_deref(),
                    config.base_dir.as_deref(),
                    false,
                )
                .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

                return decision.into_output(input, &detection, &explore_result);
            }

            (
                explore_result.output_size,
                explore_result.optimal_crf,
                u8::try_from(explore_result.iterations).map_err(|_| {
                    VidQualityError::ConversionError(format!(
                        "Exploration iteration limit exceeded ({} > 255) for {}",
                        explore_result.iterations,
                        input.display()
                    ))
                })?,
                Some(explore_result),
            )
        }

        TargetVideoFormat::Ffv1Mkv => {
            return Err(VidQualityError::GeneralError(format!(
                "Invalid HEVC delivery target for {}: strategy selected FFV1/MKV",
                input.display()
            )));
        }
        TargetVideoFormat::Skip => {
            return Err(VidQualityError::GeneralError(format!(
                "Invalid conversion state for {}: Skip target reached after skip handling",
                input.display()
            )));
        }
    };

    let cache_exact_hint = success_status_for_cache(
        strategy.target,
        explore_result_opt.as_ref(),
        config.match_quality(),
        config.explore_smaller(),
    );
    let cache_best_effort_hint = best_effort_status_for_cache(
        strategy.target,
        explore_result_opt.as_ref(),
        final_crf,
        config.match_quality(),
        config.explore_smaller(),
    );

    if cache_exact_hint {
        config.codec.record_global_crf_hit(final_crf);
    }
    if let Some(cache) = cache
        && (cache_exact_hint || cache_best_effort_hint)
    {
        if cache_exact_hint {
            detection.precision.last_best_crf = Some(final_crf);
            detection.precision.last_best_effort_crf = None;
        } else {
            detection.precision.last_best_effort_crf = Some(final_crf);
        }
        if let Err(e) = cache.store_video_analysis(input, &detection) {
            foundation::media_conversion_gate::video_cache_store_failed_audit(
                input,
                &format!("crf-hint; exact={cache_exact_hint}"),
                e,
            );
        } else {
            log_detail!(&format!(
                "Updated video cache with {} CRF hint: {:.1}",
                if cache_exact_hint {
                    "exact"
                } else {
                    "best-effort"
                },
                final_crf
            ));
        }
    }

    // Verify temp file exists before commit
    if !temp_path.exists() {
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "temp_missing_before_commit",
            input,
            format!(
                "temp output missing before commit ({})",
                temp_path.display()
            ),
        );
        return Err(VidQualityError::ConversionError(format!(
            "Temp file not found: {}",
            temp_path.display()
        )));
    }

    if !foundation::conversion::commit_temp_to_output_with_metadata(
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
        log_detail!("Output was created concurrently, skipping overwrite");
        return Ok(concurrent_output_skip_conversion_output(
            input,
            &detection,
            strategy.clone(),
        ));
    }

    if let Some(ref result) = explore_result_opt {
        let quality_block = if config.match_quality() {
            !result.perceptual_quality_met()
        } else {
            result.perceptual_quality_failed()
        };
        if quality_block {
            let decision = FinalQualityGateFailureDecision::inspect_and_log(input, result);

            // Only keep best-effort HEVC when source is Apple-incompatible (AV1/VP9/VVC/AV2).
            if config.apple_compat()
                && !source_is_gif
                && foundation::is_apple_incompatible_video_codec(detection.codec.as_str())
            {
                foundation::media_conversion_gate::apple_compat_fallback_audit(
                    "apple_compat_hevc_quality",
                    input,
                    "quality below target; keeping best-effort HEVC for iOS import",
                );
                foundation::log_detail!(&format!(
                    "Keeping best-effort output: last attempt CRF {:.1} ({} iterations), file is HEVC and importable",
                    result.optimal_crf, result.iterations
                ));
                return Ok(ConversionOutput {
                    input_path: input.display().to_string(),
                    output_path: output_path.display().to_string(),
                    strategy: ConversionStrategy {
                        target: hevc_delivery_target(config.apple_compat()),
                        reason:
                            "Apple compat fallback: best-effort HEVC kept (quality below target)"
                                .to_string(),
                        command: String::new(),
                        preserve_audio: detection.flags.streams.has_audio,
                        crf: result.optimal_crf,
                        lossless: false,
                    },
                    input_size: detection.file_size,
                    output_size: result.output_size,
                    size_ratio: {
                        let ratio =
                            Rational::from((result.output_size, detection.file_size.max(1)));
                        ratio.to_f64()
                    },
                    success: true,
                    message: format!(
                        "Apple compat fallback: kept best-effort output (CRF {:.1}, {} iters); {} below target — file is HEVC and importable",
                        result.optimal_crf, result.iterations, decision.quality_summary
                    ),
                    final_crf: result.optimal_crf,
                    exploration_attempts: u8::try_from(result.iterations).map_err(|_| {
                        VidQualityError::ConversionError(format!(
                            "Exploration iteration limit exceeded ({} > 255) for {}",
                            result.iterations,
                            input.display()
                        ))
                    })?,
                    blake3: None,
                    ignored: false,
                });
            }

            if output_path.exists() {
                let cleanup_ctx = if result.uses_ultimate_quality_contract() {
                    "3D quality gate cleanup"
                } else {
                    "low MS-SSIM cleanup"
                };
                cleanup_output_file(&output_path, cleanup_ctx);
                foundation::log_detail!(&format!(
                    "↳ Discarded output: {} below target/floor",
                    decision.quality_summary
                ));
            }
            if temp_path.exists() {
                let cleanup_ctx = if result.uses_ultimate_quality_contract() {
                    "temporary output cleanup after 3D gate failure"
                } else {
                    "temporary output cleanup after low MS-SSIM"
                };
                cleanup_output_file(&temp_path, cleanup_ctx);
            }

            foundation::copy_on_skip_or_fail(
                input,
                config.output_dir.as_deref(),
                config.base_dir.as_deref(),
                false,
            )
            .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

            return decision.into_failed_output(input, &detection, result);
        }
    }

    let pre_metadata_size = output_size;

    let actual_output_size =
        foundation::media_conversion_gate::delivery_output_file_len_or_estimate(
            &output_path,
            output_size,
        );

    let metadata_delta =
        foundation::video_explorer::detect_metadata_size(pre_metadata_size, actual_output_size);

    let verify_result = foundation::verify_strict_pure_media_paths(
        input,
        &output_path,
        config.allow_size_tolerance(),
    )
    .map_err(|err| {
        VidQualityError::ConversionError(format!(
            "Strict pure-media verification failed for {} -> {}: {err}",
            input.display(),
            output_path.display()
        ))
    })?;

    if metadata_delta > 0
        || verify_result.output_container_overhead
            > foundation::constants::CONTAINER_OVERHEAD_REPORT_THRESHOLD
    {
        foundation::log_detail!(&format!(" Metadata: +{metadata_delta} bytes"));
        foundation::log_detail!(&format!(
            "{} Container/metadata overhead: {} bytes",
            foundation::modern_ui::symbols::pick("📦", "[PKG]"),
            verify_result.output_container_overhead,
        ));
    }

    let pure_media_compressed =
        verify_result.output_pure_media_size < verify_result.input_pure_media_size;
    let pure_media_size_ratio = if verify_result.input_pure_media_size > 0 {
        let ratio = Rational::from((
            verify_result.output_pure_media_size,
            verify_result.input_pure_media_size,
        ));
        ratio.to_f64()
    } else {
        1.0_f64
    };
    let pure_media_within_tolerance = verify_result.pure_media_compressed;

    // --- require_compression phase: primary decision by exact video + audio packet payload. ---
    if config.require_compression() && !pure_media_within_tolerance {
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "compression_requirement_failed",
            input,
            format!(
                "pure media {} → {} ({:+.1}%)",
                foundation::format_bytes(verify_result.input_pure_media_size),
                foundation::format_bytes(verify_result.output_pure_media_size),
                verify_result.pure_media_size_change_percent()
            ),
        );
        log_detail!(&format!(
            "total-file diagnostic: {} -> {} ({:+.1}%), container_overhead_diff={:+}B",
            foundation::format_bytes(detection.file_size),
            foundation::format_bytes(actual_output_size),
            verify_result.total_size_change_percent(),
            verify_result.container_overhead_diff
        ));
        foundation::log_detail!(&format!(
            "{} Original file PROTECTED",
            foundation::modern_ui::symbols::SHIELD
        ));

        // Apple-compat fallback uses the same pure-media payload contract.
        if foundation::should_keep_apple_fallback_hevc_output(
            foundation::AppleFallbackKeepRequest {
                codec_str: detection.codec.as_str(),
                pure_media_size_ratio,
                flags: foundation::AppleFallbackFlags {
                    outcome: foundation::AppleOutcomeFlags {
                        pure_media_compressed,
                        allow_size_tolerance: config.allow_size_tolerance(),
                    },
                    context: foundation::AppleContextFlags {
                        apple_compat: config.apple_compat(),
                        source_is_gif,
                        ultimate_explore: config.ultimate_mode(),
                    },
                },
            },
        ) {
            foundation::media_conversion_gate::apple_compat_fallback_audit(
                "apple_compat_hevc_compression",
                input,
                "pure-media compression check failed; keeping best-effort HEVC",
            );
            foundation::log_detail!(&format!(
                "Keeping best-effort output: last attempt CRF {final_crf:.1} ({attempts} iterations), file is HEVC and importable"
            ));
            return Ok(ConversionOutput {
                input_path: input.display().to_string(),
                output_path: output_path.display().to_string(),
                strategy: ConversionStrategy {
                    target: hevc_delivery_target(config.apple_compat()),
                    reason:
                        "Apple compat fallback: best-effort HEVC kept (compression check failed)"
                            .to_string(),
                    command: String::new(),
                    preserve_audio: detection.flags.streams.has_audio,
                    crf: final_crf,
                    lossless: false,
                },
                input_size: detection.file_size,
                output_size: actual_output_size,
                size_ratio: Rational::from((actual_output_size, detection.file_size.max(1)))
                    .to_f64(),
                success: true,
                message: format!(
                    "Apple compat fallback: kept best-effort output (CRF {final_crf:.1}, {attempts} iters); pure-media compression check failed, but file is HEVC and importable"
                ),
                final_crf,
                exploration_attempts: attempts,
                blake3: None,
                ignored: false,
            });
        }

        if output_path.exists() {
            cleanup_output_file(&output_path, "compression failure cleanup");
            foundation::log_detail!(&format!(
                "↳ Discarded output: pure media increased to {} ({:+.1}%)",
                foundation::format_bytes(verify_result.output_pure_media_size),
                verify_result.pure_media_size_change_percent()
            ));
        }
        if temp_path.exists() {
            cleanup_output_file(
                &temp_path,
                "temporary output cleanup after compression failure",
            );
        }

        foundation::copy_on_skip_or_fail(
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
                    "Compression target not met: pure media {} → {} ({:+.1}%)",
                    foundation::format_bytes(verify_result.input_pure_media_size),
                    foundation::format_bytes(verify_result.output_pure_media_size),
                    verify_result.pure_media_size_change_percent(),
                ),
                command: String::new(),
                preserve_audio: detection.flags.streams.has_audio,
                crf: final_crf,
                lossless: false,
            },
            input_size: detection.file_size,
            output_size: detection.file_size,
            size_ratio: 1.0,
            success: true,
            message: format!(
                "Skipped: pure media not smaller ({} → {})",
                foundation::format_bytes(verify_result.input_pure_media_size),
                foundation::format_bytes(verify_result.output_pure_media_size),
            ),
            final_crf,
            exploration_attempts: attempts,
            blake3: None,
            ignored: false,
        });
    }

    if verify_result.is_container_overhead_issue() {
        log_detail!(&format!(
            "pure media shrank ({:+.1}%) but total file grew ({:+.1}%) due to container overhead diff {:+}B",
            verify_result.pure_media_size_change_percent(),
            verify_result.total_size_change_percent(),
            verify_result.container_overhead_diff
        ));
    }

    let output_size = actual_output_size;
    let size_ratio = {
        let ratio = Rational::from((output_size, detection.file_size.max(1)));
        ratio.to_f64()
    };

    if config.should_delete_original() {
        if let Err(e) = foundation::conversion::safe_delete_original(
            input,
            &output_path,
            foundation::MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO,
        ) {
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "delete_original_failed",
                input,
                e.to_string(),
            );
        } else {
            foundation::log_detail!(
                foundation::infra::static_logs::messages::ORIGINAL_DELETED_VERIFIED
            );
        }
    }

    foundation::log_detail!(&format!(
        "Complete: {:.1}% of original",
        size_ratio * 100.0_f64
    ));

    Ok(ConversionOutput {
        input_path: input.display().to_string(),
        output_path: output_path.display().to_string(),
        strategy: ConversionStrategy {
            target: strategy.target,
            reason: strategy.reason,
            command: String::new(),
            preserve_audio: detection.flags.streams.has_audio,
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
        ignored: false,
    })
}

fn success_status_for_cache(
    target: TargetVideoFormat,
    explore_result: Option<&foundation::ExploreResult>,
    match_quality: bool,
    explore_smaller: bool,
) -> bool {
    matches!(target, TargetVideoFormat::Gif)
        || (matches!(
            target,
            TargetVideoFormat::HevcMov | TargetVideoFormat::HevcMp4 | TargetVideoFormat::Av1Mp4
        ) && explore_result
            .is_some_and(|r| r.pipeline_acceptable(match_quality, explore_smaller)))
}

fn best_effort_status_for_cache(
    target: TargetVideoFormat,
    explore_result: Option<&foundation::ExploreResult>,
    final_crf: f32,
    match_quality: bool,
    explore_smaller: bool,
) -> bool {
    matches!(
        target,
        TargetVideoFormat::HevcMov | TargetVideoFormat::HevcMp4 | TargetVideoFormat::Av1Mp4
    ) && final_crf > 0.0
        && explore_result.is_some_and(|r| {
            !r.pipeline_acceptable(match_quality, explore_smaller) && r.quality_passed.is_failed()
        })
}

/// Calculate matched CRF based on detection results and selected codec.
///
/// # Errors
/// Returns an error if calculation fails.
///
/// # Panics
/// Panics if the ffprobe-backed detection is missing `width` or `height`.
pub fn calculate_matched_crf(detection: &Detection, codec: &SelectedCodec) -> Result<f32> {
    let mut builder = foundation::VideoAnalysisBuilder::new()
        .basic(
            detection.codec.as_str(),
            detection.width.ok_or_else(|| {
                anyhow::anyhow!(
                    "calculate_matched_crf: ffprobe returned no width for {}",
                    detection.file_path
                )
            })?,
            detection.height.ok_or_else(|| {
                anyhow::anyhow!(
                    "calculate_matched_crf: ffprobe returned no height for {}",
                    detection.file_path
                )
            })?,
            detection.fps,
            detection.duration_secs,
        )
        .bit_depth(detection.bit_depth)
        .file_size(detection.file_size);

    if let Some(vbr) = detection.video_bitrate {
        builder = builder.video_bitrate(vbr);
    } else if let Some(br) = detection.bitrate {
        builder = builder.video_bitrate(br);
    }

    if !detection.pix_fmt.is_empty() {
        builder = builder.pix_fmt(&detection.pix_fmt);
    }

    if let Some((color_space_str, is_hdr)) = detection.color_space.quality_matcher_color_profile() {
        builder = builder.color(color_space_str, is_hdr);
    }
    if detection.is_hdr() {
        builder = builder.hdr(true);
    }

    if detection.flags.content.has_b_frames {
        builder = builder.gop(Some(60), Some(2));
    }

    let analysis = builder.build();

    let result = codec
        .calculate_crf_from_quality_analysis(&analysis)
        .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

    let encoder = codec.quality_encoder_type().ok_or_else(|| {
        VidQualityError::GeneralError(format!(
            "{} encoder type not yet implemented",
            codec.as_str().to_uppercase()
        ))
    })?;
    foundation::log_quality_analysis(&analysis, &result, encoder);
    Ok(result.crf)
}

fn execute_lossless(
    detection: &Detection,
    output: &Path,
    max_threads: usize,
    codec: SelectedCodec,
    apple_compat: bool,
    encoder_modes: foundation::delivery_codec_strategy::EncoderModeFlags,
    allow_hdr10plus_static_fallback: bool,
) -> Result<u64> {
    if !codec.supports_lossless_archival_mkv() {
        return Err(VidQualityError::ConversionError(format!(
            "lossless archival encode requires --codec hevc; got {}",
            codec.as_str()
        )));
    }

    let codec_name = codec.as_str().to_uppercase();
    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
        "lossless_encode_warning",
        Path::new(&detection.file_path),
        format!("{codec_name} lossless encode: slow path with large output expected"),
    );

    // Attempt to extract DV RPU for injection (None = not DV or graceful fallback)
    let dv_rpu = prepare_dv_rpu(detection);

    // Attempt to extract HDR10+ metadata for injection
    let hdr10plus = prepare_hdr10plus_metadata(detection, allow_hdr10plus_static_fallback)?;

    let x265_memory_profile = foundation::x265_params::memory_profile_for_detection(detection);
    if x265_memory_profile.is_low_memory() {
        foundation::log_info!(
            label = "x265",
            "Applying low-memory x265 profile for large/archival-grade source: size={:.2} GB, codec={}, path={}",
            foundation::numeric_cast::u64_to_f64(detection.file_size)
                / foundation::numeric_cast::u64_to_f64(foundation::constants::BYTES_PER_GB),
            detection.codec.as_str(),
            detection.file_path
        );
    }
    let extra_x265_params = build_hevc_x265_extra_params(
        detection,
        hdr_pix_fmt(detection),
        dv_rpu.as_ref(),
        hdr10plus.as_ref(),
    );
    let x265_params = foundation::x265_params::format_x265_lossless_params(
        max_threads,
        (!extra_x265_params.is_empty()).then_some(extra_x265_params.as_str()),
        x265_memory_profile,
    );

    let pix_fmt = hdr_pix_fmt(detection);
    let vf_args = foundation::get_ffmpeg_dimension_args(
        detection.width.ok_or_else(|| {
            anyhow::anyhow!("ffprobe returned no width for {}", detection.file_path)
        })?,
        detection.height.ok_or_else(|| {
            anyhow::anyhow!("ffprobe returned no height for {}", detection.file_path)
        })?,
        false,
    );

    let input_arg = foundation::safe_path_arg(Path::new(&detection.file_path))
        .as_ref()
        .to_string();
    let ultimate =
        encoder_modes.contains(foundation::delivery_codec_strategy::EncoderModeFlags::ULTIMATE);
    let archive =
        encoder_modes.contains(foundation::delivery_codec_strategy::EncoderModeFlags::ARCHIVE);
    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(),
        max_threads.to_string(),
        "-i".to_string(),
        input_arg,
        "-c:v".to_string(),
        foundation::constants::LIB_X265.to_string(),
        "-pix_fmt".to_string(),
        pix_fmt.to_string(),
        foundation::constants::FFMPEG_ARG_X265_PARAMS.to_string(),
        x265_params,
        foundation::constants::FFMPEG_ARG_PRESET.to_string(),
        if archive {
            foundation::constants::FFMPEG_PRESET_VERYSLOW.to_string()
        } else if ultimate {
            foundation::constants::FFMPEG_PRESET_SLOWER.to_string()
        } else {
            foundation::constants::FFMPEG_PRESET_MEDIUM.to_string()
        },
    ];
    if apple_compat {
        args.extend([
            foundation::constants::FFMPEG_ARG_TAG_VIDEO.to_string(),
            foundation::constants::FFMPEG_TAG_HVC1.to_string(),
        ]);
    }

    // Forward all HDR colour metadata
    args.extend(build_hdr_ffmpeg_args(detection));

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.flags.streams.has_audio {
        // MKV supports all codecs — always copy
        args.extend(foundation::audio_args_for_container(
            detection.audio_codec.as_deref(),
            "mkv",
        ));
    } else {
        args.push("-an".to_string());
    }

    // Subtitles: MKV supports all subtitle formats — always copy
    args.extend(foundation::subtitle_args_for_container(
        detection.flags.streams.has_subtitles,
        detection.subtitle_codec.as_deref(),
        "mkv",
    ));

    let mut builder = foundation::FfmpegBuilder::new();
    builder.args(args).output(output);
    let (status, stderr) = builder.spawn()?.wait_with_output()?;

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

    fn test_pts_deltas() -> Vec<f64> {
        vec![1.0 / 30.0, 1.0 / 30.0, 1.0 / 30.0]
    }

    fn test_pkt_sizes() -> Vec<u64> {
        vec![10_000, 10_100, 9_900]
    }

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
        let detection = crate::detection_api::Detection {
            file_path: "/test/video.webm".to_string(),
            format: "webm".to_string(),
            codec: crate::detection_api::DetectedCodec::VP9,
            codec_long: "Google VP9".to_string(),
            compression: crate::detection_api::CompressionType::Standard,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(1800),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            bit_depth: Some(8),
            pix_fmt: "yuv420p".to_string(),
            file_size: 50_000_000,
            bitrate: Some(6_666_666),
            audio_codec: Some("opus".to_string()),
            quality_score: 75,
            color_space: crate::detection_api::ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            profile: None,
            bits_per_pixel: foundation::constants::VIDEO_BITS_PER_PIXEL_STANDARD,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
            ..Default::default()
        };

        let strategy = determine_strategy_with_apple_compat(
            &detection,
            Path::new(&detection.file_path),
            false,
            false,
            SelectedCodec::Hevc,
        );
        assert_eq!(
            strategy.target,
            TargetVideoFormat::Skip,
            "VP9 skipped in normal mode (modern format; use Apple-compat to convert)"
        );
    }

    #[test]
    fn test_strategy_apple_compat_converts_vp9() {
        let detection = crate::detection_api::Detection {
            file_path: "/test/video.webm".to_string(),
            format: "webm".to_string(),
            codec: crate::detection_api::DetectedCodec::VP9,
            codec_long: "Google VP9".to_string(),
            compression: crate::detection_api::CompressionType::Standard,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(1800),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            bit_depth: Some(8),
            pix_fmt: "yuv420p".to_string(),
            file_size: 50_000_000,
            bitrate: Some(6_666_666),
            audio_codec: Some("opus".to_string()),
            quality_score: 75,
            color_space: crate::detection_api::ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            profile: None,
            bits_per_pixel: foundation::constants::VIDEO_BITS_PER_PIXEL_STANDARD,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
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
        let detection = crate::detection_api::Detection {
            file_path: "/test/video.mp4".to_string(),
            format: "mp4".to_string(),
            codec: crate::detection_api::DetectedCodec::H265,
            codec_long: "HEVC".to_string(),
            compression: crate::detection_api::CompressionType::Standard,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(1800),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            bit_depth: Some(8),
            pix_fmt: "yuv420p".to_string(),
            file_size: 50_000_000,
            bitrate: Some(6_666_666),
            audio_codec: Some("aac".to_string()),
            quality_score: 80,
            color_space: crate::detection_api::ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            profile: None,
            bits_per_pixel: foundation::constants::VIDEO_BITS_PER_PIXEL_STANDARD,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
            ..Default::default()
        };

        let normal = determine_strategy_with_apple_compat(
            &detection,
            Path::new(&detection.file_path),
            false,
            false,
            SelectedCodec::Hevc,
        );
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
        let detection = crate::detection_api::Detection {
            file_path: "/test/video.mp4".to_string(),
            format: "mp4".to_string(),
            codec: crate::detection_api::DetectedCodec::H264,
            codec_long: "H.264/AVC".to_string(),
            compression: crate::detection_api::CompressionType::Standard,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(1800),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            bit_depth: Some(8),
            pix_fmt: "yuv420p".to_string(),
            file_size: 50_000_000,
            bitrate: Some(6_666_666),
            audio_codec: Some("aac".to_string()),
            quality_score: 70,
            color_space: crate::detection_api::ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            profile: None,
            bits_per_pixel: foundation::constants::VIDEO_BITS_PER_PIXEL_STANDARD,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
            ..Default::default()
        };

        let normal = determine_strategy_with_apple_compat(
            &detection,
            Path::new(&detection.file_path),
            false,
            false,
            SelectedCodec::Hevc,
        );
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

        let make_detection = |codec: DetectedCodec| -> crate::detection_api::Detection {
            crate::detection_api::Detection {
                file_path: "/test/video.mp4".to_string(),
                format: "mp4".to_string(),
                codec,
                codec_long: "Test".to_string(),
                compression: CompressionType::Standard,
                width: Some(1920),
                height: Some(1080),
                frame_count: Some(1800),
                fps: Some(30.0),
                duration_secs: Some(60.0),
                bit_depth: Some(8),
                pix_fmt: "yuv420p".to_string(),
                file_size: 50_000_000,
                bitrate: Some(6_666_666),
                audio_codec: None,
                quality_score: 70,
                color_space: ColorSpace::BT709,
                video_bitrate: Some(6_000_000),
                profile: None,
                bits_per_pixel: foundation::constants::VIDEO_BITS_PER_PIXEL_STANDARD,
                color_primaries: None,
                color_transfer: None,
                mastering_display: None,
                max_cll: None,
                dv_profile: None,
                dv_bl_signal_compatibility_id: None,
                subtitle_codec: None,
                max_b_frames: Some(0),
                encoder_params: None,
                audio_channels: None,
                precision: foundation::video_detection::VideoPrecisionMetadata::default(),
                tags: std::collections::HashMap::new(),
                pts_deltas: test_pts_deltas(),
                pkt_sizes: test_pkt_sizes(),
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

            let normal = determine_strategy_with_apple_compat(
                &detection,
                Path::new(&detection.file_path),
                false,
                false,
                SelectedCodec::Hevc,
            );
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
        let det = crate::detection_api::Detection {
            file_path: "/t.mp4".into(),
            format: "mp4".into(),
            codec: DetectedCodec::AV1,
            codec_long: "AV1".into(),
            compression: CompressionType::Standard,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(1800),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            bit_depth: Some(8),
            pix_fmt: "yuv420p".into(),
            file_size: 50_000_000,
            bitrate: Some(6_666_666),
            audio_codec: Some("opus".into()),
            quality_score: 85,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            profile: None,
            bits_per_pixel: foundation::constants::VIDEO_BITS_PER_PIXEL_STANDARD,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
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
        let det = crate::detection_api::Detection {
            file_path: "/t.mp4".into(),
            format: "mp4".into(),
            codec: DetectedCodec::VVC,
            codec_long: "VVC".into(),
            compression: CompressionType::Standard,
            width: Some(3840),
            height: Some(2160),
            frame_count: Some(3600),
            fps: Some(60.0),
            duration_secs: Some(60.0),
            bit_depth: Some(10),
            pix_fmt: "yuv420p10le".into(),
            file_size: 100_000_000,
            bitrate: Some(13_333_333),
            audio_codec: Some("aac".into()),
            quality_score: 90,
            color_space: ColorSpace::BT2020,
            video_bitrate: Some(12_000_000),
            profile: None,
            bits_per_pixel: foundation::constants::VIDEO_BITS_PER_PIXEL_LOW,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
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
        let det = crate::detection_api::Detection {
            file_path: "/t.webm".into(),
            format: "webm".into(),
            codec: DetectedCodec::VP9,
            codec_long: "VP9".into(),
            compression: CompressionType::Standard,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(1800),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            bit_depth: Some(8),
            pix_fmt: "yuv420p".into(),
            file_size: 50_000_000,
            bitrate: Some(6_666_666),
            audio_codec: None,
            quality_score: 75,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(6_000_000),
            profile: None,
            bits_per_pixel: 0.1,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
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
        let det = crate::detection_api::Detection {
            file_path: "/t.mp4".into(),
            format: "mp4".into(),
            codec: DetectedCodec::AV1,
            codec_long: "AV1".into(),
            compression: CompressionType::VisuallyLossless,
            width: Some(3840),
            height: Some(2160),
            frame_count: Some(3600),
            fps: Some(60.0),
            duration_secs: Some(60.0),
            bit_depth: Some(10),
            pix_fmt: "yuv420p10le".into(),
            file_size: 500_000_000,
            bitrate: Some(66_666_666),
            audio_codec: Some("opus".into()),
            quality_score: 95,
            color_space: ColorSpace::BT2020,
            video_bitrate: Some(60_000_000),
            profile: None,
            bits_per_pixel: foundation::constants::VIDEO_BITS_PER_PIXEL_HIGH,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
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
        let det = crate::detection_api::Detection {
            file_path: "/t.mkv".into(),
            format: "mkv".into(),
            codec: DetectedCodec::FFV1,
            codec_long: "FFV1".into(),
            compression: CompressionType::Lossless,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(900),
            fps: Some(30.0),
            duration_secs: Some(30.0),
            bit_depth: Some(10),
            pix_fmt: "yuv444p10le".into(),
            file_size: 2_000_000_000,
            bitrate: Some(533_333_333),
            audio_codec: None,
            quality_score: 100,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(533_333_333),
            profile: None,
            bits_per_pixel: 8.5,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
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
        let det = crate::detection_api::Detection {
            file_path: "/t.mov".into(),
            format: "mov".into(),
            codec: DetectedCodec::ProRes,
            codec_long: "ProRes".into(),
            compression: CompressionType::VisuallyLossless,
            width: Some(1920),
            height: Some(1080),
            frame_count: Some(1800),
            fps: Some(30.0),
            duration_secs: Some(60.0),
            bit_depth: Some(10),
            pix_fmt: "yuv422p10le".into(),
            file_size: 1_000_000_000,
            bitrate: Some(133_333_333),
            audio_codec: Some("pcm_s24le".into()),
            quality_score: 98,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(130_000_000),
            profile: None,
            bits_per_pixel: 2.1,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
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
        let det = crate::detection_api::Detection {
            file_path: "/t.webm".into(),
            format: "webm".into(),
            codec: DetectedCodec::Unknown("vp9".into()),
            codec_long: "VP9".into(),
            compression: CompressionType::Standard,
            width: Some(1280),
            height: Some(720),
            frame_count: Some(900),
            fps: Some(30.0),
            duration_secs: Some(30.0),
            bit_depth: Some(8),
            pix_fmt: "yuv420p".into(),
            file_size: 10_000_000,
            bitrate: Some(2_666_666),
            audio_codec: None,
            quality_score: 70,
            color_space: ColorSpace::BT709,
            video_bitrate: Some(2_500_000),
            profile: None,
            bits_per_pixel: 0.09,
            color_primaries: None,
            color_transfer: None,
            mastering_display: None,
            max_cll: None,
            dv_profile: None,
            dv_bl_signal_compatibility_id: None,
            subtitle_codec: None,
            max_b_frames: Some(0),
            encoder_params: None,
            audio_channels: None,
            precision: foundation::video_detection::VideoPrecisionMetadata::default(),
            tags: std::collections::HashMap::new(),
            pts_deltas: test_pts_deltas(),
            pkt_sizes: test_pkt_sizes(),
            ..Default::default()
        };
        let normal = determine_strategy_with_apple_compat(
            &det,
            Path::new(&det.file_path),
            false,
            false,
            SelectedCodec::Hevc,
        );
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
        use crate::detection_api::{ColorSpace, Detection, VideoFlags, VideoHdrFlags};
        use std::path::PathBuf;

        // Mock a 10-bit HDR10+ result
        let detection = Detection {
            file_path: "test.mp4".to_string(),
            bit_depth: Some(10),
            color_space: ColorSpace::BT2020,
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("smpte2084".to_string()),
            flags: VideoFlags {
                hdr: VideoHdrFlags {
                    is_hdr10_plus: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let mock_json_path = PathBuf::from("/tmp/hdr10plus.json");
        let hdr10plus = Hdr10PlusResult {
            json_path: mock_json_path,
            _temp_dir: tempfile::tempdir().unwrap_or_else(|e| panic!("error: {e:?}")),
        };
        let hdr_x265_params_opt = Some(build_hevc_x265_extra_params(
            &detection,
            hdr_pix_fmt(&detection),
            None,
            Some(&hdr10plus),
        ));

        // Verify the result
        let Some(final_params) = hdr_x265_params_opt else {
            panic!("missing params");
        };
        assert!(final_params.contains("hdr10=1"));
        assert!(final_params.contains("hdr-opt=1"));
        assert!(final_params.contains("repeat-headers=1"));
        assert!(final_params.contains("dhdr10-info=/tmp/hdr10plus.json"));

        log_detail!("HDR10+ x265-params injection verified: {final_params}");
    }

    #[test]
    fn hdr10plus_tool_missing_fails_closed_without_static_hdr10_fallback() {
        use crate::detection_api::{Detection, VideoFlags, VideoHdrFlags};

        let detection = Detection {
            file_path: "/tmp/hdr10plus-source.mp4".to_string(),
            flags: VideoFlags {
                hdr: VideoHdrFlags {
                    is_hdr10_plus: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let Err(err) = hdr10plus_tool_missing_decision(&detection) else {
            panic!("HDR10+ missing-tool path unexpectedly allowed static HDR10 fallback");
        };

        let message = err.to_string();
        assert!(message.contains("HDR10+ dynamic metadata"));
        assert!(message.contains("hdr10plus_tool unavailable"));
        assert!(message.contains("fail closed"));
    }

    #[test]
    fn concurrent_output_skip_has_empty_output_path() {
        let detection = Detection {
            file_path: "/tmp/input.mp4".to_string(),
            file_size: 42,
            ..Default::default()
        };
        let strategy = ConversionStrategy {
            target: TargetVideoFormat::HevcMov,
            reason: "test".to_string(),
            command: String::new(),
            preserve_audio: false,
            crf: 23.0,
            lossless: false,
        };

        let output = concurrent_output_skip_conversion_output(
            Path::new("/tmp/input.mp4"),
            &detection,
            strategy,
        );

        assert_eq!(output.output_path, "");
        assert_eq!(output.output_size, 0);
        assert_eq!(output.message, "Skipped: output was created concurrently");
    }

    #[test]
    fn test_build_hdr_ffmpeg_args_normalizes_bt2020_yuv_output() {
        use crate::detection_api::ColorSpace;

        let detection = Detection {
            color_space: ColorSpace::BT2020,
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("smpte2084".to_string()),
            ..Default::default()
        };

        assert_eq!(
            build_hdr_ffmpeg_args(&detection),
            vec![
                "-colorspace",
                "bt2020nc",
                "-color_trc",
                "smpte2084",
                "-color_primaries",
                "bt2020"
            ]
        );
    }

    #[test]
    fn test_build_hdr_ffmpeg_args_skips_rgb_colorspace_for_yuv_output() {
        use crate::detection_api::ColorSpace;

        let detection = Detection {
            color_space: ColorSpace::Unknown("rgb".to_string()),
            color_primaries: Some("bt709".to_string()),
            color_transfer: Some("bt709".to_string()),
            ..Default::default()
        };

        assert_eq!(
            build_hdr_ffmpeg_args(&detection),
            vec!["-color_trc", "bt709", "-color_primaries", "bt709"]
        );
    }

    #[test]
    fn test_non_hdr_10_bit_does_not_inject_hdr_x265_params() {
        let detection = Detection {
            file_path: "sdr-10bit.mov".to_string(),
            bit_depth: Some(10),
            ..Default::default()
        };

        let hdr_x265_params =
            build_hevc_x265_extra_params(&detection, hdr_pix_fmt(&detection), None, None);

        assert!(requires_high_bit_depth_encode(&detection));
        assert_eq!(hdr_x265_params, "");
        assert_eq!(
            hdr_pix_fmt(&detection),
            foundation::constants::PIX_FMT_YUV420P10LE
        );
    }

    #[test]
    fn test_hlg_10_bit_preserves_precision_without_injecting_hdr_x265_params() {
        use crate::detection_api::ColorSpace;

        let detection = Detection {
            file_path: "hlg-10bit.mov".to_string(),
            bit_depth: Some(10),
            color_space: ColorSpace::BT2020,
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some(foundation::constants::HDR_TRANSFER_HLG.to_string()),
            ..Default::default()
        };

        let hdr_x265_params =
            build_hevc_x265_extra_params(&detection, hdr_pix_fmt(&detection), None, None);

        assert!(requires_high_bit_depth_encode(&detection));
        assert!(detection.is_hdr());
        assert_eq!(hdr_x265_params, "");
        assert_eq!(
            hdr_pix_fmt(&detection),
            foundation::constants::PIX_FMT_YUV420P10LE
        );
    }

    #[test]
    fn test_bt2020_sdr_10_bit_preserves_precision_without_injecting_hdr_x265_params() {
        use crate::detection_api::ColorSpace;

        let detection = Detection {
            file_path: "bt2020-sdr-10bit.mov".to_string(),
            bit_depth: Some(10),
            color_space: ColorSpace::BT2020,
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some("bt709".to_string()),
            ..Default::default()
        };

        let hdr_x265_params =
            build_hevc_x265_extra_params(&detection, hdr_pix_fmt(&detection), None, None);

        assert!(requires_high_bit_depth_encode(&detection));
        assert!(!detection.is_hdr());
        assert_eq!(hdr_x265_params, "");
        assert_eq!(
            hdr_pix_fmt(&detection),
            foundation::constants::PIX_FMT_YUV420P10LE
        );
    }

    #[test]
    fn test_hlg_static_metadata_does_not_inject_hdr10_x265_params() {
        use crate::detection_api::ColorSpace;

        let detection = Detection {
            file_path: "hlg-static-metadata.mov".to_string(),
            bit_depth: Some(10),
            color_space: ColorSpace::BT2020,
            color_primaries: Some("bt2020".to_string()),
            color_transfer: Some(foundation::constants::HDR_TRANSFER_HLG.to_string()),
            mastering_display: Some(
                "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)".to_string(),
            ),
            max_cll: Some("1000,400".to_string()),
            ..Default::default()
        };

        let params = build_hevc_x265_extra_params(&detection, hdr_pix_fmt(&detection), None, None);

        assert_eq!(params, "");
    }

    #[test]
    fn test_gif_like_video_recovery() {
        use crate::detection_api::{CompressionType, DetectedCodec};
        let det = crate::detection_api::Detection {
            file_path: "sticker.mp4".into(),
            codec: DetectedCodec::H264,
            compression: CompressionType::Standard,
            width: Some(512),
            height: Some(512),
            duration_secs: Some(2.0),
            frame_count: Some(50),
            fps: Some(25.0),
            file_size: 500_000,
            ..Default::default()
        };

        let meta = foundation::LoopMeta::from_video_detection(&det);
        let profile = foundation::unit_test_loop_reference_profile();
        let verdict = foundation::evaluate_loop_tree(&meta, Some(&profile)).verdict;
        assert!(
            verdict.is_keep_gif(),
            "short silent sticker-like clip should stay in GIF domain, got {verdict:?}"
        );
        assert!(
            verdict.reason().contains("Layer"),
            "unexpected tree reason: {}",
            verdict.reason()
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
        let _stale_detection = crate::detection_api::Detection {
            file_path: "/stale/cache-hit.mp4".to_string(),
            format: "gif".into(),
            codec: DetectedCodec::Unknown("gif".into()),
            compression: CompressionType::Lossless,
            width: Some(1),
            height: Some(1),
            duration_secs: Some(0.2),
            frame_count: Some(2),
            fps: Some(10.0),
            file_size: std::fs::metadata(gif.path())
                .unwrap_or_else(|e| panic!("error: {e:?}"))
                .len(),
            ..Default::default()
        };

        assert!(foundation::should_use_gif_fast_path(gif.path()));
        let meta = foundation::LoopMeta::from_gif_path(gif.path()).unwrap_or_else(|| {
            panic!(
                "native GIF at {} must yield loop meta",
                gif.path().display()
            )
        });
        let profile = foundation::unit_test_loop_reference_profile();
        let verdict = foundation::evaluate_loop_tree(&meta, Some(&profile)).verdict;
        assert!(
            verdict.is_keep_gif(),
            "loop tree must use current gif path, not stale detection.file_path; got {verdict:?}"
        );
        assert!(
            verdict.reason().contains("Layer 0") || verdict.reason().contains("Layer 1-B"),
            "unexpected reason: {}",
            verdict.reason()
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
        let _stale_detection = crate::detection_api::Detection {
            file_path: "/stale/cache-hit.mp4".to_string(),
            format: "gif".into(),
            codec: DetectedCodec::Unknown("gif".into()),
            compression: CompressionType::Lossless,
            width: Some(1),
            height: Some(1),
            duration_secs: Some(0.2),
            frame_count: Some(2),
            fps: Some(10.0),
            file_size: std::fs::metadata(gif.path())
                .unwrap_or_else(|e| panic!("error: {e:?}"))
                .len(),
            ..Default::default()
        };

        let meta = foundation::LoopMeta::from_gif_path(gif.path()).unwrap_or_else(|| {
            panic!(
                "native GIF at {} must yield loop meta",
                gif.path().display()
            )
        });
        let profile = foundation::unit_test_loop_reference_profile();
        let tree_verdict = foundation::evaluate_loop_tree(&meta, Some(&profile)).verdict;
        let verdict = foundation::apply_apple_compat_modern_animation_policy(
            tree_verdict,
            &meta,
            true,
            false,
        );
        assert!(
            verdict.is_keep_gif(),
            "apple-compat GIF loop should keep GIF domain, got {verdict:?}"
        );
    }

    #[test]
    fn test_apple_compat_forces_gif_for_modern_animated_webp_even_if_loop_tree_errors() {
        use crate::detection_api::{CompressionType, DetectedCodec};

        // Simulate an animated WebP with degenerate duration (the historical edge case).
        // Apple compat must still force GIF delivery for modern animated formats *when it is
        // clearly an animated-image (short / sticker-like) asset*.
        let det = crate::detection_api::Detection {
            file_path: "IMG_0116.WEBP".into(),
            format: "webp".into(),
            codec: DetectedCodec::Unknown("webp".into()),
            compression: CompressionType::Standard,
            width: Some(512),
            height: Some(512),
            duration_secs: Some(0.0),
            frame_count: Some(12),
            fps: Some(0.0),
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
        let det = crate::detection_api::Detection {
            file_path: "LONG_ANIM.WEBP".into(),
            format: "webp".into(),
            codec: DetectedCodec::Unknown("webp".into()),
            compression: CompressionType::Standard,
            width: Some(720),
            height: Some(720),
            duration_secs: Some(foundation::constants::EXTREME_LONG_ABSOLUTE_LIMIT_SECS + 5.0),
            frame_count: Some(600),
            fps: Some(30.0),
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
    fn test_explore_quality_failure_reports_pure_media_reason() {
        let explore_result = foundation::ExploreResult {
            quality_passed: foundation::types::CheckResult::Failed(
                "Pure media not smaller than input".to_string(),
            ),
            ssim: Some(0.99_f64),
            actual_min_ssim: 0.95,
            input_pure_media_size: 1_000_000,
            output_pure_media_size: 1_100_000,
            ..Default::default()
        };
        let decision = ExploreGateRejectionDecision::inspect_and_log(
            Path::new("/tmp/test.mov"),
            &explore_result,
        );

        assert_eq!(
            decision.message,
            "Skipped: Pure media not smaller than input"
        );
        assert!(
            decision
                .reason
                .contains("Pure media not smaller than input")
        );
        assert!(!decision.failed);
        assert!(!decision.message.contains("total file"));
        let output = decision
            .into_output(
                Path::new("/tmp/test.mov"),
                &Detection {
                    file_size: 1_000_000,
                    ..Default::default()
                },
                &explore_result,
            )
            .expect("size gate output");
        assert_eq!(output.outcome(), foundation::conversion::Outcome::Skipped);
    }

    #[test]
    fn quality_verification_rejection_is_failed_not_skipped() {
        let explore_result = foundation::ExploreResult {
            quality_passed: foundation::types::CheckResult::Failed(
                "SSIM below threshold".to_string(),
            ),
            ssim: Some(0.80),
            actual_min_ssim: 0.95,
            ..Default::default()
        };
        let decision = ExploreGateRejectionDecision::inspect_and_log(
            Path::new("/tmp/test.mov"),
            &explore_result,
        );

        assert!(decision.failed);
        assert!(decision.message.starts_with("Failed:"));
        let output = decision
            .into_output(
                Path::new("/tmp/test.mov"),
                &Detection {
                    file_size: 1_000_000,
                    ..Default::default()
                },
                &explore_result,
            )
            .expect("quality failure output");
        assert_eq!(output.outcome(), foundation::conversion::Outcome::Failed);
    }

    #[test]
    fn animated_task_failure_remains_failed_in_video_adapter() {
        let input = Path::new("/tmp/corrupt.gif");
        let output = task_result_to_conversion_output(
            input,
            &Detection {
                file_size: 100,
                ..Default::default()
            },
            ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: "decode failure".to_string(),
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            },
            foundation::TaskResult::failed(input, 100, "decode failed", "decode_failed"),
            0.0,
            0,
            None,
        )
        .expect("failed task output");

        assert_eq!(output.outcome(), foundation::conversion::Outcome::Failed);
        assert_eq!(output.message, "decode failed");
    }

    #[test]
    fn test_hdr_pix_fmt_uses_10_bit_when_hdr_metadata_exists_without_explicit_bit_depth() {
        let detection = crate::detection_api::Detection {
            file_path: "/test/hdr.mov".to_string(),
            color_transfer: Some("smpte2084".to_string()),
            bit_depth: None,
            ..Default::default()
        };

        assert_eq!(
            hdr_pix_fmt(&detection),
            foundation::constants::PIX_FMT_YUV420P10LE
        );
    }

    #[test]
    fn test_inferred_10_bit_preserves_pix_fmt_without_requesting_hdr_signaling() {
        let detection = crate::detection_api::Detection {
            file_path: "/test/inferred-10bit.mov".to_string(),
            bit_depth: Some(10),
            precision: crate::detection_api::VideoPrecisionMetadata {
                bit_depth_inferred_from_pix_fmt: true,
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(requires_high_bit_depth_encode(&detection));
        assert_eq!(
            build_hevc_x265_extra_params(&detection, hdr_pix_fmt(&detection), None, None),
            ""
        );
        assert_eq!(
            hdr_pix_fmt(&detection),
            foundation::constants::PIX_FMT_YUV420P10LE
        );
    }

    #[test]
    fn test_vid_ignores_unsupported_static_image_cleanly() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let heic_path = temp.path().join("test.heic");

        // We need to write a valid minimal MP4 header disguised as HEIC so ffprobe doesn't crash
        // immediately with "Invalid data" but instead returns 0 frames and no duration.
        // Even simpler: create a 1-frame MP4 using ffmpeg, name it .heic
        // We write a pre-computed minimal 1-frame MP4 (generated by ffmpeg) disguised as HEIC
        // so ffprobe does not crash with "Invalid data" but instead returns 0 duration or ignores it safely.
        let minimal_mp4: &[u8] = &[
            0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d, 0x00, 0x00,
            0x02, 0x00, 0x69, 0x73, 0x6f, 0x6d, 0x69, 0x73, 0x6f, 0x32, 0x61, 0x76, 0x63, 0x31,
            0x6d, 0x70, 0x34, 0x31, 0x00, 0x00, 0x00, 0x08, 0x66, 0x72, 0x65, 0x65, 0x00, 0x00,
            0x02, 0xcd, 0x6d, 0x64, 0x61, 0x74, 0x00, 0x00, 0x02, 0xae, 0x06, 0x05, 0xff, 0xff,
            0xaa, 0xdc, 0x45, 0xe9, 0xbd, 0xe6, 0xd9, 0x48, 0xb7, 0x96, 0x2c, 0xd8, 0x20, 0xd9,
            0x23, 0xee, 0xef, 0x78, 0x32, 0x36, 0x34, 0x20, 0x2d, 0x20, 0x63, 0x6f, 0x72, 0x65,
            0x20, 0x31, 0x36, 0x35, 0x20, 0x72, 0x33, 0x32, 0x32, 0x32, 0x20, 0x62, 0x33, 0x35,
            0x36, 0x30, 0x35, 0x61, 0x20, 0x2d, 0x20, 0x48, 0x2e, 0x32, 0x36, 0x34, 0x2f, 0x4d,
            0x50, 0x45, 0x47, 0x2d, 0x34, 0x20, 0x41, 0x56, 0x43, 0x20, 0x63, 0x6f, 0x64, 0x65,
            0x63, 0x20, 0x2d, 0x20, 0x43, 0x6f, 0x70, 0x79, 0x6c, 0x65, 0x66, 0x74, 0x20, 0x32,
            0x30, 0x30, 0x33, 0x2d, 0x32, 0x30, 0x32, 0x35, 0x20, 0x2d, 0x20, 0x68, 0x74, 0x74,
            0x70, 0x3a, 0x2f, 0x2f, 0x77, 0x77, 0x77, 0x2e, 0x76, 0x69, 0x64, 0x65, 0x6f, 0x6c,
            0x61, 0x6e, 0x2e, 0x6f, 0x72, 0x67, 0x2f, 0x78, 0x32, 0x36, 0x34, 0x2e, 0x68, 0x74,
            0x6d, 0x6c, 0x20, 0x2d, 0x20, 0x6f, 0x70, 0x74, 0x69, 0x6f, 0x6e, 0x73, 0x3a, 0x20,
            0x63, 0x61, 0x62, 0x61, 0x63, 0x3d, 0x31, 0x20, 0x72, 0x65, 0x66, 0x3d, 0x33, 0x20,
            0x64, 0x65, 0x62, 0x6c, 0x6f, 0x63, 0x6b, 0x3d, 0x31, 0x3a, 0x30, 0x3a, 0x30, 0x20,
            0x61, 0x6e, 0x61, 0x6c, 0x79, 0x73, 0x65, 0x3d, 0x30, 0x78, 0x33, 0x3a, 0x30, 0x78,
            0x31, 0x31, 0x33, 0x20, 0x6d, 0x65, 0x3d, 0x68, 0x65, 0x78, 0x20, 0x73, 0x75, 0x62,
            0x6d, 0x65, 0x3d, 0x37, 0x20, 0x70, 0x73, 0x79, 0x3d, 0x31, 0x20, 0x70, 0x73, 0x79,
            0x5f, 0x72, 0x64, 0x3d, 0x31, 0x2e, 0x30, 0x30, 0x3a, 0x30, 0x2e, 0x30, 0x30, 0x20,
            0x6d, 0x69, 0x78, 0x65, 0x64, 0x5f, 0x72, 0x65, 0x66, 0x3d, 0x31, 0x20, 0x6d, 0x65,
            0x5f, 0x72, 0x61, 0x6e, 0x67, 0x65, 0x3d, 0x31, 0x36, 0x20, 0x63, 0x68, 0x72, 0x6f,
            0x6d, 0x61, 0x5f, 0x6d, 0x65, 0x3d, 0x31, 0x20, 0x74, 0x72, 0x65, 0x6c, 0x6c, 0x69,
            0x73, 0x3d, 0x31, 0x20, 0x38, 0x78, 0x38, 0x64, 0x63, 0x74, 0x3d, 0x31, 0x20, 0x63,
            0x71, 0x6d, 0x3d, 0x30, 0x20, 0x64, 0x65, 0x61, 0x64, 0x7a, 0x6f, 0x6e, 0x65, 0x3d,
            0x32, 0x31, 0x2c, 0x31, 0x31, 0x20, 0x66, 0x61, 0x73, 0x74, 0x5f, 0x70, 0x73, 0x6b,
            0x69, 0x70, 0x3d, 0x31, 0x20, 0x63, 0x68, 0x72, 0x6f, 0x6d, 0x61, 0x5f, 0x71, 0x70,
            0x5f, 0x6f, 0x66, 0x66, 0x73, 0x65, 0x74, 0x3d, 0x2d, 0x32, 0x20, 0x74, 0x68, 0x72,
            0x65, 0x61, 0x64, 0x73, 0x3d, 0x31, 0x20, 0x6c, 0x6f, 0x6f, 0x6b, 0x61, 0x68, 0x65,
            0x61, 0x64, 0x5f, 0x74, 0x68, 0x72, 0x65, 0x61, 0x64, 0x73, 0x3d, 0x31, 0x20, 0x73,
            0x6c, 0x69, 0x63, 0x65, 0x64, 0x5f, 0x74, 0x68, 0x72, 0x65, 0x61, 0x64, 0x73, 0x3d,
            0x30, 0x20, 0x6e, 0x72, 0x3d, 0x30, 0x20, 0x64, 0x65, 0x63, 0x69, 0x6d, 0x61, 0x74,
            0x65, 0x3d, 0x31, 0x20, 0x69, 0x6e, 0x74, 0x65, 0x72, 0x6c, 0x61, 0x63, 0x65, 0x64,
            0x3d, 0x30, 0x20, 0x62, 0x6c, 0x75, 0x72, 0x61, 0x79, 0x5f, 0x63, 0x6f, 0x6d, 0x70,
            0x61, 0x74, 0x3d, 0x30, 0x20, 0x63, 0x6f, 0x6e, 0x73, 0x74, 0x72, 0x61, 0x69, 0x6e,
            0x65, 0x64, 0x5f, 0x69, 0x6e, 0x74, 0x72, 0x61, 0x3d, 0x30, 0x20, 0x62, 0x66, 0x72,
            0x61, 0x6d, 0x65, 0x73, 0x3d, 0x33, 0x20, 0x62, 0x5f, 0x70, 0x79, 0x72, 0x61, 0x6d,
            0x69, 0x64, 0x3d, 0x32, 0x20, 0x62, 0x5f, 0x61, 0x64, 0x61, 0x70, 0x74, 0x3d, 0x31,
            0x20, 0x62, 0x5f, 0x62, 0x69, 0x61, 0x73, 0x3d, 0x30, 0x20, 0x64, 0x69, 0x72, 0x65,
            0x63, 0x74, 0x3d, 0x31, 0x20, 0x77, 0x65, 0x69, 0x67, 0x68, 0x74, 0x62, 0x3d, 0x31,
            0x20, 0x6f, 0x70, 0x65, 0x6e, 0x5f, 0x67, 0x6f, 0x70, 0x3d, 0x30, 0x20, 0x77, 0x65,
            0x69, 0x67, 0x68, 0x74, 0x70, 0x3d, 0x32, 0x20, 0x6b, 0x65, 0x79, 0x69, 0x6e, 0x74,
            0x3d, 0x32, 0x35, 0x30, 0x20, 0x6b, 0x65, 0x79, 0x69, 0x6e, 0x74, 0x5f, 0x6d, 0x69,
            0x6e, 0x3d, 0x32, 0x35, 0x20, 0x73, 0x63, 0x65, 0x6e, 0x65, 0x63, 0x75, 0x74, 0x3d,
            0x34, 0x30, 0x20, 0x69, 0x6e, 0x74, 0x72, 0x61, 0x5f, 0x72, 0x65, 0x66, 0x72, 0x65,
            0x73, 0x68, 0x3d, 0x30, 0x20, 0x72, 0x63, 0x5f, 0x6c, 0x6f, 0x6f, 0x6b, 0x61, 0x68,
            0x65, 0x61, 0x64, 0x3d, 0x34, 0x30, 0x20, 0x72, 0x63, 0x3d, 0x63, 0x72, 0x66, 0x20,
            0x6d, 0x62, 0x74, 0x72, 0x65, 0x65, 0x3d, 0x31, 0x20, 0x63, 0x72, 0x66, 0x3d, 0x32,
            0x33, 0x2e, 0x30, 0x20, 0x71, 0x63, 0x6f, 0x6d, 0x70, 0x3d, 0x30, 0x2e, 0x36, 0x30,
            0x20, 0x71, 0x70, 0x6d, 0x69, 0x6e, 0x3d, 0x30, 0x20, 0x71, 0x70, 0x6d, 0x61, 0x78,
            0x3d, 0x36, 0x39, 0x20, 0x71, 0x70, 0x73, 0x74, 0x65, 0x70, 0x3d, 0x34, 0x20, 0x69,
            0x70, 0x5f, 0x72, 0x61, 0x74, 0x69, 0x6f, 0x3d, 0x31, 0x2e, 0x34, 0x30, 0x20, 0x61,
            0x71, 0x3d, 0x31, 0x3a, 0x31, 0x2e, 0x30, 0x30, 0x00, 0x80, 0x00, 0x00, 0x00, 0x0f,
            0x65, 0x88, 0x84, 0x00, 0x2b, 0xff, 0xfe, 0xf6, 0x73, 0x7c, 0x0a, 0x6b, 0x6d, 0xb1,
            0x81, 0x00, 0x00, 0x03, 0x17, 0x6d, 0x6f, 0x6f, 0x76, 0x00, 0x00, 0x00, 0x6c, 0x6d,
            0x76, 0x68, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x00, 0x00, 0x28, 0x00, 0x01, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x02, 0x41, 0x74, 0x72, 0x61, 0x6b, 0x00,
            0x00, 0x00, 0x5c, 0x74, 0x6b, 0x68, 0x64, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00,
            0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x65, 0x64, 0x74,
            0x73, 0x00, 0x00, 0x00, 0x1c, 0x65, 0x6c, 0x73, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x01, 0xb9, 0x6d, 0x64, 0x69, 0x61, 0x00, 0x00, 0x00, 0x20, 0x6d,
            0x64, 0x68, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x32, 0x00, 0x00, 0x00, 0x02, 0x00, 0x55, 0xc4, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x2d, 0x68, 0x64, 0x6c, 0x72, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x76, 0x69, 0x64, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x56, 0x69, 0x64, 0x65, 0x6f, 0x48, 0x61, 0x6e, 0x64, 0x6c, 0x65,
            0x72, 0x00, 0x00, 0x00, 0x01, 0x64, 0x6d, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x14,
            0x76, 0x6d, 0x68, 0x64, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x64, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x1c,
            0x64, 0x72, 0x65, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x0c, 0x75, 0x72, 0x6c, 0x20, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x01, 0x24,
            0x73, 0x74, 0x62, 0x6c, 0x00, 0x00, 0x00, 0xc0, 0x73, 0x74, 0x73, 0x64, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xb0, 0x61, 0x76, 0x63, 0x31,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x02,
            0x00, 0x48, 0x00, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            0x15, 0x4c, 0x61, 0x76, 0x63, 0x36, 0x32, 0x2e, 0x32, 0x38, 0x2e, 0x31, 0x30, 0x31,
            0x20, 0x6c, 0x69, 0x62, 0x78, 0x32, 0x36, 0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0xff, 0xff, 0x00, 0x00, 0x00, 0x36, 0x61, 0x76,
            0x63, 0x43, 0x01, 0x64, 0x00, 0x0a, 0xff, 0xe1, 0x00, 0x19, 0x67, 0x64, 0x00, 0x0a,
            0xac, 0xd9, 0x5f, 0x88, 0x88, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00, 0x00,
            0x03, 0x00, 0xc8, 0x3c, 0x48, 0x96, 0x58, 0x01, 0x00, 0x06, 0x68, 0xeb, 0xe3, 0xcb,
            0x22, 0xc0, 0xfd, 0xf8, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x10, 0x70, 0x61, 0x73, 0x70,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x14, 0x62, 0x74,
            0x72, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x29, 0xe8, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x18, 0x73, 0x74, 0x74, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x1c,
            0x73, 0x74, 0x73, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x14,
            0x73, 0x74, 0x73, 0x7a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xc5, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x14, 0x73, 0x74, 0x63, 0x6f, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x30, 0x00, 0x00, 0x00, 0x62, 0x75, 0x64,
            0x74, 0x61, 0x00, 0x00, 0x00, 0x5a, 0x6d, 0x65, 0x74, 0x61, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x21, 0x68, 0x64, 0x6c, 0x72, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x6d, 0x64, 0x69, 0x72, 0x61, 0x70, 0x70, 0x6c, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2d, 0x69, 0x6c, 0x73, 0x74, 0x00,
            0x00, 0x00, 0x25, 0xa9, 0x74, 0x6f, 0x6f, 0x00, 0x00, 0x00, 0x1d, 0x64, 0x61, 0x74,
            0x61, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x4c, 0x61, 0x76, 0x66, 0x36,
            0x32, 0x2e, 0x31, 0x32, 0x2e, 0x31, 0x30, 0x31,
        ];
        std::fs::write(&heic_path, minimal_mp4).expect("failed to write dummy MP4");

        let config = ConversionConfig {
            codec: SelectedCodec::Hevc,
            ..Default::default()
        };

        // Call the main conversion function without a cache connection
        let result =
            crate::conversion_api::auto_convert_with_cache(&heic_path, &config, None).unwrap();

        // It MUST return Ignored without copying or attempting a video encode.
        assert!(result.ignored, "Should be cleanly ignored by vid module");
        assert_eq!(result.strategy.target, TargetVideoFormat::Ignored);
        assert!(
            result
                .message
                .contains("vid ignores potentially non-animated media")
                || result.message.contains("vid ignores static media")
        );
    }

    #[test]
    fn cache_exact_hint_uses_pipeline_acceptable_not_quality_or_size_or() {
        use foundation::ExploreResult;
        use foundation::types::CheckResult;

        let size_only_ok = ExploreResult {
            size_target_met: CheckResult::Passed,
            quality_passed: CheckResult::Failed("below SSIM floor".into()),
            size_change_pct: -8.0,
            confidence: Some(1.0),
            ..Default::default()
        };
        assert!(
            !success_status_for_cache(TargetVideoFormat::HevcMp4, Some(&size_only_ok), true, false,),
            "match_quality mode must not treat size-only pass as exact cache hit"
        );
        assert!(success_status_for_cache(
            TargetVideoFormat::HevcMp4,
            Some(&size_only_ok),
            false,
            true,
        ));

        let both_failed = ExploreResult {
            quality_passed: CheckResult::Failed("quality".into()),
            size_target_met: CheckResult::Failed("size".into()),
            confidence: Some(1.0),
            ..Default::default()
        };
        assert!(!success_status_for_cache(
            TargetVideoFormat::HevcMp4,
            Some(&both_failed),
            true,
            true,
        ));
    }
}
