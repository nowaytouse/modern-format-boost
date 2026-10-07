//! File Copier Module
//!
//! Ensures the output directory contains all files by copying unsupported
//! formats while skipping converted files and merged XMP sidecars.

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
    /// Eligible passthrough candidates inspected by the copy pass.
    pub total_files: usize,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Inspected regular files excluded from the passthrough domain.
    pub excluded: usize,
    /// Directory traversal failures, separate from per-file copy failures.
    pub scan_errors: usize,
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
            excluded: 0,
            scan_errors: 0,
            errors: Vec::new(),
        }
    }

    #[must_use]
    pub const fn has_errors(&self) -> bool {
        self.failed > 0 || self.scan_errors > 0
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
            "COPY AUDIT: Directory traversal failed during batch copy | Forensic: Path '{}', \
             Error '{err}'",
            path.display(),
        ),
    );
    result.scan_errors += 1;
    result.errors.push((path, error_msg, "walkdir".to_string()));
}

fn destination_matches_source(path: &Path, dest: &Path) -> Result<bool, String> {
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
    if src_meta.len() != dst_meta.len() {
        return Ok(false);
    }
    let source_hash = crate::common_utils::calculate_blake3_hash(path)
        .map_err(|err| format!("failed to hash source {}: {err}", path.display()))?;
    let destination_hash = crate::common_utils::calculate_blake3_hash(dest)
        .map_err(|err| format!("failed to hash destination {}: {err}", dest.display()))?;
    Ok(source_hash == destination_hash)
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
    // Do not call metadata::copy() here — it merges XMP and re-applies timestamps,
    // and handle_copied_file_xmp() would repeat both (duplicate audit noise on
    // .psd/.pdf).
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

    if let Err(error_msg) = handle_copied_file_xmp(path, dest) {
        push_copy_error(result, path, error_msg, "preserve_xmp");
        return;
    }

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

fn handle_copied_file_xmp(path: &Path, dest: &Path) -> Result<(), String> {
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
                    "XMP AUDIT: XMP merge failed, trying to copy sidecar as fallback | Forensic: \
                     File '{}', Error '{}'",
                    path.display(),
                    err
                ),
            );
            crate::media_conversion_gate::delivery_io_batch_audit(
                "delivery_io_copy",
                format!("XMP merge failed ({err}), trying to copy sidecar..."),
            );
            copy_xmp_sidecar_if_exists(path, dest).map_err(|fallback| {
                format!("XMP merge failed ({err}); sidecar preservation failed ({fallback})")
            })?;
        }
    }
    Ok(())
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

    match destination_matches_source(path, &dest) {
        Ok(true) => {
            debug!(
                file = %path.display(),
                "Skipping unsupported file copy (destination content matches source)"
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

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
/// Collect the paths that `copy_unsupported_files` would copy.
///
/// Files whose extension is outside the supported image/video/sidecar sets.
/// Used by the external-identity audit so the "unsupported" bucket can report
/// what those files actually are instead of only where they get copied.
#[must_use]
pub fn collect_unsupported_files(input_dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in build_copy_walker(input_dir, recursive) {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_file() && should_copy_file(entry.path()) {
                    paths.push(entry.into_path());
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_io_path_audit(
                    "delivery_io_copy",
                    input_dir,
                    format!(
                        "COPY AUDIT: Failed to inspect directory entry during unsupported scan | \
                         Forensic: Directory '{}', Error '{}'",
                        input_dir.display(),
                        err
                    ),
                );
            }
        }
    }
    paths
}

pub fn copy_unsupported_files(input_dir: &Path, output_dir: &Path, recursive: bool) -> CopyResult {
    let mut result = CopyResult::new();

    info!(
        input_dir = %input_dir.display(),
        output_dir = %output_dir.display(),
        recursive = recursive,
        "Starting batch file copy operation"
    );

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
        if !should_copy_file(path) {
            result.excluded += 1;
            continue;
        }
        result.total_files += 1;
        copy_candidate_file(path, input_dir, output_dir, &mut result);
    }

    info!(
        total = result.total_files,
        copied = result.copied,
        skipped = result.skipped,
        failed = result.failed,
        excluded = result.excluded,
        scan_errors = result.scan_errors,
        "Batch file copy operation completed"
    );

    if result.has_errors() {
        crate::media_conversion_gate::delivery_io_batch_audit(
            "delivery_io_copy",
            format!(
                "COPY AUDIT: Batch copy was incomplete | Forensic: FailedCount={}, \
                 ScanErrors={}",
                result.failed, result.scan_errors
            ),
        );
        crate::media_conversion_gate::delivery_io_batch_audit(
            "delivery_io_copy",
            format!(
                "Batch copy completed with {} file failures and {} scan errors among {} \
                 eligible candidates",
                result.failed, result.scan_errors, result.total_files
            ),
        );
    }

    result
}

fn copy_xmp_sidecar_if_exists(source: &Path, dest: &Path) -> Result<(), String> {
    let source_str = source.to_string_lossy();
    let dest_str = dest.to_string_lossy();

    let xmp_patterns = [
        format!("{source_str}.xmp"),
        format!("{source_str}.XMP"),
        source.with_extension("xmp").to_string_lossy().to_string(),
    ];

    for xmp_source in &xmp_patterns {
        let xmp_path = Path::new(xmp_source);
        if xmp_path.try_exists().map_err(|err| {
            format!(
                "Failed to inspect XMP sidecar {}: {err}",
                xmp_path.display()
            )
        })? {
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
                        return Ok(());
                    }
                    Err(e) => {
                        crate::media_conversion_gate::delivery_io_path_audit(
                            "delivery_io_copy",
                            xmp_path,
                            format!(
                                "Copied XMP sidecar bytes but failed to preserve metadata {} -> \
                                 {}: {e}",
                                xmp_path.display(),
                                xmp_dest,
                            ),
                        );
                        return Err(format!(
                            "Copied XMP sidecar bytes but failed to preserve metadata: {e}"
                        ));
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
                    return Err(format!(
                        "Failed to copy XMP sidecar {}: {e}",
                        xmp_path.display()
                    ));
                }
            }
        }
    }

    Err(format!(
        "No XMP sidecar available to preserve after merge failure: {}",
        source.display()
    ))
}

