//! Progress Bar Module v5.30
//!
//! 🔥 统一进度条系统：
//! - 全项目统一样式: ████████▓▓░░░░░░
//! - 更粗更显眼的进度条
//! - 固定在终端底部显示
//! - 详细进度参数（当前文件、剩余时间、处理速度、SSIM、CRF等）
//!
//! Reference: media/CONTRIBUTING.md - Visual Progress Bar requirement

use crate::modern_ui::progress_style;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::io::{self, Write};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};


pub struct CoarseProgressBar {
    total: u64,
    current: AtomicU64,
    start_time: Instant,
    prefix: String,
    last_render: Arc<Mutex<Instant>>,
    is_finished: AtomicBool,
}

impl CoarseProgressBar {
    pub fn new(total: u64, prefix: &str) -> Self {
        eprint!("\x1b[?25l");
        let _ = io::stderr().flush();

        Self {
            total,
            current: AtomicU64::new(0),
            start_time: Instant::now(),
            prefix: prefix.to_string(),
            last_render: Arc::new(Mutex::new(Instant::now())),
            is_finished: AtomicBool::new(false),
        }
    }

    pub fn set(&self, current: u64) {
        self.current.store(current, Ordering::Relaxed);

        if let Ok(mut last) = self.last_render.try_lock() {
            if last.elapsed() >= Duration::from_millis(200) {
                self.render();
                *last = Instant::now();
            }
        }
    }

    pub fn inc(&self) {
        let current = self.current.fetch_add(1, Ordering::Relaxed) + 1;
        if current.is_multiple_of(10) {
            self.set(current);
        }
    }

    pub fn set_message(&self, _msg: &str) {
        self.render();
    }

    pub fn println(&self, msg: &str) {
        if self.is_finished.load(Ordering::Relaxed) {
            eprintln!("{}", msg);
            return;
        }

        eprint!("\r\x1b[K");
        let _ = io::stderr().flush();

        eprintln!("{}", msg);

        self.render();
    }

    fn render(&self) {
        if self.is_finished.load(Ordering::Relaxed) {
            return;
        }

        let current = self.current.load(Ordering::Relaxed);
        let total = self.total.max(1);
        let percent = (current as f64 / total as f64 * 100.0).min(100.0);
        let elapsed = self.start_time.elapsed();

        let bar_width: usize = progress_style::BAR_WIDTH;
        let filled = ((percent / 100.0) * bar_width as f64) as usize;
        let empty = bar_width.saturating_sub(filled);

        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

        let color = "\x1b[32m";

        let eta_str = if current > 0 && current < total {
            let avg_time = elapsed.as_secs_f64() / current as f64;
            let remaining_secs = ((total - current) as f64 * avg_time) as u64;
            format_eta_simple(remaining_secs)
        } else {
            "---".to_string()
        };

        eprint!(
            "\r\x1b[K{}{} {}{}{}{}▏ {:>5.1}% • {}/{} • ⏱️ {:.1}s • ETA: {}\x1b[0m",
            color,
            self.prefix,
            progress_style::BAR_LEFT,
            color,
            bar,
            color,
            percent,
            current,
            total,
            elapsed.as_secs_f64(),
            eta_str
        );
        let _ = io::stderr().flush();
    }

    pub fn finish(&self) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }

        let total = self.total;
        let elapsed = self.start_time.elapsed();

        let bar_width: usize = progress_style::BAR_WIDTH;
        let bar = "█".repeat(bar_width);

        eprint!(
            "\r\x1b[K\x1b[32m{} {}\x1b[32m{}\x1b[32m▏ ✅ 100% • {}/{} • ⏱️ {:.1}s\x1b[0m\n",
            self.prefix,
            progress_style::BAR_LEFT,
            bar,
            total,
            total,
            elapsed.as_secs_f64()
        );

        eprint!("\x1b[?25h");
        let _ = io::stderr().flush();
    }

    pub fn finish_and_clear(&self) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }

        eprint!("\r\x1b[K");
        eprint!("\x1b[?25h");
        let _ = io::stderr().flush();
    }
}

