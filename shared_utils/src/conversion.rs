//! Conversion Utilities Module
//!
//! Provides common conversion functionality shared across all tools:
//! - ConversionResult: Unified result structure
//! - ConvertOptions: Common conversion options
//! - Anti-duplicate mechanism: Track processed files
//! - Result builders: Reduce boilerplate code
//! - Size formatting: Unified message formatting
//!
//! ## Atomic output (TOCTOU)
//! All conversion paths **must** write to a temp path via `temp_path_for_output()` then
//! call `commit_temp_to_output(temp, output, force)`. Do not write directly to the final output.
//!
//! ## Compress mode (authoritative)
//! When `options.compress` is true: **only** `output_size < input_size` is accepted.
//! **Any** `output_size >= input_size` (including equal) is rejected — goal not achieved.
//! All size checks use `>=` for this; do not change to `>`.
//!
//! ## allow_size_tolerance (default true)
//! When true: "oversized" threshold is `output size increase < 1_048_576 bytes` (accept). Video path may treat
//! `video_compression_ratio < 1.01` as acceptable when require_compression is checked.
//! Does **not** mean "accept up to 1_048_576 bytes larger as success" for compress goal — compress still requires output < input.

#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use crate::modern_ui::{colors, symbols};
use std::sync::{LazyLock, Mutex};
use rand::Rng;

static PROCESSED_FILES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn is_already_processed(path: &Path) -> bool {
    let canonical = path
        .canonicalize()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.display().to_string());

    let processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    processed.contains(&canonical)
}

pub fn mark_as_processed(path: &Path) {
    let canonical = path
        .canonicalize()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.display().to_string());

    let mut processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    processed.insert(canonical);
}

pub fn clear_processed_list() {
    let mut processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    processed.clear();
}

pub use crate::checkpoint::{
    safe_delete_original, verify_output_integrity,
    MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE, MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO,
};

#[cfg(unix)]
fn flock_exclusive(file: &fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
struct ProcessedListLockGuard(std::os::unix::io::RawFd);

#[cfg(unix)]
impl Drop for ProcessedListLockGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0, libc::LOCK_UN) };
    }
}

pub fn load_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !list_path.exists() {
        return Ok(());
    }

    let file = fs::File::open(list_path)?;
    #[cfg(unix)]
    flock_exclusive(&file)?;
    #[cfg(unix)]
    let _flock_guard = ProcessedListLockGuard(std::os::unix::io::AsRawFd::as_raw_fd(&file));
    let reader = BufReader::new(&file);
    let mut loaded = HashSet::new();

    let mut read_error = None;
    for line in reader.lines() {
        match line {
            Ok(path) => {
                loaded.insert(path);
            }
            Err(err) => {
                if read_error.is_none() {
                    read_error = Some(err);
                }
            }
        }
    }

    if let Some(err) = read_error {
        return Err(Box::new(err));
    }

    let mut processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    processed.extend(loaded);

    Ok(())
}

