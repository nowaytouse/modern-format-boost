// ============================================================================
// 📋 XMP Metadata Merger - Rust Implementation
// ============================================================================
//
// Reliable XMP sidecar file merger with multiple matching strategies:
// 1. Direct match: photo.jpg.xmp → photo.jpg
// 2. Same name different extension: photo.xmp → photo.jpg
// 3. XMP metadata extraction: Read original filename from XMP
// 4. DocumentID matching: Match by XMP DocumentID for UUID filenames
//
// ============================================================================

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

/// Supported media file extensions
const MEDIA_EXTENSIONS: &[&str] = &[
    // Images
    "jpg", "jpeg", "png", "tiff", "tif", "webp", "avif", "jxl", "heic", "heif", "bmp", "gif",
    // RAW formats
    "cr2", "cr3", "nef", "arw", "dng", "raf", "orf", "rw2", "pef", "srw", "raw",
    // Video
    "mp4", "mov", "avi", "mkv", "m4v", "webm", "mts", "m2ts",
];

/// XMP file information
#[derive(Debug, Clone)]
pub struct XmpFile {
    pub path: PathBuf,
    pub document_id: Option<String>,
    pub derived_from: Option<String>,
    pub source: Option<String>,
}

/// Merge result for a single XMP file
#[derive(Debug)]
pub struct MergeResult {
    pub xmp_path: PathBuf,
    pub media_path: Option<PathBuf>,
    pub success: bool,
    pub message: String,
    pub match_strategy: Option<String>,
}

/// XMP Merger configuration
#[derive(Debug, Clone)]
pub struct XmpMergerConfig {
    pub delete_xmp_after_merge: bool,
    pub overwrite_original: bool,
    pub preserve_timestamps: bool,
    pub verbose: bool,
}

impl Default for XmpMergerConfig {
    fn default() -> Self {
        Self {
            delete_xmp_after_merge: false,
            overwrite_original: true,
            preserve_timestamps: true,
            verbose: false,
        }
    }
}

/// XMP Metadata Merger
pub struct XmpMerger {
    config: XmpMergerConfig,
}

impl XmpMerger {
    pub fn new(config: XmpMergerConfig) -> Self {
        Self { config }
    }

    /// Check if exiftool is available
    pub fn check_exiftool() -> Result<()> {
        let output = Command::new("exiftool")
            .arg("-ver")
            .output()
            .context("ExifTool not found. Install with: brew install exiftool")?;
        
        if !output.status.success() {
            bail!("ExifTool check failed");
        }
        Ok(())
    }

