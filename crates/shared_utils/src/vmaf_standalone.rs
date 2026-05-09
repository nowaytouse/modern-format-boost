//! VMAF Standalone Integration
//! Uses standalone vmaf command-line tool, bypassing ffmpeg libvmaf dependency

use crate::builder_base::ToolBuilder;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

#[must_use]
pub fn is_vmaf_available() -> bool {
    crate::tool_builders::VmafBuilder::new().check_available()
}

/// Calculate MS-SSIM using the standalone `vmaf` tool.
///
/// This function converts both input files to Y4M format in temporary storage,
/// runs the `vmaf` command, and parses the resulting JSON.
///
/// # Errors
/// Returns an error if:
/// - Temporary files cannot be created.
/// - Y4M conversion fails.
/// - The `vmaf` command fails to run or returns a non-zero exit code.
/// - The resulting JSON is invalid or missing the MS-SSIM metric.
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

    let status = crate::tool_builders::VmafBuilder::new()
        .reference(ref_y4m_file.path())
        .distorted(dist_y4m_file.path())
        .feature("float_ms_ssim")
        .output(json_file.path())
        .json(true)
        .build()
        .status()
        .context("Failed to run vmaf")?;

    if !status.success() {
        anyhow::bail!("vmaf command failed");
    }

    let result = parse_vmaf_json(json_file.path())?;

    Ok(result)
}

fn convert_to_y4m(input: &Path, output_path: &Path) -> Result<()> {
    let status = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(input)
        .pix_fmt_str("yuv420p")
        .format("yuv4mpegpipe")
        .output(output_path)
        .overwrite()
        .build()
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
        .and_then(serde_json::Value::as_f64)
        .context("MS-SSIM not found in JSON")?;

    Ok(ms_ssim.clamp(0.0, 1.0))
}
