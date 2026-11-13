use super::super::{inspect_integrity, read_marker, write_marker};
use super::*;
use crate::batch::DiscoveredSource;
use crate::pipeline::verification::{
    Blake3Entry, FastImgEncodingParameters, FastImgProcessingContract, FastImgStageName,
    LibraryAssetRecord, SkippedSourceEntry,
};

fn fixture(root: &Path) -> Result<(WorkingCopyMarker, FailedRetryParent, FailedRetryAdmission)> {
    fixture_with(root, |_| {})
}

fn fixture_with(
    root: &Path,
    configure: impl FnOnce(&mut WorkingCopyMarker),
) -> Result<(WorkingCopyMarker, FailedRetryParent, FailedRetryAdmission)> {
    let root = root.canonicalize()?;
    let source = root.join("source");
    std::fs::create_dir(&source)?;
    let failed = source.join("failed.jpg");
    std::fs::write(&failed, b"synthetic retained source")?;
    let observed = DiscoveredSource::from_metadata(&failed, &std::fs::metadata(&failed)?)?;
    let hash = observed.content_blake3();
    let admission = FailedRetryAdmission {
        source_rel: "failed.jpg".into(),
        parent_source_blake3: hash.clone(),
        admitted_source_blake3: hash.clone(),
        claim: PlannedSource::encode(&observed)?,
        xmp: FailedRetrySidecar::Absent,
        aae: FailedRetrySidecar::Absent,
        route: None,
    };
    let mut marker = WorkingCopyMarker::new(source, root.join("source_optimized"), 2);
    marker.stage = FastImgStageName::CleanupComplete;
    marker.failed_sources.insert(
        "failed.jpg".into(),
        SkippedSourceEntry {
            src: hash,
            reason: "synthetic failure".into(),
        },
    );
    marker.blake3_log.insert(
        "delivered.jpg".into(),
        Blake3Entry {
            src: blake3::hash(b"delivered source").to_hex().to_string(),
            out: blake3::hash(b"delivered output").to_hex().to_string(),
            out_rel: Some("delivered.JXL".into()),
            library_asset: None,
        },
    );
    marker.processing_contract = Some(FastImgProcessingContract {
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
    marker.tool_snapshot_manifest = Some(root.join("snapshot/manifest.json"));
    marker.tool_snapshot_identity = Some(blake3::hash(b"synthetic snapshot").to_hex().to_string());
    configure(&mut marker);
    let mut historical = serde_json::to_value(&marker)?;
    historical
        .as_object_mut()
        .unwrap()
        .remove("photos_submission_history");
    let marker: WorkingCopyMarker = serde_json::from_value(historical)?;
    write_marker(&marker)?;
    let parent = inspect_failed_retry_parent(&marker.working_copy)?;
    Ok((marker, parent, admission))
}

#[test]
fn failed_retry_is_independent_idempotent_and_preserves_parent_after_cleanup() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (original, parent, admission) = fixture(root.path())?;
    assert!(!original.working_copy.exists());
    let delivery = FailedRetryDeliveryIntent::Local;
    let tools = FailedRetryToolAuthority {
        manifest_path: root
            .path()
            .canonicalize()?
            .join("current-runtime/manifest.json"),
        identity: blake3::hash(b"current runtime").to_hex().to_string(),
    };
    assert!(find_failed_retry_child(&parent, &delivery)?.is_none());
    let child = create_or_resume_failed_retry(
        &parent,
        std::slice::from_ref(&admission),
        &delivery,
        Some(&tools),
    )?;
    assert_eq!(child.src_jpeg_count, 1);
    assert_eq!(child.stage, FastImgStageName::ScanComplete);
    assert!(
        child.blake3_log.is_empty()
            && child.failed_sources.is_empty()
            && child.photos_imported_assets.is_empty()
    );
    assert_eq!(child.tool_snapshot_identity, Some(tools.identity.clone()));
    assert_eq!(child.photos_submission_history().unwrap().intents.len(), 0);
    assert_eq!(read_marker(&original.working_copy)?, original);
    assert_eq!(
        inspect_failed_retry_parent(&original.working_copy)?.snapshot_blake3(),
        parent.snapshot_blake3()
    );
    assert_eq!(
        create_or_resume_failed_retry(
            &parent,
            std::slice::from_ref(&admission),
            &delivery,
            Some(&tools),
        )?,
        child
    );
    std::fs::remove_file(original.src_dir.join("failed.jpg"))?;
    assert_eq!(
        find_failed_retry_child(&parent, &delivery)?,
        Some(child.clone())
    );
    let context = failed_retry_context(&child.working_copy)?.context("child lineage missing")?;
    assert_eq!(context.parent(), &original);
    assert_eq!(context.admissions().len(), 1);
    assert_eq!(context.ancestors().len(), 1);
    let conn = open_readonly()?.unwrap();
    assert_eq!(row_count(&conn)?, 2);
    inspect_integrity(&conn)?;
    assert!(
        !child.working_copy.exists(),
        "Core creation must not prepare a media output directory"
    );
    Ok(())
}

#[test]
fn failed_retry_freezes_parent_and_rejects_outcomes_or_intent_outside_manifest() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (original, parent, admission) = fixture(root.path())?;
    let child = create_or_resume_failed_retry(
        &parent,
        &[admission],
        &FailedRetryDeliveryIntent::Local,
        None,
    )?;
    let mut changed = original.clone();
    changed.error = Some("changed parent".into());
    assert!(write_marker(&changed).is_err());
    write_marker(&original)?;
    for mutation in 0..4 {
        let mut changed = child.clone();
        match mutation {
            0 => changed.src_jpeg_count += 1,
            1 => {
                changed.failed_sources.insert(
                    "delivered.jpg".into(),
                    SkippedSourceEntry {
                        src: blake3::hash(b"other").to_hex().to_string(),
                        reason: "outside".into(),
                    },
                );
            }
            2 => changed.failed_retry_manifest_blake3 = None,
            3 => changed.photos_import_naming_policy = Some("changed policy".into()),
            _ => unreachable!(),
        }
        assert!(write_marker(&changed).is_err(), "mutation {mutation}");
        assert_eq!(read_marker(&child.working_copy)?, child);
    }
    assert_eq!(read_marker(&original.working_copy)?, original);
    Ok(())
}

