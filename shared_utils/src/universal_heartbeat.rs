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

/// 心跳配置
///
/// 定义心跳检测的行为参数，包括操作名称、间隔时间、显示选项等。
///
/// # 字段说明
///
/// - `operation`: 操作名称，会在心跳输出中显示
/// - `interval_secs`: 心跳间隔（秒），最小值为5秒
/// - `force_display`: 是否强制显示，忽略进度条检测
/// - `extra_info`: 额外信息，会附加在心跳输出中
///
/// # 示例
///
/// ```rust
/// use shared_utils::universal_heartbeat::HeartbeatConfig;
///
/// // 使用预设配置
/// let config = HeartbeatConfig::fast("SSIM计算");
///
/// // 添加额外信息
/// let config = HeartbeatConfig::medium("视频编码")
///     .with_info("file.mp4".to_string());
///
/// // 强制显示
/// let config = HeartbeatConfig::slow("极限探索").force();
/// ```
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
    ///
    /// 适用于需要频繁反馈的操作，如质量计算、快速编码等。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::HeartbeatConfig;
    ///
    /// let config = HeartbeatConfig::fast("SSIM计算");
    /// assert_eq!(config.interval_secs, 10);
    /// ```
    pub fn fast(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 10,
            force_display: false,
            extra_info: None,
        }
    }

    /// 中等间隔(30秒) - 用于视频编码
    ///
    /// 适用于中等耗时的操作，如视频编码、图像批处理等。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::HeartbeatConfig;
    ///
    /// let config = HeartbeatConfig::medium("视频编码");
    /// assert_eq!(config.interval_secs, 30);
    /// ```
    pub fn medium(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 30,
            force_display: false,
            extra_info: None,
        }
    }

    /// 慢速间隔(60秒) - 用于极限探索
    ///
    /// 适用于长时间运行的操作，如极限探索、大规模批处理等。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::HeartbeatConfig;
    ///
    /// let config = HeartbeatConfig::slow("极限探索");
    /// assert_eq!(config.interval_secs, 60);
    /// ```
    pub fn slow(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            interval_secs: 60,
            force_display: false,
            extra_info: None,
        }
    }

    /// 自定义间隔
    ///
    /// 创建自定义间隔的心跳配置。如果间隔小于5秒，会自动调整为5秒并输出警告。
    ///
    /// # 参数
    ///
    /// - `operation`: 操作名称
    /// - `interval_secs`: 心跳间隔（秒），最小值为5秒
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::HeartbeatConfig;
    ///
    /// let config = HeartbeatConfig::custom("自定义操作", 45);
    /// assert_eq!(config.interval_secs, 45);
    ///
    /// // 间隔过短会自动调整
    /// let config = HeartbeatConfig::custom("快速操作", 3);
    /// assert_eq!(config.interval_secs, 5); // 自动调整为5秒
    /// ```
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

    /// 添加额外信息
    ///
    /// 在心跳输出中附加额外信息，如文件名、进度等。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::HeartbeatConfig;
    ///
    /// let config = HeartbeatConfig::medium("视频编码")
    ///     .with_info("file.mp4".to_string());
    /// ```
    pub fn with_info(mut self, info: String) -> Self {
        self.extra_info = Some(info);
        self
    }

    /// 强制显示(忽略进度条检测)
    ///
    /// 即使检测到活跃的进度条，也会显示心跳输出。
    /// 用于关键操作或调试场景。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::HeartbeatConfig;
    ///
    /// let config = HeartbeatConfig::slow("极限探索").force();
    /// assert!(config.force_display);
    /// ```
    pub fn force(mut self) -> Self {
        self.force_display = true;
        self
    }
}

