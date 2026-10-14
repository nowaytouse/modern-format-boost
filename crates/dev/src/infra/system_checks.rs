//! System resource checks (disk, memory, CPU).
//! Mirrors `check_system_resources()` from drag_and_drop_processor.py.

#![allow(dead_code)]

#[cfg(target_os = "linux")]
use crate::infra::hardening::parse_kb_token;
#[cfg(unix)]
use anyhow::anyhow;
use anyhow::{Result, bail};
use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::thread;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Duration;

#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::Read;

#[derive(Debug, Clone)]
pub struct SystemResources {
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
    pub memory_percent: f64,
    pub cpu_percent: f64,
}

impl Default for SystemResources {
    fn default() -> Self {
        Self {
            disk_free_bytes: 0,
            disk_total_bytes: 1,
            memory_percent: 0.0,
            cpu_percent: 0.0,
        }
    }
}

pub fn check_system_resources(check_dir: &Path, required_bytes: u64) -> Result<SystemResources> {
    let mut resources = SystemResources::default();

    // Disk check via statvfs
    let free = get_disk_free_space(check_dir)?;
    resources.disk_free_bytes = free;

    let free_gb = free as f64 / (1024.0 * 1024.0 * 1024.0);
    let required_gb = (required_bytes as f64 / (1024.0 * 1024.0 * 1024.0)) + 1.0;

    if free < required_bytes + 1024 * 1024 * 1024 {
        bail!(
            "Insufficient disk space on {}: available {:.2} GB, required {:.2} GB",
            check_dir.display(),
            free_gb,
            required_gb
        );
    }

    // Memory/CPU via sysinfo (macOS + Linux)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        refresh_sysinfo_snapshot(&mut resources);
    }

    Ok(resources)
}

/// Non-failing resource snapshot for Rich runtime dashboard.
#[must_use]
pub fn probe_system_snapshot(check_dir: &Path) -> SystemResources {
    let mut resources = SystemResources::default();
    match get_disk_free_space(check_dir) {
        Ok(free) => {
            resources.disk_free_bytes = free;
            resources.disk_total_bytes = free.saturating_mul(4);
        }
        Err(err) => eprintln!(
            "[SYSTEM] disk probe failed ({}): {err}",
            check_dir.display()
        ),
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    refresh_sysinfo_snapshot(&mut resources);
    resources
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn refresh_sysinfo_snapshot(resources: &mut SystemResources) {
    use sysinfo::System;

    let mut system = System::new();
    thread::sleep(Duration::from_millis(120));
    system.refresh_all();
    let total = system.total_memory().max(1);
    resources.memory_percent = (system.used_memory() as f64 / total as f64) * 100.0;
    resources.cpu_percent = system.global_cpu_usage() as f64;
}

/// Read currently available bytes on the filesystem containing an existing path.
/// This is a snapshot, not a capacity reservation or an estimate of required space.
///
/// # Errors
/// Rejects invalid paths, failed filesystem probes, overflow, and unsupported platforms.
#[cfg(unix)]
pub fn get_disk_free_space(path: &Path) -> Result<u64> {
    let path_cstr = disk_probe_path(path)?;

    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        let ret = libc::statvfs(path_cstr.as_ptr(), &mut stat);
        if ret != 0 {
            let error = std::io::Error::last_os_error();
            return Err(
                anyhow!(error).context(format!("read available disk bytes for {}", path.display()))
            );
        }
        statvfs_available_bytes(&stat)
    }
}

/// # Errors
/// Disk capacity snapshots are not supported on this platform.
#[cfg(not(unix))]
pub fn get_disk_free_space(_path: &Path) -> Result<u64> {
    bail!("available disk-byte snapshots are unsupported on this platform")
}

#[cfg(unix)]
fn disk_probe_path(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|error| anyhow!("invalid disk probe path: {error}"))
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // libc field widths vary across supported platforms.
fn statvfs_available_bytes(stat: &libc::statvfs) -> Result<u64> {
    anyhow::ensure!(
        stat.f_frsize > 0,
        "statvfs returned an invalid zero fragment size"
    );
    available_byte_product(stat.f_bavail as u64, stat.f_frsize as u64)
}

#[cfg(unix)]
fn available_byte_product(blocks: u64, fragment_bytes: u64) -> Result<u64> {
    blocks
        .checked_mul(fragment_bytes)
        .ok_or_else(|| anyhow!("statvfs available-byte count overflow"))
}

#[cfg(target_os = "linux")]
fn get_memory_percent_cached() -> Result<f64> {
    let mut meminfo = String::new();
    File::open("/proc/meminfo")?.read_to_string(&mut meminfo)?;
    let mut total_kb: u64 = 0;
    let mut available_kb: u64 = 0;

    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            total_kb = parse_kb_token(line).unwrap_or_else(|| {
                eprintln!("[SYSTEM] MemTotal parse failed in {line}");
                1
            });
        } else if line.starts_with("MemAvailable:") {
            available_kb = match parse_kb_token(line) {
                Some(v) => v,
                None => {
                    eprintln!("[SYSTEM] MemAvailable parse failed in {line}");
                    0
                }
            };
        }
    }

    if total_kb > 0 && available_kb > 0 {
        Ok(((total_kb - available_kb) as f64 / total_kb as f64) * 100.0)
    } else {
        Ok(0.0)
    }
}

