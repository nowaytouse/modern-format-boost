//! Heartbeat Manager - Global heartbeat manager
//!
//! 🔥 v7.7: Manages progress bar state and heartbeat registration
//!
//! ## Core Features
//! - Progress Bar Counting: Tracks the number of active progress bars
//! - Intelligent Silencing: Heartbeats automatically silence when a progress bar is present
//! - Heartbeat Registration: Tracks the number of active heartbeats
//! - Nesting Detection: Detects nested heartbeats (only the innermost is displayed)
//! - Thread-Safe: Uses atomic operations

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub struct HeartbeatManager;

static ACTIVE_PROGRESS_BARS: AtomicUsize = AtomicUsize::new(0);

static ACTIVE_HEARTBEATS: AtomicUsize = AtomicUsize::new(0);

static HEARTBEAT_REGISTRY: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

fn lock_registry() -> std::sync::MutexGuard<'static, Option<HashMap<String, usize>>> {
    HEARTBEAT_REGISTRY.lock().unwrap_or_else(|err| {
        eprintln!("⚠️ [Heartbeat Manager] registry mutex was poisoned; recovering state");
        err.into_inner()
    })
}

impl HeartbeatManager {
    pub fn register_progress_bar() {
        ACTIVE_PROGRESS_BARS.fetch_add(1, Ordering::Relaxed);
    }

    pub fn unregister_progress_bar() {
        // Avoid underflow when unregister is called more times than register (e.g. test cleanup).
        let mut current = ACTIVE_PROGRESS_BARS.load(Ordering::Relaxed);
        while current > 0 {
            match ACTIVE_PROGRESS_BARS.compare_exchange(
                current,
                current - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    pub fn has_active_progress() -> bool {
        ACTIVE_PROGRESS_BARS.load(Ordering::Relaxed) > 0
    }

    pub fn active_progress_count() -> usize {
        ACTIVE_PROGRESS_BARS.load(Ordering::Relaxed)
    }

    pub fn register_heartbeat(operation: &str) {
        ACTIVE_HEARTBEATS.fetch_add(1, Ordering::Relaxed);

        let mut registry = lock_registry();
        let map = registry.get_or_insert_with(HashMap::new);
        *map.entry(operation.to_string()).or_insert(0) += 1;

        if map[operation] > 1 && std::env::var("IMGQUALITY_DEBUG").is_ok() {
            eprintln!(
                "🔍 Debug: Multiple heartbeats with same name: {} (count: {})",
                operation, map[operation]
            );
        }
    }

    pub fn unregister_heartbeat(operation: &str) {
        let mut current = ACTIVE_HEARTBEATS.load(Ordering::Relaxed);
        while current > 0 {
            match ACTIVE_HEARTBEATS.compare_exchange(
                current,
                current - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }

        let mut registry = lock_registry();
        if let Some(map) = registry.as_mut() {
            if let Some(count) = map.get_mut(operation) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    map.remove(operation);
                }
            }
        }
    }

    pub fn active_heartbeat_count() -> usize {
        ACTIVE_HEARTBEATS.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn get_active_heartbeats() -> Vec<(String, usize)> {
        let registry = lock_registry();
        if let Some(map) = registry.as_ref() {
            return map.iter().map(|(k, v)| (k.clone(), *v)).collect();
        }
        Vec::new()
    }

    pub fn cleanup_all() {
        ACTIVE_HEARTBEATS.store(0, Ordering::Relaxed);
        ACTIVE_PROGRESS_BARS.store(0, Ordering::Relaxed);

        *lock_registry() = None;
    }
}

pub struct ProgressBarGuard;

impl ProgressBarGuard {
    #[must_use]
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
