//! Heartbeat Manager - 全局心跳管理器
//!
//! 🔥 v7.7: 管理进度条状态和心跳注册
//!
//! ## 核心功能
//! - 进度条计数: 跟踪活动进度条数量
//! - 智能静默: 有进度条时心跳自动静默
//! - 线程安全: 使用原子操作

use std::sync::atomic::{AtomicUsize, Ordering};

/// 全局心跳管理器
pub struct HeartbeatManager;

/// 全局进度条计数器
static ACTIVE_PROGRESS_BARS: AtomicUsize = AtomicUsize::new(0);

impl HeartbeatManager {
    /// 注册进度条
    pub fn register_progress_bar() {
        ACTIVE_PROGRESS_BARS.fetch_add(1, Ordering::Relaxed);
    }

    /// 注销进度条
    pub fn unregister_progress_bar() {
        ACTIVE_PROGRESS_BARS.fetch_sub(1, Ordering::Relaxed);
    }

    /// 检查是否有活动进度条
    pub fn has_active_progress() -> bool {
        ACTIVE_PROGRESS_BARS.load(Ordering::Relaxed) > 0
    }

    /// 获取活动进度条数量
    pub fn active_progress_count() -> usize {
        ACTIVE_PROGRESS_BARS.load(Ordering::Relaxed)
    }
}

/// 进度条守卫 - RAII模式自动注册/注销
pub struct ProgressBarGuard;

impl ProgressBarGuard {
    /// 创建进度条守卫
    pub fn new() -> Self {
        HeartbeatManager::register_progress_bar();
        Self
    }
}

impl Drop for ProgressBarGuard {
    fn drop(&mut self) {
        HeartbeatManager::unregister_progress_bar();
    }
}

impl Default for ProgressBarGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar_registration() {
        // 重置计数器
        while HeartbeatManager::active_progress_count() > 0 {
            HeartbeatManager::unregister_progress_bar();
        }

        assert_eq!(HeartbeatManager::active_progress_count(), 0);
        assert!(!HeartbeatManager::has_active_progress());

        HeartbeatManager::register_progress_bar();
        assert_eq!(HeartbeatManager::active_progress_count(), 1);
        assert!(HeartbeatManager::has_active_progress());

        HeartbeatManager::unregister_progress_bar();
        assert_eq!(HeartbeatManager::active_progress_count(), 0);
        assert!(!HeartbeatManager::has_active_progress());
    }

    #[test]
    fn test_progress_bar_guard() {
        // 重置计数器
        while HeartbeatManager::active_progress_count() > 0 {
            HeartbeatManager::unregister_progress_bar();
        }

        {
            let _guard = ProgressBarGuard::new();
            assert_eq!(HeartbeatManager::active_progress_count(), 1);
        }
        assert_eq!(HeartbeatManager::active_progress_count(), 0);
    }

    #[test]
    fn test_multiple_guards() {
        // 重置计数器
        while HeartbeatManager::active_progress_count() > 0 {
            HeartbeatManager::unregister_progress_bar();
        }

        {
            let _g1 = ProgressBarGuard::new();
            let _g2 = ProgressBarGuard::new();
            assert_eq!(HeartbeatManager::active_progress_count(), 2);
        }
        assert_eq!(HeartbeatManager::active_progress_count(), 0);
    }
}
