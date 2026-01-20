//! Heartbeat Manager - 全局心跳管理器
//!
//! 🔥 v7.7: 管理进度条状态和心跳注册
//!
//! ## 核心功能
//! - 进度条计数: 跟踪活动进度条数量
//! - 智能静默: 有进度条时心跳自动静默
//! - 心跳注册: 跟踪活动心跳数量
//! - 嵌套检测: 检测嵌套心跳(只显示最内层)
//! - 线程安全: 使用原子操作

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// 全局心跳管理器
pub struct HeartbeatManager;

/// 全局进度条计数器
static ACTIVE_PROGRESS_BARS: AtomicUsize = AtomicUsize::new(0);

/// 全局活动心跳计数器
static ACTIVE_HEARTBEATS: AtomicUsize = AtomicUsize::new(0);

/// 全局心跳注册表 (操作名称 -> 计数)
static HEARTBEAT_REGISTRY: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

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

    /// 注册心跳
    pub fn register_heartbeat(operation: &str) {
        ACTIVE_HEARTBEATS.fetch_add(1, Ordering::Relaxed);

        // 更新注册表
        if let Ok(mut registry) = HEARTBEAT_REGISTRY.lock() {
            let map = registry.get_or_insert_with(HashMap::new);
            *map.entry(operation.to_string()).or_insert(0) += 1;

            // 🔥 v7.8.1: 改进重复心跳检测 - 只在调试模式下警告
            if map[operation] > 1 && std::env::var("IMGQUALITY_DEBUG").is_ok() {
                eprintln!(
                    "🔍 Debug: Multiple heartbeats with same name: {} (count: {})",
                    operation, map[operation]
                );
            }
        }
    }

    /// 注销心跳
    pub fn unregister_heartbeat(operation: &str) {
        ACTIVE_HEARTBEATS.fetch_sub(1, Ordering::Relaxed);

        // 更新注册表
        if let Ok(mut registry) = HEARTBEAT_REGISTRY.lock() {
            if let Some(map) = registry.as_mut() {
                if let Some(count) = map.get_mut(operation) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        map.remove(operation);
                    }
                }
            }
        }
    }

    /// 获取活动心跳数量
    pub fn active_heartbeat_count() -> usize {
        ACTIVE_HEARTBEATS.load(Ordering::Relaxed)
    }

    /// 获取活动心跳列表(调试用)
    pub fn get_active_heartbeats() -> Vec<(String, usize)> {
        if let Ok(registry) = HEARTBEAT_REGISTRY.lock() {
            if let Some(map) = registry.as_ref() {
                return map.iter().map(|(k, v)| (k.clone(), *v)).collect();
            }
        }
        Vec::new()
    }

    /// 清理所有心跳(程序退出时调用)
    pub fn cleanup_all() {
        // 重置计数器
        ACTIVE_HEARTBEATS.store(0, Ordering::Relaxed);
        ACTIVE_PROGRESS_BARS.store(0, Ordering::Relaxed);

        // 清空注册表
        if let Ok(mut registry) = HEARTBEAT_REGISTRY.lock() {
            *registry = None;
        }
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
