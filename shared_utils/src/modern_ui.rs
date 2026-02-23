//! 🔥 v5.19: 现代化 UI/UX 模块
//!
//! 提供现代化的终端交互和视觉效果：
//! - 动态 Spinner 动画
//! - 渐变色进度条
//! - 实时状态更新
//! - 美化的结果展示

use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;


pub mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const ITALIC: &str = "\x1b[3m";

    pub const RED: &str = "\x1b[31m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const MAGENTA: &str = "\x1b[35m";
    pub const CYAN: &str = "\x1b[36m";
    pub const WHITE: &str = "\x1b[37m";

    pub const BRIGHT_RED: &str = "\x1b[91m";
    pub const BRIGHT_GREEN: &str = "\x1b[92m";
    pub const BRIGHT_YELLOW: &str = "\x1b[93m";
    pub const BRIGHT_BLUE: &str = "\x1b[94m";
    pub const BRIGHT_MAGENTA: &str = "\x1b[95m";
    pub const BRIGHT_CYAN: &str = "\x1b[96m";
}

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


pub mod progress_style {
    pub const PROGRESS_CHARS: &str = "█▓░";

    pub const BAR_WIDTH: usize = 35;

    pub const BAR_LEFT: &str = "▕";
    pub const BAR_RIGHT: &str = "▏";

    pub const SPINNER_CHARS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

    pub const BATCH_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} ▕{bar:35.green/black}▏ {percent:>3}% • {pos}/{len} • ⏱️ {elapsed_precise} (ETA: {eta}) • {msg}";

    pub const EXPLORE_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} ▕{bar:35.green/black}▏ {percent:>3}% • ⏱️ {elapsed_precise} • {msg}";

    pub const COMPACT_TEMPLATE: &str =
        "{prefix:.cyan} ▕{bar:30.green/black}▏ {percent:>3}% ({pos}/{len}) {msg:.dim}";

    pub const SPINNER_TEMPLATE: &str =
        "{spinner:.green} {prefix:.cyan.bold} • ⏱️ {elapsed_precise} • {msg}";
}


const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_DOTS: &[&str] = &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

static SPINNER_FRAME: AtomicU64 = AtomicU64::new(0);

pub fn spinner_frame() -> &'static str {
    let frame = SPINNER_FRAME.fetch_add(1, Ordering::Relaxed) as usize;
    SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
}

pub fn spinner_dots() -> &'static str {
    let frame = SPINNER_FRAME.fetch_add(1, Ordering::Relaxed) as usize;
    SPINNER_DOTS[frame % SPINNER_DOTS.len()]
}


#[derive(Clone, Copy)]
pub enum ProgressStyle {
    Classic,
    Modern,
    Gradient,
    Blocks,
}

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

pub fn render_colored_progress(progress: f64, width: usize) -> String {
    use colors::*;

    let bar = render_progress_bar(progress, width, ProgressStyle::Modern);
    let pct = (progress * 100.0) as u32;

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

    pub fn display(&self) {
        use colors::*;
        use symbols::*;

        let elapsed = self.start_time.elapsed().as_secs_f64();

        let (_size_icon, size_color) = if self.size_pct < 0.0 {
            (SAVE, BRIGHT_GREEN)
        } else {
            (WARNING, BRIGHT_YELLOW)
        };

        let ssim_str = self
            .ssim
            .map(|s| format!(" {}SSIM {:.4}{}", DIM, s, RESET))
            .unwrap_or_default();

        let best_str = self
            .best_crf
            .map(|b| format!(" {}Best: {:.1}{}", DIM, b, RESET))
            .unwrap_or_default();

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

    pub fn finish(&self, final_crf: f32, final_size_pct: f64, final_ssim: Option<f64>) {
        use colors::*;
        use symbols::*;

        let elapsed = self.start_time.elapsed().as_secs_f64();

        eprint!("\r\x1b[K");

        let (ssim_str, ssim_rating) = match final_ssim {
            Some(s) if s >= 0.99 => (format!("SSIM {:.4}", s), format!("{} Excellent", SUCCESS)),
            Some(s) if s >= 0.98 => (format!("SSIM {:.4}", s), format!("{} Very Good", SUCCESS)),
            Some(s) if s >= 0.95 => (format!("SSIM {:.4}", s), format!("{}  Good", CHECK)),
            Some(s) => (format!("SSIM {:.4}", s), format!("{}  Fair", WARNING)),
            None => (String::new(), String::new()),
        };

        let size_str = if final_size_pct < 0.0 {
            format!("{}{:+.1}%{} {}", BRIGHT_GREEN, final_size_pct, RESET, SAVE)
        } else {
            format!("{}{:+.1}%{}", BRIGHT_YELLOW, final_size_pct, RESET)
        };

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


pub fn print_result_box(title: &str, lines: &[&str]) {
    use colors::*;

    let max_width = lines
        .iter()
        .map(|l| strip_ansi(l).len())
        .max()
        .unwrap_or(40)
        .max(strip_ansi(title).len())
        .max(40);

    let box_width = max_width + 4;

    eprintln!("{}╭{}╮{}", CYAN, "─".repeat(box_width), RESET);

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

    eprintln!("{}├{}┤{}", CYAN, "─".repeat(box_width), RESET);

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

    eprintln!("{}╰{}╯{}", CYAN, "─".repeat(box_width), RESET);
}

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


pub fn print_stage(_icon: &str, title: &str) {
    use colors::*;
    eprintln!("{}📍{} {}{}{}", DIM, RESET, BOLD, title, RESET);
    let _ = io::stderr().flush();
}

pub fn print_substage(title: &str) {
    use colors::*;
    eprintln!("   {}{}•{} {}", DIM, colors::CYAN, RESET, title);
}


pub fn print_success(msg: &str) {
    use colors::*;
    eprintln!("{}{} {}{}", BRIGHT_GREEN, symbols::SUCCESS, msg, RESET);
}

pub fn print_warning(msg: &str) {
    use colors::*;
    eprintln!("{}{} {}{}", BRIGHT_YELLOW, symbols::WARNING, msg, RESET);
}

pub fn print_error(msg: &str) {
    use colors::*;
    eprintln!("{}{} {}{}", BRIGHT_RED, symbols::ERROR, msg, RESET);
}

pub fn print_info(msg: &str) {
    use colors::*;
    eprintln!("{}{} {}{}", BRIGHT_CYAN, symbols::INFO, msg, RESET);
}


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
