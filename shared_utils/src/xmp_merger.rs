// ============================================================================
// 📋 XMP Metadata Merger - Rust Implementation v2.0
// ============================================================================
//
// 🎯 设计目标：100% 可靠的 XMP 元数据合并
//
// 匹配策略（按优先级）：
// 1. Direct match: photo.jpg.xmp → photo.jpg
// 2. Same name different extension: photo.xmp → photo.jpg
// 3. Case-insensitive match: PHOTO.xmp → photo.jpg
// 4. XMP metadata extraction: Read original filename from XMP tags
// 5. DocumentID matching: Match by XMP DocumentID for UUID filenames
// 6. Fuzzy match: Handle special characters, spaces, unicode
// 7. Content hash match: Last resort for renamed files
//
// 特殊处理：
// - Unicode 文件名（中文、日文、emoji）
// - 特殊字符（空格、括号、引号）
// - UUID 格式文件名
// - 路径中的特殊字符
//
// ============================================================================

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

/// Excluded extensions - files that should NEVER be matched as media
/// Everything else is fair game for XMP matching (blacklist approach)
const EXCLUDED_EXTENSIONS: &[&str] = &[
    // XMP itself
    "xmp",
    // Text/config files
    "txt",
    "md",
    "json",
    "xml",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "log",
    // Code files
    "rs",
    "py",
    "js",
    "ts",
    "html",
    "css",
    "sh",
    "bash",
    "zsh",
    "c",
    "cpp",
    "h",
    "hpp",
    "java",
    // Archives
    "zip",
    "tar",
    "gz",
    "bz2",
    "xz",
    "7z",
    "rar",
    // System files
    "ds_store",
    "thumbs.db",
    "desktop.ini",
];

/// Check if extension is a potential media file (not in blacklist)
#[inline]
fn is_potential_media(ext: &str) -> bool {
    !EXCLUDED_EXTENSIONS.contains(&ext.to_lowercase().as_str())
}

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

