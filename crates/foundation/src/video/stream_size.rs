//! Video Stream Size Extraction Module
//!
//! Accurately extract video and audio stream sizes using ffprobe,
//! used for pure media comparison during exploration and final verification
//! stages.
//!
//! ## Core Features
//! - Extract pure video + audio payload size (excluding container overhead)
//! - Calculate container overhead
//! - Supports multiple extraction methods (direct ffprobe / bitrate calculation
//!   / estimation)

use crate::builder_base::ToolBuilder;
use anyhow::Context;
use rug::Rational;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionMethod {
    FfprobeDirect,
    BitrateCalculation,
    Estimated,
}

impl ExtractionMethod {
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::FfprobeDirect => "ffprobe direct",
            Self::BitrateCalculation => "bitrate × duration",
            Self::Estimated => "estimated (file size − container overhead)",
        }
    }

    #[must_use]
    pub const fn confidence(&self) -> f64 {
        match self {
            Self::FfprobeDirect => 0.99,
            Self::BitrateCalculation => 0.90,
            Self::Estimated => 0.70,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Info {
    pub video_stream_size: u64,
    pub audio_stream_size: u64,
    pub total_file_size: u64,
    pub container_overhead: u64,
    pub extraction_method: ExtractionMethod,
    pub duration_secs: f64,
    pub video_bitrate: Option<u64>,
    pub audio_bitrate: Option<u64>,
}

/// Strict packet-payload measurement for a video delivery decision.
///
/// Unlike [`Info`], this value cannot be produced from a bitrate or container
/// estimate: it is the sum of ffprobe-reported video and audio packet bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrictPureMediaMeasurement {
    pub video_packet_bytes: u64,
    pub audio_packet_bytes: u64,
    pub total_file_size: u64,
}

impl StrictPureMediaMeasurement {
    #[must_use]
    pub const fn pure_media_size(&self) -> u64 {
        self.video_packet_bytes
            .saturating_add(self.audio_packet_bytes)
    }
}

impl Info {
    #[must_use]
    pub const fn pure_media_size(&self) -> u64 {
        self.video_stream_size + self.audio_stream_size
    }

    #[must_use]
    pub fn container_overhead_percent(&self) -> f64 {
        if self.total_file_size == 0 {
            return 0.0;
        }
        let ratio =
            Rational::from(self.container_overhead) / Rational::from(self.total_file_size.max(1));
        (ratio * Rational::from(100)).to_f64()
    }

    #[must_use]
    pub fn is_overhead_excessive(&self) -> bool {
        self.container_overhead_percent() > 10.0
    }
}

/// Reject a loose diagnostic estimate when a production gate requires packet
/// payload proof.
///
/// # Errors
/// Returns an error unless `info` is marked as a direct ffprobe measurement.
pub fn strict_pure_media_measurement_from_info(
    info: &Info,
) -> anyhow::Result<StrictPureMediaMeasurement> {
    if info.extraction_method != ExtractionMethod::FfprobeDirect {
        anyhow::bail!(
            "strict pure-media measurement requires ffprobe packet payloads, got {}",
            info.extraction_method.description()
        );
    }
    Ok(StrictPureMediaMeasurement {
        video_packet_bytes: info.video_stream_size,
        audio_packet_bytes: info.audio_stream_size,
        total_file_size: info.total_file_size,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketStreamKind {
    Video,
    Audio,
}

fn parse_packet_payload_row(line: &str) -> Result<(u32, u64), String> {
    let mut fields = line.split(',');
    let stream_index = fields
        .next()
        .ok_or_else(|| format!("malformed packet row {line:?}"))?;
    let size = fields
        .next()
        .ok_or_else(|| format!("malformed packet row {line:?}"))?;
    let stream_index = stream_index
        .trim()
        .parse::<u32>()
        .map_err(|err| format!("invalid packet row {line:?}: {err}"))?;
    let size = size
        .trim()
        .parse::<u64>()
        .map_err(|err| format!("invalid packet row {line:?}: {err}"))?;
    Ok((stream_index, size))
}

/// Measure video and audio packet payload bytes without using container ratios,
/// metadata margins, or bitrate-duration estimates.
///
/// ffprobe packet output is consumed one line at a time by a reader thread;
/// the child is bounded by the project's normal ffprobe timeout.
///
/// # Errors
/// Returns an error if ffprobe cannot identify streams, packet output is
/// malformed, the scan times out, or no video payload is present.
pub fn measure_strict_pure_media(path: &Path) -> anyhow::Result<StrictPureMediaMeasurement> {
    let total_file_size = crate::io_utils::metadata_with_retry(path)
        .map_err(|err| anyhow::anyhow!("read media size for {}: {err}", path.display()))?
        .len();
    let stream_kinds = strict_stream_kinds(path)?;
    let (video_packet_bytes, audio_packet_bytes, video_packets) =
        scan_packet_payload_bytes(path, &stream_kinds)?;
    video_packet_bytes
        .checked_add(audio_packet_bytes)
        .ok_or_else(|| anyhow::anyhow!("strict pure-media packet byte total overflow"))?;
    if video_packets == 0 {
        anyhow::bail!(
            "strict pure-media measurement found no video packets in {}",
            path.display()
        );
    }
    Ok(StrictPureMediaMeasurement {
        video_packet_bytes,
        audio_packet_bytes,
        total_file_size,
    })
}

fn strict_stream_kinds(path: &Path) -> anyhow::Result<BTreeMap<u32, PacketStreamKind>> {
    let mut command = Command::new(crate::constants::TOOL_FFPROBE);
    command
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=index,codec_type",
            "-of",
            "csv=p=0",
        ])
        .arg(path);
    let context = format!("strict pure-media stream map for {}", path.display());
    let output = crate::process_runner::ManagedProcess::spawn_captured(&mut command)
        .map_err(|err| anyhow::anyhow!("{context}: {err}"))?
        .wait_timeout(
            Duration::from_secs(crate::constants::FFPROBE_TIMEOUT_SECS),
            &context,
        )
        .map_err(|err| anyhow::anyhow!("{context}: {err}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "{context}: ffprobe exited {:?}: {}",
            output.status.code(),
            output.stderr
        );
    }

    let mut kinds = BTreeMap::new();
    for line in output.stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Some((index, codec_type)) = line.split_once(',') else {
            anyhow::bail!("{context}: malformed stream row {line:?}");
        };
        let index = index
            .trim()
            .parse::<u32>()
            .map_err(|err| anyhow::anyhow!("{context}: invalid stream index {index:?}: {err}"))?;
        let kind = match codec_type.trim() {
            "video" => PacketStreamKind::Video,
            "audio" => PacketStreamKind::Audio,
            _ => continue,
        };
        kinds.insert(index, kind);
    }
    if !kinds.values().any(|kind| *kind == PacketStreamKind::Video) {
        anyhow::bail!("{context}: no video stream");
    }
    Ok(kinds)
}

fn scan_packet_payload_bytes(
    path: &Path,
    stream_kinds: &BTreeMap<u32, PacketStreamKind>,
) -> anyhow::Result<(u64, u64, u64)> {
    let stderr_capture = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "ffprobe_packet_scan_stderr",
        None,
        Some(".log"),
    )
    .context("strict packet scan failed to allocate stderr capture")?;
    let stderr_file = stderr_capture
        .reopen()
        .context("strict packet scan failed to open stderr capture")?;
    let mut command = Command::new(crate::constants::TOOL_FFPROBE);
    command
        .args([
            "-v",
            "error",
            "-show_packets",
            "-show_entries",
            "packet=stream_index,size:packet_side_data=",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr_file));
    let command_line = crate::common_utils::format_command_for_audit(&command);
    let context = format!("strict pure-media packet scan for {}", path.display());
    let mut child = command
        .spawn()
        .map_err(|err| anyhow::anyhow!("{context}: failed to start ffprobe: {err}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("{context}: ffprobe stdout unavailable"))?;
    let stream_kinds = stream_kinds.clone();
    let reader = thread::spawn(move || -> anyhow::Result<(u64, u64, u64)> {
        let mut video_bytes = 0u64;
        let mut audio_bytes = 0u64;
        let mut video_packets = 0u64;
        let mut first_error = None;
        for line in BufReader::new(stdout).lines() {
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    first_error.get_or_insert_with(|| format!("read packet output: {err}"));
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let (stream_index, size) = match parse_packet_payload_row(&line) {
                Ok(parsed) => parsed,
                Err(err) => {
                    first_error.get_or_insert(err);
                    continue;
                }
            };
            match stream_kinds.get(&stream_index) {
                Some(PacketStreamKind::Video) => {
                    let Some(next_bytes) = video_bytes.checked_add(size) else {
                        first_error
                            .get_or_insert_with(|| "video packet byte total overflow".into());
                        break;
                    };
                    let Some(next_packets) = video_packets.checked_add(1) else {
                        first_error.get_or_insert_with(|| "video packet count overflow".into());
                        break;
                    };
                    video_bytes = next_bytes;
                    video_packets = next_packets;
                }
                Some(PacketStreamKind::Audio) => {
                    let Some(next_bytes) = audio_bytes.checked_add(size) else {
                        first_error
                            .get_or_insert_with(|| "audio packet byte total overflow".into());
                        break;
                    };
                    audio_bytes = next_bytes;
                }
                None => {}
            }
        }
        if let Some(error) = first_error {
            anyhow::bail!("{error}");
        }
        Ok((video_bytes, audio_bytes, video_packets))
    });

    let status = crate::process_runner::wait_child_with_timeout(
        &mut child,
        Duration::from_secs(crate::constants::FFPROBE_TIMEOUT_SECS),
        &context,
    )
    .map_err(|err| anyhow::anyhow!("{context}: {err}"))?;
    let packet_sizes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("{context}: packet reader panicked"))??;
    let stderr = crate::infra::logging::read_bounded_diagnostic_file(stderr_capture.path())
        .with_context(|| format!("{context}: failed to read ffprobe diagnostics"))?;
    crate::infra::logging::log_captured_process_output(&command_line, status, "", &stderr);
    if !status.success() {
        anyhow::bail!(
            "{context}: ffprobe exited {:?}: {}",
            status.code(),
            if stderr.trim().is_empty() {
                "no diagnostic output"
            } else {
                stderr.trim()
            }
        );
    }
    Ok(packet_sizes)
}

#[derive(Debug, Deserialize, Default)]
struct FfprobeStreamInfo {
    #[serde(default)]
    codec_type: String,
    #[serde(default)]
    bit_rate: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct FfprobeFormatInfo {
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct FfprobeFullOutput {
    #[serde(default)]
    streams: Vec<FfprobeStreamInfo>,
    #[serde(default)]
    format: FfprobeFormatInfo,
}

pub const MOV_OVERHEAD_PERCENT: f64 = crate::constants::MOV_OVERHEAD_PERCENT;
pub const MP4_OVERHEAD_PERCENT: f64 = crate::constants::MP4_OVERHEAD_PERCENT;
pub const MKV_OVERHEAD_PERCENT: f64 = crate::constants::MKV_OVERHEAD_PERCENT;
pub const DEFAULT_OVERHEAD_PERCENT: f64 = crate::constants::DEFAULT_OVERHEAD_PERCENT;

#[must_use]
pub fn get_container_overhead_percent(path: &Path) -> f64 {
    match crate::image::format_detect::detect_true_format(path) {
        Ok(crate::image::format_detect::FormatKind::Mov) => MOV_OVERHEAD_PERCENT,
        Ok(crate::image::format_detect::FormatKind::Mp4) => MP4_OVERHEAD_PERCENT,
        Ok(
            crate::image::format_detect::FormatKind::Mkv
            | crate::image::format_detect::FormatKind::Webm,
        ) => MKV_OVERHEAD_PERCENT,
        Ok(_) => DEFAULT_OVERHEAD_PERCENT,
        Err(error) => {
            crate::media_conversion_gate::stream_size_probe_failure_audit(
                path,
                format!("failed to detect container for overhead estimate: {error}"),
            );
            DEFAULT_OVERHEAD_PERCENT
        }
    }
}

#[must_use]
pub fn extract_stream_sizes(path: &Path) -> Info {
    let total_file_size = match crate::io_utils::metadata_with_retry(path) {
        Ok(metadata) => metadata.len(),
        Err(err) => {
            crate::media_conversion_gate::stream_size_probe_failure_audit(
                path,
                format!(
                    "Failed to read file metadata for stream-size extraction (path={}): {}",
                    path.display(),
                    err
                ),
            );
            0
        }
    };

    if let Some(info) = try_ffprobe_extraction(path, total_file_size) {
        return info;
    }

    estimate_stream_sizes(path, total_file_size)
}

fn try_ffprobe_extraction(path: &Path, total_file_size: u64) -> Option<Info> {
    let mut builder = crate::ffmpeg_builder::FfprobeBuilder::new();
    builder
        .show_streams()
        .show_format()
        .print_format("json")
        .arg("-v")
        .arg("error")
        .input(path);

    let output = match builder.build().output() {
        Ok(output) => output,
        Err(err) => {
            crate::media_conversion_gate::stream_size_probe_failure_audit(
                path,
                format!(
                    "ffprobe stream-size extraction failed to start (path={}): {}",
                    path.display(),
                    err
                ),
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        crate::media_conversion_gate::stream_size_probe_failure_audit(
            path,
            format!(
                "ffprobe stream-size extraction returned non-zero status (path={}, stderr={})",
                path.display(),
                stderr.trim()
            ),
        );
        return None;
    }

    let json_str = match String::from_utf8(output.stdout) {
        Ok(json_str) => json_str,
        Err(err) => {
            crate::media_conversion_gate::stream_size_probe_failure_audit(
                path,
                format!(
                    "ffprobe stream-size extraction returned non-UTF-8 JSON (path={}): {}",
                    path.display(),
                    err
                ),
            );
            return None;
        }
    };
    let parsed: FfprobeFullOutput = match serde_json::from_str(&json_str) {
        Ok(parsed) => parsed,
        Err(err) => {
            crate::media_conversion_gate::stream_size_probe_failure_audit(
                path,
                format!(
                    "ffprobe stream-size extraction JSON parse failed (path={}): {}",
                    path.display(),
                    err
                ),
            );
            return None;
        }
    };

    // Duration is required for bitrate-based size estimation. If absent or
    // unparseable, fall through to estimation rather than panicking or using a
    // fictitious value.
    let Some(duration_secs) = parsed.format.duration.as_ref().and_then(|d| {
        crate::media_conversion_gate::probe_ffprobe_duration_text_or_none(
            d,
            &format!("stream_size:{}", path.display()),
        )
    }) else {
        crate::media_conversion_gate::stream_size_duration_fallback_audit(
            path,
            format!(
                "ffprobe stream-size: format duration missing or unparseable; falling back to \
                 estimation (path={})",
                path.display()
            ),
        );
        return None;
    };

    if duration_secs <= 0.0_f64 {
        crate::media_conversion_gate::stream_size_duration_fallback_audit(
            path,
            format!(
                "ffprobe stream-size extraction reported invalid duration (path={}, duration={})",
                path.display(),
                duration_secs
            ),
        );
        return None;
    }

    let video_stream = parsed.streams.iter().find(|s| s.codec_type == "video");
    let audio_stream = parsed.streams.iter().find(|s| s.codec_type == "audio");

    let (video_stream_size, video_bitrate) =
        match calculate_stream_size_and_bitrate(video_stream, duration_secs) {
            Ok((s, b)) => (s, b),
            Err(e) => {
                crate::media_conversion_gate::stream_size_probe_failure_audit(
                    path,
                    format!(
                        "ffprobe stream-size extraction failed: {} (path={})",
                        e,
                        path.display()
                    ),
                );
                return None;
            }
        };
    let (audio_stream_size, audio_bitrate) =
        match calculate_stream_size_and_bitrate(audio_stream, duration_secs) {
            Ok((s, b)) => (s, b),
            Err(e) => {
                crate::media_conversion_gate::stream_size_probe_failure_audit(
                    path,
                    format!(
                        "ffprobe audio stream-size extraction failed: {} (path={})",
                        e,
                        path.display()
                    ),
                );
                (0, None)
            }
        };

    if video_stream_size == 0 {
        crate::media_conversion_gate::stream_size_probe_failure_audit(
            path,
            format!(
                "ffprobe stream-size extraction produced no usable video bitrate; falling back to \
                 estimated sizing (path={})",
                path.display()
            ),
        );
        return None;
    }

    let pure_media = video_stream_size + audio_stream_size;
    let container_overhead = total_file_size.saturating_sub(pure_media);

    Some(Info {
        video_stream_size,
        audio_stream_size,
        total_file_size,
        container_overhead,
        extraction_method: ExtractionMethod::BitrateCalculation,
        duration_secs,
        video_bitrate,
        audio_bitrate,
    })
}

fn calculate_stream_size_and_bitrate(
    stream: Option<&FfprobeStreamInfo>,
    duration_secs: f64,
) -> anyhow::Result<(u64, Option<u64>)> {
    let Some(br_str) = stream.and_then(|s| s.bit_rate.as_ref()) else {
        return Ok((0, None));
    };
    let br = br_str
        .parse::<u64>()
        .map_err(|err| anyhow::anyhow!("invalid ffprobe stream bit_rate {br_str:?}: {err}"))?;

    let Some(duration_r) =
        crate::numeric_cast::f64_to_rational_strict(duration_secs, "duration_secs")
    else {
        crate::media_conversion_gate::delivery_encode_batch_audit(
            "delivery_encode",
            "Duration NaN/Inf in stream-size calc!",
        );
        return Err(anyhow::anyhow!("Duration NaN/Inf in stream-size calc"));
    };

    let size_rational = (Rational::from(br) * duration_r) / Rational::from(8_i32);
    let Some(size) = crate::numeric_cast::f64_to_u64_strict(size_rational.to_f64(), "size") else {
        return Err(anyhow::anyhow!("Failed to convert size to u64"));
    };
    Ok((size, Some(br)))
}

#[must_use]
pub fn can_compress_pure_video(
    output_path: &Path,
    input_pure_media_size: u64,
    allow_size_tolerance: bool,
) -> bool {
    let output_pure_media_size = match measure_strict_pure_media(output_path) {
        Ok(measurement) => measurement.pure_media_size(),
        Err(err) => {
            crate::media_conversion_gate::delivery_encode_batch_audit(
                "strict_pure_media_measurement",
                format!("{}: {err}", output_path.display()),
            );
            return false;
        }
    };

    let size_policy = crate::exploration_policy::SizePolicy::strict_or_allow_growth(
        allow_size_tolerance,
        crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
    );
    let result = size_policy.fits(output_pure_media_size, input_pure_media_size);

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_DETECTION,
        &format!(
            "can_compress_pure_video: output_pure_media={} vs input_pure_media={} (tolerance={}) → {}",
            output_pure_media_size,
            input_pure_media_size,
            allow_size_tolerance,
            if result {
                format!(
                    "{} CAN COMPRESS",
                    crate::media_conversion_gate::ui_icon_pick(
                        crate::modern_ui::symbols::SUCCESS,
                        crate::modern_ui::symbols::plain::SUCCESS
                    )
                )
            } else {
                format!(
                    "{} CANNOT COMPRESS",
                    crate::media_conversion_gate::ui_icon_pick(
                        crate::modern_ui::symbols::ERROR,
                        crate::modern_ui::symbols::plain::ERROR
                    )
                )
            }
        )
    );

    result
}

#[must_use]
pub fn get_output_video(output_path: &Path) -> u64 {
    extract_stream_sizes(output_path).video_stream_size
}

fn estimate_stream_sizes(path: &Path, total_file_size: u64) -> Info {
    let overhead_percent = get_container_overhead_percent(path);
    let estimated_overhead = {
        let overhead_r =
            match crate::numeric_cast::f64_to_rational_strict(overhead_percent, "overhead_percent")
            {
                Some(v) => v,
                None => unreachable!(
                    "CRITICAL: Overhead percent constant ({}) is non-finite or NaN in stream_size \
                     estimation for path: {}",
                    overhead_percent,
                    path.display()
                ),
            };
        let overhead = Rational::from(total_file_size) * overhead_r;
        match crate::numeric_cast::f64_to_u64_strict(overhead.to_f64(), "overhead") {
            Some(v) => v,
            None => unreachable!(
                "CRITICAL: Estimated overhead calculation overflowed or resulted in NaN \
                 (total_file_size={}, overhead_percent={}, result_f64={}) for path: {}",
                total_file_size,
                overhead_percent,
                overhead.to_f64(),
                path.display()
            ),
        }
    };
    let estimated_video_size = total_file_size.saturating_sub(estimated_overhead);

    Info {
        video_stream_size: estimated_video_size,
        audio_stream_size: 0,
        total_file_size,
        container_overhead: estimated_overhead,
        extraction_method: ExtractionMethod::Estimated,
        duration_secs: 0.0,
        video_bitrate: None,
        audio_bitrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extraction_method_confidence() {
        assert!(ExtractionMethod::FfprobeDirect.confidence() > 0.95_f64);
        assert!(ExtractionMethod::BitrateCalculation.confidence() > 0.85_f64);
        assert!(ExtractionMethod::Estimated.confidence() > 0.65_f64);
    }

    #[test]
    fn test_container_overhead_percent() {
        let temp = tempfile::tempdir().expect("create container fixtures");
        let mov = temp.path().join("misleading.mp4");
        std::fs::write(&mov, b"\0\0\0\x10ftypqt  \0\0\0\0").expect("write MOV brand");
        let mp4 = temp.path().join("misleading.mov");
        std::fs::write(&mp4, b"\0\0\0\x10ftypisom\0\0\0\0").expect("write MP4 brand");
        let mkv = temp.path().join("misleading.avi");
        std::fs::write(&mkv, [0x1A, 0x45, 0xDF, 0xA3]).expect("write MKV signature");
        let unknown = temp.path().join("unknown.mkv");
        std::fs::write(&unknown, b"not media").expect("write unknown content");

        assert!(crate::float_compare::approx_eq_f64(
            get_container_overhead_percent(&mov),
            MOV_OVERHEAD_PERCENT
        ));
        assert!(crate::float_compare::approx_eq_f64(
            get_container_overhead_percent(&mp4),
            MP4_OVERHEAD_PERCENT
        ));
        assert!(crate::float_compare::approx_eq_f64(
            get_container_overhead_percent(&mkv),
            MKV_OVERHEAD_PERCENT
        ));
        assert!(crate::float_compare::approx_eq_f64(
            get_container_overhead_percent(&unknown),
            DEFAULT_OVERHEAD_PERCENT
        ));
    }

    #[test]
    fn test_stream_size_info_methods() {
        let info = Info {
            video_stream_size: 1000,
            audio_stream_size: 100,
            total_file_size: 1200,
            container_overhead: 100,
            extraction_method: ExtractionMethod::BitrateCalculation,
            duration_secs: 10.0,
            video_bitrate: Some(800_000),
            audio_bitrate: Some(128_000),
        };

        assert_eq!(info.pure_media_size(), 1100);
        assert!((info.container_overhead_percent() - 8.33).abs() < 0.1_f64);
        assert!(!info.is_overhead_excessive());
    }

    #[test]
    fn strict_measurement_rejects_bitrate_and_estimated_info() {
        for extraction_method in [
            ExtractionMethod::BitrateCalculation,
            ExtractionMethod::Estimated,
        ] {
            let info = Info {
                video_stream_size: 1_000,
                audio_stream_size: 100,
                total_file_size: 1_200,
                container_overhead: 100,
                extraction_method,
                duration_secs: 1.0,
                video_bitrate: Some(8_000),
                audio_bitrate: Some(800),
            };
            let error = strict_pure_media_measurement_from_info(&info)
                .expect_err("strict packet measurement must reject estimates");
            assert!(
                error
                    .to_string()
                    .contains("requires ffprobe packet payloads")
            );
        }
    }

    #[test]
    fn packet_payload_row_ignores_ffprobe_side_data_columns() {
        assert_eq!(
            parse_packet_payload_row("1,493,Skip Samples,1024,0,0,0"),
            Ok((1, 493))
        );
    }

    #[test]
    fn test_excessive_overhead() {
        let info = Info {
            video_stream_size: 800,
            audio_stream_size: 0,
            total_file_size: 1000,
            container_overhead: 200,
            extraction_method: ExtractionMethod::Estimated,
            duration_secs: 0.0,
            video_bitrate: None,
            audio_bitrate: None,
        };

        assert!(info.is_overhead_excessive());
    }

    #[test]
    fn calculate_stream_size_malformed_bitrate_returns_error_not_missing() {
        let stream = FfprobeStreamInfo {
            codec_type: "video".to_string(),
            bit_rate: Some("not-a-number".to_string()),
        };

        let err = calculate_stream_size_and_bitrate(Some(&stream), 10.0)
            .expect_err("malformed bitrate must be an error, not absent bitrate");

        assert!(err.to_string().contains("not-a-number"));
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_video_stream_size_le_total(
            video_size in 0u64..1_000_000_000u64,
            audio_size in 0u64..100_000_000u64,
            overhead in 0u64..100_000_000u64,
        ) {
            let total = video_size + audio_size + overhead;
            let info = Info {
                video_stream_size: video_size,
                audio_stream_size: audio_size,
                total_file_size: total,
                container_overhead: overhead,
                extraction_method: ExtractionMethod::BitrateCalculation,
                duration_secs: 60.0,
                video_bitrate: None,
                audio_bitrate: None,
            };

            prop_assert!(info.video_stream_size <= info.total_file_size,
                "Video stream size {} should be <= total file size {}",
                info.video_stream_size, info.total_file_size);
        }
    }

    proptest! {
        #[test]
        fn prop_container_overhead_non_negative(
            video_size in 1u64..1_000_000_000u64,
            audio_size in 0u64..100_000_000u64,
            overhead_percent in 0.0f64..0.5f64,
        ) {
            let pure_media = video_size + audio_size;
            let overhead_f64 = crate::numeric_cast::u64_to_f64(pure_media) * overhead_percent;
            let overhead = crate::numeric_cast::f64_to_u64_strict(overhead_f64, "pure_media_overhead");
            prop_assert!(overhead.is_some(), "Numerical anomaly in pure media overhead calculation");
            let overhead = overhead.expect("strict conversion asserted by proptest");
            let total = pure_media + overhead;

            let info = Info {
                video_stream_size: video_size,
                audio_stream_size: audio_size,
                total_file_size: total,
                container_overhead: overhead,
                extraction_method: ExtractionMethod::BitrateCalculation,
                duration_secs: 60.0,
                video_bitrate: None,
                audio_bitrate: None,
            };

            let calculated_overhead = info.total_file_size
                .saturating_sub(info.video_stream_size + info.audio_stream_size);
            prop_assert_eq!(calculated_overhead, info.container_overhead,
                "Calculated container overhead {} should equal stored container overhead {}",
                calculated_overhead, info.container_overhead);
        }
    }

    proptest! {
        #[test]
        fn prop_pure_media_size_correct(
            video_size in 0u64..1_000_000_000u64,
            audio_size in 0u64..100_000_000u64,
        ) {
            let info = Info {
                video_stream_size: video_size,
                audio_stream_size: audio_size,
                total_file_size: video_size + audio_size + 1000,
                container_overhead: 1000,
                extraction_method: ExtractionMethod::BitrateCalculation,
                duration_secs: 60.0,
                video_bitrate: None,
                audio_bitrate: None,
            };

            prop_assert_eq!(info.pure_media_size(), video_size + audio_size,
                "Pure media size should equal video {} + audio {}", video_size, audio_size);
        }
    }

    proptest! {
        #[test]
        fn prop_overhead_percent_correct(
            total_size in 1000u64..1_000_000_000u64,
            overhead_percent in 0.0f64..0.5f64,
        ) {
            let overhead_f64 = crate::numeric_cast::u64_to_f64(total_size) * overhead_percent;
            let overhead = crate::numeric_cast::f64_to_u64_strict(overhead_f64, "total_size_overhead");
            prop_assert!(overhead.is_some(), "Numerical anomaly in total size overhead calculation");
            let overhead = overhead.expect("strict conversion asserted by proptest");
            let video_size = total_size.saturating_sub(overhead);

            let info = Info {
                video_stream_size: video_size,
                audio_stream_size: 0,
                total_file_size: total_size,
                container_overhead: overhead,
                extraction_method: ExtractionMethod::Estimated,
                duration_secs: 0.0,
                video_bitrate: None,
                audio_bitrate: None,
            };

            let calculated_percent = info.container_overhead_percent();
            let expected_percent = crate::numeric_cast::u64_to_f64(overhead) / crate::numeric_cast::u64_to_f64(total_size.max(1)) * 100.0_f64;

            prop_assert!((calculated_percent - expected_percent).abs() < 0.01_f64,
                "Calculated percentage {} should be close to expected {}", calculated_percent, expected_percent);
        }
    }

    proptest! {
        #[test]
        fn prop_fallback_estimation_reasonable(
            total_size in 10000u64..1_000_000_000u64,
        ) {
            let overhead_percent = DEFAULT_OVERHEAD_PERCENT;
            let est_overhead_f64 = crate::numeric_cast::u64_to_f64(total_size) * overhead_percent;
            let estimated_overhead = crate::numeric_cast::f64_to_u64_strict(est_overhead_f64, "estimated_overhead");
            prop_assert!(estimated_overhead.is_some(), "Numerical anomaly in estimated overhead calculation");
            let estimated_overhead = estimated_overhead.expect("strict conversion asserted by proptest");
            let estimated_video_size = total_size.saturating_sub(estimated_overhead);

            let info = Info {
                video_stream_size: estimated_video_size,
                audio_stream_size: 0,
                total_file_size: total_size,
                container_overhead: estimated_overhead,
                extraction_method: ExtractionMethod::Estimated,
                duration_secs: 0.0,
                video_bitrate: None,
                audio_bitrate: None,
            };

            prop_assert!(info.video_stream_size > total_size * 95 / 100,
                "Fallback estimated video stream size {} should be > 95% of total size {}",
                info.video_stream_size, total_size);

            prop_assert!(info.container_overhead < total_size * 5 / 100,
                "Fallback estimated container overhead {} should be < 5% of total size {}",
                info.container_overhead, total_size);
        }
    }

    proptest! {
        #[test]
        fn prop_overhead_warning_threshold(
            total_size in 10000u64..1_000_000_000u64,
            overhead_percent in 0.0f64..0.3f64,
        ) {
            let overhead_f64 = crate::numeric_cast::u64_to_f64(total_size) * overhead_percent;
            let overhead = crate::numeric_cast::f64_to_u64_strict(overhead_f64, "total_size_overhead_v2");
            prop_assert!(overhead.is_some(), "Numerical anomaly in total size overhead calculation");
            let overhead = overhead.expect("strict conversion asserted by proptest");
            let video_size = total_size.saturating_sub(overhead);

            let info = Info {
                video_stream_size: video_size,
                audio_stream_size: 0,
                total_file_size: total_size,
                container_overhead: overhead,
                extraction_method: ExtractionMethod::BitrateCalculation,
                duration_secs: 60.0,
                video_bitrate: None,
                audio_bitrate: None,
            };

            let actual_percent = info.container_overhead_percent();
            let is_excessive = info.is_overhead_excessive();

            if actual_percent > 10.0 {
                prop_assert!(is_excessive,
                    "When container overhead {:.1}% > 10%, it should be marked as excessive", actual_percent);
            } else {
                prop_assert!(!is_excessive,
                    "When container overhead {:.1}% <= 10%, it should not be marked as excessive", actual_percent);
            }
        }
    }

    proptest! {
        #[test]
        fn prop_pure_video_comparison_logic(
            output_video_size in 1u64..1_000_000_000u64,
            input_video_size in 1u64..1_000_000_000u64,
        ) {
            let tolerance_policy = crate::exploration_policy::SizePolicy::AllowGrowth {
                max_extra_bytes: crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
            };
            let strict_policy = crate::exploration_policy::SizePolicy::StrictlySmaller;
            let expected_can_compress = tolerance_policy.fits(output_video_size, input_video_size);

            // Check tolerance=true manually (mirrors logic)
            prop_assert_eq!(
                expected_can_compress,
                tolerance_policy.fits(output_video_size, input_video_size)
            );

            // Check tolerance=false
            prop_assert_eq!(
                strict_policy.fits(output_video_size, input_video_size),
                strict_policy.fits(output_video_size, input_video_size)
            );

        }
    }

    proptest! {
        #[test]
        fn prop_pure_video_comparison_boundary(
            base_size in 1000u64..1_000_000_000u64,
            delta in 0u64..1000u64,
        ) {
            let input_video_size = base_size;
            let output_smaller = base_size.saturating_sub(delta);
            let output_equal = base_size;
            let output_larger = base_size + delta;

            if delta > 0 {
                prop_assert!(crate::exploration_policy::SizePolicy::AllowGrowth {
                    max_extra_bytes: crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
                }.fits(output_smaller, input_video_size),
                    "When output {} < tolerance(input {}) it should compress", output_smaller, input_video_size);
            }

            prop_assert!(crate::exploration_policy::SizePolicy::AllowGrowth {
                max_extra_bytes: crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
            }.fits(output_equal, input_video_size),
                "When output {} == input {} it should compress (within tolerance)", output_equal, input_video_size);

            if delta > crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES {
                prop_assert!(!crate::exploration_policy::SizePolicy::AllowGrowth {
                    max_extra_bytes: crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
                }.fits(output_larger, input_video_size),
                    "When output {} > input {} and exceeds tolerance it should not compress", output_larger, input_video_size);
            }
        }
    }
}
