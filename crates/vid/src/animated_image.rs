//! - `vid`: all video encoding (including animated image → video)

use crate::{Rational, Result, VidQualityError};
use shared_utils::conversion::{ConversionResult, ConvertOptions};
use std::fs;
use std::path::Path;

use shared_utils::constants::ANIMATION_CLIP_THRESHOLD_SECS;
use shared_utils::conversion::{
    determine_output_path_with_base, is_already_processed, mark_as_processed,
};
use shared_utils::loop_intent::{is_lossless_exploration_safe, LoopMeta};
#[derive(Debug, Clone, PartialEq, Eq)]
struct VideoStreamInfo {
    index: usize,
    frame_count: u64,
    pix_fmt: String,
}

fn cleanup_temp_output(temp_output: &Path, input: &Path) {
    if let Err(e) = fs::remove_file(temp_output) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                input = %input.display(),
                temp_output = %temp_output.display(),
                error = %e,
                "Failed to remove temporary output"
            );
        }
    }
}

struct AnimatedQualityFailureDecision {
    label: &'static str,
    protect_msg: String,
    delete_msg: String,
    skip_message: String,
    skip_code: &'static str,
}

impl AnimatedQualityFailureDecision {
    fn inspect_and_log(
        input: &Path,
        explore_result: &shared_utils::ExploreResult,
        ultimate_mode: bool,
    ) -> Self {
        let actual_ssim = explore_result.ssim;
        let threshold = explore_result.actual_min_ssim;

        if ultimate_mode {
            let reason = explore_result
                .quality_passed
                .failure_reason()
                .or(explore_result.enhanced_verify_fail_reason.as_deref())
                .unwrap_or("quality/size check failed");
            tracing::warn!(input = %input.display(), reason, "Quality validation failed");
            eprintln!("   ⚠️  Quality validation FAILED: {reason}");
            return Self {
                label: "QUALITY/SIZE VALIDATION FAILED",
                protect_msg: "Original file PROTECTED (quality/size check failed)".to_string(),
                delete_msg: "Output discarded (quality/size check failed)".to_string(),
                skip_message: format!("Skipped: {reason}"),
                skip_code: "quality_failed",
            };
        }

        if actual_ssim.is_none() {
            tracing::warn!(
                input = %input.display(),
                "SSIM calculation failed — cannot validate quality"
            );
            eprintln!(
                "   ⚠️  SSIM CALCULATION FAILED │ cannot validate quality │ may indicate codec compatibility issues"
            );
            return Self {
                label: "SSIM CALCULATION FAILED",
                protect_msg: "Original file PROTECTED (SSIM not available)".to_string(),
                delete_msg: "Output discarded (SSIM calculation failed)".to_string(),
                skip_message: "Skipped: SSIM calculation failed".to_string(),
                skip_code: "quality_failed",
            };
        }

        if actual_ssim.is_some_and(|ssim| ssim < threshold) {
            let actual_ssim = actual_ssim.unwrap_or_default();
            let score_str = explore_result
                .ms_ssim_score
                .map_or_else(|| "Unknown".to_string(), |s| format!("{s:.4}"));
            tracing::warn!(
                input = %input.display(),
                ssim = actual_ssim,
                threshold,
                score = score_str,
                "Quality validation failed"
            );
            eprintln!(
                "   ⚠️  Quality validation FAILED: SSIM {actual_ssim:.4} < {threshold:.4} (Score: {score_str})"
            );
            return Self {
                label: "QUALITY VALIDATION FAILED",
                protect_msg: "Original file PROTECTED (quality below threshold)".to_string(),
                delete_msg: "Output discarded (quality below threshold)".to_string(),
                skip_message: format!(
                    "Skipped: SSIM {actual_ssim:.4} below threshold {threshold:.4}"
                ),
                skip_code: "quality_failed",
            };
        }

        let reason = explore_result
            .quality_passed
            .failure_reason()
            .or(explore_result.enhanced_verify_fail_reason.as_deref())
            .unwrap_or("quality/size check failed");
        tracing::warn!(input = %input.display(), reason, "Quality validation failed");
        eprintln!("   ⚠️  Quality validation FAILED: {reason}");
        Self {
            label: "QUALITY VALIDATION FAILED",
            protect_msg: "Original file PROTECTED (quality/size check failed)".to_string(),
            delete_msg: "Output discarded (quality/size check failed)".to_string(),
            skip_message: format!("Skipped: {reason}"),
            skip_code: "quality_failed",
        }
    }

    fn emit_summary(&self) {
        eprintln!(
            "   ⚠️  {} │ 🛡️  {} │ 🗑️  {}",
            self.label, self.protect_msg, self.delete_msg
        );
    }
}

struct AnimatedFinalGateFailureDecision {
    label: &'static str,
    skip_message: String,
    skip_code: &'static str,
}

impl AnimatedFinalGateFailureDecision {
    fn inspect_and_log(
        input: &Path,
        explore_result: &shared_utils::ExploreResult,
        ultimate_mode: bool,
    ) -> Self {
        let quality_summary = if ultimate_mode {
            explore_result
                .ultimate_quality_summary()
                .unwrap_or_else(|| "3D metrics unavailable".to_string())
        } else {
            explore_result
                .ms_ssim_score
                .map_or_else(|| "Unknown".to_string(), |s| format!("{s:.4}"))
        };
        tracing::warn!(
            input = %input.display(),
            summary = %quality_summary,
            "Final quality gate failed"
        );

        let label = if ultimate_mode {
            "3D QUALITY GATE FAILED"
        } else {
            "QUALITY TARGET FAILED"
        };

        Self {
            label,
            skip_message: if ultimate_mode {
                format!("Skipped: 3D quality gate failed ({quality_summary})")
            } else {
                format!("Skipped: MS-SSIM {quality_summary} below target 0.90")
            },
            skip_code: "quality_gate_failed",
        }
    }

    fn emit_summary(&self) {
        eprintln!(
            "   ⚠️  {} │ 🛡️  Original file PROTECTED │ 🗑️  Output discarded",
            self.label
        );
    }
}

fn probe_video_streams(input: &Path) -> Vec<VideoStreamInfo> {
    let output = match shared_utils::FfprobeBuilder::new()
        .input(input)
        .loglevel("error")
        .print_format("json")
        .show_streams()
        .build()
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    let json: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(json) => json,
        Err(_) => return Vec::new(),
    };

    json.get("streams")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|stream| stream.get("codec_type").and_then(|v| v.as_str()) == Some("video"))
        .map(|stream| VideoStreamInfo {
            index: stream
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(0),
            frame_count: stream
                .get("nb_frames")
                .and_then(|v| v.as_str())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            pix_fmt: stream
                .get("pix_fmt")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase(),
        })
        .collect()
}

fn looks_like_alpha_stream(pix_fmt: &str) -> bool {
    matches!(
        pix_fmt,
        "gray" | "gray8" | "gray10le" | "gray12le" | "gray16le"
    ) || pix_fmt.starts_with("gray")
        || pix_fmt.starts_with("ya")
}

fn is_probable_alpha_aux_pair(streams: &[VideoStreamInfo], selected_stream_index: usize) -> bool {
    if streams.len() != 2 {
        return false;
    }

    let Some(selected_stream) = streams
        .iter()
        .find(|stream| stream.index == selected_stream_index)
    else {
        return false;
    };
    let Some(aux_stream) = streams
        .iter()
        .find(|stream| stream.index != selected_stream_index)
    else {
        return false;
    };

    selected_stream.frame_count > 0
        && selected_stream.frame_count == aux_stream.frame_count
        && looks_like_alpha_stream(&aux_stream.pix_fmt)
}

fn has_probable_avif_alpha_stream(input: &Path) -> bool {
    let streams = probe_video_streams(input);
    let Ok(probe) = shared_utils::probe_video(input) else {
        return false;
    };
    is_probable_alpha_aux_pair(&streams, probe.stream_index)
}

