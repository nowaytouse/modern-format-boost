//! Unified format identity: content-based family plus provenance and, when
//! the internal detector cannot resolve a file, an optional PRONOM identity
//! from the Siegfried sidecar.
//!
//! Locked boundaries:
//! - **Identity ≠ validity ≠ loss state.** This model answers "what is this
//!   file?"; health verification stays with the per-format validators and
//!   compression semantics stay with [`crate::image_detection::CompressionType`].
//! - The extension is a **hint** only. Content evidence (magic bytes, PRONOM
//!   byte/container signatures) always wins; a mismatch is recorded, never
//!   acted on (no rename, no pipeline abort).
//! - `KnownFormat + Unsupported` and `UnknownFormat` are distinct outcomes
//!   (see [`SupportLevel`]).

use super::format_detect::{FormatKind, detect_true_format};
use super::siegfried::{
    SiegfriedFileReport, SiegfriedMatch, SiegfriedProbe, identify_paths, puid_to_format_kind,
};
use crate::unified_error::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Who produced the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionSource {
    /// Internal magic-byte/container signature detector.
    InternalSignature,
    /// Siegfried + PRONOM external identification.
    SiegfriedPronom,
    /// Internal detector plus corroborating PRONOM evidence.
    Combined,
}

/// How strong the identification evidence is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionConfidence {
    /// Content signature matched (internal magic or PRONOM byte/container).
    Confirmed,
    /// Only weaker external evidence (single non-signature match).
    Likely,
    /// Multiple PRONOM candidates; not disambiguated.
    Ambiguous,
    /// Extension evidence only — never promotable to `Confirmed`.
    ExtensionOnly,
    /// No usable evidence.
    Unknown,
}

/// Stable external identity from PRONOM (PUID is the machine identity; the
/// human-readable name is informational).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PronomIdentity {
    pub puid: String,
    pub name: String,
    pub version: String,
    pub mime: String,
    pub class: String,
    pub basis: String,
    pub warning: String,
}

impl PronomIdentity {
    fn from_match(m: &SiegfriedMatch) -> Self {
        Self {
            puid: m.id.clone(),
            name: m.format_name.clone(),
            version: m.version.clone(),
            mime: m.mime.clone(),
            class: m.class.clone(),
            basis: m.basis.clone(),
            warning: m.warning.clone(),
        }
    }

    fn is_extension_only(&self) -> bool {
        (self.basis.contains("extension match") && !self.basis.contains("byte match"))
            || self.warning.to_ascii_lowercase().contains("extension only")
    }
}

/// Combined identity for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatIdentity {
    /// Internal family. `Unknown` here still means "no internal family" even
    /// when `pronom` carries an external identity.
    pub family: FormatKind,
    pub source: DetectionSource,
    pub confidence: DetectionConfidence,
    /// Best MIME seen (internal families map to canonical MIME; PRONOM may
    /// supply one for otherwise unknown files).
    pub mime: Option<String>,
    pub extension_hint: Option<String>,
    /// Extension disagrees with the content-derived family. Diagnostic only.
    pub extension_mismatch: bool,
    /// Every PRONOM candidate, in sidecar order. Ambiguous results are never
    /// collapsed to an arbitrary representative.
    pub pronom: Vec<PronomIdentity>,
}

/// Processing stance derived from identity — "identified" never implies
/// "processable".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportLevel {
    /// Detected and convertible by this project's img pipeline.
    FullySupported,
    /// Identified (possibly only externally) but outside the conversion set.
    DetectOnly,
    /// Known container this project deliberately does not process (video in
    /// img, etc.).
    Unsupported,
    /// No identity at all.
    Unknown,
}