pub fn save_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = fs::File::create(list_path)?;
    #[cfg(unix)]
    flock_exclusive(&file)?;
    #[cfg(unix)]
    let _flock_guard = ProcessedListLockGuard(std::os::unix::io::AsRawFd::as_raw_fd(&file));

    for path in processed.iter() {
        writeln!(file, "{}", path)?;
    }
    file.flush()?;

    Ok(())
}

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
    pub fn is_jpeg_transcode(&self) -> bool {
        self.message.contains("JPEG transcoding") || self.message.contains("JPEG lossless")
    }

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

    pub fn skipped_custom(input: &Path, input_size: u64, reason: &str, skip_reason: &str) -> Self {
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: reason.to_string(),
            skipped: true,
            skip_reason: Some(skip_reason.to_string()),
        }
    }

    pub fn skipped_size_increase(input: &Path, input_size: u64, output_size: u64) -> Self {
        let diff_bytes = output_size as i64 - input_size as i64;
        let size_diff = crate::modern_ui::format_size_diff(diff_bytes);
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!("Skipped: Output would be larger ({})", size_diff),
            skipped: true,
            skip_reason: Some("size_increase".to_string()),
        }
    }

    /// Used when compress mode is on and output size equals input (goal: must be strictly smaller).
    pub fn skipped_size_unchanged(input: &Path, input_size: u64, format_label: &str) -> Self {
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!(
                "Skipped: {} output size unchanged (compression goal not achieved)",
                format_label
            ),
            skipped: true,
            skip_reason: Some("size_unchanged".to_string()),
        }
    }

    pub fn success(
        input: &Path,
        output: &Path,
        input_size: u64,
        output_size: u64,
        format_name: &str,
        extra_info: Option<&str>,
        quality_label: Option<&str>,
    ) -> Self {
        let reduction = if input_size == 0 {
            0.0
        } else {
            1.0 - (output_size as f64 / input_size as f64)
        };
        let reduction_pct = reduction * 100.0;

        // Build size-change suffix: "-14.5%" (saved) or "+2.1MB" (grew) with ANSI colors
        let size_tag = if reduction >= 0.0 {
            format!("\x1b[1;32m-{:.1}%\x1b[0m", reduction_pct)
        } else {
            let diff_bytes = output_size as i64 - input_size as i64;
            let size_diff = crate::modern_ui::format_size_diff(diff_bytes);
            format!("\x1b[1;33m{}\x1b[0m", size_diff)
        };

        // Message body (no \u2705 here — caller (log_eprintln!) already emits it).
        // Format: "「Quality」 ✅ <FormatName> transcoding: -14.5%"
        let core_msg = match extra_info {
            Some(info) => format!("{} transcoding ({}): {}", format_name, info, size_tag),
            None       => format!("{} transcoding: {}",          format_name,          size_tag),
        };
        
        let message = if let Some(q) = quality_label {
            if q.is_empty() {
                format!("✅ {}", core_msg)
            } else {
                format!("✅ {} | {}", q, core_msg)
            }
        } else {
            format!("✅ {}", core_msg)
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

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub force: bool,
    pub output_dir: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub delete_original: bool,
    pub in_place: bool,
    pub explore: bool,
    pub match_quality: bool,
    pub apple_compat: bool,
    pub compress: bool,
    pub use_gpu: bool,
    pub ultimate: bool,
    pub allow_size_tolerance: bool,
    pub verbose: bool,
    pub child_threads: usize,
    pub input_format: Option<String>,
    pub quality_label: Option<String>,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            force: false,
            output_dir: None,
            base_dir: None,
            delete_original: false,
            in_place: false,
            explore: false,
            match_quality: false,
            apple_compat: false,
            compress: false,
            use_gpu: true,
            ultimate: false,
            allow_size_tolerance: true,
            verbose: false,
            child_threads: 0,
            input_format: None,
            quality_label: None,
        }
    }
}

impl ConvertOptions {
    pub fn should_delete_original(&self) -> bool {
        self.delete_original || self.in_place
    }

    pub fn flag_mode(&self) -> Result<crate::flag_validator::FlagMode, String> {
        crate::flag_validator::validate_flags_result_with_ultimate(
            self.explore,
            self.match_quality,
            self.compress,
            self.ultimate,
        )
    }

    pub fn explore_mode(&self) -> crate::video_explorer::ExploreMode {
        // flag_mode() result is irrelevant — always use PreciseQualityMatchWithCompression
        crate::video_explorer::ExploreMode::PreciseQualityMatchWithCompression
    }
}

pub fn determine_output_path(
    input: &Path,
    extension: &str,
    output_dir: &Option<PathBuf>,
) -> Result<PathBuf, String> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let up_ext = extension.to_uppercase();
    let output = match output_dir {
        Some(dir) => {
            let _ = fs::create_dir_all(dir);
            dir.join(format!("{}.{}", stem, up_ext))
        }
        None => input.with_extension(up_ext),
    };

    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let output_canonical = if output.exists() {
        output.canonicalize().unwrap_or_else(|_| output.clone())
    } else {
        output.clone()
    };

    if input_canonical == output_canonical || input == output {
        return Err(format!(
            "Input and output paths are identical: {}\n\
             Tip: use --output/-o for a different output dir, or --in-place to replace in place (deletes original)",
            input.display()
        ));
    }

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    Ok(output)
}

pub fn determine_output_path_with_base(
    input: &Path,
    base_dir: &Path,
    extension: &str,
    output_dir: &Option<PathBuf>,
) -> Result<PathBuf, String> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let up_ext = extension.to_uppercase();
    let output = match output_dir {
        Some(dir) => {
            let rel_path = input
                .strip_prefix(base_dir)
                .unwrap_or(input)
                .parent()
                .unwrap_or(Path::new(""));

            let out_subdir = dir.join(rel_path);
            let _ = fs::create_dir_all(&out_subdir);

            out_subdir.join(format!("{}.{}", stem, up_ext))
        }
        None => input.with_extension(up_ext),
    };

    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let output_canonical = if output.exists() {
        output.canonicalize().unwrap_or_else(|_| output.clone())
    } else {
        output.clone()
    };

    if input_canonical == output_canonical || input == output {
        return Err(format!(
            "Input and output paths are identical: {}\n\
             Tip: use --output/-o for a different output dir, or --in-place to replace in place (deletes original)",
            input.display()
        ));
    }

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    Ok(output)
}

