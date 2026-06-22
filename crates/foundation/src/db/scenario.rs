// Multi-scenario embedding type system and configuration

use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScenarioType {
    /// Loop Intent Detection (GIF/Animation)
    LoopIntent,
    /// Static Image Quality (PNG/WebP/AVIF)
    ImageQuality,
    /// Animated Image Quality
    AnimatedImageQuality,
    /// Video Quality
    VideoQuality,
}

impl ScenarioType {
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::LoopIntent,
            Self::ImageQuality,
            Self::AnimatedImageQuality,
            Self::VideoQuality,
        ]
    }

    #[must_use]
    pub const fn table_name(&self) -> &'static str {
        match self {
            Self::LoopIntent => "loop_samples",
            Self::ImageQuality => "image_quality_samples",
            Self::AnimatedImageQuality => "animated_image_quality_samples",
            Self::VideoQuality => "video_quality_samples",
        }
    }

    #[must_use]
    pub const fn embedding_dimension(&self) -> usize {
        match self {
            Self::LoopIntent => 261,
            Self::ImageQuality | Self::AnimatedImageQuality | Self::VideoQuality => 256,
        }
    }

    #[must_use]
    pub const fn inference_log_table(&self) -> &'static str {
        match self {
            Self::LoopIntent => "loop_intent_inference_log",
            Self::ImageQuality => "image_quality_inference_log",
            Self::AnimatedImageQuality => "animated_image_quality_inference_log",
            Self::VideoQuality => "video_quality_inference_log",
        }
    }

    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::LoopIntent => "Loop Intent Detection",
            Self::ImageQuality => "Static Image Quality",
            Self::AnimatedImageQuality => "Animated Image Quality",
            Self::VideoQuality => "Video Quality",
        }
    }

    #[must_use]
    pub const fn task_family(&self) -> &'static str {
        match self {
            Self::LoopIntent => "loop_clustering",
            Self::ImageQuality | Self::AnimatedImageQuality | Self::VideoQuality => {
                "quality_regression"
            }
        }
    }

    #[must_use]
    pub const fn is_quality_regression(&self) -> bool {
        matches!(
            self,
            Self::ImageQuality | Self::AnimatedImageQuality | Self::VideoQuality
        )
    }

    #[must_use]
    pub const fn is_loop_clustering(&self) -> bool {
        matches!(self, Self::LoopIntent)
    }
}

impl FromStr for ScenarioType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "loop" | "loop_intent" => Ok(Self::LoopIntent),
            "image" | "image_quality" => Ok(Self::ImageQuality),
            "animated" | "animated_image" | "animated_image_quality" => {
                Ok(Self::AnimatedImageQuality)
            }
            "video" | "video_quality" => Ok(Self::VideoQuality),
            _ => anyhow::bail!(
                "Invalid scenario type: {s}. Valid options: loop, image, animated_image, video."
            ),
        }
    }
}

impl std::fmt::Display for ScenarioType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::LoopIntent => "loop_intent",
                Self::ImageQuality => "image_quality",
                Self::AnimatedImageQuality => "animated_image_quality",
                Self::VideoQuality => "video_quality",
            }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QualityTier {
    High,
    Low,
    Unknown,
}

impl QualityTier {
    #[must_use]
    pub fn from_label(s: &str) -> Self {
        let s = s.trim();
        if s.eq_ignore_ascii_case("high")
            || s.eq_ignore_ascii_case("png-high")
            || s.eq_ignore_ascii_case("modern-high")
        {
            Self::High
        } else if s.eq_ignore_ascii_case("low")
            || s.eq_ignore_ascii_case("png-low")
            || s.eq_ignore_ascii_case("modern-low")
        {
            Self::Low
        } else {
            Self::Unknown
        }
    }

    /// Parse only supported quality labels and reject ambiguous inputs.
    ///
    /// # Errors
    ///
    /// Returns an error if the label does not map to a supported high/low tier.
    pub fn parse_strict(s: &str) -> anyhow::Result<Self> {
        let tier = Self::from_label(s);
        if tier == Self::Unknown {
            anyhow::bail!(
                "Invalid quality label: {s}. Valid options: high, low, png-high, png-low, modern-high, modern-low."
            );
        }
        Ok(tier)
    }

    #[must_use]
    pub const fn to_score(self) -> f32 {
        match self {
            Self::High => 1.0,
            Self::Low => 0.0,
            Self::Unknown => 0.5,
        }
    }
}

