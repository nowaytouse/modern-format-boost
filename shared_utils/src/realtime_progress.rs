//! 🔥 v5.31: 真实进度映射的实时进度条
//!
//! 特点：
//! - 统一样式: ████████▓▓░░░░░░ (更粗更显眼)
//! - 🔥 真实进度映射：基于 CRF 搜索范围计算真实进度
//! - 纯粹的进度显示，无阻塞，无超时
//! - 原子操作更新状态，无锁竞争

use crate::modern_ui::progress_style;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 实时探索进度条 - 基于 CRF 范围的真实进度映射
///
/// 进度计算方式：
/// - 基于 CRF 搜索范围 [min_crf, max_crf]
/// - 当前进度 = (current_crf - min_crf) / (max_crf - min_crf)
/// - 这样进度条能真实反映搜索进度
pub struct RealtimeExploreProgress {
    pub bar: ProgressBar, // 公开以便 suspend 使用
    input_size: u64,
    // CRF 范围 - 用于计算真实进度
    min_crf: AtomicU64, // f32 as bits
    max_crf: AtomicU64, // f32 as bits
    // 原子状态 - 无锁更新
    current_crf: AtomicU64,  // f32 as bits
    current_size: AtomicU64,
    current_ssim: AtomicU64, // f64 as bits, 0 = None
    iterations: AtomicU64,
    best_crf: AtomicU64, // f32 as bits
    is_finished: AtomicBool,
}

impl RealtimeExploreProgress {
    /// 创建实时进度条（默认 CRF 范围 1-51）
    pub fn new(stage: &str, input_size: u64) -> Arc<Self> {
        Self::with_crf_range(stage, input_size, 1.0, 51.0)
    }

    /// 🔥 v5.31: 基于 CRF 范围创建进度条 - 真实进度映射
    pub fn with_crf_range(stage: &str, input_size: u64, min_crf: f32, max_crf: f32) -> Arc<Self> {
        // 进度条总长度 = 100（百分比）
        let bar = ProgressBar::new(100);

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

        // 🔥 v5.32: 禁用 steady_tick 避免刷屏，改用手动 tick
        // steady_tick 会导致终端刷屏问题
        // 🔥 v5.33: 增加刷新率到 10Hz，让进度条更实时
        bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(10)); // 平衡刷新率和防刷屏

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

    /// 兼容旧 API - 基于迭代次数（内部转换为 CRF 范围）
    pub fn with_max_iterations(stage: &str, input_size: u64, _max_iter: u64) -> Arc<Self> {
        // 忽略 max_iter，使用默认 CRF 范围
        Self::with_crf_range(stage, input_size, 1.0, 51.0)
    }

    /// 动态更新 CRF 搜索范围（用于二分搜索缩小范围时）
    pub fn set_crf_range(&self, min_crf: f32, max_crf: f32) {
        self.min_crf.store(min_crf.to_bits() as u64, Ordering::Relaxed);
        self.max_crf.store(max_crf.to_bits() as u64, Ordering::Relaxed);
    }

    /// 更新阶段名称
    pub fn set_stage(&self, stage: &str) {
        self.bar.set_prefix(stage.to_string());
    }

    /// 🔥 v5.31: 更新进度 - 基于 CRF 计算真实进度
    pub fn update(&self, crf: f32, size: u64, ssim: Option<f64>) {
        // 原子更新状态
        self.current_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        self.current_size.store(size, Ordering::Relaxed);
        if let Some(s) = ssim {
            self.current_ssim.store(s.to_bits(), Ordering::Relaxed);
        }
        self.iterations.fetch_add(1, Ordering::Relaxed);

        // 更新最佳 CRF（如果能压缩）
        if size < self.input_size {
            self.best_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        }

        // 🔥 计算真实进度：基于 CRF 在搜索范围中的位置
        let min = f32::from_bits(self.min_crf.load(Ordering::Relaxed) as u32);
        let max = f32::from_bits(self.max_crf.load(Ordering::Relaxed) as u32);
        let range = (max - min).max(1.0);
        let progress = ((crf - min) / range * 100.0).clamp(0.0, 100.0) as u64;
        self.bar.set_position(progress);

        // 更新消息
        self.refresh_message();

        // 🔥 v5.33: 立即刷新进度条显示，不等待下一个 Hz 周期
        self.bar.tick();
    }

    /// 刷新消息显示
    fn refresh_message(&self) {
        let crf = f32::from_bits(self.current_crf.load(Ordering::Relaxed) as u32);
        let size = self.current_size.load(Ordering::Relaxed);
        let ssim_bits = self.current_ssim.load(Ordering::Relaxed);
        let iter = self.iterations.load(Ordering::Relaxed);
        let best_crf = f32::from_bits(self.best_crf.load(Ordering::Relaxed) as u32);

        // 计算大小变化
        let size_pct = if self.input_size > 0 {
            ((size as f64 / self.input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };

        // 压缩图标
        let icon = if size < self.input_size { "💾" } else { "📈" };

        // SSIM 字符串
        let ssim_str = if ssim_bits != 0 {
            let ssim = f64::from_bits(ssim_bits);
            format!("SSIM {:.4}", ssim)
        } else {
            String::new()
        };

        // 最佳 CRF
        let best_str = if best_crf > 0.0 {
            format!("Best: {:.1}", best_crf)
        } else {
            String::new()
        };

        // 构建消息
        let msg = format!(
            "CRF {:.1} | {:+.1}% {} | {} | {} | Iter {}",
            crf, size_pct, icon, ssim_str, best_str, iter
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
        let iter = self.iterations.load(Ordering::Relaxed);

        let ssim_str = final_ssim
            .map(|s| format!("SSIM {:.4}", s))
            .unwrap_or_default();

        let icon = if size_pct < 0.0 { "✅" } else { "⚠️" };

        let msg = format!(
            "CRF {:.1} • {:+.1}% {} • {} • {} iterations",
            final_crf, size_pct, icon, ssim_str, iter
        );

        self.bar.set_position(100); // 完成时设为 100%
        self.bar.finish_with_message(msg);
    }

    /// 失败时结束
    pub fn fail(&self, error: &str) {
        self.is_finished.store(true, Ordering::Relaxed);
        self.bar.abandon_with_message(format!("❌ {}", error));
    }
}

impl Drop for RealtimeExploreProgress {
    fn drop(&mut self) {
        // 确保进度条被正确清理，不阻塞
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
    /// 创建 Spinner - 🔥 v5.30 统一样式
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
    
    /// 更新消息
    pub fn set_message(&self, msg: &str) {
        self.bar.set_message(msg.to_string());
    }
    
    /// 成功完成
    pub fn finish_success(&self, msg: &str) {
        self.bar.finish_with_message(format!("✅ {}", msg));
    }
    
    /// 失败完成
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
    use std::time::Duration;
    
    #[test]
    fn test_realtime_progress_no_block() {
        let progress = RealtimeExploreProgress::new("Test", 1000);
        
        // 模拟更新
        for i in 1..=5 {
            progress.update(20.0 + i as f32, 900 - i * 50, Some(0.95 + i as f64 * 0.01));
            thread::sleep(Duration::from_millis(100));
        }
        
        progress.finish(22.0, 800, Some(0.98));
    }
    
    #[test]
    fn test_spinner_no_block() {
        let spinner = RealtimeSpinner::new("Processing...");
        thread::sleep(Duration::from_millis(300));
        spinner.set_message("Almost done...");
        thread::sleep(Duration::from_millis(200));
        spinner.finish_success("Done!");
    }
}
