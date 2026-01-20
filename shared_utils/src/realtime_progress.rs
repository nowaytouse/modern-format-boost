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
    // 🔥 v7.7: 进度条守卫(自动注册/注销)
    _progress_guard: Option<crate::heartbeat_manager::ProgressBarGuard>,
}

impl SimpleIterationProgress {
    /// 创建新的迭代进度条
    ///
    /// # 参数
    /// - stage: 阶段名称，如"🔍 GPU Search"或"🔬 CPU Fine"
    /// - input_size: 输入文件大小（字节）
    /// - total_iterations: 预期总迭代次数（用于计算进度）
    pub fn new(stage: &str, input_size: u64, total_iterations: u64) -> Arc<Self> {
        let bar = ProgressBar::new(total_iterations);

        // 🔥 v7.4.4: 在 quiet_mode 下隐藏进度条
        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
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

            // 🔥 v5.39: 使用超快刷新率 100Hz 覆盖任何键盘输入
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(100));
        }

        // 🔥 v7.7: 注册进度条(用于心跳静默检测)
        let progress_guard = if !crate::progress_mode::is_quiet_mode() {
            Some(crate::heartbeat_manager::ProgressBarGuard::new())
        } else {
            None
        };

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
            _progress_guard: progress_guard,
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

    /// 🔥 v5.80: 暂停进度条，输出日志
    ///
    /// 这是统一的日志输出方法，确保日志不会与进度条冲突
    ///
    /// # 用法
    /// ```ignore
    /// let progress = SimpleIterationProgress::new("🔍 Search", 1000000, 20);
    /// progress.println("⚠️ Warning: something happened");
    /// progress.println("✅ Step completed");
    /// ```
    pub fn println(&self, msg: &str) {
        self.bar.suspend(|| {
            eprintln!("{}", msg);
        });
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
    }

    /// 失败结束
    pub fn fail(&self, error: &str) {
        self.is_finished.store(true, Ordering::Relaxed);
        self.bar.abandon_with_message(format!("❌ {}", error));
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

        // 🔥 v7.4.4: 在 quiet_mode 下隐藏进度条
        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(progress_style::EXPLORE_TEMPLATE)
                    .expect("Invalid template")
                    .progress_chars(progress_style::PROGRESS_CHARS)
                    .tick_chars(progress_style::SPINNER_CHARS),
            );
            bar.set_prefix(stage.to_string());
            bar.set_message("Initializing...");

            // 🔥 v5.39: 使用超快刷新率 100Hz 覆盖任何键盘输入
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(100));
        }

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
        
        // 🔥 v7.4.4: 在 quiet_mode 下隐藏进度条
        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
                    .expect("Invalid template")
                    .tick_chars(progress_style::SPINNER_CHARS)
            );
            bar.set_message(message.to_string());
            bar.enable_steady_tick(Duration::from_millis(80));
        }

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


// ═══════════════════════════════════════════════════════════════
// 🔥 v5.72: 增强进度状态 - 更详细的透明度
// ═══════════════════════════════════════════════════════════════

/// 🔥 v5.72: 详细进度状态 - 用于实时透明度
#[derive(Debug, Clone)]
pub struct DetailedProgressState {
    /// 当前阶段名称
    pub phase: String,
    /// 当前CRF值
    pub current_crf: f32,
    /// 当前SSIM值（如果已计算）
    pub current_ssim: Option<f64>,
    /// 文件大小变化百分比
    pub size_change_pct: f64,
    /// 当前迭代次数
    pub iteration: u32,
    /// 预估总迭代次数
    pub total_iterations: u32,
    /// 预估剩余时间（秒）
    pub eta_seconds: Option<f64>,
    /// SSIM趋势（最近3次的变化）
    pub ssim_trend: Vec<f64>,
    /// 文件大小趋势（最近3次的变化）
    pub size_trend: Vec<f64>,
}