fn extract_frames_for_gifski(
    input: &Path,
    selected_stream_index: Option<usize>,
    verbose: bool,
) -> Result<(tempfile::TempDir, std::path::PathBuf, usize)> {
    let frame_dir = tempfile::Builder::new()
        .prefix("gifski_frames_")
        .tempdir()
        .map_err(|e| {
            VidQualityError::ConversionError(format!("Failed to create frame temp dir: {e}"))
        })?;
    let frame_pattern = frame_dir.path().join("frame_%06d.png");

    let mut builder = shared_utils::FfmpegBuilder::new();
    builder.overwrite().input(input);
    if let Some(stream_index) = selected_stream_index {
        builder.arg("-map").arg(format!("0:{stream_index}"));
    }
    builder
        .arg("-vsync")
        .arg("0")
        .pix_fmt(shared_utils::PixFmt::Rgba)
        .output(&frame_pattern);

    let output = builder.build().output().map_err(|e| {
        tracing::warn!(?input, error = %e, "FFmpeg frame extraction failed");
        VidQualityError::ConversionError(format!("FFmpeg frame extraction failed: {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(VidQualityError::ConversionError(format!(
            "FFmpeg frame extraction failed: {stderr}"
        )));
    }

    let frame_count = fs::read_dir(frame_dir.path())
        .map_err(|e| {
            VidQualityError::ConversionError(format!("Failed to inspect extracted frames: {e}"))
        })?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("png"))
        .count();

    if frame_count < 2 {
        return Err(VidQualityError::ConversionError(format!(
            "Only extracted {frame_count} frame(s) for GIF encoding"
        )));
    }

    if verbose {
        shared_utils::progress_mode::emit_stderr(&format!(
            "   ✅ Extracted {frame_count} frames for GIF encoding"
        ));
    }

    let frame_dir_path = frame_dir.path().to_path_buf();
    Ok((frame_dir, frame_dir_path, frame_count))
}

/// Extract frames from animated WebP using webpmux and create APNG with correct timing
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
fn extract_webp_to_apng(input: &Path, output_apng: &Path, verbose: bool) -> Result<()> {
    use std::fmt::Write;
    // Create temporary directory for frames
    let temp_dir = tempfile::Builder::new()
        .prefix("webp_frames_")
        .tempdir()
        .map_err(|e| VidQualityError::ConversionError(format!("Failed to create temp dir: {e}")))?;
    let temp_dir_path = temp_dir.path();

    // Get WebP info to determine frame count and duration
    let mut builder = shared_utils::WebpmuxBuilder::new();
    builder.input(input).info(true);
    let webpmux_info = builder
        .build()
        .output()
        .map_err(|e| VidQualityError::ConversionError(format!("webpmux not found: {e}")))?;

    if !webpmux_info.status.success() {
        return Err(VidQualityError::ConversionError(
            "webpmux -info failed".to_string(),
        ));
    }

    let info_str = String::from_utf8_lossy(&webpmux_info.stdout);

    // Parse frame count and durations
    let mut frame_count = 0;
    let mut frame_durations_ms = Vec::new();
    let mut parsing_frames = false;

    for line in info_str.lines() {
        if line.contains("Number of frames:") {
            if let Some(count_str) = line.split(':').nth(1) {
                frame_count = count_str.trim().parse::<u32>().unwrap_or(0);
            }
        } else if line.contains("No.: width height") {
            parsing_frames = true;
        } else if parsing_frames {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 7 && parts.first().is_some_and(|p| p.ends_with(':')) {
                if let Some(Ok(duration)) = parts.get(6).map(|p| p.parse::<u32>()) {
                    frame_durations_ms.push(duration);
                }
            }
        }
    }

    if frame_count == 0 || frame_durations_ms.is_empty() {
        return Err(VidQualityError::ConversionError(
            "Failed to parse WebP frame metadata".to_string(),
        ));
    }

    // Fallback if mismatch: pad missing frames with the last parsed delay so
    // the animation keeps local continuity near the tail, rather than copying
    // the first frame's delay across every unparsed frame.
    if u32::try_from(frame_durations_ms.len()).unwrap_or(u32::MAX) != frame_count {
        let pad = *frame_durations_ms.last().unwrap_or(&100);
        frame_durations_ms.resize(usize::try_from(frame_count).unwrap_or(usize::MAX), pad);
    }

    // Guard against degenerate 0-duration WebPs: replace any zero delays with a
    // sane 100ms default so ffmpeg's concat demuxer doesn't produce a 0-length clip.
    for d in &mut frame_durations_ms {
        if *d == 0 {
            *d = 100;
        }
    }

    if verbose {
        let avg_dur = f64::from(frame_durations_ms.iter().sum::<u32>())
            / shared_utils::numeric_cast::usize_to_f64(frame_durations_ms.len());
        eprintln!("   📊 WebP: {frame_count} frames, ~{avg_dur:.1}ms/frame");
    }

    let concat_list_path = temp_dir_path.join("concat.txt");
    let mut concat_content = String::new();

    // Extract each frame using webpmux and convert to PNG
    for i in 1..=frame_count {
        let frame_webp_path = temp_dir_path.join(format!("frame_{i:04}.webp"));
        let frame_png_path = temp_dir_path.join(format!("frame_{i:04}.png"));

        // Extract frame as WebP
        let mut builder = shared_utils::WebpmuxBuilder::new();
        builder.get_frame(i).input(input).output(&frame_webp_path);

        let extract_result = builder.build().output().map_err(|e| {
            VidQualityError::ConversionError(format!("webpmux extract failed: {e}"))
        })?;

        if !extract_result.status.success() {
            return Err(VidQualityError::ConversionError(format!(
                "Failed to extract frame {i}"
            )));
        }

        // Convert WebP frame to PNG using FFmpeg
        let mut builder = shared_utils::FfmpegBuilder::new();
        builder
            .overwrite()
            .with_odd_dim_correction()
            .input(&frame_webp_path)
            .pix_fmt(shared_utils::PixFmt::Rgba)
            .output(&frame_png_path);

        let convert_result = builder.build().output().map_err(|e| {
            VidQualityError::ConversionError(format!("FFmpeg WebP→PNG conversion failed: {e}"))
        })?;

        if !convert_result.status.success() {
            let stderr = String::from_utf8_lossy(&convert_result.stderr);
            return Err(VidQualityError::ConversionError(format!(
                "Failed to convert frame {i} to PNG: {stderr}"
            )));
        }

        // Add to concat list
        let duration_sec = frame_durations_ms
            .get((i - 1) as usize)
            .copied()
            .map_or(0.1, |d| f64::from(d) / 1000.0);
        let _ = writeln!(
            concat_content,
            "file '{}'",
            frame_png_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        let _ = writeln!(concat_content, "duration {duration_sec}");
    }

    // Concat demuxer quirk: the last `duration` directive is ignored, so we repeat
    // the final `file 'X.png'` entry (without a new duration) to force ffmpeg to
    // honour the final frame's delay. Skip this for single-frame WebPs, where adding
    // a duplicate line would create a spurious second frame.
    if frame_durations_ms.len() >= 2 {
        if let Some(last_i) = frame_durations_ms.len().checked_sub(1) {
            use std::fmt::Write;
            let _ = writeln!(concat_content, "file 'frame_{:04}.png'", last_i + 1);
        }
    }

    if let Err(e) = std::fs::write(&concat_list_path, concat_content) {
        return Err(VidQualityError::ConversionError(format!(
            "Failed to write FFmpeg concat list: {e}"
        )));
    }

    // Create APNG from PNG sequence using FFmpeg concat demuxer
    let mut builder = shared_utils::FfmpegBuilder::new();
    builder
        .overwrite()
        .with_odd_dim_correction()
        .input_arg("-f")
        .input_arg("concat")
        .input_arg("-safe")
        .input_arg("0")
        .input(&concat_list_path)
        .pix_fmt(shared_utils::PixFmt::Rgba)
        .vcodec(shared_utils::VideoCodec::Apng)
        .format("apng")
        .arg("-plays")
        .arg("0") // Loop forever
        .output(output_apng);

    let ffmpeg_result = builder.build().output().map_err(|e| {
        VidQualityError::ConversionError(format!("FFmpeg APNG creation failed: {e}"))
    })?;

    if !ffmpeg_result.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_result.stderr);
        return Err(VidQualityError::ConversionError(format!(
            "FFmpeg APNG creation failed: {stderr}"
        )));
    }

    if verbose {
        shared_utils::progress_mode::emit_stderr(&format!(
            "   ✅ WebP → APNG conversion successful ({frame_count} frames, variable delay)"
        ));
    }

    Ok(())
}

fn get_output_path(
    input: &Path,
    extension: &str,
    options: &ConvertOptions,
) -> Result<std::path::PathBuf> {
    options.base_dir.as_ref().map_or_else(
        || {
            shared_utils::conversion::determine_output_path(input, extension, &options.output_dir)
                .map_err(VidQualityError::ConversionError)
        },
        |base| {
            determine_output_path_with_base(input, base, extension, &options.output_dir)
                .map_err(VidQualityError::ConversionError)
        },
    )
}

fn skipped_with_fallback(
    input: &Path,
    options: &ConvertOptions,
    message: &str,
    reason_id: &str,
) -> ConversionResult {
    ConversionResult::skipped_with_fallback(input, options, message, reason_id)
}

fn skipped_with_fallback_owned(
    input: &Path,
    options: &ConvertOptions,
    message: String,
    reason_id: String,
) -> ConversionResult {
    ConversionResult::skipped_with_fallback_owned(input, options, message, reason_id)
}

fn failed_with_fallback(
    input: &Path,
    options: &ConvertOptions,
    message: &str,
    reason_id: &str,
) -> ConversionResult {
    ConversionResult::failed_with_fallback(input, options, message, reason_id)
}

