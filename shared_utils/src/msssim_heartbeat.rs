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

pub struct Heartbeat {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Heartbeat {
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

    pub fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn get_beijing_time() -> String {
        let utc_now: DateTime<Utc> = Utc::now();
        let beijing_offset = FixedOffset::east_opt(8 * 3600).expect("Invalid timezone offset");
        let beijing_time = utc_now.with_timezone(&beijing_offset);
        beijing_time.format("%Y-%m-%d %H:%M:%S").to_string()
    }

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
        let time_str = Heartbeat::beijing_time_now();

        assert_eq!(time_str.len(), 19);
        assert_eq!(&time_str[4..5], "-");
        assert_eq!(&time_str[7..8], "-");
        assert_eq!(&time_str[10..11], " ");
        assert_eq!(&time_str[13..14], ":");
        assert_eq!(&time_str[16..17], ":");
    }

    #[test]
    fn test_beijing_time_offset() {
        let utc_now = Utc::now();
        let beijing_offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let beijing_time = utc_now.with_timezone(&beijing_offset);

        assert_eq!(beijing_offset.local_minus_utc(), 8 * 3600);

        let utc_hour = utc_now.hour();
        let beijing_hour = beijing_time.hour();
        let hour_diff = (beijing_hour as i32 - utc_hour as i32 + 24) % 24;
        assert_eq!(hour_diff, 8);
    }

    #[test]
    fn test_heartbeat_start_stop() {
        let heartbeat = Heartbeat::start(1);
        thread::sleep(Duration::from_millis(100));
        heartbeat.stop();
    }

    #[test]
    fn test_heartbeat_drop() {
        {
            let _heartbeat = Heartbeat::start(1);
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn test_heartbeat_output() {
        let heartbeat = Heartbeat::start(1);
        thread::sleep(Duration::from_secs(2));
        heartbeat.stop();
    }

    #[test]
    fn test_multiple_heartbeats() {
        let h1 = Heartbeat::start(1);
        let h2 = Heartbeat::start(1);
        thread::sleep(Duration::from_millis(100));
        h1.stop();
        h2.stop();
    }
}
