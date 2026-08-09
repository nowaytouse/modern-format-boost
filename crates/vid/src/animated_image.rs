//! - `vid`: all video encoding (including animated image → video)

use crate::{Result, VidQualityError};
use foundation::ToolBuilder;
use foundation::conversion::{ConvertOptions, TaskResult};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use foundation::constants::ANIMATION_CLIP_THRESHOLD_SECS;
use foundation::conversion::{
    determine_output_path_with_base, is_already_processed, mark_as_processed,
};
use foundation::loop_intent::{LoopMeta, is_lossless_exploration_safe};
use foundation::{log_detail, log_info};

fn run_animated_process(mut command: Command) -> std::io::Result<Output> {
    foundation::process_runner::run_command_with_liveness_timeout(
        &mut command,
        foundation::ffmpeg_process::ffmpeg_timeout(),
        foundation::process_runner::animated_image_process_hard_timeout(),
        "animated media subprocess",
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VideoStreamInfo {
    index: usize,
    frame_count: Option<u64>,
    pix_fmt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AvifAnimationEncoder {
    SvtAv1,
    LibAomAv1,
}

impl AvifAnimationEncoder {
    const fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::SvtAv1 => "libsvtav1",
            Self::LibAomAv1 => "libaom-av1",
        }
    }

    fn append_quality_args(self, builder: &mut foundation::FfmpegBuilder) {
        match self {
            Self::SvtAv1 => {
                builder
                    .arg(foundation::constants::FFMPEG_ARG_CRF)
                    .arg("18")
                    .arg(foundation::constants::FFMPEG_ARG_PRESET)
                    .arg(foundation::constants::FFMPEG_SVTAV1_SLOWEST_PRESET);
            }
            Self::LibAomAv1 => {
                builder
                    .arg(foundation::constants::FFMPEG_ARG_CRF)
                    .arg("18")
                    .arg("-b:v")
                    .arg("0")
                    .arg("-cpu-used")
                    .arg("0")
                    .arg("-row-mt")
                    .arg("1");
            }
        }
    }
}

fn ffmpeg_listing_has_token(listing: &str, token: &str) -> bool {
    listing
        .lines()
        .any(|line| line.split_whitespace().any(|part| part == token))
}

fn select_avif_animation_encoder_from_listing(listing: &str) -> Option<AvifAnimationEncoder> {
    if ffmpeg_listing_has_token(listing, AvifAnimationEncoder::SvtAv1.ffmpeg_name()) {
        Some(AvifAnimationEncoder::SvtAv1)
    } else if ffmpeg_listing_has_token(listing, AvifAnimationEncoder::LibAomAv1.ffmpeg_name()) {
        Some(AvifAnimationEncoder::LibAomAv1)
    } else {
        None
    }
}

fn avif_muxer_available_from_listing(listing: &str) -> bool {
    ffmpeg_listing_has_token(listing, "avif")
}

fn parse_avifdec_sequence_frame_count(info: &str) -> std::result::Result<Option<u64>, String> {
    for line in info.lines() {
        let line = line.trim();
        if let Some(start) = line.find("Image Sequence Frames: (") {
            let after_start = &line[start + "Image Sequence Frames: (".len()..];
            if let Some(end) = after_start.find(" expected frames") {
                let frames = after_start[..end].trim().parse::<u64>().map_err(|error| {
                    format!("invalid avifdec image sequence frame count: {error}")
                })?;
                return Ok(Some(frames));
            }
        }
    }

    for line in info.lines() {
        let line = line.trim();
        if !line.contains("timescales") {
            continue;
        }
        let parts: Vec<_> = line.split_whitespace().collect();
        for pair in parts.windows(2) {
            if pair[1] == "frames" {
                let count = pair[0]
                    .parse::<u64>()
                    .map_err(|error| format!("invalid avifdec timescale frame count: {error}"))?;
                return Ok(Some(count));
            }
        }
    }

    Ok(None)
}

fn animated_avif_sequence_frame_count(path: &Path) -> Result<u64> {
    let tool = foundation::common_utils::resolve_tool_path(foundation::constants::TOOL_AVIFDEC)
        .ok_or_else(|| {
            VidQualityError::ConversionError(
                "avifdec is required to verify animated AVIF meme output".to_string(),
            )
        })?;
    let mut command = Command::new(tool);
    command.arg("--info").arg(path);
    let output = run_animated_process(command).map_err(|err| {
        VidQualityError::ConversionError(format!(
            "avifdec --info failed to start for {}: {err}",
            path.display()
        ))
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let info = format!("{stdout}\n{stderr}");
    if !output.status.success() {
        let stderr_tail = foundation::io_utils::tail_error_lines(&stderr, 5);
        return Err(VidQualityError::ConversionError(format!(
            "avifdec --info failed for {}: {stderr_tail}",
            path.display()
        )));
    }
    let frame_count = parse_avifdec_sequence_frame_count(&info).map_err(|error| {
        VidQualityError::ConversionError(format!(
            "avifdec --info reported an invalid frame count for {}: {error}",
            path.display()
        ))
    })?;
    frame_count.ok_or_else(|| {
        VidQualityError::ConversionError(format!(
            "avifdec --info did not report image sequence frames for {}",
            path.display()
        ))
    })
}

fn validate_animated_avif_output(path: &Path) -> Result<u64> {
    let output_size = fs::metadata(path)
        .map_err(|err| {
            VidQualityError::ConversionError(format!(
                "Failed to read animated AVIF output metadata for {}: {err}",
                path.display()
            ))
        })?
        .len();
    let detected_format =
        foundation::image::format_detect::detect_true_format(path).map_err(|err| {
            VidQualityError::ConversionError(format!(
                "Animated AVIF output format detection failed for {}: {err}",
                path.display()
            ))
        })?;
    if output_size == 0
        || detected_format != foundation::image::format_detect::FormatKind::Avif
        || get_input_dimensions(path).is_err()
        || !matches!(animated_avif_sequence_frame_count(path), Ok(count) if count > 1)
    {
        return Err(VidQualityError::ConversionError(format!(
            "animated AVIF validation failed for {}",
            path.display()
        )));
    }
    Ok(output_size)
}

fn encode_animated_avif_with_ffmpeg(
    actual_input: &Path,
    temp_output: &Path,
    effective_stream_idx: usize,
    max_threads: usize,
    vf_args: &[String],
    encoder: AvifAnimationEncoder,
) -> std::io::Result<std::process::Output> {
    let mut builder = foundation::FfmpegBuilder::new();
    builder
        .overwrite()
        .loglevel("error")
        .threads(max_threads)
        .input(actual_input)
        .arg(foundation::constants::FFMPEG_ARG_MAP)
        .arg(format!("0:{effective_stream_idx}"))
        .arg(foundation::constants::FFMPEG_ARG_NO_AUDIO)
        .arg("-sn")
        .arg(foundation::constants::FFMPEG_ARG_CODEC_VIDEO)
        .arg(encoder.ffmpeg_name());
    encoder.append_quality_args(&mut builder);
    for arg in vf_args {
        builder.arg(arg);
    }
    builder.format("avif").output(temp_output);
    run_animated_process(builder.build())
}

fn encode_animated_avif_with_avifenc(
    actual_input: &Path,
    temp_y4m: &Path,
    temp_output: &Path,
    effective_stream_idx: usize,
    max_threads: usize,
    vf_args: &[String],
    avifenc: &Path,
) -> std::io::Result<std::process::Output> {
    let mut raster = foundation::FfmpegBuilder::new();
    raster
        .overwrite()
        .loglevel("error")
        .threads(max_threads)
        .input(actual_input)
        .arg(foundation::constants::FFMPEG_ARG_MAP)
        .arg(format!("0:{effective_stream_idx}"))
        .arg(foundation::constants::FFMPEG_ARG_NO_AUDIO)
        .arg("-sn");
    for arg in vf_args {
        raster.arg(arg);
    }
    raster
        .pix_fmt_str("yuv420p")
        .format("yuv4mpegpipe")
        .output(temp_y4m);
    let raster_output = run_animated_process(raster.build())?;
    if !raster_output.status.success() {
        return Ok(raster_output);
    }

    let mut command = Command::new(avifenc);
    command
        .arg("--speed")
        .arg("0")
        .arg("--jobs")
        .arg("all")
        .arg("-q")
        .arg("100")
        .arg(temp_y4m)
        .arg(temp_output);
    run_animated_process(command)
}

fn ffmpeg_muxers_listing() -> Result<String> {
    let mut command = foundation::FfmpegBuilder::new().get_resolved_command();
    command
        .arg(foundation::constants::FFMPEG_ARG_HIDE_BANNER)
        .arg("-muxers");
    let output = run_animated_process(command).map_err(|err| {
        VidQualityError::ConversionError(format!("ffmpeg -muxers failed to start: {err}"))
    })?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let summary = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(5)
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(VidQualityError::ConversionError(format!(
            "ffmpeg -muxers failed{}",
            if summary.is_empty() {
                format!(" with status {}", output.status)
            } else {
                format!(": {summary}")
            }
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn ensure_avif_animation_ffmpeg_support() -> Result<AvifAnimationEncoder> {
    let muxers = ffmpeg_muxers_listing()?;
    if !avif_muxer_available_from_listing(&muxers) {
        return Err(VidQualityError::ConversionError(
            "ffmpeg does not expose the AVIF muxer required for animated AVIF meme mode"
                .to_string(),
        ));
    }

    let encoders = foundation::FfmpegBuilder::list_encoders().map_err(|err| {
        VidQualityError::ConversionError(format!("ffmpeg -encoders failed: {err}"))
    })?;
    select_avif_animation_encoder_from_listing(&encoders).ok_or_else(|| {
        VidQualityError::ConversionError(
            "ffmpeg does not expose libsvtav1 or libaom-av1 for animated AVIF meme mode"
                .to_string(),
        )
    })
}

fn cleanup_temp_output(temp_output: &Path, _input: &Path) {
    if let Err(e) = fs::remove_file(temp_output)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        foundation::media_conversion_gate::delivery_cleanup_audit(
            temp_output,
            "animated temp output",
            e,
        );
    }
}

fn required_tool_available(name: &str) -> bool {
    foundation::common_utils::resolve_tool_path(name).is_some()
}

struct AnimatedGateRejectionDecision {
    failed: bool,
    label: &'static str,
    protect_msg: String,
    delete_msg: String,
    message: String,
    reason_code: &'static str,
}

impl AnimatedGateRejectionDecision {
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
                label: foundation::infra::static_logs::messages::LABEL_QUALITY_SIZE_FAIL,
                protect_msg: foundation::infra::static_logs::messages::PROTECT_QUALITY_SIZE
                    .to_string(),
                delete_msg: foundation::infra::static_logs::messages::DISCARD_QUALITY_SIZE
                    .to_string(),
                message: format!("Failed: {reason}"),
                reason_code: "quality_failed",
            };
        }

        if actual_ssim.is_none() {
            foundation::media_conversion_gate::explore_quality_gate_audit(
                "explore_quality_ssim_missing",
                input,
                format!(
                    "{} │ cannot validate quality │ may indicate codec compatibility issues",
                    foundation::infra::static_logs::messages::SSIM_CALC_FAILED
                ),
            );
            return Self {
                failed: true,
                label: foundation::infra::static_logs::messages::LABEL_SSIM_CALC_FAILED,
                protect_msg: foundation::infra::static_logs::messages::PROTECT_SSIM_NA.to_string(),
                delete_msg: foundation::infra::static_logs::messages::DISCARD_SSIM_FAIL.to_string(),
                message: format!(
                    "Failed: {}",
                    foundation::infra::static_logs::messages::SSIM_CALC_FAILED
                ),
                reason_code: "quality_failed",
            };
        }

        if let Some(ssim) = actual_ssim
            && ssim < threshold
        {
            let score_str = foundation::media_conversion_gate::explore_ms_ssim_score_display(
                explore_result.ms_ssim_score,
                &format!("animated quality fail {}", input.display()),
            );
            foundation::media_conversion_gate::explore_quality_gate_audit(
                "explore_quality_ssim_low",
                input,
                format!("SSIM {ssim:.4} < {threshold:.4} (Score: {score_str})"),
            );
            return Self {
                failed: true,
                label: foundation::infra::static_logs::messages::LABEL_QUALITY_FAIL,
                protect_msg: foundation::infra::static_logs::messages::PROTECT_QUALITY_LOW
                    .to_string(),
                delete_msg: foundation::infra::static_logs::messages::DISCARD_QUALITY_LOW
                    .to_string(),
                message: format!("Failed: SSIM {ssim:.4} below threshold {threshold:.4}"),
                reason_code: "quality_failed",
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
            label: foundation::infra::static_logs::messages::LABEL_QUALITY_FAIL,
            protect_msg: foundation::infra::static_logs::messages::PROTECT_QUALITY_SIZE.to_string(),
            delete_msg: foundation::infra::static_logs::messages::DISCARD_QUALITY_SIZE.to_string(),
            message: format!("Skipped: {reason}"),
            reason_code: "size_gate",
        }
    }

    fn emit_summary(&self) {
        foundation::media_conversion_gate::explore_quality_skip_summary_audit(
            self.label,
            &self.protect_msg,
            &self.delete_msg,
        );
    }
}

struct AnimatedFinalGateFailureDecision {
    label: &'static str,
    message: String,
    reason_code: &'static str,
}

impl AnimatedFinalGateFailureDecision {
    fn inspect_and_log(input: &Path, explore_result: &foundation::ExploreResult) -> Self {
        let ultimate_contract = explore_result.uses_ultimate_quality_contract();
        let quality_summary = if ultimate_contract {
            foundation::media_conversion_gate::explore_ultimate_summary_display(
                explore_result.ultimate_quality_summary(),
                &format!("animated final gate {}", input.display()),
            )
        } else {
            foundation::media_conversion_gate::explore_ms_ssim_score_display(
                explore_result.ms_ssim_score,
                &format!("animated final gate {}", input.display()),
            )
        };

        foundation::media_conversion_gate::explore_quality_gate_audit(
            "explore_quality_final_gate",
            input,
            format!(
                "{} summary: {quality_summary}",
                foundation::infra::static_logs::messages::QUALITY_GATE_FAILED
            ),
        );

        let label = if ultimate_contract {
            foundation::infra::static_logs::messages::LABEL_3D_QUALITY_GATE_FAILED
        } else {
            foundation::infra::static_logs::messages::LABEL_QUALITY_TARGET_FAILED
        };

        Self {
            label,
            message: if ultimate_contract {
                format!("Failed: 3D quality gate failed ({quality_summary})")
            } else {
                format!(
                    "Failed: MS-SSIM {quality_summary} below target {:.2}",
                    foundation::constants::VIDEO_QUALITY_GATE_THRESHOLD
                )
            },
            reason_code: "quality_gate_failed",
        }
    }

    fn emit_summary(&self) {
        foundation::media_conversion_gate::explore_quality_skip_summary_audit(
            self.label,
            foundation::infra::static_logs::messages::PROTECTING_ORIGINAL,
            foundation::infra::static_logs::messages::DISCARDING_OUTPUT,
        );
    }
}

fn probe_video_streams(input: &Path) -> Result<Vec<VideoStreamInfo>> {
    let command = foundation::FfprobeBuilder::new()
        .input(input)
        .loglevel("error")
        .print_format("json")
        .show_streams()
        .build();
    let output = match run_animated_process(command) {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return Err(VidQualityError::ConversionError(format!(
                "ffprobe stream probe failed for {}: status={}",
                input.display(),
                output.status
            )));
        }
        Err(err) => {
            return Err(VidQualityError::ConversionError(format!(
                "ffprobe stream probe failed for {}: {err}",
                input.display()
            )));
        }
    };

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|err| {
        VidQualityError::ConversionError(format!(
            "ffprobe stream JSON parse failed for {}: {err}",
            input.display()
        ))
    })?;

    let stream_values = json
        .get("streams")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            VidQualityError::ConversionError(format!(
                "ffprobe stream JSON missing streams array for {}",
                input.display()
            ))
        })?;
    let mut streams = Vec::new();
    for stream in stream_values
        .iter()
        .filter(|stream| stream.get("codec_type").and_then(|v| v.as_str()) == Some("video"))
    {
        let index_u64 = stream
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                VidQualityError::ConversionError(format!(
                    "ffprobe video stream missing parseable index for {}",
                    input.display()
                ))
            })?;
        let index = usize::try_from(index_u64).map_err(|err| {
            VidQualityError::ConversionError(format!(
                "ffprobe video stream index {index_u64} did not fit usize for {}: {err}",
                input.display()
            ))
        })?;
        let frame_count = match stream.get("nb_frames").and_then(|v| v.as_str()) {
            None | Some("N/A") => None,
            Some(value) => Some(value.parse::<u64>().map_err(|err| {
                VidQualityError::ConversionError(format!(
                    "ffprobe nb_frames parse failed for {} stream {index}: {value:?}: {err}",
                    input.display()
                ))
            })?),
        };
        let pix_fmt = foundation::media_conversion_gate::ffprobe_pix_fmt_or_empty(
            stream.get("pix_fmt").and_then(|v| v.as_str()),
            index,
            "animated ffprobe streams",
        );
        streams.push(VideoStreamInfo {
            index,
            frame_count,
            pix_fmt,
        });
    }
    Ok(streams)
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

    selected_stream
        .frame_count
        .is_some_and(|c| c > 0 && Some(c) == aux_stream.frame_count)
        && looks_like_alpha_stream(&aux_stream.pix_fmt)
}

