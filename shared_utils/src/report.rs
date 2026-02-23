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
        (1.0 - output_bytes as f64 / input_bytes as f64) * 100.0
    } else {
        0.0
    };

    println!();
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!(
        "║                        📊 {} Summary Report                        ║",
        operation_name
    );
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!(
        "║  📁 Files Processed:    {:>10}                                         ║",
        result.total
    );
    println!(
        "║  ✅ Succeeded:          {:>10}                                         ║",
        result.succeeded
    );
    println!(
        "║  ❌ Failed:             {:>10}                                         ║",
        result.failed
    );
    println!(
        "║  ⏭️  Skipped:            {:>10}                                         ║",
        result.skipped
    );
    println!(
        "║  📈 Success Rate:       {:>9.1}%                                         ║",
        result.success_rate()
    );
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!(
        "║  💾 Input Size:         {:>10}                                         ║",
        format_bytes(input_bytes)
    );
    println!(
        "║  💾 Output Size:        {:>10}                                         ║",
        format_bytes(output_bytes)
    );
    println!(
        "║  📉 Size Reduction:     {:>9.1}%                                         ║",
        reduction
    );
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!(
        "║  ⏱️  Total Time:         {:>10}                                         ║",
        format_duration(duration)
    );
    if result.total > 0 {
        let avg_time = duration.as_secs_f64() / result.total as f64;
        println!(
            "║  ⏱️  Avg Time/File:      {:>9.2}s                                         ║",
            avg_time
        );
    }
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");

    if !result.errors.is_empty() {
        println!();
        println!("❌ Errors encountered:");
        println!(
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        );
        for (path, error) in &result.errors {
            println!("   {} → {}", path.display(), error);
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