impl Drop for CoarseProgressBar {
    fn drop(&mut self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            self.finish();
        }
    }
}


pub struct DetailedCoarseProgressBar {
    prefix: String,
    total_iterations: u64,
    current_iteration: AtomicU64,
    input_size: u64,
    current_crf: AtomicU64,
    current_size: AtomicU64,
    current_ssim: AtomicU64,
    best_crf: AtomicU64,
    start_time: Instant,
    last_render: Arc<Mutex<Instant>>,
    is_finished: AtomicBool,
}

impl DetailedCoarseProgressBar {
    pub fn new(prefix: &str, input_size: u64, total_iterations: u64) -> Self {
        eprint!("\x1b[?25l");
        let _ = io::stderr().flush();

        Self {
            prefix: prefix.to_string(),
            total_iterations,
            current_iteration: AtomicU64::new(0),
            input_size,
            current_crf: AtomicU64::new(0),
            current_size: AtomicU64::new(0),
            current_ssim: AtomicU64::new(0),
            best_crf: AtomicU64::new(0),
            start_time: Instant::now(),
            last_render: Arc::new(Mutex::new(Instant::now())),
            is_finished: AtomicBool::new(false),
        }
    }

    pub fn inc_iteration(&self, crf: f32, size: u64, ssim: Option<f64>) {
        let iter = self.current_iteration.fetch_add(1, Ordering::Relaxed) + 1;

        self.current_crf
            .store(crf.to_bits() as u64, Ordering::Relaxed);
        self.current_size.store(size, Ordering::Relaxed);
        if let Some(s) = ssim {
            self.current_ssim.store(s.to_bits(), Ordering::Relaxed);
        }

        if size < self.input_size {
            self.best_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        }

        self.render(iter, crf, size, ssim);
    }

    fn render(&self, iter: u64, crf: f32, size: u64, ssim: Option<f64>) {
        if self.is_finished.load(Ordering::Relaxed) {
            return;
        }

        if let Ok(mut last) = self.last_render.try_lock() {
            if last.elapsed() < Duration::from_millis(100) {
                return;
            }
            *last = Instant::now();
        } else {
            return;
        }

        let total = self.total_iterations.max(1);
        let percent = (iter as f64 / total as f64 * 100.0).min(100.0);
        let elapsed = self.start_time.elapsed();

        let bar_width: usize = progress_style::BAR_WIDTH;
        let filled = ((percent / 100.0) * bar_width as f64) as usize;
        let empty = bar_width.saturating_sub(filled);
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

        let size_pct = if self.input_size > 0 {
            ((size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };

        let icon = if size < self.input_size {
            "💾"
        } else {
            "📈"
        };

        let ssim_str = if let Some(s) = ssim {
            format!("SSIM {:.4}", s)
        } else {
            String::new()
        };

        let best_crf = f32::from_bits(self.best_crf.load(Ordering::Relaxed) as u32);
        let best_str = if best_crf > 0.0 {
            format!("Best: {:.1}", best_crf)
        } else {
            String::new()
        };

        let color = "\x1b[32m";
        eprint!(
            "\r\x1b[K{}{} {}{}{}{}▏ {:.1}% • CRF {:.1} | {:+.1}% {} | {} | {} | {}/{} • ⏱️ {:.1}s\x1b[0m",
            color,
            self.prefix,
            progress_style::BAR_LEFT,
            color,
            bar,
            color,
            percent,
            crf,
            size_pct,
            icon,
            ssim_str,
            best_str,
            iter,
            total,
            elapsed.as_secs_f64()
        );
        let _ = io::stderr().flush();
    }

    pub fn println(&self, msg: &str) {
        if self.is_finished.load(Ordering::Relaxed) {
            eprintln!("{}", msg);
            return;
        }

        eprint!("\r\x1b[K");
        let _ = io::stderr().flush();

        eprintln!("{}", msg);

        let iter = self.current_iteration.load(Ordering::Relaxed);
        let crf = f32::from_bits(self.current_crf.load(Ordering::Relaxed) as u32);
        let size = self.current_size.load(Ordering::Relaxed);
        let ssim_bits = self.current_ssim.load(Ordering::Relaxed);
        let ssim = if ssim_bits != 0 {
            Some(f64::from_bits(ssim_bits))
        } else {
            None
        };

        if let Ok(mut last) = self.last_render.lock() {
            *last = Instant::now() - Duration::from_secs(1);
        }
        self.render(iter, crf, size, ssim);
    }

    pub fn finish(&self, final_crf: f32, final_size: u64, final_ssim: Option<f64>) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }

        let elapsed = self.start_time.elapsed();

        let size_pct = if self.input_size > 0 {
            ((final_size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };

        let ssim_str = final_ssim
            .map(|s| format!("SSIM {:.4}", s))
            .unwrap_or_default();

        let icon = if size_pct < 0.0 { "✅" } else { "⚠️" };
        let iter = self.current_iteration.load(Ordering::Relaxed);

        let bar_width: usize = progress_style::BAR_WIDTH;
        let bar = "█".repeat(bar_width);
        let color = "\x1b[32m";

        eprint!(
            "\r\x1b[K{}{} {}{}{}{}▏ ✅ 100% • CRF {:.1} • {:+.1}% {} • {} • {} iterations • ⏱️ {:.1}s\x1b[0m\n",
            color,
            self.prefix,
            progress_style::BAR_LEFT,
            color,
            bar,
            color,
            final_crf,
            size_pct,
            icon,
            ssim_str,
            iter,
            elapsed.as_secs_f64()
        );

        eprint!("\x1b[?25h");
        let _ = io::stderr().flush();
    }

    pub fn fail(&self, error: &str) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }

        eprint!("\r\x1b[K❌ {} {}\n", self.prefix, error);
        eprint!("\x1b[?25h");
        let _ = io::stderr().flush();
    }
}

