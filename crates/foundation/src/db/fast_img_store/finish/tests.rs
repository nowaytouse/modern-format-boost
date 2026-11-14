use super::*;
use crate::batch::DiscoveredSource;
use crate::batch::planned_source::PlannedSource;
use crate::fast_img_store::{
    FailedRetryAdmission, FailedRetrySidecar, create_or_resume_failed_retry, failed_retry_context,
    inspect_failed_retry_parent, read_marker, write_marker,
};
use crate::image::fast_img::verify_failed_retry_finish;
use crate::pipeline::verification::{
    Blake3Entry, FastImgEncodingParameters, FastImgProcessingContract, Gate1Checks,
    SkippedSourceEntry,
};

const XMP: &[u8] = b"<xmp>synthetic failed-only companion</xmp>";

fn fixture(root: &Path, journaled: bool) -> Result<(WorkingCopyMarker, WorkingCopyMarker)> {
    let root = root.canonicalize()?;
    let source = root.join("source");
    std::fs::create_dir(&source)?;
    let input = source.join("retained.jpg");
    std::fs::write(&input, b"synthetic retained source")?;
    let observed = DiscoveredSource::from_metadata(&input, &std::fs::metadata(&input)?)?;
    let source_hash = observed.content_blake3();
    let xmp = if journaled {
        let path = source.join("retained.xmp");
        std::fs::write(&path, XMP)?;
        let observed = DiscoveredSource::from_metadata(&path, &std::fs::symlink_metadata(&path)?)?;
        FailedRetrySidecar::Present {
            relative_path: "retained.xmp".into(),
            blake3: observed.content_blake3(),
            claim: PlannedSource::encode(&observed)?,
        }
    } else {
        FailedRetrySidecar::Absent
    };
    let mut parent = WorkingCopyMarker::new(source, root.join("source_optimized"), 1);
    parent.stage = FastImgStageName::CleanupComplete;
    parent.primary_cleanup_journal = journaled;
    parent.processing_contract = Some(FastImgProcessingContract {
        schema: 1,
        tool_policy: crate::runtime_config::ToolPolicy::Single,
        fallback_policy: crate::runtime_config::FallbackPolicy::Strict,
        encoding: FastImgEncodingParameters::Jxl {
            jpeg_effort: 11,
            jpeg_effort_search: crate::runtime_config::JpegEffortSearch::Full,
            archive: true,
            allow_expert_options: false,
        },
    });
    parent.failed_sources.insert(
        "retained.jpg".into(),
        SkippedSourceEntry {
            src: source_hash.clone(),
            reason: "synthetic retained failure".into(),
        },
    );
    write_marker(&parent)?;
    let reference = inspect_failed_retry_parent(&parent.working_copy)?;
    let mut child = create_or_resume_failed_retry(
        &reference,
        &[FailedRetryAdmission {
            source_rel: "retained.jpg".into(),
            parent_source_blake3: source_hash.clone(),
            admitted_source_blake3: source_hash.clone(),
            claim: PlannedSource::encode(&observed)?,
            xmp,
            aae: FailedRetrySidecar::Absent,
            route: None,
        }],
        &FailedRetryDeliveryIntent::Local,
        None,
    )?;
    std::fs::create_dir(&child.working_copy)?;
    // This models an already Gate-verified artifact. Finish recovery observes
    // its current complete bytes/type; this fixture does not claim a new decode.
    let output = if journaled {
        let mut bytes = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
        bytes.extend_from_slice(&u32::try_from(XMP.len() + 8)?.to_be_bytes());
        bytes.extend_from_slice(b"xml ");
        bytes.extend_from_slice(XMP);
        bytes
    } else {
        vec![0xff, 0x0a, 0x31, 0x45, 0x59, 0x26]
    };
    std::fs::write(child.working_copy.join("retained.JXL"), &output)?;
    child.blake3_log.insert(
        "retained.jpg".into(),
        Blake3Entry {
            src: source_hash,
            out_rel: Some("retained.JXL".into()),
            out: blake3::hash(&output).to_hex().to_string(),
            library_asset: None,
        },
    );
    child.encoded_count = 1;
    child.stage = FastImgStageName::Gate1Passed;
    child.gate1_checks = Gate1Checks {
        count: true.into(),
        blake3: true.into(),
        metadata: true.into(),
        size: true.into(),
        orient: true.into(),
        decode: true.into(),
    };
    child.error = Some("interrupted after verified local output".into());
    write_marker(&child)?;
    Ok((parent, child))
}

