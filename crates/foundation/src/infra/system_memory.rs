//! System memory detection for intelligent concurrency control.
//!
//! Used by `thread_manager` to reduce `parallel_tasks` and `child_threads` when
//! available memory is low, avoiding OOM kills (e.g. spinner/sleep or encoder
//! processes).

use crate::builder_base::ToolBuilder;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MEMORY_SAMPLE_MAX_AGE: Duration = Duration::from_millis(250);
static MEMORY_SAMPLE: Mutex<Option<MemorySample>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct MemorySample {
    sampled_at: Instant,
    memory: Option<(u64, u64)>,
}

fn sample_memory(
    cache: &Mutex<Option<MemorySample>>,
    now: Instant,
    probe: impl FnOnce() -> Option<(u64, u64)>,
) -> Option<(u64, u64)> {
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

/// Memory pressure level derived from available vs total RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    /// Plenty of RAM: no cap beyond CPU-based limits.
    Low,
    /// Moderate: slightly reduce parallelism.
    Normal,
    /// Low available: strongly cap parallelism to avoid OOM.
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
    let (available, total) = if cfg!(target_os = "macos") {
        get_memory_macos()
    } else if cfg!(target_os = "linux") {
        get_memory_linux()
    } else {
        return None;
    };
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

/// Classify current memory pressure from available/total. None if unknown.
/// Enhanced thresholds to prevent OOM kills during heavy image processing.
#[must_use]
pub fn memory_pressure_level() -> Option<MemoryPressure> {
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

fn get_memory_macos() -> (Option<u64>, Option<u64>) {
    let total = match crate::tool_builders::SysctlBuilder::new()
        .arg("-n")
        .arg("hw.memsize")
        .build()
        .output()
    {
        Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
            Ok(stdout) => match stdout.trim().parse::<u64>() {
                Ok(bytes) => Some(bytes / (1024 * 1024)),
                Err(err) => {
                    crate::media_conversion_gate::delivery_runtime_batch_audit(
                        "delivery_system",
                        format!(
                            "SYSTEM AUDIT: Failed to parse macOS total memory from sysctl | \
                             Forensic: Error '{err}'"
                        ),
                    );
                    None
                }
            },
            Err(err) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "delivery_system",
                    format!(
                        "SYSTEM AUDIT: sysctl returned non-UTF-8 total memory output | Forensic: \
                         Error '{err}'"
                    ),
                );
                None
            }
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_progress",
                format!(
                    "SYSTEM AUDIT: sysctl hw.memsize returned non-zero status | Forensic: Stderr \
                     '{}'",
                    stderr.trim()
                ),
            );
            None
        }
        Err(err) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_system",
                format!(
                    "SYSTEM AUDIT: Failed to execute sysctl hw.memsize | Forensic: Error '{err}'"
                ),
            );
            None
        }
    };

    let available = match crate::tool_builders::VmstatBuilder::new().build().output() {
        Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
            Ok(stdout) => {
                let parsed = parse_vm_stat_available(&stdout);
                if let Some(v) = parsed {
                    Some(v)
                } else {
                    crate::media_conversion_gate::delivery_runtime_batch_audit(
                        "delivery_system",
                        "SYSTEM AUDIT: Failed to parse macOS available memory from vm_stat | \
                         Forensic: Output structure unrecognized; memory-based optimizations will \
                         be disabled",
                    );
                    None
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "delivery_system",
                    format!(
                        "SYSTEM AUDIT: vm_stat returned non-UTF-8 output | Forensic: Error '{err}'"
                    ),
                );
                None
            }
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_progress",
                format!(
                    "SYSTEM AUDIT: vm_stat returned non-zero status | Forensic: Stderr '{}'",
                    stderr.trim()
                ),
            );
            None
        }
        Err(err) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_system",
                format!("SYSTEM AUDIT: Failed to execute vm_stat | Forensic: Error '{err}'"),
            );
            None
        }
    };

    (available, total)
}

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
