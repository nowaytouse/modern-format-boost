#[cfg(test)]
mod tests {
    use crate::unified_error::UnifiedError;
    use crate::types::IterationError;
    use crate::error_handler::ErrorCategory;

    #[test]
    fn test_skip_vs_error_semantics() {
        // 1. Optimization failures MUST be Skips
        let iter_err = UnifiedError::IterationLimitExceeded(IterationError {
            current: 10,
            max: 10,
            context: "test".to_string(),
        });
        assert!(iter_err.is_skip(), "IterationLimitExceeded must be a skip");
        assert!(iter_err.should_copy_original(), "IterationLimitExceeded must trigger original copy");
        assert_eq!(iter_err.category(), ErrorCategory::Optional);

        let qual_err = UnifiedError::QualityValidationFailed {
            expected_ssim: 0.99,
            actual_ssim: 0.98,
            file_path: None,
        };
        assert!(qual_err.is_skip(), "QualityValidationFailed must be a skip");
        assert!(qual_err.should_copy_original(), "QualityValidationFailed must trigger original copy");
        assert_eq!(qual_err.category(), ErrorCategory::Optional);

        // 2. Hard failures MUST be Errors (No Copy)
        let anal_err = UnifiedError::AnalysisError("failed to probe".to_string());
        assert!(!anal_err.is_skip(), "AnalysisError must NOT be a skip");
        assert!(!anal_err.should_copy_original(), "AnalysisError must NOT trigger original copy");
        assert_eq!(anal_err.category(), ErrorCategory::Recoverable);

        let io_err = UnifiedError::video_not_supported("xyz");
        assert!(!io_err.is_skip(), "VideoFormatNotSupported must NOT be a skip");
        assert!(!io_err.should_copy_original(), "VideoFormatNotSupported must NOT trigger original copy");
        assert_eq!(io_err.category(), ErrorCategory::Recoverable);
        
        let fatal_err = UnifiedError::tool_not_found("ffmpeg");
        assert!(!fatal_err.is_skip(), "ToolNotFound must NOT be a skip");
        assert_eq!(fatal_err.category(), ErrorCategory::Fatal);
    }
}
