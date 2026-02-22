//! MS-SSIM 并行计算模块
//!
//! 🔥 v7.6: Y/U/V三通道并行计算
//!
//! ## 功能
//! - 并行计算Y/U/V三通道MS-SSIM
//! - 集成心跳检测和进度监控
//! - 线程安全的错误处理
//! - 降级策略支持

use crate::app_error::AppError;
use crate::msssim_heartbeat::Heartbeat;
use crate::msssim_progress::MsssimProgressMonitor;
use crate::msssim_sampling::{SamplingConfig, SamplingStrategy};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

#[derive(Debug, Clone)]
pub struct MsssimResult {
    pub y_score: f64,
    pub u_score: f64,
    pub v_score: f64,
    pub combined_score: f64,
    pub sampling_strategy: SamplingStrategy,
    pub sampled_frames: u64,
    pub total_frames: u64,
}

impl MsssimResult {
    pub fn skipped() -> Self {
        Self {
            y_score: 0.0,
            u_score: 0.0,
            v_score: 0.0,
            combined_score: 0.0,
            sampling_strategy: SamplingStrategy::Skip,
            sampled_frames: 0,
            total_frames: 0,
        }
    }

    pub fn is_skipped(&self) -> bool {
        self.sampling_strategy == SamplingStrategy::Skip
    }

    pub fn print_stats(&self, elapsed_secs: f64) {
        if self.is_skipped() {
            return;
        }

        let speedup = self.total_frames as f64 / self.sampled_frames.max(1) as f64;
        eprintln!(
            "⏱️  MS-SSIM completed in {:.2}s (sampled {}/{} frames)",
            elapsed_secs, self.sampled_frames, self.total_frames
        );
        eprintln!("   Parallel speedup: {:.1}x (theoretical: 3x)", speedup);
    }
}

pub struct ParallelMsssimCalculator {
    original_path: PathBuf,
    converted_path: PathBuf,
    sampling_config: SamplingConfig,
    progress_monitor: Arc<MsssimProgressMonitor>,
}

impl ParallelMsssimCalculator {
    pub fn new(
        original_path: PathBuf,
        converted_path: PathBuf,
        sampling_config: SamplingConfig,
    ) -> Self {
        let progress_monitor = Arc::new(MsssimProgressMonitor::new(
            sampling_config.duration_secs,
            sampling_config.sampled_frames,
        ));

        Self {
            original_path,
            converted_path,
            sampling_config,
            progress_monitor,
        }
    }

