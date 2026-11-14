//! Performance tier detection.
//! Mirrors `mfb_performance.py` functions.

use crate::infra::hardening::{optional_env, parse_kb_token, positive_f64_env, positive_usize_env};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PerfTier {
    #[default]
    Low,
    Normal,
    High,
}

fn truthy_env(name: &str) -> bool {
    match optional_env(name) {
        Some(raw) => matches!(raw.to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        None => false,
    }
}

pub fn detect_perf_tier() -> PerfTier {
    let workers = positive_usize_env("MFB_PERF_WORKERS_LIMIT", 0);
    if workers > 0 && workers < 8 {
        PerfTier::Low
    } else if workers >= 8 {
        PerfTier::High
    } else {
        PerfTier::Normal
    }
}

pub fn memory_mb() -> (Option<usize>, Option<usize>) {
    match std::fs::read_to_string("/proc/meminfo") {
        Ok(content) => {
            let mut total_kb: usize = 0;
            let mut available_kb: usize = 0;
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    total_kb = match parse_kb_token(line) {
                        Some(v) => v as usize,
                        None => 0,
                    };
                } else if line.starts_with("MemAvailable:") {
                    available_kb = match parse_kb_token(line) {
                        Some(v) => v as usize,
                        None => 0,
                    };
                }
            }
            (Some(total_kb / 1024), Some(available_kb / 1024))
        }
        Err(err) => {
            eprintln!("[PERF] /proc/meminfo read failed: {err}");
            (None, None)
        }
    }
}

pub fn preemptive_tight(available_mb: usize, total_mb: usize) -> bool {
    available_mb < total_mb / 4
}

pub fn clamp_tier_for_stability(tier: PerfTier) -> PerfTier {
    if truthy_env("MFB_PERF_TIER_LOW") {
        PerfTier::Low
    } else {
        tier
    }
}

#[must_use]
pub fn positive_float_env(name: &str, default: f64) -> f64 {
    positive_f64_env(name, default)
}
