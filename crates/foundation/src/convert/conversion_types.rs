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
    /// Explicit semantic for assets outside the current tool's processing
    /// domain.
    Ignored,
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
        const MATCH_QUALITY = 1 << 4;
        const IN_PLACE = 1 << 5;
        const REQUIRE_COMPRESSION = 1 << 6;
        const APPLE_COMPAT = 1 << 7;
        const USE_GPU = 1 << 8;
        const FORCE_MS_SSIM_LONG = 1 << 9;
        const ULTIMATE_MODE = 1 << 10;
        const ALLOW_SIZE_TOLERANCE = 1 << 11;
        const ALLOW_HDR10PLUS_STATIC_FALLBACK = 1 << 12;
        const ARCHIVE_MODE = 1 << 13;
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
            Self::Ignored | Self::Skip => "",
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
            Self::Ignored => "Ignored",
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
    pub const fn archive_mode(&self) -> bool {
        self.flags.contains(ConfigFlags::ARCHIVE_MODE)
    }

    #[must_use]
    pub const fn allow_size_tolerance(&self) -> bool {
        self.flags.contains(ConfigFlags::ALLOW_SIZE_TOLERANCE)
    }

    #[must_use]
    pub const fn allow_hdr10plus_static_fallback(&self) -> bool {
        self.flags
            .contains(ConfigFlags::ALLOW_HDR10PLUS_STATIC_FALLBACK)
    }
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            output_dir: None,
            base_dir: None,
            flags: ConfigFlags::USE_GPU,
            min_ssim: crate::constants::MIN_SSIM_DEFAULT,
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
    /// Intentional domain ignore. Ignored results must NOT be copied to the
    /// output tree by the orchestrator.
    #[serde(default)]
    pub ignored: bool,
}

impl ConversionOutput {
    #[must_use]
    pub fn outcome(&self) -> crate::conversion::Outcome {
        if self.ignored {
            return crate::conversion::Outcome::Ignored;
        }

        if self.strategy.target == TargetVideoFormat::Skip
            || self.output_path.is_empty()
            || self.output_size == 0
        {
            return crate::conversion::Outcome::Skipped;
        }

        if !self.success {
            return crate::conversion::Outcome::Failed;
        }

        crate::conversion::Outcome::Converted
    }
}

impl crate::cli_runner::CliProcessingResult for ConversionOutput {
    fn is_skipped(&self) -> bool {
        self.outcome() == crate::conversion::Outcome::Skipped
    }

    fn is_ignored(&self) -> bool {
        self.outcome() == crate::conversion::Outcome::Ignored
    }

    fn is_success(&self) -> bool {
        self.outcome() == crate::conversion::Outcome::Converted
    }

    fn skip_reason(&self) -> Option<&str> {
        if self.is_skipped() || self.is_ignored() {
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

#[cfg(test)]
mod smoke_tests {
    use super::*;

    #[test]
    fn smoke_selected_codec_properties() {
        assert_eq!(SelectedCodec::Hevc.as_str(), "hevc");
        assert_eq!(SelectedCodec::Av1.as_str(), "av1");
        assert_eq!(SelectedCodec::Av2.as_str(), "av2");
        assert_eq!(SelectedCodec::Vvc.as_str(), "vvc");

        assert!(!SelectedCodec::Hevc.is_experimental());
        assert!(!SelectedCodec::Av1.is_experimental());
        assert!(SelectedCodec::Av2.is_experimental());
        assert!(SelectedCodec::Vvc.is_experimental());

        assert_eq!(
            SelectedCodec::Av2.min_encoder_version(),
            Some("libaom 4.0.0")
        );
        assert_eq!(SelectedCodec::Hevc.min_encoder_version(), None);
    }

    #[test]
    fn smoke_target_video_format_properties() {
        assert_eq!(TargetVideoFormat::HevcLosslessMkv.extension(), "MKV");
        assert_eq!(TargetVideoFormat::Av1Mp4.extension(), "MP4");
        assert_eq!(TargetVideoFormat::Gif.extension(), "GIF");
        assert_eq!(TargetVideoFormat::Skip.extension(), "");

        assert!(!TargetVideoFormat::Av1Mp4.is_experimental());
        assert!(TargetVideoFormat::Av2Mp4.is_experimental());
        assert!(TargetVideoFormat::VvcMp4.is_experimental());

        assert!(TargetVideoFormat::HevcMov.as_str().contains("Apple"));
    }

    #[test]
    fn smoke_conversion_config_flags() {
        let mut config = ConversionConfig {
            flags: ConfigFlags::FORCE | ConfigFlags::USE_GPU,
            ..Default::default()
        };

        assert!(config.force());
        assert!(config.use_gpu());
        assert!(!config.delete_original());
        assert!(!config.should_delete_original());

        config.flags |= ConfigFlags::IN_PLACE;
        assert!(config.in_place());
        assert!(config.should_delete_original());
    }

    #[test]
    fn smoke_conversion_output_outcome() {
        let strategy = ConversionStrategy {
            target: TargetVideoFormat::Av1Mp4,
            reason: String::new(),
            command: String::new(),
            preserve_audio: false,
            crf: 20.0,
            lossless: false,
        };

        let mut output = ConversionOutput {
            input_path: "in.mp4".to_string(),
            output_path: "out.mp4".to_string(),
            strategy,
            input_size: 100,
            output_size: 50,
            size_ratio: 0.5,
            success: true,
            message: String::new(),
            final_crf: 20.0,
            exploration_attempts: 1,
            blake3: None,
            ignored: false,
        };

        assert_eq!(output.outcome(), crate::conversion::Outcome::Converted);

        output.success = false;
        assert_eq!(output.outcome(), crate::conversion::Outcome::Failed);

        output.strategy.target = TargetVideoFormat::Skip;
        output.message = "Skipped: quality gate".to_string();
        assert_eq!(output.outcome(), crate::conversion::Outcome::Skipped);

        output.strategy.target = TargetVideoFormat::HevcLosslessMkv;
        output.success = false;
        assert_eq!(output.outcome(), crate::conversion::Outcome::Failed);

        output.ignored = true;
        assert_eq!(output.outcome(), crate::conversion::Outcome::Ignored);

        output.ignored = false;
        output.success = true;
        output.output_size = 0;
        assert_eq!(output.outcome(), crate::conversion::Outcome::Skipped);

        output.output_size = 50;
        output.strategy.target = TargetVideoFormat::Skip;
        assert_eq!(output.outcome(), crate::conversion::Outcome::Skipped);
    }
}
