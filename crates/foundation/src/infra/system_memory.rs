//! System memory detection for intelligent concurrency control.
//!
//! Used by `thread_manager` to reduce `parallel_tasks` and `child_threads` when
//! available memory is low, avoiding OOM kills (e.g. spinner/sleep or encoder
//! processes).

use std::sync::Mutex;
use std::time::{Duration, Instant};

const MEMORY_SAMPLE_MAX_AGE: Duration = Duration::from_millis(250);
static MEMORY_SAMPLE: Mutex<Option<MemorySample<Option<(u64, u64)>>>> = Mutex::new(None);
#[cfg(target_os = "macos")]
static NATIVE_PRESSURE_SAMPLE: Mutex<Option<MemorySample<Option<MemoryPressure>>>> = Mutex::new(None);
#[cfg(target_os = "macos")]
static NATIVE_PRESSURE_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy)]
struct MemorySample<T> {
    sampled_at: Instant,
    memory: T,
}

fn sample_memory<T: Copy>(
    cache: &Mutex<Option<MemorySample<T>>>,
    now: Instant,
    probe: impl FnOnce() -> T,
) -> T {
    let Ok(mut sample) = cache.lock() else {
        return probe();
    };
    if let Some(sample) = *sample
        && now.saturating_duration_since(sample.sampled_at) < MEMORY_SAMPLE_MAX_AGE
    {
        return sample.memory;
    }
    let memory = probe();
    *sample = Some(MemorySample {
        sampled_at: now,
        memory,
    });
    memory
}

/// Scheduling pressure derived from RAM headroom and native macOS pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    /// Plenty of RAM: no cap beyond CPU-based limits.
    Low,
    /// Moderate: slightly reduce parallelism.
    Normal,
    /// Low RAM or native pressure: strongly cap parallelism to avoid OOM.
    High,
}

/// Returns (`available_mb`, `total_mb`) if detection succeeds.
#[must_use]
pub fn get_memory_mb() -> Option<(u64, u64)> {
    // Admission workers share one short-lived probe instead of spawning a
    // sysctl/vm_stat pair per waiting worker on every 100 ms retry.
    sample_memory(&MEMORY_SAMPLE, Instant::now(), probe_memory_mb)
}

fn probe_memory_mb() -> Option<(u64, u64)> {
    #[cfg(target_os = "macos")]
    let (available, total) = get_memory_macos();
    #[cfg(target_os = "linux")]
    let (available, total) = get_memory_linux();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        return None;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Some((available?, total?))
}

/// Available memory in MB. None if detection fails or unsupported platform.
#[must_use]
pub fn get_available_memory_mb() -> Option<u64> {
    get_memory_mb().map(|(avail, _)| avail)
}

/// Total physical memory in MB. None if detection fails.
#[must_use]
pub fn get_total_memory_mb() -> Option<u64> {
    get_memory_mb().map(|(_, total)| total)
}

/// Combine RAM headroom with native pressure; neither can hide the other's warning.
/// Unknown native pressure is retained unless low RAM independently requires tightening.
#[must_use]
pub fn memory_pressure_level() -> Option<MemoryPressure> {
    let ram = ram_pressure_level();
    #[cfg(target_os = "macos")]
    {
        let native = sample_memory(
            &NATIVE_PRESSURE_SAMPLE,
            Instant::now(),
            probe_macos_memory_pressure,
        );
        combine_native_pressure(ram, native)
    }
    #[cfg(not(target_os = "macos"))]
    {
        ram
    }
}

fn ram_pressure_level() -> Option<MemoryPressure> {
    let (available_mb, total_mb) = get_memory_mb()?;
    if total_mb == 0 {
        return None;
    }
    let ratio =
        crate::numeric_cast::u64_to_f64(available_mb) / crate::numeric_cast::u64_to_f64(total_mb);
    // More conservative thresholds to prevent OOM during cjxl/ImageMagick
    // operations
    let level = if ratio >= crate::constants::MEMORY_PRESSURE_LOW_RATIO
        && available_mb >= crate::constants::MEMORY_PRESSURE_LOW_MIN_MB
    {
        MemoryPressure::Low
    } else if ratio >= crate::constants::MEMORY_PRESSURE_NORMAL_RATIO
        || available_mb >= crate::constants::MEMORY_PRESSURE_NORMAL_MIN_MB
    {
        MemoryPressure::Normal
    } else {
        MemoryPressure::High
    };
    Some(level)
}