#[cfg(target_os = "macos")]
fn get_memory_percent_cached() -> Result<f64> {
    Ok(0.0)
}

#[cfg(target_os = "linux")]
fn get_cpu_percent_cached() -> Result<f64> {
    let mut stat = String::new();
    File::open("/proc/stat")?.read_to_string(&mut stat)?;
    if let Some(line) = stat.lines().next() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 {
            let user = parse_cpu_field(parts[1], "user")?;
            let nice = parse_cpu_field(parts[2], "nice")?;
            let system = parse_cpu_field(parts[3], "system")?;
            let idle = parse_cpu_field(parts[4], "idle")?;
            let total = user + nice + system + idle;
            if total > 0 {
                return Ok(((user + nice + system) as f64 / total as f64) * 100.0);
            }
        }
    }
    Ok(0.0)
}

fn parse_cpu_field(raw: &str, label: &str) -> Result<u64> {
    match raw.parse::<u64>() {
        Ok(v) => Ok(v),
        Err(err) => {
            eprintln!("[SYSTEM] /proc/stat {label} parse failed for {raw:?}: {err}");
            Ok(0)
        }
    }
}

#[cfg(target_os = "macos")]
fn get_cpu_percent_cached() -> Result<f64> {
    Ok(0.0)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        available_byte_product, disk_probe_path, get_disk_free_space, statvfs_available_bytes,
    };
    use std::path::Path;

    #[test]
    fn disk_free_space_uses_statvfs_fragment_size() {
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        stat.f_bavail = 3;
        stat.f_frsize = 1024;
        stat.f_bsize = 4096;
        assert_eq!(statvfs_available_bytes(&stat).unwrap(), 3072);
        stat.f_bavail = 0;
        assert_eq!(statvfs_available_bytes(&stat).unwrap(), 0);
        stat.f_frsize = 0;
        assert!(statvfs_available_bytes(&stat).is_err());
    }

    #[test]
    fn disk_free_space_rejects_available_byte_overflow() {
        assert!(available_byte_product(u64::MAX, 2).is_err());
        assert_eq!(available_byte_product(u64::MAX, 1).unwrap(), u64::MAX);
    }

    #[test]
    fn disk_free_space_keeps_native_path_bytes_and_rejects_nul() {
        use std::os::unix::ffi::OsStringExt;

        let bytes = vec![b's', 0xff, b'\n'];
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes.clone()));
        assert_eq!(disk_probe_path(&path).unwrap().as_bytes(), bytes);
        assert!(get_disk_free_space(Path::new("synthetic\0path")).is_err());
    }

    #[test]
    fn disk_free_space_reports_missing_paths_without_creating_them() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("not-created");
        let error = get_disk_free_space(&missing).unwrap_err();
        assert!(error.downcast_ref::<std::io::Error>().is_some());
        assert!(!missing.exists());
    }
}

#[cfg(all(test, not(unix)))]
mod tests {
    #[test]
    fn disk_free_space_is_explicitly_unsupported() {
        assert!(super::get_disk_free_space(std::path::Path::new("synthetic")).is_err());
    }
}