pub fn format_size_change(input_size: u64, output_size: u64) -> String {
    let reduction = if input_size == 0 {
        0.0
    } else {
        1.0 - (output_size as f64 / input_size as f64)
    };
    let reduction_pct = reduction * 100.0;

    if reduction >= 0.0 {
        format!("size reduced {:.1}%", reduction_pct)
    } else {
        let diff_bytes = output_size as i64 - input_size as i64;
        let size_diff = crate::modern_ui::format_size_diff(diff_bytes);
        format!("size increased {:.1}% ({})", -reduction_pct, size_diff)
    }
}

pub fn calculate_size_reduction(input_size: u64, output_size: u64) -> f64 {
    if input_size == 0 {
        return 0.0;
    }
    (1.0 - (output_size as f64 / input_size as f64)) * 100.0
}

/// Pre-conversion check: tests duplicate and output-exists skip conditions.
///
/// **TOCTOU note**: The `output.exists()` check here is advisory only.
/// Callers MUST use `temp_path_for_output()` + `commit_temp_to_output()`
/// to write atomically; do NOT rely on this check as a write guard.
pub fn pre_conversion_check(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> Option<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Some(ConversionResult::skipped_duplicate(input));
    }

    if output.exists() && !options.force {
        return Some(ConversionResult::skipped_exists(input, output));
    }

    None
}

pub fn finalize_conversion(
    input: &Path,
    output: &Path,
    input_size: u64,
    format_name: &str,
    extra_info: Option<&str>,
    options: &ConvertOptions,
) -> std::io::Result<ConversionResult> {
    let output_size = std::fs::metadata(output)?.len();

    // Metadata already preserved by commit_temp_to_output_with_metadata
    // (includes EXIF, XMP, xattrs, permissions, and timestamps)

    mark_as_processed(input);

    if format_name.eq_ignore_ascii_case("JXL") {
        crate::progress_mode::jxl_success();
    }

    if options.should_delete_original() {
        let _ = safe_delete_original(input, output, MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE);
    }

    Ok(ConversionResult::success(
        input,
        output,
        input_size,
        output_size,
        format_name,
        extra_info,
        options.quality_label.as_deref(),
    ))
}

pub fn post_conversion_actions(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> std::io::Result<()> {
    if let Err(e) = crate::preserve_metadata(input, output) {
        eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }

    mark_as_processed(input);

    if options.should_delete_original() {
        safe_delete_original(input, output, MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE)?;
    }

    Ok(())
}

// --- Atomic output (TOCTOU mitigation) ---

/// Guard that removes the temp file on drop if it still exists (e.g. conversion failed before commit).
/// Hold this for the lifetime of conversion; after successful `commit_temp_to_output` the file is gone so drop is a no-op.
pub struct TempOutputGuard(PathBuf);

impl TempOutputGuard {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }
}

impl Drop for TempOutputGuard {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = fs::remove_file(&self.0);
        }
    }
}

/// Returns a path for temporary output in the same directory as `output`, so that
/// `fs::rename(temp, output)` is atomic on the same filesystem. Use with `commit_temp_to_output`.
/// Uses stem + ".tmp." + extension (e.g. file.mov → file.tmp.mov) so FFmpeg and other
/// tools that infer format from extension still see the correct extension (mov, mp4, mkv, etc.).
pub fn temp_path_for_output(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = output
        .extension()
        .map(|e| e.to_string_lossy())
        .unwrap_or_default();
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    
    let mut rng = rand::thread_rng();
    let random_id: String = (0..8)
        .map(|_| {
            let idx = rng.gen_range(0..62);
            match idx {
                0..=25 => (b'a' + idx) as char,
                26..=51 => (b'A' + (idx - 26)) as char,
                _ => (b'0' + (idx - 52)) as char,
            }
        })
        .collect();

    parent.join(format!("{}.tmp.{}.{}", stem, random_id, ext))
}

/// **DEPRECATED AND REMOVED**: This function has been removed.
/// 
/// All conversions MUST preserve metadata. Use `commit_temp_to_output_with_metadata` instead.
/// 
/// This function previously did NOT preserve metadata (timestamps, EXIF, XMP, xattrs, permissions),
/// which violated the program's core requirement of comprehensive metadata preservation.
#[deprecated(since = "0.10.71", note = "Removed. Use commit_temp_to_output_with_metadata instead.")]
pub fn commit_temp_to_output(_temp: &Path, _output: &Path, _force: bool) -> std::io::Result<bool> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "commit_temp_to_output has been removed; use commit_temp_to_output_with_metadata instead",
    ))
}

