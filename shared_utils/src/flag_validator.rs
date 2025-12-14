//! Flag 组合验证器 - 统一的 flag 组合验证逻辑
//!
//! 🔥 v4.6: 模块化设计，避免四个工具重复代码
//!
//! ## 有效组合
//! 1. `--compress` 单独：只要输出 < 输入（哪怕 1KB）
//! 2. `--explore` 单独：寻找尽可能更小的输出
//! 3. `--match-quality` 单独：粗略 SSIM 验证
//! 4. `--compress --match-quality`：输出 < 输入 + 粗略 SSIM 验证
//! 5. `--explore --match-quality`：精确质量匹配（最高 SSIM，不在乎大小）
//! 6. `--explore --match-quality --compress`：精确质量匹配 + 必须压缩
//!
//! ## 无效组合（响亮报错）
//! - `--explore --compress`（没有 `--match-quality`）

use std::fmt;

/// Flag 组合模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagMode {
    /// 默认模式：无特殊 flag
    Default,
    /// `--compress` 单独：只要输出 < 输入
    CompressOnly,
    /// `--explore` 单独：寻找尽可能更小的输出
    ExploreOnly,
    /// `--match-quality` 单独：粗略 SSIM 验证
    QualityOnly,
    /// `--compress --match-quality`：输出 < 输入 + 粗略 SSIM 验证
    CompressWithQuality,
    /// `--explore --match-quality`：精确质量匹配（最高 SSIM）
    PreciseQuality,
    /// `--explore --match-quality --compress`：精确质量匹配 + 必须压缩
    PreciseQualityWithCompress,
}

impl fmt::Display for FlagMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlagMode::Default => write!(f, "Default"),
            FlagMode::CompressOnly => write!(f, "--compress"),
            FlagMode::ExploreOnly => write!(f, "--explore"),
            FlagMode::QualityOnly => write!(f, "--match-quality"),
            FlagMode::CompressWithQuality => write!(f, "--compress --match-quality"),
            FlagMode::PreciseQuality => write!(f, "--explore --match-quality"),
            FlagMode::PreciseQualityWithCompress => write!(f, "--explore --match-quality --compress"),
        }
    }
}

impl FlagMode {
    /// 获取模式的中文描述
    pub fn description_cn(&self) -> &'static str {
        match self {
            FlagMode::Default => "默认模式",
            FlagMode::CompressOnly => "仅压缩（输出 < 输入）",
            FlagMode::ExploreOnly => "探索最小输出",
            FlagMode::QualityOnly => "粗略质量匹配",
            FlagMode::CompressWithQuality => "压缩 + 粗略质量验证",
            FlagMode::PreciseQuality => "精确质量匹配（最高 SSIM）",
            FlagMode::PreciseQualityWithCompress => "精确质量匹配 + 必须压缩",
        }
    }
    
    /// 获取模式的英文描述
    pub fn description_en(&self) -> &'static str {
        match self {
            FlagMode::Default => "Default mode",
            FlagMode::CompressOnly => "Compress only (output < input)",
            FlagMode::ExploreOnly => "Find smallest output",
            FlagMode::QualityOnly => "Basic quality match",
            FlagMode::CompressWithQuality => "Compress + basic SSIM validation",
            FlagMode::PreciseQuality => "Precise quality match (highest SSIM)",
            FlagMode::PreciseQualityWithCompress => "Precise quality match + must compress",
        }
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