#[test]
fn finish_only_commits_observed_absence_without_changing_parent_or_media() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (parent, child) = fixture(root.path(), false)?;
    let snapshot = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    std::fs::remove_dir_all(&child.src_dir)?;
    let output = std::fs::read(child.working_copy.join("retained.JXL"))?;
    let proof = verify_failed_retry_finish(&snapshot)?;
    let stale = verify_failed_retry_finish(&snapshot)?;
    let mut forged = child.clone();
    forged.stage = FastImgStageName::CleanupComplete;
    forged.error = None;
    forged.failed_retry_finish_evidence = Some(proof.evidence().clone());
    assert!(write_marker(&forged).is_err());
    let finished = finalize_failed_retry_finish(&snapshot, proof)?;
    assert!(finished.is_terminal());
    let receipt = finished.failed_retry_finish_evidence().unwrap();
    assert_eq!(receipt.verified_output_count, 1);
    assert_eq!(receipt.verified_primary_photos_count, 0);
    assert_eq!(receipt.verified_original_photos_count, 0);
    assert_eq!(
        receipt.source_root_observation,
        FailedRetrySourceRootObservation::NotFound
    );
    assert_eq!(receipt.verified_error, child.error);
    assert!(!child.src_dir.exists());
    assert_eq!(
        std::fs::read(child.working_copy.join("retained.JXL"))?,
        output
    );
    assert_eq!(read_marker(&parent.working_copy)?, parent);
    assert_eq!(read_marker(&child.working_copy)?, finished);
    assert!(inspect_failed_retry_finish(&child.working_copy)?.is_none());
    let mut inconsistent = finished.clone();
    inconsistent
        .failed_retry_finish_evidence
        .as_mut()
        .unwrap()
        .verified_error = Some("not the original verified snapshot".into());
    let conn = open_store_connection()?;
    assert!(
        validate_persisted(
            &conn,
            snapshot.task_key(),
            &inconsistent,
            snapshot.revision().checked_add(1).unwrap(),
        )
        .is_err()
    );
    assert!(finalize_failed_retry_finish(&snapshot, stale).is_err());
    assert_eq!(read_marker(&child.working_copy)?, finished);
    write_marker(&finished)?;
    let mut changed = finished.clone();
    changed.error = Some("not the verified final observation".into());
    assert!(write_marker(&changed).is_err());
    Ok(())
}

