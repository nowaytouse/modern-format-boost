use super::*;
use crate::pipeline::verification::{
    FastImgEncodingParameters, FastImgProcessingContract, FastImgStageName, Gate1Checks,
    Gate2Checks, Gate3Checks, resolve_fresh_working_copy_dir,
};
use std::fs;

fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn state_guard(root: &Path) -> crate::common_utils::EnvGuard {
    crate::common_utils::EnvGuard::set(
        crate::constants::ENV_MFB_HOME_ROOT,
        root.to_str().expect("synthetic state path is UTF-8"),
    )
}

#[test]
fn historical_source_evidence_binds_exact_receipts_and_keeps_legacy_unknown() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().canonicalize()?;
    let db = root.join("state.sqlite");
    let _store = crate::mfb_sqlite_store::set_test_store_path_for_tests(db.clone());
    let marker = complete_marker(&root)?;
    let source = "nested/converted.jpg";
    write_marker(&marker)?;
    let _writer = open_store_connection()?;

    let wal = db.with_extension("sqlite-wal");
    let snapshot = || -> Result<_> {
        Ok((
            blake3::hash(&fs::read(&db)?),
            fs::read(&wal).ok().map(|bytes| blake3::hash(&bytes)),
        ))
    };
    let before = snapshot()?;
    let primary = read_source_evidence(&marker.working_copy, source)?;
    assert_eq!(primary.coverage, "recorded");
    assert_eq!(
        primary.task_key,
        key_for_canonical(text(&marker.working_copy)?)
    );
    assert_eq!(
        primary
            .photos_receipt
            .context("no primary receipt")?
            .rel_path,
        "nested/converted.JXL"
    );
    assert!(matches!(
        primary.outcome,
        Some(source_evidence::SourceOutcome::Converted { .. })
    ));
    assert_eq!(primary.photos_submissions.len(), 1);
    assert!(primary.cleanup.is_empty());
    assert_eq!(primary.cleanup_journal, None);

    let original = read_source_evidence(&marker.working_copy, "original.webp")?;
    assert!(matches!(
        original.outcome,
        Some(source_evidence::SourceOutcome::OriginalDelivery)
    ));
    assert_eq!(
        original
            .photos_receipt
            .context("no original receipt")?
            .rel_path,
        "original.webp"
    );
    let absent = read_source_evidence(&marker.working_copy, "nested/Converted.jpg")?;
    assert_eq!(absent.coverage, "unknown");
    assert!(absent.outcome.is_none() && absent.photos_submissions.is_empty());
    assert!(read_source_evidence(&marker.working_copy, "nested/../converted.jpg").is_err());
    assert!(read_source_evidence(&marker.working_copy, "nested//converted.jpg").is_err());
    assert_eq!(before, snapshot()?);
    Ok(())
}

fn complete_marker(root: &Path) -> Result<WorkingCopyMarker> {
    let root = root.canonicalize()?;
    let source = root.join("source");
    fs::create_dir(&source)?;
    let mut marker = WorkingCopyMarker::new(source, root.join("source_optimized"), 3);
    marker.stage = FastImgStageName::CleanupComplete;
    marker.encoded_count = 1;
    marker.gate1_checks = Gate1Checks {
        count: true.into(),
        blake3: true.into(),
        metadata: true.into(),
        size: true.into(),
        orient: true.into(),
        decode: true.into(),
    };
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
    marker.blake3_log.insert(
        "nested/converted.jpg".into(),
        Blake3Entry {
            src: digest(b"converted source"),
            out: digest(b"converted output"),
            out_rel: Some("nested/converted.JXL".into()),
            library_asset: Some(digest(b"converted output")),
        },
    );
    marker.skipped_sources.insert(
        "skipped.jpg".into(),
        SkippedSourceEntry {
            src: digest(b"skipped source"),
            reason: "verified no size benefit".into(),
        },
    );
    marker.failed_sources.insert(
        "failed.jpg".into(),
        SkippedSourceEntry {
            src: digest(b"failed source"),
            reason: "synthetic unsupported input retained".into(),
        },
    );
    marker.photos_library_path = Some(root.join("Synthetic.photoslibrary"));
    marker.photos_import_naming_policy = Some("[null,null,true]".into());
    marker.photos_imported_assets.push(LibraryAssetRecord {
        rel_path: "nested/converted.JXL".into(),
        blake3: digest(b"converted output"),
        sync_status: "photos_local".into(),
        quarantined: false,
        photos_uuid: Some("synthetic-primary-uuid".into()),
        library_blake3: Some(digest(b"Photos resource bytes")),
        xmp_sidecar_blake3: None,
    });
    marker.tier2_imported_assets.push(LibraryAssetRecord {
        rel_path: "original.webp".into(),
        blake3: digest(b"original source"),
        sync_status: "icloud".into(),
        quarantined: true,
        photos_uuid: Some("synthetic-original-uuid".into()),
        library_blake3: Some(digest(b"original XMP-enriched delivery")),
        xmp_sidecar_blake3: Some(digest(b"external XMP")),
    });
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
    marker.tool_snapshot_manifest = Some(root.join("tool-runtime/manifest.json"));
    marker.tool_snapshot_identity = Some(digest(b"synthetic tool runtime"));
    marker.error = Some("one retained failure; completed evidence remains authoritative".into());
    let intents = [
        PhotosSubmissionIntent {
            kind: PhotosSubmissionKind::Encoded,
            source_rel: "nested/converted.jpg".into(),
            source_blake3: digest(b"converted source"),
            output_rel: "nested/converted.JXL".into(),
            output_blake3: digest(b"converted output"),
            album_name: "Synthetic/Nested".into(),
            photos_library_path: marker.photos_library_path.clone(),
            backend: PhotosSubmissionBackend::Applescript {
                request_id: digest(b"script request"),
            },
        },
        PhotosSubmissionIntent {
            kind: PhotosSubmissionKind::Original,
            source_rel: "original.webp".into(),
            source_blake3: digest(b"original source"),
            output_rel: "original.webp".into(),
            output_blake3: digest(b"original XMP-enriched delivery"),
            album_name: "Synthetic".into(),
            photos_library_path: marker.photos_library_path.clone(),
            backend: PhotosSubmissionBackend::Native {
                journal_path: root.join("native-journals/request.jsonl"),
                request_blake3: digest(b"exact native request"),
            },
        },
    ];
    marker
        .record_photos_submissions(&intents)
        .map_err(anyhow::Error::msg)?;
    validate_marker_structure(&marker)?;
    Ok(marker)
}

