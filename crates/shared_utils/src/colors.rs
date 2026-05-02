//! 🎨 v5.67: Terminal Color Support Module
//!
//! Provides unified colored output to improve UI/UX experience.

use console::{style, Style};


pub fn success() -> Style {
    Style::new().green().bold()
}

pub fn error() -> Style {
    Style::new().red().bold()
}

pub fn warning() -> Style {
    Style::new().yellow()
}

pub fn info() -> Style {
    Style::new().cyan()
}

pub fn highlight() -> Style {
    Style::new().magenta().bold()
}

pub fn number() -> Style {
    Style::new().blue().bold()
}

pub fn dim() -> Style {
    Style::new().dim()
}


pub fn fmt_crf(crf: f32) -> String {
    format!("{}", style(format!("CRF {:.1}", crf)).cyan().bold())
}

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

pub fn fmt_size_pct(pct: f64) -> String {
    if pct < 0.0 {
        format!("{}", style(format!("{:+.1}%", pct)).green().bold())
    } else if pct < 5.0 {
        format!("{}", style(format!("{:+.1}%", pct)).yellow())
    } else {
        format!("{}", style(format!("{:+.1}%", pct)).red())
    }
}

pub fn fmt_compress_status(compressed: bool) -> &'static str {
    if compressed { "✅" } else { "❌" }
}

pub fn fmt_size(bytes: u64) -> String {
    let (value, unit) = if bytes >= 1024 * 1024 * 1024 {
        (shared_utils::numeric_cast::u64_to_f64(bytes) / 1024.0 / 1024.0 / 1024.0, "GB")
    } else if bytes >= 1024 * 1024 {
        (shared_utils::numeric_cast::u64_to_f64(bytes) / 1024.0 / 1024.0, "MB")
    } else if bytes >= 1024 {
        (shared_utils::numeric_cast::u64_to_f64(bytes) / 1024.0, "KB")
    } else {
        (shared_utils::numeric_cast::u64_to_f64(bytes), "B")
    };
    format!("{}", style(format!("{:.2} {}", value, unit)).blue())
}

pub fn fmt_duration(secs: f64) -> String {
    if secs >= 60.0 {
        let mins = (secs / 60.0).floor();
        let remaining = secs - mins * 60.0;
        format!("{}", style(format!("{:.0}m {:.1}s", mins, remaining)).cyan())
    } else {
        format!("{}", style(format!("{:.1}s", secs)).cyan())
    }
}

pub fn fmt_iterations(iter: u32, max: u32) -> String {
    let ratio = f64::from(iter) / f64::from(max);
    if ratio <= 0.5 {
        format!("{}", style(format!("{}/{}", iter, max)).green())
    } else if ratio <= 0.8 {
        format!("{}", style(format!("{}/{}", iter, max)).yellow())
    } else {
        format!("{}", style(format!("{}/{}", iter, max)).red())
    }
}


pub fn print_header(title: &str) {
    eprintln!("{}", style(format!("═══ {} ═══", title)).cyan().bold());
}

pub fn print_separator() {
    eprintln!("{}", style("─────────────────────────────────────────────").dim());
}

pub fn print_success(msg: &str) {
    eprintln!("{} {}", style("✅").green(), style(msg).green().bold());
}

pub fn print_error(msg: &str) {
    eprintln!("{} {}", style("❌").red(), style(msg).red().bold());
}

pub fn print_warning(msg: &str) {
    eprintln!("{} {}", style("⚠️").yellow(), style(msg).yellow());
}

pub fn print_info(msg: &str) {
    eprintln!("{} {}", style("ℹ️").cyan(), style(msg).cyan());
}


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

pub fn fmt_final_result(crf: f32, size_pct: f64, ssim: Option<f64>, iterations: u32) -> String {
    let ssim_str = ssim.map(|s| fmt_ssim(s)).unwrap_or_else(|| "---".to_string());
    format!("{} {} | {} | {} | {} iterations",
        style("RESULT:").green().bold(),
        fmt_crf(crf),
        fmt_size_pct(size_pct),
        ssim_str,
        style(iterations).cyan())
}
