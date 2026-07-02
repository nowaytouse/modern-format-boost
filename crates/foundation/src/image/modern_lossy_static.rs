//! Lossy modern static image probing for fast-img tier-2 Photos import.
//!
//! Uses the same authoritative-tools-first, cascading-fallback strategy as PNG
//! validation. Only static, modern-format, lossy images are admitted.

use crate::image_detection::{CompressionType, DetectedFormat, detect_compression};
use crate::unified_error::{ImgQualityError, Result};
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
pub fn is_modern_static_image_format(format: FormatKind) -> bool {
    matches!(
        format,
        FormatKind::WebP
            | FormatKind::Jp2
            | FormatKind::Jxl
            | FormatKind::Avif
            | FormatKind::Heic
            | FormatKind::Heif
    )
}

fn detected_format_from_kind(format: FormatKind) -> Option<DetectedFormat> {
    match format {
        FormatKind::WebP => Some(DetectedFormat::WebP),
        FormatKind::Jp2 => Some(DetectedFormat::JP2),
        FormatKind::Jxl => Some(DetectedFormat::JXL),
        FormatKind::Avif => Some(DetectedFormat::AVIF),
        FormatKind::Heic => Some(DetectedFormat::HEIC),
        FormatKind::Heif => Some(DetectedFormat::HEIF),
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
    if compression == CompressionType::Lossless {
        return Ok(None);
    }

    let rel_path = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let blake3 = crate::common_utils::calculate_blake3_hash(path).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "modern lossy static probe BLAKE3 failed for {}: {err}",
            path.display()
        ))
    })?;

    Ok(Some(ModernLossyStaticCandidate {
        path: path.to_path_buf(),
        rel_path,
        format,
        blake3,
    }))
}

/// Scan `file_paths` under `src_root` and return tier-2 import candidates.
pub fn scan_modern_lossy_static_candidates(
    src_root: &Path,
    file_paths: &[PathBuf],
) -> Result<Vec<ModernLossyStaticCandidate>> {
    let mut candidates = Vec::new();
    for path in file_paths {
        if let Some(mut candidate) = probe_modern_lossy_static(path)? {
            candidate.rel_path = path
                .strip_prefix(src_root)
                .map(|rel| rel.to_string_lossy().to_string())
                .unwrap_or_else(|_| candidate.rel_path);
            candidates.push(candidate);
        }
    }
    candidates.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(candidates)
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
        Err(err) if tool_available => {
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
}
