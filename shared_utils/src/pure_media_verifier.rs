//! 🔥 v6.7: 纯媒体压缩验证器
//!
//! 使用纯视频流大小进行压缩验证，
//! 完全排除容器格式和元数据的影响。
//!
//! ## 核心逻辑
//! - 主要标准: `output_video_stream_size < input_video_stream_size`
//! - 只要纯视频流变小就算成功，无论总文件大小如何

use crate::stream_size::StreamSizeInfo;

// ═══════════════════════════════════════════════════════════════
// 数据结构
// ═══════════════════════════════════════════════════════════════

/// 纯媒体压缩验证结果
#[derive(Debug, Clone)]
pub struct PureMediaVerifyResult {
    /// 纯视频流是否压缩成功
    pub video_compressed: bool,
    /// 输入视频流大小
    pub input_video_size: u64,
    /// 输出视频流大小
    pub output_video_size: u64,
    /// 视频流压缩率（输出/输入，< 1.0 表示压缩成功）
    pub video_compression_ratio: f64,
    /// 总文件压缩率（输出/输入）
    pub total_compression_ratio: f64,
    /// 容器开销差异（输出容器开销 - 输入容器开销）
    pub container_overhead_diff: i64,
    /// 输入容器开销
    pub input_container_overhead: u64,
    /// 输出容器开销
    pub output_container_overhead: u64,
}

impl PureMediaVerifyResult {
    /// 获取视频流压缩百分比（负数表示压缩，正数表示膨胀）
    pub fn video_size_change_percent(&self) -> f64 {
        (self.video_compression_ratio - 1.0) * 100.0
    }

    /// 获取总文件压缩百分比
    pub fn total_size_change_percent(&self) -> f64 {
        (self.total_compression_ratio - 1.0) * 100.0
    }

    /// 检查是否因容器开销导致总文件未压缩
    pub fn is_container_overhead_issue(&self) -> bool {
        // 纯视频压缩成功，但总文件未压缩
        self.video_compressed && self.total_compression_ratio >= 1.0
    }