#[test]
fn failed_retry_creation_rolls_back_and_rejects_conflicting_admissions() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (original, parent, admission) = fixture(root.path())?;
    let conn = open_store_connection()?;
    conn.execute_batch("CREATE TRIGGER reject_retry_source BEFORE INSERT ON fast_img_retry_sources BEGIN SELECT RAISE(ABORT,'synthetic retry admission failure'); END")?;
    assert!(
        create_or_resume_failed_retry(
            &parent,
            std::slice::from_ref(&admission),
            &FailedRetryDeliveryIntent::Local,
            None
        )
        .is_err()
    );
    assert!(find_failed_retry_child(&parent, &FailedRetryDeliveryIntent::Local)?.is_none());
    let tasks: i64 = conn.query_row("SELECT count(*) FROM fast_img_tasks", [], |r| r.get(0))?;
    assert_eq!(tasks, 1);
    assert_eq!(read_marker(&original.working_copy)?, original);
    conn.execute_batch("DROP TRIGGER reject_retry_source")?;
    let child = create_or_resume_failed_retry(
        &parent,
        std::slice::from_ref(&admission),
        &FailedRetryDeliveryIntent::Local,
        None,
    )?;
    let mut changed = admission;
    changed.admitted_source_blake3 = blake3::hash(b"different").to_hex().to_string();
    assert!(
        create_or_resume_failed_retry(&parent, &[changed], &FailedRetryDeliveryIntent::Local, None)
            .is_err()
    );
    assert_eq!(
        find_failed_retry_child(&parent, &FailedRetryDeliveryIntent::Local)?,
        Some(child)
    );
    Ok(())
}