#[cfg(any(target_os = "macos", test))]
fn combine_native_pressure(
    ram: Option<MemoryPressure>,
    native: Option<MemoryPressure>,
) -> Option<MemoryPressure> {
    match (ram, native) {
        (Some(MemoryPressure::High), _) | (_, Some(MemoryPressure::High)) => {
            Some(MemoryPressure::High)
        }
        (ram, Some(MemoryPressure::Low)) => ram,
        (_, Some(MemoryPressure::Normal)) => Some(MemoryPressure::Normal),
        (_, None) => None,
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_pressure_from_dispatch_flags(flags: u32) -> Option<MemoryPressure> {
    // XNU's sysctl returns dispatch flags (1/2/4), not its internal pressure enum.
    match flags {
        1 => Some(MemoryPressure::Low),
        2 | 4 => Some(MemoryPressure::High),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn probe_macos_memory_pressure() -> Option<MemoryPressure> {
    use std::sync::atomic::Ordering;

    let result = (|| -> std::io::Result<MemoryPressure> {
        let mut flags = 0_u32;
        let mut size = std::mem::size_of_val(&flags);
        let status = unsafe {
            libc::sysctlbyname(
                c"kern.memorystatus_vm_pressure_level".as_ptr(),
                (&raw mut flags).cast(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if size != std::mem::size_of_val(&flags) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "macOS memory pressure returned an unexpected size",
            ));
        }
        macos_pressure_from_dispatch_flags(flags).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("macOS memory pressure returned unknown dispatch flags: {flags}"),
            )
        })
    })();
    match result {
        Ok(pressure) => {
            NATIVE_PRESSURE_FAILED.store(false, Ordering::Relaxed);
            Some(pressure)
        }
        Err(error) => {
            if !NATIVE_PRESSURE_FAILED.swap(true, Ordering::Relaxed) {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "native_memory_pressure",
                    format!(
                        "macOS memory pressure unavailable: {error}; pressure remains unknown \
                         unless low RAM independently requires tightening"
                    ),
                );
            }
            None
        }
    }
}

/// True if user requested low-memory mode via env (e.g. `MFB_LOW_MEMORY=1`).
#[must_use]
pub fn is_low_memory_env() -> bool {
    match std::env::var(crate::constants::ENV_MFB_LOW_MEMORY) {
        Ok(value) => value == "1" || value.eq_ignore_ascii_case("true") || value == "yes",
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "memory_env",
                format!(
                    "failed to read {}: {e}; low-memory env mode disabled",
                    crate::constants::ENV_MFB_LOW_MEMORY
                ),
            );
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn get_memory_macos() -> (Option<u64>, Option<u64>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FAILED: AtomicBool = AtomicBool::new(false);
    match native_macos_memory_mb() {
        Ok((available, total)) => {
            FAILED.store(false, Ordering::Relaxed);
            (Some(available), Some(total))
        }
        Err(error) => {
            if !FAILED.swap(true, Ordering::Relaxed) {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "native_memory",
                    format!("Native macOS RAM observation unavailable: {error}; RAM headroom remains unknown"),
                );
            }
            (None, None)
        }
    }
}

