//! Progress Bar Module v5.5
//! 
//! 🔥 全面改进的进度条系统：
//! - 固定在终端底部显示
//! - 详细进度参数（当前文件、剩余时间、处理速度、SSIM、CRF等）
//! - 特别优化 --explore --match-quality --compress 组合时的进度显示
//! 
//! Reference: media/CONTRIBUTING.md - Visual Progress Bar requirement

use indicatif::{ProgressBar, ProgressStyle, MultiProgress, ProgressDrawTarget};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, AtomicUsize, Ordering}};
use std::time::{Duration, Instant};
use std::io::{self, Write};

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.5: 固定底部进度条 - 核心组件
// ═══════════════════════════════════════════════════════════════

/// 固定在终端底部的进度条
/// 
/// 特点：
/// - 始终显示在终端最后一行
/// - 不会被其他输出覆盖
/// - 支持详细进度参数
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
    /// 创建固定底部进度条
    pub fn new(total: u64, prefix: &str) -> Self {
        let bar = ProgressBar::new(total);
        
        // 🔥 v5.7: Ultra-Professional Combined Style
        bar.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} {prefix:.cyan.bold} ▕{bar:30.blue}▏ {percent:>3}% • {pos}/{len} • ⏱️ {elapsed_precise} (ETA: {eta_precise}) • {msg}")
                .expect("Invalid progress bar template")
                .progress_chars("█▓▒░")
        );
        bar.set_prefix(prefix.to_string());
        // Ultra-fluid 60fps-like updates (16ms is too fast, 50ms is good)
        bar.enable_steady_tick(Duration::from_millis(50));
        
        // High refresh rate for responsiveness
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
    
    /// 设置当前处理的文件
    pub fn set_current_file(&self, filename: &str) {
        if let Ok(mut f) = self.current_file.lock() {
            *f = filename.to_string();
        }
        self.update_message();
    }
    
    /// 设置当前阶段（用于探索模式）
    pub fn set_stage(&self, stage: &str) {
        if let Ok(mut s) = self.current_stage.lock() {
            *s = stage.to_string();
        }
        self.update_message();
    }

    /// 更新消息显示
    fn update_message(&self) {
        let file = self.current_file.lock().map(|f| f.clone()).unwrap_or_default();
        let stage = self.current_stage.lock().map(|s| s.clone()).unwrap_or_default();
        
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
    
    /// 记录成功
    pub fn success(&self, input_size: u64, output_size: u64) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.succeeded.fetch_add(1, Ordering::Relaxed);
        self.input_bytes.fetch_add(input_size, Ordering::Relaxed);
        self.output_bytes.fetch_add(output_size, Ordering::Relaxed);
        self.bar.inc(1);
    }
    
    /// 记录失败
    pub fn fail(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.failed.fetch_add(1, Ordering::Relaxed);
        self.bar.inc(1);
    }
    
    /// 记录跳过
    pub fn skip(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.skipped.fetch_add(1, Ordering::Relaxed);
        self.bar.inc(1);
    }
    
    /// 获取统计信息
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
            compression_ratio: if input > 0 { output as f64 / input as f64 } else { 1.0 },
        }
    }

    /// 完成进度条
    pub fn finish(&self) {
        let stats = self.stats();
        let saved = if stats.input_bytes > stats.output_bytes {
            stats.input_bytes - stats.output_bytes
        } else {
            0
        };
        
        self.bar.finish_with_message(format!(
            "✅ {} succeeded, {} failed, {} skipped | Saved: {}",
            stats.succeeded, stats.failed, stats.skipped,
            format_bytes(saved)
        ));
    }
    
    /// 获取内部 ProgressBar 引用
    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

/// 进度统计信息
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

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.5: 探索进度条 - 专为 CRF 探索设计
// ═══════════════════════════════════════════════════════════════

