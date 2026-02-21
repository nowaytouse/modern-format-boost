//! 🔥 Standalone VMAF Tool Integration
//! 使用独立的 vmaf 命令行工具，绕过 ffmpeg libvmaf 依赖

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// 检查独立 vmaf 工具是否可用
pub fn is_vmaf_available() -> bool {
    Command::new("vmaf")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 使用独立 vmaf 工具计算 MS-SSIM
///
/// # Arguments
/// * `reference` - 参考视频（原始）
/// * `distorted` - 失真视频（编码后）
///
/// # Returns
/// MS-SSIM 分数 (0.0-1.0)
///
/// # ⚠️ Important Limitation
/// **Verified with multi-channel testing**: MS-SSIM is Y-channel (luma) only!
/// - ✅ Detects luma degradation
/// - ❌ Does NOT detect chroma (U/V) degradation
/// - 💡 This is an algorithm limitation, not a tool limitation
/// - 💡 Recommendation: Use with SSIM All for complete verification
///
/// Test results (both standalone vmaf and ffmpeg libvmaf):
/// - Y-only degradation (10%): Y=0.996, U=1.000, V=1.000 ✅ Detected
/// - UV-only degradation (30%): Y=1.000, U=1.000, V=1.000 ❌ Not detected
///
/// Even with extractplanes filter, U/V channels cannot detect chroma degradation.
pub fn calculate_ms_ssim_standalone(reference: &Path, distorted: &Path) -> Result<f64> {
    // 步骤 1: 创建临时文件 (RAII guards ensure cleanup)
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

    // 转换为 Y4M (ffmpeg writes to these paths)
    convert_to_y4m(reference, ref_y4m_file.path())?;
    convert_to_y4m(distorted, dist_y4m_file.path())?;

    // 步骤 2: 运行 vmaf 计算
    // vmaf writes JSON result to output path
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

    // 步骤 3: 解析结果
    // Read from the temp file path while the guard is still alive
    let result = parse_vmaf_json(json_file.path())?;

    // Cleanup happens automatically when guards drop
    Ok(result)
}

/// 转换视频为 Y4M 格式
fn convert_to_y4m(input: &Path, output_path: &Path) -> Result<()> {
    // ⚠️ Important: We must overwrite the empty temp file created by Builder
    let status = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-f")
        .arg("yuv4mpegpipe")
        .arg("-y") // Overwrite existing file
        .arg(crate::safe_path_arg(output_path).as_ref())
        .stderr(std::process::Stdio::null())
        .status()
        .context("Failed to convert to Y4M")?;

    if !status.success() {
        anyhow::bail!("Y4M conversion failed");
    }

    Ok(())
}

/// 解析 vmaf JSON 输出
fn parse_vmaf_json(path: &Path) -> Result<f64> {
    let content = std::fs::read_to_string(path).context("Failed to read vmaf output")?;

    let json: Value = serde_json::from_str(&content).context("Failed to parse JSON")?;

    // 提取 pooled_metrics.float_ms_ssim.mean
    let ms_ssim = json
        .get("pooled_metrics")
        .and_then(|p| p.get("float_ms_ssim"))
        .and_then(|m| m.get("mean"))
        .and_then(|v| v.as_f64())
        .context("MS-SSIM not found in JSON")?;

    Ok(ms_ssim.clamp(0.0, 1.0))
}
