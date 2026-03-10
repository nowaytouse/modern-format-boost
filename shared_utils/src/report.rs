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
    use crate::modern_ui::colors::*;

    let reduction = if input_bytes > 0 {
        (1.0 - output_bytes as f64 / input_bytes as f64) * 100.0
    } else {
        0.0
    };

    println!();
    println!("{}╭────────────────────────────────────────────────────────────────────────────╮{}", MFB_BLUE, RESET);
    println!(
        "{}│{}  {}📊 {} Summary Report{}{}                                        {}│{}",
        MFB_BLUE, RESET, BOLD, operation_name, RESET, " ".repeat(46 - operation_name.len()), MFB_BLUE, RESET
    );
    println!("{}├────────────────────────────────────────────────────────────────────────────┤{}", MFB_BLUE, RESET);
    println!(
        "{}│{}  📁 Files Processed:    {}{:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_WHITE, result.total, RESET, MFB_BLUE, RESET
    );
    println!(
        "{}│{}  {}✅ Succeeded:{}{}          {}{:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, MFB_GREEN, RESET, " ".repeat(1), BRIGHT_GREEN, result.succeeded, RESET, MFB_BLUE, RESET
    );
    println!(
        "{}│{}  {}❌ Failed:{}{}             {}{:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_RED, RESET, " ".repeat(1), BRIGHT_RED, result.failed, RESET, MFB_BLUE, RESET
    );
    println!(
        "{}│{}  {}⏭️  Skipped:{}{}            {}{:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_YELLOW, RESET, " ".repeat(1), BRIGHT_YELLOW, result.skipped, RESET, MFB_BLUE, RESET
    );
    
    let rate_color = if result.success_rate() > 90.0 { BRIGHT_GREEN } else { BRIGHT_YELLOW };
    println!(
        "{}│{}  {}📈 Success Rate:{}{}       {}{:>9.1}%{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_CYAN, RESET, " ".repeat(1), rate_color, result.success_rate(), RESET, MFB_BLUE, RESET
    );
    println!("{}├────────────────────────────────────────────────────────────────────────────┤{}", MFB_BLUE, RESET);
    println!(
        "{}│{}  💾 Input Size:         {}{:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, DIM, format_bytes(input_bytes), RESET, MFB_BLUE, RESET
    );
    
    let out_color = if reduction > 0.0 { BRIGHT_GREEN } else { BRIGHT_YELLOW };
    println!(
        "{}│{}  💾 Output Size:        {}{:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, out_color, format_bytes(output_bytes), RESET, MFB_BLUE, RESET
    );
    println!(
        "{}│{}  📉 Size Reduction:     {}{:>9.1}%{}                                         {}│{}",
        MFB_BLUE, RESET, out_color, reduction, RESET, MFB_BLUE, RESET
    );
    println!("{}├────────────────────────────────────────────────────────────────────────────┤{}", MFB_BLUE, RESET);
    println!(
        "{}│{}  ⏱️  Total Time:         {}{:>10}{}                                         {}│{}",
        MFB_BLUE, RESET, BRIGHT_CYAN, format_duration(duration), RESET, MFB_BLUE, RESET
    );
    if result.total > 0 {
        let avg_time = duration.as_secs_f64() / result.total as f64;
        println!(
            "{}│{}  ⏱️  Avg Time/File:      {}{:>9.2}s{}                                         {}│{}",
            MFB_BLUE, RESET, DIM, avg_time, RESET, MFB_BLUE, RESET
        );
    } else {
        println!("{}│{}                                                                            {}│{}", MFB_BLUE, RESET, MFB_BLUE, RESET);
    }
    println!("{}╰────────────────────────────────────────────────────────────────────────────╯{}", MFB_BLUE, RESET);

    if !result.errors.is_empty() {
        println!();
        println!("{}❌ Errors encountered:{}", BRIGHT_RED, RESET);
        println!("{}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{}", BRIGHT_RED, RESET);
        for (path, error) in &result.errors {
            println!("   {}{} → {}{}", DIM, path.display(), RESET, error);
        }
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
        (passed as f64 / total as f64) * 100.0
    } else {
        100.0
    };

    println!();
    println!("╔══════════════════════════════════════════════╗");
    println!("║        🏥 Media Health Report                ║");
    println!("╠══════════════════════════════════════════════╣");
    println!("║  ✅ Passed:                        {:>6}  ║", passed);
    println!("║  ❌ Failed:                        {:>6}  ║", failed);
    println!("║  ⚠️  Warnings:                     {:>6}  ║", warnings);
    println!(
        "║  📊 Health Rate:                  {:>5.1}%  ║",
        health_rate
    );
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
        let expected_reduction = (1.0 - output as f64 / input as f64) * 100.0;
        assert!((expected_reduction - 50.0).abs() < 0.01);

        let input = 1000u64;
        let output = 250u64;
        let expected_reduction = (1.0 - output as f64 / input as f64) * 100.0;
        assert!((expected_reduction - 75.0).abs() < 0.01);

        let input = 1000u64;
        let output = 1000u64;
        let expected_reduction = (1.0 - output as f64 / input as f64) * 100.0;
        assert!((expected_reduction - 0.0).abs() < 0.01);

        let input = 500u64;
        let output = 1000u64;
        let expected_reduction = (1.0 - output as f64 / input as f64) * 100.0;
        assert!((expected_reduction - (-100.0)).abs() < 0.01);
    }

    #[test]
    fn test_health_rate_formula() {
        let passed = 10;
        let failed = 0;
        let warnings = 0;
        let total = passed + failed + warnings;
        let health_rate = if total > 0 {
            (passed as f64 / total as f64) * 100.0
        } else {
            100.0
        };
        assert!((health_rate - 100.0).abs() < 0.01);

        let passed = 5;
        let failed = 5;
        let warnings = 0;
        let total = passed + failed + warnings;
        let health_rate = (passed as f64 / total as f64) * 100.0;
        assert!((health_rate - 50.0).abs() < 0.01);

        let passed = 0;
        let failed = 0;
        let warnings = 0;
        let total = passed + failed + warnings;
        let health_rate = if total > 0 {
            (passed as f64 / total as f64) * 100.0
        } else {
            100.0
        };
        assert!((health_rate - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_strict_avg_time_calculation() {
        let total_files = 10usize;
        let duration = Duration::from_secs(100);
        let avg_time = duration.as_secs_f64() / total_files as f64;
        assert!(
            (avg_time - 10.0).abs() < 0.001,
            "STRICT: 100s / 10 files = 10s/file, got {}",
            avg_time
        );

        let total_files = 3usize;
        let duration = Duration::from_secs(9);
        let avg_time = duration.as_secs_f64() / total_files as f64;
        assert!(
            (avg_time - 3.0).abs() < 0.001,
            "STRICT: 9s / 3 files = 3s/file, got {}",
            avg_time
        );
    }
}