#[test]
fn finish_only_rejects_concurrent_state_changes_and_rolls_back_write_failure() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (parent, mut child) = fixture(root.path(), false)?;
    let snapshot = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    std::fs::remove_dir_all(&child.src_dir)?;
    let proof = verify_failed_retry_finish(&snapshot)?;
    child.error = Some("another committed observation".into());
    write_marker(&child)?;
    let error = finalize_failed_retry_finish(&snapshot, proof).unwrap_err();
    assert!(error.to_string().contains("MFB_FINISH_STATE_CHANGED"));
    assert_eq!(read_marker(&child.working_copy)?, child);
    let current = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    let proof = verify_failed_retry_finish(&current)?;
    let conn = open_store_connection()?;
    conn.execute_batch(
        "CREATE TRIGGER reject_finish_confirmation BEFORE UPDATE ON fast_img_tasks
         BEGIN SELECT RAISE(ABORT,'synthetic finalization write rejection'); END;",
    )?;
    assert!(finalize_failed_retry_finish(&current, proof).is_err());
    assert_eq!(read_marker(&child.working_copy)?, child);
    assert_eq!(read_marker(&parent.working_copy)?, parent);
    conn.execute_batch(
        "DROP TRIGGER reject_finish_confirmation;
         CREATE TRIGGER corrupt_finish_confirmation AFTER UPDATE ON fast_img_tasks
         BEGIN UPDATE fast_img_tasks SET header_json=X'7B7D'
             WHERE task_key=NEW.task_key; END;",
    )?;
    let proof = verify_failed_retry_finish(&current)?;
    let error = finalize_failed_retry_finish(&current, proof).unwrap_err();
    assert!(error.to_string().contains("BLAKE3"));
    assert_eq!(read_marker(&child.working_copy)?, child);
    assert_eq!(read_marker(&parent.working_copy)?, parent);
    assert!(!child.src_dir.exists());
    Ok(())
}