#[derive(Debug, Clone)]
pub struct FileStats {
    pub total: usize,
    pub images: usize,
    pub videos: usize,
    pub sidecars: usize,
    pub others: usize,
    pub scan_errors: usize,
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
        scan_errors: 0,
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
                stats.scan_errors += 1;
                crate::media_conversion_gate::delivery_io_batch_audit(
                    "delivery_io_copy",
                    format!(
                        "COPY AUDIT: Failed to inspect directory entry while counting files | \
                         Forensic: Directory '{}', Error '{}'",
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
    pub scan_errors: usize,
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

    // Compare like-for-like: do not treat output sidecars/videos as "extra" when
    // the domain only expects images + passthrough files (matches Rust verify
    // integrity scope).
    let expected = count_for_domain(&input_stats, domain);
    let actual = count_for_domain(&output_stats, domain);
    compare_output_counts(
        expected,
        actual,
        input_stats.scan_errors + output_stats.scan_errors,
    )
}

/// Compare an immutable input-side expectation with an output-only scan.
///
/// This is count evidence only; matching cardinalities do not establish file
/// identity or content equality.
#[must_use]
pub fn verify_output_count(
    expected: usize,
    output_dir: &Path,
    recursive: bool,
    domain: VerifyDomain,
) -> VerifyResult {
    let output_stats = count_files(output_dir, recursive);
    compare_output_counts(
        expected,
        count_for_domain(&output_stats, domain),
        output_stats.scan_errors,
    )
}

const fn count_for_domain(stats: &FileStats, domain: VerifyDomain) -> usize {
    match domain {
        VerifyDomain::All => stats.expected_output(),
        VerifyDomain::ImagesAndPassthrough => stats.images + stats.others,
        VerifyDomain::VideosAndPassthrough => stats.videos + stats.others,
    }
}

fn compare_output_counts(expected: usize, actual: usize, scan_errors: usize) -> VerifyResult {
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
    let (passed, message) = if scan_errors > 0 {
        (
            false,
            format!(
                "{err} Verification FAILED: incomplete file scan ({scan_errors} scan error(s)); \
                 observed {actual} output files against expected count {expected} (count-only \
                 evidence)"
            ),
        )
    } else {
        match expected.cmp(&actual) {
            std::cmp::Ordering::Equal => (
                true,
                format!(
                    "{ok} Count verification passed: expected and output counts match at \
                     {actual} files (count-only; identity and contents not verified)"
                ),
            ),
            std::cmp::Ordering::Greater => (
                false,
                format!(
                    "{err} Verification FAILED: missing {} files by count (expected \
                     {expected}, got {actual})",
                    expected - actual
                ),
            ),
            std::cmp::Ordering::Less => (
                true,
                format!(
                    "{warn} Output has {} extra files by count (expected {}, got {}; \
                     identity and contents not verified)",
                    actual - expected,
                    expected,
                    actual
                ),
            ),
        }
    };

    VerifyResult {
        passed,
        expected,
        actual,
        diff,
        scan_errors,
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

    #[test]
    fn xmp_fallback_requires_a_successfully_preserved_sidecar() {
        let input = TempDir::new().expect("input dir");
        let output = TempDir::new().expect("output dir");
        let source = input.path().join("notes.txt");
        let dest = output.path().join("notes.txt");
        assert!(copy_xmp_sidecar_if_exists(&source, &dest).is_err());
        std::fs::create_dir(input.path().join("notes.txt.xmp")).expect("invalid sidecar directory");
        let error = copy_xmp_sidecar_if_exists(&source, &dest).expect_err("sidecar copy must fail");
        assert!(error.contains("Failed to copy XMP sidecar"));
    }

    #[test]
    fn copy_result_counts_only_eligible_candidates_and_matching_existing_content() {
        let input = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let output = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        touch(&input.path().join("photo.jpg"));
        touch(&input.path().join("clip.mp4"));
        touch(&input.path().join("photo.xmp"));
        touch(&input.path().join(".hidden.txt"));
        let source = input.path().join("notes.txt");
        std::fs::write(&source, b"same content").unwrap_or_else(|e| panic!("write failed: {e}"));
        let destination = output.path().join("notes.txt");
        std::fs::write(&destination, b"same content")
            .unwrap_or_else(|e| panic!("write failed: {e}"));

        let result = copy_unsupported_files(input.path(), output.path(), false);

        assert_eq!(result.total_files, 1);
        assert_eq!(result.copied, 0);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.excluded, 4);
        assert_eq!(result.scan_errors, 0);
        assert_eq!(
            result.total_files,
            result.copied + result.skipped + result.failed
        );
        assert!(!result.has_errors());
    }

    #[test]
    fn same_size_different_content_is_copied_instead_of_skipped() {
        let input = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let output = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let source = input.path().join("notes.txt");
        let destination = output.path().join("notes.txt");
        std::fs::write(&source, b"source").unwrap_or_else(|e| panic!("write failed: {e}"));
        std::fs::write(&destination, b"target").unwrap_or_else(|e| panic!("write failed: {e}"));

        let result = copy_unsupported_files(input.path(), output.path(), false);

        assert_eq!(result.total_files, 1);
        assert_eq!(result.copied, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(
            std::fs::read(destination).unwrap_or_else(|e| panic!("read failed: {e}")),
            b"source"
        );
    }

    #[test]
    fn copy_failure_is_counted_once_as_failed() {
        let root = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let input = root.path().join("input");
        std::fs::create_dir(&input).unwrap_or_else(|e| panic!("mkdir failed: {e}"));
        let output = root.path().join("output-file");
        touch(&input.join("notes.txt"));
        touch(&output);

        let result = copy_unsupported_files(&input, &output, false);

        assert_eq!(result.total_files, 1);
        assert_eq!(result.copied, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(
            result.total_files,
            result.copied + result.skipped + result.failed
        );
        assert!(result.has_errors());
    }

    #[test]
    fn output_count_verification_fails_closed_on_scan_error() {
        let root = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let missing_output = root.path().join("missing-output");

        let verify = verify_output_count(1, &missing_output, false, VerifyDomain::All);

        assert!(!verify.passed);
        assert_eq!(verify.expected, 1);
        assert_eq!(verify.actual, 0);
        assert!(verify.scan_errors > 0);
    }

    #[test]
    fn immutable_expected_count_still_detects_delivery_missing_after_source_removal() {
        let root = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        let source = root.path().join("source.txt");
        let output = root.path().join("output");
        std::fs::create_dir(&output).unwrap_or_else(|e| panic!("mkdir failed: {e}"));
        touch(&source);
        let expected_before_source_removal = 1;
        std::fs::remove_file(&source).unwrap_or_else(|e| panic!("remove failed: {e}"));

        let verify = verify_output_count(
            expected_before_source_removal,
            &output,
            false,
            VerifyDomain::All,
        );

        assert!(!verify.passed);
        assert_eq!(verify.expected, 1);
        assert_eq!(verify.actual, 0);
        assert!(verify.message.contains("missing 1 files by count"));
    }

    #[test]
    fn output_count_verification_keeps_extra_output_warning_as_count_only() {
        let output = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
        touch(&output.path().join("first.txt"));
        touch(&output.path().join("second.txt"));

        let verify = verify_output_count(1, output.path(), false, VerifyDomain::All);

        assert!(verify.passed);
        assert_eq!(verify.diff, -1);
        assert!(verify.message.contains("extra files by count"));
        assert!(
            verify
                .message
                .contains("identity and contents not verified")
        );
    }
}