fn failed_with_fallback_owned(
    input: &Path,
    options: &ConvertOptions,
    message: String,
    reason_id: String,
) -> ConversionResult {
    ConversionResult::failed_with_fallback_owned(input, options, message, reason_id)
}

/// Get the dimensions of an input video file.
///
/// # Errors
/// Returns an error if ffprobe fails.
pub fn get_input_dimensions(input: &Path) -> Result<(u32, u32)> {
    shared_utils::conversion::get_input_dimensions(input).map_err(VidQualityError::ConversionError)
}

fn get_max_threads(options: &ConvertOptions) -> usize {
    if options.child_threads > 0 {
        options.child_threads
    } else {
        shared_utils::thread_manager::get_balanced_thread_config(
            shared_utils::thread_manager::WorkloadType::Video,
        )
        .child_threads
    }
}

#[must_use]
pub fn is_high_quality_animated(width: u32, height: u32) -> bool {
    let total_pixels = u64::from(width) * u64::from(height);
    width >= shared_utils::constants::HQ_HD_WIDTH
        || height >= shared_utils::constants::HQ_HD_HEIGHT
        || total_pixels >= shared_utils::constants::HQ_PIX_COUNT_HD
}

fn skipped_already_processed(input: &Path, options: &ConvertOptions) -> ConversionResult {
    shared_utils::ConversionResult::skipped_with_fallback(
        input,
        options,
        "Skipped: Already processed",
        "duplicate",
    )
}

fn skipped_output_exists(input: &Path, output: &Path, _input_size: u64) -> ConversionResult {
    ConversionResult::skipped_exists(input, output)
}

/// Return true when the input is either a native GIF or a GIF-like silent loop
/// video that the scorer says should stay in the GIF domain.
fn assess_loop_intent_for_path(path: &Path) -> Option<shared_utils::LoopIntentVerdict> {
    if shared_utils::should_use_gif_fast_path(path) {
        if let Some(meta) = shared_utils::LoopMeta::from_gif_path(path) {
            return Some(shared_utils::assess_loop_intent_from_meta(
                &meta,
                Some(path),
            ));
        }
    }

    shared_utils::probe_video(path)
        .ok()
        .map(|probe| shared_utils::assess_loop_intent_from_probe(&probe, path))
}

/// Return true when the input is either a native GIF or a GIF-like silent loop
/// video that the scorer says should stay in the GIF domain.
fn is_gif_meme(path: &Path) -> bool {
    assess_loop_intent_for_path(path).is_some_and(|verdict| verdict.is_keep_gif())
}

/// Returns true if the file is an animated image format but effectively static (0 or negligible duration).
/// Callers should skip video conversion and treat as static image (e.g. route to JXL in `img`).
fn is_static_animated_image(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !shared_utils::quality_matcher::parse_source_codec(&ext).can_be_animated() {
        return false;
    }
    if let Ok(analysis) = shared_utils::image_analyzer::analyze_image(path) {
        if let Some(duration_secs) = analysis.duration_secs {
            if duration_secs
                < shared_utils::numeric_cast::f64_to_f32_lossy(
                    shared_utils::constants::NEGLIGIBLE_DURATION_SECS,
                )
            {
                return true;
            }
        }
    }
    false
}

