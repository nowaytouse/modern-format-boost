//! Flag 组合验证器 - 简化逻辑，仅支持推荐组合
//!
//! 有效组合仅一种（均为默认开启）：
//! - `explore + match_quality + compress`（可选 `--ultimate`）
//!   其他组合一律 Invalid，不再兼容老旧单独/部分组合。

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagMode {
    PreciseQualityWithCompress,
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
    pub fn description_cn(&self) -> &'static str {
        match self {
            FlagMode::PreciseQualityWithCompress => "精确质量匹配+必须压缩",
            FlagMode::UltimateExplore => "🔥 极限探索（SSIM饱和）",
        }
    }

    pub fn description_en(&self) -> &'static str {
        match self {
            FlagMode::PreciseQualityWithCompress => "Precise quality match + must compress",
            FlagMode::UltimateExplore => "🔥 Ultimate explore (SSIM saturation)",
        }
    }

    pub fn is_ultimate(&self) -> bool {
        matches!(self, FlagMode::UltimateExplore)
    }
}

#[derive(Debug)]
pub enum FlagValidation {
    Valid(FlagMode),
    Invalid(String),
}

pub fn validate_flags(explore: bool, match_quality: bool, compress: bool) -> FlagValidation {
    validate_flags_with_ultimate(explore, match_quality, compress, false)
}

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

pub fn print_flag_help() {
    eprintln!("📋 Flag (simplified): Only the recommended combination is supported.");
    eprintln!("   Default: explore + match-quality + compress (all on).");
    eprintln!("   Optional: --ultimate for SSIM saturation search.");
    eprintln!("   To disable optional features only: --no-apple-compat, --no-recursive, --no-allow-size-tolerance");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_recommended_combination_valid() {
        assert!(matches!(
            validate_flags(true, true, true),
            FlagValidation::Valid(FlagMode::PreciseQualityWithCompress)
        ));
    }

    #[test]
    fn test_any_other_combination_invalid() {
        assert!(matches!(
            validate_flags(false, false, false),
            FlagValidation::Invalid(_)
        ));
        assert!(matches!(
            validate_flags(false, false, true),
            FlagValidation::Invalid(_)
        ));
        assert!(matches!(
            validate_flags(false, true, false),
            FlagValidation::Invalid(_)
        ));
        assert!(matches!(
            validate_flags(false, true, true),
            FlagValidation::Invalid(_)
        ));
        assert!(matches!(
            validate_flags(true, false, false),
            FlagValidation::Invalid(_)
        ));
        assert!(matches!(
            validate_flags(true, false, true),
            FlagValidation::Invalid(_)
        ));
        assert!(matches!(
            validate_flags(true, true, false),
            FlagValidation::Invalid(_)
        ));
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
        assert!(FlagMode::PreciseQualityWithCompress
            .description_en()
            .contains("Precise"));
        assert!(FlagMode::UltimateExplore.description_en().contains("Ultimate"));
        assert!(FlagMode::UltimateExplore.is_ultimate());
        assert!(!FlagMode::PreciseQualityWithCompress.is_ultimate());
    }
}
