//! Unified performance governor (SSOT).
//!
//! Wider thread/scan headroom when RAM is plentiful; faster tightening under
//! memory pressure, multi-instance contention, or explicit low-memory mode.
//!
//! **Stability:** absolute parallelism ceilings, minimum OS core reserve, and
//! compute fan-out caps prevent OOM / UI freezes even when tier is `relaxed` or
//! env requests `wide`.

use crate::system_memory::{self, MemoryPressure};
use crate::thread_manager;
use crate::thread_manager::WorkloadType;
use crate::x265_params::X265MemoryProfile;

/// Ratio below which `Normal` RAM still maps to `tight` (early preemptive
/// throttle).
const PREEMPTIVE_TIGHT_RATIO: f64 = 0.24;
/// Available MB below which `Normal` RAM still maps to `tight`.
const PREEMPTIVE_TIGHT_MIN_MB: u64 = 2304;

/// Scheduling aggressiveness derived from live system signals and env
/// overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PerfGovernorTier {
    /// Max headroom: fewer reserved cores, higher parallelism caps.
    Relaxed,
    /// Default production balance.
    Balanced,
    /// Fast reaction to pressure: reserve more cores, cap parallelism hard.
    Tight,
}

impl PerfGovernorTier {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Relaxed => "relaxed",
            Self::Balanced => "balanced",
            Self::Tight => "tight",
        }
    }

    #[must_use]
    pub fn parse_env(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "relaxed" | "wide" => Some(Self::Relaxed),
            "balanced" | "normal" | "default" => Some(Self::Balanced),
            "tight" | "strict" | "conservative" => Some(Self::Tight),
            _ => None,
        }
    }
}

/// Fraction of cores to reserve plus min/max reserved bounds.
#[derive(Debug, Clone, Copy)]
pub struct HeadroomReservation {
    pub fraction: f64,
    pub min_reserved: usize,
    pub max_reserved: usize,
}

#[must_use]
fn should_preemptive_tight() -> bool {
    let Some((available_mb, total_mb)) = system_memory::get_memory_mb() else {
        return false;
    };
    if total_mb == 0 {
        return false;
    }
    let ratio =
        crate::numeric_cast::u64_to_f64(available_mb) / crate::numeric_cast::u64_to_f64(total_mb);
    ratio < PREEMPTIVE_TIGHT_RATIO || available_mb < PREEMPTIVE_TIGHT_MIN_MB
}

#[must_use]
pub fn perf_tier_from_env() -> Option<PerfGovernorTier> {
    match std::env::var(crate::constants::ENV_MFB_PERF_TIER) {
        Ok(value) => PerfGovernorTier::parse_env(&value),
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "performance_tier_env",
                format!(
                    "failed to read {}: {e}",
                    crate::constants::ENV_MFB_PERF_TIER
                ),
            );
            None
        }
    }
}

/// Raw tier from env + RAM (may request `relaxed` on a stressed host).
#[must_use]
pub fn perf_tier_uncapped() -> PerfGovernorTier {
    if let Some(tier) = perf_tier_from_env() {
        return tier;
    }
    if system_memory::is_low_memory_env() {
        return PerfGovernorTier::Tight;
    }

    let multi = thread_manager::is_multi_instance();
    match system_memory::memory_pressure_level() {
        Some(MemoryPressure::Low) if !multi => PerfGovernorTier::Relaxed,
        Some(MemoryPressure::Low) => PerfGovernorTier::Balanced,
        Some(MemoryPressure::Normal) | None => {
            if multi || should_preemptive_tight() {
                PerfGovernorTier::Tight
            } else {
                PerfGovernorTier::Balanced
            }
        }
        Some(MemoryPressure::High) => PerfGovernorTier::Tight,
    }
}

