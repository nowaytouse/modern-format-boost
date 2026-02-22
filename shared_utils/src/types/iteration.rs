//! IterationGuard - 迭代次数守卫
//!
//! 防止无限循环，提供迭代次数边界保护。

use std::fmt;

pub use crate::crf_constants::{EMERGENCY_MAX_ITERATIONS, NORMAL_MAX_ITERATIONS};

pub const LONG_VIDEO_THRESHOLD_SECS: f32 = 300.0;

pub const VERY_LONG_VIDEO_THRESHOLD_SECS: f32 = 600.0;

pub const LONG_VIDEO_FALLBACK_ITERATIONS: u32 = 100;

pub const VERY_LONG_VIDEO_FALLBACK_ITERATIONS: u32 = 80;


#[derive(Debug, Clone, PartialEq)]
pub struct IterationError {
    pub current: u32,
    pub max: u32,
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


#[derive(Debug, Clone)]
pub struct IterationGuard {
    current: u32,
    max: u32,
    context: String,
}

impl IterationGuard {
    pub fn new(max: u32, context: &str) -> Self {
        let safe_max = max.min(EMERGENCY_MAX_ITERATIONS);
        Self {
            current: 0,
            max: safe_max,
            context: context.to_string(),
        }
    }

    pub fn for_duration(duration_secs: f32, ultimate_mode: bool, context: &str) -> Self {
        let max = calculate_max_iterations_for_duration(duration_secs, ultimate_mode);
        Self::new(max, context)
    }

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

    #[inline]
    pub fn current(&self) -> u32 {
        self.current
    }

    #[inline]
    pub fn max(&self) -> u32 {
        self.max
    }

    #[inline]
    pub fn remaining(&self) -> u32 {
        self.max.saturating_sub(self.current)
    }

    #[inline]
    pub fn is_exhausted(&self) -> bool {
        self.current >= self.max
    }

    pub fn reset(&mut self) {
        self.current = 0;
    }

    pub fn progress_percent(&self) -> f64 {
        if self.max == 0 {
            100.0
        } else {
            (self.current as f64 / self.max as f64) * 100.0
        }
    }

    pub fn context(&self) -> &str {
        &self.context
    }
}


pub fn calculate_max_iterations_for_duration(duration_secs: f32, ultimate_mode: bool) -> u32 {
    if duration_secs >= VERY_LONG_VIDEO_THRESHOLD_SECS {
        VERY_LONG_VIDEO_FALLBACK_ITERATIONS
    } else if duration_secs >= LONG_VIDEO_THRESHOLD_SECS {
        LONG_VIDEO_FALLBACK_ITERATIONS
    } else if ultimate_mode {
        200
    } else {
        NORMAL_MAX_ITERATIONS
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iteration_guard_basic() {
        let mut guard = IterationGuard::new(5, "test");

        for i in 1..=5 {
            assert_eq!(guard.increment().unwrap(), i);
        }

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
        let guard = IterationGuard::new(1000, "test");
        assert_eq!(guard.max(), EMERGENCY_MAX_ITERATIONS);
    }

    #[test]
    fn test_iteration_guard_for_duration() {
        let guard = IterationGuard::for_duration(60.0, false, "short");
        assert_eq!(guard.max(), NORMAL_MAX_ITERATIONS);

        let guard = IterationGuard::for_duration(400.0, false, "long");
        assert_eq!(guard.max(), LONG_VIDEO_FALLBACK_ITERATIONS);

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
