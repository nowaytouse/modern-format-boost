//! Rich-style terminal panels (zero extra deps; mirrors Python `rich.Panel` /
//! `Table`).

use crate::infra::process_stream::ProcessorStats;
use crate::infra::ui_tokens::{colors_enabled, pick_symbol};
use std::io::{self, IsTerminal, Write};

const BRAND: &str = "\x1b[38;5;39m"; // ~#43a0ff
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";
const WHITE: &str = "\x1b[97m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const GRAY: &str = "\x1b[90m";
const BORDER: &str = "\x1b[38;5;240m";

fn styled(text: &str, style: &str) -> String {
    if colors_enabled() {
        format!("{style}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn flush() {
    let _ = io::stdout().flush();
}

/// Clear terminal (ANSI).
pub fn clear_screen() {
    if colors_enabled() {
        print!("\x1B[2J\x1B[1;1H");
        flush();
    }
}

/// Brand banner panel (mirrors Python `draw_header`).
pub fn draw_banner(version: &str) {
    let title = format!("MODERN FORMAT BOOST v{version}");
    let width = 70usize;
    if colors_enabled() {
        println!();
        println!("{BORDER}╭{}╮{RESET}", "─".repeat(width.saturating_sub(2)));
        let pad = width.saturating_sub(title.len() + 2) / 2;
        println!(
            "{BORDER}│{RESET}{} {BOLD}{WHITE}{title}{RESET}{BORDER} │{RESET}",
            " ".repeat(pad)
        );
        println!("{BORDER}│{RESET}  {DIM}PREMIUM MEDIA OPTIMIZER{RESET}{BORDER}│{RESET}");
        println!(
            "{BORDER}│{RESET}  {GREEN}-{RESET} {DIM}No Data Loss{RESET}   {GREEN}-{RESET} \
             {DIM}Smart Conversion{RESET}   {GREEN}-{RESET} \
             {DIM}Auto-Repair{RESET}{BORDER}│{RESET}"
        );
        println!("{BORDER}╰{}╯{RESET}", "─".repeat(width.saturating_sub(2)));
    } else {
        println!("\n=== {title} ===");
        println!("PREMIUM MEDIA OPTIMIZER");
        println!("- No Data Loss | Smart Conversion | Auto-Repair");
    }
    println!(
        "   {} Always keep a backup of your original media before optimization.\n",
        styled("WARNING:", RED)
    );
    flush();
}

/// Section separator (mirrors Python `draw_separator`).
pub fn draw_separator(title: &str) {
    let hash = "#".repeat(50);
    if colors_enabled() {
        println!("{DIM}# {BOLD}{WHITE}{title}{RESET} {DIM}{hash}{RESET}\n",);
    } else {
        println!("# {title} {hash}\n");
    }
    flush();
}

/// Runtime configuration dashboard (mirrors Rich `Panel` + `Table` before
/// processing).
#[derive(Debug, Clone)]
pub struct RuntimeDashboard {
    pub target_path: String,
    pub mode_label: String,
    pub target_type: String,
    pub output_path: Option<String>,
    pub ultimate: bool,
    pub watch: bool,
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub disk_free_gb: Option<f64>,
}

pub fn print_runtime_panel(dashboard: &RuntimeDashboard) {
    let folder = pick_symbol("📁", "[PATH]");
    let launch = pick_symbol("🚀", "[MODE]");
    let target = pick_symbol("🎯", "[TYPE]");
    let temp = pick_symbol("🌡", "[CPU]");
    let stats = pick_symbol("📊", "[RAM]");

    let rows = [
        (
            format!("{folder} Target Path"),
            dashboard.target_path.clone(),
        ),
        (
            format!("{launch} Mode"),
            if dashboard.ultimate {
                format!("{} Ultimate", dashboard.mode_label)
            } else {
                dashboard.mode_label.clone()
            },
        ),
        (
            format!("{target} Target Type"),
            dashboard.target_type.clone(),
        ),
    ];

    if let Some(ref out) = dashboard.output_path {
        print_panel_table(
            "Runtime Configuration",
            &rows
                .iter()
                .chain(std::iter::once(&(
                    pick_symbol("📂", "[OUT]").to_string(),
                    out.clone(),
                )))
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
            dashboard.cpu_percent,
            dashboard.memory_percent,
            dashboard.disk_free_gb,
            temp,
            stats,
        );
    } else {
        print_panel_table(
            "Runtime Configuration",
            &rows
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
            dashboard.cpu_percent,
            dashboard.memory_percent,
            dashboard.disk_free_gb,
            temp,
            stats,
        );
    }

    if dashboard.watch {
        println!(
            "   {} Watch mode: enabled (debounced re-run)",
            pick_symbol("👁", "[WATCH]")
        );
    }
    println!();
    flush();
}

#[allow(clippy::too_many_arguments)]
fn print_panel_table(
    title: &str,
    rows: &[(&str, &str)],
    cpu_percent: Option<f64>,
    memory_percent: Option<f64>,
    disk_free_gb: Option<f64>,
    temp_icon: &str,
    stats_icon: &str,
) {
    if colors_enabled() {
        println!(
            "{BORDER}╭─ {GRAY}{title}{RESET}{BORDER} ─────────────────────────────────╮{RESET}"
        );
        for (key, value) in rows {
            println!("{BORDER}│{RESET} {DIM}{key:>22}{RESET}  {BRAND}{BOLD}{value}{RESET}");
        }
        if let Some(cpu) = cpu_percent {
            println!(
                "{BORDER}│{RESET} {DIM}{temp_icon}  CPU Load{RESET:>14}  \
                 {BRAND}{BOLD}{cpu:.0}%{RESET}"
            );
        }
        if let Some(mem) = memory_percent {
            println!(
                "{BORDER}│{RESET} {DIM}{stats_icon} RAM Usage{RESET:>13}  \
                 {BRAND}{BOLD}{mem:.0}%{RESET}"
            );
        }
        if let Some(disk) = disk_free_gb {
            println!(
                "{BORDER}│{RESET} {DIM}💾 Disk Free{RESET:>13}  {BRAND}{BOLD}{disk:.2} GB{RESET}"
            );
        }
        println!("{BORDER}╰──────────────────────────────────────────────────────────╯{RESET}");
    } else {
        println!("[{title}]");
        for (key, value) in rows {
            println!("  {key}: {value}");
        }
        if let Some(cpu) = cpu_percent {
            println!("  {temp_icon} CPU: {cpu:.0}%");
        }
        if let Some(mem) = memory_percent {
            println!("  {stats_icon} RAM: {mem:.0}%");
        }
        if let Some(disk) = disk_free_gb {
            println!("  Disk free: {disk:.2} GB");
        }
    }
}

/// Combined pipeline stats for summary table.
#[derive(Debug, Clone, Default)]
pub struct PipelineSummary {
    pub img: ProcessorStats,
    pub vid: ProcessorStats,
    pub integrity_state: Option<&'static str>,
    pub integrity_issue_count: usize,
    /// Parsed from fast-img `[SIZE]` lines (session-scoped source bytes).
    pub fast_img_session_source_bytes: Option<u64>,
    /// Parsed from fast-img `[SIZE]` lines (session-scoped output bytes).
    pub fast_img_session_output_bytes: Option<u64>,
    /// Post-delivery size override for drag summary when output dir is cleaned
    /// (Shortest Path).
    pub fast_img_size_after_override: Option<u64>,
    /// Names (relative) of files that failed conversion, for terminal and
    /// session-log enumeration. Each entry is "filename: reason".
    pub failed_file_names: Vec<String>,
    /// Names (relative) of files that were skipped, for terminal and
    /// session-log enumeration. Each entry is "filename: reason".
    pub skipped_file_names: Vec<String>,
}

impl PipelineSummary {
    #[must_use]
    pub fn total_succeeded(&self) -> usize {
        self.img.succeeded + self.vid.succeeded
    }

    #[must_use]
    pub fn total_failed(&self) -> usize {
        self.img.failed + self.vid.failed
    }

    #[must_use]
    pub fn total_skipped(&self) -> usize {
        self.img.skipped + self.vid.skipped
    }

    #[must_use]
    pub fn total_ignored(&self) -> usize {
        self.img.ignored + self.vid.ignored
    }

    #[must_use]
    pub fn has_image_stats(&self) -> bool {
        self.img.total() > 0 || self.img.exit_code != 0
    }

    #[must_use]
    pub fn has_video_stats(&self) -> bool {
        self.vid.total() > 0 || self.vid.exit_code != 0
    }
}

/// Optimization summary report (mirrors Python end-of-run Rich table + success
/// bar).
pub fn print_summary_report(summary: &PipelineSummary) {
    draw_separator("Task Completed");

    let effective_s = summary.total_succeeded();
    let effective_f = summary.total_failed();
    let rate_denominator = effective_s + effective_f;
    let success_rate = (effective_s * 100)
        .checked_div(rate_denominator)
        .unwrap_or(100);

    if colors_enabled() {
        println!("{BOLD}{GRAY}Optimization Summary Report{RESET}");
        println!(
            "{GRAY}{}  Succeeded  Skipped  Ignored  Failed{RESET}",
            "Type".to_string() + &" ".repeat(12)
        );
        if summary.has_image_stats() {
            print_summary_row(
                &format!("{} Images", pick_symbol("🖼️", "IMG")),
                &summary.img,
            );
        }
        if summary.has_video_stats() {
            print_summary_row(
                &format!("{} Videos", pick_symbol("🎬", "VID")),
                &summary.vid,
            );
        }
        println!("{}", styled(&"─".repeat(56), GRAY));
        print_summary_row(
            &format!("{} Total", pick_symbol("📦", "TOT")),
            &ProcessorStats {
                succeeded: effective_s,
                skipped: summary.total_skipped(),
                ignored: summary.total_ignored(),
                failed: effective_f,
                exit_code: 0,
            },
        );
        println!();
        if rate_denominator > 0 {
            let bar_len = 20usize;
            let filled = (success_rate * bar_len) / 100;
            let bar: String = "█".repeat(filled) + &"░".repeat(bar_len.saturating_sub(filled));
            let rate_color = if success_rate >= 90 {
                GREEN
            } else if success_rate >= 50 {
                YELLOW
            } else {
                RED
            };
            println!(
                "   {BOLD}{GRAY}Success Rate:{RESET} [{rate_color}]{bar}{RESET} {success_rate}%"
            );
        }
        if let Some(state) = summary.integrity_state {
            let color = if state == "WARNINGS" { YELLOW } else { GREEN };
            println!("   {BOLD}{GRAY}Integrity:{RESET} [{color}]{state}{RESET}");
        }
    } else {
        println!(
            "[Summary] succeeded={effective_s} failed={effective_f} skipped={} ignored={}",
            summary.total_skipped(),
            summary.total_ignored()
        );
        println!("Success rate: {success_rate}%");
    }
    println!();
    flush();
}

fn print_summary_row(label: &str, stats: &ProcessorStats) {
    println!(
        "  {label:<16} {GREEN}{}{RESET}  {YELLOW}{}{RESET}  {DIM}{}{RESET}  {RED}{}{RESET}",
        stats.succeeded, stats.skipped, stats.ignored, stats.failed
    );
}

/// Menu row for interactive TUI (mirrors Python `select_mode` highlighting).
pub fn print_menu_row(selected: bool, title: &str, description: &str) {
    if selected {
        if colors_enabled() {
            println!("  {BRAND}{BOLD}➜{RESET} {BRAND}{BOLD} {title} {RESET}");
            println!("     {CYAN}{description}{RESET}\n");
        } else {
            println!("> {title}");
            println!("  {description}\n");
        }
    } else if colors_enabled() {
        println!("     {DIM}○ {title}{RESET}");
        println!("     {DIM}{description}{RESET}\n");
    } else {
        println!("  - {title}");
        println!("    {description}\n");
    }
    flush();
}

pub fn print_menu_hint() {
    let hint = "(↑/↓ navigate · Tab cycle option · Enter select · q quit · 0-9 quick pick)";
    if colors_enabled() {
        println!("{DIM}{hint}{RESET}");
    } else {
        println!("{hint}");
    }
    flush();
}

/// Critical error panel (mirrors Python `stream_and_log_process` failure UX).
pub fn print_critical_error_panel(processor: &str, exit_code: i32) {
    println!();
    if colors_enabled() {
        println!(
            "{RED}{BOLD}╭──────────────────────────────────────────────────────────────╮{RESET}"
        );
        println!(
            "{RED}{BOLD}│  🚨 CRITICAL ERROR — '{processor}' exited with code {exit_code}  \
             │{RESET}"
        );
        println!(
            "{RED}{BOLD}╰──────────────────────────────────────────────────────────────╯{RESET}"
        );
        println!(
            "  {YELLOW}Review the terminal output above for the specific error message.{RESET}\n"
        );
    } else {
        println!(
            "[CRITICAL ERROR] The '{processor}' processor exited unexpectedly with code \
             {exit_code}."
        );
        println!("Review the terminal output above for the specific error message.\n");
    }
    flush();
}

/// Hold terminal open after GUI/double-click failures (mirrors Python keypress
/// wait).
pub fn pause_before_gui_exit() {
    let gui = match std::env::var("MFB_GUI_LAUNCH") {
        Ok(v) => !v.trim().is_empty() && v != "0",
        Err(err) => {
            let _ = err;
            false
        }
    };
    if gui && io::stdin().is_terminal() {
        let _ = write!(
            io::stdout(),
            "\nPress Enter to exit and close this window..."
        );
        let _ = io::stdout().flush();
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_summary_totals() {
        let mut s = PipelineSummary::default();
        s.img.succeeded = 3;
        s.vid.failed = 1;
        assert_eq!(s.total_succeeded(), 3);
        assert_eq!(s.total_failed(), 1);
    }
}
