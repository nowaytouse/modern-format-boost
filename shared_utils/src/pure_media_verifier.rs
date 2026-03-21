//! 🔥 v6.7: 纯媒体压缩验证器
//!
//! 使用纯视频流大小进行压缩验证，
//! 完全排除容器格式和元数据的影响。
//!
//! ## 核心逻辑
//! - 主要标准: `output_video_stream_size < input_video_stream_size + 1_048_576`
//! - 只要纯视频流变小或是稍大（增幅 < 1MB），就算成功，无论总文件大小如何

use crate::stream_size::StreamSizeInfo;

#[derive(Debug, Clone)]
pub struct PureMediaVerifyResult {
    pub video_compressed: bool,
    pub input_video_size: u64,
    pub output_video_size: u64,
    pub video_compression_ratio: f64,
    pub total_compression_ratio: f64,
    pub container_overhead_diff: i64,
    pub input_container_overhead: u64,
    pub output_container_overhead: u64,
}

impl PureMediaVerifyResult {
    pub fn video_size_change_percent(&self) -> f64 {
        (self.video_compression_ratio - 1.0) * 100.0
    }

    pub fn total_size_change_percent(&self) -> f64 {
        (self.total_compression_ratio - 1.0) * 100.0
    }

    pub fn is_container_overhead_issue(&self) -> bool {
        self.video_compressed && self.total_compression_ratio >= 1.0
    }

    pub fn description(&self) -> String {
        if self.video_compressed {
            if self.is_container_overhead_issue() {
                format!(
                    "✅ Video compressed ({:+.1}%), but container overhead increased total size ({:+.1}%)",
                    self.video_size_change_percent(),
                    self.total_size_change_percent()
                )
            } else {
                format!(
                    "✅ Compression success: Video {:+.1}%, Total {:+.1}%",
                    self.video_size_change_percent(),
                    self.total_size_change_percent()
                )
            }
        } else {
            format!(
                "❌ Compression failed: Video {:+.1}% (Not smaller)",
                self.video_size_change_percent()
            )
        }
    }
}

pub fn verify_pure_media_compression(
    input_info: &StreamSizeInfo,
    output_info: &StreamSizeInfo,
    allow_size_tolerance: bool,
) -> PureMediaVerifyResult {
    let input_video = input_info.video_stream_size;
    let output_video = output_info.video_stream_size;

    let video_compressed = if allow_size_tolerance {
        output_video < input_video.saturating_add(1_048_576)
    } else {
        output_video < input_video
    };

    let video_compression_ratio = if input_video > 0 {
        output_video as f64 / input_video as f64
    } else {
        1.0
    };

    let total_compression_ratio = if input_info.total_file_size > 0 {
        output_info.total_file_size as f64 / input_info.total_file_size as f64
    } else {
        1.0
    };

    let container_overhead_diff =
        output_info.container_overhead as i64 - input_info.container_overhead as i64;

    PureMediaVerifyResult {
        video_compressed,
        input_video_size: input_video,
        output_video_size: output_video,
        video_compression_ratio,
        total_compression_ratio,
        container_overhead_diff,
        input_container_overhead: input_info.container_overhead,
        output_container_overhead: output_info.container_overhead,
    }
}

#[inline]
pub fn is_video_compressed(
    input_video_size: u64,
    output_video_size: u64,
    allow_size_tolerance: bool,
) -> bool {
    if allow_size_tolerance {
        output_video_size < input_video_size.saturating_add(1_048_576)
    } else {
        output_video_size < input_video_size
    }
}

