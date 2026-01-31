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

// ═══════════════════════════════════════════════════════════════
// 常量定义
// ═══════════════════════════════════════════════════════════════

/// 🔥 长视频阈值（秒）- 超过此时长默认跳过 MS-SSIM
pub const LONG_VIDEO_THRESHOLD: f32 = 300.0;

// ═══════════════════════════════════════════════════════════════
// 类型定义
// ═══════════════════════════════════════════════════════════════

/// 质量验证阈值
#[derive(Debug, Clone)]
pub struct QualityThresholds {
    /// 最小 SSIM（0.0-1.0，推荐 >= 0.95）
    pub min_ssim: f64,
    /// 最小 PSNR（dB，推荐 >= 35）
    pub min_psnr: f64,
    /// 最小 MS-SSIM（0.0-1.0，推荐 >= 0.90）
    pub min_ms_ssim: f64,
    /// 是否启用 SSIM 验证
    pub validate_ssim: bool,
    /// 是否启用 PSNR 验证
    pub validate_psnr: bool,
    /// 是否启用 MS-SSIM 验证（多尺度 SSIM，更准确但稍慢）
    pub validate_ms_ssim: bool,
    /// 🔥 强制长视频也验证 MS-SSIM（默认 false，>5分钟视频跳过 MS-SSIM）
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

/// 🔥 v4.1: 交叉验证结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossValidationResult {
    /// 所有指标一致通过 (SSIM + PSNR + MS-SSIM)
    AllAgree,
    /// 多数指标通过 (2/3)
    MajorityAgree,
    /// 指标分歧 (1/3 或更少)
    Divergent,
}

// ═══════════════════════════════════════════════════════════════
// 公共函数
// ═══════════════════════════════════════════════════════════════

/// 获取视频时长（秒）
///
/// 用于判断是否启用 MS-SSIM 验证
pub fn get_video_duration(input: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(["-show_entries", "format=duration"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        // .arg("--") // 🔥 v7.9: ffmpeg does not support '--' as delimiter
        .arg(input)
        .output()
        .ok()?;

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
}

/// 🔥 v5.69: 增强的 SSIM 计算（多策略 fallback）
///
/// 策略：标准方法优先，仅在失败时才 fallback 到格式转换
/// 这样可以保证大多数视频使用最准确的 SSIM 计算方式
pub fn calculate_ssim_enhanced(input: &Path, output: &Path) -> Option<f64> {
    // 🔥 v5.69.4: 定义滤镜策略（按优先级排序）
    let filters: &[(&str, &str)] = &[
        // 策略 1: 标准方法 - 适用于大多数视频
        ("standard", "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim"),
        // 策略 2: 格式转换 - 处理 VP8/VP9/AV1/10-bit/alpha 等特殊格式
        ("format_convert", "[0:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[ref];[1:v]format=yuv420p[cmp];[ref][cmp]ssim"),
        // 策略 3: 简单方法 - 最后的尝试
        ("simple", "ssim"),
    ];

    for (name, filter) in filters {
        let result = Command::new("ffmpeg")
            .arg("-i")
            // .arg("--") // 🔥 v7.9: ffmpeg does not support '--' as delimiter
            .arg(input)
            .arg("-i")
            .arg(output)
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
                    // 使用 precision 模块验证 SSIM 有效性
                    if is_valid_ssim_value(ssim) {
                        eprintln!("   📊 SSIM calculated using {} method: {:.6}", name, ssim);
                        return Some(ssim);
                    }
                }
            }
            Ok(_) => {
                // 当前策略失败，尝试下一个
                eprintln!("   ⚠️  SSIM {} method failed, trying next...", name);
            }
            Err(e) => {
                eprintln!("   ⚠️  ffmpeg {} failed: {}", name, e);
            }
        }
    }

    // 所有策略都失败
    eprintln!("   ❌ ALL SSIM CALCULATION METHODS FAILED!");
    None
}

/// 🔥 v6.9.3: 计算完整 SSIM（包含 Y/U/V 所有通道）
///
/// MS-SSIM 只计算亮度通道，对于 yuv444p → yuv420p 的色度下采样无法检测
/// 此函数返回 SSIM All（加权平均），能更准确反映色度损失
///
/// # Returns
/// (y_ssim, u_ssim, v_ssim, all_ssim)
pub fn calculate_ssim_all(input: &Path, output: &Path) -> Option<(f64, f64, f64, f64)> {
    let result = Command::new("ffmpeg")
        .arg("-i")
        // .arg("--") // 🔥 v7.9: ffmpeg does not support '--' as delimiter
        .arg(input)
        .arg("-i")
        .arg(output)
        .arg("-lavfi")
        .arg("[0:v][1:v]ssim")
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();

    if let Ok(out) = result {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // 解析: [Parsed_ssim_0 @ ...] SSIM Y:0.999399 ... U:0.966225 ... V:0.936907 ... All:0.967510 ...
        for line in stderr.lines() {
            if line.contains("SSIM Y:") && line.contains("All:") {
                let y = extract_ssim_value(line, "Y:");
                let u = extract_ssim_value(line, "U:");
                let v = extract_ssim_value(line, "V:");
                let all = extract_ssim_value(line, "All:");
                if let (Some(y), Some(u), Some(v), Some(all)) = (y, u, v, all) {
                    return Some((y, u, v, all));
                }
            }
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════
// 辅助函数
// ═══════════════════════════════════════════════════════════════

/// 🔥 v5.69: 从 ffmpeg 输出解析 SSIM 值
fn parse_ssim_from_output(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        if line.contains("SSIM") && line.contains("All:") {
            if let Some(all_pos) = line.find("All:") {
                let after_all = &line[all_pos + 4..];
                let after_all = after_all.trim_start();
                // 处理格式: "All:0.987654 (12.34)" 或 "All:0.987654"
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

/// 从 SSIM 输出行提取指定通道的值
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

/// 简单的 SSIM 有效性检查（0.0-1.0 范围）
#[inline]
fn is_valid_ssim_value(ssim: f64) -> bool {
    (0.0..=1.0).contains(&ssim) && !ssim.is_nan()
}
