//! Smart Thread Manager for Apple Silicon optimization
//!
//! Provides intelligent thread allocation that:
//! - Maximizes performance on Apple Silicon chips
//! - Prevents system overload during multi-instance scenarios
//! - Reduces parallelism when system memory is low (avoids OOM kills)
//! - Allows environment-based configuration (`MFB_LOW_MEMORY`, `MFB_MULTI_INSTANCE`)

use crate::{RsyncBuilder, ToolBuilder};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::x265_params::X265MemoryProfile;

static MULTI_INSTANCE_MODE: AtomicBool = AtomicBool::new(false);

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
            core_percentage: crate::constants::THREAD_PERCENTAGE_DEFAULT,
            min_threads: 2,
            max_threads: 16,
            multi_instance_aware: true,
        }
    }
}

impl ThreadConfig {
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            core_percentage: crate::constants::THREAD_PERCENTAGE_CONSERVATIVE,
            min_threads: 1,
            max_threads: 8,
            multi_instance_aware: true,
        }
    }

    #[must_use]
    pub const fn aggressive() -> Self {
        Self {
            core_percentage: crate::constants::THREAD_PERCENTAGE_AGGRESSIVE,
            min_threads: 4,
            max_threads: 32,
            multi_instance_aware: false,
        }
    }

    #[must_use]
    pub const fn video_processing() -> Self {
        Self {
            core_percentage: crate::constants::THREAD_PERCENTAGE_VIDEO,
            min_threads: 2,
            max_threads: 12,
            multi_instance_aware: true,
        }
    }
}

#[must_use]
pub fn calculate_optimal_threads(config: &ThreadConfig) -> usize {
    let cpu_count = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);

    let effective_percentage = if config.multi_instance_aware && is_multi_instance() {
        config.core_percentage / 2
    } else {
        config.core_percentage
    };

    let mut calculated = (cpu_count * effective_percentage / 100).max(1);
    calculated = calculated.clamp(config.min_threads, config.max_threads);

    let memory_cap = match current_memory_profile() {
        X265MemoryProfile::LowMemory => 2,
        X265MemoryProfile::Moderate => 4,
        X265MemoryProfile::Default => calculated,
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

fn current_memory_profile() -> X265MemoryProfile {
    crate::x265_params::current_memory_profile()
}

fn reserve_headroom_cores(total_cores: usize, profile: X265MemoryProfile) -> usize {
    let upper = total_cores.saturating_sub(1).max(1);
    let (fraction, min_reserved, max_reserved) = match profile {
        X265MemoryProfile::Default => (crate::constants::X265_MEM_RATIO_DEFAULT, 1, 2),
        X265MemoryProfile::Moderate => (crate::constants::X265_MEM_RATIO_MODERATE, 2, 4),
        X265MemoryProfile::LowMemory => (crate::constants::X265_MEM_RATIO_LOW, 3, 6),
    };
    let calculated = crate::numeric_cast::f64_to_usize_sat(
        (crate::numeric_cast::usize_to_f64(total_cores) * fraction).ceil(),
    );
    let lower = min_reserved.min(upper);
    let upper = max_reserved.min(upper).max(lower);
    calculated.clamp(lower, upper)
}

fn clamp_child_threads(
    per_task: usize,
    available_cores: usize,
    min_threads: usize,
    max_threads: usize,
) -> usize {
    let upper = max_threads.min(available_cores).max(1);
    let lower = min_threads.min(upper);
    per_task.clamp(lower, upper)
}

/// Apply the x265-aligned RAM tier to file-level parallelism so low-memory mode
/// degrades to single-file processing with extra headroom instead of thrashing.
fn apply_memory_cap(
    workload: WorkloadType,
    parallel_tasks: usize,
    child_threads: usize,
    profile: X265MemoryProfile,
) -> (usize, usize) {
    match profile {
        X265MemoryProfile::Default => (parallel_tasks, child_threads),
        X265MemoryProfile::Moderate => match workload {
            WorkloadType::Image => (parallel_tasks.min(4), child_threads.min(2)),
            WorkloadType::Video => (parallel_tasks.min(2), child_threads.min(4)),
        },
        X265MemoryProfile::LowMemory => (1, 1),
    }
}

fn apply_multi_instance_cap(
    workload: WorkloadType,
    parallel_tasks: usize,
    child_threads: usize,
    multi_instance: bool,
) -> (usize, usize) {
    if !multi_instance {
        return (parallel_tasks, child_threads);
    }

    match workload {
        WorkloadType::Image => ((parallel_tasks / 2).max(1), child_threads.max(1)),
        WorkloadType::Video => (parallel_tasks.min(1), child_threads.div_ceil(2).max(1)),
    }
}

fn balanced_thread_config_for(
    total_cores: usize,
    workload: WorkloadType,
    profile: X265MemoryProfile,
    multi_instance: bool,
) -> ThreadAllocation {
    let reserved = reserve_headroom_cores(total_cores, profile);
    let available_cores = total_cores.saturating_sub(reserved).max(1);

    let (parallel_tasks, child_threads) = match workload {
        WorkloadType::Image => {
            let child_threads = if matches!(profile, X265MemoryProfile::LowMemory) {
                1
            } else {
                2.min(available_cores).max(1)
            };
            let parallel_tasks = available_cores.div_ceil(child_threads).clamp(1, 12);
            (parallel_tasks, child_threads)
        }
        WorkloadType::Video => match profile {
            X265MemoryProfile::Default => {
                let parallel_tasks = (available_cores / 2).max(1).clamp(1, 4);
                let per_task = (available_cores / parallel_tasks).max(1);
                let child_threads = clamp_child_threads(per_task, available_cores, 2, 4);
                (parallel_tasks, child_threads)
            }
            X265MemoryProfile::Moderate => {
                let parallel_tasks = (available_cores / 3).max(1).clamp(1, 2);
                let per_task = (available_cores / parallel_tasks).max(1);
                let child_threads = clamp_child_threads(per_task, available_cores, 2, 4);
                (parallel_tasks, child_threads)
            }
            X265MemoryProfile::LowMemory => (1, 1),
        },
    };

    let (parallel_tasks, child_threads) = if multi_instance {
        apply_multi_instance_cap(workload, parallel_tasks, child_threads, true)
    } else {
        (parallel_tasks, child_threads)
    };
    let (parallel_tasks, child_threads) =
        apply_memory_cap(workload, parallel_tasks, child_threads, profile);

    ThreadAllocation {
        parallel_tasks: parallel_tasks.max(1),
        child_threads: child_threads.max(1),
    }
}

#[must_use]
pub fn get_balanced_thread_config(workload: WorkloadType) -> ThreadAllocation {
    let total_cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let profile = current_memory_profile();
    balanced_thread_config_for(total_cores, workload, profile, is_multi_instance())
}

#[must_use]
pub fn get_optimal_threads() -> usize {
    get_balanced_thread_config(WorkloadType::Image).parallel_tasks
}

/// Optional hint for logging when parallelism was reduced due to memory (e.g. "low memory: reduced parallelism").
#[must_use]
pub fn memory_cap_hint() -> Option<&'static str> {
    match current_memory_profile() {
        X265MemoryProfile::LowMemory => {
            Some("low available RAM: single-file mode enabled to preserve responsiveness")
        }
        X265MemoryProfile::Moderate => {
            Some("moderate RAM: parallelism trimmed to preserve headroom")
        }
        X265MemoryProfile::Default => None,
    }
}