#[test]
fn failed_retry_cold_read_rejects_corrupt_manifest_and_missing_relations() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (_, parent, admission) = fixture(root.path())?;
    let child = create_or_resume_failed_retry(
        &parent,
        &[admission],
        &FailedRetryDeliveryIntent::Local,
        None,
    )?;
    let conn = open_store_connection()?;
    conn.execute("UPDATE fast_img_retry_sources SET source_claim='{}'", [])?;
    assert!(failed_retry_context(&child.working_copy).is_err());
    assert!(inspect_integrity(&conn).is_err());
    conn.execute("DELETE FROM fast_img_retry_sources", [])?;
    conn.execute("DELETE FROM fast_img_retry_tasks", [])?;
    assert!(
        read_marker(&child.working_copy).is_err(),
        "Missing lineage must not make a retry child ordinary"
    );
    assert!(inspect_integrity(&conn).is_err());
    Ok(())
}

#[test]
fn failed_retry_preserves_complete_ancestry_without_inventing_old_coverage() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (original, parent, admission) = fixture(root.path())?;
    let mut child = create_or_resume_failed_retry(
        &parent,
        std::slice::from_ref(&admission),
        &FailedRetryDeliveryIntent::Local,
        None,
    )?;
    child.stage = FastImgStageName::CleanupComplete;
    child.failed_sources.insert(
        admission.source_rel.clone(),
        SkippedSourceEntry {
            src: admission.admitted_source_blake3.clone(),
            reason: "child failure".into(),
        },
    );
    write_marker(&child)?;
    let next_parent = inspect_failed_retry_parent(&child.working_copy)?;
    let grandchild = create_or_resume_failed_retry(
        &next_parent,
        &[admission],
        &FailedRetryDeliveryIntent::Local,
        None,
    )?;
    let context = failed_retry_context(&grandchild.working_copy)?.unwrap();
    assert_eq!(context.ancestors(), &[child.clone(), original.clone()]);
    assert_eq!(context.parent(), &child);
    assert!(
        context
            .ancestors()
            .last()
            .unwrap()
            .photos_submission_history()
            .is_none()
    );
    assert_eq!(read_marker(&original.working_copy)?, original);
    let mut changed = child;
    changed.error = Some("attempt to rewrite child now used as parent".into());
    assert!(write_marker(&changed).is_err());
    inspect_integrity(&open_store_connection()?)?;
    Ok(())
}

#[test]
fn failed_retry_delivery_request_is_immutable_and_first_photos_binding_is_allowed() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (original, parent, admission) = fixture(root.path())?;
    assert!(original.photos_library_path.is_none());
    assert!(original.photos_import_naming_policy.is_none());
    let request = FailedRetryDeliveryIntent::Photos {
        library_path: None,
        naming_policy: "[null,\"Current album\",true]".into(),
    };
    let mut child =
        create_or_resume_failed_retry(&parent, std::slice::from_ref(&admission), &request, None)?;
    assert_eq!(child.photos_library_path, None);
    assert_eq!(
        child.photos_import_naming_policy.as_deref(),
        Some("[null,\"Current album\",true]")
    );
    assert!(
        child.tool_snapshot_identity.is_none(),
        "A new attempt must not inherit the parent's tool authority"
    );
    let anchor = child.failed_retry_manifest_blake3.clone();
    let selected = root
        .path()
        .canonicalize()?
        .join("Currently selected.photoslibrary");
    child.photos_library_path = Some(selected);
    write_marker(&child)?;
    assert_eq!(child.failed_retry_manifest_blake3, anchor);
    assert_eq!(
        failed_retry_context(&child.working_copy)?
            .unwrap()
            .requested_delivery(),
        &request
    );
    assert_eq!(
        find_failed_retry_child(&parent, &request)?,
        Some(child.clone())
    );
    assert!(find_failed_retry_child(&parent, &FailedRetryDeliveryIntent::Local).is_err());
    assert!(
        create_or_resume_failed_retry(
            &parent,
            &[admission],
            &FailedRetryDeliveryIntent::Local,
            None
        )
        .is_err()
    );
    let changed_request = FailedRetryDeliveryIntent::Photos {
        library_path: None,
        naming_policy: "[null,\"Different album\",true]".into(),
    };
    assert!(find_failed_retry_child(&parent, &changed_request).is_err());
    for path in [
        None,
        Some(root.path().canonicalize()?.join("Different.photoslibrary")),
    ] {
        let mut changed = child.clone();
        changed.photos_library_path = path;
        assert!(write_marker(&changed).is_err());
    }
    assert_eq!(read_marker(&original.working_copy)?, original);
    assert_eq!(read_marker(&child.working_copy)?, child);
    Ok(())
}