fn has_probable_avif_alpha_stream(input: &Path) -> Result<bool> {
    let streams = probe_video_streams(input).map_err(|err| {
        foundation::media_conversion_gate::probe_layer_batch_audit(
            "avif_alpha_stream_probe_failed",
            format!(
                "AVIF alpha stream probe failed for {}; refusing alpha-aux decision: {err}",
                input.display(),
            ),
        );
        err
    })?;
    let probe = foundation::probe_video(input).map_err(|err| {
        let message = format!(
            "AVIF alpha stream selected-stream probe failed for {}: {err}",
            input.display()
        );
        foundation::media_conversion_gate::probe_layer_batch_audit(
            "avif_alpha_stream_selected_probe_failed",
            &message,
        );
        VidQualityError::ConversionError(message)
    })?;
    Ok(is_probable_alpha_aux_pair(&streams, probe.stream_index))
}

fn avif_video_stream_count(input: &Path, context: &str) -> Result<usize> {
    let mut builder = foundation::FfprobeBuilder::new();
    builder
        .input(input)
        .select_streams(foundation::StreamType::Video)
        .show_entries("stream=index")
        .print_format("csv=p=0");

    let output = run_animated_process(builder.build()).map_err(|err| {
        let message = format!(
            "AVIF stream-count probe failed to start for {} in {context}: {err}",
            input.display()
        );
        foundation::media_conversion_gate::probe_layer_batch_audit(
            "avif_stream_count_probe_failed",
            &message,
        );
        VidQualityError::ConversionError(message)
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = format!(
            "AVIF stream-count probe failed for {} in {context}: status={} stderr={}",
            input.display(),
            output.status,
            stderr.trim(),
        );
        foundation::media_conversion_gate::probe_layer_batch_audit(
            "avif_stream_count_probe_failed",
            &message,
        );
        return Err(VidQualityError::ConversionError(message));
    }

    Ok(String::from_utf8_lossy(&output.stdout).lines().count())
}

fn extract_frames_for_gifski(
    input: &Path,
    selected_stream_index: Option<usize>,
    _verbose: bool,
) -> Result<(tempfile::TempDir, std::path::PathBuf, usize)> {
    let frame_dir = foundation::media_conversion_gate::delivery_temp_dir_in_scratch_or_err(
        "gif_frame_extract",
        "gifski_frames_",
    )
    .map_err(|e| {
        VidQualityError::ConversionError(format!("Failed to create frame temp dir: {e}"))
    })?;
    let frame_pattern = frame_dir.path().join("frame_%06d.png");

    let mut builder = foundation::FfmpegBuilder::new();
    builder.overwrite().input(input);
    if let Some(stream_index) = selected_stream_index {
        builder.arg("-map").arg(format!("0:{stream_index}"));
    }
    builder
        .arg("-fps_mode")
        .arg("passthrough")
        .pix_fmt(foundation::PixFmt::Rgba)
        .output(&frame_pattern);

    let output = run_animated_process(builder.build()).map_err(|e| {
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "gif_frame_extract_ffmpeg",
            input,
            format!("ffmpeg frame extraction failed to start: {e}"),
        );
        VidQualityError::ConversionError(format!("FFmpeg frame extraction failed: {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(VidQualityError::ConversionError(format!(
            "FFmpeg frame extraction failed: {stderr}"
        )));
    }

    let mut frame_count = 0usize;
    for entry in fs::read_dir(frame_dir.path()).map_err(|e| {
        VidQualityError::ConversionError(format!("Failed to inspect extracted frames: {e}"))
    })? {
        let entry = entry.map_err(|e| {
            VidQualityError::ConversionError(format!(
                "Failed to inspect extracted frame entry: {e}"
            ))
        })?;
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("png") {
            frame_count += 1;
        }
    }

    if frame_count < 2 {
        return Err(VidQualityError::ConversionError(format!(
            "Only extracted {frame_count} frame(s) for GIF encoding"
        )));
    }

    log_detail!(&format!(
        "  ↳ Deconstructed GIF: Extracted {frame_count} raw frames"
    ));

    let frame_dir_path = frame_dir.path().to_path_buf();
    Ok((frame_dir, frame_dir_path, frame_count))
}

/// Read authoritative WebP timing with `webpmux`, then let `FFmpeg` 9 coalesce the
/// animation canvas and encode APNG. Extracting individual WebP frame rectangles
/// loses their x/y offsets plus blend/dispose semantics.
fn extract_webp_to_apng(input: &Path, output_apng: &Path, verbose: bool) -> Result<()> {
    let (frame_count, mut frame_durations_ms) = parse_webpmux_info(input)?;
    let parsed_duration_count = foundation::numeric_cast::usize_to_u32_strict(
        frame_durations_ms.len(),
        "webp_frame_duration_count",
    )
    .ok_or_else(|| {
        VidQualityError::ConversionError(format!(
            "Parsed too many WebP frame durations for {}",
            input.display()
        ))
    })?;
    if parsed_duration_count != frame_count {
        let pad = frame_durations_ms.last().copied().ok_or_else(|| {
            VidQualityError::ConversionError(format!(
                "Cannot pad WebP frame durations for {} because no durations were parsed",
                input.display()
            ))
        })?;
        foundation::media_conversion_gate::webp_frame_duration_pad_audit(
            input,
            frame_count,
            frame_durations_ms.len(),
            pad,
        );
        frame_durations_ms.resize(
            foundation::numeric_cast::u32_to_usize_strict(frame_count, "webp_frame_count")
                .ok_or_else(|| {
                    VidQualityError::ConversionError(format!(
                        "WebP frame count does not fit usize for {}",
                        input.display()
                    ))
                })?,
            pad,
        );
    }

    let zero_duration_frames = frame_durations_ms
        .iter()
        .enumerate()
        .filter_map(|(index, duration)| (*duration == 0).then_some(index + 1))
        .collect::<Vec<_>>();
    for duration in &mut frame_durations_ms {
        if *duration == 0 {
            *duration = crate::constants::DEFAULT_ANIMATION_DELAY_MS;
        }
    }
    let final_duration_ms = frame_durations_ms.last().copied().ok_or_else(|| {
        VidQualityError::ConversionError(format!(
            "webpmux reported no WebP frame duration for {}",
            input.display()
        ))
    })?;

    let normalized_input = if zero_duration_frames.is_empty() {
        None
    } else {
        let temp = foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "webp_zero_duration_normalized",
            None,
            Some(".webp"),
        )?;
        let tool = foundation::common_utils::resolve_tool_path(foundation::constants::TOOL_WEBPMUX)
            .ok_or_else(|| {
                VidQualityError::ConversionError(
                    "webpmux is required to normalize zero-duration WebP frames".to_string(),
                )
            })?;
        let mut command = Command::new(tool);
        for frame in zero_duration_frames {
            command.arg("-duration").arg(format!(
                "{},{frame}",
                crate::constants::DEFAULT_ANIMATION_DELAY_MS
            ));
        }
        command.arg(input).arg("-o").arg(temp.path());
        let output = run_animated_process(command).map_err(|error| {
            VidQualityError::ConversionError(format!(
                "webpmux zero-duration normalization failed to start: {error}"
            ))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VidQualityError::ConversionError(format!(
                "webpmux zero-duration normalization failed: {}",
                foundation::io_utils::tail_error_lines(&stderr, 5)
            )));
        }
        Some(temp)
    };
    let ffmpeg_input = normalized_input.as_ref().map_or(input, |file| file.path());

    if verbose {
        let avg_dur = f64::from(frame_durations_ms.iter().sum::<u32>())
            / foundation::numeric_cast::usize_to_f64(frame_durations_ms.len());
        log_detail!("Stats: WebP: {frame_count} frames, ~{avg_dur:.1}ms/frame");
    }

    let mut builder = foundation::FfmpegBuilder::new();
    builder
        .overwrite()
        .input_arg("-f")
        .input_arg("webp_anim")
        .input(ffmpeg_input)
        .arg("-fps_mode")
        .arg("passthrough")
        .pix_fmt(foundation::PixFmt::Rgba)
        .vcodec(foundation::VideoCodec::Apng)
        .format("apng")
        .arg("-plays")
        .arg("0")
        .arg("-final_delay")
        .arg(format!("{final_duration_ms}/1000"))
        .output(output_apng);

    let ffmpeg_result = run_animated_process(builder.build()).map_err(|e| {
        VidQualityError::ConversionError(format!(
            "FFmpeg 9 animated WebP → APNG conversion failed to start: {e}"
        ))
    })?;

    if !ffmpeg_result.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_result.stderr);
        return Err(VidQualityError::ConversionError(format!(
            "FFmpeg 9 animated WebP → APNG conversion failed: {}",
            foundation::io_utils::tail_error_lines(&stderr, 5)
        )));
    }

    if verbose {
        log_detail!(
            "  ↳ Intermediate conversion: FFmpeg 9 coalesced WebP canvas to APNG ({frame_count} frames)"
        );
    }

    Ok(())
}

fn get_output_path(
    input: &Path,
    extension: &str,
    options: &ConvertOptions,
) -> Result<std::path::PathBuf> {
    match options.base_dir.as_ref() {
        None => {
            foundation::conversion::determine_output_path(input, extension, &options.output_dir)
                .map_err(VidQualityError::ConversionError)
        }
        Some(base) => determine_output_path_with_base(input, base, extension, &options.output_dir)
            .map_err(VidQualityError::ConversionError),
    }
}

fn skipped_with_fallback(
    input: &Path,
    options: &ConvertOptions,
    message: &str,
    reason_id: &str,
) -> Result<TaskResult> {
    TaskResult::skipped_with_fallback(input, options, message, reason_id)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))
}

fn skipped_with_fallback_owned(
    input: &Path,
    options: &ConvertOptions,
    message: String,
    reason_id: String,
) -> Result<TaskResult> {
    TaskResult::skipped_with_fallback_owned(input, options, message, reason_id)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))
}

fn failed_with_fallback(
    input: &Path,
    options: &ConvertOptions,
    message: &str,
    reason_id: &str,
) -> Result<TaskResult> {
    TaskResult::failed_with_fallback(input, options, message, reason_id)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))
}

fn failed_with_fallback_owned(
    input: &Path,
    options: &ConvertOptions,
    message: String,
    reason_id: String,
) -> Result<TaskResult> {
    TaskResult::failed_with_fallback_owned(input, options, message, reason_id)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))
}

/// Get the dimensions of an input video file.
///
/// # Errors
/// Returns an error if ffprobe fails.
pub fn get_input_dimensions(input: &Path) -> Result<(u32, u32)> {
    foundation::conversion::get_input_dimensions(input).map_err(VidQualityError::ConversionError)
}

fn get_max_threads(options: &ConvertOptions) -> usize {
    if options.child_threads > 0 {
        options.child_threads
    } else {
        foundation::thread_manager::get_balanced_thread_config(
            foundation::thread_manager::WorkloadType::Video,
        )
        .child_threads
    }
}

/// `FFmpeg` input path after optional JXL/WebP/AVIF-alpha → APNG deconstruct.
pub(crate) struct PreparedAnimatedRaster {
    pub(crate) input_ext: String,
    pub(crate) actual_input: std::path::PathBuf,
    temp_apng: Option<tempfile::NamedTempFile>,
}

pub(crate) enum PrepareAnimatedRasterOutcome {
    Ready(PreparedAnimatedRaster),
    Early(TaskResult),
}

