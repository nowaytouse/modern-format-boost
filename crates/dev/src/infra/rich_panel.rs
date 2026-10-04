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
    pub img_events: usize,
    pub vid_events: usize,
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
    pub fn add_image_stats(&mut self, stats: &ProcessorStats) {
        add_processor_stats(&mut self.img, stats, self.img_events == 0);
        self.img_events = self.img_events.saturating_add(1);
    }

    pub fn add_video_stats(&mut self, stats: &ProcessorStats) {
        add_processor_stats(&mut self.vid, stats, self.vid_events == 0);
        self.vid_events = self.vid_events.saturating_add(1);
    }

    #[must_use]
    pub fn total_succeeded(&self) -> usize {
        self.img.succeeded.saturating_add(self.vid.succeeded)
    }

    #[must_use]
    pub fn total_failed(&self) -> usize {
        self.img.failed.saturating_add(self.vid.failed)
    }

    #[must_use]
    pub fn total_skipped(&self) -> usize {
        self.img.skipped.saturating_add(self.vid.skipped)
    }

    #[must_use]
    pub fn total_ignored(&self) -> usize {
        self.img.ignored.saturating_add(self.vid.ignored)
    }

    #[must_use]
    pub fn total_count(&self, index: usize) -> Option<usize> {
        let (img_seen, img_value, vid_seen, vid_value) = match index {
            0 => (
                self.image_seen(),
                self.img.succeeded,
                self.video_seen(),
                self.vid.succeeded,
            ),
            1 => (
                self.image_seen(),
                self.img.skipped,
                self.video_seen(),
                self.vid.skipped,
            ),
            2 => (
                self.image_seen(),
                self.img.ignored,
                self.video_seen(),
                self.vid.ignored,
            ),
            3 => (
                self.image_seen(),
                self.img.failed,
                self.video_seen(),
                self.vid.failed,
            ),
            _ => return None,
        };
        let img = if img_seen {
            self.img.reported[index].then_some(img_value)
        } else {
            Some(0)
        }?;
        let vid = if vid_seen {
            self.vid.reported[index].then_some(vid_value)
        } else {
            Some(0)
        }?;
        img.checked_add(vid)
    }

    #[must_use]
    pub fn total_unprocessed(&self) -> Option<usize> {
        let img = if self.image_seen() {
            self.img.unprocessed
        } else {
            Some(0)
        }?;
        let vid = if self.video_seen() {
            self.vid.unprocessed
        } else {
            Some(0)
        }?;
        img.checked_add(vid)
    }

    #[must_use]
    pub fn success_rate_percent(&self) -> Option<usize> {
        let succeeded = self.total_count(0)?;
        let failed = self.total_count(3)?;
        let attempts = succeeded.checked_add(failed)?;
        if attempts == 0 {
            return None;
        }
        let widen = |value: usize| match u128::try_from(value) {
            Ok(value) => Some(value),
            Err(error) => {
                eprintln!("[SUMMARY] Cannot widen count {value} for percentage: {error}");
                None
            }
        };
        let percentage = (widen(succeeded)? * 100) / widen(attempts)?;
        match usize::try_from(percentage) {
            Ok(value) => Some(value),
            Err(error) => {
                eprintln!("[SUMMARY] Cannot represent success percentage {percentage}: {error}");
                None
            }
        }
    }

    #[must_use]
    pub fn has_image_stats(&self) -> bool {
        self.image_seen() || self.img.total() > 0 || self.img.exit_code != 0
    }

    #[must_use]
    pub fn has_video_stats(&self) -> bool {
        self.video_seen() || self.vid.total() > 0 || self.vid.exit_code != 0
    }

    fn image_seen(&self) -> bool {
        self.img_events > 0
            || self.img.reported.iter().any(|reported| *reported)
            || self.img.exit_code != 0
    }

    fn video_seen(&self) -> bool {
        self.vid_events > 0
            || self.vid.reported.iter().any(|reported| *reported)
            || self.vid.exit_code != 0
    }
}

fn add_processor_stats(total: &mut ProcessorStats, next: &ProcessorStats, first: bool) {
    if first {
        *total = next.clone();
        return;
    }
    let left = [total.succeeded, total.skipped, total.ignored, total.failed];
    let right = [next.succeeded, next.skipped, next.ignored, next.failed];
    for index in 0..4 {
        if total.reported[index]
            && next.reported[index]
            && let Some(sum) = left[index].checked_add(right[index])
        {
            set_stat_value(total, index, sum);
            continue;
        }
        set_stat_value(total, index, 0);
        total.reported[index] = false;
    }
    total.unprocessed = total
        .unprocessed
        .zip(next.unprocessed)
        .and_then(|(left, right)| left.checked_add(right));
    total.unprocessed_invalid |= next.unprocessed_invalid;
    if next.exit_code != 0 {
        total.exit_code = next.exit_code;
    }
}

fn set_stat_value(stats: &mut ProcessorStats, index: usize, value: usize) {
    match index {
        0 => stats.succeeded = value,
        1 => stats.skipped = value,
        2 => stats.ignored = value,
        3 => stats.failed = value,
        _ => unreachable!(),
    }
}

fn totals_as_stats(summary: &PipelineSummary) -> ProcessorStats {
    let mut stats = ProcessorStats {
        unprocessed: summary.total_unprocessed(),
        ..ProcessorStats::default()
    };
    for index in 0..4 {
        if let Some(value) = summary.total_count(index) {
            set_stat_value(&mut stats, index, value);
            stats.reported[index] = true;
        }
    }
    stats
}