    /// Find all XMP files in directory
    pub fn find_xmp_files(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let mut xmp_files = Vec::new();
        
        for entry in WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext.to_string_lossy().to_lowercase() == "xmp" {
                        xmp_files.push(path.to_path_buf());
                    }
                }
            }
        }
        
        Ok(xmp_files)
    }

    /// Extract metadata from XMP file using exiftool
    fn extract_xmp_metadata(&self, xmp_path: &Path) -> Result<XmpFile> {
        let output = Command::new("exiftool")
            .args(["-s3", "-DocumentID", "-DerivedFrom", "-Source", "-OriginalDocumentID"])
            .arg(xmp_path)
            .output()
            .context("Failed to run exiftool")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        Ok(XmpFile {
            path: xmp_path.to_path_buf(),
            document_id: lines.first().map(|s| s.to_string()).filter(|s| !s.is_empty()),
            derived_from: lines.get(1).map(|s| s.to_string()).filter(|s| !s.is_empty()),
            source: lines.get(2).map(|s| s.to_string()).filter(|s| !s.is_empty()),
        })
    }

    /// Check if filename looks like a UUID
    fn is_uuid_filename(name: &str) -> bool {
        // UUID format: 8-4-4-4-12 hex characters
        let parts: Vec<&str> = name.split('-').collect();
        if parts.len() != 5 {
            return false;
        }
        let expected_lens = [8, 4, 4, 4, 12];
        parts.iter().zip(expected_lens.iter()).all(|(part, &len)| {
            part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit())
        })
    }

    /// Strategy 1: Direct match (photo.jpg.xmp → photo.jpg)
    fn find_direct_match(&self, xmp_path: &Path) -> Option<PathBuf> {
        let xmp_str = xmp_path.to_string_lossy();
        if xmp_str.to_lowercase().ends_with(".xmp") {
            let base = &xmp_str[..xmp_str.len() - 4];
            let base_path = PathBuf::from(base);
            if base_path.exists() && base_path.is_file() {
                return Some(base_path);
            }
        }
        None
    }

    /// Strategy 2: Same name different extension (photo.xmp → photo.jpg)
    fn find_same_name_different_ext(&self, xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let stem = xmp_path.file_stem()?.to_string_lossy();
        
        for ext in MEDIA_EXTENSIONS {
            let candidate = parent.join(format!("{}.{}", stem, ext));
            if candidate.exists() {
                return Some(candidate);
            }
            // Also try uppercase extension
            let candidate_upper = parent.join(format!("{}.{}", stem, ext.to_uppercase()));
            if candidate_upper.exists() {
                return Some(candidate_upper);
            }
        }
        None
    }

    /// Strategy 3: Extract original filename from XMP metadata
    fn find_by_xmp_metadata(&self, xmp_path: &Path, xmp_info: &XmpFile) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        
        // Try DerivedFrom field
        if let Some(ref derived) = xmp_info.derived_from {
            if !derived.contains("uuid:") {
                let candidate = parent.join(derived);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        
        // Try Source field
        if let Some(ref source) = xmp_info.source {
            let candidate = parent.join(source);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        
        None
    }

    /// Strategy 4: Match by DocumentID for UUID filenames
    fn find_by_document_id(&self, xmp_path: &Path, xmp_info: &XmpFile) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let xmp_doc_id = xmp_info.document_id.as_ref()?;
        
        // Only use this strategy for UUID-like filenames
        let stem = xmp_path.file_stem()?.to_string_lossy();
        if !Self::is_uuid_filename(&stem) {
            return None;
        }

        if self.config.verbose {
            eprintln!("  🔍 Searching by DocumentID: {}", xmp_doc_id);
        }

        // Search for media files with matching DocumentID
        for entry in std::fs::read_dir(parent).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            
            if !path.is_file() {
                continue;
            }
            
            let ext = path.extension()?.to_string_lossy().to_lowercase();
            if ext == "xmp" || !MEDIA_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }

            // Get DocumentID from media file
            let output = Command::new("exiftool")
                .args(["-s3", "-DocumentID"])
                .arg(&path)
                .output()
                .ok()?;

            let media_doc_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            
            if !media_doc_id.is_empty() && media_doc_id == *xmp_doc_id {
                if self.config.verbose {
                    eprintln!("  ✅ Found match: {}", path.display());
                }
                return Some(path);
            }
        }
        
        None
    }

    /// Find matching media file for XMP using all strategies
    pub fn find_media_file(&self, xmp_path: &Path) -> Result<(Option<PathBuf>, String)> {
        // Strategy 1: Direct match
        if let Some(media) = self.find_direct_match(xmp_path) {
            return Ok((Some(media), "direct_match".to_string()));
        }

        // Strategy 2: Same name different extension
        if let Some(media) = self.find_same_name_different_ext(xmp_path) {
            return Ok((Some(media), "same_name".to_string()));
        }

        // Extract XMP metadata for advanced strategies
        let xmp_info = self.extract_xmp_metadata(xmp_path)?;

        // Strategy 3: XMP metadata extraction
        if let Some(media) = self.find_by_xmp_metadata(xmp_path, &xmp_info) {
            return Ok((Some(media), "xmp_metadata".to_string()));
        }

        // Strategy 4: DocumentID matching (for UUID filenames)
        if let Some(media) = self.find_by_document_id(xmp_path, &xmp_info) {
            return Ok((Some(media), "document_id".to_string()));
        }

        Ok((None, "no_match".to_string()))
    }

    /// Merge XMP metadata into media file
    pub fn merge_xmp(&self, xmp_path: &Path, media_path: &Path) -> Result<()> {
        let mut args = vec!["-P".to_string()];
        
        if self.config.overwrite_original {
            args.push("-overwrite_original".to_string());
        }
        
        args.push("-tagsfromfile".to_string());
        args.push(xmp_path.to_string_lossy().to_string());
        args.push("-all:all".to_string());
        args.push(media_path.to_string_lossy().to_string());

        let output = Command::new("exiftool")
            .args(&args)
            .output()
            .context("Failed to run exiftool merge")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("ExifTool merge failed: {}", stderr);
        }

        // Preserve timestamps
        if self.config.preserve_timestamps {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if let Ok(xmp_meta) = std::fs::metadata(xmp_path) {
                    let atime = filetime::FileTime::from_unix_time(xmp_meta.atime(), 0);
                    let mtime = filetime::FileTime::from_unix_time(xmp_meta.mtime(), 0);
                    let _ = filetime::set_file_times(media_path, atime, mtime);
                }
            }
        }

        Ok(())
    }

    /// Process a single XMP file
    pub fn process_xmp(&self, xmp_path: &Path) -> MergeResult {
        let (media_path, strategy) = match self.find_media_file(xmp_path) {
            Ok((path, strat)) => (path, strat),
            Err(e) => {
                return MergeResult {
                    xmp_path: xmp_path.to_path_buf(),
                    media_path: None,
                    success: false,
                    message: format!("Error finding media: {}", e),
                    match_strategy: None,
                };
            }
        };

        let Some(media) = media_path else {
            return MergeResult {
                xmp_path: xmp_path.to_path_buf(),
                media_path: None,
                success: false,
                message: "No matching media file found".to_string(),
                match_strategy: Some(strategy),
            };
        };

        match self.merge_xmp(xmp_path, &media) {
            Ok(()) => {
                // Delete XMP if configured
                if self.config.delete_xmp_after_merge {
                    let _ = std::fs::remove_file(xmp_path);
                }
                
                MergeResult {
                    xmp_path: xmp_path.to_path_buf(),
                    media_path: Some(media),
                    success: true,
                    message: "Merged successfully".to_string(),
                    match_strategy: Some(strategy),
                }
            }
            Err(e) => MergeResult {
                xmp_path: xmp_path.to_path_buf(),
                media_path: Some(media),
                success: false,
                message: format!("Merge failed: {}", e),
                match_strategy: Some(strategy),
            },
        }
    }

    /// Process all XMP files in directory
    pub fn process_directory(&self, dir: &Path) -> Result<Vec<MergeResult>> {
        Self::check_exiftool()?;
        
        let xmp_files = self.find_xmp_files(dir)?;
        let mut results = Vec::with_capacity(xmp_files.len());

        for xmp_path in xmp_files {
            let result = self.process_xmp(&xmp_path);
            results.push(result);
        }

        Ok(results)
    }
}

