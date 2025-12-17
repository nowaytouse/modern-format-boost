//! AppError - 统一的应用错误类型
//!
//! 提供清晰的错误分类，区分可恢复和不可恢复错误。

use std::fmt;
use std::path::PathBuf;
use crate::error_handler::ErrorCategory;
use crate::types::{CrfError, SsimError, IterationError};

// ============================================================================
// AppError
// ============================================================================

/// 统一的应用错误类型
/// 
/// 所有错误都分为两类：
/// - **可恢复错误**：用户输入错误、外部工具失败、文件不存在等
/// - **不可恢复错误**：程序员错误、类型不变量违反等（应该 panic）
#[derive(Debug)]
pub enum AppError {
    // === File/IO Errors (Recoverable) ===
    
    /// 文件不存在
    FileNotFound {
        path: PathBuf,
    },
    
    /// 文件读取失败
    FileReadError {
        path: PathBuf,
        source: std::io::Error,
    },
    
    /// 文件写入失败
    FileWriteError {
        path: PathBuf,
        source: std::io::Error,
    },
    
    /// 目录不存在
    DirectoryNotFound {
        path: PathBuf,
    },
    
    // === Validation Errors (Recoverable) ===
    
    /// 无效的 CRF 值
    InvalidCrf(CrfError),
    
    /// 无效的 SSIM 值
    InvalidSsim(SsimError),
    
    /// 迭代次数超限
    IterationLimitExceeded(IterationError),
    
    // === External Tool Errors (Recoverable) ===
    
    /// FFmpeg 执行失败
    FfmpegError {
        message: String,
        stderr: String,
        exit_code: Option<i32>,
    },
    
    /// FFprobe 执行失败
    FfprobeError {
        message: String,
        stderr: String,
    },
    
    /// 外部工具未找到
    ToolNotFound {
        tool_name: String,
    },
    
    // === Conversion Errors (Recoverable) ===
    
    /// 压缩失败（输出 >= 输入）
    CompressionFailed {
        input_size: u64,
        output_size: u64,
    },
    
    /// 质量验证失败
    QualityValidationFailed {
        expected_ssim: f64,
        actual_ssim: f64,
    },
    
    /// 输出文件已存在
    OutputExists {
        path: PathBuf,
    },
    
    // === Generic Errors ===
    
    /// IO 错误
    Io(std::io::Error),
    
    /// 其他错误（来自 anyhow）
    Other(anyhow::Error),
}

impl AppError {
    /// 是否可恢复
    /// 
    /// 可恢复错误应该返回 Result::Err，
    /// 不可恢复错误应该 panic。
    pub fn is_recoverable(&self) -> bool {
        // 所有 AppError 变体都是可恢复的
        // 不可恢复错误应该直接 panic，不应该创建 AppError
        true
    }
    
    /// 获取错误分类
    /// 
    /// 使用现有的 ErrorCategory 枚举：
    /// - Recoverable: 可恢复错误
    /// - Fatal: 致命错误
    /// - Optional: 可选操作失败
    pub fn category(&self) -> ErrorCategory {
        match self {
            // 文件不存在通常是致命错误
            AppError::FileNotFound { .. } |
            AppError::DirectoryNotFound { .. } => ErrorCategory::Fatal,
            
            // IO 错误通常是致命的
            AppError::FileReadError { .. } |
            AppError::FileWriteError { .. } |
            AppError::Io(_) => ErrorCategory::Fatal,
            
            // 验证错误是可恢复的
            AppError::InvalidCrf(_) |
            AppError::InvalidSsim(_) => ErrorCategory::Recoverable,
            
            // 外部工具错误是致命的
            AppError::FfmpegError { .. } |
            AppError::FfprobeError { .. } |
            AppError::ToolNotFound { .. } => ErrorCategory::Fatal,
            
            // 压缩/质量失败是可恢复的
            AppError::CompressionFailed { .. } |
            AppError::QualityValidationFailed { .. } => ErrorCategory::Recoverable,
            
            // 输出已存在是可选的（跳过）
            AppError::OutputExists { .. } => ErrorCategory::Optional,
            
            // 迭代超限是可恢复的
            AppError::IterationLimitExceeded(_) => ErrorCategory::Recoverable,
            
            // 其他错误默认为致命
            AppError::Other(_) => ErrorCategory::Fatal,
        }
    }
    
