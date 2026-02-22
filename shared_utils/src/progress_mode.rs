//! 🔥 v7.3.2: Progress Mode - 控制进度条显示模式
//!
//! 解决并行处理时进度条输出混乱的问题

use std::sync::atomic::{AtomicBool, Ordering};

static QUIET_MODE: AtomicBool = AtomicBool::new(false);

pub fn enable_quiet_mode() {
    QUIET_MODE.store(true, Ordering::Relaxed);
}

pub fn disable_quiet_mode() {
    QUIET_MODE.store(false, Ordering::Relaxed);
}

pub fn is_quiet_mode() -> bool {
    QUIET_MODE.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! quiet_eprintln {
    ($($arg:tt)*) => {
        if !$crate::progress_mode::is_quiet_mode() {
            eprintln!($($arg)*);
        }
    };
}

pub fn create_conditional_progress(total: u64, prefix: &str) -> indicatif::ProgressBar {
    if is_quiet_mode() {
        indicatif::ProgressBar::hidden()
    } else {
        crate::create_progress_bar(total, prefix)
    }
}
