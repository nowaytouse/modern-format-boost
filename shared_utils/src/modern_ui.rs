//! 🔥 v5.19: 现代化 UI/UX 模块
//!
//! 提供现代化的终端交互和视觉效果：
//! - 动态 Spinner 动画
//! - 渐变色进度条
//! - 实时状态更新
//! - 美化的结果展示

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
// Mutex 暂未使用，保留以备将来扩展
use std::time::Instant;

// ═══════════════════════════════════════════════════════════════
// 🎨 颜色和样式常量
// ═══════════════════════════════════════════════════════════════

/// ANSI 颜色代码
pub mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const ITALIC: &str = "\x1b[3m";

    // 前景色
    pub const RED: &str = "\x1b[31m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const MAGENTA: &str = "\x1b[35m";
    pub const CYAN: &str = "\x1b[36m";
    pub const WHITE: &str = "\x1b[37m";

    // 亮色
    pub const BRIGHT_RED: &str = "\x1b[91m";
    pub const BRIGHT_GREEN: &str = "\x1b[92m";
    pub const BRIGHT_YELLOW: &str = "\x1b[93m";
    pub const BRIGHT_BLUE: &str = "\x1b[94m";
    pub const BRIGHT_MAGENTA: &str = "\x1b[95m";
    pub const BRIGHT_CYAN: &str = "\x1b[96m";
}

/// Unicode 符号
pub mod symbols {
    pub const CHECK: &str = "✓";
    pub const CROSS: &str = "✗";
    pub const ARROW_RIGHT: &str = "→";
    pub const ARROW_DOWN: &str = "↓";
    pub const BULLET: &str = "•";
    pub const STAR: &str = "★";
    pub const SPARKLE: &str = "✨";
    pub const FIRE: &str = "🔥";
    pub const ROCKET: &str = "🚀";
    pub const SEARCH: &str = "🔍";
    pub const CHART: &str = "📊";
    pub const FOLDER: &str = "📁";
    pub const VIDEO: &str = "🎬";
    pub const IMAGE: &str = "🖼️";
    pub const COMPRESS: &str = "📦";
    pub const QUALITY: &str = "🎯";
    pub const GPU: &str = "⚡";
    pub const CPU: &str = "🖥️";
    pub const CLOCK: &str = "⏱️";
    pub const SAVE: &str = "💾";
    pub const WARNING: &str = "⚠️";
    pub const ERROR: &str = "❌";
    pub const SUCCESS: &str = "✅";
    pub const INFO: &str = "ℹ️";
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.30: 统一进度条样式 - 更粗更显眼
// ═══════════════════════════════════════════════════════════════

/// 统一进度条样式常量 - 全项目使用
pub mod progress_style {
    /// 🔥 统一进度条字符: 填充 + 当前位置 + 空白
    /// indicatif 需要 3 个字符: (filled, current, empty)
    /// 视觉效果: ████████▓░░░░░░░
    pub const PROGRESS_CHARS: &str = "█▓░";

    /// 进度条宽度 - 统一 35 字符，足够显眼
    pub const BAR_WIDTH: usize = 35;

    /// 进度条边框字符
    pub const BAR_LEFT: &str = "▕";
    pub const BAR_RIGHT: &str = "▏";

    /// Spinner 字符序列 - 统一使用 Braille 点阵
    pub const SPINNER_CHARS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

    /// 统一模板 - 批量处理进度条
    /// 🔥 v7.9.1: 使用 {eta} 替代 {eta_precise}，避免溢出显示天文数字
    pub const BATCH_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} ▕{bar:35.green/black}▏ {percent:>3}% • {pos}/{len} • ⏱️ {elapsed_precise} (ETA: {eta}) • {msg}";

    /// 统一模板 - 探索进度条（迭代次数在 msg 中显示）
    pub const EXPLORE_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} ▕{bar:35.green/black}▏ {percent:>3}% • ⏱️ {elapsed_precise} • {msg}";

    /// 统一模板 - 简洁进度条
    pub const COMPACT_TEMPLATE: &str =
        "{prefix:.cyan} ▕{bar:30.green/black}▏ {percent:>3}% ({pos}/{len}) {msg:.dim}";

    /// 统一模板 - Spinner
    pub const SPINNER_TEMPLATE: &str =
        "{spinner:.green} {prefix:.cyan.bold} • ⏱️ {elapsed_precise} • {msg}";
}

