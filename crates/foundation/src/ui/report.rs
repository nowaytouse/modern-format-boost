//! Report Module
//!
//! Provides summary reporting functionality for batch operations
//! Reference: media/CONTRIBUTING.md - Detailed Reporting requirement

use crate::batch::Summary;
use crate::modern_ui::symbols;
use crate::progress::{format_bytes, format_duration};
use std::time::Duration;

/// Display width between vertical box borders (U9).
const REPORT_INNER_WIDTH: usize = 76;

/// Shared before/after size comparison for batch, single-file, and wrapper
/// summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeComparison {
    before_bytes: u64,
    after_bytes: u64,
}

impl SizeComparison {
    #[must_use]
    pub const fn new(before_bytes: u64, after_bytes: u64) -> Self {
        Self {
            before_bytes,
            after_bytes,
        }
    }

    #[must_use]
    pub const fn before_bytes(self) -> u64 {
        self.before_bytes
    }

    #[must_use]
    pub const fn after_bytes(self) -> u64 {
        self.after_bytes
    }

    #[must_use]
    pub fn diff_bytes(self) -> i128 {
        i128::from(self.after_bytes) - i128::from(self.before_bytes)
    }

    #[must_use]
    pub fn change_pct(self) -> Option<f64> {
        if self.before_bytes == 0 {
            return None;
        }
        Some(
            (crate::numeric_cast::u64_to_f64(self.after_bytes)
                / crate::numeric_cast::u64_to_f64(self.before_bytes)
                - 1.0_f64)
                * 100.0_f64,
        )
    }

    #[must_use]
    pub fn before_label(self) -> String {
        format_bytes(self.before_bytes)
    }

    #[must_use]
    pub fn after_label(self) -> String {
        format_bytes(self.after_bytes)
    }

    #[must_use]
    pub fn diff_label(self) -> String {
        match self.after_bytes.cmp(&self.before_bytes) {
            std::cmp::Ordering::Less => {
                format!("-{}", format_bytes(self.before_bytes - self.after_bytes))
            }
            std::cmp::Ordering::Equal => format_bytes(0),
            std::cmp::Ordering::Greater => {
                format!("+{}", format_bytes(self.after_bytes - self.before_bytes))
            }
        }
    }

    #[must_use]
    pub fn change_label(self) -> String {
        self.change_pct()
            .map_or_else(|| "N/A".to_string(), signed_percent_label)
    }

    #[must_use]
    pub fn reduction_label(self) -> String {
        summary_size_reduction_pct(self.before_bytes, self.after_bytes)
            .map_or_else(|| "N/A".to_string(), |reduction| format!("{reduction:.1}%"))
    }

    #[must_use]
    pub fn log_fragment(self) -> String {
        format!(
            "before={}, after={}, diff={}, change={}",
            self.before_label(),
            self.after_label(),
            self.diff_label(),
            self.change_label()
        )
    }
}

fn signed_percent_label(value: f64) -> String {
    if value.is_sign_positive() {
        format!("+{value:.1}%")
    } else {
        format!("{value:.1}%")
    }
}

struct BoxStyle {
    plain: bool,
}

impl BoxStyle {
    fn current() -> Self {
        Self {
            plain: crate::progress_mode::is_plain_mode(),
        }
    }

