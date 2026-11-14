//! A launcher observation checked by the worker, never a delivery or deletion proof.

use super::DiscoveredSource;
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_PROOF_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedSource {
    schema_version: u8,
    source: PathBuf,
    resolved: PathBuf,
    identity: super::CachedFileIdentity,
    content_blake3: String,
}

impl PlannedSource {
    /// Serialize the original observation, without silently replacing it with a newer scan.
    ///
    /// # Errors
    /// Returns path resolution, serialization or transport-bound errors.
    pub fn encode(source: &DiscoveredSource) -> Result<String> {
        let encoded = serde_json::to_string(&Self::from_observation(source)?)?;
        ensure!(
            encoded.len() <= MAX_PROOF_BYTES,
            "planned source exceeds transport bound"
        );
        Ok(encoded)
    }

    fn from_observation(source: &DiscoveredSource) -> Result<Self> {
        Ok(Self {
            schema_version: 1,
            source: crate::media_conversion_gate::delivery_join_relative_to_cwd_or_err(
                source.path(),
                "bind planned worker source",
            )
            .map_err(anyhow::Error::msg)?,
            resolved: source.resolved.clone(),
            identity: source.identity.clone(),
            content_blake3: source.content_blake3(),
        })
    }

    fn parse(encoded: &str) -> Result<Self> {
        ensure!(
            encoded.len() <= MAX_PROOF_BYTES,
            "planned source exceeds transport bound"
        );
        let proof: Self = serde_json::from_str(encoded).context("invalid planned source JSON")?;
        proof.check_structure()?;
        Ok(proof)
    }

    fn check_structure(&self) -> Result<()> {
        let proof = self;
        ensure!(
            proof.schema_version == 1,
            "unsupported planned source schema"
        );
        ensure!(
            proof.source.is_absolute() && proof.resolved.is_absolute(),
            "planned source paths must be absolute"
        );
        ensure!(
            proof.content_blake3.len() == 64
                && proof
                    .content_blake3
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "planned source hash is invalid"
        );
        Ok(())
    }

    /// Check a persisted admission's exact path and content claim without reading media.
    /// This is structural evidence inspection, not current custody or file availability.
    ///
    /// # Errors
    /// Rejects malformed, unsupported or differently bound observations.
    pub fn validate_identity(encoded: &str, input: &Path, expected_blake3: &str) -> Result<()> {
        let proof = Self::parse(encoded)?;
        for path in [&proof.source, &proof.resolved, input] {
            ensure!(
                path.is_absolute()
                    && path.file_name().is_some()
                    && !path.as_os_str().as_encoded_bytes().contains(&0)
                    && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
                    && !path.components().any(|part| matches!(
                        part,
                        std::path::Component::CurDir | std::path::Component::ParentDir
                    )),
                "persisted source observation requires normalized absolute file paths"
            );
        }
        ensure!(
            proof.source.as_os_str() == input.as_os_str()
                && proof.content_blake3 == expected_blake3,
            "persisted source observation disagrees with its admission path or content hash"
        );
        Ok(())
    }

    /// Re-read the worker's actual input and match the launcher's bounded claim.
    ///
    /// # Errors
    /// Rejects malformed claims, unavailable files and changed paths or complete bytes.
    pub fn verify(encoded: &str, input: &Path) -> Result<()> {
        Self::parse(encoded)?.verify_observation(input)
    }

    fn verify_observation(&self, input: &Path) -> Result<()> {
        let proof = self;
        let input = crate::media_conversion_gate::delivery_join_relative_to_cwd_or_err(
            input,
            "check planned worker input",
        )
        .map_err(anyhow::Error::msg)?;
        ensure!(
            input == proof.source,
            "worker input does not match the planned source; source retained"
        );
        let metadata = std::fs::metadata(&input).context("planned worker source is unavailable")?;
        ensure!(
            super::CachedFileIdentity::from_metadata(&metadata)? == proof.identity,
            "worker source identity changed after preflight; source retained and processing refused"
        );
        let observed = DiscoveredSource::from_metadata(&input, &metadata)
            .context("planned worker source could not be read consistently")?;
        ensure!(
            observed.resolved == proof.resolved
                && observed.identity == proof.identity
                && observed.content_blake3() == proof.content_blake3,
            "worker source changed after preflight; source retained and processing refused"
        );
        Ok(())
    }
}