impl FromStr for QualityTier {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_strict(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageQualityLabel {
    PngHigh,
    PngLow,
    ModernHigh,
    ModernLow,
}

impl ImageQualityLabel {
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        let label = label.trim();
        if label.eq_ignore_ascii_case("png-high") {
            Some(Self::PngHigh)
        } else if label.eq_ignore_ascii_case("png-low") {
            Some(Self::PngLow)
        } else if label.eq_ignore_ascii_case("modern-high") {
            Some(Self::ModernHigh)
        } else if label.eq_ignore_ascii_case("modern-low") {
            Some(Self::ModernLow)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PngHigh => "png-high",
            Self::PngLow => "png-low",
            Self::ModernHigh => "modern-high",
            Self::ModernLow => "modern-low",
        }
    }

    #[must_use]
    pub const fn to_tier(self) -> QualityTier {
        match self {
            Self::PngHigh | Self::ModernHigh => QualityTier::High,
            Self::PngLow | Self::ModernLow => QualityTier::Low,
        }
    }

    #[must_use]
    pub const fn to_score(self) -> f32 {
        self.to_tier().to_score()
    }

    #[must_use]
    pub const fn is_png_family(self) -> bool {
        matches!(self, Self::PngHigh | Self::PngLow)
    }

    #[must_use]
    pub fn matches_format(self, format: &str) -> bool {
        self.is_png_family() == format_uses_png_quality_family(format)
    }

    /// Resolve a label into the strict canonical image-quality label for the
    /// actual asset format.
    ///
    /// # Errors
    ///
    /// Returns an error if the label is unsupported or conflicts with the
    /// asset's detected storage family.
    pub fn resolve_for_format(label: &str, format: &str) -> anyhow::Result<Self> {
        if let Some(parsed) = Self::from_label(label) {
            if parsed.matches_format(format) {
                return Ok(parsed);
            }
            anyhow::bail!(
                "Image quality label '{label}' is incompatible with detected format '{format}'"
            );
        }

        Ok(match QualityTier::parse_strict(label)? {
            QualityTier::High => {
                if format_uses_png_quality_family(format) {
                    Self::PngHigh
                } else {
                    Self::ModernHigh
                }
            }
            QualityTier::Low => {
                if format_uses_png_quality_family(format) {
                    Self::PngLow
                } else {
                    Self::ModernLow
                }
            }
            QualityTier::Unknown => {
                anyhow::bail!("Image quality labels must resolve to a strict high/low tier")
            }
        })
    }
}

impl FromStr for ImageQualityLabel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_label(s).ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid image quality label: {s}. Valid options: png-high, png-low, modern-high, modern-low."
            )
        })
    }
}

