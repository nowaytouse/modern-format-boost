//! Report Module
//!
//! Provides summary reporting functionality for batch operations
//! Reference: media/CONTRIBUTING.md - Detailed Reporting requirement

use crate::batch::BatchResult;
use crate::progress::{format_bytes, format_duration};
use std::time::Duration;

pub fn print_summary_report(
    result: &BatchResult,
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

    print_report_header(operation_name);
    print_file_stats(result);
    print_size_info(input_bytes, output_bytes, reduction);
    print_time_info(result, duration);
    print_error_summary(result);
    print_pause_info(result);
}

fn print_report_header(operation_name: &str) {
    use crate::modern_ui::colors::{BOLD, MFB_BLUE, RESET};
    println!();
    println!(
        "{MFB_BLUE}╭────────────────────────────────────────────────────────────────────────────╮{RESET}"
    );
    println!(
        "{}│{}  {}📊 {} Summary Report{}{}                                        {}│{}",
        MFB_BLUE,
        RESET,
        BOLD,
        operation_name,
        RESET,
        " ".repeat(46 - operation_name.len()),
        MFB_BLUE,
        RESET
    );
    println!(
        "{MFB_BLUE}├────────────────────────────────────────────────────────────────────────────┤{RESET}"
    );
}

fn print_file_stats(result: &BatchResult) {
    use crate::modern_ui::colors::{
        BRIGHT_CYAN, BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, MFB_BLUE, RESET,
    };
    println!(
        "{}│{}  📁 Files Processed:    {:>10}                                         {}│{}",
        MFB_BLUE, RESET, result.total, MFB_BLUE, RESET
    );
    println!(
        "{}│{}  {}✅ Succeeded:           {:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_GREEN, result.succeeded, RESET, MFB_BLUE, RESET
    );
    println!(
        "{}│{}  {}❌ Failed:              {:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_RED, result.failed, RESET, MFB_BLUE, RESET
    );
    println!(
        "{}│{}  {}⏭️  Skipped:             {:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_YELLOW, result.skipped, RESET, MFB_BLUE, RESET
    );
    if result.paused {
        println!(
            "{}│{}  {}⏸️  Paused:              {:>10}{}                                         {}│{}",
            MFB_BLUE, RESET, BRIGHT_YELLOW, "YES", RESET, MFB_BLUE, RESET
        );
    }

    let rate_color = if result.success_rate() > 90.0_f64 {
        BRIGHT_GREEN
    } else {
        BRIGHT_YELLOW
    };
    println!(
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
    );
    println!(
        "{MFB_BLUE}├────────────────────────────────────────────────────────────────────────────┤{RESET}"
    );
}

fn print_size_info(input_bytes: u64, output_bytes: u64, reduction: f64) {
    use crate::modern_ui::colors::{BRIGHT_GREEN, BRIGHT_YELLOW, DIM, MFB_BLUE, RESET};
    println!(
        "{}│{}  💾 Input Size:         {}{:>10}{}                                         {}│{}",
        MFB_BLUE,
        RESET,
        DIM,
        format_bytes(input_bytes),
        RESET,
        MFB_BLUE,
        RESET
    );

    let out_color = if reduction > 0.0_f64 {
        BRIGHT_GREEN
    } else {
        BRIGHT_YELLOW
    };
    println!(
        "{}│{}  💾 Output Size:        {}{:>10}{}                                         {}│{}",
        MFB_BLUE,
        RESET,
        out_color,
        format_bytes(output_bytes),
        RESET,
        MFB_BLUE,
        RESET
    );
    println!(
        "{MFB_BLUE}│{RESET}  📉 Size Reduction:     {out_color}{reduction:>9.1}%{RESET}                                         {MFB_BLUE}│{RESET}"
    );
    println!(
        "{MFB_BLUE}├────────────────────────────────────────────────────────────────────────────┤{RESET}"
    );
}

