//! Batch Processing Module
//!
//! Provides utilities for batch file processing with proper error handling
//! Reference: media/CONTRIBUTING.md - Batch Processing Capability requirement
//!
//! Batch Processing Module with File Sorting

use crate::ffprobe::probe_video;
use crate::file_sorter::{SortStrategy, sort_by_size_ascending};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use walkdir::{DirEntry, WalkDir};

/// Cached state information for a directory in the path tree.
///
/// Stores the directory path and its last modification time to detect
/// when the directory structure has changed and the cache needs to be updated.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedDirectoryState {
    /// The absolute path to the directory.
    path: PathBuf,
    /// The last modification time of the directory in Unix seconds (`None` when
    /// unavailable).
    modified_unix_secs: Option<u64>,
}

/// Cached entry for an image file in the sorted path tree.
///
/// Contains metadata needed for efficient sorting and processing of images
/// in batch operations. This information is cached to avoid repeated filesystem
/// and metadata lookups.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedImageSortEntry {
    /// The absolute path to the image file.
    path: PathBuf,
    /// File size in bytes.
    size: u64,
    /// Depth relative to the root directory (`None` when path is outside root).
    relative_depth: Option<usize>,
    /// Priority value for format-based sorting (lower = higher priority).
    format_priority: u8,
    /// Total number of pixels (width × height) if available.
    pixel_count: Option<u64>,
}

/// Complete snapshot of the image tree structure and metadata.
///
/// This represents the cached state of all directories and files in a path
/// tree, including the configuration parameters used to generate it. The
/// snapshot can be serialized and deserialized to persist the cache between
/// runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedImageTreeSnapshot {
    /// The schema version of this cache format.
    schema_version: u32,
    /// The root directory path for this tree.
    root: PathBuf,
    /// Whether this tree was built recursively.
    recursive: bool,
    /// File extensions that were included in this tree.
    extensions: Vec<String>,
    /// Cached state information for all directories.
    directories: Vec<CachedDirectoryState>,
    /// Cached metadata for all image files.
    files: Vec<CachedImageSortEntry>,
}

/// Cached entry for a video file in the sorted path tree.
///
/// Contains metadata needed for efficient sorting and processing of videos
/// in batch operations. This information is cached to avoid repeated filesystem
/// and video metadata lookups.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedVideoSortEntry {
    /// The absolute path to the video file.
    path: PathBuf,
    /// File size in bytes.
    size: u64,
    /// Depth relative to the root directory (`None` when path is outside root).
    relative_depth: Option<usize>,
    /// Total number of pixels (width × height) if available.
    pixel_count: Option<u64>,
    /// Video duration in seconds if available.
    duration_secs: Option<f64>,
    /// Video frame rate if available.
    frame_rate: Option<f64>,
    /// Estimated processing work units based on video complexity.
    estimated_work: Option<u64>,
}

/// Complete snapshot of the video tree structure and metadata.
///
/// This represents the cached state of all directories and video files in a
/// path tree, including the configuration parameters used to generate it. The
/// snapshot can be serialized and deserialized to persist the cache between
/// runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedVideoTreeSnapshot {
    /// The schema version of this cache format.
    schema_version: u32,
    /// The root directory path for this tree.
    root: PathBuf,
    /// Whether this tree was built recursively.
    recursive: bool,
    /// File extensions that were included in this tree.
    extensions: Vec<String>,
    /// Cached state information for all directories.
    directories: Vec<CachedDirectoryState>,
    /// Cached metadata for all video files.
    files: Vec<CachedVideoSortEntry>,
}

/// Checks if a directory entry is safe to process in batch operations.
///
/// This function validates symlinks to ensure they don't point to dangerous
/// locations. Unresolvable symlinks are also considered unsafe.
///
/// # Arguments
/// * `entry` - The directory entry to check
///
/// # Returns
/// `true` if the entry is safe, `false` otherwise
fn is_safe_entry(entry: &DirEntry) -> bool {
    if entry.path_is_symlink() {
        // Use gate canonicalization to keep batch path-tree rooting auditable and
        // avoid silent canonicalize fallbacks in production.
        let canonical = crate::media_conversion_gate::canonicalize_for_tool_input(entry.path());
        if canonical.as_os_str().is_empty() {
            // Treat empty canonicalization as unsafe (unresolvable symlink / policy
            // rejection).
            return false;
        }
        if canonical.exists() {
            if crate::safety::check_dangerous_directory(&canonical).is_err() {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "delivery_pipeline_batch",
                    entry.path(),
                    crate::infra::static_logs::messages::MSG_BATCH_SECURITY_SYMLINK
                        .replace("{}", &entry.path().display().to_string()),
                );
                return false;
            }
        } else {
            // Unresolvable symlinks are inherently unsafe in batch contexts
            return false;
        }
    }
    true
}

fn identify_source_codec_for_batch(
    path: &Path,
    context: &str,
) -> Option<crate::quality_matcher::SourceCodec> {
    match crate::quality_matcher::SourceCodec::identify_by_content(path) {
        Ok(codec) => codec,
        Err(err) => {
            crate::media_conversion_gate::delivery_pipeline_path_audit(
                "delivery_pipeline_batch",
                path,
                format!(
                    "{context}: failed content identification for {}: {err}",
                    path.display()
                ),
            );
            None
        }
    }
}

pub fn collect_files(dir: &Path, extensions: &[&str], recursive: bool) -> Vec<PathBuf> {
    let walker = if recursive {
        WalkDir::new(dir).follow_links(true)
    } else {
        WalkDir::new(dir).max_depth(1)
    };

    let mut files = Vec::new();
    for entry in walker.into_iter().filter_entry(is_safe_entry) {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if !entry.file_type().is_file() {
                    continue;
                }

                // Strict content-based identification. Filename extension is not
                // an admission gate because spoofed extensions must be handled by
                // the shared scanner, not by per-tool special cases.
                if let Some(codec) = identify_source_codec_for_batch(path, "collect_files")
                    && codec_matches_requested_extensions(codec, extensions)
                {
                    files.push(path.to_path_buf());
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "delivery_pipeline_batch",
                    dir,
                    format!(
                        "Batch Audit: Failed to collect assets for processing under {dir_path}: \
                         {err}",
                        dir_path = dir.display(),
                    ),
                );
            }
        }
    }
    files
}

#[must_use]
pub fn collect_files_sorted(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
    sort_strategy: SortStrategy,
) -> Vec<PathBuf> {
    let files = collect_files(dir, extensions, recursive);

    match sort_strategy {
        SortStrategy::None => files,
        SortStrategy::SizeAscending => sort_by_size_ascending(files),
        _ => crate::file_sorter::FileSorter::new(sort_strategy).sort(files),
    }
}