/// The XMP lookup precondition for an IMG worker. This records lookup behavior, not ownership.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum PlannedXmp {
    Absent,
    Present(PlannedSource),
    Ambiguous,
}

impl PlannedXmp {
    fn parse(encoded: &str) -> Result<Self> {
        ensure!(
            encoded.len() <= MAX_PROOF_BYTES,
            "planned XMP observation exceeds transport bound"
        );
        let claim: Self =
            serde_json::from_str(encoded).context("invalid planned XMP observation")?;
        if let Self::Present(observed) = &claim {
            observed.check_structure()?;
        }
        Ok(claim)
    }

    fn check_selection(&self, input: &Path) -> Result<Option<PathBuf>> {
        let input = std::path::absolute(input).context("resolve planned XMP source")?;
        let reject = || {
            crate::UnifiedError::analysis_error(format!(
                "XMP selection changed, is ambiguous, or is unsafe for {}; source and sidecar retained",
                input.display()
            ))
        };
        if matches!(self, Self::Ambiguous) {
            return Err(reject().into());
        }
        let actual = crate::metadata::find_xmp_sidecar(&input).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData
            ) {
                anyhow::Error::from(reject()).context(error.to_string())
            } else {
                anyhow::Error::from(error)
            }
        })?;
        match (self, actual) {
            (Self::Absent, None) => Ok(None),
            (Self::Present(expected), Some(path))
                if path.as_os_str() == expected.source.as_os_str() =>
            {
                Ok(Some(path))
            }
            _ => Err(reject().into()),
        }
    }
}

/// Serialize the XMP observation already chosen by the launcher. A directory scan
/// supplies its original observation, and a single-file scan observes that file's
/// adjacent dependency separately.
///
/// # Errors
/// Refuses transport or serialization failure.
fn encode_xmp_claim(observed: Option<&DiscoveredSource>, ambiguous: bool) -> Result<String> {
    let claim = if ambiguous {
        PlannedXmp::Ambiguous
    } else if let Some(observed) = observed {
        let mut proof = PlannedSource::from_observation(observed)?;
        proof.source = std::path::absolute(observed.path()).context("resolve observed XMP path")?;
        PlannedXmp::Present(proof)
    } else {
        PlannedXmp::Absent
    };
    let encoded = serde_json::to_string(&claim)?;
    ensure!(
        encoded.len() <= MAX_PROOF_BYTES,
        "planned XMP observation exceeds transport bound"
    );
    Ok(encoded)
}

/// Lookup of previously observed XMP files using the same keys as metadata delivery.
#[derive(Default)]
pub struct PlannedXmpIndex<'a> {
    by_directory: HashMap<&'a Path, HashMap<String, Vec<&'a DiscoveredSource>>>,
}

impl<'a> PlannedXmpIndex<'a> {
    pub fn insert(&mut self, source: &'a DiscoveredSource) {
        if let Some(key) = crate::metadata::xmp_candidate_key(source.path()) {
            self.by_directory
                .entry(crate::media_conversion_gate::path_parent_or_dot(
                    source.path(),
                ))
                .or_default()
                .entry(key)
                .or_default()
                .push(source);
        }
    }

    /// Encode the XMP lookup using the original directory inventory.
    ///
    /// # Errors
    /// Rejects unsupported source names or failed encoding.
    pub fn encode_for(&self, input: &Path) -> Result<String> {
        let (stem, filename) = crate::metadata::xmp_source_names(input)?;
        let Some(directory) = self
            .by_directory
            .get(crate::media_conversion_gate::path_parent_or_dot(input))
        else {
            return encode_xmp_claim(None, false);
        };
        let mut candidates = directory.get(&stem).into_iter().flatten();
        let first = candidates.next().copied();
        let second = candidates.next().is_some();
        let exact = if filename != stem {
            directory.get(&filename).map_or(&[][..], Vec::as_slice)
        } else {
            &[]
        };
        let ambiguous = second || exact.len() > 1 || (first.is_some() && !exact.is_empty());
        encode_xmp_claim(first.or_else(|| exact.first().copied()), ambiguous)
    }
}

