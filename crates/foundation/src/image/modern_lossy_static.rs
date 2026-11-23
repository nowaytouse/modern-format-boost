//! Content-based static image routing for fast-img encoding and original-format import.
//!
//! Uses the same authoritative-tools-first, cascading-fallback strategy as PNG
//! validation. Each admitted source belongs to exactly one delivery route.
//! Generic HEIF is admitted only after the same positive static/lossy codec
//! probe as every other candidate; its filename brand is never used as proof.

use crate::image_detection::{CompressionType, DetectedFormat, detect_compression};
use crate::unified_error::{ImgQualityError, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
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
        FormatKind::WebP
            | FormatKind::Jp2
            | FormatKind::Jxl
            | FormatKind::Avif
            | FormatKind::Heic
            | FormatKind::Heif
    )
}

const fn detected_format_from_kind(format: FormatKind) -> Option<DetectedFormat> {
    match format {
        FormatKind::Png => Some(DetectedFormat::PNG),
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
    Ok(probe_static(
        path,
        detect_true_format(path)?,
        false,
        &mut crate::live_photo::LivePhotoScan::default(),
    )?
    .map(|(_, candidate)| candidate))
}

/// Check the file content and complete static-image evidence before d=0 encoding.
///
/// # Errors
/// Returns a probe error if a supported container is damaged or changes during inspection.
pub fn probe_fast_img_lossless_static(path: &Path) -> Result<bool> {
    let format = detect_true_format(path)?;
    if !matches!(
        format,
        FormatKind::Png | FormatKind::WebP | FormatKind::Avif
    ) {
        return Ok(false);
    }
    Ok(probe_static(
        path,
        format,
        true,
        &mut crate::live_photo::LivePhotoScan::default(),
    )?
    .is_some_and(|(codec, _)| codec == CompressionType::Lossless))
}

fn probe_static(
    path: &Path,
    format: FormatKind,
    include_lossless: bool,
    live_photos: &mut crate::live_photo::LivePhotoScan,
) -> Result<Option<(CompressionType, ModernLossyStaticCandidate)>> {
    if !(is_modern_static_image_format(format) || include_lossless && format == FormatKind::Png) {
        return Ok(None);
    }
    if live_photos.is_live(path)? {
        tracing::info!(
            target: "modern_lossy_static",
            path = %path.display(),
            "retaining Live Photo asset outside standalone tier-2 import"
        );
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

    let compression = if format == FormatKind::Png {
        if super::png_validation::parse_apng_animation(&read_static_probe_input(path)?)?.is_some() {
            return Ok(None);
        }
        if !crate::image::png_validation::is_true_png(path)? {
            return Err(ImgQualityError::AnalysisError(
                "static PNG failed validation".into(),
            ));
        }
        if crate::image::png_validation::png_heuristic_enabled() {
            detect_compression(&detected, path)?
        } else {
            CompressionType::Lossless
        }
    } else {
        let webp = if format == FormatKind::WebP {
            let Some(codec) = probe_static_webp_compression(path)? else {
                return Ok(None);
            };
            Some(codec)
        } else {
            if !confirmed_static_only(path, &detected)? {
                return Ok(None);
            }
            None
        };
        let codec = detect_modern_compression_authoritative(path, format, &detected, webp)?;
        if include_lossless
            && format == FormatKind::Avif
            && codec == CompressionType::Unknown
            && crate::image::avif_lossless::proven_static_lossless(path)?
        {
            CompressionType::Lossless
        } else {
            codec
        }
    };
    match compression {
        CompressionType::Lossy if format != FormatKind::Png => {}
        CompressionType::Lossless
            if include_lossless
                && matches!(
                    format,
                    FormatKind::Png | FormatKind::WebP | FormatKind::Avif
                ) => {}
        _ => return Ok(None),
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
    Ok(Some((
        compression,
        ModernLossyStaticCandidate {
            path: path.to_path_buf(),
            rel_path,
            format,
            blake3: after_blake3,
        },
    )))
}

/// Result of a tier-2 scan: admitted candidates plus per-file probe failures.
///
/// A probe failure (unreadable or non-standard modern media) quarantines that
/// file with an explicit reason instead of aborting the whole scan — the same
/// retention doctrine as the fast-img source scan.
#[derive(Debug, Default)]
pub struct ModernLossyStaticScan {
    pub candidates: Vec<ModernLossyStaticCandidate>,
    /// Positive source evidence for the separate d=0 encoder, keyed by actual path.
    pub lossless_sources: BTreeMap<PathBuf, String>,
    pub probe_failures: Vec<(PathBuf, String)>,
}

impl ModernLossyStaticScan {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.candidates.is_empty()
            && self.lossless_sources.is_empty()
            && self.probe_failures.is_empty()
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
    scan_static_sources(src_root, file_paths, false)
}

/// Inspect each source once and separate original-format delivery from d=0 encoding.
///
/// # Errors
/// Returns an error for duplicate or unbound source keys.
pub fn scan_fast_img_static_candidates(
    src_root: &Path,
    file_paths: &[PathBuf],
) -> Result<ModernLossyStaticScan> {
    scan_static_sources(src_root, file_paths, true)
}

fn scan_static_sources(
    src_root: &Path,
    file_paths: &[PathBuf],
    include_lossless: bool,
) -> Result<ModernLossyStaticScan> {
    let progress = crate::progress::FileStageProgress::new(
        file_paths.len() as u64,
        crate::progress::FileProgressStage::ModernSourceInspection,
    );
    let mut scan = ModernLossyStaticScan::default();
    let mut relative_paths = BTreeSet::new();
    let mut live_photos = crate::live_photo::LivePhotoScan::default();
    for (index, path) in file_paths.iter().enumerate() {
        crate::batch_control::checkpoint()?;
        let result = detect_true_format(path)
            .and_then(|format| probe_static(path, format, include_lossless, &mut live_photos));
        // Inspection counts every completed probe, including skips and failures.
        progress.set((index + 1) as u64);
        match result {
            Ok(Some((compression, candidate))) => {
                let rel_path = crate::pipeline::verification::fast_img_relative_key(
                    path,
                    src_root,
                    "Tier-2 source",
                )
                .map_err(ImgQualityError::AnalysisError)?;
                if !relative_paths.insert(rel_path.clone()) {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "modern lossy static scan produced duplicate relative path {rel_path}"
                    )));
                }
                if compression == CompressionType::Lossless {
                    scan.lossless_sources
                        .insert(candidate.path, candidate.blake3);
                } else {
                    scan.candidates.push(ModernLossyStaticCandidate {
                        rel_path,
                        ..candidate
                    });
                }
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
    live_photos.verify_unchanged()?;
    scan.probe_failures.sort();
    progress.finish();
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

pub(super) fn read_static_probe_input(path: &Path) -> Result<Vec<u8>> {
    let limit = crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE;
    crate::common_utils::validate_file_size_limit(path, limit)
        .map_err(|error| ImgQualityError::AnalysisError(error.to_string()))?;
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut data)?;
    let read_bytes =
        crate::numeric_cast::usize_to_u64_strict(data.len(), "static_probe_read_length")
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "Static image read byte count cannot be represented".to_string(),
                )
            })?;
    if read_bytes > limit {
        return Err(ImgQualityError::AnalysisError(format!(
            "Static image exceeded the {limit}-byte analysis limit while reading {}",
            path.display()
        )));
    }
    Ok(data)
}