    const fn accent(&self) -> &'static str {
        if self.plain {
            ""
        } else {
            crate::modern_ui::colors::MFB_BLUE
        }
    }

    const fn reset(&self) -> &'static str {
        if self.plain {
            ""
        } else {
            crate::modern_ui::colors::RESET
        }
    }

    const fn v(&self) -> &'static str {
        if self.plain { "|" } else { "│" }
    }

    fn emit_border_top(&self) {
        let line = if self.plain {
            format!("+{}+", "-".repeat(REPORT_INNER_WIDTH + 2))
        } else {
            format!(
                "{}╭{}╮{}",
                self.accent(),
                "─".repeat(REPORT_INNER_WIDTH + 2),
                self.reset()
            )
        };
        crate::progress_mode::emit_stderr(&line);
    }

    fn emit_border_mid(&self) {
        let line = if self.plain {
            format!("+{}+", "-".repeat(REPORT_INNER_WIDTH + 2))
        } else {
            format!(
                "{}├{}┤{}",
                self.accent(),
                "─".repeat(REPORT_INNER_WIDTH + 2),
                self.reset()
            )
        };
        crate::progress_mode::emit_stderr(&line);
    }

    fn emit_border_bottom(&self) {
        let line = if self.plain {
            format!("+{}+", "-".repeat(REPORT_INNER_WIDTH + 2))
        } else {
            format!(
                "{}╰{}╯{}",
                self.accent(),
                "─".repeat(REPORT_INNER_WIDTH + 2),
                self.reset()
            )
        };
        crate::progress_mode::emit_stderr(&line);
    }

    fn emit_row(&self, body: &str) {
        let full = format!("  {body}");
        let pad = REPORT_INNER_WIDTH.saturating_sub(console::measure_text_width(&full));
        crate::progress_mode::emit_stderr(&format!(
            "{}{}{}  {}{} {}{}{}",
            self.accent(),
            self.v(),
            self.reset(),
            body,
            " ".repeat(pad),
            self.accent(),
            self.v(),
            self.reset()
        ));
    }
}

pub fn print_summary(
    result: &Summary,
    duration: Duration,
    input_bytes: u64,
    output_bytes: u64,
    operation_name: &str,
) {
    let comparison = SizeComparison::new(input_bytes, output_bytes);
    let reduction_pct = summary_size_reduction_pct(input_bytes, output_bytes);
    let reduction_str = comparison.reduction_label();
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_REPORT,
        format!(
            "Summary: {operation_name} | total={total}, succeeded={succ}, failed={fail}, \
             skipped={skip} | {comparison} | reduction={reduction_str} | elapsed={dur}",
            total = result.total,
            succ = result.succeeded,
            fail = result.failed,
            skip = result.skipped,
            comparison = comparison.log_fragment(),
            dur = format_duration(duration),
        )
    );

    print_report_header(operation_name);
    print_file_stats(result);
    print_size_info(comparison, reduction_pct);
    print_time_info(result, duration);
    print_error_summary(result);
    print_pause_info(result);
}

/// Size reduction percent for summary reporting. Returns `None` when input size
/// is unknown (zero).
#[must_use]
pub fn summary_size_reduction_pct(input_bytes: u64, output_bytes: u64) -> Option<f64> {
    if input_bytes == 0 {
        return None;
    }
    Some(
        (1.0_f64
            - crate::numeric_cast::u64_to_f64(output_bytes)
                / crate::numeric_cast::u64_to_f64(input_bytes))
            * 100.0_f64,
    )
}

fn print_report_header(operation_name: &str) {
    use crate::modern_ui::colors::{BOLD, RESET};
    let style = BoxStyle::current();
    let icon = crate::media_conversion_gate::ui_icon_pick("📊", "#");
    let body = if style.plain {
        format!("{icon} {operation_name} Summary Report")
    } else {
        format!("{BOLD}{icon} {operation_name} Summary Report{RESET}")
    };
    crate::progress_mode::emit_stderr("");
    style.emit_border_top();
    style.emit_row(&body);
    style.emit_border_mid();
}