/// 探索模式专用进度条
/// 
/// 显示：
/// - 当前 CRF 值
/// - 当前 SSIM
/// - 文件大小变化
/// - 迭代次数
/// - 搜索阶段
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
    /// 创建探索进度条
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
    
    /// 更新当前测试的 CRF
    pub fn update_crf(&self, crf: f32, size: u64, ssim: Option<f64>) {
        if let Ok(mut c) = self.current_crf.lock() { *c = crf; }
        if let Ok(mut s) = self.current_size.lock() { *s = size; }
        if let Ok(mut ss) = self.current_ssim.lock() { *ss = ssim; }
        self.iterations.fetch_add(1, Ordering::Relaxed);
        self.print_status();
    }
    
    /// 设置搜索阶段
    pub fn set_stage(&self, stage: &str) {
        if let Ok(mut s) = self.stage.lock() {
            *s = stage.to_string();
        }
        self.print_status();
    }
    
    /// 更新最佳结果
    pub fn update_best(&self, crf: f32, ssim: f64) {
        if let Ok(mut c) = self.best_crf.lock() { *c = crf; }
        if let Ok(mut s) = self.best_ssim.lock() { *s = ssim; }
    }
    
    /// 打印当前状态到固定位置
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
        let ssim_str = ssim.map(|s| format!("{:.4}", s)).unwrap_or_else(|| "---".to_string());
        let compress_icon = if size < self.input_size { "✅" } else { "❌" };
        
        // 🔥 Concise Explore Status
        eprint!("\r\x1b[K"); // Clear line
        eprint!(
            "🔍 Explore: {} • CRF {:.1} • SSIM {} • Size {:+.1}% {} • Iter {} • Best: CRF {:.1} / SSIM {:.4} • ⏱️ {:.1}s",
            stage, crf, ssim_str, size_change, compress_icon, iter, best_crf, best_ssim, elapsed.as_secs_f64()
        );
        let _ = io::stderr().flush();
    }

    /// 完成探索
    pub fn finish(&self, result_crf: f32, result_ssim: f64, result_size: u64) {
        let size_change = if self.input_size > 0 {
            ((result_size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };
        let elapsed = self.start_time.elapsed();
        let iter = self.iterations.load(Ordering::Relaxed);
        
        eprintln!("\r\x1b[K"); // Clear progress line
        eprintln!("✅ Explore Done: CRF {:.1} • SSIM {:.4} • Size {:+.1}% • {} iter in {:.1}s",
            result_crf, result_ssim, size_change, iter, elapsed.as_secs_f64());
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.5: 实时探索日志 - 带进度条的日志输出
// ═══════════════════════════════════════════════════════════════

/// 实时探索日志器
/// 
/// 在探索过程中实时显示：
/// - 每次编码的 CRF、大小、SSIM
/// - 搜索方向和决策
/// - 最终结果
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
    
    /// 记录阶段开始
    pub fn stage(&mut self, name: &str) {
        if self.show_progress_bar {
            eprintln!("\n   📍 {}", name);
        }
    }
    
    /// 记录编码测试
    pub fn test(&mut self, crf: f32, size: u64, ssim: Option<f64>) {
        self.iterations += 1;
        let size_change = self.calc_change(size);
        let compress_ok = size < self.input_size;
        
        if self.show_progress_bar {
            let ssim_str = ssim.map(|s| format!("SSIM {:.4}", s)).unwrap_or_default();
            let icon = if compress_ok { "✅" } else { "❌" };
            eprint!("\r\x1b[K   🔄 CRF {:.1}: {:+.1}% {} {}", crf, size_change, icon, ssim_str);
            let _ = io::stderr().flush();
        }
    }

    /// 记录新的最佳结果
    pub fn new_best(&mut self, crf: f32, size: u64, ssim: f64) {
        self.best_crf = crf;
        self.best_size = size;
        self.best_ssim = ssim;
        
        if self.show_progress_bar {
            eprintln!(" ← 🎯 New best!");
        }
    }
    
    /// 记录搜索方向
    pub fn direction(&self, msg: &str) {
        if self.show_progress_bar {
            eprintln!("\r\x1b[K      {}", msg);
        }
    }
    
    /// 记录提前终止
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
    
    /// 完成并打印摘要
    pub fn finish(&self) {
        if !self.show_progress_bar { return; }
        
        let elapsed = self.start_time.elapsed();
        let size_change = self.calc_change(self.best_size);
        let saved = if self.best_size < self.input_size {
            self.input_size - self.best_size
        } else {
            0
        };
        
        eprintln!("\r\x1b[K");
        eprintln!("   ═══════════════════════════════════════════════════");
        eprintln!("   📊 结果: CRF {:.1} | SSIM {:.4} | {:+.1}%", 
            self.best_crf, self.best_ssim, size_change);
        if saved > 0 {
            eprintln!("   💾 节省: {} ({:.2} MB)", format_bytes(saved), saved as f64 / 1024.0 / 1024.0);
        }
        eprintln!("   📈 迭代: {} 次 | 耗时: {:.1}s", self.iterations, elapsed.as_secs_f64());
    }
}

// ═══════════════════════════════════════════════════════════════
// 原有函数保持兼容
// ═══════════════════════════════════════════════════════════════

/// 🔥 v5.7: Create a unified professional spinner
pub fn create_professional_spinner(prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {prefix:.cyan.bold} • ⏱️ {elapsed_precise} • {msg}")
            .expect("Invalid spinner template")
            // Classic detailed spinner
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
    );
    pb.set_prefix(prefix.to_string());
    pb.enable_steady_tick(Duration::from_millis(50));
    pb
}

/// Create a styled progress bar for batch processing with improved ETA
/// 
/// 🔥 v5.5: 升级为固定底部样式
pub fn create_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("\r{prefix:.cyan.bold} [{bar:40.green/dim}] {pos}/{len} ({percent}%) | {elapsed_precise} | ETA: {eta_precise} | {msg}")
            .expect("Invalid progress bar template")
            .progress_chars("━╸─")
    );
    pb.set_prefix(prefix.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// 🔥 v5.5: 创建详细进度条（带更多参数）
pub fn create_detailed_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(concat!(
                "\r{prefix:.cyan.bold} [{bar:35.green/dim}] {pos}/{len} ({percent:>3}%)\n",
                "  ⏱️ {elapsed_precise} | ETA: {eta_precise} | {per_sec} | {msg}"
            ))
            .expect("Invalid progress bar template")
            .progress_chars("━╸─")
    );
    pb.set_prefix(prefix.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    pb
}