/// 通用心跳检测器
///
/// 在后台线程中定期输出心跳信息，用于监控长时间运行的操作。
/// 支持智能静默（检测进度条）、自定义间隔、RAII自动清理等特性。
///
/// # 使用建议
///
/// 推荐使用 [`HeartbeatGuard`] 而不是直接使用此结构体，
/// 因为 Guard 模式提供了更安全的 RAII 资源管理。
///
/// # 示例
///
/// ```rust
/// use shared_utils::universal_heartbeat::{UniversalHeartbeat, HeartbeatConfig};
///
/// let config = HeartbeatConfig::fast("测试操作");
/// let heartbeat = UniversalHeartbeat::start(config);
///
/// // 执行耗时操作...
///
/// heartbeat.stop(); // 手动停止
/// ```
pub struct UniversalHeartbeat {
    /// 心跳配置（使用Arc避免克隆）
    config: Arc<HeartbeatConfig>,
    /// 运行状态标志
    running: Arc<AtomicBool>,
    /// 操作开始时间（保留用于未来扩展，如查询运行时间）
    #[allow(dead_code)]
    start_time: Instant,
    /// 后台线程句柄
    handle: Option<JoinHandle<()>>,
}

impl UniversalHeartbeat {
    /// 启动心跳检测
    ///
    /// 创建并启动一个新的心跳检测器。如果检测到活跃的进度条且未设置强制显示，
    /// 则会进入静默模式（不启动后台线程）。
    ///
    /// # 参数
    ///
    /// - `config`: 心跳配置
    ///
    /// # 返回
    ///
    /// 返回心跳检测器实例。调用 `stop()` 方法或让其 Drop 时会自动停止。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::{UniversalHeartbeat, HeartbeatConfig};
    ///
    /// let config = HeartbeatConfig::medium("视频编码");
    /// let heartbeat = UniversalHeartbeat::start(config);
    ///
    /// // 执行操作...
    ///
    /// heartbeat.stop();
    /// ```
    pub fn start(config: HeartbeatConfig) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);

        // 🔥 使用Arc避免克隆整个config
        let config = Arc::new(config);
        let config_clone = Arc::clone(&config);

        let start_time = Instant::now();

        // 🔥 v7.7: 注册心跳到全局管理器
        crate::heartbeat_manager::HeartbeatManager::register_heartbeat(&config.operation);

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
    ///
    /// 在后台线程中运行，定期输出心跳信息。
    /// 使用 catch_unwind 捕获 panic，确保不会影响主流程。
    fn heartbeat_loop(running: Arc<AtomicBool>, config: Arc<HeartbeatConfig>, start_time: Instant) {
        // 🔥 v7.7: 使用 catch_unwind 捕获 panic
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while running.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(config.interval_secs));

                if running.load(Ordering::Relaxed) {
                    let elapsed = start_time.elapsed();
                    let elapsed_str = Self::format_elapsed(elapsed);

                    // 🔥 v7.7: 时间获取失败时使用 fallback
                    let beijing_time =
                        Self::get_beijing_time().unwrap_or_else(|_| "N/A".to_string());

                    let extra = config
                        .extra_info
                        .as_ref()
                        .map(|s| format!(" - {}", s))
                        .unwrap_or_default();

                    // 🔥 v7.7: 输出失败时静默跳过(不中断主流程)
                    let _ = std::io::stderr().write_fmt(format_args!(
                        "💓 [{}] Active (elapsed: {}, Beijing Time: {}){}",
                        config.operation, elapsed_str, beijing_time, extra
                    ));
                    let _ = std::io::stderr().write(b"\n");
                    let _ = std::io::stderr().flush();
                }
            }
        }));

        // 🔥 v7.7: panic 捕获 - 记录错误但不影响主流程
        if let Err(e) = result {
            eprintln!("❌ Heartbeat thread panicked: {:?}", e);
        }
    }

    /// 格式化已耗时
    ///
    /// 将 Duration 格式化为人类可读的字符串。
    ///
    /// # 格式
    ///
    /// - 小于60秒: "30s"
    /// - 小于1小时: "5m30s"
    /// - 大于1小时: "2h15m"
    ///
    /// # 示例
    ///
    /// ```ignore
    /// // This is a private function, example for documentation only
    /// use std::time::Duration;
    /// use shared_utils::universal_heartbeat::UniversalHeartbeat;
    ///
    /// // format_elapsed(Duration::from_secs(30)) => "30s"
    /// // format_elapsed(Duration::from_secs(90)) => "1m30s"
    /// // format_elapsed(Duration::from_secs(3700)) => "1h01m"
    /// ```
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
    ///
    /// 返回当前北京时间的格式化字符串。
    ///
    /// # 返回
    ///
    /// 成功时返回格式为 "YYYY-MM-DD HH:MM:SS" 的时间字符串。
    /// 失败时返回错误。
    fn get_beijing_time() -> Result<String, Box<dyn std::error::Error>> {
        let utc_now: DateTime<Utc> = Utc::now();
        let beijing_offset =
            FixedOffset::east_opt(8 * 3600).ok_or("Failed to create Beijing timezone offset")?;
        let beijing_time = utc_now.with_timezone(&beijing_offset);
        Ok(beijing_time.format("%Y-%m-%d %H:%M:%S").to_string())
    }

    /// 停止心跳
    ///
    /// 停止后台线程并注销心跳。此方法会等待后台线程完全退出。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::{UniversalHeartbeat, HeartbeatConfig};
    ///
    /// let heartbeat = UniversalHeartbeat::start(HeartbeatConfig::fast("测试"));
    /// // ... 执行操作 ...
    /// heartbeat.stop(); // 显式停止
    /// ```
    pub fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // 🔥 v7.7: 注销心跳
        crate::heartbeat_manager::HeartbeatManager::unregister_heartbeat(&self.config.operation);
    }
}