fn table_count(conn: &Connection, table: &str) -> Result<i64> {
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?,
    )
}

fn raw_family_rows(
    conn: &Connection,
) -> Result<BTreeMap<&'static str, Vec<Vec<rusqlite::types::Value>>>> {
    let mut family = BTreeMap::new();
    for table in TABLES {
        let mut statement = conn.prepare(&format!("SELECT * FROM {table} ORDER BY 1, 2"))?;
        let columns = statement.column_count();
        let mut rows = statement.query([])?;
        let mut values = Vec::new();
        while let Some(row) = rows.next()? {
            values.push(
                (0..columns)
                    .map(|index| row.get(index))
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
        family.insert(table, values);
    }
    Ok(family)
}

#[test]
fn complete_dto_roundtrip_uses_normalized_collection_rows() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let marker = complete_marker(root.path())?;
    write_marker(&marker)?;
    assert_eq!(read_marker(&marker.working_copy)?, marker);
    let conn = open_store_connection()?;
    for (table, expected) in [
        ("fast_img_tasks", 1),
        ("fast_img_source_outcomes", 3),
        ("fast_img_photos_requests", 2),
        ("fast_img_photos_entries", 2),
        ("fast_img_photos_receipts", 2),
        ("fast_img_migrations", 0),
    ] {
        assert_eq!(table_count(&conn, table)?, expected, "{table}");
    }
    assert_eq!(inspect_row_count(&conn)?, 10);
    assert_eq!(inspect_integrity(&conn)?, 10);
    let header: Vec<u8> = conn.query_row("SELECT header_json FROM fast_img_tasks", [], |row| {
        row.get(0)
    })?;
    let header: WorkingCopyMarker = serde_json::from_slice(&header)?;
    assert!(
        header.blake3_log.is_empty()
            && header.skipped_sources.is_empty()
            && header.failed_sources.is_empty()
            && header.photos_imported_assets.is_empty()
            && header.tier2_imported_assets.is_empty()
    );
    assert_eq!(header.photos_submission_history().unwrap().intents, []);
    assert_eq!(matching_markers(&marker.src_dir, "jxl")?, [marker]);
    Ok(())
}

