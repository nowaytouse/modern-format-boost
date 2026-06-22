//! Lossless Converter Module
//!
//! Provides conversion API for verified lossless/lossy images.
//! Uses `foundation` for common functionality (anti-duplicate, `TaskResult`, etc.)
//!
//! **Unified Compress Check**: All image conversions call `check_size_tolerance` after
//! successful encoding and obtaining `output_size`, before finalization. When `options.compress`
//! is true, only accept when output < input, otherwise skip and keep original file.
//! Covered paths: `convert_to_jxl`, `convert_jpeg_to_jxl` (including fallback),
//! `convert_to_avif`, `convert_to_avif_lossless`, `convert_to_jxl_matched`.

#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::unnecessary_wraps)]

use crate::Rational;
use crate::{ImgQualityError, Result};
use foundation::ImagePrecisionProfile;
use foundation::ToolBuilder;
use foundation::ffprobe_json::ColorInfo;
use foundation::image_analyzer::{ConversionColorContext, ConversionColorRole};
use foundation::image_jpeg_analysis::is_jpeg_complete;
use foundation::jxl_effort_policy::{JxlEffortPlan, JxlEffortSearchKind, size_ge_1mib};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::time::Duration;

pub use foundation::conversion::{
    ConvertFlags, ConvertOptions, TaskResult, check_size_tolerance, clear_processed_list,
    determine_output_path_with_base, finalize_task, format_size_change, is_already_processed,
    mark_as_processed,
};
use foundation::{EXT_AVIF, EXT_JXL, LABEL_AVIF, LABEL_JXL, log_detail, log_skip, log_stat};

/// Output size as percent of input. Returns `None` when input size is unknown (zero).
fn output_size_ratio_pct(input_size: u64, output_size: u64) -> Option<f64> {
    if input_size == 0 {
        return None;
    }
    Some(Rational::from((output_size, input_size)).to_f64() * 100.0)
}

fn format_output_size_ratio_pct(input_size: u64, output_size: u64) -> String {
    output_size_ratio_pct(input_size, output_size)
        .map_or_else(|| "N/A".to_string(), |pct| format!("{pct:.1}%"))
}

fn format_output_size_ratio_pct_plain(input_size: u64, output_size: u64) -> String {
    output_size_ratio_pct(input_size, output_size)
        .map_or_else(|| "N/A".to_string(), |pct| format!("{pct:.1}"))
}

const fn jxl_encode_effort_for_size(
    archive: bool,
    ultimate: bool,
    explore: bool,
    file_size: u64,
) -> u8 {
    if archive {
        return foundation::jxl_effort_policy::archive_effort(JxlEffortSearchKind::DirectEncode);
    }
    foundation::jxl_effort_policy::encode_effort_for_size(ultimate, explore, file_size)
}

#[derive(Debug, Clone, Copy)]
struct JxlEffortCandidate {
    effort: u8,
    output_size: u64,
}

fn jxl_effort_search_plan(
    archive: bool,
    ultimate: bool,
    explore: bool,
    file_size: u64,
) -> Vec<JxlEffortPlan> {
    if archive {
        return foundation::jxl_effort_policy::archive_effort_search_plan(
            JxlEffortSearchKind::DirectEncode,
        );
    }
    foundation::jxl_effort_policy::effort_search_plan(
        JxlEffortSearchKind::DirectEncode,
        ultimate,
        explore,
        file_size,
        false,
    )
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    struct JpegEffortModeFlags: u8 {
        const ARCHIVE = 1 << 0;
        const ULTIMATE = 1 << 1;
        const EXPLORE = 1 << 2;
        const ALLOW_EXPERT_OPTIONS = 1 << 3;
    }
}

fn jpeg_effort_search_plan(flags: JpegEffortModeFlags, file_size: u64) -> Vec<JxlEffortPlan> {
    if flags.contains(JpegEffortModeFlags::ARCHIVE) {
        return foundation::jxl_effort_policy::archive_effort_search_plan(
            JxlEffortSearchKind::JpegLosslessTranscode,
        );
    }
    foundation::jxl_effort_policy::effort_search_plan(
        JxlEffortSearchKind::JpegLosslessTranscode,
        flags.contains(JpegEffortModeFlags::ULTIMATE),
        flags.contains(JpegEffortModeFlags::EXPLORE),
        file_size,
        flags.contains(JpegEffortModeFlags::ALLOW_EXPERT_OPTIONS),
    )
}

fn select_jxl_effort_winner(candidates: &[JxlEffortCandidate]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| (candidate.output_size, candidate.effort))
        .map(|(idx, _)| idx)
}

const fn jxl_effort_from_plan_item(item: JxlEffortPlan) -> u8 {
    item.effort()
}

fn format_jxl_effort_plan(plan: &[JxlEffortPlan]) -> String {
    plan.iter()
        .map(|item| jxl_effort_from_plan_item(*item))
        .map(|effort| format!("e{effort}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug)]
enum JxlDirectEncodeError {
    Launch(std::io::Error),
    Conversion(String),
}

impl std::fmt::Display for JxlDirectEncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Launch(err) => write!(f, "{err}"),
            Self::Conversion(err) => f.write_str(err),
        }
    }
}

impl std::error::Error for JxlDirectEncodeError {}

fn copy_original_on_skip(input: &Path, options: &ConvertOptions) -> Result<Option<PathBuf>> {
    foundation::copy_on_skip_or_fail(
        input,
        options.output_dir.as_deref(),
        options.base_dir.as_deref(),
        options.verbose(),
    )
    .map_err(|e| ImgQualityError::ConversionError(e.to_string()))
}

fn cleanup_temp_output(temp_output: &Path, _input: &Path) {
    foundation::media_conversion_gate::delivery_remove_file_or_audit(
        "img temp output cleanup",
        temp_output,
    );
}

enum CommitOutcome {
    Skipped(TaskResult),
    Ready,
}

fn commit_with_size_check(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    output_size: u64,
    options: &ConvertOptions,
    format_label: &str,
    extra_info: Option<&str>,
) -> Result<CommitOutcome> {
    let efficiency_label = format_output_size_ratio_pct(input_size, output_size);

    let input_label = foundation::media_conversion_gate::path_file_name_for_log(input);

    log_detail!(&format!(
        "{} Finalizing bitstream: {} | Statistics: {}B -> {}B (Efficiency: {}) | Strategy: {} | Context: {:?}",
        foundation::infra::static_logs::messages::LABEL_PHASE_5,
        input_label,
        input_size,
        output_size,
        efficiency_label,
        format_label,
        extra_info
    ));

    if !foundation::conversion::commit_temp_to_output_with_metadata_pixel_already_verified(
        temp_output,
        output,
        options.force(),
        Some(input),
    )? {
        return Ok(CommitOutcome::Skipped(TaskResult::skipped_exists(
            input, output,
        )?));
    }

    if let Some(skipped) = check_size_tolerance(
        input,
        output,
        input_size,
        output_size,
        options,
        format_label,
    ) {
        return Ok(CommitOutcome::Skipped(skipped));
    }

    Ok(CommitOutcome::Ready)
}

/// Finalize conversion with size check and metadata preservation.
/// Common pattern: commit temp → check size → finalize.
/// Returns `TaskResult` on success or error.
/// # Errors
///
/// Returns an error if the temp file cannot be committed or metadata cannot be preserved.
fn finalize_with_size_check(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    output_size: u64,
    options: &ConvertOptions,
    format_label: &str,
    extra_info: Option<&str>,
) -> Result<TaskResult> {
    match commit_with_size_check(
        input,
        temp_output,
        output,
        input_size,
        output_size,
        options,
        format_label,
        extra_info,
    )? {
        CommitOutcome::Skipped(task) => Ok(task),
        CommitOutcome::Ready => {
            finalize_task(input, output, input_size, format_label, extra_info, options)
                .map_err(ImgQualityError::IoError)
        }
    }
}

fn finalize_with_sidecars_and_size_check(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    output_size: u64,
    options: &ConvertOptions,
    format_label: &str,
    extra_info: Option<&str>,
    artifacts: &foundation::hdr::HdrArtifacts,
) -> Result<TaskResult> {
    match commit_with_size_check(
        input,
        temp_output,
        output,
        input_size,
        output_size,
        options,
        format_label,
        extra_info,
    )? {
        CommitOutcome::Skipped(task) => Ok(task),
        CommitOutcome::Ready => {
            if let Err(err) = foundation::hdr::persist_hdr_artifacts(output, artifacts) {
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "hdr sidecar persist failure output cleanup",
                    output,
                );
                return Err(ImgQualityError::ConversionError(format!(
                    "Failed to persist HDR sidecars for {}: {err}",
                    output.display()
                )));
            }

            finalize_task(input, output, input_size, format_label, extra_info, options)
                .map_err(ImgQualityError::IoError)
        }
    }
}

/// Finalize a JXL produced by a fallback pipeline (ffmpeg or imagemagick).
/// Verifies health, then delegates to `finalize_with_size_check`.
fn finalize_fallback_jxl(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    options: &ConvertOptions,
    label: &str,
) -> Result<TaskResult> {
    let output_size = fs::metadata(temp_output)?.len();
    if let Err(e) = verify_jxl_health(temp_output) {
        cleanup_temp_output(temp_output, input);
        return Err(e);
    }
    finalize_with_size_check(
        input,
        temp_output,
        output,
        input_size,
        output_size,
        options,
        LABEL_JXL,
        Some(label),
    )
}

/// Convert `HEIC` with Gainmap to `HDR` `JXL`.
///
/// # Errors
///
/// Returns an error if:
/// - The input file is invalid or a duplicate.
/// - The `HDR` synthesis process fails.
/// - The output file cannot be written or finalized.
pub fn convert_heic_gainmap_to_jxl(input: &Path, options: &ConvertOptions) -> Result<TaskResult> {
    if let Err(e) = foundation::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(TaskResult::skipped_duplicate(input)?);
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, EXT_JXL, options)?;

    if output.exists() && !options.force() {
        return Ok(TaskResult::skipped_exists(input, &output)?);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    // Use 16-bit PNG for PQ HDR encoding with cjxl
    let intermediate_format = foundation::hdr::IntermediateFormat::Png16;

    // Call the synthesis logic from foundation
    let artifacts = foundation::hdr::convert_heic_with_gainmap_to_jxl(
        input,
        &temp_output,
        options.apple_compat(),
        intermediate_format,
        options.ultimate(),
        options.archive(),
    )
    .map_err(|e| {
        let msg = format!(" HDR Synthesis Failure: {e}");
        ImgQualityError::ConversionError(msg)
    })?;

    let output_size = fs::metadata(&temp_output)
        .map_err(|e| {
            ImgQualityError::ConversionError(format!(
                "Failed to retrieve HDR synthesis output metadata: {e}"
            ))
        })?
        .len();

    // Verify health
    if let Err(e) = verify_jxl_health(&temp_output) {
        cleanup_temp_output(&temp_output, input);
        return Err(ImgQualityError::ConversionError(format!(
            "⛔ Synthetic HDR JXL health check failed: {e}"
        )));
    }

    finalize_with_sidecars_and_size_check(
        input,
        &temp_output,
        &output,
        input_size,
        output_size,
        options,
        foundation::infra::static_logs::messages::LABEL_HDR_SYNTHESIS,
        None,
        &artifacts,
    )
    .map_err(|e| {
        ImgQualityError::ConversionError(format!(" HDR Synthesis Finalization Error: {e}"))
    })
}

/// Convert `UltraHDR JPEG` with gainmap metadata to synthesized HDR `JXL`.
///
/// # Errors
///
/// Returns an error if extraction, synthesis, or finalization fails.
pub fn convert_ultrahdr_jpeg_to_jxl(input: &Path, options: &ConvertOptions) -> Result<TaskResult> {
    if let Err(e) = foundation::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(TaskResult::skipped_duplicate(input)?);
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, EXT_JXL, options)?;

    if output.exists() && !options.force() {
        return Ok(TaskResult::skipped_exists(input, &output)?);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    // Synthesize into an isolated temp path so final commit/metadata handling stays atomic.
    let artifacts = foundation::hdr::convert_ultrahdr_jpeg_to_jxl(
        input,
        &temp_output,
        options.apple_compat(),
        foundation::hdr::IntermediateFormat::Png16,
        options.ultimate(),
        options.archive(),
    )
    .map_err(|e| {
        let msg = format!(" UltraHDR Synthesis Failure: {e}");
        ImgQualityError::ConversionError(msg)
    })?;

    let output_size = fs::metadata(&temp_output)
        .map_err(|e| {
            ImgQualityError::ConversionError(format!(
                "Failed to retrieve synthesized JXL metadata: {e}"
            ))
        })?
        .len();

    // Verify health
    if let Err(e) = verify_jxl_health(&temp_output) {
        cleanup_temp_output(&temp_output, input);
        return Err(ImgQualityError::ConversionError(format!(
            "⛔ Synthesized UltraHDR JXL health check failed: {e}"
        )));
    }

    finalize_with_sidecars_and_size_check(
        input,
        &temp_output,
        &output,
        input_size,
        output_size,
        options,
        foundation::infra::static_logs::messages::LABEL_ULTRAHDR_SYNTHESIS,
        Some("Native HDR"),
        &artifacts,
    )
    .map_err(|e| {
        ImgQualityError::ConversionError(format!(" UltraHDR Synthesis Finalization Error: {e}"))
    })
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.

enum FallbackResult {
    Finalized(crate::Result<TaskResult>),
    Exhausted(std::result::Result<std::process::Output, JxlDirectEncodeError>),
}

fn perform_icc_d50_retry(
    input: &Path,
    actual_input: &Path,
    temp_output: &Path,
    actual_dist: f32,
    max_threads: usize,
    options: &ConvertOptions,
    color_info: Option<&ColorInfo>,
    effort_plan: &[JxlEffortPlan],
    original_result: std::result::Result<std::process::Output, JxlDirectEncodeError>,
) -> std::result::Result<std::process::Output, JxlDirectEncodeError> {
    // Robustness: cjxl rejected the ICC profile (likely Capture One D50 rounding
    // deviation). Re-extract with D50 patch applied and retry once.
    use console::style;
    foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
        "icc_d50_retry",
        input,
        "ICC D50 rounding anomaly; retrying with patched profile",
    );
    let patched_icc = match foundation::jxl_utils::extract_icc_with_d50_patch(input) {
        Ok(patched) => patched,
        Err(err) => {
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "icc_patch_prepare",
                input,
                format!("patched ICC prepare failed ({err}); continuing without patch"),
            );
            None
        }
    };
    let patched_icc_path = patched_icc.as_ref().map(tempfile::NamedTempFile::path);

    let retry_out = run_direct_jxl_encode_effort_search(
        actual_input,
        temp_output,
        actual_dist,
        max_threads,
        options.apple_compat(),
        patched_icc_path,
        color_info,
        effort_plan,
    );
    match &retry_out {
        Ok(o) if o.status.success() => {
            log_detail!(foundation::infra::static_logs::messages::ICC_PATCH_SUCCESS);
        }
        Ok(o) => {
            let stderr_line = foundation::media_conversion_gate::stderr_first_line_label(
                &o.stderr,
                input,
                "icc_retry_stderr",
            );
            let line = format!("ICC patch retry also failed: {stderr_line}");
            log_detail!(&line);
        }
        Err(err) => {
            log_detail!(&format!("ICC patch retry launch failed: {err}"));
        }
    }
    // drop style to satisfy unused import lint when feature is off
    let _ = style("");
    retry_out.or(original_result)
}

