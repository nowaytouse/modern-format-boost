//! MS-SSIM 心跳检测模块
//!
//! 🔥 v7.6: 定期输出活动状态，防止用户误以为程序卡死
//!
//! ## 功能
//! - 每30秒输出一次心跳信息
//! - 显示北京时间（UTC+8）
//! - 线程安全的启动和停止
//! - RAII模式自动清理

#[cfg(test)]
use chrono::Timelike;
use chrono::{DateTime, FixedOffset, Utc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// 心跳检测器
///
/// 在后台线程中定期输出活动状态，让用户知道程序还在运行
pub struct Heartbeat {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Heartbeat {
    /// 启动心跳检测（每N秒输出一次）
    ///
    /// # Arguments
    /// * `interval_secs` - 心跳间隔（秒）
    ///
    /// # Returns
    /// Heartbeat实例，Drop时自动停止
    ///
    /// # Examples
    /// ```no_run
    /// use shared_utils::msssim_heartbeat::Heartbeat;
    ///
    /// let heartbeat = Heartbeat::start(30);
    /// // 做一些耗时操作...
    /// heartbeat.stop(); // 或者让它自动Drop
    /// ```
    pub fn start(interval_secs: u64) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);

        let handle = thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(interval_secs));

                if running_clone.load(Ordering::Relaxed) {
                    let beijing_time = Self::get_beijing_time();
                    eprintln!("💓 Heartbeat: Active (Beijing Time: {})", beijing_time);
                }
            }
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    /// 停止心跳检测
    ///
    /// 显式停止心跳线程。如果不调用此方法，Drop时也会自动停止。
    pub fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    /// 获取北京时间（UTC+8）
    ///
    /// # Returns
    /// 格式化的北京时间字符串："YYYY-MM-DD HH:MM:SS"
    fn get_beijing_time() -> String {
        let utc_now: DateTime<Utc> = Utc::now();
        let beijing_offset = FixedOffset::east_opt(8 * 3600).expect("Invalid timezone offset");
        let beijing_time = utc_now.with_timezone(&beijing_offset);
        beijing_time.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// 获取当前北京时间（公开方法，用于测试）
    ///
    /// # Returns
    /// 格式化的北京时间字符串
    pub fn beijing_time_now() -> String {
        Self::get_beijing_time()
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_beijing_time_format() {
        // 测试时间格式
        let time_str = Heartbeat::beijing_time_now();

        // 验证格式：YYYY-MM-DD HH:MM:SS
        assert_eq!(time_str.len(), 19);
        assert_eq!(&time_str[4..5], "-");
        assert_eq!(&time_str[7..8], "-");
        assert_eq!(&time_str[10..11], " ");
        assert_eq!(&time_str[13..14], ":");
        assert_eq!(&time_str[16..17], ":");
    }

    #[test]
    fn test_beijing_time_offset() {
        // 测试北京时间比UTC快8小时
        let utc_now = Utc::now();
        let beijing_offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let beijing_time = utc_now.with_timezone(&beijing_offset);

        // 验证时区偏移
        assert_eq!(beijing_offset.local_minus_utc(), 8 * 3600);

        // 验证小时差（考虑跨天情况）
        let utc_hour = utc_now.hour();
        let beijing_hour = beijing_time.hour();
        let hour_diff = (beijing_hour as i32 - utc_hour as i32 + 24) % 24;
        assert_eq!(hour_diff, 8);
    }

    #[test]
    fn test_heartbeat_start_stop() {
        // 测试启动和停止
        let heartbeat = Heartbeat::start(1);
        thread::sleep(Duration::from_millis(100));
        heartbeat.stop();
        // 如果没有panic，说明启动和停止成功
    }

    #[test]
    fn test_heartbeat_drop() {
        // 测试Drop自动清理
        {
            let _heartbeat = Heartbeat::start(1);
            thread::sleep(Duration::from_millis(100));
            // heartbeat在这里Drop
        }
        // 如果没有panic，说明Drop成功
    }

    #[test]
    fn test_heartbeat_output() {
        // 测试心跳输出（不验证具体内容，只验证不会panic）
        let heartbeat = Heartbeat::start(1);
        thread::sleep(Duration::from_secs(2)); // 等待至少一次心跳
        heartbeat.stop();
    }

    #[test]
    fn test_multiple_heartbeats() {
        // 测试多个心跳实例
        let h1 = Heartbeat::start(1);
        let h2 = Heartbeat::start(1);
        thread::sleep(Duration::from_millis(100));
        h1.stop();
        h2.stop();
    }
}