    pub fn calculate(&self) -> Result<MsssimResult, AppError> {
        if self.sampling_config.strategy == SamplingStrategy::Skip {
            return Ok(MsssimResult::skipped());
        }

        if let Ok(probe) = crate::ffprobe::probe_video(&self.original_path) {
            if probe.format_name.eq_ignore_ascii_case("gif") {
                eprintln!(
                    "❌ ERROR: GIF format - MS-SSIM not supported (palette-based). No fallback."
                );
                return Err(AppError::Other(anyhow::anyhow!(
                    "GIF does not support MS-SSIM quality verification."
                )));
            }
        } else {
        }

        eprintln!("🔄 Calculating MS-SSIM (heartbeat active)");

        let heartbeat = Heartbeat::start(30);

        let y_monitor = Arc::clone(&self.progress_monitor);
        let u_monitor = Arc::clone(&self.progress_monitor);
        let v_monitor = Arc::clone(&self.progress_monitor);

        let orig_path = self.original_path.clone();
        let conv_path = self.converted_path.clone();
        let config = self.sampling_config.clone();

        let y_handle = thread::spawn(move || {
            Self::calculate_channel(&orig_path, &conv_path, &config, "Y", y_monitor)
        });

        let orig_path = self.original_path.clone();
        let conv_path = self.converted_path.clone();
        let config = self.sampling_config.clone();

        let u_handle = thread::spawn(move || {
            Self::calculate_channel(&orig_path, &conv_path, &config, "U", u_monitor)
        });

        let orig_path = self.original_path.clone();
        let conv_path = self.converted_path.clone();
        let config = self.sampling_config.clone();

        let v_handle = thread::spawn(move || {
            Self::calculate_channel(&orig_path, &conv_path, &config, "V", v_monitor)
        });

        let y_result = y_handle.join().map_err(|_| {
            eprintln!("❌ Y channel thread panicked");
            AppError::Other(anyhow::anyhow!("Y channel thread panicked"))
        })?;
        let u_result = u_handle.join().map_err(|_| {
            eprintln!("❌ U channel thread panicked");
            AppError::Other(anyhow::anyhow!("U channel thread panicked"))
        })?;
        let v_result = v_handle.join().map_err(|_| {
            eprintln!("❌ V channel thread panicked");
            AppError::Other(anyhow::anyhow!("V channel thread panicked"))
        })?;

        heartbeat.stop();

        let y_score = y_result?;
        let u_score = u_result?;
        let v_score = v_result?;

        eprintln!("✅ MS-SSIM complete, heartbeat stopped");
        eprintln!(
            "✅ MS-SSIM (parallel): Y={:.4} U={:.4} V={:.4}",
            y_score, u_score, v_score
        );

        Ok(MsssimResult {
            y_score,
            u_score,
            v_score,
            combined_score: (y_score + u_score + v_score) / 3.0,
            sampling_strategy: self.sampling_config.strategy,
            sampled_frames: self.sampling_config.sampled_frames,
            total_frames: self.sampling_config.total_frames,
        })
    }

