//! File Copier Module
//!
//! Ensures the output directory contains all files by copying unsupported formats
//! while skipping converted files and merged XMP sidecars.

use crate::quality_matcher::SourceCodec;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};
use walkdir::WalkDir;

pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = SourceCodec::supported_image_extensions();
pub const IMAGE_EXTENSIONS_FOR_CONVERT: &[&str] = SourceCodec::image_extensions_for_convert();
pub const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = SourceCodec::supported_video_extensions();

pub const IMAGE_EXTENSIONS_ANALYZE: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif",
];

pub const SIDECAR_EXTENSIONS: &[&str] = &["xmp"];

#[derive(Debug, Clone)]
pub struct CopyResult {
    pub total_files: usize,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub errors: Vec<(PathBuf, String, String)>,
}

impl CopyResult {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total_files: 0,
            copied: 0,
            skipped: 0,
            failed: 0,
            errors: Vec::new(),
        }
    }
}

impl Default for CopyResult {
    fn default() -> Self {
        Self::new()
    }
}

fn should_copy_file(path: &Path) -> bool {
    let Some(name) =
        crate::media_conversion_gate::path_file_name_utf8_or_none(path, "file_copier_filter")
    else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }

    let ext = crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path);

    !SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str())
        && !SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str())
        && !SIDECAR_EXTENSIONS.contains(&ext.as_str())
}

fn build_copy_walker(input_dir: &Path, recursive: bool) -> WalkDir {
    if recursive {
        WalkDir::new(input_dir).follow_links(true)
    } else {
        WalkDir::new(input_dir).max_depth(1)
    }
}

fn prescan_copy_candidates(input_dir: &Path, recursive: bool) -> usize {
    let mut total_files = 0usize;
    for entry in build_copy_walker(input_dir, recursive) {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_file() && should_copy_file(entry.path()) {
                    total_files += 1;
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_io_path_audit(
                    "delivery_io_copy",
                    input_dir,
                    format!(
                        "COPY AUDIT: Failed to inspect directory entry during pre-scan | Forensic: Directory '{}', Error '{}'",
                        input_dir.display(),
                        err
                    ),
                );
            }
        }
    }
    total_files
}

fn push_copy_error(result: &mut CopyResult, path: &Path, error_msg: String, category: &str) {
    result.failed += 1;
    result
        .errors
        .push((path.to_path_buf(), error_msg, category.to_string()));
}

fn record_walkdir_failure(input_dir: &Path, err: &walkdir::Error, result: &mut CopyResult) {
    let path = match err.path() {
        None => input_dir.to_path_buf(),
        Some(p) => p.to_path_buf(),
    };
    let error_msg = format!("Directory traversal failed: {err}");
    crate::media_conversion_gate::delivery_io_path_audit(
        "delivery_io_copy",
        &path,
        format!(
            "COPY AUDIT: Directory traversal failed during batch copy | Forensic: Path '{}', Error '{err}'",
            path.display(),
        ),
    );
    push_copy_error(result, &path, error_msg, "walkdir");
}

fn destination_matches_source_size(path: &Path, dest: &Path) -> Result<bool, String> {
    match dest.try_exists() {
        Ok(false) => return Ok(false),
        Ok(true) => {}
        Err(err) => {
            return Err(format!(
                "failed to inspect destination existence for {}: {err}",
                dest.display()
            ));
        }
    }
    let src_meta = std::fs::metadata(path).map_err(|err| {
        format!(
            "failed to read source metadata for {}: {err}",
            path.display()
        )
    })?;
    let dst_meta = std::fs::metadata(dest).map_err(|err| {
        format!(
            "failed to read destination metadata for {}: {err}",
            dest.display()
        )
    })?;
    Ok(src_meta.len() == dst_meta.len())
}

fn resolve_copy_destination(path: &Path, input_dir: &Path, output_dir: &Path) -> Option<PathBuf> {
    match path.strip_prefix(input_dir) {
        Ok(rel_path) => Some(output_dir.join(rel_path)),
        Err(err) => {
            let error_msg = format!("Failed to compute relative path: {err}");
            error!(
                file = %path.display(),
                input_dir = %input_dir.display(),
                error = %err,
                "Path computation failed"
            );
            crate::media_conversion_gate::delivery_io_path_audit(
                "delivery_io_copy",
                path,
                format!("Path error for {}: {}", path.display(), error_msg),
            );
            None
        }
    }
}

fn record_relative_path_failure(path: &Path, input_dir: &Path, result: &mut CopyResult) {
    let error_msg = format!(
        "Failed to compute relative path for '{}' against '{}'",
        path.display(),
        input_dir.display()
    );
    push_copy_error(result, path, error_msg, "compute_path");
}

