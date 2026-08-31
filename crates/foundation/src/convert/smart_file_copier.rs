//! Smart File Copier - Unified File Copy Module
//!
//! Features:
//! - ✅ Preserves full directory structure
//! - ✅ Preserves file metadata (timestamps, permissions)
//! - ✅ Automatically merges XMP sidecar files
//! - ✅ Loud errors, fully transparent
//!
//! This module unifies file copy logic across all converters, avoiding code
//! duplication.
//!
//! ## Extension Correction & Validation Order
//! - `fix_extension_if_mismatch` corrects extension based on file magic bytes
//!   (prevents panics/misjudgment due to faked extensions).
//! - Design convention: **Fix first, then branch by extension**. All entry
//!   points (`cli_runner`, img_*) call `fix_extension` before processing. All
//!   subsequent "extension-only" logic should be based on the fixed path. See
//!   `CODE_AUDIT.md` §36.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Fix the extension of a file if it doesn't match its content.
///
/// # Errors
/// Returns an error if content analysis fails.
pub fn fix_extension_if_mismatch(path: &std::path::Path) -> Result<PathBuf> {
    use crate::quality_matcher::SourceCodec;

    let current_ext =
        crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path);

    if let Some(codec) = SourceCodec::identify_by_content(path)?
        && !codec.is_extension_compatible(&current_ext)
    {
        let content_format = codec.default_extension();
        let new_path = path.with_extension(content_format);

        if new_path.exists() {
            let src_meta = fs::metadata(path);
            let dst_meta = fs::metadata(&new_path);
            let same_file = match (src_meta, dst_meta) {
                #[cfg(unix)]
                (Ok(s), Ok(d)) => {
                    use std::os::unix::fs::MetadataExt;
                    s.ino() == d.ino() && s.dev() == d.dev()
                }
                _ => false,
            };
            if !same_file {
                crate::ui_stderr::line(
                    crate::modern_ui::symbols::WARNING,
                    crate::modern_ui::symbols::plain::WARNING,
                    format!(
                        "[Extension Fix] SKIPPED: {} -> .{} (target {} already exists)",
                        path.display(),
                        content_format,
                        new_path.display()
                    ),
                );
                return Ok(path.to_path_buf());
            }
        }

        crate::ui_stderr::line(
            crate::modern_ui::symbols::WARNING,
            crate::modern_ui::symbols::plain::WARNING,
            format!(
                "[Extension Fix] {} -> .{} (content does not match extension)",
                path.display(),
                content_format
            ),
        );

        fs::rename(path, &new_path).with_context(|| {
            format!(
                "Failed to rename {} to {}",
                path.display(),
                new_path.display()
            )
        })?;

        crate::ui_stderr::line(
            crate::modern_ui::symbols::SUCCESS,
            crate::modern_ui::symbols::plain::SUCCESS,
            format!("[Extension Fix] Complete: {}", new_path.display()),
        );

        return Ok(new_path);
    }

    Ok(path.to_path_buf())
}

/// Check if a file's extension mismatches its content, but do NOT rename.
/// Returns the path unchanged. Logs the mismatch for downstream awareness.
///
/// Use this variant when the source directory must remain immutable
/// (e.g., when an output directory is configured).
///
/// # Errors
/// Returns an error if content analysis fails.
pub fn check_extension_mismatch_readonly(path: &std::path::Path) -> Result<PathBuf> {
    use crate::quality_matcher::SourceCodec;

    let current_ext =
        crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path);

    if let Some(codec) = SourceCodec::identify_by_content(path)?
        && !codec.is_extension_compatible(&current_ext)
    {
        let content_format = codec.default_extension();
        crate::media_conversion_gate::delivery_runtime_path_audit(
            "delivery_runtime",
            path,
            format!(
                "{} has .{} extension but content is .{} (source directory immutable, not \
                 renaming)",
                path.display(),
                current_ext,
                content_format
            ),
        );
    }

    Ok(path.to_path_buf())
}

/// Copy a file to the output directory while preserving structure.
///
/// # Errors
/// Returns an error if copying fails.
pub fn smart_copy_with_structure(
    source: &Path,
    output_dir: &Path,
    base_dir: Option<&Path>,
    verbose: bool,
) -> Result<PathBuf> {
    let dest = if let Some(base) = base_dir {
        let rel_path =
            crate::media_conversion_gate::strip_prefix_or_self(source, base, "delivery_io_copy");
        output_dir.join(rel_path)
    } else {
        let file_name = source.file_name().context("Source file has no filename")?;
        output_dir.join(file_name)
    };

    if paths_alias(source, &dest)? {
        return Err(anyhow::anyhow!(
            "Refusing to copy source onto itself: {}",
            source.display()
        ));
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    if !dest.exists() {
        fs::copy(source, &dest).with_context(|| {
            format!("Failed to copy {} to {}", source.display(), dest.display())
        })?;

        if verbose {
            crate::ui_stderr::line(
                "📋",
                "[META]",
                format!("   Copied: {} → {}", source.display(), dest.display()),
            );
        }
    } else if verbose {
        match fs::metadata(&dest) {
            Ok(meta) => {
                crate::ui_stderr::line(
                    "⏭️",
                    "[SKIP]",
                    format!(
                        "   Already exists: {} ({} bytes)",
                        dest.display(),
                        meta.len()
                    ),
                );
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "smart_file_copy",
                    &dest,
                    format!("failed to read destination metadata for skip message: {e}"),
                );
                crate::ui_stderr::line(
                    crate::modern_ui::symbols::WARNING,
                    crate::modern_ui::symbols::plain::WARNING,
                    format!("   Already exists but inaccessible: {}", dest.display()),
                );
            }
        }
    }

    let dest = fix_extension_if_mismatch(&dest)?;

    crate::copy(source, &dest).with_context(|| {
        format!(
            "Copied {} to {} but failed to preserve metadata",
            source.display(),
            dest.display()
        )
    })?;

    Ok(dest)
}