    /// 获取用户友好的错误消息
    pub fn user_message(&self) -> String {
        match self {
            AppError::FileNotFound { path } => {
                format!("❌ 文件不存在: {}", path.display())
            }
            AppError::DirectoryNotFound { path } => {
                format!("❌ 目录不存在: {}", path.display())
            }
            AppError::FileReadError { path, source } => {
                format!("❌ 无法读取文件 {}: {}", path.display(), source)
            }
            AppError::FileWriteError { path, source } => {
                format!("❌ 无法写入文件 {}: {}", path.display(), source)
            }
            AppError::InvalidCrf(e) => {
                format!("❌ 无效的 CRF 值: {}", e)
            }
            AppError::InvalidSsim(e) => {
                format!("❌ 无效的 SSIM 值: {}", e)
            }
            AppError::IterationLimitExceeded(e) => {
                format!("⚠️ 迭代次数超限: {}", e)
            }
            AppError::FfmpegError { message, stderr, exit_code } => {
                let code_str = exit_code.map(|c| format!(" (exit code: {})", c)).unwrap_or_default();
                format!("❌ FFmpeg 失败{}: {}\n{}", code_str, message, stderr)
            }
            AppError::FfprobeError { message, stderr } => {
                format!("❌ FFprobe 失败: {}\n{}", message, stderr)
            }
            AppError::ToolNotFound { tool_name } => {
                format!("❌ 未找到工具: {}\n💡 请确保 {} 已安装并在 PATH 中", tool_name, tool_name)
            }
            AppError::CompressionFailed { input_size, output_size } => {
                let ratio = *output_size as f64 / *input_size as f64 * 100.0;
                format!("❌ 压缩失败: 输出 ({} bytes) >= 输入 ({} bytes), 比率 {:.1}%", 
                    output_size, input_size, ratio)
            }
            AppError::QualityValidationFailed { expected_ssim, actual_ssim } => {
                format!("❌ 质量验证失败: 期望 SSIM >= {:.4}, 实际 {:.4}", 
                    expected_ssim, actual_ssim)
            }
            AppError::OutputExists { path } => {
                format!("⏭️ 输出文件已存在: {}", path.display())
            }
            AppError::Io(e) => {
                format!("❌ IO 错误: {}", e)
            }
            AppError::Other(e) => {
                format!("❌ 错误: {}", e)
            }
        }
    }
    
    /// 是否应该跳过（而非失败）
    /// 
    /// 某些错误（如输出已存在）应该被视为跳过而非失败。
    pub fn is_skip(&self) -> bool {
        matches!(self, AppError::OutputExists { .. })
    }
}

// ============================================================================
// Display and Error Traits
// ============================================================================

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::FileNotFound { path } => {
                write!(f, "File not found: {}", path.display())
            }
            AppError::DirectoryNotFound { path } => {
                write!(f, "Directory not found: {}", path.display())
            }
            AppError::FileReadError { path, source } => {
                write!(f, "Failed to read {}: {}", path.display(), source)
            }
            AppError::FileWriteError { path, source } => {
                write!(f, "Failed to write {}: {}", path.display(), source)
            }
            AppError::InvalidCrf(e) => write!(f, "Invalid CRF: {}", e),
            AppError::InvalidSsim(e) => write!(f, "Invalid SSIM: {}", e),
            AppError::IterationLimitExceeded(e) => write!(f, "{}", e),
            AppError::FfmpegError { message, .. } => write!(f, "FFmpeg error: {}", message),
            AppError::FfprobeError { message, .. } => write!(f, "FFprobe error: {}", message),
            AppError::ToolNotFound { tool_name } => write!(f, "Tool not found: {}", tool_name),
            AppError::CompressionFailed { input_size, output_size } => {
                write!(f, "Compression failed: output ({}) >= input ({})", output_size, input_size)
            }
            AppError::QualityValidationFailed { expected_ssim, actual_ssim } => {
                write!(f, "Quality validation failed: expected SSIM >= {:.4}, got {:.4}", 
                    expected_ssim, actual_ssim)
            }
            AppError::OutputExists { path } => {
                write!(f, "Output exists: {}", path.display())
            }
            AppError::Io(e) => write!(f, "IO error: {}", e),
            AppError::Other(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppError::FileReadError { source, .. } => Some(source),
            AppError::FileWriteError { source, .. } => Some(source),
            AppError::Io(e) => Some(e),
            _ => None,
        }
    }
}

