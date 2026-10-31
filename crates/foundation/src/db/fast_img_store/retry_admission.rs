//! Current failed-source observations, separate from historical custody evidence.

use super::{FailedRetryAdmission, FailedRetryParent, FailedRetrySidecar};
use crate::batch::{DiscoveredSource, planned_source::PlannedSource};
use crate::pipeline::verification::{FastImgEncodingParameters, WorkingCopyMarker};
use anyhow::{Context, Result, ensure};
use std::path::{Component, Path, PathBuf};

fn relative(value: &str) -> Result<&Path> {
    let path = Path::new(value);
    ensure!(
        !value.is_empty()
            && !value.contains('\0')
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str(),
        "failed-only admission requires a normalized relative path"
    );
    Ok(path)
}

fn require_regular_path(root: &Path, rel: &Path) -> Result<PathBuf> {
    let root_metadata = std::fs::symlink_metadata(root)?;
    ensure!(
        root_metadata.is_dir() && !root_metadata.file_type().is_symlink(),
        "failed-only source root is not a real directory"
    );
    let mut path = root.to_path_buf();
    let components: Vec<_> = rel.components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            anyhow::bail!("invalid failed-only path");
        };
        path.push(name);
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("inspect failed-only source {}", path.display()))?;
        ensure!(
            !metadata.file_type().is_symlink()
                && if index + 1 == components.len() {
                    metadata.is_file()
                } else {
                    metadata.is_dir()
                },
            "failed-only admission refuses non-regular or linked path {}",
            path.display()
        );
    }
    Ok(path)
}

fn observe(root: &Path, rel: &str) -> Result<(String, String)> {
    let path = require_regular_path(root, relative(rel)?)?;
    let observed = DiscoveredSource::from_metadata(&path, &std::fs::symlink_metadata(&path)?)?;
    let hash = observed.content_blake3();
    let claim = PlannedSource::encode(&observed)?;
    PlannedSource::validate_identity(&claim, &path, &hash)?;
    Ok((hash, claim))
}

fn sidecar_observation(root: &Path, path: Option<PathBuf>) -> Result<FailedRetrySidecar> {
    let Some(path) = path else {
        return Ok(FailedRetrySidecar::Absent);
    };
    let rel = path
        .strip_prefix(root)?
        .to_str()
        .context("failed-only sidecar path cannot be represented without loss")?
        .to_owned();
    let (blake3, claim) = observe(root, &rel)?;
    Ok(FailedRetrySidecar::Present {
        relative_path: rel,
        blake3,
        claim,
    })
}

fn capture(root: &Path, rel: &str, parent_hash: &str) -> Result<FailedRetryAdmission> {
    let (admitted_source_blake3, claim) = observe(root, rel)?;
    let source = root.join(relative(rel)?);
    let admission = FailedRetryAdmission {
        source_rel: rel.to_owned(),
        parent_source_blake3: parent_hash.to_owned(),
        admitted_source_blake3,
        claim,
        xmp: sidecar_observation(root, crate::metadata::find_xmp_sidecar(&source)?)?,
        aae: sidecar_observation(root, crate::metadata::find_aae_sidecar(&source)?)?,
    };
    // Bind all observations to the same admission, rather than letting sidecar
    // discovery silently replace the earlier source observation.
    verify_failed_retry_admission(root, &admission)?;
    Ok(admission)
}

/// Observe the current failed set. Changed bytes are new admission facts, never
/// an assertion that an earlier sidecar or source remained unchanged.
///
/// # Errors
/// Rejects unavailable, ambiguous, linked, out-of-scope or changing inputs.
pub fn capture_failed_retry_admissions(
    parent: &FailedRetryParent,
) -> Result<Vec<FailedRetryAdmission>> {
    parent
        .marker()
        .failed_sources
        .iter()
        .map(|(rel, failure)| capture(&parent.marker().src_dir, rel, &failure.src))
        .collect()
}

