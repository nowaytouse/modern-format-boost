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

/// Load processed files list from disk
pub fn load_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !list_path.exists() {
        return Ok(());
    }
    
    let file = fs::File::open(list_path)?;
    let reader = BufReader::new(file);
    let mut processed = PROCESSED_FILES.lock().unwrap();
    
    for line in reader.lines() {
        if let Ok(path) = line {
            processed.insert(path);
        }
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
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            force: false,
            output_dir: None,
            delete_original: false,
            in_place: false,
        }
    }
}

impl ConvertOptions {
    /// Check if original should be deleted (either via delete_original or in_place)
    pub fn should_delete_original(&self) -> bool {
        self.delete_original || self.in_place
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
    
    if input_canonical == output_canonical || input == &output {
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
    
    // Delete original if requested (via delete_original or in_place)
    if options.should_delete_original() {
        fs::remove_file(input)?;
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_format_size_change_reduction() {
        let msg = format_size_change(1000, 500);
        assert!(msg.contains("reduced"));
        assert!(msg.contains("50.0%"));
    }
    
    #[test]
    fn test_format_size_change_increase() {
        let msg = format_size_change(500, 1000);
        assert!(msg.contains("increased"));
        assert!(msg.contains("100.0%"));
    }
    
    #[test]
    fn test_calculate_size_reduction() {
        assert!((calculate_size_reduction(1000, 500) - 50.0).abs() < 0.1);
        assert!((calculate_size_reduction(500, 1000) - (-100.0)).abs() < 0.1);
    }
    
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
}