#[test]
fn jpeg_execution_receipt_is_atomic_historical_and_rejects_corruption() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let mut published = complete_marker(root.path())?;
    published.stage = FastImgStageName::OutputPrepared;
    published.photos_imported_assets.clear();
    published.tier2_imported_assets.clear();
    let rel = "nested/converted.jpg";
    let source = published.src_dir.join(rel);
    let delivery = published.working_copy.join("nested/converted.JXL");
    fs::create_dir_all(source.parent().unwrap())?;
    fs::create_dir_all(delivery.parent().unwrap())?;
    fs::write(&source, b"converted source")?;
    fs::write(&delivery, b"converted output")?;
    let mut prior = published.clone();
    prior.encoded_count = 0;
    prior.blake3_log.clear();
    write_marker(&prior)?;
    assert!(read_jpeg_execution_receipts_for_source(&published.working_copy, rel)?.is_empty());
    let encoder = JpegEncoderInvocation {
        executable: "/synthetic/cjxl".to_owned(),
        executable_blake3: digest(b"encoder"),
        argv: [
            source.to_str().unwrap(),
            "/synthetic/scratch/winner.jxl",
            "--container=1",
            "-e",
            "7",
            "--num_threads",
            "1",
            "--lossless_jpeg=1",
        ]
        .map(str::to_owned)
        .to_vec(),
        idle_timeout_seconds: 60,
        hard_timeout_seconds: 120,
        exit_code: 0,
    };
    let attempt = Some(JpegExecutionAttempt {
        run_id: Some("synthetic-run".into()),
        invocation_id: "synthetic-invocation".into(),
    });
    let conn = open_store_connection()?;
    conn.execute_batch(
        "CREATE TRIGGER reject_synthetic_execution BEFORE INSERT ON fast_img_jpeg_execution
         BEGIN SELECT RAISE(ABORT,'synthetic JPEG execution write failure'); END",
    )?;
    let error = write_marker_with_jpeg_execution(&published, rel, encoder.clone(), attempt.clone())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("synthetic JPEG execution write failure"),
        "{error:#}"
    );
    assert_eq!(read_marker(&published.working_copy)?, prior);
    assert!(read_jpeg_execution_receipts_for_source(&published.working_copy, rel)?.is_empty());
    assert!(
        read_source_evidence(&published.working_copy, rel)?
            .primary_result
            .is_none()
    );
    assert_eq!(fs::read(&source)?, b"converted source");
    assert_eq!(fs::read(&delivery)?, b"converted output");
    conn.execute_batch("DROP TRIGGER reject_synthetic_execution")?;
    write_marker_with_jpeg_execution(&published, rel, encoder.clone(), attempt.clone())?;
    write_marker_with_jpeg_execution(&published, rel, encoder.clone(), attempt.clone())?;
    let receipts = read_jpeg_execution_receipts_for_source(&published.working_copy, rel)?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(table_count(&conn, "fast_img_primary_results")?, 1);
    let evidence = read_source_evidence(&published.working_copy, rel)?;
    let primary_result = evidence.primary_result.context("missing primary result")?;
    assert_eq!(
        primary_result
            .observation
            .context("missing primary observation")?
            .attempt,
        attempt
    );
    assert_eq!(receipts[0].schema_version, 1);
    assert!(receipts[0].reconstructed_jpeg_blake3.is_none());
    assert_eq!(receipts[0].encoder, encoder);
    assert_eq!(receipts[0].attempt, attempt);
    assert_eq!(receipts[0].source_blake3, digest(b"converted source"));
    assert_eq!(receipts[0].output_blake3, digest(b"converted output"));
    assert_eq!(receipts[0].logical_task_key, receipts[0].task_key);
    write_marker(&prior)?;
    write_marker(&published)?;
    assert!(
        read_source_evidence(&published.working_copy, rel)?
            .primary_result
            .is_none(),
        "an unobserved matching outcome cannot revive prior attempt attribution"
    );
    write_marker(&prior)?;
    assert_eq!(
        read_jpeg_execution_receipts_for_source(&published.working_copy, rel)?,
        receipts
    );
    assert!(
        read_jpeg_execution_receipts_for_source(&published.working_copy, "Nested/converted.jpg")?
            .is_empty()
    );
    assert_eq!(inspect_integrity(&conn)?, inspect_row_count(&conn)?);
    conn.execute(
        "UPDATE fast_img_jpeg_execution SET source_rel='wrong.jpg'",
        [],
    )?;
    assert!(read_jpeg_execution_receipts_for_source(&published.working_copy, "wrong.jpg").is_err());
    assert!(inspect_integrity(&conn).is_err());
    conn.execute("UPDATE fast_img_jpeg_execution SET source_rel=?1", [rel])?;
    conn.execute(
        "INSERT INTO fast_img_jpeg_execution SELECT task_key,'unrelated.jpg',receipt_key,
         payload,zeroblob(32) FROM fast_img_jpeg_execution WHERE source_rel=?1",
        [rel],
    )?;
    assert_eq!(
        read_jpeg_execution_receipts_for_source(&published.working_copy, rel)?,
        receipts
    );
    assert!(inspect_integrity(&conn).is_err());
    conn.execute(
        "DELETE FROM fast_img_jpeg_execution WHERE source_rel='unrelated.jpg'",
        [],
    )?;
    conn.execute(
        "UPDATE fast_img_jpeg_execution SET payload_blake3=zeroblob(32)",
        [],
    )?;
    assert!(read_jpeg_execution_receipts_for_source(&published.working_copy, rel).is_err());
    assert!(inspect_integrity(&conn).is_err());
    assert_eq!(read_marker(&published.working_copy)?, prior);
    Ok(())
}

