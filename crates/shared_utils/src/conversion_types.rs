use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetVideoFormat {
    Ffv1Mkv,
    Av1Mp4,
    /// AV2 (`AOMedia` Video 2) - Next-generation codec, experimental
    Av2Mp4,
    /// VVC (H.266) - High efficiency codec, patent-encumbered
    VvcMp4,
    Gif,
    HevcLosslessMkv,
    HevcMov,
    HevcMp4,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SelectedCodec {
    #[default]
    Hevc,
    Av1,
    /// AV2 (`AOMedia` Video 2) - Experimental, requires libaom 4.0+
    Av2,
    /// VVC (H.266) - High efficiency, patent-encumbered
    Vvc,
}
bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ConfigFlags: u32 {
        const FORCE = 1 << 0;
        const DELETE_ORIGINAL = 1 << 1;
        const EXPLORE_SMALLER = 1 << 2;
        const USE_LOSSLESS = 1 << 3;
        const MATCH_QUALITY = 1 << 4;
        const IN_PLACE = 1 << 5;
        const REQUIRE_COMPRESSION = 1 << 6;
        const APPLE_COMPAT = 1 << 7;
        const USE_GPU = 1 << 8;
        const FORCE_MS_SSIM_LONG = 1 << 9;
        const ULTIMATE_MODE = 1 << 10;
        const ALLOW_SIZE_TOLERANCE = 1 << 11;
    }
}

impl SelectedCodec {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::Av2 => "av2",
            Self::Vvc => "vvc",
        }
    }

    /// Returns true if this codec is experimental/bleeding-edge
    #[must_use]
    pub const fn is_experimental(&self) -> bool {
        matches!(self, Self::Av2 | Self::Vvc)
    }

    /// Returns the minimum encoder version required
    #[must_use]
    pub const fn min_encoder_version(&self) -> Option<&str> {
        match self {
            Self::Av2 => Some("libaom 4.0.0"),
            Self::Vvc => Some("vvenc 1.9.0"),
            _ => None,
        }
    }
}

impl TargetVideoFormat {
    #[must_use]
    pub const fn extension(&self) -> &str {
        match self {
            Self::Ffv1Mkv | Self::HevcLosslessMkv => "MKV",
            Self::HevcMov => "MOV",
            Self::Av1Mp4 | Self::Av2Mp4 | Self::VvcMp4 | Self::HevcMp4 => "MP4",
            Self::Gif => "GIF",
            Self::Skip => "",
        }
    }

    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Ffv1Mkv => "FFV1 MKV (Archival)",
            Self::Av1Mp4 => "AV1 MP4 (High Quality)",
            Self::Av2Mp4 => "AV2 MP4 (Experimental)",
            Self::VvcMp4 => "VVC MP4 (H.266)",
            Self::Gif => "GIF (Loop Asset)",
            Self::HevcLosslessMkv => "HEVC Lossless MKV (Archival)",
            Self::HevcMov => "HEVC MOV (Apple Compatible)",
            Self::HevcMp4 => "HEVC MP4 (High Quality)",
            Self::Skip => "Skip",
        }
    }

    /// Returns true if this format is experimental/bleeding-edge
    #[must_use]
    pub const fn is_experimental(&self) -> bool {
        matches!(self, Self::Av2Mp4 | Self::VvcMp4)
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
    pub flags: ConfigFlags,
    pub min_ssim: f64,
    pub child_threads: usize,
    pub codec: SelectedCodec,
}

impl ConversionConfig {
    #[must_use]
    pub const fn force(&self) -> bool {
        self.flags.contains(ConfigFlags::FORCE)
    }
    #[must_use]
    pub const fn delete_original(&self) -> bool {
        self.flags.contains(ConfigFlags::DELETE_ORIGINAL)
    }
    #[must_use]
    pub const fn explore_smaller(&self) -> bool {
        self.flags.contains(ConfigFlags::EXPLORE_SMALLER)
    }
    #[must_use]
    pub const fn use_lossless(&self) -> bool {
        self.flags.contains(ConfigFlags::USE_LOSSLESS)
    }
    #[must_use]
    pub const fn match_quality(&self) -> bool {
        self.flags.contains(ConfigFlags::MATCH_QUALITY)
    }
    #[must_use]
    pub const fn in_place(&self) -> bool {
        self.flags.contains(ConfigFlags::IN_PLACE)
    }
    #[must_use]
    pub const fn require_compression(&self) -> bool {
        self.flags.contains(ConfigFlags::REQUIRE_COMPRESSION)
    }
    #[must_use]
    pub const fn apple_compat(&self) -> bool {
        self.flags.contains(ConfigFlags::APPLE_COMPAT)
    }
    #[must_use]
    pub const fn use_gpu(&self) -> bool {
        self.flags.contains(ConfigFlags::USE_GPU)
    }
    #[must_use]
    pub const fn force_ms_ssim_long(&self) -> bool {
        self.flags.contains(ConfigFlags::FORCE_MS_SSIM_LONG)
    }
    #[must_use]
    pub const fn ultimate_mode(&self) -> bool {
        self.flags.contains(ConfigFlags::ULTIMATE_MODE)
    }
    #[must_use]
    pub const fn allow_size_tolerance(&self) -> bool {
        self.flags.contains(ConfigFlags::ALLOW_SIZE_TOLERANCE)
    }
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            output_dir: None,
            base_dir: None,
            flags: ConfigFlags::USE_GPU | ConfigFlags::ALLOW_SIZE_TOLERANCE,
            min_ssim: 0.95,
            child_threads: 0,
            codec: SelectedCodec::Hevc,
        }
    }
}

impl ConversionConfig {
    #[must_use]
    pub const fn should_delete_original(&self) -> bool {
        self.delete_original() || self.in_place()
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
    pub blake3: Option<String>,
}

impl ConversionOutput {
    #[must_use]
    pub fn outcome(&self) -> crate::conversion::ConversionOutcome {
        if !self.success {
            return crate::conversion::ConversionOutcome::Failed;
        }

        if self.strategy.target == TargetVideoFormat::Skip
            || self.output_path.is_empty()
            || self.output_size == 0
        {
            return crate::conversion::ConversionOutcome::Skipped;
        }

        crate::conversion::ConversionOutcome::Converted
    }
}

impl crate::cli_runner::CliProcessingResult for ConversionOutput {
    fn is_skipped(&self) -> bool {
        self.outcome() == crate::conversion::ConversionOutcome::Skipped
    }

    fn is_success(&self) -> bool {
        self.outcome() == crate::conversion::ConversionOutcome::Converted
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
    fn blake3(&self) -> Option<&str> {
        self.blake3.as_deref()
    }
}
