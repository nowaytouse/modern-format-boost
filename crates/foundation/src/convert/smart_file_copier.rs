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
    let requested_dest = if let Some(base) = base_dir {
        let rel_path =
            crate::media_conversion_gate::strip_prefix_or_self(source, base, "delivery_io_copy");
        if rel_path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return Err(anyhow::anyhow!(
                "Refusing archive copy outside output directory: source {} does not form a safe relative path under {}",
                source.display(),
                base.display()
            ));
        }
        output_dir.join(rel_path)
    } else {
        let file_name = source.file_name().context("Source file has no filename")?;
        output_dir.join(file_name)
    };

    if paths_alias(source, &requested_dest)? {
        return Err(anyhow::anyhow!(
            "Refusing to copy source onto itself: {}",
            source.display()
        ));
    }

    let dest = corrected_copy_destination(source, &requested_dest)?;
    if dest != requested_dest && paths_alias(source, &dest)? {
        return Err(anyhow::anyhow!(
            "Refusing to copy source onto itself: {}",
            source.display()
        ));
    }

    // The selected root may itself be an intentional alias, but descendants
    // must not redirect a structured archive copy outside that selected tree.
    if let Some(relative_parent) = dest.strip_prefix(output_dir)?.parent() {
        let mut directory = output_dir.to_path_buf();
        for component in relative_parent.components() {
            directory.push(component);
            match fs::symlink_metadata(&directory) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(anyhow::anyhow!(
                        "Refusing archive copy through symbolic-link subdirectory: {}",
                        directory.display()
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("Failed to inspect archive directory"),
            }
        }
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let destination_exists = match fs::symlink_metadata(&dest) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(anyhow::anyhow!(
                    "Refusing to preserve onto symbolic-link destination: {}",
                    dest.display()
                ));
            }
            if !metadata.is_file() {
                return Err(anyhow::anyhow!(
                    "Refusing to preserve onto non-regular destination: {}",
                    dest.display()
                ));
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Failed to inspect copy destination: {}", dest.display())
            });
        }
    };

    let parent = dest.parent().context("Copy destination has no parent")?;
    let suffix = dest
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .map_or_else(|| String::from(".tmp"), |extension| format!(".{extension}"));
    let staged = crate::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
        "smart_file_copy",
        parent,
        ".mfb-copy-",
        &suffix,
    )?;
    let copied = fs::copy(source, staged.path()).with_context(|| {
        format!(
            "Failed to stage copy {} for {}",
            source.display(),
            dest.display()
        )
    })?;
    let source_size = fs::metadata(source)
        .with_context(|| format!("Failed to inspect copy source: {}", source.display()))?
        .len();
    if copied != source_size {
        return Err(anyhow::anyhow!(
            "Staged copy length mismatch for {}: expected {source_size}, copied {copied}",
            source.display()
        ));
    }

    crate::copy(source, staged.path()).with_context(|| {
        format!(
            "Staged {} for {} but failed to preserve metadata",
            source.display(),
            dest.display()
        )
    })?;

    if destination_exists {
        verify_existing_archive_copy(staged.path(), &dest)?;
        if verbose {
            crate::ui_stderr::line(
                "⏭️",
                "[SKIP]",
                format!("   Already preserved: {}", dest.display()),
            );
        }
        return Ok(dest);
    }

    if let Err(error) = staged.persist_noclobber(&dest) {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            // A concurrent pair member may have published the same archive.
            // Reuse only after proving payload and metadata, never overwrite it.
            verify_existing_archive_copy(error.file.path(), &dest)?;
        } else {
            return Err(anyhow::anyhow!(
                "Failed to publish preserved copy {} without overwriting an existing path: {}",
                dest.display(),
                error.error
            ));
        }
    }

    if verbose {
        crate::ui_stderr::line(
            "📋",
            "[META]",
            format!("   Copied: {} → {}", source.display(), dest.display()),
        );
    }

    Ok(dest)
}

fn verify_existing_archive_copy(staged: &Path, destination: &Path) -> Result<()> {
    if !fs::symlink_metadata(destination)?.is_file() {
        return Err(anyhow::anyhow!(
            "Refusing to reuse a non-regular archive destination: {}",
            destination.display()
        ));
    }
    if crate::common_utils::calculate_blake3_hash(staged)?
        != crate::common_utils::calculate_blake3_hash(destination)?
    {
        return Err(anyhow::anyhow!(
            "Refusing to reuse archive destination with a different payload: {}",
            destination.display()
        ));
    }
    crate::metadata::verify_exact_metadata_copy(staged, destination).with_context(|| {
        format!(
            "Refusing to reuse existing copy with different filesystem metadata: {}",
            destination.display()
        )
    })?;
    Ok(())
}