#[inline]
pub fn video_compression_ratio(input_video_size: u64, output_video_size: u64) -> f64 {
    if input_video_size > 0 {
        output_video_size as f64 / input_video_size as f64
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_size::ExtractionMethod;

    fn make_stream_info(video: u64, audio: u64, overhead: u64) -> StreamSizeInfo {
        StreamSizeInfo {
            video_stream_size: video,
            audio_stream_size: audio,
            total_file_size: video + audio + overhead,
            container_overhead: overhead,
            extraction_method: ExtractionMethod::BitrateCalculation,
            duration_secs: 60.0,
            video_bitrate: None,
            audio_bitrate: None,
        }
    }

    #[test]
    fn test_video_compressed_success() {
        let input = make_stream_info(1000, 100, 50);
        let output = make_stream_info(800, 100, 50);

        let result = verify_pure_media_compression(&input, &output, false);

        assert!(result.video_compressed);
        assert!(result.video_compression_ratio < 1.0);
    }

    #[test]
    fn test_video_compressed_success_within_tolerance() {
        let input = make_stream_info(10_000_000, 100, 50);
        let output = make_stream_info(10_500_000, 100, 50); // 500,000 bytes larger

        let result = verify_pure_media_compression(&input, &output, true);

        assert!(result.video_compressed); // Accepts because < 1_048_576 increase
        assert!(result.video_compression_ratio > 1.0);
    }

    #[test]
    fn test_video_not_compressed_exceeds_tolerance() {
        let input = make_stream_info(1000, 100, 50);
        let output = make_stream_info(1_049_577, 100, 50); // > 1_048_576 bytes larger

        let result = verify_pure_media_compression(&input, &output, true);

        assert!(!result.video_compressed);
        assert!(result.video_compression_ratio > 1.0);
    }

    #[test]
    fn test_container_overhead_issue() {
        let input = make_stream_info(1000, 100, 50);
        let output = make_stream_info(900, 100, 200);

        let result = verify_pure_media_compression(&input, &output, false);

        assert!(result.video_compressed);
        assert!(result.is_container_overhead_issue());
        assert!(result.total_compression_ratio > 1.0);
    }

    #[test]
    fn test_is_video_compressed() {
        assert!(is_video_compressed(1000, 900, false));
        assert!(is_video_compressed(10_000_000, 10_500_000, true)); // Within tolerance
        assert!(!is_video_compressed(10_000, 1_058_577, true)); // Exceeds tolerance
    }

    #[test]
    fn test_video_compression_ratio() {
        assert!((video_compression_ratio(1000, 800) - 0.8).abs() < 0.001);
        assert!((video_compression_ratio(1000, 1000) - 1.0).abs() < 0.001);
        assert!((video_compression_ratio(1000, 1200) - 1.2).abs() < 0.001);
        assert!((video_compression_ratio(0, 100) - 1.0).abs() < 0.001);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use crate::stream_size::ExtractionMethod;
    use proptest::prelude::*;

    fn make_stream_info(video: u64, audio: u64, overhead: u64) -> StreamSizeInfo {
        StreamSizeInfo {
            video_stream_size: video,
            audio_stream_size: audio,
            total_file_size: video + audio + overhead,
            container_overhead: overhead,
            extraction_method: ExtractionMethod::BitrateCalculation,
            duration_secs: 60.0,
            video_bitrate: None,
            audio_bitrate: None,
        }
    }

    proptest! {
        #[test]
        fn prop_compression_judgment_correct(
            input_video in 1000u64..1_000_000_000u64,
            output_video in 1u64..1_000_000_000u64,
            audio in 0u64..100_000_000u64,
            overhead in 0u64..100_000_000u64,
        ) {
            let input = make_stream_info(input_video, audio, overhead);
            let output = make_stream_info(output_video, audio, overhead);

            let result = verify_pure_media_compression(&input, &output, true);

            let expected_compressed = output_video < input_video.saturating_add(1_048_576);
            prop_assert_eq!(result.video_compressed, expected_compressed,
                "When output {} {} input {}, video_compressed should be {}",
                output_video, if expected_compressed { "<" } else { ">=" },
                input_video, expected_compressed);
        }
    }

    proptest! {
        #[test]
        fn prop_compression_ratio_correct(
            input_video in 1u64..1_000_000_000u64,
            output_video in 1u64..1_000_000_000u64,
        ) {
            let ratio = video_compression_ratio(input_video, output_video);
            let expected = output_video as f64 / input_video as f64;

            prop_assert!((ratio - expected).abs() < 0.0001,
                "Compression ratio {} should be close to expected {}", ratio, expected);
        }
    }

    proptest! {
        #[test]
        fn prop_container_overhead_issue_detection(
            input_video in 1000u64..1_000_000_000u64,
            compression_percent in 1u64..50u64,
            input_overhead in 0u64..10_000_000u64,
            extra_overhead in 0u64..100_000_000u64,
        ) {
            let output_video = input_video * (100 - compression_percent) / 100;
            let output_overhead = input_overhead + extra_overhead;

            let input = make_stream_info(input_video, 0, input_overhead);
            let output = make_stream_info(output_video, 0, output_overhead);

            let result = verify_pure_media_compression(&input, &output, true);

            prop_assert!(result.video_compressed,
                "Video compressed from {} to {} should succeed", input_video, output_video);

            let input_total = input.total_file_size;
            let output_total = output.total_file_size;

            if output_total >= input_total {
                prop_assert!(result.is_container_overhead_issue(),
                    "When total file {} >= {} but video successfully compressed, a container overhead issue should be detected",
                    output_total, input_total);
            }
        }
    }
}