#[cfg(target_os = "macos")]
fn native_macos_memory_mb() -> std::io::Result<(u64, u64)> {
    unsafe extern "C" {
        #[link_name = "vm_kernel_page_size"]
        static KERNEL_PAGE_SIZE: libc::vm_size_t;
        #[link_name = "mach_host_self"]
        fn acquire_host_port() -> libc::host_t;
    }
    // Hold one send right for this process instead of acquiring one per sample.
    static HOST: std::sync::OnceLock<libc::host_t> = std::sync::OnceLock::new();
    let host = *HOST.get_or_init(|| unsafe { acquire_host_port() });
    if host == 0 {
        return Err(std::io::Error::other("Mach host port is unavailable"));
    }
    let mut total = 0_u64;
    let mut size = std::mem::size_of_val(&total);
    if unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut total).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if size != std::mem::size_of_val(&total) || total == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid native physical RAM observation",
        ));
    }
    let mut statistics: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let status = unsafe {
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            (&raw mut statistics).cast(),
            &raw mut count,
        )
    };
    if status != libc::KERN_SUCCESS {
        return Err(std::io::Error::other(format!(
            "Mach VM statistics failed: {status}"
        )));
    }
    let required = u32::try_from(
        (std::mem::offset_of!(libc::vm_statistics64, speculative_count)
            + std::mem::size_of::<libc::natural_t>())
            / std::mem::size_of::<libc::integer_t>(),
    )
    .map_err(std::io::Error::other)?;
    if count < required || count > libc::HOST_VM_INFO64_COUNT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Incomplete native VM statistics",
        ));
    }
    let page_size = u64::try_from(unsafe { KERNEL_PAGE_SIZE }).map_err(std::io::Error::other)?;
    let available = macos_available_mb(
        u64::from(statistics.free_count),
        u64::from(statistics.inactive_count),
        u64::from(statistics.speculative_count),
        page_size,
    )
    .ok_or_else(|| std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "Invalid native available RAM counters",
    ))?;
    if available > total / (1024 * 1024) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Available RAM exceeds physical RAM",
        ));
    }
    Ok((available, total / (1024 * 1024)))
}

#[cfg(any(target_os = "macos", test))]
fn macos_available_mb(free: u64, inactive: u64, speculative: u64, page_size: u64) -> Option<u64> {
    // vm_stat excludes speculative pages from its displayed "Pages free" count.
    if !page_size.is_power_of_two() {
        return None;
    }
    free.checked_sub(speculative)?
        .checked_add(inactive)?
        .checked_mul(page_size)
        .map(|bytes| bytes / (1024 * 1024))
}

#[cfg(test)]
fn parse_vm_stat_available(out: &str) -> Option<u64> {
    let mut page_size = None;
    let mut pages_available = None::<u64>;
    let mut pages_free = None::<u64>;
    let mut pages_inactive = None::<u64>;

    for line in out.lines() {
        let line = line.trim();
        if let Some((_, rest)) = line.split_once("page size of ") {
            let value = rest.split_whitespace().next()?.replace(',', "");
            page_size = match value.parse::<u64>() {
                Ok(size) if size.is_power_of_two() => Some(size),
                Ok(size) => {
                    crate::media_conversion_gate::delivery_runtime_batch_audit(
                        "delivery_system",
                        format!(
                            "SYSTEM AUDIT: Invalid vm_stat page size | Forensic: {size} is not a nonzero power of two"
                        ),
                    );
                    return None;
                }
                Err(error) => {
                    crate::media_conversion_gate::delivery_runtime_batch_audit(
                        "delivery_system",
                        format!(
                            "SYSTEM AUDIT: Failed to parse vm_stat page size | Forensic: Input '{value}', Error '{error}'"
                        ),
                    );
                    return None;
                }
            };
        } else if line.starts_with("Pages available:") {
            pages_available = parse_vm_stat_value(line);
        } else if line.starts_with("Pages free:") {
            pages_free = parse_vm_stat_value(line);
        } else if line.starts_with("Pages inactive:") {
            pages_inactive = parse_vm_stat_value(line);
        }
    }

    let mut pages = pages_available;
    if pages.is_none() {
        pages = pages_free.and_then(|f| pages_inactive.and_then(|i| f.checked_add(i)));
        if pages_free.is_some() && pages_inactive.is_some() && pages.is_none() {
            return None;
        }
    }
    if pages.is_none() {
        pages = pages_free;
    }
    let pages = pages?;
    Some(pages.checked_mul(page_size?)? / (1024 * 1024))
}