#[allow(clippy::too_many_arguments)]
fn try_pipeline_recovery_fallbacks(
    input: &Path,
    _actual_input: &Path,
    temp_output: &Path,
    output: &Path,
    options: &ConvertOptions,
    actual_dist: f32,
    max_threads: usize,
    actual_eff: u8,
    input_size: u64,
    color_info: Option<&ColorInfo>,
    stderr: &str,
    original_result: std::result::Result<std::process::Output, JxlDirectEncodeError>,
) -> FallbackResult {
    use std::process::Stdio;
    // Check if this is a grayscale ICC profile mismatch error
    // If so, use ImageMagick fallback which has proper retry logic with -strip
    if foundation::jxl_utils::is_grayscale_icc_cjxl_error(stderr) {
        foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
            "grayscale_icc_imagemagick",
            input,
            "grayscale ICC mismatch; using ImageMagick fallback",
        );

        if try_imagemagick_fallback_with_effort(
            input,
            temp_output,
            actual_dist,
            max_threads,
            options.apple_compat(),
            actual_eff,
        )
        .is_ok()
        {
            // ImageMagick fallback succeeded — finalize directly
            let _output_size = match fs::metadata(temp_output) {
                Ok(m) => m.len(),
                Err(e) => {
                    cleanup_temp_output(temp_output, input);
                    return FallbackResult::Finalized(Err(anyhow::anyhow!(
                        "read ImageMagick fallback output metadata: {e}"
                    )
                    .into()));
                }
            };
            if let Err(e) = verify_jxl_health(temp_output) {
                cleanup_temp_output(temp_output, input);
                return FallbackResult::Finalized(Err(e));
            }
            match foundation::conversion::commit_temp_to_output_with_metadata_pixel_already_verified(
                temp_output,
                output,
                options.force(),
                Some(input),
            ) {
                Ok(true) => {
                    return FallbackResult::Finalized(
                        finalize_task(
                            input,
                            output,
                            input_size,
                            LABEL_JXL,
                            Some("(grayscale ICC fix)"),
                            options,
                        )
                        .map_err(ImgQualityError::IoError),
                    );
                }
                Ok(false) => {
                    return FallbackResult::Finalized(
                        TaskResult::skipped_exists(input, output).map_err(ImgQualityError::from),
                    );
                }
                Err(e) => {
                    return FallbackResult::Finalized(Err(ImgQualityError::IoError(e)));
                }
            }
        }
    }

    // Not a grayscale ICC error, or ImageMagick fallback failed
    // Try FFmpeg pipeline as before
    foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
        "cjxl_ffmpeg_predecode",
        input,
        format!(
            "cjxl native decode failed; FFmpeg pre-decode fallback ({})",
            stderr.trim()
        ),
    );

    let mut cjxl_fallback_color = foundation::ffprobe_json::ColorInfo::default();
    let color_for_precision = match color_info {
        None => foundation::media_conversion_gate::color_info_for_cjxl_prep(
            input,
            None,
            &mut cjxl_fallback_color,
        ),
        Some(info) => info,
    };
    let precision = ImagePrecisionProfile::inspect(input, color_for_precision);
    let pix_fmt = foundation::media_conversion_gate::precision_still_pipe_rgb_pix_fmt(&precision);

    let mut ffmpeg_builder = foundation::FfmpegBuilder::new();
    ffmpeg_builder
        .threads(max_threads)
        .input(input)
        .frames_v(1)
        .pix_fmt(pix_fmt)
        .vcodec(foundation::VideoCodec::Png)
        .format("image2pipe")
        .output_pipe(); // output to pipe

    let ffmpeg_result = ffmpeg_builder
        .build()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    match ffmpeg_result {
        Ok(mut ffmpeg_proc) => {
            if let Some(ffmpeg_stdout) = ffmpeg_proc.stdout.take() {
                let mut cjxl_builder = foundation::CjxlBuilder::new();
                cjxl_builder
                    .use_stdin(true)
                    .output(temp_output)
                    .distance(actual_dist)
                    .effort(actual_eff)
                    .threads(max_threads)
                    .apple_compat(options.apple_compat());

                let cjxl_result = cjxl_builder
                    .build()
                    .stdin(ffmpeg_stdout)
                    .stderr(Stdio::piped())
                    .spawn();

                match cjxl_result {
                    Ok(mut cjxl_proc) => {
                        let ffmpeg_stderr_thread = ffmpeg_proc.stderr.take().map(|stderr| {
                            std::thread::spawn(move || {
                                use std::io::Read;
                                let mut buf =
                                    String::with_capacity(crate::constants::STDERR_BUFFER_INITIAL);
                                if let Err(err) = stderr
                                    .take(foundation::numeric_cast::usize_to_u64(
                                        crate::constants::STDERR_BUFFER_MAX,
                                    ))
                                    .read_to_string(&mut buf)
                                {
                                    log_detail!(&format!(
                                        "Failed to read FFmpeg stderr output: {err}"
                                    ));
                                }
                                buf
                            })
                        });

                        // Drain cjxl stderr in background so cjxl does not block when pipe buffer fills.
                        let cjxl_stderr_input = input.display().to_string();
                        let cjxl_stderr_thread =
                            cjxl_proc.stderr.take().map(|stderr| {
                                std::thread::spawn(move || {
                                    use std::io::Read;
                                    let mut buf = String::with_capacity(
                                        crate::constants::STDERR_BUFFER_INITIAL,
                                    );
                                    if let Err(err) = stderr
                                        .take(foundation::numeric_cast::usize_to_u64(
                                            crate::constants::STDERR_BUFFER_MAX,
                                        ))
                                        .read_to_string(&mut buf)
                                    {
                                        foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                                            "cjxl_stderr_pipe",
                                            format!(
                                                "{cjxl_stderr_input}: failed to read cjxl stderr: {err}"
                                            ),
                                        );
                                    }
                                    buf
                                })
                            });

                        let ffmpeg_status = ffmpeg_proc.wait();
                        let cjxl_status = cjxl_proc.wait();

                        let pipe_input = input.display().to_string();
                        let ffmpeg_stderr_str = match ffmpeg_stderr_thread {
                            Some(handle) => match handle.join() {
                                Ok(res) => res,
                                Err(p) => {
                                    let msg = p
                                        .downcast_ref::<&str>()
                                        .map(|s| (*s).to_string())
                                        .or_else(|| p.downcast_ref::<String>().cloned())
                                        .unwrap_or_else(|| "non-string panic payload".to_string());
                                    foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                                        "ffmpeg_stderr_thread_panic",
                                        format!("{pipe_input}: FFmpeg stderr reader thread panicked: {msg}"),
                                    );
                                    return FallbackResult::Finalized(Err(
                                        ImgQualityError::ConversionError(format!(
                                            "FFmpeg stderr thread panicked: {msg}"
                                        )),
                                    ));
                                }
                            },
                            None => String::new(),
                        };
                        let cjxl_stderr_str = match cjxl_stderr_thread {
                            Some(handle) => match handle.join() {
                                Ok(res) => res,
                                Err(p) => {
                                    let msg = p
                                        .downcast_ref::<&str>()
                                        .map(|s| (*s).to_string())
                                        .or_else(|| p.downcast_ref::<String>().cloned())
                                        .unwrap_or_else(|| "non-string panic payload".to_string());
                                    foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                                        "cjxl_stderr_thread_panic",
                                        format!("{pipe_input}: cjxl stderr reader thread panicked: {msg}"),
                                    );
                                    return FallbackResult::Finalized(Err(
                                        ImgQualityError::ConversionError(format!(
                                            "cjxl stderr thread panicked: {msg}"
                                        )),
                                    ));
                                }
                            },
                            None => String::new(),
                        };

                        let ffmpeg_ok = match ffmpeg_status {
                            Ok(status) if status.success() => true,
                            Ok(status) => {
                                let line =
                                    format!("FFmpeg failed with exit code: {:?}", status.code());
                                log_detail!(&line);
                                if !ffmpeg_stderr_str.is_empty() {
                                    let line2 = format!(
                                        "Error: {}",
                                        foundation::media_conversion_gate::stderr_first_line_label(
                                            ffmpeg_stderr_str.as_bytes(),
                                            input,
                                            "ffmpeg_stderr",
                                        )
                                    );
                                    log_detail!(&line2);
                                }
                                false
                            }
                            Err(e) => {
                                let line = format!(" Failed to wait for FFmpeg: {e}");
                                log_detail!(&line);
                                false
                            }
                        };

                        let cjxl_ok = match cjxl_status {
                            Ok(status) if status.success() => true,
                            Ok(status) => {
                                let line =
                                    format!("cjxl failed with exit code: {:?}", status.code());
                                log_detail!(&line);
                                if !cjxl_stderr_str.is_empty() {
                                    let line2 = format!(
                                        "Error: {}",
                                        foundation::media_conversion_gate::stderr_first_line_label(
                                            cjxl_stderr_str.as_bytes(),
                                            input,
                                            "cjxl_stderr",
                                        )
                                    );
                                    log_detail!(&line2);
                                }
                                false
                            }
                            Err(e) => {
                                let line = format!(" Failed to wait for cjxl: {e}");
                                log_detail!(&line);
                                false
                            }
                        };

                        if ffmpeg_ok && cjxl_ok {
                            foundation::progress_mode::fallback_success();
                            // Early-return: finalize directly instead of faking an Output
                            let output_size = match fs::metadata(temp_output) {
                                Ok(m) => m.len(),
                                Err(e) => {
                                    cleanup_temp_output(temp_output, input);
                                    return FallbackResult::Finalized(Err(anyhow::anyhow!(
                                        "read FFmpeg fallback output metadata: {e}"
                                    )
                                    .into()));
                                }
                            };
                            if let Err(e) = verify_jxl_health(temp_output) {
                                cleanup_temp_output(temp_output, input);
                                return FallbackResult::Finalized(Err(e));
                            }
                            return FallbackResult::Finalized(finalize_with_size_check(
                                input,
                                temp_output,
                                output,
                                input_size,
                                output_size,
                                options,
                                LABEL_JXL,
                                Some("(ffmpeg fallback)"),
                            ));
                        }

                        let line = format!(
                            "FFmpeg pipeline failed for file: {} (ffmpeg: {}, cjxl: {})",
                            input.display(),
                            foundation::modern_ui::symbols::pick(
                                if ffmpeg_ok { "✓" } else { "✗" },
                                if ffmpeg_ok { "Y" } else { "N" }
                            ),
                            foundation::modern_ui::symbols::pick(
                                if cjxl_ok { "✓" } else { "✗" },
                                if cjxl_ok { "Y" } else { "N" }
                            )
                        );
                        log_detail!(&line);
                        log_detail!(" SECONDARY FALLBACK: Trying ImageMagick pipeline...",);
                        if try_imagemagick_fallback_with_effort(
                            input,
                            temp_output,
                            actual_dist,
                            max_threads,
                            options.apple_compat(),
                            actual_eff,
                        )
                        .is_ok()
                        {
                            return FallbackResult::Finalized(finalize_fallback_jxl(
                                input,
                                temp_output,
                                output,
                                input_size,
                                options,
                                "(imagemagick fallback)",
                            ));
                        }
                        FallbackResult::Exhausted(original_result)
                    }
                    Err(e) => {
                        let line = format!(" Failed to start cjxl process: {e}");
                        log_detail!(&line);
                        log_detail!(
                            "Pipeline Recovery: FFmpeg fallback exhausted; engaging ImageMagick secondary pre-decode stage",
                        );
                        if try_imagemagick_fallback_with_effort(
                            input,
                            temp_output,
                            actual_dist,
                            max_threads,
                            options.apple_compat(),
                            actual_eff,
                        )
                        .is_ok()
                        {
                            return FallbackResult::Finalized(finalize_fallback_jxl(
                                input,
                                temp_output,
                                output,
                                input_size,
                                options,
                                "(imagemagick fallback)",
                            ));
                        }
                        FallbackResult::Exhausted(original_result)
                    }
                }
            } else {
                log_detail!("Failed to capture FFmpeg stdout");
                if let Err(kill_err) = ffmpeg_proc.kill() {
                    let line =
                        format!("Failed to stop FFmpeg after stdout capture failure: {kill_err}");
                    log_detail!(&line);
                }
                log_detail!(foundation::infra::static_logs::messages::LOSSLESS_FALLBACK_MAGICK);
                if try_imagemagick_fallback_with_effort(
                    input,
                    temp_output,
                    actual_dist,
                    max_threads,
                    options.apple_compat(),
                    actual_eff,
                )
                .is_ok()
                {
                    return FallbackResult::Finalized(finalize_fallback_jxl(
                        input,
                        temp_output,
                        output,
                        input_size,
                        options,
                        "(imagemagick fallback)",
                    ));
                }
                FallbackResult::Exhausted(original_result)
            }
        }
        Err(e) => {
            let line = format!(" FFmpeg not available or failed to start: {e}");
            log_detail!(&line);
            log_detail!(
                "System Audit: Critical tool 'ffmpeg' missing from path. Pipeline cannot proceed without backend decoder."
            );
            log_detail!("  Recommended Action: Install via Homebrew: 'brew install ffmpeg'");

            log_detail!(foundation::infra::static_logs::messages::LOSSLESS_FALLBACK_MAGICK);
            if try_imagemagick_fallback_with_effort(
                input,
                temp_output,
                actual_dist,
                max_threads,
                options.apple_compat(),
                actual_eff,
            )
            .is_ok()
            {
                return FallbackResult::Finalized(finalize_fallback_jxl(
                    input,
                    temp_output,
                    output,
                    input_size,
                    options,
                    "(imagemagick fallback)",
                ));
            }
            FallbackResult::Exhausted(original_result)
        }
    }
}

pub fn convert_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    distance: f32,
    color_context: Option<&ConversionColorContext>,
) -> Result<TaskResult> {
    // Validate input file
    if let Err(e) = foundation::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(TaskResult::skipped_duplicate(input)?);
    }

    let input_size = fs::metadata(input)?.len();

    if let Some(ext) = input.extension()
        && ext.to_string_lossy().to_lowercase() == "png"
        && input_size < crate::constants::SMALL_PNG_THRESHOLD_BYTES
    {
        if options.verbose() {
            log_skip!(
                &foundation::media_conversion_gate::path_file_name_for_log(input),
                &format!(
                    "{} Skipped: Source asset below optimization threshold (PNG < 500KB) | Integrity: Preserved",
                    foundation::infra::static_logs::messages::LABEL_PHASE_5
                )
            );
        }
        copy_original_on_skip(input, options)?;
        mark_as_processed(input);
        return Ok(TaskResult::skipped_custom(
            input,
            input_size,
            "Skipped: Small PNG (< 500KB)",
            "small_file",
        ));
    }
    let output = get_output_path(input, EXT_JXL, options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force() {
        return Ok(TaskResult::skipped_exists(input, &output)?);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    let color_info = color_context.and_then(ConversionColorContext::conversion_color_info);
    let color_role = color_context.and_then(ConversionColorContext::role);

    let (actual_input, _temp_file_guard) = prepare_input_for_cjxl(input, options, color_info)?;

    // Extract ICC Profile from original input for preservation
    let icc_temp = foundation::jxl_utils::extract_icc_profile(input);
    let icc_path = icc_temp.as_ref().map(tempfile::NamedTempFile::path);

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        foundation::thread_manager::get_optimal_threads()
    };

    log_stat!(
        foundation::infra::static_logs::messages::LABEL_PHASE_1,
        format!(
            "JXL Encoding Cycle: {} | Threads: {} | ICC: {} | Precision/HDR: {}",
            input.display(),
            max_threads,
            if icc_path.is_some() {
                "Preserved"
            } else {
                "None"
            },
            match color_role {
                Some(ConversionColorRole::TrueHdrMetadata) => "HDR",
                Some(ConversionColorRole::PrecisionOrWideGamutHint) => "Precision/WCG",
                None => "None",
            }
        )
    );

    let actual_dist = foundation::constants::jxl_distance_for_mode(distance, options.ultimate());
    let is_extreme_explore =
        size_ge_1mib(input_size) && options.ultimate() && options.explore() && !options.archive();
    let actual_eff = jxl_encode_effort_for_size(
        options.archive(),
        options.ultimate(),
        options.explore(),
        input_size,
    );
    let effort_plan = jxl_effort_search_plan(
        options.archive(),
        options.ultimate(),
        options.explore(),
        input_size,
    );

    // Add conversion color metadata via CICP if available.
    if let Some(info) = color_info
        && let Some(cicp) = foundation::color_info_to_cicp(info)
        && options.verbose()
    {
        foundation::log_stat!(
            foundation::infra::static_logs::messages::LABEL_COLOR_SPACE,
            format!(
                "Color metadata synthesis: Injecting CICP {cicp} ({})",
                match color_role {
                    Some(ConversionColorRole::TrueHdrMetadata) => "true HDR metadata",
                    Some(ConversionColorRole::PrecisionOrWideGamutHint) => {
                        "wide-gamut/precision hint"
                    }
                    None => "unspecified context",
                }
            )
        );
    }

    if options.verbose() {
        log_detail!(&format!(
            "{} Orchestrating cjxl (D:{:.2}, E-plan:[{}], T:{}) -> {} to {}",
            foundation::infra::static_logs::messages::LABEL_PHASE_1,
            actual_dist,
            format_jxl_effort_plan(&effort_plan),
            max_threads,
            actual_input.display(),
            temp_output.display()
        ));
    }

    let result = run_direct_jxl_encode_effort_search(
        &actual_input,
        &temp_output,
        actual_dist,
        max_threads,
        options.apple_compat(),
        icc_path,
        color_info,
        &effort_plan,
    );

    let result = match result {
        Ok(output_cmd) if !output_cmd.status.success() => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr).into_owned();
            let result = Ok(output_cmd);
            if foundation::jxl_utils::is_icc_rounding_error(&stderr) {
                perform_icc_d50_retry(
                    input,
                    &actual_input,
                    &temp_output,
                    actual_dist,
                    max_threads,
                    options,
                    color_info,
                    &effort_plan,
                    result,
                )
            } else if stderr.contains("Getting pixel data failed")
                || stderr.contains("Failed to decode")
                || stderr.contains("Decoding failed")
                || stderr.contains("pixel data")
                || stderr.contains("Error while decoding")
                || stderr.contains("libpng warning")
                || foundation::jxl_utils::is_grayscale_icc_cjxl_error(&stderr)
            {
                match try_pipeline_recovery_fallbacks(
                    input,
                    &actual_input,
                    &temp_output,
                    &output,
                    options,
                    actual_dist,
                    max_threads,
                    actual_eff,
                    input_size,
                    color_info,
                    &stderr,
                    result,
                ) {
                    FallbackResult::Finalized(res) => return res,
                    FallbackResult::Exhausted(orig) => orig,
                }
            } else {
                result
            }
        }
        _ => result,
    };

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if let Err(e) = verify_jxl_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(e);
            }

            let (final_output_size, extra_info) = if is_extreme_explore
                && let Some(explore_result) = try_explore_ultimate_jxl_distance(
                    input,
                    &actual_input,
                    &temp_output,
                    input_size,
                    output_size,
                    max_threads,
                    options,
                    icc_path,
                    color_info,
                )? {
                (
                    explore_result.output_size,
                    Some(format!(
                        "(screened e7, finalized e10 d={})",
                        foundation::jxl_explorer::format_distance_for_log(
                            explore_result.accepted_distance
                        )
                    )),
                )
            } else {
                (output_size, None)
            };

            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                final_output_size,
                options,
                LABEL_JXL,
                extra_info.as_deref(),
            )
        }
        Ok(output_cmd) => {
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "cjxl failed: {stderr}"
            )))
        }
        Err(JxlDirectEncodeError::Launch(e)) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::tool_not_found("cjxl").with_operation(e.to_string()))
        }
        Err(JxlDirectEncodeError::Conversion(e)) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::ConversionError(e))
        }
    }
}

/// True when cjxl failed with "JPEG bitstream reconstruction data could not be created" / "`allow_jpeg_reconstruction`".
fn is_jpeg_reconstruction_cjxl_error(stderr: &str) -> bool {
    stderr.contains("allow_jpeg_reconstruction")
        || stderr.contains("bitstream reconstruction data could not be created")
        || stderr.contains("too much tail data")
}

fn cjxl_exit_summary(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_string(),
    }
}

fn cjxl_failure_summary(stage: &str, output: &foundation::process_runner::ProcessOutput) -> String {
    let stderr_tail = output
        .stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("<empty stderr>");
    format!(
        "{stage}: {} | stderr: {stderr_tail}",
        cjxl_exit_summary(output.status)
    )
}

fn truncate_jpeg_ladder_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.len() <= 512 {
        return trimmed.to_string();
    }
    let mut end = 512;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

#[derive(Debug)]
enum JpegJbrdLayerDiagnostic {
    Process {
        stage: String,
        exit_code: String,
        stderr: String,
    },
    Skipped {
        stage: String,
        reason: String,
    },
    Error {
        stage: String,
        reason: String,
    },
}

#[derive(Debug)]
struct JpegJbrdLadderDiagnostics {
    source: String,
    layers: Vec<JpegJbrdLayerDiagnostic>,
}

impl JpegJbrdLadderDiagnostics {
    fn new<S: Into<String>>(source: S) -> Self {
        Self {
            source: source.into(),
            layers: Vec::new(),
        }
    }

    fn record_process_output(
        &mut self,
        stage: &str,
        output: &foundation::process_runner::ProcessOutput,
    ) {
        self.layers.push(JpegJbrdLayerDiagnostic::Process {
            stage: stage.to_string(),
            exit_code: cjxl_exit_summary(output.status),
            stderr: truncate_jpeg_ladder_stderr(&output.stderr),
        });
    }

    #[cfg(test)]
    fn record_process_failure(&mut self, stage: &str, exit_code: i32, stderr: &str) {
        self.layers.push(JpegJbrdLayerDiagnostic::Process {
            stage: stage.to_string(),
            exit_code: format!("exit code {exit_code}"),
            stderr: truncate_jpeg_ladder_stderr(stderr),
        });
    }

    fn record_skipped(&mut self, stage: &str, reason: &str) {
        self.layers.push(JpegJbrdLayerDiagnostic::Skipped {
            stage: stage.to_string(),
            reason: reason.to_string(),
        });
    }

    fn record_error(&mut self, stage: &str, reason: impl std::fmt::Display) {
        self.layers.push(JpegJbrdLayerDiagnostic::Error {
            stage: stage.to_string(),
            reason: reason.to_string(),
        });
    }