/// 🔥 v5.1: 创建紧凑型进度条（单行，不刷屏）
pub fn create_compact_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("\r{prefix:.cyan} [{bar:30.green/dim}] {percent:>3}% ({pos}/{len}) {msg:.dim}")
            .expect("Invalid progress bar template")
            .progress_chars("━╸─")
    );
    pb.set_prefix(prefix.to_string());
    pb.enable_steady_tick(Duration::from_millis(200));
    pb
}

/// Create a progress bar with custom ETA calculation (for variable-time tasks)
pub fn create_progress_bar_with_eta(total: u64, prefix: &str) -> SmartProgressBar {
    SmartProgressBar::new(total, prefix)
}

/// Smart progress bar with moving average ETA calculation
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
        bar.set_style(
            ProgressStyle::default_bar()
                .template("{prefix:.cyan.bold} [{bar:40.green/dim}] {pos}/{len} ({percent}%) | ETA: {msg}")
                .expect("Invalid progress bar template")
                .progress_chars("━╸─")
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
        
        if self.recent_times.len() >= 10 {
            self.recent_times.remove(0);
        }
        self.recent_times.push(elapsed);
        
        self.processed += 1;
        self.bar.inc(1);
        
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

/// Format ETA with reasonable limits
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

    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

// ═══════════════════════════════════════════════════════════════
// 辅助函数
// ═══════════════════════════════════════════════════════════════

/// 截断文件名以适应显示宽度
fn truncate_filename(filename: &str, max_len: usize) -> String {
    if filename.len() <= max_len {
        filename.to_string()
    } else {
        let half = (max_len - 3) / 2;
        format!("{}...{}", &filename[..half], &filename[filename.len()-half..])
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

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.5: 全局进度管理器 - 用于 App 和脚本
// ═══════════════════════════════════════════════════════════════

/// 全局进度管理器
/// 
/// 用于在整个处理流程中维护一个统一的进度显示
pub struct GlobalProgressManager {
    multi: MultiProgress,
    main_bar: Option<ProgressBar>,
    sub_bar: Option<ProgressBar>,
    start_time: Instant,
}

impl GlobalProgressManager {
    pub fn new() -> Self {
        Self {
            multi: MultiProgress::new(),
            main_bar: None,
            sub_bar: None,
            start_time: Instant::now(),
        }
    }
    
    /// 创建主进度条（总体进度）
    pub fn create_main(&mut self, total: u64, prefix: &str) -> &ProgressBar {
        let bar = self.multi.add(ProgressBar::new(total));
        bar.set_style(
            ProgressStyle::default_bar()
                .template("\r{prefix:.cyan.bold} [{bar:40.green/dim}] {pos}/{len} ({percent}%) | {elapsed_precise} | ETA: {eta_precise}")
                .expect("Invalid template")
                .progress_chars("━╸─")
        );
        bar.set_prefix(prefix.to_string());
        bar.enable_steady_tick(Duration::from_millis(100));
        self.main_bar = Some(bar);
        self.main_bar.as_ref().unwrap()
    }
    
    /// 创建子进度条（当前文件进度）
    pub fn create_sub(&mut self, prefix: &str) -> &ProgressBar {
        let bar = self.multi.add(ProgressBar::new_spinner());
        bar.set_style(
            ProgressStyle::default_spinner()
                .template("  {spinner:.dim} {prefix:.dim}: {msg}")
                .expect("Invalid template")
        );
        bar.set_prefix(prefix.to_string());
        bar.enable_steady_tick(Duration::from_millis(80));
        self.sub_bar = Some(bar);
        self.sub_bar.as_ref().unwrap()
    }
    
    /// 更新主进度
    pub fn inc_main(&self) {
        if let Some(bar) = &self.main_bar {
            bar.inc(1);
        }
    }
    
    /// 设置主进度消息
    pub fn set_main_message(&self, msg: &str) {
        if let Some(bar) = &self.main_bar {
            bar.set_message(msg.to_string());
        }
    }
    
    /// 设置子进度消息
    pub fn set_sub_message(&self, msg: &str) {
        if let Some(bar) = &self.sub_bar {
            bar.set_message(msg.to_string());
        }
    }
    
    /// 完成所有进度条
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
        assert_eq!(truncate_filename("very_long_filename_that_needs_truncation.txt", 20).len(), 20);
    }
}