fn skipped_static_animated(input: &Path, options: &ConvertOptions) -> ConversionResult {
    shared_utils::ConversionResult::skipped_with_fallback(
        input,
        options,
        "Skipped: Static image (1 frame), use image conversion path instead",
        "static_animated",
    )
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
/// Convert animated image to MP4 (HEVC or AV1).
///
/// # Errors
/// Returns an error if encoding fails.
/// Convert an animated image (GIF, animated WebP, etc.) to a video container.
///
/// # Errors
///
/// Returns an error if the conversion fails or input is malformed.
pub fn convert_to_mp4(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    use shared_utils::conversion_types::SelectedCodec;
    if !options.force() && is_already_processed(input) {
        return Ok(skipped_already_processed(input, options));
    }

    if is_static_animated_image(input) {
        if options.verbose() {
            eprintln!(
                "   ⏭️  Detected static animated image (1 frame), skipping video conversion: {}",
                input.display()
            );
        }
        return Ok(skipped_static_animated(input, options));
    }

    // GIF / GIF-like video meme-score: if the asset behaves like a looping sticker, keep it
    // in the GIF domain instead of re-encoding to a video container.
    if is_gif_meme(input) {
        return Ok(skipped_with_fallback(
            input,
            options,
            "Skipped: GIF-like asset identified as meme/sticker (meme-score / loop score)",
            "gif_meme",
        ));
    }

    let input_size = fs::metadata(input)?.len();

    let input_ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    let ext = if options.apple_compat() { "MOV" } else { "MP4" };
    let output = get_output_path(input, ext, options)?;

    tracing::debug!(
        input = ?input.file_name().unwrap_or_default(),
        input_ext,
        apple_compat = options.apple_compat(),
        target_ext = %ext,
        "Starting animated image to video conversion"
    );

    if output.exists() && !options.force() {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = shared_utils::conversion::TempOutputGuard::new(temp_output.clone());

    // Special handling for animated JXL: FFmpeg's jpegxl_anim decoder is incomplete
    // and cannot properly decode animated JXL files. We must use djxl to convert to APNG first.
    // Special handling for animated WebP: FFmpeg's WebP decoder is unreliable for animated WebP.
    // We must use webpmux to extract frames and create APNG with correct timing.
    let (actual_input, temp_apng_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
        if input_ext == "jxl" {
            if options.verbose() {
                eprintln!("   🔧 Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)");
            }

            // Check if djxl is available
            if which::which("djxl").is_err() {
                tracing::warn!(input = %input.display(), "djxl not found; cannot process animated JXL");
                return Ok(failed_with_fallback(
                    input,
                    options,
                    "Skipped: djxl not found (required for animated JXL)",
                    "djxl_not_found",
                ));
            }

            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| {
                    VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                })?;
            let temp_apng_path = temp_apng.path().to_path_buf();

            // Convert JXL to APNG using djxl
            let mut builder = shared_utils::DjxlBuilder::new();
            builder.input(input).output(&temp_apng_path);
            let djxl_result = builder.build().output();

            match djxl_result {
                Ok(output) if output.status.success() && temp_apng_path.exists() => {
                    if options.verbose() {
                        shared_utils::progress_mode::emit_stderr(
                            "   ✅ JXL → APNG conversion successful",
                        );
                    }
                    (temp_apng_path, Some(temp_apng))
                }
                _ => {
                    tracing::warn!(input = %input.display(), "djxl conversion failed");
                    return Ok(failed_with_fallback(
                        input,
                        options,
                        "JXL → APNG conversion failed (djxl error)",
                        "djxl_failed",
                    ));
                }
            }
        } else if input_ext == shared_utils::constants::EXT_WEBP {
            if options.verbose() {
                eprintln!("   🔧 Detected WebP format, extracting frames with webpmux");
            }

            // Check if webpmux is available
            if which::which(shared_utils::constants::TOOL_WEBPMUX).is_err() {
                tracing::warn!(input = %input.display(), "webpmux not found");
                return Ok(failed_with_fallback(
                    input,
                    options,
                    "Skipped: webpmux not found (required for animated WebP)",
                    "webpmux_not_found",
                ));
            }

            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| {
                    VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                })?;
            let temp_apng_path = temp_apng.path().to_path_buf();

            // Extract WebP frames and create APNG with correct timing
            match extract_webp_to_apng(input, &temp_apng_path, options.verbose()) {
                Ok(()) => (temp_apng_path, Some(temp_apng)),
                Err(e) => {
                    tracing::warn!(input = %input.display(), error = %e, "WebP extraction failed");
                    return Ok(failed_with_fallback_owned(
                        input,
                        options,
                        format!("WebP extraction failed: {e}"),
                        "webp_extraction_failed".to_string(),
                    ));
                }
            }
        } else if input_ext == shared_utils::constants::EXT_AVIF
            && has_probable_avif_alpha_stream(input)
        {
            if options.verbose() {
                eprintln!("   🔧 Detected AVIF auxiliary alpha stream, pre-converting to APNG");
            }
            let temp_apng = tempfile::Builder::new().suffix(".apng").tempfile()?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            let mut builder = shared_utils::FfmpegBuilder::new();
            builder
                .overwrite()
                .input(input)
                .with_odd_dim_correction()
                .arg("-filter_complex")
                .arg("[0:v:0][0:v:1]alphamerge")
                .arg("-plays")
                .arg("0")
                .vcodec(shared_utils::VideoCodec::Apng)
                .output(&temp_apng_path);

            let res = builder.build().output()?;
            if res.status.success() {
                (temp_apng_path, Some(temp_apng))
            } else {
                (input.to_path_buf(), None)
            }
        } else {
            (input.to_path_buf(), None)
        };

    let (width, height) = get_input_dimensions(&actual_input)?;
    let has_alpha = input_ext == shared_utils::constants::EXT_WEBP
        || input_ext == shared_utils::constants::EXT_GIF
        || input_ext == shared_utils::constants::EXT_JXL
        || (input_ext == shared_utils::constants::EXT_AVIF
            && has_probable_avif_alpha_stream(input))
        || input_ext == shared_utils::constants::EXT_APNG
        || input_ext == shared_utils::constants::EXT_PNG;
    let mut vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, has_alpha);

    let color_info = shared_utils::ffprobe_json::extract_color_info(input);
    let targeted_info =
        shared_utils::hdr_utils::infer_bt709_if_modern(color_info, width, height, &input_ext);
    vf_args.extend(shared_utils::hdr_utils::color_info_to_ffmpeg_args(
        &targeted_info,
    ));

    let max_threads = get_max_threads(options);

    // Set encoder and parameters based on codec
    let (v_codec, v_tag, codec_params_flag, codec_params) = match options.codec {
        SelectedCodec::Hevc => (
            shared_utils::constants::FFMPEG_ENCODER_X265,
            if options.apple_compat() {
                shared_utils::constants::FFMPEG_TAG_HVC1
            } else {
                shared_utils::constants::FFMPEG_TAG_HEV1
            },
            "-x265-params",
            format!("log-level=error:pools={max_threads}"),
        ),
        SelectedCodec::Av1 => (
            "libsvtav1",
            "av01",
            "-svtav1-params",
            format!("tune=0:film-grain=0:lp={max_threads}"),
        ),
        SelectedCodec::Av2 | SelectedCodec::Vvc => {
            return Err(VidQualityError::GeneralError(format!(
                "{} encoding not yet implemented for animated images",
                options.codec.as_str().to_uppercase()
            )));
        }
    };

    // Probe ORIGINAL input to get stream index for multi-stream files (animated AVIF/HEIC)
    // For JXL/WebP, actual_input is APNG (single stream), so we probe the original input
    let stream_idx = if let Ok(probe) = shared_utils::probe_video(input) {
        probe.stream_index
    } else {
        0 // Default to first stream
    };

    // For APNG (converted from JXL/WebP), stream_idx should be 0 since APNG is single-stream
    // For AVIF/HEIC with multiple streams, use the stream_idx from probe
    let effective_stream_idx = if input_ext == "jxl" || input_ext == "webp" {
        0 // APNG is always single-stream
    } else {
        stream_idx
    };

    let mut builder = shared_utils::FfmpegBuilder::new();
    builder
        .overwrite()
        .with_odd_dim_correction()
        .threads(max_threads)
        .input(&actual_input)
        .arg(shared_utils::constants::FFMPEG_ARG_MAP)
        .arg(format!("0:{effective_stream_idx}")) // Select the correct stream
        // NO -r parameter: preserve original frame rate
        .arg(shared_utils::constants::FFMPEG_ARG_CODEC_VIDEO)
        .arg(v_codec)
        .arg(shared_utils::constants::FFMPEG_ARG_CRF)
        .arg("0")
        .arg(shared_utils::constants::FFMPEG_ARG_PRESET)
        .arg(match options.codec {
            SelectedCodec::Hevc => {
                if options.ultimate() {
                    shared_utils::constants::FFMPEG_PRESET_SLOWER
                } else {
                    shared_utils::constants::FFMPEG_PRESET_MEDIUM
                }
            }
            SelectedCodec::Av1 => shared_utils::constants::FFMPEG_SVTAV1_DEFAULT_PRESET,
            SelectedCodec::Av2 | SelectedCodec::Vvc => unreachable!("handled above"),
        })
        .arg(shared_utils::constants::FFMPEG_ARG_TAG_VIDEO)
        .arg(v_tag)
        .arg(codec_params_flag)
        .arg(&codec_params);

    builder.args(&vf_args);

    builder
        .arg("-movflags")
        .arg("+faststart")
        .output(&temp_output);
    let result = builder.build().output();

    // Clean up temporary APNG file if it was created
    drop(temp_apng_file);

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output).map_or(0, |m| m.len());
            if output_size == 0 || get_input_dimensions(&temp_output).is_err() {
                cleanup_temp_output(&temp_output, input);
                let codec_name = options.codec.as_str().to_uppercase();
                tracing::warn!(input = %input.display(), "{} output invalid (empty or unreadable); copying original", codec_name);
                return Ok(failed_with_fallback_owned(
                    input,
                    options,
                    format!("{codec_name} output invalid; original copied"),
                    format!("{}_invalid_output", options.codec.as_str()),
                ));
            }

            if !shared_utils::conversion::commit_temp_to_output_with_metadata(
                &temp_output,
                &output,
                options.force(),
                Some(input),
            )? {
                return Ok(skipped_output_exists(input, &output, input_size));
            }

            shared_utils::copy_metadata(input, &output);
            mark_as_processed(input);

            if options.should_delete_original() {
                if let Err(e) = shared_utils::conversion::safe_delete_original(
                    input,
                    &output,
                    shared_utils::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                ) {
                    tracing::warn!(input = %input.display(), output = %output.display(), error = %e, "Failed to delete original after HEVC conversion");
                }
            }

            let codec_name = options.codec.as_str().to_uppercase();
            Ok(ConversionResult::success(
                input,
                &output,
                input_size,
                output_size,
                &codec_name,
                None,
                options.quality_label.as_deref(),
            ))
        }
        Ok(output_cmd) => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            cleanup_temp_output(&temp_output, input);
            let codec_name = options.codec.as_str().to_uppercase();
            tracing::warn!(input = %input.display(), stderr = %stderr, "ffmpeg {} encode failed; copying original", codec_name);
            Ok(failed_with_fallback_owned(
                input,
                options,
                format!(
                    "{} encode failed; original copied (ffmpeg: {})",
                    codec_name,
                    shared_utils::io_utils::tail_error_lines(&stderr, 5)
                ),
                format!("{}_encode_failed", options.codec.as_str()),
            ))
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            tracing::warn!(input = %input.display(), err = %e, "ffmpeg not found; copying original");
            Ok(failed_with_fallback_owned(
                input,
                options,
                format!("HEVC encode failed (ffmpeg not found: {e}); original copied"),
                "hevc_encode_failed".to_string(),
            ))
        }
    }
}