/// Honest early exit: copy original when possible; never downgrade to `ignored_custom` on gate I/O failure.
fn prepare_early_fallback(
    input: &Path,
    options: &ConvertOptions,
    message: impl Into<String>,
    reason: &str,
) -> PrepareAnimatedRasterOutcome {
    let message = message.into();
    match failed_with_fallback_owned(input, options, message.clone(), reason.to_string()) {
        Ok(task) => PrepareAnimatedRasterOutcome::Early(task),
        Err(e) => {
            foundation::media_conversion_gate::delivery_pipeline_batch_audit(
                "prepare_early_fallback_failed",
                format!(
                    "{}: animated preprocess gate '{reason}' could not copy original: {e}",
                    input.display()
                ),
            );
            let input_size = match fs::metadata(input) {
                Ok(meta) => meta.len(),
                Err(meta_err) => {
                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                        "prepare_early_fallback_missing_input_size",
                        input,
                        format!("cannot read input size after gate failure: {meta_err}"),
                    );
                    0
                }
            };
            PrepareAnimatedRasterOutcome::Early(TaskResult::failed(
                input,
                input_size,
                &format!("{message} (original copy failed: {e})"),
                reason,
            ))
        }
    }
}

fn content_aware_extension_or_path_extension(input: &Path, context: &str) -> Result<String> {
    let path_ext =
        foundation::media_conversion_gate::path_extension_lowercase_or_empty(input, context);

    let detected_ext = match foundation::image_detection::detect_format_from_bytes(input) {
        Ok(format) => match format {
            foundation::image_detection::DetectedFormat::PNG => {
                if path_ext == foundation::constants::EXT_APNG {
                    Some(foundation::constants::EXT_APNG)
                } else {
                    Some(foundation::constants::EXT_PNG)
                }
            }
            foundation::image_detection::DetectedFormat::JPEG => Some("jpg"),
            foundation::image_detection::DetectedFormat::GIF => {
                Some(foundation::constants::EXT_GIF)
            }
            foundation::image_detection::DetectedFormat::WebP => {
                Some(foundation::constants::EXT_WEBP)
            }
            foundation::image_detection::DetectedFormat::HEIC => Some("heic"),
            foundation::image_detection::DetectedFormat::HEIF => Some("heif"),
            foundation::image_detection::DetectedFormat::AVIF => {
                Some(foundation::constants::EXT_AVIF)
            }
            foundation::image_detection::DetectedFormat::JXL => {
                Some(foundation::constants::EXT_JXL)
            }
            foundation::image_detection::DetectedFormat::TIFF => Some("tif"),
            foundation::image_detection::DetectedFormat::BMP => Some("bmp"),
            foundation::image_detection::DetectedFormat::QOI => Some("qoi"),
            foundation::image_detection::DetectedFormat::JP2 => Some("jp2"),
            foundation::image_detection::DetectedFormat::ICO => Some("ico"),
            foundation::image_detection::DetectedFormat::TGA => Some("tga"),
            foundation::image_detection::DetectedFormat::EXR => Some("exr"),
            foundation::image_detection::DetectedFormat::FLIF => Some("flif"),
            foundation::image_detection::DetectedFormat::PSD => Some("psd"),
            foundation::image_detection::DetectedFormat::PNM => Some("pnm"),
            foundation::image_detection::DetectedFormat::DDS => Some("dds"),
            foundation::image_detection::DetectedFormat::MP4 => Some("mp4"),
            foundation::image_detection::DetectedFormat::MOV => Some("mov"),
            foundation::image_detection::DetectedFormat::MKV => Some("mkv"),
            foundation::image_detection::DetectedFormat::WEBM => Some("webm"),
            foundation::image_detection::DetectedFormat::Unknown(reason) => {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "animated_content_format_unknown",
                    input,
                    format!("refusing path extension fallback for {context}: {reason}"),
                );
                return Err(VidQualityError::ConversionError(format!(
                    "Unknown true input format for {} in {context}; refusing extension-based routing",
                    input.display()
                )));
            }
        },
        Err(err) => {
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "animated_content_format_detect_failed",
                input,
                format!("refusing path extension fallback for {context}: {err}"),
            );
            return Err(VidQualityError::ConversionError(format!(
                "Failed to detect true input format for {} in {context}: {err}",
                input.display()
            )));
        }
    };

    if let Some(detected_ext) = detected_ext {
        if !path_ext.is_empty() && path_ext != detected_ext {
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "animated_content_extension_override",
                input,
                format!(
                    "content extension {detected_ext} overrides path extension {path_ext} for {context}"
                ),
            );
        }
        Ok(detected_ext.to_string())
    } else {
        Err(VidQualityError::ConversionError(format!(
            "Unsupported true input format for {} in {context}; refusing extension-based routing",
            input.display()
        )))
    }
}

/// Preprocess animated raster sources before any ffmpeg video encode.
pub(crate) fn prepare_animated_raster_for_encode(
    input: &Path,
    options: &ConvertOptions,
    context: &str,
) -> PrepareAnimatedRasterOutcome {
    let input_ext = match content_aware_extension_or_path_extension(input, context) {
        Ok(ext) => ext,
        Err(err) => {
            return prepare_early_fallback(
                input,
                options,
                format!("Skipped: {err}"),
                "format_detection_failed",
            );
        }
    };

    let (actual_input, temp_apng_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
        if input_ext == "jxl" {
            if options.verbose() {
                log_detail!(
                    "   Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)",
                );
            }
            if !required_tool_available(foundation::constants::TOOL_DJXL) {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "djxl_not_found",
                    input,
                    "cannot process animated JXL",
                );
                return prepare_early_fallback(
                    input,
                    options,
                    "Skipped: djxl not found (required for animated JXL)",
                    "djxl_not_found",
                );
            }
            let temp_apng =
                match foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                    "animated_apng_temp",
                    None,
                    Some(".apng"),
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        return prepare_early_fallback(
                            input,
                            options,
                            format!("Failed to create temp APNG: {e}"),
                            "temp_apng",
                        );
                    }
                };
            let temp_apng_path = temp_apng.path().to_path_buf();
            let mut builder = foundation::DjxlBuilder::new();
            builder.input(input).output(&temp_apng_path);
            let djxl_result = run_animated_process(builder.build());
            match djxl_result {
                Ok(output) if output.status.success() && temp_apng_path.exists() => {
                    if options.verbose() {
                        log_detail!(
                            " ↳ Intermediate conversion: JXL decoded to APNG for video encoding"
                        );
                    }
                    (temp_apng_path, Some(temp_apng))
                }
                _ => {
                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                        "djxl_failed",
                        input,
                        "JXL → APNG conversion failed",
                    );
                    return prepare_early_fallback(
                        input,
                        options,
                        "JXL → APNG conversion failed (djxl error)",
                        "djxl_failed",
                    );
                }
            }
        } else if input_ext == foundation::constants::EXT_WEBP {
            if options.verbose() {
                log_detail!("  Detected WebP format, extracting frames with webpmux");
            }
            if !required_tool_available(foundation::constants::TOOL_WEBPMUX) {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "webpmux_not_found",
                    input,
                    "cannot process animated WebP",
                );
                return prepare_early_fallback(
                    input,
                    options,
                    "Skipped: webpmux not found (required for animated WebP)",
                    "webpmux_not_found",
                );
            }
            let temp_apng =
                match foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                    "animated_apng_temp",
                    None,
                    Some(".apng"),
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        return prepare_early_fallback(
                            input,
                            options,
                            format!("Failed to create temp APNG: {e}"),
                            "temp_apng",
                        );
                    }
                };
            let temp_apng_path = temp_apng.path().to_path_buf();
            match extract_webp_to_apng(input, &temp_apng_path, options.verbose()) {
                Ok(()) => (temp_apng_path, Some(temp_apng)),
                Err(e) => {
                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                        "webp_extraction_failed",
                        input,
                        format!("error: {e}"),
                    );
                    return prepare_early_fallback(
                        input,
                        options,
                        format!("WebP extraction failed: {e}"),
                        "webp_extraction_failed",
                    );
                }
            }
        } else if input_ext == foundation::constants::EXT_AVIF {
            let has_alpha_aux = match has_probable_avif_alpha_stream(input) {
                Ok(value) => value,
                Err(err) => {
                    return prepare_early_fallback(
                        input,
                        options,
                        format!("Skipped: AVIF alpha stream probe failed: {err}"),
                        "avif_alpha_stream_probe_failed",
                    );
                }
            };
            if has_alpha_aux {
                if options.verbose() {
                    log_detail!("  Detected AVIF auxiliary alpha stream, pre-converting to APNG");
                }
                let temp_apng =
                    match foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                        "animated_apng_temp",
                        None,
                        Some(".apng"),
                    ) {
                        Ok(t) => t,
                        Err(e) => {
                            return prepare_early_fallback(
                                input,
                                options,
                                format!("Failed to create temp APNG: {e}"),
                                "temp_apng",
                            );
                        }
                    };
                let temp_apng_path = temp_apng.path().to_path_buf();
                let mut builder = foundation::FfmpegBuilder::new();
                builder
                    .overwrite()
                    .input(input)
                    .with_odd_dim_correction()
                    .arg("-filter_complex")
                    .arg("[0:v:0][0:v:1]alphamerge")
                    .pix_fmt(foundation::PixFmt::Rgba)
                    .arg("-plays")
                    .arg("0")
                    .vcodec(foundation::VideoCodec::Apng)
                    .output(&temp_apng_path);
                match run_animated_process(builder.build()) {
                    Ok(res) if res.status.success() => (temp_apng_path, Some(temp_apng)),
                    Ok(res) => {
                        let stderr = String::from_utf8_lossy(&res.stderr);
                        return prepare_early_fallback(
                            input,
                            options,
                            format!(
                                "AVIF alpha preprocess ffmpeg failed: {}",
                                foundation::io_utils::tail_error_lines(&stderr, 5)
                            ),
                            "avif_alpha_preprocess_failed",
                        );
                    }
                    Err(e) => {
                        return prepare_early_fallback(
                            input,
                            options,
                            format!("AVIF alpha preprocess ffmpeg error: {e}"),
                            "avif_alpha_preprocess_failed",
                        );
                    }
                }
            } else {
                (input.to_path_buf(), None)
            }
        } else {
            (input.to_path_buf(), None)
        };

    PrepareAnimatedRasterOutcome::Ready(PreparedAnimatedRaster {
        input_ext,
        actual_input,
        temp_apng: temp_apng_file,
    })
}

#[must_use]
pub fn is_high_quality_animated(width: u32, height: u32) -> bool {
    let total_pixels = u64::from(width).saturating_mul(u64::from(height));
    width >= foundation::constants::HQ_HD_WIDTH
        || height >= foundation::constants::HQ_HD_HEIGHT
        || total_pixels >= foundation::constants::HQ_PIX_COUNT_HD
}

fn skipped_already_processed(input: &Path, options: &ConvertOptions) -> Result<TaskResult> {
    foundation::TaskResult::skipped_with_fallback(
        input,
        options,
        "Skipped: Already processed",
        "duplicate",
    )
    .map_err(|e| VidQualityError::ConversionError(e.to_string()))
}

fn skipped_output_exists(input: &Path, output: &Path, _input_size: u64) -> Result<TaskResult> {
    TaskResult::skipped_exists(input, output)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))
}

/// Return true when the input is either a native GIF or a GIF-like silent loop
/// video that the scorer says should stay in the GIF domain.
fn assess_loop_intent_for_path(path: &Path) -> Option<foundation::LoopIntentVerdict> {
    if foundation::should_use_gif_fast_path(path)
        && let Some(meta) = foundation::LoopMeta::from_gif_path(path)
    {
        return Some(foundation::assess_loop_intent_from_meta(&meta, Some(path)));
    }

    match foundation::probe_video(path) {
        Ok(probe) => Some(foundation::assess_loop_intent_from_probe(&probe, path)),
        Err(err) => {
            foundation::media_conversion_gate::probe_layer_batch_audit(
                "loop_intent_probe_failed",
                format!(
                    "Loop intent probe failed for {}; refusing GIF-domain fallback: {err}",
                    path.display()
                ),
            );
            None
        }
    }
}

/// `LoopIntent` assessment for GIF-only `FastMode`.
///
/// Unlike legacy helper gates, probe failures are returned as errors so the
/// `FastMode` command can fail closed instead of treating missing evidence as a
/// normal non-loop verdict.
pub fn assess_loop_intent_for_fast_gif(path: &Path) -> Result<foundation::LoopIntentVerdict> {
    if foundation::should_use_gif_fast_path(path)
        && let Some(meta) = foundation::LoopMeta::from_gif_path(path)
    {
        return Ok(foundation::assess_loop_intent_from_meta(&meta, Some(path)));
    }

    foundation::probe_video(path)
        .map(|probe| foundation::assess_loop_intent_from_probe(&probe, path))
        .map_err(|err| {
            foundation::media_conversion_gate::probe_layer_batch_audit(
                "fast_gif_loop_intent_probe_failed",
                format!(
                    "Fast GIF loop intent probe failed for {}; refusing GIF output decision: {err}",
                    path.display()
                ),
            );
            VidQualityError::ConversionError(format!(
                "Fast GIF loop intent probe failed for {}: {err}",
                path.display()
            ))
        })
}

/// Return true when the input is either a native GIF or a GIF-like silent loop
/// video that the scorer says should stay in the GIF domain.
fn is_gif_meme(path: &Path) -> bool {
    assess_loop_intent_for_path(path).is_some_and(|verdict| verdict.is_keep_gif())
}

/// Returns true if the file is an animated image format but effectively static (0 or negligible duration).
/// Callers should ignore it rather than producing a video output.
fn animation_analysis_is_static(is_animated: bool, duration_secs: Option<f32>) -> bool {
    !is_animated
        || duration_secs.is_some_and(|duration| {
            duration
                < foundation::numeric_cast::f64_to_f32_lossy(
                    foundation::constants::NEGLIGIBLE_DURATION_SECS,
                )
        })
}

fn is_static_animated_image(path: &Path) -> Result<bool> {
    let ext = content_aware_extension_or_path_extension(
        path,
        &format!("static animated check {}", path.display()),
    )?;
    if !foundation::quality_matcher::parse_source_codec(&ext).can_be_animated() {
        return Ok(false);
    }
    let analysis = foundation::image_analyzer::analyze_image(path).map_err(|err| {
        let message = format!(
            "static animated image analysis failed for {}: {err}",
            path.display()
        );
        foundation::media_conversion_gate::probe_layer_batch_audit(
            "static_animated_image_analysis_failed",
            &message,
        );
        VidQualityError::ConversionError(message)
    })?;
    Ok(animation_analysis_is_static(
        analysis.is_animated,
        analysis.duration_secs,
    ))
}

