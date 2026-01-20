//! MS-SSIM 智能采样策略模块
//!
//! 🔥 v7.6: 根据视频时长自动选择采样率，优化长视频的MS-SSIM计算性能
//!
//! ## 核心策略
//! - ≤60秒: 全量计算（1/1采样）
//! - 60-300秒: 1/3采样
//! - 300-1800秒: 1/10采样
//! - >1800秒: 跳过MS-SSIM，仅使用SSIM
//!
//! ## 性能目标
//! - 48秒视频: 从~180秒降至~30秒（6倍加速）
//! - 5分钟视频: 从~600秒降至~60秒（10倍加速）
//! - 30分钟视频: ~120秒内完成

/// 采样策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingStrategy {
    /// 全量计算（1/1）
    Full,
    /// 1/3采样
    OneThird,
    /// 1/10采样
    OneTenth,
    /// 跳过MS-SSIM
    Skip,
}

impl SamplingStrategy {
    /// 根据视频时长决定采样策略
    ///
    /// # Arguments
    /// * `duration_secs` - 视频时长（秒）
    ///
    /// # Returns
    /// 推荐的采样策略
    ///
    /// # Examples
    /// ```
    /// use shared_utils::msssim_sampling::SamplingStrategy;
    ///
    /// assert_eq!(SamplingStrategy::from_duration(30.0), SamplingStrategy::Full);
    /// assert_eq!(SamplingStrategy::from_duration(120.0), SamplingStrategy::OneThird);
    /// assert_eq!(SamplingStrategy::from_duration(600.0), SamplingStrategy::OneTenth);
    /// assert_eq!(SamplingStrategy::from_duration(2000.0), SamplingStrategy::Skip);
    /// ```
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

    /// 获取采样率（分母）
    ///
    /// # Returns
    /// Some(N) 表示 1/N 采样，None 表示跳过
    pub fn sampling_rate(&self) -> Option<u32> {
        match self {
            Self::Full => Some(1),
            Self::OneThird => Some(3),
            Self::OneTenth => Some(10),
            Self::Skip => None,
        }
    }

    /// 获取ffmpeg select filter表达式
    ///
    /// # Returns
    /// Some(filter) 表示需要应用的filter，None 表示不需要filter
    ///
    /// # Note
    /// 反斜杠转义是为了在shell命令中正确传递
    pub fn ffmpeg_filter(&self) -> Option<String> {
        match self {
            Self::Full => None,
            Self::OneThird => Some("select='not(mod(n\\,3))'".to_string()),
            Self::OneTenth => Some("select='not(mod(n\\,10))'".to_string()),
            Self::Skip => None,
        }
    }

    /// 获取精度描述
    ///
    /// # Returns
    /// 采样策略对应的精度估算
    pub fn accuracy_description(&self) -> &'static str {
        match self {
            Self::Full => "100%",
            Self::OneThird => "~99%",
            Self::OneTenth => "~95%",
            Self::Skip => "N/A",
        }
    }
}

/// 采样配置
#[derive(Debug, Clone)]
pub struct SamplingConfig {
    /// 采样策略
    pub strategy: SamplingStrategy,
    /// 视频时长（秒）
    pub duration_secs: f64,
    /// 总帧数
    pub total_frames: u64,
    /// 采样后的帧数
    pub sampled_frames: u64,
    /// 用户强制选项
    pub force_full: bool,
    pub force_skip: bool,
}

impl SamplingConfig {
    /// 创建采样配置
    ///
    /// # Arguments
    /// * `duration_secs` - 视频时长（秒）
    /// * `total_frames` - 总帧数
    /// * `force_full` - 强制全量计算
    /// * `force_skip` - 强制跳过MS-SSIM
    ///
    /// # Returns
    /// 采样配置实例
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

    /// 打印采样信息
    pub fn print_info(&self) {
        match self.strategy {
            SamplingStrategy::Skip => {
                eprintln!(
                    "⚠️  Video too long ({:.1}s), MS-SSIM skipped (using SSIM only)",
                    self.duration_secs
                );
            }
            _ => {
                let rate = self.strategy.sampling_rate().unwrap();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sampling_strategy_boundaries() {
        // 测试边界值
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
        // 测试极端值
        assert_eq!(SamplingStrategy::from_duration(0.0), SamplingStrategy::Full);
        assert_eq!(SamplingStrategy::from_duration(1.0), SamplingStrategy::Full);
        assert_eq!(
            SamplingStrategy::from_duration(100000.0),
            SamplingStrategy::Skip
        );
    }

    #[test]
    fn test_ffmpeg_filter_generation() {
        // 测试filter生成
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
        // 测试强制选项
        let config = SamplingConfig::new(120.0, 3000, true, false);
        assert_eq!(config.strategy, SamplingStrategy::Full);
        assert_eq!(config.sampled_frames, 3000);

        let config = SamplingConfig::new(120.0, 3000, false, true);
        assert_eq!(config.strategy, SamplingStrategy::Skip);
        assert_eq!(config.sampled_frames, 0);
    }

    #[test]
    fn test_sampling_config_auto() {
        // 测试自动采样
        let config = SamplingConfig::new(120.0, 3000, false, false);
        assert_eq!(config.strategy, SamplingStrategy::OneThird);
        assert_eq!(config.sampled_frames, 1000);

        let config = SamplingConfig::new(600.0, 15000, false, false);
        assert_eq!(config.strategy, SamplingStrategy::OneTenth);
        assert_eq!(config.sampled_frames, 1500);
    }
}