// ═══════════════════════════════════════════════════════════════
// 🔄 Spinner 动画
// ═══════════════════════════════════════════════════════════════

/// Spinner 帧序列
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_DOTS: &[&str] = &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
#[allow(dead_code)]
const SPINNER_BOUNCE: &[&str] = &["⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈"];

/// 全局 Spinner 状态
static SPINNER_FRAME: AtomicU64 = AtomicU64::new(0);
#[allow(dead_code)]
static SPINNER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 获取当前 Spinner 帧
pub fn spinner_frame() -> &'static str {
    let frame = SPINNER_FRAME.fetch_add(1, Ordering::Relaxed) as usize;
    SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
}

/// 获取 Dots Spinner 帧
pub fn spinner_dots() -> &'static str {
    let frame = SPINNER_FRAME.fetch_add(1, Ordering::Relaxed) as usize;
    SPINNER_DOTS[frame % SPINNER_DOTS.len()]
}

// ═══════════════════════════════════════════════════════════════
// 📊 现代化进度条
// ═══════════════════════════════════════════════════════════════

/// 进度条样式
#[derive(Clone, Copy)]
pub enum ProgressStyle {
    /// 经典样式: [████████░░░░]
    Classic,
    /// 现代样式: ━━━━━━━━───
    Modern,
    /// 渐变样式: ▓▓▓▓▒▒░░
    Gradient,
    /// 块状样式: █▓▒░
    Blocks,
}

/// 渲染进度条
pub fn render_progress_bar(progress: f64, width: usize, style: ProgressStyle) -> String {
    let progress = progress.clamp(0.0, 1.0);
    let filled = (progress * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);

    match style {
        ProgressStyle::Classic => {
            format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
        }
        ProgressStyle::Modern => {
            format!("{}{}", "━".repeat(filled), "─".repeat(empty))
        }
        ProgressStyle::Gradient => {
            let mut bar = String::new();
            for i in 0..width {
                if i < filled {
                    bar.push('▓');
                } else if i == filled && progress > 0.0 {
                    bar.push('▒');
                } else {
                    bar.push('░');
                }
            }
            bar
        }
        ProgressStyle::Blocks => {
            let mut bar = String::new();
            for i in 0..width {
                let pos = i as f64 / width as f64;
                if pos < progress - 0.1 {
                    bar.push('█');
                } else if pos < progress - 0.05 {
                    bar.push('▓');
                } else if pos < progress {
                    bar.push('▒');
                } else {
                    bar.push('░');
                }
            }
            bar
        }
    }
}

/// 带颜色的进度条
pub fn render_colored_progress(progress: f64, width: usize) -> String {
    use colors::*;

    let bar = render_progress_bar(progress, width, ProgressStyle::Modern);
    let pct = (progress * 100.0) as u32;

    // 根据进度选择颜色
    let color = if pct >= 80 {
        BRIGHT_GREEN
    } else if pct >= 50 {
        BRIGHT_CYAN
    } else if pct >= 25 {
        BRIGHT_YELLOW
    } else {
        BRIGHT_RED
    };

    format!("{}{}{}", color, bar, RESET)
}

// ═══════════════════════════════════════════════════════════════
// 🎯 智能探索进度显示
// ═══════════════════════════════════════════════════════════════

/// 探索进度状态
pub struct ExploreProgressState {
    pub stage: String,
    pub crf: f32,
    pub size_pct: f64,
    pub ssim: Option<f64>,
    pub iteration: u32,
    pub best_crf: Option<f32>,
    pub start_time: Instant,
}

impl ExploreProgressState {
    pub fn new(stage: &str) -> Self {
        Self {
            stage: stage.to_string(),
            crf: 0.0,
            size_pct: 0.0,
            ssim: None,
            iteration: 0,
            best_crf: None,
            start_time: Instant::now(),
        }
    }

    /// 更新并显示进度
    pub fn update(&mut self, crf: f32, size_pct: f64, ssim: Option<f64>) {
        self.crf = crf;
        self.size_pct = size_pct;
        self.ssim = ssim;
        self.iteration += 1;

        if size_pct < 0.0 {
            self.best_crf = Some(crf);
        }

        self.display();
    }

