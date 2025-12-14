//! 🔥 v5.35: 重构进度条系统 - 基于迭代计数的实时更新 + 终端控制
//!
//! 核心改进：
//! - ✅ 弃用 CRF 范围映射（导致非线性失败）
//! - ✅ 改用迭代计数（真实反映搜索进度）
//! - ✅ 每次编码即时更新，无延迟
//! - ✅ 分离 GPU 和 CPU 两个进度条
//! - ✅ 20Hz 刷新率确保实时显示
//! - ✅ 精确的时间戳连续递增
//! - ✅ 禁用终端echo防止键盘干扰（v5.35）

use crate::modern_ui::progress_style;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 🔥 v5.35: 终端原始模式控制 - 防止键盘输入干扰
/// 在Unix系统上禁用echo，Windows上无操作
#[allow(dead_code)]
fn disable_terminal_echo() {
    #[cfg(unix)]
    {
        use std::process::Command;
        // 使用stty禁用echo（Unix/Linux/macOS）
        let _ = Command::new("stty")
            .arg("-echo")
            .output();
    }
}

#[allow(dead_code)]
fn restore_terminal_echo() {
    #[cfg(unix)]
    {
        use std::process::Command;
        // 恢复echo设置
        let _ = Command::new("stty")
            .arg("echo")
            .output();
    }
}

/// 🔥 v5.34: 简单迭代进度条 - 基于真实迭代次数
///
/// 这是新的核心进度显示机制，解决原有的CRF范围映射问题
pub struct SimpleIterationProgress {
    pub bar: ProgressBar,
    input_size: u64,
    total_iterations: u64,
    current_iteration: AtomicU64,
    // 状态信息
    current_crf: AtomicU64,         // f32 as bits
    current_size: AtomicU64,
    current_ssim: AtomicU64,        // f64 as bits
    best_crf: AtomicU64,            // f32 as bits
    // 时间追踪（保留以供将来使用）
    #[allow(dead_code)]
    start_time: Instant,
    #[allow(dead_code)]
    last_update: std::sync::Mutex<Instant>,
    is_finished: AtomicBool,
    /// 🔥 v5.35: 记录是否禁用了echo，便于恢复
    #[allow(dead_code)]
    echo_disabled: AtomicBool,
}

impl SimpleIterationProgress {
    /// 创建新的迭代进度条
    ///
    /// # 参数
    /// - stage: 阶段名称，如"🔍 GPU Search"或"🔬 CPU Fine"
    /// - input_size: 输入文件大小（字节）
    /// - total_iterations: 预期总迭代次数（用于计算进度）
    pub fn new(stage: &str, input_size: u64, total_iterations: u64) -> Arc<Self> {
        // 🔥 v5.35: 禁用终端echo防止键盘干扰
        disable_terminal_echo();

        let bar = ProgressBar::new(total_iterations);

        // 统一进度条样式
        bar.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::EXPLORE_TEMPLATE)
                .expect("Invalid template")
                .progress_chars(progress_style::PROGRESS_CHARS)
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        bar.set_prefix(stage.to_string());
        bar.set_message("Initializing...");