#[cfg(test)]
fn parse_vm_stat_value(line: &str) -> Option<u64> {
    let val_str = line.split(':').nth(1)?.trim().replace('.', "");
    match val_str.parse::<u64>() {
        Ok(v) => Some(v),
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_system",
                format!(
                    "SYSTEM AUDIT: Failed to parse vm_stat value | Forensic: Input '{val_str}', \
                     Error '{e}'"
                ),
            );
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn get_memory_linux() -> (Option<u64>, Option<u64>) {
    let content = match std::fs::read_to_string("/proc/meminfo") {
        Ok(c) => c,
        Err(err) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_system",
                format!("SYSTEM AUDIT: Failed to read /proc/meminfo | Forensic: Error '{err}'"),
            );
            return (None, None);
        }
    };
    let mut mem_available = None::<u64>;
    let mut mem_total = None::<u64>;
    for line in content.lines() {
        if line.starts_with("MemAvailable:") {
            mem_available = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| match s.parse::<u64>() {
                    Ok(v) => Some(v),
                    Err(e) => {
                        crate::media_conversion_gate::delivery_runtime_batch_audit(
                            "delivery_system",
                            format!(
                                "SYSTEM AUDIT: Failed to parse MemAvailable from /proc/meminfo | \
                                 Forensic: Input '{s}', Error '{e}'"
                            ),
                        );
                        None
                    }
                })
                .map(|kb| kb / 1024);
        } else if line.starts_with("MemTotal:") {
            mem_total = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| match s.parse::<u64>() {
                    Ok(v) => Some(v),
                    Err(e) => {
                        crate::media_conversion_gate::delivery_runtime_batch_audit(
                            "delivery_system",
                            format!(
                                "SYSTEM AUDIT: Failed to parse MemTotal from /proc/meminfo | \
                                 Forensic: Input '{s}', Error '{e}'"
                            ),
                        );
                        None
                    }
                })
                .map(|kb| kb / 1024);
        }
    }
    if mem_available.is_none() || mem_total.is_none() {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_system",
            format!(
                "SYSTEM AUDIT: Missing expected memory fields in /proc/meminfo | Forensic: \
                 has_mem_available={}, has_mem_total={}",
                mem_available.is_some(),
                mem_total.is_some()
            ),
        );
    }
    if mem_available.is_none() {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_system",
            "SYSTEM AUDIT: 'MemAvailable' missing from /proc/meminfo | Forensic: memory-based \
             optimizations will be disabled",
        );
    }
    if mem_total.is_none() {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_system",
            "SYSTEM AUDIT: 'MemTotal' missing from /proc/meminfo | Forensic: memory statistics \
             unavailable",
        );
    }
    let available = mem_available;
    let total = mem_total;
    (available, total)
}