impl Drop for UniversalHeartbeat {
    /// 自动清理资源
    ///
    /// 当 UniversalHeartbeat 离开作用域时，自动停止后台线程并注销心跳。
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // 🔥 v7.7: 注销心跳
        crate::heartbeat_manager::HeartbeatManager::unregister_heartbeat(&self.config.operation);
    }
}

/// RAII守卫 - 推荐使用方式
///
/// 提供自动资源管理的心跳守卫。当守卫离开作用域时，会自动停止心跳。
/// 这是使用心跳系统的推荐方式，因为它保证了资源的正确清理。
///
/// # 优势
///
/// - **自动清理**: 无需手动调用 stop()，作用域结束时自动清理
/// - **异常安全**: 即使发生 panic，也会正确清理资源
/// - **简洁易用**: 一行代码即可启动心跳监控
///
/// # 示例
///
/// ```rust
/// use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
///
/// fn process_video() {
///     // 创建守卫，自动开始心跳
///     let _guard = HeartbeatGuard::new(HeartbeatConfig::medium("视频处理"));
///
///     // 执行耗时操作...
///     // 心跳会自动每30秒输出一次
///
/// } // 守卫在此处自动停止心跳，无需手动清理
/// ```
///
/// # 与 UniversalHeartbeat 的区别
///
/// - `HeartbeatGuard`: RAII模式，自动管理生命周期（推荐）
/// - `UniversalHeartbeat`: 需要手动调用 stop()，适合需要精确控制的场景
pub struct HeartbeatGuard(Option<UniversalHeartbeat>);

impl HeartbeatGuard {
    /// 创建心跳守卫
    ///
    /// 创建并启动一个新的心跳守卫。守卫会在离开作用域时自动停止心跳。
    ///
    /// # 参数
    ///
    /// - `config`: 心跳配置
    ///
    /// # 返回
    ///
    /// 返回心跳守卫实例
    ///
    /// # 示例
    ///
    /// ```rust
    /// use shared_utils::universal_heartbeat::{HeartbeatGuard, HeartbeatConfig};
    ///
    /// // 基础用法
    /// let _guard = HeartbeatGuard::new(HeartbeatConfig::fast("SSIM计算"));
    ///
    /// // 带额外信息
    /// let config = HeartbeatConfig::medium("编码")
    ///     .with_info("file.mp4".to_string());
    /// let _guard = HeartbeatGuard::new(config);
    /// ```
    pub fn new(config: HeartbeatConfig) -> Self {
        Self(Some(UniversalHeartbeat::start(config)))
    }
}

impl Drop for HeartbeatGuard {
    /// 自动清理资源
    ///
    /// 当守卫离开作用域时，自动停止心跳并清理资源。
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
        // 验证Drop正常工作
    }
}
