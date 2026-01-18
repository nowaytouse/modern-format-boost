//! 🔥 Standalone VMAF Tool Integration
//! 使用独立的 vmaf 命令行工具，绕过 ffmpeg libvmaf 依赖

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use serde_json::Value;

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
/// # Note
/// vmaf 的 float_ms_ssim 特征在 YUV 色彩空间上计算，
/// 已隐式包含亮度(Y)和色度(U,V)信息，无需分别计算各通道。
pub fn calculate_ms_ssim_standalone(
    reference: &Path,
    distorted: &Path,
) -> Result<f64> {
    // 步骤 1: 转换为 Y4M 格式（vmaf 需要）
    let ref_y4m = convert_to_y4m(reference)?;
    let dist_y4m = convert_to_y4m(distorted)?;
    
    // 步骤 2: 运行 vmaf 计算
    let output_json = format!("/tmp/vmaf_result_{}.json", 
        std::process::id());
    
    let status = Command::new("vmaf")
        .arg("--reference").arg(&ref_y4m)
        .arg("--distorted").arg(&dist_y4m)
        .arg("--feature").arg("float_ms_ssim")
        .arg("--output").arg(&output_json)
        .arg("--json")
        .status()
        .context("Failed to run vmaf")?;
    
    if !status.success() {
        anyhow::bail!("vmaf command failed");
    }
    
    // 步骤 3: 解析结果
    let result = parse_vmaf_json(&output_json)?;
    
    // 清理临时文件
    let _ = std::fs::remove_file(&ref_y4m);
    let _ = std::fs::remove_file(&dist_y4m);
    let _ = std::fs::remove_file(&output_json);
    
    Ok(result)
}

/// 转换视频为 Y4M 格式
fn convert_to_y4m(input: &Path) -> Result<String> {
    let output = format!("/tmp/vmaf_{}_{}.y4m", 
        input.file_stem().unwrap().to_string_lossy(),
        std::process::id());
    
    let status = Command::new("ffmpeg")
        .arg("-i").arg(input)
        .arg("-pix_fmt").arg("yuv420p")
        .arg("-f").arg("yuv4mpegpipe")
        .arg("-y").arg(&output)
        .stderr(std::process::Stdio::null())
        .status()
        .context("Failed to convert to Y4M")?;
    
    if !status.success() {
        anyhow::bail!("Y4M conversion failed");
    }
    
    Ok(output)
}

/// 解析 vmaf JSON 输出
fn parse_vmaf_json(path: &str) -> Result<f64> {
    let content = std::fs::read_to_string(path)
        .context("Failed to read vmaf output")?;
    
    let json: Value = serde_json::from_str(&content)
        .context("Failed to parse JSON")?;
    
    // 提取 pooled_metrics.float_ms_ssim.mean
    let ms_ssim = json
        .get("pooled_metrics")
        .and_then(|p| p.get("float_ms_ssim"))
        .and_then(|m| m.get("mean"))
        .and_then(|v| v.as_f64())
        .context("MS-SSIM not found in JSON")?;
    
    Ok(ms_ssim.clamp(0.0, 1.0))
}
