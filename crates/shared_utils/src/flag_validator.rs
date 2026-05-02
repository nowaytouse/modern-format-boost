//! Flag Combination Validator - Simplified logic, supporting recommended combinations only.
//!
//! Only one valid combination (all enabled by default):
//! - `explore + match_quality + compress` (optional `--ultimate`)
//!   All other combinations are Invalid; no longer compatible with legacy individual or partial combinations.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagMode {
    PreciseQualityWithCompress,
    UltimateExplore,
}

impl fmt::Display for FlagMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreciseQualityWithCompress => {
                write!(f, "--explore --match-quality --compress")
            }
            Self::UltimateExplore => {
                write!(f, "--explore --match-quality --compress --ultimate")
            }
        }
    }
}

impl FlagMode {
    #[must_use]
    pub const fn description_en(&self) -> &'static str {
        match self {
            Self::PreciseQualityWithCompress => "Precise quality match + must compress",
            Self::UltimateExplore => "🔥 Ultimate explore (3D quality plateau search) [GPU+CPU]",
        }
    }

    #[must_use]
    pub const fn is_ultimate(&self) -> bool {
        matches!(self, Self::UltimateExplore)
    }
}

#[derive(Debug)]
pub enum FlagValidation {
    Valid(FlagMode),
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct FlagRequest {
    pub explore: bool,
    pub match_quality: bool,
    pub compress: bool,
    pub ultimate: bool,
}

#[must_use]
pub fn validate_flags(explore: bool, match_quality: bool, compress: bool) -> FlagValidation {
    validate_flags_with_ultimate(FlagRequest {
        explore,
        match_quality,
        compress,
        ultimate: false,
    })
}

#[must_use]
pub fn validate_flags_with_ultimate(request: FlagRequest) -> FlagValidation {
    if !request.explore || !request.match_quality || !request.compress {
        return FlagValidation::Invalid(
            "❌ Only the recommended flag combination is supported: explore + match-quality + compress (all on by default).\n\
             💡 Omit flags to use defaults, or do not turn off explore/match-quality/compress.".to_string(),
        );
    }
    if request.ultimate {
        return FlagValidation::Valid(FlagMode::UltimateExplore);
    }
    FlagValidation::Valid(FlagMode::PreciseQualityWithCompress)
}

/// Validate flags and determine the final operation mode.
///
/// # Errors
/// Returns an error message if flag combination is invalid.
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

/// Validate flags with ultimate mode considered.
///
/// # Errors
/// Returns an error message if flag combination is invalid.
pub fn validate_flags_result_with_ultimate(request: FlagRequest) -> Result<FlagMode, String> {
    match validate_flags_with_ultimate(request) {
        FlagValidation::Valid(mode) => Ok(mode),
        FlagValidation::Invalid(err) => Err(err),
    }
}

pub fn print_flag_help() {
    eprintln!("📋 Flag (simplified): Only the recommended combination is supported.");
    eprintln!("   Default: explore + match-quality + compress (all on).");
    eprintln!("   Optional: --ultimate for tighter 3D quality plateau search.");
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
        let r = validate_flags_result_with_ultimate(FlagRequest {
            explore: true,
            match_quality: true,
            compress: true,
            ultimate: true,
        });
        assert!(r.is_ok());
        assert_eq!(r.unwrap_or_else(|e| panic!("error: {e:?}")), FlagMode::UltimateExplore);
    }

    #[test]
    fn test_ultimate_invalid_with_incomplete() {
        assert!(validate_flags_result_with_ultimate(FlagRequest {
            explore: false,
            match_quality: false,
            compress: false,
            ultimate: true,
        })
        .is_err());
        assert!(validate_flags_result_with_ultimate(FlagRequest {
            explore: true,
            match_quality: true,
            compress: false,
            ultimate: true,
        })
        .is_err());
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
        assert!(FlagMode::UltimateExplore
            .description_en()
            .contains("Ultimate"));
        assert!(FlagMode::UltimateExplore.is_ultimate());
        assert!(!FlagMode::PreciseQualityWithCompress.is_ultimate());
    }
}
