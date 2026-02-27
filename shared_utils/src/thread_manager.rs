//! Smart Thread Manager for Apple Silicon optimization
//!
//! Provides intelligent thread allocation that:
//! - Maximizes performance on Apple Silicon chips
//! - Prevents system overload during multi-instance scenarios
//! - Reduces parallelism when system memory is low (avoids OOM kills)
//! - Allows environment-based configuration (MFB_LOW_MEMORY, MFB_MULTI_INSTANCE)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::system_memory::{self, MemoryPressure};

static MULTI_INSTANCE_MODE: AtomicBool = AtomicBool::new(false);

static OPTIMAL_THREADS: OnceLock<usize> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct ThreadConfig {
    pub core_percentage: usize,
    pub min_threads: usize,
    pub max_threads: usize,
    pub multi_instance_aware: bool,
}

impl Default for ThreadConfig {
    fn default() -> Self {
        Self {
            core_percentage: 70,
            min_threads: 2,
            max_threads: 16,
            multi_instance_aware: true,
        }
    }
}

impl ThreadConfig {
    pub fn conservative() -> Self {
        Self {
            core_percentage: 50,
            min_threads: 1,
            max_threads: 8,
            multi_instance_aware: true,
        }
    }

    pub fn aggressive() -> Self {
        Self {
            core_percentage: 90,
            min_threads: 4,
            max_threads: 32,
            multi_instance_aware: false,
        }
    }

    pub fn video_processing() -> Self {
        Self {
            core_percentage: 60,
            min_threads: 2,
            max_threads: 12,
            multi_instance_aware: true,
        }
    }
}

pub fn calculate_optimal_threads(config: &ThreadConfig) -> usize {
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let effective_percentage = if config.multi_instance_aware && is_multi_instance() {
        config.core_percentage / 2
    } else {
        config.core_percentage
    };

    let mut calculated = (cpu_count * effective_percentage / 100).max(1);
    calculated = calculated.clamp(config.min_threads, config.max_threads);

    let memory_cap = match (system_memory::memory_pressure_level(), system_memory::is_low_memory_env()) {
        (_, true) | (Some(MemoryPressure::High), _) => 2,
        (Some(MemoryPressure::Normal), _) => 4,
        _ => calculated,
    };
    calculated.min(memory_cap).max(1)
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadAllocation {
    pub parallel_tasks: usize,
    pub child_threads: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum WorkloadType {
    Image,
    Video,
}

/// Apply memory-pressure caps so we don't spawn too many heavy workers and trigger OOM.
fn apply_memory_cap(parallel_tasks: usize, child_threads: usize) -> (usize, usize) {
    let pressure = system_memory::memory_pressure_level();
    let low_mem_env = system_memory::is_low_memory_env();

    if low_mem_env || pressure == Some(MemoryPressure::High) {
        return (1, 1);
    }
    if pressure == Some(MemoryPressure::Normal) {
        let pt = parallel_tasks.min(2);
        let ct = child_threads.min(2);
        return (pt, ct);
    }
    (parallel_tasks, child_threads)
}

pub fn get_balanced_thread_config(workload: WorkloadType) -> ThreadAllocation {
    let total_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let reserved = (total_cores as f64 * 0.2).ceil() as usize;
    let reserved = reserved.clamp(1, 2);

    let available_cores = total_cores.saturating_sub(reserved).max(1);

    let (parallel_tasks, child_threads) = match workload {
        WorkloadType::Image => {
            let child_threads = 2;
            let parallel_tasks = (available_cores / child_threads).max(1);
            let parallel_tasks = parallel_tasks.clamp(1, 8);
            apply_memory_cap(parallel_tasks, child_threads)
        }
        WorkloadType::Video => {
            let parallel_tasks = if available_cores >= 8 { 2 } else { 1 };
            let child_threads = (available_cores / parallel_tasks).max(1);
            apply_memory_cap(parallel_tasks, child_threads)
        }
    };

    ThreadAllocation {
        parallel_tasks: parallel_tasks.max(1),
        child_threads: child_threads.max(1),
    }
}

pub fn get_optimal_threads() -> usize {
    *OPTIMAL_THREADS.get_or_init(|| get_balanced_thread_config(WorkloadType::Image).parallel_tasks)
}

/// Optional hint for logging when parallelism was reduced due to memory (e.g. "low memory: reduced parallelism").
pub fn memory_cap_hint() -> Option<&'static str> {
    if system_memory::is_low_memory_env() {
        return Some("MFB_LOW_MEMORY=1: reduced parallelism");
    }
    match system_memory::memory_pressure_level() {
        Some(MemoryPressure::High) => Some("low available RAM: parallelism reduced to avoid OOM"),
        Some(MemoryPressure::Normal) => Some("moderate RAM: slightly reduced parallelism"),
        _ => None,
    }
}

pub fn get_ffmpeg_threads() -> usize {
    calculate_optimal_threads(&ThreadConfig::video_processing())
}

pub fn is_multi_instance() -> bool {
    if std::env::var("MFB_MULTI_INSTANCE").is_ok() {
        return true;
    }

    MULTI_INSTANCE_MODE.load(Ordering::Relaxed)
}

pub fn enable_multi_instance_mode() {
    MULTI_INSTANCE_MODE.store(true, Ordering::Relaxed);
}

pub fn disable_multi_instance_mode() {
    MULTI_INSTANCE_MODE.store(false, Ordering::Relaxed);
}

pub fn get_rsync_path() -> &'static str {
    static RSYNC_PATH: OnceLock<String> = OnceLock::new();

    RSYNC_PATH.get_or_init(|| {
        which::which("rsync")
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| "rsync".to_string())
    })
}

pub fn get_rsync_version() -> Option<String> {
    use std::process::Command;

    let output = Command::new(get_rsync_path())
        .arg("--version")
        .output()
        .ok()?;

    if output.status.success() {
        let version_line = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()?
            .to_string();
        Some(version_line)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_thread_calculation() {
        let threads = get_optimal_threads();
        assert!(threads >= 1, "memory cap may reduce to 1");
        assert!(threads <= 16);
    }

    #[test]
    fn test_ffmpeg_threads() {
        let threads = get_ffmpeg_threads();
        assert!(threads >= 1, "memory cap may reduce to 1");
        assert!(threads <= 12);
    }

    #[test]
    fn test_thread_config() {
        let config = ThreadConfig::conservative();
        let threads = calculate_optimal_threads(&config);
        assert!(threads >= config.min_threads);
        assert!(threads <= config.max_threads);
    }

    #[test]
    fn test_rsync_path() {
        let path = get_rsync_path();
        assert!(!path.is_empty());
    }
}
