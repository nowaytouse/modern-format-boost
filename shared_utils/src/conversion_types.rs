use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Target video format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetVideoFormat {
    /// FFV1 in MKV container - for archival
    Ffv1Mkv,
    /// AV1 in MP4 container - for compression
    Av1Mp4,
    /// HEVC Lossless in MKV container - for archival
    HevcLosslessMkv,
    /// HEVC in MP4 container - for compression
    HevcMp4,
    /// Skip conversion (already modern/efficient)
    Skip,
}

impl TargetVideoFormat {
    pub fn extension(&self) -> &str {
        match self {
            TargetVideoFormat::Ffv1Mkv | TargetVideoFormat::HevcLosslessMkv => "mkv",
            TargetVideoFormat::Av1Mp4 | TargetVideoFormat::HevcMp4 => "mp4",
            TargetVideoFormat::Skip => "",
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            TargetVideoFormat::Ffv1Mkv => "FFV1 MKV (Archival)",
            TargetVideoFormat::Av1Mp4 => "AV1 MP4 (High Quality)",
            TargetVideoFormat::HevcLosslessMkv => "HEVC Lossless MKV (Archival)",
            TargetVideoFormat::HevcMp4 => "HEVC MP4 (High Quality)",
            TargetVideoFormat::Skip => "Skip",
        }
    }
}

/// Conversion strategy result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionStrategy {
    pub target: TargetVideoFormat,
    pub reason: String,
    pub command: String,
    pub preserve_audio: bool,
    /// CRF value (unified to f32 to support both u8 and f32 use cases)
    pub crf: f32,
    pub lossless: bool,
}

/// Unified Conversion Configuration
#[derive(Debug, Clone)]
pub struct ConversionConfig {
    pub output_dir: Option<PathBuf>,
    /// Base directory for preserving directory structure
    pub base_dir: Option<PathBuf>,
    pub force: bool,
    pub delete_original: bool,
    pub preserve_metadata: bool,
    /// Enable size exploration (try higher CRF if output > input)
    pub explore_smaller: bool,
    /// Use mathematical lossless mode
    pub use_lossless: bool,
    /// Match input video quality level
    pub match_quality: bool,
    /// In-place conversion: convert and delete original file
    pub in_place: bool,
    /// Minimum SSIM threshold for quality validation
    pub min_ssim: f64,
    /// Enable VMAF validation (slower but more accurate)
    pub validate_ms_ssim: bool,
    /// Minimum VMAF/MS-SSIM threshold
    pub min_ms_ssim: f64,
    /// Require compression - output must be smaller than input
    pub require_compression: bool,
    /// Apple compatibility mode
    pub apple_compat: bool,
    /// Use GPU acceleration
    pub use_gpu: bool,

    // HEVC specific flags (optional or defaulted for others)
    pub force_ms_ssim_long: bool,
    pub ultimate_mode: bool,

    // 🔥 v7.6: MS-SSIM优化配置
    /// MS-SSIM采样率（1/N，例如3表示1/3采样）
    pub ms_ssim_sampling: Option<u32>,
    /// 强制全量MS-SSIM计算（禁用采样）
    pub full_ms_ssim: bool,
    /// 跳过MS-SSIM计算
    pub skip_ms_ssim: bool,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            output_dir: None,
            base_dir: None,
            force: false,
            delete_original: false,
            preserve_metadata: true,
            explore_smaller: false,
            use_lossless: false,
            match_quality: false,
            in_place: false,
            min_ssim: 0.95,
            validate_ms_ssim: false,
            min_ms_ssim: 0.90,
            require_compression: false,
            apple_compat: false,
            use_gpu: true,
            force_ms_ssim_long: false,
            ultimate_mode: false,
            // 🔥 v7.6: MS-SSIM优化默认值
            ms_ssim_sampling: None, // 自动选择
            full_ms_ssim: false,
            skip_ms_ssim: false,
        }
    }
}

impl ConversionConfig {
    /// Check if original should be deleted (either via delete_original or in_place)
    pub fn should_delete_original(&self) -> bool {
        self.delete_original || self.in_place
    }
}

/// Conversion output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionOutput {
    pub input_path: String,
    pub output_path: String,
    pub strategy: ConversionStrategy,
    pub input_size: u64,
    pub output_size: u64,
    pub size_ratio: f64,
    pub success: bool,
    pub message: String,
    /// CRF used for final output
    pub final_crf: f32,
    /// Number of exploration attempts
    pub exploration_attempts: u8,
}

// Implement CliProcessingResult for ConversionOutput
impl crate::cli_runner::CliProcessingResult for ConversionOutput {
    fn is_skipped(&self) -> bool {
        self.output_size == 0 && self.output_path.is_empty()
    }

    fn skip_reason(&self) -> Option<&str> {
        if self.is_skipped() {
            Some(&self.message)
        } else {
            None
        }
    }

    fn input_path(&self) -> &str {
        &self.input_path
    }
    fn output_path(&self) -> Option<&str> {
        if self.output_path.is_empty() {
            None
        } else {
            Some(&self.output_path)
        }
    }
    fn input_size(&self) -> u64 {
        self.input_size
    }
    fn output_size(&self) -> Option<u64> {
        if self.output_size == 0 {
            None
        } else {
            Some(self.output_size)
        }
    }
    fn message(&self) -> &str {
        &self.message
    }
}
