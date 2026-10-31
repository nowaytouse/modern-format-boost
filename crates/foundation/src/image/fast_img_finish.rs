//! Read-only custody verification for an interrupted failed-only attempt.
//! A missing source is an observation, never evidence that MFB deleted it.

use super::{FastImgLibraryAssetProbe, LibraryAssetRecord, WorkingCopyMarker};
use crate::batch::{DiscoveredSource, planned_source::PlannedSource};
use crate::fast_img_store::{
    FailedRetryAdmission, FailedRetryDeliveryIntent, FailedRetryFinishSnapshot, FailedRetrySidecar,
};
use crate::pipeline::verification::{FailedRetryFinishEvidence, FailedRetrySourceRootObservation};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// A capability produced only after the exact snapshot's custody was checked.
/// Callers cannot turn counts, old logs, or an absent directory into this proof.
#[derive(Debug)]
pub struct VerifiedFailedRetryFinish {
    evidence: FailedRetryFinishEvidence,
}

impl VerifiedFailedRetryFinish {
    #[cfg(test)]
    pub(crate) const fn evidence(&self) -> &FailedRetryFinishEvidence {
        &self.evidence
    }

    pub(crate) fn into_evidence(self) -> FailedRetryFinishEvidence {
        self.evidence
    }
}

/// Reverify a failed-only attempt without encoding, importing, or creating sources.
///
/// Existing Gate evidence remains historical. Current complete output bytes and,
/// for Photos, original resources are checked again before Core may finalize.
///
/// # Errors
/// Rejects incomplete admission coverage, changed files, unknown output contents,
/// unresolved companion custody, missing UUIDs, and failed Photos queries.
pub fn verify_failed_retry_finish(
    snapshot: &FailedRetryFinishSnapshot,
) -> Result<VerifiedFailedRetryFinish> {
    let _photos_lock = if matches!(
        snapshot.context().requested_delivery(),
        FailedRetryDeliveryIntent::Photos { .. }
    ) {
        Some(super::acquire_photos_import_lock()?)
    } else {
        None
    };
    verify_with(
        snapshot,
        &mut |uuids, library| super::query_selected_photos_asset_probes(uuids, Some(library)),
        &mut |library| super::require_active_photos_library(Some(library)),
        &mut |path| {
            #[cfg(target_os = "macos")]
            {
                super::path_has_quarantine_xattr(path)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Ok(super::path_has_quarantine_xattr(path))
            }
        },
    )
}

fn absent_source_root(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect finish-only source root"),
        Ok(_) => anyhow::bail!("finish-only requires the recorded source root to be absent"),
    }
    // Reject a replaced ancestor redirecting the saved absolute locator into a
    // different, missing tree. This creates neither the source nor its parents.
    for ancestor in root.ancestors().skip(1) {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "finish-only source ancestor is linked or is not a directory"
                );
                ensure!(
                    ancestor.canonicalize()?.as_os_str() == ancestor.as_os_str(),
                    "finish-only source ancestor no longer has its recorded locator"
                );
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect finish-only source ancestor"),
        }
    }
    anyhow::bail!("finish-only source has no verifiable existing ancestor")
}

fn relative(value: &str) -> Result<&Path> {
    let path = Path::new(value);
    ensure!(
        !value.is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str(),
        "finish-only requires an explicit normalized relative identity"
    );
    Ok(path)
}

#[derive(PartialEq, Eq)]
struct OutputInventory {
    present: bool,
    files: BTreeSet<PathBuf>,
}

