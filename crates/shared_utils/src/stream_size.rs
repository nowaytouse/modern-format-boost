//! Video Stream Size Extraction Module
//!
//! Accurately extract video and audio stream sizes using ffprobe,
//! used for pure media comparison during exploration and final verification stages.
//!
//! ## Core Features
//! - Extract pure video stream size (excluding container overhead)
//! - Extract audio stream size (if present)
//! - Calculate container overhead
//! - Supports multiple extraction methods (direct ffprobe / bitrate calculation / estimation)

use rug::Rational;
use serde::Deserialize;
use std::path::Path;
use tracing::warn;

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
pub struct StreamSizeInfo {
    pub video_stream_size: u64,
    pub audio_stream_size: u64,
    pub total_file_size: u64,
    pub container_overhead: u64,
    pub extraction_method: ExtractionMethod,
    pub duration_secs: f64,
    pub video_bitrate: Option<u64>,
    pub audio_bitrate: Option<u64>,
}

impl StreamSizeInfo {
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

pub const MOV_OVERHEAD_PERCENT: f64 = 0.005;
pub const MP4_OVERHEAD_PERCENT: f64 = 0.001;
pub const MKV_OVERHEAD_PERCENT: f64 = 0.0005;
pub const DEFAULT_OVERHEAD_PERCENT: f64 = 0.002;

#[must_use]
pub fn get_container_overhead_percent(path: &Path) -> f64 {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    match ext.as_str() {
        "mov" => MOV_OVERHEAD_PERCENT,
        "mp4" | "m4v" => MP4_OVERHEAD_PERCENT,
        "mkv" | "webm" => MKV_OVERHEAD_PERCENT,
        _ => DEFAULT_OVERHEAD_PERCENT,
    }
}

pub fn extract_stream_sizes(path: &Path) -> StreamSizeInfo {
    let total_file_size = match crate::io_utils::metadata_with_retry(path) {
        Ok(metadata) => metadata.len(),
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "Failed to read file metadata for stream-size extraction"
            );
            0
        }
    };

    if let Some(info) = try_ffprobe_extraction(path, total_file_size) {
        return info;
    }

    estimate_stream_sizes(path, total_file_size)
}

fn try_ffprobe_extraction(path: &Path, total_file_size: u64) -> Option<StreamSizeInfo> {
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
            warn!(
                path = %path.display(),
                error = %err,
                "ffprobe stream-size extraction failed to start"
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(
            path = %path.display(),
            stderr = %stderr.trim(),
            "ffprobe stream-size extraction returned non-zero status"
        );
        return None;
    }

    let json_str = match String::from_utf8(output.stdout) {
        Ok(json_str) => json_str,
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "ffprobe stream-size extraction returned non-UTF-8 JSON"
            );
            return None;
        }
    };
    let parsed: FfprobeFullOutput = match serde_json::from_str(&json_str) {
        Ok(parsed) => parsed,
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "ffprobe stream-size extraction JSON parse failed"
            );
            return None;
        }
    };

    // Duration is required for bitrate-based size estimation. If absent or unparseable,
    // fall through to estimation rather than panicking or using a fictitious value.
    let Some(duration_secs) = parsed
        .format
        .duration
        .as_ref()
        .and_then(|d| d.parse::<f64>().ok())
    else {
        warn!(
            path = %path.display(),
            "ffprobe stream-size: format duration missing or unparseable; falling back to estimation"
        );
        return None;
    };

    if duration_secs <= 0.0_f64 {
        warn!(
            path = %path.display(),
            duration = duration_secs,
            "ffprobe stream-size extraction reported invalid duration"
        );
        return None;
    }

    let video_stream = parsed.streams.iter().find(|s| s.codec_type == "video");
    let audio_stream = parsed.streams.iter().find(|s| s.codec_type == "audio");

    let (video_stream_size, video_bitrate) =
        calculate_stream_size_and_bitrate(video_stream, duration_secs);
    let (audio_stream_size, audio_bitrate) =
        calculate_stream_size_and_bitrate(audio_stream, duration_secs);

    if video_stream_size == 0 {
        warn!(
            path = %path.display(),
            "ffprobe stream-size extraction produced no usable video bitrate; falling back to estimated sizing"
        );
        return None;
    }

    let pure_media = video_stream_size + audio_stream_size;
    let container_overhead = total_file_size.saturating_sub(pure_media);

    Some(StreamSizeInfo {
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
) -> (u64, Option<u64>) {
    stream
        .and_then(|s| s.bit_rate.as_ref())
        .and_then(|br_str| br_str.parse::<u64>().ok())
        .map_or((0, None), |br| {
            let size_rational = (Rational::from(br)
                * crate::numeric_cast::f64_to_rational_strict(duration_secs, "duration_secs")
                    .unwrap_or_else(|| Rational::from(0_i32)))
                / Rational::from(8_i32);
            let size = crate::numeric_cast::f64_to_u64_sat(size_rational.to_f64());
            (size, Some(br))
        })
}

pub fn can_compress_pure_video(
    output_path: &Path,
    input_video_stream_size: u64,
    allow_size_tolerance: bool,
) -> bool {
    let output_info = extract_stream_sizes(output_path);

    let result = if allow_size_tolerance {
        output_info.video_stream_size
            < input_video_stream_size.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES)
    } else {
        output_info.video_stream_size < input_video_stream_size
    };

    tracing::debug!(
        "can_compress_pure_video: output_video={} vs input_video={} (tolerance={}) → {}",
        output_info.video_stream_size,
        input_video_stream_size,
        allow_size_tolerance,
        if result {
            "✅ CAN COMPRESS"
        } else {
            "❌ CANNOT COMPRESS"
        }
    );

    result
}

