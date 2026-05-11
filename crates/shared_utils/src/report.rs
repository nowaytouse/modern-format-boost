//! Report Module
//!
//! Provides summary reporting functionality for batch operations
//! Reference: media/CONTRIBUTING.md - Detailed Reporting requirement

use crate::batch::Summary;
use crate::progress::{format_bytes, format_duration};
use std::time::Duration;

pub fn print_summary(
    result: &Summary,
    duration: Duration,
    input_bytes: u64,
    output_bytes: u64,
    operation_name: &str,
) {
    let reduction = if input_bytes > 0 {
        (1.0_f64
            - crate::numeric_cast::u64_to_f64(output_bytes)
                / crate::numeric_cast::u64_to_f64(input_bytes))
            * 100.0_f64
    } else {
        0.0_f64
    };

    let reduction_str = format!("{reduction:.1}%");
    crate::log_info!(
        crate::static_logs::messages::LABEL_REPORT,
        &format!(
            "SUMMARY: {operation_name} | Files: {total} (Succ:{succ}, Fail:{fail}, Skip:{skip}) | Size: {in_b} -> {out_b} ({reduction_str} reduction) | Time: {dur}",
            total = result.total,
            succ = result.succeeded,
            fail = result.failed,
            skip = result.skipped,
            in_b = format_bytes(input_bytes),
            out_b = format_bytes(output_bytes),
            dur = format_duration(duration)
        )
    );

    print_report_header(operation_name);
    print_file_stats(result);
    print_size_info(input_bytes, output_bytes, reduction);
    print_time_info(result, duration);
    print_error_summary(result);
    print_pause_info(result);
}

fn print_report_header(operation_name: &str) {
    use crate::modern_ui::colors::{BOLD, MFB_BLUE, RESET};
    crate::progress_mode::emit_stderr("");
    crate::progress_mode::emit_stderr(&format!(
        "{MFB_BLUE}╭────────────────────────────────────────────────────────────────────────────╮{RESET}"
    ));
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  {}📊 {} Summary Report{}{}                                        {}│{}",
        MFB_BLUE,
        RESET,
        BOLD,
        operation_name,
        RESET,
        " ".repeat(46 - operation_name.len()),
        MFB_BLUE,
        RESET
    ));
    crate::progress_mode::emit_stderr(&format!(
        "{MFB_BLUE}├────────────────────────────────────────────────────────────────────────────┤{RESET}"
    ));
}

fn print_file_stats(result: &Summary) {
    use crate::modern_ui::colors::{
        BRIGHT_CYAN, BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, MFB_BLUE, RESET,
    };
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  📁 Files Processed:    {:>10}                                         {}│{}",
        MFB_BLUE, RESET, result.total, MFB_BLUE, RESET
    ));
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  {}✅ Succeeded:           {:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_GREEN, result.succeeded, RESET, MFB_BLUE, RESET
    ));
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  {}❌ Failed:              {:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_RED, result.failed, RESET, MFB_BLUE, RESET
    ));
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  {}⏭️  Skipped:             {:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_YELLOW, result.skipped, RESET, MFB_BLUE, RESET
    ));
    if result.paused {
        crate::progress_mode::emit_stderr(&format!(
            "{}│{}  {}⏸️  Paused:              {:>10}{}                                         {}│{}",
            MFB_BLUE, RESET, BRIGHT_YELLOW, "YES", RESET, MFB_BLUE, RESET
        ));
    }

    let rate_color = if result.success_rate() > 90.0_f64 {
        BRIGHT_GREEN
    } else {
        BRIGHT_YELLOW
    };
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  {}📈 Success Rate:{}        {}{:>9.1}%{}                                         {}│{}",
        MFB_BLUE,
        RESET,
        BRIGHT_CYAN,
        RESET,
        rate_color,
        result.success_rate(),
        RESET,
        MFB_BLUE,
        RESET
    ));
    crate::progress_mode::emit_stderr(&format!(
        "{MFB_BLUE}├────────────────────────────────────────────────────────────────────────────┤{RESET}"
    ));
}

fn print_size_info(input_bytes: u64, output_bytes: u64, reduction: f64) {
    use crate::modern_ui::colors::{BRIGHT_GREEN, BRIGHT_YELLOW, DIM, MFB_BLUE, RESET};
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  💾 Input Size:         {}{:>10}{}                                         {}│{}",
        MFB_BLUE,
        RESET,
        DIM,
        format_bytes(input_bytes),
        RESET,
        MFB_BLUE,
        RESET
    ));

    let out_color = if reduction > 0.0_f64 {
        BRIGHT_GREEN
    } else {
        BRIGHT_YELLOW
    };
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  💾 Output Size:        {}{:>10}{}                                         {}│{}",
        MFB_BLUE,
        RESET,
        out_color,
        format_bytes(output_bytes),
        RESET,
        MFB_BLUE,
        RESET
    ));
    crate::progress_mode::emit_stderr(&format!(
        "{MFB_BLUE}│{RESET}  📉 Size Reduction:     {out_color}{reduction:>9.1}%{RESET}                                         {MFB_BLUE}│{RESET}"
    ));
    crate::progress_mode::emit_stderr(&format!(
        "{MFB_BLUE}├────────────────────────────────────────────────────────────────────────────┤{RESET}"
    ));
}

