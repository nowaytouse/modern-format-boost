//! 🔥 Standalone VMAF Tool Integration
//! 使用独立的 vmaf 命令行工具，绕过 ffmpeg libvmaf 依赖

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

pub fn is_vmaf_available() -> bool {
    Command::new("vmaf")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn calculate_ms_ssim_standalone(reference: &Path, distorted: &Path) -> Result<f64> {
    let ref_y4m_file = tempfile::Builder::new()
        .prefix("vmaf_ref_")
        .suffix(".y4m")
        .tempfile()
        .context("Failed to create ref temp file")?;
    let dist_y4m_file = tempfile::Builder::new()
        .prefix("vmaf_dist_")
        .suffix(".y4m")
        .tempfile()
        .context("Failed to create dist temp file")?;
    let json_file = tempfile::Builder::new()
        .prefix("vmaf_result_")
        .suffix(".json")
        .tempfile()
        .context("Failed to create json temp file")?;

    convert_to_y4m(reference, ref_y4m_file.path())?;
    convert_to_y4m(distorted, dist_y4m_file.path())?;

    let status = Command::new("vmaf")
        .arg("--reference")
        .arg(ref_y4m_file.path())
        .arg("--distorted")
        .arg(dist_y4m_file.path())
        .arg("--feature")
        .arg("float_ms_ssim")
        .arg("--output")
        .arg(json_file.path())
        .arg("--json")
        .status()
        .context("Failed to run vmaf")?;

    if !status.success() {
        anyhow::bail!("vmaf command failed");
    }

    let result = parse_vmaf_json(json_file.path())?;

    Ok(result)
}

fn convert_to_y4m(input: &Path, output_path: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-f")
        .arg("yuv4mpegpipe")
        .arg("-y")
        .arg(crate::safe_path_arg(output_path).as_ref())
        .stderr(std::process::Stdio::null())
        .status()
        .context("Failed to convert to Y4M")?;

    if !status.success() {
        anyhow::bail!("Y4M conversion failed");
    }

    Ok(())
}

fn parse_vmaf_json(path: &Path) -> Result<f64> {
    let content = std::fs::read_to_string(path).context("Failed to read vmaf output")?;

    let json: Value = serde_json::from_str(&content).context("Failed to parse JSON")?;

    let ms_ssim = json
        .get("pooled_metrics")
        .and_then(|p| p.get("float_ms_ssim"))
        .and_then(|m| m.get("mean"))
        .and_then(|v| v.as_f64())
        .context("MS-SSIM not found in JSON")?;

    Ok(ms_ssim.clamp(0.0, 1.0))
}