    /// 显示当前进度
    pub fn display(&self) {
        use colors::*;
        use symbols::*;

        let elapsed = self.start_time.elapsed().as_secs_f64();

        // 大小变化图标和颜色
        let (_size_icon, size_color) = if self.size_pct < 0.0 {
            (SAVE, BRIGHT_GREEN)
        } else {
            (WARNING, BRIGHT_YELLOW)
        };

        // SSIM 显示
        let ssim_str = self
            .ssim
            .map(|s| format!(" {}SSIM {:.4}{}", DIM, s, RESET))
            .unwrap_or_default();

        // 最佳 CRF
        let best_str = self
            .best_crf
            .map(|b| format!(" {}Best: {:.1}{}", DIM, b, RESET))
            .unwrap_or_default();

        // 固定底部单行显示
        eprint!(
            "\r\x1b[K{} {}{}{} {} CRF {:.1} {} {}{:+.1}%{}{}{} {} {}{:.1}s{}",
            spinner_frame(),
            CYAN,
            self.stage,
            RESET,
            BULLET,
            self.crf,
            BULLET,
            size_color,
            self.size_pct,
            RESET,
            ssim_str,
            best_str,
            BULLET,
            DIM,
            elapsed,
            RESET
        );
        let _ = io::stderr().flush();
    }