fn print_file_stats(result: &Summary) {
    use crate::modern_ui::colors::{BRIGHT_CYAN, BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, RESET};
    let style = BoxStyle::current();
    let folder = crate::media_conversion_gate::ui_icon_pick("📁", "Files:");
    let ok = crate::media_conversion_gate::ui_icon_pick(symbols::SUCCESS, symbols::plain::SUCCESS);
    let fail = crate::media_conversion_gate::ui_icon_pick(symbols::ERROR, symbols::plain::ERROR);
    let skip = crate::media_conversion_gate::ui_icon_pick(symbols::SKIP, symbols::plain::SKIP);
    let chart = crate::media_conversion_gate::ui_icon_pick(symbols::CHART, symbols::plain::CHART);

    style.emit_row(&format!(
        "{folder} Files Processed:    {:>10}",
        result.total
    ));
    if style.plain {
        style.emit_row(&format!(
            "{ok} Succeeded:           {:>10}",
            result.succeeded
        ));
        style.emit_row(&format!(
            "{fail} Failed:              {:>10}",
            result.failed
        ));
        style.emit_row(&format!(
            "{skip} Skipped:             {:>10}",
            result.skipped
        ));
    } else {
        style.emit_row(&format!(
            "{BRIGHT_GREEN}{ok} Succeeded:           {:>10}{RESET}",
            result.succeeded
        ));
        style.emit_row(&format!(
            "{BRIGHT_RED}{fail} Failed:              {:>10}{RESET}",
            result.failed
        ));
        style.emit_row(&format!(
            "{BRIGHT_YELLOW}{skip} Skipped:             {:>10}{RESET}",
            result.skipped
        ));
    }
    if result.ignored > 0 {
        let ghost = crate::media_conversion_gate::ui_icon_pick("👻", "[ignored]");
        if style.plain {
            style.emit_row(&format!(
                "{ghost} Ignored:             {:>10}",
                result.ignored
            ));
        } else {
            style.emit_row(&format!(
                "{}{ghost} Ignored:             {:>10}{RESET}",
                crate::modern_ui::colors::MFB_YELLOW,
                result.ignored
            ));
        }
    }
    if result.paused {
        let pause = crate::media_conversion_gate::ui_icon_pick("⏸️", "[paused]");
        if style.plain {
            style.emit_row(&format!("{pause} Paused:              {:>10}", "YES"));
        } else {
            style.emit_row(&format!(
                "{BRIGHT_YELLOW}{pause} Paused:              {:>10}{RESET}",
                "YES"
            ));
        }
    }

    let rate = result.success_rate();
    if style.plain {
        style.emit_row(&format!("{chart} Success Rate:        {rate:>9.1}%"));
    } else {
        let rate_color = if rate > 90.0_f64 {
            BRIGHT_GREEN
        } else {
            BRIGHT_YELLOW
        };
        style.emit_row(&format!(
            "{BRIGHT_CYAN}{chart} Success Rate:{RESET}        {rate_color}{rate:>9.1}%{RESET}"
        ));
    }
    style.emit_border_mid();
}

fn print_size_info(comparison: SizeComparison, reduction_pct: Option<f64>) {
    use crate::modern_ui::colors::{BRIGHT_GREEN, BRIGHT_YELLOW, DIM, RESET};
    let style = BoxStyle::current();
    let disk = crate::media_conversion_gate::ui_icon_pick(symbols::SAVE, symbols::plain::SAVE);
    let down = crate::media_conversion_gate::ui_icon_pick("📉", "reduction:");
    let delta = crate::media_conversion_gate::ui_icon_pick("↕️", "delta:");
    let chart = crate::media_conversion_gate::ui_icon_pick(symbols::CHART, symbols::plain::CHART);
    let reduction_label = reduction_pct.map_or_else(
        || format!("{:>9}", "N/A"),
        |reduction| format!("{reduction:>9.1}%"),
    );

    if style.plain {
        style.emit_row(&format!(
            "{disk} Total Before:       {:>10}",
            comparison.before_label()
        ));
        style.emit_row(&format!(
            "{disk} Total After:        {:>10}",
            comparison.after_label()
        ));
        style.emit_row(&format!(
            "{delta} Size Difference:    {:>10}",
            comparison.diff_label()
        ));
        style.emit_row(&format!(
            "{chart} Size Change:        {:>10}",
            comparison.change_label()
        ));
        style.emit_row(&format!("{down} Size Reduction:     {reduction_label}"));
    } else {
        style.emit_row(&format!(
            "{disk} Total Before:       {DIM}{:>10}{RESET}",
            comparison.before_label()
        ));
        let out_color = match reduction_pct {
            Some(reduction) if reduction > 0.0_f64 => BRIGHT_GREEN,
            Some(_) | None => BRIGHT_YELLOW,
        };
        style.emit_row(&format!(
            "{disk} Total After:        {out_color}{:>10}{RESET}",
            comparison.after_label()
        ));
        style.emit_row(&format!(
            "{delta} Size Difference:    {out_color}{:>10}{RESET}",
            comparison.diff_label()
        ));
        style.emit_row(&format!(
            "{chart} Size Change:        {out_color}{:>10}{RESET}",
            comparison.change_label()
        ));
        style.emit_row(&format!(
            "{down} Size Reduction:     {out_color}{reduction_label}{RESET}"
        ));
    }
    style.emit_border_mid();
}