/// Convert video to MP4 (HEVC or AV1) with matched quality.
///
/// # Errors
/// Returns an error if matching or encoding fails.
pub fn convert_to_mp4_matched(
    input: &Path,
    options: &ConvertOptions,
    initial_crf: f32,
    has_alpha: bool,
) -> Result<ConversionResult> {
    use shared_utils::conversion_types::SelectedCodec;
    if !options.force() && is_already_processed(input) {
        return Ok(skipped_already_processed(input, options));
    }

    if is_static_animated_image(input) {
        if options.verbose() {
            eprintln!(
                "   ⏭️  Detected static animated image (1 frame), skipping video conversion: {}",
                input.display()
            );
        }
        return Ok(skipped_static_animated(input, options));
    }

    // GIF / GIF-like video meme-score: if the asset behaves like a looping sticker, keep it
    // in the GIF domain instead of re-encoding to a video container.
    if is_gif_meme(input) {
        return Ok(skipped_with_fallback(
            input,
            options,
            "Skipped: GIF-like asset identified as meme/sticker (meme-score / loop score)",
            "gif_meme",
        ));
    }

    let input_size = fs::metadata(input)?.len();

    let input_ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    let ext = if options.apple_compat() { "MOV" } else { "MP4" };
    let output = get_output_path(input, ext, options)?;

    if output.exists() && !options.force() {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = shared_utils::conversion::TempOutputGuard::new(temp_output.clone());

    // Special handling for animated JXL/WebP: pre-convert to APNG
    let (actual_input, temp_apng_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
        if input_ext == "jxl" {
            if options.verbose() {
                eprintln!("   🔧 Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)");
            }
            if which::which("djxl").is_err() {
                tracing::warn!(input = %input.display(), "djxl not found; cannot process animated JXL");
                return Ok(failed_with_fallback(
                    input,
                    options,
                    "Skipped: djxl not found (required for animated JXL)",
                    "djxl_not_found",
                ));
            }
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| {
                    VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                })?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            let mut builder = shared_utils::DjxlBuilder::new();
            builder.input(input).output(&temp_apng_path);
            let djxl_result = builder.build().output();
            match djxl_result {
                Ok(output) if output.status.success() && temp_apng_path.exists() => {
                    if options.verbose() {
                        shared_utils::progress_mode::emit_stderr(
                            "   ✅ JXL → APNG conversion successful",
                        );
                    }
                    (temp_apng_path, Some(temp_apng))
                }
                _ => {
                    tracing::warn!(input = %input.display(), "djxl conversion failed");
                    return Ok(failed_with_fallback(
                        input,
                        options,
                        "JXL → APNG conversion failed (djxl error)",
                        "djxl_failed",
                    ));
                }
            }
        } else if input_ext == shared_utils::constants::EXT_WEBP {
            if options.verbose() {
                eprintln!("   🔧 Detected WebP format, extracting frames with webpmux");
            }

            // Check if webpmux is available
            if which::which(shared_utils::constants::TOOL_WEBPMUX).is_err() {
                tracing::warn!(input = %input.display(), "webpmux not found");
                return Ok(failed_with_fallback(
                    input,
                    options,
                    "Skipped: webpmux not found (required for animated WebP)",
                    "webpmux_not_found",
                ));
            }

            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| {
                    VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                })?;
            let temp_apng_path = temp_apng.path().to_path_buf();

            // Extract WebP frames and create APNG with correct timing
            match extract_webp_to_apng(input, &temp_apng_path, options.verbose()) {
                Ok(()) => (temp_apng_path, Some(temp_apng)),
                Err(e) => {
                    tracing::warn!(input = %input.display(), error = %e, "WebP extraction failed");
                    return Ok(failed_with_fallback_owned(
                        input,
                        options,
                        format!("WebP extraction failed: {e}"),
                        "webp_extraction_failed".to_string(),
                    ));
                }
            }
        } else if input_ext == shared_utils::constants::EXT_AVIF && {
            let mut builder = shared_utils::FfprobeBuilder::new();
            builder
                .input(input)
                .select_streams(shared_utils::StreamType::Video)
                .show_entries("stream=index")
                .print_format("csv=p=0");

            let out = builder.build().output();
            out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().count() > 1)
        } {
            if options.verbose() {
                eprintln!("   🔧 Detected transparent AVIF format, pre-converting to APNG to retain alpha explicitly");
            }
            let temp_apng = tempfile::Builder::new().suffix(".apng").tempfile()?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            let mut builder = shared_utils::FfmpegBuilder::new();
            builder
                .overwrite()
                .input(input)
                .with_odd_dim_correction()
                .arg("-filter_complex")
                .arg("[0:v:0][0:v:1]alphamerge")
                .arg("-plays")
                .arg("0")
                .vcodec(shared_utils::VideoCodec::Apng)
                .output(&temp_apng_path);

            let res = builder.build().output()?;
            if res.status.success() {
                (temp_apng_path, Some(temp_apng))
            } else {
                (input.to_path_buf(), None)
            }
        } else {
            (input.to_path_buf(), None)
        };

    // For multi-stream AVIF/HEIC, convert the correct stream to APNG
    // This ensures explore functions work with the correct stream
    let (final_input, temp_stream_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
        if (input_ext == "avif" || input_ext == "heic" || input_ext == "heif")
            && temp_apng_file.is_none()
        {
            if let Ok(probe) = shared_utils::probe_video(input) {
                // Check if there are multiple video streams
                let mut builder = shared_utils::FfprobeBuilder::new();
                builder
                    .input(input)
                    .select_streams(shared_utils::StreamType::Video)
                    .show_entries("stream=index")
                    .print_format("csv=p=0");

                let stream_count_output = builder.build().output();

                let has_multiple_streams = stream_count_output
                    .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().count() > 1);

                if has_multiple_streams && probe.stream_index > 0 {
                    if options.verbose() {
                        eprintln!("   🔧 Multi-stream {} detected, converting stream {} to APNG ({} frames)", 
                            input_ext.to_uppercase(), probe.stream_index, probe.frame_count);
                    }

                    // Create temporary APNG file
                    let temp_stream = tempfile::Builder::new()
                        .suffix(".apng")
                        .tempfile()
                        .map_err(|e| {
                            VidQualityError::ConversionError(format!(
                                "Failed to create temp APNG: {e}"
                            ))
                        })?;
                    let temp_stream_path = temp_stream.path().to_path_buf();

                    // Convert the correct stream to APNG using FFmpeg
                    let mut builder = shared_utils::FfmpegBuilder::new();
                    builder
                        .overwrite()
                        .input(input)
                        .arg("-map")
                        .arg(format!("0:{}", probe.stream_index))
                        .vcodec(shared_utils::VideoCodec::Apng)
                        .format("apng")
                        .arg("-plays")
                        .arg("0")
                        .output(&temp_stream_path);

                    let extract_result = builder.build().output();

                    match extract_result {
                        Ok(output) if output.status.success() && temp_stream_path.exists() => {
                            if options.verbose() {
                                shared_utils::progress_mode::emit_stderr(
                                    "   ✅ Stream → APNG conversion successful",
                                );
                            }
                            (temp_stream_path, Some(temp_stream))
                        }
                        _ => {
                            if options.verbose() {
                                eprintln!("   ⚠️  Stream conversion failed, using original file");
                            }
                            (actual_input, None)
                        }
                    }
                } else {
                    (actual_input, None)
                }
            } else {
                (actual_input, None)
            }
        } else {
            (actual_input, None)
        };

    let (width, height) = get_input_dimensions(&final_input)?;
    let mut vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, has_alpha);

    let color_info = shared_utils::ffprobe_json::extract_color_info(input);
    let targeted_info =
        shared_utils::hdr_utils::infer_bt709_if_modern(color_info, width, height, &input_ext);
    vf_args.extend(shared_utils::hdr_utils::color_info_to_ffmpeg_args(
        &targeted_info,
    ));

    let flag_mode = options
        .flag_mode()
        .map_err(VidQualityError::ConversionError)?;

    let use_gpu = options.use_gpu();
    if !use_gpu && options.verbose() {
        eprintln!("   🖥️  CPU Mode: Using libx265 for higher SSIM (≥0.98)");
    }

    let is_gif = shared_utils::is_gif_magic(&final_input);
    let mut actual_initial_crf = initial_crf;

    // Get duration and metadata for smart CRF initialization
    let probe = shared_utils::ffprobe::probe_video(input).ok();
    let duration = probe.as_ref().map_or(0.0, |p| {
        shared_utils::numeric_cast::f64_to_f32_lossy(p.duration)
    });

    let is_safe_for_lossless = (is_gif && flag_mode.is_ultimate())
        && probe.as_ref().map_or_else(
            || duration < ANIMATION_CLIP_THRESHOLD_SECS,
            |p| {
                let meta = LoopMeta::from_ffprobe_result(p, input);
                is_lossless_exploration_safe(&meta, Some(input))
            },
        );

    if is_safe_for_lossless {
        // [Data-Driven Optimization]
        // Allow long, low-entropy memes to undergo CRF 0.00 probing.
        // High-value artwork still maintains a 30s threshold to prevent overflow.
        actual_initial_crf = 0.0;
    } else if let Some(hint) = shared_utils::crf_constants::get_global_last_hit_crf_hevc() {
        if options.verbose() {
            eprintln!("   💡 Using global last hit CRF: {hint:.1} (warm start)");
        }
        actual_initial_crf = hint;
    }

    if options.verbose() {
        eprintln!(
            "   {} Mode: CRF {:.1} (based on input analysis/cache)",
            flag_mode.description_en(),
            actual_initial_crf
        );
    }

    let explore_result = if flag_mode.is_ultimate() {
        match options.codec {
            SelectedCodec::Hevc => {
                shared_utils::explore_hevc_with_gpu(&shared_utils::GpuSearchRequest {
                    input: final_input,
                    output: temp_output.clone(),
                    vf_args: vf_args.clone(),
                    baseline_crf: actual_initial_crf,
                    warm_start_crf: None,
                    ultimate_mode: true,
                    force_ms_ssim_long: false,
                    allow_size_tolerance: options.allow_size_tolerance(),
                    min_ssim: 0.0, // calculated internally
                    max_threads: options.child_threads,
                    hdr_x265_params: None,
                    apple_compat: options.apple_compat(),
                    preset: shared_utils::EncoderPreset::Slower,
                })
            }
            SelectedCodec::Av1 => {
                shared_utils::explore_av1_with_gpu(&shared_utils::GpuSearchRequest {
                    input: final_input,
                    output: temp_output.clone(),
                    vf_args: vf_args.clone(),
                    baseline_crf: actual_initial_crf,
                    warm_start_crf: None,
                    ultimate_mode: true,
                    force_ms_ssim_long: false,
                    allow_size_tolerance: options.allow_size_tolerance(),
                    min_ssim: 0.0, // calculated internally
                    max_threads: options.child_threads,
                    hdr_x265_params: None,
                    apple_compat: options.apple_compat(),
                    preset: shared_utils::EncoderPreset::Slower,
                })
            }
            SelectedCodec::Av2 | SelectedCodec::Vvc => {
                return Err(VidQualityError::GeneralError(format!(
                    "{} encoding not yet implemented for animated images",
                    options.codec.as_str().to_uppercase()
                )));
            }
        }
    } else {
        match options.codec {
            SelectedCodec::Hevc => {
                shared_utils::explore_hevc_with_gpu(&shared_utils::GpuSearchRequest {
                    input: final_input,
                    output: temp_output.clone(),
                    vf_args: vf_args.clone(),
                    baseline_crf: actual_initial_crf,
                    warm_start_crf: None,
                    ultimate_mode: false,
                    force_ms_ssim_long: false,
                    allow_size_tolerance: options.allow_size_tolerance(),
                    min_ssim: 0.0, // calculated internally
                    max_threads: options.child_threads,
                    hdr_x265_params: None,
                    apple_compat: options.apple_compat(),
                    preset: shared_utils::EncoderPreset::Medium,
                })
            }
            SelectedCodec::Av1 => {
                shared_utils::explore_av1_with_gpu(&shared_utils::GpuSearchRequest {
                    input: final_input,
                    output: temp_output.clone(),
                    vf_args: vf_args.clone(),
                    baseline_crf: actual_initial_crf,
                    warm_start_crf: None,
                    ultimate_mode: false,
                    force_ms_ssim_long: false,
                    allow_size_tolerance: options.allow_size_tolerance(),
                    min_ssim: 0.0, // calculated internally
                    max_threads: options.child_threads,
                    hdr_x265_params: None,
                    apple_compat: options.apple_compat(),
                    preset: shared_utils::EncoderPreset::Medium,
                })
            }
            SelectedCodec::Av2 | SelectedCodec::Vvc => {
                return Err(VidQualityError::GeneralError(format!(
                    "{} encoding not yet implemented for animated images",
                    options.codec.as_str().to_uppercase()
                )));
            }
        }
    }
    .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

    // Clean up temporary files
    drop(temp_apng_file);
    drop(temp_stream_file);

    for log in &explore_result.log {
        eprintln!("{log}");
    }

    let tolerance_ratio = if options.allow_size_tolerance() {
        1.01
    } else {
        1.0
    };
    // We use Rational for precise max size calculation. tolerance_ratio (e.g. 1.05)
    let max_allowed_size = {
        let input_rat = Rational::from(input_size);
        let tol_rat = Rational::from_f64(tolerance_ratio).unwrap_or_else(|| Rational::from(1));
        let res: Rational = input_rat * tol_rat;
        shared_utils::numeric_cast::f64_to_u64_sat(res.to_f64().round())
    };

    // apple_compat mode: compatibility takes priority over file size.
    // However, if the source is already apple-compatible (like GIF/APNG), size guard stays active.
    // For definitive loop assets, compatibility/domain correctness beats size.
    // If loop intent says this should stay in the GIF domain, do not apply the size guard.
    let is_guard_active = shared_utils::is_size_guard_active(&input_ext, options.apple_compat())
        && !is_gif_meme(input);

    if is_guard_active && explore_result.output_size > max_allowed_size {
        let size_increase_pct = {
            let ratio = Rational::from((explore_result.output_size, input_size.max(1)));
            (ratio.to_f64() - 1.0) * 100.0
        };
        let codec_name = options.codec.as_str().to_uppercase();
        if let Err(e) = fs::remove_file(&temp_output) {
            eprintln!("⚠️ [cleanup] Failed to remove oversized {codec_name} output: {e}");
        }
        if options.allow_size_tolerance() {
            eprintln!(
                "   ⏭️  Skipping: {codec_name} output larger than input by {size_increase_pct:.1}% (tolerance: 1.0%)"
            );
        } else {
            eprintln!(
                "   ⏭️  Skipping: {codec_name} output larger than input by {size_increase_pct:.1}% (strict mode: no tolerance)"
            );
        }
        eprintln!(
            "   📊 Size comparison: {} → {} bytes (+{:.1}%)",
            input_size, explore_result.output_size, size_increase_pct
        );
        return Ok(skipped_with_fallback_owned(
            input,
            options,
            format!(
                "Skipped: {codec_name} output larger than input by {size_increase_pct:.1}% ({width}x{height}, tolerance exceeded)"
            ),
            "size_increase_beyond_tolerance".to_string(),
        ));
    }

    // apple_compat: if quality_passed=false only because the file couldn't be compressed
    // (not because of actual quality degradation), still accept the HEVC output.
    // A larger-but-playable HEVC is always better than a non-playable original (e.g. AVIF).
    let quality_or_compat_ok = explore_result.quality_passed.is_passed()
        || (options.apple_compat()
            && !flag_mode.is_ultimate()
            && explore_result.ssim.is_some_and(|s| s >= 0.90));

    if !quality_or_compat_ok {
        let decision = AnimatedQualityFailureDecision::inspect_and_log(
            input,
            &explore_result,
            flag_mode.is_ultimate(),
        );
        decision.emit_summary();

        return Ok(failed_with_fallback_owned(
            input,
            options,
            decision.skip_message,
            decision.skip_code.to_string(),
        ));
    }

    if explore_result.ms_ssim_passed.is_failed() {
        let decision = AnimatedFinalGateFailureDecision::inspect_and_log(
            input,
            &explore_result,
            flag_mode.is_ultimate(),
        );
        decision.emit_summary();

        return Ok(failed_with_fallback_owned(
            input,
            options,
            decision.skip_message,
            decision.skip_code.to_string(),
        ));
    }

    if explore_result.quality_passed.is_passed() && explore_result.optimal_crf > 0.0 {
        match options.codec {
            shared_utils::conversion_types::SelectedCodec::Hevc => {
                shared_utils::crf_constants::update_global_last_hit_crf_hevc(
                    explore_result.optimal_crf,
                );
            }
            shared_utils::conversion_types::SelectedCodec::Av1 => {
                shared_utils::crf_constants::update_global_last_hit_crf_av1(
                    explore_result.optimal_crf,
                );
            }
            shared_utils::conversion_types::SelectedCodec::Av2
            | shared_utils::conversion_types::SelectedCodec::Vvc => {
                // No global CRF hints for experimental codecs yet
            }
        }
    }

    if !shared_utils::conversion::commit_temp_to_output_with_metadata(
        &temp_output,
        &output,
        options.force(),
        Some(input),
    )? {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    shared_utils::copy_metadata(input, &output);
    mark_as_processed(input);

    if options.should_delete_original() {
        if let Err(e) = shared_utils::conversion::safe_delete_original(
            input,
            &output,
            shared_utils::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
        ) {
            let codec_name = options.codec.as_str().to_uppercase();
            tracing::warn!(input = %input.display(), output = %output.display(), error = %e, "Failed to delete original after {} animated conversion", codec_name);
        }
    }

    Ok(ConversionResult::success_video_explored(
        input,
        &output,
        &shared_utils::conversion::VideoExplorationMetrics {
            input_size,
            output_size: explore_result.output_size,
            codec_name: options.codec.as_str(),
            crf: explore_result.optimal_crf,
            is_lossless: explore_result.optimal_crf
                < shared_utils::numeric_cast::f64_to_f32_lossy(
                    shared_utils::constants::NEGLIGIBLE_DURATION_SECS,
                ),
            iterations: explore_result.iterations,
            ssim: explore_result.ssim,
            explored_from_crf: Some(actual_initial_crf),
            quality_label: options.quality_label.as_deref(),
        },
    ))
}