impl Drop for DetailedCoarseProgressBar {
    fn drop(&mut self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            eprint!("\r\x1b[K");
            eprint!("\x1b[?25h");
            let _ = io::stderr().flush();
        }
    }
}

fn format_eta_simple(seconds: u64) -> String {
    if seconds > 86400 {
        return ">1d".to_string();
    }
    if seconds >= 3600 {
        format!("{}h{}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m{}s", seconds / 60, seconds % 60)
    } else {
        format!("{}s", seconds)
    }
}


pub struct FixedBottomProgress {
    bar: ProgressBar,
    start_time: Instant,
    total: u64,
    processed: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    skipped: AtomicU64,
    input_bytes: AtomicU64,
    output_bytes: AtomicU64,
    current_file: Arc<Mutex<String>>,
    current_stage: Arc<Mutex<String>>,
}

impl FixedBottomProgress {
    pub fn new(total: u64, prefix: &str) -> Self {
        let bar = ProgressBar::new(total);

        bar.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::BATCH_TEMPLATE)
                .expect("Invalid progress bar template")
                .progress_chars(progress_style::PROGRESS_CHARS)
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        bar.set_prefix(prefix.to_string());
        bar.enable_steady_tick(Duration::from_millis(100));

        bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));

        Self {
            bar,
            start_time: Instant::now(),
            total,
            processed: AtomicU64::new(0),
            succeeded: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            input_bytes: AtomicU64::new(0),
            output_bytes: AtomicU64::new(0),
            current_file: Arc::new(Mutex::new(String::new())),
            current_stage: Arc::new(Mutex::new(String::new())),
        }
    }

    pub fn set_current_file(&self, filename: &str) {
        if let Ok(mut f) = self.current_file.lock() {
            *f = filename.to_string();
        }
        self.update_message();
    }

    pub fn set_stage(&self, stage: &str) {
        if let Ok(mut s) = self.current_stage.lock() {
            *s = stage.to_string();
        }
        self.update_message();
    }

    fn update_message(&self) {
        let file = self
            .current_file
            .lock()
            .map(|f| f.clone())
            .unwrap_or_default();
        let stage = self
            .current_stage
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();

        let msg = if !stage.is_empty() && !file.is_empty() {
            format!("{} | {}", stage, truncate_filename(&file, 40))
        } else if !file.is_empty() {
            truncate_filename(&file, 50)
        } else if !stage.is_empty() {
            stage
        } else {
            "Processing...".to_string()
        };

        self.bar.set_message(msg);
    }

    pub fn success(&self, input_size: u64, output_size: u64) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.succeeded.fetch_add(1, Ordering::Relaxed);
        self.input_bytes.fetch_add(input_size, Ordering::Relaxed);
        self.output_bytes.fetch_add(output_size, Ordering::Relaxed);
        self.bar.inc(1);
    }

    pub fn fail(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.failed.fetch_add(1, Ordering::Relaxed);
        self.bar.inc(1);
    }

    pub fn skip(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.skipped.fetch_add(1, Ordering::Relaxed);
        self.bar.inc(1);
    }

    pub fn stats(&self) -> ProgressStats {
        let input = self.input_bytes.load(Ordering::Relaxed);
        let output = self.output_bytes.load(Ordering::Relaxed);
        ProgressStats {
            total: self.total,
            processed: self.processed.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            input_bytes: input,
            output_bytes: output,
            elapsed: self.start_time.elapsed(),
            compression_ratio: if input > 0 {
                output as f64 / input as f64
            } else {
                1.0
            },
        }
    }

    pub fn finish(&self) {
        let stats = self.stats();
        let saved = stats.input_bytes.saturating_sub(stats.output_bytes);

        self.bar.finish_with_message(format!(
            "✅ {} succeeded, {} failed, {} skipped | Saved: {}",
            stats.succeeded,
            stats.failed,
            stats.skipped,
            format_bytes(saved)
        ));
    }

    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