/// Returns available bytes on the filesystem containing `path`. None if
/// detection fails.
#[must_use]
pub fn get_available_disk_bytes(path: &std::path::Path) -> Option<u64> {
    // Resolve to an existing ancestor (the path itself may not exist yet, e.g.
    // output dir).
    let existing = {
        let mut p = path;
        loop {
            if p.exists() {
                break p.to_path_buf();
            }
            if let Some(parent) = p.parent() {
                p = parent;
            } else {
                crate::media_conversion_gate::delivery_runtime_path_audit(
                    "delivery_system",
                    path,
                    format!(
                        "SYSTEM AUDIT: No existing ancestor found for disk-space probe | \
                         Forensic: Path '{}'",
                        path.display()
                    ),
                );
                return None;
            }
        }
    };

    #[cfg(unix)]
    {
        use std::ffi::CString;
        let c_path = match CString::new(existing.to_string_lossy().as_bytes()) {
            Ok(c_path) => c_path,
            Err(err) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "delivery_system",
                    format!(
                        "SYSTEM AUDIT: Failed to prepare path for statvfs | Forensic: Path '{}', \
                         Error '{}'",
                        existing.display(),
                        err
                    ),
                );
                return None;
            }
        };
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::statvfs(c_path.as_ptr(), &raw mut stat) };
        if ret == 0 {
            // f_bavail / f_frsize field widths differ by OS; widen via u128 before
            // narrowing to u64.
            let blocks = u128::from(stat.f_bavail);
            let frsize = u128::from(stat.f_frsize);
            let avail_u128 = blocks.saturating_mul(frsize);
            let avail =
                crate::media_conversion_gate::delivery_system_avail_bytes_from_u128(avail_u128);
            return Some(avail);
        }
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_system",
            format!(
                "SYSTEM AUDIT: statvfs failed during disk-space probe | Forensic: Path '{}', \
                 Errno '{}'",
                existing.display(),
                std::io::Error::last_os_error()
            ),
        );
        None
    }

    #[cfg(not(unix))]
    {
        let _ = existing;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_vm_counters_keep_the_existing_vm_stat_available_definition() {
        for page_size in [4096, 16384] {
            let text = format!(
                "page size of {page_size} bytes\nPages free: 34559.\nPages inactive: 208154.\nPages speculative: 4096.\n"
            );
            assert_eq!(
                macos_available_mb(34559 + 4096, 208154, 4096, page_size),
                parse_vm_stat_available(&text),
            );
        }
        assert_eq!(macos_available_mb(1, 0, 2, 16384), None);
        assert_eq!(macos_available_mb(u64::MAX, 1, 0, 16384), None);
        assert_eq!(macos_available_mb(u64::MAX, 0, 0, 16384), None);
        assert_eq!(macos_available_mb(1, 0, 0, 0), None);
        assert_eq!(macos_available_mb(1, 0, 0, 3), None);
    }

    #[test]
    fn native_pressure_cannot_be_hidden_by_reclaimable_ram() {
        assert_eq!(
            macos_pressure_from_dispatch_flags(1),
            Some(MemoryPressure::Low)
        );
        for flags in [2, 4] {
            let native = macos_pressure_from_dispatch_flags(flags);
            assert_eq!(native, Some(MemoryPressure::High));
            for ram in [None, Some(MemoryPressure::Low), Some(MemoryPressure::Normal)] {
                assert_eq!(
                    combine_native_pressure(ram, native),
                    Some(MemoryPressure::High)
                );
            }
        }
        for flags in [0, 3, 8, u32::MAX] {
            assert_eq!(macos_pressure_from_dispatch_flags(flags), None);
        }
        assert_eq!(combine_native_pressure(Some(MemoryPressure::Low), None), None);
        assert_eq!(
            combine_native_pressure(Some(MemoryPressure::High), None),
            Some(MemoryPressure::High)
        );
        assert_eq!(combine_native_pressure(None, Some(MemoryPressure::Low)), None);
        assert_eq!(
            combine_native_pressure(Some(MemoryPressure::Low), Some(MemoryPressure::Low)),
            Some(MemoryPressure::Low)
        );
    }

    #[test]
    fn vm_stat_uses_real_macos_header_page_size() {
        let pages = "Pages free: 34559.\nPages inactive: 208154.\n";
        let arm = format!("Mach Virtual Memory Statistics: (page size of 16384 bytes)\n{pages}");
        let intel = format!("Mach Virtual Memory Statistics: (page size of 4096 bytes)\n{pages}");
        assert_eq!(parse_vm_stat_available(&arm), Some(3792));
        assert_eq!(parse_vm_stat_available(&intel), Some(948));
        assert_eq!(parse_vm_stat_available(pages), None);
        for invalid in ["invalid", "0", "3", "18446744073709551616"] {
            assert_eq!(
                parse_vm_stat_available(&arm.replace("16384", invalid)),
                None
            );
        }
    }

    #[test]
    fn vm_stat_explicit_available_and_overflow_are_handled() {
        assert_eq!(
            parse_vm_stat_available(
                "page size of 16384 bytes\nPages available: 128.\nPages free: 1.\n"
            ),
            Some(2)
        );
        assert_eq!(
            parse_vm_stat_available(&format!(
                "page size of 16384 bytes\nPages free: {}.\nPages inactive: 1.\n",
                u64::MAX
            )),
            None
        );
        assert_eq!(
            parse_vm_stat_available(&format!(
                "page size of 16384 bytes\nPages available: {}.\n",
                u64::MAX
            )),
            None
        );
    }

    #[test]
    fn memory_probe_is_shared_but_expires_and_does_not_hide_failure() {
        let cache = Mutex::new(None);
        let now = Instant::now();
        assert_eq!(
            sample_memory(&cache, now, || Some((4000, 16000))),
            Some((4000, 16000))
        );
        assert_eq!(
            sample_memory(&cache, now + Duration::from_millis(249), || panic!(
                "cached probe repeated"
            )),
            Some((4000, 16000))
        );
        assert_eq!(
            sample_memory(&cache, now + MEMORY_SAMPLE_MAX_AGE, || None),
            None
        );
        assert_eq!(
            sample_memory(&cache, now + MEMORY_SAMPLE_MAX_AGE * 2, || Some((
                500, 16000
            ))),
            Some((500, 16000))
        );
    }

    #[test]
    fn test_memory_detection_does_not_panic() {
        let _ = get_memory_mb();
        let _ = memory_pressure_level();
    }
}
