//! Encoder Preset Types
//!
//! Provides unified enumeration for encoding speed/quality trade-offs (presets).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Preset {
    Ultrafast,
    Fast,
    #[default]
    Medium,
    Slow,
    Slower,
    Veryslow,
}

impl Preset {
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
    pub const fn hevc_name_for_archive(self, archive: bool) -> &'static str {
        if archive {
            Self::Veryslow.x26x_name()
        } else {
            self.hevc_name()
        }
    }

    /// SVT-AV1 policy window: same delivery band as HEVC (`medium` / `slow` / `slower`).
    #[must_use]
    pub const fn sanitize_av1(self) -> Self {
        match self {
            Self::Ultrafast | Self::Fast => Self::Medium,
            Self::Veryslow => Self::Slower,
            Self::Medium | Self::Slow | Self::Slower => self,
        }
    }

    #[must_use]
    pub const fn av1_name(self) -> &'static str {
        self.sanitize_av1().x26x_name()
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

    #[must_use]
    pub const fn svtav1_preset_for_archive(self, archive: bool) -> u8 {
        if archive {
            Self::Veryslow.svtav1_preset()
        } else {
            self.sanitize_av1().svtav1_preset()
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

#[must_use]
pub fn sanitize_av1_preset_name(preset: &str) -> &'static str {
    sanitize_hevc_preset_name(preset)
}

#[cfg(test)]
mod tests {
    use super::{Preset, sanitize_av1_preset_name, sanitize_hevc_preset_name};

    #[test]
    fn test_hevc_preset_sanitizer_clamps_to_allowed_window() {
        assert_eq!(Preset::Ultrafast.sanitize_hevc(), Preset::Medium);
        assert_eq!(Preset::Fast.sanitize_hevc(), Preset::Medium);
        assert_eq!(Preset::Medium.sanitize_hevc(), Preset::Medium);
        assert_eq!(Preset::Slow.sanitize_hevc(), Preset::Slow);
        assert_eq!(Preset::Slower.sanitize_hevc(), Preset::Slower);
        assert_eq!(Preset::Veryslow.sanitize_hevc(), Preset::Slower);
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

    #[test]
    fn test_av1_preset_sanitizer_matches_hevc_delivery_window() {
        assert_eq!(Preset::Ultrafast.sanitize_av1(), Preset::Medium);
        assert_eq!(Preset::Fast.sanitize_av1(), Preset::Medium);
        assert_eq!(Preset::Veryslow.sanitize_av1(), Preset::Slower);
        assert_eq!(sanitize_av1_preset_name("fast"), "medium");
        assert_eq!(sanitize_av1_preset_name("slower"), "slower");
    }

    #[test]
    fn archive_preset_helpers_hard_override_to_slowest_values() {
        for preset in [
            Preset::Ultrafast,
            Preset::Fast,
            Preset::Medium,
            Preset::Slow,
            Preset::Slower,
            Preset::Veryslow,
        ] {
            assert_eq!(preset.hevc_name_for_archive(true), "veryslow");
            assert_eq!(preset.svtav1_preset_for_archive(true), 0);
        }

        assert_eq!(Preset::Veryslow.hevc_name_for_archive(true), "veryslow");
        assert_eq!(Preset::Veryslow.hevc_name_for_archive(false), "slower");
        assert_eq!(Preset::Veryslow.svtav1_preset_for_archive(true), 0);
        assert_eq!(Preset::Veryslow.svtav1_preset_for_archive(false), 2);
    }
}
