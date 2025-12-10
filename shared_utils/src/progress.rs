//! Progress Bar Module
//! 
//! Provides visual progress feedback with ETA estimation
//! Reference: media/CONTRIBUTING.md - Visual Progress Bar requirement

use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use std::time::{Duration, Instant};

/// Create a styled progress bar for batch processing with improved ETA
/// 
/// # Example
/// ```
/// use shared_utils::create_progress_bar;
/// let pb = create_progress_bar(100, "Converting");
/// for _ in 0..100 {
///     pb.inc(1);
/// }
/// pb.finish_with_message("Done!");
/// ```
pub fn create_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            // 使用 elapsed_precise 替代 eta，更可靠
            .template("{prefix:.cyan.bold} [{bar:40.green/dim}] {pos}/{len} ({percent}%) | {elapsed_precise} | {msg}")
            .expect("Invalid progress bar template")
            .progress_chars("█▓░")
    );
    pb.set_prefix(prefix.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// Create a progress bar with custom ETA calculation (for variable-time tasks)
pub fn create_progress_bar_with_eta(total: u64, prefix: &str) -> SmartProgressBar {
    SmartProgressBar::new(total, prefix)
}

/// Smart progress bar with moving average ETA calculation
/// Better for tasks with highly variable processing times (like media conversion)
pub struct SmartProgressBar {
    bar: ProgressBar,
    start_time: Instant,
    total: u64,
    processed: u64,
    /// Moving average of last N processing times (in seconds)
    recent_times: Vec<f64>,
    last_update: Instant,
}

impl SmartProgressBar {
    pub fn new(total: u64, prefix: &str) -> Self {
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::default_bar()
                .template("{prefix:.cyan.bold} [{bar:40.green/dim}] {pos}/{len} ({percent}%) | ETA: {msg}")
                .expect("Invalid progress bar template")
                .progress_chars("█▓░")
        );
        bar.set_prefix(prefix.to_string());
        bar.enable_steady_tick(Duration::from_millis(100));
        
        Self {
            bar,
            start_time: Instant::now(),
            total,
            processed: 0,
            recent_times: Vec::with_capacity(10),
            last_update: Instant::now(),
        }
    }
    
    /// Increment progress and update ETA
    pub fn inc(&mut self, message: &str) {
        let elapsed = self.last_update.elapsed().as_secs_f64();
        self.last_update = Instant::now();
        
        // Keep only last 10 times for moving average
        if self.recent_times.len() >= 10 {
            self.recent_times.remove(0);
        }
        self.recent_times.push(elapsed);
        
        self.processed += 1;
        self.bar.inc(1);
        
        // Calculate ETA using moving average
        let remaining = self.total.saturating_sub(self.processed);
        let eta = if !self.recent_times.is_empty() && remaining > 0 {
            let avg_time: f64 = self.recent_times.iter().sum::<f64>() / self.recent_times.len() as f64;
            let eta_secs = avg_time * remaining as f64;
            format_eta(eta_secs)
        } else {
            "calculating...".to_string()
        };
        
        self.bar.set_message(format!("{} | {}", eta, message));
    }
    
    pub fn finish(&self) {
        let total_time = self.start_time.elapsed();
        self.bar.finish_with_message(format!("Done in {}", format_duration(total_time)));
    }
    
    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

/// Format ETA with reasonable limits (cap at 24h, show "very long" for extreme values)
fn format_eta(seconds: f64) -> String {
    if seconds.is_nan() || seconds.is_infinite() || seconds < 0.0 {
        return "unknown".to_string();
    }
    
    let secs = seconds as u64;
    
    // Cap at 24 hours - anything longer is unreliable
    if secs > 86400 {
        return ">24h".to_string();
    }
    
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Create a spinner for indeterminate progress
pub fn create_spinner(message: &str) -> ProgressBar {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("Invalid spinner template")
    );
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(Duration::from_millis(80));
    spinner
}

/// Create a multi-progress container for parallel operations
pub fn create_multi_progress() -> MultiProgress {
    MultiProgress::new()
}

/// Progress tracker for batch operations with statistics
pub struct BatchProgress {
    pub total: u64,
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub skipped: u64,
    bar: ProgressBar,
}

impl BatchProgress {
    pub fn new(total: u64, prefix: &str) -> Self {
        Self {
            total,
            processed: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            bar: create_progress_bar(total, prefix),
        }
    }

    pub fn success(&mut self, message: &str) {
        self.processed += 1;
        self.succeeded += 1;
        self.bar.set_message(format!("✅ {}", message));
        self.bar.inc(1);
    }

    pub fn fail(&mut self, message: &str) {
        self.processed += 1;
        self.failed += 1;
        self.bar.set_message(format!("❌ {}", message));
        self.bar.inc(1);
    }

    pub fn skip(&mut self, message: &str) {
        self.processed += 1;
        self.skipped += 1;
        self.bar.set_message(format!("⏭️  {}", message));
        self.bar.inc(1);
    }

    pub fn finish(&self) {
        self.bar.finish_with_message(format!(
            "Complete: {} succeeded, {} failed, {} skipped",
            self.succeeded, self.failed, self.skipped
        ));
    }

    /// Get the underlying progress bar for custom operations
    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

/// Format bytes to human-readable string
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format duration to human-readable string
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m 1s");
    }
}
