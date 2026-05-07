//! Encoder Preset Types
//!
//! Provides unified enumeration for encoding speed/quality trade-offs (presets).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
    #[must_use]
    pub const fn x26x_name(self) -> &'static str {
        match self {
            Self::Ultrafast => "ultrafast",
            Self::Fast => "fast",
            Self::Medium => "medium",
            Self::Slow => "slow",
            Self::Slower => "slower",
            Self::Veryslow => "veryslow",
        }
    }

    #[must_use]
    pub const fn ffmpeg_name(self) -> &'static str {
        self.x26x_name()
    }

    /// HEVC/x265 policy window: only `medium`, `slow`, and `slower` are allowed.
    #[must_use]
    pub const fn sanitize_hevc(self) -> Self {
        match self {
            Self::Ultrafast | Self::Fast => Self::Medium,
            Self::Veryslow => Self::Slower,
            Self::Medium | Self::Slow | Self::Slower => self,
        }
    }

    #[must_use]
    pub const fn hevc_name(self) -> &'static str {
        self.sanitize_hevc().x26x_name()
    }

    #[must_use]
    pub const fn svtav1_preset(self) -> u8 {
        match self {
            Self::Ultrafast => 12,
            Self::Fast => 8,
            Self::Medium => 6,
            Self::Slow => 4,
            Self::Slower => 2,
            Self::Veryslow => 0,
        }
    }
}

#[must_use]
pub fn sanitize_hevc_preset_name(preset: &str) -> &'static str {
    match preset.trim().to_ascii_lowercase().as_str() {
        "slow" => "slow",
        "slower" | "veryslow" | "placebo" => "slower",
        _ => "medium",
    }
}

#[cfg(test)]
mod tests {
    use super::{EncoderPreset, sanitize_hevc_preset_name};

    #[test]
    fn test_hevc_preset_sanitizer_clamps_to_allowed_window() {
        assert_eq!(
            EncoderPreset::Ultrafast.sanitize_hevc(),
            EncoderPreset::Medium
        );
        assert_eq!(EncoderPreset::Fast.sanitize_hevc(), EncoderPreset::Medium);
        assert_eq!(EncoderPreset::Medium.sanitize_hevc(), EncoderPreset::Medium);
        assert_eq!(EncoderPreset::Slow.sanitize_hevc(), EncoderPreset::Slow);
        assert_eq!(EncoderPreset::Slower.sanitize_hevc(), EncoderPreset::Slower);
        assert_eq!(
            EncoderPreset::Veryslow.sanitize_hevc(),
            EncoderPreset::Slower
        );
    }

    #[test]
    fn test_hevc_preset_name_sanitizer_handles_raw_strings() {
        assert_eq!(sanitize_hevc_preset_name("fast"), "medium");
        assert_eq!(sanitize_hevc_preset_name("medium"), "medium");
        assert_eq!(sanitize_hevc_preset_name("slow"), "slow");
        assert_eq!(sanitize_hevc_preset_name("slower"), "slower");
        assert_eq!(sanitize_hevc_preset_name("veryslow"), "slower");
        assert_eq!(sanitize_hevc_preset_name("placebo"), "slower");
        assert_eq!(sanitize_hevc_preset_name("unknown"), "medium");
    }
}