fn ensure_destination_parent(path: &Path, dest: &Path, result: &mut CopyResult) -> bool {
    if let Some(parent) = dest.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        let error_msg = format!("Failed to create directory: {err}");
        error!(
            file = %path.display(),
            dest_dir = %parent.display(),
            error = %err,
            "Directory creation failed"
        );
        crate::media_conversion_gate::delivery_io_path_audit(
            "delivery_io_copy",
            path,
            format!(
                "Failed to create directory for {}: {}",
                path.display(),
                error_msg
            ),
        );
        push_copy_error(result, path, error_msg, "create_dir");
        return false;
    }
    true
}

fn handle_copied_file_success(path: &Path, dest: &Path, result: &mut CopyResult) {
    // Bytes are already on disk via std::fs::copy in copy_candidate_file.
    // Do not call metadata::copy() here — it merges XMP and re-applies timestamps, and
    // handle_copied_file_xmp() would repeat both (duplicate audit noise on .psd/.pdf).
    if let Err(e) = crate::metadata::preserve(path, dest) {
        let error_msg = format!(
            "Copied file but failed to preserve metadata from {} to {}: {}",
            path.display(),
            dest.display(),
            e
        );
        crate::media_conversion_gate::delivery_io_batch_audit("delivery_io_copy", &error_msg);
        push_copy_error(result, path, error_msg, "preserve_metadata");
        return;
    }

    handle_copied_file_xmp(path, dest);

    if let Err(e) = crate::metadata::apply_file_timestamps(path, dest) {
        let error_msg = format!(
            "Copied file but failed to synchronize timestamps from {} to {}: {}",
            path.display(),
            dest.display(),
            e
        );
        crate::media_conversion_gate::delivery_io_batch_audit("delivery_io_copy", &error_msg);
        push_copy_error(result, path, error_msg, "preserve_metadata");
        return;
    }

    result.copied += 1;
    let ext = crate::media_conversion_gate::path_extension_label(path);

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_COPY,
        &format!("Copied unsupported file (.{}): {}", ext, path.display())
    );

    debug!(
        source = %path.display(),
        dest = %dest.display(),
        extension = ext,
        "File copied successfully"
    );
}

fn handle_copied_file_xmp(path: &Path, dest: &Path) {
    match crate::merge_xmp_for_copied_file(path, dest) {
        Ok(true) => {
            debug!(file = %path.display(), "XMP merged successfully");
        }
        Ok(false) => {
            debug!(file = %path.display(), "No XMP sidecar found");
        }
        Err(err) => {
            crate::media_conversion_gate::delivery_io_path_audit(
                "delivery_io_copy",
                path,
                format!(
                    "XMP AUDIT: XMP merge failed, trying to copy sidecar as fallback | Forensic: File '{}', Error '{}'",
                    path.display(),
                    err
                ),
            );
            crate::media_conversion_gate::delivery_io_batch_audit(
                "delivery_io_copy",
                format!("XMP merge failed ({err}), trying to copy sidecar..."),
            );
            copy_xmp_sidecar_if_exists(path, dest);
        }
    }
}

fn record_copy_failure(path: &Path, dest: &Path, err: &std::io::Error, result: &mut CopyResult) {
    let error_msg = format!("Copy failed: {err}");
    error!(
        source = %path.display(),
        dest = %dest.display(),
        error = %err,
        error_kind = ?err.kind(),
        "File copy operation failed"
    );
    crate::media_conversion_gate::delivery_io_path_audit(
        "delivery_io_copy",
        path,
        format!("Failed to copy {}: {}", path.display(), err),
    );
    push_copy_error(result, path, error_msg, "copy_file");
}

