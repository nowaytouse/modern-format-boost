//! Smart Thread Manager for Apple Silicon optimization
//!
//! Provides intelligent thread allocation that:
//! - Maximizes performance on Apple Silicon chips
//! - Prevents system overload during multi-instance scenarios
//! - Reduces parallelism when system memory is low (avoids OOM kills)
//! - Allows environment-based configuration (`MFB_LOW_MEMORY`,
//!   `MFB_MULTI_INSTANCE`)

use crate::{RsyncBuilder, ToolBuilder};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::performance_schedule::PerfGovernorTier;
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
    let cpu_count = crate::media_conversion_gate::runtime_available_parallelism_or_default(
        "thread_manager::calculate_optimal_threads",
    );

    let tier = crate::performance_schedule::current_perf_tier();
    let tier_scale = crate::performance_schedule::thread_percentage_scale(tier);
    let mut effective_percentage = config.core_percentage * tier_scale / 100;
    if config.multi_instance_aware && is_multi_instance() {
        effective_percentage /= 2;
    }

    let mut calculated = (cpu_count * effective_percentage / 100).max(1);
    calculated = calculated.clamp(config.min_threads, config.max_threads);

    let profile = current_memory_profile();
    let memory_cap = match (profile, tier) {
        (X265MemoryProfile::LowMemory, _)
        | (X265MemoryProfile::Moderate, crate::performance_schedule::PerfGovernorTier::Tight) => 2,
        (X265MemoryProfile::Moderate, _) => 4,
        (X265MemoryProfile::Default, crate::performance_schedule::PerfGovernorTier::Tight) => 3,
        (X265MemoryProfile::Default, crate::performance_schedule::PerfGovernorTier::Balanced) => {
            calculated.min(16)
        }
        (X265MemoryProfile::Default, crate::performance_schedule::PerfGovernorTier::Relaxed) => {
            calculated.min(crate::constants::PERF_STABILITY_MAX_IMAGE_PARALLEL)
        }
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

fn reserve_headroom_cores(
    total_cores: usize,
    profile: X265MemoryProfile,
    perf_tier: PerfGovernorTier,
) -> usize {
    let upper = total_cores.saturating_sub(1).max(1);
    let reservation = crate::performance_schedule::headroom_reservation(profile, perf_tier);
    let fraction_reserve = crate::numeric_cast::f64_to_usize_sat(
        (crate::numeric_cast::usize_to_f64(total_cores) * reservation.fraction).ceil(),
    );
    let os_reserve = crate::performance_schedule::minimum_os_reserve_cores(total_cores, perf_tier);
    let calculated = fraction_reserve.max(os_reserve);
    let lower = reservation.min_reserved.max(os_reserve).min(upper);
    let upper_bound = reservation
        .max_reserved
        .max(os_reserve)
        .min(upper)
        .max(lower);
    calculated.clamp(lower, upper_bound)
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
    perf_tier: PerfGovernorTier,
) -> (usize, usize) {
    let (parallel_tasks, child_threads) = match profile {
        X265MemoryProfile::Default => (parallel_tasks, child_threads),
        X265MemoryProfile::Moderate => match workload {
            WorkloadType::Image => (parallel_tasks.min(4), child_threads.min(2)),
            WorkloadType::Video => (parallel_tasks.min(2), child_threads.min(4)),
        },
        X265MemoryProfile::LowMemory => (1, 1),
    };
    let child_cap = crate::performance_schedule::child_thread_cap(workload, profile, perf_tier);
    let (parallel_tasks, child_threads) = crate::performance_schedule::apply_delivery_parallel_cap(
        workload,
        parallel_tasks,
        child_threads.min(child_cap),
        perf_tier,
    );
    (parallel_tasks, child_threads)
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
    perf_tier: PerfGovernorTier,
) -> ThreadAllocation {
    let reserved = reserve_headroom_cores(total_cores, profile, perf_tier);
    let available_cores = total_cores.saturating_sub(reserved).max(1);

    let (parallel_tasks, child_threads) = match workload {
        WorkloadType::Image => {
            let child_threads = if matches!(profile, X265MemoryProfile::LowMemory) {
                1
            } else {
                crate::performance_schedule::default_child_threads_per_task(perf_tier)
                    .min(available_cores)
                    .max(1)
            };
            let cap = crate::performance_schedule::image_parallel_cap(profile, perf_tier);
            let child_cap =
                crate::performance_schedule::child_thread_cap(workload, profile, perf_tier);
            let child_threads = child_threads.min(child_cap);
            let parallel_tasks = available_cores.div_ceil(child_threads).clamp(1, cap);
            (parallel_tasks, child_threads)
        }
        WorkloadType::Video => match profile {
            X265MemoryProfile::Default => {
                let cap = crate::performance_schedule::video_parallel_cap(profile, perf_tier);
                let child_cap =
                    crate::performance_schedule::child_thread_cap(workload, profile, perf_tier);
                let parallel_tasks = (available_cores / 2).max(1).clamp(1, cap);
                let per_task = (available_cores / parallel_tasks).max(1);
                let child_threads =
                    clamp_child_threads(per_task, available_cores, 2, 4).min(child_cap);
                (parallel_tasks, child_threads)
            }
            X265MemoryProfile::Moderate => {
                let child_cap =
                    crate::performance_schedule::child_thread_cap(workload, profile, perf_tier);
                let parallel_tasks = (available_cores / 3).max(1).clamp(1, 2);
                let per_task = (available_cores / parallel_tasks).max(1);
                let child_threads =
                    clamp_child_threads(per_task, available_cores, 2, 4).min(child_cap);
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
        apply_memory_cap(workload, parallel_tasks, child_threads, profile, perf_tier);
    let (parallel_tasks, child_threads) = crate::performance_schedule::clamp_compute_fanout(
        workload,
        parallel_tasks,
        child_threads,
        total_cores,
        perf_tier,
    );

    ThreadAllocation {
        parallel_tasks: parallel_tasks.max(1),
        child_threads: child_threads.max(1),
    }
}

#[must_use]
pub fn get_balanced_thread_config(workload: WorkloadType) -> ThreadAllocation {
    let total_cores = crate::media_conversion_gate::runtime_available_parallelism_or_default(
        "thread_manager::get_balanced_thread_config",
    );
    let profile = current_memory_profile();
    balanced_thread_config_for(
        total_cores,
        workload,
        profile,
        is_multi_instance(),
        crate::performance_schedule::current_perf_tier(),
    )
}

#[must_use]
pub fn get_optimal_threads() -> usize {
    get_balanced_thread_config(WorkloadType::Image).parallel_tasks
}

/// Optional hint for logging when parallelism was reduced due to memory (e.g.
/// "low memory: reduced parallelism").
#[must_use]
pub fn memory_cap_hint() -> Option<&'static str> {
    if let Some(hint) = crate::performance_schedule::stability_cap_hint() {
        return Some(hint);
    }
    if matches!(
        crate::performance_schedule::current_perf_tier(),
        crate::performance_schedule::PerfGovernorTier::Tight
    ) {
        return Some("performance governor tight: parallelism capped for responsiveness");
    }
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
    if std::env::var(crate::constants::ENV_MFB_MULTI_INSTANCE).is_ok() {
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

    RSYNC_PATH.get_or_init(crate::media_conversion_gate::delivery_rsync_executable_or_default)
}

#[must_use]
pub fn get_rsync_version() -> Option<String> {
    let output = RsyncBuilder::new()
        .executable(get_rsync_path())
        .arg("--version")
        .build()
        .output();
    let output = match output {
        Ok(output) => output,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "rsync_version",
                format!("failed to run rsync --version: {e}"),
            );
            return None;
        }
    };

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
        assert_ne!(path, "");
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
            PerfGovernorTier::Balanced,
        );
        let video = balanced_thread_config_for(
            10,
            WorkloadType::Video,
            X265MemoryProfile::LowMemory,
            false,
            PerfGovernorTier::Balanced,
        );

        assert_eq!(image.parallel_tasks, 1);
        assert_eq!(image.child_threads, 1);
        assert_eq!(video.parallel_tasks, 1);
        assert_eq!(video.child_threads, 1);
    }

    #[test]
    fn test_default_profile_enables_more_video_parallelism_than_moderate() {
        let default = balanced_thread_config_for(
            12,
            WorkloadType::Video,
            X265MemoryProfile::Default,
            false,
            PerfGovernorTier::Relaxed,
        );
        let moderate = balanced_thread_config_for(
            12,
            WorkloadType::Video,
            X265MemoryProfile::Moderate,
            false,
            PerfGovernorTier::Relaxed,
        );

        assert!(default.parallel_tasks > moderate.parallel_tasks);
        assert!(default.parallel_tasks > 1);
    }

    #[test]
    fn test_default_profile_scales_image_parallelism() {
        let allocation = balanced_thread_config_for(
            24,
            WorkloadType::Image,
            X265MemoryProfile::Default,
            false,
            PerfGovernorTier::Relaxed,
        );

        assert!(allocation.parallel_tasks >= 6);
        assert!(allocation.child_threads >= 2);
        assert!(
            allocation
                .parallel_tasks
                .saturating_mul(allocation.child_threads)
                <= 28
        );
    }
}
