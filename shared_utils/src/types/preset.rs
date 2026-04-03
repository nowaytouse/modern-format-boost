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
    pub const fn x26x_name(&self) -> &'static str {
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
    pub const fn ffmpeg_name(&self) -> &'static str {
        self.x26x_name()
    }

    #[must_use]
    pub const fn svtav1_preset(&self) -> u8 {
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