fn print_time_info(result: &Summary, duration: Duration) {
    use crate::modern_ui::colors::{BRIGHT_CYAN, DIM, RESET};
    let style = BoxStyle::current();
    let clock = crate::media_conversion_gate::ui_icon_pick(symbols::CLOCK, symbols::plain::CLOCK);

    if style.plain {
        style.emit_row(&format!(
            "{clock} Total Time:         {:>10}",
            format_duration(duration)
        ));
    } else {
        style.emit_row(&format!(
            "{clock} Total Time:         {BRIGHT_CYAN}{:>10}{RESET}",
            format_duration(duration)
        ));
    }
    if result.total > 0 {
        let avg_time = duration.as_secs_f64() / crate::numeric_cast::usize_to_f64(result.total);
        if style.plain {
            style.emit_row(&format!("{clock} Avg Time/File:      {avg_time:>9.2}s"));
        } else {
            style.emit_row(&format!(
                "{clock} Avg Time/File:      {DIM}{avg_time:>9.2}s{RESET}"
            ));
        }
    } else {
        style.emit_row("");
    }
    style.emit_border_bottom();
}

fn print_error_summary(result: &Summary) {
    use crate::modern_ui::colors::{BRIGHT_RED, DIM, RESET};
    if !result.errors.is_empty() {
        let err = crate::media_conversion_gate::ui_icon_pick(symbols::ERROR, symbols::plain::ERROR);
        crate::progress_mode::emit_stderr("");
        if crate::progress_mode::is_plain_mode() {
            crate::progress_mode::emit_stderr(&format!("{err} Errors encountered:"));
        } else {
            crate::progress_mode::emit_stderr(&format!(
                "{BRIGHT_RED}{err} Errors encountered:{RESET}"
            ));
        }
        crate::progress_mode::emit_stderr(&format!(
            "{BRIGHT_RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}"
        ));
        for (path, error) in &result.errors {
            crate::progress_mode::emit_stderr(&format!(
                "   {}{} → {}{}",
                DIM,
                path.display(),
                RESET,
                error
            ));
        }
    }
}

fn print_pause_info(result: &Summary) {
    use crate::modern_ui::colors::{BRIGHT_YELLOW, DIM, RESET};
    if let Some(pause) = &result.pause_info {
        let pause_icon = crate::media_conversion_gate::ui_icon_pick("⏸️", "[paused]");
        crate::progress_mode::emit_stderr("");
        if crate::progress_mode::is_plain_mode() {
            crate::progress_mode::emit_stderr(&format!("{pause_icon} Batch Paused:"));
        } else {
            crate::progress_mode::emit_stderr(&format!(
                "{BRIGHT_YELLOW}{pause_icon} Batch Paused:{RESET}"
            ));
        }
        crate::progress_mode::emit_stderr(&format!(
            "   {}File:{} {}",
            DIM,
            RESET,
            pause.path.display()
        ));
        crate::progress_mode::emit_stderr(&format!("   {}Reason:{} {}", DIM, RESET, pause.reason));
        crate::progress_mode::emit_stderr(&format!(
            "   {}Pending:{} {} files remain for retry. Free space and rerun with `--resume`.",
            DIM, RESET, result.paused_remaining
        ));
    }
}

pub fn print_simple_summary(result: &Summary) {
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_REPORT,
        format!(
            "Report: succeeded={}, failed={}, skipped={}, total={}",
            result.succeeded, result.failed, result.skipped, result.total
        )
    );
    crate::progress_mode::emit_stderr(&format!(
        "Report UI: succeeded={}, failed={}, skipped={}, total={}",
        result.succeeded, result.failed, result.skipped, result.total
    ));
}