fn probe_static_webp_compression(path: &Path) -> Result<Option<CompressionType>> {
    let data = read_static_probe_input(path)?;
    let has_animation_chunks =
        crate::image_formats::webp::contains_top_level_chunk(&data, *b"ANIM")?
            || crate::image_formats::webp::contains_top_level_chunk(&data, *b"ANMF")?;
    let features = ::webp::BitstreamFeatures::new(&data).ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "WebP static admission requires valid native bitstream features: {}",
            path.display()
        ))
    })?;
    if features.has_animation() || has_animation_chunks {
        tracing::debug!(
            target: "modern_lossy_static",
            path = %path.display(),
            "retaining animated WebP outside static-only delivery"
        );
        return Ok(None);
    }
    if features.width() == 0 || features.height() == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "WebP static admission requires nonzero native dimensions: {}",
            path.display()
        )));
    }
    let compression = match features.format() {
        Some(::webp::BitstreamFormat::Lossy) => CompressionType::Lossy,
        Some(::webp::BitstreamFormat::Lossless) => CompressionType::Lossless,
        Some(::webp::BitstreamFormat::Undefined) | None => {
            tracing::debug!(
                target: "modern_lossy_static",
                path = %path.display(),
                "retaining WebP without positive static codec evidence"
            );
            return Ok(None);
        }
    };
    // Header features establish codec semantics, not complete pixel validity.
    let decoded = crate::image_detection::open_image_with_limits(path).map_err(|error| {
        ImgQualityError::AnalysisError(format!(
            "WebP static admission could not establish a complete bounded decode for {}: {error}",
            path.display()
        ))
    })?;
    if decoded.width() != features.width() || decoded.height() != features.height() {
        return Err(ImgQualityError::AnalysisError(format!(
            "WebP decoded dimensions disagree with native bitstream features: {}",
            path.display()
        )));
    }
    Ok(Some(compression))
}