fn corrected_copy_destination(source: &Path, destination: &Path) -> Result<PathBuf> {
    use crate::quality_matcher::SourceCodec;

    let current_ext =
        crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(destination);
    if let Some(codec) = SourceCodec::identify_by_content(source)?
        && !codec.is_extension_compatible(&current_ext)
    {
        let corrected = destination.with_extension(codec.default_extension());
        crate::ui_stderr::line(
            crate::modern_ui::symbols::WARNING,
            crate::modern_ui::symbols::plain::WARNING,
            format!(
                "[Extension Fix] {} -> {} (content does not match extension)",
                destination.display(),
                corrected.display()
            ),
        );
        return Ok(corrected);
    }
    Ok(destination.to_path_buf())
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
                // Retained originals carry the same edit-history obligation as
                // converted assets, including when an existing copy is reused.
                crate::metadata::handle_aae_sidecar(source, &dest)?;
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
    fn test_smart_copy_rejects_parent_escape_from_output_root() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let base = temp.path().join("input");
        let output = temp.path().join("delivery/nested");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&output).unwrap();
        let source = base.join("../original.txt");
        fs::write(&source, b"original archive bytes").unwrap();
        let escaped_destination = temp.path().join("delivery/original.txt");

        let result = smart_copy_with_structure(&source, &output, Some(&base), false);

        assert_eq!(fs::read(&source).unwrap(), b"original archive bytes");
        assert!(
            result.is_err(),
            "relative parent components must not escape output root"
        );
        assert!(
            !escaped_destination.exists(),
            "no out-of-root copy may be created"
        );
    }

    #[test]
    fn test_copy_on_skip_preserves_aae_and_rejects_sidecar_collision() {
        let temp = TempDir::new().expect("archive fixture");
        let source = temp.path().join("archive.txt");
        let sidecar = source.with_extension("AAE");
        let output = temp.path().join("output");
        fs::write(&source, b"original archive").unwrap();
        fs::write(&sidecar, b"original edit history").unwrap();

        let destination = copy_on_skip_or_fail(&source, Some(&output), None, false)
            .expect("preserve archive asset")
            .expect("delivered path");
        let delivered_sidecar = destination.with_extension("AAE");
        assert_eq!(
            fs::read(&delivered_sidecar).unwrap(),
            fs::read(&sidecar).unwrap()
        );
        copy_on_skip_or_fail(&source, Some(&output), None, false)
            .expect("complete archive can be reused");

        fs::write(&delivered_sidecar, b"unrelated edit history").unwrap();
        assert!(copy_on_skip_or_fail(&source, Some(&output), None, false).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"original archive");
        assert_eq!(fs::read(&sidecar).unwrap(), b"original edit history");
        assert_eq!(
            fs::read(&delivered_sidecar).unwrap(),
            b"unrelated edit history"
        );
    }

    #[test]
    fn test_concurrent_archive_copy_is_idempotent_with_aae() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("archive.txt");
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        fs::write(&source, vec![42_u8; 65536]).unwrap();
        fs::write(source.with_extension("AAE"), b"edit history").unwrap();
        let barrier = std::sync::Barrier::new(8);
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        copy_on_skip_or_fail(&source, Some(&output), None, false)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        for result in results {
            result.expect("concurrent identical archive delivery must be idempotent");
        }
        assert_eq!(
            fs::read(output.join("archive.txt")).unwrap(),
            fs::read(&source).unwrap()
        );
        assert_eq!(
            fs::read(output.join("archive.AAE")).unwrap(),
            b"edit history"
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

    #[test]
    fn test_smart_copy_rejects_existing_different_payload_without_mutation() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source_dir = temp.path().join("input");
        let output_dir = temp.path().join("output");
        fs::create_dir_all(&source_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::create_dir_all(&output_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = source_dir.join("archive.jxl");
        let destination = output_dir.join("archive.jxl");
        fs::write(&source, b"source archive payload").unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::write(&destination, b"unrelated existing payload")
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let error = smart_copy_with_structure(&source, &output_dir, Some(&source_dir), false)
            .expect_err("a conflicting archive destination must fail closed");

        assert!(error.to_string().contains("different payload"));
        assert_eq!(fs::read(&source).unwrap(), b"source archive payload");
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"unrelated existing payload"
        );
    }

    #[test]
    fn test_smart_copy_rejects_existing_metadata_mismatch_without_mutation() {
        for mismatch in ["mtime", "permissions"] {
            let temp = TempDir::new().expect("create metadata-conflict fixture");
            let source = temp.path().join("archive.txt");
            let output_dir = temp.path().join("output");
            fs::create_dir(&output_dir).expect("create output directory");
            let destination = output_dir.join("archive.txt");
            fs::write(&source, b"matching archive payload").expect("write source");
            fs::write(&destination, b"matching archive payload").expect("write existing copy");
            crate::copy(&source, &destination).expect("align baseline metadata");
            let source_before = fs::metadata(&source).expect("source metadata");
            if mismatch == "mtime" {
                filetime::set_file_mtime(
                    &destination,
                    filetime::FileTime::from_unix_time(1_600_000_000, 0),
                )
                .expect("set stale output timestamp");
            } else {
                let mut permissions = source_before.permissions();
                permissions.set_readonly(true);
                fs::set_permissions(&destination, permissions)
                    .expect("set conflicting permissions");
            }
            let destination_before = fs::metadata(&destination).expect("existing metadata");

            let result = smart_copy_with_structure(&source, &output_dir, None, false);

            let destination_after =
                fs::metadata(&destination).expect("preserved existing metadata");
            assert_eq!(
                destination_after.modified().unwrap(),
                destination_before.modified().unwrap()
            );
            assert_eq!(
                destination_after.permissions(),
                destination_before.permissions()
            );
            assert_eq!(fs::read(&source).unwrap(), b"matching archive payload");
            assert_eq!(fs::read(&destination).unwrap(), b"matching archive payload");
            assert_eq!(
                fs::metadata(&source).unwrap().permissions(),
                source_before.permissions()
            );
            assert_eq!(
                fs::metadata(&source).unwrap().modified().unwrap(),
                source_before.modified().unwrap()
            );
            // Restore only test-owned permissions so Windows can remove the fixture.
            fs::set_permissions(&destination, source_before.permissions())
                .expect("restore fixture permissions");
            let error =
                result.expect_err("matching bytes must not hide missing filesystem metadata");
            assert!(
                error.to_string().contains("different filesystem metadata"),
                "{mismatch}: {error:#}"
            );
        }
    }

    #[test]
    fn test_copy_on_skip_accepts_existing_identical_regular_payload() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source_dir = temp.path().join("input");
        let output_dir = temp.path().join("output");
        fs::create_dir_all(&source_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::create_dir_all(&output_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = source_dir.join("archive.jxl");
        let destination = output_dir.join("archive.jxl");
        fs::write(&source, b"matching archive payload").unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::write(&destination, b"matching archive payload")
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        crate::copy(&source, &destination).expect("align matching archive metadata");

        let preserved = copy_on_skip_or_fail(&source, Some(&output_dir), Some(&source_dir), false)
            .unwrap_or_else(|e| panic!("identical destination should be idempotent: {e:?}"));

        assert_eq!(preserved, Some(destination.clone()));
        assert_eq!(fs::read(&source).unwrap(), b"matching archive payload");
        assert_eq!(fs::read(&destination).unwrap(), b"matching archive payload");
    }

    #[test]
    fn test_smart_copy_repeat_accepts_matching_xmp_enriched_delivery() {
        assert!(
            crate::ExiftoolBuilder::check_available(),
            "repeat-copy regression requires ExifTool"
        );
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source_dir = temp.path().join("input");
        let output_dir = temp.path().join("output");
        fs::create_dir_all(&source_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = source_dir.join("archive.jpg");
        image::RgbImage::from_pixel(2, 2, image::Rgb([18, 52, 86]))
            .save(&source)
            .unwrap_or_else(|e| panic!("write JPEG fixture: {e:?}"));
        fs::write(
            source.with_extension("xmp"),
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>archive copy regression</dc:title>
</rdf:Description>
</rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap_or_else(|e| panic!("write XMP fixture: {e:?}"));

        let destination = smart_copy_with_structure(&source, &output_dir, Some(&source_dir), false)
            .unwrap_or_else(|e| panic!("first XMP-enriched copy failed: {e:?}"));
        let first_delivery = fs::read(&destination).unwrap();
        assert_ne!(
            first_delivery,
            fs::read(&source).unwrap(),
            "fixture must prove that sidecar enrichment changes delivered bytes"
        );

        let repeated = smart_copy_with_structure(&source, &output_dir, Some(&source_dir), false)
            .unwrap_or_else(|e| panic!("matching enriched delivery must be idempotent: {e:?}"));

        assert_eq!(repeated, destination);
        assert_eq!(fs::read(&destination).unwrap(), first_delivery);
    }

    #[test]
    fn test_smart_copy_rejects_conflict_at_content_corrected_destination() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source_dir = temp.path().join("input");
        let output_dir = temp.path().join("output");
        fs::create_dir_all(&source_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::create_dir_all(&output_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = source_dir.join("video.jpg");
        let requested_destination = output_dir.join("video.jpg");
        let corrected_destination = output_dir.join("video.mp4");
        let mut mp4_payload = [0_u8; 32];
        mp4_payload[4..8].copy_from_slice(b"ftyp");
        mp4_payload[8..12].copy_from_slice(b"isom");
        fs::write(&source, mp4_payload).unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::write(&corrected_destination, b"unrelated corrected-name payload")
            .unwrap_or_else(|e| panic!("error: {e:?}"));

        let error = smart_copy_with_structure(&source, &output_dir, Some(&source_dir), false)
            .expect_err("a conflict at the corrected destination must fail closed");

        assert!(error.to_string().contains("different payload"));
        assert_eq!(fs::read(&source).unwrap(), mp4_payload);
        assert_eq!(
            fs::read(&corrected_destination).unwrap(),
            b"unrelated corrected-name payload"
        );
        assert!(!requested_destination.exists());
    }

    #[test]
    fn test_smart_copy_rejects_requested_source_alias_before_extension_correction() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = temp.path().join("video.jpg");
        let mut mp4_payload = [0_u8; 32];
        mp4_payload[4..8].copy_from_slice(b"ftyp");
        mp4_payload[8..12].copy_from_slice(b"isom");
        fs::write(&source, mp4_payload).unwrap_or_else(|e| panic!("error: {e:?}"));

        let error = smart_copy_with_structure(&source, temp.path(), None, false)
            .expect_err("requested source alias must fail before extension correction");

        assert!(error.to_string().contains("source onto itself"));
        assert_eq!(fs::read(&source).unwrap(), mp4_payload);
        assert!(!temp.path().join("video.mp4").exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_archive_copy_cannot_escape_through_linked_subdirectory() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("input");
        let output = temp.path().join("output");
        let unrelated = temp.path().join("unrelated");
        fs::create_dir_all(base.join("photos")).unwrap();
        fs::create_dir(&output).unwrap();
        fs::create_dir(&unrelated).unwrap();
        let source = base.join("photos/original.txt");
        fs::write(&source, b"original archive").unwrap();
        std::os::unix::fs::symlink(&unrelated, output.join("photos")).unwrap();

        assert!(copy_on_skip_or_fail(&source, Some(&output), Some(&base), false).is_err());
        assert!(!unrelated.join("original.txt").exists());
        assert_eq!(fs::read(&source).unwrap(), b"original archive");
    }

    #[cfg(unix)]
    #[test]
    fn test_smart_copy_rejects_destination_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source_dir = temp.path().join("input");
        let output_dir = temp.path().join("output");
        fs::create_dir_all(&source_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::create_dir_all(&output_dir).unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = source_dir.join("archive.jxl");
        let target = temp.path().join("symlink-target.jxl");
        let destination = output_dir.join("archive.jxl");
        fs::write(&source, b"source archive payload").unwrap_or_else(|e| panic!("error: {e:?}"));
        fs::write(&target, b"target must remain untouched")
            .unwrap_or_else(|e| panic!("error: {e:?}"));
        symlink(&target, &destination).unwrap_or_else(|e| panic!("error: {e:?}"));

        let error = smart_copy_with_structure(&source, &output_dir, Some(&source_dir), false)
            .expect_err("a destination symlink must fail closed");

        assert!(error.to_string().contains("symbolic-link destination"));
        assert_eq!(fs::read(&source).unwrap(), b"source archive payload");
        assert_eq!(fs::read(&target).unwrap(), b"target must remain untouched");
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
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