impl DetailedProgressState {
    /// 创建新的进度状态
    pub fn new(phase: &str) -> Self {
        Self {
            phase: phase.to_string(),
            current_crf: 0.0,
            current_ssim: None,
            size_change_pct: 0.0,
            iteration: 0,
            total_iterations: 0,
            eta_seconds: None,
            ssim_trend: Vec::new(),
            size_trend: Vec::new(),
        }
    }

    /// 更新CRF和大小
    pub fn update_crf(&mut self, crf: f32, size_pct: f64) {
        self.current_crf = crf;
        self.size_change_pct = size_pct;
        self.size_trend.push(size_pct);
        if self.size_trend.len() > 3 {
            self.size_trend.remove(0);
        }
    }

    /// 更新SSIM
    pub fn update_ssim(&mut self, ssim: f64) {
        self.current_ssim = Some(ssim);
        self.ssim_trend.push(ssim);
        if self.ssim_trend.len() > 3 {
            self.ssim_trend.remove(0);
        }
    }

    /// 更新迭代进度
    pub fn update_iteration(&mut self, current: u32, total: u32, elapsed_secs: f64) {
        self.iteration = current;
        self.total_iterations = total;
        if current > 0 {
            let avg_time_per_iter = elapsed_secs / current as f64;
            let remaining = total.saturating_sub(current) as f64;
            self.eta_seconds = Some(avg_time_per_iter * remaining);
        }
    }

    /// 切换阶段
    pub fn set_phase(&mut self, phase: &str) {
        self.phase = phase.to_string();
        // 清空趋势数据
        self.ssim_trend.clear();
        self.size_trend.clear();
    }

    /// 格式化为显示字符串
    pub fn format_display(&self) -> String {
        let ssim_str = self.current_ssim
            .map(|s| format!("{:.4}", s))
            .unwrap_or_else(|| "---".to_string());
        
        let eta_str = self.eta_seconds
            .map(|e| format!("{:.0}s", e))
            .unwrap_or_else(|| "---".to_string());
        
        let trend_indicator = if self.ssim_trend.len() >= 2 {
            let last = self.ssim_trend.last().unwrap();
            let prev = self.ssim_trend[self.ssim_trend.len() - 2];
            if *last > prev { "↑" } else if *last < prev { "↓" } else { "→" }
        } else {
            "→"
        };

        format!(
            "[{}] CRF {:.1} | SSIM {} {} | Size {:+.1}% | {}/{} | ETA {}",
            self.phase,
            self.current_crf,
            ssim_str,
            trend_indicator,
            self.size_change_pct,
            self.iteration,
            self.total_iterations,
            eta_str
        )
    }

    /// 打印阶段切换信息
    pub fn print_phase_change(&self) {
        eprintln!("┌─────────────────────────────────────────────────────");
        eprintln!("│ 📍 Phase: {}", self.phase);
        eprintln!("└─────────────────────────────────────────────────────");
    }
}

#[cfg(test)]
mod detailed_progress_tests {
    use super::*;

    #[test]
    fn test_progress_state_creation() {
        let state = DetailedProgressState::new("GPU Coarse");
        assert_eq!(state.phase, "GPU Coarse");
        assert_eq!(state.iteration, 0);
    }

    #[test]
    fn test_progress_state_update() {
        let mut state = DetailedProgressState::new("CPU Fine");
        state.update_crf(18.5, -15.3);
        state.update_ssim(0.9523);
        state.update_iteration(5, 20, 10.0);
        
        assert!((state.current_crf - 18.5).abs() < 0.01);
        assert_eq!(state.current_ssim, Some(0.9523));
        assert_eq!(state.iteration, 5);
        assert!(state.eta_seconds.is_some());
    }

    #[test]
    fn test_progress_state_format() {
        let mut state = DetailedProgressState::new("Test");
        state.update_crf(20.0, -10.0);
        state.update_ssim(0.95);
        state.update_iteration(3, 10, 6.0);
        
        let display = state.format_display();
        assert!(display.contains("Test"));
        assert!(display.contains("20.0"));
        assert!(display.contains("0.9500"));
    }
}
