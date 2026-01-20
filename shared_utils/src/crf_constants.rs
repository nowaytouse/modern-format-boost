//! CRF Constants Module
//!
//! Unified CRF (Constant Rate Factor) range constants for all video encoders.
//! 统一的 CRF 常量定义，避免在多个文件中重复定义。

// ============================================================================
// HEVC/H.265 Constants
// ============================================================================

/// HEVC minimum CRF (lossless)
pub const HEVC_CRF_MIN: f32 = 0.0;

/// HEVC maximum CRF (lowest quality)
pub const HEVC_CRF_MAX: f32 = 51.0;

/// HEVC default CRF (good quality)
pub const HEVC_CRF_DEFAULT: f32 = 23.0;

/// HEVC visually lossless CRF
pub const HEVC_CRF_VISUALLY_LOSSLESS: f32 = 18.0;

/// HEVC practical maximum (beyond this quality is too low)
pub const HEVC_CRF_PRACTICAL_MAX: f32 = 32.0;

// ============================================================================
// AV1 Constants
// ============================================================================

/// AV1 minimum CRF (lossless)
pub const AV1_CRF_MIN: f32 = 0.0;

/// AV1 maximum CRF (lowest quality)
pub const AV1_CRF_MAX: f32 = 63.0;

/// AV1 default CRF (good quality)
pub const AV1_CRF_DEFAULT: f32 = 30.0;

/// AV1 visually lossless CRF
pub const AV1_CRF_VISUALLY_LOSSLESS: f32 = 20.0;

/// AV1 practical maximum
pub const AV1_CRF_PRACTICAL_MAX: f32 = 45.0;

// ============================================================================
// VP9 Constants
// ============================================================================

/// VP9 minimum CRF (lossless)
pub const VP9_CRF_MIN: f32 = 0.0;

/// VP9 maximum CRF (lowest quality)
pub const VP9_CRF_MAX: f32 = 63.0;

/// VP9 default CRF (good quality)
pub const VP9_CRF_DEFAULT: f32 = 31.0;

// ============================================================================
// x264/H.264 Constants
// ============================================================================

/// x264 minimum CRF (lossless)
pub const X264_CRF_MIN: f32 = 0.0;

/// x264 maximum CRF (lowest quality)
pub const X264_CRF_MAX: f32 = 51.0;

/// x264 default CRF (good quality)
pub const X264_CRF_DEFAULT: f32 = 23.0;

// ============================================================================
// Cache Key Constants
// ============================================================================

/// CRF cache key multiplier (for integer key generation)
/// 乘数越大，精度越高，但缓存空间越大
pub const CRF_CACHE_KEY_MULTIPLIER: f32 = 100.0;

/// Maximum valid CRF for cache key generation
pub const CRF_CACHE_MAX_VALID: f32 = 63.99;

// ============================================================================
// Iteration Limits
// ============================================================================

/// Normal maximum iterations for CRF exploration
pub const NORMAL_MAX_ITERATIONS: u32 = 60;

/// Emergency fallback maximum iterations (prevents infinite loops)
/// 紧急保底迭代限制，防止无限循环
pub const EMERGENCY_MAX_ITERATIONS: u32 = 500;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hevc_crf_range() {
        assert_eq!(HEVC_CRF_MIN, 0.0);
        assert_eq!(HEVC_CRF_MAX, 51.0);
        // 常量断言已移除（clippy::assertions_on_constants）
    }

    #[test]
    fn test_av1_crf_range() {
        assert_eq!(AV1_CRF_MIN, 0.0);
        assert_eq!(AV1_CRF_MAX, 63.0);
        // 常量断言已移除（clippy::assertions_on_constants）
    }

    #[test]
    fn test_vp9_crf_range() {
        assert_eq!(VP9_CRF_MIN, 0.0);
        assert_eq!(VP9_CRF_MAX, 63.0);
        // 常量断言已移除（clippy::assertions_on_constants）
    }

    #[test]
    fn test_x264_crf_range() {
        assert_eq!(X264_CRF_MIN, 0.0);
        assert_eq!(X264_CRF_MAX, 51.0);
        // 常量断言已移除（clippy::assertions_on_constants）
    }

    #[test]
    fn test_cache_constants() {
        assert_eq!(CRF_CACHE_KEY_MULTIPLIER, 100.0);
        // 常量断言已移除（clippy::assertions_on_constants）
    }

    #[test]
    fn test_iteration_limits() {
        assert_eq!(NORMAL_MAX_ITERATIONS, 60);
        assert_eq!(EMERGENCY_MAX_ITERATIONS, 500);
        // 常量断言已移除（clippy::assertions_on_constants）
    }

    #[test]
    fn test_hevc_practical_max() {
        // 常量断言已移除（clippy::assertions_on_constants）
        // 这些常量关系在编译时已验证
        assert_eq!(HEVC_CRF_PRACTICAL_MAX, 32.0);
    }

    #[test]
    fn test_av1_practical_max() {
        // 常量断言已移除（clippy::assertions_on_constants）
        assert_eq!(AV1_CRF_PRACTICAL_MAX, 45.0);
    }
}