    fn layer_report(&self) -> String {
        self.layers
            .iter()
            .map(|layer| match layer {
                JpegJbrdLayerDiagnostic::Process {
                    stage,
                    exit_code,
                    stderr,
                } => format!("{stage}: {exit_code} | stderr: {stderr}"),
                JpegJbrdLayerDiagnostic::Skipped { stage, reason } => {
                    format!("{stage}: skipped | reason: {reason}")
                }
                JpegJbrdLayerDiagnostic::Error { stage, reason } => {
                    format!("{stage}: failed before exit status | error: {reason}")
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn fail_closed_message(&self, pixel_fallback_allowed: bool) -> String {
        let fallback_note = if pixel_fallback_allowed {
            "explicit pixel re-encode fallback was enabled, but structural JBRD recovery still failed before fallback handoff"
        } else {
            "refusing implicit ImageMagick pixel re-encode fallback. Explicitly opt in with ALLOW_JPEG_PIXEL_REENCODE_FALLBACK if decoded-pixel delivery is intended"
        };
        format!(
            "cjxl JPEG lossless transcode failed after JBRD structural recovery ladder; source: {}; {fallback_note}; layers: {}",
            self.source,
            self.layer_report()
        )
    }
}

const CJXL_TIMEOUT_SECS_ENV: &str = "MFB_CJXL_TIMEOUT_SECS";

fn cjxl_timeout() -> anyhow::Result<Duration> {
    let raw = match std::env::var(CJXL_TIMEOUT_SECS_ENV) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(Duration::from_mins(5)),
        Err(err) => {
            anyhow::bail!("Failed to read {CJXL_TIMEOUT_SECS_ENV} for cjxl timeout: {err}");
        }
    };
    let value = raw.trim();
    if value.is_empty() {
        anyhow::bail!("{CJXL_TIMEOUT_SECS_ENV} must not be empty");
    }
    let seconds = value.parse::<u64>().map_err(|err| {
        anyhow::anyhow!("Failed to parse {CJXL_TIMEOUT_SECS_ENV}={value:?}: {err}")
    })?;
    if seconds == 0 {
        anyhow::bail!("{CJXL_TIMEOUT_SECS_ENV} must be greater than zero");
    }
    Ok(Duration::from_secs(seconds))
}

fn run_cjxl_jpeg_transcode_with_effort(
    input: &Path,
    temp_output: &Path,
    options: &ConvertOptions,
    max_threads: usize,
    allow_jpeg_reconstruction: Option<u8>,
    effort: u8,
) -> anyhow::Result<foundation::process_runner::ProcessOutput> {
    let mut builder = foundation::CjxlBuilder::new();
    builder
        .input(input)
        .output(temp_output)
        .lossless_jpeg(true)
        .allow_expert_options(options.allow_expert_options())
        .effort(effort)
        .threads(max_threads)
        .apple_compat(options.apple_compat());

    if let Some(v) = allow_jpeg_reconstruction {
        builder.allow_jpeg_reconstruction(v != 0);
    }

    let mut command = builder.build();
    foundation::process_runner::ManagedProcess::spawn(&mut command)?
        .wait_timeout(cjxl_timeout()?, "cjxl JPEG lossless transcode")
}

fn jpeg_effort_stage_label(base: &str, effort: u8) -> String {
    format!("{base} e{effort}")
}

pub const JPEG_LOSSLESS_TRANSCODE_UNAVAILABLE_SKIP_REASON: &str =
    "jpeg_lossless_transcode_unavailable";

#[derive(Clone, Copy)]
enum JpegLosslessTranscodePlanMode {
    Policy,
    AggressiveE11,
    StandardFallback,
}

fn jpeg_aggressive_lossless_plan(enabled: bool) -> Vec<JxlEffortPlan> {
    if enabled {
        vec![JxlEffortPlan::Single(
            foundation::constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT,
        )]
    } else {
        Vec::new()
    }
}

fn jpeg_standard_transcode_fallback_plan(source_size: u64) -> Vec<JxlEffortPlan> {
    if size_ge_1mib(source_size) {
        vec![
            JxlEffortPlan::Candidate(foundation::constants::JXL_DEFAULT_EFFORT),
            JxlEffortPlan::Candidate(foundation::constants::JXL_DEEP_EFFORT),
            JxlEffortPlan::Candidate(foundation::constants::JXL_ULTIMATE_EFFORT),
        ]
    } else {
        vec![JxlEffortPlan::Single(
            foundation::constants::JXL_DEFAULT_EFFORT,
        )]
    }
}

fn jpeg_lossless_transcode_plan(
    mode: JpegLosslessTranscodePlanMode,
    options: &ConvertOptions,
    source_size: u64,
) -> Vec<JxlEffortPlan> {
    match mode {
        JpegLosslessTranscodePlanMode::Policy => jpeg_effort_search_plan(
            JpegEffortModeFlags::empty()
                | if options.archive() {
                    JpegEffortModeFlags::ARCHIVE
                } else {
                    JpegEffortModeFlags::empty()
                }
                | if options.ultimate() {
                    JpegEffortModeFlags::ULTIMATE
                } else {
                    JpegEffortModeFlags::empty()
                }
                | if options.explore() {
                    JpegEffortModeFlags::EXPLORE
                } else {
                    JpegEffortModeFlags::empty()
                }
                | if options.allow_expert_options() {
                    JpegEffortModeFlags::ALLOW_EXPERT_OPTIONS
                } else {
                    JpegEffortModeFlags::empty()
                },
            source_size,
        ),
        JpegLosslessTranscodePlanMode::AggressiveE11 => {
            if options.archive() {
                foundation::jxl_effort_policy::archive_effort_search_plan(
                    JxlEffortSearchKind::JpegLosslessTranscode,
                )
            } else {
                jpeg_aggressive_lossless_plan(true)
            }
        }
        JpegLosslessTranscodePlanMode::StandardFallback => {
            if options.archive() {
                foundation::jxl_effort_policy::archive_effort_search_plan(
                    JxlEffortSearchKind::JpegLosslessTranscode,
                )
            } else {
                jpeg_standard_transcode_fallback_plan(source_size)
            }
        }
    }
}

const fn jpeg_aggressive_lossless_enabled(options: &ConvertOptions) -> bool {
    options.ultimate() || options.require_output_delivery()
}

fn run_cjxl_jpeg_transcode_effort_search(
    input: &Path,
    temp_output: &Path,
    options: &ConvertOptions,
    max_threads: usize,
    allow_jpeg_reconstruction: Option<u8>,
    plan: &[JxlEffortPlan],
) -> anyhow::Result<foundation::process_runner::ProcessOutput> {
    let mut successes: Vec<(
        JxlEffortCandidate,
        PathBuf,
        foundation::process_runner::ProcessOutput,
    )> = Vec::new();
    let mut failures: Vec<foundation::process_runner::ProcessOutput> = Vec::new();
    let mut failure_summaries: Vec<String> = Vec::new();

    for item in plan {
        let effort = match item {
            JxlEffortPlan::Single(effort) | JxlEffortPlan::Candidate(effort) => *effort,
        };
        let candidate_output = foundation::path_safety::isolated_temp_path_for_search(temp_output)?;
        let output = run_cjxl_jpeg_transcode_with_effort(
            input,
            &candidate_output,
            options,
            max_threads,
            allow_jpeg_reconstruction,
            effort,
        )?;

        if output.status.success() {
            if let Err(err) = verify_jxl_health(&candidate_output) {
                failure_summaries.push(format!(
                    "{}: output health verification failed: {err}",
                    jpeg_effort_stage_label("JPEG lossless effort candidate", effort)
                ));
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "jpeg_effort_candidate_health_failed",
                    &candidate_output,
                );
                continue;
            }
            let output_size = std::fs::metadata(&candidate_output)?.len();
            successes.push((
                JxlEffortCandidate {
                    effort,
                    output_size,
                },
                candidate_output,
                output,
            ));
        } else {
            failure_summaries.push(cjxl_failure_summary(
                &jpeg_effort_stage_label("JPEG lossless effort candidate", effort),
                &output,
            ));
            failures.push(output);
            foundation::media_conversion_gate::delivery_remove_file_or_audit(
                "jpeg_effort_candidate_failed",
                &candidate_output,
            );
        }
    }

    if successes.is_empty() {
        if let Some(mut failure) = failures.into_iter().next() {
            let joined = failure_summaries.join("; ");
            failure.stderr = format!(
                "JPEG lossless effort exploration failed for every candidate: {joined}\n{}",
                failure.stderr
            );
            return Ok(failure);
        }
        let joined = failure_summaries.join("; ");
        anyhow::bail!("JPEG lossless effort exploration produced no valid candidate: {joined}");
    }

    let candidate_stats: Vec<JxlEffortCandidate> = successes
        .iter()
        .map(|(candidate, _, _)| *candidate)
        .collect();
    let winner_idx = select_jxl_effort_winner(&candidate_stats).ok_or_else(|| {
        anyhow::anyhow!("JPEG lossless effort exploration had successes but no selectable winner")
    })?;
    let (winner, winner_path, winner_output) = successes.swap_remove(winner_idx);

    foundation::media_conversion_gate::delivery_remove_file_or_audit(
        "jpeg_effort_winner_prepare",
        temp_output,
    );
    std::fs::copy(&winner_path, temp_output).map_err(|err| {
        anyhow::anyhow!(
            "failed to copy JPEG effort winner e{} ({} bytes) from {} to {}: {err}",
            winner.effort,
            winner.output_size,
            winner_path.display(),
            temp_output.display()
        )
    })?;
    foundation::media_conversion_gate::delivery_remove_file_or_audit(
        "jpeg_effort_winner_cleanup",
        &winner_path,
    );

    for (_, path, _) in successes {
        foundation::media_conversion_gate::delivery_remove_file_or_audit(
            "jpeg_effort_nonwinner_cleanup",
            &path,
        );
    }

    log_detail!(&format!(
        "JPEG lossless effort exploration selected e{} ({} bytes)",
        winner.effort, winner.output_size
    ));

    Ok(winner_output)
}

fn run_cjxl_jpeg_transcode_with_plan_mode(
    input: &Path,
    temp_output: &Path,
    options: &ConvertOptions,
    source_size: u64,
    max_threads: usize,
    allow_jpeg_reconstruction: Option<u8>,
    mode: JpegLosslessTranscodePlanMode,
) -> anyhow::Result<foundation::process_runner::ProcessOutput> {
    let plan = jpeg_lossless_transcode_plan(mode, options, source_size);
    match plan.as_slice() {
        [JxlEffortPlan::Single(effort)] => run_cjxl_jpeg_transcode_with_effort(
            input,
            temp_output,
            options,
            max_threads,
            allow_jpeg_reconstruction,
            *effort,
        ),
        _ => run_cjxl_jpeg_transcode_effort_search(
            input,
            temp_output,
            options,
            max_threads,
            allow_jpeg_reconstruction,
            &plan,
        ),
    }
}

fn run_jpegtran_layer(
    input: &Path,
    output: &Path,
    copy_mode: &str,
    optimize: bool,
) -> anyhow::Result<foundation::process_runner::ProcessOutput> {
    let mut command = std::process::Command::new("jpegtran");
    command.arg("-copy").arg(copy_mode);
    if optimize {
        command.arg("-optimize");
    }
    command
        .arg("-outfile")
        .arg(foundation::safe_path_arg(output).as_ref())
        .arg(foundation::safe_path_arg(input).as_ref());
    foundation::process_runner::ManagedProcess::spawn(&mut command)?
        .wait_timeout(cjxl_timeout()?, "jpegtran JPEG structural rebuild")
}

fn run_exiftool_restore_all_metadata(
    source: &Path,
    target: &Path,
) -> anyhow::Result<foundation::process_runner::ProcessOutput> {
    let mut builder = foundation::ExiftoolBuilder::new();
    builder
        .overwrite_original()
        .preserve_date()
        .ignore_minor()
        .tags_from_file(source)
        .arg("-all:all")
        .input(target);
    let mut command = builder.build();
    foundation::process_runner::ManagedProcess::spawn(&mut command)?
        .wait_timeout(cjxl_timeout()?, "exiftool JPEG metadata restoration")
}

#[allow(clippy::too_many_arguments)]
fn run_jbrd_retry_from_temp(
    candidate: &Path,
    temp_output: &Path,
    output: &Path,
    input: &Path,
    input_size: u64,
    options: &ConvertOptions,
    max_threads: usize,
    label: &str,
    proof: JpegTranscodeProof,
    diagnostics: &mut JpegJbrdLadderDiagnostics,
    mode: JpegLosslessTranscodePlanMode,
) -> Option<Result<TaskResult>> {
    foundation::media_conversion_gate::delivery_remove_file_or_audit(
        "jpeg_jbrd_retry_temp_output",
        temp_output,
    );
    match run_cjxl_jpeg_transcode_with_plan_mode(
        candidate,
        temp_output,
        options,
        input_size,
        max_threads,
        None,
        mode,
    ) {
        Ok(out) if out.status.success() => Some(commit_jpeg_to_jxl_success(
            input,
            temp_output,
            output,
            input_size,
            options,
            label,
            proof,
        )),
        Ok(out) => {
            diagnostics.record_process_output(label, &out);
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_jbrd_retry_failed",
                input,
                diagnostics.layer_report(),
            );
            None
        }
        Err(err) => {
            diagnostics.record_error(label, &err);
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_jbrd_retry_error",
                input,
                format!("{label}: {err}"),
            );
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JpegTranscodeProof {
    BitstreamReconstruction,
    PixelEquivalence,
}

fn jpeg_transcode_proof_for_success(
    original_input: &Path,
    encoded_input: &Path,
    allow_jpeg_reconstruction: Option<u8>,
) -> JpegTranscodeProof {
    if allow_jpeg_reconstruction == Some(0) || original_input != encoded_input {
        JpegTranscodeProof::PixelEquivalence
    } else {
        JpegTranscodeProof::BitstreamReconstruction
    }
}

const fn jpeg_pixel_reencode_fallback_allowed(options: &ConvertOptions) -> bool {
    options.allow_jpeg_pixel_reencode_fallback()
}

fn jpeg_transcode_threads(options: &ConvertOptions) -> usize {
    if options.child_threads > 0 {
        options.child_threads
    } else {
        foundation::thread_manager::get_optimal_threads()
    }
}

fn run_standard_jpeg_lossless_fallback(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    options: &ConvertOptions,
    max_threads: usize,
) -> Option<Result<TaskResult>> {
    foundation::media_conversion_gate::delivery_remove_file_or_audit(
        "jpeg_standard_fallback_temp_output",
        temp_output,
    );
    let primary = run_cjxl_jpeg_transcode_with_plan_mode(
        input,
        temp_output,
        options,
        input_size,
        max_threads,
        None,
        JpegLosslessTranscodePlanMode::StandardFallback,
    );
    match &primary {
        Ok(out) if out.status.success() => {
            return Some(commit_jpeg_to_jxl_success(
                input,
                temp_output,
                output,
                input_size,
                options,
                "JPEG lossless standard fallback",
                JpegTranscodeProof::BitstreamReconstruction,
            ));
        }
        Ok(out) => {
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_standard_fallback_primary_failed",
                input,
                cjxl_failure_summary("standard fallback JPEG lossless", out),
            );
        }
        Err(err) => {
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_standard_fallback_primary_error",
                input,
                format!("standard fallback JPEG lossless process failed: {err}"),
            );
            return Some(Err(ImgQualityError::ConversionError(format!(
                "cjxl standard JPEG lossless fallback process failed: {err}"
            ))));
        }
    }
    cleanup_temp_output(temp_output, input);

    let primary_stderr = match &primary {
        Ok(out) => out.stderr.as_str(),
        Err(_) => "",
    };
    if !is_jpeg_reconstruction_cjxl_error(primary_stderr) {
        return None;
    }

    let (source_to_use, _guard): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
        match foundation::jxl_utils::strip_jpeg_tail_to_temp(input) {
            Ok(Some((cleaned, guard))) => (cleaned, Some(guard)),
            Ok(None) => (input.to_path_buf(), None),
            Err(err) => {
                foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                    "jpeg_standard_fallback_tail_strip_error",
                    input,
                    format!("standard fallback tail normalization failed: {err}"),
                );
                (input.to_path_buf(), None)
            }
        };

    let retry_original = run_cjxl_jpeg_transcode_with_plan_mode(
        &source_to_use,
        temp_output,
        options,
        input_size,
        max_threads,
        None,
        JpegLosslessTranscodePlanMode::StandardFallback,
    );
    match &retry_original {
        Ok(out) if out.status.success() => {
            let label = if source_to_use == input {
                "JPEG lossless standard fallback"
            } else {
                "JPEG lossless standard fallback (sanitized tail)"
            };
            return Some(commit_jpeg_to_jxl_success(
                input,
                temp_output,
                output,
                input_size,
                options,
                label,
                jpeg_transcode_proof_for_success(input, &source_to_use, None),
            ));
        }
        Ok(out) => {
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_standard_fallback_tail_retry_failed",
                input,
                cjxl_failure_summary("standard fallback tail-normalized retry", out),
            );
        }
        Err(err) => {
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_standard_fallback_tail_retry_error",
                input,
                format!("standard fallback tail-normalized retry process failed: {err}"),
            );
            return Some(Err(ImgQualityError::ConversionError(format!(
                "cjxl standard JPEG lossless fallback tail retry process failed: {err}"
            ))));
        }
    }
    cleanup_temp_output(temp_output, input);

    let retry_no_recon = run_cjxl_jpeg_transcode_with_plan_mode(
        &source_to_use,
        temp_output,
        options,
        input_size,
        max_threads,
        Some(0),
        JpegLosslessTranscodePlanMode::StandardFallback,
    );
    match &retry_no_recon {
        Ok(out) if out.status.success() => Some(commit_jpeg_to_jxl_success(
            input,
            temp_output,
            output,
            input_size,
            options,
            "JPEG lossless standard fallback (--allow_jpeg_reconstruction 0)",
            jpeg_transcode_proof_for_success(input, &source_to_use, Some(0)),
        )),
        Ok(out) => {
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_standard_fallback_no_recon_failed",
                input,
                cjxl_failure_summary("standard fallback no-JBRD retry", out),
            );
            cleanup_temp_output(temp_output, input);
            None
        }
        Err(err) => {
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_standard_fallback_no_recon_error",
                input,
                format!("standard fallback no-JBRD retry process failed: {err}"),
            );
            cleanup_temp_output(temp_output, input);
            Some(Err(ImgQualityError::ConversionError(format!(
                "cjxl standard JPEG lossless fallback no-JBRD retry process failed: {err}"
            ))))
        }
    }
}