#[test]
fn reconstructed_jpeg_execution_keeps_jxl_and_encoder_input_evidence_distinct() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let mut prior = complete_marker(root.path())?;
    prior.stage = FastImgStageName::OutputPrepared;
    prior.photos_imported_assets.clear();
    prior.tier2_imported_assets.clear();
    let mut marker = prior.clone();
    let source_rel = "no-extension";
    let output_blake3 = digest(b"published JXL");
    marker.encoded_count += 1;
    marker.blake3_log.insert(
        source_rel.into(),
        Blake3Entry {
            src: digest(b"admitted JXL"),
            out: output_blake3.clone(),
            out_rel: Some("no-extension.JXL".into()),
            library_asset: None,
        },
    );
    let reconstructed_jpeg_blake3 = digest(b"reconstructed JPEG");
    let scratch_jpeg = "/synthetic/tmp/mfb-jxl-abc.jpg";
    let encoder = JpegEncoderInvocation {
        executable: "/synthetic/cjxl".into(),
        executable_blake3: digest(b"encoder"),
        argv: [
            scratch_jpeg,
            "/synthetic/tmp/mfb-jpeg-full-effort-abc/e7.jxl",
            "-e",
            "7",
            "--num_threads",
            "1",
            "--lossless_jpeg=1",
        ]
        .map(str::to_owned)
        .to_vec(),
        idle_timeout_seconds: 60,
        hard_timeout_seconds: 120,
        exit_code: 0,
    };
    write_marker(&prior)?;
    let proof = |candidate_blake3: String| ReconstructedJpegExecution {
        reconstructed_jpeg_blake3: reconstructed_jpeg_blake3.clone(),
        candidate_blake3,
        encoder: encoder.clone(),
    };
    assert!(write_marker_with_jpeg_execution(&marker, source_rel, encoder.clone(), None).is_err());
    assert!(
        write_marker_with_reconstructed_jpeg_execution(
            &marker,
            source_rel,
            proof(digest(b"wrong candidate")),
            None,
        )
        .is_err()
    );
    assert_eq!(read_marker(&marker.working_copy)?, prior);
    assert!(read_jpeg_execution_receipts_for_source(&marker.working_copy, source_rel)?.is_empty());
    write_marker_with_reconstructed_jpeg_execution(
        &marker,
        source_rel,
        proof(output_blake3.clone()),
        None,
    )?;
    let receipts = read_jpeg_execution_receipts_for_source(&marker.working_copy, source_rel)?;
    let [receipt] = receipts.as_slice() else {
        anyhow::bail!("Expected one replacement receipt")
    };
    assert_eq!(receipt.schema_version, 2);
    assert_eq!(receipt.source_blake3, digest(b"admitted JXL"));
    assert_eq!(
        receipt.reconstructed_jpeg_blake3.as_ref(),
        Some(&reconstructed_jpeg_blake3)
    );
    assert_eq!(receipt.candidate_blake3.as_ref(), Some(&output_blake3));
    assert_eq!(receipt.output_blake3, output_blake3);
    assert_eq!(receipt.encoder, encoder);
    assert_eq!(read_marker(&marker.working_copy)?, marker);
    let conn = open_store_connection()?;
    assert_eq!(inspect_integrity(&conn)?, inspect_row_count(&conn)?);
    Ok(())
}

#[test]
fn avif_execution_receipt_is_atomic_historical_and_rejects_corruption() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let src_dir = root.path().canonicalize()?.join("source");
    fs::create_dir(&src_dir)?;
    let mut marker = WorkingCopyMarker::new(src_dir, root.path().canonicalize()?.join("output"), 1);
    marker.strategy = "avif".into();
    marker.stage = FastImgStageName::OutputPrepared;
    marker.processing_contract = Some(FastImgProcessingContract {
        schema: 1,
        tool_policy: crate::runtime_config::ToolPolicy::Single,
        fallback_policy: crate::runtime_config::FallbackPolicy::Strict,
        encoding: FastImgEncodingParameters::Avif {
            avif_quality: 100,
            avif_speed: 8,
            quality_search: crate::runtime_config::QualitySearch::Fixed,
            avif_metadata_policy: crate::runtime_config::AvifMetadataPolicy::Clear,
            allow_expert_options: false,
        },
    });
    let before = marker.clone();
    write_marker(&before)?;
    let source_rel = "nested/converted.png";
    let output_blake3 = digest(b"verified candidate");
    marker.encoded_count = 1;
    marker.blake3_log.insert(
        source_rel.into(),
        Blake3Entry {
            src: digest(b"original source"),
            out: output_blake3.clone(),
            out_rel: Some("nested/converted.AVIF".into()),
            library_asset: None,
        },
    );
    let encoder = AvifEncoderInvocation {
        executable: "/synthetic/avifenc".into(),
        executable_blake3: digest(b"avifenc"),
        argv: [
            "--speed",
            "8",
            "-q",
            "100",
            "--cicp",
            "1/13/0",
            "--progressive",
            "/synthetic/scratch/normalized.png",
            "/synthetic/scratch/winning.avif",
        ]
        .map(str::to_owned)
        .to_vec(),
        idle_timeout_seconds: 60,
        hard_timeout_seconds: 120,
        exit_code: 0,
    };
    let attempt = Some(JpegExecutionAttempt {
        run_id: Some("synthetic-run".into()),
        invocation_id: "synthetic-avif".into(),
    });
    let conn = open_store_connection()?;
    conn.execute_batch(
        "CREATE TRIGGER reject_synthetic_avif BEFORE INSERT ON fast_img_avif_execution
         BEGIN SELECT RAISE(ABORT,'synthetic AVIF receipt write failure'); END",
    )?;
    let save = |hash: &str| {
        write_marker_with_avif_execution(
            &marker,
            source_rel,
            hash,
            encoder.clone(),
            attempt.clone(),
        )
    };
    assert!(
        save(&digest(b"wrong candidate"))
            .unwrap_err()
            .to_string()
            .contains("candidate")
    );
    assert!(
        save(&output_blake3)
            .unwrap_err()
            .to_string()
            .contains("synthetic AVIF receipt write failure")
    );
    assert_eq!(read_marker(&marker.working_copy)?, before);
    assert!(
        read_avif_execution_receipt_page(&marker.working_copy, source_rel, None)?
            .receipts
            .is_empty()
    );
    conn.execute_batch("DROP TRIGGER reject_synthetic_avif")?;
    save(&output_blake3)?;
    save(&output_blake3)?;

    let page = read_avif_execution_receipt_page(&marker.working_copy, source_rel, None)?;
    assert!(page.next_cursor.is_none());
    assert_eq!(page.receipts.len(), 1);
    let receipt = &page.receipts[0];
    assert_eq!(receipt.encoder, encoder);
    assert_eq!(receipt.attempt, attempt);
    assert_eq!(
        receipt.candidate_blake3.as_deref(),
        Some(output_blake3.as_str())
    );
    assert_eq!(receipt.output_blake3, output_blake3);
    assert_eq!(receipt.source_blake3, digest(b"original source"));
    assert_eq!(read_marker(&marker.working_copy)?, marker);
    assert!(
        read_avif_execution_receipt_page(&marker.working_copy, "Nested/converted.png", None)?
            .receipts
            .is_empty()
    );
    assert!(read_jpeg_execution_receipts_for_source(&marker.working_copy, source_rel)?.is_empty());
    assert_eq!(inspect_integrity(&conn)?, inspect_row_count(&conn)?);
    conn.execute(
        "UPDATE fast_img_avif_execution SET payload_blake3=zeroblob(32)",
        [],
    )?;
    assert!(read_avif_execution_receipt_page(&marker.working_copy, source_rel, None).is_err());
    assert!(inspect_integrity(&conn).is_err());
    Ok(())
}