fn ignored_static_animated(input: &Path) -> Result<TaskResult> {
    let input_size = fs::metadata(input)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?
        .len();
    Ok(foundation::TaskResult::ignored_custom(
        input,
        input_size,
        "IGNORED: static image (1 frame) is outside vid domain",
        "static_animated",
    ))
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
/// Convert animated image to MP4 (HEVC or AV1).
///
/// # Errors
/// Returns an error if encoding fails.
/// Convert an animated image (GIF, animated WebP, etc.) to a video container.
///
/// # Errors
///
/// Returns an error if the conversion fails or input is malformed.
pub fn convert_to_mp4(input: &Path, options: &ConvertOptions) -> Result<TaskResult> {
    if !options.force() && is_already_processed(input) {
        return skipped_already_processed(input, options);
    }

    if is_static_animated_image(input)? {
        if options.verbose() {
            log_detail!(
                "Skipping: Static animated image (1 frame) is outside vid domain: {}",
                input.display(),
            );
        }
        return ignored_static_animated(input);
    }

    // GIF / GIF-like video meme-score: if the asset behaves like a looping sticker, keep it
    // in the GIF domain instead of re-encoding to a video container.
    if is_gif_meme(input) {
        return skipped_with_fallback(
            input,
            options,
            "Skipped: GIF-like asset identified as meme/sticker (meme-score / loop score)",
            "gif_meme",
        );
    }

    let input_size = fs::metadata(input)?.len();

    let prep = match prepare_animated_raster_for_encode(
        input,
        options,
        &format!("animated deconstruct {}", input.display()),
    ) {
        PrepareAnimatedRasterOutcome::Ready(p) => p,
        PrepareAnimatedRasterOutcome::Early(task) => return Ok(task),
    };
    let input_ext = prep.input_ext;
    let actual_input = prep.actual_input;
    let temp_apng_file = prep.temp_apng;

    let ext = if options.apple_compat() { "MOV" } else { "MP4" };
    let output = get_output_path(input, ext, options)?;

    log_detail!(
        "Starting animated image deconstruction: {} (Input: {}, AppleCompat={})",
        foundation::media_conversion_gate::path_file_name_for_log(input),
        input_ext,
        options.apple_compat()
    );

    if output.exists() && !options.force() {
        return skipped_output_exists(input, &output, input_size);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = foundation::conversion::TempOutputGuard::new(temp_output.clone());

    let (width, height) = get_input_dimensions(&actual_input)?;
    let has_alpha = input_ext == foundation::constants::EXT_WEBP
        || input_ext == foundation::constants::EXT_GIF
        || input_ext == foundation::constants::EXT_JXL
        || (input_ext == foundation::constants::EXT_AVIF && has_probable_avif_alpha_stream(input)?)
        || input_ext == foundation::constants::EXT_APNG
        || input_ext == foundation::constants::EXT_PNG;
    let mut vf_args = foundation::get_ffmpeg_dimension_args(width, height, has_alpha);

    let color_info = foundation::ffprobe_json::extract_color_info(input);
    let targeted_info =
        foundation::hdr::infer_bt709_if_modern(color_info, width, height, &input_ext);
    vf_args.extend(foundation::hdr::color_info_to_ffmpeg_args(&targeted_info));

    let max_threads = get_max_threads(options);

    let video_spec = options.codec.animated_lossless_ffmpeg_video_spec(
        options.apple_compat(),
        options.ultimate(),
        options.archive(),
        max_threads,
    )?;
    let v_codec = video_spec.v_codec;
    let v_tag = video_spec.v_tag;
    let codec_params_flag = video_spec.params_flag;
    let codec_params = video_spec.params;

    // Probe ORIGINAL input to get stream index for multi-stream files (animated AVIF/HEIC).
    // For JXL/WebP, actual_input is APNG (single stream) and effective_stream_idx is forced
    // to 0 below, so we skip probing and skip propagating an unrelated probe error.
    let effective_stream_idx = if input_ext == "jxl" || input_ext == "webp" {
        0 // APNG is always single-stream
    } else {
        // Honest: a failed probe on a multi-stream-capable container must surface;
        // silently selecting stream 0 would mis-map AVIF/HEIC inputs whose primary
        // image item is not at index 0.
        foundation::probe_video(input)
            .map_err(|e| {
                VidQualityError::ConversionError(format!(
                    "ffprobe failed to determine stream index for {} (refusing to silently default to stream 0): {e}",
                    input.display()
                ))
            })?
            .stream_index
    };

    let mut builder = foundation::FfmpegBuilder::new();
    builder
        .overwrite()
        .with_odd_dim_correction()
        .threads(max_threads)
        .input(&actual_input)
        .arg(foundation::constants::FFMPEG_ARG_MAP)
        .arg(format!("0:{effective_stream_idx}")) // Select the correct stream
        // NO -r parameter: preserve original frame rate
        .arg(foundation::constants::FFMPEG_ARG_CODEC_VIDEO)
        .arg(v_codec)
        .arg(foundation::constants::FFMPEG_ARG_CRF)
        .arg("0")
        .arg(foundation::constants::FFMPEG_ARG_PRESET)
        .arg(video_spec.preset)
        .arg(foundation::constants::FFMPEG_ARG_TAG_VIDEO)
        .arg(v_tag)
        .arg(codec_params_flag)
        .arg(&codec_params);

    builder.args(&vf_args);

    builder
        .arg("-movflags")
        .arg("+faststart")
        .output(&temp_output);
    let result = run_animated_process(builder.build());

    // Clean up temporary APNG file if it was created
    drop(temp_apng_file);

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)
                .map_err(|e| {
                    cleanup_temp_output(&temp_output, input);
                    VidQualityError::ConversionError(format!(
                        "Failed to read encoded output metadata for {}: {e}",
                        temp_output.display()
                    ))
                })?
                .len();
            if output_size == 0 || get_input_dimensions(&temp_output).is_err() {
                cleanup_temp_output(&temp_output, input);
                let codec_name = options.codec.as_str().to_uppercase();
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "encode_invalid_output",
                    input,
                    format!("{codec_name} output empty or unreadable; copying original"),
                );
                return failed_with_fallback_owned(
                    input,
                    options,
                    format!("{codec_name} output invalid; original copied"),
                    format!("{}_invalid_output", options.codec.as_str()),
                );
            }

            if !foundation::conversion::commit_temp_to_output_with_metadata(
                &temp_output,
                &output,
                options.force(),
                Some(input),
            )? {
                return skipped_output_exists(input, &output, input_size);
            }

            mark_as_processed(input);

            if options.should_delete_original()
                && let Err(e) = foundation::conversion::safe_delete_original(
                    input,
                    &output,
                    foundation::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                )
            {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "delete_original_failed",
                    input,
                    format!("output {}: {e}", output.display()),
                );
            }

            let codec_name = options.codec.as_str().to_uppercase();
            Ok(TaskResult::success(
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
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "encode_failed",
                input,
                format!(
                    "{codec_name} ffmpeg failed: {}",
                    foundation::io_utils::tail_error_lines(&stderr, 5)
                ),
            );
            failed_with_fallback_owned(
                input,
                options,
                format!(
                    "{} encode failed; original copied (ffmpeg: {})",
                    codec_name,
                    foundation::io_utils::tail_error_lines(&stderr, 5)
                ),
                format!("{}_encode_failed", options.codec.as_str()),
            )
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "ffmpeg_not_found",
                input,
                format!("error: {e}"),
            );
            failed_with_fallback_owned(
                input,
                options,
                format!("HEVC encode failed (ffmpeg not found: {e}); original copied"),
                "hevc_encode_failed".to_string(),
            )
        }
    }
}

