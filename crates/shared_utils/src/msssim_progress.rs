//! MS-SSIM Progress Monitoring Module
//!
//! 🔥 v7.6: Real-time progress display and ETA estimation.
//!
//! ## Features
//! - Parses ffmpeg progress output
//! - Calculates completion percentage
//! - Estimates remaining time (ETA)
//! - Outputs progress every 10%

use crate::FfmpegBuilder;
use crate::builder_base::ToolBuilder;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub struct Monitor {
    duration_secs: f64,
    current_time_us: AtomicU64,
    channel_scores: Mutex<HashMap<String, f64>>,
    start_time: Instant,
}

impl Monitor {
    #[must_use]
    pub fn new(duration_secs: f64, _total_frames: u64) -> Self {
        Self {
            duration_secs,
            current_time_us: AtomicU64::new(0),
            channel_scores: Mutex::new(HashMap::new()),
            start_time: Instant::now(),
        }
    }

    pub fn update_from_line(&self, line: &str) -> Option<u32> {
        let val = line.strip_prefix("out_time_us=")?;
        let time_us = val.parse::<u64>().ok()?;

        self.current_time_us.store(time_us, Ordering::Relaxed);

        let current_secs =
            crate::numeric_cast::u64_to_f64(time_us) / crate::constants::MICROSECONDS_PER_SECOND;
        let progress_pct = if self.duration_secs > 0.0_f64 {
            crate::numeric_cast::f64_to_u32_strict(
                (current_secs / self.duration_secs * crate::constants::PERCENTAGE_FACTOR)
                    .min(crate::constants::PERCENTAGE_FACTOR),
                "current_pct",
            )
            .or_else(|| {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_QUALITY,
                    "MS-SSIM progress calculation failed: NaN/Inf/overflow detected"
                );
                None
            })?
        } else {
            0
        };