fn print_time_info(result: &Summary, duration: Duration) {
    use crate::modern_ui::colors::{BRIGHT_CYAN, DIM, MFB_BLUE, RESET};
    crate::progress_mode::emit_stderr(&format!(
        "{}│{}  ⏱️  Total Time:         {}{:>10}{}                                         {}│{}",
        MFB_BLUE,
        RESET,
        BRIGHT_CYAN,
        format_duration(duration),
        RESET,
        MFB_BLUE,
        RESET
    ));
    if result.total > 0 {
        let avg_time = duration.as_secs_f64() / crate::numeric_cast::usize_to_f64(result.total);
        crate::progress_mode::emit_stderr(&format!(
            "{MFB_BLUE}│{RESET}  ⏱️  Avg Time/File:      {DIM}{avg_time:>9.2}s{RESET}                                         {MFB_BLUE}│{RESET}"
        ));
    } else {
        crate::progress_mode::emit_stderr(&format!(
            "{MFB_BLUE}│{RESET}                                                                            {MFB_BLUE}│{RESET}"
        ));
    }
    crate::progress_mode::emit_stderr(&format!(
        "{MFB_BLUE}╰────────────────────────────────────────────────────────────────────────────╯{RESET}"
    ));
}

fn print_error_summary(result: &Summary) {
    use crate::modern_ui::colors::{BRIGHT_RED, DIM, RESET};
    if !result.errors.is_empty() {
        crate::progress_mode::emit_stderr("");
        crate::progress_mode::emit_stderr(&format!("{BRIGHT_RED}❌ Errors encountered:{RESET}"));
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
        crate::progress_mode::emit_stderr("");
        crate::progress_mode::emit_stderr(&format!("{BRIGHT_YELLOW}⏸️ Batch Paused:{RESET}"));
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
        crate::static_logs::messages::LABEL_REPORT,
        &format!(
            "Complete: {succ} succeeded, {fail} failed, {skip} skipped (total: {total})",
            succ = result.succeeded,
            fail = result.failed,
            skip = result.skipped,
            total = result.total
        )
    );
    crate::progress_mode::emit_stderr(&format!(
        "\n✅ Complete: {} succeeded, {} failed, {} skipped (total: {})",
        result.succeeded, result.failed, result.skipped, result.total
    ));
}

pub fn print_health(passed: usize, failed: usize, warnings: usize) {
    let total = passed + failed + warnings;
    let health_rate = if total > 0 {
        (crate::numeric_cast::usize_to_f64(passed) / crate::numeric_cast::usize_to_f64(total))
            * 100.0_f64
    } else {
        100.0_f64
    };

    crate::log_info!(
        crate::static_logs::messages::LABEL_REPORT,
        &format!(
            "Health: {health_rate:.1}% (Passed:{passed}, Failed:{failed}, Warnings:{warnings})"
        )
    );
    crate::progress_mode::emit_stderr("");
    crate::progress_mode::emit_stderr("╔══════════════════════════════════════════════╗");
    crate::progress_mode::emit_stderr("║        🏥 Media Health Report                ║");
    crate::progress_mode::emit_stderr("╠══════════════════════════════════════════════╣");
    crate::progress_mode::emit_stderr(&format!(
        "║  ✅ Passed:                        {passed:>6}  ║"
    ));
    crate::progress_mode::emit_stderr(&format!(
        "║  ❌ Failed:                        {failed:>6}  ║"
    ));
    crate::progress_mode::emit_stderr(&format!(
        "║  ⚠️  Warnings:                     {warnings:>6}  ║"
    ));
    crate::progress_mode::emit_stderr(&format!(
        "║  📊 Health Rate:                  {health_rate:>5.1}%  ║"
    ));
    crate::progress_mode::emit_stderr("╚══════════════════════════════════════════════╝");
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
    fn test_print_summary_zero_input() {
        let result = Summary::new();
        let duration = Duration::from_secs(1);

        print_summary(&result, duration, 0, 0, "Test");
    }

    #[test]
    fn test_print_health_no_panic() {
        print_health(10, 2, 3);

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