/// Optimization summary report (mirrors Python end-of-run Rich table + success
/// bar).
pub fn print_summary_report(summary: &PipelineSummary) {
    draw_separator("Task Completed");

    let rate = summary.success_rate_percent();

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
            &totals_as_stats(summary),
        );
        println!();
        if let Some(success_rate) = rate {
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
        } else {
            println!("   {BOLD}{GRAY}Success Rate:{RESET} N/A");
        }
        if let Some(state) = summary.integrity_state {
            let color = if state == "WARNINGS" { YELLOW } else { GREEN };
            println!("   {BOLD}{GRAY}Integrity:{RESET} [{color}]{state}{RESET}");
        }
    } else {
        println!(
            "[Summary] succeeded={} failed={} skipped={} ignored={}",
            summary
                .total_count(0)
                .map_or_else(|| "?".to_owned(), |n| n.to_string()),
            summary
                .total_count(3)
                .map_or_else(|| "?".to_owned(), |n| n.to_string()),
            summary
                .total_count(1)
                .map_or_else(|| "?".to_owned(), |n| n.to_string()),
            summary
                .total_count(2)
                .map_or_else(|| "?".to_owned(), |n| n.to_string())
        );
        println!(
            "Success rate: {}",
            rate.map_or_else(|| "N/A".to_owned(), |n| format!("{n}%"))
        );
    }
    println!(
        "Unprocessed: {}",
        summary
            .total_unprocessed()
            .map_or_else(|| "?".to_owned(), |n| n.to_string())
    );
    println!("Counts summarize processor outcomes, not unique files or Photos assets.");
    println!();
    flush();
}

fn print_summary_row(label: &str, stats: &ProcessorStats) {
    let count = |index: usize, value: usize| {
        if stats.reported[index] {
            value.to_string()
        } else {
            "?".to_owned()
        }
    };
    println!(
        "  {label:<16} {GREEN}{}{RESET}  {YELLOW}{}{RESET}  {DIM}{}{RESET}  {RED}{}{RESET}",
        count(0, stats.succeeded),
        count(1, stats.skipped),
        count(2, stats.ignored),
        count(3, stats.failed)
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

/// Summarize a propagated pipeline error without inventing a child exit code.
pub fn print_pipeline_failure_panel() {
    println!("\n{}", render_pipeline_failure_panel(colors_enabled()));
    flush();
}

fn render_pipeline_failure_panel(colored: bool) -> String {
    let detail = "The batch did not complete. Review the error details above.";
    if colored {
        format!(
            "{RED}{BOLD}╭──────────────────────────────────────────────────────────────╮{RESET}\n\
             {RED}{BOLD}│  PROCESSING FAILED                                           │{RESET}\n\
             {RED}{BOLD}╰──────────────────────────────────────────────────────────────╯{RESET}\n\
               {YELLOW}{detail}{RESET}\n"
        )
    } else {
        format!("[PROCESSING FAILED] {detail}\n")
    }
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
    fn pipeline_failure_panel_does_not_invent_a_processor_crash_or_exit_code() {
        for colored in [false, true] {
            let panel = render_pipeline_failure_panel(colored);
            assert!(panel.contains("PROCESSING FAILED"));
            assert!(panel.contains("batch did not complete"));
            assert!(panel.contains("error details above"));
            assert!(!panel.contains("unexpectedly"));
            assert!(!panel.contains("exited"));
            assert!(!panel.contains("code 1"));
        }
    }

    #[test]
    fn pipeline_summary_totals() {
        let mut s = PipelineSummary::default();
        s.img.succeeded = 3;
        s.vid.failed = 1;
        assert_eq!(s.total_succeeded(), 3);
        assert_eq!(s.total_failed(), 1);
    }

    #[test]
    fn pipeline_aggregation_preserves_unknown_overflow_and_failure_status() {
        let mut summary = PipelineSummary::default();
        let first = ProcessorStats {
            succeeded: 4,
            failed: 1,
            reported: [true; 4],
            unprocessed: Some(2),
            exit_code: 1,
            ..ProcessorStats::default()
        };
        summary.add_image_stats(&first);
        assert_eq!(summary.total_count(0), Some(4));
        assert_eq!(summary.total_unprocessed(), Some(2));
        assert_eq!(summary.success_rate_percent(), Some(80));
        let next = ProcessorStats {
            succeeded: usize::MAX,
            reported: [true, false, true, true],
            unprocessed: None,
            ..ProcessorStats::default()
        };
        summary.add_image_stats(&next);
        assert_eq!(summary.total_count(0), None);
        assert_eq!(summary.total_count(1), None);
        assert_eq!(summary.total_unprocessed(), None);
        assert_eq!(summary.success_rate_percent(), None);
        assert_eq!(summary.img.exit_code, 1);
    }

    #[test]
    fn no_attempts_have_no_rate_and_large_counts_do_not_overflow_percent_math() {
        let mut summary = PipelineSummary::default();
        let mut stats = ProcessorStats {
            skipped: 10,
            reported: [true; 4],
            unprocessed: Some(0),
            ..ProcessorStats::default()
        };
        summary.add_image_stats(&stats);
        assert_eq!(summary.success_rate_percent(), None);
        stats.skipped = 0;
        stats.succeeded = usize::MAX;
        let mut large = PipelineSummary::default();
        large.add_image_stats(&stats);
        assert_eq!(large.success_rate_percent(), Some(100));
    }
}