fn handle_irreversible_jpeg_transcode_failure(
    input: &Path,
    input_size: u64,
    options: &ConvertOptions,
    color_context: Option<&ConversionColorContext>,
    failure: &str,
) -> Result<TaskResult> {
    if options.require_output_delivery() {
        foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
            "jpeg_lossless_fast_img_skip",
            input,
            failure,
        );
        return Ok(TaskResult::skipped_custom(
            input,
            input_size,
            "Skipped: JPEG cannot be reversibly transcoded; source remains unmodified",
            JPEG_LOSSLESS_TRANSCODE_UNAVAILABLE_SKIP_REASON,
        ));
    }

    foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
        "jpeg_lossless_direct_encode_fallback",
        input,
        failure,
    );
    let mut direct_options = options.clone();
    direct_options.flags.remove(
        ConvertFlags::REQUIRE_JPEG_RECONSTRUCTION
            | ConvertFlags::REQUIRE_OUTPUT_DELIVERY
            | ConvertFlags::ALLOW_JPEG_PIXEL_REENCODE_FALLBACK,
    );
    convert_to_jxl(
        input,
        &direct_options,
        foundation::constants::JXL_ULTIMATE_DISTANCE,
        color_context,
    )
    .map_err(|err| {
        ImgQualityError::ConversionError(format!(
            "{failure}; irreversible direct JXL encode fallback failed: {err}"
        ))
    })
}

fn commit_jpeg_to_jxl_success(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    options: &ConvertOptions,
    label: &str,
    proof: JpegTranscodeProof,
) -> Result<TaskResult> {
    if let Err(e) = verify_jxl_health(temp_output) {
        cleanup_temp_output(temp_output, input);
        return Err(e);
    }
    match proof {
        JpegTranscodeProof::BitstreamReconstruction if options.require_jpeg_reconstruction() => {
            if let Err(e) = foundation::fast_img::verify_jxl_roundtrip_integrity(input, temp_output)
            {
                cleanup_temp_output(temp_output, input);
                return Err(ImgQualityError::ConversionError(format!(
                    "JPEG reconstruction proof failed before metadata commit: {e}"
                )));
            }
        }
        JpegTranscodeProof::BitstreamReconstruction => {}
        JpegTranscodeProof::PixelEquivalence => {
            if let Err(e) =
                foundation::fast_img::verify_jxl_pixel_equivalence_integrity(input, temp_output)
            {
                cleanup_temp_output(temp_output, input);
                return Err(ImgQualityError::ConversionError(format!(
                    "JPEG pixel-equivalence proof failed before metadata commit: {e}"
                )));
            }
        }
    }
    let output_size = fs::metadata(temp_output)
        .map_err(|e| {
            ImgQualityError::ConversionError(format!(
                "Failed to read metadata for JXL output {}: {e}",
                temp_output.display()
            ))
        })?
        .len();
    finalize_with_size_check(
        input,
        temp_output,
        output,
        input_size,
        output_size,
        options,
        label,
        None,
    )
}

/// Convert a JPEG image to JXL format using lossless JPEG transcoding.
///
/// # Arguments
/// * `input` - Path to the input JPEG file
/// * `options` - Conversion options
///
/// # Returns
/// * `Ok(TaskResult)` - Conversion result
/// * `Err(ImgQualityError)` - Conversion failed
///
/// # Behavior
/// - Uses `cjxl --lossless_jpeg=1` for bitstream reconstruction when possible
/// - On reconstruction failure: strips JPEG tail and retries with pixel-equivalence proof
/// - On non-Type-B failures: fails closed unless pixel re-encode fallback is explicitly enabled
/// - Verifies JXL health and checks size tolerance
///
/// # Fallback Chain
/// 1. Primary: cjxl with lossless JPEG mode
/// 2. Strip JPEG tail → retry
/// 3. Use --`allow_jpeg_reconstruction=0`
/// 4. Explicit opt-in only: `ImageMagick` pixel re-encode fallback
///
/// Transcode JPEG to JXL losslessly. Standard cases are byte-reconstructible;
/// Type-B reconstruction failures fall back to decoded pixel equivalence.
///
/// # Errors
/// Returns an error if transcoding fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
enum JbrdLadderResult {
    Recovered(crate::Result<TaskResult>),
    Exhausted(JpegJbrdLadderDiagnostics),
}

fn try_jbrd_reconstruction_ladder(
    input: &Path,
    temp_output: &Path,
    output: &Path,
    input_size: u64,
    options: &ConvertOptions,
    max_threads: usize,
    transcode_plan_mode: JpegLosslessTranscodePlanMode,
    output_cmd: &foundation::process_runner::ProcessOutput,
) -> JbrdLadderResult {
    let mut ladder = JpegJbrdLadderDiagnostics::new(input.display().to_string());
    ladder.record_process_output("primary JPEG lossless", output_cmd);

    let jpegtran_available = foundation::common_utils::is_command_available("jpegtran");
    let exiftool_available = foundation::ExiftoolBuilder::check_available();

    if jpegtran_available {
        match foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "jpeg_jbrd_layer1_optimize",
            Some("mfb-jbrd-opt-"),
            Some(".jpg"),
        ) {
            Ok(tmp1) => match run_jpegtran_layer(input, tmp1.path(), "all", true) {
                Ok(jpegtran_out) if jpegtran_out.status.success() => {
                    if let Some(result) = run_jbrd_retry_from_temp(
                        tmp1.path(),
                        temp_output,
                        output,
                        input,
                        input_size,
                        options,
                        max_threads,
                        "jpegtran optimize retry",
                        JpegTranscodeProof::PixelEquivalence,
                        &mut ladder,
                        transcode_plan_mode,
                    ) {
                        return JbrdLadderResult::Recovered(result);
                    }
                }
                Ok(jpegtran_out) => {
                    ladder.record_process_output("jpegtran optimize", &jpegtran_out);
                }
                Err(err) => {
                    ladder.record_error("jpegtran optimize", err);
                }
            },
            Err(err) => {
                ladder.record_error("jpegtran optimize temp", err);
            }
        }
    } else {
        ladder.record_skipped("jpegtran optimize retry", "jpegtran unavailable");
    }

    if jpegtran_available && exiftool_available {
        match (
            foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                "jpeg_jbrd_layer2_struct",
                Some("mfb-jbrd-struct-"),
                Some(".jpg"),
            ),
            foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                "jpeg_jbrd_layer2_meta",
                Some("mfb-jbrd-meta-"),
                Some(".jpg"),
            ),
        ) {
            (Ok(tmp_struct), Ok(tmp2)) => {
                match run_jpegtran_layer(input, tmp_struct.path(), "none", false) {
                    Ok(struct_out) if struct_out.status.success() => {
                        match std::fs::copy(tmp_struct.path(), tmp2.path()) {
                            Ok(_) => match run_exiftool_restore_all_metadata(input, tmp2.path()) {
                                Ok(exiftool_out) if exiftool_out.status.success() => {
                                    if let Some(result) = run_jbrd_retry_from_temp(
                                        tmp2.path(),
                                        temp_output,
                                        output,
                                        input,
                                        input_size,
                                        options,
                                        max_threads,
                                        "metadata-safe structural rebuild retry",
                                        JpegTranscodeProof::PixelEquivalence,
                                        &mut ladder,
                                        transcode_plan_mode,
                                    ) {
                                        return JbrdLadderResult::Recovered(result);
                                    }
                                }
                                Ok(exiftool_out) => {
                                    ladder.record_process_output(
                                        "metadata-safe structural rebuild exiftool",
                                        &exiftool_out,
                                    );
                                }
                                Err(err) => {
                                    ladder.record_error(
                                        "metadata-safe structural rebuild exiftool",
                                        err,
                                    );
                                }
                            },
                            Err(err) => {
                                ladder.record_error("metadata-safe structural rebuild copy", err);
                            }
                        }
                    }
                    Ok(struct_out) => {
                        ladder.record_process_output(
                            "metadata-safe structural rebuild jpegtran",
                            &struct_out,
                        );
                    }
                    Err(err) => {
                        ladder.record_error("metadata-safe structural rebuild jpegtran", err);
                    }
                }
            }
            (Err(err), _) => {
                ladder.record_error("metadata-safe structural rebuild temp", err);
            }
            (_, Err(err)) => {
                ladder.record_error("metadata-safe structural rebuild metadata temp", err);
            }
        }
    } else if !jpegtran_available && !exiftool_available {
        ladder.record_skipped(
            "metadata-safe structural rebuild",
            "jpegtran unavailable; exiftool unavailable",
        );
    } else if !jpegtran_available {
        ladder.record_skipped("metadata-safe structural rebuild", "jpegtran unavailable");
    } else {
        ladder.record_skipped("metadata-safe structural rebuild", "exiftool unavailable");
    }

    JbrdLadderResult::Exhausted(ladder)
}

pub fn convert_jpeg_to_jxl(
    input: &Path,
    options: &ConvertOptions,
    color_context: Option<&ConversionColorContext>,
) -> Result<TaskResult> {
    // Validate input file
    if let Err(e) = foundation::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(TaskResult::skipped_duplicate(input)?);
    }

    let input_size = fs::metadata(input)?.len();

    // Missing EOI means reversible JPEG bitstream reconstruction is impossible.
    // Route through the irreversible-media policy so fast delivery can record a
    // skip and standard img mode can attempt the documented direct-encode path.
    if !is_jpeg_complete(&fs::read(input)?) {
        return handle_irreversible_jpeg_transcode_failure(
            input,
            input_size,
            options,
            color_context,
            "JPEG lossless transcode preflight rejected source before cjxl: JPEG is truncated or missing EOI",
        );
    }

    // UltraHDR JPEGs must follow the HDR synthesis path, not the legacy lossless transcode path.
    if foundation::image_jpeg_analysis::is_ultra_hdr_jpeg_file(input)? {
        log_detail!(&format!(
            "{} 🌈 UltraHDR detected: {} - delegating to native HDR synthesis pipeline",
            foundation::infra::static_logs::messages::LABEL_PHASE_5,
            foundation::media_conversion_gate::path_file_name_for_log(input)
        ));
        return convert_ultrahdr_jpeg_to_jxl(input, options);
    }

    // Standard JPEG conversion (non-UltraHDR)
    log_detail!(&format!(
        "{} UltraHDR not detected for {}: performing standard JPEG transcoding",
        foundation::infra::static_logs::messages::LABEL_PHASE_5,
        foundation::media_conversion_gate::path_file_name_for_log(input)
    ));

    let output = get_output_path(input, EXT_JXL, options)?;

    if output.exists() && !options.force() {
        return Ok(TaskResult::skipped_exists(input, &output)?);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    let max_threads = jpeg_transcode_threads(options);
    let aggressive_e11 = jpeg_aggressive_lossless_enabled(options);
    let transcode_plan_mode = if aggressive_e11 {
        JpegLosslessTranscodePlanMode::AggressiveE11
    } else {
        JpegLosslessTranscodePlanMode::Policy
    };

    let result = run_cjxl_jpeg_transcode_with_plan_mode(
        input,
        &temp_output,
        options,
        input_size,
        max_threads,
        None,
        transcode_plan_mode,
    );

    let output_cmd = match result {
        Ok(out) => out,
        Err(e) if aggressive_e11 => {
            let failure =
                format!("cjxl aggressive e11 JPEG lossless transcode process failed: {e}");
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jpeg_aggressive_e11_process_error",
                input,
                &failure,
            );
            cleanup_temp_output(&temp_output, input);
            if let Some(fallback) = run_standard_jpeg_lossless_fallback(
                input,
                &temp_output,
                &output,
                input_size,
                options,
                max_threads,
            ) {
                return fallback;
            }
            return handle_irreversible_jpeg_transcode_failure(
                input,
                input_size,
                options,
                color_context,
                &failure,
            );
        }
        Err(e) => {
            return Err(ImgQualityError::ConversionError(format!(
                "cjxl JPEG lossless transcode process failed: {e}"
            )));
        }
    };

    if output_cmd.status.success() {
        return commit_jpeg_to_jxl_success(
            input,
            &temp_output,
            &output,
            input_size,
            options,
            "JPEG lossless",
            JpegTranscodeProof::BitstreamReconstruction,
        );
    }

    let stderr = output_cmd.stderr.clone();
    let primary_failure = cjxl_failure_summary("primary JPEG lossless", &output_cmd);
    cleanup_temp_output(&temp_output, input);

    if is_jpeg_reconstruction_cjxl_error(&stderr) {
        // 1) Fix: strip trailing data after JPEG EOI so cjxl can use bitstream reconstruction
        let (source_to_use, _guard): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
            match foundation::jxl_utils::strip_jpeg_tail_to_temp(input) {
                Ok(Some((cleaned, guard))) => {
                    if options.verbose() {
                        log_detail!(
                            foundation::infra::static_logs::messages::JXL_STRIPPED_TAIL_RETRY
                        );
                    }
                    (cleaned, Some(guard))
                }
                _ => (input.to_path_buf(), None),
            };

        // 2) Retry with original cjxl flags (no --allow_jpeg_reconstruction 0) on fixed or original
        let retry_original = run_cjxl_jpeg_transcode_with_plan_mode(
            &source_to_use,
            &temp_output,
            options,
            input_size,
            max_threads,
            None,
            transcode_plan_mode,
        );
        match &retry_original {
            Ok(out) if out.status.success() => {
                let label = if source_to_use == input {
                    "JPEG lossless"
                } else {
                    "JPEG lossless (sanitized tail)"
                };
                return commit_jpeg_to_jxl_success(
                    input,
                    &temp_output,
                    &output,
                    input_size,
                    options,
                    label,
                    jpeg_transcode_proof_for_success(input, &source_to_use, None),
                );
            }
            _ => {}
        }
        cleanup_temp_output(&temp_output, input);

        // 3) Fallback: --allow_jpeg_reconstruction 0 (no bitstream reconstruction, often larger)
        let retry_no_recon = run_cjxl_jpeg_transcode_with_plan_mode(
            &source_to_use,
            &temp_output,
            options,
            input_size,
            max_threads,
            Some(0),
            transcode_plan_mode,
        );
        match &retry_no_recon {
            Ok(out) if out.status.success() => {
                return commit_jpeg_to_jxl_success(
                    input,
                    &temp_output,
                    &output,
                    input_size,
                    options,
                    "JPEG lossless (--allow_jpeg_reconstruction 0)",
                    jpeg_transcode_proof_for_success(input, &source_to_use, Some(0)),
                );
            }
            _ => {}
        }
        cleanup_temp_output(&temp_output, input);
        let retry_original_failure = match &retry_original {
            Ok(out) => cjxl_failure_summary("tail-normalized JPEG lossless retry", out),
            Err(err) => format!("tail-normalized JPEG lossless retry process failed: {err}"),
        };
        let retry_no_recon_failure = match &retry_no_recon {
            Ok(out) => cjxl_failure_summary("no-JBRD JPEG lossless retry", out),
            Err(err) => format!("no-JBRD JPEG lossless retry process failed: {err}"),
        };
        let failure = format!(
            "cjxl JPEG transcode failed after guarded cascade: {primary_failure}; {retry_original_failure}; {retry_no_recon_failure}"
        );
        if aggressive_e11
            && let Some(fallback) = run_standard_jpeg_lossless_fallback(
                input,
                &temp_output,
                &output,
                input_size,
                options,
                max_threads,
            )
        {
            return fallback;
        }
        return handle_irreversible_jpeg_transcode_failure(
            input,
            input_size,
            options,
            color_context,
            &failure,
        );
    }

    let ladder = match try_jbrd_reconstruction_ladder(
        input,
        &temp_output,
        &output,
        input_size,
        options,
        max_threads,
        transcode_plan_mode,
        &output_cmd,
    ) {
        JbrdLadderResult::Recovered(res) => return res,
        JbrdLadderResult::Exhausted(ladder) => ladder,
    };

    if aggressive_e11
        && let Some(fallback) = run_standard_jpeg_lossless_fallback(
            input,
            &temp_output,
            &output,
            input_size,
            options,
            max_threads,
        )
    {
        return fallback;
    }

    if !jpeg_pixel_reencode_fallback_allowed(options) {
        return handle_irreversible_jpeg_transcode_failure(
            input,
            input_size,
            options,
            color_context,
            &ladder.fail_closed_message(false),
        );
    }

    if stderr.contains("Error while decoding")
        || stderr.contains("Corrupt JPEG")
        || stderr.contains("Premature end")
    {
        // For truncated JPEGs, the ImageMagick fallback often "repairs" them but results in
        // large JXL files that we eventually discard. We skip fallback if it's incomplete.
        if !is_jpeg_complete(&std::fs::read(input)?) {
            log_detail!(foundation::infra::static_logs::messages::MSG_CORRUPTION_SKIP);
            return Err(ImgQualityError::ConversionError(format!(
                "JPEG is truncated or missing EOI, and cjxl bitstream reconstruction failed: {stderr}"
            )));
        }

        foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
            "jpeg_pixel_reencode_fallback_triggered",
            input,
            "JBRD reconstruction ladder exhausted; falling back to ImageMagick pixel re-encode after corruption decode failure",
        );
        match foundation::jxl_utils::try_imagemagick_fallback(
            input,
            &temp_output,
            0.0,
            max_threads,
            options.apple_compat(),
            options.ultimate(),
        ) {
            Ok(()) => commit_jpeg_to_jxl_success(
                input,
                &temp_output,
                &output,
                input_size,
                options,
                "JPEG (Sanitized) -> JXL",
                JpegTranscodeProof::PixelEquivalence,
            ),
            Err(e) => Err(ImgQualityError::ConversionError(format!(
                "{}; ImageMagick pixel re-encode fallback failed after JPEG corruption: {e}",
                ladder.fail_closed_message(true)
            ))),
        }
    } else {
        log_detail!(foundation::infra::static_logs::messages::LOSSLESS_FALLBACK_MAGICK);
        foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
            "jpeg_pixel_reencode_fallback_triggered",
            input,
            "JBRD reconstruction ladder exhausted; falling back to ImageMagick pixel re-encode",
        );
        match foundation::jxl_utils::try_imagemagick_fallback(
            input,
            &temp_output,
            0.0,
            max_threads,
            options.apple_compat(),
            options.ultimate(),
        ) {
            Ok(()) => commit_jpeg_to_jxl_success(
                input,
                &temp_output,
                &output,
                input_size,
                options,
                "JPEG -> JXL (ImageMagick fallback)",
                JpegTranscodeProof::PixelEquivalence,
            ),
            Err(e) => Err(ImgQualityError::ConversionError(format!(
                "{}; ImageMagick pixel re-encode fallback failed: {e}",
                ladder.fail_closed_message(true)
            ))),
        }
    }
}

/// Convert an image to AVIF format with specified quality.
///
/// # Arguments
/// * `input` - Path to the input image file
/// * `quality` - AVIF quality (0-100, None = 85)
/// * `options` - Conversion options
///
/// # Returns
/// * `Ok(TaskResult)` - Conversion result
/// * `Err(ImgQualityError)` - Conversion failed
///
/// # Behavior
/// - Uses avifenc with speed 4 and all threads
/// - Verifies AVIF health after encoding
/// - Checks size tolerance and compress mode
///
/// Convert to AVIF using specific quality.
///
/// # Errors
/// Returns an error if avifenc execution fails.
pub fn convert_to_avif(
    input: &Path,
    quality: Option<u8>,
    options: &ConvertOptions,
) -> Result<TaskResult> {
    // Validate input file
    if let Err(e) = foundation::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(TaskResult::skipped_duplicate(input)?);
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, EXT_AVIF, options)?;

    if output.exists() && !options.force() {
        return Ok(TaskResult::skipped_exists(input, &output)?);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
    let q = foundation::media_conversion_gate::avif_quality_or_fallback(quality);

    let mut builder = foundation::AvifencBuilder::new();
    builder
        .speed(4)
        .jobs("all")
        .quality(q, q)
        .input(input)
        .output(&temp_output);

    let result = builder.build().output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();
            if let Err(e) = foundation::quality_verifier_enhanced::verify_avif_health(&temp_output)
            {
                cleanup_temp_output(&temp_output, input);
                return Err(ImgQualityError::ConversionError(format!(
                    "AVIF health check failed: {e}"
                )));
            }
            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                output_size,
                options,
                LABEL_AVIF,
                None,
            )
        }
        Ok(output_cmd) => {
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "avifenc failed: {stderr}"
            )))
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::tool_not_found("avifenc").with_operation(e.to_string()))
        }
    }
}