/// Downgrade auto-selected `relaxed` when RAM/host signals cannot safely
/// sustain max throughput.
///
/// Explicit `MFB_PERF_TIER` is honored except under `MemoryPressure::High` (OOM
/// guard).
#[must_use]
pub fn clamp_tier_for_stability(tier: PerfGovernorTier) -> PerfGovernorTier {
    let env_override = perf_tier_from_env().is_some();
    match tier {
        PerfGovernorTier::Relaxed => {
            if matches!(
                system_memory::memory_pressure_level(),
                Some(MemoryPressure::High)
            ) {
                return PerfGovernorTier::Tight;
            }
            if env_override {
                return PerfGovernorTier::Relaxed;
            }
            if should_preemptive_tight() || thread_manager::is_multi_instance() {
                return PerfGovernorTier::Balanced;
            }
            if let Some((_, total_mb)) = system_memory::get_memory_mb()
                && total_mb < crate::constants::PERF_STABILITY_MIN_TOTAL_RAM_MB_FOR_RELAXED
            {
                return PerfGovernorTier::Balanced;
            }
            PerfGovernorTier::Relaxed
        }
        PerfGovernorTier::Balanced | PerfGovernorTier::Tight => tier,
    }
}

/// Effective tier used by conversion/training schedulers (stability-clamped).
#[must_use]
pub fn current_perf_tier() -> PerfGovernorTier {
    clamp_tier_for_stability(perf_tier_uncapped())
}

/// True when env asked for a tier stricter than stability allows right now.
#[must_use]
pub fn perf_tier_was_downgraded_for_stability() -> bool {
    perf_tier_uncapped() != current_perf_tier()
}

#[must_use]
pub fn stability_cap_hint() -> Option<&'static str> {
    if perf_tier_was_downgraded_for_stability() {
        Some(
            "performance tier downgraded for system stability (RAM pressure, host size, or \
             contention)",
        )
    } else {
        None
    }
}

#[must_use]
fn stability_cap_usize(value: usize, max: usize) -> usize {
    value.min(max)
}

/// Minimum cores left for the OS/desktop under load (in addition to
/// fraction-based reserve).
#[must_use]
pub fn minimum_os_reserve_cores(total_cores: usize, tier: PerfGovernorTier) -> usize {
    if total_cores <= 2 {
        return 1;
    }
    let reserve = match tier {
        PerfGovernorTier::Relaxed => (total_cores / 4).max(2),
        PerfGovernorTier::Balanced => (total_cores / 5).max(2),
        PerfGovernorTier::Tight => total_cores.saturating_mul(2) / 5,
    };
    reserve.min(8).min(total_cores.saturating_sub(1).max(1))
}

/// Cap `parallel_tasks * child_threads` to avoid thread/RAM storms on wide
/// CPUs.
#[must_use]
pub fn clamp_compute_fanout(
    workload: WorkloadType,
    parallel_tasks: usize,
    child_threads: usize,
    total_cores: usize,
    tier: PerfGovernorTier,
) -> (usize, usize) {
    let total_cores = total_cores.max(1);
    let mut parallel_tasks = parallel_tasks.max(1);
    let mut child_threads = child_threads.max(1);
    let workload_max = match workload {
        WorkloadType::Image => 28usize,
        WorkloadType::Video => 20usize,
    };
    let max_fanout = match tier {
        PerfGovernorTier::Relaxed => total_cores.saturating_mul(2).min(workload_max),
        PerfGovernorTier::Balanced => total_cores.saturating_mul(3).div_ceil(2).min(22),
        PerfGovernorTier::Tight => total_cores.saturating_add(2).min(12),
    }
    .max(1);

    if parallel_tasks.saturating_mul(child_threads) <= max_fanout {
        return (parallel_tasks, child_threads);
    }
    child_threads = (max_fanout / parallel_tasks).max(1);
    if parallel_tasks.saturating_mul(child_threads) > max_fanout {
        parallel_tasks = (max_fanout / child_threads).max(1);
    }
    (parallel_tasks, child_threads)
}