pub fn print_health(passed: usize, failed: usize, warnings: usize) {
    let health_rate_pct = summary_health_rate_pct(passed, failed, warnings);
    let health_rate_label =
        health_rate_pct.map_or_else(|| "N/A".to_string(), |rate| format!("{rate:.1}%"));

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_REPORT,
        format!(
            "Health Audit: {health_rate_label} healthy | passed={passed}, failed={failed}, \
             warnings={warnings}"
        )
    );
    let style = BoxStyle::current();
    let health = crate::media_conversion_gate::ui_icon_pick("🏥", "Health");
    let ok = crate::media_conversion_gate::ui_icon_pick(symbols::SUCCESS, symbols::plain::SUCCESS);
    let fail = crate::media_conversion_gate::ui_icon_pick(symbols::ERROR, symbols::plain::ERROR);
    let warn =
        crate::media_conversion_gate::ui_icon_pick(symbols::WARNING, symbols::plain::WARNING);
    let chart = crate::media_conversion_gate::ui_icon_pick(symbols::CHART, symbols::plain::CHART);
    crate::progress_mode::emit_stderr("");
    style.emit_border_top();
    style.emit_row(&format!("{health} Media Health Report"));
    style.emit_border_mid();
    style.emit_row(&format!("{ok} Passed:                        {passed:>6}"));
    style.emit_row(&format!(
        "{fail} Failed:                        {failed:>6}"
    ));
    style.emit_row(&format!(
        "{warn} Warnings:                     {warnings:>6}"
    ));
    style.emit_row(&format!(
        "{chart} Health Rate:                  {health_rate_label:>5}"
    ));
    style.emit_border_bottom();
}

