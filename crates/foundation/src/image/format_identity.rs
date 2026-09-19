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
    SiegfriedFileReport, SiegfriedMatch, SiegfriedProbe, has_content_evidence, identify_paths,
    puid_to_format_kind,
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
    pub namespace: String,
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
            namespace: m.ns.clone(),
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
        (self.basis.contains("extension match") && !has_content_evidence(&self.basis))
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
    /// Sidecar execution, missing-report or per-file scan failure. Rejected
    /// candidates remain in `pronom` for diagnostics, never for promotion.
    pub external_error: Option<String>,
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
            if identity.external_error.is_none()
                && identity.pronom.iter().any(|item| !item.is_extension_only())
            {
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
fn pronom_confidence(report: &SiegfriedFileReport) -> DetectionConfidence {
    match report.matches.as_slice() {
        [] => DetectionConfidence::Unknown,
        [candidate] => {
            if candidate.is_extension_only() {
                DetectionConfidence::ExtensionOnly
            } else if has_content_evidence(&candidate.basis) {
                DetectionConfidence::Confirmed
            } else {
                DetectionConfidence::Likely
            }
        }
        _ => DetectionConfidence::Ambiguous,
    }
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
        external_error: None,
    })
}

fn merge_pronom_report(identity: &mut FormatIdentity, report: &SiegfriedFileReport) {
    identity.pronom = report
        .matches
        .iter()
        .map(PronomIdentity::from_match)
        .collect();
    identity.external_error = (!report.errors.is_empty()).then(|| report.errors.clone());
    if identity.external_error.is_some() {
        return;
    }
    let confidence = pronom_confidence(report);
    if identity.family == FormatKind::Unknown {
        identity.confidence = confidence;
        if !report.matches.is_empty() {
            identity.source = DetectionSource::SiegfriedPronom;
        }
    }
    // Only one content-backed match is eligible to enrich the machine
    // identity. Weak, ambiguous and extension-only evidence stays diagnostic.
    if confidence != DetectionConfidence::Confirmed {
        return;
    }
    let m = &report.matches[0];
    // PUIDs are meaningful only inside their namespace, including when sf
    // is configured with additional/custom signature identifiers.
    let external_family = if m.ns == "pronom" {
        puid_to_format_kind(&m.id)
    } else {
        None
    };
    if identity.family == FormatKind::Unknown {
        if !m.mime.is_empty() {
            identity.mime = Some(m.mime.clone());
        }
        if let Some(family) = external_family {
            identity.family = family;
            identity.extension_mismatch =
                extension_mismatches(family, identity.extension_hint.as_deref());
        }
    }
    // Internal magic evidence outranks PRONOM for known families; external
    // data is Combined only when it actually corroborates the same family.
    if identity.source == DetectionSource::InternalSignature
        && identity.family != FormatKind::Unknown
        && external_family == Some(identity.family)
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

    merge_siegfried_probe(paths, &mut identities, &identify_paths(&external_paths)?);
    Ok(identities)
}

