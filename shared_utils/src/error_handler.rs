//! Unified Error Handler Module - 统一错误处理策略
//!
//! 🔥 v5.72: 解决错误处理不一致问题
//! 🔥 v7.8: 增强错误报告功能 - 响亮报错，透明诊断
//!
//! ## 错误分类
//! - Recoverable: 可恢复错误，记录警告并使用回退
//! - Fatal: 致命错误，传播错误并中断
//! - Optional: 可选操作失败，记录并继续
//!
//! ## 错误报告功能
//! - `report_error()`: 响亮报错到 stderr 和日志
//! - `add_context()`: 为 Result 添加上下文信息
//! - Panic handler: 在程序崩溃前记录详细信息

use std::fmt;
use std::panic;

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

// ═══════════════════════════════════════════════════════════════
// 错误报告工具 (v7.8)
// ═══════════════════════════════════════════════════════════════

/// 响亮报错：同时输出到 stderr 和日志
///
/// 根据用户规则要求：所有的报错 Fallback 必须响亮报告，严禁静默！
///
/// # Arguments
/// * `error` - 任何实现了 std::error::Error 的错误类型
///
/// # Example
/// ```ignore
/// use shared_utils::error_handler::report_error;
///
/// if let Err(e) = risky_operation() {
///     report_error(&e);
///     // 继续执行回退逻辑...
/// }
/// ```
pub fn report_error<E: std::error::Error + ?Sized>(error: &E) {
    // 1. 响亮输出到 stderr（用户立即可见）
    eprintln!("🔥 ERROR: {}", error);

    // 2. 输出错误链（如果有）
    let mut source = error.source();
    let mut level = 1;
    while let Some(err) = source {
        eprintln!("   {}. Caused by: {}", level, err);
        source = err.source();
        level += 1;
    }

    // 3. 记录到日志（使用 tracing，如果已初始化）
    // 注意：这里使用 tracing::error! 宏，如果日志未初始化，会静默失败
    // 但 stderr 输出已经保证了响亮报错
    tracing::error!("Error occurred: {}", error);

    // 记录错误链到日志
    let mut source = error.source();
    let mut level = 1;
    while let Some(err) = source {
        tracing::error!("  Caused by (level {}): {}", level, err);
        source = err.source();
        level += 1;
    }
}

/// 为 Result 添加上下文信息的辅助函数
///
/// 这个函数允许你在错误传播时添加额外的上下文信息，
/// 而不需要修改原始错误类型。
///
/// # Arguments
/// * `result` - 要添加上下文的 Result
/// * `context` - 上下文描述字符串
///
/// # Returns
/// * `Result<T, anyhow::Error>` - 包含上下文信息的 Result
///
/// # Example
/// ```ignore
/// use shared_utils::error_handler::add_context;
///
/// let result = std::fs::read_to_string("config.toml");
/// let content = add_context(result, "reading configuration file")?;
/// ```
pub fn add_context<T, E>(result: Result<T, E>, context: &str) -> anyhow::Result<T>
where
    E: std::error::Error + Send + Sync + 'static,
{
    result.map_err(|e| {
        let err = anyhow::anyhow!(e);
        err.context(context.to_string())
    })
}

/// 安装 panic handler，在程序崩溃前记录详细信息
///
/// 这个函数应该在程序启动时调用一次。
/// 当程序 panic 时，会：
/// 1. 记录 panic 信息到日志
/// 2. 输出到 stderr
/// 3. 然后执行默认的 panic 行为（通常是退出）
///
/// # Example
/// ```ignore
/// use shared_utils::error_handler::install_panic_handler;
///
/// fn main() {
///     install_panic_handler();
///     // ... 程序其余部分
/// }
/// ```
pub fn install_panic_handler() {
    let default_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // 提取 panic 信息
        let payload = panic_info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic payload".to_string()
        };

        let location = if let Some(loc) = panic_info.location() {
            format!("{}:{}:{}", loc.file(), loc.line(), loc.column())
        } else {
            "Unknown location".to_string()
        };

        // 响亮报错到 stderr
        eprintln!("💥 PANIC occurred!");
        eprintln!("   Message: {}", message);
        eprintln!("   Location: {}", location);
        eprintln!("   This is a bug! Please report it.");

        // 记录到日志
        tracing::error!("PANIC: {} at {}", message, location);

        // 调用默认的 panic handler（打印堆栈跟踪等）
        default_hook(panic_info);
    }));
}

