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
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoEncoder {
    Hevc,
    Av1,
    H264,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncoderPreset {
    Ultrafast,
    Fast,
    #[default]
    Medium,
    Slow,
    Slower,
    Veryslow,
}

impl EncoderPreset {
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

impl VideoEncoder {
    pub fn ffmpeg_name(&self) -> &'static str {
        match self {
            VideoEncoder::Hevc => {
                if Self::is_encoder_available("libx265") {
                    "libx265"
                } else {
                    warn!("⚠️  libx265 not available, falling back to hevc_videotoolbox");
                    "hevc_videotoolbox"
                }
            }
            VideoEncoder::Av1 => "libsvtav1",
            VideoEncoder::H264 => {
                if Self::is_encoder_available("libx264") {
                    "libx264"
                } else {
                    warn!("⚠️  libx264 not available, falling back to h264_videotoolbox");
                    "h264_videotoolbox"
                }
            }
        }
    }

    fn is_encoder_available(encoder: &str) -> bool {
        static LIBX265_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        static LIBX264_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

        let cache = match encoder {
            "libx265" => &LIBX265_AVAILABLE,
            "libx264" => &LIBX264_AVAILABLE,
            _ => return true,
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

    pub fn container(&self) -> &'static str {
        match self {
            VideoEncoder::Hevc => "mp4",
            VideoEncoder::Av1 => "mp4",
            VideoEncoder::H264 => "mp4",
        }
    }

    pub fn extra_args(&self, max_threads: usize) -> Vec<String> {
        self.extra_args_with_preset(max_threads, EncoderPreset::default())
    }

    pub fn extra_args_with_preset(&self, max_threads: usize, preset: EncoderPreset) -> Vec<String> {
        match self {
            VideoEncoder::Hevc => {
                if Self::is_encoder_available("libx265") {
                    vec![
                        "-preset".to_string(),
                        preset.x26x_name().to_string(),
                        "-tag:v".to_string(),
                        "hvc1".to_string(),
                        "-x265-params".to_string(),
                        format!("log-level=error:pools={}", max_threads),
                    ]
                } else {
                    // Fallback for hevc_videotoolbox
                    // Note: videotoolbox doesn't support -x265-params or standard presets
                    vec![
                        "-tag:v".to_string(),
                        "hvc1".to_string(),
                        "-allow_sw".to_string(),
                        "1".to_string(),
                    ]
                }
            }
            VideoEncoder::Av1 => vec![
                "-svtav1-params".to_string(),
                format!(
                    "tune=0:film-grain=0:preset={}:lp={}",
                    preset.svtav1_preset(),
                    max_threads
                ),
            ],
            VideoEncoder::H264 => {
                if Self::is_encoder_available("libx264") {
                    vec![
                        "-preset".to_string(),
                        preset.x26x_name().to_string(),
                        "-profile:v".to_string(),
                        "high".to_string(),
                    ]
                } else {
                    // Fallback for h264_videotoolbox
                    vec![
                        "-profile:v".to_string(),
                        "high".to_string(),
                        "-allow_sw".to_string(),
                        "1".to_string(),
                    ]
                }
            }
        }
    }
}
