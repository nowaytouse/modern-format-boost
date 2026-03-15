//! Metadata Parsing Module - 元数据解析模块
//!
//! 本模块负责视频元数据的解析和处理，包括：
//! - 元数据大小计算
//! - 元数据余量计算
//! - 纯视频数据大小提取
//!
//! ## 设计原理
//!
//! 视频文件由两部分组成：
//! 1. 纯视频流数据（实际的编码视频）
//! 2. 容器元数据（文件头、索引、字幕等）
//!
//! 在探索模式中，我们需要精确计算纯视频数据的大小，
//! 以便准确判断压缩效果。

pub const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;

pub const METADATA_MARGIN_MIN: u64 = 2048;

pub const METADATA_MARGIN_MAX: u64 = 102400;

pub const METADATA_MARGIN_PERCENT: f64 = 0.005;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionVerifyStrategy {
    PureVideo,
    TotalSize,
}

#[inline]
pub fn calculate_metadata_margin(input_size: u64) -> u64 {
    let percent_based = (input_size as f64 * METADATA_MARGIN_PERCENT) as u64;
    percent_based.clamp(METADATA_MARGIN_MIN, METADATA_MARGIN_MAX)
}

#[inline]
pub fn detect_metadata_size(pre_metadata_size: u64, post_metadata_size: u64) -> u64 {
    post_metadata_size.saturating_sub(pre_metadata_size)
}

#[inline]
pub fn pure_video_size(total_size: u64, metadata_size: u64) -> u64 {
    total_size.saturating_sub(metadata_size)
}

#[inline]
pub fn compression_target_size(input_size: u64) -> u64 {
    let margin = calculate_metadata_margin(input_size);
    input_size.saturating_sub(margin)
}

#[inline]
pub fn can_compress_with_metadata(output_size: u64, input_size: u64) -> bool {
    output_size < compression_target_size(input_size)
}

#[inline]
pub fn verify_compression_precise(
    output_size: u64,
    input_size: u64,
    actual_metadata_size: u64,
) -> (bool, u64, CompressionVerifyStrategy) {
    if input_size < SMALL_FILE_THRESHOLD {
        let pure_output = pure_video_size(output_size, actual_metadata_size);
        (
            pure_output < input_size,
            pure_output,
            CompressionVerifyStrategy::PureVideo,
        )
    } else {
        (
            output_size < input_size,
            output_size,
            CompressionVerifyStrategy::TotalSize,
        )
    }
}

#[inline]
pub fn verify_compression_simple(
    output_size: u64,
    input_size: u64,
    actual_metadata_size: u64,
) -> (bool, u64) {
    let (can_compress, compare_size, _) =
        verify_compression_precise(output_size, input_size, actual_metadata_size);
    (can_compress, compare_size)
}