fn output_inventory(root: &Path, expected: &BTreeMap<PathBuf, String>) -> Result<OutputInventory> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OutputInventory {
                present: false,
                files: BTreeSet::new(),
            });
        }
        Err(error) => return Err(error).context("inspect finish-only output root"),
        Ok(metadata) => ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "finish-only output is not a real directory"
        ),
    }
    ensure!(
        root.canonicalize()?.as_os_str() == root.as_os_str(),
        "finish-only output locator changed"
    );
    let mut allowed_directories = BTreeSet::from([PathBuf::new()]);
    for path in expected.keys() {
        for parent in path.ancestors().skip(1) {
            allowed_directories.insert(parent.to_path_buf());
        }
    }
    let mut files = BTreeSet::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.context("enumerate finish-only outputs")?;
        let rel = entry.path().strip_prefix(root)?;
        if entry.file_type().is_dir() {
            ensure!(
                allowed_directories.contains(rel),
                "finish-only output contains an unowned directory"
            );
        } else {
            ensure!(
                entry.file_type().is_file(),
                "finish-only output contains a link or non-regular file"
            );
            ensure!(
                expected.contains_key(rel),
                "finish-only output contains an unknown file"
            );
            ensure!(
                files.insert(rel.to_path_buf()),
                "finish-only output identity occurs twice"
            );
        }
    }
    Ok(OutputInventory {
        present: true,
        files,
    })
}

fn observe_file(
    path: &Path,
    expected: &str,
    observations: &mut Vec<(PathBuf, String)>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect finish-only verified resource")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 0,
        "finish-only resource is not a nonempty regular file"
    );
    let observed = DiscoveredSource::from_metadata(path, &metadata)?;
    ensure!(
        observed.content_blake3() == expected,
        "finish-only complete resource BLAKE3 differs from the recorded output"
    );
    observations.push((path.to_path_buf(), PlannedSource::encode(&observed)?));
    Ok(())
}

fn primary_xmp_custody(
    marker: &WorkingCopyMarker,
    admission: &FailedRetryAdmission,
    artifact: &Path,
    expected: &str,
) -> Result<()> {
    let FailedRetrySidecar::Present { blake3, .. } = &admission.xmp else {
        return Ok(());
    };
    let clears_avif_metadata = marker.processing_contract.as_ref().is_some_and(|contract| {
        matches!(
            contract.encoding,
            crate::pipeline::verification::FastImgEncodingParameters::Avif {
                avif_metadata_policy: crate::runtime_config::AvifMetadataPolicy::Clear,
                ..
            }
        )
    });
    if !clears_avif_metadata {
        crate::metadata::verify_jxl_xmp_digest_custody(artifact, expected, blake3)
            .context("finish-only source/XMP has no complete final-packet custody")?;
    }
    Ok(())
}

fn uuid(asset: &LibraryAssetRecord) -> Result<&str> {
    let uuid = asset
        .photos_uuid
        .as_deref()
        .context("finish-only Photos receipt has no UUID")?;
    ensure!(
        !uuid.is_empty() && !asset.quarantined,
        "finish-only Photos receipt is incomplete or quarantined"
    );
    Ok(uuid)
}