/// Commits a temp file with complete metadata preservation from the original file.
/// Preserves: timestamps (atime, mtime, btime), xattrs, permissions, EXIF data, XMP sidecars.
pub fn commit_temp_to_output_with_metadata(
    temp: &Path,
    output: &Path,
    force: bool,
    original: Option<&Path>,
) -> std::io::Result<bool> {
    if temp.exists() {
        let size = fs::metadata(temp)?.len();
        if size == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Refusing to commit empty output (temp file size 0)",
            ));
        }
    }
    if !force && output.exists() {
        let _ = fs::remove_file(temp);
        return Ok(false);
    }
    fs::rename(temp, output)?;
    
    // Preserve complete metadata from original file if provided
    if let Some(src) = original {
        // Step 1: Preserve metadata (EXIF, XMP, xattrs, permissions)
        // This may modify the file (e.g., ExifTool writes EXIF/XMP), which changes timestamps
        if let Err(e) = crate::metadata::preserve_metadata(src, output) {
            eprintln!("⚠️ Failed to preserve metadata: {}", e);
        }
        crate::metadata::merge_xmp_sidecar_into_dest(src, output);
        
        // Step 2: Apply timestamps AFTER all file modifications
        // This is critical because ExifTool and other tools reset creation time to current time
        // We must reapply timestamps as the final step to preserve original creation time
        crate::metadata::apply_file_timestamps(src, output);
    }
    
    Ok(true)
}

/// Get image/video dimensions using ffprobe → image crate → ImageMagick fallback chain.
///
/// Returns (width, height) or an error if all methods fail.
pub fn get_input_dimensions(input: &Path) -> Result<(u32, u32), String> {
    // Method 1: ffprobe
    if let Ok(probe) = crate::probe_video(input) {
        if probe.width > 0 && probe.height > 0 {
            return Ok((probe.width, probe.height));
        }
    }

    // Method 2: image crate
    if let Ok((w, h)) = image::image_dimensions(input) {
        return Ok((w, h));
    }

    // Method 3: ImageMagick identify
    {
        use std::process::Command;
        let safe_path = crate::safe_path_arg(input);
        let output = Command::new("magick")
            .args(["identify", "-format", "%w %h\n"])
            .arg(safe_path.as_ref())
            .output()
            .or_else(|_| {
                Command::new("identify")
                    .args(["-format", "%w %h\n"])
                    .arg(safe_path.as_ref())
                    .output()
            });
        if let Ok(out) = output {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = s.lines().next() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                            if w > 0 && h > 0 {
                                return Ok((w, h));
                            }
                        }
                    }
                }
            }
        }
    }

    Err(format!(
        "Could not get file dimensions: {}\n\
         ffprobe, image crate, and ImageMagick identify all failed; check file integrity or install ffmpeg/ImageMagick",
        input.display(),
    ))
}