fn copy_candidate_file(path: &Path, input_dir: &Path, output_dir: &Path, result: &mut CopyResult) {
    let Some(dest) = resolve_copy_destination(path, input_dir, output_dir) else {
        record_relative_path_failure(path, input_dir, result);
        return;
    };

    match destination_matches_source_size(path, &dest) {
        Ok(true) => {
            debug!(
                file = %path.display(),
                "Skipping unsupported file copy (already exists in destination with matching size)"
            );
            result.skipped += 1;
            return;
        }
        Ok(false) => {}
        Err(err) => {
            let error_msg =
                format!("[ERROR] Metadata comparison failed before unsupported file copy: {err}");
            crate::media_conversion_gate::delivery_io_path_audit(
                "delivery_io_copy",
                path,
                error_msg.clone(),
            );
            push_copy_error(result, path, error_msg, "metadata_compare");
            return;
        }
    }

    if !ensure_destination_parent(path, &dest, result) {
        return;
    }

    match std::fs::copy(path, &dest) {
        Ok(_) => {
            handle_copied_file_success(path, &dest, result);
        }
        Err(err) => {
            record_copy_failure(path, &dest, &err, result);
        }
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
pub fn copy_unsupported_files(input_dir: &Path, output_dir: &Path, recursive: bool) -> CopyResult {
    let mut result = CopyResult::new();

    info!(
        input_dir = %input_dir.display(),
        output_dir = %output_dir.display(),
        recursive = recursive,
        "Starting batch file copy operation"
    );

    let total_files = prescan_copy_candidates(input_dir, recursive);

    debug!(total_files = total_files, "Pre-scan completed");

    for entry in build_copy_walker(input_dir, recursive) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                record_walkdir_failure(input_dir, &err, &mut result);
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        result.total_files += 1;

        if !should_copy_file(path) {
            result.skipped += 1;
            continue;
        }
        copy_candidate_file(path, input_dir, output_dir, &mut result);
    }

    info!(
        total = result.total_files,
        copied = result.copied,
        skipped = result.skipped,
        failed = result.failed,
        "Batch file copy operation completed"
    );

    if result.failed > 0 {
        crate::media_conversion_gate::delivery_io_batch_audit(
            "delivery_io_copy",
            format!(
                "COPY AUDIT: Some files failed to copy during batch operation | Forensic: FailedCount={}",
                result.failed
            ),
        );
        crate::media_conversion_gate::delivery_io_batch_audit(
            "delivery_io_copy",
            format!(
                "Batch copy completed with {} failures out of {} files",
                result.failed, result.total_files
            ),
        );
    }

    result
}

fn copy_xmp_sidecar_if_exists(source: &Path, dest: &Path) {
    let source_str = source.to_string_lossy();
    let dest_str = dest.to_string_lossy();

    let xmp_patterns = [
        format!("{source_str}.xmp"),
        format!("{source_str}.XMP"),
        source.with_extension("xmp").to_string_lossy().to_string(),
    ];

    for xmp_source in &xmp_patterns {
        let xmp_path = Path::new(xmp_source);
        if xmp_path.exists() {
            let xmp_dest = format!("{dest_str}.xmp");

            match std::fs::copy(xmp_path, &xmp_dest) {
                Ok(_) => match crate::copy(xmp_path, Path::new(&xmp_dest)) {
                    Ok(()) => {
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_XMP,
                            &format!("Copied XMP sidecar: {}", xmp_path.display())
                        );

                        debug!(
                            source = %xmp_path.display(),
                            dest = %xmp_dest,
                            "XMP sidecar copied successfully"
                        );
                    }
                    Err(e) => {
                        crate::media_conversion_gate::delivery_io_path_audit(
                            "delivery_io_copy",
                            xmp_path,
                            format!(
                                "Copied XMP sidecar bytes but failed to preserve metadata {} -> {}: {e}",
                                xmp_path.display(),
                                xmp_dest,
                            ),
                        );
                    }
                },
                Err(e) => {
                    error!(
                        source = %xmp_path.display(),
                        dest = %xmp_dest,
                        error = %e,
                        error_kind = ?e.kind(),
                        "Failed to copy XMP sidecar"
                    );
                    crate::media_conversion_gate::delivery_io_path_audit(
                        "delivery_io_copy",
                        xmp_path,
                        format!("Failed to copy XMP sidecar {}: {e}", xmp_path.display()),
                    );
                }
            }
            return;
        }
    }

    debug!(
        source = %source.display(),
        "No XMP sidecar found for file"
    );
}

#[derive(Debug, Clone)]
pub struct FileStats {
    pub total: usize,
    pub images: usize,
    pub videos: usize,
    pub sidecars: usize,
    pub others: usize,
}

impl FileStats {
    #[must_use]
    pub const fn expected_output(&self) -> usize {
        self.total - self.sidecars
    }
}

#[must_use]
pub fn count_files(dir: &Path, recursive: bool) -> FileStats {
    let mut stats = FileStats {
        total: 0,
        images: 0,
        videos: 0,
        sidecars: 0,
        others: 0,
    };

    let walker = if recursive {
        WalkDir::new(dir).follow_links(true)
    } else {
        WalkDir::new(dir).max_depth(1)
    };

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                crate::media_conversion_gate::delivery_io_batch_audit(
                    "delivery_io_copy",
                    format!(
                        "COPY AUDIT: Failed to inspect directory entry while counting files | Forensic: Directory '{}', Error '{}'",
                        dir.display(),
                        err
                    ),
                );
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }

        stats.total += 1;

        let ext = crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path);

        if SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            stats.images += 1;
        } else if SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            stats.videos += 1;
        } else if SIDECAR_EXTENSIONS.contains(&ext.as_str()) {
            stats.sidecars += 1;
        } else {
            stats.others += 1;
        }
    }

    stats
}

