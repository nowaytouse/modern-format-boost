use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetVideoFormat {
    Ffv1Mkv,
    Av1Mp4,
    HevcLosslessMkv,
    HevcMp4,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionStrategy {
    pub target: TargetVideoFormat,
    pub reason: String,
    pub command: String,
    pub preserve_audio: bool,
    pub crf: f32,
    pub lossless: bool,
}

#[derive(Debug, Clone)]
pub struct ConversionConfig {
    pub output_dir: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub force: bool,
    pub delete_original: bool,
    pub explore_smaller: bool,
    pub use_lossless: bool,
    pub match_quality: bool,
    pub in_place: bool,
    pub min_ssim: f64,
    pub require_compression: bool,
    pub apple_compat: bool,
    pub use_gpu: bool,

    pub force_ms_ssim_long: bool,
    pub ultimate_mode: bool,

    pub child_threads: usize,
    /// When true (default): "oversized" threshold is output > input * 1.01. Video path may treat
    /// `video_compression_ratio < 1.01` as acceptable for require_compression / Apple fallback.
    /// Does not relax compress goal: compress still requires output < input.
    pub allow_size_tolerance: bool,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            output_dir: None,
            base_dir: None,
            force: false,
            delete_original: false,
            explore_smaller: false,
            use_lossless: false,
            match_quality: false,
            in_place: false,
            min_ssim: 0.95,
            require_compression: false,
            apple_compat: false,
            use_gpu: true,
            force_ms_ssim_long: false,
            ultimate_mode: false,
            child_threads: 0,
            allow_size_tolerance: true,
        }
    }
}

impl ConversionConfig {
    pub fn should_delete_original(&self) -> bool {
        self.delete_original || self.in_place
    }
}

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
    pub final_crf: f32,
    pub exploration_attempts: u8,
}

impl crate::cli_runner::CliProcessingResult for ConversionOutput {
    fn is_skipped(&self) -> bool {
        self.success && (self.output_size == 0 && self.output_path.is_empty())
    }

    fn is_success(&self) -> bool {
        self.success && !(self.output_size == 0 && self.output_path.is_empty())
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