/// Convert to MKV losslessly (HEVC-only for now).
///
/// # Errors
/// Returns an error if encoding fails.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
pub fn convert_to_mkv_lossless(input: &Path, options: &ConvertOptions) -> Result<ConversionResult> {
    eprintln!(
        "⚠️  Mathematical lossless encoding (HEVC) - this will be SLOW and produce large files!"
    );

    if !options.force() && is_already_processed(input) {
        return Ok(skipped_already_processed(input, options));
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mkv", options)?;

    if output.exists() && !options.force() {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = shared_utils::conversion::TempOutputGuard::new(temp_output.clone());

    let (width, height) = get_input_dimensions(input)?;
    let vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, false);

    let max_threads = get_max_threads(options);
    let x265_params = format!("lossless=1:log-level=error:pools={max_threads}");
    let mut builder = shared_utils::FfmpegBuilder::new();
    builder
        .overwrite()
        .threads(max_threads)
        .input(input)
        .vcodec(shared_utils::VideoCodec::Hevc)
        .x265_params(x265_params);

    if options.ultimate() {
        builder.preset(shared_utils::EncoderPreset::Slower);
    } else {
        builder.preset(shared_utils::EncoderPreset::Medium);
    }

    if options.apple_compat() {
        builder.tag_video(shared_utils::constants::FFMPEG_TAG_HVC1);
    }

    for arg in &vf_args {
        builder.arg(arg);
    }

    builder
        .arg("-movflags")
        .arg("+faststart")
        .output(&temp_output);

    let result = builder.build().output();

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if !shared_utils::conversion::commit_temp_to_output_with_metadata(
                &temp_output,
                &output,
                options.force(),
                Some(input),
            )? {
                return Ok(skipped_output_exists(input, &output, input_size));
            }

            shared_utils::copy_metadata(input, &output);
            mark_as_processed(input);

            if options.should_delete_original() {
                if let Err(e) = shared_utils::conversion::safe_delete_original(
                    input,
                    &output,
                    shared_utils::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                ) {
                    tracing::warn!(input = %input.display(), output = %output.display(), error = %e, "Failed to delete original after lossless HEVC conversion");
                }
            }

            Ok(ConversionResult::success(
                input,
                &output,
                input_size,
                output_size,
                "Lossless",
                None,
                options.quality_label.as_deref(),
            ))
        }
        Ok(output_cmd) => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            cleanup_temp_output(&temp_output, input);
            tracing::warn!(input = %input.display(), stderr = %stderr, "ffmpeg lossless failed; copying original");
            Ok(failed_with_fallback_owned(
                input,
                options,
                format!(
                    "Lossless failed; original copied ({})",
                    shared_utils::io_utils::tail_error_lines(&stderr, 5)
                ),
                "lossless_failed".to_string(),
            ))
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            tracing::warn!(input = %input.display(), err = %e, "ffmpeg not found for lossless; copying original");
            Ok(failed_with_fallback_owned(
                input,
                options,
                format!("Lossless failed (ffmpeg not found: {e}); original copied"),
                "lossless_failed".to_string(),
            ))
        }
    }
}