/// Bind the actual dependency adjacent to a single-file selection.
///
/// # Errors
/// Refuses incomplete or failed sidecar reads without claiming absence.
pub fn encode_single_source_xmp(input: &Path) -> Result<String> {
    let input = std::path::absolute(input).context("resolve single-file XMP source")?;
    match crate::metadata::find_xmp_sidecar(&input) {
        Ok(None) => encode_xmp_claim(None, false),
        Ok(Some(path)) => {
            let sidecar =
                DiscoveredSource::from_metadata(&path, &std::fs::symlink_metadata(&path)?)
                    .with_context(|| format!("observe adjacent XMP {}", path.display()))?;
            encode_xmp_claim(Some(&sidecar), false)
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
            ) =>
        {
            encode_xmp_claim(None, true)
        }
        Err(error) => Err(error).context("read adjacent XMP selection"),
    }
}

/// Check the launcher selection (including absence) and the original XMP
/// filesystem observation before spawning the worker. The worker checks full bytes.
///
/// # Errors
/// Rejects invalid or stale claims or an unreadable namespace.
pub fn verify_xmp_claim_before_launch(encoded: &str, input: &Path) -> Result<()> {
    let claim = PlannedXmp::parse(encoded)?;
    if let (Some(path), PlannedXmp::Present(expected)) = (claim.check_selection(input)?, claim) {
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("inspect selected XMP {}", path.display()))?;
        let identity = super::CachedFileIdentity::from_metadata(&metadata);
        if !identity.is_ok_and(|identity| identity == expected.identity)
            || std::fs::canonicalize(&path)? != expected.resolved
        {
            return Err(crate::UnifiedError::analysis_error(format!(
                "XMP identity changed before launch: {}; source and sidecar retained",
                path.display()
            ))
            .into());
        }
    }
    Ok(())
}

/// Internal invocation evidence, separate from persistent user settings.
#[derive(Args, Debug, Default)]
pub struct PlannedSourceArgs {
    #[arg(long, global = true, hide = true)]
    planned_source: Option<String>,
    #[arg(long, global = true, hide = true, requires = "planned_source")]
    planned_xmp: Option<String>,
}