/// Convert to AVIF losslessly.
///
/// # Errors
/// Returns an error if encoding fails.
pub fn convert_to_avif_lossless(input: &Path, options: &ConvertOptions) -> Result<TaskResult> {
    // Validate input file
    if let Err(e) = foundation::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if options.verbose() {
        log_detail!(foundation::infra::static_logs::messages::AVIF_MATHEMATICAL_LOSSLESS_WARNING);
    }

    if !options.force() && is_already_processed(input) {
        return Ok(TaskResult::skipped_duplicate(input)?);
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, EXT_AVIF, options)?;

    if output.exists() && !options.force() {
        return Ok(TaskResult::skipped_exists(input, &output)?);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    let mut builder = foundation::AvifencBuilder::new();
    builder
        .lossless(true)
        .speed(4)
        .jobs("all")
        .input(input)
        .output(&temp_output);

    let result = builder.build().output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();
            if let Err(e) = foundation::quality_verifier_enhanced::verify_avif_health(&temp_output)
            {
                cleanup_temp_output(&temp_output, input);
                return Err(ImgQualityError::ConversionError(format!(
                    "Lossless AVIF health check failed: {e}"
                )));
            }
            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                output_size,
                options,
                "Lossless AVIF",
                None,
            )
        }
        Ok(output_cmd) => {
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "avifenc lossless failed: {stderr}"
            )))
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            Err(ImgQualityError::tool_not_found("avifenc").with_operation(e.to_string()))
        }
    }
}

/// Calculate matched JXL distance based on image complexity and file size.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_matched_distance_for_static(
    analysis: &crate::ImageAnalysis,
    file_size: u64,
) -> Result<f32> {
    let estimated_quality = analysis.jpeg_analysis.as_ref().map(|j| j.estimated_quality);

    let quality_analysis = foundation::from_image_analysis(
        &analysis.format,
        analysis.width,
        analysis.height,
        analysis.color_depth,
        analysis.has_alpha,
        file_size,
        None,
        None,
        estimated_quality,
    )
    .ok_or_else(|| {
        ImgQualityError::AnalysisError("Failed to calculate quality analysis metrics".to_string())
    })?;

    match foundation::calculate_jxl_distance(&quality_analysis) {
        Ok(result) => {
            foundation::log_quality_analysis(
                &quality_analysis,
                &result,
                foundation::EncoderType::Jxl,
            );
            Ok(result.distance)
        }
        Err(e) => Err(ImgQualityError::AnalysisError(format!(
            "Quality analysis failed: {e}"
        ))),
    }
}

/// Convert to JXL with matched distance.
///
/// # Errors
/// Returns an error if matching or encoding fails.
pub fn convert_to_jxl_matched(
    input: &Path,
    options: &ConvertOptions,
    analysis: &crate::ImageAnalysis,
) -> Result<TaskResult> {
    // Validate input file
    if let Err(e) = foundation::conversion::validate_input_file(input) {
        return Err(ImgQualityError::ConversionError(e));
    }

    if !options.force() && is_already_processed(input) {
        return Ok(TaskResult::skipped_duplicate(input)?);
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, EXT_JXL, options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force() {
        return Ok(TaskResult::skipped_exists(input, &output)?);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    let distance = calculate_matched_distance_for_static(analysis, input_size)?;
    log_stat!(
        foundation::infra::static_logs::messages::LABEL_JXL,
        format!("Forensic: Matched JXL distance finalized at {distance:.2}")
    );

    let max_threads = if options.child_threads > 0 {
        options.child_threads
    } else {
        foundation::thread_manager::get_optimal_threads()
    };

    let actual_dist = foundation::constants::jxl_distance_for_mode(distance, options.ultimate());
    let effort_plan =
        jxl_effort_search_plan(options.archive(), options.ultimate(), false, input_size);

    log_detail!(&format!(
        "{} Encoding quality-matched JXL: {} (distance={}, effort_plan=[{}], threads={})",
        foundation::infra::static_logs::messages::LABEL_JXL,
        input.display(),
        distance,
        format_jxl_effort_plan(&effort_plan),
        max_threads
    ));

    let result = run_direct_jxl_encode_effort_search(
        input,
        &temp_output,
        actual_dist,
        max_threads,
        options.apple_compat(),
        None,
        None,
        &effort_plan,
    );

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if let Err(e) = verify_jxl_health(&temp_output) {
                cleanup_temp_output(&temp_output, input);
                return Err(e);
            }

            let extra = format!("d={distance:.2}");
            finalize_with_size_check(
                input,
                &temp_output,
                &output,
                input_size,
                output_size,
                options,
                "Quality-matched JXL",
                Some(&extra),
            )
        }
        Ok(output_cmd) => {
            cleanup_temp_output(&temp_output, input);
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            Err(ImgQualityError::ConversionError(format!(
                "cjxl failed: {stderr}"
            )))
        }
        Err(JxlDirectEncodeError::Launch(e)) => {
            Err(ImgQualityError::tool_not_found("cjxl").with_operation(e.to_string()))
        }
        Err(JxlDirectEncodeError::Conversion(e)) => Err(ImgQualityError::ConversionError(e)),
    }
}

const fn jxl_screening_effort(archive: bool, ultimate: bool, explore: bool) -> u8 {
    if archive {
        return foundation::jxl_effort_policy::archive_effort(JxlEffortSearchKind::DirectEncode);
    }
    foundation::jxl_effort_policy::screening_effort(ultimate, explore)
}

fn try_imagemagick_fallback_with_effort(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
    apple_compat: bool,
    effort: u8,
) -> std::result::Result<(), std::io::Error> {
    foundation::jxl_utils::try_imagemagick_fallback_with_effort(
        input,
        output,
        distance,
        effort,
        max_threads,
        apple_compat,
    )
}

fn cjxl_std_failure_summary(stage: &str, output: &Output) -> String {
    let stderr_text = String::from_utf8_lossy(&output.stderr);
    let stderr_tail = stderr_text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("<empty stderr>");
    format!(
        "{stage}: {} | stderr: {}",
        cjxl_exit_summary(output.status),
        truncate_jpeg_ladder_stderr(stderr_tail)
    )
}

fn run_direct_jxl_encode_with_effort(
    input: &Path,
    output: &Path,
    distance: f32,
    effort: u8,
    max_threads: usize,
    apple_compat: bool,
    icc_path: Option<&Path>,
    color_info: Option<&ColorInfo>,
) -> std::io::Result<Output> {
    let mut builder = foundation::CjxlBuilder::new();
    builder
        .input(input)
        .output(output)
        .distance(distance)
        .effort(effort)
        .threads(max_threads)
        .apple_compat(apple_compat);

    if let Some(info) = color_info
        && let Some(cicp) = foundation::color_info_to_cicp(info)
    {
        builder.cicp(cicp);
    }

    if let Some(icc) = icc_path {
        builder.icc_profile(icc);
    }

    builder.build().output()
}

fn run_direct_jxl_encode_effort_search(
    input: &Path,
    temp_output: &Path,
    distance: f32,
    max_threads: usize,
    apple_compat: bool,
    icc_path: Option<&Path>,
    color_info: Option<&ColorInfo>,
    plan: &[JxlEffortPlan],
) -> std::result::Result<Output, JxlDirectEncodeError> {
    match plan {
        [JxlEffortPlan::Single(effort)] => {
            return run_direct_jxl_encode_with_effort(
                input,
                temp_output,
                distance,
                *effort,
                max_threads,
                apple_compat,
                icc_path,
                color_info,
            )
            .map_err(JxlDirectEncodeError::Launch);
        }
        [] => {
            return Err(JxlDirectEncodeError::Conversion(
                "JXL effort exploration plan was empty".to_string(),
            ));
        }
        _ => {}
    }

    let mut successes: Vec<(JxlEffortCandidate, PathBuf, Output)> = Vec::new();
    let mut failures: Vec<Output> = Vec::new();
    let mut failure_summaries: Vec<String> = Vec::new();

    for item in plan {
        let effort = jxl_effort_from_plan_item(*item);
        let candidate_output = foundation::path_safety::isolated_temp_path_for_search(temp_output)
            .map_err(|err| JxlDirectEncodeError::Conversion(err.to_string()))?;
        let output = match run_direct_jxl_encode_with_effort(
            input,
            &candidate_output,
            distance,
            effort,
            max_threads,
            apple_compat,
            icc_path,
            color_info,
        ) {
            Ok(output) => output,
            Err(err) => {
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "jxl_effort_candidate_launch_failed",
                    &candidate_output,
                );
                for (_, path, _) in successes {
                    foundation::media_conversion_gate::delivery_remove_file_or_audit(
                        "jxl_effort_prior_success_cleanup_after_launch_failure",
                        &path,
                    );
                }
                return Err(JxlDirectEncodeError::Launch(err));
            }
        };

        if output.status.success() {
            if let Err(err) = verify_jxl_health(&candidate_output) {
                failure_summaries.push(format!(
                    "{}: output health verification failed: {err}",
                    jpeg_effort_stage_label("JXL effort candidate", effort)
                ));
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "jxl_effort_candidate_health_failed",
                    &candidate_output,
                );
                continue;
            }
            let output_size = match fs::metadata(&candidate_output) {
                Ok(metadata) => metadata.len(),
                Err(err) => {
                    failure_summaries.push(format!(
                        "{}: output metadata read failed: {err}",
                        jpeg_effort_stage_label("JXL effort candidate", effort)
                    ));
                    foundation::media_conversion_gate::delivery_remove_file_or_audit(
                        "jxl_effort_candidate_metadata_failed",
                        &candidate_output,
                    );
                    continue;
                }
            };
            log_detail!(&format!(
                "JXL effort candidate e{effort} produced {output_size} bytes"
            ));
            successes.push((
                JxlEffortCandidate {
                    effort,
                    output_size,
                },
                candidate_output,
                output,
            ));
        } else {
            failure_summaries.push(cjxl_std_failure_summary(
                &jpeg_effort_stage_label("JXL effort candidate", effort),
                &output,
            ));
            failures.push(output);
            foundation::media_conversion_gate::delivery_remove_file_or_audit(
                "jxl_effort_candidate_failed",
                &candidate_output,
            );
        }
    }

    if successes.is_empty() {
        let joined = failure_summaries.join("; ");
        if let Some(mut failure) = failures.into_iter().next() {
            let original_stderr = String::from_utf8_lossy(&failure.stderr);
            failure.stderr = format!(
                "JXL effort exploration failed for every candidate: {joined}\n{original_stderr}"
            )
            .into_bytes();
            return Ok(failure);
        }
        return Err(JxlDirectEncodeError::Conversion(format!(
            "JXL effort exploration produced no valid candidate: {joined}"
        )));
    }

    let candidate_stats: Vec<JxlEffortCandidate> = successes
        .iter()
        .map(|(candidate, _, _)| *candidate)
        .collect();
    let winner_idx = select_jxl_effort_winner(&candidate_stats).ok_or_else(|| {
        JxlDirectEncodeError::Conversion(
            "JXL effort exploration had successes but no selectable winner".to_string(),
        )
    })?;
    let (winner, winner_path, winner_output) = successes.swap_remove(winner_idx);

    foundation::media_conversion_gate::delivery_remove_file_or_audit(
        "jxl_effort_winner_prepare",
        temp_output,
    );
    foundation::io_utils::robust_move(&winner_path, temp_output).map_err(|err| {
        JxlDirectEncodeError::Conversion(format!(
            "failed to move JXL effort winner e{} ({} bytes) from {} to {}: {err}",
            winner.effort,
            winner.output_size,
            winner_path.display(),
            temp_output.display()
        ))
    })?;

    for (_, path, _) in successes {
        foundation::media_conversion_gate::delivery_remove_file_or_audit(
            "jxl_effort_nonwinner_cleanup",
            &path,
        );
    }

    log_detail!(&format!(
        "JXL effort exploration selected e{} ({} bytes)",
        winner.effort, winner.output_size
    ));

    Ok(winner_output)
}

fn encode_direct_jxl_probe_with_effort(
    input: &Path,
    output: &Path,
    distance: f32,
    effort: u8,
    max_threads: usize,
    apple_compat: bool,
    icc_path: Option<&Path>,
    color_info: Option<&ColorInfo>,
) -> std::result::Result<(), String> {
    let output = run_direct_jxl_encode_with_effort(
        input,
        output,
        distance,
        effort,
        max_threads,
        apple_compat,
        icc_path,
        color_info,
    )
    .map_err(|e| format!("Failed to run cjxl probe: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err("cjxl probe failed without stderr output".to_string())
        } else {
            Err(stderr)
        }
    }
}

fn encode_jxl_probe_to_output(
    input: &Path,
    actual_input: &Path,
    output: &Path,
    distance: f32,
    effort: u8,
    max_threads: usize,
    apple_compat: bool,
    icc_path: Option<&Path>,
    color_info: Option<&ColorInfo>,
    stage_label: &str,
) -> std::result::Result<u64, String> {
    foundation::media_conversion_gate::delivery_remove_file_or_audit(
        "jxl probe output pre-clean",
        output,
    );

    let mut direct_encode = |candidate_distance| {
        encode_direct_jxl_probe_with_effort(
            actual_input,
            output,
            candidate_distance,
            effort,
            max_threads,
            apple_compat,
            icc_path,
            color_info,
        )?;
        verify_jxl_health(output)
            .map_err(|err| format!("Health check failed after direct cjxl probe: {err}"))?;
        fs::metadata(output)
            .map(|meta| meta.len())
            .map_err(|e| e.to_string())
    };

    let mut fallback_encode = |candidate_distance| {
        foundation::media_conversion_gate::delivery_remove_file_or_audit(
            "jxl probe fallback output pre-clean",
            output,
        );
        log_detail!(&format!(
            "{} {} d={}: cjxl engine failure - falling back to ImageMagick container at e{}",
            foundation::infra::static_logs::messages::LABEL_JXL,
            stage_label,
            foundation::jxl_explorer::format_distance_for_log(candidate_distance),
            effort
        ));
        try_imagemagick_fallback_with_effort(
            input,
            output,
            candidate_distance,
            max_threads,
            apple_compat,
            effort,
        )
        .map_err(|e| e.to_string())?;
        verify_jxl_health(output).map_err(|err| {
            format!("Health check failed after ImageMagick exploration probe: {err}")
        })?;
        fs::metadata(output)
            .map(|meta| meta.len())
            .map_err(|e| e.to_string())
    };

    run_jxl_exploration_probe_with(distance, &mut direct_encode, &mut fallback_encode)
}

fn run_jxl_exploration_probe_with<Direct, Fallback>(
    distance: f32,
    direct_encode: &mut Direct,
    fallback_encode: &mut Fallback,
) -> std::result::Result<u64, String>
where
    Direct: FnMut(f32) -> std::result::Result<u64, String>,
    Fallback: FnMut(f32) -> std::result::Result<u64, String>,
{
    match direct_encode(distance) {
Ok(size) => Ok(size),
Err(direct_err) => fallback_encode(distance).map_err(|fallback_err| {
format!(
"JXL exploration probe failed at d={}: direct cjxl: {direct_err}; ImageMagick fallback: {fallback_err}",
foundation::jxl_explorer::format_distance_for_log(distance)
)
}),
}
}

fn compare_jxl_finalists(
    input_size: u64,
    left_distance: f32,
    left_size: u64,
    right_distance: f32,
    right_size: u64,
) -> std::cmp::Ordering {
    let left_smaller_than_input = left_size < input_size;
    let right_smaller_than_input = right_size < input_size;

    match (left_smaller_than_input, right_smaller_than_input) {
        (true, false) => return std::cmp::Ordering::Less,
        (false, true) => return std::cmp::Ordering::Greater,
        _ => {}
    }

    left_distance
        .total_cmp(&right_distance)
        .then_with(|| left_size.cmp(&right_size))
}

