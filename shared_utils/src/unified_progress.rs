//! 🔥 v8.0: Unified Progress Bar System
//!
//! Provides a consistent experience for both batch processing and video exploration.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub mod templates {
    pub const BATCH: &str = "{spinner:.green} {prefix:.cyan.bold} ▕{bar:35.green/black}▏ {percent:>3}% • {pos}/{len} • ⏱️ {elapsed_precise} • {msg}";
    pub const EXPLORE: &str = "{spinner:.green} {prefix:.cyan.bold} ▕{bar:35.green/black}▏ {percent:>3}% • ⏱️ {elapsed_precise} • {msg}";
    pub const PROGRESS_CHARS: &str = "█▓░";
    pub const SPINNER_CHARS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
}

pub struct UnifiedProgressBar {
    pub bar: ProgressBar,
    input_size: u64,
    current_iteration: AtomicU64,
    is_finished: AtomicBool,
}

impl UnifiedProgressBar {
    pub fn new(total: u64, message: &str) -> Arc<Self> {
        let bar = ProgressBar::new(total);
        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(templates::BATCH)
                    .expect("Invalid template")
                    .progress_chars(templates::PROGRESS_CHARS)
                    .tick_chars(templates::SPINNER_CHARS),
            );
            bar.set_prefix(message.to_string());
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
        }
        Arc::new(Self {
            bar,
            input_size: 0,
            current_iteration: AtomicU64::new(0),
            is_finished: AtomicBool::new(false),
        })
    }

    pub fn new_iteration(message: &str, input_size: u64, total_iterations: u64) -> Arc<Self> {
        let bar = ProgressBar::new(total_iterations);
        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(templates::EXPLORE)
                    .expect("Invalid template")
                    .progress_chars(templates::PROGRESS_CHARS)
                    .tick_chars(templates::SPINNER_CHARS),
            );
            bar.set_prefix(message.to_string());
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(100));
        }
        Arc::new(Self {
            bar,
            input_size,
            current_iteration: AtomicU64::new(0),
            is_finished: AtomicBool::new(false),
        })
    }

    pub fn inc(&self) {
        self.bar.inc(1);
    }
    pub fn set_position(&self, pos: u64) {
        self.bar.set_position(pos);
    }
    pub fn set_message(&self, msg: impl Into<String>) {
        self.bar.set_message(msg.into());
    }
    pub fn println(&self, msg: &str) {
        self.bar.suspend(|| eprintln!("{}", msg));
    }

    pub fn inc_iteration(&self, crf: f32, size: u64, ssim: Option<f64>) {
        let iter = self.current_iteration.fetch_add(1, Ordering::Relaxed) + 1;
        self.bar.set_position(iter);
        let size_pct = if self.input_size > 0 {
            ((size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };
        let ssim_str = ssim
            .map(|s| format!("SSIM {:.4}", s))
            .unwrap_or_else(|| "N/A".to_string());
        self.bar
            .set_message(format!("CRF {:.1} | {:+.1}% | {}", crf, size_pct, ssim_str));
    }

    pub fn finish_iteration(&self, final_crf: f32, final_size: u64, final_ssim: Option<f64>) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }
        let size_pct = if self.input_size > 0 {
            ((final_size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };
        let ssim_str = final_ssim
            .map(|s| format!("SSIM {:.4}", s))
            .unwrap_or_default();
        self.bar.finish_with_message(format!(
            "✅ CRF {:.1} • {:+.1}% • {}",
            final_crf, size_pct, ssim_str
        ));
    }

    pub fn finish_with_message(&self, msg: &str) {
        self.is_finished.store(true, Ordering::Relaxed);
        self.bar.finish_with_message(msg.to_string());
    }
}

impl Drop for UnifiedProgressBar {
    fn drop(&mut self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            self.bar.finish_and_clear();
        }
    }
}