#[derive(Debug)]
pub struct VerifyResult {
    pub passed: bool,
    pub expected: usize,
    pub actual: usize,
    pub diff: i64,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyDomain {
    All,
    ImagesAndPassthrough,
    VideosAndPassthrough,
}

#[must_use]
pub fn verify_output_completeness(
    input_dir: &Path,
    output_dir: &Path,
    recursive: bool,
) -> VerifyResult {
    verify_output_completeness_for_domain(input_dir, output_dir, recursive, VerifyDomain::All)
}

#[must_use]
pub fn verify_output_completeness_for_domain(
    input_dir: &Path,
    output_dir: &Path,
    recursive: bool,
    domain: VerifyDomain,
) -> VerifyResult {
    let input_stats = count_files(input_dir, recursive);
    let output_stats = count_files(output_dir, recursive);

    let expected = match domain {
        VerifyDomain::All => input_stats.expected_output(),
        VerifyDomain::ImagesAndPassthrough => input_stats.images + input_stats.others,
        VerifyDomain::VideosAndPassthrough => input_stats.videos + input_stats.others,
    };
    // Compare like-for-like: do not treat output sidecars/videos as "extra" when the
    // domain only expects images + passthrough files (matches Rust verify integrity scope).
    let actual = match domain {
        VerifyDomain::All => output_stats.expected_output(),
        VerifyDomain::ImagesAndPassthrough => output_stats.images + output_stats.others,
        VerifyDomain::VideosAndPassthrough => output_stats.videos + output_stats.others,
    };
    let diff = crate::numeric_cast::usize_to_i64_sat(expected)
        - crate::numeric_cast::usize_to_i64_sat(actual);

    let ok = crate::media_conversion_gate::ui_icon_pick(
        crate::modern_ui::symbols::SUCCESS,
        crate::modern_ui::symbols::plain::SUCCESS,
    );
    let err = crate::media_conversion_gate::ui_icon_pick(
        crate::modern_ui::symbols::ERROR,
        crate::modern_ui::symbols::plain::ERROR,
    );
    let warn = crate::media_conversion_gate::ui_icon_pick(
        crate::modern_ui::symbols::WARNING,
        crate::modern_ui::symbols::plain::WARNING,
    );
    let (passed, message) = match diff.cmp(&0) {
        std::cmp::Ordering::Equal => (
            true,
            format!("{ok} Verification passed: {actual} files (no loss)"),
        ),
        std::cmp::Ordering::Greater => (
            false,
            format!(
                "{err} Verification FAILED: missing {diff} files! (expected {expected}, got {actual})"
            ),
        ),
        std::cmp::Ordering::Less => (
            true,
            format!(
                "{warn} Output has {} extra files (expected {}, got {})",
                -diff, expected, actual
            ),
        ),
    };

    VerifyResult {
        passed,
        expected,
        actual,
        diff,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(path: &Path) {
        std::fs::write(path, b"test").unwrap_or_else(|e| {
            panic!("failed to write {}: {e}", path.display());
        });
    }

    #[test]
    fn test_should_copy_file() {
        assert!(!should_copy_file(Path::new("test.jpg")));
        assert!(!should_copy_file(Path::new("test.PNG")));
        assert!(!should_copy_file(Path::new("test.mp4")));

        assert!(!should_copy_file(Path::new("test.xmp")));

        assert!(should_copy_file(Path::new("test.psd")));
        assert!(should_copy_file(Path::new("test.txt")));
        assert!(should_copy_file(Path::new("test.pdf")));

        assert!(!should_copy_file(Path::new(".DS_Store")));
    }

    #[test]
    fn test_verify_output_completeness_for_image_domain_excludes_videos() {
        let input = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let output = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));

        touch(&input.path().join("photo.jpg"));
        touch(&input.path().join("clip.mp4"));
        touch(&output.path().join("photo.jxl"));

        let verify = verify_output_completeness_for_domain(
            input.path(),
            output.path(),
            false,
            VerifyDomain::ImagesAndPassthrough,
        );

        assert!(verify.passed, "{}", verify.message);
        assert_eq!(verify.expected, 1);
        assert_eq!(verify.actual, 1);
    }

    #[test]
    fn test_verify_output_completeness_for_video_domain_excludes_images() {
        let input = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let output = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));

        touch(&input.path().join("photo.jpg"));
        touch(&input.path().join("clip.mp4"));
        touch(&output.path().join("clip.mp4"));

        let verify = verify_output_completeness_for_domain(
            input.path(),
            output.path(),
            false,
            VerifyDomain::VideosAndPassthrough,
        );

        assert!(verify.passed, "{}", verify.message);
        assert_eq!(verify.expected, 1);
        assert_eq!(verify.actual, 1);
    }
}
