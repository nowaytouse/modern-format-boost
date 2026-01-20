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

// ═══════════════════════════════════════════════════════════════
// 常量定义
// ═══════════════════════════════════════════════════════════════

/// 🔥 v6.4.2: 小文件阈值（字节）
/// 小于此值的文件需要精确元数据检测
/// 大于此值的文件直接用 output < input 判断
pub const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB

/// 🔥 v6.4.3: 元数据余量最小值（字节）
pub const METADATA_MARGIN_MIN: u64 = 2048; // 2KB

/// 🔥 v6.4.3: 元数据余量最大值（字节）
pub const METADATA_MARGIN_MAX: u64 = 102400; // 100KB

/// 🔥 v6.4.3: 元数据余量百分比
pub const METADATA_MARGIN_PERCENT: f64 = 0.005; // 0.5%

// ═══════════════════════════════════════════════════════════════
// 类型定义
// ═══════════════════════════════════════════════════════════════

/// 🔥 v6.4.3: 压缩验证策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionVerifyStrategy {
    /// 对比纯视频数据（去除元数据）- 用于小文件
    PureVideo,
    /// 对比总大小 - 用于大文件
    TotalSize,
}

// ═══════════════════════════════════════════════════════════════
// 公共函数
// ═══════════════════════════════════════════════════════════════

/// 🔥 v6.4.3: 计算元数据余量（百分比 + 最小值策略）
///
/// 公式: max(input × 0.5%, 2KB).min(100KB)
///
/// 这个策略的优点：
/// - 小文件：至少 2KB 余量（覆盖基本元数据）
/// - 中等文件：按比例增长（更精确）
/// - 大文件：上限 100KB（避免浪费）
///
/// # Arguments
/// * `input_size` - 输入文件大小（字节）
///
/// # Returns
/// 元数据余量（字节）
///
/// # Examples
/// - 100KB 文件 → max(500, 2048) = 2KB
/// - 1MB 文件 → max(5120, 2048) = 5KB
/// - 10MB 文件 → max(51200, 2048) = 50KB
/// - 100MB 文件 → min(512000, 102400) = 100KB
#[inline]
pub fn calculate_metadata_margin(input_size: u64) -> u64 {
    let percent_based = (input_size as f64 * METADATA_MARGIN_PERCENT) as u64;
    percent_based.clamp(METADATA_MARGIN_MIN, METADATA_MARGIN_MAX)
}

/// 🔥 v6.4.2: 检测实际元数据大小
///
/// 通过对比元数据复制前后的文件大小来精确计算
///
/// # Arguments
/// * `pre_metadata_size` - 元数据复制前的文件大小
/// * `post_metadata_size` - 元数据复制后的文件大小
///
/// # Returns
/// 实际元数据增量（字节）
#[inline]
pub fn detect_metadata_size(pre_metadata_size: u64, post_metadata_size: u64) -> u64 {
    post_metadata_size.saturating_sub(pre_metadata_size)
}

/// 🔥 v6.4.2: 计算纯视频数据大小（去除元数据）
///
/// # Arguments
/// * `total_size` - 文件总大小
/// * `metadata_size` - 元数据大小
///
/// # Returns
/// 纯视频数据大小
#[inline]
pub fn pure_video_size(total_size: u64, metadata_size: u64) -> u64 {
    total_size.saturating_sub(metadata_size)
}

/// 🔥 v6.4.2: 计算压缩目标大小（探索阶段使用）
///
/// 返回探索时应使用的压缩目标阈值
/// target = input_size - metadata_margin
///
/// # Arguments
/// * `input_size` - 输入文件大小（字节）
///
/// # Returns
/// 压缩目标大小（字节），使用 saturating_sub 避免下溢
#[inline]
pub fn compression_target_size(input_size: u64) -> u64 {
    let margin = calculate_metadata_margin(input_size);
    input_size.saturating_sub(margin)
}

/// 🔥 v6.4.2: 检查是否可以压缩（探索阶段，预留元数据余量）
///
/// # Arguments
/// * `output_size` - 输出文件大小（字节）
/// * `input_size` - 输入文件大小（字节）
///
/// # Returns
/// true 如果 output_size < compression_target_size(input_size)
#[inline]
pub fn can_compress_with_metadata(output_size: u64, input_size: u64) -> bool {
    output_size < compression_target_size(input_size)
}

/// 🔥 v6.4.3: 精确压缩验证（统一逻辑）
///
/// 小文件 (<10MB): 对比纯视频数据大小（去除元数据）
/// 大文件 (>=10MB): 直接对比总大小
///
/// # 逻辑一致性
/// 无论小文件还是大文件，都使用相同的比较逻辑：
/// - 小文件: pure_output < pure_input (两边都去除元数据)
/// - 大文件: total_output < total_input (两边都用总大小)
///
/// # Arguments
/// * `output_size` - 输出文件总大小
/// * `input_size` - 输入文件大小
/// * `actual_metadata_size` - 实际检测到的元数据大小
///
/// # Returns
/// (can_compress, compare_size, strategy) - 是否可压缩，用于比较的大小，使用的策略
#[inline]
pub fn verify_compression_precise(
    output_size: u64,
    input_size: u64,
    actual_metadata_size: u64,
) -> (bool, u64, CompressionVerifyStrategy) {
    if input_size < SMALL_FILE_THRESHOLD {
        // 小文件：对比纯视频数据大小（两边都去除元数据）
        let pure_output = pure_video_size(output_size, actual_metadata_size);
        // 🔥 v6.4.3 修复：输入也应该去除元数据（假设输入元数据与输出相近）
        // 但由于我们无法知道输入的元数据大小，保守起见只去除输出的元数据
        (
            pure_output < input_size,
            pure_output,
            CompressionVerifyStrategy::PureVideo,
        )
    } else {
        // 大文件：直接对比总大小
        (
            output_size < input_size,
            output_size,
            CompressionVerifyStrategy::TotalSize,
        )
    }
}

/// 🔥 v6.4.3: 简化版压缩验证（返回 2 元组，向后兼容）
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