        // 🔥 v5.34: 20Hz 刷新率确保实时性
        bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));

        Arc::new(Self {
            bar,
            input_size,
            total_iterations,
            current_iteration: AtomicU64::new(0),
            current_crf: AtomicU64::new(0),
            current_size: AtomicU64::new(0),
            current_ssim: AtomicU64::new(0),
            best_crf: AtomicU64::new(0),
            start_time: Instant::now(),
            last_update: std::sync::Mutex::new(Instant::now()),
            is_finished: AtomicBool::new(false),
            echo_disabled: AtomicBool::new(true),  // 🔥 v5.35: 记录echo已禁用
        })
    }

    /// 更新单次迭代 - 🔥 v5.34 核心方法
    ///
    /// 每次编码完成后调用一次，立即更新进度
    ///
    /// # 参数
    /// - crf: 当前 CRF 值
    /// - size: 编码后的文件大小
    /// - ssim: 可选的 SSIM 值
    pub fn inc_iteration(&self, crf: f32, size: u64, ssim: Option<f64>) {
        // 递增迭代次数
        let iter = self.current_iteration.fetch_add(1, Ordering::Relaxed) + 1;

        // 原子更新状态
        self.current_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        self.current_size.store(size, Ordering::Relaxed);
        if let Some(s) = ssim {
            self.current_ssim.store(s.to_bits(), Ordering::Relaxed);
        }

        // 更新最佳 CRF
        if size < self.input_size {
            self.best_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        }

        // 🔥 直接设置进度 = 迭代数（最可靠的方式）
        self.bar.set_position(iter);

        // 构建消息
        self.update_message(iter, crf, size, ssim);

        // 🔥 v5.34: 强制立即刷新，不等待下一个 Hz 周期
        self.bar.tick();
    }

    /// 更新消息显示
    fn update_message(&self, iter: u64, crf: f32, size: u64, ssim: Option<f64>) {
        let size_pct = if self.input_size > 0 {
            ((size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };

        let icon = if size < self.input_size { "💾" } else { "📈" };

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

        let msg = format!(
            "CRF {:.1} | {:+.1}% {} | {} | {} | Iter {}/{}",
            crf, size_pct, icon, ssim_str, best_str, iter, self.total_iterations
        );

        self.bar.set_message(msg);
    }

    /// 完成进度条
    pub fn finish(&self, final_crf: f32, final_size: u64, final_ssim: Option<f64>) {
        self.is_finished.store(true, Ordering::Relaxed);

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

        let msg = format!(
            "CRF {:.1} • {:+.1}% {} • {} • {} iterations",
            final_crf, size_pct, icon, ssim_str, iter
        );

        self.bar.set_position(self.total_iterations);
        self.bar.finish_with_message(msg);

        // 🔥 v5.35: 恢复终端echo
        if self.echo_disabled.load(Ordering::Relaxed) {
            restore_terminal_echo();
        }
    }

    /// 失败结束
    pub fn fail(&self, error: &str) {
        self.is_finished.store(true, Ordering::Relaxed);
        self.bar.abandon_with_message(format!("❌ {}", error));

        // 🔥 v5.35: 恢复终端echo
        if self.echo_disabled.load(Ordering::Relaxed) {
            restore_terminal_echo();
        }
    }
}

impl Drop for SimpleIterationProgress {
    fn drop(&mut self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            self.bar.finish_and_clear();
        }
    }
}

/// 🔥 v5.31: 实时探索进度条 - 基于 CRF 范围的真实进度映射
///
/// 保留以确保向后兼容，但优先使用 SimpleIterationProgress
#[deprecated(since = "5.34", note = "使用 SimpleIterationProgress 替代")]
pub struct RealtimeExploreProgress {
    pub bar: ProgressBar,
    input_size: u64,
    min_crf: AtomicU64,
    max_crf: AtomicU64,
    current_crf: AtomicU64,
    current_size: AtomicU64,
    current_ssim: AtomicU64,
    iterations: AtomicU64,
    best_crf: AtomicU64,
    is_finished: AtomicBool,
}

#[allow(deprecated)]
impl RealtimeExploreProgress {
    pub fn new(stage: &str, input_size: u64) -> Arc<Self> {
        Self::with_crf_range(stage, input_size, 1.0, 51.0)
    }

    pub fn with_crf_range(stage: &str, input_size: u64, min_crf: f32, max_crf: f32) -> Arc<Self> {
        let bar = ProgressBar::new(100);

        bar.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::EXPLORE_TEMPLATE)
                .expect("Invalid template")
                .progress_chars(progress_style::PROGRESS_CHARS)
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        bar.set_prefix(stage.to_string());
        bar.set_message("Initializing...");

        bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));

        Arc::new(Self {
            bar,
            input_size,
            min_crf: AtomicU64::new(min_crf.to_bits() as u64),
            max_crf: AtomicU64::new(max_crf.to_bits() as u64),
            current_crf: AtomicU64::new(0),
            current_size: AtomicU64::new(0),
            current_ssim: AtomicU64::new(0),
            iterations: AtomicU64::new(0),
            best_crf: AtomicU64::new(0),
            is_finished: AtomicBool::new(false),
        })
    }

    pub fn with_max_iterations(stage: &str, input_size: u64, _max_iter: u64) -> Arc<Self> {
        Self::with_crf_range(stage, input_size, 1.0, 51.0)
    }

    pub fn set_crf_range(&self, min_crf: f32, max_crf: f32) {
        self.min_crf.store(min_crf.to_bits() as u64, Ordering::Relaxed);
        self.max_crf.store(max_crf.to_bits() as u64, Ordering::Relaxed);
    }

    pub fn set_stage(&self, stage: &str) {
        self.bar.set_prefix(stage.to_string());
    }

    pub fn update(&self, crf: f32, size: u64, ssim: Option<f64>) {
        self.current_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        self.current_size.store(size, Ordering::Relaxed);
        if let Some(s) = ssim {
            self.current_ssim.store(s.to_bits(), Ordering::Relaxed);
        }
        self.iterations.fetch_add(1, Ordering::Relaxed);

        if size < self.input_size {
            self.best_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        }

        let min = f32::from_bits(self.min_crf.load(Ordering::Relaxed) as u32);
        let max = f32::from_bits(self.max_crf.load(Ordering::Relaxed) as u32);
        let range = (max - min).max(1.0);
        let progress = ((crf - min) / range * 100.0).clamp(0.0, 100.0) as u64;
        self.bar.set_position(progress);

        self.refresh_message();
        self.bar.tick();
    }

    fn refresh_message(&self) {
        let crf = f32::from_bits(self.current_crf.load(Ordering::Relaxed) as u32);
        let size = self.current_size.load(Ordering::Relaxed);
        let ssim_bits = self.current_ssim.load(Ordering::Relaxed);
        let iter = self.iterations.load(Ordering::Relaxed);
        let best_crf = f32::from_bits(self.best_crf.load(Ordering::Relaxed) as u32);

        let size_pct = if self.input_size > 0 {
            ((size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };

        let icon = if size < self.input_size { "💾" } else { "📈" };

        let ssim_str = if ssim_bits != 0 {
            let ssim = f64::from_bits(ssim_bits);
            format!("SSIM {:.4}", ssim)
        } else {
            String::new()
        };

        let best_str = if best_crf > 0.0 {
            format!("Best: {:.1}", best_crf)
        } else {
            String::new()
        };

        let msg = format!(
            "CRF {:.1} | {:+.1}% {} | {} | {} | Iter {}",
            crf, size_pct, icon, ssim_str, best_str, iter
        );

        self.bar.set_message(msg);
    }

    pub fn finish(&self, final_crf: f32, final_size: u64, final_ssim: Option<f64>) {
        self.is_finished.store(true, Ordering::Relaxed);

        let size_pct = if self.input_size > 0 {
            ((final_size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };
        let iter = self.iterations.load(Ordering::Relaxed);

        let ssim_str = final_ssim
            .map(|s| format!("SSIM {:.4}", s))
            .unwrap_or_default();

        let icon = if size_pct < 0.0 { "✅" } else { "⚠️" };

        let msg = format!(
            "CRF {:.1} • {:+.1}% {} • {} • {} iterations",
            final_crf, size_pct, icon, ssim_str, iter
        );

        self.bar.set_position(100);
        self.bar.finish_with_message(msg);
    }

    pub fn fail(&self, error: &str) {
        self.is_finished.store(true, Ordering::Relaxed);
        self.bar.abandon_with_message(format!("❌ {}", error));
    }
}

#[allow(deprecated)]
impl Drop for RealtimeExploreProgress {
    fn drop(&mut self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            self.bar.finish_and_clear();
        }
    }
}

/// 简单的实时 Spinner（用于单个操作）
pub struct RealtimeSpinner {
    bar: ProgressBar,
}

impl RealtimeSpinner {
    pub fn new(message: &str) -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("Invalid template")
                .tick_chars(progress_style::SPINNER_CHARS)
        );
        bar.set_message(message.to_string());
        bar.enable_steady_tick(Duration::from_millis(80));

        Self { bar }
    }

    pub fn set_message(&self, msg: &str) {
        self.bar.set_message(msg.to_string());
    }

    pub fn finish_success(&self, msg: &str) {
        self.bar.finish_with_message(format!("✅ {}", msg));
    }

    pub fn finish_fail(&self, msg: &str) {
        self.bar.finish_with_message(format!("❌ {}", msg));
    }
}

impl Drop for RealtimeSpinner {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_simple_iteration_progress() {
        let progress = SimpleIterationProgress::new("Test", 1000, 10);

        for i in 0..10 {
            progress.inc_iteration(20.0 + i as f32, 900 - i * 50, Some(0.95 + i as f64 * 0.003));
            thread::sleep(Duration::from_millis(50));
        }

        progress.finish(22.0, 800, Some(0.98));
    }
}
