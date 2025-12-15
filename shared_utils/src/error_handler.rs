//! Unified Error Handler Module - 统一错误处理策略
//!
//! 🔥 v5.72: 解决错误处理不一致问题
//!
//! ## 错误分类
//! - Recoverable: 可恢复错误，记录警告并使用回退
//! - Fatal: 致命错误，传播错误并中断
//! - Optional: 可选操作失败，记录并继续

use std::fmt;

/// 错误类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 可恢复错误：记录警告，使用回退值继续
    /// 例如：元数据读取失败、SSIM计算失败
    Recoverable,
    /// 致命错误：传播错误，中断操作
    /// 例如：编码器启动失败、输入文件不存在
    Fatal,
    /// 可选操作失败：记录并继续，不影响主操作
    /// 例如：时间戳保留失败、缓存写入失败
    Optional,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorCategory::Recoverable => write!(f, "RECOVERABLE"),
            ErrorCategory::Fatal => write!(f, "FATAL"),
            ErrorCategory::Optional => write!(f, "OPTIONAL"),
        }
    }
}

/// 错误处理结果
#[derive(Debug)]
pub enum ErrorAction {
    /// 继续执行（用于Recoverable和Optional）
    Continue,
    /// 中断执行（用于Fatal）
    Abort(anyhow::Error),
}

/// 统一错误处理函数
///
/// # Arguments
/// * `category` - 错误类别
/// * `context` - 错误上下文描述
/// * `error` - 错误信息
/// * `suggestion` - 建议操作（可选）
///
/// # Returns
/// * `ErrorAction::Continue` - 对于Recoverable和Optional
/// * `ErrorAction::Abort` - 对于Fatal
pub fn handle_error<E: std::error::Error + Send + Sync + 'static>(
    category: ErrorCategory,
    context: &str,
    error: E,
    suggestion: Option<&str>,
) -> ErrorAction {
    let suggestion_str = suggestion.unwrap_or("No specific action required");
    
    match category {
        ErrorCategory::Recoverable => {
            eprintln!("⚠️ [{}] {}: {}", category, context, error);
            eprintln!("   → Suggested action: {}", suggestion_str);
            eprintln!("   → Continuing with fallback behavior...");
            ErrorAction::Continue
        }
        ErrorCategory::Fatal => {
            eprintln!("❌ [{}] {}: {}", category, context, error);
            eprintln!("   → Suggested action: {}", suggestion_str);
            eprintln!("   → Operation aborted.");
            ErrorAction::Abort(anyhow::anyhow!("{}: {}", context, error))
        }
        ErrorCategory::Optional => {
            eprintln!("ℹ️ [{}] {}: {}", category, context, error);
            eprintln!("   → This is non-critical, continuing...");
            ErrorAction::Continue
        }
    }
}

/// 简化的错误处理宏 - 用于Recoverable错误
#[macro_export]
macro_rules! handle_recoverable {
    ($context:expr, $error:expr) => {
        $crate::error_handler::handle_error(
            $crate::error_handler::ErrorCategory::Recoverable,
            $context,
            $error,
            None,
        )
    };
    ($context:expr, $error:expr, $suggestion:expr) => {
        $crate::error_handler::handle_error(
            $crate::error_handler::ErrorCategory::Recoverable,
            $context,
            $error,
            Some($suggestion),
        )
    };
}

/// 简化的错误处理宏 - 用于Optional错误
#[macro_export]
macro_rules! handle_optional {
    ($context:expr, $error:expr) => {
        $crate::error_handler::handle_error(
            $crate::error_handler::ErrorCategory::Optional,
            $context,
            $error,
            None,
        )
    };
}

/// 简化的错误处理宏 - 用于Fatal错误（返回Result）
#[macro_export]
macro_rules! handle_fatal {
    ($context:expr, $error:expr) => {{
        let action = $crate::error_handler::handle_error(
            $crate::error_handler::ErrorCategory::Fatal,
            $context,
            $error,
            None,
        );
        match action {
            $crate::error_handler::ErrorAction::Abort(e) => Err(e),
            _ => unreachable!(),
        }
    }};
    ($context:expr, $error:expr, $suggestion:expr) => {{
        let action = $crate::error_handler::handle_error(
            $crate::error_handler::ErrorCategory::Fatal,
            $context,
            $error,
            Some($suggestion),
        );
        match action {
            $crate::error_handler::ErrorAction::Abort(e) => Err(e),
            _ => unreachable!(),
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_recoverable_error() {
        let error = io::Error::new(io::ErrorKind::NotFound, "test error");
        let action = handle_error(
            ErrorCategory::Recoverable,
            "Reading metadata",
            error,
            Some("Use default values"),
        );
        assert!(matches!(action, ErrorAction::Continue));
    }

    #[test]
    fn test_fatal_error() {
        let error = io::Error::new(io::ErrorKind::NotFound, "encoder not found");
        let action = handle_error(
            ErrorCategory::Fatal,
            "Starting encoder",
            error,
            Some("Install ffmpeg"),
        );
        assert!(matches!(action, ErrorAction::Abort(_)));
    }

    #[test]
    fn test_optional_error() {
        let error = io::Error::new(io::ErrorKind::PermissionDenied, "cannot set timestamp");
        let action = handle_error(
            ErrorCategory::Optional,
            "Preserving timestamp",
            error,
            None,
        );
        assert!(matches!(action, ErrorAction::Continue));
    }
}


// ═══════════════════════════════════════════════════════════════
// 属性测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod prop_tests {
    use super::*;
    use std::io;

    // **Feature: video-explorer-robustness-v5.72, Property 6: 错误处理一致性**
    // **Validates: Requirements 3.1, 3.2, 3.3**
    #[test]
    fn prop_error_handling_consistency() {
        // 测试每种错误类别的响应行为一致性
        let test_cases = vec![
            (ErrorCategory::Recoverable, true),  // 应该返回Continue
            (ErrorCategory::Fatal, false),       // 应该返回Abort
            (ErrorCategory::Optional, true),     // 应该返回Continue
        ];

        for (category, should_continue) in test_cases {
            let error = io::Error::new(io::ErrorKind::Other, "test error");
            let action = handle_error(category, "test context", error, None);
            
            let is_continue = matches!(action, ErrorAction::Continue);
            assert_eq!(is_continue, should_continue,
                "Category {:?} should {} but got {}",
                category,
                if should_continue { "continue" } else { "abort" },
                if is_continue { "continue" } else { "abort" }
            );
        }
    }

    #[test]
    fn prop_error_category_display() {
        // 测试错误类别的显示格式
        assert_eq!(format!("{}", ErrorCategory::Recoverable), "RECOVERABLE");
        assert_eq!(format!("{}", ErrorCategory::Fatal), "FATAL");
        assert_eq!(format!("{}", ErrorCategory::Optional), "OPTIONAL");
    }
}