/// Convert to GIF with Apple compatibility.
///
/// # Errors
/// Returns an error if encoding fails.
pub fn convert_to_gif_apple_compat(
    input: &Path,
    options: &ConvertOptions,
) -> Result<ConversionResult> {
    if !options.force() && is_already_processed(input) {
        return Ok(skipped_already_processed(input, options));
    }

    if is_static_animated_image(input) {
        if options.verbose() {
            eprintln!(
                "   ⏭️  Detected static animated image (1 frame), skipping video conversion: {}",
                input.display()
            );
        }
        return Ok(skipped_static_animated(input, options));
    }

    let input_size = fs::metadata(input)?.len();

    let input_ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    if input_ext == "gif" {
        eprintln!("   ⏭️  Input is already GIF, skipping re-encode (would likely increase size)");
        return Ok(skipped_with_fallback(
            input,
            options,
            "Skipped: Already GIF (re-encoding would increase size)",
            "already_gif",
        ));
    }

    let output = get_output_path(input, "GIF", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force() {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = shared_utils::conversion::TempOutputGuard::new(temp_output.clone());

    // Special handling for animated JXL: FFmpeg's jpegxl_anim decoder is incomplete
    // and cannot properly decode animated JXL files. We must use djxl to convert to APNG first.
    // Special handling for animated WebP: FFmpeg's WebP decoder is unreliable for animated WebP.
    // We must use webpmux to extract frames and create APNG with correct timing.
    let (actual_input, temp_apng_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
        if input_ext == "jxl" {
            if options.verbose() {
                eprintln!("   🔧 Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)");
            }

            // Check if djxl is available
            if which::which("djxl").is_err() {
                tracing::warn!(input = %input.display(), "djxl not found; cannot process animated JXL");
                return Ok(failed_with_fallback(
                    input,
                    options,
                    "Skipped: djxl not found (required for animated JXL)",
                    "djxl_not_found",
                ));
            }

            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| {
                    VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                })?;
            let temp_apng_path = temp_apng.path().to_path_buf();

            // Convert JXL to APNG using djxl
            let djxl_result = shared_utils::DjxlBuilder::new()
                .input(input)
                .output(&temp_apng_path)
                .build()
                .output();

            match djxl_result {
                Ok(output) if output.status.success() && temp_apng_path.exists() => {
                    if options.verbose() {
                        shared_utils::progress_mode::emit_stderr(
                            "   ✅ JXL → APNG conversion successful",
                        );
                    }
                    (temp_apng_path, Some(temp_apng))
                }
                _ => {
                    tracing::warn!(input = %input.display(), "djxl conversion failed");
                    return Ok(failed_with_fallback(
                        input,
                        options,
                        "JXL → APNG conversion failed (djxl error)",
                        "djxl_failed",
                    ));
                }
            }
        } else if input_ext == shared_utils::constants::EXT_WEBP {
            if options.verbose() {
                eprintln!("   🔧 Detected WebP format, extracting frames with webpmux");
            }

            // Check if webpmux is available
            if which::which(shared_utils::constants::TOOL_WEBPMUX).is_err() {
                tracing::warn!(input = %input.display(), "webpmux not found; cannot process animated WebP");
                return Ok(failed_with_fallback(
                    input,
                    options,
                    "Skipped: webpmux not found (required for animated WebP)",
                    "webpmux_not_found",
                ));
            }

            // Create temporary APNG file
            let temp_apng = tempfile::Builder::new()
                .suffix(".apng")
                .tempfile()
                .map_err(|e| {
                    VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                })?;
            let temp_apng_path = temp_apng.path().to_path_buf();

            // Extract WebP frames and create APNG with correct timing
            match extract_webp_to_apng(input, &temp_apng_path, options.verbose()) {
                Ok(()) => (temp_apng_path, Some(temp_apng)),
                Err(e) => {
                    tracing::warn!(input = %input.display(), error = %e, "WebP extraction failed");
                    return Ok(failed_with_fallback_owned(
                        input,
                        options,
                        format!("WebP extraction failed: {e}"),
                        "webp_extraction_failed".to_string(),
                    ));
                }
            }
        } else if input_ext == "avif" && has_probable_avif_alpha_stream(input) {
            if options.verbose() {
                eprintln!("   🔧 Detected AVIF auxiliary alpha stream, pre-converting to APNG");
            }
            let temp_apng = tempfile::Builder::new().suffix(".apng").tempfile()?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            let mut builder = shared_utils::FfmpegBuilder::new();
            builder
                .overwrite()
                .input(input)
                .with_odd_dim_correction()
                .arg("-filter_complex")
                .arg("[0:v:0][0:v:1]alphamerge")
                .arg("-plays")
                .arg("0")
                .pix_fmt(shared_utils::PixFmt::Rgba)
                .vcodec(shared_utils::VideoCodec::Apng)
                .output(&temp_apng_path);

            let res = builder.build().output()?;
            if res.status.success() {
                (temp_apng_path, Some(temp_apng))
            } else {
                (input.to_path_buf(), None)
            }
        } else {
            (input.to_path_buf(), None)
        };

    let (width, height) = get_input_dimensions(&actual_input)?;

    // Probe ORIGINAL input to get stream index for multi-stream files (animated AVIF/HEIC)
    // For JXL/WebP, actual_input is APNG (single stream), so we probe the original input
    let stream_idx = if let Ok(probe) = shared_utils::probe_video(input) {
        probe.stream_index
    } else {
        0
    };

    // For APNG (converted from JXL/WebP), stream_idx should be 0 since APNG is single-stream
    // For AVIF/HEIC with multiple streams, use the stream_idx from probe
    let effective_stream_idx = if input_ext == "jxl" || input_ext == "webp" {
        0 // APNG is always single-stream
    } else {
        stream_idx
    };

    let has_multiple_streams = probe_video_streams(&actual_input).len() > 1;
    let frame_stream_index =
        if input_ext == "jxl" || input_ext == "webp" || temp_apng_file.is_some() {
            None
        } else if has_multiple_streams && effective_stream_idx != 0 {
            Some(effective_stream_idx)
        } else {
            None
        };

    let gifski_ok = if which::which("gifski").is_err() {
        false
    } else {
        let (gifski_frames_dir, gifski_frames_path, extracted_count) =
            match extract_frames_for_gifski(&actual_input, frame_stream_index, options.verbose()) {
                Ok(value) => value,
                Err(e) => {
                    tracing::error!(
                        input = %input.display(),
                        error = %e,
                        "Failed to extract frames for GIF conversion"
                    );
                    return Ok(failed_with_fallback_owned(
                        input,
                        options,
                        format!("GIF frame extraction failed: {e}"),
                        "gif_frame_extraction_failed".to_string(),
                    ));
                }
            };

        let probe_res = shared_utils::probe_video(input).map_err(|e| {
            VidQualityError::ConversionError(format!("Failed to probe source for FPS: {e}"))
        })?;

        let fps = if probe_res.duration > 0.0 && extracted_count > 0 {
            // 100% data-driven: Actual extracted frames / Metadata total duration
            shared_utils::numeric_cast::usize_to_f64(extracted_count) / probe_res.duration
        } else if probe_res.avg_frame_rate > 0.0 {
            // Use directly reported average frame rate
            probe_res.avg_frame_rate
        } else if probe_res.frame_rate > 0.0 {
            // Use directly reported r_frame_rate
            probe_res.frame_rate
        } else {
            return Err(VidQualityError::ConversionError(
                "Source metadata lacks both duration and frame rate - cannot determine native speed".to_string()
             ));
        };

        if options.verbose() {
            eprintln!("   🔧 GIF Encoding: Native speed ({} frames / {:.2}s duration) -> target speed: {:.3} FPS", 
                extracted_count, probe_res.duration, fps);
        }
        let mut gifski_builder = shared_utils::GifskiBuilder::new();
        gifski_builder
            .output(&temp_output)
            .fps(shared_utils::numeric_cast::f64_to_f32_lossy(fps))
            .dimensions(width, height)
            .quality(100)
            .motion_quality(100)
            .lossy_quality(100)
            .repeat(0)
            .arg("--extra");

        // Collect and sort extracted PNG frames to ensure correct sequence
        let mut frames: Vec<std::path::PathBuf> = std::fs::read_dir(&gifski_frames_path)
            .map_err(|e| {
                VidQualityError::ConversionError(format!("Failed to read frame directory: {e}"))
            })?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "png"))
            .collect();
        frames.sort();

        for frame in frames {
            gifski_builder.add_input(frame);
        }

        let res = gifski_builder.build().output();

        drop(gifski_frames_dir);
        match res {
            Ok(o) if o.status.success() && temp_output.exists() => true,
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::error!(
                    input = %input.display(),
                    stderr = %stderr.trim(),
                    "gifski conversion failed with status: {:?}",
                    o.status.code()
                );
                false
            }
            Err(e) => {
                tracing::error!(input = %input.display(), error = %e, "gifski command failed to start");
                false
            }
        }
    };

    // Clean up temporary APNG file if it was created
    drop(temp_apng_file);

    if !gifski_ok {
        // gifski conversion failed — copy original so data is not lost
        cleanup_temp_output(&temp_output, input);
        tracing::warn!(input = %input.display(), "GIF conversion failed (gifski unavailable or failed); copying original");
        return Ok(failed_with_fallback(
            input,
            options,
            "GIF conversion failed (gifski unavailable or failed); original copied",
            "gif_encode_failed",
        ));
    }

    // Validate output
    let output_size = fs::metadata(&temp_output).map_or(0, |m| m.len());
    if output_size == 0 || get_input_dimensions(&temp_output).is_err() {
        cleanup_temp_output(&temp_output, input);
        tracing::warn!(input = %input.display(), "GIF output invalid (empty or unreadable); copying original");
        return Ok(failed_with_fallback(
            input,
            options,
            "GIF output invalid; original copied",
            "gif_invalid_output",
        ));
    }

    let tolerance_ratio = if options.allow_size_tolerance() {
        1.01
    } else {
        1.0
    };
    let max_allowed_size = {
        let input_rat = Rational::from(input_size);
        let tol_rat = Rational::from_f64(tolerance_ratio).unwrap_or_else(|| Rational::from(1));
        let res: Rational = input_rat * tol_rat;
        shared_utils::numeric_cast::f64_to_u64_sat(res.to_f64().round())
    };

    // apple_compat: compatibility takes priority — a playable GIF is always
    // better than a non-playable original (e.g. animated AVIF).
    // But if the source is already playable (like APNG or GIF), size guard stays active.
    let is_guard_active = shared_utils::is_size_guard_active(&input_ext, options.apple_compat());

    if is_guard_active && output_size > max_allowed_size {
        let size_increase_pct = {
            let ratio = Rational::from((output_size, input_size.max(1)));
            (ratio.to_f64() - 1.0) * 100.0
        };
        if let Err(e) = fs::remove_file(&temp_output) {
            eprintln!("⚠️ [cleanup] Failed to remove oversized GIF output: {e}");
        }
        if options.allow_size_tolerance() {
            eprintln!(
                "   ⏭️  Skipping: GIF output larger than input by {size_increase_pct:.1}% (tolerance: 1.0%)"
            );
        } else {
            eprintln!(
                "   ⏭️  Skipping: GIF output larger than input by {size_increase_pct:.1}% (strict mode: no tolerance)"
            );
        }
        eprintln!(
            "   📊 Size comparison: {input_size} → {output_size} bytes (+{size_increase_pct:.1}%)"
        );
        return Ok(skipped_with_fallback_owned(
            input,
            options,
            format!(
                "Skipped: GIF output larger than input by {size_increase_pct:.1}% (tolerance exceeded)"
            ),
            "size_increase_beyond_tolerance".to_string(),
        ));
    }

    if !shared_utils::conversion::commit_temp_to_output_with_metadata(
        &temp_output,
        &output,
        options.force(),
        Some(input),
    )? {
        return Ok(skipped_output_exists(input, &output, input_size));
    }

    shared_utils::copy_metadata(input, &output);
    mark_as_processed(input);

    if options.should_delete_original() {
        if let Err(e) = shared_utils::conversion::safe_delete_original(
            input,
            &output,
            shared_utils::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
        ) {
            tracing::warn!(input = %input.display(), output = %output.display(), error = %e, "Failed to delete original after GIF apple-compat HEVC conversion");
        }
    }

    Ok(ConversionResult::success(
        input,
        &output,
        input_size,
        output_size,
        "GIF",
        Some("Apple Compat"),
        options.quality_label.as_deref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::Builder;

    #[test]
    fn test_alpha_aux_detection_rejects_poster_plus_animation_avif() {
        let streams = vec![
            VideoStreamInfo {
                index: 0,
                frame_count: 1,
                pix_fmt: "yuv420p".to_string(),
            },
            VideoStreamInfo {
                index: 1,
                frame_count: 11,
                pix_fmt: "yuv420p".to_string(),
            },
        ];

        assert!(!is_probable_alpha_aux_pair(&streams, 1));
    }

    #[test]
    fn test_alpha_aux_detection_accepts_matching_gray_aux_stream() {
        let streams = vec![
            VideoStreamInfo {
                index: 0,
                frame_count: 24,
                pix_fmt: "yuv420p".to_string(),
            },
            VideoStreamInfo {
                index: 1,
                frame_count: 24,
                pix_fmt: "gray8".to_string(),
            },
        ];

        assert!(is_probable_alpha_aux_pair(&streams, 0));
    }

    #[test]
    fn test_apple_compat_blocks_copying_incompatible_originals() {
        let mut options = ConvertOptions::default();
        options
            .flags
            .set(shared_utils::conversion::ConvertFlags::APPLE_COMPAT, true);

        assert!(!options.should_copy_original_on_skip(Path::new("/tmp/test.avif")));
        assert!(!options.should_copy_original_on_skip(Path::new("/tmp/test.webp")));
        assert!(options.should_copy_original_on_skip(Path::new("/tmp/test.gif")));
        assert!(options.should_copy_original_on_skip(Path::new("/tmp/test.heic")));
    }

    #[test]
    fn test_animated_quality_failure_prefers_total_size_reason_over_stream_growth() {
        let decision = AnimatedQualityFailureDecision::inspect_and_log(
            Path::new("/tmp/test.gif"),
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
            decision.skip_message,
            "Skipped: Total file not smaller than input"
        );
        assert_eq!(decision.label, "QUALITY VALIDATION FAILED");
        assert!(!decision.skip_message.contains("video stream"));
    }

    #[test]
    fn test_short_gif_loop_intent_uses_gif_header_fast_path() {
        // Keep the payload intentionally tiny: loop intent should come from the GIF header scan,
        // not from a fragile ffprobe-only path.
        let gif_data: &[u8] = &[
            b'G', b'I', b'F', b'8', b'9', b'a', // Header
            0x01, 0x00, 0x01, 0x00, // Logical screen: 1x1
            0x80, 0x00, 0x00, // Global color table, background, aspect
            0x00, 0x00, 0x00, // Color #0
            0xFF, 0xFF, 0xFF, // Color #1
            0x21, 0xFF, 0x0B, // App extension introducer
            b'N', b'E', b'T', b'S', b'C', b'A', b'P', b'E', b'2', b'.', b'0', 0x03, 0x01, 0x00,
            0x00, 0x00, // Infinite loop
            0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00, // Frame 1 GCE, 100 ms
            0x2C, // Frame 1 image descriptor
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x01, 0x00,
            0x00, // Minimal image data block
            0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00, // Frame 2 GCE, 100 ms
            0x2C, // Frame 2 image descriptor
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x01, 0x00,
            0x00, // Minimal image data block
            0x3B, // Trailer
        ];

        let mut file = Builder::new()
            .suffix(".gif")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        file.write_all(gif_data)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let verdict = assess_loop_intent_for_path(file.path())
            .unwrap_or_else(|| panic!("short GIF should produce a loop-intent verdict"));

        assert!(
            verdict.is_keep_gif(),
            "expected short looping GIF to stay in GIF domain, got {verdict:?}"
        );
        assert!(is_gif_meme(file.path()));
    }
}