fn verify_with(
    snapshot: &FailedRetryFinishSnapshot,
    query: &mut impl FnMut(&[String], &Path) -> super::Result<Vec<FastImgLibraryAssetProbe>>,
    active_library: &mut impl FnMut(&Path) -> super::Result<()>,
    quarantined: &mut impl FnMut(&Path) -> super::Result<bool>,
) -> Result<VerifiedFailedRetryFinish> {
    snapshot.validate_eligibility()?;
    let marker = snapshot.marker();
    let context = snapshot.context();
    absent_source_root(&marker.src_dir)?;
    marker
        .validate_relative_path_contract()
        .map_err(anyhow::Error::msg)?;
    marker
        .validate_library_asset_receipts_unique()
        .map_err(anyhow::Error::msg)?;
    ensure!(
        marker.skipped_sources.is_empty() && marker.failed_sources.is_empty(),
        "finish-only cannot complete failed, skipped, or unknown sources"
    );
    let mut expected_outputs = BTreeMap::new();
    let mut covered = BTreeSet::new();
    for (source, entry) in &marker.blake3_log {
        let admission = context
            .admissions()
            .get(source)
            .context("finish-only output has no admitted source")?;
        ensure!(
            entry.src == admission.admitted_source_blake3,
            "finish-only output belongs to another source identity"
        );
        let out = relative(
            entry
                .out_rel
                .as_deref()
                .context("finish-only output has no explicit relative identity")?,
        )?;
        ensure!(
            expected_outputs
                .insert(out.to_path_buf(), entry.out.clone())
                .is_none(),
            "finish-only sources share one output"
        );
        ensure!(
            covered.insert(source.as_str()),
            "finish-only source is covered more than once"
        );
    }
    for asset in &marker.tier2_imported_assets {
        let admission = context
            .admissions()
            .get(&asset.rel_path)
            .context("finish-only original has no admitted source")?;
        ensure!(
            asset.blake3 == admission.admitted_source_blake3,
            "finish-only original belongs to another source identity"
        );
        ensure!(
            covered.insert(asset.rel_path.as_str()),
            "finish-only source has conflicting successful routes"
        );
        match &admission.xmp {
            FailedRetrySidecar::Absent => ensure!(
                asset.xmp_sidecar_blake3.is_none()
                    && asset
                        .library_blake3
                        .as_deref()
                        .is_none_or(|hash| hash == asset.blake3),
                "finish-only original has unexplained metadata or byte changes"
            ),
            FailedRetrySidecar::Present { blake3, .. } => ensure!(
                asset.xmp_sidecar_blake3.as_ref() == Some(blake3) && asset.library_blake3.is_some(),
                "finish-only original lacks its admitted complete XMP delivery proof"
            ),
        }
    }
    ensure!(
        covered == context.admissions().keys().map(String::as_str).collect(),
        "finish-only successful results do not exactly cover the admitted work set"
    );
    for admission in context.admissions().values() {
        ensure!(
            matches!(admission.aae, FailedRetrySidecar::Absent),
            "finish-only cannot explain a missing AAE companion"
        );
    }
    let inventory = output_inventory(&marker.working_copy, &expected_outputs)?;
    let mut observations = Vec::new();
    let mut artifacts = BTreeMap::new();
    for rel in &inventory.files {
        crate::batch_control::checkpoint()?;
        let path = marker.working_copy.join(rel);
        let expected = expected_outputs
            .get(rel)
            .context("finish-only output escaped its inventory")?;
        observe_file(&path, expected, &mut observations)?;
        ensure!(!quarantined(&path)?, "finish-only output is quarantined");
        artifacts.insert(rel.clone(), path);
    }
    let (primary_count, original_count) = match context.requested_delivery() {
        FailedRetryDeliveryIntent::Local => {
            ensure!(
                inventory.present && inventory.files.len() == expected_outputs.len(),
                "finish-only local delivery requires every recorded output to remain present"
            );
            ensure!(
                marker.photos_imported_assets.is_empty() && marker.tier2_imported_assets.is_empty(),
                "finish-only local attempt contains Photos custody"
            );
            (0, 0)
        }
        FailedRetryDeliveryIntent::Photos {
            library_path,
            naming_policy,
        } => {
            ensure!(
                marker.photos_import_naming_policy.as_ref() == Some(naming_policy),
                "finish-only Photos naming intent differs from its immutable request"
            );
            super::validate_photos_import_naming_policy(marker)?;
            let library = marker
                .photos_library_path
                .as_deref()
                .context("finish-only Photos custody has no bound actual library")?;
            ensure!(
                library_path.as_deref().is_none_or(|path| path == library),
                "finish-only Photos target differs from its explicit request"
            );
            ensure!(
                super::selected_photos_library(Some(library))?.as_deref() == Some(library),
                "finish-only Photos selection changed"
            );
            ensure!(
                marker.photos_imported_assets.len() == expected_outputs.len(),
                "finish-only primary Photos receipts do not cover all outputs"
            );
            let mut targets = BTreeMap::new();
            for asset in &marker.photos_imported_assets {
                let rel = relative(&asset.rel_path)?;
                let expected = expected_outputs
                    .get(rel)
                    .context("finish-only Photos receipt has no recorded output")?;
                ensure!(
                    &asset.blake3 == expected
                        && asset
                            .library_blake3
                            .as_deref()
                            .is_none_or(|hash| hash == expected),
                    "finish-only requires complete output bytes, not a previous pixel-equivalence fallback"
                );
                ensure!(
                    targets
                        .insert(
                            uuid(asset)?.to_owned(),
                            (asset, expected.as_str(), Some(rel))
                        )
                        .is_none(),
                    "finish-only Photos UUID is duplicated"
                );
            }
            for asset in &marker.tier2_imported_assets {
                let expected = asset.library_blake3.as_deref().unwrap_or(&asset.blake3);
                ensure!(
                    targets
                        .insert(uuid(asset)?.to_owned(), (asset, expected, None))
                        .is_none(),
                    "finish-only Photos UUID has conflicting source routes"
                );
            }
            let uuids = targets.keys().cloned().collect::<Vec<_>>();
            active_library(library)?;
            let probes = super::index_photos_probes_by_uuid(&uuids, query(&uuids, library)?)?;
            for (uuid, (_, expected, output_rel)) in targets {
                crate::batch_control::checkpoint()?;
                let probe = probes
                    .get(&uuid)
                    .context("finish-only Photos query has no exact UUID mapping")?;
                ensure!(!probe.ismissing, "finish-only Photos original is missing");
                ensure!(
                    !super::fast_img_require_icloud_upload_proof() || probe.incloud == Some(true),
                    "finish-only requested iCloud proof is unavailable"
                );
                observe_file(&probe.path, expected, &mut observations)?;
                ensure!(
                    !quarantined(&probe.path)?,
                    "finish-only Photos original is quarantined"
                );
                if let Some(rel) = output_rel {
                    artifacts
                        .entry(rel.to_path_buf())
                        .or_insert_with(|| probe.path.clone());
                }
            }
            active_library(library)?;
            (
                marker.photos_imported_assets.len(),
                marker.tier2_imported_assets.len(),
            )
        }
    };
    for (source, entry) in &marker.blake3_log {
        let out = relative(
            entry
                .out_rel
                .as_deref()
                .context("finish-only output identity disappeared")?,
        )?;
        let artifact = artifacts
            .get(out)
            .context("finish-only output has no current complete-byte custody")?;
        ensure!(
            crate::image::format_detect::detect_true_format(artifact)?
                == crate::delivery_codec_strategy::strategy_to_format_kind(&marker.strategy)
                    .context("finish-only strategy is unsupported")?,
            "finish-only artifact content does not match its recorded output format"
        );
        let admission = context
            .admissions()
            .get(source)
            .context("finish-only output lost its admitted source")?;
        primary_xmp_custody(marker, admission, artifact, &entry.out)?;
    }
    for (path, claim) in observations {
        PlannedSource::verify(&claim, &path)?;
    }
    ensure!(
        output_inventory(&marker.working_copy, &expected_outputs)? == inventory,
        "finish-only output inventory changed during verification"
    );
    absent_source_root(&marker.src_dir)?;
    Ok(VerifiedFailedRetryFinish {
        evidence: FailedRetryFinishEvidence {
            schema: 1,
            task_key: snapshot.task_key().to_owned(),
            request_key: snapshot.request_key().to_owned(),
            manifest_blake3: snapshot.manifest_blake3().to_owned(),
            verified_snapshot_blake3: snapshot.snapshot_blake3().to_owned(),
            verified_revision: snapshot.revision(),
            verified_stage: marker.stage.clone(),
            verified_tier2_in_progress: marker.tier2_in_progress,
            verified_error: marker.error.clone(),
            source_root_observation: FailedRetrySourceRootObservation::NotFound,
            verified_output_count: u64::try_from(expected_outputs.len())?,
            verified_primary_photos_count: u64::try_from(primary_count)?,
            verified_original_photos_count: u64::try_from(original_count)?,
            observed_at: chrono::Utc::now().to_rfc3339(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::verification::{
        Blake3Entry, FastImgEncodingParameters, FastImgProcessingContract, FastImgStageName,
        Gate1Checks, Gate2Checks, Gate3Checks, SkippedSourceEntry, write_marker_atomic,
    };
    use std::cell::Cell;

    const UUID: &str = "01234567-89AB-CDEF-0123-456789ABCDEF";
    const XMP: &[u8] = b"<xmp>complete opaque packet</xmp>";

    struct Fixture {
        snapshot: FailedRetryFinishSnapshot,
        marker: WorkingCopyMarker,
        library: PathBuf,
        original: PathBuf,
        bytes: Vec<u8>,
    }

    fn fixture(root: &Path, name: &str, photos: bool, companion: Option<&str>) -> Result<Fixture> {
        let source = root.join(format!("source-{name}"));
        fs::create_dir(&source)?;
        let source = source.canonicalize()?;
        fs::write(source.join("a.jpg"), b"synthetic admitted JPEG source")?;
        if companion == Some("xmp") {
            fs::write(source.join("a.xmp"), XMP)?;
        }
        if companion == Some("aae") {
            fs::write(source.join("a.aae"), b"retained opaque AAE")?;
        }
        let source_hash = crate::common_utils::calculate_blake3_hash(&source.join("a.jpg"))?;
        let mut parent =
            WorkingCopyMarker::new(source.clone(), root.join(format!("parent-{name}")), 1);
        parent.stage = FastImgStageName::CleanupComplete;
        parent.processing_contract = Some(FastImgProcessingContract {
            schema: 1,
            tool_policy: crate::runtime_config::ToolPolicy::Single,
            fallback_policy: crate::runtime_config::FallbackPolicy::Strict,
            encoding: FastImgEncodingParameters::Jxl {
                jpeg_effort: 11,
                jpeg_effort_search: crate::runtime_config::JpegEffortSearch::Full,
                archive: false,
                allow_expert_options: false,
            },
        });
        parent.failed_sources.insert(
            "a.jpg".into(),
            SkippedSourceEntry {
                src: source_hash.clone(),
                reason: "synthetic retained failure".into(),
            },
        );
        write_marker_atomic(&parent)?;
        let reference = crate::fast_img_store::inspect_failed_retry_parent(&parent.working_copy)?;
        let admissions = crate::fast_img_store::capture_failed_retry_admissions(&reference)?;
        let library = root.join(format!("{name}.photoslibrary"));
        fs::create_dir_all(library.join("database"))?;
        fs::write(
            library.join("database/Photos.sqlite"),
            b"synthetic database",
        )?;
        let library = library.canonicalize()?;
        let delivery = if photos {
            FailedRetryDeliveryIntent::Photos {
                library_path: Some(library.clone()),
                naming_policy: super::super::current_photos_import_naming_policy(),
            }
        } else {
            FailedRetryDeliveryIntent::Local
        };
        let mut marker = crate::fast_img_store::create_or_resume_failed_retry(
            &reference,
            &admissions,
            &delivery,
            None,
        )?;
        fs::create_dir(&marker.working_copy)?;
        // Only the old Gate is modeled. These bytes test present format/hash
        // and complete XMP packet binding, never a new image decoding proof.
        let mut bytes = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
        if companion == Some("xmp") {
            bytes.extend_from_slice(&u32::try_from(XMP.len() + 8)?.to_be_bytes());
            bytes.extend_from_slice(b"xml ");
            bytes.extend_from_slice(XMP);
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        fs::write(marker.working_copy.join("a.JXL"), &bytes)?;
        marker.stage = if photos {
            FastImgStageName::Gate3Passed
        } else {
            FastImgStageName::Gate1Passed
        };
        marker.encoded_count = 1;
        marker.gate1_checks = Gate1Checks {
            count: true.into(),
            blake3: true.into(),
            metadata: true.into(),
            size: true.into(),
            orient: true.into(),
            decode: true.into(),
        };
        marker.blake3_log.insert(
            "a.jpg".into(),
            Blake3Entry {
                src: source_hash,
                out: hash.clone(),
                out_rel: Some("a.JXL".into()),
                library_asset: photos.then(|| hash.clone()),
            },
        );
        if photos {
            marker.gate2_checks = Gate2Checks {
                count: true.into(),
                blake3_sample: true.into(),
                no_error: true.into(),
            };
            marker.gate3_checks = Gate3Checks {
                count_x3: true.into(),
                sync: true.into(),
                quarantine: true.into(),
                chain: true.into(),
            };
            marker.photos_imported_assets.push(LibraryAssetRecord {
                rel_path: "a.JXL".into(),
                blake3: hash.clone(),
                sync_status: "photos_local".into(),
                photos_uuid: Some(UUID.into()),
                library_blake3: Some(hash),
                ..Default::default()
            });
        }
        write_marker_atomic(&marker)?;
        fs::remove_file(source.join("a.jpg"))?;
        if let Some(extension) = companion {
            fs::remove_file(source.join(format!("a.{extension}")))?;
        }
        fs::remove_dir(&source)?;
        let original = library.join("a.JXL");
        fs::write(&original, &bytes)?;
        let snapshot = crate::fast_img_store::inspect_failed_retry_finish(&marker.working_copy)?
            .context("synthetic child is not a recoverable snapshot")?;
        Ok(Fixture {
            snapshot,
            marker,
            library,
            original,
            bytes,
        })
    }

    fn probe(fixture: &Fixture) -> FastImgLibraryAssetProbe {
        FastImgLibraryAssetProbe {
            uuid: UUID.into(),
            path: fixture.original.clone(),
            iscloudasset: false,
            incloud: Some(true),
            ismissing: false,
        }
    }

    #[test]
    fn finish_only_local_keeps_the_exact_work_set_and_current_output_proof() -> Result<()> {
        if crate::common_utils::isolated_test_process() {
            return Ok(());
        }
        unsafe { std::env::remove_var("MFB_PHOTOS_LIBRARY_PATH") };
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?;
        let _state = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            root.join("state").to_str().unwrap(),
        );
        let fixture = fixture(&root, "local", false, None)?;
        let proof = verify_failed_retry_finish(&fixture.snapshot)?;
        assert_eq!(proof.evidence().verified_output_count, 1);
        assert_eq!(proof.evidence().verified_primary_photos_count, 0);
        assert_eq!(
            crate::pipeline::verification::read_marker(&fixture.marker.working_copy)?,
            fixture.marker
        );
        assert!(!fixture.marker.src_dir.exists());
        let unknown = fixture.marker.working_copy.join("unowned.txt");
        fs::write(&unknown, b"outside the immutable output set")?;
        assert!(verify_failed_retry_finish(&fixture.snapshot).is_err());
        fs::remove_file(unknown)?;
        let output = fixture.marker.working_copy.join("a.JXL");
        fs::write(&output, b"changed artifact")?;
        assert!(verify_failed_retry_finish(&fixture.snapshot).is_err());
        fs::remove_file(&output)?;
        assert!(verify_failed_retry_finish(&fixture.snapshot).is_err());
        fs::write(&output, &fixture.bytes)?;
        fs::create_dir(&fixture.marker.src_dir)?;
        assert!(verify_failed_retry_finish(&fixture.snapshot).is_err());
        Ok(())
    }

    #[test]
    fn finish_only_photos_requeries_full_original_bytes_when_local_output_is_absent() -> Result<()>
    {
        if crate::common_utils::isolated_test_process() {
            return Ok(());
        }
        unsafe { std::env::remove_var("MFB_PHOTOS_LIBRARY_PATH") };
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?;
        let _state = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            root.join("state").to_str().unwrap(),
        );
        let fixture = fixture(&root, "photos", true, Some("xmp"))?;
        fs::remove_file(fixture.marker.working_copy.join("a.JXL"))?;
        fs::remove_dir(&fixture.marker.working_copy)?;
        let queries = Cell::new(0);
        let active_checks = Cell::new(0);
        let proof = verify_with(
            &fixture.snapshot,
            &mut |ids, library| {
                assert_eq!(ids, &[UUID]);
                assert_eq!(library, fixture.library);
                queries.set(queries.get() + 1);
                Ok(vec![probe(&fixture)])
            },
            &mut |library| {
                assert_eq!(library, fixture.library);
                active_checks.set(active_checks.get() + 1);
                Ok(())
            },
            &mut |_| Ok(false),
        )?;
        assert_eq!((queries.get(), active_checks.get()), (1, 2));
        assert_eq!(
            proof.evidence().verified_output_count,
            1,
            "the actual Photos bytes prove one archive artifact, not a local file count"
        );
        assert_eq!(proof.evidence().verified_primary_photos_count, 1);
        for result in [
            Ok(Vec::new()),
            Ok(vec![probe(&fixture), probe(&fixture)]),
            Err(super::super::ImgQualityError::AnalysisError(
                "query denied, with Photos verifier BLAKE3 mismatch in diagnostic text".into(),
            )),
        ] {
            let mut result = Some(result);
            assert!(
                verify_with(
                    &fixture.snapshot,
                    &mut |_, _| result.take().unwrap(),
                    &mut |_| Ok(()),
                    &mut |_| Ok(false)
                )
                .is_err()
            );
        }
        fs::write(&fixture.original, b"different complete bytes")?;
        assert!(
            verify_with(
                &fixture.snapshot,
                &mut |_, _| Ok(vec![probe(&fixture)]),
                &mut |_| Ok(()),
                &mut |_| Ok(false)
            )
            .is_err()
        );
        fs::write(&fixture.original, &fixture.bytes)?;
        let checks = Cell::new(0);
        assert!(
            verify_with(
                &fixture.snapshot,
                &mut |_, _| Ok(vec![probe(&fixture)]),
                &mut |_| {
                    checks.set(checks.get() + 1);
                    if checks.get() == 2 {
                        fs::write(&fixture.original, b"changed after querying")?;
                    }
                    Ok(())
                },
                &mut |_| Ok(false)
            )
            .is_err()
        );
        assert!(!fixture.marker.src_dir.exists() && !fixture.marker.working_copy.exists());
        assert_eq!(
            crate::pipeline::verification::read_marker(&fixture.marker.working_copy)?,
            fixture.marker
        );
        Ok(())
    }

    #[test]
    fn finish_only_retains_companion_and_nonregular_output_boundaries() -> Result<()> {
        if crate::common_utils::isolated_test_process() {
            return Ok(());
        }
        unsafe { std::env::remove_var("MFB_PHOTOS_LIBRARY_PATH") };
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?;
        let _state = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            root.join("state").to_str().unwrap(),
        );
        let aae = fixture(&root, "aae", false, Some("aae"))?;
        assert!(
            verify_failed_retry_finish(&aae.snapshot).is_err(),
            "missing AAE has no deletion permission"
        );
        let xmp = fixture(&root, "xmp", false, Some("xmp"))?;
        assert_eq!(
            verify_failed_retry_finish(&xmp.snapshot)?
                .evidence()
                .verified_output_count,
            1
        );
        let packet = b"<xmp>different complete packet</xmp>";
        let mut changed = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
        changed.extend_from_slice(&u32::try_from(packet.len() + 8)?.to_be_bytes());
        changed.extend_from_slice(b"xml ");
        changed.extend_from_slice(packet);
        fs::write(xmp.marker.working_copy.join("a.JXL"), &changed)?;
        let mut wrong_packet = xmp.marker;
        wrong_packet
            .blake3_log
            .get_mut("a.jpg")
            .context("synthetic output missing")?
            .out = blake3::hash(&changed).to_hex().to_string();
        write_marker_atomic(&wrong_packet)?;
        let changed_snapshot =
            crate::fast_img_store::inspect_failed_retry_finish(&wrong_packet.working_copy)?
                .context("synthetic snapshot missing")?;
        assert!(
            verify_failed_retry_finish(&changed_snapshot).is_err(),
            "matching the current complete file hash cannot replace admitted XMP packet custody"
        );
        let linked = fixture(&root, "linked", false, None)?;
        let output = linked.marker.working_copy.join("a.JXL");
        fs::remove_file(&output)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&linked.original, &output)?;
            assert!(verify_failed_retry_finish(&linked.snapshot).is_err());
        }
        Ok(())
    }
}