#[must_use]
pub fn support_level(identity: &FormatIdentity) -> SupportLevel {
    use FormatKind as F;
    match identity.family {
        F::Jpeg
        | F::Png
        | F::WebP
        | F::Avif
        | F::Heic
        | F::Heif
        | F::Jxl
        | F::Gif
        | F::Tiff
        | F::Jp2 => SupportLevel::FullySupported,
        F::Bmp | F::Qoi | F::Ico | F::Exr | F::Flif | F::Psd | F::Pnm | F::Dds => {
            SupportLevel::DetectOnly
        }
        F::Mp4 | F::Mov | F::Mkv | F::Webm => SupportLevel::Unsupported,
        F::Unknown => {
            if identity.pronom.iter().any(|item| !item.is_extension_only()) {
                SupportLevel::DetectOnly
            } else {
                SupportLevel::Unknown
            }
        }
    }
}

fn extension_hint(path: &Path) -> Option<String> {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
}

const fn canonical_mime(family: FormatKind) -> Option<&'static str> {
    use FormatKind as F;
    Some(match family {
        F::Jpeg => "image/jpeg",
        F::Png => "image/png",
        F::WebP => "image/webp",
        F::Avif => "image/avif",
        F::Heic => "image/heic",
        F::Heif => "image/heif",
        F::Jxl => "image/jxl",
        F::Gif => "image/gif",
        F::Tiff => "image/tiff",
        F::Jp2 => "image/jp2",
        _ => return None,
    })
}

fn extension_mismatches(family: FormatKind, hint: Option<&str>) -> bool {
    match (family.valid_extensions(), hint) {
        (valid, Some(hint)) if !valid.is_empty() => !valid.contains(&hint),
        // No extension or no canonical extension cannot disagree.
        _ => false,
    }
}

/// Classify external evidence into a confidence level. Multiple matches stay
/// `Ambiguous`; extension-only matches stay `ExtensionOnly`.
fn pronom_confidence(report: &SiegfriedFileReport) -> (DetectionConfidence, usize) {
    let count = report.matches.len();
    let confidence = match count {
        0 => DetectionConfidence::Unknown,
        1 => {
            if report.matches[0].is_extension_only() {
                DetectionConfidence::ExtensionOnly
            } else {
                DetectionConfidence::Confirmed
            }
        }
        _ => DetectionConfidence::Ambiguous,
    };
    (confidence, count)
}

/// Resolve the identity of `path`.
///
/// Fast path: the internal magic-byte detector alone (no sidecar spawn).
/// The Siegfried fallback runs only when the internal detector cannot name
/// the family, or when the extension disagrees with the content — the
/// suspicious/unknown cases. A missing or failing sidecar degrades to the
/// internal result; it never fails this call.
///
/// # Errors
/// Propagates only IO failures from reading the file header.
fn internal_identity(path: &Path) -> Result<FormatIdentity> {
    let internal_family = detect_true_format(path)?;
    let hint = extension_hint(path);
    let internal_mismatch = extension_mismatches(internal_family, hint.as_deref());
    Ok(FormatIdentity {
        family: internal_family,
        source: DetectionSource::InternalSignature,
        confidence: if internal_family == FormatKind::Unknown {
            DetectionConfidence::Unknown
        } else {
            DetectionConfidence::Confirmed
        },
        mime: canonical_mime(internal_family).map(str::to_string),
        extension_hint: hint,
        extension_mismatch: internal_mismatch,
        pronom: Vec::new(),
    })
}

fn merge_pronom_report(identity: &mut FormatIdentity, report: &SiegfriedFileReport) {
    let (confidence, _) = pronom_confidence(report);
    identity.pronom = report
        .matches
        .iter()
        .map(PronomIdentity::from_match)
        .collect();
    // Only one content-backed match is eligible to enrich the machine
    // identity. Multiple matches remain Ambiguous and an extension-only match
    // remains a diagnostic hint.
    if report.matches.len() == 1 && !report.matches[0].is_extension_only() {
        let m = &report.matches[0];
        if identity.mime.is_none() && !m.mime.is_empty() {
            identity.mime = Some(m.mime.clone());
        }
        if identity.family == FormatKind::Unknown
            && let Some(family) = puid_to_format_kind(&m.id)
        {
            identity.family = family;
            identity.source = DetectionSource::SiegfriedPronom;
            identity.confidence = confidence;
        }
    }
    // Internal magic evidence outranks PRONOM for known families; external
    // data is Combined only when it actually corroborates the same family.
    if identity.source == DetectionSource::InternalSignature
        && identity.family != FormatKind::Unknown
        && report.matches.len() == 1
        && !report.matches[0].is_extension_only()
        && puid_to_format_kind(&report.matches[0].id) == Some(identity.family)
    {
        identity.source = DetectionSource::Combined;
    }
}

