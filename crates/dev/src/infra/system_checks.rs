//! System resource checks (disk, memory, CPU).
//! Mirrors `check_system_resources()` from drag_and_drop_processor.py.

#![allow(dead_code)]

#[cfg(target_os = "linux")]
use crate::infra::hardening::parse_kb_token;
use anyhow::{Result, anyhow, bail};
use std::path::Path;
use std::thread;
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn get_disk_free_space(path: &Path) -> Result<u64> {
    let path_str = path.to_string_lossy();
    let path_cstr =
        std::ffi::CString::new(path_str.as_bytes()).map_err(|e| anyhow!("path to CString: {e}"))?;

    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        let ret = libc::statvfs(path_cstr.as_ptr(), &mut stat);
        if ret != 0 {
            bail!("statvfs failed");
        }
        Ok((stat.f_bavail as u64) * (stat.f_bsize as u64))
    }
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
