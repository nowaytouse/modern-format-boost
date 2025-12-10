//! Media Date Analysis Module
//!
//! Deep EXIF/XMP date extraction and analysis for media files.
//! 
//! Priority order (most reliable to least):
//! 1. XMP-photoshop:DateCreated - Photoshop original creation
//! 2. XMP-xmp:CreateDate - XMP creation date
//! 3. XMP-xmpMM:HistoryWhen - Edit history timestamps
//! 4. EXIF:DateTimeOriginal - Camera original
//! 5. EXIF:CreateDate - Generic creation
//! 6. XMP-xmp:MetadataDate - When metadata was last modified
//!
//! ⚠️ FileModifyDate is EXCLUDED as it's unreliable (download/copy time)

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use chrono::{NaiveDateTime, Datelike};
use serde::{Deserialize, Serialize};

/// Date source priority (higher = more reliable)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DateSource {
    XmpPhotoshop,    // Priority 1: XMP-photoshop:DateCreated
    XmpCreateDate,   // Priority 2: XMP-xmp:CreateDate
    XmpHistory,      // Priority 3: XMP-xmpMM:HistoryWhen
    ExifOriginal,    // Priority 4: EXIF:DateTimeOriginal
    ExifCreateDate,  // Priority 5: EXIF:CreateDate
    XmpMetadata,     // Priority 6: XMP-xmp:MetadataDate
    None,            // No valid date found
}

impl DateSource {
    pub fn priority(&self) -> u8 {
        match self {
            DateSource::XmpPhotoshop => 6,
            DateSource::XmpCreateDate => 5,
            DateSource::XmpHistory => 4,
            DateSource::ExifOriginal => 3,
            DateSource::ExifCreateDate => 2,
            DateSource::XmpMetadata => 1,
            DateSource::None => 0,
        }
    }
    
    pub fn name(&self) -> &'static str {
        match self {
            DateSource::XmpPhotoshop => "XMP-Photoshop",
            DateSource::XmpCreateDate => "XMP-CreateDate",
            DateSource::XmpHistory => "XMP-History",
            DateSource::ExifOriginal => "EXIF-Original",
            DateSource::ExifCreateDate => "EXIF-CreateDate",
            DateSource::XmpMetadata => "XMP-Metadata",
            DateSource::None => "None",
        }
    }
}

/// Result of date extraction for a single file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDateInfo {
    pub filename: String,
    pub path: String,
    pub best_date: Option<NaiveDateTime>,
    pub date_source: DateSource,
    pub all_dates: HashMap<String, String>,
}

/// Analysis results for a directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateAnalysisResult {
    pub total_files: usize,
    pub files_with_dates: usize,
    pub files_without_dates: usize,
    pub earliest: Option<FileDateInfo>,
    pub latest: Option<FileDateInfo>,
    pub by_source: HashMap<String, usize>,
    pub by_year: HashMap<i32, usize>,
    pub by_month: HashMap<String, usize>,
    pub files: Vec<FileDateInfo>,
}

/// Configuration for date analysis
#[derive(Debug, Clone)]
pub struct DateAnalysisConfig {
    pub min_valid_year: i32,
    pub max_valid_year: i32,
    pub extensions: Vec<String>,
}

impl Default for DateAnalysisConfig {
    fn default() -> Self {
        let current_year = chrono::Local::now().year();
        Self {
            min_valid_year: 1990,
            max_valid_year: current_year + 1,
            extensions: vec![
                "jpg".to_string(), "jpeg".to_string(), "png".to_string(),
                "gif".to_string(), "webp".to_string(), "mp4".to_string(),
                "mov".to_string(), "jfif".to_string(), "heic".to_string(),
                "avif".to_string(), "jxl".to_string(),
            ],
        }
    }
}

/// Raw exiftool JSON output structure
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fields used for deserialization completeness
struct ExiftoolOutput {
    #[serde(rename = "SourceFile")]
    source_file: Option<String>,
    #[serde(rename = "FileName")]
    file_name: Option<String>,
    #[serde(rename = "XMP-photoshop:DateCreated")]
    xmp_ps_created: Option<String>,
    #[serde(rename = "XMP-xmp:CreateDate")]
    xmp_created: Option<String>,
    #[serde(rename = "XMP-xmp:MetadataDate")]
    xmp_metadata: Option<String>,
    #[serde(rename = "XMP-xmp:ModifyDate")]
    xmp_modified: Option<String>,
    #[serde(rename = "XMP-xmpMM:HistoryWhen")]
    xmp_history: Option<serde_json::Value>, // Can be string or array
    #[serde(rename = "EXIF:DateTimeOriginal")]
    exif_original: Option<String>,
    #[serde(rename = "EXIF:CreateDate")]
    exif_created: Option<String>,
    #[serde(rename = "EXIF:ModifyDate")]
    exif_modified: Option<String>,
}

