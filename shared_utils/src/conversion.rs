//! Conversion Utilities Module
//! 
//! Provides common conversion functionality shared across all tools:
//! - ConversionResult: Unified result structure
//! - ConvertOptions: Common conversion options
//! - Anti-duplicate mechanism: Track processed files
//! - Result builders: Reduce boilerplate code
//! - Size formatting: Unified message formatting

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ============================================================================
// Global processed files tracker (anti-duplicate)
// ============================================================================

lazy_static::lazy_static! {
    static ref PROCESSED_FILES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
}

/// Check if file has already been processed (anti-duplicate)
pub fn is_already_processed(path: &Path) -> bool {
    let canonical = path.canonicalize().ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.display().to_string());
    
    let processed = PROCESSED_FILES.lock().unwrap();
    processed.contains(&canonical)
}

/// Mark file as processed
pub fn mark_as_processed(path: &Path) {
    let canonical = path.canonicalize().ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.display().to_string());
    
    let mut processed = PROCESSED_FILES.lock().unwrap();
    processed.insert(canonical);
}

/// Clear processed files list
pub fn clear_processed_list() {
    let mut processed = PROCESSED_FILES.lock().unwrap();
    processed.clear();
}

// ============================================================================
// 🔥 Atomic Operation Protection (断电保护)
// Re-exported from checkpoint module for backward compatibility
// ============================================================================

pub use crate::checkpoint::{verify_output_integrity, safe_delete_original};

/// Load processed files list from disk
pub fn load_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !list_path.exists() {
        return Ok(());
    }
    
    let file = fs::File::open(list_path)?;
    let reader = BufReader::new(file);
    let mut processed = PROCESSED_FILES.lock().unwrap();
    
    for path in reader.lines().flatten() {
        processed.insert(path);
    }
    
    Ok(())
}

/// Save processed files list to disk
pub fn save_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let processed = PROCESSED_FILES.lock().unwrap();
    let mut file = fs::File::create(list_path)?;
    
    for path in processed.iter() {
        writeln!(file, "{}", path)?;
    }
    
    Ok(())
}

// ============================================================================
// Conversion Result
// ============================================================================

/// Unified conversion result structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionResult {
    pub success: bool,
    pub input_path: String,
    pub output_path: Option<String>,
    pub input_size: u64,
    pub output_size: Option<u64>,
    pub size_reduction: Option<f64>,
    pub message: String,
    pub skipped: bool,
    pub skip_reason: Option<String>,
}

impl ConversionResult {
    /// Create a skipped result (already processed)
    /// 
    /// 注意：这里使用 unwrap_or(0) 是合理的，因为：
    /// 1. 这是跳过场景，文件大小仅用于显示目的
    /// 2. 如果文件不存在（极端情况），返回0不会影响功能
    /// 3. 跳过的文件不会被转换，所以大小信息不影响质量
    pub fn skipped_duplicate(input: &Path) -> Self {
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        }
    }
    
    /// Create a skipped result (output exists)
    /// 
    /// 注意：这里使用 unwrap_or(0) 是合理的（同上）
    pub fn skipped_exists(input: &Path, output: &Path) -> Self {
        let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        }
    }
    
    /// Create a skipped result (size increase - rollback)
    pub fn skipped_size_increase(input: &Path, input_size: u64, output_size: u64) -> Self {
        let increase_pct = (output_size as f64 / input_size as f64 - 1.0) * 100.0;
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!("Skipped: Output would be larger (+{:.1}%)", increase_pct),
            skipped: true,
            skip_reason: Some("size_increase".to_string()),
        }
    }
    
    /// Create a successful conversion result
    pub fn success(
        input: &Path,
        output: &Path,
        input_size: u64,
        output_size: u64,
        format_name: &str,
        extra_info: Option<&str>,
    ) -> Self {
        let reduction = 1.0 - (output_size as f64 / input_size as f64);
        let reduction_pct = reduction * 100.0;
        
        let message = if reduction >= 0.0 {
            match extra_info {
                Some(info) => format!("{} ({}): size reduced {:.1}%", format_name, info, reduction_pct),
                None => format!("{} conversion successful: size reduced {:.1}%", format_name, reduction_pct),
            }
        } else {
            match extra_info {
                Some(info) => format!("{} ({}): size increased {:.1}%", format_name, info, -reduction_pct),
                None => format!("{} conversion successful: size increased {:.1}%", format_name, -reduction_pct),
            }
        };
        
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: Some(output_size),
            size_reduction: Some(reduction_pct),
            message,
            skipped: false,
            skip_reason: None,
        }
    }
}