        Some(progress_pct)
    }

    pub fn print_progress(&self, channel: &str, progress_pct: u32) {
        let current_secs =
            crate::numeric_cast::u64_to_f64(self.current_time_us.load(Ordering::Relaxed))
                / crate::constants::MICROSECONDS_PER_SECOND;

        let elapsed = self.start_time.elapsed().as_secs_f64();
        let eta_secs = if progress_pct > 0 {
            let total_estimated =
                elapsed * crate::constants::PERCENTAGE_FACTOR / f64::from(progress_pct);
            (total_estimated - elapsed).max(0.0)
        } else {
            0.0_f64
        };

        crate::log_info!(
            crate::static_logs::messages::LABEL_QUALITY,
            &format!(
                "MS-SSIM Progress [{channel}]: {progress_pct}% ({current_secs:.1}s/{duration:.1}s) ETA: {eta:.0}s",
                duration = self.duration_secs,
                eta = eta_secs
            )
        );
    }

    pub fn store_channel_score(&self, channel: &str, score: f64) {
        if let Ok(mut scores) = self.channel_scores.lock() {
            scores.insert(channel.to_string(), score);
        } else {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_QUALITY,
                "Failed to acquire lock for channel scores (poisoned)"
            );
        }
    }

    pub fn get_channel_score(&self, channel: &str) -> Option<f64> {
        let Ok(scores) = self.channel_scores.lock() else {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_QUALITY,
                "Failed to acquire lock for channel scores (poisoned)! Result data may be lost."
            );
            return None;
        };
        scores.get(channel).copied()
    }

    pub fn current_progress(&self) -> Option<u32> {
        let current_secs =
            crate::numeric_cast::u64_to_f64(self.current_time_us.load(Ordering::Relaxed))
                / crate::constants::MICROSECONDS_PER_SECOND;
        if self.duration_secs > 0.0 {
            crate::numeric_cast::f64_to_u32_strict(
                (current_secs / self.duration_secs * crate::constants::PERCENTAGE_FACTOR)
                    .min(crate::constants::PERCENTAGE_FACTOR),
                "current_progress",
            )
        } else {
            None
        }
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Monitor an ffmpeg process and report progress.
    ///
    /// # Errors
    /// Returns an error message if the process fails or the channel is invalid.
    pub fn monitor_ffmpeg_process(
        &self,
        ffmpeg_args: &[&str],
        channel: &str,
    ) -> Result<(), String> {
        let mut builder = FfmpegBuilder::new();
        for arg in ffmpeg_args {
            builder.arg(arg);
        }
        let mut cmd = builder.arg("-progress").arg("pipe:1").output_pipe().build();
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("❌ Failed to spawn ffmpeg: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "❌ Failed to capture ffmpeg stdout".to_string())?;

        let reader = BufReader::new(stdout);
        let mut last_printed_pct = 0u32;

        for line in reader.lines() {
            let line = line.map_err(|e| format!("❌ Failed to read ffmpeg output: {e}"))?;

            if let Some(progress_pct) = self.update_from_line(&line)
                && (progress_pct >= last_printed_pct + crate::constants::MSSSIM_PROGRESS_PRINT_STEP
                    || progress_pct == crate::constants::PERCENTAGE_FACTOR_U32)
            {
                self.print_progress(channel, progress_pct);
                last_printed_pct = progress_pct;
            }
        }

        let status = child
            .wait()
            .map_err(|e| format!("❌ Failed to wait for ffmpeg: {e}"))?;

        if !status.success() {
            return Err(format!("❌ FFmpeg exited with status: {status}"));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_monitor_creation() {
        let monitor = Monitor::new(
            crate::constants::PROGRESS_DEFAULT_DURATION,
            crate::constants::PROGRESS_DEFAULT_FRAMES,
        );
        assert!(crate::float_compare::approx_eq_f64(
            monitor.duration_secs,
            crate::constants::PROGRESS_DEFAULT_DURATION
        ));
        assert_eq!(monitor.current_progress(), Some(0));
    }

    #[test]
    fn test_update_from_line() {
        let monitor = Monitor::new(
            crate::constants::PROGRESS_DEFAULT_DURATION,
            crate::constants::PROGRESS_DEFAULT_FRAMES,
        );

        let progress = monitor.update_from_line("out_time_us=60000000");
        assert_eq!(progress, Some(50));

        let progress = monitor.update_from_line("frame=100");
        assert_eq!(progress, None);
    }

    #[test]
    fn test_progress_calculation() {
        let monitor = Monitor::new(crate::constants::PERCENTAGE_FACTOR, 2500);

        monitor.update_from_line("out_time_us=0");
        assert_eq!(monitor.current_progress(), Some(0));

        monitor.update_from_line("out_time_us=25000000");
        assert_eq!(monitor.current_progress(), Some(25));

        monitor.update_from_line("out_time_us=50000000");
        assert_eq!(monitor.current_progress(), Some(50));

        monitor.update_from_line("out_time_us=100000000");
        assert_eq!(monitor.current_progress(), Some(100));

        monitor.update_from_line("out_time_us=150000000");
        assert_eq!(monitor.current_progress(), Some(100));
    }

    #[test]
    fn test_channel_score_storage() {
        let monitor = Monitor::new(
            crate::constants::PROGRESS_DEFAULT_DURATION,
            crate::constants::PROGRESS_DEFAULT_FRAMES,
        );

        monitor.store_channel_score("Y", 0.9876);
        monitor.store_channel_score("U", 0.9543);
        monitor.store_channel_score("V", 0.9321);

        assert_eq!(monitor.get_channel_score("Y"), Some(0.987_6_f64));
        assert_eq!(monitor.get_channel_score("U"), Some(0.954_3_f64));
        assert_eq!(monitor.get_channel_score("V"), Some(0.932_1_f64));
        assert_eq!(monitor.get_channel_score("A"), None);
    }

    #[test]
    fn test_zero_duration() {
        let monitor = Monitor::new(0.0, 0);

        monitor.update_from_line("out_time_us=1000000");
        assert_eq!(monitor.current_progress(), None);
    }

    #[test]
    fn test_monitor_ffmpeg_process_invalid_command() {
        let monitor = Monitor::new(10.0, 250);
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
                let duration_secs = crate::constants::PERCENTAGE_FACTOR;
                let monitor = Monitor::new(duration_secs, 2500);
                let line = format!("out_time_us={time_us}");
                let progress = monitor.update_from_line(&line);
                prop_assert!(progress.is_some());
                let pct = progress.unwrap_or_else(|| panic!("missing progress"));
                let expected_secs = crate::numeric_cast::u64_to_f64(time_us) / crate::constants::MICROSECONDS_PER_SECOND;
                let expected_pct = crate::numeric_cast::f64_to_u32_strict(
                    (expected_secs / duration_secs * crate::constants::PERCENTAGE_FACTOR)
                        .min(crate::constants::PERCENTAGE_FACTOR),
                    "expected_pct",
                )
                .expect("expected_pct: invalid value (NaN/Inf/overflow)");
                prop_assert_eq!(pct, expected_pct);
            }

            #[test]
            fn prop_progress_percentage_bounds(
                duration_secs in 1.0f64..10000.0f64,
                time_us in 0u64..10_000_000_000u64
            ) {
                let monitor = Monitor::new(duration_secs, crate::constants::MSSSIM_DEFAULT_SAMPLED_FRAMES as u64);
                let line = format!("out_time_us={time_us}");
                if let Some(pct) = monitor.update_from_line(&line) {
                    prop_assert!(pct <= 100);
                }
            }
        }
    }
}