#[must_use]
pub fn headroom_reservation(
    profile: X265MemoryProfile,
    tier: PerfGovernorTier,
) -> HeadroomReservation {
    let (base_frac, base_min, base_max) = match profile {
        X265MemoryProfile::Default => (crate::constants::X265_MEM_RATIO_DEFAULT, 1usize, 2usize),
        X265MemoryProfile::Moderate => (crate::constants::X265_MEM_RATIO_MODERATE, 2, 4),
        X265MemoryProfile::LowMemory => (crate::constants::X265_MEM_RATIO_LOW, 3, 6),
    };

    match tier {
        PerfGovernorTier::Relaxed => HeadroomReservation {
            fraction: base_frac * 0.50,
            min_reserved: base_min,
            max_reserved: 1,
        },
        PerfGovernorTier::Balanced => HeadroomReservation {
            fraction: base_frac,
            min_reserved: base_min,
            max_reserved: base_max,
        },
        PerfGovernorTier::Tight => HeadroomReservation {
            fraction: (base_frac * 1.50).min(0.60),
            min_reserved: base_min.saturating_add(1).min(6),
            max_reserved: (base_max.saturating_add(2)).min(8),
        },
    }
}

/// Upper bound on image batch parallelism for the given RAM profile and
/// governor tier.
#[must_use]
pub fn image_parallel_cap(profile: X265MemoryProfile, tier: PerfGovernorTier) -> usize {
    match (profile, tier) {
        (X265MemoryProfile::LowMemory, _) => 1,
        (X265MemoryProfile::Moderate, PerfGovernorTier::Relaxed) => 8,
        (X265MemoryProfile::Moderate, PerfGovernorTier::Balanced) => 5,
        (X265MemoryProfile::Moderate, PerfGovernorTier::Tight) => 2,
        (X265MemoryProfile::Default, PerfGovernorTier::Relaxed) => {
            stability_cap_usize(24, crate::constants::PERF_STABILITY_MAX_IMAGE_PARALLEL)
        }
        (X265MemoryProfile::Default, PerfGovernorTier::Balanced) => {
            stability_cap_usize(16, crate::constants::PERF_STABILITY_MAX_IMAGE_PARALLEL)
        }
        (X265MemoryProfile::Default, PerfGovernorTier::Tight) => 3,
    }
}

/// Upper bound on concurrent video tasks for Default RAM profile.
#[must_use]
pub fn video_parallel_cap(profile: X265MemoryProfile, tier: PerfGovernorTier) -> usize {
    match (profile, tier) {
        (X265MemoryProfile::LowMemory, _)
        | (X265MemoryProfile::Default, PerfGovernorTier::Tight) => 1,
        (X265MemoryProfile::Moderate, PerfGovernorTier::Relaxed) => 3,
        (X265MemoryProfile::Moderate, _) => 2,
        (X265MemoryProfile::Default, PerfGovernorTier::Relaxed) => {
            stability_cap_usize(6, crate::constants::PERF_STABILITY_MAX_VIDEO_PARALLEL)
        }
        (X265MemoryProfile::Default, PerfGovernorTier::Balanced) => {
            stability_cap_usize(4, crate::constants::PERF_STABILITY_MAX_VIDEO_PARALLEL)
        }
    }
}

/// Scale `ThreadConfig::core_percentage` before CPU-based thread calculation.
#[must_use]
pub const fn thread_percentage_scale(tier: PerfGovernorTier) -> usize {
    match tier {
        PerfGovernorTier::Relaxed => 100,
        PerfGovernorTier::Balanced => 92,
        PerfGovernorTier::Tight => 50,
    }
}

/// Default per-file child thread target before RAM profile caps.
#[must_use]
pub const fn default_child_threads_per_task(tier: PerfGovernorTier) -> usize {
    match tier {
        PerfGovernorTier::Relaxed => 3,
        PerfGovernorTier::Balanced => 2,
        PerfGovernorTier::Tight => 1,
    }
}

