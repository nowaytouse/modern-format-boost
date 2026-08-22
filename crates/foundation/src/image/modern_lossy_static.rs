//! Lossy modern static image probing for fast-img tier-2 Photos import.
//!
//! Uses the same authoritative-tools-first, cascading-fallback strategy as PNG
//! validation. Only static, modern-format, lossy images are admitted.
//! Generic HEIF is retained because its brand does not identify the primary
//! item's codec; HEIC has a separate codec-constrained route.

use crate::image_detection::{CompressionType, DetectedFormat, detect_compression};
use crate::unified_error::{ImgQualityError, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::format_detect::{FormatKind, detect_true_format, validate_format_forensic};

/// A source file confirmed as a static, lossy, modern-format image suitable for
/// direct Apple Photos import (fast-img tier 2).
#[derive(Debug, Clone)]
pub struct ModernLossyStaticCandidate {
    pub path: PathBuf,
    pub rel_path: String,
    pub format: FormatKind,
    pub blake3: String,
}

#[must_use]
pub const fn is_modern_static_image_format(format: FormatKind) -> bool {
    matches!(
        format,
        FormatKind::WebP | FormatKind::Jp2 | FormatKind::Jxl | FormatKind::Avif | FormatKind::Heic
    )
}

const fn detected_format_from_kind(format: FormatKind) -> Option<DetectedFormat> {
    match format {
        FormatKind::WebP => Some(DetectedFormat::WebP),
        FormatKind::Jp2 => Some(DetectedFormat::JP2),
        FormatKind::Jxl => Some(DetectedFormat::JXL),
        FormatKind::Avif => Some(DetectedFormat::AVIF),
        FormatKind::Heic => Some(DetectedFormat::HEIC),
        _ => None,
    }
}

/// Probe whether `path` is a static, lossy, modern-format image.
///
/// Returns `Ok(None)` when the file is not eligible (wrong format, animated,
/// lossless, or inconclusive). Returns `Err` on parse/decode failure for
/// non-standard media.
pub fn probe_modern_lossy_static(path: &Path) -> Result<Option<ModernLossyStaticCandidate>> {
    let format = detect_true_format(path)?;
    if !is_modern_static_image_format(format) {
        return Ok(None);
    }

    // Bind every classification decision to the exact bytes later offered to
    // Photos. A path may be replaced while external/container probes run.
    let before_blake3 = crate::common_utils::calculate_blake3_hash(path).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "modern lossy static pre-probe BLAKE3 failed for {}: {err}",
            path.display()
        ))
    })?;

    let detected = detected_format_from_kind(format).ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "modern lossy static probe missing DetectedFormat mapping for {format:?}: {}",
            path.display()
        ))
    })?;

    if !confirmed_static_only(path, &detected)? {
        tracing::debug!(
            target: "modern_lossy_static",
            path = %path.display(),
            format = ?format,
            "skipping animated or inconclusive modern format"
        );
        return Ok(None);
    }

    let compression = detect_modern_compression_authoritative(path, format, &detected)?;
    match compression {
        CompressionType::Lossy => {}
        CompressionType::Lossless => {
            tracing::debug!(
                target: "modern_lossy_static",
                path = %path.display(),
                format = ?format,
                "skipping lossless modern format (not eligible for tier-2 lossy import)"
            );
            return Ok(None);
        }
        CompressionType::JpegReconstruction => {
            tracing::debug!(
                target: "modern_lossy_static",
                path = %path.display(),
                format = ?format,
                "skipping jbrd JPEG-reconstruction JXL (reversible-JPEG route, not a lossy modern source)"
            );
            return Ok(None);
        }
        // Unproven compression semantics: admitting the file as "lossy" would
        // be a fabricated verdict; tier-2 direct import requires the positive
        // ConfirmedLossy proof.
        CompressionType::Unknown => {
            tracing::debug!(
                target: "modern_lossy_static",
                path = %path.display(),
                format = ?format,
                "skipping modern format with unproven compression semantics (fail-closed)"
            );
            return Ok(None);
        }
    }

    let after_blake3 = crate::common_utils::calculate_blake3_hash(path).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "modern lossy static post-probe BLAKE3 failed for {}: {err}",
            path.display()
        ))
    })?;
    let final_format = detect_true_format(path)?;
    if before_blake3 != after_blake3 || final_format != format {
        return Err(ImgQualityError::AnalysisError(format!(
            "modern lossy static source changed during classification: {}",
            path.display()
        )));
    }

    let rel_path = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    Ok(Some(ModernLossyStaticCandidate {
        path: path.to_path_buf(),
        rel_path,
        format,
        blake3: after_blake3,
    }))
}

