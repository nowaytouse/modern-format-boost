//! Universal Heartbeat System
//!
//! 🔥 v7.7: Extended heartbeat detection to all time-consuming operations, completely replacing timeout mechanisms
//!
//! ## Core Features
//! - Smart Silence: Automatically silent when a progress bar is present, shows when no progress is shown
//! - Graded Intervals: 10s/30s/60s depending on operation type
//! - Context Awareness: Displays operation name and elapsed time
//! - RAII Pattern: Automatic resource cleanup
//! - Beijing Time: All times displayed in UTC+8
//!
//! ## Usage Examples
//!
//! ### Basic Usage - RAII Guard Pattern (Recommended)
//!
//! ```rust
//! use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
//!
//! fn long_running_operation() {
//!     // Create heartbeat guard, automatically cleaned up at end of scope
//!     let _guard = HeartbeatGuard::new(HeartbeatConfig::fast("SSIM Calculation"));
//!
//!     // Executing time-consuming operation...
//!     // Heartbeat will output automatically every 10 seconds
//! } // Guard stops heartbeat automatically here
//! ```
//!
//! ### Heartbeat with extra info
//!
//! ```rust
//! use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
//!
//! fn encode_video(filename: &str) {
//!     let config = HeartbeatConfig::medium("Video Encoding")
//!         .with_info(format!("File: {}", filename));
//!     let _guard = HeartbeatGuard::new(config);
//!
//!     // Executing encoding...
//! }
//! ```
//!
//! ### Forced Heartbeat Display (Ignoring progress bar detection)
//!
//! ```rust
//! use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
//!
//! fn critical_operation() {
//!     let config = HeartbeatConfig::slow("Extreme Exploration").force();
//!     let _guard = HeartbeatGuard::new(config);
//!
//!     // Heartbeat will be displayed even if a progress bar is present
//! }
//! ```
//!
//! ### Custom interval
//!
//! ```rust
//! use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
//!
//! fn custom_operation() {
//!     // Output heartbeat every 45 seconds
//!     let config = HeartbeatConfig::custom("Custom Operation", 45);
//!     let _guard = HeartbeatGuard::new(config);
//!
//!     // Executing operation...
//! }
//! ```
//!
//! ## Preset Interval Descriptions
//!
//! - **fast (10s)**: Used for quality calculations like SSIM/PSNR, requiring frequent feedback
//! - **medium (30s)**: Used for medium-duration operations like video encoding
//! - **slow (60s)**: Used for long-duration operations like extreme exploration
//!
//! ## Smart Silence Mechanism
//!
//! The heartbeat system automatically detects if there's an active progress bar:
//! - If a progress bar is displayed, the heartbeat automatically silents (to avoid output conflicts)
//! - If no progress bar is present, the heartbeat displays normally
//! - Can use `.force()` to force display, ignoring progress bar detection

use crate::progress_mode::format_duration_compact;
use chrono::{DateTime, FixedOffset, Utc};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    pub operation: String,
    pub interval_secs: u64,
    pub force_display: bool,
    pub extra_info: Option<String>,
}

impl HeartbeatConfig {
    #[must_use]
    pub fn fast(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 10,
            force_display: false,
            extra_info: None,
        }
    }

    #[must_use]
    pub fn medium(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 30,
            force_display: false,
            extra_info: None,
        }
    }

    #[must_use]
    pub fn slow(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 60,
            force_display: false,
            extra_info: None,
        }
    }

    #[must_use]
    pub fn custom(operation: &str, interval_secs: u64) -> Self {
        let interval = if interval_secs < 5 {
            crate::log_rare_error!(
                "Heartbeat Config",
                "Heartbeat interval too short ({interval_secs} < 5s), using 5s"
            );
            5
        } else {
            interval_secs
        };

        Self {
            operation: operation.to_string(),
            interval_secs: interval,
            force_display: false,
            extra_info: None,
        }
    }

    #[must_use]
    pub fn with_info(mut self, info: String) -> Self {
        self.extra_info = Some(info);
        self
    }

    #[must_use]
    pub const fn force(mut self) -> Self {
        self.force_display = true;
        self
    }
}

pub struct UniversalHeartbeat {
    config: Arc<HeartbeatConfig>,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl UniversalHeartbeat {
    #[must_use]
    pub fn start(config: HeartbeatConfig) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);

        let config = Arc::new(config);
        let config_clone = Arc::clone(&config);