/// 验证 flag 组合
///
/// # Arguments
/// * `explore` - `--explore` flag
/// * `match_quality` - `--match-quality` flag
/// * `compress` - `--compress` flag
///
/// # Returns
/// * `FlagValidation::Valid(mode)` - 有效组合及其模式
/// * `FlagValidation::Invalid(error)` - 无效组合及错误信息
///
/// # Example
/// ```
/// use shared_utils::flag_validator::{validate_flags, FlagValidation, FlagMode};
///
/// match validate_flags(true, true, false) {
///     FlagValidation::Valid(mode) => println!("Mode: {}", mode),
///     FlagValidation::Invalid(err) => eprintln!("Error: {}", err),
/// }
/// ```
pub fn validate_flags(explore: bool, match_quality: bool, compress: bool) -> FlagValidation {
    match (explore, match_quality, compress) {
        // 无效组合：--explore --compress（没有 --match-quality）
        (true, false, true) => FlagValidation::Invalid(
            "❌ 无效的 flag 组合: --explore --compress\n\
             💡 --explore 寻找最小输出，--compress 只要更小即可，两者目标冲突\n\
             💡 有效组合:\n\
                • --compress 单独：只要输出 < 输入\n\
                • --explore 单独：寻找尽可能更小的输出\n\
                • --explore --match-quality --compress：精确质量匹配 + 必须压缩".to_string()
        ),
        
        // 有效组合 6: --explore --match-quality --compress
        (true, true, true) => FlagValidation::Valid(FlagMode::PreciseQualityWithCompress),
        
        // 有效组合 5: --explore --match-quality
        (true, true, false) => FlagValidation::Valid(FlagMode::PreciseQuality),
        
        // 有效组合 4: --compress --match-quality
        (false, true, true) => FlagValidation::Valid(FlagMode::CompressWithQuality),
        
        // 有效组合 3: --match-quality 单独
        (false, true, false) => FlagValidation::Valid(FlagMode::QualityOnly),
        
        // 有效组合 2: --explore 单独
        (true, false, false) => FlagValidation::Valid(FlagMode::ExploreOnly),
        
        // 有效组合 1: --compress 单独
        (false, false, true) => FlagValidation::Valid(FlagMode::CompressOnly),
        
        // 默认模式：无特殊 flag
        (false, false, false) => FlagValidation::Valid(FlagMode::Default),
    }
}

/// 验证 flag 组合并返回 Result
///
/// 便捷函数，直接返回 Result 类型，方便在 ? 操作符中使用
pub fn validate_flags_result(explore: bool, match_quality: bool, compress: bool) -> Result<FlagMode, String> {
    match validate_flags(explore, match_quality, compress) {
        FlagValidation::Valid(mode) => Ok(mode),
        FlagValidation::Invalid(err) => Err(err),
    }
}