/// Convert video to MP4 (HEVC or AV1) with matched quality.
///
/// # Errors
/// Returns an error if matching or encoding fails.
///
/// # Panics
/// Panics if the tolerance ratio cannot be converted to a finite rational number.
pub fn convert_to_mp4_matched(
    input: &Path,
    options: &ConvertOptions,
    initial_crf: f32,
    has_alpha: bool,
) -> Result<TaskResult> {
    if !options.force() && is_already_processed(input) {
        return skipped_already_processed(input, options);
    }

    if is_static_animated_image(input)? {
        if options.verbose() {
            log_detail!(
                "   Detected static animated image (1 frame), ignoring outside vid domain: {}",
                input.display(),
            );
        }
        return ignored_static_animated(input);
    }

    // GIF / GIF-like video meme-score: if the asset behaves like a looping sticker, keep it
    // in the GIF domain instead of re-encoding to a video container.
    if is_gif_meme(input) {
        return skipped_with_fallback(
            input,
            options,
            "Skipped: GIF-like asset identified as meme/sticker (meme-score / loop score)",
            "gif_meme",
        );
    }

    let input_size = fs::metadata(input)?.len();

    let input_ext = content_aware_extension_or_path_extension(
        input,
        &format!("gif meme path {}", input.display()),
    )?;

    let ext = if options.apple_compat() { "MOV" } else { "MP4" };
    let output = get_output_path(input, ext, options)?;

    if output.exists() && !options.force() {
        return skipped_output_exists(input, &output, input_size);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = foundation::conversion::TempOutputGuard::new(temp_output.clone());

    // Special handling for animated JXL/WebP: pre-convert to APNG
    let (actual_input, temp_apng_file): (std::path::PathBuf, Option<tempfile::NamedTempFile>) =
        if input_ext == "jxl" {
            if options.verbose() {
                log_detail!(
                    "   Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)",
                );
            }
            if !required_tool_available(foundation::constants::TOOL_DJXL) {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "djxl_not_found",
                    input,
                    "cannot process animated JXL",
                );
                return failed_with_fallback(
                    input,
                    options,
                    "Skipped: djxl not found (required for animated JXL)",
                    "djxl_not_found",
                );
            }
            let temp_apng =
                foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                    "animated_apng_temp",
                    None,
                    Some(".apng"),
                )
                .map_err(|e| {
                    VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                })?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            let mut builder = foundation::DjxlBuilder::new();
            builder.input(input).output(&temp_apng_path);
            let djxl_result = run_animated_process(builder.build());
            match djxl_result {
                Ok(output) if output.status.success() && temp_apng_path.exists() => {
                    if options.verbose() {
                        log_info!(
                            foundation::infra::static_logs::messages::LABEL_JXL,
                            "Intermediate conversion: JXL deconstructed to APNG"
                        );
                    }
                    (temp_apng_path, Some(temp_apng))
                }
                _ => {
                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                        "djxl_failed",
                        input,
                        "JXL → APNG conversion failed",
                    );
                    return failed_with_fallback(
                        input,
                        options,
                        "JXL → APNG conversion failed (djxl error)",
                        "djxl_failed",
                    );
                }
            }
        } else if input_ext == foundation::constants::EXT_WEBP {
            if options.verbose() {
                log_detail!("  Detected WebP format, extracting frames with webpmux");
            }

            // Check if webpmux is available
            if !required_tool_available(foundation::constants::TOOL_WEBPMUX) {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "webpmux_not_found",
                    input,
                    "cannot process animated WebP",
                );
                return failed_with_fallback(
                    input,
                    options,
                    "Skipped: webpmux not found (required for animated WebP)",
                    "webpmux_not_found",
                );
            }

            // Create temporary APNG file
            let temp_apng =
                foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                    "animated_apng_temp",
                    None,
                    Some(".apng"),
                )
                .map_err(|e| {
                    VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                })?;
            let temp_apng_path = temp_apng.path().to_path_buf();

            // Extract WebP frames and create APNG with correct timing
            match extract_webp_to_apng(input, &temp_apng_path, options.verbose()) {
                Ok(()) => (temp_apng_path, Some(temp_apng)),
                Err(e) => {
                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                        "webp_extraction_failed",
                        input,
                        format!("error: {e}"),
                    );
                    return failed_with_fallback_owned(
                        input,
                        options,
                        format!("WebP extraction failed: {e}"),
                        "webp_extraction_failed".to_string(),
                    );
                }
            }
        } else if input_ext == "avif"
            && avif_video_stream_count(input, "matched alpha preprocess")? > 1
        {
            if options.verbose() {
                log_detail!(
                    "   Detected transparent AVIF format, pre-converting to APNG to retain alpha explicitly",
                );
            }
            let temp_apng =
                foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                    "animated_apng_temp",
                    None,
                    Some(".apng"),
                )?;
            let temp_apng_path = temp_apng.path().to_path_buf();
            let mut builder = foundation::FfmpegBuilder::new();
            builder
                .overwrite()
                .input(input)
                .with_odd_dim_correction()
                .arg("-filter_complex")
                .arg("[0:v:0][0:v:1]alphamerge")
                .arg("-plays")
                .arg("0")
                .vcodec(foundation::VideoCodec::Apng)
                .output(&temp_apng_path);

            let res = run_animated_process(builder.build())?;
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
            let probe = foundation::probe_video(input).map_err(|err| {
                let message = format!(
                    "multi-stream {} probe failed for {}: {err}",
                    input_ext,
                    input.display()
                );
                foundation::media_conversion_gate::probe_layer_batch_audit(
                    "animated_multi_stream_probe_failed",
                    &message,
                );
                VidQualityError::ConversionError(message)
            })?;
            let has_multiple_streams =
                avif_video_stream_count(input, "matched multi-stream extraction")? > 1;

            if has_multiple_streams && probe.stream_index > 0 {
                if options.verbose() {
                    log_detail!(
                        "   Multi-stream {} detected, converting stream {} to APNG ({} frames)",
                        input_ext.to_uppercase(),
                        probe.stream_index,
                        foundation::media_conversion_gate::delivery_frame_count_label_u64(
                            probe.frame_count,
                            &format!("multi-stream APNG {}", input.display()),
                        ),
                    );
                }

                // Create temporary APNG file
                let temp_stream =
                    foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                        "animated_stream_apng_temp",
                        None,
                        Some(".apng"),
                    )
                    .map_err(|e| {
                        VidQualityError::ConversionError(format!("Failed to create temp APNG: {e}"))
                    })?;
                let temp_stream_path = temp_stream.path().to_path_buf();

                // Convert the correct stream to APNG using FFmpeg
                let mut builder = foundation::FfmpegBuilder::new();
                builder
                    .overwrite()
                    .input(input)
                    .arg("-map")
                    .arg(format!("0:{}", probe.stream_index))
                    .vcodec(foundation::VideoCodec::Apng)
                    .format("apng")
                    .arg("-plays")
                    .arg("0")
                    .output(&temp_stream_path);

                let extract_result = run_animated_process(builder.build());

                match extract_result {
                    Ok(output) if output.status.success() && temp_stream_path.exists() => {
                        if options.verbose() {
                            log_info!(
                                foundation::infra::static_logs::messages::LABEL_AVIF,
                                "Intermediate conversion: Target stream deconstructed to APNG"
                            );
                        }
                        (temp_stream_path, Some(temp_stream))
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        return failed_with_fallback_owned(
                            input,
                            options,
                            format!(
                                "Multi-stream {} extraction failed: {}",
                                input_ext,
                                foundation::io_utils::tail_error_lines(&stderr, 5)
                            ),
                            "multi_stream_extraction_failed".to_string(),
                        );
                    }
                    Err(err) => {
                        return failed_with_fallback_owned(
                            input,
                            options,
                            format!("Multi-stream {input_ext} extraction failed: {err}"),
                            "multi_stream_extraction_failed".to_string(),
                        );
                    }
                }
            } else {
                (actual_input, None)
            }
        } else {
            (actual_input, None)
        };

    let (width, height) = get_input_dimensions(&final_input)?;
    let mut vf_args = foundation::get_ffmpeg_dimension_args(width, height, has_alpha);

    let color_info = foundation::ffprobe_json::extract_color_info(input);
    let targeted_info =
        foundation::hdr::infer_bt709_if_modern(color_info, width, height, &input_ext);
    vf_args.extend(foundation::hdr::color_info_to_ffmpeg_args(&targeted_info));

    let flag_mode = options
        .flag_mode()
        .map_err(VidQualityError::ConversionError)?;

    let use_gpu = options.use_gpu();
    if !use_gpu && options.verbose() {
        log_detail!(
            "   CPU Mode: Using {} for higher SSIM (≥{:.2})",
            options.codec.cpu_encoder_name(),
            foundation::constants::HIGH_QUALITY_MIN_SSIM,
        );
    }

    let is_gif = foundation::is_gif_magic(&final_input).map_err(|e| {
        VidQualityError::ConversionError(format!(
            "Failed to probe GIF magic for {}: {e}",
            final_input.display()
        ))
    })?;
    let mut actual_initial_crf = initial_crf;

    let probe = foundation::ffprobe::probe_video(input).map_err(|e| {
        if options.verbose() {
            foundation::log_detail!(
                "{} ffprobe analysis failed for {}: {}",
                foundation::modern_ui::symbols::pick(
                    foundation::modern_ui::symbols::WARNING,
                    foundation::modern_ui::symbols::plain::WARNING,
                ),
                input.display(),
                e
            );
        }
        foundation::media_conversion_gate::probe_layer_audit(
            "animated_lossless_safety_ffprobe_failed",
            input,
            format!("ffprobe failed during animated lossless-safety decision: {e}"),
        );
        VidQualityError::ConversionError(format!(
            "ffprobe failed during animated lossless-safety decision for {}: {e}",
            input.display()
        ))
    })?;
    let is_safe_for_lossless = if let Some(dur_val) = probe.duration {
        let duration = foundation::numeric_cast::f64_to_f32_lossy(dur_val);
        (is_gif && flag_mode.is_ultimate())
            && (if duration < ANIMATION_CLIP_THRESHOLD_SECS {
                let meta = LoopMeta::from_ffprobe_result(&probe, input);
                is_lossless_exploration_safe(&meta, Some(input))
            } else {
                false
            })
    } else {
        false
    };

    if is_safe_for_lossless {
        // [Data-Driven Optimization]
        // Allow long, low-entropy memes to undergo CRF 0.00 probing.
        // High-value artwork still maintains a 30s threshold to prevent overflow.
        actual_initial_crf = 0.0;
    } else if let Some(hint) = options.codec.warm_start_crf_hint() {
        if options.verbose() {
            log_detail!(
                "  Warm Start: Utilizing global {} success CRF ({hint:.1}) for anchor convergence",
                options.codec.as_str().to_uppercase()
            );
        }
        actual_initial_crf = foundation::numeric_cast::f64_to_f32_lossy(hint);
    }

    if options.verbose() {
        log_detail!(
            "   {} Mode: CRF {:.1} (based on input analysis/cache)",
            flag_mode.description_en(),
            actual_initial_crf,
        );
    }

    let ultimate = flag_mode.is_ultimate();
    let explore_preset = if options.archive() {
        foundation::EncoderPreset::Veryslow
    } else if ultimate {
        foundation::EncoderPreset::Slower
    } else {
        foundation::EncoderPreset::Medium
    };
    let explore_result = options
        .codec
        .explore_with_gpu(&foundation::GpuSearchRequest {
            input: final_input,
            output: temp_output.clone(),
            vf_args: vf_args.clone(),
            baseline_crf: actual_initial_crf,
            warm_start_crf: None,
            flags: foundation::delivery_codec_strategy::gpu_search_flags_for_codec(
                options.codec,
                foundation::GpuSearchFeatures {
                    ultimate_mode: ultimate,
                    apple_compat: options.apple_compat(),
                    archive_mode: options.archive(),
                },
                foundation::GpuSearchValidation {
                    force_ms_ssim_long: false,
                    allow_size_tolerance: options.allow_size_tolerance(),
                },
            ),
            min_ssim: 0.0,
            max_threads: options.child_threads,
            hdr_x265_params: None,
            preset: explore_preset,
        })
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

    // Clean up temporary files
    drop(temp_apng_file);
    drop(temp_stream_file);

    for log in &explore_result.log {
        log_detail!("{log}");
    }

    // apple_compat mode: compatibility takes priority over file size.
    // However, if the source is already apple-compatible (like GIF/APNG), size guard stays active.
    // For definitive loop assets, compatibility/domain correctness beats size.
    // If loop intent says this should stay in the GIF domain, do not apply the size guard.
    let is_guard_active =
        foundation::is_size_guard_active(&input_ext, options.apple_compat()) && !is_gif_meme(input);

    if is_guard_active {
        let verification = foundation::verify_strict_pure_media_paths(
            input,
            &temp_output,
            options.allow_size_tolerance(),
        )
        .map_err(|err| {
            VidQualityError::ConversionError(format!(
                "Strict animated pure-media verification failed for {} -> {}: {err}",
                input.display(),
                temp_output.display()
            ))
        })?;
        if !verification.pure_media_compressed {
            let size_increase_pct = verification.pure_media_size_change_percent();
            let codec_name = options.codec.as_str().to_uppercase();
            if let Err(e) = fs::remove_file(&temp_output) {
                log_detail!(&format!(
                    "Cleanup Audit: Failed to remove pure-media-oversized {codec_name} temporary output at {}. Error: {e}",
                    temp_output.display()
                ));
            }
            if options.allow_size_tolerance() {
                log_detail!(
                    "   Skipping: {} pure media larger than input by {:.1}% (allowed growth: {} bytes)",
                    codec_name,
                    size_increase_pct,
                    foundation::constants::DEFAULT_SIZE_TOLERANCE_BYTES
                );
            } else {
                log_detail!(
                    "   Skipping: {} pure media larger than input by {:.1}% (strict mode: no tolerance)",
                    codec_name,
                    size_increase_pct
                );
            }
            log_detail!(
                "   Pure-media comparison: {} → {} bytes ({:+.1}%)",
                verification.input_pure_media_size,
                verification.output_pure_media_size,
                size_increase_pct
            );
            return skipped_with_fallback_owned(
                input,
                options,
                format!(
                    "Skipped: {codec_name} pure media larger than input by {size_increase_pct:.1}% ({width}x{height}, tolerance exceeded)"
                ),
                "size_increase_beyond_tolerance".to_string(),
            );
        }
    }

    // apple_compat: if exploration gates failed only because the file couldn't be compressed
    // (not because of actual quality degradation), still accept the HEVC output.
    // A larger-but-playable HEVC is always better than a non-playable original (e.g. AVIF).
    let pipeline_ok =
        explore_result.pipeline_acceptable(options.match_quality(), options.explore());
    let quality_or_compat_ok = pipeline_ok
        || (options.apple_compat()
            && !explore_result.uses_ultimate_quality_contract()
            && explore_result
                .ssim
                .is_some_and(|s| s >= foundation::constants::ACCEPTABLE_MIN_SSIM));

    if !quality_or_compat_ok {
        let decision = AnimatedGateRejectionDecision::inspect_and_log(input, &explore_result);
        decision.emit_summary();

        return if decision.failed {
            failed_with_fallback_owned(
                input,
                options,
                decision.message,
                decision.reason_code.to_string(),
            )
        } else {
            skipped_with_fallback_owned(
                input,
                options,
                decision.message,
                decision.reason_code.to_string(),
            )
        };
    }

    let final_gate_block = if options.match_quality() {
        !explore_result.perceptual_quality_met()
    } else {
        explore_result.perceptual_quality_failed()
    };
    if final_gate_block {
        let decision = AnimatedFinalGateFailureDecision::inspect_and_log(input, &explore_result);
        decision.emit_summary();

        return failed_with_fallback_owned(
            input,
            options,
            decision.message,
            decision.reason_code.to_string(),
        );
    }

    if pipeline_ok {
        options
            .codec
            .record_global_crf_hit(explore_result.optimal_crf);
    }

    if !foundation::conversion::commit_temp_to_output_with_metadata(
        &temp_output,
        &output,
        options.force(),
        Some(input),
    )? {
        return skipped_output_exists(input, &output, input_size);
    }

    mark_as_processed(input);

    if options.should_delete_original()
        && let Err(e) = foundation::conversion::safe_delete_original(
            input,
            &output,
            foundation::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
        )
    {
        let codec_name = options.codec.as_str().to_uppercase();
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "delete_original_failed",
            input,
            format!(
                "after {codec_name} animated conversion, output {}: {e}",
                output.display()
            ),
        );
    }

    Ok(TaskResult::success_video_explored(
        input,
        &output,
        &foundation::conversion::VideoExplorationMetrics {
            input_size,
            output_size: explore_result.output_size,
            codec_name: options.codec.as_str(),
            crf: explore_result.optimal_crf,
            is_lossless: explore_result.optimal_crf
                < foundation::numeric_cast::f64_to_f32_lossy(
                    foundation::constants::NEGLIGIBLE_DURATION_SECS,
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
pub fn convert_to_mkv_lossless(input: &Path, options: &ConvertOptions) -> Result<TaskResult> {
    log_detail!(
        "Compute Warning: Executing mathematical lossless encoding (HEVC); expect significant resource consumption and large payload sizes",
    );

    if !options.force() && is_already_processed(input) {
        return skipped_already_processed(input, options);
    }

    let input_size = fs::metadata(input)?.len();
    let output = get_output_path(input, "mkv", options)?;

    if output.exists() && !options.force() {
        return skipped_output_exists(input, &output, input_size);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = foundation::conversion::TempOutputGuard::new(temp_output.clone());

    let prep = match prepare_animated_raster_for_encode(
        input,
        options,
        &format!("mkv_lossless {}", input.display()),
    ) {
        PrepareAnimatedRasterOutcome::Ready(p) => p,
        PrepareAnimatedRasterOutcome::Early(task) => return Ok(task),
    };
    let input_ext = prep.input_ext;
    let actual_input = prep.actual_input;
    let _temp_apng_file = prep.temp_apng;

    let (width, height) = get_input_dimensions(&actual_input)?;
    let vf_args = foundation::get_ffmpeg_dimension_args(width, height, false);

    let effective_stream_idx = if input_ext == "jxl" || input_ext == "webp" {
        0
    } else {
        foundation::probe_video(input)
            .map_err(|e| {
                VidQualityError::ConversionError(format!(
                    "ffprobe failed to determine stream index for {} (refusing to silently default to stream 0): {e}",
                    input.display()
                ))
            })?
            .stream_index
    };

    let max_threads = get_max_threads(options);
    let x265_params = format!("lossless=1:log-level=error:pools={max_threads}");
    let mut builder = foundation::FfmpegBuilder::new();
    builder
        .overwrite()
        .with_odd_dim_correction()
        .threads(max_threads)
        .input(&actual_input)
        .arg(foundation::constants::FFMPEG_ARG_MAP)
        .arg(format!("0:{effective_stream_idx}"))
        .vcodec(foundation::VideoCodec::Hevc)
        .x265_params(x265_params);

    if options.ultimate() {
        builder.preset(foundation::EncoderPreset::Slower);
    } else {
        builder.preset(foundation::EncoderPreset::Medium);
    }

    if options.apple_compat() {
        builder.tag_video(foundation::constants::FFMPEG_TAG_HVC1);
    }

    for arg in &vf_args {
        builder.arg(arg);
    }

    builder
        .arg("-movflags")
        .arg("+faststart")
        .output(&temp_output);

    let result = run_animated_process(builder.build());

    match result {
        Ok(output_cmd) if output_cmd.status.success() => {
            let output_size = fs::metadata(&temp_output)?.len();

            if !foundation::conversion::commit_temp_to_output_with_metadata(
                &temp_output,
                &output,
                options.force(),
                Some(input),
            )? {
                return skipped_output_exists(input, &output, input_size);
            }

            mark_as_processed(input);

            if options.should_delete_original()
                && let Err(e) = foundation::conversion::safe_delete_original(
                    input,
                    &output,
                    foundation::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                )
            {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "delete_original_failed",
                    input,
                    format!("lossless HEVC, output {}: {e}", output.display()),
                );
            }

            Ok(TaskResult::success(
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
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "lossless_encode_failed",
                input,
                format!(
                    "ffmpeg lossless failed; copying original ({})",
                    foundation::io_utils::tail_error_lines(&stderr, 5)
                ),
            );
            failed_with_fallback_owned(
                input,
                options,
                format!(
                    "Lossless failed; original copied ({})",
                    foundation::io_utils::tail_error_lines(&stderr, 5)
                ),
                "lossless_failed".to_string(),
            )
        }
        Err(e) => {
            cleanup_temp_output(&temp_output, input);
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "ffmpeg_not_found",
                input,
                format!("ffmpeg not found for lossless encode; copying original: {e}"),
            );
            failed_with_fallback_owned(
                input,
                options,
                format!("Lossless failed (ffmpeg not found: {e}); original copied"),
                "lossless_failed".to_string(),
            )
        }
    }
}

/// Convert animated/video input to AVIF for meme mode without loop-intent filtering.
///
/// # Errors
/// Returns an error if encoding fails.
///
/// # Panics
/// Panics if the tolerance ratio cannot be converted to a finite rational number.
pub fn convert_to_avif_meme(input: &Path, options: &ConvertOptions) -> Result<TaskResult> {
    if !options.force() && is_already_processed(input) {
        return skipped_already_processed(input, options);
    }

    if is_static_animated_image(input)? {
        if options.verbose() {
            log_detail!(
                "   Detected static animated image (1 frame), ignoring outside vid domain: {}",
                input.display(),
            );
        }
        return ignored_static_animated(input);
    }

    let input_size = fs::metadata(input)?.len();

    let prep = match prepare_animated_raster_for_encode(
        input,
        options,
        &format!("meme avif {}", input.display()),
    ) {
        PrepareAnimatedRasterOutcome::Ready(p) => p,
        PrepareAnimatedRasterOutcome::Early(task) => return Ok(task),
    };
    let input_ext = prep.input_ext;
    let actual_input = prep.actual_input;
    let temp_apng_file = prep.temp_apng;
    let preprocessed_to_apng = temp_apng_file.is_some();

    let output = get_output_path(input, "avif", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force() {
        return skipped_output_exists(input, &output, input_size);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = foundation::conversion::TempOutputGuard::new(temp_output.clone());

    let (width, height) = get_input_dimensions(&actual_input)?;
    let has_alpha = input_ext == foundation::constants::EXT_WEBP
        || input_ext == foundation::constants::EXT_GIF
        || input_ext == foundation::constants::EXT_JXL
        || input_ext == foundation::constants::EXT_APNG
        || input_ext == foundation::constants::EXT_PNG
        || (input_ext == foundation::constants::EXT_AVIF && preprocessed_to_apng);
    let mut vf_args = foundation::get_ffmpeg_dimension_args(width, height, has_alpha);

    let color_info = foundation::ffprobe_json::extract_color_info(input);
    let targeted_info =
        foundation::hdr::infer_bt709_if_modern(color_info, width, height, &input_ext);
    vf_args.extend(foundation::hdr::color_info_to_ffmpeg_args(&targeted_info));

    let max_threads = get_max_threads(options);
    let effective_stream_idx = if input_ext == foundation::constants::EXT_JXL
        || input_ext == foundation::constants::EXT_WEBP
        || preprocessed_to_apng
    {
        0
    } else {
        foundation::probe_video(input)
            .map_err(|e| {
                VidQualityError::ConversionError(format!(
                    "ffprobe failed to determine stream index for {} (refusing to silently default to stream 0): {e}",
                    input.display()
                ))
            })?
            .stream_index
    };

    let can_use_official_avifenc = !has_alpha;
    let result = if can_use_official_avifenc {
        let temp_y4m = temp_output.with_extension("mfb-y4m");
        let _temp_y4m_guard = foundation::conversion::TempOutputGuard::new(temp_y4m.clone());
        match foundation::common_utils::resolve_tool_path(foundation::constants::TOOL_AVIFENC) {
            Some(avifenc) => match encode_animated_avif_with_avifenc(
                &actual_input,
                &temp_y4m,
                &temp_output,
                effective_stream_idx,
                max_threads,
                &vf_args,
                &avifenc,
            ) {
                Ok(output)
                    if output.status.success()
                        && validate_animated_avif_output(&temp_output).is_ok() =>
                {
                    log_detail!(
                        "Animated AVIF Meme Mode: used official avifenc frame-sequence encoder for {}",
                        input.display()
                    );
                    Ok(output)
                }
                Ok(_output) => {
                    cleanup_temp_output(&temp_output, input);
                    log_detail!(
                        "Animated AVIF Meme Mode: avifenc frame-sequence attempt was unsuitable; trying FFmpeg fallback for {}",
                        input.display()
                    );
                    match ensure_avif_animation_ffmpeg_support() {
                        Ok(encoder) => encode_animated_avif_with_ffmpeg(
                            &actual_input,
                            &temp_output,
                            effective_stream_idx,
                            max_threads,
                            &vf_args,
                            encoder,
                        ),
                        Err(err) => Err(std::io::Error::other(err.to_string())),
                    }
                }
                Err(err) => {
                    cleanup_temp_output(&temp_output, input);
                    log_detail!(
                        "Animated AVIF Meme Mode: avifenc launch failed ({err}); trying FFmpeg fallback for {}",
                        input.display()
                    );
                    match ensure_avif_animation_ffmpeg_support() {
                        Ok(encoder) => encode_animated_avif_with_ffmpeg(
                            &actual_input,
                            &temp_output,
                            effective_stream_idx,
                            max_threads,
                            &vf_args,
                            encoder,
                        ),
                        Err(err) => Err(std::io::Error::other(err.to_string())),
                    }
                }
            },
            None => match ensure_avif_animation_ffmpeg_support() {
                Ok(encoder) => encode_animated_avif_with_ffmpeg(
                    &actual_input,
                    &temp_output,
                    effective_stream_idx,
                    max_threads,
                    &vf_args,
                    encoder,
                ),
                Err(err) => Err(std::io::Error::other(err.to_string())),
            },
        }
    } else {
        log_detail!(
            "Animated AVIF Meme Mode: compositing alpha on black for AVIF compatibility with \
             FFmpeg AV1 encoder for {}",
            input.display()
        );
        match ensure_avif_animation_ffmpeg_support() {
            Ok(encoder) => encode_animated_avif_with_ffmpeg(
                &actual_input,
                &temp_output,
                effective_stream_idx,
                max_threads,
                &vf_args,
                encoder,
            ),
            Err(err) => Err(std::io::Error::other(err.to_string())),
        }
    };
    drop(temp_apng_file);

    match result {
        Ok(output_cmd) if output_cmd.status.success() && temp_output.exists() => {
            let output_size = fs::metadata(&temp_output)
                .map_err(|e| {
                    cleanup_temp_output(&temp_output, input);
                    VidQualityError::ConversionError(format!(
                        "Failed to read AVIF meme output metadata for {}: {e}",
                        temp_output.display()
                    ))
                })?
                .len();

            let detected_format = foundation::image::format_detect::detect_true_format(
                &temp_output,
            )
            .map_err(|err| {
                cleanup_temp_output(&temp_output, input);
                VidQualityError::ConversionError(format!(
                    "AVIF meme output format detection failed for {}: {err}",
                    temp_output.display()
                ))
            })?;
            let dimensions_ok = get_input_dimensions(&temp_output).is_ok();
            let sequence_frame_count = animated_avif_sequence_frame_count(&temp_output);
            let sequence_ok = matches!(sequence_frame_count.as_ref(), Ok(count) if *count > 1);
            if output_size == 0
                || detected_format != foundation::image::format_detect::FormatKind::Avif
                || !dimensions_ok
                || !sequence_ok
            {
                let sequence_detail = match &sequence_frame_count {
                    Ok(count) => format!("{count} frame(s)"),
                    Err(err) => err.to_string(),
                };
                cleanup_temp_output(&temp_output, input);
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "avif_meme_invalid_output",
                    input,
                    format!(
                        "AVIF meme output empty, unreadable, wrong format ({detected_format:?}), or not animated ({sequence_detail}); copying original"
                    ),
                );
                return failed_with_fallback(
                    input,
                    options,
                    "AVIF meme output invalid or static; original copied",
                    "avif_meme_invalid_output",
                );
            }

            if !foundation::conversion::commit_temp_to_output_with_metadata(
                &temp_output,
                &output,
                options.force(),
                None,
            )? {
                return skipped_output_exists(input, &output, input_size);
            }

            foundation::metadata::verify_output_embedded_metadata(
                input,
                &output,
                foundation::metadata::MetadataOutputPolicy::Clear,
            )
            .map_err(|error| {
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "animated meme cleared-metadata mismatch output cleanup",
                    &output,
                );
                VidQualityError::ConversionError(format!(
                    "Animated Meme Mode cleared-metadata verification failed for {}: {error}",
                    output.display()
                ))
            })?;

            mark_as_processed(input);

            if options.should_delete_original()
                && let Err(e) = foundation::conversion::safe_delete_original(
                    input,
                    &output,
                    foundation::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
                )
            {
                foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                    "delete_original_failed",
                    input,
                    format!("after AVIF meme mode, output {}: {e}", output.display()),
                );
            }

            Ok(TaskResult::success(
                input,
                &output,
                input_size,
                output_size,
                "AVIF",
                Some("Meme Mode"),
                options.quality_label.as_deref(),
            ))
        }
        Ok(output_cmd) => {
            let stderr = String::from_utf8_lossy(&output_cmd.stderr);
            cleanup_temp_output(&temp_output, input);
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "avif_meme_encode_failed",
                input,
                format!(
                    "ffmpeg AVIF meme encode failed; copying original ({})",
                    foundation::io_utils::tail_error_lines(&stderr, 5)
                ),
            );
            failed_with_fallback_owned(
                input,
                options,
                format!(
                    "AVIF meme encode failed; original copied ({})",
                    foundation::io_utils::tail_error_lines(&stderr, 5)
                ),
                "avif_meme_encode_failed".to_string(),
            )
        }
        Err(err) => {
            cleanup_temp_output(&temp_output, input);
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "avif_meme_encode_start_failed",
                input,
                format!("ffmpeg AVIF meme encode failed to start; copying original: {err}"),
            );
            failed_with_fallback_owned(
                input,
                options,
                format!("AVIF meme encode failed to start; original copied ({err})"),
                "avif_meme_encode_start_failed".to_string(),
            )
        }
    }
}

/// Convert to GIF with Apple compatibility.
///
/// # Errors
/// Returns an error if encoding fails.
///
/// # Panics
/// Panics if the tolerance ratio cannot be converted to a finite rational number.
pub fn convert_to_gif_apple_compat(input: &Path, options: &ConvertOptions) -> Result<TaskResult> {
    if !options.force() && is_already_processed(input) {
        return skipped_already_processed(input, options);
    }

    if is_static_animated_image(input)? {
        if options.verbose() {
            log_detail!(
                "   Detected static animated image (1 frame), ignoring outside vid domain: {}",
                input.display(),
            );
        }
        return ignored_static_animated(input);
    }

    let input_size = fs::metadata(input)?.len();

    let input_ext_initial = content_aware_extension_or_path_extension(
        input,
        &format!("gif compat {}", input.display()),
    )?;

    if input_ext_initial == "gif" {
        log_detail!("  Input is already GIF, skipping re-encode (would likely increase size)");
        return skipped_with_fallback(
            input,
            options,
            "Skipped: Already GIF (re-encoding would increase size)",
            "already_gif",
        );
    }

    let prep = match prepare_animated_raster_for_encode(
        input,
        options,
        &format!("gif compat {}", input.display()),
    ) {
        PrepareAnimatedRasterOutcome::Ready(p) => p,
        PrepareAnimatedRasterOutcome::Early(task) => return Ok(task),
    };
    let input_ext = prep.input_ext;
    let actual_input = prep.actual_input;
    let temp_apng_file = prep.temp_apng;

    let output = get_output_path(input, "GIF", options)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    if output.exists() && !options.force() {
        return skipped_output_exists(input, &output, input_size);
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;
    let _temp_output_guard = foundation::conversion::TempOutputGuard::new(temp_output.clone());

    let (width, height) = get_input_dimensions(&actual_input)?;

    // See `convert_to_mp4`: same honest stream-index policy. JXL/WebP are forced to 0
    // (APNG is single-stream); other containers must surface a probe failure rather
    // than silently defaulting to stream 0.
    let effective_stream_idx = if input_ext == "jxl" || input_ext == "webp" {
        0
    } else {
        foundation::probe_video(input)
            .map_err(|e| {
                VidQualityError::ConversionError(format!(
                    "ffprobe failed to determine stream index for {} (refusing to silently default to stream 0): {e}",
                    input.display()
                ))
            })?
            .stream_index
    };

    let has_multiple_streams = probe_video_streams(&actual_input)?.len() > 1;
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
                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                        "gif_frame_extraction_failed",
                        input,
                        format!("gifski prep failed for output {}: {e}", output.display()),
                    );
                    return failed_with_fallback_owned(
                        input,
                        options,
                        format!("GIF frame extraction failed: {e}"),
                        "gif_frame_extraction_failed".to_string(),
                    );
                }
            };

        let probe_res = foundation::probe_video(input).map_err(|e| {
            VidQualityError::ConversionError(format!("Failed to probe source for FPS: {e}"))
        })?;

        let duration_val = probe_res.duration.ok_or_else(|| {
            VidQualityError::ConversionError(
                "Source duration missing - cannot determine native speed".to_string(),
            )
        })?;

        let fps = foundation::media_conversion_gate::gif_encode_fps_from_probe(
            input,
            duration_val,
            extracted_count,
            probe_res.avg_frame_rate,
            probe_res.frame_rate,
        )
        .ok_or_else(|| {
            VidQualityError::ConversionError(
                "Source metadata lacks both duration and frame rate - cannot determine native speed"
                    .to_string(),
            )
        })?;

        if options.verbose() {
            log_detail!(
                "   🔧 GIF Encoding: Native speed ({extracted_count} frames / {duration_val:.2}s duration) -> target speed: {fps:.3} FPS",
            );
        }
        let mut gifski_builder = foundation::GifskiBuilder::new();
        gifski_builder
            .output(&temp_output)
            .fps(foundation::numeric_cast::f64_to_f32_lossy(fps))
            .dimensions(width, height)
            .quality(100)
            .motion_quality(100)
            .lossy_quality(100)
            .repeat(0)
            .arg("--extra");

        // Collect and sort extracted PNG frames to ensure correct sequence
        let mut frames = Vec::new();
        for entry in std::fs::read_dir(&gifski_frames_path).map_err(|e| {
            VidQualityError::ConversionError(format!("Failed to read frame directory: {e}"))
        })? {
            let entry = entry.map_err(|e| {
                VidQualityError::ConversionError(format!("Failed to read GIF frame entry: {e}"))
            })?;
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|ext| ext == std::ffi::OsStr::new("png"))
            {
                frames.push(path);
            }
        }
        frames.sort();

        for frame in frames {
            gifski_builder.input(frame);
        }

        let res = run_animated_process(gifski_builder.build());

        drop(gifski_frames_dir);
        match res {
            Ok(o) if o.status.success() && temp_output.exists() => true,
            Ok(o) => {
                log_failure!(
                    "Gifski",
                    "{} → {} - gifski failed with status {}: {}",
                    input.display(),
                    output.display(),
                    foundation::media_conversion_gate::process_exit_code_label(
                        o.status.code(),
                        "gifski",
                        input,
                    ),
                    String::from_utf8_lossy(&o.stderr)
                );
                false
            }
            Err(e) => {
                log_failure!(
                    "Gifski",
                    "{} → {} - gifski command failed to start: {}",
                    input.display(),
                    output.display(),
                    e
                );
                false
            }
        }
    };

    // Clean up temporary APNG file if it was created
    drop(temp_apng_file);

    if !gifski_ok {
        // gifski conversion failed — copy original so data is not lost
        cleanup_temp_output(&temp_output, input);
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "gif_encode_failed",
            input,
            "gifski unavailable or failed; copying original",
        );
        return failed_with_fallback(
            input,
            options,
            "GIF conversion failed (gifski unavailable or failed); original copied",
            "gif_encode_failed",
        );
    }

    // Validate output
    let output_size = fs::metadata(&temp_output)
        .map_err(|e| {
            cleanup_temp_output(&temp_output, input);
            VidQualityError::ConversionError(format!(
                "Failed to read GIF output metadata for {}: {e}",
                temp_output.display()
            ))
        })?
        .len();
    if output_size == 0 || get_input_dimensions(&temp_output).is_err() {
        cleanup_temp_output(&temp_output, input);
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "gif_invalid_output",
            input,
            "GIF output empty or unreadable; copying original",
        );
        return failed_with_fallback(
            input,
            options,
            "GIF output invalid; original copied",
            "gif_invalid_output",
        );
    }

    // apple_compat: compatibility takes priority — a playable GIF is always
    // better than a non-playable original (e.g. animated AVIF).
    // But if the source is already playable (like APNG or GIF), size guard stays active.
    let is_guard_active = foundation::is_size_guard_active(&input_ext, options.apple_compat());

    if is_guard_active {
        let verification = foundation::verify_strict_pure_media_paths(
            input,
            &temp_output,
            options.allow_size_tolerance(),
        )
        .map_err(|err| {
            VidQualityError::ConversionError(format!(
                "Strict GIF pure-media verification failed for {} -> {}: {err}",
                input.display(),
                temp_output.display()
            ))
        })?;
        if !verification.pure_media_compressed {
            let size_increase_pct = verification.pure_media_size_change_percent();
            if let Err(e) = fs::remove_file(&temp_output) {
                log_detail!("[cleanup] Failed to remove pure-media-oversized GIF output: {e}");
            }
            if options.allow_size_tolerance() {
                log_detail!(
                    "   Skipping: GIF pure media larger than input by {size_increase_pct:.1}% (allowed growth: {} bytes)",
                    foundation::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
                );
            } else {
                log_detail!(
                    "   Skipping: GIF pure media larger than input by {size_increase_pct:.1}% (strict mode: no tolerance)",
                );
            }
            log_detail!(
                "   Pure-media comparison: {} → {} bytes ({size_increase_pct:+.1}%)",
                verification.input_pure_media_size,
                verification.output_pure_media_size,
            );
            return skipped_with_fallback_owned(
                input,
                options,
                format!(
                    "Skipped: GIF pure media larger than input by {size_increase_pct:.1}% (tolerance exceeded)"
                ),
                "size_increase_beyond_tolerance".to_string(),
            );
        }
    }

    if !foundation::conversion::commit_temp_to_output_with_metadata(
        &temp_output,
        &output,
        options.force(),
        Some(input),
    )? {
        return skipped_output_exists(input, &output, input_size);
    }

    mark_as_processed(input);

    if options.should_delete_original()
        && let Err(e) = foundation::conversion::safe_delete_original(
            input,
            &output,
            foundation::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
        )
    {
        foundation::media_conversion_gate::delivery_api_path_fallback_audit(
            "delete_original_failed",
            input,
            format!("after GIF apple-compat, output {}: {e}", output.display()),
        );
    }

    Ok(TaskResult::success(
        input,
        &output,
        input_size,
        output_size,
        "GIF",
        Some("Apple Compat"),
        options.quality_label.as_deref(),
    ))
}