// ============================================================================
// Conversion Options
// ============================================================================

/// Common conversion options
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct ConvertOptions {
    /// Force conversion even if already processed
    pub force: bool,
    /// Output directory (None = same as input)
    pub output_dir: Option<PathBuf>,
    /// Delete original after successful conversion
    pub delete_original: bool,
    /// In-place conversion: convert and delete original (effectively "replace")
    /// When true, the original file is deleted after successful conversion
    /// This is equivalent to delete_original but with clearer semantics
    pub in_place: bool,
    /// 探索模式：寻找更小的文件大小
    /// - 单独使用：仅探索更小大小，提示裁判验证准确度
    /// - 与 match_quality 组合：精确质量匹配（二分搜索 + SSIM 验证）
    pub explore: bool,
    /// 质量匹配模式：匹配输入质量
    /// - 单独使用：使用算法预测的 CRF + SSIM 验证
    /// - 与 explore 组合：精确质量匹配（二分搜索 + SSIM 验证）
    pub match_quality: bool,
    /// 🍎 Apple compatibility mode: Convert non-Apple-compatible formats to HEVC
    /// When enabled, AV1/VP9 animated images will be converted to HEVC MP4
    /// instead of being skipped as "modern format"
    pub apple_compat: bool,
}


impl ConvertOptions {
    /// Check if original should be deleted (either via delete_original or in_place)
    pub fn should_delete_original(&self) -> bool {
        self.delete_original || self.in_place
    }
    
    /// 获取探索模式
    pub fn explore_mode(&self) -> crate::video_explorer::ExploreMode {
        match (self.explore, self.match_quality) {
            (true, true) => crate::video_explorer::ExploreMode::PreciseQualityMatch,
            (true, false) => crate::video_explorer::ExploreMode::SizeOnly,
            (false, true) => crate::video_explorer::ExploreMode::QualityMatch,
            (false, false) => crate::video_explorer::ExploreMode::QualityMatch, // 默认使用质量匹配
        }
    }
}

// ============================================================================
// Output Path Utilities
// ============================================================================

/// Determine output path and ensure directory exists
/// Returns Err if input and output would be the same file
pub fn determine_output_path(input: &Path, extension: &str, output_dir: &Option<PathBuf>) -> Result<PathBuf, String> {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    
    let output = match output_dir {
        Some(dir) => {
            // Ensure output directory exists
            let _ = fs::create_dir_all(dir);
            dir.join(format!("{}.{}", stem, extension))
        }
        None => input.with_extension(extension),
    };
    
    // 🔥 检测输入输出路径冲突
    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let output_canonical = if output.exists() {
        output.canonicalize().unwrap_or_else(|_| output.clone())
    } else {
        output.clone()
    };
    
    if input_canonical == output_canonical || input == output {
        return Err(format!(
            "❌ 输入和输出路径相同: {}\n\
             💡 建议:\n\
             - 使用 --output/-o 指定不同的输出目录\n\
             - 或使用 --in-place 参数进行原地替换（会删除原文件）",
            input.display()
        ));
    }
    
    // Ensure output directory exists
    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }
    
    Ok(output)
}

// ============================================================================
// Size Formatting
// ============================================================================

/// Format size reduction/increase message
pub fn format_size_change(input_size: u64, output_size: u64) -> String {
    let reduction = 1.0 - (output_size as f64 / input_size as f64);
    let reduction_pct = reduction * 100.0;
    
    if reduction >= 0.0 {
        format!("size reduced {:.1}%", reduction_pct)
    } else {
        format!("size increased {:.1}%", -reduction_pct)
    }
}

/// Calculate size reduction percentage (positive = smaller, negative = larger)
pub fn calculate_size_reduction(input_size: u64, output_size: u64) -> f64 {
    (1.0 - (output_size as f64 / input_size as f64)) * 100.0
}

// ============================================================================
// Pre-conversion Checks
// ============================================================================

