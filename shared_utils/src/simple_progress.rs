//! 🔥 v5.5: 简洁进度条模块
//!
//! 固定在终端底部，实时更新，防止用户以为程序卡住
//!
//! 格式: `[CRF 18.0] 编码中... 3/10 | -12.5% | 8.2s`

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// 全局简洁进度条
static PROGRESS_ENABLED: AtomicBool = AtomicBool::new(true);
static PROGRESS_ITER: AtomicU32 = AtomicU32::new(0);
static PROGRESS_START: Mutex<Option<Instant>> = Mutex::new(None);

/// 初始化进度条
pub fn progress_init() {
    PROGRESS_ENABLED.store(true, Ordering::Relaxed);
    PROGRESS_ITER.store(0, Ordering::Relaxed);
    if let Ok(mut start) = PROGRESS_START.lock() {
        *start = Some(Instant::now());
    }
}

/// 禁用进度条（用于非交互模式）
pub fn progress_disable() {
    PROGRESS_ENABLED.store(false, Ordering::Relaxed);
}

/// 更新进度条 - 固定底部单行显示
///
/// # Arguments
/// * `crf` - 当前 CRF 值
/// * `size_pct` - 大小变化百分比（负数=压缩）
/// * `status` - 状态字符 ('.' = 进行中, '✓' = 成功, '✗' = 失败)
pub fn progress_update(crf: f32, size_pct: f64, status: char) {
    if !PROGRESS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    
    let iter = PROGRESS_ITER.fetch_add(1, Ordering::Relaxed) + 1;
    let elapsed = PROGRESS_START.lock()
        .ok()
        .and_then(|s| s.map(|t| t.elapsed().as_secs_f64()))
        .unwrap_or(0.0);
    
    // 固定底部单行: \r 回到行首, \x1b[K 清除到行尾
    eprint!("\r\x1b[K[CRF {:.1}] {} {:+.1}% | iter {} | {:.1}s",
        crf, status, size_pct, iter, elapsed);
    let _ = io::stderr().flush();
}

/// 完成进度条 - 换行并显示最终结果
pub fn progress_finish(final_crf: f32, final_size_pct: f64, ssim: Option<f64>) {
    if !PROGRESS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    
    let iter = PROGRESS_ITER.load(Ordering::Relaxed);
    let elapsed = PROGRESS_START.lock()
        .ok()
        .and_then(|s| s.map(|t| t.elapsed().as_secs_f64()))
        .unwrap_or(0.0);
    
    // 清除进度行，打印最终结果
    eprint!("\r\x1b[K");
    
    let ssim_str = ssim.map(|s| format!(" SSIM {:.4}", s)).unwrap_or_default();
    eprintln!("✓ CRF {:.1} | {:+.1}%{} | {} iter | {:.1}s",
        final_crf, final_size_pct, ssim_str, iter, elapsed);
}

/// 进度条失败 - 显示错误信息
pub fn progress_fail(msg: &str) {
    if !PROGRESS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    
    eprint!("\r\x1b[K");
    eprintln!("✗ {}", msg);
}

// ═══════════════════════════════════════════════════════════════
// 便捷宏
// ═══════════════════════════════════════════════════════════════

/// 简洁进度更新宏
#[macro_export]
macro_rules! progress {
    ($crf:expr, $size_pct:expr) => {
        $crate::simple_progress::progress_update($crf, $size_pct, '.');
    };
    ($crf:expr, $size_pct:expr, ok) => {
        $crate::simple_progress::progress_update($crf, $size_pct, '✓');
    };
    ($crf:expr, $size_pct:expr, fail) => {
        $crate::simple_progress::progress_update($crf, $size_pct, '✗');
    };
}

/// 进度完成宏
#[macro_export]
macro_rules! progress_done {
    ($crf:expr, $size_pct:expr) => {
        $crate::simple_progress::progress_finish($crf, $size_pct, None);
    };
    ($crf:expr, $size_pct:expr, $ssim:expr) => {
        $crate::simple_progress::progress_finish($crf, $size_pct, Some($ssim));
    };
}