#[test]
fn failed_retry_photos_receipts_bind_exact_per_file_custody() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (_, parent, admission) = fixture(root.path())?;
    let request = FailedRetryDeliveryIntent::Photos {
        library_path: Some(root.path().canonicalize()?.join("Synthetic.photoslibrary")),
        naming_policy: "[null,null,true]".into(),
    };
    let mut child =
        create_or_resume_failed_retry(&parent, std::slice::from_ref(&admission), &request, None)?;
    child.stage = FastImgStageName::Importing;
    let output_rel = "failed.JXL";
    let output_hash = blake3::hash(b"synthetic encoded output").to_hex().to_string();
    let wrong_hash = blake3::hash(b"different synthetic bytes").to_hex().to_string();
    let mut original = LibraryAssetRecord {
        rel_path: admission.source_rel.clone(),
        blake3: wrong_hash.clone(),
        photos_uuid: Some("synthetic-original-uuid".into()),
        ..LibraryAssetRecord::default()
    };
    let mut wrong_original = child.clone();
    wrong_original.tier2_imported_assets.push(original.clone());
    let error = write_marker(&wrong_original).expect_err("original receipt has different bytes");
    assert!(error.to_string().contains("admitted source bytes"), "{error}");
    original.blake3.clone_from(&admission.admitted_source_blake3);
    let mut valid_original = child.clone();
    valid_original.tier2_imported_assets.push(original.clone());
    write_marker(&valid_original)?;
    assert_eq!(read_marker(&child.working_copy)?, valid_original);

    child.blake3_log.insert(
        admission.source_rel.clone(),
        Blake3Entry {
            src: admission.admitted_source_blake3,
            out: output_hash.clone(),
            out_rel: Some(output_rel.into()),
            library_asset: Some(output_hash.clone()),
        },
    );
    child.photos_imported_assets.push(LibraryAssetRecord {
        rel_path: output_rel.into(),
        blake3: output_hash,
        photos_uuid: Some("synthetic-output-uuid".into()),
        ..LibraryAssetRecord::default()
    });
    let mut conflicting_original = child.clone();
    conflicting_original.tier2_imported_assets.push(original);
    let error = write_marker(&conflicting_original).expect_err("source has two delivery outcomes");
    assert!(error.to_string().contains("both a converted output"), "{error}");
    write_marker(&child)?;
    let mut no_uuid = child.clone();
    no_uuid.photos_imported_assets[0].photos_uuid = None;
    let error = write_marker(&no_uuid).expect_err("output receipt has no UUID");
    assert!(error.to_string().contains("has no Photos UUID"), "{error}");
    let mut wrong_output = child.clone();
    wrong_output.photos_imported_assets[0].blake3.clone_from(&wrong_hash);
    let error = write_marker(&wrong_output).expect_err("output receipt has different bytes");
    assert!(error.to_string().contains("recorded output bytes"), "{error}");
    assert_eq!(read_marker(&child.working_copy)?, child);

    let conn = open_store_connection()?;
    let key = key_for_canonical(text(&child.working_copy)?);
    let digest = super::super::snapshot_digest(
        text(&child.working_copy)?,
        text(&child.src_dir)?,
        &wrong_output,
    )?;
    conn.execute(
        "UPDATE fast_img_photos_receipts SET blake3=?2 WHERE task_key=?1 AND receipt_kind='primary'",
        params![key, wrong_hash],
    )?;
    conn.execute(
        "UPDATE fast_img_tasks SET snapshot_blake3=?2 WHERE task_key=?1",
        params![key, digest.as_bytes().as_slice()],
    )?;
    let error = read_marker(&child.working_copy).expect_err("cold read accepted wrong output bytes");
    assert!(error.to_string().contains("recorded output bytes"), "{error}");
    Ok(())
}