/// Analyze media dates in a directory
pub fn analyze_directory(dir: &Path, config: &DateAnalysisConfig) -> Result<DateAnalysisResult, String> {
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", dir.display()));
    }
    
    // Build exiftool command
    let output = Command::new("exiftool")
        .arg("-r")
        .arg("-j")
        .arg("-G1")
        .arg("-XMP-photoshop:DateCreated")
        .arg("-XMP-xmp:CreateDate")
        .arg("-XMP-xmp:MetadataDate")
        .arg("-XMP-xmp:ModifyDate")
        .arg("-XMP-xmpMM:HistoryWhen")
        .arg("-EXIF:DateTimeOriginal")
        .arg("-EXIF:CreateDate")
        .arg("-EXIF:ModifyDate")
        .arg("-FileName")
        .args(config.extensions.iter().flat_map(|e| vec!["-ext".to_string(), e.clone()]))
        .arg(dir)
        .output()
        .map_err(|e| format!("Failed to run exiftool: {}", e))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // exiftool returns non-zero if no files found, which is OK
        if !stderr.contains("No matching files") {
            return Err(format!("exiftool failed: {}", stderr));
        }
    }
    
    let json_str = String::from_utf8_lossy(&output.stdout);
    if json_str.trim().is_empty() || json_str.trim() == "[]" {
        return Ok(DateAnalysisResult {
            total_files: 0,
            files_with_dates: 0,
            files_without_dates: 0,
            earliest: None,
            latest: None,
            by_source: HashMap::new(),
            by_year: HashMap::new(),
            by_month: HashMap::new(),
            files: Vec::new(),
        });
    }
    
    let raw_data: Vec<ExiftoolOutput> = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse exiftool JSON: {}", e))?;
    
    // Process each file
    let mut files: Vec<FileDateInfo> = Vec::new();
    let mut by_source: HashMap<String, usize> = HashMap::new();
    let mut by_year: HashMap<i32, usize> = HashMap::new();
    let mut by_month: HashMap<String, usize> = HashMap::new();
    
    for item in raw_data {
        let file_info = extract_best_date(&item, config);
        
        // Update statistics
        *by_source.entry(file_info.date_source.name().to_string()).or_insert(0) += 1;
        
        if let Some(date) = &file_info.best_date {
            *by_year.entry(date.year()).or_insert(0) += 1;
            let month_key = format!("{:04}-{:02}", date.year(), date.month());
            *by_month.entry(month_key).or_insert(0) += 1;
        }
        
        files.push(file_info);
    }
    
    // Find earliest and latest
    let files_with_dates: Vec<_> = files.iter()
        .filter(|f| f.best_date.is_some())
        .collect();
    
    let earliest = files_with_dates.iter()
        .min_by_key(|f| f.best_date)
        .cloned()
        .cloned();
    
    let latest = files_with_dates.iter()
        .max_by_key(|f| f.best_date)
        .cloned()
        .cloned();
    
    let files_with_dates_count = files_with_dates.len();
    let total = files.len();
    
    Ok(DateAnalysisResult {
        total_files: total,
        files_with_dates: files_with_dates_count,
        files_without_dates: total - files_with_dates_count,
        earliest,
        latest,
        by_source,
        by_year,
        by_month,
        files,
    })
}

/// Extract the best date from exiftool output
fn extract_best_date(item: &ExiftoolOutput, config: &DateAnalysisConfig) -> FileDateInfo {
    let filename = item.file_name.clone().unwrap_or_default();
    let path = item.source_file.clone().unwrap_or_default();
    
    let mut all_dates = HashMap::new();
    
    // Collect all dates
    if let Some(d) = &item.xmp_ps_created { all_dates.insert("XMP-Photoshop".to_string(), d.clone()); }
    if let Some(d) = &item.xmp_created { all_dates.insert("XMP-CreateDate".to_string(), d.clone()); }
    if let Some(d) = &item.xmp_metadata { all_dates.insert("XMP-Metadata".to_string(), d.clone()); }
    if let Some(d) = &item.exif_original { all_dates.insert("EXIF-Original".to_string(), d.clone()); }
    if let Some(d) = &item.exif_created { all_dates.insert("EXIF-CreateDate".to_string(), d.clone()); }
    
    // Handle XMP history (can be array)
    let xmp_history_str = match &item.xmp_history {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(arr)) => arr.first()
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };
    if let Some(d) = &xmp_history_str { all_dates.insert("XMP-History".to_string(), d.clone()); }
    
    // Try each source in priority order
    let candidates = [
        (&item.xmp_ps_created, DateSource::XmpPhotoshop),
        (&item.xmp_created, DateSource::XmpCreateDate),
        (&xmp_history_str, DateSource::XmpHistory),
        (&item.exif_original, DateSource::ExifOriginal),
        (&item.exif_created, DateSource::ExifCreateDate),
        (&item.xmp_metadata, DateSource::XmpMetadata),
    ];
    
    for (date_opt, source) in candidates {
        if let Some(date_str) = date_opt {
            if let Some(parsed) = parse_date(date_str, config) {
                return FileDateInfo {
                    filename,
                    path,
                    best_date: Some(parsed),
                    date_source: source,
                    all_dates,
                };
            }
        }
    }
    
    FileDateInfo {
        filename,
        path,
        best_date: None,
        date_source: DateSource::None,
        all_dates,
    }
}