    /// 获取压缩结果描述
    pub fn description(&self) -> String {
        if self.video_compressed {
            if self.is_container_overhead_issue() {
                format!(
                    "✅ 纯视频压缩成功 ({:+.1}%)，但容器开销导致总文件未压缩 ({:+.1}%)",
                    self.video_size_change_percent(),
                    self.total_size_change_percent()
                )
            } else {
                format!(
                    "✅ 压缩成功：视频 {:+.1}%，总文件 {:+.1}%",
                    self.video_size_change_percent(),
                    self.total_size_change_percent()
                )
            }
        } else {
            format!(
                "❌ 压缩失败：视频 {:+.1}%（未变小）",
                self.video_size_change_percent()
            )
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// 核心验证函数
// ═══════════════════════════════════════════════════════════════

/// 验证纯媒体压缩
///
/// # Arguments
/// * `input_info` - 输入文件的流大小信息
/// * `output_info` - 输出文件的流大小信息
///
/// # Returns
/// `PureMediaVerifyResult` 包含压缩验证结果
///
/// # 核心逻辑
/// 主要标准: `output_video_stream_size < input_video_stream_size`
/// 只要纯视频流变小就算成功，无论总文件大小如何
pub fn verify_pure_media_compression(
    input_info: &StreamSizeInfo,
    output_info: &StreamSizeInfo,
) -> PureMediaVerifyResult {
    let input_video = input_info.video_stream_size;
    let output_video = output_info.video_stream_size;

    // 核心判断：纯视频流是否变小
    let video_compressed = output_video < input_video;

    // 计算压缩率
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

    // 计算容器开销差异
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

/// 快速验证纯视频流是否压缩
///
/// # Arguments
/// * `input_video_size` - 输入视频流大小
/// * `output_video_size` - 输出视频流大小
///
/// # Returns
/// `true` 如果输出视频流 < 输入视频流
#[inline]
pub fn is_video_compressed(input_video_size: u64, output_video_size: u64) -> bool {
    output_video_size < input_video_size
}

/// 计算视频流压缩率
///
/// # Arguments
/// * `input_video_size` - 输入视频流大小
/// * `output_video_size` - 输出视频流大小
///
/// # Returns
/// 压缩率（输出/输入），< 1.0 表示压缩成功
#[inline]
pub fn video_compression_ratio(input_video_size: u64, output_video_size: u64) -> f64 {
    if input_video_size > 0 {
        output_video_size as f64 / input_video_size as f64
    } else {
        1.0
    }
}

// ═══════════════════════════════════════════════════════════════
// 单元测试
// ═══════════════════════════════════════════════════════════════

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

        let result = verify_pure_media_compression(&input, &output);

        assert!(result.video_compressed);
        assert!(result.video_compression_ratio < 1.0);
    }

    #[test]
    fn test_video_not_compressed() {
        let input = make_stream_info(1000, 100, 50);
        let output = make_stream_info(1100, 100, 50);

        let result = verify_pure_media_compression(&input, &output);

        assert!(!result.video_compressed);
        assert!(result.video_compression_ratio > 1.0);
    }

    #[test]
    fn test_container_overhead_issue() {
        // 视频压缩成功，但容器开销导致总文件更大
        let input = make_stream_info(1000, 100, 50); // 总: 1150
        let output = make_stream_info(900, 100, 200); // 总: 1200

        let result = verify_pure_media_compression(&input, &output);

        assert!(result.video_compressed);
        assert!(result.is_container_overhead_issue());
        assert!(result.total_compression_ratio > 1.0);
    }

    #[test]
    fn test_is_video_compressed() {
        assert!(is_video_compressed(1000, 900));
        assert!(!is_video_compressed(1000, 1000));
        assert!(!is_video_compressed(1000, 1100));
    }

    #[test]
    fn test_video_compression_ratio() {
        assert!((video_compression_ratio(1000, 800) - 0.8).abs() < 0.001);
        assert!((video_compression_ratio(1000, 1000) - 1.0).abs() < 0.001);
        assert!((video_compression_ratio(1000, 1200) - 1.2).abs() < 0.001);
        assert!((video_compression_ratio(0, 100) - 1.0).abs() < 0.001);
    }
}

// ═══════════════════════════════════════════════════════════════
// 属性测试
// ═══════════════════════════════════════════════════════════════

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

    // **Feature: container-overhead-fix-v6.7, 属性 3: 纯媒体压缩判断正确性**
    // **验证: 需求 4.1, 4.2**
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

            let result = verify_pure_media_compression(&input, &output);

            // 属性 3: 当 output_video < input_video 时，video_compressed 应为 true
            let expected_compressed = output_video < input_video;
            prop_assert_eq!(result.video_compressed, expected_compressed,
                "当 output {} {} input {} 时，video_compressed 应为 {}",
                output_video, if expected_compressed { "<" } else { ">=" },
                input_video, expected_compressed);
        }
    }

    // **Feature: container-overhead-fix-v6.7, 属性: 压缩率计算正确性**
    proptest! {
        #[test]
        fn prop_compression_ratio_correct(
            input_video in 1u64..1_000_000_000u64,
            output_video in 1u64..1_000_000_000u64,
        ) {
            let ratio = video_compression_ratio(input_video, output_video);
            let expected = output_video as f64 / input_video as f64;

            prop_assert!((ratio - expected).abs() < 0.0001,
                "压缩率 {} 应接近预期 {}", ratio, expected);
        }
    }

    // **Feature: container-overhead-fix-v6.7, 属性: 容器开销问题检测**
    proptest! {
        #[test]
        fn prop_container_overhead_issue_detection(
            input_video in 1000u64..1_000_000_000u64,
            compression_percent in 1u64..50u64,  // 1-50% 压缩
            input_overhead in 0u64..10_000_000u64,
            extra_overhead in 0u64..100_000_000u64,
        ) {
            let output_video = input_video * (100 - compression_percent) / 100;
            let output_overhead = input_overhead + extra_overhead;

            let input = make_stream_info(input_video, 0, input_overhead);
            let output = make_stream_info(output_video, 0, output_overhead);

            let result = verify_pure_media_compression(&input, &output);

            // 视频应该压缩成功
            prop_assert!(result.video_compressed,
                "视频从 {} 压缩到 {} 应该成功", input_video, output_video);

            // 如果额外开销足够大，应该检测到容器开销问题
            let input_total = input.total_file_size;
            let output_total = output.total_file_size;

            if output_total >= input_total {
                prop_assert!(result.is_container_overhead_issue(),
                    "当总文件 {} >= {} 但视频压缩成功时，应检测到容器开销问题",
                    output_total, input_total);
            }
        }
    }
}