impl PlannedSourceArgs {
    /// Check an explicit claim before runtime initialization can write output or state.
    ///
    /// # Errors
    /// Rejects claims on unsupported commands or inputs that no longer match.
    pub fn verify_for_input(&self, input: Option<&Path>) -> Result<()> {
        if let Some(encoded) = &self.planned_source {
            let input = input.context("planned source requires a standard single-file run")?;
            PlannedSource::verify(encoded, input)?;
            if let Some(encoded) = &self.planned_xmp {
                let claim = PlannedXmp::parse(encoded)?;
                if let (Some(path), PlannedXmp::Present(expected)) =
                    (claim.check_selection(input)?, claim)
                {
                    expected.verify_observation(&path).with_context(|| {
                        format!(
                            "XMP changed after preflight; source retained: {}",
                            path.display()
                        )
                    })?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_source_binds_complete_bytes_path_and_resolved_target() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("Unicode 图\nsource.jpg");
        std::fs::write(&input, b"first bytes")?;
        let observed = DiscoveredSource::from_metadata(&input, &std::fs::metadata(&input)?)?;
        let encoded = PlannedSource::encode(&observed)?;
        PlannedSource::verify(&encoded, &input)?;
        let other = directory.path().join("other.jpg");
        std::fs::write(&other, b"first bytes")?;
        assert!(PlannedSource::verify(&encoded, &other).is_err());
        std::fs::write(&input, b"other bytes")?;
        assert!(PlannedSource::verify(&encoded, &input).is_err());
        std::fs::remove_file(&input)?;
        assert!(PlannedSource::verify(&encoded, &input).is_err());
        assert!(!directory.path().join("output").exists());
        Ok(())
    }

    #[test]
    fn planned_source_refuses_unknown_incomplete_and_directory_claims() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("source");
        std::fs::write(&input, b"bytes")?;
        let observed = DiscoveredSource::from_metadata(&input, &std::fs::metadata(&input)?)?;
        let encoded = PlannedSource::encode(&observed)?;
        let original: serde_json::Value = serde_json::from_str(&encoded)?;
        for (key, value) in [
            ("schema_version", serde_json::json!(2)),
            ("content_blake3", serde_json::json!("")),
            ("unexpected", serde_json::json!(true)),
            ("source", serde_json::json!("relative")),
        ] {
            let mut invalid = original.clone();
            invalid[key] = value;
            assert!(PlannedSource::verify(&serde_json::to_string(&invalid)?, &input).is_err());
        }
        assert!(PlannedSource::verify("{}", &input).is_err());
        assert!(PlannedSource::verify(&"x".repeat(MAX_PROOF_BYTES + 1), &input).is_err());
        let mut invalid = original;
        invalid["source"] = serde_json::to_value(directory.path())?;
        assert!(
            PlannedSource::verify(&serde_json::to_string(&invalid)?, directory.path()).is_err()
        );
        let args = PlannedSourceArgs {
            planned_source: Some(encoded),
            ..PlannedSourceArgs::default()
        };
        assert!(args.verify_for_input(None).is_err());
        PlannedSourceArgs::default().verify_for_input(None)?;
        Ok(())
    }

    #[test]
    fn planned_xmp_worker_guard_handles_absent_changed_and_ambiguous_companions() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("Photo.jpg");
        let sidecar = directory.path().join("pHOTo.JPG.XMP");
        let ambiguous = directory.path().join("Photo.xmp");
        std::fs::write(&input, b"synthetic JPEG bytes")?;
        let observed = DiscoveredSource::from_metadata(&input, &std::fs::metadata(&input)?)?;
        let mut args = PlannedSourceArgs {
            planned_source: Some(PlannedSource::encode(&observed)?),
            planned_xmp: Some(encode_single_source_xmp(&input)?),
        };
        args.verify_for_input(Some(&input))?;
        std::fs::write(&sidecar, b"<xmp>first</xmp>")?;
        assert!(args.verify_for_input(Some(&input)).is_err());

        let observed = DiscoveredSource::from_metadata(&sidecar, &std::fs::metadata(&sidecar)?)?;
        let mut index = PlannedXmpIndex::default();
        index.insert(&observed);
        args.planned_xmp = Some(index.encode_for(&input)?);
        let claim = args.planned_xmp.as_deref().expect("XMP claim");
        verify_xmp_claim_before_launch(claim, &input)?;
        args.verify_for_input(Some(&input))?;
        std::fs::write(&ambiguous, b"<xmp>extra</xmp>")?;
        assert!(verify_xmp_claim_before_launch(claim, &input).is_err());
        assert!(args.verify_for_input(Some(&input)).is_err());
        std::fs::remove_file(&ambiguous)?;

        std::fs::write(&sidecar, b"<xmp>other</xmp>")?;
        assert!(args.verify_for_input(Some(&input)).is_err());
        std::fs::remove_file(&sidecar)?;
        assert!(verify_xmp_claim_before_launch(claim, &input).is_err());
        assert!(args.verify_for_input(Some(&input)).is_err());
        assert_eq!(std::fs::read(&input)?, b"synthetic JPEG bytes");
        Ok(())
    }

    #[test]
    fn persisted_source_inspection_is_cold_and_binds_the_admitted_identity() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("observed.jpg");
        std::fs::write(&input, b"complete observed source")?;
        let observed = DiscoveredSource::from_metadata(&input, &std::fs::metadata(&input)?)?;
        let encoded = PlannedSource::encode(&observed)?;
        let hash = observed.content_blake3();
        PlannedSource::validate_identity(&encoded, &input, &hash)?;
        std::fs::remove_file(&input)?;
        PlannedSource::validate_identity(&encoded, &input, &hash)?;
        assert!(PlannedSource::verify(&encoded, &input).is_err());
        assert!(PlannedSource::validate_identity(&encoded, &input, &"0".repeat(64)).is_err());
        assert!(
            PlannedSource::validate_identity(&encoded, &directory.path().join("other.jpg"), &hash)
                .is_err()
        );
        let mut unknown: serde_json::Value = serde_json::from_str(&encoded)?;
        unknown["identity"]["unknown_native_field"] = serde_json::json!(true);
        assert!(
            PlannedSource::validate_identity(&serde_json::to_string(&unknown)?, &input, &hash)
                .is_err()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn planned_source_refuses_retargeted_link_even_with_equal_bytes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let input = directory.path().join("selected");
        std::fs::write(&first, b"same")?;
        std::fs::write(&second, b"same")?;
        std::os::unix::fs::symlink(&first, &input)?;
        let observed = DiscoveredSource::from_metadata(&input, &std::fs::metadata(&input)?)?;
        let encoded = PlannedSource::encode(&observed)?;
        PlannedSource::verify(&encoded, &input)?;
        std::fs::remove_file(&input)?;
        std::os::unix::fs::symlink(&second, &input)?;
        assert!(PlannedSource::verify(&encoded, &input).is_err());
        assert_eq!(std::fs::read(&first)?, b"same");
        assert_eq!(std::fs::read(&second)?, b"same");
        Ok(())
    }
}