/// Parse a date string and validate year range
fn parse_date(date_str: &str, config: &DateAnalysisConfig) -> Option<NaiveDateTime> {
    if date_str.is_empty() || date_str == "-" || date_str.starts_with("0000") {
        return None;
    }
    
    // Remove timezone suffix
    let clean = date_str.split('+').next().unwrap_or(date_str);
    let clean = clean.replace('T', " ");
    
    // Try common formats
    let formats = [
        "%Y:%m:%d %H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y:%m:%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
    ];
    
    for fmt in formats {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&clean, fmt) {
            let year = dt.year();
            if year >= config.min_valid_year && year <= config.max_valid_year {
                return Some(dt);
            }
        }
    }
    
    None
}

/// Print analysis results in a formatted way
pub fn print_analysis(result: &DateAnalysisResult) {
    println!("\n📊 Deep Analysis Results");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    println!("\n📈 Statistics:");
    println!("   Total files:           {}", result.total_files);
    println!("   With reliable dates:   {}", result.files_with_dates);
    println!("   Without dates:         {}", result.files_without_dates);
    
    println!("\n📋 Date Source Distribution:");
    let mut sources: Vec<_> = result.by_source.iter().collect();
    sources.sort_by(|a, b| b.1.cmp(a.1));
    for (source, count) in sources {
        println!("   {}: {} files", source, count);
    }
    
    if let Some(earliest) = &result.earliest {
        println!("\n📅 TRUE Date Range (Original Creation Time):");
        if let Some(date) = &earliest.best_date {
            println!("   Earliest: {}", date.format("%Y-%m-%d %H:%M:%S"));
            println!("   File:     {}", earliest.filename);
            println!("   Source:   {}", earliest.date_source.name());
        }
    }
    
    if let Some(latest) = &result.latest {
        if let Some(date) = &latest.best_date {
            println!();
            println!("   Latest:   {}", date.format("%Y-%m-%d %H:%M:%S"));
            println!("   File:     {}", latest.filename);
            println!("   Source:   {}", latest.date_source.name());
        }
    }
    
    if !result.by_year.is_empty() {
        println!("\n📆 Distribution by Year:");
        let mut years: Vec<_> = result.by_year.iter().collect();
        years.sort_by_key(|&(y, _)| y);
        let total = result.files_with_dates as f64;
        for (year, count) in years {
            let pct = (*count as f64 / total * 100.0) as usize;
            let bar: String = "█".repeat(pct / 3 + 1);
            println!("   {}: {:4} files ({:2}%) {}", year, count, pct, bar);
        }
    }
    
    if !result.by_month.is_empty() {
        println!("\n📆 Distribution by Month (Top 15):");
        let mut months: Vec<_> = result.by_month.iter().collect();
        months.sort_by(|a, b| b.1.cmp(a.1));
        for (month, count) in months.iter().take(15) {
            println!("   {}: {:4} files", month, count);
        }
    }
    
    println!("\n✅ Deep analysis complete!");
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_date() {
        let config = DateAnalysisConfig::default();
        
        // Valid dates
        assert!(parse_date("2023:05:15 10:30:00", &config).is_some());
        assert!(parse_date("2023-05-15 10:30:00", &config).is_some());
        assert!(parse_date("2023-05-15T10:30:00", &config).is_some());
        
        // Invalid dates
        assert!(parse_date("", &config).is_none());
        assert!(parse_date("-", &config).is_none());
        assert!(parse_date("0000:00:00 00:00:00", &config).is_none());
        assert!(parse_date("1800:01:01 00:00:00", &config).is_none()); // Too old
    }
    
    #[test]
    fn test_date_source_priority() {
        assert!(DateSource::XmpPhotoshop.priority() > DateSource::ExifOriginal.priority());
        assert!(DateSource::ExifOriginal.priority() > DateSource::None.priority());
    }
}