/// Result of a tier-2 scan: admitted candidates plus per-file probe failures.
///
/// A probe failure (unreadable or non-standard modern media) quarantines that
/// file with an explicit reason instead of aborting the whole scan — the same
/// retention doctrine as the fast-img source scan.
#[derive(Debug, Default)]
pub struct ModernLossyStaticScan {
    pub candidates: Vec<ModernLossyStaticCandidate>,
    pub probe_failures: Vec<(PathBuf, String)>,
}

impl ModernLossyStaticScan {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.candidates.is_empty() && self.probe_failures.is_empty()
    }
}

/// Scan `file_paths` under `src_root` for tier-2 import candidates.
///
/// Files that are not eligible (wrong format, animated, lossless) are skipped.
/// Files whose probe fails (parse/decode/hash errors) are quarantined in
/// `probe_failures` with the failure reason; they never abort the scan and
/// never masquerade as candidates.
pub fn scan_modern_lossy_static_candidates(
    src_root: &Path,
    file_paths: &[PathBuf],
) -> Result<ModernLossyStaticScan> {
    let mut scan = ModernLossyStaticScan::default();
    let mut relative_paths = BTreeSet::new();
    for path in file_paths {
        match probe_modern_lossy_static(path) {
            Ok(Some(candidate)) => {
                let rel = crate::media_conversion_gate::strip_prefix_or_self(
                    path,
                    src_root,
                    "modern_lossy_static_rel",
                );
                let rel_path = rel.to_string_lossy().to_string();
                if !relative_paths.insert(rel_path.clone()) {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "modern lossy static scan produced duplicate relative path {rel_path}"
                    )));
                }
                scan.candidates.push(ModernLossyStaticCandidate {
                    rel_path,
                    ..candidate
                });
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    target: "modern_lossy_static",
                    path = %path.display(),
                    error = %err,
                    "modern lossy static probe failed; quarantining file"
                );
                scan.probe_failures.push((path.clone(), err.to_string()));
            }
        }
    }
    scan.candidates.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    scan.probe_failures.sort();
    Ok(scan)
}

fn confirmed_static_only(path: &Path, detected: &DetectedFormat) -> Result<bool> {
    use crate::image_detection::{animatable_format_confirmed_static_only, detect_animation};

    let (is_animated, frame_count, _) = detect_animation(path, detected)?;
    if is_animated {
        return Ok(false);
    }
    animatable_format_confirmed_static_only(path, detected, false, frame_count)
}