#[test]
fn finish_only_binds_the_exact_primary_cleanup_journal() -> Result<()> {
    use crate::fast_img_store::{PrimaryCleanupJournal, PrimaryCleanupScope};
    use crate::image::fast_img::{SourceUnlinkEvent, SourceUnlinkKind};

    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (parent, child) = fixture(root.path(), true)?;
    assert!(child.primary_cleanup_journal);
    let before_intent = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    assert!(before_intent.primary_cleanup_blake3().is_some());
    assert!(before_intent.validate_eligibility().is_err());

    let input = child.src_dir.join("retained.jpg");
    let xmp = child.src_dir.join("retained.xmp");
    let hash = &child.blake3_log["retained.jpg"].src;
    let xmp_hash = blake3::hash(XMP).to_hex().to_string();
    let mut journal = PrimaryCleanupJournal::open(&child)?;
    {
        let mut observe = journal.observe(&input, PrimaryCleanupScope::MediaAndCompanion)?;
        observe(
            SourceUnlinkEvent::Intent,
            SourceUnlinkKind::Media,
            &input,
            hash,
        )?;
        assert!(
            observe(
                SourceUnlinkEvent::Ready,
                SourceUnlinkKind::Media,
                &input,
                hash
            )
            .is_err()
        );
    }
    let before_xmp_intent = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    assert!(before_xmp_intent.validate_eligibility().is_err());
    let mut observe = journal.observe(&input, PrimaryCleanupScope::MediaAndCompanion)?;
    observe(
        SourceUnlinkEvent::Intent,
        SourceUnlinkKind::Xmp,
        &xmp,
        &xmp_hash,
    )?;
    observe(
        SourceUnlinkEvent::Intent,
        SourceUnlinkKind::Media,
        &input,
        hash,
    )?;
    observe(
        SourceUnlinkEvent::Ready,
        SourceUnlinkKind::Media,
        &input,
        hash,
    )?;
    std::fs::remove_file(&input)?;
    let snapshot = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    snapshot.validate_eligibility()?;
    assert!(verify_failed_retry_finish(&snapshot).is_err());
    std::fs::remove_file(&xmp)?;
    std::fs::remove_dir(&child.src_dir)?;

    let snapshot = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    let proof = verify_failed_retry_finish(&snapshot)?;
    assert!(snapshot.primary_cleanup_blake3().is_some());
    assert_eq!(
        proof.evidence().verified_primary_cleanup_blake3.as_deref(),
        snapshot.primary_cleanup_blake3()
    );

    // Record the observed unlink after the first proof, without a marker revision change.
    std::fs::create_dir(&child.src_dir)?;
    observe(
        SourceUnlinkEvent::Removed,
        SourceUnlinkKind::Media,
        &input,
        hash,
    )?;
    std::fs::remove_dir(&child.src_dir)?;
    let error = finalize_failed_retry_finish(&snapshot, proof).unwrap_err();
    assert!(error.to_string().contains("MFB_FINISH_STATE_CHANGED"));

    let current = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    assert_eq!(current.revision(), snapshot.revision());
    assert_ne!(
        current.primary_cleanup_blake3(),
        snapshot.primary_cleanup_blake3()
    );
    let proof = verify_failed_retry_finish(&current)?;
    let completed = finalize_failed_retry_finish(&current, proof)?;
    assert_eq!(read_marker(&child.working_copy)?, completed);
    assert_eq!(read_marker(&parent.working_copy)?, parent);
    let mut inconsistent = completed;
    inconsistent
        .failed_retry_finish_evidence
        .as_mut()
        .unwrap()
        .verified_primary_cleanup_blake3 = snapshot.primary_cleanup_blake3().map(str::to_owned);
    assert!(
        validate_persisted(
            &open_store_connection()?,
            current.task_key(),
            &inconsistent,
            current.revision().checked_add(1).unwrap(),
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn finish_inspection_preserves_nonterminal_stage_and_does_not_create_storage() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        state.to_str().unwrap(),
    );
    assert!(inspect_failed_retry_finish(&root.path().join("not_created"))?.is_none());
    assert!(!state.exists());
    let (_, mut child) = fixture(root.path(), false)?;
    child.stage = FastImgStageName::CleanupComplete;
    child.tier2_in_progress = true;
    write_marker(&child)?;
    assert!(!child.is_terminal());
    let snapshot = inspect_failed_retry_finish(&child.working_copy)?.unwrap();
    assert_eq!(snapshot.marker(), &child);
    assert!(inspect_failed_retry_parent(&child.working_copy).is_err());
    assert!(snapshot.validate_eligibility().is_err());
    assert_eq!(read_marker(&child.working_copy)?, child);
    Ok(())
}

#[test]
fn failed_retry_source_selection_requires_one_verified_child_without_time_guessing() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        state.to_str().unwrap(),
    );
    let source = root.path().canonicalize()?.join("source");
    assert!(resolve_failed_retry_child_for_source(&source, "jxl")?.is_none());
    assert!(!state.exists());
    assert!(!source.exists());
    let (parent, mut child) = fixture(root.path(), false)?;
    assert_eq!(
        resolve_failed_retry_child_for_source(&source, "jxl")?,
        Some(child.clone())
    );
    let moved_source = source.with_file_name("source_temporarily_moved");
    std::fs::rename(&source, &moved_source)?;
    assert_eq!(
        resolve_failed_retry_child_for_source(&source, "jxl")?,
        Some(child.clone())
    );
    assert!(!source.exists());
    std::fs::rename(&moved_source, &source)?;
    assert!(resolve_failed_retry_child_for_source(&source, "avif")?.is_none());
    let context = failed_retry_context(&child.working_copy)?.unwrap();
    let admissions = context.admissions().values().cloned().collect::<Vec<_>>();
    child.blake3_log.clear();
    child.encoded_count = 0;
    child.stage = FastImgStageName::CleanupComplete;
    child.failed_sources.clone_from(&parent.failed_sources);
    write_marker(&child)?;
    let retry_parent = inspect_failed_retry_parent(&child.working_copy)?;
    let grandchild = create_or_resume_failed_retry(
        &retry_parent,
        &admissions,
        &FailedRetryDeliveryIntent::Local,
        None,
    )?;
    assert!(child.is_terminal());
    assert!(!grandchild.is_terminal());
    std::fs::remove_dir_all(&source)?;
    let error = resolve_failed_retry_child_for_source(&source, "jxl").unwrap_err();
    assert!(error.to_string().contains("MFB_RETRY_SELECTION_AMBIGUOUS"));
    assert!(!source.exists());
    assert_eq!(read_marker(&parent.working_copy)?, parent);
    Ok(())
}