/// Resolve a batch with one batched Siegfried fallback for every
/// unknown/extension-mismatched path.
pub fn resolve_format_identities(paths: &[std::path::PathBuf]) -> Result<Vec<FormatIdentity>> {
    let mut identities = paths
        .iter()
        .map(|path| internal_identity(path))
        .collect::<Result<Vec<_>>>()?;
    let external_paths = paths
        .iter()
        .zip(&identities)
        .filter(|(_, identity)| {
            identity.family == FormatKind::Unknown || identity.extension_mismatch
        })
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    if external_paths.is_empty() {
        return Ok(identities);
    }

    match identify_paths(&external_paths)? {
        SiegfriedProbe::Identified { files, .. } => {
            let reports = files
                .iter()
                .map(|report| (report.filename.as_str(), report))
                .collect::<BTreeMap<_, _>>();
            for (path, identity) in paths.iter().zip(&mut identities) {
                if identity.family != FormatKind::Unknown && !identity.extension_mismatch {
                    continue;
                }
                if let Some(report) = reports.get(path.to_string_lossy().as_ref()) {
                    merge_pronom_report(identity, report);
                }
            }
        }
        SiegfriedProbe::Unavailable { reason } => tracing::debug!(
            target: "format_identity",
            reason = %reason,
            "external batch identification unavailable; internal results retained"
        ),
    }
    Ok(identities)
}