#[test]
fn jpeg_execution_pages_preserve_order_and_reject_foreign_cursors() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let mut marker = complete_marker(root.path())?;
    marker.stage = FastImgStageName::OutputPrepared;
    marker.photos_imported_assets.clear();
    marker.tier2_imported_assets.clear();
    let mut other = marker.clone();
    other.working_copy = marker.working_copy.with_file_name("other_output");
    let rel = "nested/converted.jpg";
    let source = marker.src_dir.join(rel);
    let encoder = JpegEncoderInvocation {
        executable: "/synthetic/cjxl".into(),
        executable_blake3: digest(b"encoder"),
        argv: [
            source.to_str().unwrap(),
            "/synthetic/scratch/winner.jxl",
            "--container=1",
            "-e",
            "7",
            "--num_threads",
            "1",
            "--lossless_jpeg=1",
        ]
        .map(str::to_owned)
        .to_vec(),
        idle_timeout_seconds: 60,
        hard_timeout_seconds: 120,
        exit_code: 0,
    };
    for index in 0..65 {
        write_marker_with_jpeg_execution(
            &marker,
            rel,
            encoder.clone(),
            Some(JpegExecutionAttempt {
                run_id: None,
                invocation_id: format!("synthetic-invocation-{index}"),
            }),
        )?;
    }
    write_marker_with_jpeg_execution(&other, rel, encoder, None)?;
    let conn = open_store_connection()?;
    let database = database_path()?;
    let wal = database.with_extension("sqlite-wal");
    let before = (fs::read(&database)?, fs::read(&wal)?);
    let keys = conn
        .prepare(
            "SELECT lower(hex(receipt_key)) FROM fast_img_jpeg_execution
         WHERE task_key=?1 AND source_rel=?2 ORDER BY receipt_key",
        )?
        .query_map(
            rusqlite::params![
                key_for_canonical(&canonical_text(&marker.working_copy)?),
                rel
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let first = read_jpeg_execution_receipt_page(&marker.working_copy, rel, None)?;
    assert_eq!(first.receipts.len(), 64);
    assert_eq!(first.next_cursor.as_deref(), Some(keys[63].as_str()));
    let repeated = read_jpeg_execution_receipt_page(&marker.working_copy, rel, None)?;
    assert_eq!(repeated.receipts, first.receipts);
    let second =
        read_jpeg_execution_receipt_page(&marker.working_copy, rel, first.next_cursor.as_deref())?;
    assert_eq!(second.receipts.len(), 1);
    assert!(second.next_cursor.is_none());
    let invocation_ids = first
        .receipts
        .iter()
        .chain(&second.receipts)
        .map(|receipt| &receipt.attempt.as_ref().unwrap().invocation_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(invocation_ids.len(), 65);
    let end = read_jpeg_execution_receipt_page(&marker.working_copy, rel, Some(keys[64].as_str()))?;
    assert!(end.receipts.is_empty() && end.next_cursor.is_none());
    assert!(read_jpeg_execution_receipts_for_source(&marker.working_copy, rel).is_err());
    assert!(
        read_jpeg_execution_receipt_page(&other.working_copy, rel, first.next_cursor.as_deref())
            .is_err()
    );
    assert!(
        read_jpeg_execution_receipt_page(
            &marker.working_copy,
            "Nested/converted.jpg",
            first.next_cursor.as_deref(),
        )
        .is_err()
    );
    assert!(read_jpeg_execution_receipt_page(&marker.working_copy, rel, Some("bad")).is_err());
    assert_eq!((fs::read(&database)?, fs::read(&wal)?), before);
    Ok(())
}

#[test]
fn changing_one_outcome_updates_only_that_row_and_the_task_digest() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let mut marker = complete_marker(root.path())?;
    write_marker(&marker)?;
    let mut conn = open_store_connection()?;
    let before: i64 = conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
    let unchanged = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    write_snapshot(&unchanged, &marker)?;
    unchanged.commit()?;
    let unchanged_count: i64 = conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
    assert_eq!(
        unchanged_count, before,
        "an unchanged snapshot must perform no SQL mutations"
    );
    marker.failed_sources.get_mut("failed.jpg").unwrap().reason =
        "new explicit failure detail".into();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    write_snapshot(&tx, &marker)?;
    tx.commit()?;
    let after: i64 = conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
    assert_eq!(
        after - before,
        2,
        "an outcome change must not rewrite unrelated outcomes, receipts or requests"
    );
    assert_eq!(read_marker(&marker.working_copy)?, marker);
    Ok(())
}

#[test]
fn a_persisted_task_reserves_its_output_even_after_local_directory_cleanup() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let marker = complete_marker(root.path())?;
    write_marker(&marker)?;
    assert!(!marker.working_copy.exists());
    assert!(is_reserved(&marker.working_copy)?);
    assert_eq!(
        resolve_fresh_working_copy_dir(&marker.src_dir)?,
        marker.working_copy.with_file_name("source_optimized_2")
    );
    assert_eq!(read_marker(&marker.working_copy)?, marker);
    Ok(())
}