fn detect_modern_compression_authoritative(
    path: &Path,
    format: FormatKind,
    detected: &DetectedFormat,
) -> Result<CompressionType> {
    let forensic_tool =
        super::format_detect::forensic_tool_for_format(format).map(|policy| policy.tool);
    let tool_available = forensic_tool
        .and_then(crate::common_utils::resolve_tool_path)
        .is_some();
    let in_process_validator_available = cfg!(feature = "v1_21")
        && matches!(format, FormatKind::Heic | FormatKind::Heif);
    let forensic_validator_available = tool_available || in_process_validator_available;

    // JXL's compression classifier already performs an in-process structural
    // decode and, for Modular streams, one bounded jxlinfo query. Avoid a
    // duplicate jxlinfo spawn here.
    if format == FormatKind::Jxl {
        return detect_compression(detected, path);
    }

    // The internal JP2 parser proves the effective first-tile wavelet but is
    // intentionally not a full codestream decoder. Require the existing
    // ImageMagick/OpenJPEG-backed structural validation before its positive
    // result can reach the destructive tier; absence of the tool is a retain.
    if format == FormatKind::Jp2 && !tool_available {
        tracing::debug!(
            target: "modern_lossy_static",
            path = %path.display(),
            "retaining JP2 because authoritative structural validation is unavailable"
        );
        return Ok(CompressionType::Unknown);
    }

    match validate_format_forensic(path, format) {
        Ok(check) => {
            tracing::debug!(
                target: "modern_lossy_static",
                path = %path.display(),
                format = ?format,
                tool = %check.tool,
                "forensic format validation passed before compression probe"
            );
        }
        Err(err) if forensic_validator_available => {
            return Err(ImgQualityError::AnalysisError(format!(
                "forensic validation rejected modern format candidate {} ({format:?}): {err}",
                path.display()
            )));
        }
        Err(_) => {
            tracing::debug!(
                target: "modern_lossy_static",
                path = %path.display(),
                format = ?format,
                "forensic tool unavailable; falling back to spec-level compression probe"
            );
        }
    }

    detect_compression(detected, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn modern_static_format_filter() {
        assert!(is_modern_static_image_format(FormatKind::WebP));
        assert!(is_modern_static_image_format(FormatKind::Avif));
        assert!(is_modern_static_image_format(FormatKind::Heic));
        assert!(
            !is_modern_static_image_format(FormatKind::Heif),
            "generic HEIF has no codec guarantee; retain it until the primary item/property association is resolved"
        );
        assert!(!is_modern_static_image_format(FormatKind::Jpeg));
        assert!(!is_modern_static_image_format(FormatKind::Png));
        assert!(!is_modern_static_image_format(FormatKind::Gif));
    }

    #[test]
    fn non_modern_file_is_not_candidate() -> Result<()> {
        let mut file = tempfile::NamedTempFile::new().expect("temp");
        file.write_all(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46])
            .expect("write");
        assert!(probe_modern_lossy_static(file.path())?.is_none());
        Ok(())
    }

    #[test]
    fn test_scan_modern_lossy_static_candidates_skips_non_modern() -> Result<()> {
        let tempdir = tempfile::tempdir().unwrap();
        let jpeg_path = tempdir.path().join("photo.jpg");
        std::fs::write(&jpeg_path, [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46]).unwrap();

        let scan = scan_modern_lossy_static_candidates(tempdir.path(), &[jpeg_path])?;
        assert!(scan.candidates.is_empty());
        assert!(
            scan.probe_failures.is_empty(),
            "non-modern media is a skip, never a probe failure"
        );
        Ok(())
    }

    #[test]
    fn test_scan_quarantines_unparseable_modern_media_instead_of_aborting() -> Result<()> {
        let tempdir = tempfile::tempdir().unwrap();
        // RIFF/WEBP magic (so detect_true_format says WebP) with a RIFF size
        // declaring far more bytes than exist: structurally invalid.
        let broken_webp = tempdir.path().join("broken.webp");
        std::fs::write(&broken_webp, b"RIFF\x99\x00\x00\x00WEBP")?;
        // A second, healthy non-modern file must still be scanned even though
        // the first file's probe fails.
        let jpeg_path = tempdir.path().join("photo.jpg");
        std::fs::write(&jpeg_path, [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46])?;

        let scan =
            scan_modern_lossy_static_candidates(tempdir.path(), &[broken_webp.clone(), jpeg_path])?;
        assert!(scan.candidates.is_empty());
        assert_eq!(
            scan.probe_failures.len(),
            1,
            "broken WebP must be quarantined"
        );
        assert_eq!(scan.probe_failures[0].0, broken_webp);
        assert!(
            scan.probe_failures[0]
                .1
                .chars()
                .any(|ch| !ch.is_whitespace()),
            "quarantined probe failure must carry a non-blank reason"
        );
        Ok(())
    }
}