/// Check if output exceeds size tolerance and clean up if so.
///
/// **Two independent but coordinated flags:**
/// - `allow_size_tolerance`: when true, allows size increase < 1_048_576 bytes; when false, requires `output <= input`.
///   This absolute byte tolerance is fairer to all file sizes than percentage-based.
/// - `compress`: when true, **goal is to make output smaller than input**.
///   **BUT: respects `allow_size_tolerance` when enabled** - if increase < 1_048_576 bytes, still accepts.
///   Only when increase ≥ 1_048_576 bytes (or tolerance disabled), compress mode rejects the output.
///
/// **Logic flow:**
/// 1. Check oversized threshold (tolerance-based): if increase ≥ 1_048_576 bytes → reject
/// 2. Check compress goal: if compress=true AND increase ≥ tolerance → reject
/// 3. Otherwise: accept
///
/// Returns `Some(ConversionResult)` if the output should be rejected (caller should return it),
/// or `None` if the output passes the size check.
pub fn check_size_tolerance(
    input: &Path,
    output: &Path,
    input_size: u64,
    output_size: u64,
    options: &ConvertOptions,
    format_label: &str,
) -> Option<ConversionResult> {
    // Tolerance: allow size increase < 1_048_576 bytes (1MB)
    const TOLERANCE_BYTES: u64 = 1_048_576; // 1MB absolute value
    
    let max_allowed_size = if options.allow_size_tolerance {
        input_size.saturating_add(TOLERANCE_BYTES - 1) // up to 1_048_576 - 1 bytes

    } else {
        input_size
    };

    // Over tolerance: output larger than allowed (e.g. > input or increase ≥ 1MB)
    if output_size > max_allowed_size {
        let size_increase_bytes = output_size.saturating_sub(input_size);
        let size_increase_kb = size_increase_bytes as f64 / 1024.0;
        let size_increase_mb = size_increase_bytes as f64 / (1024.0 * 1024.0);
        let size_increase_pct = if input_size == 0 {
            0.0
        } else {
            ((output_size as f64 / input_size as f64) - 1.0) * 100.0
        };
        
        // Always log deletion (not just in verbose mode)
        let mode = if options.allow_size_tolerance {
            "tolerance: absolute (< 1_048_576 bytes increase)".to_string()
        } else {
            "strict mode: no tolerance".to_string()
        };
        
        // Display in KB or MB depending on size
        if size_increase_mb >= 1.0 {
            crate::log_eprintln!(
                "   {} {} output discarded │ {}ratio: {:.1}%{} │ {}increase: +{:.2}MB{} │ {}",
                symbols::CROSS,
                format_label,
                colors::BOLD, 100.0 + size_increase_pct, colors::RESET,
                colors::MFB_ORANGE, size_increase_mb, colors::RESET,
                mode
            );
            crate::log_eprintln!(
                "   {} Size: {} → {} (Δ +{:.2}MB)",
                symbols::CHART,
                format!("{}{}{} bytes", colors::DIM, input_size, colors::RESET),
                format!("{}{}{} bytes", colors::MFB_RED, output_size, colors::RESET),
                size_increase_mb
            );
        } else {
            crate::log_eprintln!(
                "   {} {} output discarded │ {}ratio: {:.1}%{} │ {}increase: +{:.1}KB{} │ {}",
                symbols::CROSS,
                format_label,
                colors::BOLD, 100.0 + size_increase_pct, colors::RESET,
                colors::MFB_ORANGE, size_increase_kb, colors::RESET,
                mode
            );
            crate::log_eprintln!(
                "   {} Size: {} → {} (Δ +{:.1}KB)",
                symbols::CHART,
                format!("{}{}{} bytes", colors::DIM, input_size, colors::RESET),
                format!("{}{}{} bytes", colors::MFB_RED, output_size, colors::RESET),
                size_increase_kb
            );
        }
        
        if let Err(e) = fs::remove_file(output) {
            crate::log_eprintln!("   {} Cleanup failed: {}", symbols::WARNING, e);
        }
        
        // Copy original to output directory
        match crate::copy_on_skip_or_fail(
            input,
            options.output_dir.as_deref(),
            options.base_dir.as_deref(),
            false,
        ) {
            Ok(Some(dest)) => {
                crate::log_eprintln!("   {} Original preserved: {}", symbols::SHIELD, format!("{}{}{}", colors::DIM, dest.display(), colors::RESET));
            }
            Ok(None) => {
                // No output_dir specified, nothing to copy
            }
            Err(e) => {
                eprintln!("   ⚠️  Failed to copy original: {}", e);
            }
        }
        
        mark_as_processed(input);
        return Some(ConversionResult::skipped_size_increase(
            input,
            input_size,
            output_size,
        ));
    }

    // Compress mode: goal is strictly smaller; equal = not achieved
    // BUT: respect tolerance setting when enabled
    if options.compress && output_size >= input_size {
        let size_increase_bytes = output_size.saturating_sub(input_size);
        
        // If tolerance is enabled and increase is within tolerance (< 1_048_576 bytes), accept it
        if options.allow_size_tolerance && size_increase_bytes < TOLERANCE_BYTES {
            // Within tolerance, accept the output
            return None;
        }
        
        // Beyond tolerance or tolerance disabled: reject
        let size_change_kb = size_increase_bytes as f64 / 1024.0;
        let size_change_mb = size_increase_bytes as f64 / (1024.0 * 1024.0);
        let change_pct = if input_size == 0 {
            0.0
        } else {
            ((output_size as f64 / input_size as f64) - 1.0) * 100.0
        };
        
        if change_pct.abs() < 0.01 {
            crate::log_eprintln!(
                "   🗑️  {} output deleted: {}",
                format_label,
                "\x1b[1;33msize unchanged (compression goal not achieved)\x1b[0m"
            );
            crate::log_eprintln!(
                "   📊 Size: {} → {} bytes",
                format!("\x1b[2m{}\x1b[0m", input_size),
                format!("\x1b[2m{}\x1b[0m", output_size)
            );
        } else if size_change_mb >= 1.0 {
            crate::log_eprintln!(
                "   {} {} output discarded │ {}ratio: {:.1}%{} │ {}increase: +{:.2}MB{}",
                symbols::CROSS,
                format_label,
                colors::BOLD, change_pct + 100.0, colors::RESET,
                colors::MFB_ORANGE, size_change_mb, colors::RESET
            );
            crate::log_eprintln!(
                "   {} Size: {} → {} (Δ +{:.2}MB)",
                symbols::CHART,
                format!("{}{}{} bytes", colors::DIM, input_size, colors::RESET),
                format!("{}{}{} bytes", colors::MFB_RED, output_size, colors::RESET),
                size_change_mb
            );
        } else {
            crate::log_eprintln!(
                "   {} {} output discarded │ {}ratio: {:.1}%{} │ {}increase: +{:.1}KB{}",
                symbols::CROSS,
                format_label,
                colors::BOLD, change_pct + 100.0, colors::RESET,
                colors::MFB_ORANGE, size_change_kb, colors::RESET
            );
            crate::log_eprintln!(
                "   {} Size: {} → {} (Δ +{:.1}KB)",
                symbols::CHART,
                format!("{}{}{} bytes", colors::DIM, input_size, colors::RESET),
                format!("{}{}{} bytes", colors::MFB_RED, output_size, colors::RESET),
                size_change_kb
            );
        }

        if let Err(e) = fs::remove_file(output) {
            crate::log_upstream_error!("File cleanup", "Failed to remove output file: {}", e);
        }

        // Copy original to output directory
        match crate::copy_on_skip_or_fail(
            input,
            options.output_dir.as_deref(),
            options.base_dir.as_deref(),
            false,
        ) {
            Ok(Some(dest)) => {
                crate::log_eprintln!("   📋 Original copied to: {}", format!("\x1b[2m{}\x1b[0m", dest.display()));
            }
            Ok(None) => {
                // No output_dir specified, nothing to copy
            }
            Err(e) => {
                crate::log_upstream_error!("File copy", "Failed to copy original to output dir: {}", e);
            }
        }
        
        mark_as_processed(input);
        return Some(ConversionResult::skipped_size_unchanged(
            input,
            input_size,
            format_label,
        ));
    }

    None
}