#[test]
fn historical_unknown_submission_coverage_stays_unknown_and_intents_are_append_only() -> Result<()>
{
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let full = complete_marker(root.path())?;
    let mut value = serde_json::to_value(&full)?;
    value
        .as_object_mut()
        .unwrap()
        .remove("photos_submission_history");
    let mut legacy: WorkingCopyMarker = serde_json::from_value(value)?;
    write_marker(&legacy)?;
    assert!(
        read_marker(&legacy.working_copy)?
            .photos_submission_history()
            .is_none()
    );
    let mut invented = serde_json::to_value(&legacy)?;
    invented["photos_submission_history"] =
        serde_json::json!({"schema":1,"complete_from_creation":true,"intents":[]});
    let invented: WorkingCopyMarker = serde_json::from_value(invented)?;
    assert!(
        write_marker(&invented).is_err(),
        "a writer must not invent complete historical coverage"
    );
    let first = full.photos_submission_history().unwrap().intents[0].clone();
    legacy
        .record_photos_submissions(&[first])
        .map_err(anyhow::Error::msg)?;
    write_marker(&legacy)?;
    let saved = read_marker(&legacy.working_copy)?;
    assert!(
        !saved
            .photos_submission_history()
            .unwrap()
            .complete_from_creation
    );
    assert_eq!(saved.photos_submission_history().unwrap().intents.len(), 1);
    let second = full.photos_submission_history().unwrap().intents[1].clone();
    legacy
        .record_photos_submissions(&[second])
        .map_err(anyhow::Error::msg)?;
    write_marker(&legacy)?;
    let mut truncated = serde_json::to_value(&legacy)?;
    truncated["photos_submission_history"]["intents"] = serde_json::json!([]);
    let truncated: WorkingCopyMarker = serde_json::from_value(truncated)?;
    assert!(
        write_marker(&truncated).is_err(),
        "submission references must survive later snapshots"
    );
    assert_eq!(read_marker(&legacy.working_copy)?, legacy);
    Ok(())
}

#[test]
fn locked_write_and_failed_commit_preserve_the_published_snapshot() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let original = complete_marker(root.path())?;
    write_marker(&original)?;
    let mut next = original.clone();
    next.failed_sources.get_mut("failed.jpg").unwrap().reason = "new failure".into();
    let candidate_before = next.clone();
    let mut writer = open_store_connection()?;
    let held = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
    held.execute_batch("UPDATE fast_img_tasks SET updated_at=updated_at;")?;
    assert!(
        write_marker(&next).is_err(),
        "a busy writer must not replace formal state"
    );
    held.rollback()?;
    assert_eq!(read_marker(&original.working_copy)?, original);
    assert_eq!(next, candidate_before);
    let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch("PRAGMA defer_foreign_keys=ON;")?;
    write_snapshot(&tx, &next)?;
    tx.execute(
        "INSERT INTO fast_img_photos_requests(task_key,request_key,backend,applescript_request_id)
        VALUES('missing-task','synthetic-orphan','applescript','synthetic-request')",
        [],
    )?;
    assert!(
        tx.commit().is_err(),
        "the synthetic deferred violation must fail COMMIT"
    );
    assert!(
        writer.is_autocommit(),
        "failed COMMIT must roll back its snapshot transaction"
    );
    assert_eq!(read_marker(&original.working_copy)?, original);
    assert_eq!(next, candidate_before);
    Ok(())
}

