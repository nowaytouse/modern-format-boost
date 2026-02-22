//! MS-SSIM 进度监控模块
//!
//! 🔥 v7.6: 实时进度显示和ETA估算
//!
//! ## 功能
//! - 解析ffmpeg的progress输出
//! - 计算完成百分比
//! - 估算剩余时间（ETA）
//! - 每10%输出一次进度

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

pub struct MsssimProgressMonitor {
    duration_secs: f64,
    current_time_us: AtomicU64,
    channel_scores: Mutex<HashMap<String, f64>>,
    start_time: Instant,
}

impl MsssimProgressMonitor {
    pub fn new(duration_secs: f64, _total_frames: u64) -> Self {
        Self {
            duration_secs,
            current_time_us: AtomicU64::new(0),
            channel_scores: Mutex::new(HashMap::new()),
            start_time: Instant::now(),
        }
    }

    pub fn update_from_line(&self, line: &str) -> Option<u32> {
        if let Some(val) = line.strip_prefix("out_time_us=") {
            if let Ok(time_us) = val.parse::<u64>() {
                self.current_time_us.store(time_us, Ordering::Relaxed);

                let current_secs = time_us as f64 / 1_000_000.0;
                let progress_pct = if self.duration_secs > 0.0 {
                    (current_secs / self.duration_secs * 100.0).min(100.0) as u32
                } else {
                    0
                };

                return Some(progress_pct);
            }
        }

        None
    }

    pub fn print_progress(&self, channel: &str, progress_pct: u32) {
        let current_secs = self.current_time_us.load(Ordering::Relaxed) as f64 / 1_000_000.0;

        let elapsed = self.start_time.elapsed().as_secs_f64();
        let eta_secs = if progress_pct > 0 {
            let total_estimated = elapsed * 100.0 / progress_pct as f64;
            (total_estimated - elapsed).max(0.0)
        } else {
            0.0
        };

        eprintln!(
            "⏳ MS-SSIM Progress [{}]: {}% ({:.1}s/{:.1}s) ETA: {:.0}s",
            channel, progress_pct, current_secs, self.duration_secs, eta_secs
        );
    }

    pub fn store_channel_score(&self, channel: &str, score: f64) {
        if let Ok(mut scores) = self.channel_scores.lock() {
            scores.insert(channel.to_string(), score);
        } else {
            eprintln!("❌ Failed to acquire lock for channel scores (poisoned)");
        }
    }

    pub fn get_channel_score(&self, channel: &str) -> Option<f64> {
        let scores = self.channel_scores.lock().ok()?;
        scores.get(channel).copied()
    }