/// Default GPU encode slot cap when `MODERN_FORMAT_BOOST_GPU_CONCURRENCY` is
/// unset.
#[must_use]
pub fn gpu_concurrency_cap(tier: PerfGovernorTier) -> usize {
    let base = crate::constants::GPU_DEFAULT_CONCURRENCY;
    match tier {
        PerfGovernorTier::Relaxed => stability_cap_usize(
            base + 1,
            crate::constants::PERF_STABILITY_MAX_GPU_CONCURRENCY,
        ),
        PerfGovernorTier::Balanced => base,
        PerfGovernorTier::Tight => 1,
    }
}

/// Cap libx265 pool threads after RAM profile caps (`format` /
/// `format_x265_lossless_params`).
#[must_use]
pub fn x265_pool_thread_cap(requested: usize, tier: PerfGovernorTier) -> usize {
    let requested = requested.max(1);
    match tier {
        PerfGovernorTier::Relaxed => {
            requested.min(crate::constants::PERF_STABILITY_MAX_X265_POOL_THREADS_RELAXED)
        }
        PerfGovernorTier::Balanced => requested.min(10),
        PerfGovernorTier::Tight => requested.min(3),
    }
}

/// Per-file encoder/tool thread cap (cjxl, ffmpeg child, x265 pools input).
#[must_use]
pub fn child_thread_cap(
    workload: WorkloadType,
    profile: X265MemoryProfile,
    tier: PerfGovernorTier,
) -> usize {
    match (workload, profile, tier) {
        (_, X265MemoryProfile::LowMemory, _)
        | (WorkloadType::Image, _, PerfGovernorTier::Tight) => 1,
        (WorkloadType::Image, _, PerfGovernorTier::Balanced) => 3,
        (WorkloadType::Image | WorkloadType::Video, _, PerfGovernorTier::Relaxed) => {
            stability_cap_usize(6, crate::constants::PERF_STABILITY_MAX_CHILD_THREADS)
        }
        (WorkloadType::Video, _, PerfGovernorTier::Tight) => 2,
        (WorkloadType::Video, _, PerfGovernorTier::Balanced) => 4,
    }
}

fn scale_threshold_u64(base: u64, factor: f64) -> u64 {
    crate::numeric_cast::f64_to_u64_sat(crate::numeric_cast::u64_to_f64(base) * factor)
}

fn scale_threshold_f32(base: f32, factor: f32) -> f32 {
    base * factor
}

/// GPU coarse-search: tier scales when parallel probe is allowed vs sequential.
#[must_use]
pub fn gpu_large_file_threshold_bytes(tier: PerfGovernorTier) -> u64 {
    let base = crate::constants::GPU_LARGE_FILE_THRESHOLD_BYTES;
    match tier {
        PerfGovernorTier::Relaxed => scale_threshold_u64(base, 1.25),
        PerfGovernorTier::Balanced => scale_threshold_u64(base, 0.88),
        PerfGovernorTier::Tight => base / 2,
    }
}

#[must_use]
pub fn gpu_very_large_file_threshold_bytes(tier: PerfGovernorTier) -> u64 {
    let base = crate::constants::GPU_VERY_LARGE_FILE_THRESHOLD_BYTES;
    match tier {
        PerfGovernorTier::Relaxed => scale_threshold_u64(base, 1.25),
        PerfGovernorTier::Balanced => scale_threshold_u64(base, 0.88),
        PerfGovernorTier::Tight => base / 2,
    }
}

#[must_use]
pub fn gpu_long_duration_threshold_secs(tier: PerfGovernorTier) -> f32 {
    let base = crate::constants::VIDEO_DURATION_LONG_SECS;
    match tier {
        PerfGovernorTier::Relaxed => scale_threshold_f32(base, 1.20),
        PerfGovernorTier::Balanced => scale_threshold_f32(base, 0.90),
        PerfGovernorTier::Tight => scale_threshold_f32(base, 0.70),
    }
}

#[must_use]
pub fn gpu_very_long_duration_threshold_secs(tier: PerfGovernorTier) -> f32 {
    let base = crate::constants::VIDEO_DURATION_VERY_LONG_SECS;
    match tier {
        PerfGovernorTier::Relaxed => scale_threshold_f32(base, 1.20),
        PerfGovernorTier::Balanced => scale_threshold_f32(base, 0.90),
        PerfGovernorTier::Tight => scale_threshold_f32(base, 0.70),
    }
}

