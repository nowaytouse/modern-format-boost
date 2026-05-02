//! 🔥 v0.11.2: Smart File Copier - Unified File Copy Module
//!
//! Features:
//! - ✅ Preserves full directory structure
//! - ✅ Preserves file metadata (timestamps, permissions)
//! - ✅ Automatically merges XMP sidecar files
//! - ✅ Loud errors, fully transparent
//!
//! This module unifies file copy logic across all converters, avoiding code duplication.
//!
//! ## Extension Correction & Validation Order
//! - `fix_extension_if_mismatch` corrects extension based on file magic bytes (prevents panics/misjudgment due to faked extensions).
//! - Design convention: **Fix first, then branch by extension**. All entry points (`cli_runner`, img_*) call `fix_extension` before processing. All subsequent "extension-only" logic should be based on the fixed path. See `CODE_AUDIT.md` §36.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Fix the extension of a file if it doesn't match its content.
///
/// # Errors
/// Returns an error if content analysis fails.
pub fn fix_extension_if_mismatch(path: &std::path::Path) -> Result<PathBuf> {
    use crate::quality_matcher::SourceCodec;

    let current_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    if let Some(codec) = SourceCodec::identify_by_content(path) {
        if !codec.is_extension_compatible(&current_ext) {
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
                    eprintln!(
                        "⚠️  [Extension Fix] SKIPPED: {} -> .{} (target {} already exists)",
                        path.display(),
                        content_format,
                        new_path.display()
                    );
                    return Ok(path.to_path_buf());
                }
            }

            eprintln!(
                "⚠️  [Extension Fix] {} -> .{} (content does not match extension)",
                path.display(),
                content_format
            );

            fs::rename(path, &new_path).with_context(|| {
                format!(
                    "Failed to rename {} to {}",
                    path.display(),
                    new_path.display()
                )
            })?;

            eprintln!("✅  [Extension Fix] Complete: {}", new_path.display());

            return Ok(new_path);
        }
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

    let current_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    if let Some(codec) = SourceCodec::identify_by_content(path) {
        if !codec.is_extension_compatible(&current_ext) {
            let content_format = codec.default_extension();
            tracing::warn!(
                path = %path.display(),
                current_ext,
                detected_format = content_format,
                "Extension mismatch detected (source immutable, not renaming)"
            );
            eprintln!(
                "⚠️  [Extension Check] {} has .{} extension but content is .{} (source directory immutable, not renaming)",
                path.display(),
                current_ext,
                content_format
            );
        }
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
        let rel_path = source.strip_prefix(base).unwrap_or(source);
        output_dir.join(rel_path)
    } else {
        let file_name = source.file_name().context("Source file has no filename")?;
        output_dir.join(file_name)
    };

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    if !dest.exists() {
        fs::copy(source, &dest).with_context(|| {
            format!("Failed to copy {} to {}", source.display(), dest.display())
        })?;

        if verbose {
            eprintln!("   📋 Copied: {} → {}", source.display(), dest.display());
        }
    } else if verbose {
        if let Ok(meta) = fs::metadata(&dest) {
            eprintln!(
                "   ⏭️  Already exists: {} ({} bytes)",
                dest.display(),
                meta.len()
            );
        } else {
            eprintln!("   ⚠️  Already exists but inaccessible: {}", dest.display());
        }
    }

    let dest = fix_extension_if_mismatch(&dest)?;

    crate::copy_metadata(source, &dest);

    Ok(dest)
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
    if let Some(out_dir) = output_dir {
        match smart_copy_with_structure(source, out_dir, base_dir, verbose) {
            Ok(dest) => Ok(Some(dest)),
            Err(e) => {
                eprintln!("❌ COPY FAILED: {e}");
                eprintln!("   Source: {}", source.display());
                eprintln!("   Output dir: {}", out_dir.display());
                Err(e)
            }
        }
    } else {
        Ok(None)
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

        let dest = smart_copy_with_structure(&source, &output, Some(&base), false).unwrap_or_else(|e| panic!("error: {e:?}"));

        assert_eq!(dest, output.join("photos/2024/test.txt"));
        assert!(dest.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap_or_else(|e| panic!("error: {e:?}")), "test");
    }

    #[test]
    fn test_copy_on_skip_with_none() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = temp.path().join("test.txt");
        fs::write(&source, "test").unwrap_or_else(|e| panic!("error: {e:?}"));

        let result = copy_on_skip_or_fail(&source, None, None, false).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert!(result.is_none());
    }

    /// Content is video (MP4 ftyp+isom) but extension was wrong → corrected to .mp4.
    #[test]
    fn test_fix_extension_video_content_wrong_ext() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        // File named .jpg but content is MP4 (ftyp box, isom brand)
        let wrong_ext = temp.path().join("video.jpg");
        let mut header = [0u8; 32];
        header[4..8].copy_from_slice(b"ftyp");
        header[8..12].copy_from_slice(b"isom");
        fs::write(&wrong_ext, header).unwrap_or_else(|e| panic!("error: {e:?}"));

        let fixed = fix_extension_if_mismatch(&wrong_ext).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(fixed.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert!(fixed.exists());
        assert!(!wrong_ext.exists());
    }
}
