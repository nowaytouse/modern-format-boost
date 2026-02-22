//! Universal Heartbeat System - 统一心跳检测模块
//!
//! 🔥 v7.7: 扩展心跳检测到所有耗时操作,完全替代超时机制
//!
//! ## 核心功能
//! - 智能静默: 有进度条时自动静默,无进度时显示
//! - 分级间隔: 10s/30s/60s根据操作类型
//! - 上下文感知: 显示操作名称和已耗时
//! - RAII模式: 自动资源清理
//! - 北京时间: 所有时间显示UTC+8
//!
//! ## 使用示例
//!
//! ### 基础用法 - RAII守卫模式（推荐）
//!
//! ```rust
//! use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
//!
//! fn long_running_operation() {
//!     // 创建心跳守卫，自动在作用域结束时清理
//!     let _guard = HeartbeatGuard::new(HeartbeatConfig::fast("SSIM计算"));
//!
//!     // 执行耗时操作...
//!     // 心跳会每10秒自动输出一次
//! } // 守卫在此处自动停止心跳
//! ```
//!
//! ### 带额外信息的心跳
//!
//! ```rust
//! use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
//!
//! fn encode_video(filename: &str) {
//!     let config = HeartbeatConfig::medium("视频编码")
//!         .with_info(format!("文件: {}", filename));
//!     let _guard = HeartbeatGuard::new(config);
//!
//!     // 执行编码...
//! }
//! ```
//!
//! ### 强制显示心跳（忽略进度条检测）
//!
//! ```rust
//! use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
//!
//! fn critical_operation() {
//!     let config = HeartbeatConfig::slow("极限探索").force();
//!     let _guard = HeartbeatGuard::new(config);
//!
//!     // 即使有进度条，也会显示心跳
//! }
//! ```
//!
//! ### 自定义间隔
//!
//! ```rust
//! use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
//!
//! fn custom_operation() {
//!     // 每45秒输出一次心跳
//!     let config = HeartbeatConfig::custom("自定义操作", 45);
//!     let _guard = HeartbeatGuard::new(config);
//!
//!     // 执行操作...
//! }
//! ```
//!
//! ## 预设间隔说明
//!
//! - **fast (10秒)**: 用于SSIM/PSNR等质量计算，需要频繁反馈
//! - **medium (30秒)**: 用于视频编码等中等耗时操作
//! - **slow (60秒)**: 用于极限探索等长时间操作
//!
//! ## 智能静默机制
//!
//! 心跳系统会自动检测是否有活跃的进度条：
//! - 如果有进度条显示，心跳会自动静默（避免输出冲突）
//! - 如果没有进度条，心跳会正常显示
//! - 可以使用 `.force()` 强制显示，忽略进度条检测

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
    pub fn fast(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 10,
            force_display: false,
            extra_info: None,
        }
    }

    pub fn medium(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 30,
            force_display: false,
            extra_info: None,
        }
    }

    pub fn slow(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 60,
            force_display: false,
            extra_info: None,
        }
    }

    pub fn custom(operation: &str, interval_secs: u64) -> Self {
        let interval = if interval_secs < 5 {
            eprintln!(
                "⚠️  Heartbeat interval too short ({} < 5s), using 5s",
                interval_secs
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

    pub fn with_info(mut self, info: String) -> Self {
        self.extra_info = Some(info);
        self
    }

    pub fn force(mut self) -> Self {
        self.force_display = true;
        self
    }
}

pub struct UniversalHeartbeat {
    config: Arc<HeartbeatConfig>,
    running: Arc<AtomicBool>,
    #[allow(dead_code)]
    start_time: Instant,
    handle: Option<JoinHandle<()>>,
}

impl UniversalHeartbeat {
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
            start_time,
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
                        .map(|s| format!(" - {}", s))
                        .unwrap_or_default();

                    let _ = std::io::stderr().write_fmt(format_args!(
                        "💓 [{}] Active (elapsed: {}, Beijing Time: {}){}",
                        config.operation, elapsed_str, beijing_time, extra
                    ));
                    let _ = std::io::stderr().write(b"\n");
                    let _ = std::io::stderr().flush();
                }
            }
        }));

        if let Err(e) = result {
            eprintln!("❌ Heartbeat thread panicked: {:?}", e);
        }
    }

    fn format_elapsed(duration: Duration) -> String {
        let secs = duration.as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m{:02}s", secs / 60, secs % 60)
        } else {
            format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
        }
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
            let _ = handle.join();
        }
        crate::heartbeat_manager::HeartbeatManager::unregister_heartbeat(&self.config.operation);
    }
}

impl Drop for UniversalHeartbeat {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        crate::heartbeat_manager::HeartbeatManager::unregister_heartbeat(&self.config.operation);
    }
}

pub struct HeartbeatGuard(Option<UniversalHeartbeat>);

impl HeartbeatGuard {
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
