//! MS-SSIM Smart Sampling Strategy Module
//!
//! Automatically selects sampling rate based on video duration
//!
//! ## Core Strategy
//! - ≤60s: Full calculation (1/1 sampling)
//! - 60-300s: 1/3 sampling
//! - 300-1800s: 1/10 sampling
//! - >1800s: Skip MS-SSIM, use SSIM only
//!
//! ## Performance Goals
//! - 48s video: Reduced from ~180s to ~30s (6x speedup)
//! - 5m video: Reduced from ~600s to ~60s (10x speedup)
//! - 30m video: Completed within ~120s

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingStrategy {
    Full,
    OneThird,
    OneTenth,
    Skip,
}

impl SamplingStrategy {
    #[must_use]
    pub fn from_duration(duration_secs: f64) -> Self {
        if duration_secs <= 60.0 {
            Self::Full
        } else if duration_secs <= 300.0 {
            Self::OneThird
        } else if duration_secs <= 1800.0 {
            Self::OneTenth
        } else {
            Self::Skip
        }
    }

    #[must_use]
    pub const fn sampling_rate(&self) -> Option<u32> {
        match self {
            Self::Full => Some(1),
            Self::OneThird => Some(3),
            Self::OneTenth => Some(10),
            Self::Skip => None,
        }
    }

    #[must_use]
    pub fn ffmpeg_filter(&self) -> Option<String> {
        match self {
            Self::OneThird => Some("select='not(mod(n\\,3))'".to_string()),
            Self::OneTenth => Some("select='not(mod(n\\,10))'".to_string()),
            _ => None,
        }
    }

    #[must_use]
    pub const fn accuracy_description(&self) -> &'static str {
        match self {
            Self::Full => "100%",
            Self::OneThird => "~99%",
            Self::OneTenth => "~95%",
            Self::Skip => "N/A",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SamplingConfig {
    pub strategy: SamplingStrategy,
    pub duration_secs: f64,
    pub total_frames: u64,
    pub sampled_frames: u64,
    pub force_full: bool,
    pub force_skip: bool,
}

impl SamplingConfig {
    #[must_use]
    pub fn new(duration_secs: f64, total_frames: u64, force_full: bool, force_skip: bool) -> Self {
        let strategy = if force_skip {
            SamplingStrategy::Skip
        } else if force_full {
            SamplingStrategy::Full
        } else {
            SamplingStrategy::from_duration(duration_secs)
        };

        let sampled_frames = match strategy {
            SamplingStrategy::Full => total_frames,
            SamplingStrategy::OneThird => total_frames / 3,
            SamplingStrategy::OneTenth => total_frames / 10,
            SamplingStrategy::Skip => 0,
        };

        Self {
            strategy,
            duration_secs,
            total_frames,
            sampled_frames,
            force_full,
            force_skip,
        }
    }

    /// Print sampling information to the log.
    pub fn print_info(&self) {
        if self.strategy == SamplingStrategy::Skip {
            eprintln!(
                "⚠️  Quality verification: video too long ({:.1}s), MS-SSIM skipped (using SSIM only).",
                self.duration_secs
            );
        } else {
            let Some(rate) = self.strategy.sampling_rate() else {
                eprintln!(
                    "⚠️  Quality verification sampling rate unavailable; continuing without MS-SSIM sampling detail."
                );
                return;
            };
            let accuracy = self.strategy.accuracy_description();
            eprintln!(
                "📊 MS-SSIM: Sampling 1/{} frames (duration: {:.1}s, accuracy: {})",
                rate, self.duration_secs, accuracy
            );
            eprintln!(
                "   Frames: {} → {} (speedup: {:.1}x)",
                self.total_frames,
                self.sampled_frames,
                self.total_frames as f64 / self.sampled_frames.max(1) as f64
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sampling_strategy_boundaries() {
        assert_eq!(
            SamplingStrategy::from_duration(60.0),
            SamplingStrategy::Full
        );
        assert_eq!(
            SamplingStrategy::from_duration(60.1),
            SamplingStrategy::OneThird
        );
        assert_eq!(
            SamplingStrategy::from_duration(300.0),
            SamplingStrategy::OneThird
        );
        assert_eq!(
            SamplingStrategy::from_duration(300.1),
            SamplingStrategy::OneTenth
        );
        assert_eq!(
            SamplingStrategy::from_duration(1800.0),
            SamplingStrategy::OneTenth
        );
        assert_eq!(
            SamplingStrategy::from_duration(1800.1),
            SamplingStrategy::Skip
        );
    }

    #[test]
    fn test_sampling_strategy_extremes() {
        assert_eq!(SamplingStrategy::from_duration(0.0), SamplingStrategy::Full);
        assert_eq!(SamplingStrategy::from_duration(1.0), SamplingStrategy::Full);
        assert_eq!(
            SamplingStrategy::from_duration(100_000.0),
            SamplingStrategy::Skip
        );
    }

    #[test]
    fn test_ffmpeg_filter_generation() {
        assert_eq!(SamplingStrategy::Full.ffmpeg_filter(), None);
        assert_eq!(
            SamplingStrategy::OneThird.ffmpeg_filter(),
            Some("select='not(mod(n\\,3))'".to_string())
        );
        assert_eq!(
            SamplingStrategy::OneTenth.ffmpeg_filter(),
            Some("select='not(mod(n\\,10))'".to_string())
        );
        assert_eq!(SamplingStrategy::Skip.ffmpeg_filter(), None);
    }

    #[test]
    fn test_sampling_config_force_options() {
        let config = SamplingConfig::new(120.0, 3000, true, false);
        assert_eq!(config.strategy, SamplingStrategy::Full);
        assert_eq!(config.sampled_frames, 3000);

        let config = SamplingConfig::new(120.0, 3000, false, true);
        assert_eq!(config.strategy, SamplingStrategy::Skip);
        assert_eq!(config.sampled_frames, 0);
    }

    #[test]
    fn test_sampling_config_auto() {
        let config = SamplingConfig::new(120.0, 3000, false, false);
        assert_eq!(config.strategy, SamplingStrategy::OneThird);
        assert_eq!(config.sampled_frames, 1000);

        let config = SamplingConfig::new(600.0, 15000, false, false);
        assert_eq!(config.strategy, SamplingStrategy::OneTenth);
        assert_eq!(config.sampled_frames, 1500);
    }
}