fn detect_modern_compression_authoritative(
    path: &Path,
    format: FormatKind,
    detected: &DetectedFormat,
    webp_compression: Option<CompressionType>,
) -> Result<CompressionType> {
    let forensic_tool =
        super::format_detect::forensic_tool_for_format(format).map(|policy| policy.tool);
    let tool_available = forensic_tool
        .and_then(crate::common_utils::resolve_tool_path)
        .is_some();
    let in_process_validator_available =
        cfg!(feature = "v1_21") && matches!(format, FormatKind::Heic | FormatKind::Heif);
    let forensic_validator_available = tool_available || in_process_validator_available;

    // JXL's compression classifier already performs an in-process structural
    // decode and, for Modular streams, one bounded jxlinfo query. Avoid a
    // duplicate jxlinfo spawn here.
    if format == FormatKind::Jxl {
        return detect_compression(detected, path);
    }

    // A codec configuration can describe subsampling without proving that
    // the image payload is decodable. Require forensic validation for AVIF and JP2.
    if matches!(format, FormatKind::Avif | FormatKind::Jp2) && !tool_available {
        tracing::debug!(
            target: "modern_lossy_static",
            path = %path.display(),
            format = ?format,
            "retaining image because authoritative decode validation is unavailable"
        );
        return Ok(CompressionType::Unknown);
    }

    require_forensic_result_when_available(
        path,
        format,
        forensic_validator_available.then(|| validate_format_forensic(path, format)),
    )?;

    if format == FormatKind::WebP {
        return webp_compression.ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "WebP compression admission is missing complete native static proof: {}",
                path.display()
            ))
        });
    }
    detect_compression(detected, path)
}