#[test]
fn corrupt_headers_collections_hashes_and_future_store_versions_are_never_absence() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let marker = complete_marker(root.path())?;
    write_marker(&marker)?;
    let conn = open_store_connection()?;
    let (header, header_hash, snapshot): (Vec<u8>, Vec<u8>, Vec<u8>) = conn.query_row(
        "SELECT header_json,header_blake3,snapshot_blake3 FROM fast_img_tasks",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    for corruption in [
        "header",
        "header-hash",
        "snapshot-hash",
        "collection",
        "locator-binding",
        "future-schema",
    ] {
        match corruption {
            "header" => {
                let bytes = b"{";
                conn.execute(
                    "UPDATE fast_img_tasks SET header_json=?1,header_blake3=?2",
                    params![bytes.as_slice(), blake3::hash(bytes).as_bytes().as_slice()],
                )?;
            }
            "header-hash" => {
                conn.execute("UPDATE fast_img_tasks SET header_blake3=zeroblob(32)", [])?;
            }
            "snapshot-hash" => {
                conn.execute("UPDATE fast_img_tasks SET snapshot_blake3=zeroblob(32)", [])?;
            }
            "collection" => {
                conn.execute("UPDATE fast_img_source_outcomes SET reason='tampered' WHERE disposition='failed'", [])?;
            }
            "locator-binding" => {
                let mut forged = marker.clone();
                forged.working_copy = marker.working_copy.with_file_name("different-output");
                let bytes = small_header(&forged)?;
                let wc = canonical_text(&marker.working_copy)?;
                let source = canonical_text(&marker.src_dir)?;
                let digest = snapshot_digest(&wc, &source, &forged)?;
                conn.execute(
                    "UPDATE fast_img_tasks SET header_json=?1,header_blake3=?2,snapshot_blake3=?3",
                    params![
                        bytes,
                        blake3::hash(&bytes).as_bytes().as_slice(),
                        digest.as_bytes().as_slice()
                    ],
                )?;
            }
            _ => {
                conn.execute(
                    "UPDATE store_metadata SET value=99 WHERE key='schema_version'",
                    [],
                )?;
            }
        }
        let error = read_marker(&marker.working_copy).expect_err(corruption);
        assert_ne!(
            error.kind(),
            io::ErrorKind::NotFound,
            "{corruption}: {error}"
        );
        conn.execute(
            "UPDATE fast_img_tasks SET header_json=?1,header_blake3=?2,snapshot_blake3=?3",
            params![header, header_hash, snapshot],
        )?;
        conn.execute(
            "UPDATE fast_img_source_outcomes SET reason=?1 WHERE disposition='failed'",
            [&marker.failed_sources["failed.jpg"].reason],
        )?;
        conn.execute(
            "UPDATE store_metadata SET value=?1 WHERE key='schema_version'",
            params![crate::db::mfb_sqlite_store::STORE_SCHEMA_VERSION],
        )?;
        assert_eq!(read_marker(&marker.working_copy)?, marker);
    }
    Ok(())
}

#[test]
fn readonly_absence_does_not_create_or_initialize_storage() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let state = root.path().join("state-not-created");
    let _state = state_guard(&state);
    let source = root.path().join("source");
    fs::create_dir(&source)?;
    let output = root.path().join("source_optimized");
    let database = database_path()?;
    assert_eq!(
        read_marker(&output).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(matching_markers(&source, "jxl")?, []);
    assert!(!is_reserved(&output)?);
    assert_eq!(resolve_fresh_working_copy_dir(&source)?, output);
    assert!(!database.exists());
    assert!(
        !state.exists(),
        "read-only discovery must not create its state root or SQLite access files"
    );
    Ok(())
}

#[test]
fn lost_bound_storage_is_not_reported_as_an_absent_task() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let state = root.path().join("formal-state");
    let media = root.path().join("synthetic-media");
    fs::create_dir(&media)?;
    let _state = state_guard(&state);
    let marker = complete_marker(&media)?;
    write_marker(&marker)?;
    fs::rename(&state, root.path().join("retained-original-state"))?;
    let error = read_marker(&marker.working_copy).unwrap_err();
    assert_ne!(
        error.kind(),
        io::ErrorKind::NotFound,
        "storage failure must not authorize the caller's no-saved-task path: {error}"
    );
    assert!(matching_markers(&marker.src_dir, "jxl").is_err());
    assert!(is_reserved(&marker.working_copy).is_err());
    assert!(write_marker(&marker).is_err());
    assert!(!state.exists(), "lost state must not be recreated");
    Ok(())
}

