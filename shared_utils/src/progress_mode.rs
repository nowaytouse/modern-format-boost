//! 🔥 v7.3.2: Progress Mode - 控制进度条显示模式
//! 
//! 解决并行处理时进度条输出混乱的问题

use std::sync::atomic::{AtomicBool, Ordering};

/// 全局进度条模式控制
static QUIET_MODE: AtomicBool = AtomicBool::new(false);

/// 启用安静模式（禁用详细的子进度条）
/// 
/// 在并行处理时调用此函数，避免多个线程的进度条互相干扰
pub fn enable_quiet_mode() {
    QUIET_MODE.store(true, Ordering::Relaxed);
}

/// 禁用安静模式（恢复详细进度条）
pub fn disable_quiet_mode() {
    QUIET_MODE.store(false, Ordering::Relaxed);
}

/// 检查是否处于安静模式
pub fn is_quiet_mode() -> bool {
    QUIET_MODE.load(Ordering::Relaxed)
}

/// 🔥 条件性打印 - 只在非安静模式下打印
/// 
/// # 示例
/// ```ignore
/// quiet_eprintln!("🔍 Starting GPU search...");
/// ```
#[macro_export]
macro_rules! quiet_eprintln {
    ($($arg:tt)*) => {
        if !$crate::progress_mode::is_quiet_mode() {
            eprintln!($($arg)*);
        }
    };
}

/// 🔥 条件性进度条创建
/// 
/// 在安静模式下返回隐藏的进度条，避免输出混乱
pub fn create_conditional_progress(total: u64, prefix: &str) -> indicatif::ProgressBar {
    if is_quiet_mode() {
        // 安静模式：创建隐藏的进度条
        indicatif::ProgressBar::hidden()
    } else {
        // 正常模式：创建可见的进度条
        crate::create_progress_bar(total, prefix)
    }
}
