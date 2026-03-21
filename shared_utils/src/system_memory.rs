//! System memory detection for intelligent concurrency control.
//!
//! Used by thread_manager to reduce parallel_tasks and child_threads when
//! available memory is low, avoiding OOM kills (e.g. spinner/sleep or encoder processes).

use std::process::Command;
use tracing::warn;

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

/// Returns (available_mb, total_mb) if detection succeeds.
pub fn get_memory_mb() -> Option<(u64, u64)> {
    let (available, total) = if cfg!(target_os = "macos") {
        get_memory_macos()
    } else if cfg!(target_os = "linux") {
        get_memory_linux()
    } else {
        return None;
    };
    Some((available, total))
}

/// Available memory in MB. None if detection fails or unsupported platform.
pub fn get_available_memory_mb() -> Option<u64> {
    get_memory_mb().map(|(avail, _)| avail)
}

/// Total physical memory in MB. None if detection fails.
pub fn get_total_memory_mb() -> Option<u64> {
    get_memory_mb().map(|(_, total)| total)
}

/// Classify current memory pressure from available/total. None if unknown.
pub fn memory_pressure_level() -> Option<MemoryPressure> {
    let (available_mb, total_mb) = get_memory_mb()?;
    if total_mb == 0 {
        return None;
    }
    let ratio = available_mb as f64 / total_mb as f64;
    let level = if ratio >= 0.25 && available_mb >= 2048 {
        MemoryPressure::Low
    } else if ratio >= 0.10 || available_mb >= 1024 {
        MemoryPressure::Normal
    } else {
        MemoryPressure::High
    };
    Some(level)
}

/// True if user requested low-memory mode via env (e.g. MFB_LOW_MEMORY=1).
pub fn is_low_memory_env() -> bool {
    std::env::var("MFB_LOW_MEMORY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v == "yes")
        .unwrap_or(false)
}

fn get_memory_macos() -> (u64, u64) {
    let total = match Command::new("sysctl").arg("-n").arg("hw.memsize").output() {
        Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
            Ok(stdout) => match stdout.trim().parse::<u64>() {
                Ok(bytes) => bytes / (1024 * 1024),
                Err(err) => {
                    warn!(error = %err, "Failed to parse macOS total memory from sysctl");
                    0
                }
            },
            Err(err) => {
                warn!(error = %err, "sysctl returned non-UTF-8 total memory output");
                0
            }
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(stderr = %stderr.trim(), "sysctl hw.memsize returned non-zero status");
            0
        }
        Err(err) => {
            warn!(error = %err, "Failed to execute sysctl hw.memsize");
            0
        }
    };

    let available = match Command::new("vm_stat").output() {
        Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
            Ok(stdout) => match parse_vm_stat_available(&stdout) {
                Some(available) => available,
                None => {
                    warn!("Failed to parse macOS available memory from vm_stat");
                    0
                }
            },
            Err(err) => {
                warn!(error = %err, "vm_stat returned non-UTF-8 output");
                0
            }
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(stderr = %stderr.trim(), "vm_stat returned non-zero status");
            0
        }
        Err(err) => {
            warn!(error = %err, "Failed to execute vm_stat");
            0
        }
    };

    (available, total)
}

fn parse_vm_stat_available(out: &str) -> Option<u64> {
    let mut page_size = 4096u64;
    let mut pages_available = None::<u64>;
    let mut pages_free = None::<u64>;
    let mut pages_inactive = None::<u64>;

    for line in out.lines() {
        let line = line.trim();
        if line.starts_with("page size of ") {
            if let Some(rest) = line.strip_prefix("page size of ").and_then(|s| s.strip_suffix(" bytes")) {
                if let Ok(n) = rest.replace(',', "").parse::<u64>() {
                    page_size = n;
                }
            }
        } else if line.starts_with("Pages available:") {
            pages_available = parse_vm_stat_value(line);
        } else if line.starts_with("Pages free:") {
            pages_free = parse_vm_stat_value(line);
        } else if line.starts_with("Pages inactive:") {
            pages_inactive = parse_vm_stat_value(line);
        }
    }

    let pages = pages_available
        .or_else(|| pages_free.and_then(|f| pages_inactive.map(|i| f + i)))
        .or(pages_free)?;
    Some((pages * page_size) / (1024 * 1024))
}

fn parse_vm_stat_value(line: &str) -> Option<u64> {
    line.split(':').nth(1)?.trim().replace('.', "").parse().ok()
}

fn get_memory_linux() -> (u64, u64) {
    let content = match std::fs::read_to_string("/proc/meminfo") {
        Ok(c) => c,
        Err(err) => {
            warn!(error = %err, "Failed to read /proc/meminfo");
            return (0, 0);
        }
    };
    let mut mem_available = None::<u64>;
    let mut mem_total = None::<u64>;
    for line in content.lines() {
        if line.starts_with("MemAvailable:") {
            mem_available = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
                .map(|kb| kb / 1024);
        } else if line.starts_with("MemTotal:") {
            mem_total = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
                .map(|kb| kb / 1024);
        }
    }
    if mem_available.is_none() || mem_total.is_none() {
        warn!(
            has_mem_available = mem_available.is_some(),
            has_mem_total = mem_total.is_some(),
            "Missing expected memory fields in /proc/meminfo"
        );
    }
    let available = mem_available.unwrap_or(0);
    let total = mem_total.unwrap_or(0);
    (available, total)
}

/// Returns available bytes on the filesystem containing `path`. None if detection fails.
pub fn get_available_disk_bytes(path: &std::path::Path) -> Option<u64> {
    // Resolve to an existing ancestor (the path itself may not exist yet, e.g. output dir).
    let existing = {
        let mut p = path;
        loop {
            if p.exists() {
                break p.to_path_buf();
            }
            match p.parent() {
                Some(parent) => p = parent,
                None => {
                    warn!(path = %path.display(), "No existing ancestor found for disk-space probe");
                    return None;
                }
            }
        }
    };

    #[cfg(unix)]
    {
        use std::ffi::CString;
        let c_path = match CString::new(existing.to_string_lossy().as_bytes()) {
            Ok(c_path) => c_path,
            Err(err) => {
                warn!(path = %existing.display(), error = %err, "Failed to prepare path for statvfs");
                return None;
            }
        };
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
        if ret == 0 {
            // f_bavail: blocks available to unprivileged processes; f_frsize: fundamental block size
            let avail = stat.f_bavail as u64 * stat.f_frsize as u64;
            return Some(avail);
        }
        warn!(path = %existing.display(), errno = std::io::Error::last_os_error().to_string(), "statvfs failed during disk-space probe");
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
    fn test_memory_detection_does_not_panic() {
        let _ = get_memory_mb();
        let _ = memory_pressure_level();
    }
}