#[test]
fn orphaned_task_children_are_not_absence_or_a_reusable_output() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let marker = complete_marker(root.path())?;
    write_marker(&marker)?;
    let damaged = Connection::open(database_path()?)?;
    damaged.execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM fast_img_tasks;")?;
    let preserved = raw_family_rows(&damaged)?;
    assert_eq!(table_count(&damaged, "fast_img_tasks")?, 0);
    assert_eq!(table_count(&damaged, "fast_img_source_outcomes")?, 3);
    let error = read_marker(&marker.working_copy).unwrap_err();
    assert_ne!(error.kind(), io::ErrorKind::NotFound);
    assert!(is_reserved(&marker.working_copy).is_err());
    assert!(matching_markers(&marker.src_dir, "jxl").is_err());
    assert!(
        write_marker(&marker).is_err(),
        "missing task authority must not be silently recreated over orphaned evidence"
    );
    assert!(inspect_integrity(&damaged).is_err());
    assert_eq!(
        raw_family_rows(&damaged)?,
        preserved,
        "all damaged-but-retained evidence must survive failed operations"
    );
    Ok(())
}

#[test]
fn receipt_reorder_removal_and_insertion_preserve_exact_other_collections() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let mut marker = complete_marker(root.path())?;
    for name in ["second", "third"] {
        marker.tier2_imported_assets.push(LibraryAssetRecord {
            rel_path: format!("{name}.webp"),
            blake3: digest(name.as_bytes()),
            sync_status: "photos_local".into(),
            quarantined: false,
            photos_uuid: Some(format!("synthetic-{name}-uuid")),
            library_blake3: None,
            xmp_sidecar_blake3: None,
        });
    }
    write_marker(&marker)?;
    let original = marker.clone();
    marker.tier2_imported_assets.reverse();
    write_marker(&marker)?;
    assert_eq!(
        read_marker(&marker.working_copy)?,
        marker,
        "ordinal swaps must remain unique and ordered"
    );
    marker.tier2_imported_assets.remove(1);
    write_marker(&marker)?;
    assert_eq!(
        read_marker(&marker.working_copy)?,
        marker,
        "removal must compact receipt ordinals without stale rows"
    );
    marker.tier2_imported_assets.insert(
        0,
        LibraryAssetRecord {
            rel_path: "new-original.webp".into(),
            blake3: digest(b"new original"),
            sync_status: "photos_local".into(),
            quarantined: false,
            photos_uuid: Some("synthetic-new-original-uuid".into()),
            library_blake3: Some(digest(b"new delivered resource")),
            xmp_sidecar_blake3: Some(digest(b"new original XMP")),
        },
    );
    write_marker(&marker)?;
    let restored = read_marker(&marker.working_copy)?;
    assert_eq!(
        restored, marker,
        "inserting ahead of existing receipts must not overwrite their authority"
    );
    assert_eq!(restored.blake3_log, original.blake3_log);
    assert_eq!(restored.skipped_sources, original.skipped_sources);
    assert_eq!(restored.failed_sources, original.failed_sources);
    assert_eq!(
        restored.photos_imported_assets,
        original.photos_imported_assets
    );
    assert_eq!(
        restored.photos_submission_history(),
        original.photos_submission_history()
    );
    let conn = open_store_connection()?;
    assert_eq!(table_count(&conn, "fast_img_photos_receipts")?, 4);
    assert_eq!(inspect_integrity(&conn)?, inspect_row_count(&conn)?);
    Ok(())
}

#[test]
fn readers_see_only_whole_snapshots_during_an_uncommitted_writer_update() -> Result<()> {
    if crate::common_utils::isolated_test_process() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let _state = state_guard(root.path());
    let original = complete_marker(root.path())?;
    write_marker(&original)?;
    let mut next = original.clone();
    next.failed_sources.get_mut("failed.jpg").unwrap().reason =
        "new committed failure reason".into();
    next.error = Some("new committed task summary".into());
    let database = database_path()?;
    let (ready_send, ready_recv) = std::sync::mpsc::channel();
    let (commit_send, commit_recv) = std::sync::mpsc::channel();
    let pending = next.clone();
    let worker = std::thread::spawn(move || -> Result<()> {
        let conn = Connection::open(database)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
        write_snapshot(&conn, &pending)?;
        ready_send.send(())?;
        commit_recv.recv()?;
        conn.execute_batch("COMMIT;")?;
        Ok(())
    });
    ready_recv.recv()?;
    let observed = read_marker(&original.working_copy);
    commit_send.send(())?;
    worker.join().expect("synthetic writer thread panicked")?;
    assert_eq!(
        observed?, original,
        "a reader must not combine old task headers and new collection rows"
    );
    assert_eq!(read_marker(&next.working_copy)?, next);
    Ok(())
}