        let start_time = Instant::now();

        crate::heartbeat_manager::HeartbeatManager::register_heartbeat(&config.operation);

        let should_display = config.force_display
            || !crate::heartbeat_manager::HeartbeatManager::has_active_progress();

        let handle = if should_display {
            Some(thread::spawn(move || {
                Self::heartbeat_loop(running_clone, config_clone, start_time);
            }))
        } else {
            None
        };

        Self {
            config,
            running,
            handle,
        }
    }

    fn heartbeat_loop(running: Arc<AtomicBool>, config: Arc<HeartbeatConfig>, start_time: Instant) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while running.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(config.interval_secs));

                if running.load(Ordering::Relaxed) {
                    let elapsed = start_time.elapsed();
                    let elapsed_str = Self::format_elapsed(elapsed);

                    let beijing_time =
                        Self::get_beijing_time().unwrap_or_else(|_| "N/A".to_string());

                    let extra = config
                        .extra_info
                        .as_ref()
                        .map(|s| format!(" - {s}"))
                        .unwrap_or_default();

                    let mut stderr = std::io::stderr();
                    if let Err(err) = stderr.write_fmt(format_args!(
                        "💓 [{}] Active (elapsed: {}, Beijing Time: {}){}",
                        config.operation, elapsed_str, beijing_time, extra
                    )) {
                        crate::log_rare_error!("Heartbeat IO", "Failed to write heartbeat: {err}");
                    } else if let Err(err) = stderr.write_all(b"\n") {
                        crate::log_rare_error!(
                            "Heartbeat IO",
                            "Failed to write heartbeat newline: {err}"
                        );
                    } else if let Err(err) = stderr.flush() {
                        crate::log_rare_error!("Heartbeat IO", "Failed to flush heartbeat: {err}");
                    }
                }
            }
        }));

        if let Err(e) = result {
            crate::log_rare_error!("Heartbeat Thread", "Thread panicked during loop: {e:?}");
        }
    }

    fn format_elapsed(duration: Duration) -> String {
        format_duration_compact(duration)
    }

    fn get_beijing_time() -> Result<String, Box<dyn std::error::Error>> {
        let utc_now: DateTime<Utc> = Utc::now();
        let beijing_offset =
            FixedOffset::east_opt(8 * 3600).ok_or("Failed to create Beijing timezone offset")?;
        let beijing_time = utc_now.with_timezone(&beijing_offset);
        Ok(beijing_time.format("%Y-%m-%d %H:%M:%S").to_string())
    }

    pub fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                crate::log_rare_error!("Heartbeat Thread", "Thread panicked while stopping");
            }
        }
        crate::heartbeat_manager::HeartbeatManager::unregister_heartbeat(&self.config.operation);
    }
}

impl Drop for UniversalHeartbeat {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                crate::log_rare_error!("Heartbeat Thread", "Thread panicked during drop");
            }
        }
        crate::heartbeat_manager::HeartbeatManager::unregister_heartbeat(&self.config.operation);
    }
}

pub struct HeartbeatGuard(Option<UniversalHeartbeat>);

impl HeartbeatGuard {
    #[must_use]
    pub fn new(config: HeartbeatConfig) -> Self {
        Self(Some(UniversalHeartbeat::start(config)))
    }
}

impl Drop for HeartbeatGuard {
    fn drop(&mut self) {
        if let Some(hb) = self.0.take() {
            hb.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_presets() {
        let fast = HeartbeatConfig::fast("Test");
        assert_eq!(fast.interval_secs, 10);

        let medium = HeartbeatConfig::medium("Test");
        assert_eq!(medium.interval_secs, 30);

        let slow = HeartbeatConfig::slow("Test");
        assert_eq!(slow.interval_secs, 60);
    }

    #[test]
    fn test_format_elapsed() {
        assert_eq!(
            UniversalHeartbeat::format_elapsed(Duration::from_secs(30)),
            "30s"
        );
        assert_eq!(
            UniversalHeartbeat::format_elapsed(Duration::from_secs(90)),
            "1m30s"
        );
        assert_eq!(
            UniversalHeartbeat::format_elapsed(Duration::from_secs(3700)),
            "1h01m"
        );
    }

    #[test]
    fn test_heartbeat_guard() {
        {
            let _guard = HeartbeatGuard::new(HeartbeatConfig::fast("Test"));
            thread::sleep(Duration::from_millis(100));
        }
    }
}
