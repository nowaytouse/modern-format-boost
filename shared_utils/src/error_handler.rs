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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Recoverable,
    Fatal,
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

#[derive(Debug)]
pub enum ErrorAction {
    Continue,
    Abort(anyhow::Error),
}

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


pub fn report_error<E: std::error::Error + ?Sized>(error: &E) {
    eprintln!("🔥 ERROR: {}", error);

    let mut source = error.source();
    let mut level = 1;
    while let Some(err) = source {
        eprintln!("   {}. Caused by: {}", level, err);
        source = err.source();
        level += 1;
    }

    tracing::error!("Error occurred: {}", error);

    let mut source = error.source();
    let mut level = 1;
    while let Some(err) = source {
        tracing::error!("  Caused by (level {}): {}", level, err);
        source = err.source();
        level += 1;
    }
}

pub fn add_context<T, E>(result: Result<T, E>, context: &str) -> anyhow::Result<T>
where
    E: std::error::Error + Send + Sync + 'static,
{
    result.map_err(|e| {
        let err = anyhow::anyhow!(e);
        err.context(context.to_string())
    })
}

pub fn install_panic_handler() {
    let default_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
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

        eprintln!("💥 PANIC occurred!");
        eprintln!("   Message: {}", message);
        eprintln!("   Location: {}", location);
        eprintln!("   This is a bug! Please report it.");

        tracing::error!("PANIC: {} at {}", message, location);

        default_hook(panic_info);
    }));
}

pub trait ResultExt<T, E> {
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


    #[test]
    fn test_report_error() {
        let error = io::Error::new(io::ErrorKind::NotFound, "test file not found");
        report_error(&error);
    }

    #[test]
    fn test_add_context() {
        let result: Result<i32, io::Error> = Ok(42);
        let with_context = add_context(result, "test operation");
        assert!(with_context.is_ok());
        assert_eq!(with_context.unwrap(), 42);

        let result: Result<i32, io::Error> =
            Err(io::Error::new(io::ErrorKind::NotFound, "test error"));
        let with_context = add_context(result, "test operation");
        assert!(with_context.is_err());

        let err_msg = format!("{}", with_context.unwrap_err());
        assert!(err_msg.contains("test operation"));
    }

    #[test]
    fn test_result_ext_trait() {
        let result: Result<i32, io::Error> =
            Err(io::Error::new(io::ErrorKind::NotFound, "test error"));

        let with_context = result.context_err("using ResultExt trait");
        assert!(with_context.is_err());

        let err_msg = format!("{}", with_context.unwrap_err());
        assert!(err_msg.contains("using ResultExt trait"));
    }

    #[test]
    fn test_install_panic_handler() {
        install_panic_handler();

    }

    #[test]
    fn test_error_chain_reporting() {
        let outer_error: Box<dyn std::error::Error> =
            Box::new(io::Error::other("outer error with inner cause"));

        report_error(outer_error.as_ref());
    }
}


#[cfg(test)]
mod prop_tests {
    use super::*;
    use std::io;

    #[test]
    fn prop_error_handling_consistency() {
        let test_cases = vec![
            (ErrorCategory::Recoverable, true),
            (ErrorCategory::Fatal, false),
            (ErrorCategory::Optional, true),
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
        assert_eq!(format!("{}", ErrorCategory::Recoverable), "RECOVERABLE");
        assert_eq!(format!("{}", ErrorCategory::Fatal), "FATAL");
        assert_eq!(format!("{}", ErrorCategory::Optional), "OPTIONAL");
    }
}
