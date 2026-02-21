//! Flag 组合验证器 - 简化逻辑，仅支持推荐组合
//!
//! 有效组合仅一种（均为默认开启）：
//! - `explore + match_quality + compress`（可选 `--ultimate`）
//! 其他组合一律 Invalid，不再兼容老旧单独/部分组合。

use std::fmt;

/// Flag 组合模式（简化后仅两种有效）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagMode {
    /// 推荐模式：explore + match_quality + compress
    PreciseQualityWithCompress,
    /// 极限探索：上述 + --ultimate（SSIM 饱和）
    UltimateExplore,
}

impl fmt::Display for FlagMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlagMode::PreciseQualityWithCompress => {
                write!(f, "--explore --match-quality --compress")
            }
            FlagMode::UltimateExplore => {
                write!(f, "--explore --match-quality --compress --ultimate")
            }
        }
    }
}

impl FlagMode {
    /// 获取模式的中文描述
    pub fn description_cn(&self) -> &'static str {
        match self {
            FlagMode::PreciseQualityWithCompress => "精确质量匹配+必须压缩",
            FlagMode::UltimateExplore => "🔥 极限探索（SSIM饱和）",
        }
    }

    /// 获取模式的英文描述
    pub fn description_en(&self) -> &'static str {
        match self {
            FlagMode::PreciseQualityWithCompress => "Precise quality match + must compress",
            FlagMode::UltimateExplore => "🔥 Ultimate explore (SSIM saturation)",
        }
    }

    /// 是否为极限探索模式
    pub fn is_ultimate(&self) -> bool {
        matches!(self, FlagMode::UltimateExplore)
    }
}

/// Flag 组合验证结果
#[derive(Debug)]
pub enum FlagValidation {
    /// 有效组合
    Valid(FlagMode),
    /// 无效组合（包含错误信息）
    Invalid(String),
}

/// 验证 flag 组合（不含 ultimate）。仅接受 explore && match_quality && compress。
pub fn validate_flags(explore: bool, match_quality: bool, compress: bool) -> FlagValidation {
    validate_flags_with_ultimate(explore, match_quality, compress, false)
}

/// 验证 flag 组合（含 ultimate）。仅接受推荐组合：explore + match_quality + compress（可选 ultimate）。
/// 其他组合一律 Invalid，不再兼容老旧单独/部分组合。
pub fn validate_flags_with_ultimate(
    explore: bool,
    match_quality: bool,
    compress: bool,
    ultimate: bool,
) -> FlagValidation {
    if !explore || !match_quality || !compress {
        return FlagValidation::Invalid(
            "❌ Only the recommended flag combination is supported: explore + match-quality + compress (all on by default).\n\
             💡 Omit flags to use defaults, or do not turn off explore/match-quality/compress.".to_string(),
        );
    }
    if ultimate {
        return FlagValidation::Valid(FlagMode::UltimateExplore);
    }
    FlagValidation::Valid(FlagMode::PreciseQualityWithCompress)
}

/// 验证并返回 Result（不含 ultimate）
pub fn validate_flags_result(
    explore: bool,
    match_quality: bool,
    compress: bool,
) -> Result<FlagMode, String> {
    match validate_flags(explore, match_quality, compress) {
        FlagValidation::Valid(mode) => Ok(mode),
        FlagValidation::Invalid(err) => Err(err),
    }
}

/// 验证并返回 Result（含 ultimate）
pub fn validate_flags_result_with_ultimate(
    explore: bool,
    match_quality: bool,
    compress: bool,
    ultimate: bool,
) -> Result<FlagMode, String> {
    match validate_flags_with_ultimate(explore, match_quality, compress, ultimate) {
        FlagValidation::Valid(mode) => Ok(mode),
        FlagValidation::Invalid(err) => Err(err),
    }
}

/// 打印 flag 组合帮助信息（简化：仅推荐组合有效）
pub fn print_flag_help() {
    eprintln!("📋 Flag (simplified): Only the recommended combination is supported.");
    eprintln!("   Default: explore + match-quality + compress (all on).");
    eprintln!("   Optional: --ultimate for SSIM saturation search.");
    eprintln!("   To disable optional features only: --no-apple-compat, --no-recursive, --no-allow-size-tolerance");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════════
    // 基础有效组合测试
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_only_recommended_combination_valid() {
        assert!(matches!(
            validate_flags(true, true, true),
            FlagValidation::Valid(FlagMode::PreciseQualityWithCompress)
        ));
    }

    #[test]
    fn test_any_other_combination_invalid() {
        assert!(matches!(validate_flags(false, false, false), FlagValidation::Invalid(_)));
        assert!(matches!(validate_flags(false, false, true), FlagValidation::Invalid(_)));
        assert!(matches!(validate_flags(false, true, false), FlagValidation::Invalid(_)));
        assert!(matches!(validate_flags(false, true, true), FlagValidation::Invalid(_)));
        assert!(matches!(validate_flags(true, false, false), FlagValidation::Invalid(_)));
        assert!(matches!(validate_flags(true, false, true), FlagValidation::Invalid(_)));
        assert!(matches!(validate_flags(true, true, false), FlagValidation::Invalid(_)));
    }

    #[test]
    fn test_ultimate_valid_only_with_full_combination() {
        let r = validate_flags_result_with_ultimate(true, true, true, true);
        assert!(r.is_ok());
        assert_eq!(r.unwrap(), FlagMode::UltimateExplore);
    }

    #[test]
    fn test_ultimate_invalid_with_incomplete() {
        assert!(validate_flags_result_with_ultimate(false, false, false, true).is_err());
        assert!(validate_flags_result_with_ultimate(true, true, false, true).is_err());
    }

    #[test]
    fn test_flag_mode_display_and_descriptions() {
        assert_eq!(
            format!("{}", FlagMode::PreciseQualityWithCompress),
            "--explore --match-quality --compress"
        );
        assert!(FlagMode::PreciseQualityWithCompress.description_cn().contains("精确"));
        assert!(FlagMode::UltimateExplore.description_cn().contains("极限"));
        assert!(FlagMode::UltimateExplore.is_ultimate());
        assert!(!FlagMode::PreciseQualityWithCompress.is_ultimate());
    }

}