#[must_use]
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

#[must_use]
pub fn get_rsync_version() -> Option<String> {
    let output = RsyncBuilder::new()
        .executable(get_rsync_path())
        .arg("--version")
        .build()
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

    #[test]
    fn test_apply_multi_instance_cap_reduces_image_parallelism() {
        let (parallel_tasks, child_threads) =
            apply_multi_instance_cap(WorkloadType::Image, 6, 2, true);

        assert_eq!(parallel_tasks, 3);
        assert_eq!(child_threads, 2);
    }

    #[test]
    fn test_apply_multi_instance_cap_reduces_video_parallelism() {
        let (parallel_tasks, child_threads) =
            apply_multi_instance_cap(WorkloadType::Video, 2, 8, true);

        assert_eq!(parallel_tasks, 1);
        assert_eq!(child_threads, 4);
    }

    #[test]
    fn test_low_memory_disables_parallel_batch_processing() {
        let image = balanced_thread_config_for(
            10,
            WorkloadType::Image,
            X265MemoryProfile::LowMemory,
            false,
        );
        let video = balanced_thread_config_for(
            10,
            WorkloadType::Video,
            X265MemoryProfile::LowMemory,
            false,
        );

        assert_eq!(image.parallel_tasks, 1);
        assert_eq!(image.child_threads, 1);
        assert_eq!(video.parallel_tasks, 1);
        assert_eq!(video.child_threads, 1);
    }

    #[test]
    fn test_default_profile_enables_more_video_parallelism_than_moderate() {
        let default =
            balanced_thread_config_for(12, WorkloadType::Video, X265MemoryProfile::Default, false);
        let moderate =
            balanced_thread_config_for(12, WorkloadType::Video, X265MemoryProfile::Moderate, false);

        assert!(default.parallel_tasks > moderate.parallel_tasks);
        assert!(default.parallel_tasks > 1);
    }

    #[test]
    fn test_default_profile_scales_image_parallelism() {
        let allocation =
            balanced_thread_config_for(20, WorkloadType::Image, X265MemoryProfile::Default, false);

        assert!(allocation.parallel_tasks >= 8);
        assert_eq!(allocation.child_threads, 2);
    }
}