#[derive(Debug, Clone)]
pub struct ProgressStats {
    pub total: u64,
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub skipped: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub elapsed: Duration,
    pub compression_ratio: f64,
}


pub struct ExploreProgress {
    start_time: Instant,
    input_size: u64,
    current_crf: Arc<Mutex<f32>>,
    current_ssim: Arc<Mutex<Option<f64>>>,
    current_size: Arc<Mutex<u64>>,
    iterations: AtomicUsize,
    stage: Arc<Mutex<String>>,
    best_crf: Arc<Mutex<f32>>,
    best_ssim: Arc<Mutex<f64>>,
}

impl ExploreProgress {
    pub fn new(input_size: u64) -> Self {
        Self {
            start_time: Instant::now(),
            input_size,
            current_crf: Arc::new(Mutex::new(0.0)),
            current_ssim: Arc::new(Mutex::new(None)),
            current_size: Arc::new(Mutex::new(0)),
            iterations: AtomicUsize::new(0),
            stage: Arc::new(Mutex::new("Initializing".to_string())),
            best_crf: Arc::new(Mutex::new(0.0)),
            best_ssim: Arc::new(Mutex::new(0.0)),
        }
    }

    pub fn update_crf(&self, crf: f32, size: u64, ssim: Option<f64>) {
        if let Ok(mut c) = self.current_crf.lock() {
            *c = crf;
        }
        if let Ok(mut s) = self.current_size.lock() {
            *s = size;
        }
        if let Ok(mut ss) = self.current_ssim.lock() {
            *ss = ssim;
        }
        self.iterations.fetch_add(1, Ordering::Relaxed);
        self.print_status();
    }

    pub fn set_stage(&self, stage: &str) {
        if let Ok(mut s) = self.stage.lock() {
            *s = stage.to_string();
        }
        self.print_status();
    }

    pub fn update_best(&self, crf: f32, ssim: f64) {
        if let Ok(mut c) = self.best_crf.lock() {
            *c = crf;
        }
        if let Ok(mut s) = self.best_ssim.lock() {
            *s = ssim;
        }
    }

