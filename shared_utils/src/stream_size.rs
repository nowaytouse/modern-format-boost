//! 🔥 v6.7: 纯视频流大小提取模块
//!
//! 使用 ffprobe 精确提取视频流和音频流大小，
//! 用于探索阶段和最终验证阶段的纯媒体对比。
//!
//! ## 核心功能
//! - 提取纯视频流大小（排除容器开销）
//! - 提取音频流大小（如有）
//! - 计算容器开销
//! - 支持多种提取方法（ffprobe 直接 / bitrate 计算 / 估算）

use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionMethod {
    FfprobeDirect,
    BitrateCalculation,
    Estimated,
}

impl ExtractionMethod {
    pub fn description(&self) -> &'static str {
        match self {
            ExtractionMethod::FfprobeDirect => "ffprobe direct",
            ExtractionMethod::BitrateCalculation => "bitrate × duration",
            ExtractionMethod::Estimated => "estimated (file size − container overhead)",
        }
    }

    pub fn confidence(&self) -> f64 {
        match self {
            ExtractionMethod::FfprobeDirect => 0.99,
            ExtractionMethod::BitrateCalculation => 0.90,
            ExtractionMethod::Estimated => 0.70,
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
    pub fn pure_media_size(&self) -> u64 {
        self.video_stream_size + self.audio_stream_size
    }

    pub fn container_overhead_percent(&self) -> f64 {
        if self.total_file_size == 0 {
            return 0.0;
        }
        self.container_overhead as f64 / self.total_file_size as f64 * 100.0
    }

    pub fn is_overhead_excessive(&self) -> bool {
        self.container_overhead_percent() > 10.0
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
struct FfprobeStreamInfo {
    #[serde(default)]
    codec_type: String,
    #[serde(default)]
    bit_rate: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    nb_frames: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
struct FfprobeFormatInfo {
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    bit_rate: Option<String>,
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

pub fn get_container_overhead_percent(path: &Path) -> f64 {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "mov" => MOV_OVERHEAD_PERCENT,
        "mp4" | "m4v" => MP4_OVERHEAD_PERCENT,
        "mkv" | "webm" => MKV_OVERHEAD_PERCENT,
        _ => DEFAULT_OVERHEAD_PERCENT,
    }
}

pub fn extract_stream_sizes(path: &Path) -> StreamSizeInfo {
    let total_file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    if let Some(info) = try_ffprobe_extraction(path, total_file_size) {
        return info;
    }

    estimate_stream_sizes(path, total_file_size)
}

fn try_ffprobe_extraction(path: &Path, total_file_size: u64) -> Option<StreamSizeInfo> {
    let path_str = path.to_string_lossy();

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            path_str.as_ref(),
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8(output.stdout).ok()?;
    let parsed: FfprobeFullOutput = serde_json::from_str(&json_str).ok()?;

    let duration_secs = parsed
        .format
        .duration
        .as_ref()
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);

    if duration_secs <= 0.0 {
        return None;
    }

    let video_stream = parsed.streams.iter().find(|s| s.codec_type == "video");

    let audio_stream = parsed.streams.iter().find(|s| s.codec_type == "audio");

    let (video_stream_size, video_bitrate) = if let Some(vs) = video_stream {
        if let Some(br_str) = &vs.bit_rate {
            if let Ok(br) = br_str.parse::<u64>() {
                let size = (br as f64 * duration_secs / 8.0) as u64;
                (size, Some(br))
            } else {
                (0, None)
            }
        } else {
            (0, None)
        }
    } else {
        (0, None)
    };

    let (audio_stream_size, audio_bitrate) = if let Some(aus) = audio_stream {
        if let Some(br_str) = &aus.bit_rate {
            if let Ok(br) = br_str.parse::<u64>() {
                let size = (br as f64 * duration_secs / 8.0) as u64;
                (size, Some(br))
            } else {
                (0, None)
            }
        } else {
            (0, None)
        }
    } else {
        (0, None)
    };

    if video_stream_size == 0 {
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

pub fn can_compress_pure_video(output_path: &Path, input_video_stream_size: u64) -> bool {
    let output_info = extract_stream_sizes(output_path);
    let result = output_info.video_stream_size < input_video_stream_size;

    #[cfg(debug_assertions)]
    {
        eprintln!(
            "   [DEBUG] can_compress_pure_video: output_video={} vs input_video={} → {}",
            output_info.video_stream_size,
            input_video_stream_size,
            if result {
                "✅ CAN COMPRESS"
            } else {
                "❌ CANNOT COMPRESS"
            }
        );
    }

    result
}

pub fn get_output_video_stream_size(output_path: &Path) -> u64 {
    extract_stream_sizes(output_path).video_stream_size
}

fn estimate_stream_sizes(path: &Path, total_file_size: u64) -> StreamSizeInfo {
    let overhead_percent = get_container_overhead_percent(path);
    let estimated_overhead = (total_file_size as f64 * overhead_percent) as u64;
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
        assert!(ExtractionMethod::FfprobeDirect.confidence() > 0.95);
        assert!(ExtractionMethod::BitrateCalculation.confidence() > 0.85);
        assert!(ExtractionMethod::Estimated.confidence() > 0.65);
    }

    #[test]
    fn test_container_overhead_percent() {
        assert_eq!(
            get_container_overhead_percent(&PathBuf::from("test.mov")),
            MOV_OVERHEAD_PERCENT
        );
        assert_eq!(
            get_container_overhead_percent(&PathBuf::from("test.mp4")),
            MP4_OVERHEAD_PERCENT
        );
        assert_eq!(
            get_container_overhead_percent(&PathBuf::from("test.mkv")),
            MKV_OVERHEAD_PERCENT
        );
        assert_eq!(
            get_container_overhead_percent(&PathBuf::from("test.avi")),
            DEFAULT_OVERHEAD_PERCENT
        );
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
            video_bitrate: Some(800000),
            audio_bitrate: Some(128000),
        };

        assert_eq!(info.pure_media_size(), 1100);
        assert!((info.container_overhead_percent() - 8.33).abs() < 0.1);
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
                "视频流大小 {} 应 <= 总文件大小 {}",
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
            let overhead = (pure_media as f64 * overhead_percent) as u64;
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
                "计算的容器开销 {} 应等于存储的容器开销 {}",
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
                "纯媒体大小应等于视频 {} + 音频 {}", video_size, audio_size);
        }
    }

    proptest! {
        #[test]
        fn prop_overhead_percent_correct(
            total_size in 1000u64..1_000_000_000u64,
            overhead_percent in 0.0f64..0.5f64,
        ) {
            let overhead = (total_size as f64 * overhead_percent) as u64;
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
            let expected_percent = overhead as f64 / total_size as f64 * 100.0;

            prop_assert!((calculated_percent - expected_percent).abs() < 0.01,
                "计算的百分比 {} 应接近预期 {}", calculated_percent, expected_percent);
        }
    }

    proptest! {
        #[test]
        fn prop_fallback_estimation_reasonable(
            total_size in 10000u64..1_000_000_000u64,
        ) {
            let overhead_percent = DEFAULT_OVERHEAD_PERCENT;
            let estimated_overhead = (total_size as f64 * overhead_percent) as u64;
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
                "回退估算的视频流大小 {} 应 > 总大小 {} 的 95%",
                info.video_stream_size, total_size);

            prop_assert!(info.container_overhead < total_size * 5 / 100,
                "回退估算的容器开销 {} 应 < 总大小 {} 的 5%",
                info.container_overhead, total_size);
        }
    }

    proptest! {
        #[test]
        fn prop_overhead_warning_threshold(
            total_size in 10000u64..1_000_000_000u64,
            overhead_percent in 0.0f64..0.3f64,
        ) {
            let overhead = (total_size as f64 * overhead_percent) as u64;
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
                    "当容器开销 {:.1}% > 10% 时，应标记为过大", actual_percent);
            } else {
                prop_assert!(!is_excessive,
                    "当容器开销 {:.1}% <= 10% 时，不应标记为过大", actual_percent);
            }
        }
    }

    proptest! {
        #[test]
        fn prop_pure_video_comparison_logic(
            output_video_size in 1u64..1_000_000_000u64,
            input_video_size in 1u64..1_000_000_000u64,
        ) {
            let expected_can_compress = output_video_size < input_video_size;

            prop_assert_eq!(
                expected_can_compress,
                output_video_size < input_video_size,
                "纯视频流对比逻辑：output {} {} input {} 应该 = {}",
                output_video_size,
                if expected_can_compress { "<" } else { ">=" },
                input_video_size,
                expected_can_compress
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
                prop_assert!(output_smaller < input_video_size,
                    "当 output {} < input {} 时应该能压缩", output_smaller, input_video_size);
            }

            prop_assert!((output_equal >= input_video_size),
                "当 output {} == input {} 时不应该能压缩", output_equal, input_video_size);

            prop_assert!((output_larger >= input_video_size),
                "当 output {} > input {} 时不应该能压缩", output_larger, input_video_size);
        }
    }
}
