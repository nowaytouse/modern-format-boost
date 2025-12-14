//! 🔥 v5.21: 真正的条状实时进度条
//!
//! 特点：
//! - 真正的条状进度条（不是 Spinner）
//! - 彩色渐变显示
//! - 后台线程自动更新
//! - 原子操作更新状态，无锁竞争
//! - 自动清理，不会死循环

use indicatif::{ProgressBar, ProgressStyle, ProgressDrawTarget};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 实时探索进度条 - 真正的条状进度条
/// 
/// 使用 indicatif 的 steady_tick 实现真正的实时更新
pub struct RealtimeExploreProgress {
    bar: ProgressBar,
    input_size: u64,
    max_iterations: u64,
    // 原子状态 - 无锁更新
    current_crf: AtomicU64,      // f32 as bits
    current_size: AtomicU64,
    current_ssim: AtomicU64,     // f64 as bits, 0 = None
    iterations: AtomicU64,
    best_crf: AtomicU64,         // f32 as bits
    is_finished: AtomicBool,
}

impl RealtimeExploreProgress {
    /// 创建实时条状进度条
    pub fn new(stage: &str, input_size: u64) -> Arc<Self> {
        Self::with_max_iterations(stage, input_size, 20) // 默认最大 20 次迭代
    }
    
    /// 创建带最大迭代次数的进度条
    pub fn with_max_iterations(stage: &str, input_size: u64, max_iter: u64) -> Arc<Self> {
        let bar = ProgressBar::new(max_iter);
        
        // 🔥 v5.21: 真正的条状进度条样式
        bar.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} {prefix:.cyan.bold} ▕{bar:25.green/black}▏ {percent:>3}% • {pos}/{len} iter • ⏱️ {elapsed_precise} • {msg}")
                .expect("Invalid template")
                .progress_chars("━━─")  // 彩色条状字符
        );
        bar.set_prefix(stage.to_string());
        bar.set_message("Initializing...");
        
        // 🔥 关键：启用 steady_tick，后台线程自动更新
        bar.enable_steady_tick(Duration::from_millis(80));
        
        // 高刷新率
        bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
        
        Arc::new(Self {
            bar,
            input_size,
            max_iterations: max_iter,
            current_crf: AtomicU64::new(0),
            current_size: AtomicU64::new(0),
            current_ssim: AtomicU64::new(0),
            iterations: AtomicU64::new(0),
            best_crf: AtomicU64::new(0),
            is_finished: AtomicBool::new(false),
        })
    }
    
    /// 更新阶段名称
    pub fn set_stage(&self, stage: &str) {
        self.bar.set_prefix(stage.to_string());
    }
    
    /// 更新当前测试状态
    pub fn update(&self, crf: f32, size: u64, ssim: Option<f64>) {
        // 原子更新状态
        self.current_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        self.current_size.store(size, Ordering::Relaxed);
        if let Some(s) = ssim {
            self.current_ssim.store(s.to_bits(), Ordering::Relaxed);
        }
        let iter = self.iterations.fetch_add(1, Ordering::Relaxed) + 1;
        
        // 更新最佳 CRF（如果能压缩）
        if size < self.input_size {
            self.best_crf.store(crf.to_bits() as u64, Ordering::Relaxed);
        }
        
        // 🔥 更新进度条位置
        self.bar.set_position(iter.min(self.max_iterations));
        
        // 更新消息
        self.refresh_message();
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
        // 确保进度条被正确清理
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
    /// 创建 Spinner
    pub fn new(message: &str) -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("Invalid template")
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
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