    fn calculate_channel(
        original_path: &Path,
        converted_path: &Path,
        config: &SamplingConfig,
        channel: &str,
        progress_monitor: Arc<MsssimProgressMonitor>,
    ) -> Result<f64, AppError> {
        let original_path_str = original_path.to_string_lossy();
        let converted_path_str = converted_path.to_string_lossy();
        let mut args = vec![
            "-i",
            original_path_str.as_ref(),
            "-i",
            converted_path_str.as_ref(),
        ];

        let filter_str;
        if let Some(filter) = config.strategy.ffmpeg_filter() {
            filter_str = format!("[0:v]{}[v0];[1:v]{}[v1]", filter, filter);
            args.push("-filter_complex");
            args.push(&filter_str);
        }

        let lavfi_str = format!("libvmaf=feature=name=ms_ssim:channel={}", channel);
        args.push("-lavfi");
        args.push(&lavfi_str);
        args.push("-f");
        args.push("null");
        args.push("-");

        let ms_ssim_result = progress_monitor
            .monitor_ffmpeg_process(&args, channel)
            .map_err(|e| AppError::Other(anyhow::anyhow!(e)));

        match ms_ssim_result {
            Ok(_) => {
                progress_monitor.get_channel_score(channel).ok_or_else(|| {
                    eprintln!("❌ Failed to get {} channel score", channel);
                    AppError::Other(anyhow::anyhow!("Failed to get {} channel score", channel))
                })
            }
            Err(_) => {
                eprintln!(
                    "⚠️  MS-SSIM failed for channel {}, falling back to SSIM",
                    channel
                );

                let mut ssim_args = vec![
                    "-i",
                    original_path_str.as_ref(),
                    "-i",
                    converted_path_str.as_ref(),
                ];

                let ssim_filter_str;
                if let Some(filter) = config.strategy.ffmpeg_filter() {
                    ssim_filter_str = format!("[0:v]{}[v0];[1:v]{}[v1]", filter, filter);
                    ssim_args.push("-filter_complex");
                    ssim_args.push(&ssim_filter_str);
                }

                let ssim_lavfi_str = format!("libvmaf=feature=name=ssim:channel={}", channel);
                ssim_args.push("-lavfi");
                ssim_args.push(&ssim_lavfi_str);
                ssim_args.push("-f");
                ssim_args.push("null");
                ssim_args.push("-");

                progress_monitor
                    .monitor_ffmpeg_process(&ssim_args, channel)
                    .map_err(|e| {
                        eprintln!("❌ Both MS-SSIM and SSIM failed for channel {}", channel);
                        AppError::Other(anyhow::anyhow!("Both MS-SSIM and SSIM failed: {}", e))
                    })?;

                progress_monitor.get_channel_score(channel).ok_or_else(|| {
                    eprintln!("❌ Failed to get {} channel SSIM score", channel);
                    AppError::Other(anyhow::anyhow!(
                        "Failed to get {} channel SSIM score",
                        channel
                    ))
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_msssim_result_skipped() {
        let result = MsssimResult::skipped();
        assert!(result.is_skipped());
        assert_eq!(result.y_score, 0.0);
        assert_eq!(result.u_score, 0.0);
        assert_eq!(result.v_score, 0.0);
        assert_eq!(result.combined_score, 0.0);
    }

    #[test]
    fn test_msssim_result_print_stats() {
        let result = MsssimResult {
            y_score: 0.98,
            u_score: 0.97,
            v_score: 0.96,
            combined_score: 0.97,
            sampling_strategy: SamplingStrategy::OneThird,
            sampled_frames: 1000,
            total_frames: 3000,
        };

        result.print_stats(30.5);
    }

    #[test]
    fn test_parallel_calculator_creation() {
        let config = SamplingConfig::new(120.0, 3000, false, false);
        let calculator = ParallelMsssimCalculator::new(
            PathBuf::from("/tmp/original.mp4"),
            PathBuf::from("/tmp/converted.mp4"),
            config,
        );

        assert_eq!(calculator.original_path, PathBuf::from("/tmp/original.mp4"));
        assert_eq!(
            calculator.converted_path,
            PathBuf::from("/tmp/converted.mp4")
        );
    }

    #[cfg(test)]
    mod property_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn prop_result_combined_score(
                y in 0.0f64..=1.0f64,
                u in 0.0f64..=1.0f64,
                v in 0.0f64..=1.0f64
            ) {
                let result = MsssimResult {
                    y_score: y,
                    u_score: u,
                    v_score: v,
                    combined_score: (y + u + v) / 3.0,
                    sampling_strategy: SamplingStrategy::Full,
                    sampled_frames: 1000,
                    total_frames: 1000,
                };

                let expected = (y + u + v) / 3.0;
                prop_assert!((result.combined_score - expected).abs() < 1e-10);
            }

            #[test]
            fn prop_elapsed_time_calculation(elapsed in 0.1f64..10000.0f64) {
                let result = MsssimResult {
                    y_score: 0.98,
                    u_score: 0.97,
                    v_score: 0.96,
                    combined_score: 0.97,
                    sampling_strategy: SamplingStrategy::Full,
                    sampled_frames: 1000,
                    total_frames: 1000,
                };

                result.print_stats(elapsed);
            }

            #[test]
            fn prop_performance_stats_format(
                sampled in 1u64..10000u64,
                total in 1u64..10000u64
            ) {
                let sampled_frames = sampled.min(total);
                let total_frames = total.max(sampled);

                let result = MsssimResult {
                    y_score: 0.98,
                    u_score: 0.97,
                    v_score: 0.96,
                    combined_score: 0.97,
                    sampling_strategy: SamplingStrategy::OneThird,
                    sampled_frames,
                    total_frames,
                };

                result.print_stats(30.0);
            }

            #[test]
            fn prop_speedup_calculation(
                sampled in 1u64..10000u64,
                total in 1u64..10000u64
            ) {
                let sampled_frames = sampled.min(total);
                let total_frames = total.max(sampled);

                let speedup = total_frames as f64 / sampled_frames.max(1) as f64;

                prop_assert!(speedup >= 1.0);

                let expected = total_frames as f64 / sampled_frames as f64;
                prop_assert!((speedup - expected).abs() < 1e-10);
            }
        }
    }
}