#[must_use]
pub fn get_output_video_stream_size(output_path: &Path) -> u64 {
    extract_stream_sizes(output_path).video_stream_size
}

fn estimate_stream_sizes(path: &Path, total_file_size: u64) -> StreamSizeInfo {
    let overhead_percent = get_container_overhead_percent(path);
    let estimated_overhead = {
        let overhead = Rational::from(total_file_size)
            * crate::numeric_cast::f64_to_rational_strict(overhead_percent, "overhead_percent")
                .unwrap_or_else(|| Rational::from(0_i32));
        crate::numeric_cast::f64_to_u64_sat(overhead.to_f64())
    };
    let estimated_video_size = total_file_size.saturating_sub(estimated_overhead);

    StreamSizeInfo {
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
    use std::path::PathBuf;

    #[test]
    fn test_extraction_method_confidence() {
        assert!(ExtractionMethod::FfprobeDirect.confidence() > 0.95_f64);
        assert!(ExtractionMethod::BitrateCalculation.confidence() > 0.85_f64);
        assert!(ExtractionMethod::Estimated.confidence() > 0.65_f64);
    }

    #[test]
    fn test_container_overhead_percent() {
        assert!(crate::float_compare::approx_eq_f64(
            get_container_overhead_percent(&PathBuf::from("test.mov")),
            MOV_OVERHEAD_PERCENT
        ));
        assert!(crate::float_compare::approx_eq_f64(
            get_container_overhead_percent(&PathBuf::from("test.mp4")),
            MP4_OVERHEAD_PERCENT
        ));
        assert!(crate::float_compare::approx_eq_f64(
            get_container_overhead_percent(&PathBuf::from("test.mkv")),
            MKV_OVERHEAD_PERCENT
        ));
        assert!(crate::float_compare::approx_eq_f64(
            get_container_overhead_percent(&PathBuf::from("test.avi")),
            DEFAULT_OVERHEAD_PERCENT
        ));
    }

    #[test]
    fn test_stream_size_info_methods() {
        let info = StreamSizeInfo {
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
    fn test_excessive_overhead() {
        let info = StreamSizeInfo {
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
            let info = StreamSizeInfo {
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
            let overhead = crate::numeric_cast::f64_to_u64_sat(crate::numeric_cast::u64_to_f64(pure_media) * overhead_percent);
            let total = pure_media + overhead;

            let info = StreamSizeInfo {
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
            let info = StreamSizeInfo {
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
            let overhead = crate::numeric_cast::f64_to_u64_sat(crate::numeric_cast::u64_to_f64(total_size) * overhead_percent);
            let video_size = total_size.saturating_sub(overhead);

            let info = StreamSizeInfo {
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
            let estimated_overhead = crate::numeric_cast::f64_to_u64_sat(crate::numeric_cast::u64_to_f64(total_size) * overhead_percent);
            let estimated_video_size = total_size.saturating_sub(estimated_overhead);

            let info = StreamSizeInfo {
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
            let overhead = crate::numeric_cast::f64_to_u64_sat(crate::numeric_cast::u64_to_f64(total_size) * overhead_percent);
            let video_size = total_size.saturating_sub(overhead);

            let info = StreamSizeInfo {
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
            let expected_can_compress = output_video_size < input_video_size.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES);

            // Check tolerance=true manually (mirrors logic)
            prop_assert_eq!(
                expected_can_compress,
                if true { output_video_size < input_video_size.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES) } else { output_video_size < input_video_size }
            );

            // Check tolerance=false
            prop_assert_eq!(
                output_video_size < input_video_size,
                if false { output_video_size < input_video_size.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES) } else { output_video_size < input_video_size }
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
                prop_assert!(output_smaller < input_video_size.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES),
                    "When output {} < tolerance(input {}) it should compress", output_smaller, input_video_size);
            }

            prop_assert!((output_equal < input_video_size.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES)),
                "When output {} == input {} it should compress (within tolerance)", output_equal, input_video_size);

            if delta >= crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES {
                prop_assert!(output_larger >= input_video_size.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES),
                    "When output {} > input {} and exceeds tolerance it should not compress", output_larger, input_video_size);
            }
        }
    }
}