fn require_forensic_result_when_available(
    path: &Path,
    format: FormatKind,
    validation: Option<Result<super::format_detect::ForensicFormatCheck>>,
) -> Result<()> {
    match validation {
        Some(Ok(check)) => {
            tracing::debug!(
                target: "modern_lossy_static",
                path = %path.display(),
                format = ?format,
                tool = %check.tool,
                "forensic format validation passed before compression probe"
            );
        }
        Some(Err(err)) => {
            return Err(ImgQualityError::AnalysisError(format!(
                "forensic validation rejected modern format candidate {} ({format:?}): {err}",
                path.display()
            )));
        }
        None => {
            tracing::debug!(
                target: "modern_lossy_static",
                path = %path.display(),
                format = ?format,
                "forensic tool unavailable; continuing only with the route's native compression evidence"
            );
        }
    }

    Ok(())
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
        assert!(is_modern_static_image_format(FormatKind::Heif));
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
    fn modern_webp_requires_native_codec_and_complete_decode_evidence() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let pixels = (0..16 * 16 * 3)
            .map(|sample| u8::try_from(sample % 251).expect("bounded synthetic sample"))
            .collect::<Vec<_>>();
        let lossy = ::webp::Encoder::from_rgb(&pixels, 16, 16)
            .encode(70.0)
            .to_vec();
        let lossless = ::webp::Encoder::from_rgb(&pixels, 16, 16)
            .encode_lossless()
            .to_vec();
        let lossy_path = directory.path().join("lossy.webp");
        let lossless_path = directory.path().join("lossless.webp");
        std::fs::write(&lossy_path, &lossy)?;
        std::fs::write(&lossless_path, &lossless)?;
        assert_eq!(
            probe_static_webp_compression(&lossy_path)?,
            Some(CompressionType::Lossy)
        );
        assert_eq!(
            probe_static_webp_compression(&lossless_path)?,
            Some(CompressionType::Lossless)
        );
        assert!(probe_modern_lossy_static(&lossy_path)?.is_some());
        assert!(probe_modern_lossy_static(&lossless_path)?.is_none());
        let scan = scan_fast_img_static_candidates(
            directory.path(),
            &[lossy_path.clone(), lossless_path.clone()],
        )?;
        assert!(scan.probe_failures.is_empty());
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(scan.candidates[0].path, lossy_path);
        assert_eq!(scan.lossless_sources.len(), 1);
        assert!(scan.lossless_sources.contains_key(&lossless_path));

        assert_eq!(&lossy[12..16], b"VP8 ");
        let payload_len = usize::try_from(u32::from_le_bytes(
            lossy[16..20].try_into().expect("VP8 chunk length"),
        ))
        .expect("synthetic payload length fits usize");
        assert!(payload_len <= 16 * 1024, "bounded synthetic prefix search");
        let payload = lossy
            .get(20..20 + payload_len)
            .expect("complete encoder-produced VP8 chunk");
        let incomplete = (1..payload_len)
            .find_map(|prefix_len| {
                let padded_len = prefix_len + (prefix_len & 1);
                let mut candidate = lossy[..20].to_vec();
                candidate[4..8].copy_from_slice(
                    &u32::try_from(12 + padded_len)
                        .expect("bounded RIFF size")
                        .to_le_bytes(),
                );
                candidate[16..20].copy_from_slice(
                    &u32::try_from(prefix_len)
                        .expect("bounded VP8 prefix size")
                        .to_le_bytes(),
                );
                candidate.extend_from_slice(&payload[..prefix_len]);
                if prefix_len & 1 != 0 {
                    candidate.push(0);
                }
                let features = ::webp::BitstreamFeatures::new(&candidate)?;
                (matches!(features.format(), Some(::webp::BitstreamFormat::Lossy))
                    && !features.has_animation()
                    && ::webp::Decoder::new(&candidate).decode().is_none())
                .then_some(candidate)
            })
            .expect(
                "a strict VP8 prefix must retain native features while failing complete decode",
            );
        let features = ::webp::BitstreamFeatures::new(&incomplete)
            .expect("truncated fixture retains readable native VP8 header");
        assert!(matches!(
            features.format(),
            Some(::webp::BitstreamFormat::Lossy)
        ));
        assert!(::webp::Decoder::new(&incomplete).decode().is_none());
        let incomplete_path = directory.path().join("header-only.webp");
        std::fs::write(&incomplete_path, &incomplete)?;
        assert!(probe_modern_lossy_static(&incomplete_path).is_err());
        assert_eq!(std::fs::read(&incomplete_path)?, incomplete);
        assert_eq!(std::fs::read(&lossy_path)?, lossy);
        assert_eq!(std::fs::read(&lossless_path)?, lossless);
        Ok(())
    }

    #[test]
    fn lossless_png_uses_content_and_rejects_partial_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("synthetic.jpg");
        image::RgbaImage::new(3, 5).save_with_format(&file, image::ImageFormat::Png)?;
        assert!(probe_fast_img_lossless_static(&file)?);
        let mut bytes = std::fs::read(&file)?;
        bytes.pop();
        std::fs::write(&file, bytes)?;
        assert!(probe_fast_img_lossless_static(&file).is_err());
        Ok(())
    }

    #[test]
    fn modern_webp_empty_bitstream_is_quarantined_not_admitted_from_chunk_name() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("empty-vp8.webp");
        let bytes = b"RIFF\x0c\0\0\0WEBPVP8 \0\0\0\0";
        std::fs::write(&path, bytes)?;
        assert!(::webp::BitstreamFeatures::new(bytes).is_none());
        let scan =
            scan_modern_lossy_static_candidates(directory.path(), std::slice::from_ref(&path))?;
        assert_eq!(scan.candidates.len(), 0);
        assert_eq!(scan.probe_failures.len(), 1);
        assert_eq!(scan.probe_failures[0].0, path);
        assert_eq!(std::fs::read(&path)?, bytes);
        Ok(())
    }

    #[test]
    fn modern_webp_animation_stays_outside_static_admission() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("animation.webp");
        let red = [255, 0, 0].repeat(16 * 16);
        let blue = [0, 0, 255].repeat(16 * 16);
        let green = [0, 255, 0].repeat(16 * 16);
        let config = ::webp::WebPConfig::new().expect("native WebP config");
        let mut encoder = ::webp::AnimEncoder::new(16, 16, &config);
        encoder.add_frame(::webp::AnimFrame::from_rgb(&red, 16, 16, 0));
        encoder.add_frame(::webp::AnimFrame::from_rgb(&blue, 16, 16, 80));
        encoder.add_frame(::webp::AnimFrame::from_rgb(&green, 16, 16, 160));
        let bytes = encoder.try_encode().expect("synthetic animation").to_vec();
        assert!(
            ::webp::BitstreamFeatures::new(&bytes)
                .expect("animated header")
                .has_animation()
        );
        std::fs::write(&path, &bytes)?;
        assert!(probe_static_webp_compression(&path)?.is_none());
        assert!(probe_modern_lossy_static(&path)?.is_none());
        assert_eq!(std::fs::read(&path)?, bytes);
        Ok(())
    }

    #[test]
    fn modern_forensic_rejection_remains_an_error_when_native_evidence_exists() {
        let path = Path::new("synthetic.webp");
        let rejected =
            || ImgQualityError::AnalysisError("explicit validator rejection".to_string());
        let error =
            require_forensic_result_when_available(path, FormatKind::WebP, Some(Err(rejected())))
                .expect_err("available forensic validation cannot be replaced by native success");
        assert!(error.to_string().contains("explicit validator rejection"));
        assert!(require_forensic_result_when_available(path, FormatKind::WebP, None,).is_ok());
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
    fn modern_source_inspection_reports_classified_inventory_without_admitting_nonmodern_files()
    -> anyhow::Result<()> {
        const CHILD: &str = "MFB_MODERN_SOURCE_INSPECTION_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let directory = tempfile::tempdir()?;
            let jpeg_bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
            let paths = [
                directory.path().join("first.jpg"),
                directory.path().join("second.jpg"),
            ];
            for path in &paths {
                std::fs::write(path, jpeg_bytes)?;
            }
            let scan = scan_modern_lossy_static_candidates(directory.path(), &paths)?;
            assert!(
                scan.candidates.is_empty(),
                "inspection must preserve admission"
            );
            assert!(
                scan.probe_failures.is_empty(),
                "non-modern files remain skips"
            );
            for path in &paths {
                assert_eq!(std::fs::read(path)?, jpeg_bytes);
            }
            return Ok(());
        }

        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "image::modern_lossy_static::tests::modern_source_inspection_reports_classified_inventory_without_admitting_nonmodern_files",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("MFB_SESSION_ID", "synthetic-modern-source-inspection")
            .output()?;
        assert!(
            output.status.success(),
            "child scan failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout)?;
        let events = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("MFB_PROGRESS="))
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert!(
            (2..=4).contains(&events.len()),
            "two inspected files need bounded start/update/finish events: {stdout}"
        );
        let first = events.first().expect("start progress event");
        assert_eq!(first["stage"], "modern_source_inspection");
        assert_eq!(first["processed"], 0);
        assert_eq!(first["state"], "running");
        let stage_id = first["stage_id"].as_str().expect("stage identity");
        assert_ne!(stage_id, "");
        let mut previous = 0;
        for event in &events {
            assert_eq!(event["schema_version"], 1);
            assert_eq!(event["stage_id"], stage_id);
            assert_eq!(event["stage"], "modern_source_inspection");
            assert_eq!(event["total"], 2);
            let processed = event["processed"].as_u64().expect("inspected file count");
            assert!((previous..=2).contains(&processed));
            previous = processed;
            assert!(event.get("success").is_none());
            assert!(event.get("path").is_none());
        }
        let last = events.last().expect("finished progress event");
        assert_eq!(last["processed"], 2, "skipped files are still inspected");
        assert_eq!(last["state"], "finished");
        assert_eq!(
            events
                .iter()
                .filter(|event| event["state"] == "finished")
                .count(),
            1
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
