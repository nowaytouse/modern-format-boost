//! Batch Processing Module
//!
//! Provides utilities for batch file processing with proper error handling
//! Reference: media/CONTRIBUTING.md - Batch Processing Capability requirement
//!
//! 🔥 v7.5: 添加文件排序功能，优先处理小文件

use crate::file_sorter::{sort_by_size_ascending, SortStrategy};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;
use tracing::{debug, warn};
use walkdir::WalkDir;

const PATH_TREE_CACHE_SCHEMA_VERSION: u32 = 1;
const PATH_TREE_CACHE_DIR: &str = "path_tree";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedDirectoryState {
    path: PathBuf,
    modified_unix_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedImageSortEntry {
    path: PathBuf,
    size: u64,
    relative_depth: usize,
    format_priority: u8,
    pixel_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedImageTreeSnapshot {
    schema_version: u32,
    root: PathBuf,
    recursive: bool,
    extensions: Vec<String>,
    directories: Vec<CachedDirectoryState>,
    files: Vec<CachedImageSortEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedVideoSortEntry {
    path: PathBuf,
    size: u64,
    relative_depth: usize,
    pixel_count: Option<u64>,
    duration_secs: Option<f64>,
    frame_rate: Option<f64>,
    estimated_work: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedVideoTreeSnapshot {
    schema_version: u32,
    root: PathBuf,
    recursive: bool,
    extensions: Vec<String>,
    directories: Vec<CachedDirectoryState>,
    files: Vec<CachedVideoSortEntry>,
}

pub fn collect_files(dir: &Path, extensions: &[&str], recursive: bool) -> Vec<PathBuf> {
    let walker = if recursive {
        WalkDir::new(dir).follow_links(true)
    } else {
        WalkDir::new(dir).max_depth(1)
    };

    let mut files = Vec::new();
    for entry in walker.into_iter() {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_file()
                    && crate::common_utils::has_extension(entry.path(), extensions)
                {
                    files.push(entry.path().to_path_buf());
                }
            }
            Err(err) => {
                warn!(
                    dir = %dir.display(),
                    error = %err,
                    "Failed to inspect directory entry while collecting files"
                );
            }
        }
    }
    files
}

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

pub fn collect_files_small_first(dir: &Path, extensions: &[&str], recursive: bool) -> Vec<PathBuf> {
    collect_files_sorted(dir, extensions, recursive, SortStrategy::SizeAscending)
}

pub fn collect_image_files_for_perceived_speed(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> Vec<PathBuf> {
    let snapshot = load_cached_image_tree(dir, extensions, recursive)
        .filter(|snapshot| validate_cached_image_tree(snapshot, dir, extensions, recursive))
        .unwrap_or_else(|| {
            let snapshot = scan_image_tree_snapshot(dir, extensions, recursive);
            if let Err(err) = save_cached_image_tree(&snapshot) {
                warn!(
                    path = %dir.display(),
                    error = %err,
                    "Failed to persist path-tree cache; continuing without cache"
                );
            }
            snapshot
        });

    snapshot.files.into_iter().map(|entry| entry.path).collect()
}

pub fn collect_video_files_for_perceived_speed(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> Vec<PathBuf> {
    let snapshot = load_cached_video_tree(dir, extensions, recursive)
        .filter(|snapshot| validate_cached_video_tree(snapshot, dir, extensions, recursive))
        .unwrap_or_else(|| {
            let snapshot = scan_video_tree_snapshot(dir, extensions, recursive);
            if let Err(err) = save_cached_video_tree(&snapshot) {
                warn!(
                    path = %dir.display(),
                    error = %err,
                    "Failed to persist video path-tree cache; continuing without cache"
                );
            }
            snapshot
        });

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
    for entry in walker.into_iter() {
        match entry {
            Ok(entry) => {
                if !entry.file_type().is_file()
                    || !crate::common_utils::has_extension(entry.path(), extensions)
                {
                    continue;
                }
                match std::fs::metadata(entry.path()) {
                    Ok(metadata) => total += metadata.len(),
                    Err(err) => {
                        warn!(
                            path = %entry.path().display(),
                            error = %err,
                            "Failed to read file metadata while calculating directory size"
                        );
                    }
                }
            }
            Err(err) => {
                warn!(
                    dir = %dir.display(),
                    error = %err,
                    "Failed to inspect directory entry while calculating directory size"
                );
            }
        }
    }
    total
}

pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif", "avif",
    "jxl", "bmp",
];

pub const ANIMATED_EXTENSIONS: &[&str] = &["gif", "webp", "png"];

#[derive(Debug, Clone)]
pub struct BatchPauseInfo {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct BatchPauseController {
    paused: AtomicBool,
    info: Mutex<Option<BatchPauseInfo>>,
}

impl BatchPauseController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn request_pause(&self, path: &Path, reason: impl Into<String>) -> bool {
        let reason = reason.into();
        let newly_paused = self
            .paused
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();

        if newly_paused {
            let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
            *info = Some(BatchPauseInfo {
                path: path.to_path_buf(),
                reason,
            });
        }

        newly_paused
    }

    pub fn pause_info(&self) -> Option<BatchPauseInfo> {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

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
        Some(
            "Disk space was exhausted during processing. Batch paused; free space and rerun with --resume to continue."
                .to_string(),
        )
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct BatchResult {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<(PathBuf, String)>,
    pub paused: bool,
    pub pause_info: Option<BatchPauseInfo>,
    pub paused_remaining: usize,
}

impl BatchResult {
    pub fn new() -> Self {
        Self {
            total: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            errors: Vec::new(),
            paused: false,
            pause_info: None,
            paused_remaining: 0,
        }
    }

    pub fn success(&mut self) {
        self.total += 1;
        self.succeeded += 1;
    }

    pub fn fail(&mut self, path: PathBuf, error: String) {
        self.total += 1;
        self.failed += 1;
        self.errors.push((path, error));
    }

    pub fn skip(&mut self) {
        self.total += 1;
        self.skipped += 1;
    }

    pub fn pause(&mut self, path: PathBuf, reason: String, remaining: usize) {
        self.paused = true;
        self.pause_info = Some(BatchPauseInfo { path, reason });
        self.paused_remaining = remaining;
    }

    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            (self.succeeded as f64 / self.total as f64) * 100.0
        }
    }
}

impl Default for BatchResult {
    fn default() -> Self {
        Self::new()
    }
}

fn normalized_extensions(extensions: &[&str]) -> Vec<String> {
    let mut normalized: Vec<String> = extensions
        .iter()
        .map(|ext| ext.to_ascii_lowercase())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn path_modified_unix_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn relative_depth_from_root(root: &Path, path: &Path) -> usize {
    path.strip_prefix(root)
        .ok()
        .and_then(Path::parent)
        .map(|parent| parent.components().count())
        .unwrap_or(0)
}

fn format_priority_for_image(path: &Path) -> u8 {
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

fn image_pixel_count(path: &Path) -> Option<u64> {
    image::image_dimensions(path)
        .ok()
        .map(|(width, height)| (width as u64).saturating_mul(height as u64))
}

fn float_ord_key(value: f64) -> u64 {
    if value.is_finite() && value >= 0.0 {
        (value * 1000.0).round() as u64
    } else {
        u64::MAX
    }
}

fn compare_image_sort_entries(
    left: &CachedImageSortEntry,
    right: &CachedImageSortEntry,
) -> std::cmp::Ordering {
    right
        .relative_depth
        .cmp(&left.relative_depth)
        .then_with(|| left.format_priority.cmp(&right.format_priority))
        .then_with(|| left.size.cmp(&right.size))
        .then_with(|| {
            left.pixel_count
                .unwrap_or(u64::MAX)
                .cmp(&right.pixel_count.unwrap_or(u64::MAX))
        })
        .then_with(|| left.path.cmp(&right.path))
}

fn sort_cached_image_entries(entries: &mut [CachedImageSortEntry]) {
    entries.sort_by(compare_image_sort_entries);
}

fn build_cached_image_entry(root: &Path, path: &Path) -> Option<CachedImageSortEntry> {
    let metadata = fs::metadata(path).ok()?;
    Some(CachedImageSortEntry {
        path: path.to_path_buf(),
        size: metadata.len(),
        relative_depth: relative_depth_from_root(root, path),
        format_priority: format_priority_for_image(path),
        pixel_count: image_pixel_count(path),
    })
}

fn project_cache_dir() -> io::Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    dir.push(".cache");
    dir.push(PATH_TREE_CACHE_DIR);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn path_tree_cache_file(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
    media_kind: &str,
) -> io::Result<PathBuf> {
    let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut input = canonical_dir.to_string_lossy().into_owned();
    input.push('|');
    input.push_str(media_kind);
    input.push('|');
    input.push_str(if recursive { "recursive" } else { "flat" });
    input.push('|');
    input.push_str(&normalized_extensions(extensions).join(","));
    let file_name = format!("{}.json", blake3::hash(input.as_bytes()).to_hex());
    Ok(project_cache_dir()?.join(file_name))
}

fn load_cached_image_tree(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> Option<CachedImageTreeSnapshot> {
    let cache_file = path_tree_cache_file(dir, extensions, recursive, "image").ok()?;
    let content = fs::read_to_string(cache_file).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_cached_image_tree(snapshot: &CachedImageTreeSnapshot) -> io::Result<()> {
    let cache_file = path_tree_cache_file(
        &snapshot.root,
        &snapshot.extensions_as_refs(),
        snapshot.recursive,
        "image",
    )?;
    let content = serde_json::to_string_pretty(snapshot)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    fs::write(cache_file, content)
}

fn load_cached_video_tree(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> Option<CachedVideoTreeSnapshot> {
    let cache_file = path_tree_cache_file(dir, extensions, recursive, "video").ok()?;
    let content = fs::read_to_string(cache_file).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_cached_video_tree(snapshot: &CachedVideoTreeSnapshot) -> io::Result<()> {
    let cache_file = path_tree_cache_file(
        &snapshot.root,
        &snapshot.extensions_as_refs(),
        snapshot.recursive,
        "video",
    )?;
    let content = serde_json::to_string_pretty(snapshot)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    fs::write(cache_file, content)
}

fn validate_cached_image_tree(
    snapshot: &CachedImageTreeSnapshot,
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> bool {
    if snapshot.schema_version != PATH_TREE_CACHE_SCHEMA_VERSION {
        return false;
    }

    let expected_root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
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

fn validate_cached_video_tree(
    snapshot: &CachedVideoTreeSnapshot,
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> bool {
    if snapshot.schema_version != PATH_TREE_CACHE_SCHEMA_VERSION {
        return false;
    }

    let expected_root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
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

fn scan_image_tree_snapshot(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> CachedImageTreeSnapshot {
    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let walker = if recursive {
        WalkDir::new(&root).follow_links(true)
    } else {
        WalkDir::new(&root).max_depth(1)
    };

    let mut directories = Vec::new();
    let mut files = Vec::new();

    for entry in walker.into_iter() {
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

                if entry.file_type().is_file()
                    && crate::common_utils::has_extension(entry.path(), extensions)
                {
                    if let Some(file_entry) = build_cached_image_entry(&root, entry.path()) {
                        files.push(file_entry);
                    }
                }
            }
            Err(err) => {
                warn!(
                    dir = %root.display(),
                    error = %err,
                    "Failed to inspect directory entry while building path-tree cache"
                );
            }
        }
    }

    sort_cached_image_entries(&mut files);

    debug!(
        path = %root.display(),
        file_count = files.len(),
        dir_count = directories.len(),
        "Path-tree snapshot refreshed"
    );

    CachedImageTreeSnapshot {
        schema_version: PATH_TREE_CACHE_SCHEMA_VERSION,
        root,
        recursive,
        extensions: normalized_extensions(extensions),
        directories,
        files,
    }
}

fn video_probe_priority_data(path: &Path) -> (Option<u64>, Option<f64>, Option<f64>, Option<u64>) {
    let probe = match crate::probe_video(path) {
        Ok(probe) => probe,
        Err(_) => return (None, None, None, None),
    };

    let pixel_count = if probe.width > 0 && probe.height > 0 {
        Some((probe.width as u64).saturating_mul(probe.height as u64))
    } else {
        None
    };

    let duration_secs = if probe.duration.is_finite() && probe.duration > 0.0 {
        Some(probe.duration)
    } else {
        None
    };

    let frame_rate = if probe.frame_rate.is_finite() && probe.frame_rate > 0.0 {
        Some(probe.frame_rate)
    } else {
        None
    };

    let frame_count = if probe.frame_count > 0 {
        Some(probe.frame_count)
    } else if let (Some(duration), Some(fps)) = (duration_secs, frame_rate) {
        Some((duration * fps).round().max(1.0) as u64)
    } else {
        None
    };

    let estimated_work = pixel_count
        .zip(frame_count)
        .map(|(pixels, frames)| pixels.saturating_mul(frames.max(1)));

    (pixel_count, duration_secs, frame_rate, estimated_work)
}

fn compare_video_sort_entries(
    left: &CachedVideoSortEntry,
    right: &CachedVideoSortEntry,
) -> std::cmp::Ordering {
    right
        .relative_depth
        .cmp(&left.relative_depth)
        .then_with(|| {
            left.estimated_work
                .unwrap_or(u64::MAX)
                .cmp(&right.estimated_work.unwrap_or(u64::MAX))
        })
        .then_with(|| {
            left.duration_secs
                .map(float_ord_key)
                .unwrap_or(u64::MAX)
                .cmp(&right.duration_secs.map(float_ord_key).unwrap_or(u64::MAX))
        })
        .then_with(|| left.size.cmp(&right.size))
        .then_with(|| {
            left.pixel_count
                .unwrap_or(u64::MAX)
                .cmp(&right.pixel_count.unwrap_or(u64::MAX))
        })
        .then_with(|| {
            left.frame_rate
                .map(float_ord_key)
                .unwrap_or(u64::MAX)
                .cmp(&right.frame_rate.map(float_ord_key).unwrap_or(u64::MAX))
        })
        .then_with(|| left.path.cmp(&right.path))
}

fn sort_cached_video_entries(entries: &mut [CachedVideoSortEntry]) {
    entries.sort_by(compare_video_sort_entries);
}

fn build_cached_video_entry(root: &Path, path: &Path) -> Option<CachedVideoSortEntry> {
    let metadata = fs::metadata(path).ok()?;
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

fn scan_video_tree_snapshot(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
) -> CachedVideoTreeSnapshot {
    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let walker = if recursive {
        WalkDir::new(&root).follow_links(true)
    } else {
        WalkDir::new(&root).max_depth(1)
    };

    let mut directories = Vec::new();
    let mut files = Vec::new();

    for entry in walker.into_iter() {
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

                if entry.file_type().is_file()
                    && crate::common_utils::has_extension(entry.path(), extensions)
                {
                    if let Some(file_entry) = build_cached_video_entry(&root, entry.path()) {
                        files.push(file_entry);
                    }
                }
            }
            Err(err) => {
                warn!(
                    dir = %root.display(),
                    error = %err,
                    "Failed to inspect directory entry while building video path-tree cache"
                );
            }
        }
    }

    sort_cached_video_entries(&mut files);

    debug!(
        path = %root.display(),
        file_count = files.len(),
        dir_count = directories.len(),
        "Video path-tree snapshot refreshed"
    );

    CachedVideoTreeSnapshot {
        schema_version: PATH_TREE_CACHE_SCHEMA_VERSION,
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
        image.save_with_format(path, format).unwrap();
    }

    #[test]
    fn test_batch_result_new() {
        let result = BatchResult::new();
        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_batch_result_success() {
        let mut result = BatchResult::new();
        result.success();

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn test_batch_result_fail() {
        let mut result = BatchResult::new();
        result.fail(PathBuf::from("test.png"), "Error message".to_string());

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].1, "Error message");
    }

    #[test]
    fn test_batch_result_skip() {
        let mut result = BatchResult::new();
        result.skip();

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_batch_result_mixed() {
        let mut result = BatchResult::new();
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
        let result = BatchResult::new();
        assert!(
            (result.success_rate() - 100.0).abs() < 0.01,
            "Empty batch should have 100% success rate"
        );
    }

    #[test]
    fn test_success_rate_all_success() {
        let mut result = BatchResult::new();
        for _ in 0..10 {
            result.success();
        }
        assert!(
            (result.success_rate() - 100.0).abs() < 0.01,
            "All success should be 100%"
        );
    }

    #[test]
    fn test_success_rate_all_fail() {
        let mut result = BatchResult::new();
        for i in 0..10 {
            result.fail(PathBuf::from(format!("file{}.png", i)), "Error".to_string());
        }
        assert!(
            (result.success_rate() - 0.0).abs() < 0.01,
            "All fail should be 0%"
        );
    }

    #[test]
    fn test_success_rate_50_percent() {
        let mut result = BatchResult::new();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());

        assert!(
            (result.success_rate() - 50.0).abs() < 0.01,
            "1 success, 1 fail should be 50%, got {}",
            result.success_rate()
        );
    }

    #[test]
    fn test_success_rate_with_skipped() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.skip();
        result.skip();

        assert!(
            (result.success_rate() - 50.0).abs() < 0.01,
            "2 success, 2 skipped should be 50%, got {}",
            result.success_rate()
        );
    }

    #[test]
    fn test_strict_success_rate_formula() {
        let test_cases = [
            (10, 0, 0, 100.0),
            (5, 5, 0, 50.0),
            (3, 1, 0, 75.0),
            (1, 3, 0, 25.0),
            (0, 10, 0, 0.0),
            (7, 2, 1, 70.0),
        ];

        for (success, fail, skip, expected) in test_cases {
            let mut result = BatchResult::new();
            for _ in 0..success {
                result.success();
            }
            for i in 0..fail {
                result.fail(PathBuf::from(format!("f{}.png", i)), "E".to_string());
            }
            for _ in 0..skip {
                result.skip();
            }

            let rate = result.success_rate();
            let expected_calc = if result.total == 0 {
                100.0
            } else {
                (result.succeeded as f64 / result.total as f64) * 100.0
            };

            assert!(
                (rate - expected).abs() < 0.001,
                "STRICT: {}s/{}f/{}k expected {}%, got {}%",
                success,
                fail,
                skip,
                expected,
                rate
            );
            assert!(
                (rate - expected_calc).abs() < 0.0001,
                "STRICT: Formula mismatch"
            );
        }
    }

    #[test]
    fn test_strict_large_numbers() {
        let mut result = BatchResult::new();

        for _ in 0..500_000 {
            result.success();
        }
        for i in 0..500_000 {
            result.fail(PathBuf::from(format!("f{}.png", i)), "E".to_string());
        }

        assert_eq!(result.total, 1_000_000);
        assert!(
            (result.success_rate() - 50.0).abs() < 0.001,
            "STRICT: Large batch should calculate correctly"
        );
    }

    #[test]
    fn test_consistency_success_rate() {
        let mut result = BatchResult::new();
        result.success();
        result.success();
        result.fail(PathBuf::from("test.png"), "Error".to_string());

        let rate1 = result.success_rate();
        let rate2 = result.success_rate();
        let rate3 = result.success_rate();

        assert!((rate1 - rate2).abs() < 0.0000001);
        assert!((rate2 - rate3).abs() < 0.0000001);
    }

    #[test]
    fn test_total_equals_sum() {
        let mut result = BatchResult::new();
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
        let mut result = BatchResult::new();
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
        let controller = BatchPauseController::new();

        assert!(controller.request_pause(Path::new("first.png"), "first"));
        assert!(!controller.request_pause(Path::new("second.png"), "second"));

        let info = controller.pause_info().expect("pause info should exist");
        assert_eq!(info.path, PathBuf::from("first.png"));
        assert_eq!(info.reason, "first");
    }

    #[test]
    fn test_collect_image_files_for_perceived_speed_respects_priority_order() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let nested = root.join("nested");
        let deeper = nested.join("deeper");
        fs::create_dir_all(&deeper).unwrap();

        let root_png = root.join("root.png");
        let nested_jpg = nested.join("nested.jpg");
        let deeper_png = deeper.join("deeper.png");
        let deeper_jpg = deeper.join("deeper.jpg");

        write_test_image(&root_png, 32, 32, ImageFormat::Png);
        write_test_image(&nested_jpg, 48, 48, ImageFormat::Jpeg);
        write_test_image(&deeper_png, 24, 24, ImageFormat::Png);
        write_test_image(&deeper_jpg, 12, 12, ImageFormat::Jpeg);

        let files = collect_image_files_for_perceived_speed(root, &["png", "jpg"], true);
        let ordered_names = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
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
    }

    #[test]
    fn test_validate_cached_image_tree_detects_directory_changes() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();

        let image_path = nested.join("sample.jpg");
        write_test_image(&image_path, 16, 16, ImageFormat::Jpeg);

        let snapshot = scan_image_tree_snapshot(root, &["jpg"], true);
        assert!(validate_cached_image_tree(&snapshot, root, &["jpg"], true));

        let bumped = FileTime::from_unix_time(path_modified_unix_secs(&nested) as i64 + 10, 0);
        filetime::set_file_mtime(&nested, bumped).unwrap();

        assert!(
            !validate_cached_image_tree(&snapshot, root, &["jpg"], true),
            "Directory mtime drift should invalidate the cached path tree"
        );
    }

    #[test]
    fn test_video_sort_entries_prioritize_depth_then_size_then_resolution() {
        let fast_finish = CachedVideoSortEntry {
            path: PathBuf::from("a/deeper-fast.mov"),
            size: 160,
            relative_depth: 2,
            pixel_count: Some(640 * 360),
            duration_secs: Some(4.0),
            frame_rate: Some(24.0),
            estimated_work: Some(640 * 360 * 96),
        };
        let shallower = CachedVideoSortEntry {
            path: PathBuf::from("b/shallower-large.mov"),
            size: 500,
            relative_depth: 1,
            pixel_count: Some(320 * 240),
            duration_secs: Some(2.0),
            frame_rate: Some(24.0),
            estimated_work: Some(320 * 240 * 48),
        };
        let same_depth_shorter = CachedVideoSortEntry {
            path: PathBuf::from("c/tie-depth-shorter.mov"),
            size: 220,
            relative_depth: 2,
            pixel_count: Some(1280 * 720),
            duration_secs: Some(2.0),
            frame_rate: Some(24.0),
            estimated_work: Some(1280 * 720 * 48),
        };
        let same_depth_heavier = CachedVideoSortEntry {
            path: PathBuf::from("d/tie-depth-heavier.mov"),
            size: 80,
            relative_depth: 2,
            pixel_count: Some(1920 * 1080),
            duration_secs: Some(6.0),
            frame_rate: Some(60.0),
            estimated_work: Some(1920 * 1080 * 360),
        };

        let mut entries = vec![
            shallower.clone(),
            same_depth_heavier.clone(),
            fast_finish.clone(),
            same_depth_shorter.clone(),
        ];
        sort_cached_video_entries(&mut entries);

        assert_eq!(entries[0].path, fast_finish.path);
        assert_eq!(entries[1].path, same_depth_shorter.path);
        assert_eq!(entries[2].path, same_depth_heavier.path);
        assert_eq!(entries[3].path, shallower.path);
    }
}