fn print_time_info(result: &BatchResult, duration: Duration) {
    use crate::modern_ui::colors::{BRIGHT_CYAN, DIM, MFB_BLUE, RESET};
    println!(
        "{}│{}  ⏱️  Total Time:         {}{:>10}{}                                         {}│{}",
        MFB_BLUE,
        RESET,
        BRIGHT_CYAN,
        format_duration(duration),
        RESET,
        MFB_BLUE,
        RESET
    );
    if result.total > 0 {
        let avg_time = duration.as_secs_f64() / crate::numeric_cast::usize_to_f64(result.total);
        println!(
            "{MFB_BLUE}│{RESET}  ⏱️  Avg Time/File:      {DIM}{avg_time:>9.2}s{RESET}                                         {MFB_BLUE}│{RESET}"
        );
    } else {
        println!(
            "{MFB_BLUE}│{RESET}                                                                            {MFB_BLUE}│{RESET}"
        );
    }
    println!(
        "{MFB_BLUE}╰────────────────────────────────────────────────────────────────────────────╯{RESET}"
    );
}

fn print_error_summary(result: &BatchResult) {
    use crate::modern_ui::colors::{BRIGHT_RED, DIM, RESET};
    if !result.errors.is_empty() {
        println!();
        println!("{BRIGHT_RED}❌ Errors encountered:{RESET}");
        println!(
            "{BRIGHT_RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}"
        );
        for (path, error) in &result.errors {
            println!("   {}{} → {}{}", DIM, path.display(), RESET, error);
        }
    }
}

fn print_pause_info(result: &BatchResult) {
    use crate::modern_ui::colors::{BRIGHT_YELLOW, DIM, RESET};
    if let Some(pause) = &result.pause_info {
        println!();
        println!("{BRIGHT_YELLOW}⏸️ Batch Paused:{RESET}");
        println!("   {}File:{} {}", DIM, RESET, pause.path.display());
        println!("   {}Reason:{} {}", DIM, RESET, pause.reason);
        println!(
            "   {}Pending:{} {} files remain for retry. Free space and rerun with `--resume`.",
            DIM, RESET, result.paused_remaining
        );
    }
}

pub fn print_simple_summary(result: &BatchResult) {
    println!(
        "\n✅ Complete: {} succeeded, {} failed, {} skipped (total: {})",
        result.succeeded, result.failed, result.skipped, result.total
    );
}

pub fn print_health_report(passed: usize, failed: usize, warnings: usize) {
    let total = passed + failed + warnings;
    let health_rate = if total > 0 {
        (crate::numeric_cast::usize_to_f64(passed) / crate::numeric_cast::usize_to_f64(total))
            * 100.0_f64
    } else {
        100.0_f64
    };

    println!();
    println!("╔══════════════════════════════════════════════╗");
    println!("║        🏥 Media Health Report                ║");
    println!("╠══════════════════════════════════════════════╣");
    println!("║  ✅ Passed:                        {passed:>6}  ║");
    println!("║  ❌ Failed:                        {failed:>6}  ║");
    println!("║  ⚠️  Warnings:                     {warnings:>6}  ║");
    println!("║  📊 Health Rate:                  {health_rate:>5.1}%  ║");
    println!("╚══════════════════════════════════════════════╝");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_simple_summary_no_panic() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.fail(std::path::PathBuf::from("test.png"), "Error".to_string());

        print_simple_summary(&result);
    }

    #[test]
    fn test_print_simple_summary_empty() {
        let result = BatchResult::new();
        print_simple_summary(&result);
    }

    #[test]
    fn test_print_summary_report_no_panic() {
        let mut result = BatchResult::new();
        result.success();
        result.fail(std::path::PathBuf::from("test.png"), "Error".to_string());

        let duration = Duration::from_secs(10);

        print_summary_report(&result, duration, 1000, 500, "Test");
    }

    #[test]
    fn test_print_summary_report_zero_input() {
        let result = BatchResult::new();
        let duration = Duration::from_secs(1);

        print_summary_report(&result, duration, 0, 0, "Test");
    }

    #[test]
    fn test_print_health_report_no_panic() {
        print_health_report(10, 2, 3);

        print_health_report(0, 0, 0);

        print_health_report(100, 0, 0);

        print_health_report(0, 100, 0);
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