fn merge_siegfried_probe(
    paths: &[std::path::PathBuf],
    identities: &mut [FormatIdentity],
    probe: &SiegfriedProbe,
) {
    let reports = match probe {
        SiegfriedProbe::Identified { files, .. } => files
            .iter()
            .map(|report| (report.filename.as_str(), report))
            .collect::<BTreeMap<_, _>>(),
        SiegfriedProbe::Unavailable { .. } => BTreeMap::new(),
    };
    for (path, identity) in paths.iter().zip(identities) {
        if identity.family != FormatKind::Unknown && !identity.extension_mismatch {
            continue;
        }
        if let Some(report) = reports.get(path.to_string_lossy().as_ref()) {
            merge_pronom_report(identity, report);
        } else {
            identity.external_error = Some(match probe {
                SiegfriedProbe::Unavailable { reason } => reason.clone(),
                SiegfriedProbe::Identified { .. } => {
                    "sf produced no report entry for this path".to_string()
                }
            });
        }
        if let Some(reason) = &identity.external_error {
            tracing::warn!(
                target: "format_identity",
                path = %path.display(),
                reason = %reason,
                "external identification failed; internal identity retained"
            );
        }
    }
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
    use crate::siegfried::SiegfriedMeta;

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
            external_error: None,
        };
        assert_eq!(support_level(&identity), SupportLevel::DetectOnly);

        identity.family = FormatKind::Mp4;
        assert_eq!(support_level(&identity), SupportLevel::Unsupported);

        // Externally identified but internally unknown: known-format +
        // unsupported, never conflated with unknown-format.
        identity.family = FormatKind::Unknown;
        identity.pronom = vec![PronomIdentity {
            namespace: "pronom".to_string(),
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
            external_error: None,
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
        assert_eq!(identity.source, DetectionSource::SiegfriedPronom);
        assert_eq!(identity.confidence, DetectionConfidence::Ambiguous);
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
            external_error: None,
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
        assert_eq!(identity.confidence, DetectionConfidence::ExtensionOnly);
        assert_eq!(support_level(&identity), SupportLevel::Unknown);
    }

    fn unknown_identity() -> FormatIdentity {
        FormatIdentity {
            family: FormatKind::Unknown,
            source: DetectionSource::InternalSignature,
            confidence: DetectionConfidence::Unknown,
            mime: None,
            extension_hint: Some("bin".to_string()),
            extension_mismatch: false,
            pronom: Vec::new(),
            external_error: None,
        }
    }

    #[test]
    fn single_pronom_candidate_requires_positive_content_evidence() {
        for (basis, expected_confidence, expected_family) in [
            ("", DetectionConfidence::Likely, FormatKind::Unknown),
            (
                "name match",
                DetectionConfidence::Likely,
                FormatKind::Unknown,
            ),
            (
                "byte match at 0, 8",
                DetectionConfidence::Confirmed,
                FormatKind::Png,
            ),
            (
                "container match with PNG",
                DetectionConfidence::Confirmed,
                FormatKind::Png,
            ),
            (
                "extension match png; container match with PNG",
                DetectionConfidence::Confirmed,
                FormatKind::Png,
            ),
        ] {
            let mut identity = unknown_identity();
            let report = SiegfriedFileReport {
                matches: vec![SiegfriedMatch {
                    ns: "pronom".to_string(),
                    id: "fmt/11".to_string(),
                    mime: "image/png".to_string(),
                    basis: basis.to_string(),
                    ..SiegfriedMatch::default()
                }],
                ..SiegfriedFileReport::default()
            };
            merge_pronom_report(&mut identity, &report);
            assert_eq!(identity.confidence, expected_confidence, "basis: {basis}");
            assert_eq!(identity.family, expected_family, "basis: {basis}");
            if expected_family == FormatKind::Unknown {
                assert!(identity.mime.is_none(), "weak evidence must not set MIME");
            } else {
                assert!(
                    identity.extension_mismatch,
                    "promoted family must recheck extension"
                );
            }
        }
    }

    #[test]
    fn unmapped_pronom_identity_preserves_confidence_without_enabling_conversion() {
        let mut identity = unknown_identity();
        let report = SiegfriedFileReport {
            matches: vec![SiegfriedMatch {
                ns: "pronom".to_string(),
                id: "fmt/999999".to_string(),
                mime: "application/x-unknown-family".to_string(),
                basis: "byte match at 0, 8".to_string(),
                ..SiegfriedMatch::default()
            }],
            ..SiegfriedFileReport::default()
        };
        merge_pronom_report(&mut identity, &report);
        assert_eq!(identity.confidence, DetectionConfidence::Confirmed);
        assert_eq!(identity.source, DetectionSource::SiegfriedPronom);
        assert_eq!(identity.family, FormatKind::Unknown);
        assert_eq!(support_level(&identity), SupportLevel::DetectOnly);
    }

    #[test]
    fn errored_pronom_report_cannot_promote_or_corroborate_identity() {
        let report = SiegfriedFileReport {
            errors: "failed to read the complete file".to_string(),
            matches: vec![SiegfriedMatch {
                ns: "pronom".to_string(),
                id: "fmt/11".to_string(),
                mime: "image/png".to_string(),
                basis: "byte match at 0, 8".to_string(),
                ..SiegfriedMatch::default()
            }],
            ..SiegfriedFileReport::default()
        };
        let mut identity = unknown_identity();
        merge_pronom_report(&mut identity, &report);
        assert_eq!(identity.family, FormatKind::Unknown);
        assert_eq!(identity.confidence, DetectionConfidence::Unknown);
        assert_eq!(support_level(&identity), SupportLevel::Unknown);
        assert!(identity.mime.is_none());
        assert_eq!(
            identity.pronom.len(),
            1,
            "keep rejected evidence for diagnostics"
        );
        assert_eq!(
            identity.external_error.as_deref(),
            Some(report.errors.as_str())
        );

        identity.family = FormatKind::Png;
        identity.source = DetectionSource::InternalSignature;
        identity.confidence = DetectionConfidence::Confirmed;
        identity.mime = Some("image/png".to_string());
        merge_pronom_report(&mut identity, &report);
        assert_eq!(identity.source, DetectionSource::InternalSignature);
        assert_eq!(identity.family, FormatKind::Png);
        assert_eq!(identity.confidence, DetectionConfidence::Confirmed);
    }

    #[test]
    fn pronom_namespace_and_internal_signature_bound_promotion() {
        for (namespace, basis, warning, id) in [
            ("custom", "byte match at 0, 8", "", "fmt/11"),
            ("", "byte match at 0, 8", "", "fmt/11"),
            ("pronom", "name match", "", "fmt/11"),
            (
                "pronom",
                "byte match at 0, 8",
                "match on extension only",
                "fmt/11",
            ),
        ] {
            let report = SiegfriedFileReport {
                matches: vec![SiegfriedMatch {
                    ns: namespace.to_string(),
                    id: id.to_string(),
                    mime: "image/png".to_string(),
                    basis: basis.to_string(),
                    warning: warning.to_string(),
                    ..SiegfriedMatch::default()
                }],
                ..SiegfriedFileReport::default()
            };
            let mut identity = unknown_identity();
            merge_pronom_report(&mut identity, &report);
            assert_eq!(identity.family, FormatKind::Unknown);
            assert_eq!(identity.pronom[0].namespace, namespace);
            identity.family = FormatKind::Png;
            identity.source = DetectionSource::InternalSignature;
            identity.confidence = DetectionConfidence::Confirmed;
            identity.mime = Some("image/png".to_string());
            merge_pronom_report(&mut identity, &report);
            assert_eq!(identity.source, DetectionSource::InternalSignature);
            assert_eq!(identity.confidence, DetectionConfidence::Confirmed);
            assert_eq!(identity.mime.as_deref(), Some("image/png"));
        }
    }

    #[test]
    fn batch_failures_are_observable_without_changing_internal_fast_path() {
        let paths = [
            "unknown.bin".into(),
            "mismatch.jpg".into(),
            "normal.png".into(),
        ];
        let mut known = unknown_identity();
        known.family = FormatKind::Png;
        known.confidence = DetectionConfidence::Confirmed;
        known.mime = Some("image/png".to_string());
        known.extension_mismatch = true;
        let mut normal = known.clone();
        normal.extension_mismatch = false;
        for probe in [
            SiegfriedProbe::Unavailable {
                reason: "sf timed out".to_string(),
            },
            SiegfriedProbe::Identified {
                meta: SiegfriedMeta::default(),
                files: Vec::new(),
            },
        ] {
            let mut identities = [unknown_identity(), known.clone(), normal.clone()];
            merge_siegfried_probe(&paths, &mut identities, &probe);
            assert_eq!(identities[0].family, FormatKind::Unknown);
            assert_eq!(support_level(&identities[0]), SupportLevel::Unknown);
            assert_eq!(identities[1].family, known.family);
            assert_eq!(identities[1].confidence, known.confidence);
            assert_eq!(identities[1].source, known.source);
            for identity in &identities[..2] {
                let error = identity
                    .external_error
                    .as_deref()
                    .expect("explicit diagnostic");
                match &probe {
                    SiegfriedProbe::Unavailable { reason } => assert_eq!(error, reason),
                    SiegfriedProbe::Identified { .. } => assert!(error.contains("no report entry")),
                }
            }
            assert_eq!(identities[2], normal, "normal fast path stays untouched");
        }
    }

    #[test]
    fn partial_batch_preserves_good_identity_and_failed_file_evidence() {
        let paths = ["good.bin".into(), "bad.bin".into()];
        let good = SiegfriedFileReport {
            filename: "good.bin".to_string(),
            matches: vec![SiegfriedMatch {
                ns: "pronom".to_string(),
                id: "fmt/11".to_string(),
                basis: "byte match at 0, 8".to_string(),
                ..SiegfriedMatch::default()
            }],
            ..SiegfriedFileReport::default()
        };
        let mut bad = good.clone();
        bad.filename = "bad.bin".to_string();
        bad.errors = "truncated read".to_string();
        let probe = SiegfriedProbe::Identified {
            meta: SiegfriedMeta::default(),
            files: vec![good, bad],
        };
        let mut identities = [unknown_identity(), unknown_identity()];
        merge_siegfried_probe(&paths, &mut identities, &probe);
        assert_eq!(identities[0].family, FormatKind::Png);
        assert!(identities[0].external_error.is_none());
        assert_eq!(identities[1].family, FormatKind::Unknown);
        assert_eq!(
            identities[1].external_error.as_deref(),
            Some("truncated read")
        );
        assert_eq!(identities[1].pronom.len(), 1);
        assert_eq!(support_level(&identities[1]), SupportLevel::Unknown);
    }
}