/// Validate input file for conversion.
/// Checks:
/// - File exists and is a regular file (not directory or special file)
/// - File is not a symbolic link (security risk)
/// - File is readable
///
/// Returns Ok(()) if valid, Err with descriptive message otherwise.
pub fn validate_input_file(input: &Path) -> Result<(), String> {
    // Check if path exists
    if !input.exists() {
        return Err(format!("Input file does not exist: {}", input.display()));
    }

    // Check if it's a symbolic link (security risk)
    if input.is_symlink() {
        return Err(format!(
            "Symbolic links are not supported for security reasons: {}",
            input.display()
        ));
    }

    // Check if it's a regular file
    if !input.is_file() {
        return Err(format!(
            "Input is not a regular file (may be a directory or special file): {}",
            input.display()
        ));
    }

    // Check if file is readable by attempting to open it
    if let Err(e) = fs::File::open(input) {
        return Err(format!(
            "Cannot read input file {}: {}",
            input.display(),
            e
        ));
    }

    Ok(())
}

/// Validate output path for conversion.
/// Checks:
/// - Output is not a symbolic link (security risk)
///
/// Returns Ok(()) if valid, Err with descriptive message otherwise.
///
/// Note: Path traversal check removed - output paths are generated programmatically
/// and may intentionally be in adjacent directories (e.g., _optimized suffix mode).
pub fn validate_output_path(
    output: &Path,
    _base_dir: Option<&Path>,
) -> Result<(), String> {
    // Check if output is a symbolic link
    if output.exists() && output.is_symlink() {
        return Err(format!(
            "Output path is a symbolic link, refusing to overwrite: {}",
            output.display()
        ));
    }

    Ok(())
}

