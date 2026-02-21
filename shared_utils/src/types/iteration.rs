//! IterationGuard - 迭代次数守卫
//!
//! 防止无限循环，提供迭代次数边界保护。

use std::fmt;

// 从 crf_constants 导入统一的迭代限制常量（单一来源）
pub use crate::crf_constants::{EMERGENCY_MAX_ITERATIONS, NORMAL_MAX_ITERATIONS};

/// 长视频阈值（秒）
pub const LONG_VIDEO_THRESHOLD_SECS: f32 = 300.0;

/// 超长视频阈值（秒）
pub const VERY_LONG_VIDEO_THRESHOLD_SECS: f32 = 600.0;

/// 长视频保底迭代上限
pub const LONG_VIDEO_FALLBACK_ITERATIONS: u32 = 100;

/// 超长视频保底迭代上限
pub const VERY_LONG_VIDEO_FALLBACK_ITERATIONS: u32 = 80;

// ============================================================================
// IterationError
// ============================================================================

/// 迭代错误类型
#[derive(Debug, Clone, PartialEq)]
pub struct IterationError {
    /// 当前迭代次数
    pub current: u32,
    /// 最大迭代次数
    pub max: u32,
    /// 上下文描述
    pub context: String,
}

impl fmt::Display for IterationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Iteration limit exceeded: {}/{} in {}",
            self.current, self.max, self.context
        )
    }
}

impl std::error::Error for IterationError {}

// ============================================================================
// IterationGuard
// ============================================================================

/// 迭代次数守卫，防止无限循环
///
/// # Examples
/// ```
/// use shared_utils::types::iteration::IterationGuard;
///
/// let mut guard = IterationGuard::new(10, "test loop");
///
/// for _ in 0..10 {
///     guard.increment().unwrap();
/// }
///
/// // 第 11 次会返回错误
/// assert!(guard.increment().is_err());
/// ```
#[derive(Debug, Clone)]
pub struct IterationGuard {
    current: u32,
    max: u32,
    context: String,
}

impl IterationGuard {
    /// 创建守卫
    ///
    /// # Arguments
    /// * `max` - 最大迭代次数
    /// * `context` - 上下文描述（用于错误消息）
    pub fn new(max: u32, context: &str) -> Self {
        // 确保不超过紧急保底
        let safe_max = max.min(EMERGENCY_MAX_ITERATIONS);
        Self {
            current: 0,
            max: safe_max,
            context: context.to_string(),
        }
    }

    /// 根据视频时长创建（自动计算上限）
    ///
    /// # Arguments
    /// * `duration_secs` - 视频时长（秒）
    /// * `ultimate_mode` - 是否为极限模式
    /// * `context` - 上下文描述
    pub fn for_duration(duration_secs: f32, ultimate_mode: bool, context: &str) -> Self {
        let max = calculate_max_iterations_for_duration(duration_secs, ultimate_mode);
        Self::new(max, context)
    }

    /// 递增并检查是否超限
    ///
    /// # Returns
    /// * `Ok(current)` - 当前迭代次数
    /// * `Err(IterationError)` - 如果超过最大迭代次数
    pub fn increment(&mut self) -> Result<u32, IterationError> {
        self.current += 1;
        if self.current > self.max {
            Err(IterationError {
                current: self.current,
                max: self.max,
                context: self.context.clone(),
            })
        } else {
            Ok(self.current)
        }
    }

    /// 当前迭代次数
    #[inline]
    pub fn current(&self) -> u32 {
        self.current
    }

    /// 最大迭代次数
    #[inline]
    pub fn max(&self) -> u32 {
        self.max
    }

    /// 剩余迭代次数
    #[inline]
    pub fn remaining(&self) -> u32 {
        self.max.saturating_sub(self.current)
    }

    /// 是否已达到上限
    #[inline]
    pub fn is_exhausted(&self) -> bool {
        self.current >= self.max
    }

    /// 重置计数器
    pub fn reset(&mut self) {
        self.current = 0;
    }

    /// 获取进度百分比
    pub fn progress_percent(&self) -> f64 {
        if self.max == 0 {
            100.0
        } else {
            (self.current as f64 / self.max as f64) * 100.0
        }
    }

    /// 获取上下文
    pub fn context(&self) -> &str {
        &self.context
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// 根据视频时长计算最大迭代次数
///
/// # Arguments
/// * `duration_secs` - 视频时长（秒）
/// * `ultimate_mode` - 是否为极限模式
///
/// # Returns
/// 保底迭代上限
pub fn calculate_max_iterations_for_duration(duration_secs: f32, ultimate_mode: bool) -> u32 {
    if duration_secs >= VERY_LONG_VIDEO_THRESHOLD_SECS {
        VERY_LONG_VIDEO_FALLBACK_ITERATIONS
    } else if duration_secs >= LONG_VIDEO_THRESHOLD_SECS {
        LONG_VIDEO_FALLBACK_ITERATIONS
    } else if ultimate_mode {
        // 极限模式：更高的上限
        200
    } else {
        NORMAL_MAX_ITERATIONS
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iteration_guard_basic() {
        let mut guard = IterationGuard::new(5, "test");

        for i in 1..=5 {
            assert_eq!(guard.increment().unwrap(), i);
        }

        // 第 6 次应该失败
        assert!(guard.increment().is_err());
    }

    #[test]
    fn test_iteration_guard_remaining() {
        let mut guard = IterationGuard::new(10, "test");

        assert_eq!(guard.remaining(), 10);
        guard.increment().unwrap();
        assert_eq!(guard.remaining(), 9);
    }

    #[test]
    fn test_iteration_guard_reset() {
        let mut guard = IterationGuard::new(5, "test");

        for _ in 0..3 {
            guard.increment().unwrap();
        }
        assert_eq!(guard.current(), 3);

        guard.reset();
        assert_eq!(guard.current(), 0);
    }

    #[test]
    fn test_iteration_guard_emergency_limit() {
        // 即使请求更高的上限，也会被钳制到 EMERGENCY_MAX_ITERATIONS
        let guard = IterationGuard::new(1000, "test");
        assert_eq!(guard.max(), EMERGENCY_MAX_ITERATIONS);
    }

    #[test]
    fn test_iteration_guard_for_duration() {
        // 短视频
        let guard = IterationGuard::for_duration(60.0, false, "short");
        assert_eq!(guard.max(), NORMAL_MAX_ITERATIONS);

        // 长视频
        let guard = IterationGuard::for_duration(400.0, false, "long");
        assert_eq!(guard.max(), LONG_VIDEO_FALLBACK_ITERATIONS);

        // 超长视频
        let guard = IterationGuard::for_duration(700.0, false, "very long");
        assert_eq!(guard.max(), VERY_LONG_VIDEO_FALLBACK_ITERATIONS);
    }

    #[test]
    fn test_iteration_error_display() {
        let error = IterationError {
            current: 101,
            max: 100,
            context: "CRF exploration".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "Iteration limit exceeded: 101/100 in CRF exploration"
        );
    }

    #[test]
    fn test_progress_percent() {
        let mut guard = IterationGuard::new(100, "test");
        assert_eq!(guard.progress_percent(), 0.0);

        for _ in 0..50 {
            guard.increment().unwrap();
        }
        assert_eq!(guard.progress_percent(), 50.0);
    }
}