/// 打印 flag 组合帮助信息
pub fn print_flag_help() {
    eprintln!("📋 Flag Combination Guide:");
    eprintln!("   --compress              Just need output < input (even 1KB)");
    eprintln!("   --explore               Find smallest possible output");
    eprintln!("   --match-quality         Rough SSIM validation");
    eprintln!("   --compress --match-quality");
    eprintln!("                           Output < input + rough SSIM validation");
    eprintln!("   --explore --match-quality");
    eprintln!("                           Precise quality match (highest SSIM, ignore size)");
    eprintln!("   --explore --match-quality --compress");
    eprintln!("                           Precise quality match + must compress");
    eprintln!("");
    eprintln!("❌ Invalid combinations:");
    eprintln!("   --explore --compress    Conflicting goals, please add --match-quality");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════════
    // 基础有效组合测试
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_valid_combinations() {
        // 默认模式
        assert!(matches!(
            validate_flags(false, false, false),
            FlagValidation::Valid(FlagMode::Default)
        ));
        
        // --compress 单独
        assert!(matches!(
            validate_flags(false, false, true),
            FlagValidation::Valid(FlagMode::CompressOnly)
        ));
        
        // --explore 单独
        assert!(matches!(
            validate_flags(true, false, false),
            FlagValidation::Valid(FlagMode::ExploreOnly)
        ));
        
        // --match-quality 单独
        assert!(matches!(
            validate_flags(false, true, false),
            FlagValidation::Valid(FlagMode::QualityOnly)
        ));
        
        // --compress --match-quality
        assert!(matches!(
            validate_flags(false, true, true),
            FlagValidation::Valid(FlagMode::CompressWithQuality)
        ));
        
        // --explore --match-quality
        assert!(matches!(
            validate_flags(true, true, false),
            FlagValidation::Valid(FlagMode::PreciseQuality)
        ));
        
        // --explore --match-quality --compress
        assert!(matches!(
            validate_flags(true, true, true),
            FlagValidation::Valid(FlagMode::PreciseQualityWithCompress)
        ));
    }

    // ═══════════════════════════════════════════════════════════════
    // 无效组合测试
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_invalid_combination() {
        // --explore --compress（无效）
        assert!(matches!(
            validate_flags(true, false, true),
            FlagValidation::Invalid(_)
        ));
    }

    #[test]
    fn test_invalid_combination_error_message() {
        // 验证错误信息包含关键内容
        if let FlagValidation::Invalid(err) = validate_flags(true, false, true) {
            assert!(err.contains("--explore --compress"), "错误信息应包含无效组合");
            assert!(err.contains("目标冲突") || err.contains("冲突"), "错误信息应说明冲突原因");
            assert!(err.contains("--match-quality"), "错误信息应建议添加 --match-quality");
        } else {
            panic!("应该返回 Invalid");
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // Result API 测试
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_validate_flags_result() {
        assert!(validate_flags_result(true, true, false).is_ok());
        assert!(validate_flags_result(true, false, true).is_err());
    }

    #[test]
    fn test_validate_flags_result_all_combinations() {
        // 所有 8 种组合的完整测试
        let test_cases = [
            // (explore, match_quality, compress, expected_ok)
            (false, false, false, true),  // Default
            (false, false, true, true),   // CompressOnly
            (false, true, false, true),   // QualityOnly
            (false, true, true, true),    // CompressWithQuality
            (true, false, false, true),   // ExploreOnly
            (true, false, true, false),   // ❌ Invalid: explore + compress
            (true, true, false, true),    // PreciseQuality
            (true, true, true, true),     // PreciseQualityWithCompress
        ];
        
        for (explore, match_quality, compress, expected_ok) in test_cases {
            let result = validate_flags_result(explore, match_quality, compress);
            assert_eq!(
                result.is_ok(), expected_ok,
                "validate_flags_result({}, {}, {}) should be {}",
                explore, match_quality, compress,
                if expected_ok { "Ok" } else { "Err" }
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // FlagMode 方法测试
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_flag_mode_display() {
        assert_eq!(format!("{}", FlagMode::Default), "Default");
        assert_eq!(format!("{}", FlagMode::CompressOnly), "--compress");
        assert_eq!(format!("{}", FlagMode::ExploreOnly), "--explore");
        assert_eq!(format!("{}", FlagMode::QualityOnly), "--match-quality");
        assert_eq!(format!("{}", FlagMode::CompressWithQuality), "--compress --match-quality");
        assert_eq!(format!("{}", FlagMode::PreciseQuality), "--explore --match-quality");
        assert_eq!(format!("{}", FlagMode::PreciseQualityWithCompress), "--explore --match-quality --compress");
    }

    #[test]
    fn test_flag_mode_description_cn() {
        // 确保所有模式都有中文描述
        assert!(!FlagMode::Default.description_cn().is_empty());
        assert!(!FlagMode::CompressOnly.description_cn().is_empty());
        assert!(!FlagMode::ExploreOnly.description_cn().is_empty());
        assert!(!FlagMode::QualityOnly.description_cn().is_empty());
        assert!(!FlagMode::CompressWithQuality.description_cn().is_empty());
        assert!(!FlagMode::PreciseQuality.description_cn().is_empty());
        assert!(!FlagMode::PreciseQualityWithCompress.description_cn().is_empty());
        
        // 验证描述内容合理
        assert!(FlagMode::CompressOnly.description_cn().contains("压缩"));
        assert!(FlagMode::ExploreOnly.description_cn().contains("探索") || FlagMode::ExploreOnly.description_cn().contains("最小"));
        assert!(FlagMode::PreciseQuality.description_cn().contains("精确") || FlagMode::PreciseQuality.description_cn().contains("SSIM"));
    }

    #[test]
    fn test_flag_mode_description_en() {
        // 确保所有模式都有英文描述
        assert!(!FlagMode::Default.description_en().is_empty());
        assert!(!FlagMode::CompressOnly.description_en().is_empty());
        assert!(!FlagMode::ExploreOnly.description_en().is_empty());
        assert!(!FlagMode::QualityOnly.description_en().is_empty());
        assert!(!FlagMode::CompressWithQuality.description_en().is_empty());
        assert!(!FlagMode::PreciseQuality.description_en().is_empty());
        assert!(!FlagMode::PreciseQualityWithCompress.description_en().is_empty());
        
        // 验证描述内容合理
        assert!(FlagMode::CompressOnly.description_en().to_lowercase().contains("compress"));
        assert!(FlagMode::PreciseQuality.description_en().to_lowercase().contains("precise") 
            || FlagMode::PreciseQuality.description_en().to_lowercase().contains("ssim"));
    }

    // ═══════════════════════════════════════════════════════════════
    // 边缘案例测试
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_flag_mode_equality() {
        // 测试 FlagMode 的 PartialEq 实现
        assert_eq!(FlagMode::Default, FlagMode::Default);
        assert_ne!(FlagMode::Default, FlagMode::CompressOnly);
        assert_ne!(FlagMode::ExploreOnly, FlagMode::CompressOnly);
        assert_eq!(FlagMode::PreciseQualityWithCompress, FlagMode::PreciseQualityWithCompress);
    }

    #[test]
    fn test_flag_mode_clone() {
        // 测试 FlagMode 的 Clone 实现
        let mode = FlagMode::PreciseQuality;
        let cloned = mode.clone();
        assert_eq!(mode, cloned);
    }

    #[test]
    fn test_flag_mode_copy() {
        // 测试 FlagMode 的 Copy 实现
        let mode = FlagMode::CompressWithQuality;
        let copied = mode; // Copy, not move
        assert_eq!(mode, copied);
    }

    #[test]
    fn test_flag_mode_debug() {
        // 测试 FlagMode 的 Debug 实现
        let debug_str = format!("{:?}", FlagMode::PreciseQualityWithCompress);
        assert!(debug_str.contains("PreciseQualityWithCompress"));
    }

    // ═══════════════════════════════════════════════════════════════
    // 语义正确性测试
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_semantic_compress_only_vs_explore_only() {
        // --compress: 只要更小即可
        // --explore: 寻找最小输出
        // 两者语义不同，不应混淆
        let compress = validate_flags_result(false, false, true).unwrap();
        let explore = validate_flags_result(true, false, false).unwrap();
        
        assert_ne!(compress, explore, "CompressOnly 和 ExploreOnly 应该是不同的模式");
        assert_eq!(compress, FlagMode::CompressOnly);
        assert_eq!(explore, FlagMode::ExploreOnly);
    }

    #[test]
    fn test_semantic_quality_modes() {
        // --match-quality: 粗略验证
        // --explore --match-quality: 精确匹配
        let basic = validate_flags_result(false, true, false).unwrap();
        let precise = validate_flags_result(true, true, false).unwrap();
        
        assert_ne!(basic, precise, "QualityOnly 和 PreciseQuality 应该是不同的模式");
        assert_eq!(basic, FlagMode::QualityOnly);
        assert_eq!(precise, FlagMode::PreciseQuality);
    }

    #[test]
    fn test_semantic_compress_with_quality_vs_precise_with_compress() {
        // --compress --match-quality: 压缩 + 粗略验证
        // --explore --match-quality --compress: 精确匹配 + 必须压缩
        let basic_compress = validate_flags_result(false, true, true).unwrap();
        let precise_compress = validate_flags_result(true, true, true).unwrap();
        
        assert_ne!(basic_compress, precise_compress, 
            "CompressWithQuality 和 PreciseQualityWithCompress 应该是不同的模式");
        assert_eq!(basic_compress, FlagMode::CompressWithQuality);
        assert_eq!(precise_compress, FlagMode::PreciseQualityWithCompress);
    }

    // ═══════════════════════════════════════════════════════════════
    // 完整性测试 - 确保所有 8 种布尔组合都有处理
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_all_boolean_combinations_handled() {
        // 穷举所有 2^3 = 8 种组合，确保都有处理（不会 panic）
        for explore in [false, true] {
            for match_quality in [false, true] {
                for compress in [false, true] {
                    let result = validate_flags(explore, match_quality, compress);
                    // 确保返回的是 Valid 或 Invalid，不会 panic
                    match result {
                        FlagValidation::Valid(_) => {}
                        FlagValidation::Invalid(_) => {}
                    }
                }
            }
        }
    }

    #[test]
    fn test_exactly_one_invalid_combination() {
        // 确保只有一种无效组合：--explore --compress
        let mut invalid_count = 0;
        for explore in [false, true] {
            for match_quality in [false, true] {
                for compress in [false, true] {
                    if let FlagValidation::Invalid(_) = validate_flags(explore, match_quality, compress) {
                        invalid_count += 1;
                        // 验证是正确的无效组合
                        assert!(explore && !match_quality && compress,
                            "唯一的无效组合应该是 explore=true, match_quality=false, compress=true");
                    }
                }
            }
        }
        assert_eq!(invalid_count, 1, "应该只有一种无效组合");
    }
}