/// Perform standard pre-conversion checks
/// Returns Some(ConversionResult) if should skip, None if should proceed
pub fn pre_conversion_check(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> Option<ConversionResult> {
    // Anti-duplicate check
    if !options.force && is_already_processed(input) {
        return Some(ConversionResult::skipped_duplicate(input));
    }
    
    // Output exists check
    if output.exists() && !options.force {
        return Some(ConversionResult::skipped_exists(input, output));
    }
    
    None
}

// ============================================================================
// Post-conversion Actions
// ============================================================================

/// Perform standard post-conversion actions
pub fn post_conversion_actions(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> std::io::Result<()> {
    // Copy metadata
    if let Err(e) = crate::preserve_metadata(input, output) {
        eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }
    
    // Mark as processed
    mark_as_processed(input);
    
    // 🔥 Safe delete with integrity check (断电保护)
    if options.should_delete_original() {
        // Minimum output size: at least 100 bytes for any valid media file
        safe_delete_original(input, output, 100)?;
    }
    
    Ok(())
}

// ============================================================
// 🔬 PRECISION VALIDATION TESTS ("裁判" Tests)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    // ============================================================
    // Size Reduction Calculation Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_calculate_size_reduction_50_percent() {
        // 1000 -> 500 = 50% reduction
        let reduction = calculate_size_reduction(1000, 500);
        assert!((reduction - 50.0).abs() < 0.01,
            "1000->500 should be 50% reduction, got {}", reduction);
    }
    
    #[test]
    fn test_calculate_size_reduction_75_percent() {
        // 1000 -> 250 = 75% reduction
        let reduction = calculate_size_reduction(1000, 250);
        assert!((reduction - 75.0).abs() < 0.01,
            "1000->250 should be 75% reduction, got {}", reduction);
    }
    
    #[test]
    fn test_calculate_size_reduction_no_change() {
        // Same size = 0% reduction
        let reduction = calculate_size_reduction(1000, 1000);
        assert!((reduction - 0.0).abs() < 0.01,
            "Same size should be 0% reduction, got {}", reduction);
    }
    
    #[test]
    fn test_calculate_size_reduction_increase() {
        // 500 -> 1000 = -100% (doubled)
        let reduction = calculate_size_reduction(500, 1000);
        assert!((reduction - (-100.0)).abs() < 0.01,
            "500->1000 should be -100% (increase), got {}", reduction);
    }
    
    #[test]
    fn test_calculate_size_reduction_small_increase() {
        // 1000 -> 1100 = -10% increase
        let reduction = calculate_size_reduction(1000, 1100);
        assert!((reduction - (-10.0)).abs() < 0.01,
            "1000->1100 should be -10% (increase), got {}", reduction);
    }
    
    // ============================================================
    // 🔬 Strict Precision Tests (裁判机制)
    // ============================================================
    
    /// Strict test: Size reduction formula must be mathematically correct
    #[test]
    fn test_strict_size_reduction_formula() {
        // Formula: (1 - output/input) * 100
        let test_cases = [
            (1000u64, 500u64, 50.0f64),
            (1000, 250, 75.0),
            (1000, 100, 90.0),
            (1000, 900, 10.0),
            (1000, 1000, 0.0),
            (1000, 2000, -100.0),
            (1000, 1500, -50.0),
        ];
        
        for (input, output, expected) in test_cases {
            let result = calculate_size_reduction(input, output);
            let expected_calc = (1.0 - (output as f64 / input as f64)) * 100.0;
            
            assert!((result - expected).abs() < 0.001,
                "STRICT: {}->{}  expected {}, got {}", input, output, expected, result);
            assert!((result - expected_calc).abs() < 0.0001,
                "STRICT: Formula mismatch for {}->{}", input, output);
        }
    }
    
    /// Strict test: Large file sizes (GB range)
    #[test]
    fn test_strict_large_file_sizes() {
        // 10GB -> 5GB = 50% reduction
        let reduction = calculate_size_reduction(10_000_000_000, 5_000_000_000);
        assert!((reduction - 50.0).abs() < 0.001,
            "STRICT: 10GB->5GB should be exactly 50%, got {}", reduction);
        
        // 100GB -> 25GB = 75% reduction
        let reduction = calculate_size_reduction(100_000_000_000, 25_000_000_000);
        assert!((reduction - 75.0).abs() < 0.001,
            "STRICT: 100GB->25GB should be exactly 75%, got {}", reduction);
    }
    
    /// Strict test: Small file sizes (bytes range)
    #[test]
    fn test_strict_small_file_sizes() {
        // 100 bytes -> 50 bytes = 50% reduction
        let reduction = calculate_size_reduction(100, 50);
        assert!((reduction - 50.0).abs() < 0.001,
            "STRICT: 100->50 bytes should be exactly 50%, got {}", reduction);
    }
    
    // ============================================================
    // Format Size Change Message Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_format_size_change_reduction() {
        let msg = format_size_change(1000, 500);
        assert!(msg.contains("reduced"), "Should say 'reduced' for smaller output");
        assert!(msg.contains("50.0%"), "Should show 50.0% for half size");
    }
    
    #[test]
    fn test_format_size_change_increase() {
        let msg = format_size_change(500, 1000);
        assert!(msg.contains("increased"), "Should say 'increased' for larger output");
        assert!(msg.contains("100.0%"), "Should show 100.0% for doubled size");
    }
    
    #[test]
    fn test_format_size_change_no_change() {
        let msg = format_size_change(1000, 1000);
        assert!(msg.contains("reduced"), "Same size shows as 0% reduced");
        assert!(msg.contains("0.0%"), "Should show 0.0% for same size");
    }
    
    // ============================================================
    // Output Path Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_determine_output_path() {
        let input = Path::new("/path/to/image.png");
        let output = determine_output_path(input, "jxl", &None).unwrap();
        assert_eq!(output, Path::new("/path/to/image.jxl"));
    }
    
    #[test]
    fn test_determine_output_path_with_dir() {
        let input = Path::new("/path/to/image.png");
        let output_dir = Some(PathBuf::from("/output"));
        let output = determine_output_path(input, "avif", &output_dir).unwrap();
        assert_eq!(output, Path::new("/output/image.avif"));
    }
    
    #[test]
    fn test_determine_output_path_various_extensions() {
        let input = Path::new("/path/to/video.mp4");
        
        let webm = determine_output_path(input, "webm", &None).unwrap();
        assert_eq!(webm, Path::new("/path/to/video.webm"));
        
        let mkv = determine_output_path(input, "mkv", &None).unwrap();
        assert_eq!(mkv, Path::new("/path/to/video.mkv"));
    }
    
    // ============================================================
    // ConversionResult Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_conversion_result_success() {
        let input = Path::new("/test/input.png");
        let output = Path::new("/test/output.avif");
        
        let result = ConversionResult::success(input, output, 1000, 500, "AVIF", None);
        
        assert!(result.success);
        assert!(!result.skipped);
        assert_eq!(result.input_size, 1000);
        assert_eq!(result.output_size, Some(500));
        assert!((result.size_reduction.unwrap() - 50.0).abs() < 0.1);
        assert!(result.message.contains("reduced"));
    }
    
    #[test]
    fn test_conversion_result_size_increase() {
        let input = Path::new("/test/input.png");
        
        let result = ConversionResult::skipped_size_increase(input, 500, 1000);
        
        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("size_increase".to_string()));
        assert!(result.message.contains("larger"));
    }
    
    // ============================================================
    // ConvertOptions Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_convert_options_default() {
        let opts = ConvertOptions::default();
        
        assert!(!opts.force);
        assert!(opts.output_dir.is_none());
        assert!(!opts.delete_original);
        assert!(!opts.in_place);
        assert!(!opts.should_delete_original());
    }
    
    #[test]
    fn test_convert_options_delete_original() {
        let mut opts = ConvertOptions::default();
        opts.delete_original = true;
        
        assert!(opts.should_delete_original());
    }
    
    #[test]
    fn test_convert_options_in_place() {
        let mut opts = ConvertOptions::default();
        opts.in_place = true;
        
        assert!(opts.should_delete_original());
    }
    
    // ============================================================
    // Consistency Tests (裁判机制)
    // ============================================================
    
    #[test]
    fn test_consistency_size_reduction() {
        // Same input should always produce same output
        for _ in 0..10 {
            let result1 = calculate_size_reduction(1000, 500);
            let result2 = calculate_size_reduction(1000, 500);
            assert!((result1 - result2).abs() < 0.0000001,
                "Size reduction calculation must be deterministic");
        }
    }
    
    #[test]
    fn test_consistency_format_message() {
        // Same input should always produce same message
        let msg1 = format_size_change(1000, 500);
        let msg2 = format_size_change(1000, 500);
        assert_eq!(msg1, msg2, "Format message must be deterministic");
    }
}