fn parse_webpmux_info(input: &Path) -> Result<(u32, Vec<u32>)> {
    let mut builder = foundation::WebpmuxBuilder::new();
    builder.input(input).info(true);
    let webpmux_info = run_animated_process(builder.build())
        .map_err(|e| VidQualityError::ConversionError(format!("webpmux not found: {e}")))?;

    if !webpmux_info.status.success() {
        return Err(VidQualityError::ConversionError(
            "webpmux -info failed".to_string(),
        ));
    }

    let info_str = String::from_utf8_lossy(&webpmux_info.stdout);

    let mut frame_count = 0;
    let mut frame_durations_ms = Vec::new();
    let mut parsing_frames = false;

    for line in info_str.lines() {
        if line.contains("Number of frames:") {
            if let Some(count_str) = line.split(':').nth(1) {
                frame_count = count_str.trim().parse::<u32>().map_err(|e| {
                    VidQualityError::ConversionError(format!(
                        "Failed to parse WebP frame count for {} (value: {count_str}, error: {e})",
                        input.display()
                    ))
                })?;
            }
        } else if line.contains("No.: width height") {
            parsing_frames = true;
        } else if parsing_frames {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 7
                && parts.first().is_some_and(|p| p.ends_with(':'))
                && let Some(Ok(duration)) = parts.get(6).map(|p| p.parse::<u32>())
            {
                frame_durations_ms.push(duration);
            }
        }
    }

    if frame_count == 0 || frame_durations_ms.is_empty() {
        return Err(VidQualityError::ConversionError(
            "Failed to parse WebP frame metadata".to_string(),
        ));
    }

    Ok((frame_count, frame_durations_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::Builder;

    #[test]
    fn animated_webp_ffmpeg9_coalesces_offsets_and_preserves_timing() {
        for tool in ["ffmpeg", "ffprobe", "webpmux"] {
            if !required_tool_available(tool) {
                eprintln!("skipping animated WebP integration test: {tool} unavailable");
                return;
            }
        }

        let ffmpeg = foundation::common_utils::resolve_tool_path("ffmpeg")
            .expect("ffmpeg was checked above");
        let decoder_listing = Command::new(&ffmpeg)
            .args(["-hide_banner", "-decoders"])
            .output()
            .expect("list ffmpeg decoders");
        if !String::from_utf8_lossy(&decoder_listing.stdout).contains("webp_anim") {
            eprintln!("skipping animated WebP integration test: FFmpeg 9 decoder unavailable");
            return;
        }

        let temp = tempfile::tempdir().expect("create WebP regression tempdir");
        let base = temp.path().join("base.webp");
        let patch = temp.path().join("patch.webp");
        let animated = temp.path().join("offset.webp");
        let apng = temp.path().join("offset.apng");
        std::fs::write(
            &base,
            [
                0x52, 0x49, 0x46, 0x46, 0x1c, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50,
                0x38, 0x4c, 0x0f, 0x00, 0x00, 0x00, 0x2f, 0x03, 0xc0, 0x00, 0x00, 0x07, 0x10, 0xfd,
                0x8f, 0xfe, 0x07, 0x22, 0xa2, 0xff, 0x01, 0x00,
            ],
        )
        .expect("write red 4x4 WebP frame");
        std::fs::write(
            &patch,
            [
                0x52, 0x49, 0x46, 0x46, 0x1c, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50,
                0x38, 0x4c, 0x0f, 0x00, 0x00, 0x00, 0x2f, 0x01, 0x40, 0x00, 0x00, 0x07, 0x10, 0xd1,
                0xff, 0xfe, 0x07, 0x22, 0xa2, 0xff, 0x01, 0x00,
            ],
        )
        .expect("write blue 2x2 WebP frame");

        let mut mux = foundation::WebpmuxBuilder::new();
        mux.add_frame(&base, 100, 0, 0, false)
            .add_frame(&patch, 0, 2, 2, true)
            .add_frame(&base, 200, 0, 0, false)
            .loop_count(0)
            .output(&animated);
        let muxed = run_animated_process(mux.build()).expect("assemble animated WebP");
        assert!(muxed.status.success(), "webpmux failed: {:?}", muxed.stderr);

        extract_webp_to_apng(&animated, &apng, false).expect("coalesce animated WebP to APNG");

        let (_frames, _frames_path, frame_count) =
            extract_frames_for_gifski(&apng, None, false).expect("extract APNG frames for gifski");
        assert_eq!(frame_count, 3);

        let decoded = Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error"])
            .arg("-i")
            .arg(&apng)
            .args([
                "-vf",
                "select=eq(n\\,1)",
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .output()
            .expect("decode second APNG frame");
        assert!(
            decoded.status.success(),
            "ffmpeg failed: {:?}",
            decoded.stderr
        );
        assert_eq!(decoded.stdout.len(), 4 * 4 * 4);
        assert_eq!(&decoded.stdout[0..4], &[255, 0, 0, 255]);
        assert_eq!(&decoded.stdout[60..64], &[0, 0, 255, 255]);

        let ffprobe = foundation::common_utils::resolve_tool_path("ffprobe")
            .expect("ffprobe was checked above");
        let timing = Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "frame=duration_time",
                "-of",
                "csv=p=0",
            ])
            .arg(&apng)
            .output()
            .expect("probe APNG timing");
        assert!(timing.status.success());
        let durations = String::from_utf8_lossy(&timing.stdout);
        assert_eq!(
            durations.lines().collect::<Vec<_>>(),
            ["0.100000", "0.100000", "0.200000"]
        );
    }

    #[test]
    fn avif_animation_encoder_prefers_svt_then_aom() {
        let listing = "
 V....D libaom-av1           libaom AV1
 V....D libsvtav1            SVT-AV1
";

        assert_eq!(
            select_avif_animation_encoder_from_listing(listing),
            Some(AvifAnimationEncoder::SvtAv1)
        );
    }

    #[test]
    fn avif_animation_encoder_falls_back_to_libaom() {
        let listing = "
 V....D libaom-av1           libaom AV1
";

        assert_eq!(
            select_avif_animation_encoder_from_listing(listing),
            Some(AvifAnimationEncoder::LibAomAv1)
        );
    }

    #[test]
    fn avif_animation_muxer_parser_requires_exact_token() {
        assert!(avif_muxer_available_from_listing(" E avif           AVIF"));
        assert!(!avif_muxer_available_from_listing(
            " E notavif        Different muxer"
        ));
    }

    #[test]
    fn avif_animation_frame_count_parser_reads_sequence_frames() {
        let info = " * Image Sequence Frames: (12 expected frames)";

        assert_eq!(parse_avifdec_sequence_frame_count(info), Ok(Some(12)));
    }

    #[test]
    fn avif_animation_frame_count_parser_reads_timescale_summary() {
        let info = " * 12288 timescales per second, 2.00 seconds (24576 timescales), 12 frames";

        assert_eq!(parse_avifdec_sequence_frame_count(info), Ok(Some(12)));
    }

    #[test]
    fn avif_animation_frame_count_parser_rejects_still_info() {
        let info = "Image decoded: still.avif\n * Resolution     : 64x64";

        assert_eq!(parse_avifdec_sequence_frame_count(info), Ok(None));
    }

    #[test]
    fn avif_animation_frame_count_parser_rejects_malformed_frame_count() {
        let info = " * Image Sequence Frames: (not-a-count expected frames)";

        assert!(parse_avifdec_sequence_frame_count(info).is_err());
    }

    #[test]
    fn synthetic_grayscale_animation_avif_roundtrip_stays_neutral() {
        let Some(avifenc) = foundation::common_utils::resolve_tool_path("avifenc") else {
            eprintln!("Skipping grayscale animation test: avifenc is unavailable");
            return;
        };
        if !required_tool_available("ffmpeg") || !required_tool_available("avifdec") {
            eprintln!("Skipping grayscale animation test: ffmpeg or avifdec is unavailable");
            return;
        }

        let root = tempfile::tempdir().expect("tempdir");
        let input = root.path().join("synthetic_gray.apng");
        let y4m = root.path().join("synthetic_gray.y4m");
        let encoded = root.path().join("synthetic_gray.avif");

        let mut source = foundation::FfmpegBuilder::new();
        source
            .overwrite()
            .loglevel("error")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("color=c=gray:s=64x64:r=4:d=0.75")
            .pix_fmt_str("gray")
            .format("apng")
            .output(&input);
        let output = run_animated_process(source.build()).expect("create synthetic grayscale APNG");
        assert!(
            output.status.success(),
            "synthetic APNG creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let output = encode_animated_avif_with_avifenc(&input, &y4m, &encoded, 0, 1, &[], &avifenc)
            .expect("encode synthetic grayscale animation");
        assert!(
            output.status.success(),
            "animated AVIF encode failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            validate_animated_avif_output(&encoded).expect("validate animated AVIF") > 1,
            "animated AVIF must retain multiple frames"
        );

        let decoded = root.path().join("synthetic_gray.ppm");
        let mut decode = foundation::FfmpegBuilder::new();
        decode
            .overwrite()
            .loglevel("error")
            .input(&encoded)
            .arg("-frames:v")
            .arg("1")
            .pix_fmt_str("rgb24")
            .format("image2")
            .output(&decoded);
        let output = run_animated_process(decode.build()).expect("decode first AVIF frame");
        assert!(
            output.status.success(),
            "animated AVIF decode failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ppm = std::fs::read(decoded).expect("read decoded PPM");
        let pixel_offset = ppm
            .windows(5)
            .position(|window| window == b"\n255\n")
            .map(|offset| offset + 5)
            .expect("valid PPM header");
        let pixels = &ppm[pixel_offset..];
        assert_eq!(pixels.len(), 64 * 64 * 3, "unexpected decoded frame size");
        let (max_channel_delta, worst_pixel) =
            pixels
                .as_chunks::<3>()
                .0
                .iter()
                .fold((0u8, [0u8; 3]), |worst, pixel| {
                    let delta = pixel[0]
                        .abs_diff(pixel[1])
                        .max(pixel[1].abs_diff(pixel[2]))
                        .max(pixel[0].abs_diff(pixel[2]));
                    if delta > worst.0 {
                        (delta, [pixel[0], pixel[1], pixel[2]])
                    } else {
                        worst
                    }
                });
        assert!(
            max_channel_delta <= 2,
            "grayscale animation gained a color cast: max RGB delta {max_channel_delta}, pixel {worst_pixel:?}"
        );
    }

    #[test]
    fn test_alpha_aux_detection_rejects_poster_plus_animation_avif() {
        let streams = vec![
            VideoStreamInfo {
                index: 0,
                frame_count: Some(1),
                pix_fmt: "yuv420p".to_string(),
            },
            VideoStreamInfo {
                index: 1,
                frame_count: Some(11),
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
                frame_count: Some(24),
                pix_fmt: "yuv420p".to_string(),
            },
            VideoStreamInfo {
                index: 1,
                frame_count: Some(24),
                pix_fmt: "gray8".to_string(),
            },
        ];

        assert!(is_probable_alpha_aux_pair(&streams, 0));
    }

    #[test]
    fn probe_video_streams_handles_missing_nb_frames_without_panicking() {
        // Regression: GIFs (and many fragmented containers) cause ffprobe to omit
        // or emit "N/A" for nb_frames. Prior code .expect()'d the parse and panicked
        // the whole batch mid-run. Now "unknown" frame counts surface as 0 and the
        // alpha-aux heuristic gates them out cleanly.
        let json = serde_json::json!({
            "streams": [
                { "codec_type": "video", "index": 0, "nb_frames": "N/A", "pix_fmt": "yuv420p" },
                { "codec_type": "video", "index": 1, "pix_fmt": "rgba" },
                { "codec_type": "audio", "index": 2 },
            ]
        });
        let streams: Vec<VideoStreamInfo> = json
            .get("streams")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter(|stream| stream.get("codec_type").and_then(|v| v.as_str()) == Some("video"))
            .filter_map(|stream| {
                let index = stream
                    .get("index")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| match usize::try_from(value) {
                        Ok(index) => Some(index),
                        Err(err) => {
                            eprintln!("test stream index {value} does not fit usize: {err}");
                            None
                        }
                    })?;
                let frame_count =
                    stream
                        .get("nb_frames")
                        .and_then(|v| v.as_str())
                        .and_then(|value| match value.parse::<u64>() {
                            Ok(parsed) => Some(parsed),
                            Err(err) => {
                                eprintln!("test frame count '{value}' is not numeric: {err}");
                                None
                            }
                        });
                let pix_fmt = foundation::media_conversion_gate::ffprobe_pix_fmt_or_empty(
                    stream.get("pix_fmt").and_then(|v| v.as_str()),
                    index,
                    "animated ffprobe streams test",
                );
                Some(VideoStreamInfo {
                    index,
                    frame_count,
                    pix_fmt,
                })
            })
            .collect();
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].frame_count, None);
        assert_eq!(streams[1].frame_count, None);

        assert!(!is_probable_alpha_aux_pair(&streams, 0));
    }

    #[test]
    fn test_apple_compat_blocks_copying_incompatible_originals() {
        let mut options = ConvertOptions::default();
        options
            .flags
            .set(foundation::conversion::ConvertFlags::APPLE_COMPAT, true);

        assert!(!options.should_copy_original_on_skip(Path::new("/tmp/test.avif")));
        assert!(!options.should_copy_original_on_skip(Path::new("/tmp/test.webp")));
        assert!(options.should_copy_original_on_skip(Path::new("/tmp/test.gif")));
        assert!(options.should_copy_original_on_skip(Path::new("/tmp/test.heic")));
    }

    #[test]
    fn test_animated_quality_failure_reports_pure_media_reason() {
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
        let decision = AnimatedGateRejectionDecision::inspect_and_log(
            Path::new("/tmp/test.gif"),
            &explore_result,
        );

        assert_eq!(
            decision.message,
            "Skipped: Pure media not smaller than input"
        );
        assert_eq!(
            decision.label,
            foundation::infra::static_logs::messages::LABEL_QUALITY_FAIL
        );
        assert!(!decision.failed);
        assert!(!decision.message.contains("total file"));
    }

    #[test]
    fn animated_quality_verification_rejection_is_failed_not_skipped() {
        let decision = AnimatedGateRejectionDecision::inspect_and_log(
            Path::new("/tmp/test.gif"),
            &foundation::ExploreResult {
                quality_passed: foundation::types::CheckResult::Failed(
                    "SSIM below threshold".to_string(),
                ),
                ssim: Some(0.80),
                actual_min_ssim: 0.95,
                ..Default::default()
            },
        );

        assert!(decision.failed);
        assert!(decision.message.starts_with("Failed:"));
        assert_eq!(decision.reason_code, "quality_failed");
    }

    #[test]
    fn static_animated_probe_rejects_malformed_true_gif() {
        let mut input_file = Builder::new()
            .suffix(".gif")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(b"GIF89a\x01\x00")
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let err = is_static_animated_image(input_file.path()).unwrap_err();

        assert!(
            err.to_string()
                .contains("static animated image analysis failed"),
            "malformed true GIF must fail the probe, got: {err}"
        );
    }

    #[test]
    fn animation_domain_rejects_single_frame_even_without_duration() {
        assert!(animation_analysis_is_static(false, None));
        assert!(animation_analysis_is_static(false, Some(1.0)));
        assert!(!animation_analysis_is_static(true, None));
        assert!(!animation_analysis_is_static(true, Some(1.0)));
    }

    #[test]
    fn loop_intent_probe_failure_returns_no_gif_verdict() {
        let missing = Path::new("/tmp/mfb_missing_loop_probe_input_123456.mp4");

        assert!(assess_loop_intent_for_path(missing).is_none());
    }

    #[test]
    fn fast_gif_loop_intent_probe_failure_returns_error() {
        let missing = Path::new("/tmp/mfb_missing_fast_gif_loop_probe_input_123456.mp4");

        let err = assess_loop_intent_for_fast_gif(missing)
            .expect_err("fast-gif helper must fail closed on probe failure");

        assert!(
            err.to_string()
                .contains("Fast GIF loop intent probe failed")
        );
    }

    #[test]
    fn probe_video_streams_missing_input_returns_error() {
        let missing = Path::new("/tmp/mfb_missing_stream_probe_input_123456.mp4");

        assert!(probe_video_streams(missing).is_err());
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

        let meta = foundation::LoopMeta::from_gif_path(file.path()).unwrap_or_else(|| {
            panic!(
                "short looping GIF at {} must yield loop meta",
                file.path().display()
            )
        });
        let profile = foundation::unit_test_loop_reference_profile();
        let verdict = foundation::evaluate_loop_tree(&meta, Some(&profile)).verdict;

        assert!(
            verdict.is_keep_gif(),
            "expected short looping GIF to stay in GIF domain, got {verdict:?}"
        );
        assert!(
            foundation::should_use_gif_fast_path(file.path()),
            "GIF fast path must be selected for native GIF input"
        );
    }

    #[test]
    fn contract_prepare_early_fallback_copies_original_on_success() {
        // Contract: prepare_early_fallback must copy the original file when possible
        // and return Early(TaskResult) with the copied file, not downgrade to skipped_custom
        let test_data = b"GIF89a\x01\x00\x01\x00\x00\x00\x21\xF9\x04\x00\x0A\x00\x00\x00\x2C\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x01\x00\x00\x3B";
        let mut input_file = Builder::new()
            .suffix(".gif")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(test_data)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let options = ConvertOptions {
            output_dir: input_file.path().parent().map(std::path::Path::to_path_buf),
            ..ConvertOptions::default()
        };

        let result = prepare_early_fallback(
            input_file.path(),
            &options,
            "Test fallback message",
            "test_reason",
        );

        match result {
            PrepareAnimatedRasterOutcome::Early(task) => {
                assert_eq!(task.outcome(), foundation::conversion::Outcome::Failed);
                assert!(!task.message.contains("original copy failed"));
                assert!(task.message.contains("Test fallback message"));
            }
            PrepareAnimatedRasterOutcome::Ready(_) => {
                panic!("prepare_early_fallback should return Early outcome, not Ready");
            }
        }
    }

    #[test]
    fn contract_prepare_early_fallback_handles_missing_input_gracefully() {
        // Contract: when input file doesn't exist, prepare_early_fallback must not panic
        // and should return a sensible error result
        let nonexistent_path = Path::new("/tmp/nonexistent_animated_test_file_12345.gif");
        let options = ConvertOptions::default();

        let result = prepare_early_fallback(
            nonexistent_path,
            &options,
            "Test missing file",
            "missing_input",
        );

        match result {
            PrepareAnimatedRasterOutcome::Early(task) => {
                assert_eq!(task.outcome(), foundation::conversion::Outcome::Failed);
                assert!(task.message.contains("original copy failed"));
            }
            PrepareAnimatedRasterOutcome::Ready(_) => {
                panic!("prepare_early_fallback should return Early outcome for missing input");
            }
        }
    }

    #[test]
    fn contract_animation_routing_passes_through_unknown_formats() {
        // Contract: animation routing must not trust an extension when bytes are unknown.
        let test_data = b"FAKE_FORMAT\x00\x00\x00\x00";
        let mut input_file = Builder::new()
            .suffix(".fake")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(test_data)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let options = ConvertOptions::default();

        let result = prepare_animated_raster_for_encode(
            input_file.path(),
            &options,
            "contract_test_routing",
        );

        match result {
            PrepareAnimatedRasterOutcome::Early(task) => {
                assert!(
                    task.message.contains("Unknown true input format"),
                    "unknown bytes must fail before extension routing: {}",
                    task.message
                );
            }
            PrepareAnimatedRasterOutcome::Ready(prep) => {
                panic!(
                    "Unknown bytes should not pass through as Ready with extension {}",
                    prep.input_ext
                );
            }
        }
    }

    #[test]
    fn contract_animation_routing_rejects_spoofed_jxl_extension() {
        // Contract: fake JXL extensions must fail before djxl routing.
        let test_data = b"FAKE_JXL\x00\x00\x00\x00";
        let mut input_file = Builder::new()
            .suffix(".jxl")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(test_data)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let options = ConvertOptions::default();

        // Temporarily hide djxl from PATH to test fallback
        let original_path = std::env::var("PATH").unwrap_or_default();
        unsafe { std::env::set_var("PATH", "") };

        let result = prepare_animated_raster_for_encode(
            input_file.path(),
            &options,
            "contract_test_jxl_routing",
        );

        // Restore PATH
        unsafe { std::env::set_var("PATH", original_path) };

        match result {
            PrepareAnimatedRasterOutcome::Early(task) => {
                assert!(
                    task.message.contains("Unknown true input format"),
                    "spoofed JXL extension must fail true-format detection: {}",
                    task.message
                );
            }
            PrepareAnimatedRasterOutcome::Ready(_) => {
                panic!("Spoofed JXL extension should return Early fallback, not Ready");
            }
        }
    }

    #[test]
    fn contract_animation_routing_uses_magic_bytes_before_extension() {
        let jxl_box_magic: &[u8] = &[
            0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
        ];
        let mut input_file = Builder::new()
            .suffix(".mp4")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(jxl_box_magic)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let options = ConvertOptions::default();
        let original_path = std::env::var("PATH").unwrap_or_default();
        unsafe { std::env::set_var("PATH", "") };

        let result = prepare_animated_raster_for_encode(
            input_file.path(),
            &options,
            "contract_test_magic_routing",
        );

        unsafe { std::env::set_var("PATH", original_path) };

        match result {
            PrepareAnimatedRasterOutcome::Early(task) => {
                assert!(
                    task.skip_reason.as_deref() == Some("djxl_not_found")
                        || task.skip_reason.as_deref() == Some("djxl_failed"),
                    "JXL magic with fake extension must route through the JXL preprocess gate, got skip_reason={:?} message={}",
                    task.skip_reason,
                    task.message
                );
            }
            PrepareAnimatedRasterOutcome::Ready(prep) => {
                panic!(
                    "JXL magic with fake extension should route as JXL, got {}",
                    prep.input_ext
                );
            }
        }
    }

    #[test]
    fn contract_animation_routing_rejects_spoofed_webp_extension() {
        // Contract: fake WebP extensions must fail before webpmux routing.
        let test_data = b"FAKE_WEBP\x00\x00\x00\x00";
        let mut input_file = Builder::new()
            .suffix(".webp")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(test_data)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let options = ConvertOptions::default();

        // Temporarily hide webpmux from PATH to test fallback
        let original_path = std::env::var("PATH").unwrap_or_default();
        unsafe { std::env::set_var("PATH", "") };

        let result = prepare_animated_raster_for_encode(
            input_file.path(),
            &options,
            "contract_test_webp_routing",
        );

        // Restore PATH
        unsafe { std::env::set_var("PATH", original_path) };

        match result {
            PrepareAnimatedRasterOutcome::Early(task) => {
                assert!(
                    task.message.contains("Unknown true input format"),
                    "spoofed WebP extension must fail true-format detection: {}",
                    task.message
                );
            }
            PrepareAnimatedRasterOutcome::Ready(_) => {
                panic!("Spoofed WebP extension should return Early fallback, not Ready");
            }
        }
    }

    #[test]
    fn contract_animation_routing_gif_passes_through() {
        // Contract: GIF files should pass through without preprocessing
        let gif_data: &[u8] = &[
            b'G', b'I', b'F', b'8', b'9', b'a', 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x2C,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x01, 0x00, 0x00, 0x3B,
        ];

        let mut input_file = Builder::new()
            .suffix(".gif")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(gif_data)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let options = ConvertOptions::default();

        let result = prepare_animated_raster_for_encode(
            input_file.path(),
            &options,
            "contract_test_gif_routing",
        );

        match result {
            PrepareAnimatedRasterOutcome::Ready(prep) => {
                // GIF should pass through unchanged
                assert_eq!(prep.input_ext, "gif");
                assert_eq!(prep.actual_input, input_file.path());
                assert!(prep.temp_apng.is_none());
            }
            PrepareAnimatedRasterOutcome::Early(_) => {
                panic!("GIF should pass through as Ready, not Early");
            }
        }
    }

    #[test]
    fn contract_animation_routing_apng_passes_through() {
        // Contract: APNG files should pass through without preprocessing
        let apng_data: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

        let mut input_file = Builder::new()
            .suffix(".apng")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(apng_data)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let options = ConvertOptions::default();

        let result = prepare_animated_raster_for_encode(
            input_file.path(),
            &options,
            "contract_test_apng_routing",
        );

        match result {
            PrepareAnimatedRasterOutcome::Ready(prep) => {
                // APNG should pass through unchanged
                assert_eq!(prep.input_ext, "apng");
                assert_eq!(prep.actual_input, input_file.path());
                assert!(prep.temp_apng.is_none());
            }
            PrepareAnimatedRasterOutcome::Early(_) => {
                panic!("APNG should pass through as Ready, not Early");
            }
        }
    }

    #[test]
    fn contract_animation_routing_preserves_input_path_on_fallback() {
        // Contract: when routing fails, the fallback must preserve the original input path
        // in the error message for debugging
        let test_data = b"FAKE_JXL\x00\x00\x00\x00";
        let mut input_file = Builder::new()
            .suffix(".jxl")
            .tempfile()
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        input_file
            .write_all(test_data)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let options = ConvertOptions::default();

        let original_path = std::env::var("PATH").unwrap_or_default();
        unsafe { std::env::set_var("PATH", "") };

        let result = prepare_animated_raster_for_encode(
            input_file.path(),
            &options,
            "contract_test_path_preservation",
        );

        unsafe { std::env::set_var("PATH", original_path) };

        match result {
            PrepareAnimatedRasterOutcome::Early(task) => {
                let message = &task.message;
                assert!(
                    message.contains(&input_file.path().display().to_string()),
                    "Fallback message should mention input path"
                );
            }
            PrepareAnimatedRasterOutcome::Ready(_) => {
                panic!("Should return Early on tool missing");
            }
        }
    }
}