/// Extra cap on batch parallelism after RAM profile limits (`thread_manager`).
#[must_use]
pub fn apply_delivery_parallel_cap(
    workload: WorkloadType,
    parallel_tasks: usize,
    child_threads: usize,
    tier: PerfGovernorTier,
) -> (usize, usize) {
    let parallel_tasks = parallel_tasks.max(1);
    let child_threads = child_threads.max(1);
    match tier {
        PerfGovernorTier::Relaxed => (parallel_tasks, child_threads),
        PerfGovernorTier::Balanced => match workload {
            WorkloadType::Image => (parallel_tasks, child_threads),
            WorkloadType::Video => (parallel_tasks.min(4), child_threads),
        },
        PerfGovernorTier::Tight => match workload {
            WorkloadType::Image => (parallel_tasks.min(2), child_threads.min(2)),
            WorkloadType::Video => (parallel_tasks.min(1), child_threads.min(2)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common_utils::EnvGuard;

    #[test]
    fn perf_tier_parse_env_aliases() {
        assert_eq!(
            PerfGovernorTier::parse_env("wide"),
            Some(PerfGovernorTier::Relaxed)
        );
        assert_eq!(
            PerfGovernorTier::parse_env("strict"),
            Some(PerfGovernorTier::Tight)
        );
        assert!(PerfGovernorTier::parse_env("bogus").is_none());
    }

    #[test]
    fn tight_reserves_more_headroom_than_relaxed() {
        let relaxed = headroom_reservation(X265MemoryProfile::Default, PerfGovernorTier::Relaxed);
        let tight = headroom_reservation(X265MemoryProfile::Default, PerfGovernorTier::Tight);
        assert!(relaxed.fraction < tight.fraction);
        assert!(relaxed.max_reserved <= tight.max_reserved);
    }

    #[test]
    fn image_parallel_cap_tight_below_relaxed() {
        assert!(
            image_parallel_cap(X265MemoryProfile::Default, PerfGovernorTier::Tight)
                < image_parallel_cap(X265MemoryProfile::Default, PerfGovernorTier::Relaxed)
        );
    }

    #[test]
    fn gpu_concurrency_tight_below_relaxed() {
        assert!(
            gpu_concurrency_cap(PerfGovernorTier::Tight)
                < gpu_concurrency_cap(PerfGovernorTier::Relaxed)
        );
    }

    #[test]
    fn delivery_parallel_cap_tight_reduces_video_batch() {
        let (p, c) =
            apply_delivery_parallel_cap(WorkloadType::Video, 5, 4, PerfGovernorTier::Tight);
        assert_eq!(p, 1);
        assert_eq!(c, 2);
    }

    #[test]
    fn relaxed_gpu_thresholds_exceed_balanced() {
        assert!(
            gpu_large_file_threshold_bytes(PerfGovernorTier::Relaxed)
                > gpu_large_file_threshold_bytes(PerfGovernorTier::Balanced)
        );
    }

    #[test]
    fn clamp_compute_fanout_limits_relaxed_image_storm() {
        let (parallel, child) =
            clamp_compute_fanout(WorkloadType::Image, 16, 4, 10, PerfGovernorTier::Relaxed);
        assert!(parallel.saturating_mul(child) <= 20);
    }

    #[test]
    fn minimum_os_reserve_leaves_headroom_on_wide_cpus() {
        assert!(minimum_os_reserve_cores(16, PerfGovernorTier::Relaxed) >= 4);
    }

    #[serial_test::serial]
    #[test]
    fn env_override_forces_tight_tier() {
        let _guard = EnvGuard::set(crate::constants::ENV_MFB_PERF_TIER, "tight");
        assert_eq!(current_perf_tier(), PerfGovernorTier::Tight);
    }
}