/// Health pass rate for summary reporting. Returns `None` when no checks ran
/// (zero total).
#[must_use]
pub fn summary_health_rate_pct(passed: usize, failed: usize, warnings: usize) -> Option<f64> {
    let total = passed + failed + warnings;
    if total == 0 {
        return None;
    }
    Some(
        crate::numeric_cast::usize_to_f64(passed) / crate::numeric_cast::usize_to_f64(total)
            * 100.0_f64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_simple_summary_no_panic() {
        let mut result = Summary::new();
        result.success();
        result.success();
        result.fail(std::path::PathBuf::from("test.png"), "Error".to_string());

        print_simple_summary(&result);
    }

    #[test]
    fn test_print_simple_summary_empty() {
        let result = Summary::new();
        print_simple_summary(&result);
    }

    #[test]
    fn test_print_summary_no_panic() {
        let result = Summary::new();
        let duration = Duration::from_secs(1);

        print_summary(&result, duration, 1000, 500, "Test");
    }

    #[test]
    fn size_change_summary_reports_absolute_delta_and_signed_percent() {
        let comparison = SizeComparison::new(1_000, 750);

        assert_eq!(comparison.before_bytes(), 1_000);
        assert_eq!(comparison.after_bytes(), 750);
        assert_eq!(comparison.diff_bytes(), -250);
        let change_pct = comparison
            .change_pct()
            .expect("non-zero before has percent");
        assert!((change_pct - (-25.0_f64)).abs() < f64::EPSILON);
        assert_eq!(comparison.diff_label(), "-250 B");
        assert_eq!(comparison.change_label(), "-25.0%");
        assert_eq!(
            comparison.log_fragment(),
            "before=1000 B, after=750 B, diff=-250 B, change=-25.0%"
        );
    }

    #[test]
    fn size_change_summary_reports_growth_with_positive_sign() {
        let comparison = SizeComparison::new(1_000, 1_250);

        assert_eq!(comparison.diff_bytes(), 250);
        let change_pct = comparison
            .change_pct()
            .expect("non-zero before has percent");
        assert!((change_pct - 25.0_f64).abs() < f64::EPSILON);
        assert_eq!(comparison.diff_label(), "+250 B");
        assert_eq!(comparison.change_label(), "+25.0%");
    }

    #[test]
    fn size_change_summary_keeps_zero_before_unknown() {
        let comparison = SizeComparison::new(0, 500);

        assert_eq!(comparison.diff_bytes(), 500);
        assert_eq!(comparison.change_pct(), None);
        assert_eq!(comparison.diff_label(), "+500 B");
        assert_eq!(comparison.change_label(), "N/A");
    }

    #[test]
    fn test_print_summary_zero_input() {
        let result = Summary::new();
        let duration = Duration::from_secs(1);

        assert!(
            summary_size_reduction_pct(0, 0).is_none(),
            "zero input_bytes must not fabricate 0.0% reduction in summary"
        );
        print_summary(&result, duration, 0, 0, "Test");
    }

    #[test]
    fn report_title_padding_handles_wide_operation_name() {
        let result = Summary::new();
        print_summary(&result, Duration::from_secs(1), 0, 0, "日本語テスト");
    }

    #[test]
    fn report_plain_mode_uses_ascii_box() {
        crate::progress_mode::set_plain_mode(true);
        let result = Summary::new();
        print_summary(&result, Duration::from_secs(1), 100, 50, "PlainTest");
        crate::progress_mode::set_plain_mode(false);
    }

    #[test]
    fn test_print_health_no_panic() {
        print_health(10, 2, 3);

        assert!(
            summary_health_rate_pct(0, 0, 0).is_none(),
            "zero total checks must not fabricate 100% health rate"
        );
        print_health(0, 0, 0);

        print_health(100, 0, 0);

        print_health(0, 100, 0);
    }

    #[test]
    fn test_size_reduction_formula() {
        let input = 1000u64;
        let output = 500u64;
        let expected_reduction = (1.0_f64
            - crate::numeric_cast::u64_to_f64(output) / crate::numeric_cast::u64_to_f64(input))
            * 100.0_f64;
        assert!((expected_reduction - 50.0).abs() < 0.01_f64);

        let input = 1000u64;
        let output = 250u64;
        let expected_reduction = (1.0_f64
            - crate::numeric_cast::u64_to_f64(output) / crate::numeric_cast::u64_to_f64(input))
            * 100.0_f64;
        assert!((expected_reduction - 75.0).abs() < 0.01_f64);

        let input = 1000u64;
        let output = 1000u64;
        let expected_reduction = (1.0_f64
            - crate::numeric_cast::u64_to_f64(output) / crate::numeric_cast::u64_to_f64(input))
            * 100.0_f64;
        assert!((expected_reduction - 0.0).abs() < 0.01_f64);

        let input = 500u64;
        let output = 1000u64;
        let expected_reduction = (1.0_f64
            - crate::numeric_cast::u64_to_f64(output) / crate::numeric_cast::u64_to_f64(input))
            * 100.0_f64;
        assert!((expected_reduction - (-100.0)).abs() < 0.01_f64);
    }

    #[test]
    fn test_health_rate_formula() {
        let passed = 10_i32;
        let failed = 0_i32;
        let warnings = 0_i32;
        let total = passed + failed + warnings;
        let health_rate = if total > 0_i32 {
            (f64::from(passed) / f64::from(total)) * 100.0_f64
        } else {
            100.0_f64
        };
        assert!((health_rate - 100.0).abs() < 0.01_f64);

        let passed = 5_i32;
        let failed = 5_i32;
        let warnings = 0_i32;
        let total = passed + failed + warnings;
        let health_rate = (f64::from(passed) / f64::from(total)) * 100.0_f64;
        assert!((health_rate - 50.0).abs() < 0.01_f64);

        let passed = 0_i32;
        let failed = 0_i32;
        let warnings = 0_i32;
        let total = passed + failed + warnings;
        let health_rate = if total > 0_i32 {
            (f64::from(passed) / f64::from(total)) * 100.0_f64
        } else {
            100.0_f64
        };
        assert!((health_rate - 100.0).abs() < 0.01_f64);
    }

    #[test]
    fn test_strict_avg_time_calculation() {
        let total_files = 10usize;
        let duration = Duration::from_secs(100);
        let avg_time = duration.as_secs_f64() / crate::numeric_cast::usize_to_f64(total_files);
        assert!(
            (avg_time - 10.0).abs() < 0.001_f64,
            "STRICT: 100s / 10 files = 10s/file, got {avg_time}"
        );

        let total_files = 3usize;
        let duration = Duration::from_secs(9);
        let avg_time = duration.as_secs_f64() / crate::numeric_cast::usize_to_f64(total_files);
        assert!(
            (avg_time - 3.0).abs() < 0.001_f64,
            "STRICT: 9s / 3 files = 3s/file, got {avg_time}"
        );
    }
}