#[must_use]
pub fn format_uses_png_quality_family(format: &str) -> bool {
    format.trim().eq_ignore_ascii_case("png")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scenario_properties() {
        assert_eq!(ScenarioType::LoopIntent.table_name(), "loop_samples");
        assert_eq!(ScenarioType::LoopIntent.embedding_dimension(), 261);
        assert_eq!(ScenarioType::LoopIntent.to_string(), "loop_intent");
        assert_eq!(ScenarioType::LoopIntent.task_family(), "loop_clustering");
        assert!(ScenarioType::LoopIntent.is_loop_clustering());
        assert!(!ScenarioType::LoopIntent.is_quality_regression());

        assert_eq!(
            ScenarioType::ImageQuality.table_name(),
            "image_quality_samples"
        );
        assert_eq!(ScenarioType::ImageQuality.embedding_dimension(), 256);
        assert_eq!(ScenarioType::ImageQuality.to_string(), "image_quality");
        assert_eq!(
            ScenarioType::ImageQuality.task_family(),
            "quality_regression"
        );
        assert!(ScenarioType::ImageQuality.is_quality_regression());
        assert!(!ScenarioType::ImageQuality.is_loop_clustering());

        assert_eq!(
            ScenarioType::AnimatedImageQuality.table_name(),
            "animated_image_quality_samples"
        );
        assert_eq!(
            ScenarioType::AnimatedImageQuality.embedding_dimension(),
            256
        );
        assert_eq!(
            ScenarioType::AnimatedImageQuality.description(),
            "Animated Image Quality"
        );
        assert_eq!(
            ScenarioType::AnimatedImageQuality.to_string(),
            "animated_image_quality"
        );
        assert_eq!(
            ScenarioType::AnimatedImageQuality.task_family(),
            "quality_regression"
        );

        assert_eq!(
            ScenarioType::VideoQuality.table_name(),
            "video_quality_samples"
        );
        assert_eq!(ScenarioType::VideoQuality.embedding_dimension(), 256);
        assert_eq!(ScenarioType::VideoQuality.to_string(), "video_quality");
        assert_eq!(
            ScenarioType::VideoQuality.task_family(),
            "quality_regression"
        );
    }

    #[test]
    fn test_scenario_from_str() {
        // Test primary aliases
        assert_eq!(
            "loop_intent".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::LoopIntent
        );
        assert_eq!(
            "image_quality".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::ImageQuality
        );
        assert_eq!(
            "animated_image_quality".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::AnimatedImageQuality
        );
        assert_eq!(
            "video_quality".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::VideoQuality
        );

        // Test shorthand aliases
        assert_eq!(
            "loop".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::LoopIntent
        );
        assert_eq!(
            "image".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::ImageQuality
        );
        assert_eq!(
            "animated_image".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::AnimatedImageQuality
        );
        assert_eq!(
            "video".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::VideoQuality
        );

        // Test case insensitivity
        assert_eq!(
            "Animated_Image".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::AnimatedImageQuality
        );
        assert_eq!(
            "Loop_Intent".parse::<ScenarioType>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ScenarioType::LoopIntent
        );

        // Test invalid inputs
        assert!("unknown".parse::<ScenarioType>().is_err());
        assert!("".parse::<ScenarioType>().is_err());
        assert!("gif".parse::<ScenarioType>().is_err());
        assert!("gif_quality".parse::<ScenarioType>().is_err());
    }

    #[test]
    fn test_quality_tier_mapping() {
        fn assert_score_eq(actual: f32, expected: f32) {
            assert!((actual - expected).abs() < f32::EPSILON);
        }

        // High Tier
        assert_eq!(QualityTier::from_label("high"), QualityTier::High);
        assert_eq!(QualityTier::from_label("png-high"), QualityTier::High);
        assert_eq!(QualityTier::from_label("modern-high"), QualityTier::High);
        assert_score_eq(QualityTier::High.to_score(), 1.0);

        // Low Tier
        assert_eq!(QualityTier::from_label("low"), QualityTier::Low);
        assert_eq!(QualityTier::from_label("png-low"), QualityTier::Low);
        assert_eq!(QualityTier::from_label("modern-low"), QualityTier::Low);
        assert_score_eq(QualityTier::Low.to_score(), 0.0);

        // Unknown/Edge Cases
        assert_eq!(QualityTier::from_label("unknown"), QualityTier::Unknown);
        assert_eq!(QualityTier::from_label(""), QualityTier::Unknown);
        assert_eq!(QualityTier::from_label("high-speed"), QualityTier::Unknown); // Exact match check
        assert_score_eq(QualityTier::Unknown.to_score(), 0.5);

        // Case Insensitivity
        assert_eq!(QualityTier::from_label("HIGH"), QualityTier::High);
        assert_eq!(QualityTier::from_label("Modern-Low"), QualityTier::Low);
        assert_eq!(QualityTier::from_label(" high "), QualityTier::High);
    }

    #[test]
    fn test_quality_tier_parse_strict_rejects_unknown_labels() {
        assert!("unknown".parse::<QualityTier>().is_err());
        assert!("high-speed".parse::<QualityTier>().is_err());
        assert_eq!(
            "png-high".parse::<QualityTier>().unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            QualityTier::High
        );
    }

    #[test]
    fn test_image_quality_label_resolution_tracks_format_family() {
        assert_eq!(
            ImageQualityLabel::resolve_for_format("high", "PNG").unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ImageQualityLabel::PngHigh
        );
        assert_eq!(
            ImageQualityLabel::resolve_for_format("low", "webp").unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ImageQualityLabel::ModernLow
        );
        assert_eq!(
            ImageQualityLabel::resolve_for_format("modern-high", "jpeg").unwrap(), // audited: db module unit-test fixture assertion; not production DB runtime path
            ImageQualityLabel::ModernHigh
        );
        assert!(ImageQualityLabel::resolve_for_format("png-high", "webp").is_err());
    }
}
