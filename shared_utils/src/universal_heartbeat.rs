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

use chrono::{DateTime, FixedOffset, Utc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// 心跳配置
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// 操作名称
    pub operation: String,
    /// 间隔(秒)
    pub interval_secs: u64,
    /// 强制显示(忽略进度条检测)
    pub force_display: bool,
    /// 额外信息
    pub extra_info: Option<String>,
}

impl HeartbeatConfig {
    /// 快速间隔(10秒) - 用于SSIM/PSNR计算
    pub fn fast(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 10,
            force_display: false,
            extra_info: None,
        }
    }

    /// 中等间隔(30秒) - 用于视频编码
    pub fn medium(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 30,
            force_display: false,
            extra_info: None,
        }
    }

    /// 慢速间隔(60秒) - 用于极限探索
    pub fn slow(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 60,
            force_display: false,
            extra_info: None,
        }
    }

    /// 自定义间隔
    pub fn custom(operation: &str, interval_secs: u64) -> Self {
        let interval = if interval_secs < 5 {
            eprintln!("⚠️  Heartbeat interval too short ({} < 5s), using 5s", interval_secs);
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

    /// 添加额外信息
    pub fn with_info(mut self, info: String) -> Self {
        self.extra_info = Some(info);
        self
    }

    /// 强制显示(忽略进度条检测)
    pub fn force(mut self) -> Self {
        self.force_display = true;
        self
    }
}

/// 通用心跳检测器
pub struct UniversalHeartbeat {
    config: HeartbeatConfig,
    running: Arc<AtomicBool>,
    start_time: Instant,
    handle: Option<JoinHandle<()>>,
}

impl UniversalHeartbeat {
    /// 启动心跳检测
    pub fn start(config: HeartbeatConfig) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let config_clone = config.clone();
        let start_time = Instant::now();

        // 检查是否应该显示
        let should_display = config.force_display 
            || !crate::heartbeat_manager::HeartbeatManager::has_active_progress();

        let handle = if should_display {
            Some(thread::spawn(move || {
                Self::heartbeat_loop(running_clone, config_clone, start_time);
            }))
        } else {
            None // 静默模式,不启动线程
        };

        Self {
            config,
            running,
            start_time,
            handle,
        }
    }

    /// 心跳循环
    fn heartbeat_loop(running: Arc<AtomicBool>, config: HeartbeatConfig, start_time: Instant) {
        while running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(config.interval_secs));

            if running.load(Ordering::Relaxed) {
                let elapsed = start_time.elapsed();
                let elapsed_str = Self::format_elapsed(elapsed);
                let beijing_time = Self::get_beijing_time();
                
                let extra = config.extra_info.as_ref()
                    .map(|s| format!(" - {}", s))
                    .unwrap_or_default();
                
                eprintln!(
                    "💓 [{}] Active (elapsed: {}, Beijing Time: {}){}",
                    config.operation, elapsed_str, beijing_time, extra
                );
            }
        }
    }

    /// 格式化已耗时
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

    /// 获取北京时间(UTC+8)
    fn get_beijing_time() -> String {
        let utc_now: DateTime<Utc> = Utc::now();
        let beijing_offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let beijing_time = utc_now.with_timezone(&beijing_offset);
        beijing_time.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// 停止心跳
    pub fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for UniversalHeartbeat {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// RAII守卫 - 推荐使用方式
pub struct HeartbeatGuard(Option<UniversalHeartbeat>);

impl HeartbeatGuard {
    /// 创建心跳守卫
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
        assert_eq!(UniversalHeartbeat::format_elapsed(Duration::from_secs(30)), "30s");
        assert_eq!(UniversalHeartbeat::format_elapsed(Duration::from_secs(90)), "1m30s");
        assert_eq!(UniversalHeartbeat::format_elapsed(Duration::from_secs(3700)), "1h01m");
    }

    #[test]
    fn test_heartbeat_guard() {
        {
            let _guard = HeartbeatGuard::new(HeartbeatConfig::fast("Test"));
            thread::sleep(Duration::from_millis(100));
        }
        // 验证Drop正常工作
    }
}
