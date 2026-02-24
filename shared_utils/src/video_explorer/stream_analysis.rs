//! Stream Analysis Module - 视频流分析模块
//!
//! 本模块负责视频流的分析和质量评估，包括：
//! - SSIM (Structural Similarity Index) 计算
//! - PSNR (Peak Signal-to-Noise Ratio) 计算
//! - MS-SSIM (Multi-Scale SSIM) 计算
//! - 视频时长检测
//! - 质量阈值验证

use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

pub const LONG_VIDEO_THRESHOLD: f32 = 300.0;

#[derive(Debug, Clone)]
pub struct QualityThresholds {
    pub min_ssim: f64,
    pub min_psnr: f64,
    pub min_ms_ssim: f64,
    pub validate_ssim: bool,
    pub validate_psnr: bool,
    pub validate_ms_ssim: bool,
    pub force_ms_ssim_long: bool,
}

impl Default for QualityThresholds {
    fn default() -> Self {
        Self {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_ms_ssim: 0.90,
            validate_ssim: true,
            validate_psnr: false,
            validate_ms_ssim: false,
            force_ms_ssim_long: false,
        }
    }
}

pub fn get_video_duration(input: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(["-show_entries", "format=duration"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg("--")
        .arg(crate::safe_path_arg(input).as_ref())
        .output()
        .ok()?;

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
}

pub fn calculate_ssim_enhanced(input: &Path, output: &Path) -> Option<f64> {
    let filters: &[(&str, &str)] = &[
        ("standard", "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim"),
        ("format_convert", "[0:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[ref];[1:v]format=yuv420p[cmp];[ref][cmp]ssim"),
        ("simple", "ssim"),
    ];

    for (name, filter) in filters {
        let result = Command::new("ffmpeg")
            .arg("-i")
            .arg(crate::safe_path_arg(input).as_ref())
            .arg("-i")
            .arg(crate::safe_path_arg(output).as_ref())
            .arg("-lavfi")
            .arg(*filter)
            .arg("-f")
            .arg("null")
            .arg("-")
            .output();

        match result {
            Ok(out) if out.status.success() => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if let Some(ssim) = parse_ssim_from_output(&stderr) {
                    if is_valid_ssim_value(ssim) {
                        info!(method = %name, ssim = %ssim, "SSIM calculated");
                        return Some(ssim);
                    }
                }
            }
            Ok(_) => {
                warn!(method = %name, "SSIM method failed, trying next");
            }
            Err(e) => {
                warn!(method = %name, error = %e, "ffmpeg failed");
            }
        }
    }

    tracing::error!("ALL SSIM CALCULATION METHODS FAILED");
    None
}

/// Run ffmpeg with the given lavfi filter and parse SSIM Y/U/V/All from stderr.
fn run_ssim_all_filter(input: &Path, output: &Path, lavfi: &str) -> Option<(f64, f64, f64, f64)> {
    let out = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-i")
        .arg(crate::safe_path_arg(output).as_ref())
        .arg("-lavfi")
        .arg(lavfi)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines() {
        if line.contains("SSIM Y:") && line.contains("All:") {
            let y = extract_ssim_value(line, "Y:");
            let u = extract_ssim_value(line, "U:");
            let v = extract_ssim_value(line, "V:");
            let all = extract_ssim_value(line, "All:");
            if let (Some(y), Some(u), Some(v), Some(all)) = (y, u, v, all) {
                if is_valid_ssim_value(y) && is_valid_ssim_value(all) {
                    return Some((y, u, v, all));
                }
            }
        }
    }
    None
}

/// SSIM Y/U/V/All between input and output. Tries in order:
/// 1. Direct ssim (when formats already match).
/// 2. Format normalization (GIF palette / odd-size → yuv420p even).
/// 3. Alpha flatten: composite input on black (same as encoder) then compare,
///    so transparent GIF/WebP/PNG matches HEVC output that has no alpha.
pub fn calculate_ssim_all(input: &Path, output: &Path) -> Option<(f64, f64, f64, f64)> {
    const DIRECT: &str = "[0:v][1:v]ssim";
    const FORMAT_NORM: &str = "[0:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[ref];[1:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[cmp];[ref][cmp]ssim";
    // Match encoder: format=rgba, premultiply (composite on black), then yuv420p.
    const ALPHA_FLATTEN: &str = "[0:v]format=rgba,premultiply=inplace=1,format=rgb24,format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[ref];[1:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[cmp];[ref][cmp]ssim";

    run_ssim_all_filter(input, output, DIRECT)
        .or_else(|| run_ssim_all_filter(input, output, FORMAT_NORM))
        .or_else(|| run_ssim_all_filter(input, output, ALPHA_FLATTEN))
}

fn parse_ssim_from_output(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        if line.contains("SSIM") && line.contains("All:") {
            if let Some(all_pos) = line.find("All:") {
                let after_all = &line[all_pos + 4..];
                let after_all = after_all.trim_start();
                let end = after_all
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(after_all.len());
                if end > 0 {
                    return after_all[..end].parse::<f64>().ok();
                }
            }
        }
    }
    None
}

fn extract_ssim_value(line: &str, prefix: &str) -> Option<f64> {
    if let Some(pos) = line.find(prefix) {
        let after = &line[pos + prefix.len()..];
        let end = after
            .find(|c: char| !c.is_numeric() && c != '.')
            .unwrap_or(after.len());
        if end > 0 {
            return after[..end].parse::<f64>().ok();
        }
    }
    None
}

#[inline]
fn is_valid_ssim_value(ssim: f64) -> bool {
    (0.0..=1.0).contains(&ssim) && !ssim.is_nan()
}
