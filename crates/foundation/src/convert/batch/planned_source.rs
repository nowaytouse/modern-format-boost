//! A launcher observation checked by the worker, never a delivery or deletion proof.

use super::DiscoveredSource;
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::{Deserialize, Serialize};
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
        let proof = Self {
            schema_version: 1,
            source: crate::media_conversion_gate::delivery_join_relative_to_cwd_or_err(
                source.path(),
                "bind planned worker source",
            )
            .map_err(anyhow::Error::msg)?,
            resolved: source.resolved.clone(),
            identity: source.identity.clone(),
            content_blake3: source.content_blake3(),
        };
        let encoded = serde_json::to_string(&proof)?;
        ensure!(
            encoded.len() <= MAX_PROOF_BYTES,
            "planned source exceeds transport bound"
        );
        Ok(encoded)
    }

    fn parse(encoded: &str) -> Result<Self> {
        ensure!(
            encoded.len() <= MAX_PROOF_BYTES,
            "planned source exceeds transport bound"
        );
        let proof: Self = serde_json::from_str(encoded).context("invalid planned source JSON")?;
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
        Ok(proof)
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
        let proof = Self::parse(encoded)?;
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

/// Internal invocation evidence, separate from persistent user settings.
#[derive(Args, Debug, Default)]
pub struct PlannedSourceArgs {
    #[arg(long, global = true, hide = true)]
    planned_source: Option<String>,
}

impl PlannedSourceArgs {
    /// Check an explicit claim before runtime initialization can write output or state.
    ///
    /// # Errors
    /// Rejects claims on unsupported commands or inputs that no longer match.
    pub fn verify_for_input(&self, input: Option<&Path>) -> Result<()> {
        if let Some(encoded) = &self.planned_source {
            PlannedSource::verify(
                encoded,
                input.context("planned source requires a standard single-file run")?,
            )?;
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
        };
        assert!(args.verify_for_input(None).is_err());
        PlannedSourceArgs::default().verify_for_input(None)?;
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