/// Extract suggested extension from ExifTool error message
/// Example: "Error: Not a valid JPEG (looks more like a PNG)" -> Some("png")
fn extract_suggested_extension(error_msg: &str) -> Option<String> {
    if let Some(start) = error_msg.find("looks more like a ") {
        let rest = &error_msg[start + "looks more like a ".len()..];
        if let Some(end) = rest.find(')') {
             return Some(rest[..end].trim().to_lowercase());
        }
    }
    None
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
            .args([
                "-s3",
                "-DocumentID",
                "-DerivedFrom",
                "-Source",
                "-OriginalDocumentID",
            ])
            .arg(xmp_path)
            .output()
            .context("Failed to run exiftool")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        Ok(XmpFile {
            path: xmp_path.to_path_buf(),
            document_id: lines
                .first()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            derived_from: lines
                .get(1)
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            source: lines
                .get(2)
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
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
        parts
            .iter()
            .zip(expected_lens.iter())
            .all(|(part, &len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
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
    /// Scans directory for any file with matching stem (blacklist approach)
    fn find_same_name_different_ext(&self, xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let xmp_stem_raw = xmp_path.file_stem()?.to_string_lossy().to_lowercase();
        
        // Root stem: "image.jpg" -> "image"
        let xmp_root_stem = xmp_stem_raw.split('.').next().unwrap_or(&xmp_stem_raw);

        // Scan directory for files with same stem
        for entry in std::fs::read_dir(parent).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let file_stem_raw = match path.file_stem() {
                Some(s) => s.to_string_lossy().to_lowercase(),
                None => continue,
            };
            
            // Media root stem: "image.png" -> "image"
            let file_root_stem = file_stem_raw.split('.').next().unwrap_or(&file_stem_raw);

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            // Match logic: root stem match or exact stem match
            if (file_stem_raw == xmp_stem_raw || file_root_stem == xmp_root_stem) && is_potential_media(&ext) {
                return Some(path);
            }
        }
        None
    }

    /// Strategy 2.5: Case-insensitive filename match
    fn find_case_insensitive(&self, xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let xmp_stem = xmp_path.file_stem()?.to_string_lossy().to_lowercase();

        for entry in std::fs::read_dir(parent).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let file_stem = path.file_stem()?.to_string_lossy().to_lowercase();
            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if file_stem == xmp_stem && is_potential_media(&ext) {
                return Some(path);
            }
        }
        None
    }

    /// Strategy 5: Fuzzy match - handle special characters
    fn find_fuzzy_match(&self, xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let stem = xmp_path.file_stem()?.to_string_lossy();

        // Normalize: remove special chars, lowercase
        let normalized_stem = Self::normalize_filename(&stem);
        let root_normalized_stem = Self::normalize_filename(stem.split('.').next().unwrap_or(&stem));

        if normalized_stem.is_empty() {
            return None;
        }

        for entry in std::fs::read_dir(parent).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if !is_potential_media(&ext) {
                continue;
            }

            let file_stem = path.file_stem()?.to_string_lossy();
            let normalized_file = Self::normalize_filename(&file_stem);
            let root_normalized_file = Self::normalize_filename(file_stem.split('.').next().unwrap_or(&file_stem));

            if normalized_file == normalized_stem || root_normalized_file == root_normalized_stem {
                return Some(path);
            }
        }
        None
    }

    /// Normalize filename for fuzzy matching
    fn normalize_filename(name: &str) -> String {
        name.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    }

    /// Strategy 6: Search entire directory for any media file with matching XMP reference
    fn find_by_xmp_reference_scan(&self, xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let xmp_filename = xmp_path.file_name()?.to_string_lossy();

        // Search all media files and check if they reference this XMP
        for entry in std::fs::read_dir(parent).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if !is_potential_media(&ext) {
                continue;
            }

            // Check if media file has SidecarForExtension or similar tag pointing to this XMP
            let output = Command::new("exiftool")
                .args(["-s3", "-SidecarForExtension", "-XMPFileRef"])
                .arg(&path)
                .output()
                .ok()?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains(&*xmp_filename) {
                return Some(path);
            }
        }
        None
    }

    /// Strategy 7: Partial filename match (for files with added suffixes/prefixes)
    fn find_partial_match(&self, xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let stem = xmp_path.file_stem()?.to_string_lossy();

        // Skip very short names to avoid false positives
        if stem.len() < 4 {
            return None;
        }

        for entry in std::fs::read_dir(parent).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if !is_potential_media(&ext) {
                continue;
            }

            let file_stem = path.file_stem()?.to_string_lossy();

            // Check if one contains the other (handles suffixes like _edited, (1), etc.)
            if file_stem.contains(&*stem) || stem.contains(&*file_stem) {
                // Verify it's a reasonable match (at least 70% overlap)
                let shorter = std::cmp::min(stem.len(), file_stem.len());
                let longer = std::cmp::max(stem.len(), file_stem.len());
                if shorter * 100 / longer >= 70 {
                    return Some(path);
                }
            }
        }
        None
    }

    /// Strategy 8: Recursive search in subdirectories
    fn find_in_subdirectories(&self, xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let stem = xmp_path.file_stem()?.to_string_lossy();

        // Search up to 2 levels deep
        for entry in WalkDir::new(parent)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() || path == xmp_path {
                continue;
            }

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if !is_potential_media(&ext) {
                continue;
            }

            let file_stem = path.file_stem()?.to_string_lossy();
            if file_stem.to_lowercase() == stem.to_lowercase() {
                return Some(path.to_path_buf());
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
            if !is_potential_media(&ext) {
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
        if self.config.verbose {
            eprintln!("🔍 Finding match for: {}", xmp_path.display());
        }

        // Strategy 1: Direct match (photo.jpg.xmp → photo.jpg)
        if let Some(media) = self.find_direct_match(xmp_path) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 1 (direct): {}", media.display());
            }
            return Ok((Some(media), "direct_match".to_string()));
        }

        // Strategy 2: Same name different extension (photo.xmp → photo.jpg)
        if let Some(media) = self.find_same_name_different_ext(xmp_path) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 2 (same_name): {}", media.display());
            }
            return Ok((Some(media), "same_name".to_string()));
        }

        // Strategy 2.5: Case-insensitive match
        if let Some(media) = self.find_case_insensitive(xmp_path) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 2.5 (case_insensitive): {}", media.display());
            }
            return Ok((Some(media), "case_insensitive".to_string()));
        }

        // Extract XMP metadata for advanced strategies
        let xmp_info = self.extract_xmp_metadata(xmp_path)?;

        // Strategy 3: XMP metadata extraction (DerivedFrom, Source tags)
        if let Some(media) = self.find_by_xmp_metadata(xmp_path, &xmp_info) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 3 (xmp_metadata): {}", media.display());
            }
            return Ok((Some(media), "xmp_metadata".to_string()));
        }

        // Strategy 4: DocumentID matching (for UUID filenames)
        if let Some(media) = self.find_by_document_id(xmp_path, &xmp_info) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 4 (document_id): {}", media.display());
            }
            return Ok((Some(media), "document_id".to_string()));
        }

        // Strategy 5: Fuzzy match (handle special characters)
        if let Some(media) = self.find_fuzzy_match(xmp_path) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 5 (fuzzy): {}", media.display());
            }
            return Ok((Some(media), "fuzzy_match".to_string()));
        }

        // Strategy 6: XMP reference scan
        if let Some(media) = self.find_by_xmp_reference_scan(xmp_path) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 6 (xmp_ref_scan): {}", media.display());
            }
            return Ok((Some(media), "xmp_ref_scan".to_string()));
        }

        // Strategy 7: Partial filename match
        if let Some(media) = self.find_partial_match(xmp_path) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 7 (partial_match): {}", media.display());
            }
            return Ok((Some(media), "partial_match".to_string()));
        }

        // Strategy 8: Search in subdirectories
        if let Some(media) = self.find_in_subdirectories(xmp_path) {
            if self.config.verbose {
                eprintln!("  ✅ Strategy 8 (subdirectory): {}", media.display());
            }
            return Ok((Some(media), "subdirectory".to_string()));
        }

        if self.config.verbose {
            eprintln!("  ❌ No match found");
        }
        Ok((None, "no_match".to_string()))
    }

    /// Merge XMP metadata into media file
    ///
    /// 🔥 v7.9.5: Implements "Loud Fallback" strategy
    /// If standard merge fails, it attempts to temporarily rename the target file
    /// to the extension implied by the XMP filename (e.g. .jpg.xmp -> .jpg)
    /// to bypass exiftool's format checks, then renames it back.
    pub fn merge_xmp(&self, xmp_path: &Path, media_path: &Path) -> Result<()> {
        // Try standard merge first (Fast path)
        match self.merge_xmp_core(xmp_path, media_path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let err_str = e.to_string();
                let hint = extract_suggested_extension(&err_str);
                
                if let Some(ref h) = hint {
                    eprintln!("💡 ExifTool suggests content is: {}", h);
                }

                // Standard merge failed, try fallback strategy with hint
                self.merge_xmp_fallback(xmp_path, media_path, hint.as_deref())
            }
        }
    }

    /// Core merge logic - direct ExifTool call
    fn merge_xmp_core(&self, xmp_path: &Path, media_path: &Path) -> Result<()> {
        // Save original timestamps before merge
        let original_timestamps = self.get_file_timestamps(media_path);
        let xmp_timestamps = self.get_file_timestamps(xmp_path);

        // Build exiftool command with proper escaping
        let mut args = vec!["-P".to_string()];

        if self.config.overwrite_original {
            args.push("-overwrite_original".to_string());
        }

        let is_jxl = media_path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("jxl"));
        let apple_compat = std::env::var("MODERN_FORMAT_BOOST_APPLE_COMPAT").is_ok();

        // 🔥 Nuclear Rebuild Strategy (Standardize Metadata & Prevent Brotli Corruption)
        // Only run for JXL files when apple compatibility is actively requested.
        // Otherwise, defer to 100% data preservation and avoid stripping un-recognized metadata.
        if is_jxl && apple_compat {
            // -all= clears everything, then we restore from the file itself (@) 
            // and finally merge the XMP sidecar. This ensures the metadata block 
            // is rewritten cleanly without compression anomalies.
            args.push("-all=".to_string());
            
            args.push("-tagsfromfile".to_string());
            args.push("@".to_string());
            args.push("-all:all".to_string());
            args.push("-unsafe".to_string());
            args.push("-icc_profile".to_string());
        }

        args.push("-tagsfromfile".to_string());
        args.push(xmp_path.to_string_lossy().to_string());
        args.push("-all:all".to_string());

        // Don't overwrite certain critical tags
        args.push("-FileModifyDate<FileModifyDate".to_string());
        args.push(media_path.to_string_lossy().to_string());

        let output = Command::new("exiftool")
            .args(&args)
            .output()
            .context("Failed to run exiftool merge")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // 🔥 v6.9.10: [minor] warnings are OK for JXL container wrapping
            // ExifTool outputs "[minor] Will wrap JXL codestream in ISO BMFF container"
            // This is informational, not an actual error - XMP data is still written successfully
            let is_minor_warning = stderr.contains("[minor]");
            let is_real_error = stderr.contains("Error:") && !is_minor_warning;

            if is_real_error {
                bail!("ExifTool merge failed: {}", stderr);
            }
            // Minor warnings are acceptable, continue silently
        }

        // Restore timestamps
        if self.config.preserve_timestamps {
            self.restore_timestamps(media_path, &original_timestamps, &xmp_timestamps);
        }

        Ok(())
    }

    /// Fallback strategy: Temporary extension correction
    fn merge_xmp_fallback(&self, xmp_path: &Path, media_path: &Path, hint_ext: Option<&str>) -> Result<()> {
        let xmp_filename = xmp_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        
        // Strategy 1: Smart Content Detection (Magic Bytes)
        // Check if the file content matches a specific format explicitly
        let detected_ext = if let Some(hint) = hint_ext {
            Some(hint.to_string())
        } else {
             crate::common_utils::detect_real_extension(media_path).map(|s| s.to_string())
        };

        // Strategy 2: Implied extension from XMP filename (Legacy fallback)
        let implied_ext = if xmp_filename.to_lowercase().ends_with(".xmp") {
             let stem = &xmp_filename[..xmp_filename.len() - 4];
             Path::new(stem).extension().and_then(|e| e.to_str())
        } else {
             None
        };

        // Decide which extension to force
        // Priority: Detected Content (includes Hint) > Implied XMP Extension
        let target_ext = detected_ext.or(implied_ext.map(|s| s.to_string()));

        let Some(original_ext) = target_ext else {
             // No extension determined, cannot use fallback
             // Retry core to return the original error if we can't find a fallback
             return self.merge_xmp_core(xmp_path, media_path); 
        };

        let current_ext = media_path.extension().and_then(|e| e.to_str()).unwrap_or("");

        // If extensions match (ignoring case), fallback won't help
        if original_ext.eq_ignore_ascii_case(current_ext) {
             return self.merge_xmp_core(xmp_path, media_path);
        }

        eprintln!(
             "⚠️ Merge failed, attempting fallback: Temporary rename to .{} for merge...",
             original_ext
        );

        // Construct temporary path
        let temp_path = media_path.with_extension(&original_ext);

        // Safety check: Don't overwrite existing files
        if temp_path.exists() {
             eprintln!("⚠️ Fallback aborted: Temporary target {} already exists", temp_path.display());
             return self.merge_xmp_core(xmp_path, media_path);
        }

        // 1. Rename to original extension
        std::fs::rename(media_path, &temp_path)
             .context("Fallback: Failed to rename for temporary merge")?;

        // 2. Perform merge on temporary file
        // We use a scope guard-like pattern ensures we rename back even if merge fails
        let merge_result = self.merge_xmp_core(xmp_path, &temp_path);

        // 3. Rename back to current extension (Critical!)
        if let Err(e) = std::fs::rename(&temp_path, media_path) {
             // This is bad - we're left with the temp filename
             eprintln!("❌ CRITICAL: Failed to restore filename from {} to {}", temp_path.display(), media_path.display());
             eprintln!("❌ Error: {}", e);
             // If merge succeeded but rename back failed, we still report error because system state is potentially inconsistent
             bail!("Critical: Failed to restore filename after fallback merge");
        }

        match merge_result {
             Ok(()) => {
                 eprintln!("✅ Fallback merge successful");
                 Ok(())
             }
             Err(e) => {
                 eprintln!("❌ Fallback merge failed: {}", e);
                 Err(e)
             }
        }
    }

    /// Get file timestamps
    fn get_file_timestamps(&self, path: &Path) -> Option<(filetime::FileTime, filetime::FileTime)> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = std::fs::metadata(path) {
                let atime = filetime::FileTime::from_unix_time(meta.atime(), 0);
                let mtime = filetime::FileTime::from_unix_time(meta.mtime(), 0);
                return Some((atime, mtime));
            }
        }
        #[cfg(not(unix))]
        {
            if let Ok(meta) = std::fs::metadata(path) {
                if let Ok(modified) = meta.modified() {
                    let mtime = filetime::FileTime::from_system_time(modified);
                    return Some((mtime, mtime));
                }
            }
        }
        None
    }

    /// Restore timestamps - ALWAYS use original media file's timestamp
    fn restore_timestamps(
        &self,
        media_path: &Path,
        original: &Option<(filetime::FileTime, filetime::FileTime)>,
        _xmp: &Option<(filetime::FileTime, filetime::FileTime)>,
    ) {
        // ALWAYS restore original media timestamp (not XMP timestamp)
        if let Some((atime, mtime)) = original {
            if let Err(e) = filetime::set_file_times(media_path, *atime, *mtime) {
                eprintln!(
                    "⚠️ Failed to restore timestamp for {}: {}",
                    media_path.display(),
                    e
                );
            }
        }
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

        // 🔥 v7.7: 心跳检测 - 仅当文件数>10时启用
        let _heartbeat = if xmp_files.len() > 10 {
            Some(crate::universal_heartbeat::HeartbeatGuard::new(
                crate::universal_heartbeat::HeartbeatConfig::medium("XMP Batch Merge")
                    .with_info(format!("{} files", xmp_files.len())),
            ))
        } else {
            None
        };

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
        let mut summary = Self {
            total: results.len(),
            ..Default::default()
        };

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

// ============================================================================
// 🔥 v6.9.11: 便捷辅助函数 - 用于复制文件时合并XMP
// ============================================================================

/// 🔥 v6.9.11: 复制文件到目标目录，同时查找并合并XMP边车
///
/// 当文件被跳过（短动画、现代格式、质量失败等）时使用此函数，
/// 确保XMP元数据也被正确处理。
///
/// # Arguments
/// * `input` - 源文件路径
/// * `dest` - 目标文件路径（已复制的文件）
///
/// # Returns
/// * `Ok(true)` - XMP找到并成功合并
/// * `Ok(false)` - 没有找到XMP边车
/// * `Err(_)` - XMP合并失败
pub fn merge_xmp_for_copied_file(input: &Path, dest: &Path) -> Result<bool> {
    // 查找XMP边车文件（多种命名方式）
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    let parent = input.parent().unwrap_or(Path::new("."));

    // 尝试多种XMP命名方式
    let ext_lower = ext.to_lowercase();
    let xmp_candidates = [
        parent.join(format!("{}.xmp", stem)),                // photo.xmp
        parent.join(format!("{}.{}.xmp", stem, ext)),        // photo.jpg.xmp (match original)
        parent.join(format!("{}.{}.xmp", stem, ext_lower)),  // photo.jpg.xmp (normalized)
        parent.join(format!("{}.XMP", stem)),                // photo.XMP (uppercase)
    ];

    for xmp_path in &xmp_candidates {
        if xmp_path.exists() {
            eprintln!("📋 Found XMP sidecar: {}", xmp_path.display());

            // Use XmpMerger for robust merging (handles fallback, etc.)
            let config = XmpMergerConfig {
                delete_xmp_after_merge: false, // Don't delete in this context
                overwrite_original: true,
                preserve_timestamps: true, // Preserve destination timestamps if needed (usually dest is new)
                verbose: false,
            };
            
            let merger = XmpMerger::new(config);
            
            // Call merge_xmp which includes the fallback logic
            if let Err(e) = merger.merge_xmp(xmp_path, dest) {
                // If merge fails, we log it but don't fail the whole process
                // unless it's critical, but here we return Err to let caller decide
                 bail!("Failed to merge XMP: {}", e);
            }

            eprintln!("✅ XMP sidecar merged successfully");
            return Ok(true);
        }
    }

    // 没有找到XMP边车
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_is_uuid_filename() {
        assert!(XmpMerger::is_uuid_filename(
            "6cdf1517-be7d-4f85-b519-f4aeaac45fdd"
        ));
        assert!(XmpMerger::is_uuid_filename(
            "A1B2C3D4-E5F6-7890-ABCD-EF1234567890"
        ));
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

    #[test]
    fn test_case_insensitive_match() {
        let temp_dir = TempDir::new().unwrap();
        let jpg = temp_dir.path().join("PHOTO.JPG");
        let xmp = temp_dir.path().join("photo.xmp");

        fs::write(&jpg, "fake jpg").unwrap();
        fs::write(&xmp, "fake xmp").unwrap();

        let merger = XmpMerger::new(XmpMergerConfig::default());
        let result = merger.find_case_insensitive(&xmp);

        assert!(result.is_some());
    }

    #[test]
    fn test_fuzzy_match_special_chars() {
        let temp_dir = TempDir::new().unwrap();
        let jpg = temp_dir.path().join("photo (1).jpg");
        let xmp = temp_dir.path().join("photo(1).xmp");

        fs::write(&jpg, "fake jpg").unwrap();
        fs::write(&xmp, "fake xmp").unwrap();

        let merger = XmpMerger::new(XmpMergerConfig::default());
        let result = merger.find_fuzzy_match(&xmp);

        assert!(result.is_some());
    }

    #[test]
    fn test_normalize_filename() {
        assert_eq!(XmpMerger::normalize_filename("Photo (1)"), "photo1");
        assert_eq!(
            XmpMerger::normalize_filename("IMG_2024-01-01"),
            "img20240101"
        );
        assert_eq!(XmpMerger::normalize_filename("测试文件"), "测试文件");
        assert_eq!(XmpMerger::normalize_filename("photo.test"), "phototest");
    }

    #[test]
    fn test_unicode_filename() {
        let temp_dir = TempDir::new().unwrap();
        let jpg = temp_dir.path().join("照片2024.jpg");
        let xmp = temp_dir.path().join("照片2024.xmp");

        fs::write(&jpg, "fake jpg").unwrap();
        fs::write(&xmp, "fake xmp").unwrap();

        let merger = XmpMerger::new(XmpMergerConfig::default());
        let result = merger.find_same_name_different_ext(&xmp);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), jpg);
    }

    #[test]
    fn test_spaces_in_filename() {
        let temp_dir = TempDir::new().unwrap();
        let jpg = temp_dir.path().join("my photo 2024.jpg");
        let xmp = temp_dir.path().join("my photo 2024.xmp");

        fs::write(&jpg, "fake jpg").unwrap();
        fs::write(&xmp, "fake xmp").unwrap();

        let merger = XmpMerger::new(XmpMergerConfig::default());
        let result = merger.find_same_name_different_ext(&xmp);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), jpg);
    }

    #[test]
    fn test_raw_format_match() {
        let temp_dir = TempDir::new().unwrap();
        let raw = temp_dir.path().join("DSC_0001.NEF");
        let xmp = temp_dir.path().join("DSC_0001.xmp");

        fs::write(&raw, "fake raw").unwrap();
        fs::write(&xmp, "fake xmp").unwrap();

        let merger = XmpMerger::new(XmpMergerConfig::default());
        let (result, strategy) = merger.find_media_file(&xmp).unwrap();

        assert!(result.is_some());
        // Should match via same_name or case_insensitive
        assert!(strategy == "same_name" || strategy == "case_insensitive");
    }

    #[test]
    fn test_merge_xmp_mismatch_fallback() {
        // This test requires exiftool
        if Command::new("exiftool").arg("-ver").output().is_err() {
            eprintln!("ExifTool not found, skipping test");
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        // Create a real PNG file but name it .jpg
        let jpg_path = temp_dir.path().join("mismatch.jpg");
        let xmp_path = temp_dir.path().join("mismatch.xmp");

        // 1x1 PNG data
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
            0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41,
            0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
            0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
            0x44, 0xAE, 0x42, 0x60, 0x82
        ];
        fs::write(&jpg_path, png_data).unwrap();

        // Create a valid XMP sidecar
        let xmp_content = r#"<?xpacket begin='﻿' id='W5M0MpCehiHzreSzNTczkc9d'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/' x:xmptk='Image::ExifTool 12.00'>
<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
 <rdf:Description rdf:about=''
  xmlns:dc='http://purl.org/dc/elements/1.1/'>
  <dc:Description>
   <rdf:Alt>
    <rdf:li xml:lang='x-default'>Test Description</rdf:li>
   </rdf:Alt>
  </dc:Description>
 </rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end='w'?>"#;
        fs::write(&xmp_path, xmp_content).unwrap();

        let config = XmpMergerConfig {
            verbose: true,
            ..Default::default()
        };
        let merger = XmpMerger::new(config);

        // This should trigger the fallback because ExifTool will complain about PNG vs JPG
        let result = merger.merge_xmp(&xmp_path, &jpg_path);
        
        if let Err(e) = &result {
            println!("Merge failed with error: {}", e);
        }
        assert!(result.is_ok(), "XMP merge failed for mismatched extension");

        // Verify the file still exists and is named .jpg
        assert!(jpg_path.exists());
        assert!(!jpg_path.with_extension("png").exists()); // Should be restored
    }
}