    fn print_status(&self) {
        let crf = self.current_crf.lock().map(|c| *c).unwrap_or(0.0);
        let size = self.current_size.lock().map(|s| *s).unwrap_or(0);
        let ssim = self.current_ssim.lock().ok().and_then(|s| *s);
        let stage = self.stage.lock().map(|s| s.clone()).unwrap_or_default();
        let iter = self.iterations.load(Ordering::Relaxed);
        let best_crf = self.best_crf.lock().map(|c| *c).unwrap_or(0.0);
        let best_ssim = self.best_ssim.lock().map(|s| *s).unwrap_or(0.0);

        let size_change = if self.input_size > 0 {
            ((size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };

        let elapsed = self.start_time.elapsed();
        let ssim_str = ssim
            .map(|s| format!("{:.4}", s))
            .unwrap_or_else(|| "---".to_string());
        let compress_icon = if size < self.input_size { "✅" } else { "❌" };

        eprint!("\r\x1b[K");
        eprint!(
            "🔍 Explore: {} • CRF {:.1} • SSIM {} • Size {:+.1}% {} • Iter {} • Best: CRF {:.1} / SSIM {:.4} • ⏱️ {:.1}s",
            stage, crf, ssim_str, size_change, compress_icon, iter, best_crf, best_ssim, elapsed.as_secs_f64()
        );
        let _ = io::stderr().flush();
    }

    pub fn finish(&self, result_crf: f32, result_ssim: f64, result_size: u64) {
        let size_change = if self.input_size > 0 {
            ((result_size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };
        let elapsed = self.start_time.elapsed();
        let iter = self.iterations.load(Ordering::Relaxed);

        eprintln!("\r\x1b[K");
        eprintln!(
            "✅ Explore Done: CRF {:.1} • SSIM {:.4} • Size {:+.1}% • {} iter in {:.1}s",
            result_crf,
            result_ssim,
            size_change,
            iter,
            elapsed.as_secs_f64()
        );
    }
}


pub struct ExploreLogger {
    input_size: u64,
    start_time: Instant,
    iterations: usize,
    best_crf: f32,
    best_ssim: f64,
    best_size: u64,
    show_progress_bar: bool,
}

impl ExploreLogger {
    pub fn new(input_size: u64, show_progress_bar: bool) -> Self {
        Self {
            input_size,
            start_time: Instant::now(),
            iterations: 0,
            best_crf: 0.0,
            best_ssim: 0.0,
            best_size: 0,
            show_progress_bar,
        }
    }

    pub fn stage(&mut self, name: &str) {
        if self.show_progress_bar {
            eprintln!("\n   📍 {}", name);
        }
    }

    pub fn test(&mut self, crf: f32, size: u64, ssim: Option<f64>) {
        self.iterations += 1;
        let size_change = self.calc_change(size);
        let compress_ok = size < self.input_size;

        if self.show_progress_bar {
            let ssim_str = ssim.map(|s| format!("SSIM {:.4}", s)).unwrap_or_default();
            let icon = if compress_ok { "✅" } else { "❌" };
            eprint!(
                "\r\x1b[K   🔄 CRF {:.1}: {:+.1}% {} {}",
                crf, size_change, icon, ssim_str
            );
            let _ = io::stderr().flush();
        }
    }

    pub fn new_best(&mut self, crf: f32, size: u64, ssim: f64) {
        self.best_crf = crf;
        self.best_size = size;
        self.best_ssim = ssim;

        if self.show_progress_bar {
            eprintln!(" ← 🎯 New best!");
        }
    }

    pub fn direction(&self, msg: &str) {
        if self.show_progress_bar {
            eprintln!("\r\x1b[K      {}", msg);
        }
    }

    pub fn early_stop(&self, reason: &str) {
        if self.show_progress_bar {
            eprintln!("\r\x1b[K   ⚡ Early stop: {}", reason);
        }
    }

    fn calc_change(&self, size: u64) -> f64 {
        if self.input_size > 0 {
            ((size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        }
    }

    pub fn finish(&self) {
        if !self.show_progress_bar {
            return;
        }

        let elapsed = self.start_time.elapsed();
        let size_change = self.calc_change(self.best_size);
        let saved = self.input_size.saturating_sub(self.best_size);

        eprintln!("\r\x1b[K");
        eprintln!("   ═══════════════════════════════════════════════════");
        eprintln!(
            "   📊 Result: CRF {:.1} | SSIM {:.4} | {:+.1}%",
            self.best_crf, self.best_ssim, size_change
        );
        if saved > 0 {
            eprintln!(
                "   💾 Saved: {} ({:.2} MB)",
                format_bytes(saved),
                saved as f64 / 1024.0 / 1024.0
            );
        }
        eprintln!(
            "   📈 Iterations: {} | Time: {:.1}s",
            self.iterations,
            elapsed.as_secs_f64()
        );
    }
}


pub fn create_professional_spinner(prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();

    if crate::progress_mode::is_quiet_mode() {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        pb.set_style(
            ProgressStyle::default_spinner()
                .template(progress_style::SPINNER_TEMPLATE)
                .expect("Invalid spinner template")
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
    }
    pb
}

pub fn create_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);

    if crate::progress_mode::is_quiet_mode() {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        pb.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::BATCH_TEMPLATE)
                .expect("Invalid progress bar template")
                .progress_chars(progress_style::PROGRESS_CHARS)
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
    }
    pb
}

pub fn create_detailed_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);

    if crate::progress_mode::is_quiet_mode() {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        pb.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::BATCH_TEMPLATE)
                .expect("Invalid progress bar template")
                .progress_chars(progress_style::PROGRESS_CHARS)
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    }
    pb
}

pub fn create_compact_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);

    if crate::progress_mode::is_quiet_mode() {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        pb.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::COMPACT_TEMPLATE)
                .expect("Invalid progress bar template")
                .progress_chars(progress_style::PROGRESS_CHARS),
        );
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(Duration::from_millis(200));
    }
    pb
}