fn describe_jxl_finalist_pass(
    finalist: &foundation::jxl_explorer::JxlScreenedCandidate,
    screening: &foundation::jxl_explorer::JxlScreeningResult,
    input_size: u64,
) -> String {
    let distance = foundation::jxl_explorer::format_distance_for_log(finalist.distance);
    let ratio_pct = output_size_ratio_pct(input_size, finalist.output_size);
    let ratio_label = format_output_size_ratio_pct(input_size, finalist.output_size);
    let origin = if finalist.ladder_phase {
        "screened"
    } else {
        "refined"
    };
    let role = if finalist.distance <= foundation::constants::JXL_EXPLORE_FLOOR + f32::EPSILON {
        "rechecking the required floor"
    } else if (finalist.distance - screening.best_distance).abs() < f32::EPSILON {
        "rechecking the screened leader"
    } else if ratio_pct.is_some_and(|pct| pct <= foundation::constants::JXL_BREAK_EVEN_RATIO_PCT) {
        "verifying a break-even candidate"
    } else {
        "sampling a shortlist branch"
    };

    format!("{role}: d={distance} from the {origin} pass ({ratio_label} of input at e7)")
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
fn try_explore_ultimate_jxl_distance(
    input: &Path,
    actual_input: &Path,
    temp_output: &Path,
    input_size: u64,
    initial_output_size: u64,
    max_threads: usize,
    options: &ConvertOptions,
    icc_path: Option<&Path>,
    color_info: Option<&ColorInfo>,
) -> Result<Option<foundation::jxl_explorer::JxlExploreResult>> {
    const MAX_CONTINUED_ITERATIONS: u32 = 20;
    foundation::infra::static_logs::log_stage(
        foundation::modern_ui::symbols::SEARCH,
        "JXL",
        "Ultimate Exploration: screening with e7, promoting a shortlist, finalizing with e10",
    );

    let screening_effort = jxl_screening_effort(false, true, true);
    let final_effort = foundation::constants::jxl_effort_for_mode(true);
    let screening = foundation::jxl_explorer::screen_jxl_candidates(
        input_size,
        initial_output_size,
        |distance| {
            let candidate_output =
                foundation::path_safety::isolated_temp_path_for_search(temp_output)
                    .map_err(|e| e.to_string())?;
            let result = encode_jxl_probe_to_output(
                input,
                actual_input,
                &candidate_output,
                distance,
                screening_effort,
                max_threads,
                options.apple_compat(),
                icc_path,
                color_info,
                "Screening probe",
            );
            foundation::media_conversion_gate::delivery_remove_file_or_audit(
                "jxl screening probe candidate cleanup",
                &candidate_output,
            );
            result
        },
    );

    let Some(screening) = (match screening {
        Ok(result) => result,
        Err(err) => {
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "jxl_explore_screening_abort",
                input,
                format!("ultimate JXL e7 screening aborted; reverting to baseline: {err}"),
            );
            return Ok(None);
        }
    }) else {
        return Ok(None);
    };

    for line in &screening.log {
        log_detail!(&format!(
            "{} {}",
            foundation::infra::static_logs::messages::LABEL_PHASE_1,
            line
        ));
    }

    let mut best_final: Option<(usize, u64, std::path::PathBuf)> = None;

    for (finalist_idx, finalist) in screening.finalists.iter().enumerate() {
        log_detail!(&format!(
            "{} Finalist {}/{}: e{} pass | {}",
            foundation::infra::static_logs::messages::LABEL_PHASE_2,
            finalist_idx + 1,
            screening.finalists.len(),
            final_effort,
            describe_jxl_finalist_pass(finalist, &screening, input_size)
        ));

        let candidate_output = foundation::path_safety::isolated_temp_path_for_search(temp_output)
            .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

        match encode_jxl_probe_to_output(
            input,
            actual_input,
            &candidate_output,
            finalist.distance,
            final_effort,
            max_threads,
            options.apple_compat(),
            icc_path,
            color_info,
            "Finalist encode",
        ) {
            Ok(size) => {
                log_detail!(&format!(
                    "{} ↳ e{} Result: {} efficiency",
                    foundation::infra::static_logs::messages::LABEL_PHASE_2,
                    final_effort,
                    format_output_size_ratio_pct(input_size, size),
                ));
                let replace_best = best_final.as_ref().is_none_or(|(best_idx, best_size, _)| {
                    let best_f = match screening.finalists.get(*best_idx) {
                        Some(v) => v,
                        None => unreachable!(
                            "CRITICAL: best_idx ({}) out of range in finalists (len={}) during JXL exploration",
                            best_idx,
                            screening.finalists.len()
                        ),
                    };
                    compare_jxl_finalists(
                        input_size,
                        finalist.distance,
                        size,
                        best_f.distance,
                        *best_size,
                    ) == std::cmp::Ordering::Less
                });

                if replace_best {
                    if let Some((_, _, old_path)) =
                        best_final.replace((finalist_idx, size, candidate_output.clone()))
                    {
                        foundation::media_conversion_gate::delivery_remove_file_or_audit(
                            "jxl finalist previous best cleanup",
                            &old_path,
                        );
                    }
                } else {
                    foundation::media_conversion_gate::delivery_remove_file_or_audit(
                        "jxl finalist nonwinner cleanup",
                        &candidate_output,
                    );
                }
            }
            Err(err) => {
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "jxl failed finalist cleanup",
                    &candidate_output,
                );
                foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                    "jxl_explore_finalist_failed",
                    input,
                    format!(
                        "finalist pass failed (e{final_effort}, d={}): {err}",
                        foundation::jxl_explorer::format_distance_for_log(finalist.distance)
                    ),
                );
            }
        }
    }

    let Some((best_idx, best_size, best_path)) = best_final else {
        log_detail!(foundation::infra::static_logs::messages::JXL_E10_FAILURE_KEEP_E7);
        return Ok(None);
    };

    if best_size >= input_size {
        foundation::media_conversion_gate::delivery_remove_file_or_audit(
            "jxl oversized best candidate cleanup",
            &best_path,
        );
        log_detail!(&format!(
            "All e10 finalists exceed input size (best={} of input); skipping JXL",
            format_output_size_ratio_pct(input_size, best_size),
        ));
        return Ok(None);
    }

    let best_candidate = screening.finalists.get(best_idx).ok_or_else(|| {
        ImgQualityError::ConversionError(
            "Failed to find best JXL candidate in finalists".to_string(),
        )
    })?;
    foundation::media_conversion_gate::delivery_remove_file_or_audit(
        "jxl best candidate commit pre-clean",
        temp_output,
    );
    foundation::io_utils::robust_move(&best_path, temp_output)
        .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

    // ── Phase: Continued Downward Exploration at e10 ────────────────────
    // e10 has greater compression potential than e7. After finalists settle,
    // continue stepping distance downward (higher quality) to see if e10
    // can produce even smaller files at lower distances.
    // STRICT RULE: stop immediately on first size increase.
    let mut accepted_distance = best_candidate.distance;
    let mut accepted_size = best_size;
    {
        let floor = foundation::constants::JXL_EXPLORE_FLOOR;
        // Adaptive step: use 1/10th of current distance, clamped to sane bounds
        let step = (accepted_distance / 10.0).clamp(
            foundation::constants::JXL_EXPLORE_BINARY_SEARCH_PRECISION,
            0.01,
        );
        let mut test_distance = accepted_distance - step;
        let mut continued_iterations = 0u32;

        if test_distance >= floor {
            log_detail!(&format!(
                "{} Continued e{} exploration: stepping down from d={} (step={})",
                foundation::modern_ui::symbols::pick("🔬", "[AUDIT]"),
                final_effort,
                foundation::jxl_explorer::format_distance_for_log(accepted_distance),
                foundation::jxl_explorer::format_distance_for_log(step),
            ));
        }

        while test_distance >= floor && continued_iterations < MAX_CONTINUED_ITERATIONS {
            // Canonicalize to avoid float drift
            let candidate_distance =
                foundation::jxl_explorer::clamp_explore_distance(test_distance);

            let candidate_output =
                foundation::path_safety::isolated_temp_path_for_search(temp_output)
                    .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

            match encode_jxl_probe_to_output(
                input,
                actual_input,
                &candidate_output,
                candidate_distance,
                final_effort,
                max_threads,
                options.apple_compat(),
                icc_path,
                color_info,
                "Continued exploration",
            ) {
                Ok(size) => {
                    continued_iterations += 1;
                    let pct_label = format_output_size_ratio_pct(input_size, size);

                    if size < accepted_size {
                        // Progress: smaller file at lower distance
                        log_detail!(&format!(
                            "{} d={} -> {} of input (gain)",
                            foundation::modern_ui::symbols::pick("✓", "[+]"),
                            foundation::jxl_explorer::format_distance_for_log(candidate_distance),
                            pct_label
                        ));
                        accepted_distance = candidate_distance;
                        accepted_size = size;
                        // Move the new best to temp_output
                        foundation::media_conversion_gate::delivery_remove_file_or_audit(
                            "jxl continued best candidate commit pre-clean",
                            temp_output,
                        );
                        foundation::io_utils::robust_move(&candidate_output, temp_output)
                            .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;
                        test_distance -= step;
                    } else {
                        // Size increased or stayed the same — stop immediately
                        log_detail!(&format!(
                            "{} d={} -> {} of input (size increased, stopping)",
                            foundation::modern_ui::symbols::pick("✗", "[x]"),
                            foundation::jxl_explorer::format_distance_for_log(candidate_distance),
                            pct_label
                        ));
                        foundation::media_conversion_gate::delivery_remove_file_or_audit(
                            "jxl continued nonwinner cleanup",
                            &candidate_output,
                        );
                        break;
                    }
                }
                Err(err) => {
                    foundation::media_conversion_gate::delivery_remove_file_or_audit(
                        "jxl continued failed candidate cleanup",
                        &candidate_output,
                    );
                    log_detail!(&format!(
                        "Continued exploration probe failed at d={}: {err}",
                        foundation::jxl_explorer::format_distance_for_log(candidate_distance)
                    ));
                    break;
                }
            }
        }

        if continued_iterations > 0 && accepted_distance < best_candidate.distance {
            log_stat!(
                foundation::infra::static_logs::messages::LABEL_PHASE_3,
                format!(
                    "Refinement Improved: d={} -> d={} ({} probes)",
                    foundation::jxl_explorer::format_distance_for_log(best_candidate.distance),
                    foundation::jxl_explorer::format_distance_for_log(accepted_distance),
                    continued_iterations
                )
            );
        }
    }

    let mut log = screening.log.clone();
    log.push(format!(
        "Accepted e10 finalist d={} -> {} of input",
        foundation::jxl_explorer::format_distance_for_log(accepted_distance),
        format_output_size_ratio_pct(input_size, accepted_size),
    ));

    if foundation::progress_mode::is_verbose_mode() {
        for line in &log {
            foundation::log_detail!(line);
        }
    }
    let result = foundation::jxl_explorer::JxlExploreResult {
        accepted_distance,
        output_size: accepted_size,
        iterations: screening.iterations,
        ladder_phase: best_candidate.ladder_phase,
        screened_best_distance: screening.best_distance,
        screened_best_size: screening.best_output_size,
        promoted_distances: screening
            .finalists
            .iter()
            .map(|candidate| candidate.distance)
            .collect(),
        log,
        initial_ratio: screening.initial_ratio,
        pressure_stops: screening.pressure_stops,
        profile_label: screening.profile_label,
        target_distance: screening.target_distance,
    }
    .sealed();

    if !foundation::media_conversion_gate::jxl_explore_delivery_acceptable(&result) {
        foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
            "jxl_explore_delivery_rejected",
            input,
            format!(
                "ultimate JXL explore sealed but failed strict delivery (size={}, distance={}, iterations={})",
                result.output_size,
                foundation::jxl_explorer::format_distance_for_log(result.accepted_distance),
                result.iterations
            ),
        );
        return Ok(None);
    }

    foundation::log_info!(
        foundation::infra::static_logs::messages::LABEL_JXL,
        &format!(
            "Ultimate JXL exploration accepted d={} after e7 screening / e10 finalization ({} of input)",
            foundation::jxl_explorer::format_distance_for_log(result.accepted_distance),
            format_output_size_ratio_pct(input_size, result.output_size),
        )
    );
    log_detail!(&format!(
        "TELEMETRY: outcome_distance={} outcome_pct={} profile={} pressure_stops={:.4}",
        foundation::jxl_explorer::format_distance_for_log(result.accepted_distance),
        format_output_size_ratio_pct_plain(input_size, result.output_size),
        result.profile_label,
        result.pressure_stops
    ));
    Ok(Some(result))
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
fn log_image_precision_decision(
    input: &Path,
    options: &ConvertOptions,
    color_assessment: &foundation::ffprobe_json::ColorInfoAssessment,
    precision: &ImagePrecisionProfile,
) {
    if precision.used_float_extension_hint() && options.verbose() {
        foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
            "float_container_hint",
            input,
            "color metadata omitted float precision; preserving float/high-precision decode from container",
        );
    }

    if precision.is_float() && options.verbose() {
        log_detail!(&format!(
            "{} Source is 32-bit float | OpenEXR intermediate bitstream selected",
            foundation::infra::static_logs::messages::LABEL_METADATA
        ));
    } else if precision.bit_depth().is_some_and(|depth| depth > 8)
        && precision.bit_depth_inferred_from_pix_fmt()
        && options.verbose()
    {
        log_detail!(&format!(
            "{} Source bit depth inferred as {}-bit from pix_fmt | 16-bit intermediate bitstream selected conservatively",
            foundation::infra::static_logs::messages::LABEL_METADATA,
            precision
                .bit_depth()
                .unwrap_or_else(|| unreachable!("checked Some(bit_depth > 8) above"))
        ));
    } else if precision.bit_depth().is_some_and(|depth| depth > 8) && options.verbose() {
        log_detail!(&format!(
            "{} Source is {}-bit | 16-bit intermediate bitstream selected",
            foundation::infra::static_logs::messages::LABEL_METADATA,
            precision
                .bit_depth()
                .unwrap_or_else(|| unreachable!("checked Some(bit_depth > 8) above"))
        ));
    } else if color_assessment.has_hdr_signaling() && options.verbose() {
        log_detail!(&format!(
            "{} {} present but precise bit depth is unknown | 16-bit intermediate bitstream selected",
            foundation::infra::static_logs::messages::LABEL_METADATA,
            foundation::media_conversion_gate::optional_nonempty_label(
                "hdr_signal_label",
                color_assessment.hdr_signal_label(),
                "HDR metadata",
                &input.display().to_string(),
            )
        ));
    } else if precision.preserve_unknown_container_with_16bit() && options.verbose() {
        log_detail!(&format!(
            "{} Source bit depth is unknown for precision-preserving container {} | 16-bit intermediate selected to avoid truncation",
            foundation::infra::static_logs::messages::LABEL_METADATA,
            foundation::media_conversion_gate::path_extension_label(input)
        ));
    }
}

fn try_high_precision_decode(
    input: &Path,
    color_info: &ColorInfo,
    color_assessment: &foundation::ffprobe_json::ColorInfoAssessment,
    precision: &ImagePrecisionProfile,
) -> Result<Option<(std::path::PathBuf, Option<tempfile::NamedTempFile>)>> {
    let decode_label = if color_assessment.has_hdr_signaling() {
        format!(
            "Forensic {} Decode Cycle: Initiating FFmpeg high bit-depth preservation",
            foundation::media_conversion_gate::optional_nonempty_label(
                "hdr_signal_label",
                color_assessment.hdr_signal_label(),
                "HDR",
                &input.display().to_string(),
            )
        )
    } else {
        "Forensic High-Precision Decode Cycle: Initiating FFmpeg 16-bit preservation".to_string()
    };
    log_detail!(&format!(
        "{} {decode_label}",
        foundation::infra::static_logs::messages::LABEL_METADATA
    ));

    match foundation::hdr::decode_image_to_png16_preserving_precision(input, color_info) {
        Ok((png16_path, temp_file)) => {
            let success_label = if color_assessment.has_hdr_signaling() {
                format!(
                    "{} decode successful: 16-bit PNG bitstream finalized",
                    foundation::media_conversion_gate::optional_nonempty_label(
                        "hdr_signal_label",
                        color_assessment.hdr_signal_label(),
                        "HDR",
                        &input.display().to_string(),
                    )
                )
            } else if precision.bit_depth_inferred_from_pix_fmt() {
                "High-precision decode successful: 16-bit PNG finalized from pix_fmt-inferred source"
                    .to_string()
            } else {
                "High-precision decode successful: 16-bit PNG bitstream finalized".to_string()
            };
            log_stat!(
                foundation::infra::static_logs::messages::LABEL_METADATA,
                success_label
            );
            Ok(Some((png16_path, Some(temp_file))))
        }
        Err(e) => {
            let failure_prefix = if color_assessment.has_hdr_signaling() {
                format!(
                    "{} decode cycle failure",
                    foundation::media_conversion_gate::optional_nonempty_label(
                        "hdr_signal_label",
                        color_assessment.hdr_signal_label(),
                        "HDR",
                        &input.display().to_string(),
                    )
                )
            } else {
                "High-precision decode cycle failure".to_string()
            };
            foundation::media_conversion_gate::delivery_jxl_path_fallback_audit(
                "hdr_png16_decode",
                input,
                format!("{failure_prefix} ({e}); falling back to standard bitstream decode"),
            );
            Ok(None)
        }
    }
}

fn preprocess_webp_for_cjxl(
    input: &Path,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    use console::style;
    log_detail!(&format!(
        "{} {}",
        style("🔧 PRE-PROCESSING:").cyan().bold(),
        style("WebP detected, using dwebp for ICC profile compatibility").dim()
    ));

    let temp_png_file =
        foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "img_lossless_webp_png",
            None,
            Some(".png"),
        )?;
    let temp_png = temp_png_file.path().to_path_buf();

    let mut builder = foundation::image_builders::DwebpBuilder::new();
    builder.input(input).output(&temp_png);

    let result = builder.build().output();

    match result {
        Ok(output) if output.status.success() && temp_png.exists() => {
            foundation::progress_mode::preprocessing_success();
            Ok((temp_png, Some(temp_png_file)))
        }
        _ => {
            log_detail!(&format!(
                "{} {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style("WebP").dim(),
                style("→ failed, trying direct cjxl").yellow()
            ));
            Ok((input.to_path_buf(), None))
        }
    }
}

fn preprocess_tiff_for_cjxl(
    input: &Path,
    options: &ConvertOptions,
    precision: &ImagePrecisionProfile,
    intermediate_depth: Option<u8>,
    depth_str: &str,
    intermediate_suffix: &str,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    let label = if precision.is_float() {
        "32-bit float OpenEXR"
    } else {
        &format!("{depth_str}-bit PNG")
    };

    let temp_file = foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "img_lossless_tiff_intermediate",
        None,
        Some(intermediate_suffix),
    )
    .map_err(ImgQualityError::IoError)?;
    let temp_path = temp_file.path().to_path_buf();

    let mut builder = foundation::MagickBuilder::new();
    builder.input(input).output(&temp_path);
    if precision.is_float() {
        builder.format("exr");
    }
    if let Some(depth) = intermediate_depth {
        builder.depth(depth);
    }
    let out = builder.build().output().map_err(ImgQualityError::IoError)?;
    if out.status.success() && temp_path.exists() {
        if options.verbose() {
            log_detail!(&format!("TIFF detected, using ImageMagick to emit {label}"));
        }
        Ok((temp_path, Some(temp_file)))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(ImgQualityError::ConversionError(format!(
            "magick TIFF conversion failed: {err}"
        )))
    }
}

fn preprocess_bmp_for_cjxl(
    input: &Path,
    options: &ConvertOptions,
    precision: &ImagePrecisionProfile,
    intermediate_depth: Option<u8>,
    depth_str: &str,
    intermediate_suffix: &str,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    let label = if precision.is_float() {
        "32-bit float OpenEXR"
    } else {
        &format!("{depth_str}-bit PNG")
    };

    let temp_file = foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "img_lossless_tiff_intermediate",
        None,
        Some(intermediate_suffix),
    )
    .map_err(ImgQualityError::IoError)?;
    let temp_path = temp_file.path().to_path_buf();

    let mut builder = foundation::MagickBuilder::new();
    builder.input(input).output(&temp_path);
    if precision.is_float() {
        builder.format("exr");
    }
    if let Some(depth) = intermediate_depth {
        builder.depth(depth);
    }
    let out = builder.build().output().map_err(ImgQualityError::IoError)?;
    if out.status.success() && temp_path.exists() {
        if options.verbose() {
            log_detail!(&format!("BMP detected, using ImageMagick to emit {label}"));
        }
        Ok((temp_path, Some(temp_file)))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(ImgQualityError::ConversionError(format!(
            "magick BMP conversion failed: {err}"
        )))
    }
}