/// Summary statistics for merge operation
#[derive(Debug, Default)]
pub struct MergeSummary {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
    pub strategies: HashMap<String, usize>,
}

impl MergeSummary {
    pub fn from_results(results: &[MergeResult]) -> Self {
        let mut summary = Self::default();
        summary.total = results.len();
        
        for result in results {
            if result.success {
                summary.success += 1;
            } else if result.media_path.is_none() {
                summary.skipped += 1;
            } else {
                summary.failed += 1;
            }
            
            if let Some(ref strategy) = result.match_strategy {
                *summary.strategies.entry(strategy.clone()).or_insert(0) += 1;
            }
        }
        
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn test_is_uuid_filename() {
        assert!(XmpMerger::is_uuid_filename("6cdf1517-be7d-4f85-b519-f4aeaac45fdd"));
        assert!(XmpMerger::is_uuid_filename("A1B2C3D4-E5F6-7890-ABCD-EF1234567890"));
        assert!(!XmpMerger::is_uuid_filename("photo"));
        assert!(!XmpMerger::is_uuid_filename("photo-2024"));
        assert!(!XmpMerger::is_uuid_filename("123-456-789"));
    }

    #[test]
    fn test_find_xmp_files() {
        let temp_dir = TempDir::new().unwrap();
        let xmp1 = temp_dir.path().join("photo1.xmp");
        let xmp2 = temp_dir.path().join("photo2.jpg.xmp");
        let jpg = temp_dir.path().join("photo1.jpg");
        
        fs::write(&xmp1, "").unwrap();
        fs::write(&xmp2, "").unwrap();
        fs::write(&jpg, "").unwrap();
        
        let merger = XmpMerger::new(XmpMergerConfig::default());
        let xmp_files = merger.find_xmp_files(temp_dir.path()).unwrap();
        
        assert_eq!(xmp_files.len(), 2);
    }

    #[test]
    fn test_direct_match_strategy() {
        let temp_dir = TempDir::new().unwrap();
        let jpg = temp_dir.path().join("photo.jpg");
        let xmp = temp_dir.path().join("photo.jpg.xmp");
        
        fs::write(&jpg, "fake jpg").unwrap();
        fs::write(&xmp, "fake xmp").unwrap();
        
        let merger = XmpMerger::new(XmpMergerConfig::default());
        let result = merger.find_direct_match(&xmp);
        
        assert!(result.is_some());
        assert_eq!(result.unwrap(), jpg);
    }

    #[test]
    fn test_same_name_different_ext_strategy() {
        let temp_dir = TempDir::new().unwrap();
        let jpg = temp_dir.path().join("photo.jpg");
        let xmp = temp_dir.path().join("photo.xmp");
        
        fs::write(&jpg, "fake jpg").unwrap();
        fs::write(&xmp, "fake xmp").unwrap();
        
        let merger = XmpMerger::new(XmpMergerConfig::default());
        let result = merger.find_same_name_different_ext(&xmp);
        
        assert!(result.is_some());
        assert_eq!(result.unwrap(), jpg);
    }
}