pub fn create_progress_bar_with_eta(total: u64, prefix: &str) -> SmartProgressBar {
    SmartProgressBar::new(total, prefix)
}

pub struct SmartProgressBar {
    bar: ProgressBar,
    start_time: Instant,
    total: u64,
    processed: u64,
    recent_times: Vec<f64>,
    last_update: Instant,
}

impl SmartProgressBar {
    pub fn new(total: u64, prefix: &str) -> Self {
        let bar = ProgressBar::new(total);

        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(progress_style::BATCH_TEMPLATE)
                    .expect("Invalid progress bar template")
                    .progress_chars(progress_style::PROGRESS_CHARS)
                    .tick_chars(progress_style::SPINNER_CHARS),
            );
            bar.set_prefix(prefix.to_string());
            bar.enable_steady_tick(Duration::from_millis(100));
        }

        Self {
            bar,
            start_time: Instant::now(),
            total,
            processed: 0,
            recent_times: Vec::with_capacity(10),
            last_update: Instant::now(),
        }
    }

    pub fn inc(&mut self, message: &str) {
        let elapsed = self.last_update.elapsed().as_secs_f64();
        self.last_update = Instant::now();

        if self.recent_times.len() >= 10 {
            self.recent_times.remove(0);
        }
        self.recent_times.push(elapsed);

        self.processed += 1;
        self.bar.inc(1);

        let remaining = self.total.saturating_sub(self.processed);
        let eta = if !self.recent_times.is_empty() && remaining > 0 {
            let avg_time: f64 =
                self.recent_times.iter().sum::<f64>() / self.recent_times.len() as f64;
            let eta_secs = avg_time * remaining as f64;
            format_eta(eta_secs)
        } else {
            "calculating...".to_string()
        };

        self.bar.set_message(format!("{} | {}", eta, message));
    }

    pub fn finish(&self) {
        let total_time = self.start_time.elapsed();
        self.bar
            .finish_with_message(format!("Done in {}", format_duration(total_time)));
    }

    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