fn preprocess_heic_for_cjxl(
    input: &Path,
    precision: &ImagePrecisionProfile,
    intermediate_depth: Option<u8>,
    intermediate_suffix: &str,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    use console::style;
    log_detail!(&format!(
        "{} {}",
        style("🔧 PRE-PROCESSING:").cyan().bold(),
        style("HEIC/HEIF detected, using sips/ImageMagick for cjxl compatibility").dim()
    ));

    let temp_png_file =
        foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "img_lossless_webp_png",
            None,
            Some(".png"),
        )?;
    let temp_png = temp_png_file.path().to_path_buf();

    log_detail!(foundation::infra::static_logs::messages::SIPS_TRY_FIRST);
    let mut builder = foundation::image_builders::SipsBuilder::new();
    builder.format("png").input(input).output(&temp_png);

    let result = builder.build().output();

    match result {
        Ok(output) if output.status.success() && temp_png.exists() => {
            log_detail!(foundation::infra::static_logs::messages::SIPS_SUCCESS);
            // sips doesn't easily support forcing 16-bit depth for PNG
            // If we need 16-bit, we might want to sanitize with magick afterwards if depth was lost,
            // but for now we trust sips for standard HEIC.
            Ok((temp_png, Some(temp_png_file)))
        }
        _ => {
            log_detail!(foundation::infra::static_logs::messages::SIPS_FAIL_TRY_MAGICK);
            let temp_file =
                foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                    "img_lossless_heic_intermediate",
                    None,
                    Some(intermediate_suffix),
                )?;
            let temp_path = temp_file.path().to_path_buf();

            let mut builder = foundation::image_builders::MagickBuilder::new();
            builder.input(input).output(&temp_path);

            if precision.is_float() {
                builder.format("exr");
            }

            if let Some(depth) = intermediate_depth {
                builder.depth(depth);
            }

            match builder.build().output() {
                Ok(output) if output.status.success() && temp_path.exists() => {
                    log_detail!(foundation::infra::static_logs::messages::MAGICK_SUCCESS);
                    Ok((temp_path, Some(temp_file)))
                }
                _ => {
                    log_detail!(foundation::infra::static_logs::messages::MAGICK_FAIL_TRY_CJXL);
                    Ok((input.to_path_buf(), None))
                }
            }
        }
    }
}

fn preprocess_gif_for_cjxl(
    input: &Path,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    use console::style;
    log_detail!(&format!(
        "{} {}",
        style("🔧 PRE-PROCESSING:").cyan().bold(),
        style("GIF detected, using FFmpeg for static frame extraction").dim()
    ));

    let temp_png_file =
        foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "img_lossless_webp_png",
            None,
            Some(".png"),
        )?;
    let temp_png = temp_png_file.path().to_path_buf();

    let mut builder = foundation::FfmpegBuilder::new();
    builder
        .overwrite()
        .input(input)
        .frames_v(1)
        .output(&temp_png);

    let result = builder.build().output();

    match result {
        Ok(out) if out.status.success() && temp_png.exists() => {
            foundation::progress_mode::preprocessing_success();
            Ok((temp_png, Some(temp_png_file)))
        }
        _ => {
            log_detail!(&format!(
                "{} {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style("GIF").dim(),
                style("→ failed, trying direct cjxl").yellow()
            ));
            Ok((input.to_path_buf(), None))
        }
    }
}

fn preprocess_fallback_extension_align(
    input: &Path,
    ext: &str,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    if let Some(actual_ext) = input.extension().and_then(|e| e.to_str()) {
        if actual_ext.to_lowercase() == ext {
            Ok((input.to_path_buf(), None))
        } else {
            log_detail!(&format!(
                "🔧 PRE-PROCESSING: Extension mismatch detected (.{actual_ext} vs {ext}), creating aligned temp file"
            ));

            let temp_aligned_file =
                foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                    "img_lossless_ext_align",
                    None,
                    Some(&format!(".{ext}")),
                )?;
            let temp_path = temp_aligned_file.path().to_path_buf();

            match std::fs::copy(input, &temp_path) {
                Ok(_) => Ok((temp_path, Some(temp_aligned_file))),
                Err(e) => {
                    log_detail!(&format!(
                        "🔧 PRE-PROCESSING: extension-align copy failed: {e}; proceeding with original path"
                    ));
                    Ok((input.to_path_buf(), None))
                }
            }
        }
    } else {
        Ok((input.to_path_buf(), None))
    }
}

fn preprocess_jpeg_for_cjxl(
    input: &Path,
    precision: &ImagePrecisionProfile,
    intermediate_depth: Option<u8>,
    intermediate_suffix: &str,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    let is_header_valid = foundation::media_conversion_gate::jpeg_magic_valid_for_delivery(input);

    if is_header_valid {
        Ok((input.to_path_buf(), None))
    } else {
        use console::style;
        log_detail!(&format!(
            "{} {}",
            style("🔧 PRE-PROCESSING:").yellow().bold(),
            style("Corrupted JPEG header detected, using ImageMagick to sanitize").yellow()
        ));

        let temp_file =
            foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                "img_lossless_jpeg_sanitize",
                None,
                Some(intermediate_suffix),
            )?;
        let temp_path = temp_file.path().to_path_buf();

        let mut builder = foundation::MagickBuilder::new();
        builder.input(input).output(&temp_path);

        if precision.is_float() {
            builder.format("exr");
        }

        if let Some(depth) = intermediate_depth {
            builder.depth(depth);
        }

        let result = builder.build().output();

        match result {
            Ok(output) if output.status.success() && temp_path.exists() => {
                let label = if precision.is_float() {
                    "OpenEXR"
                } else {
                    "ImageMagick PNG"
                };
                log_detail!(&format!(
                    "{} {} {} sanitization successful",
                    style("").green(),
                    style(label).green().bold(),
                    style("JPEG").dim()
                ));
                Ok((temp_path, Some(temp_file)))
            }
            _ => {
                log_detail!(&format!(
                    "{} {}",
                    style("").red(),
                    style("ImageMagick sanitization failed, trying direct input").dim()
                ));
                Ok((input.to_path_buf(), None))
            }
        }
    }
}

fn prepare_input_for_cjxl(
    input: &Path,
    options: &ConvertOptions,
    color_info: Option<&ColorInfo>,
) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    // Ensure we have color info for bit depth detection if not provided
    let mut probed_color_info = foundation::ffprobe_json::ColorInfo::default();
    let color_info = foundation::media_conversion_gate::color_info_for_cjxl_prep(
        input,
        color_info,
        &mut probed_color_info,
    );

    let color_assessment = color_info.assessment();
    let precision = ImagePrecisionProfile::inspect(input, color_info);
    let depth_str = precision.intermediate_depth_str();
    let intermediate_depth = if precision.is_float() {
        None
    } else {
        Some(depth_str.parse::<u8>().map_err(|err| {
            ImgQualityError::ConversionError(format!(
                "invalid intermediate bit depth '{depth_str}' for {}: {err}",
                input.display()
            ))
        })?)
    };
    let intermediate_suffix = precision.intermediate_suffix();

    log_image_precision_decision(input, options, &color_assessment, &precision);

    // Check if we need 16-bit decode for HDR or high-precision preservation.
    if precision.should_use_high_precision_png16_decode()
        && let Some(res) =
            try_high_precision_decode(input, color_info, &color_assessment, &precision)?
    {
        return Ok(res);
    }

    let detected_ext = foundation::common_utils::detect_real_extension(input);
    let literal_ext = foundation::media_conversion_gate::path_extension_lowercase_or_empty(
        input,
        &format!("lossless literal ext {}", input.display()),
    );

    let ext = if let Some(real) = detected_ext {
        if !literal_ext.is_empty()
            && real != literal_ext
            && !((real == "jpg" && literal_ext == "jpeg")
                || (real == "jpeg" && literal_ext == "jpg"))
        {
            log_detail!(&format!(
                "{} Smart-fix extension mismatch: '{}' (.{}) -> re-tagged as {}",
                foundation::infra::static_logs::messages::LABEL_METADATA,
                input.display(),
                literal_ext,
                real.to_uppercase()
            ));
        }
        real.to_string()
    } else if let Some(ref format) = options.input_format {
        format.to_lowercase()
    } else {
        literal_ext
    };

    match ext.as_str() {
        foundation::constants::EXT_JPG | foundation::constants::EXT_JPEG => {
            preprocess_jpeg_for_cjxl(input, &precision, intermediate_depth, intermediate_suffix)
        }

        foundation::constants::EXT_WEBP => preprocess_webp_for_cjxl(input),

        foundation::constants::EXT_TIFF | foundation::constants::EXT_TIF => {
            preprocess_tiff_for_cjxl(
                input,
                options,
                &precision,
                intermediate_depth,
                depth_str,
                intermediate_suffix,
            )
        }

        foundation::constants::EXT_BMP => preprocess_bmp_for_cjxl(
            input,
            options,
            &precision,
            intermediate_depth,
            depth_str,
            intermediate_suffix,
        ),

        foundation::constants::EXT_HEIC | foundation::constants::EXT_HEIF => {
            preprocess_heic_for_cjxl(input, &precision, intermediate_depth, intermediate_suffix)
        }

        foundation::constants::EXT_GIF => preprocess_gif_for_cjxl(input),

        _ => preprocess_fallback_extension_align(input, &ext),
    }
}

fn get_output_path(
    input: &Path,
    extension: &str,
    options: &ConvertOptions,
) -> Result<std::path::PathBuf> {
    let output = if let Some(ref base) = options.base_dir {
        foundation::conversion::determine_output_path_with_base(
            input,
            base,
            extension,
            &options.output_dir,
        )
        .map_err(ImgQualityError::ConversionError)?
    } else {
        foundation::conversion::determine_output_path(input, extension, &options.output_dir)
            .map_err(ImgQualityError::ConversionError)?
    };

    // Validate output path (check path traversal, symlinks)
    foundation::conversion::validate_output_path(&output, options.base_dir.as_deref())
        .map_err(ImgQualityError::ConversionError)?;

    Ok(output)
}

fn has_jxl_magic(header: &[u8]) -> bool {
    const JXL_CODESTREAM_MAGIC: &[u8] = &[0xFF, 0x0A];
    const JXL_CONTAINER_MAGIC: &[u8] = &[
        0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
    ];
    header.starts_with(JXL_CODESTREAM_MAGIC) || header.starts_with(JXL_CONTAINER_MAGIC)
}