fn verify_companion(
    root: &Path,
    expected: &FailedRetrySidecar,
    current: Option<PathBuf>,
) -> Result<()> {
    match (expected, current) {
        (FailedRetrySidecar::Absent, None) => Ok(()),
        (
            FailedRetrySidecar::Present {
                relative_path,
                blake3,
                claim,
            },
            Some(current),
        ) => {
            let path = require_regular_path(root, relative(relative_path)?)?;
            ensure!(
                path.as_os_str() == current.as_os_str(),
                "failed-only sidecar selection changed"
            );
            PlannedSource::validate_identity(claim, &path, blake3)?;
            PlannedSource::verify(claim, &path)
        }
        _ => anyhow::bail!(
            "failed-only companion presence changed after admission; sources retained"
        ),
    }
}

/// Re-read the complete source and every supported companion before reuse or execution.
///
/// # Errors
/// Rejects stale identity/content, changed companion selection and I/O failures.
pub fn verify_failed_retry_admission(root: &Path, admission: &FailedRetryAdmission) -> Result<()> {
    let source = require_regular_path(root, relative(&admission.source_rel)?)?;
    PlannedSource::validate_identity(&admission.claim, &source, &admission.admitted_source_blake3)?;
    PlannedSource::verify(&admission.claim, &source)?;
    verify_companion(
        root,
        &admission.xmp,
        crate::metadata::find_xmp_sidecar(&source)?,
    )?;
    verify_companion(
        root,
        &admission.aae,
        crate::metadata::find_aae_sidecar(&source)?,
    )?;
    PlannedSource::verify(&admission.claim, &source)
}