fn format_eta(seconds: f64) -> String {
    if seconds.is_nan() || seconds.is_infinite() || seconds < 0.0 {
        return "unknown".to_string();
    }

    let secs = seconds as u64;

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

pub fn create_spinner(message: &str) -> ProgressBar {
    let spinner = ProgressBar::new_spinner();

    if crate::progress_mode::is_quiet_mode() {
        spinner.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("Invalid spinner template")
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        spinner.set_message(message.to_string());
        spinner.enable_steady_tick(Duration::from_millis(80));
    }
    spinner
}

pub fn create_multi_progress() -> MultiProgress {
    MultiProgress::new()
}

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

    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}


fn truncate_filename(filename: &str, max_len: usize) -> String {
    if filename.len() <= max_len {
        filename.to_string()
    } else {
        let half = (max_len - 3) / 2;
        format!(
            "{}...{}",
            &filename[..half],
            &filename[filename.len() - half..]
        )
    }
}

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


pub struct GlobalProgressManager {
    multi: MultiProgress,
    main_bar: Option<ProgressBar>,
    sub_bar: Option<ProgressBar>,
    #[allow(dead_code)]
    _start_time: Instant,
}

impl GlobalProgressManager {
    pub fn new() -> Self {
        Self {
            multi: MultiProgress::new(),
            main_bar: None,
            sub_bar: None,
            _start_time: Instant::now(),
        }
    }

    pub fn create_main(&mut self, total: u64, prefix: &str) -> &ProgressBar {
        let bar = self.multi.add(ProgressBar::new(total));

        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(progress_style::BATCH_TEMPLATE)
                    .expect("Invalid template")
                    .progress_chars(progress_style::PROGRESS_CHARS)
                    .tick_chars(progress_style::SPINNER_CHARS),
            );
            bar.set_prefix(prefix.to_string());
            bar.enable_steady_tick(Duration::from_millis(100));
        }
        self.main_bar = Some(bar);
        // SAFETY: set to Some on the line above
        self.main_bar.as_ref().unwrap()
    }

    pub fn create_sub(&mut self, prefix: &str) -> &ProgressBar {
        let bar = self.multi.add(ProgressBar::new_spinner());

        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_spinner()
                    .template("  {spinner:.green} {prefix:.dim}: {msg}")
                    .expect("Invalid template")
                    .tick_chars(progress_style::SPINNER_CHARS),
            );
            bar.set_prefix(prefix.to_string());
            bar.enable_steady_tick(Duration::from_millis(80));
        }
        self.sub_bar = Some(bar);
        // SAFETY: set to Some on the line above
        self.sub_bar.as_ref().unwrap()
    }

    pub fn inc_main(&self) {
        if let Some(bar) = &self.main_bar {
            bar.inc(1);
        }
    }

    pub fn set_main_message(&self, msg: &str) {
        if let Some(bar) = &self.main_bar {
            bar.set_message(msg.to_string());
        }
    }

    pub fn set_sub_message(&self, msg: &str) {
        if let Some(bar) = &self.sub_bar {
            bar.set_message(msg.to_string());
        }
    }

    pub fn finish_all(&self, summary: &str) {
        if let Some(bar) = &self.sub_bar {
            bar.finish_and_clear();
        }
        if let Some(bar) = &self.main_bar {
            bar.finish_with_message(summary.to_string());
        }
    }
}

impl Default for GlobalProgressManager {
    fn default() -> Self {
        Self::new()
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

    #[test]
    fn test_truncate_filename() {
        assert_eq!(truncate_filename("short.txt", 20), "short.txt");
        let truncated = truncate_filename("very_long_filename_that_needs_truncation.txt", 20);
        assert!(
            truncated.len() <= 20,
            "truncated len {} > 20",
            truncated.len()
        );
        assert!(truncated.contains("..."));
    }
}