/// Return whether two existing paths identify the same filesystem object.
///
/// A fallback copy must never target the source itself (including a hard link
/// or symlink alias), because the metadata/XMP preservation pass would then
/// mutate the only source copy. Missing destinations are not aliases.
fn paths_alias(source: &Path, destination: &Path) -> Result<bool> {
    if source == destination {
        return Ok(true);
    }

    let source_metadata = fs::metadata(source)
        .with_context(|| format!("Failed to inspect copy source: {}", source.display()))?;
    let destination_metadata = match fs::metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Failed to inspect copy destination: {}",
                    destination.display()
                )
            });
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(source_metadata.dev() == destination_metadata.dev()
            && source_metadata.ino() == destination_metadata.ino())
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(
            source_metadata.volume_serial_number() == destination_metadata.volume_serial_number()
                && source_metadata.file_index() == destination_metadata.file_index(),
        )
    }

    #[cfg(not(any(unix, windows)))]
    {
        let source_canonical = fs::canonicalize(source)
            .with_context(|| format!("Failed to resolve copy source: {}", source.display()))?;
        let destination_canonical = fs::canonicalize(destination).with_context(|| {
            format!(
                "Failed to resolve copy destination: {}",
                destination.display()
            )
        })?;
        Ok(source_canonical == destination_canonical)
    }
}

/// Copy the source file if conversion was skipped or failed.
///
/// # Errors
/// Returns an error if copying fails.
pub fn copy_on_skip_or_fail(
    source: &Path,
    output_dir: Option<&Path>,
    base_dir: Option<&Path>,
    verbose: bool,
) -> Result<Option<PathBuf>> {
    match output_dir {
        None => Ok(None),
        Some(out_dir) => match smart_copy_with_structure(source, out_dir, base_dir, verbose) {
            Ok(dest) => {
                let reason = "adjacent_copy_on_skip_or_fail";
                crate::infra::static_logs::emit_mfb_audit(
                    "preserved",
                    "batch",
                    Some(source),
                    reason,
                    None,
                );
                crate::ui_stderr::line(
                    "📋",
                    "[PRESERVE]",
                    format!("   {} → {} ({reason})", source.display(), dest.display()),
                );
                Ok(Some(dest))
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "delivery_runtime",
                    format!(
                        "COPY FAILED: {} (Source: {}, Output: {})",
                        e,
                        source.display(),
                        out_dir.display()
                    ),
                );
                Err(e)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_smart_copy_preserves_structure() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let base = temp.path().join("input");
        let output = temp.path().join("output");

        fs::create_dir_all(base.join("photos/2024")).unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = base.join("photos/2024/test.txt");
        fs::write(&source, "test").unwrap_or_else(|e| panic!("error: {e:?}"));

        let dest = smart_copy_with_structure(&source, &output, Some(&base), false)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        assert_eq!(dest, output.join("photos/2024/test.txt"));
        assert!(dest.exists());
        assert_eq!(
            fs::read_to_string(&dest).unwrap_or_else(|e| panic!("error: {e:?}")),
            "test"
        );
    }

    #[test]
    fn test_copy_on_skip_with_none() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = temp.path().join("test.txt");
        fs::write(&source, "test").unwrap_or_else(|e| panic!("error: {e:?}"));

        let result = copy_on_skip_or_fail(&source, None, None, false)
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        assert!(result.is_none());
    }

    #[test]
    fn test_smart_copy_rejects_source_alias() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = temp.path().join("test.txt");
        fs::write(&source, "test").unwrap_or_else(|e| panic!("error: {e:?}"));

        let error = smart_copy_with_structure(&source, temp.path(), None, false)
            .expect_err("fallback copy must refuse a source/destination alias");
        assert!(error.to_string().contains("source onto itself"));
        assert_eq!(fs::read_to_string(&source).unwrap(), "test");
    }

    /// Content is video (MP4 ftyp+isom) but extension was wrong → corrected to
    /// .mp4.
    #[test]
    fn test_fix_extension_video_content_wrong_ext() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        // File named .jpg but content is MP4 (ftyp box, isom brand)
        let wrong_ext = temp.path().join("video.jpg");
        let mut header = [0u8; 32];
        header[4..8].copy_from_slice(b"ftyp");
        header[8..12].copy_from_slice(b"isom");
        fs::write(&wrong_ext, header).unwrap_or_else(|e| panic!("error: {e:?}"));

        let fixed =
            fix_extension_if_mismatch(&wrong_ext).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(fixed.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert!(fixed.exists());
        assert!(!wrong_ext.exists());
    }

    #[test]
    fn test_check_extension_mismatch_readonly() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let wrong_ext = temp.path().join("video_readonly.jpg");
        let mut header = [0u8; 32];
        header[4..8].copy_from_slice(b"ftyp");
        header[8..12].copy_from_slice(b"isom");
        fs::write(&wrong_ext, header).unwrap_or_else(|e| panic!("error: {e:?}"));

        let checked = check_extension_mismatch_readonly(&wrong_ext)
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        // It should return the original path
        assert_eq!(checked, wrong_ext);
        // It should NOT rename the file
        assert!(wrong_ext.exists());
        assert!(!temp.path().join("video_readonly.mp4").exists());
    }
}