#[must_use]
pub fn collect_files_small_first(dir: &Path, extensions: &[&str], recursive: bool) -> Vec<PathBuf> {
    collect_files_sorted(dir, extensions, recursive, SortStrategy::SizeAscending)
}

/// Pure filesystem scan — no DB, walkdir only. Fully testable.
/// // \[SB\] split from `collect_image_files_for_perceived_speed`.
pub fn scan_image_files(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> crate::unified_error::Result<Vec<PathBuf>> {
    Ok(scan_image_tree_snapshot(dir, extensions, recursive)
        .files
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

pub fn collect_image_files_for_perceived_speed(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> crate::unified_error::Result<Vec<PathBuf>> {
    let cached = load_cached_image_tree(dir, extensions, recursive)
        .filter(|snapshot| validate_cached_image_tree(snapshot, dir, extensions, recursive));
    let snapshot = if let Some(snapshot) = cached {
        snapshot
    } else {
        let files = scan_image_files(dir, extensions, recursive)?; // [SB] pure scan first
        let snapshot = scan_image_tree_snapshot_from_files(dir, extensions, recursive, files);
        if let Err(err) = save_cached_image_tree(&snapshot) {
            crate::media_conversion_gate::delivery_pipeline_path_audit(
                "probe_heic",
                dir,
                format!(
                    "Batch Audit: Failed to persist analysis to cache for {dir_path}: {err}",
                    dir_path = dir.display(),
                ),
            );
        }
        snapshot
    };

    Ok(snapshot.files.into_iter().map(|entry| entry.path).collect())
}

#[must_use]
pub fn collect_video_files_for_perceived_speed(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> Vec<PathBuf> {
    let cached = load_cached_video_tree(dir, extensions, recursive)
        .filter(|snapshot| validate_cached_video_tree(snapshot, dir, extensions, recursive));
    let snapshot = if let Some(snapshot) = cached {
        snapshot
    } else {
        let snapshot = scan_video_tree_snapshot(dir, extensions, recursive);
        if let Err(err) = save_cached_video_tree(&snapshot) {
            crate::media_conversion_gate::delivery_pipeline_path_audit(
                "probe_heic",
                dir,
                format!(
                    "Batch Audit: Failed to persist video analysis to cache for {dir_path}: {err}",
                    dir_path = dir.display(),
                ),
            );
        }
        snapshot
    };

    snapshot.files.into_iter().map(|entry| entry.path).collect()
}

pub fn calculate_directory_size_by_extensions(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> u64 {
    let walker = if recursive {
        WalkDir::new(dir).follow_links(true)
    } else {
        WalkDir::new(dir).max_depth(1)
    };

    let mut total = 0u64;
    for entry in walker.into_iter().filter_entry(is_safe_entry) {
        match entry {
            Ok(entry) => {
                if !entry.file_type().is_file()
                    || !crate::common_utils::has_extension(entry.path(), extensions)
                {
                    continue;
                }
                match crate::io_utils::metadata_with_retry(entry.path()) {
                    Ok(metadata) => total = total.saturating_add(metadata.len()),
                    Err(err) => {
                        crate::media_conversion_gate::delivery_pipeline_path_audit(
                            "delivery_pipeline_batch",
                            entry.path(),
                            format!(
                                "Batch Audit: Failed to process metadata for {entry_path}: {err}",
                                entry_path = entry.path().display(),
                            ),
                        );
                    }
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "delivery_pipeline_batch",
                    dir,
                    format!(
                        "Batch Audit: Failed to calculate total batch size under {dir_path}: {err}",
                        dir_path = dir.display(),
                    ),
                );
            }
        }
    }
    total
}

#[derive(Debug, Clone)]
/// Information about why a batch operation was paused.
///
/// Contains the path where the pause occurred and a human-readable reason.
/// This information can be displayed to users to help them understand
/// why processing was interrupted.
pub struct PauseInfo {
    /// The file path where the pause occurred.
    pub path: PathBuf,
    /// Human-readable explanation for the pause.
    pub reason: String,
}

/// Controller for managing pause/resume functionality in batch operations.
///
/// Provides thread-safe pause controls with atomic operations and
/// optional pause information. This allows batch operations to be
/// gracefully interrupted and resumed while maintaining context.
#[derive(Debug, Default)]
pub struct PauseController {
    /// Atomic flag indicating if the batch is currently paused.
    paused: AtomicBool,
    /// Optional information about the current pause state.
    info: Mutex<Option<PauseInfo>>,
}

impl PauseController {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if the batch operation is currently paused.
    ///
    /// # Returns
    /// `true` if paused, `false` otherwise
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Requests to pause the batch operation at the current file.
    ///
    /// This method is thread-safe and will only set the pause state once.
    /// If the batch is already paused, this returns `false`.
    ///
    /// # Arguments
    /// * `path` - The file path where the pause is being requested
    /// * `reason` - Human-readable reason for the pause
    ///
    /// # Returns
    /// `true` if this call newly set the pause state, `false` if already paused
    pub fn request_pause(&self, path: &Path, reason: impl Into<String>) -> bool {
        let reason = reason.into();
        let newly_paused = self
            .paused
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();

        if newly_paused {
            let mut info = crate::media_conversion_gate::mutex_guard_or_recover(
                "batch_pause_info",
                self.info.lock(),
            );
            *info = Some(PauseInfo {
                path: path.to_path_buf(),
                reason,
            });
        }

        newly_paused
    }

    /// Gets the current pause information if the batch is paused.
    ///
    /// # Returns
    /// `Some(PauseInfo)` if paused, `None` if not paused
    pub fn pause_info(&self) -> Option<PauseInfo> {
        return crate::media_conversion_gate::mutex_guard_or_recover(
            "batch_pause_info",
            self.info.lock(),
        )
        .clone();
    }
}

#[must_use]
pub fn disk_full_pause_reason(message: &str) -> Option<String> {
    let lower = message.to_lowercase();
    let disk_full = [
        "no space left on device",
        "disk full",
        "storage full",
        "database or disk is full",
        "there is not enough space on the disk",
        "not enough space",
        "enospc",
        "no usable temporary file name found",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    if disk_full {
        Some(crate::infra::static_logs::messages::MSG_BATCH_PAUSE_DISK_FULL.to_string())
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub ignored: usize,
    pub errors: Vec<(PathBuf, String)>,
    pub paused: bool,
    pub pause_info: Option<PauseInfo>,
    pub paused_remaining: usize,
}

impl Summary {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            ignored: 0,
            errors: Vec::new(),
            paused: false,
            pause_info: None,
            paused_remaining: 0,
        }
    }

    pub const fn success(&mut self) {
        self.total = self.total.saturating_add(1);
        self.succeeded = self.succeeded.saturating_add(1);
    }

    pub fn fail(&mut self, path: PathBuf, error: String) {
        self.total = self.total.saturating_add(1);
        self.failed = self.failed.saturating_add(1);
        self.errors.push((path, error));
    }

    pub fn warn(&mut self, message: &str) {
        self.errors
            .push((PathBuf::new(), format!("WARNING: {message}")));
    }

    pub const fn skip(&mut self) {
        self.total = self.total.saturating_add(1);
        self.skipped = self.skipped.saturating_add(1);
    }

    pub const fn ignore(&mut self) {
        self.total = self.total.saturating_add(1);
        self.ignored = self.ignored.saturating_add(1);
    }

    pub fn pause(&mut self, path: PathBuf, reason: String, remaining: usize) {
        self.paused = true;
        self.pause_info = Some(PauseInfo { path, reason });
        self.paused_remaining = remaining;
    }

    #[must_use]
    pub fn success_rate(&self) -> f64 {
        // Success rate should only consider succeeded and failed as active actions.
        // Skipped items are implicitly successful (already optimized).
        // Ignored items are excluded from the denominator.
        let denominator = self.succeeded.saturating_add(self.failed);
        if denominator == 0 {
            100.0
        } else {
            (crate::numeric_cast::usize_to_f64(self.succeeded)
                / crate::numeric_cast::usize_to_f64(denominator))
                * 100.0
        }
    }
}

impl Default for Summary {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalizes and deduplicates file extensions.
///
/// Converts all extensions to lowercase, sorts them alphabetically,
/// and removes duplicates to ensure consistent processing.
///
/// # Arguments
/// * `extensions` - Slice of file extension strings
///
/// # Returns
/// Vector of normalized, unique extensions
fn normalized_extensions(extensions: &[&str]) -> Vec<String> {
    let mut normalized: Vec<String> = extensions
        .iter()
        .map(|ext| ext.to_ascii_lowercase())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn codec_matches_requested_extensions(
    codec: crate::quality_matcher::SourceCodec,
    extensions: &[&str],
) -> bool {
    extensions.is_empty()
        || extensions
            .iter()
            .any(|extension| codec.is_extension_compatible(extension.trim_start_matches('.')))
}

/// Gets the last modification time of a path in Unix seconds.
///
/// # Arguments
/// * `path` - The file or directory path
///
/// # Returns
/// Last modification time as Unix seconds (`None` when metadata is
/// unavailable).
fn path_modified_unix_secs(path: &Path) -> Option<u64> {
    crate::media_conversion_gate::delivery_path_modified_unix_secs_optional(path)
}

/// Relative depth from batch root (`None` when `path` is not under `root`).
fn relative_depth_from_root(root: &Path, path: &Path) -> Option<usize> {
    crate::media_conversion_gate::delivery_batch_relative_depth_optional(root, path)
}

/// Determines the format priority for an image file.
///
/// Lower numbers indicate higher priority for sorting.
/// This is used to prefer certain formats over others in batch operations.
///
/// # Arguments
/// * `path` - The image file path
///
/// # Returns
/// Priority value (0 = highest priority, 6 = lowest)
fn format_priority_for_image(path: &Path) -> u8 {
    if let Some(codec) = identify_source_codec_for_batch(path, "format_priority_for_image") {
        return match codec {
            crate::quality_matcher::SourceCodec::Jpeg
            | crate::quality_matcher::SourceCodec::Mjpeg => 0,
            crate::quality_matcher::SourceCodec::Png | crate::quality_matcher::SourceCodec::Bmp => {
                1
            }
            crate::quality_matcher::SourceCodec::WebpStatic
            | crate::quality_matcher::SourceCodec::WebpAnimated => 2,
            crate::quality_matcher::SourceCodec::Heic
            | crate::quality_matcher::SourceCodec::Avif => 3,
            crate::quality_matcher::SourceCodec::Gif
            | crate::quality_matcher::SourceCodec::Apng => 4,
            crate::quality_matcher::SourceCodec::Tiff => 5,
            _ => 6,
        };
    }

    match crate::common_utils::get_extension_lowercase(path).as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => 0,
        "png" | "bmp" | "ico" => 1,
        "webp" => 2,
        "heic" | "heif" | "avif" => 3,
        "gif" => 4,
        "tiff" | "tif" | "jp2" | "j2k" | "svg" => 5,
        _ => 6,
    }
}

/// Total pixel count (width × height) for sorting.
///
/// Uses the shared ffprobe → `image` crate → `ImageMagick` `identify` fallback
/// chain so modern formats (HEIC/HEIF/AVIF/JXL) are handled uniformly.
fn image_pixel_count(path: &Path) -> Option<u64> {
    match crate::conversion::get_input_dimensions(path) {
        Ok((width, height)) => {
            crate::media_conversion_gate::delivery_pipeline_pixel_count_u64_or_none(
                width,
                height,
                path,
                "delivery_pipeline_batch",
                format!(
                    "Batch Audit: Pixel count overflow during estimation for {path_display} \
                     ({width}x{height})",
                    path_display = path.display(),
                ),
            )
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_pipeline_path_audit(
                "delivery_pipeline_batch",
                path,
                crate::infra::static_logs::messages::MSG_BATCH_PIXEL_READ_FAIL
                    .replacen("{}", &path.display().to_string(), 1)
                    .replacen("{}", &e, 1),
            );
            None
        }
    }
}

/// Compares optional numeric sort keys without forging sentinel values.
///
/// Missing metadata is an explicit worst-case sort key, not a synthetic
/// `u64::MAX` payload.
fn cmp_optional_u64_missing_last(left: Option<u64>, right: Option<u64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Deeper paths first; unknown depth sorts after known depths.
fn cmp_optional_usize_deeper_first(
    left: Option<usize>,
    right: Option<usize>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn cmp_optional_f64_missing_last(left: Option<f64>, right: Option<f64>) -> std::cmp::Ordering {
    cmp_optional_u64_missing_last(left.and_then(float_ord_key), right.and_then(float_ord_key))
}

/// Converts a floating-point value to a sortable ordinal key.
///
/// Multiplies by 1000 and rounds to preserve 3 decimal places of precision.
/// Non-finite, negative, or overflowing values remain absent.
///
/// # Arguments
/// * `value` - The floating-point value to convert
///
/// # Returns
/// Sortable ordinal key for comparison
fn float_ord_key(value: f64) -> Option<u64> {
    if value.is_finite() && value >= 0.0 {
        crate::numeric_cast::f64_to_u64_strict((value * 1000.0).round(), "float_ord_key")
    } else {
        None
    }
}

/// Compares two cached image sort entries for ordering.
///
/// Implements the sorting logic for batch operations based on multiple
/// criteria: depth, format priority, size, and pixel count.
///
/// # Arguments
/// * `left` - First sort entry
/// * `right` - Second sort entry
///
/// # Returns
/// Ordering comparison result
fn compare_image_sort_entries(
    left: &CachedImageSortEntry,
    right: &CachedImageSortEntry,
) -> std::cmp::Ordering {
    cmp_optional_usize_deeper_first(left.relative_depth, right.relative_depth)
        .then_with(|| left.format_priority.cmp(&right.format_priority))
        .then_with(|| left.size.cmp(&right.size))
        .then_with(|| cmp_optional_u64_missing_last(left.pixel_count, right.pixel_count))
        .then_with(|| left.path.cmp(&right.path))
}

/// Sorts cached image entries using the comparison function.
///
/// # Arguments
/// * `entries` - Mutable slice of cached image entries to sort
fn sort_cached_image_entries(entries: &mut [CachedImageSortEntry]) {
    entries.sort_by(compare_image_sort_entries);
}

/// Builds a cached image entry from file metadata.
///
/// Collects all necessary information for an image file to be used in
/// batch sorting operations.
///
/// # Arguments
/// * `root` - The root directory path
/// * `path` - The image file path
///
/// # Returns
/// Cached image entry, or None if metadata cannot be read
fn build_cached_image_entry(
    root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
) -> CachedImageSortEntry {
    CachedImageSortEntry {
        path: path.to_path_buf(),
        size: metadata.len(),
        relative_depth: relative_depth_from_root(root, path),
        format_priority: format_priority_for_image(path),
        pixel_count: image_pixel_count(path),
    }
}

/// Loads a cached image tree snapshot from the path-tree store.
///
/// Attempts to read and deserialize a previously cached image tree.
/// Returns None if the cache file doesn't exist, is corrupted, or schema
/// version mismatch.
///
/// # Arguments
/// * `dir` - The root directory path
/// * `extensions` - File extensions included in the tree
/// * `recursive` - Whether the tree is recursive
///
/// # Returns
/// Cached image tree snapshot, or None if loading fails
fn load_cached_image_tree(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> Option<CachedImageTreeSnapshot> {
    let cache_key =
        crate::path_tree_cache::path_tree_cache_key(dir, extensions, recursive, "image");
    match crate::path_tree_cache::load_path_tree_snapshot(
        &cache_key,
        crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION,
    ) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "delivery_pipeline_batch",
                format!("path_tree image cache load failed for {cache_key}: {err}"),
            );
            None
        }
    }
}

/// Saves a cached image tree snapshot to disk.
///
/// Serializes and writes the image tree snapshot to the cache file.
/// Creates the cache directory if it doesn't exist.
///
/// # Arguments
/// * `snapshot` - The image tree snapshot to save
///
/// # Returns
/// Ok(()) if successful, or IO error if writing fails
fn save_cached_image_tree(snapshot: &CachedImageTreeSnapshot) -> io::Result<()> {
    let cache_key = crate::path_tree_cache::path_tree_cache_key(
        &snapshot.root,
        &snapshot.extensions_as_refs(),
        snapshot.recursive,
        "image",
    );
    crate::path_tree_cache::save_path_tree_snapshot(
        &cache_key,
        "image",
        &snapshot.root,
        crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION,
        snapshot,
    )
    .map_err(io::Error::other)
}

/// Loads a cached video tree snapshot from the path-tree store.
///
/// Attempts to read and deserialize a previously cached video tree.
/// Returns None if the cache file doesn't exist, is corrupted, or schema
/// version mismatch.
///
/// # Arguments
/// * `dir` - The root directory path
/// * `extensions` - File extensions included in the tree
/// * `recursive` - Whether the tree is recursive
///
/// # Returns
/// Cached video tree snapshot, or None if loading fails
fn load_cached_video_tree(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> Option<CachedVideoTreeSnapshot> {
    let cache_key =
        crate::path_tree_cache::path_tree_cache_key(dir, extensions, recursive, "video");
    match crate::path_tree_cache::load_path_tree_snapshot(
        &cache_key,
        crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION,
    ) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "delivery_pipeline_batch",
                format!("path_tree video cache load failed for {cache_key}: {err}"),
            );
            None
        }
    }
}

/// Saves a cached video tree snapshot to disk.
///
/// Serializes and writes the video tree snapshot to the cache file.
/// Creates the cache directory if it doesn't exist.
///
/// # Arguments
/// * `snapshot` - The video tree snapshot to save
///
/// # Returns
/// Ok(()) if successful, or IO error if writing fails
fn save_cached_video_tree(snapshot: &CachedVideoTreeSnapshot) -> io::Result<()> {
    let cache_key = crate::path_tree_cache::path_tree_cache_key(
        &snapshot.root,
        &snapshot.extensions_as_refs(),
        snapshot.recursive,
        "video",
    );
    crate::path_tree_cache::save_path_tree_snapshot(
        &cache_key,
        "video",
        &snapshot.root,
        crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION,
        snapshot,
    )
    .map_err(io::Error::other)
}

/// Validates that a cached image tree snapshot matches the expected
/// configuration.
///
/// Checks schema version, root directory, recursive flag, and extensions
/// to ensure the cache is still valid for the current request.
///
/// # Arguments
/// * `snapshot` - The cached snapshot to validate
/// * `dir` - The expected root directory
/// * `extensions` - The expected file extensions
/// * `recursive` - The expected recursive flag
///
/// # Returns
/// `true` if the cache is valid, `false` otherwise
fn validate_cached_image_tree(
    snapshot: &CachedImageTreeSnapshot,
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> bool {
    if snapshot.schema_version != crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION {
        return false;
    }

    let expected_root = crate::media_conversion_gate::canonicalize_for_tool_input(dir);
    if snapshot.root != expected_root
        || snapshot.recursive != recursive
        || snapshot.extensions != normalized_extensions(extensions)
    {
        return false;
    }

    snapshot.directories.iter().all(|directory| {
        let current_mtime = path_modified_unix_secs(&directory.path);
        current_mtime == directory.modified_unix_secs
    })
}

/// Validates that a cached video tree snapshot matches the expected
/// configuration.
///
/// Checks schema version, root directory, recursive flag, and extensions
/// to ensure the cache is still valid for the current request.
///
/// # Arguments
/// * `snapshot` - The cached snapshot to validate
/// * `dir` - The expected root directory
/// * `extensions` - The expected file extensions
/// * `recursive` - The expected recursive flag
///
/// # Returns
/// `true` if the cache is valid, `false` otherwise
fn validate_cached_video_tree(
    snapshot: &CachedVideoTreeSnapshot,
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> bool {
    if snapshot.schema_version != crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION {
        return false;
    }

    let expected_root = crate::media_conversion_gate::canonicalize_for_tool_input(dir);
    if snapshot.root != expected_root
        || snapshot.recursive != recursive
        || snapshot.extensions != normalized_extensions(extensions)
    {
        return false;
    }

    snapshot.directories.iter().all(|directory| {
        let current_mtime = path_modified_unix_secs(&directory.path);
        current_mtime == directory.modified_unix_secs
    })
}

/// Scans the filesystem to create a fresh image tree snapshot.
///
/// Walks the directory tree, collecting metadata for all directories
/// and image files matching the specified extensions.
///
/// # Arguments
/// * `dir` - The root directory to scan
/// * `extensions` - File extensions to include
/// * `recursive` - Whether to scan recursively
///
/// # Returns
/// Complete image tree snapshot with current filesystem state
fn scan_image_tree_snapshot(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> CachedImageTreeSnapshot {
    let root = crate::media_conversion_gate::canonicalize_for_tool_input(dir);
    let walker = if recursive {
        WalkDir::new(&root).follow_links(true)
    } else {
        WalkDir::new(&root).max_depth(1)
    };

    let mut directories = Vec::new();
    let mut files = Vec::new();

    for entry in walker.into_iter().filter_entry(is_safe_entry) {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_dir() {
                    if recursive || entry.depth() == 0 {
                        directories.push(CachedDirectoryState {
                            path: entry.path().to_path_buf(),
                            modified_unix_secs: path_modified_unix_secs(entry.path()),
                        });
                    }
                    continue;
                }

                if entry.file_type().is_file() {
                    let path = entry.path();
                    if let Some(codec) =
                        identify_source_codec_for_batch(path, "scan_image_tree_snapshot")
                        && codec.is_image()
                        && codec_matches_requested_extensions(codec, extensions)
                    {
                        let metadata = match fs::metadata(path) {
                            Ok(m) => m,
                            Err(e) => {
                                crate::media_conversion_gate::delivery_pipeline_path_audit(
                                    "delivery_pipeline_batch",
                                    path,
                                    format!(
                                        "METADATA AUDIT: Failed to read metadata for image entry \
                                         '{}' during batch scan | Forensic: Error '{}'; skipping \
                                         entry in path-tree cache",
                                        path.display(),
                                        e
                                    ),
                                );
                                continue;
                            }
                        };
                        let file_entry = build_cached_image_entry(&root, path, &metadata);
                        files.push(file_entry);
                    }
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "delivery_pipeline_batch",
                    &root,
                    format!(
                        "COLLECTION AUDIT: Failed to inspect directory entry in '{}' while \
                         building path-tree cache | Forensic: Error '{err}'; skipping entry to \
                         maintain cache build continuity",
                        root.display(),
                    ),
                );
            }
        }
    }

    sort_cached_image_entries(&mut files);

    tracing::info!(
        target: "static_log",
        path = %root.display(),
        file_count = files.len(),
        dir_count = directories.len(),
        outcome = "batch_snapshot",
        "Path-tree snapshot refreshed"
    );
    for entry in &files {
        tracing::trace!(
            target: "static_log",
            path = %entry.path.display(),
            outcome = "batch_snapshot_entry",
            "Path-tree file"
        );
    }

    CachedImageTreeSnapshot {
        schema_version: crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION,
        root,
        recursive,
        extensions: normalized_extensions(extensions),
        directories,
        files,
    }
}

fn scan_image_tree_snapshot_from_files(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
    files: Vec<PathBuf>,
) -> CachedImageTreeSnapshot {
    let root = crate::media_conversion_gate::canonicalize_for_tool_input(dir);
    let walker = if recursive {
        WalkDir::new(&root).follow_links(true)
    } else {
        WalkDir::new(&root).max_depth(1)
    };
    let mut directories = Vec::new();

    for entry in walker.into_iter().filter_entry(is_safe_entry) {
        match entry {
            Ok(entry) if entry.file_type().is_dir() && (recursive || entry.depth() == 0) => {
                directories.push(CachedDirectoryState {
                    path: entry.path().to_path_buf(),
                    modified_unix_secs: path_modified_unix_secs(entry.path()),
                });
            }
            Ok(_) => {}
            Err(err) => {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "delivery_pipeline_batch",
                    &root,
                    format!(
                        "COLLECTION AUDIT: Failed to inspect directory entry in '{}' while \
                         building path-tree cache | Forensic: Error '{err}'; skipping entry to \
                         maintain cache build continuity",
                        root.display(),
                    ),
                );
            }
        }
    }

    let files = files
        .into_iter()
        .filter_map(|path| {
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(e) => {
                    crate::media_conversion_gate::delivery_pipeline_path_audit(
                        "delivery_pipeline_batch",
                        &path,
                        format!(
                            "METADATA AUDIT: Failed to read metadata for image entry '{}' during \
                             batch scan | Forensic: Error '{}'; skipping entry in path-tree cache",
                            path.display(),
                            e
                        ),
                    );
                    return None;
                }
            };
            Some(build_cached_image_entry(&root, &path, &metadata))
        })
        .collect();

    CachedImageTreeSnapshot {
        schema_version: crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION,
        root,
        recursive,
        extensions: normalized_extensions(extensions),
        directories,
        files,
    }
}

/// Probes video file to extract priority data for sorting.
///
/// Extracts pixel count, duration, frame rate, and estimated work
/// to determine video processing priority in batch operations.
///
/// # Arguments
/// * `path` - The video file path to probe
///
/// # Returns
/// Tuple of (`pixel_count`, `duration_secs`, `frame_rate`, `estimated_work`)
fn video_probe_priority_data(path: &Path) -> (Option<u64>, Option<f64>, Option<f64>, Option<u64>) {
    let Ok(probe) = probe_video(path) else {
        return (None, None, None, None);
    };

    let pixel_count = if probe.width > 0 && probe.height > 0 {
        crate::media_conversion_gate::delivery_pipeline_pixel_count_u64_or_none(
            probe.width,
            probe.height,
            path,
            "delivery_pipeline_cli",
            format!(
                "NUMERIC ANOMALY: Video pixel-count multiplication overflowed for '{}' ({}x{}) | \
                 Forensic: u64 overflow during video metric calculation; pixel count metric \
                 unavailable",
                path.display(),
                probe.width,
                probe.height
            ),
        )
    } else {
        None
    };

    let duration_secs = probe.duration.filter(|&d| d.is_finite() && d > 0.0);

    let frame_rate = probe.frame_rate.filter(|&f| f.is_finite() && f > 0.0_f64);

    let frame_count = crate::media_conversion_gate::delivery_video_frame_count_or_estimate(
        probe.frame_count,
        duration_secs,
        frame_rate,
    );

    let estimated_work = pixel_count.zip(frame_count).and_then(|(pixels, frames)| {
        crate::media_conversion_gate::delivery_video_sort_work_or_none(pixels, frames, path)
    });

    (pixel_count, duration_secs, frame_rate, estimated_work)
}

/// Compares two cached video sort entries for ordering.
///
/// Implements the sorting logic for batch operations based on multiple
/// criteria: depth, estimated work, duration, and other video-specific metrics.
///
/// # Arguments
/// * `left` - First sort entry
/// * `right` - Second sort entry
///
/// # Returns
/// Ordering comparison result
fn compare_video_sort_entries(
    left: &CachedVideoSortEntry,
    right: &CachedVideoSortEntry,
) -> std::cmp::Ordering {
    cmp_optional_usize_deeper_first(left.relative_depth, right.relative_depth)
        .then_with(|| cmp_optional_u64_missing_last(left.estimated_work, right.estimated_work))
        .then_with(|| cmp_optional_f64_missing_last(left.duration_secs, right.duration_secs))
        .then_with(|| left.size.cmp(&right.size))
        .then_with(|| cmp_optional_u64_missing_last(left.pixel_count, right.pixel_count))
        .then_with(|| cmp_optional_f64_missing_last(left.frame_rate, right.frame_rate))
        .then_with(|| left.path.cmp(&right.path))
}

/// Sorts cached video entries using the comparison function.
///
/// # Arguments
/// * `entries` - Mutable slice of cached video entries to sort
fn sort_cached_video_entries(entries: &mut [CachedVideoSortEntry]) {
    entries.sort_by(compare_video_sort_entries);
}

/// Builds a cached video entry from file metadata and probe data.
///
/// Collects all necessary information for a video file to be used in
/// batch sorting operations, including probing for video-specific metrics.
///
/// # Arguments
/// * `root` - The root directory path
/// * `path` - The video file path
///
/// # Returns
/// Cached video entry, or None if metadata cannot be read
fn build_cached_video_entry(root: &Path, path: &Path) -> Option<CachedVideoSortEntry> {
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            crate::media_conversion_gate::delivery_pipeline_path_audit(
                "delivery_pipeline_batch",
                path,
                format!(
                    "METADATA AUDIT: Failed to read metadata for video entry '{}' during batch \
                     scan | Forensic: Error '{}'; skipping entry in video path-tree cache",
                    path.display(),
                    e
                ),
            );
            return None;
        }
    };
    let (pixel_count, duration_secs, frame_rate, estimated_work) = video_probe_priority_data(path);
    Some(CachedVideoSortEntry {
        path: path.to_path_buf(),
        size: metadata.len(),
        relative_depth: relative_depth_from_root(root, path),
        pixel_count,
        duration_secs,
        frame_rate,
        estimated_work,
    })
}

/// Scans the filesystem to create a fresh video tree snapshot.
///
/// Walks the directory tree, collecting metadata for all directories
/// and video files matching the specified extensions.
///
/// # Arguments
/// * `dir` - The root directory to scan
/// * `extensions` - File extensions to include
/// * `recursive` - Whether to scan recursively
///
/// # Returns
/// Complete video tree snapshot with current filesystem state
fn scan_video_tree_snapshot(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> CachedVideoTreeSnapshot {
    let root = crate::media_conversion_gate::canonicalize_for_tool_input(dir);
    let walker = if recursive {
        WalkDir::new(&root).follow_links(true)
    } else {
        WalkDir::new(&root).max_depth(1)
    };

    let mut directories = Vec::new();
    let mut files = Vec::new();

    for entry in walker.into_iter().filter_entry(is_safe_entry) {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_dir() {
                    if recursive || entry.depth() == 0 {
                        directories.push(CachedDirectoryState {
                            path: entry.path().to_path_buf(),
                            modified_unix_secs: path_modified_unix_secs(entry.path()),
                        });
                    }
                    continue;
                }

                if entry.file_type().is_file() {
                    let path = entry.path();
                    if let Some(codec) =
                        identify_source_codec_for_batch(path, "scan_video_tree_snapshot")
                        && codec_matches_requested_extensions(codec, extensions)
                    {
                        // Admission: it's a video OR it's an animated image candidate for the 'vid'
                        // tool
                        if (codec.is_video() || codec.can_be_animated())
                            && let Some(file_entry) = build_cached_video_entry(&root, path)
                        {
                            files.push(file_entry);
                        }
                    }
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "delivery_pipeline_batch",
                    &root,
                    format!(
                        "COLLECTION AUDIT: Failed to inspect directory entry in '{}' while \
                         building video path-tree cache | Forensic: Error '{err}'; skipping entry \
                         to maintain cache build continuity",
                        root.display(),
                    ),
                );
            }
        }
    }

    sort_cached_video_entries(&mut files);

    tracing::info!(
        target: "static_log",
        path = %root.display(),
        file_count = files.len(),
        dir_count = directories.len(),
        outcome = "batch_snapshot",
        "Video path-tree snapshot refreshed"
    );
    for entry in &files {
        tracing::trace!(
            target: "static_log",
            path = %entry.path.display(),
            outcome = "batch_snapshot_entry",
            "Video path-tree file"
        );
    }

    CachedVideoTreeSnapshot {
        schema_version: crate::path_tree_cache::PATH_TREE_SCHEMA_VERSION,
        root,
        recursive,
        extensions: normalized_extensions(extensions),
        directories,
        files,
    }
}

impl CachedImageTreeSnapshot {
    fn extensions_as_refs(&self) -> Vec<&str> {
        self.extensions.iter().map(String::as_str).collect()
    }
}

impl CachedVideoTreeSnapshot {
    fn extensions_as_refs(&self) -> Vec<&str> {
        self.extensions.iter().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::FileTime;
    use image::{ImageFormat, Rgb, RgbImage};
    use tempfile::TempDir;

    fn write_test_image(path: &Path, width: u32, height: u32, format: ImageFormat) {
        let image = RgbImage::from_pixel(width, height, Rgb([128, 96, 64]));
        image
            .save_with_format(path, format)
            .expect("failed to write test image");
    }

    fn sorted_relative_paths(root: &Path, files: &[PathBuf]) -> anyhow::Result<Vec<String>> {
        let canonical_root = root.canonicalize()?;
        let mut rels = files
            .iter()
            .map(|path| {
                path.canonicalize()?
                    .strip_prefix(&canonical_root)
                    .map(|rel| rel.to_string_lossy().to_string())
                    .map_err(anyhow::Error::msg)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        rels.sort();
        Ok(rels)
    }

    #[test]
    fn test_batch_result_new() {
        let result = Summary::new();
        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_batch_result_success() {
        let mut result = Summary::new();
        result.success();

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn test_batch_result_fail() {
        let mut result = Summary::new();
        result.fail(PathBuf::from("test.png"), "Error message".to_string());

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors.first().map_or("", |e| &e.1), "Error message");
    }

    #[test]
    fn test_batch_result_skip() {
        let mut result = Summary::new();
        result.skip();

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_batch_result_mixed() {
        let mut result = Summary::new();
        result.success();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());
        result.skip();

        assert_eq!(result.total, 4);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_success_rate_empty() {
        let result = Summary::new();
        assert!(
            (result.success_rate() - 100.0).abs() < 0.01_f64,
            "Empty batch should have 100% success rate"
        );
    }

    #[test]
    fn test_success_rate_all_success() {
        let mut result = Summary::new();
        for _ in 0_i32..10_i32 {
            result.success();
        }
        assert!(
            (result.success_rate() - 100.0).abs() < 0.01_f64,
            "All success should be 100%"
        );
    }

    #[test]
    fn test_success_rate_all_fail() {
        let mut result = Summary::new();
        for i in 0_i32..10_i32 {
            result.fail(PathBuf::from(format!("file{i}.png")), "Error".to_string());
        }
        assert!(
            (result.success_rate() - 0.0).abs() < 0.01_f64,
            "All fail should be 0%"
        );
    }

    #[test]
    fn test_success_rate_50_percent() {
        let mut result = Summary::new();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());

        assert!(
            (result.success_rate() - 50.0).abs() < 0.01_f64,
            "1 success, 1 fail should be 50%, got {}",
            result.success_rate()
        );
    }

    #[test]
    fn test_success_rate_with_skipped() {
        let mut result = Summary::new();
        result.success();
        result.success();
        result.skip();
        result.skip();

        assert!(
            (result.success_rate() - 100.0).abs() < 0.01_f64,
            "2 success, 2 skipped should be 100% because skips are excluded, got {}",
            result.success_rate()
        );
    }

    #[test]
    fn test_strict_success_rate_formula() {
        let test_cases = [
            (10_i32, 0_i32, 0_i32, 100.0_f64),
            (5_i32, 5_i32, 0_i32, 50.0_f64),
            (3_i32, 1_i32, 0_i32, 75.0_f64),
            (1_i32, 3_i32, 0_i32, 25.0_f64),
            (0_i32, 10_i32, 0_i32, 0.0_f64),
            (7_i32, 2_i32, 1_i32, 77.777_777_777_777_79_f64),
        ];

        for (success, fail, skip, expected) in test_cases {
            let mut result = Summary::new();
            for _ in 0_i32..success {
                result.success();
            }
            for i in 0_i32..fail {
                result.fail(PathBuf::from(format!("f{i}.png")), "E".to_string());
            }
            for _ in 0_i32..skip {
                result.skip();
            }

            let rate = result.success_rate();
            let denominator = result.succeeded.saturating_add(result.failed);
            let expected_calc = if denominator == 0 {
                100.0_f64
            } else {
                (crate::numeric_cast::usize_to_f64(result.succeeded)
                    / crate::numeric_cast::usize_to_f64(denominator))
                    * 100.0_f64
            };

            assert!(
                (rate - expected).abs() < 0.001_f64,
                "STRICT: {success}s/{fail}f/{skip}k expected {expected}%, got {rate}%"
            );
            assert!(
                (rate - expected_calc).abs() < 0.000_1_f64,
                "STRICT: Formula mismatch"
            );
        }
    }

    #[test]
    fn test_strict_large_numbers() {
        let mut result = Summary::new();

        for _ in 0_i32..500_000_i32 {
            result.success();
        }
        for i in 0_i32..500_000_i32 {
            result.fail(PathBuf::from(format!("f{i}.png")), "E".to_string());
        }

        assert_eq!(result.total, 1_000_000);
        assert!(
            (result.success_rate() - 50.0).abs() < 0.001_f64,
            "STRICT: Large batch should calculate correctly"
        );
    }

    #[test]
    fn test_consistency_success_rate() {
        let mut result = Summary::new();
        result.success();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());

        let rate1 = result.success_rate();
        let rate2 = result.success_rate();
        let rate3 = result.success_rate();

        assert!((rate1 - rate2).abs() < 1e-7_f64);
        assert!((rate2 - rate3).abs() < 1e-7_f64);
    }

    #[test]
    fn test_total_equals_sum() {
        let mut result = Summary::new();
        result.success();
        result.success();
        result.success();
        result.fail(PathBuf::from("f1.png"), "E".to_string());
        result.fail(PathBuf::from("f2.png"), "E".to_string());
        result.skip();

        assert_eq!(
            result.total,
            result.succeeded + result.failed + result.skipped,
            "STRICT: total must equal succeeded + failed + skipped"
        );
    }

    #[test]
    fn test_disk_full_pause_reason_matches_common_messages() {
        assert!(disk_full_pause_reason("No space left on device").is_some());
        assert!(disk_full_pause_reason("sqlite error: database or disk is full").is_some());
        assert!(disk_full_pause_reason("ENOSPC while writing temp output").is_some());
        assert!(disk_full_pause_reason("permission denied").is_none());
    }

    #[test]
    fn test_batch_result_pause_tracks_remaining_work() {
        let mut result = Summary::new();
        result.success();
        result.pause(
            PathBuf::from("example.mov"),
            "Disk exhausted".to_string(),
            5,
        );

        assert!(result.paused);
        assert_eq!(result.total, 1);
        assert_eq!(result.paused_remaining, 5);
        assert_eq!(
            result.pause_info.as_ref().map(|info| info.path.as_path()),
            Some(Path::new("example.mov"))
        );
    }

    #[test]
    fn test_pause_controller_keeps_first_pause_reason() {
        let controller = PauseController::new();

        assert!(controller.request_pause(Path::new("first.png"), "first"));
        assert!(!controller.request_pause(Path::new("second.png"), "second"));

        let info = controller
            .pause_info()
            .unwrap_or_else(|| panic!("pause info should exist"));
        assert_eq!(info.path, PathBuf::from("first.png"));
        assert_eq!(info.reason, "first");
    }

    #[test]
    fn test_collect_image_files_for_perceived_speed_respects_priority_order() -> anyhow::Result<()>
    {
        let temp_dir = TempDir::new().map_err(|e| anyhow::anyhow!("temp dir: {e}"))?;
        let root = temp_dir.path();
        let nested = root.join("nested");
        let deeper = nested.join("deeper");
        fs::create_dir_all(&deeper).map_err(|e| anyhow::anyhow!("create dir: {e}"))?;

        let root_png = root.join("root.png");
        let nested_jpg = nested.join("nested.jpg");
        let deeper_png = deeper.join("deeper.png");
        let deeper_jpg = deeper.join("deeper.jpg");

        write_test_image(&root_png, 32, 32, ImageFormat::Png);
        write_test_image(&nested_jpg, 48, 48, ImageFormat::Jpeg);
        write_test_image(&deeper_png, 24, 24, ImageFormat::Png);
        write_test_image(&deeper_jpg, 12, 12, ImageFormat::Jpeg);

        let files = collect_image_files_for_perceived_speed(root, &["png", "jpg"], true)?;
        let ordered_names = files
            .iter()
            .map(|path| {
                path.file_name().map_or_else(
                    || "unknown".to_string(),
                    |n| n.to_string_lossy().into_owned(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            ordered_names,
            vec![
                "deeper.jpg".to_string(),
                "deeper.png".to_string(),
                "nested.jpg".to_string(),
                "root.png".to_string(),
            ],
            "Expected deeper paths first, then JPEG fast-lane, then remaining files"
        );
        Ok(())
    }

    #[test]
    fn scan_image_files_admits_true_images_with_arbitrary_extensions() -> anyhow::Result<()> {
        let temp_dir = TempDir::new().map_err(|e| anyhow::anyhow!("temp dir: {e}"))?;
        let root = temp_dir.path();
        fs::create_dir_all(root.join("nested")).map_err(|e| anyhow::anyhow!("create dir: {e}"))?;

        write_test_image(&root.join("jpeg-disguised.mp4"), 12, 12, ImageFormat::Jpeg);
        write_test_image(&root.join("png-disguised.txt"), 12, 12, ImageFormat::Png);
        write_test_image(&root.join("nested/no-extension"), 12, 12, ImageFormat::Jpeg);
        write_test_image(&root.join("gif-disguised.jpg"), 12, 12, ImageFormat::Gif);
        fs::write(
            root.join("video-disguised.jpg"),
            b"\0\0\0\x10ftypisom\0\0\0\0",
        )
            .map_err(|e| anyhow::anyhow!("write video header: {e}"))?;

        let files = scan_image_files(root, &["jpg", "png"], true)?;
        let rels = sorted_relative_paths(root, &files)?;

        assert_eq!(
            rels,
            vec![
                "jpeg-disguised.mp4".to_string(),
                "nested/no-extension".to_string(),
                "png-disguised.txt".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn collect_files_filters_by_detected_codec_not_filename_extension() -> anyhow::Result<()> {
        let temp_dir = TempDir::new().map_err(|e| anyhow::anyhow!("temp dir: {e}"))?;
        let root = temp_dir.path();

        write_test_image(&root.join("jpeg-disguised.heic"), 12, 12, ImageFormat::Jpeg);
        write_test_image(&root.join("png-disguised.mp4"), 12, 12, ImageFormat::Png);
        write_test_image(&root.join("gif-disguised.jpg"), 12, 12, ImageFormat::Gif);

        let files = collect_files(root, &["jpg", "png"], false);
        let rels = sorted_relative_paths(root, &files)?;

        assert_eq!(
            rels,
            vec![
                "jpeg-disguised.heic".to_string(),
                "png-disguised.mp4".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn test_validate_cached_image_tree_detects_directory_changes() -> anyhow::Result<()> {
        let temp_dir = TempDir::new().map_err(|e| anyhow::anyhow!("temp dir: {e}"))?;
        let root = temp_dir.path();
        let nested = root.join("nested");
        fs::create_dir_all(&nested).map_err(|e| anyhow::anyhow!("create dir: {e}"))?;

        let image_path = nested.join("sample.jpg");
        write_test_image(&image_path, 16, 16, ImageFormat::Jpeg);

        let snapshot = scan_image_tree_snapshot(root, &["jpg"], true);
        assert!(validate_cached_image_tree(&snapshot, root, &["jpg"], true));

        let bumped = FileTime::from_unix_time(
            crate::numeric_cast::u64_to_i64_strict(
                path_modified_unix_secs(&nested).expect("nested dir mtime must be readable"),
                "mtime",
            )
            .expect("Failed to get mtime for test directory")
                + 10,
            0,
        );
        filetime::set_file_mtime(&nested, bumped).map_err(|e| anyhow::anyhow!("set mtime: {e}"))?;

        assert!(
            !validate_cached_image_tree(&snapshot, root, &["jpg"], true),
            "Directory mtime drift should invalidate the cached path tree"
        );
        Ok(())
    }

    #[test]
    fn test_video_sort_entries_prioritize_depth_then_size_then_resolution() {
        let fast_finish = CachedVideoSortEntry {
            path: PathBuf::from("a/deeper-fast.mov"),
            size: 160,
            relative_depth: Some(2),
            pixel_count: Some(640 * 360),
            duration_secs: Some(4.0_f64),
            frame_rate: Some(24.0_f64),
            estimated_work: Some(640 * 360 * 96),
        };
        let shallower = CachedVideoSortEntry {
            path: PathBuf::from("b/shallower-large.mov"),
            size: 500,
            relative_depth: Some(1),
            pixel_count: Some(320 * 240),
            duration_secs: Some(2.0_f64),
            frame_rate: Some(24.0_f64),
            estimated_work: Some(320 * 240 * 48),
        };
        let same_depth_shorter = CachedVideoSortEntry {
            path: PathBuf::from("c/tie-depth-shorter.mov"),
            size: 220,
            relative_depth: Some(2),
            pixel_count: Some(1280 * 720),
            duration_secs: Some(2.0_f64),
            frame_rate: Some(24.0_f64),
            estimated_work: Some(1280 * 720 * 48),
        };
        let same_depth_heavier = CachedVideoSortEntry {
            path: PathBuf::from("d/tie-depth-heavier.mov"),
            size: 80,
            relative_depth: Some(2),
            pixel_count: Some(1920 * 1080),
            duration_secs: Some(6.0_f64),
            frame_rate: Some(60.0_f64),
            estimated_work: Some(1920 * 1080 * 360),
        };

        let mut entries = vec![
            shallower.clone(),
            same_depth_heavier.clone(),
            fast_finish.clone(),
            same_depth_shorter.clone(),
        ];
        sort_cached_video_entries(&mut entries);

        assert_eq!(entries.first().map(|e| &e.path), Some(&fast_finish.path));
        assert_eq!(
            entries.get(1).map(|e| &e.path),
            Some(&same_depth_shorter.path)
        );
        assert_eq!(
            entries.get(2).map(|e| &e.path),
            Some(&same_depth_heavier.path)
        );
        assert_eq!(entries.get(3).map(|e| &e.path), Some(&shallower.path));
    }
}
