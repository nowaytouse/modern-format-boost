//! Codec Detection Module - 编解码器检测模块
//!
//! 本模块负责视频编解码器的检测和配置，包括：
//! - 编码器类型检测（HEVC/AV1/H264）
//! - 编码器可用性检测
//! - 编码器参数配置
//! - Preset 配置
//!
//! ## 支持的编码器
//!
//! - **HEVC/H.265**: libx265 (CPU) / hevc_videotoolbox (GPU)
//! - **AV1**: libsvtav1
//! - **H.264**: libx264 (CPU) / h264_videotoolbox (GPU)
//!
//! ## Preset 说明
//!
//! Preset 控制编码速度和质量的权衡：
//! - `ultrafast`: 最快，质量最低
//! - `fast`: 快速，适合实时编码
//! - `medium`: 默认，平衡速度和质量
//! - `slow`: 慢速，更好的压缩率
//! - `slower`: 非常慢，最佳压缩率（推荐）
//! - `veryslow`: 极慢，极致压缩

use std::process::Command;

// ═══════════════════════════════════════════════════════════════
// 类型定义
// ═══════════════════════════════════════════════════════════════

/// 视频编码器类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoEncoder {
    /// HEVC/H.265 (libx265)
    Hevc,
    /// AV1 (libsvtav1)
    Av1,
    /// H.264 (libx264)
    H264,
}

/// 编码器 Preset（速度/质量权衡）
///
/// 🔥 重要：探索模式必须使用与最终压制相同的 preset！
/// 否则探索出的 CRF 在最终压制时会产生不同的文件大小。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum EncoderPreset {
    /// 最快（质量最低，仅用于测试）
    Ultrafast,
    /// 快速（适合实时编码）
    Fast,
    /// 中等（默认，平衡速度和质量）
    #[default]
    Medium,
    /// 慢速（更好的压缩率）
    Slow,
    /// 非常慢（最佳压缩率，推荐用于最终输出）
    Slower,
    /// 极慢（极致压缩，耗时很长）
    Veryslow,
}

// ═══════════════════════════════════════════════════════════════
// EncoderPreset 实现
// ═══════════════════════════════════════════════════════════════

impl EncoderPreset {
    /// 获取 x265/x264 preset 字符串
    pub fn x26x_name(&self) -> &'static str {
        match self {
            EncoderPreset::Ultrafast => "ultrafast",
            EncoderPreset::Fast => "fast",
            EncoderPreset::Medium => "medium",
            EncoderPreset::Slow => "slow",
            EncoderPreset::Slower => "slower",
            EncoderPreset::Veryslow => "veryslow",
        }
    }

    /// 获取 SVT-AV1 preset 数字 (0-13, 0=最慢最好, 13=最快最差)
    pub fn svtav1_preset(&self) -> u8 {
        match self {
            EncoderPreset::Ultrafast => 12,
            EncoderPreset::Fast => 8,
            EncoderPreset::Medium => 6,
            EncoderPreset::Slow => 4,
            EncoderPreset::Slower => 2,
            EncoderPreset::Veryslow => 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// VideoEncoder 实现
// ═══════════════════════════════════════════════════════════════

impl VideoEncoder {
    /// 获取 ffmpeg 编码器名称
    /// 🔥 v6.9.17: 动态检测可用编码器，回退到硬件加速
    pub fn ffmpeg_name(&self) -> &'static str {
        match self {
            VideoEncoder::Hevc => {
                // 🔥 检测 libx265 是否可用，不可用则回退到 hevc_videotoolbox
                if Self::is_encoder_available("libx265") {
                    "libx265"
                } else {
                    eprintln!("⚠️  libx265 not available, falling back to hevc_videotoolbox");
                    "hevc_videotoolbox"
                }
            }
            VideoEncoder::Av1 => "libsvtav1",
            VideoEncoder::H264 => {
                // 🔥 检测 libx264 是否可用，不可用则回退到 h264_videotoolbox
                if Self::is_encoder_available("libx264") {
                    "libx264"
                } else {
                    eprintln!("⚠️  libx264 not available, falling back to h264_videotoolbox");
                    "h264_videotoolbox"
                }
            }
        }
    }

    /// 🔥 v6.9.17: 检测编码器是否可用
    fn is_encoder_available(encoder: &str) -> bool {
        // 缓存检测结果避免重复调用
        static LIBX265_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        static LIBX264_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

        let cache = match encoder {
            "libx265" => &LIBX265_AVAILABLE,
            "libx264" => &LIBX264_AVAILABLE,
            _ => return true, // 其他编码器假设可用
        };

        *cache.get_or_init(|| {
            Command::new("ffmpeg")
                .args(["-hide_banner", "-encoders"])
                .output()
                .ok()
                .map(|output| {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    stdout.contains(encoder)
                })
                .unwrap_or(false)
        })
    }

    /// 获取输出容器格式
    pub fn container(&self) -> &'static str {
        match self {
            VideoEncoder::Hevc => "mp4",
            VideoEncoder::Av1 => "mp4",
            VideoEncoder::H264 => "mp4",
        }
    }

    /// 获取额外的编码器参数（使用默认 preset）
    pub fn extra_args(&self, max_threads: usize) -> Vec<String> {
        self.extra_args_with_preset(max_threads, EncoderPreset::default())
    }

    /// 🔥 v5.74: 获取额外的编码器参数（指定 preset）
    ///
    /// # Arguments
    /// * `max_threads` - 最大线程数
    /// * `preset` - 编码器 preset
    ///
    /// # 重要
    /// 探索模式和最终压制必须使用相同的 preset！
    pub fn extra_args_with_preset(&self, max_threads: usize, preset: EncoderPreset) -> Vec<String> {
        match self {
            VideoEncoder::Hevc => vec![
                "-preset".to_string(),
                preset.x26x_name().to_string(),
                "-tag:v".to_string(),
                "hvc1".to_string(),
                "-x265-params".to_string(),
                format!("log-level=error:pools={}", max_threads),
            ],
            VideoEncoder::Av1 => vec![
                "-svtav1-params".to_string(),
                format!(
                    "tune=0:film-grain=0:preset={}:lp={}",
                    preset.svtav1_preset(),
                    max_threads
                ),
            ],
            VideoEncoder::H264 => vec![
                "-preset".to_string(),
                preset.x26x_name().to_string(),
                "-profile:v".to_string(),
                "high".to_string(),
            ],
        }
    }
}
