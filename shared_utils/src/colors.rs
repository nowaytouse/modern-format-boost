//! 🎨 v5.67: 终端颜色支持模块
//!
//! 提供统一的彩色输出，改善 UI/UX 体验

use console::{style, Style};

// ═══════════════════════════════════════════════════════════════
// 🎨 颜色常量
// ═══════════════════════════════════════════════════════════════

/// 成功颜色（绿色）
pub fn success() -> Style {
    Style::new().green().bold()
}

/// 错误颜色（红色）
pub fn error() -> Style {
    Style::new().red().bold()
}

/// 警告颜色（黄色）
pub fn warning() -> Style {
    Style::new().yellow()
}

/// 信息颜色（青色）
pub fn info() -> Style {
    Style::new().cyan()
}

/// 强调颜色（品红）
pub fn highlight() -> Style {
    Style::new().magenta().bold()
}

/// 数值颜色（蓝色）
pub fn number() -> Style {
    Style::new().blue().bold()
}

/// 暗淡颜色（灰色）
pub fn dim() -> Style {
    Style::new().dim()
}

// ═══════════════════════════════════════════════════════════════
// 🎨 格式化辅助函数
// ═══════════════════════════════════════════════════════════════

/// 格式化 CRF 值（带颜色）
pub fn fmt_crf(crf: f32) -> String {
    format!("{}", style(format!("CRF {:.1}", crf)).cyan().bold())
}

/// 格式化 SSIM 值（带颜色和评级）
pub fn fmt_ssim(ssim: f64) -> String {
    let (color_ssim, grade) = if ssim >= 0.99 {
        (style(format!("{:.4}", ssim)).green().bold(), "🟢")
    } else if ssim >= 0.97 {
        (style(format!("{:.4}", ssim)).green(), "🟡")
    } else if ssim >= 0.95 {
        (style(format!("{:.4}", ssim)).yellow(), "🟠")
    } else {
        (style(format!("{:.4}", ssim)).red(), "🔴")
    };
    format!("SSIM {} {}", color_ssim, grade)
}

/// 格式化大小变化百分比（带颜色）
pub fn fmt_size_pct(pct: f64) -> String {
    if pct < 0.0 {
        // 压缩成功（绿色）
        format!("{}", style(format!("{:+.1}%", pct)).green().bold())
    } else if pct < 5.0 {
        // 略微增大（黄色）
        format!("{}", style(format!("{:+.1}%", pct)).yellow())
    } else {
        // 明显增大（红色）
        format!("{}", style(format!("{:+.1}%", pct)).red())
    }
}

/// 格式化压缩状态图标
pub fn fmt_compress_status(compressed: bool) -> &'static str {
    if compressed { "✅" } else { "❌" }
}

/// 格式化文件大小（带单位和颜色）
pub fn fmt_size(bytes: u64) -> String {
    let (value, unit) = if bytes >= 1024 * 1024 * 1024 {
        (bytes as f64 / 1024.0 / 1024.0 / 1024.0, "GB")
    } else if bytes >= 1024 * 1024 {
        (bytes as f64 / 1024.0 / 1024.0, "MB")
    } else if bytes >= 1024 {
        (bytes as f64 / 1024.0, "KB")
    } else {
        (bytes as f64, "B")
    };
    format!("{}", style(format!("{:.2} {}", value, unit)).blue())
}

/// 格式化时长（秒）
pub fn fmt_duration(secs: f64) -> String {
    if secs >= 60.0 {
        let mins = (secs / 60.0).floor();
        let remaining = secs - mins * 60.0;
        format!("{}", style(format!("{:.0}m {:.1}s", mins, remaining)).cyan())
    } else {
        format!("{}", style(format!("{:.1}s", secs)).cyan())
    }
}

/// 格式化迭代次数
pub fn fmt_iterations(iter: u32, max: u32) -> String {
    let ratio = iter as f64 / max as f64;
    if ratio <= 0.5 {
        format!("{}", style(format!("{}/{}", iter, max)).green())
    } else if ratio <= 0.8 {
        format!("{}", style(format!("{}/{}", iter, max)).yellow())
    } else {
        format!("{}", style(format!("{}/{}", iter, max)).red())
    }
}

// ═══════════════════════════════════════════════════════════════
// 🎨 分隔线和标题
// ═══════════════════════════════════════════════════════════════

/// 打印彩色标题
pub fn print_header(title: &str) {
    eprintln!("{}", style(format!("═══ {} ═══", title)).cyan().bold());
}

/// 打印彩色分隔线
pub fn print_separator() {
    eprintln!("{}", style("─────────────────────────────────────────────").dim());
}

/// 打印成功消息
pub fn print_success(msg: &str) {
    eprintln!("{} {}", style("✅").green(), style(msg).green().bold());
}

/// 打印错误消息
pub fn print_error(msg: &str) {
    eprintln!("{} {}", style("❌").red(), style(msg).red().bold());
}

/// 打印警告消息
pub fn print_warning(msg: &str) {
    eprintln!("{} {}", style("⚠️").yellow(), style(msg).yellow());
}

/// 打印信息消息
pub fn print_info(msg: &str) {
    eprintln!("{} {}", style("ℹ️").cyan(), style(msg).cyan());
}

// ═══════════════════════════════════════════════════════════════
// 🎨 结果格式化
// ═══════════════════════════════════════════════════════════════

/// 格式化搜索结果行
pub fn fmt_search_result(crf: f32, size_pct: f64, ssim: Option<f64>, compressed: bool) -> String {
    let status = fmt_compress_status(compressed);
    let size_str = fmt_size_pct(size_pct);
    
    if let Some(s) = ssim {
        let ssim_str = fmt_ssim(s);
        format!("   {} {} | {} | {}", 
            if compressed { style("✓").green() } else { style("✗").red() },
            fmt_crf(crf), size_str, ssim_str)
    } else {
        format!("   {} {} | {} {}", 
            if compressed { style("✓").green() } else { style("✗").red() },
            fmt_crf(crf), size_str, status)
    }
}

/// 格式化最终结果
pub fn fmt_final_result(crf: f32, size_pct: f64, ssim: Option<f64>, iterations: u32) -> String {
    let ssim_str = ssim.map(|s| fmt_ssim(s)).unwrap_or_else(|| "---".to_string());
    format!("{} {} | {} | {} | {} iterations",
        style("RESULT:").green().bold(),
        fmt_crf(crf),
        fmt_size_pct(size_pct),
        ssim_str,
        style(iterations).cyan())
}
