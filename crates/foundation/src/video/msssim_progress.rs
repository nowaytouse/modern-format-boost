//! MS-SSIM Progress Monitoring Module
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
        let time_us = match val.parse::<u64>() {
            Ok(time_us) => time_us,
            Err(err) => {
                crate::media_conversion_gate::delivery_progress_batch_audit(
                    "msssim_progress_time_parse_failed",
                    format!("failed to parse MS-SSIM progress out_time_us {val:?}: {err}"),
                );
                return None;
            }
        };

        self.current_time_us.store(time_us, Ordering::Relaxed);

        let current_secs =
            crate::numeric_cast::u64_to_f64(time_us) / crate::constants::MICROSECONDS_PER_SECOND;
        let progress_pct = if self.duration_secs > 0.0_f64 {
            let pct_opt = crate::numeric_cast::f64_to_u32_strict(
                (current_secs / self.duration_secs * crate::constants::PERCENTAGE_FACTOR)
                    .min(crate::constants::PERCENTAGE_FACTOR),
                "current_pct",
            );
            if let Some(v) = pct_opt {
                v
            } else {
                crate::media_conversion_gate::delivery_msssim_progress_pct_invalid_audit();
                return None;
            }
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

        tracing::info!(
            target: "mfb::progress",
            "{} MS-SSIM Progress [{}]: {}% ({:.1}s/{:.1}s) ETA: {:.0}s",
            crate::modern_ui::symbols::INFO,
            channel,
            progress_pct,
            current_secs,
            self.duration_secs,
            eta_secs
        );
    }

    pub fn store_channel_score(&self, channel: &str, score: f64) {
        match self.channel_scores.lock() {
            Ok(mut scores) => {
                scores.insert(channel.to_string(), score);
            }
            Err(_) => {
                crate::media_conversion_gate::delivery_msssim_channel_mutex_audit(
                    "msssim_channel_scores_store_mutex",
                );
            }
        }
    }

    pub fn get_channel_score(&self, channel: &str) -> Option<f64> {
        let Ok(scores) = self.channel_scores.lock() else {
            crate::media_conversion_gate::delivery_msssim_channel_mutex_audit(
                "msssim_channel_scores_read_mutex",
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
        let err = crate::media_conversion_gate::ui_icon_pick(
            crate::modern_ui::symbols::ERROR,
            crate::modern_ui::symbols::plain::ERROR,
        );
        let mut builder = FfmpegBuilder::new();
        for arg in ffmpeg_args {
            builder.arg(arg);
        }
        let mut cmd = builder
            .loglevel("error")
            .arg("-progress")
            .arg("pipe:1")
            .output_pipe()
            .build();
        let stderr_capture =
            crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                "msssim_ffmpeg_stderr",
                None,
                Some(".log"),
            )
            .map_err(|error| format!("{err} Failed to allocate FFmpeg stderr capture: {error}"))?;
        let stderr_file = stderr_capture
            .reopen()
            .map_err(|error| format!("{err} Failed to open FFmpeg stderr capture: {error}"))?;
        cmd.stdout(Stdio::piped()).stderr(Stdio::from(stderr_file));
        let command_line = crate::common_utils::format_command_for_audit(&cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("{err} Failed to spawn ffmpeg: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{err} Failed to capture ffmpeg stdout"))?;

        let reader = BufReader::new(stdout);
        let mut last_printed_pct = 0u32;

        for line in reader.lines() {
            let line = line.map_err(|e| format!("{err} Failed to read ffmpeg output: {e}"))?;

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
            .map_err(|e| format!("{err} Failed to wait for ffmpeg: {e}"))?;
        let diagnostic = crate::infra::logging::read_bounded_diagnostic_file(stderr_capture.path())
            .map_err(|error| format!("{err} Failed to read FFmpeg diagnostics: {error}"))?;
        crate::infra::logging::log_captured_process_output(&command_line, status, "", &diagnostic);

        if !status.success() {
            return Err(format!(
                "{err} FFmpeg exited with status {status}: {}",
                if diagnostic.trim().is_empty() {
                    "no diagnostic output"
                } else {
                    diagnostic.trim()
                }
            ));
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
                .unwrap_or_else(|| {
                    unreachable!(
                        "CRITICAL: expected_pct calculation failed in test (time_us={}, duration_secs={})",
                        time_us, duration_secs
                    )
                });
                prop_assert_eq!(pct, expected_pct);
            }

            #[test]
            fn prop_progress_percentage_bounds(
                duration_secs in 1.0f64..10000.0f64,
                time_us in 0u64..10_000_000_000u64
            ) {
                let monitor = Monitor::new(
                    duration_secs,
                    crate::numeric_cast::usize_to_u64(crate::constants::MSSSIM_DEFAULT_SAMPLED_FRAMES),
                );
                let line = format!("out_time_us={time_us}");
                if let Some(pct) = monitor.update_from_line(&line) {
                    prop_assert!(pct <= 100);
                }
            }
        }
    }
}