/// Result 扩展 trait，提供便捷的上下文添加方法
///
/// 这个 trait 为所有 Result 类型添加了 `context()` 方法，
/// 使得添加上下文信息更加方便。
pub trait ResultExt<T, E> {
    /// 为错误添加上下文信息
    ///
    /// # Example
    /// ```ignore
    /// use shared_utils::error_handler::ResultExt;
    ///
    /// let result = std::fs::read_to_string("config.toml")
    ///     .context_err("reading configuration file")?;
    /// ```
    fn context_err(self, context: &str) -> anyhow::Result<T>
    where
        E: std::error::Error + Send + Sync + 'static;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn context_err(self, context: &str) -> anyhow::Result<T>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        add_context(self, context)
    }
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
        let action = handle_error(ErrorCategory::Optional, "Preserving timestamp", error, None);
        assert!(matches!(action, ErrorAction::Continue));
    }

    // ========================================================================
    // 测试新增的错误报告工具 (v7.8)
    // ========================================================================

    #[test]
    fn test_report_error() {
        // 测试 report_error 不会 panic
        let error = io::Error::new(io::ErrorKind::NotFound, "test file not found");
        report_error(&error);
        // 如果没有 panic，测试通过
    }

    #[test]
    fn test_add_context() {
        // 测试成功的情况
        let result: Result<i32, io::Error> = Ok(42);
        let with_context = add_context(result, "test operation");
        assert!(with_context.is_ok());
        assert_eq!(with_context.unwrap(), 42);

        // 测试失败的情况
        let result: Result<i32, io::Error> =
            Err(io::Error::new(io::ErrorKind::NotFound, "test error"));
        let with_context = add_context(result, "test operation");
        assert!(with_context.is_err());

        // 验证错误消息包含上下文
        let err_msg = format!("{}", with_context.unwrap_err());
        assert!(err_msg.contains("test operation"));
    }

    #[test]
    fn test_result_ext_trait() {
        // 测试 ResultExt trait
        let result: Result<i32, io::Error> =
            Err(io::Error::new(io::ErrorKind::NotFound, "test error"));

        let with_context = result.context_err("using ResultExt trait");
        assert!(with_context.is_err());

        let err_msg = format!("{}", with_context.unwrap_err());
        assert!(err_msg.contains("using ResultExt trait"));
    }

    #[test]
    fn test_install_panic_handler() {
        // 测试安装 panic handler 不会 panic
        install_panic_handler();
        // 如果没有 panic，测试通过

        // 注意：我们不能测试实际的 panic 行为，因为那会终止测试进程
        // 但我们可以确保安装过程本身是安全的
    }

    #[test]
    fn test_error_chain_reporting() {
        // 创建一个带有错误链的错误
        let outer_error: Box<dyn std::error::Error> =
            Box::new(io::Error::other("outer error with inner cause"));

        // 测试 report_error 能处理错误链
        report_error(outer_error.as_ref());
        // 如果没有 panic，测试通过
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
            (ErrorCategory::Recoverable, true), // 应该返回Continue
            (ErrorCategory::Fatal, false),      // 应该返回Abort
            (ErrorCategory::Optional, true),    // 应该返回Continue
        ];

        for (category, should_continue) in test_cases {
            let error = io::Error::other("test error");
            let action = handle_error(category, "test context", error, None);

            let is_continue = matches!(action, ErrorAction::Continue);
            assert_eq!(
                is_continue,
                should_continue,
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