/// Handle Apple AAE (Apple Adjustment Envelope) files.
/// AAE files store photo editing metadata from iPhone/Photos.app.
/// When the source image is converted, the AAE becomes orphaned.
///
/// - In apple_compat mode: migrate AAE to output directory
/// - Otherwise: delete orphaned AAE file
pub fn handle_aae_file(input: &Path, output: &Path, apple_compat: bool) {
    let aae_path = input.with_extension("AAE");
    let aae_path_lower = input.with_extension("aae");

    let existing_aae = if aae_path.exists() {
        Some(aae_path)
    } else if aae_path_lower.exists() {
        Some(aae_path_lower)
    } else {
        None
    };

    if let Some(aae) = existing_aae {
        if apple_compat {
            // Migrate AAE to output directory
            if let Some(output_dir) = output.parent() {
                if let Some(filename) = aae.file_name() {
                    let target_aae = output_dir.join(filename);
                    if let Err(e) = fs::copy(&aae, &target_aae) {
                        eprintln!("⚠️  Failed to migrate AAE file: {}", e);
                    }
                }
            }
        } else {
            // Delete orphaned AAE file
            if let Err(e) = fs::remove_file(&aae) {
                eprintln!("⚠️  Failed to delete orphaned AAE file: {}", e);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_strict_size_reduction_formula() {
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

            assert!(
                (result - expected).abs() < 0.001,
                "STRICT: {}->{}  expected {}, got {}",
                input,
                output,
                expected,
                result
            );
            assert!(
                (result - expected_calc).abs() < 0.0001,
                "STRICT: Formula mismatch for {}->{}",
                input,
                output
            );
        }
    }

    #[test]
    fn test_strict_large_file_sizes() {
        let reduction = calculate_size_reduction(10_000_000_000, 5_000_000_000);
        assert!(
            (reduction - 50.0).abs() < 0.001,
            "STRICT: 10GB->5GB should be exactly 50%, got {}",
            reduction
        );

        let reduction = calculate_size_reduction(100_000_000_000, 25_000_000_000);
        assert!(
            (reduction - 75.0).abs() < 0.001,
            "STRICT: 100GB->25GB should be exactly 75%, got {}",
            reduction
        );
    }

    #[test]
    fn test_strict_small_file_sizes() {
        let reduction = calculate_size_reduction(100, 50);
        assert!(
            (reduction - 50.0).abs() < 0.001,
            "STRICT: 100->50 bytes should be exactly 50%, got {}",
            reduction
        );
    }

    #[test]
    fn test_format_size_change_reduction() {
        let msg = format_size_change(1000, 500);
        assert!(
            msg.contains("reduced"),
            "Should say 'reduced' for smaller output"
        );
        assert!(msg.contains("50.0%"), "Should show 50.0% for half size");
    }

    #[test]
    fn test_temp_path_for_output_keeps_extension() {
        // Temp path must end with same extension as output so FFmpeg/muxers see correct format.
        let path1 = temp_path_for_output(Path::new("/dir/file.mov")).to_string_lossy().to_string();
        assert!(path1.starts_with("/dir/file.tmp."));
        assert!(path1.ends_with(".mov"));

        let path2 = temp_path_for_output(Path::new("out.mp4")).to_string_lossy().to_string();
        assert!(path2.starts_with("out.tmp."));
        assert!(path2.ends_with(".mp4"));

        let path3 = temp_path_for_output(Path::new("a/b/c.mkv")).to_string_lossy().to_string();
        assert!(path3.starts_with("a/b/c.tmp."));
        assert!(path3.ends_with(".mkv"));

        let path4 = temp_path_for_output(Path::new("name.with.dots.mov")).to_string_lossy().to_string();
        assert!(path4.starts_with("name.with.dots.tmp."));
        assert!(path4.ends_with(".mov"));
    }

    #[test]
    fn test_removed_commit_temp_to_output_returns_error() {
        #[expect(deprecated, reason = "regression test for removed compatibility shim")]
        let err = commit_temp_to_output(Path::new("temp.tmp"), Path::new("out.mp4"), false)
            .expect_err("removed API should return an error instead of panicking");

        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(
            err.to_string().contains("commit_temp_to_output has been removed"),
            "unexpected error message: {}",
            err
        );
    }

    #[test]
    fn test_load_processed_list_is_atomic_on_invalid_utf8() {
        clear_processed_list();

        let tracked = std::env::temp_dir().join("mfb-processed-track.mp4");
        let tracked_canonical = tracked.display().to_string();
        let mut list = NamedTempFile::new().expect("failed to create processed list");
        list.write_all(tracked_canonical.as_bytes())
            .expect("failed to write valid entry");
        list.write_all(b"\n\xff\n")
            .expect("failed to write invalid utf8");

        let err = load_processed_list(list.path())
            .expect_err("invalid utf8 should fail instead of partially loading state");
        assert!(
            !is_already_processed(&tracked),
            "processed list should not be partially updated on read failure"
        );
        assert!(
            err.to_string().contains("stream did not contain valid UTF-8"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_format_size_change_increase() {
        let msg = format_size_change(500, 1000);
        assert!(
            msg.contains("increased"),
            "Should say 'increased' for larger output"
        );
        assert!(
            msg.contains("100.0%"),
            "Should show 100.0% for doubled size"
        );
    }

    #[test]
    fn test_format_size_change_no_change() {
        let msg = format_size_change(1000, 1000);
        assert!(msg.contains("reduced"), "Same size shows as 0% reduced");
        assert!(msg.contains("0.0%"), "Should show 0.0% for same size");
    }

    #[test]
    fn test_determine_output_path() {
        let input = Path::new("/path/to/image.png");
        let output = determine_output_path(input, "jxl", &None).unwrap();
        assert_eq!(output, Path::new("/path/to/image.JXL"));
    }

    #[test]
    fn test_determine_output_path_with_dir() {
        let input = Path::new("/path/to/image.png");
        let output_dir = Some(PathBuf::from("/output"));
        let output = determine_output_path(input, "avif", &output_dir).unwrap();
        assert_eq!(output, Path::new("/output/image.AVIF"));
    }

    #[test]
    fn test_determine_output_path_various_extensions() {
        let input = Path::new("/path/to/video.mp4");

        let webm = determine_output_path(input, "webm", &None).unwrap();
        assert_eq!(webm, Path::new("/path/to/video.WEBM"));

        let mkv = determine_output_path(input, "mkv", &None).unwrap();
        assert_eq!(mkv, Path::new("/path/to/video.MKV"));
    }

    #[test]
    fn test_conversion_result_success() {
        let input = Path::new("/test/input.png");
        let output = Path::new("/test/output.avif");

        let result = ConversionResult::success(input, output, 1000, 500, "AVIF", None, None);

        assert!(result.success);
        assert!(!result.skipped);
        assert_eq!(result.input_size, 1000);
        assert_eq!(result.output_size, Some(500));
        assert!((result.size_reduction.unwrap() - 50.0).abs() < 0.1);
        assert!(result.message.contains("transcoding"), "expected 'transcoding' in: {}", result.message);
        assert!(result.message.contains("-50.0%"), "expected '-50.0%' in: {}", result.message);
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

    #[test]
    fn test_conversion_result_size_unchanged() {
        let input = Path::new("/test/input.png");

        let result =
            ConversionResult::skipped_size_unchanged(input, 1000, "JXL");

        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("size_unchanged".to_string()));
        assert!(result.message.contains("unchanged"));
        assert!(result.message.contains("compression goal not achieved"));
    }

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

    #[test]
    fn test_flag_mode_with_gpu() {
        let mut opts = ConvertOptions::default();
        opts.explore = true;
        opts.match_quality = true;
        opts.compress = true;
        opts.use_gpu = true;

        let mode = opts.flag_mode().unwrap();
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
        assert!(opts.use_gpu, "GPU should remain enabled");
    }

    #[test]
    fn test_flag_mode_with_cpu() {
        let mut opts = ConvertOptions::default();
        opts.explore = true;
        opts.match_quality = true;
        opts.compress = true;
        opts.use_gpu = false;

        let mode = opts.flag_mode().unwrap();
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
        assert!(!opts.use_gpu, "CPU mode should remain disabled");
    }

    #[test]
    fn test_only_recommended_flags_valid_with_gpu_cpu() {
        let mut opts_gpu = ConvertOptions::default();
        opts_gpu.explore = true;
        opts_gpu.match_quality = true;
        opts_gpu.compress = true;
        opts_gpu.use_gpu = true;
        assert!(opts_gpu.flag_mode().is_ok());

        let mut opts_cpu = ConvertOptions::default();
        opts_cpu.explore = true;
        opts_cpu.match_quality = true;
        opts_cpu.compress = true;
        opts_cpu.use_gpu = false;
        assert!(opts_cpu.flag_mode().is_ok());

        assert_eq!(opts_gpu.flag_mode().unwrap(), opts_cpu.flag_mode().unwrap());
    }

    #[test]
    fn test_invalid_flag_combinations_rejected() {
        let invalid_combos = [
            (false, false, false),
            (false, false, true),
            (false, true, false),
            (true, false, false),
        ];

        for (explore, match_quality, compress) in invalid_combos {
            let mut opts = ConvertOptions::default();
            opts.explore = explore;
            opts.match_quality = match_quality;
            opts.compress = compress;
            assert!(
                opts.flag_mode().is_err(),
                "({}, {}, {}) should be invalid",
                explore,
                match_quality,
                compress
            );
        }
    }

    #[test]
    fn test_convert_options_all_flags_enabled() {
        let mut opts = ConvertOptions::default();
        opts.force = true;
        opts.delete_original = true;
        opts.in_place = true;
        opts.explore = true;
        opts.match_quality = true;
        opts.compress = true;
        opts.apple_compat = true;
        opts.use_gpu = false;

        assert!(opts.force);
        assert!(opts.should_delete_original());
        assert!(opts.apple_compat);
        assert!(!opts.use_gpu);

        let mode = opts.flag_mode().unwrap();
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
    }

    #[test]
    fn test_convert_options_invalid_flag_combination() {
        let mut opts = ConvertOptions::default();
        opts.explore = true;
        opts.match_quality = false;
        opts.compress = true;

        let result = opts.flag_mode();
        assert!(
            result.is_err(),
            "explore + compress without match_quality should be invalid"
        );
    }

    #[test]
    fn test_explore_mode_returns_precise_quality_with_compression() {
        let mut opts = ConvertOptions::default();
        opts.explore = true;
        opts.match_quality = true;
        opts.compress = true;

        assert_eq!(
            opts.explore_mode(),
            crate::video_explorer::ExploreMode::PreciseQualityMatchWithCompression,
        );
    }
}
