//! Unified Progress Bar System
//!
//! Provides a consistent experience for both batch processing and video
//! exploration.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub mod templates {
    use crate::modern_ui::progress_style;

    pub const BATCH: &str = "{spinner:.green} {prefix:.cyan.bold} [{bar:35.green/black}] \
                             {percent:>3}% * {pos}/{len} * {elapsed_precise} * {msg}";
    pub const EXPLORE: &str = "{spinner:.green} {prefix:.cyan.bold} [{bar:35.green/black}] \
                               {percent:>3}% * {elapsed_precise} * {msg}";
    pub const PROGRESS_CHARS: &str = progress_style::PROGRESS_CHARS;
    pub const SPINNER_CHARS: &str = progress_style::SPINNER_CHARS;
}

pub struct Bar {
    pub bar: ProgressBar,
    input_size: u64,
    current_iteration: AtomicU64,
    is_finished: AtomicBool,
}

fn progress_bar_style(template: &'static str, branch: &'static str) -> ProgressStyle {
    let style = match ProgressStyle::default_bar().template(template) {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(
                target: "mfb.ui",
                branch,
                error = %err,
                "invalid progress bar template; using default bar style"
            );
            ProgressStyle::default_bar()
        }
    };
    style
        .progress_chars(templates::PROGRESS_CHARS)
        .tick_chars(templates::SPINNER_CHARS)
}

impl Bar {
    #[must_use]
    /// Create a new batch progress bar.
    pub fn new(total: u64, message: &str) -> Arc<Self> {
        let bar = ProgressBar::new(total);
        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(progress_bar_style(templates::BATCH, "batch_progress"));
            bar.set_prefix(message.to_string());
            bar.enable_steady_tick(std::time::Duration::from_millis(8));
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
        }
        Arc::new(Self {
            bar,
            input_size: 0,
            current_iteration: AtomicU64::new(0),
            is_finished: AtomicBool::new(false),
        })
    }

    #[must_use]
    /// Create a new iteration progress bar.
    pub fn new_iteration(message: &str, input_size: u64, total_iterations: u64) -> Arc<Self> {
        let bar = ProgressBar::new(total_iterations);
        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(progress_bar_style(templates::EXPLORE, "explore_progress"));
            bar.set_prefix(message.to_string());
            bar.enable_steady_tick(std::time::Duration::from_millis(8));
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
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
        let msg_str = msg.into();
        let term_width = crate::progress::terminal_columns();
        // The indicatif template includes prefix, bar (35), percent, and elapsed time.
        // This adds up to approximately 85 characters of overhead.
        let max_msg_len = term_width.saturating_sub(85).max(10);
        let truncated = console::truncate_str(&msg_str, max_msg_len, "…").to_string();
        self.bar.set_message(truncated);
    }

    pub fn println(&self, msg: &str) {
        crate::progress_mode::emit_stderr(msg);
    }

    pub fn inc_iteration(&self, crf: f32, size: u64, ssim: Option<f64>) {
        let iter = self.current_iteration.fetch_add(1, Ordering::Relaxed) + 1;
        self.bar.set_position(iter);
        let size_pct = if self.input_size > 0 {
            ((crate::numeric_cast::u64_to_f64(size)
                / crate::numeric_cast::u64_to_f64(self.input_size))
                - 1.0_f64)
                * 100.0_f64
        } else {
            0.0_f64
        };
        let ssim_str = crate::media_conversion_gate::ui_ssim_inline_or_na(ssim);
        let ssim_sep = if ssim_str.is_empty() { "" } else { " | " };
        self.set_message(format!(
            "CRF {crf:.1} | {size_pct:+.1}%{ssim_sep}{ssim_str}"
        ));
    }

    pub fn finish_iteration(&self, final_crf: f32, final_size: u64, final_ssim: Option<f64>) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }
        let size_pct = if self.input_size > 0 {
            ((crate::numeric_cast::u64_to_f64(final_size)
                / crate::numeric_cast::u64_to_f64(self.input_size))
                - 1.0_f64)
                * 100.0_f64
        } else {
            0.0_f64
        };
        let ssim_str =
            crate::media_conversion_gate::ui_optional_f64_display_suffix(final_ssim, "SSIM");
        self.finish_with_message(&format!(
            "✅ CRF {final_crf:.1} • {size_pct:+.1}% • {ssim_str}"
        ));
    }

    pub fn finish_with_message(&self, msg: &str) {
        self.is_finished.store(true, Ordering::Relaxed);
        let term_width = crate::progress::terminal_columns();
        let max_msg_len = term_width.saturating_sub(85).max(10);
        let truncated = console::truncate_str(msg, max_msg_len, "…").to_string();
        self.bar.finish_with_message(truncated);
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            self.bar.finish_and_clear();
        }
    }
}