    /// 完成并显示结果
    pub fn finish(&self, final_crf: f32, final_size_pct: f64, final_ssim: Option<f64>) {
        use colors::*;
        use symbols::*;

        let elapsed = self.start_time.elapsed().as_secs_f64();

        // 清除进度行
        eprint!("\r\x1b[K");

        // SSIM 评级
        let (ssim_str, ssim_rating) = match final_ssim {
            Some(s) if s >= 0.99 => (format!("SSIM {:.4}", s), format!("{} Excellent", SUCCESS)),
            Some(s) if s >= 0.98 => (format!("SSIM {:.4}", s), format!("{} Very Good", SUCCESS)),
            Some(s) if s >= 0.95 => (format!("SSIM {:.4}", s), format!("{}  Good", CHECK)),
            Some(s) => (format!("SSIM {:.4}", s), format!("{}  Fair", WARNING)),
            None => (String::new(), String::new()),
        };

        // 大小变化
        let size_str = if final_size_pct < 0.0 {
            format!("{}{:+.1}%{} {}", BRIGHT_GREEN, final_size_pct, RESET, SAVE)
        } else {
            format!("{}{:+.1}%{}", BRIGHT_YELLOW, final_size_pct, RESET)
        };

        // 结果行
        eprintln!(
            "{} {}Result:{} CRF {:.1} {} {} {} {} {} {} iter {} {:.1}s",
            SUCCESS,
            BOLD,
            RESET,
            final_crf,
            BULLET,
            size_str,
            BULLET,
            ssim_str,
            ssim_rating,
            BULLET,
            self.iteration,
            elapsed
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// 📦 美化的结果框
// ═══════════════════════════════════════════════════════════════

/// 显示结果框
pub fn print_result_box(title: &str, lines: &[&str]) {
    use colors::*;

    // 计算最大宽度
    let max_width = lines
        .iter()
        .map(|l| strip_ansi(l).len())
        .max()
        .unwrap_or(40)
        .max(strip_ansi(title).len())
        .max(40);

    let box_width = max_width + 4;

    // 顶部边框
    eprintln!("{}╭{}╮{}", CYAN, "─".repeat(box_width), RESET);

    // 标题
    let title_padding = box_width - strip_ansi(title).len() - 2;
    eprintln!(
        "{}│{} {}{}{} {}{}│{}",
        CYAN,
        RESET,
        BOLD,
        title,
        RESET,
        " ".repeat(title_padding),
        CYAN,
        RESET
    );

    // 分隔线
    eprintln!("{}├{}┤{}", CYAN, "─".repeat(box_width), RESET);

    // 内容行
    for line in lines {
        let padding = box_width - strip_ansi(line).len() - 2;
        eprintln!(
            "{}│{} {}{} {}│{}",
            CYAN,
            RESET,
            line,
            " ".repeat(padding),
            CYAN,
            RESET
        );
    }

    // 底部边框
    eprintln!("{}╰{}╯{}", CYAN, "─".repeat(box_width), RESET);
}

/// 移除 ANSI 转义序列（用于计算实际字符宽度）
fn strip_ansi(s: &str) -> String {
    let mut result = String::new();
    let mut in_escape = false;

    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            result.push(c);
        }
    }

    result
}

// ═══════════════════════════════════════════════════════════════
// 🎬 阶段标题
// ═══════════════════════════════════════════════════════════════

/// 显示阶段标题
pub fn print_stage(_icon: &str, title: &str) {
    use colors::*;
    eprintln!("{}📍{} {}{}{}", DIM, RESET, BOLD, title, RESET);
    let _ = io::stderr().flush();
}

/// 显示子阶段
pub fn print_substage(title: &str) {
    use colors::*;
    eprintln!("   {}{}•{} {}", DIM, colors::CYAN, RESET, title);
}

// ═══════════════════════════════════════════════════════════════
// 🔔 通知和提示
// ═══════════════════════════════════════════════════════════════

/// 成功消息
pub fn print_success(msg: &str) {
    use colors::*;
    eprintln!("{}{} {}{}", BRIGHT_GREEN, symbols::SUCCESS, msg, RESET);
}

/// 警告消息
pub fn print_warning(msg: &str) {
    use colors::*;
    eprintln!("{}{} {}{}", BRIGHT_YELLOW, symbols::WARNING, msg, RESET);
}

/// 错误消息
pub fn print_error(msg: &str) {
    use colors::*;
    eprintln!("{}{} {}{}", BRIGHT_RED, symbols::ERROR, msg, RESET);
}

/// 信息消息
pub fn print_info(msg: &str) {
    use colors::*;
    eprintln!("{}{} {}{}", BRIGHT_CYAN, symbols::INFO, msg, RESET);
}

// ═══════════════════════════════════════════════════════════════
// 📊 格式化工具
// ═══════════════════════════════════════════════════════════════

/// 格式化文件大小
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// 格式化时长
pub fn format_duration(secs: f64) -> String {
    if secs >= 3600.0 {
        let h = (secs / 3600.0).floor() as u32;
        let m = ((secs % 3600.0) / 60.0).floor() as u32;
        let s = (secs % 60.0).floor() as u32;
        format!("{}h {:02}m {:02}s", h, m, s)
    } else if secs >= 60.0 {
        let m = (secs / 60.0).floor() as u32;
        let s = (secs % 60.0).floor() as u32;
        format!("{}m {:02}s", m, s)
    } else {
        format!("{:.1}s", secs)
    }
}

/// 格式化百分比变化
pub fn format_size_change(pct: f64) -> String {
    use colors::*;

    if pct < -50.0 {
        format!("{}{:+.1}%{} {}", BRIGHT_GREEN, pct, RESET, symbols::SPARKLE)
    } else if pct < 0.0 {
        format!("{}{:+.1}%{} {}", BRIGHT_GREEN, pct, RESET, symbols::SAVE)
    } else if pct < 10.0 {
        format!("{}{:+.1}%{}", BRIGHT_YELLOW, pct, RESET)
    } else {
        format!("{}{:+.1}%{} {}", BRIGHT_RED, pct, RESET, symbols::WARNING)
    }
}

/// 🔥 v6.2: 格式化大小差异（自动选择合适单位）
/// 根据差异大小自动选择 B/KB/MB 单位，避免小文件显示 +0.0 MB
pub fn format_size_diff(diff_bytes: i64) -> String {
    let abs_diff = diff_bytes.unsigned_abs();
    let sign = if diff_bytes >= 0 { "+" } else { "-" };

    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    if abs_diff >= MB {
        format!("{}{:.1} MB", sign, abs_diff as f64 / MB as f64)
    } else if abs_diff >= KB {
        format!("{}{:.1} KB", sign, abs_diff as f64 / KB as f64)
    } else {
        format!("{}{} B", sign, abs_diff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar() {
        let bar = render_progress_bar(0.5, 20, ProgressStyle::Modern);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1500), "1.5 KB");
        assert_eq!(format_size(1_500_000), "1.43 MB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(5.5), "5.5s");
        assert_eq!(format_duration(65.0), "1m 05s");
        assert_eq!(format_duration(3665.0), "1h 01m 05s");
    }

    #[test]
    fn test_strip_ansi() {
        let s = "\x1b[31mRed\x1b[0m Text";
        assert_eq!(strip_ansi(s), "Red Text");
    }
}