/// Resolve one path. Batch callers should use [`resolve_format_identities`] to
/// keep the optional sidecar at one process per bounded batch.
pub fn resolve_format_identity(path: &Path) -> Result<FormatIdentity> {
    resolve_format_identities(&[path.to_path_buf()])?
        .pop()
        .ok_or_else(|| {
            crate::unified_error::ImgQualityError::AnalysisError(
                "format identity resolver returned no result".to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_BY_ONE_RGBA_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn internal_identity_is_fast_path_without_sidecar() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plain.png");
        std::fs::write(&path, ONE_BY_ONE_RGBA_PNG).expect("write png");

        let identity = resolve_format_identity(&path).expect("resolve identity");
        assert_eq!(identity.family, FormatKind::Png);
        assert_eq!(identity.source, DetectionSource::InternalSignature);
        assert_eq!(identity.confidence, DetectionConfidence::Confirmed);
        assert!(!identity.extension_mismatch);
        assert_eq!(identity.mime.as_deref(), Some("image/png"));
        assert_eq!(support_level(&identity), SupportLevel::FullySupported);
    }

    #[test]
    fn extension_mismatch_is_recorded_but_content_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("masquerade.jpg");
        std::fs::write(&path, ONE_BY_ONE_RGBA_PNG).expect("write png bytes");

        let identity = resolve_format_identity(&path).expect("resolve identity");
        // Content wins over the extension hint; mismatch is diagnostic only.
        assert_eq!(identity.family, FormatKind::Png);
        assert!(identity.extension_mismatch);
        assert_eq!(identity.extension_hint.as_deref(), Some("jpg"));
    }

    #[test]
    fn valid_extension_alias_is_not_reported_as_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("photo.jpeg");
        std::fs::write(&path, [0xFF, 0xD8, 0xFF, 0xD9]).expect("write jpeg bytes");

        let identity = resolve_format_identity(&path).expect("resolve identity");
        assert_eq!(identity.family, FormatKind::Jpeg);
        assert!(
            !identity.extension_mismatch,
            "a registered extension alias must not trigger external fallback"
        );
    }

    #[test]
    fn extensionless_file_is_identified_by_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("noextension");
        let file = tempfile::NamedTempFile::new().expect("temp");
        drop(file);
        std::fs::write(&path, ONE_BY_ONE_RGBA_PNG).expect("write png bytes");

        let identity = resolve_format_identity(&path).expect("resolve identity");
        assert_eq!(identity.family, FormatKind::Png);
        assert!(identity.extension_hint.is_none());
        assert!(!identity.extension_mismatch);
    }

    #[test]
    fn garbage_content_stays_unknown_without_fabrication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("garbage.bin");
        std::fs::write(&path, [0xA5u8; 64]).expect("write garbage");

        let identity = resolve_format_identity(&path).expect("resolve identity");
        // The internal family is never fabricated. When sf is installed,
        // PRONOM's catch-all ("Binary File", fmt/208) supplies an external
        // identity → DetectOnly; without sf the level stays Unknown.
        assert_eq!(identity.family, FormatKind::Unknown);
        assert!(matches!(
            support_level(&identity),
            SupportLevel::DetectOnly | SupportLevel::Unknown
        ));
    }

    #[test]
    fn support_policy_separates_detectable_and_processable() {
        let mut identity = FormatIdentity {
            family: FormatKind::Bmp,
            source: DetectionSource::InternalSignature,
            confidence: DetectionConfidence::Confirmed,
            mime: None,
            extension_hint: Some("bmp".to_string()),
            extension_mismatch: false,
            pronom: Vec::new(),
        };
        assert_eq!(support_level(&identity), SupportLevel::DetectOnly);

        identity.family = FormatKind::Mp4;
        assert_eq!(support_level(&identity), SupportLevel::Unsupported);

        // Externally identified but internally unknown: known-format +
        // unsupported, never conflated with unknown-format.
        identity.family = FormatKind::Unknown;
        identity.pronom = vec![PronomIdentity {
            puid: "fmt/999".to_string(),
            name: "Some Ancient Raster Format".to_string(),
            version: String::new(),
            mime: "image/x-ancient".to_string(),
            class: "Image (Raster)".to_string(),
            basis: "byte match at 0".to_string(),
            warning: String::new(),
        }];
        assert_eq!(support_level(&identity), SupportLevel::DetectOnly);
    }

    #[test]
    fn ambiguous_pronom_matches_never_upgrade_unknown_family() {
        let mut identity = FormatIdentity {
            family: FormatKind::Unknown,
            source: DetectionSource::InternalSignature,
            confidence: DetectionConfidence::Unknown,
            mime: None,
            extension_hint: Some("bin".to_string()),
            extension_mismatch: false,
            pronom: Vec::new(),
        };
        let report = SiegfriedFileReport {
            filename: "ambiguous.bin".to_string(),
            errors: String::new(),
            matches: vec![
                SiegfriedMatch {
                    id: "fmt/43".to_string(),
                    basis: "byte match at 0".to_string(),
                    ..SiegfriedMatch::default()
                },
                SiegfriedMatch {
                    id: "fmt/11".to_string(),
                    basis: "byte match at 0".to_string(),
                    ..SiegfriedMatch::default()
                },
            ],
        };
        merge_pronom_report(&mut identity, &report);
        assert_eq!(identity.family, FormatKind::Unknown);
        assert_eq!(identity.source, DetectionSource::InternalSignature);
        assert_eq!(identity.pronom.len(), 2);
        assert_ne!(support_level(&identity), SupportLevel::FullySupported);
    }

    #[test]
    fn extension_only_pronom_match_is_diagnostic_only() {
        let mut identity = FormatIdentity {
            family: FormatKind::Unknown,
            source: DetectionSource::InternalSignature,
            confidence: DetectionConfidence::Unknown,
            mime: None,
            extension_hint: Some("jxl".to_string()),
            extension_mismatch: false,
            pronom: Vec::new(),
        };
        let report = SiegfriedFileReport {
            filename: "hint.jxl".to_string(),
            errors: String::new(),
            matches: vec![SiegfriedMatch {
                id: "fmt/1484".to_string(),
                basis: "extension match jxl".to_string(),
                ..SiegfriedMatch::default()
            }],
        };
        merge_pronom_report(&mut identity, &report);
        assert_eq!(identity.family, FormatKind::Unknown);
        assert_eq!(support_level(&identity), SupportLevel::Unknown);
    }
}