/// For a source whose recorded cleanup is separately checked by the caller,
/// verify retained companions without inventing a new live source observation.
/// Missing XMP needs exact final-output custody; AAE has no implicit deletion permission.
///
/// # Errors
/// Rejects changed retained companions or a missing companion without bound custody evidence.
pub fn verify_failed_retry_retained_companions(
    marker: &WorkingCopyMarker,
    admission: &FailedRetryAdmission,
) -> Result<()> {
    let root = &marker.src_dir;
    let source = root.join(relative(&admission.source_rel)?);
    verify_companion(
        root,
        &admission.aae,
        crate::metadata::find_aae_sidecar(&source)?,
    )?;
    let current = crate::metadata::find_xmp_sidecar(&source)?;
    if let (FailedRetrySidecar::Present { blake3, .. }, None) = (&admission.xmp, &current) {
        let entry = marker
            .blake3_log
            .get(&admission.source_rel)
            .context("missing source has no recorded converted output")?;
        let checks = &marker.gate1_checks;
        ensure!(
            entry.src == admission.admitted_source_blake3
                && checks.count.0
                && checks.blake3.0
                && checks.metadata.0
                && checks.size.0
                && checks.orient.0
                && checks.decode.0,
            "missing source/XMP has no complete recorded output verification"
        );
        let out_rel = entry
            .out_rel
            .as_deref()
            .context("missing source output has no explicit relative identity")?;
        let output = marker.working_copy.join(relative(out_rel)?);
        let clear_avif = marker.processing_contract.as_ref().is_some_and(|contract| {
            matches!(
                contract.encoding,
                FastImgEncodingParameters::Avif {
                    avif_metadata_policy: crate::runtime_config::AvifMetadataPolicy::Clear,
                    ..
                }
            )
        });
        if clear_avif {
            ensure!(
                crate::common_utils::calculate_blake3_hash(&output)? == entry.out,
                "recorded clear-policy output changed; sidecar cleanup is unconfirmed"
            );
        } else {
            crate::metadata::verify_jxl_xmp_digest_custody(&output, &entry.out, blake3)?;
        }
        return Ok(());
    }
    verify_companion(root, &admission.xmp, current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_admission_records_changed_bytes_but_resume_never_recaptures_them() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let source = root.join("changed.jpg");
        std::fs::write(&source, b"newly repaired source")?;
        let old_hash = blake3::hash(b"old failed source").to_hex().to_string();
        let admission = capture(&root, "changed.jpg", &old_hash)?;
        assert_ne!(
            admission.parent_source_blake3,
            admission.admitted_source_blake3
        );
        verify_failed_retry_admission(&root, &admission)?;
        std::fs::write(&source, b"later change")?;
        assert!(verify_failed_retry_admission(&root, &admission).is_err());
        Ok(())
    }

    #[test]
    fn companion_absence_content_and_ambiguity_remain_distinct() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        std::fs::write(root.join("source.jpg"), b"synthetic source")?;
        let hash = blake3::hash(b"synthetic source").to_hex().to_string();
        let absent = capture(&root, "source.jpg", &hash)?;
        std::fs::write(root.join("source.xmp"), b"<xmp/>")?;
        assert!(verify_failed_retry_admission(&root, &absent).is_err());
        let with_xmp = capture(&root, "source.jpg", &hash)?;
        std::fs::write(root.join("source.xmp"), b"<changed/>")?;
        assert!(verify_failed_retry_admission(&root, &with_xmp).is_err());
        std::fs::write(root.join("source.jpg.xmp"), b"<other/>")?;
        assert!(capture(&root, "source.jpg", &hash).is_err());
        Ok(())
    }

    #[test]
    fn aae_change_or_disappearance_is_not_implicit_cleanup_permission() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        std::fs::write(root.join("source.jpg"), b"synthetic source")?;
        let hash = blake3::hash(b"synthetic source").to_hex().to_string();
        std::fs::write(root.join("source.AAE"), b"original adjustment bytes")?;
        let admission = capture(&root, "source.jpg", &hash)?;
        verify_failed_retry_admission(&root, &admission)?;
        std::fs::write(root.join("source.AAE"), b"changed adjustment bytes")?;
        assert!(verify_failed_retry_admission(&root, &admission).is_err());
        std::fs::remove_file(root.join("source.AAE"))?;
        assert!(verify_failed_retry_admission(&root, &admission).is_err());
        Ok(())
    }

    #[test]
    fn missing_xmp_digest_custody_binds_whole_container_and_complete_final_packet() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let output = temp.path().join("packet.JXL");
        let packet = b"<xmp>opaque retained bytes</xmp>";
        // This fixture proves container packet/hash binding only, not image decoding.
        let mut bytes = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
        bytes.extend_from_slice(&u32::try_from(packet.len() + 8)?.to_be_bytes());
        bytes.extend_from_slice(b"xml ");
        bytes.extend_from_slice(packet);
        std::fs::write(&output, &bytes)?;
        let output_hash = blake3::hash(&bytes).to_hex().to_string();
        let packet_hash = blake3::hash(packet).to_hex().to_string();
        crate::metadata::verify_jxl_xmp_digest_custody(&output, &output_hash, &packet_hash)?;
        assert!(
            crate::metadata::verify_jxl_xmp_digest_custody(&output, &"0".repeat(64), &packet_hash)
                .is_err()
        );
        assert!(
            crate::metadata::verify_jxl_xmp_digest_custody(&output, &output_hash, &"0".repeat(64))
                .is_err()
        );
        bytes.push(0);
        std::fs::write(&output, bytes)?;
        assert!(
            crate::metadata::verify_jxl_xmp_digest_custody(&output, &output_hash, &packet_hash)
                .is_err()
        );
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn linked_and_out_of_scope_inputs_are_not_admitted() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        std::fs::write(root.join("real.jpg"), b"source")?;
        std::os::unix::fs::symlink(root.join("real.jpg"), root.join("link.jpg"))?;
        let hash = blake3::hash(b"source").to_hex().to_string();
        assert!(capture(&root, "link.jpg", &hash).is_err());
        assert!(capture(&root, "../real.jpg", &hash).is_err());
        Ok(())
    }
}
