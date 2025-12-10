//! Report Module
//! 
//! Provides summary reporting functionality for batch operations
//! Reference: media/CONTRIBUTING.md - Detailed Reporting requirement

use crate::batch::BatchResult;
use crate::progress::{format_bytes, format_duration};
use std::time::Duration;

/// Print a detailed summary report after batch processing
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
    println!("║                        📊 {} Summary Report                        ║", operation_name);
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!("║  📁 Files Processed:    {:>10}                                         ║", result.total);
    println!("║  ✅ Succeeded:          {:>10}                                         ║", result.succeeded);
    println!("║  ❌ Failed:             {:>10}                                         ║", result.failed);
    println!("║  ⏭️  Skipped:            {:>10}                                         ║", result.skipped);
    println!("║  📈 Success Rate:       {:>9.1}%                                         ║", result.success_rate());
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!("║  💾 Input Size:         {:>10}                                         ║", format_bytes(input_bytes));
    println!("║  💾 Output Size:        {:>10}                                         ║", format_bytes(output_bytes));
    println!("║  📉 Size Reduction:     {:>9.1}%                                         ║", reduction);
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!("║  ⏱️  Total Time:         {:>10}                                         ║", format_duration(duration));
    if result.total > 0 {
        let avg_time = duration.as_secs_f64() / result.total as f64;
        println!("║  ⏱️  Avg Time/File:      {:>9.2}s                                         ║", avg_time);
    }
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");

    // Print errors if any
    if !result.errors.is_empty() {
        println!();
        println!("❌ Errors encountered:");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        for (path, error) in &result.errors {
            println!("   {} → {}", path.display(), error);
        }
    }
}

/// Print a simple one-line summary
pub fn print_simple_summary(result: &BatchResult) {
    println!(
        "\n✅ Complete: {} succeeded, {} failed, {} skipped (total: {})",
        result.succeeded, result.failed, result.skipped, result.total
    );
}

/// Print health check report
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
    println!("║  📊 Health Rate:                  {:>5.1}%  ║", health_rate);
    println!("╚══════════════════════════════════════════════╝");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_simple_summary() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.fail(std::path::PathBuf::from("test.png"), "Error".to_string());
        
        // Just verify it doesn't panic
        print_simple_summary(&result);
    }
}