#[test]
fn failed_retry_new_avif_attempt_derives_metadata_version_without_rewriting_failed_parent()
-> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _env = crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.path().join("state").to_str().unwrap(),
    );
    let (original, parent, admission) = fixture_with(root.path(), |marker| {
        marker.blake3_log.clear();
        marker.src_jpeg_count = 1;
        marker.strategy = "avif".into();
        marker.processing_contract.as_mut().unwrap().encoding = FastImgEncodingParameters::Avif {
            avif_quality: 100,
            avif_speed: 0,
            quality_search: crate::runtime_config::QualitySearch::Adaptive,
            avif_metadata_policy: crate::runtime_config::AvifMetadataPolicy::Clear,
            allow_expert_options: false,
        };
    })?;
    assert_eq!(original.metadata_policy_version, 0);
    let mut child = create_or_resume_failed_retry(
        &parent,
        std::slice::from_ref(&admission),
        &FailedRetryDeliveryIntent::Local,
        None,
    )?;
    assert_eq!(
        child.metadata_policy_version,
        crate::runtime_config::AvifMetadataPolicy::Clear.version()
    );
    assert_eq!(child.metadata_policy_version, 4);
    child.stage = FastImgStageName::TranscodeComplete;
    child.encoded_count = 1;
    child.blake3_log.insert(
        admission.source_rel,
        Blake3Entry {
            src: admission.admitted_source_blake3,
            out_rel: Some("failed.AVIF".into()),
            out: blake3::hash(b"candidate").to_hex().to_string(),
            library_asset: None,
        },
    );
    write_marker(&child)?;
    assert_eq!(read_marker(&child.working_copy)?, child);
    assert_eq!(read_marker(&original.working_copy)?, original);
    assert_eq!(original.metadata_policy_version, 0);
    Ok(())
}

#[test]
fn routed_original_retry_requires_exact_settlement_and_preserves_parent() -> Result<()> {
    let root = tempfile::tempdir()?;
    let _store = crate::mfb_sqlite_store::set_test_store_path_for_tests(
        root.path().join("state.sqlite"),
    );
    let (parent_marker, parent, mut admission) = fixture_with(root.path(), |marker| {
        marker.src_jpeg_count = 0;
        marker.blake3_log.clear();
        marker.tier2_failed_sources = std::mem::take(&mut marker.failed_sources);
    })?;
    let captured = super::super::capture_failed_retry_admissions(&parent)?;
    assert_eq!(captured, [admission.clone()]);
    assert!(create_or_resume_failed_retry(&parent, &captured, &FailedRetryDeliveryIntent::Local, None).is_err());
    admission.route = Some(FailedRetrySourceRoute::Original);
    let admissions = std::slice::from_ref(&admission);
    assert!(create_or_resume_failed_retry(&parent, admissions, &FailedRetryDeliveryIntent::Local, None).is_err());
    let delivery = FailedRetryDeliveryIntent::Photos {
        library_path: Some(root.path().canonicalize()?.join("Synthetic.photoslibrary")),
        naming_policy: "[null,null,false]".into(),
    };
    let mut child = create_or_resume_failed_retry(&parent, admissions, &delivery, None)?;
    assert_eq!(child.src_jpeg_count, 1);
    assert_eq!(child.primary_source_count(), 0);
    assert!(child.tier2_cleanup_journal);
    assert_eq!(
        failed_retry_context(&child.working_copy)?.unwrap().admissions().get(&admission.source_rel),
        Some(&admission)
    );
    child.stage = FastImgStageName::CleanupComplete;
    assert!(write_marker(&child).is_err(), "missing original outcome must block completion");
    child.tier2_failed_sources.insert(admission.source_rel.clone(), SkippedSourceEntry {
        src: blake3::hash(b"wrong original").to_hex().to_string(),
        reason: "synthetic pre-submit failure".into(),
    });
    assert!(write_marker(&child).is_err(), "wrong original hash must block completion");
    child.tier2_failed_sources.get_mut(&admission.source_rel).unwrap().src.clone_from(&admission.admitted_source_blake3);
    write_marker(&child)?;
    assert_eq!(read_marker(&child.working_copy)?, child);
    assert_eq!(read_marker(&parent_marker.working_copy)?, parent_marker);
    assert_eq!(inspect_failed_retry_parent(&child.working_copy)?.marker().failed_source_count(), 1);
    Ok(())
}