// ============================================================================
// From Implementations
// ============================================================================

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<CrfError> for AppError {
    fn from(e: CrfError) -> Self {
        AppError::InvalidCrf(e)
    }
}

impl From<SsimError> for AppError {
    fn from(e: SsimError) -> Self {
        AppError::InvalidSsim(e)
    }
}

impl From<IterationError> for AppError {
    fn from(e: IterationError) -> Self {
        AppError::IterationLimitExceeded(e)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Other(e)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_error_is_recoverable() {
        let error = AppError::FileNotFound { path: PathBuf::from("/test") };
        assert!(error.is_recoverable());
        
        let error = AppError::CompressionFailed { input_size: 1000, output_size: 1100 };
        assert!(error.is_recoverable());
    }

    #[test]
    fn test_app_error_category() {
        let error = AppError::FileNotFound { path: PathBuf::from("/test") };
        assert_eq!(error.category(), ErrorCategory::Fatal);
        
        let error = AppError::FfmpegError { 
            message: "test".to_string(), 
            stderr: "".to_string(),
            exit_code: Some(1),
        };
        assert_eq!(error.category(), ErrorCategory::Fatal);
        
        let error = AppError::OutputExists { path: PathBuf::from("/test.mp4") };
        assert_eq!(error.category(), ErrorCategory::Optional);
    }

    #[test]
    fn test_app_error_is_skip() {
        let error = AppError::OutputExists { path: PathBuf::from("/test.mp4") };
        assert!(error.is_skip());
        
        let error = AppError::FileNotFound { path: PathBuf::from("/test") };
        assert!(!error.is_skip());
    }

    #[test]
    fn test_app_error_user_message() {
        let error = AppError::ToolNotFound { tool_name: "ffmpeg".to_string() };
        let msg = error.user_message();
        assert!(msg.contains("ffmpeg"));
        assert!(msg.contains("PATH"));
    }

    #[test]
    fn test_app_error_from_io() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let app_error: AppError = io_error.into();
        assert!(matches!(app_error, AppError::Io(_)));
    }
}

// ============================================================================
// Property-Based Tests
// ============================================================================

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // ========================================================================
    // **Feature: rust-type-safety-v7.1, Property 10: AppError Recoverability**
    // *For any* AppError, is_recoverable() should return true for user/external 
    // errors and false for programmer bugs.
    // **Validates: Requirements 4.1, 4.2**
    // ========================================================================
    
    // 生成随机 AppError
    fn arb_app_error() -> impl Strategy<Value = AppError> {
        prop_oneof![
            any::<String>().prop_map(|s| AppError::FileNotFound { 
                path: PathBuf::from(s) 
            }),
            any::<String>().prop_map(|s| AppError::DirectoryNotFound { 
                path: PathBuf::from(s) 
            }),
            any::<String>().prop_map(|s| AppError::ToolNotFound { 
                tool_name: s 
            }),
            (any::<u64>(), any::<u64>()).prop_map(|(i, o)| AppError::CompressionFailed { 
                input_size: i, 
                output_size: o 
            }),
            any::<String>().prop_map(|s| AppError::OutputExists { 
                path: PathBuf::from(s) 
            }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        
        #[test]
        fn app_error_recoverability_property(error in arb_app_error()) {
            // 所有 AppError 变体都应该是可恢复的
            // 不可恢复错误应该直接 panic，不应该创建 AppError
            prop_assert!(error.is_recoverable(),
                "AppError {:?} should be recoverable", error
            );
        }
        
        #[test]
        fn app_error_has_category(error in arb_app_error()) {
            // 所有 AppError 都应该有一个有效的分类
            let _category = error.category();
            // 如果没有 panic，测试通过
        }
        
        #[test]
        fn app_error_has_user_message(error in arb_app_error()) {
            // 所有 AppError 都应该有用户友好的消息
            let msg = error.user_message();
            prop_assert!(!msg.is_empty(),
                "AppError {:?} should have non-empty user message", error
            );
        }
    }
}