fn validate_jxl_output_preflight(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path).map_err(|err| {
        ImgQualityError::ConversionError(format!(
            "cjxl reported success but output is missing: {} ({err})",
            path.display()
        ))
    })?;
    if metadata.len() == 0 {
        return Err(ImgQualityError::ConversionError(format!(
            "cjxl reported success but output is empty: {}",
            path.display()
        )));
    }

    let mut file = fs::File::open(path).map_err(|err| {
        ImgQualityError::ConversionError(format!(
            "cjxl output preflight cannot open {}: {err}",
            path.display()
        ))
    })?;
    let mut header = [0u8; 12];
    let read_len = file.read(&mut header).map_err(|err| {
        ImgQualityError::ConversionError(format!(
            "cjxl output preflight cannot read {}: {err}",
            path.display()
        ))
    })?;
    if !has_jxl_magic(&header[..read_len]) {
        return Err(ImgQualityError::ConversionError(format!(
            "cjxl reported success but output magic is not JXL: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_jxl_health(path: &Path) -> Result<()> {
    validate_jxl_output_preflight(path)?;
    foundation::jxl_utils::verify_jxl_health(path).map_err(ImgQualityError::ConversionError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::process::{Command, Stdio};
    use tempfile::tempdir;
    use vid::animated_image::is_high_quality_animated;

    fn test_tool_available(tool: &str) -> bool {
        match Command::new(tool)
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) => status.success(),
            Err(_err) => false,
        }
    }

    #[test]
    fn jxl_magic_accepts_codestream_and_container_headers() {
        assert!(has_jxl_magic(&[0xFF, 0x0A, 0x00]));
        assert!(has_jxl_magic(&[
            0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
        ]));
        assert!(!has_jxl_magic(b"not-jxl"));
    }

    #[test]
    fn magick_intermediate_depth_uses_single_fail_closed_parse() {
        let source = include_str!("lossless_converter.rs");
        let needle = ["if let ", "Ok(depth) = depth_str.", "parse::<u8>()"].concat();

        assert!(
            !source.contains(&needle),
            "intermediate depth must not be silently skipped by parse gate"
        );
    }

    #[test]
    fn jxl_output_preflight_rejects_missing_empty_and_wrong_magic() {
        let tmp = tempdir().unwrap_or_else(|e| panic!("tempdir: {e:?}"));
        let missing = tmp.path().join("missing.JXL");
        let empty = tmp.path().join("empty.JXL");
        let wrong = tmp.path().join("wrong.JXL");
        fs::write(&empty, []).unwrap_or_else(|e| panic!("write empty: {e:?}"));
        fs::write(&wrong, b"not-jxl").unwrap_or_else(|e| panic!("write wrong: {e:?}"));

        assert!(
            validate_jxl_output_preflight(&missing)
                .unwrap_err()
                .to_string()
                .contains("output is missing")
        );
        assert!(
            validate_jxl_output_preflight(&empty)
                .unwrap_err()
                .to_string()
                .contains("output is empty")
        );
        assert!(
            validate_jxl_output_preflight(&wrong)
                .unwrap_err()
                .to_string()
                .contains("magic is not JXL")
        );
    }

    #[test]
    fn cjxl_exit_summary_reports_nonzero_exit_code() {
        let status = Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .status()
            .unwrap_or_else(|err| panic!("spawn shell: {err:?}"));

        assert_eq!(cjxl_exit_summary(status), "exit code 7");
    }

    #[test]
    fn jpeg_lossless_transcode_leaves_color_encoding_to_libjxl() {
        let source = include_str!("lossless_converter.rs");
        let start = source
            .find("fn run_cjxl_jpeg_transcode_with_effort(")
            .unwrap_or_else(|| panic!("JPEG transcode runner anchor missing"));
        let end = source[start..]
            .find("fn jpeg_effort_stage_label")
            .map_or_else(
                || panic!("JPEG transcode runner end anchor missing"),
                |offset| start + offset,
            );
        let body = &source[start..end];

        assert!(
            !body.contains("extract_icc_profile("),
            "JPEG lossless transcode must let libjxl adopt the source JPEG ICC"
        );
        assert!(
            !body.contains(".icc_profile("),
            "JPEG lossless transcode must not force an ICC override"
        );
        assert!(
            !body.contains("color_info_to_cicp("),
            "JPEG lossless transcode must not synthesize CICP over JPEG-native color"
        );
        assert!(
            !body.contains(".cicp("),
            "JPEG lossless transcode must not force a CICP override"
        );
    }

    #[test]
    fn grayscale_jpeg_with_gray_icc_transcodes_to_jxl() {
        for tool in ["magick", "cjxl", "djxl", "jxlinfo", "exiftool"] {
            if !test_tool_available(tool) {
                eprintln!("Skipping grayscale JPEG ICC transcode test: {tool} is unavailable");
                return;
            }
        }

        let gray_profile =
            Path::new("/System/Library/ColorSync/Profiles/Generic Gray Gamma 2.2 Profile.icc");
        if !gray_profile.exists() {
            eprintln!(
                "Skipping grayscale JPEG ICC transcode test: {} is unavailable",
                gray_profile.display()
            );
            return;
        }

        clear_processed_list();
        let tmp = tempdir().unwrap_or_else(|err| panic!("tempdir: {err:?}"));
        let input = tmp.path().join("gray_icc.jpg");
        let output = tmp.path().join("gray_icc.jxl");

        let magick_status = Command::new("magick")
            .arg("-size")
            .arg("64x64")
            .arg("gradient:")
            .arg("-colorspace")
            .arg("Gray")
            .arg("-profile")
            .arg(gray_profile)
            .arg(&input)
            .status()
            .unwrap_or_else(|err| panic!("magick grayscale ICC JPEG fixture failed: {err:?}"));
        assert!(
            magick_status.success(),
            "failed to create grayscale ICC JPEG"
        );

        let icc_probe = Command::new("exiftool")
            .arg("-icc_profile")
            .arg("-b")
            .arg(&input)
            .output()
            .unwrap_or_else(|err| panic!("exiftool ICC probe failed: {err:?}"));
        assert!(
            icc_probe.status.success() && !icc_probe.stdout.is_empty(),
            "fixture must contain a grayscale ICC profile"
        );

        let mut options = ConvertOptions::default();
        options.flags.set(ConvertFlags::FORCE, true);
        options
            .flags
            .set(ConvertFlags::REQUIRE_OUTPUT_DELIVERY, true);
        options
            .flags
            .set(ConvertFlags::REQUIRE_JPEG_RECONSTRUCTION, true);
        options.child_threads = 1;

        let result = convert_jpeg_to_jxl(&input, &options, None)
            .unwrap_or_else(|err| panic!("grayscale JPEG ICC transcode failed: {err}"));

        assert!(!result.skipped, "grayscale ICC JPEG must not be skipped");
        assert!(output.exists(), "JXL output was not delivered");
        verify_jxl_health(&output).unwrap_or_else(|err| panic!("JXL health failed: {err}"));
        assert_eq!(
            foundation::image::format_detect::detect_true_format(&output)
                .unwrap_or_else(|err| panic!("format detect failed: {err}")),
            foundation::image::format_detect::FormatKind::Jxl
        );
    }

    #[test]
    fn jpeg_transcode_proof_policy_distinguishes_bitstream_from_pixel_equivalence() {
        let input = Path::new("/tmp/source.jpg");
        let sanitized = Path::new("/tmp/source.sanitized.jpg");

        assert_eq!(
            jpeg_transcode_proof_for_success(input, input, None),
            JpegTranscodeProof::BitstreamReconstruction
        );
        assert_eq!(
            jpeg_transcode_proof_for_success(input, sanitized, None),
            JpegTranscodeProof::PixelEquivalence
        );
        assert_eq!(
            jpeg_transcode_proof_for_success(input, input, Some(0)),
            JpegTranscodeProof::PixelEquivalence
        );
    }

    #[test]
    fn jpeg_type_a_pixel_reencode_fallback_requires_explicit_opt_in() {
        let default_options = ConvertOptions::default();
        let mut opted_in_options = ConvertOptions::default();
        opted_in_options
            .flags
            .set(ConvertFlags::ALLOW_JPEG_PIXEL_REENCODE_FALLBACK, true);

        assert!(!jpeg_pixel_reencode_fallback_allowed(&default_options));
        assert!(jpeg_pixel_reencode_fallback_allowed(&opted_in_options));
    }

    #[test]
    fn truncated_jpeg_in_fast_delivery_is_skipped_not_raw_eoi_error() {
        let tmp = tempdir().unwrap_or_else(|e| panic!("tempdir: {e:?}"));
        let input = tmp.path().join("truncated.jpeg");
        fs::write(&input, [0xFF, 0xD8, 0xFF, 0xE0])
            .unwrap_or_else(|e| panic!("write truncated jpeg: {e:?}"));
        let mut options = ConvertOptions::default();
        options
            .flags
            .set(ConvertFlags::REQUIRE_OUTPUT_DELIVERY, true);

        let result = convert_jpeg_to_jxl(&input, &options, None)
            .unwrap_or_else(|e| panic!("fast delivery should skip truncated JPEG, not error: {e}"));

        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(
            result.skip_reason.as_deref(),
            Some(JPEG_LOSSLESS_TRANSCODE_UNAVAILABLE_SKIP_REASON)
        );
        assert!(
            !result.message.contains("JPEG is truncated or missing EOI"),
            "raw EOI probe failure must not be the user-facing batch result: {}",
            result.message
        );
    }

    #[test]
    fn jpeg_transcode_threads_honor_explicit_child_thread_cap() {
        let mut options = ConvertOptions {
            child_threads: 2,
            ..ConvertOptions::default()
        };
        assert_eq!(jpeg_transcode_threads(&options), 2);

        options.child_threads = 0;
        assert!(jpeg_transcode_threads(&options) >= 1);
    }

    #[test]
    fn jpeg_effort_policy_uses_fixed_e7_below_1mib_and_mode_effort_above() {
        assert!(!size_ge_1mib(1_048_575));
        assert!(size_ge_1mib(1_048_576));
        assert_eq!(
            jxl_encode_effort_for_size(false, false, false, 1_048_575),
            7
        );
        assert_eq!(jxl_encode_effort_for_size(false, true, false, 1_048_575), 7);
        assert_eq!(
            jxl_encode_effort_for_size(false, false, false, 1_048_576),
            7
        );
        assert_eq!(
            jxl_encode_effort_for_size(false, true, false, 1_048_576),
            10
        );
        assert_eq!(jxl_encode_effort_for_size(false, true, true, 1_048_576), 7);
        assert_eq!(
            jxl_encode_effort_for_size(true, false, false, 1_048_575),
            10
        );
    }

    #[test]
    fn jpeg_effort_search_plan_explores_supported_efforts_at_or_above_1mib() {
        assert_eq!(
            jpeg_effort_search_plan(JpegEffortModeFlags::empty(), 1_048_575),
            vec![JxlEffortPlan::Single(7)]
        );
        assert_eq!(
            jpeg_effort_search_plan(JpegEffortModeFlags::empty(), 1_048_576),
            vec![
                JxlEffortPlan::Candidate(7),
                JxlEffortPlan::Candidate(8),
                JxlEffortPlan::Candidate(10),
                JxlEffortPlan::Candidate(foundation::constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT),
            ]
        );
        assert_eq!(
            jpeg_effort_search_plan(JpegEffortModeFlags::ALLOW_EXPERT_OPTIONS, 1_048_576),
            vec![
                JxlEffortPlan::Candidate(7),
                JxlEffortPlan::Candidate(8),
                JxlEffortPlan::Candidate(10),
                JxlEffortPlan::Candidate(foundation::constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT),
            ]
        );
    }

    #[test]
    fn extreme_jpeg_lossless_transcode_starts_with_e11_aggressive_phase() {
        assert_eq!(
            jpeg_aggressive_lossless_plan(true),
            vec![JxlEffortPlan::Single(
                foundation::constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT
            )]
        );
        assert!(
            jpeg_aggressive_lossless_plan(false).is_empty(),
            "non-extreme JPEG transcode must not force the e11-only phase"
        );
    }

    #[test]
    fn jpeg_standard_transcode_fallback_excludes_e11_after_aggressive_phase() {
        assert_eq!(
            jpeg_standard_transcode_fallback_plan(1_048_575),
            vec![JxlEffortPlan::Single(7)]
        );
        let fallback = jpeg_standard_transcode_fallback_plan(1_048_576);
        assert_eq!(
            fallback,
            vec![
                JxlEffortPlan::Candidate(7),
                JxlEffortPlan::Candidate(8),
                JxlEffortPlan::Candidate(10),
            ]
        );
        assert!(
            !fallback.iter().any(
                |item| item.effort() == foundation::constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT
            ),
            "Phase 2 must not retry e11 after the aggressive e11 phase is already hopeless"
        );
    }

    #[test]
    fn aggressive_e11_process_error_reaches_standard_fallback_branch() {
        let source = include_str!("lossless_converter.rs");
        let start = source
            .find("let output_cmd = match result {")
            .unwrap_or_else(|| panic!("JPEG transcode result match anchor missing"));
        let end = source[start..]
            .find("let stderr = output_cmd.stderr.clone();")
            .map_or_else(
                || panic!("JPEG transcode stderr anchor missing"),
                |offset| start + offset,
            );
        let branch = &source[start..end];

        assert!(
            branch.contains("Err(e) if aggressive_e11 =>"),
            "aggressive e11 process errors must not bypass Phase 2"
        );
        assert!(
            branch.contains("run_standard_jpeg_lossless_fallback("),
            "aggressive e11 process errors must attempt standard lossless fallback"
        );
        assert!(
            branch.contains("handle_irreversible_jpeg_transcode_failure("),
            "aggressive e11 process errors must reach the fast-img skip/direct-encode branch"
        );
    }

    #[test]
    fn jxl_effort_search_plan_matches_jpeg_policy_for_large_inputs() {
        assert_eq!(
            jxl_effort_search_plan(false, false, false, 1_048_575),
            vec![JxlEffortPlan::Single(7)]
        );
        assert_eq!(
            jxl_effort_search_plan(false, false, false, 1_048_576),
            vec![
                JxlEffortPlan::Candidate(7),
                JxlEffortPlan::Candidate(8),
                JxlEffortPlan::Candidate(10),
            ]
        );
        assert_eq!(
            jxl_effort_search_plan(false, true, false, 1_048_576),
            vec![
                JxlEffortPlan::Candidate(10),
                JxlEffortPlan::Candidate(7),
                JxlEffortPlan::Candidate(8),
            ]
        );
    }

    #[test]
    fn jxl_primary_encode_paths_invoke_effort_search_runner() {
        let source = include_str!("lossless_converter.rs");

        let convert_start = source
            .find("pub fn convert_to_jxl(")
            .unwrap_or_else(|| panic!("convert_to_jxl source anchor missing"));
        let matched_start = source
            .find("pub fn convert_to_jxl_matched(")
            .unwrap_or_else(|| panic!("convert_to_jxl_matched source anchor missing"));
        let convert_body = &source[convert_start..matched_start];
        let direct_runner_start = convert_body
            .find("let result = run_direct_jxl_encode_effort_search(")
            .unwrap_or_else(|| panic!("convert_to_jxl must invoke direct JXL effort search"));
        let primary_setup = &convert_body[..direct_runner_start];
        assert!(
            !primary_setup.contains("CjxlBuilder::new()"),
            "convert_to_jxl primary path must not build one direct cjxl effort before effort search"
        );

        let matched_end = source
            .find("const fn jxl_screening_effort")
            .unwrap_or_else(|| panic!("jxl_screening_effort source anchor missing"));
        let matched_body = &source[matched_start..matched_end];
        assert!(
            matched_body.contains("let result = run_direct_jxl_encode_effort_search("),
            "convert_to_jxl_matched must invoke direct JXL effort search"
        );
        assert!(
            !matched_body.contains("let mut builder = foundation::CjxlBuilder::new();"),
            "convert_to_jxl_matched must not regress to a single direct cjxl effort"
        );
    }

    #[test]
    fn jpeg_effort_search_winner_uses_smallest_successful_output() {
        let candidates = [
            JxlEffortCandidate {
                effort: 7,
                output_size: 1_200,
            },
            JxlEffortCandidate {
                effort: 10,
                output_size: 1_000,
            },
        ];

        assert_eq!(select_jxl_effort_winner(&candidates), Some(1));
    }

    #[test]
    fn jpeg_jbrd_ladder_failure_message_lists_structural_recovery_layers() {
        let mut diagnostics = JpegJbrdLadderDiagnostics::new("/tmp/source.jpg");
        diagnostics.record_process_failure("primary JPEG lossless", 1, "EncodeImageJXL() failed.");
        diagnostics.record_skipped("jpegtran optimize retry", "jpegtran unavailable");
        diagnostics.record_skipped("metadata-safe structural rebuild", "exiftool unavailable");

        let message = diagnostics.fail_closed_message(false);

        assert!(message.contains("/tmp/source.jpg"));
        assert!(message.contains("primary JPEG lossless: exit code 1"));
        assert!(message.contains("EncodeImageJXL() failed."));
        assert!(message.contains("jpegtran optimize retry: skipped"));
        assert!(message.contains("jpegtran unavailable"));
        assert!(message.contains("metadata-safe structural rebuild: skipped"));
        assert!(message.contains("exiftool unavailable"));
        assert!(message.contains("ALLOW_JPEG_PIXEL_REENCODE_FALLBACK"));
    }

    #[test]
    fn test_get_output_path() {
        let tmp = tempdir().unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let root =
            std::fs::canonicalize(tmp.path()).unwrap_or_else(|e| panic!("canonicalize: {e:?}"));
        let input_dir = root.join("path").join("to");
        std::fs::create_dir_all(&input_dir).unwrap_or_else(|e| panic!("create input dir: {e:?}"));
        let input = input_dir.join("image.png");
        std::fs::write(&input, b"png").unwrap_or_else(|e| panic!("write input file: {e:?}"));
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let output =
            get_output_path(&input, "jxl", &options).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(output, input_dir.join("image.JXL"));
    }

    #[test]
    fn test_get_output_path_with_dir() {
        let tmp = tempdir().unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let root =
            std::fs::canonicalize(tmp.path()).unwrap_or_else(|e| panic!("canonicalize: {e:?}"));
        let input_dir = root.join("path").join("to");
        std::fs::create_dir_all(&input_dir).unwrap_or_else(|e| panic!("create input dir: {e:?}"));
        let input = input_dir.join("image.png");
        std::fs::write(&input, b"png").unwrap_or_else(|e| panic!("write input file: {e:?}"));
        let output_dir = root.join("output");
        let options = ConvertOptions {
            output_dir: Some(output_dir.clone()),
            base_dir: None,
            ..Default::default()
        };
        let output =
            get_output_path(&input, "avif", &options).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(output, output_dir.join("image.AVIF"));
    }

    #[test]
    fn test_get_output_path_same_file_error() {
        let tmp = tempdir().unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let root =
            std::fs::canonicalize(tmp.path()).unwrap_or_else(|e| panic!("canonicalize: {e:?}"));
        let input = root.join("image.JXL");
        let options = ConvertOptions {
            output_dir: None,
            base_dir: None,
            ..Default::default()
        };
        let result = get_output_path(&input, "jxl", &options);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_high_quality_720p() {
        assert!(is_high_quality_animated(1280, 720));
    }

    #[test]
    fn test_is_high_quality_1080p() {
        assert!(is_high_quality_animated(1920, 1080));
    }

    #[test]
    fn test_is_high_quality_width_only() {
        assert!(is_high_quality_animated(1280, 480));
    }

    #[test]
    fn test_is_high_quality_height_only() {
        assert!(is_high_quality_animated(960, 720));
    }

    #[test]
    fn test_is_high_quality_total_pixels() {
        assert!(is_high_quality_animated(1024, 900));
    }

    #[test]
    fn test_is_not_high_quality_small() {
        assert!(!is_high_quality_animated(640, 480));
    }

    #[test]
    fn test_is_not_high_quality_480p() {
        assert!(!is_high_quality_animated(854, 480));
    }

    #[test]
    fn test_is_not_high_quality_typical_gif() {
        assert!(!is_high_quality_animated(400, 300));
        assert!(!is_high_quality_animated(500, 500));
        assert!(!is_high_quality_animated(320, 240));
    }

    #[test]
    fn test_calculate_matched_distance_allows_unknown_bit_depth() {
        let analysis = crate::ImageAnalysis {
            format: "PNG".to_string(),
            width: 1920,
            height: 1080,
            file_size: 500_000,
            color_depth: None,
            has_alpha: false,
            ..Default::default()
        };

        // Unknown bit_depth is OK when JPEG estimated_quality bypasses chroma-based effective_bpp.
        let distance = calculate_matched_distance_for_static(
            &crate::ImageAnalysis {
                jpeg_analysis: Some(foundation::image_jpeg_analysis::JpegQualityAnalysis {
                    estimated_quality: 85,
                    confidence: 1.0,
                    is_standard_table: true,
                    luminance_sse: 0.0,
                    chrominance_sse: None,
                    luminance_quality: 85,
                    chrominance_quality: None,
                    quality_description: String::from("unit-test"),
                    is_high_quality_original: false,
                    is_complete: true,
                    encoder_hint: None,
                }),
                ..analysis
            },
            analysis.file_size,
        )
        .expect("jpeg estimated_quality path must work without bit_depth");
        assert!(distance >= 0.0);
    }

    #[test]
    fn test_float_inputs_skip_high_precision_png16_decode() {
        let hdr_info = ColorInfo {
            bit_depth: Some(32),
            is_float: true,
            ..Default::default()
        };
        let precision = ImagePrecisionProfile::inspect(std::path::Path::new("test.exr"), &hdr_info);

        assert!(!precision.should_use_high_precision_png16_decode());
    }

    #[test]
    fn test_integer_hdr_inputs_still_use_high_precision_png16_decode() {
        let hdr_info = ColorInfo {
            bit_depth: Some(10),
            color_transfer: Some("smpte2084".to_string()),
            ..Default::default()
        };
        let precision = ImagePrecisionProfile::inspect(std::path::Path::new("test.png"), &hdr_info);

        assert!(precision.should_use_high_precision_png16_decode());
    }

    #[test]
    fn test_inferred_high_bit_depth_preserves_precision_without_claiming_explicit_depth() {
        let hdr_info = ColorInfo {
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: true,
            ..Default::default()
        };

        let precision =
            ImagePrecisionProfile::inspect(std::path::Path::new("test.avif"), &hdr_info);

        assert_eq!(precision.bit_depth(), Some(10));
        assert!(precision.bit_depth_inferred_from_pix_fmt());
        assert!(precision.should_preserve_high_precision());
        assert!(precision.should_use_high_precision_png16_decode());
    }

    fn should_convert_to_video_format(duration: f32, width: u32, height: u32) -> bool {
        const DURATION_THRESHOLD: f32 = 3.0;
        duration >= DURATION_THRESHOLD || is_high_quality_animated(width, height)
    }

    #[test]
    fn smoke_apple_compat_routing_short_low_quality() {
        assert!(
            !should_convert_to_video_format(2.0, 400, 300),
            "Short animation (2s) + low quality (400x300) should convert to GIF"
        );
    }

    #[test]
    fn smoke_apple_compat_routing_short_high_quality() {
        assert!(
            should_convert_to_video_format(2.0, 1920, 1080),
            "Short animation (2s) + high quality (1920x1080) should convert to video"
        );
    }

    #[test]
    fn smoke_apple_compat_routing_long_low_quality() {
        assert!(
            should_convert_to_video_format(5.0, 400, 300),
            "Long animation (5s) should convert to video regardless of quality"
        );
    }

    #[test]
    fn smoke_apple_compat_routing_boundary_3_seconds() {
        assert!(
            should_convert_to_video_format(3.0, 400, 300),
            "Exactly 3 seconds should convert to video"
        );
    }

    #[test]
    fn smoke_apple_compat_routing_boundary_under_3_seconds() {
        assert!(
            !should_convert_to_video_format(2.99, 400, 300),
            "2.99s + low quality should convert to GIF"
        );
    }

    #[test]
    fn smoke_format_classification_no_overlap() {
        let preprocess_formats = [
            foundation::constants::EXT_WEBP,
            foundation::constants::EXT_TIFF,
            foundation::constants::EXT_TIF,
            foundation::constants::EXT_BMP,
            foundation::constants::EXT_HEIC,
            foundation::constants::EXT_HEIF,
        ];
        let direct_formats = [
            foundation::constants::EXT_PNG,
            foundation::constants::EXT_JPG,
            foundation::constants::EXT_JPEG,
            foundation::constants::EXT_GIF,
            foundation::constants::EXT_JXL,
            foundation::constants::EXT_AVIF,
        ];

        for fmt in &preprocess_formats {
            assert!(
                !direct_formats.contains(fmt),
                "Format '{fmt}' appears in both preprocess and direct format lists; configuration error"
            );
        }
    }

    #[test]
    fn smoke_jxl_exploration_probe_uses_imagemagick_fallback() {
        let direct_calls = Cell::new(0);
        let fallback_calls = Cell::new(0);

        let mut direct = |distance: f32| {
            direct_calls.set(direct_calls.get() + 1);
            assert!((distance - 0.2).abs() < f32::EPSILON);
            Err("direct cjxl failed".to_string())
        };
        let mut fallback = |distance: f32| {
            fallback_calls.set(fallback_calls.get() + 1);
            assert!((distance - 0.2).abs() < f32::EPSILON);
            Ok(88)
        };

        let size = run_jxl_exploration_probe_with(0.2, &mut direct, &mut fallback)
            .unwrap_or_else(|e| panic!("fallback should recover the exploration probe: {e:?}"));

        assert_eq!(size, 88);
        assert_eq!(direct_calls.get(), 1);
        assert_eq!(fallback_calls.get(), 1);
    }

    #[test]
    fn smoke_jxl_exploration_probe_skips_fallback_after_direct_success() {
        let direct_calls = Cell::new(0);
        let fallback_calls = Cell::new(0);

        let mut direct = |_distance: f32| {
            direct_calls.set(direct_calls.get() + 1);
            Ok(77)
        };
        let mut fallback = |_distance: f32| {
            fallback_calls.set(fallback_calls.get() + 1);
            Ok(55)
        };

        let size = run_jxl_exploration_probe_with(0.1, &mut direct, &mut fallback)
            .unwrap_or_else(|e| panic!("direct cjxl probe should win: {e:?}"));

        assert_eq!(size, 77);
        assert_eq!(direct_calls.get(), 1);
        assert_eq!(fallback_calls.get(), 0);
    }

    #[test]
    fn smoke_jxl_screening_effort_only_drops_to_e7_for_ultimate_explore() {
        assert_eq!(jxl_screening_effort(false, true, true), 7);
        assert_eq!(jxl_screening_effort(false, true, false), 10);
        assert_eq!(jxl_screening_effort(false, false, true), 7);
        assert_eq!(jxl_screening_effort(false, false, false), 7);
        assert_eq!(jxl_screening_effort(true, false, false), 10);
    }

    #[test]
    fn smoke_jxl_final_round_prefers_lower_distance_once_size_beats_source() {
        assert_eq!(
            compare_jxl_finalists(9_000_000, 0.01, 8_800_000, 0.1, 7_500_000),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn smoke_jxl_final_round_requires_beating_source_before_quality_preference() {
        assert_eq!(
            compare_jxl_finalists(9_000_000, 0.01, 9_200_000, 0.1, 8_900_000),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn cjxl_timeout_malformed_env_returns_error_not_default() {
        let previous = match std::env::var(CJXL_TIMEOUT_SECS_ENV) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(err) => panic!("test env var read failed: {err}"),
        };
        unsafe {
            std::env::set_var(CJXL_TIMEOUT_SECS_ENV, "not-a-number");
        }

        let err = cjxl_timeout().expect_err("malformed cjxl timeout must fail closed");
        assert!(
            err.to_string().contains(CJXL_TIMEOUT_SECS_ENV),
            "unexpected error: {err}"
        );

        unsafe {
            match previous {
                Some(value) => std::env::set_var(CJXL_TIMEOUT_SECS_ENV, value),
                None => std::env::remove_var(CJXL_TIMEOUT_SECS_ENV),
            }
        }
    }

    #[test]
    fn output_size_ratio_pct_zero_input_is_none_not_fabricated_hundred() {
        assert!(
            output_size_ratio_pct(0, 500).is_none(),
            "zero input_size must not fabricate 100.0% efficiency"
        );
        assert_eq!(
            format_output_size_ratio_pct(0, 500),
            "N/A",
            "display must disclose unavailable ratio"
        );
    }
}