    pub fn current_progress(&self) -> u32 {
        let current_secs = self.current_time_us.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        if self.duration_secs > 0.0 {
            (current_secs / self.duration_secs * 100.0).min(100.0) as u32
        } else {
            0
        }
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    pub fn monitor_ffmpeg_process(
        &self,
        ffmpeg_args: &[&str],
        channel: &str,
    ) -> Result<(), String> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(ffmpeg_args)
            .arg("-progress")
            .arg("pipe:1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("❌ Failed to spawn ffmpeg: {}", e))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "❌ Failed to capture ffmpeg stdout".to_string())?;

        let reader = BufReader::new(stdout);
        let mut last_printed_pct = 0u32;

        for line in reader.lines() {
            let line = line.map_err(|e| format!("❌ Failed to read ffmpeg output: {}", e))?;

            if let Some(progress_pct) = self.update_from_line(&line) {
                if progress_pct >= last_printed_pct + 10 || progress_pct == 100 {
                    self.print_progress(channel, progress_pct);
                    last_printed_pct = progress_pct;
                }
            }
        }

        let status = child
            .wait()
            .map_err(|e| format!("❌ Failed to wait for ffmpeg: {}", e))?;

        if !status.success() {
            return Err(format!("❌ FFmpeg exited with status: {}", status));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_monitor_creation() {
        let monitor = MsssimProgressMonitor::new(120.0, 3000);
        assert_eq!(monitor.duration_secs, 120.0);
        assert_eq!(monitor.current_progress(), 0);
    }

    #[test]
    fn test_update_from_line() {
        let monitor = MsssimProgressMonitor::new(120.0, 3000);

        let progress = monitor.update_from_line("out_time_us=60000000");
        assert_eq!(progress, Some(50));

        let progress = monitor.update_from_line("frame=100");
        assert_eq!(progress, None);
    }

    #[test]
    fn test_progress_calculation() {
        let monitor = MsssimProgressMonitor::new(100.0, 2500);

        monitor.update_from_line("out_time_us=0");
        assert_eq!(monitor.current_progress(), 0);

        monitor.update_from_line("out_time_us=25000000");
        assert_eq!(monitor.current_progress(), 25);

        monitor.update_from_line("out_time_us=50000000");
        assert_eq!(monitor.current_progress(), 50);

        monitor.update_from_line("out_time_us=100000000");
        assert_eq!(monitor.current_progress(), 100);

        monitor.update_from_line("out_time_us=150000000");
        assert_eq!(monitor.current_progress(), 100);
    }

    #[test]
    fn test_channel_score_storage() {
        let monitor = MsssimProgressMonitor::new(120.0, 3000);

        monitor.store_channel_score("Y", 0.9876);
        monitor.store_channel_score("U", 0.9543);
        monitor.store_channel_score("V", 0.9321);

        assert_eq!(monitor.get_channel_score("Y"), Some(0.9876));
        assert_eq!(monitor.get_channel_score("U"), Some(0.9543));
        assert_eq!(monitor.get_channel_score("V"), Some(0.9321));
        assert_eq!(monitor.get_channel_score("A"), None);
    }

    #[test]
    fn test_zero_duration() {
        let monitor = MsssimProgressMonitor::new(0.0, 0);

        monitor.update_from_line("out_time_us=1000000");
        assert_eq!(monitor.current_progress(), 0);
    }

    #[test]
    fn test_print_progress() {
        let monitor = MsssimProgressMonitor::new(120.0, 3000);
        monitor.update_from_line("out_time_us=60000000");

        monitor.print_progress("Y", 50);
    }

    #[test]
    fn test_monitor_ffmpeg_process_invalid_command() {
        let monitor = MsssimProgressMonitor::new(10.0, 250);

        let result = monitor.monitor_ffmpeg_process(&["invalid_command"], "Y");
        assert!(result.is_err());
    }

    #[cfg(test)]
    mod property_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn prop_progress_parsing_correctness(time_us in 0u64..1_000_000_000u64) {
                let duration_secs = 100.0;
                let monitor = MsssimProgressMonitor::new(duration_secs, 2500);

                let line = format!("out_time_us={}", time_us);
                let progress = monitor.update_from_line(&line);

                prop_assert!(progress.is_some());

                let pct = progress.unwrap();
                let expected_secs = time_us as f64 / 1_000_000.0;
                let expected_pct = ((expected_secs / duration_secs * 100.0).min(100.0)) as u32;

                prop_assert_eq!(pct, expected_pct);
            }

            #[test]
            fn prop_progress_percentage_bounds(
                duration_secs in 1.0f64..10000.0f64,
                time_us in 0u64..10_000_000_000u64
            ) {
                let monitor = MsssimProgressMonitor::new(duration_secs, 1000);

                let line = format!("out_time_us={}", time_us);
                if let Some(pct) = monitor.update_from_line(&line) {
                    prop_assert!(pct <= 100);
                }
            }

            #[test]
            fn prop_progress_output_format(
                duration_secs in 1.0f64..1000.0f64,
                progress_pct in 0u32..=100u32
            ) {
                let monitor = MsssimProgressMonitor::new(duration_secs, 1000);

                monitor.print_progress("Y", progress_pct);
                monitor.print_progress("U", progress_pct);
                monitor.print_progress("V", progress_pct);
            }
        }
    }
}
