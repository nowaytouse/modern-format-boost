//! Canonical, row-oriented Fast IMG state. Media custody is verified separately.
//! Legacy JSON is accepted only by the named migration operation.

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use super::mfb_sqlite_store::{
    FAST_IMG_PHOTOS_UNCERTAINTY_SCHEMA_VERSION, inspect_store_schema_version,
    open_store_connection, open_store_readonly, store_path_current_for_read,
};
use crate::pipeline::verification::{
    Blake3Entry, LibraryAssetRecord, PhotosSubmissionBackend, PhotosSubmissionIntent,
    PhotosSubmissionKind, SkippedSourceEntry, WorkingCopyMarker, canonical_working_copy_path,
    validate_marker_structure,
};
pub use crate::pipeline::verification::{
    FailedRetryFinishEvidence, FailedRetrySourceRootObservation,
};

mod admission;
mod cleanup;
mod finish;
mod jpeg_execution;
mod migration;
mod primary_result;
mod retry;
mod retry_admission;
mod source_evidence;
mod tier2_cleanup;
pub use crate::db::mfb_sqlite_store::FAST_IMG_PRIMARY_RESULT_SCHEMA_VERSION;
pub use crate::mfb_sqlite_store::{
    FAST_IMG_ADMISSION_AAE_SCHEMA_VERSION, FAST_IMG_ADMISSION_SCHEMA_VERSION,
    FAST_IMG_ADMISSION_XMP_SCHEMA_VERSION,
};
pub use admission::{
    FastImgAdmissionPlan, FastImgAdmissionRoute, FastImgAdmissionSidecar, FastImgSourceAdmission,
    initialize_with_admission, read_admission_plan,
};
pub use cleanup::{PrimaryCleanupJournal, PrimaryCleanupScope, validate_primary_cleanup_resume};
pub use finish::{
    FailedRetryFinishSnapshot, finalize_failed_retry_finish, inspect_failed_retry_finish,
    resolve_failed_retry_child_for_source,
};
pub use jpeg_execution::{
    AVIF_EXECUTION_SCHEMA_VERSION, AvifEncoderInvocation, AvifExecutionReceipt,
    AvifExecutionReceiptPage, EXECUTION_TABLES, EncoderExecutionReceiptPage,
    JPEG_EXECUTION_SCHEMA_VERSION, JpegEncoderInvocation, JpegExecutionAttempt,
    JpegExecutionReceipt, JpegExecutionReceiptPage, ReconstructedJpegExecution,
    read_avif_execution_receipt_page, read_jpeg_execution_receipt_page,
    read_jpeg_execution_receipts_for_source, write_marker_with_avif_execution,
    write_marker_with_jpeg_execution, write_marker_with_reconstructed_jpeg_execution,
};
pub use primary_result::{
    PrimaryResultEvidence, PrimaryResultObservation, write_marker_with_primary_result,
};
pub use retry::{
    FailedRetryAdmission, FailedRetryContext, FailedRetryDeliveryIntent, FailedRetryParent,
    FailedRetrySidecar, FailedRetrySourceRoute, FailedRetryToolAuthority,
    create_or_resume_failed_retry, failed_retry_context, find_failed_retry_child,
    inspect_failed_retry_parent, inspect_failed_retry_parent_for_delivery,
};
pub use retry_admission::{
    FailedRetryAdmissionBlocked, capture_failed_retry_admissions, verify_failed_retry_admission,
    verify_failed_retry_retained_companions,
};
pub use source_evidence::{SourceEvidence, read_source_evidence};
pub use tier2_cleanup::{
    Tier2CleanupJournal, validate_tier2_cleanup_resume, verify_tier2_cleanup_complete,
};
#[cfg(test)]
mod tests;

const TABLES: [&str; 6] = [
    "fast_img_tasks",
    "fast_img_source_outcomes",
    "fast_img_photos_requests",
    "fast_img_photos_entries",
    "fast_img_photos_receipts",
    "fast_img_migrations",
];
const MAX_LEGACY_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Default, Serialize)]
pub struct MigrationReport {
    pub migrated: usize,
    pub retired: usize,
}

#[derive(Debug)]
struct StateMigrationRequired(PathBuf);

impl std::fmt::Display for StateMigrationRequired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "MFB_STATE_MIGRATION_REQUIRED: legacy checkpoint {} requires the explicit Fast IMG state migration; no JSON was used as runtime proof",
            self.0.display()
        )
    }
}

impl std::error::Error for StateMigrationRequired {}

pub fn is_state_migration_required(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|error| error.is::<StateMigrationRequired>())
}

fn as_io(error: anyhow::Error) -> io::Error {
    let error = match error.downcast::<StateMigrationRequired>() {
        Ok(required) => return io::Error::other(required),
        Err(error) => error,
    };
    if error
        .downcast_ref::<io::Error>()
        .is_some_and(crate::pipeline::verification::is_photos_working_copy_unavailable)
    {
        return error
            .downcast::<io::Error>()
            .unwrap_or_else(io::Error::other);
    }
    let kind = error
        .downcast_ref::<io::Error>()
        .map_or(io::ErrorKind::Other, io::Error::kind);
    // Only read_marker's explicit, successfully established absence may use
    // NotFound. A lost bound root/database is unavailable state, not no task.
    let kind = if kind == io::ErrorKind::NotFound {
        io::ErrorKind::Other
    } else {
        kind
    };
    io::Error::new(kind, error)
}

fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .context("Fast IMG state path is not UTF-8; no lossy identity was assigned")
}

fn canonical_text(path: &Path) -> Result<String> {
    Ok(text(&canonical_working_copy_path(path)?)?.to_owned())
}

fn key_for_canonical(path: &str) -> String {
    blake3::hash(path.as_bytes()).to_hex().to_string()
}

fn validate_locator(path: &str) -> Result<()> {
    let path = Path::new(path);
    let normalized: PathBuf = path.components().collect();
    ensure!(
        path.is_absolute()
            && path.file_name().is_some()
            && normalized.as_os_str() == path.as_os_str()
            && !path.components().any(|part| matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            ))
            && !path.as_os_str().as_encoded_bytes().contains(&0),
        "Fast IMG state locator must be an absolute normalized non-root path"
    );
    Ok(())
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

fn snapshot_digest(wc: &str, source: &str, marker: &WorkingCopyMarker) -> Result<blake3::Hash> {
    Ok(blake3::hash(&canonical_bytes(&(wc, source, marker))?))
}

fn small_header(marker: &WorkingCopyMarker) -> Result<Vec<u8>> {
    let mut value = serde_json::to_value(marker)?;
    if let Some(fields) = value.as_object_mut() {
        fields.remove("tier2_failed_sources");
    }
    for name in ["blake3_log", "skipped_sources", "failed_sources"] {
        value[name] = serde_json::json!({});
    }
    for name in ["photos_imported_assets", "tier2_imported_assets"] {
        value[name] = serde_json::json!([]);
    }
    if let Some(history) = value
        .get_mut("photos_submission_history")
        .filter(|v| !v.is_null())
    {
        history["intents"] = serde_json::json!([]);
    }
    canonical_bytes(&value)
}

fn open_readonly() -> Result<Option<Connection>> {
    open_store_readonly()
}

fn current_family(conn: &Connection) -> Result<bool> {
    match inspect_store_schema_version(conn)? {
        Some(3..=crate::mfb_sqlite_store::STORE_SCHEMA_VERSION) => Ok(true),
        Some(1 | 2) => Ok(false),
        None => {
            bail!("Existing formal database has no recognized schema; not treated as empty state")
        }
        Some(version) => bail!("Unsupported formal state schema {version}"),
    }
}

pub fn database_path() -> io::Result<PathBuf> {
    store_path_current_for_read().map_err(as_io)
}

#[derive(Debug)]
struct TaskHeader {
    wc: String,
    source: String,
    bytes: Vec<u8>,
    header_hash: Vec<u8>,
    snapshot_hash: Vec<u8>,
    revision: i64,
}

fn read_snapshot_base(conn: &Connection, key: &str) -> Result<Option<WorkingCopyMarker>> {
    let header = conn
        .query_row(
            "SELECT working_copy,source_root,header_json,header_blake3,snapshot_blake3,revision
         FROM fast_img_tasks WHERE task_key=?1",
            [key],
            |row| {
                Ok(TaskHeader {
                    wc: row.get(0)?,
                    source: row.get(1)?,
                    bytes: row.get(2)?,
                    header_hash: row.get(3)?,
                    snapshot_hash: row.get(4)?,
                    revision: row.get(5)?,
                })
            },
        )
        .optional()?;
    let Some(header) = header else {
        for table in TABLES.into_iter().skip(1) {
            let orphan: bool = conn.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE task_key=?1)"),
                [key],
                |row| row.get(0),
            )?;
            ensure!(
                !orphan,
                "Fast IMG task header is missing but protected child evidence remains; state is not absent"
            );
        }
        retry::reject_missing_task_evidence(conn, key)?;
        cleanup::reject_missing_task_evidence(conn, key)?;
        tier2_cleanup::reject_missing_task_evidence(conn, key)?;
        let version = inspect_store_schema_version(conn)?;
        if version.is_some_and(|version| {
            version >= crate::mfb_sqlite_store::FAST_IMG_ADMISSION_SCHEMA_VERSION
        }) {
            admission::reject_orphans(conn, key)?;
        }
        if version.is_some_and(|version| version >= 8) {
            let has_failures: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM fast_img_tier2_failures WHERE task_key=?1)",
                [key],
                |row| row.get(0),
            )?;
            ensure!(
                !has_failures,
                "Missing Fast IMG task still has protected original-format failures"
            );
        }
        if version.is_some_and(|version| version >= FAST_IMG_PHOTOS_UNCERTAINTY_SCHEMA_VERSION) {
            let has_uncertainty: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM fast_img_photos_uncertain WHERE task_key=?1)",
                [key],
                |row| row.get(0),
            )?;
            ensure!(
                !has_uncertainty,
                "Missing Fast IMG task still has protected Photos uncertainty evidence"
            );
        }
        for (table, required_since) in EXECUTION_TABLES {
            if version.is_none_or(|version| version < required_since) {
                continue;
            }
            let has_execution: bool = conn.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE task_key=?1)"),
                [key],
                |row| row.get(0),
            )?;
            ensure!(
                !has_execution,
                "Missing Fast IMG task still has protected encoder execution evidence"
            );
        }
        if version.is_some_and(|version| version >= FAST_IMG_PRIMARY_RESULT_SCHEMA_VERSION) {
            ensure!(
                !primary_result::has_rows(conn, key, None)?,
                "Missing Fast IMG task still has protected primary result evidence"
            );
        }
        return Ok(None);
    };
    validate_locator(&header.wc)?;
    validate_locator(&header.source)?;
    ensure!(
        header.revision >= 0 && key_for_canonical(&header.wc) == key,
        "Fast IMG task locator/revision is invalid; state retained"
    );
    ensure!(
        blake3::hash(&header.bytes).as_bytes().as_slice() == header.header_hash,
        "Fast IMG task header failed BLAKE3 verification; state retained"
    );
    // Parse the typed header before editing its collections: duplicate or future
    // members must not disappear through a generic JSON object conversion.
    let parsed: WorkingCopyMarker = serde_json::from_slice(&header.bytes)?;
    validate_marker_structure(&parsed)?;
    ensure!(
        parsed.working_copy == Path::new(&header.wc) && parsed.src_dir == Path::new(&header.source),
        "Fast IMG header paths disagree with their authoritative SQL locators"
    );
    ensure!(
        parsed.blake3_log.is_empty()
            && parsed.skipped_sources.is_empty()
            && parsed.failed_sources.is_empty()
            && parsed.tier2_failed_sources.is_empty()
            && parsed.photos_imported_assets.is_empty()
            && parsed.tier2_imported_assets.is_empty()
            && parsed
                .photos_submission_history()
                .is_none_or(|h| h.intents.is_empty()),
        "Fast IMG header contains a second collection authority"
    );
    let mut value = serde_json::to_value(parsed)?;
    let mut converted = BTreeMap::new();
    let mut skipped = BTreeMap::new();
    let mut failed = BTreeMap::new();
    let mut statement = conn.prepare(
        "SELECT source_rel,disposition,source_blake3,output_rel,output_blake3,library_blake3,reason
         FROM fast_img_source_outcomes WHERE task_key=?1 ORDER BY source_rel",
    )?;
    let mut rows = statement.query([key])?;
    while let Some(row) = rows.next()? {
        let rel: String = row.get(0)?;
        let disposition: String = row.get(1)?;
        let source: String = row.get(2)?;
        let out_rel: Option<String> = row.get(3)?;
        let out: Option<String> = row.get(4)?;
        let library: Option<String> = row.get(5)?;
        let reason: Option<String> = row.get(6)?;
        match disposition.as_str() {
            "converted" => {
                ensure!(
                    reason.is_none(),
                    "Converted Fast IMG row has an unexplained reason field"
                );
                converted.insert(
                    rel,
                    Blake3Entry {
                        src: source,
                        out_rel,
                        out: out.context("Converted Fast IMG row has no output identity")?,
                        library_asset: library,
                    },
                );
            }
            "skipped" | "failed" => {
                ensure!(
                    out_rel.is_none() && out.is_none() && library.is_none(),
                    "Non-converted Fast IMG row carries output proof"
                );
                let entry = SkippedSourceEntry {
                    src: source,
                    reason: reason.context("Fast IMG outcome has no explicit reason")?,
                };
                if disposition == "skipped" {
                    skipped.insert(rel, entry);
                } else {
                    failed.insert(rel, entry);
                }
            }
            _ => bail!("Unknown Fast IMG source disposition {disposition}"),
        }
    }
    value["blake3_log"] = serde_json::to_value(converted)?;
    value["skipped_sources"] = serde_json::to_value(skipped)?;
    value["failed_sources"] = serde_json::to_value(failed)?;
    let version = inspect_store_schema_version(conn)?;
    if version.is_some_and(|version| version >= 8) {
        let mut statement = conn.prepare(
            "SELECT source_rel,source_blake3,reason FROM fast_img_tier2_failures WHERE task_key=?1 ORDER BY source_rel",
        )?;
        let failures = statement
            .query_map([key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    SkippedSourceEntry {
                        src: row.get(1)?,
                        reason: row.get(2)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        if !failures.is_empty() {
            value["tier2_failed_sources"] = serde_json::to_value(failures)?;
        }
    }
    let mut requests = BTreeMap::new();
    let mut statement = conn.prepare(
        "SELECT request_key,backend,native_journal_path,
        native_request_blake3,applescript_request_id,library_path
        FROM fast_img_photos_requests WHERE task_key=?1",
    )?;
    let mut rows = statement.query([key])?;
    while let Some(row) = rows.next()? {
        let request_key: String = row.get(0)?;
        let backend: String = row.get(1)?;
        let journal: Option<String> = row.get(2)?;
        let hash: Option<String> = row.get(3)?;
        let script_id: Option<String> = row.get(4)?;
        let library: Option<String> = row.get(5)?;
        let backend = match backend.as_str() {
            "native" => {
                ensure!(
                    script_id.is_none(),
                    "Native request contains an AppleScript identity"
                );
                PhotosSubmissionBackend::Native {
                    journal_path: PathBuf::from(journal.context("Native request has no journal")?),
                    request_blake3: hash.context("Native request has no exact request hash")?,
                }
            }
            "applescript" => {
                ensure!(
                    journal.is_none() && hash.is_none(),
                    "AppleScript request contains native proof"
                );
                PhotosSubmissionBackend::Applescript {
                    request_id: script_id.context("Missing request ID")?,
                }
            }
            _ => bail!("Unknown Fast IMG request backend {backend}"),
        };
        ensure!(
            request_key == request_identity(&backend)?,
            "Fast IMG request key binding changed"
        );
        requests.insert(request_key, (backend, library.map(PathBuf::from)));
    }
    let mut intents = Vec::new();
    let mut used_requests = BTreeSet::new();
    let mut statement = conn.prepare(
        "SELECT ordinal,request_key,kind,source_rel,source_blake3,
        output_rel,output_blake3,album_name FROM fast_img_photos_entries
        WHERE task_key=?1 ORDER BY ordinal",
    )?;
    let mut rows = statement.query([key])?;
    while let Some(row) = rows.next()? {
        ensure!(
            row.get::<_, i64>(0)? == i64::try_from(intents.len())?,
            "Fast IMG history ordinal gap"
        );
        let request_key: String = row.get(1)?;
        let (backend, library) = requests
            .get(&request_key)
            .context("Submission has no request authority")?;
        let kind: String = row.get(2)?;
        let kind = match kind.as_str() {
            "encoded" => PhotosSubmissionKind::Encoded,
            "original" => PhotosSubmissionKind::Original,
            _ => bail!("Unknown submission kind {kind}"),
        };
        intents.push(PhotosSubmissionIntent {
            kind,
            source_rel: row.get(3)?,
            source_blake3: row.get(4)?,
            output_rel: row.get(5)?,
            output_blake3: row.get(6)?,
            album_name: row.get(7)?,
            backend: backend.clone(),
            photos_library_path: library.clone(),
        });
        used_requests.insert(request_key);
    }
    ensure!(
        used_requests.len() == requests.len(),
        "Fast IMG contains orphaned request rows"
    );
    if let Some(history) = value
        .get_mut("photos_submission_history")
        .filter(|v| !v.is_null())
    {
        history["intents"] = serde_json::to_value(intents)?;
    } else {
        ensure!(
            intents.is_empty(),
            "Unknown history header contains invented submission rows"
        );
    }
    for (kind, field) in [
        ("primary", "photos_imported_assets"),
        ("original", "tier2_imported_assets"),
    ] {
        let mut assets = Vec::new();
        let mut statement = conn.prepare(
            "SELECT ordinal,rel_path,blake3,sync_status,quarantined,
            photos_uuid,library_blake3,xmp_sidecar_blake3 FROM fast_img_photos_receipts
            WHERE task_key=?1 AND receipt_kind=?2 ORDER BY ordinal",
        )?;
        let mut rows = statement.query(params![key, kind])?;
        while let Some(row) = rows.next()? {
            ensure!(
                row.get::<_, i64>(0)? == i64::try_from(assets.len())?,
                "Fast IMG receipt ordinal gap"
            );
            let quarantined: i64 = row.get(4)?;
            ensure!(matches!(quarantined, 0 | 1), "Invalid quarantine state");
            assets.push(LibraryAssetRecord {
                rel_path: row.get(1)?,
                blake3: row.get(2)?,
                sync_status: row.get(3)?,
                quarantined: quarantined == 1,
                photos_uuid: row.get(5)?,
                library_blake3: row.get(6)?,
                xmp_sidecar_blake3: row.get(7)?,
            });
        }
        value[field] = serde_json::to_value(assets)?;
    }
    let marker: WorkingCopyMarker = serde_json::from_value(value)?;
    validate_marker_structure(&marker)?;
    admission::validate_row_presence(conn, key, &marker, version)?;
    cleanup::validate_rows(conn, key, &marker)?;
    tier2_cleanup::validate_rows(conn, key, &marker)?;
    ensure!(
        snapshot_digest(&header.wc, &header.source, &marker)?
            .as_bytes()
            .as_slice()
            == header.snapshot_hash,
        "Fast IMG complete state snapshot failed BLAKE3 verification; no rows were reset"
    );
    finish::validate_persisted(conn, key, &marker, header.revision)?;
    Ok(Some(marker))
}

fn read_snapshot(conn: &Connection, key: &str) -> Result<Option<WorkingCopyMarker>> {
    let marker = read_snapshot_base(conn, key)?;
    if let Some(marker) = &marker {
        retry::validate_task(conn, key, marker)?;
    }
    Ok(marker)
}

fn request_identity(backend: &PhotosSubmissionBackend) -> Result<String> {
    Ok(blake3::hash(&canonical_bytes(backend)?)
        .to_hex()
        .to_string())
}

fn photos_uncertainty_evidence(
    conn: &Connection,
    task_key: &str,
    marker: &WorkingCopyMarker,
) -> Result<Vec<PhotosSubmissionIntent>> {
    let saved = conn
        .prepare(
            "SELECT request_key,intent_blake3 FROM fast_img_photos_uncertain
             WHERE task_key=?1 ORDER BY request_key",
        )?
        .query_map([task_key], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    if saved.is_empty() {
        return Ok(Vec::new());
    }
    let history = marker
        .photos_submission_history()
        .context("Photos uncertainty evidence lacks its owning submission history")?;
    let mut observed = Vec::with_capacity(saved.len());
    for intent in &history.intents {
        if !matches!(intent.backend, PhotosSubmissionBackend::Applescript { .. }) {
            continue;
        }
        let request_key = request_identity(&intent.backend)?;
        if let Some(digest) = saved.get(&request_key) {
            ensure!(
                digest.as_slice() == blake3::hash(&canonical_bytes(intent)?).as_bytes(),
                "Photos uncertainty evidence no longer matches its exact submission intent"
            );
            observed.push(intent.clone());
        }
    }
    ensure!(
        observed.len() == saved.len(),
        "Photos uncertainty evidence does not resolve to one intent per request"
    );
    Ok(observed)
}

/// Read AppleScript requests armed before launch or previously observed
/// returning zero IDs. Group reservations alone do not arm a request.
pub(crate) fn uncertain_photos_intents(
    marker: &WorkingCopyMarker,
) -> io::Result<Vec<PhotosSubmissionIntent>> {
    if marker.photos_submission_history().is_none_or(|history| {
        !history
            .intents
            .iter()
            .any(|intent| matches!(intent.backend, PhotosSubmissionBackend::Applescript { .. }))
    }) {
        return Ok(Vec::new());
    }
    (|| {
        let conn = open_readonly()?
            .context("Fast IMG has AppleScript history but no committed task database")?;
        in_snapshot(&conn, |conn| {
            let version = inspect_store_schema_version(conn)?
                .context("Fast IMG has AppleScript history but its database is uninitialized")?;
            ensure!(
                version >= 3,
                "Fast IMG AppleScript history has no owning structured database"
            );
            if version < FAST_IMG_PHOTOS_UNCERTAINTY_SCHEMA_VERSION {
                return Ok(Vec::new());
            }
            let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM fast_img_tasks WHERE task_key=?1)",
                [&key],
                |row| row.get(0),
            )?;
            ensure!(
                exists,
                "Fast IMG AppleScript history has no committed owning task"
            );
            photos_uncertainty_evidence(conn, &key, marker)
        })
    })()
    .map_err(as_io)
}

/// Arm the one exact saved permit before launching AppleScript.
pub(crate) fn arm_photos_submission(
    marker: &WorkingCopyMarker,
    intent: &PhotosSubmissionIntent,
) -> io::Result<()> {
    (|| {
        ensure!(
            matches!(intent.backend, PhotosSubmissionBackend::Applescript { .. }),
            "Only an AppleScript request can be armed here"
        );
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let wc = canonical_text(&marker.working_copy)?;
        let key = key_for_canonical(&wc);
        let request_key = request_identity(&intent.backend)?;
        let source = canonical_text(&marker.src_dir)?;
        let PhotosSubmissionBackend::Applescript { request_id } = &intent.backend else {
            unreachable!("AppleScript request required above");
        };
        let route = match intent.kind {
            PhotosSubmissionKind::Encoded => "encoded",
            PhotosSubmissionKind::Original => "original",
        };
        let library = intent.photos_library_path.as_deref().map(text).transpose()?;
        // Permit consumption already checked the in-memory owner/history. Look up
        // just this exact normalized request at the send boundary, independent
        // of how many other items the batch reserved or verified.
        let exact_rows: i64 = tx.query_row(
            "SELECT count(*) FROM fast_img_tasks t
             JOIN fast_img_photos_requests r ON r.task_key=t.task_key
             JOIN fast_img_photos_entries e ON e.task_key=r.task_key AND e.request_key=r.request_key
             WHERE t.task_key=?1 AND t.working_copy=?2 AND t.source_root=?3
               AND r.request_key=?4 AND r.backend='applescript' AND r.applescript_request_id=?5
               AND r.library_path IS ?6 AND e.kind=?7 AND e.source_rel=?8 AND e.source_blake3=?9
               AND e.output_rel=?10 AND e.output_blake3=?11 AND e.album_name=?12",
            params![key, wc, source, request_key, request_id, library, route, intent.source_rel,
                intent.source_blake3, intent.output_rel, intent.output_blake3, intent.album_name],
            |row| row.get(0),
        )?;
        ensure!(exact_rows == 1, "Photos dispatch does not match one exact committed request");
        let digest = blake3::hash(&canonical_bytes(intent)?);
        let inserted = tx.execute(
            "INSERT INTO fast_img_photos_uncertain(task_key,request_key,intent_blake3)
             VALUES(?1,?2,?3) ON CONFLICT(task_key,request_key) DO NOTHING",
            params![key, request_key, digest.as_bytes().as_slice()],
        )?;
        ensure!(inserted == 1, "Photos request was already armed and cannot be sent again");
        let saved: Vec<u8> = tx.query_row(
            "SELECT intent_blake3 FROM fast_img_photos_uncertain WHERE task_key=?1 AND request_key=?2",
            params![key, request_key],
            |row| row.get(0),
        )?;
        ensure!(
            saved.as_slice() == digest.as_bytes(),
            "Photos dispatch did not survive exact database readback"
        );
        tx.commit()?;
        Ok(())
    })().map_err(as_io)
}

/// Release an armed permit after a typed result establishes that AppleScript
/// could not submit any media. The original reservation remains in history.
pub(crate) fn release_unsent_photos_submission(
    marker: &WorkingCopyMarker,
    intent: &PhotosSubmissionIntent,
) -> io::Result<()> {
    (|| {
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
        let request = request_identity(&intent.backend)?;
        let digest = blake3::hash(&canonical_bytes(intent)?);
        let removed = tx.execute(
            "DELETE FROM fast_img_photos_uncertain
             WHERE task_key=?1 AND request_key=?2 AND intent_blake3=?3",
            params![key, request, digest.as_bytes().as_slice()],
        )?;
        ensure!(
            removed == 1,
            "Photos unsent proof does not match exactly one armed request"
        );
        tx.commit()?;
        Ok(())
    })()
    .map_err(as_io)
}

fn in_snapshot<T>(conn: &Connection, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    if !conn.is_autocommit() {
        return read(conn);
    }
    let snapshot = conn.unchecked_transaction()?;
    let result = read(&snapshot)?;
    snapshot.commit()?;
    Ok(result)
}

fn checked_marker(conn: &Connection, wc: &str) -> Result<Option<WorkingCopyMarker>> {
    let marker = read_snapshot(conn, &key_for_canonical(wc))?;
    if let Some(marker) = &marker {
        ensure!(
            canonical_text(&marker.working_copy)? == wc,
            "Fast IMG task output identity no longer matches its database locator"
        );
        let source: String = conn.query_row(
            "SELECT source_root FROM fast_img_tasks WHERE task_key=?1",
            [key_for_canonical(wc)],
            |row| row.get(0),
        )?;
        ensure!(
            canonical_text(&marker.src_dir)? == source,
            "Fast IMG source root no longer matches its saved identity"
        );
        marker
            .validate_checkpoint_path_contract(Path::new(wc))
            .map_err(anyhow::Error::msg)?;
    }
    Ok(marker)
}

/// Read one committed task. Missing state is distinct from unreadable or old state.
///
/// # Errors
/// Rejects unmigrated JSON, unsupported/corrupt SQL, or changed path ownership.
pub fn read_marker(working_copy: &Path) -> io::Result<WorkingCopyMarker> {
    let marker = (|| -> Result<Option<WorkingCopyMarker>> {
        migration::reject_unmigrated_for_working_copy(working_copy)?;
        let wc = canonical_text(working_copy)?;
        let marker = if let Some(conn) = open_readonly()? {
            in_snapshot(&conn, |conn| {
                if current_family(conn)? {
                    checked_marker(conn, &wc)
                } else {
                    Ok(None)
                }
            })?
        } else {
            None
        };
        Ok(marker)
    })()
    .map_err(as_io)?;
    marker.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "No committed Fast IMG task exists for this output",
        )
    })
}

/// Discover committed tasks by exact source identity and strategy, without media scanning.
///
/// # Errors
/// Propagates state and path errors; never substitutes an empty list for broken storage.
pub fn matching_markers(source: &Path, strategy: &str) -> io::Result<Vec<WorkingCopyMarker>> {
    (|| -> Result<Vec<WorkingCopyMarker>> {
        migration::reject_unmigrated_for_source(source)?;
        let source = canonical_text(source)?;
        let Some(conn) = open_readonly()? else {
            return Ok(Vec::new());
        };
        in_snapshot(&conn, |conn| {
            if !current_family(conn)? {
                return Ok(Vec::new());
            }
            matching_markers_in_snapshot(conn, &source, strategy, checked_marker)
        })
    })()
    .map_err(as_io)
}

fn matching_markers_in_snapshot(
    conn: &Connection,
    source: &str,
    strategy: &str,
    mut inspect: impl FnMut(&Connection, &str) -> Result<Option<WorkingCopyMarker>>,
) -> Result<Vec<WorkingCopyMarker>> {
    ensure_no_orphans(conn)?;
    let paths = conn
        .prepare(
            "SELECT working_copy FROM fast_img_tasks WHERE source_root=?1 ORDER BY working_copy",
        )?
        .query_map([source], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut found = Vec::new();
    for path in paths {
        let marker = inspect(conn, &path)?
            .context("Indexed Fast IMG task disappeared in a read snapshot")?;
        if marker.strategy == strategy {
            found.push(marker);
        }
    }
    Ok(found)
}

/// A missing output directory does not release its durable task reservation.
///
/// # Errors
/// Rejects legacy/unknown state rather than allowing a new task to overwrite it.
pub fn is_reserved(working_copy: &Path) -> io::Result<bool> {
    (|| -> Result<bool> {
        migration::reject_unmigrated_for_working_copy(working_copy)?;
        let wc = canonical_text(working_copy)?;
        let Some(conn) = open_readonly()? else {
            return Ok(false);
        };
        in_snapshot(&conn, |conn| {
            if !current_family(conn)? {
                return Ok(false);
            }
            // Read and verify the complete snapshot, even if only reservation is requested.
            Ok(checked_marker(conn, &wc)?.is_some())
        })
    })()
    .map_err(as_io)
}

/// Prove an exact historical reservation without following an occupied output leaf.
/// This is for reporting only and does not validate the live directory for resume.
pub(crate) fn has_saved_reservation(working_copy: &Path) -> io::Result<bool> {
    (|| -> Result<bool> {
        let parent = working_copy
            .parent()
            .context("Fast IMG output has no parent")?;
        let leaf = working_copy
            .file_name()
            .context("Fast IMG output has no file name")?;
        let locator = parent.canonicalize()?.join(leaf);
        let locator = text(&locator)?;
        validate_locator(locator)?;
        let Some(conn) = open_readonly()? else {
            return Ok(false);
        };
        in_snapshot(&conn, |conn| {
            if !current_family(conn)? {
                return Ok(false);
            }
            Ok(read_snapshot(conn, &key_for_canonical(locator))?.is_some())
        })
    })()
    .map_err(as_io)
}

/// Write a complete logical snapshot as one transaction, updating only changed rows.
///
/// # Errors
/// Rejects changed task identity, dropped submission evidence and all storage errors.
pub fn write_marker(marker: &WorkingCopyMarker) -> io::Result<()> {
    (|| -> Result<()> {
        migration::reject_unmigrated_for_working_copy(&marker.working_copy)?;
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_snapshot(&tx, marker)?;
        tx.commit()
            .context("Cannot commit Fast IMG state; prior committed task retained")
    })()
    .map_err(as_io)
}

#[derive(Debug, PartialEq, Eq)]
struct SourceOutcome<'a> {
    disposition: &'static str,
    source: &'a str,
    output_rel: Option<&'a str>,
    output: Option<&'a str>,
    library: Option<&'a str>,
    reason: Option<&'a str>,
}

fn source_outcome<'a>(marker: &'a WorkingCopyMarker, rel: &str) -> Option<SourceOutcome<'a>> {
    if let Some(entry) = marker.blake3_log.get(rel) {
        return Some(SourceOutcome {
            disposition: "converted",
            source: &entry.src,
            output_rel: entry.out_rel.as_deref(),
            output: Some(&entry.out),
            library: entry.library_asset.as_deref(),
            reason: None,
        });
    }
    for (disposition, entries) in [
        ("skipped", &marker.skipped_sources),
        ("failed", &marker.failed_sources),
    ] {
        if let Some(entry) = entries.get(rel) {
            return Some(SourceOutcome {
                disposition,
                source: &entry.src,
                output_rel: None,
                output: None,
                library: None,
                reason: Some(&entry.reason),
            });
        }
    }
    None
}

fn source_outcomes(marker: &WorkingCopyMarker) -> BTreeMap<&str, SourceOutcome<'_>> {
    let mut rows = BTreeMap::new();
    for (rel, entry) in &marker.blake3_log {
        rows.insert(
            rel.as_str(),
            SourceOutcome {
                disposition: "converted",
                source: &entry.src,
                output_rel: entry.out_rel.as_deref(),
                output: Some(&entry.out),
                library: entry.library_asset.as_deref(),
                reason: None,
            },
        );
    }
    for (disposition, entries) in [
        ("skipped", &marker.skipped_sources),
        ("failed", &marker.failed_sources),
    ] {
        for (rel, entry) in entries {
            rows.insert(
                rel.as_str(),
                SourceOutcome {
                    disposition,
                    source: &entry.src,
                    output_rel: None,
                    output: None,
                    library: None,
                    reason: Some(&entry.reason),
                },
            );
        }
    }
    rows
}

fn preserve_history(old: &WorkingCopyMarker, next: &WorkingCopyMarker) -> Result<()> {
    match (
        old.photos_submission_history(),
        next.photos_submission_history(),
    ) {
        (Some(old), Some(next)) => ensure!(
            old.schema == next.schema
                && old.complete_from_creation == next.complete_from_creation
                && next.intents.starts_with(&old.intents),
            "Saved Photos submission evidence is append-only; stale or truncated snapshot refused"
        ),
        (Some(_), None) => bail!("Saved Photos submission evidence cannot be removed"),
        (None, Some(next)) => ensure!(
            !next.complete_from_creation,
            "Historical missing submission coverage cannot be promoted to complete"
        ),
        (None, None) => {}
    }
    Ok(())
}

fn write_sources(
    conn: &Connection,
    key: &str,
    old: Option<&WorkingCopyMarker>,
    next: &WorkingCopyMarker,
    revision: i64,
    observation: Option<(&str, &PrimaryResultObservation)>,
) -> Result<()> {
    let previous = match old {
        Some(marker) => source_outcomes(marker),
        None => BTreeMap::new(),
    };
    let next = source_outcomes(next);
    if let Some((rel, observation)) = observation {
        let row = next
            .get(rel)
            .context("Primary result does not have a committed source disposition")?;
        primary_result::write(conn, key, rel, revision, Some(row), Some(observation))?;
    }
    for rel in previous.keys().filter(|rel| !next.contains_key(**rel)) {
        primary_result::forget_prior(conn, key, rel, revision, None)?;
        conn.execute(
            "DELETE FROM fast_img_source_outcomes WHERE task_key=?1 AND source_rel=?2",
            params![key, rel],
        )?;
    }
    for (rel, row) in next {
        if previous.get(rel) == Some(&row) {
            continue;
        }
        if old.is_some()
            && observation.is_none_or(|(observed, _)| observed != rel)
            && previous.get(rel).is_none_or(|old| {
                (
                    old.disposition,
                    old.source,
                    old.output_rel,
                    old.output,
                    old.reason,
                ) != (
                    row.disposition,
                    row.source,
                    row.output_rel,
                    row.output,
                    row.reason,
                )
            })
        {
            primary_result::forget_prior(conn, key, rel, revision, Some(&row))?;
        }
        conn.execute("INSERT INTO fast_img_source_outcomes
            (task_key,source_rel,disposition,source_blake3,output_rel,output_blake3,library_blake3,reason)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
            ON CONFLICT(task_key,source_rel) DO UPDATE SET disposition=excluded.disposition,
            source_blake3=excluded.source_blake3,output_rel=excluded.output_rel,
            output_blake3=excluded.output_blake3,library_blake3=excluded.library_blake3,reason=excluded.reason",
            params![key,rel,row.disposition,row.source,row.output_rel,row.output,row.library,row.reason])?;
    }
    Ok(())
}

fn append_submissions(
    conn: &Connection,
    key: &str,
    old: Option<&WorkingCopyMarker>,
    next: &WorkingCopyMarker,
) -> Result<()> {
    let old_len = match old.and_then(WorkingCopyMarker::photos_submission_history) {
        Some(history) => history.intents.len(),
        None => 0,
    };
    let Some(history) = next.photos_submission_history() else {
        return Ok(());
    };
    for (ordinal, intent) in history.intents.iter().enumerate().skip(old_len) {
        let request = request_identity(&intent.backend)?;
        let (backend, journal, hash, script_id) = match &intent.backend {
            PhotosSubmissionBackend::Native {
                journal_path,
                request_blake3,
            } => (
                "native",
                Some(text(journal_path)?),
                Some(request_blake3.as_str()),
                None,
            ),
            PhotosSubmissionBackend::Applescript { request_id } => {
                ("applescript", None, None, Some(request_id.as_str()))
            }
        };
        let library = intent
            .photos_library_path
            .as_deref()
            .map(text)
            .transpose()?;
        conn.execute(
            "INSERT INTO fast_img_photos_requests(task_key,request_key,backend,
            native_journal_path,native_request_blake3,applescript_request_id,library_path)
            VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(task_key,request_key) DO NOTHING",
            params![key, request, backend, journal, hash, script_id, library],
        )?;
        conn.execute(
            "INSERT INTO fast_img_photos_entries(task_key,ordinal,request_key,kind,
            source_rel,source_blake3,output_rel,output_blake3,album_name)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                key,
                i64::try_from(ordinal)?,
                request,
                match intent.kind {
                    PhotosSubmissionKind::Encoded => "encoded",
                    PhotosSubmissionKind::Original => "original",
                },
                intent.source_rel,
                intent.source_blake3,
                intent.output_rel,
                intent.output_blake3,
                intent.album_name
            ],
        )?;
    }
    Ok(())
}

fn write_receipts(
    conn: &Connection,
    key: &str,
    kind: &str,
    old: &[LibraryAssetRecord],
    next: &[LibraryAssetRecord],
) -> Result<()> {
    let previous: BTreeMap<_, _> = old
        .iter()
        .enumerate()
        .map(|(index, asset)| (asset.rel_path.as_str(), (index, asset)))
        .collect();
    let desired: BTreeMap<_, _> = next
        .iter()
        .enumerate()
        .map(|(index, asset)| (asset.rel_path.as_str(), (index, asset)))
        .collect();
    for rel in previous.keys().filter(|rel| !desired.contains_key(**rel)) {
        conn.execute("DELETE FROM fast_img_photos_receipts WHERE task_key=?1 AND receipt_kind=?2 AND rel_path=?3",
            params![key,kind,rel])?;
    }
    // Move only reordered rows outside both live ordinal ranges before assigning
    // final ordinals. This preserves uniqueness during swaps and compaction.
    let staging_start = i64::try_from(old.len().max(next.len()))?;
    for (rel, (ordinal, _)) in &previous {
        if desired
            .get(rel)
            .is_some_and(|(new_ordinal, _)| ordinal != new_ordinal)
        {
            let temporary = staging_start
                .checked_add(i64::try_from(*ordinal)?)
                .context("Fast IMG receipt staging ordinal overflow")?;
            conn.execute(
                "UPDATE fast_img_photos_receipts SET ordinal=?4
                WHERE task_key=?1 AND receipt_kind=?2 AND rel_path=?3",
                params![key, kind, rel, temporary],
            )?;
        }
    }
    for (ordinal, asset) in next.iter().enumerate() {
        if previous
            .get(asset.rel_path.as_str())
            .is_some_and(|(old_ordinal, old_asset)| *old_ordinal == ordinal && *old_asset == asset)
        {
            continue;
        }
        conn.execute("INSERT INTO fast_img_photos_receipts(task_key,receipt_kind,rel_path,ordinal,
            blake3,sync_status,quarantined,photos_uuid,library_blake3,xmp_sidecar_blake3)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
            ON CONFLICT(task_key,receipt_kind,rel_path) DO UPDATE SET ordinal=excluded.ordinal,
            blake3=excluded.blake3,sync_status=excluded.sync_status,quarantined=excluded.quarantined,
            photos_uuid=excluded.photos_uuid,library_blake3=excluded.library_blake3,xmp_sidecar_blake3=excluded.xmp_sidecar_blake3",
            params![key,kind,asset.rel_path,i64::try_from(ordinal)?,asset.blake3,asset.sync_status,
                i64::from(asset.quarantined),asset.photos_uuid,asset.library_blake3,asset.xmp_sidecar_blake3])?;
    }
    Ok(())
}

/// Caller must own an IMMEDIATE transaction, including when migrating legacy state.
fn write_snapshot(conn: &Connection, marker: &WorkingCopyMarker) -> Result<()> {
    write_observed_snapshot(conn, marker, None)
}

fn write_observed_snapshot(
    conn: &Connection,
    marker: &WorkingCopyMarker,
    observation: Option<(&str, &PrimaryResultObservation)>,
) -> Result<()> {
    let wc = canonical_text(&marker.working_copy)?;
    let source = canonical_text(&marker.src_dir)?;
    let mut normalized = marker.clone();
    normalized.working_copy = PathBuf::from(&wc);
    normalized.src_dir = PathBuf::from(&source);
    write_snapshot_at_with_evidence(
        conn,
        &normalized,
        &wc,
        &source,
        true,
        (None, None),
        observation,
    )
}

fn write_snapshot_at(
    conn: &Connection,
    marker: &WorkingCopyMarker,
    wc: &str,
    source: &str,
    validate_retry: bool,
) -> Result<()> {
    write_snapshot_at_with_finish(conn, marker, wc, source, validate_retry, None, None)
}

fn write_snapshot_at_with_finish(
    conn: &Connection,
    marker: &WorkingCopyMarker,
    wc: &str,
    source: &str,
    validate_retry: bool,
    finalization: Option<(&FailedRetryFinishSnapshot, &FailedRetryFinishEvidence)>,
    new_admissions: Option<&FastImgAdmissionPlan>,
) -> Result<()> {
    write_snapshot_at_with_evidence(
        conn,
        marker,
        wc,
        source,
        validate_retry,
        (finalization, new_admissions),
        None,
    )
}

fn write_snapshot_at_with_evidence(
    conn: &Connection,
    marker: &WorkingCopyMarker,
    wc: &str,
    source: &str,
    validate_retry: bool,
    (finalization, new_admissions): (
        Option<(&FailedRetryFinishSnapshot, &FailedRetryFinishEvidence)>,
        Option<&FastImgAdmissionPlan>,
    ),
    observation: Option<(&str, &PrimaryResultObservation)>,
) -> Result<()> {
    ensure!(
        !conn.is_autocommit(),
        "Fast IMG snapshot requires an explicit transaction"
    );
    validate_marker_structure(marker)?;
    if validate_retry && finalization.is_none() {
        marker
            .validate_checkpoint_path_contract(&marker.working_copy)
            .map_err(anyhow::Error::msg)?;
    } else if !validate_retry {
        // The new child has no media artifacts to inspect. Its canonical paths
        // come from a verified parent and deterministic reservation, and its
        // manifest is validated before this same transaction can commit.
        ensure!(
            marker.failed_retry_manifest_blake3.is_some()
                && marker.stage == crate::pipeline::verification::FastImgStageName::ScanComplete
                && marker.blake3_log.is_empty()
                && marker.skipped_sources.is_empty()
                && marker.failed_sources.is_empty()
                && marker.tier2_failed_sources.is_empty()
                && marker.photos_imported_assets.is_empty()
                && marker.tier2_imported_assets.is_empty()
                && marker
                    .photos_submission_history()
                    .is_some_and(|history| history.intents.is_empty()),
            "A retry snapshot may only initialize an empty, manifest-bound child"
        );
    }
    validate_locator(wc)?;
    validate_locator(source)?;
    ensure!(
        marker.working_copy == Path::new(wc) && marker.src_dir == Path::new(source),
        "Fast IMG snapshot paths differ from their canonical locators"
    );
    ensure!(
        wc != source && !Path::new(wc).starts_with(source) && !Path::new(source).starts_with(wc),
        "Fast IMG source and output roots overlap"
    );
    // Persist one canonical path authority. Display spellings must not create
    // a second task identity hidden inside the header JSON.
    let key = key_for_canonical(wc);
    let previous = if validate_retry {
        read_snapshot(conn, &key)?
    } else {
        read_snapshot_base(conn, &key)?
    };
    let revision = if let Some(old) = &previous {
        ensure!(
            new_admissions.is_none() && old.admission_plan_blake3 == marker.admission_plan_blake3,
            "Fast IMG durable admissions cannot be replaced or removed"
        );
        let (saved_source, revision): (String, i64) = conn.query_row(
            "SELECT source_root,revision FROM fast_img_tasks WHERE task_key=?1",
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        ensure!(
            saved_source == source
                && old.started_at == marker.started_at
                && old.strategy == marker.strategy,
            "Fast IMG output is reserved by a different task; saved state was not replaced"
        );
        finish::validate_write(old, marker, revision, finalization)?;
        preserve_history(old, marker)?;
        ensure!(
            old.tier2_failed_sources
                .iter()
                .all(|(rel, failure)| marker.tier2_failed_sources.get(rel) == Some(failure)),
            "Saved original-format failure evidence cannot be removed or replaced"
        );
        ensure!(
            !old.primary_cleanup_journal || marker.primary_cleanup_journal,
            "Fast IMG primary cleanup journal cannot be disabled"
        );
        cleanup::validate_rows(conn, &key, marker)?;
        ensure!(
            !old.tier2_cleanup_journal || marker.tier2_cleanup_journal,
            "Fast IMG original-format cleanup journal cannot be disabled"
        );
        tier2_cleanup::validate_rows(conn, &key, marker)?;
        retry::validate_update(old, marker)?;
        if old == marker {
            match observation {
                None => return Ok(()),
                Some((rel, incoming))
                    if primary_result::current(conn, &key, rel, revision, old)?
                        .and_then(|saved| saved.observation)
                        .as_ref()
                        == Some(incoming) =>
                {
                    return Ok(());
                }
                Some(_) => {}
            }
        }
        retry::require_parent_unmodified(conn, &key)?;
        revision
            .checked_add(1)
            .context("Fast IMG state revision overflow")?
    } else {
        ensure!(
            new_admissions.is_some() == marker.admission_plan_blake3.is_some(),
            "New Fast IMG admission rows require their atomic task header"
        );
        ensure!(
            finalization.is_none() && marker.failed_retry_finish_evidence().is_none(),
            "Finish-only evidence cannot initialize or replace a missing task"
        );
        0
    };
    if validate_retry {
        retry::validate_task(conn, &key, marker)?;
    }
    let header = small_header(marker)?;
    let digest = snapshot_digest(wc, source, marker)?;
    let now = chrono::Utc::now().timestamp();
    ensure!(now >= 0, "Fast IMG state clock is before Unix epoch");
    conn.execute(
        "INSERT INTO fast_img_tasks(task_key,working_copy,source_root,header_json,
        header_blake3,snapshot_blake3,revision,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
        ON CONFLICT(task_key) DO UPDATE SET header_json=excluded.header_json,
        header_blake3=excluded.header_blake3,snapshot_blake3=excluded.snapshot_blake3,
        revision=excluded.revision,updated_at=excluded.updated_at",
        params![
            key,
            wc,
            source,
            header,
            blake3::hash(&header).as_bytes().as_slice(),
            digest.as_bytes().as_slice(),
            revision,
            now
        ],
    )?;
    if let Some(plan) = new_admissions {
        admission::insert_rows(conn, &key, plan)?;
        ensure!(
            admission::read_validated(conn, &key, marker)?.as_ref() == Some(plan),
            "Fast IMG admission readback did not match the requested plan"
        );
    }
    write_sources(conn, &key, previous.as_ref(), marker, revision, observation)?;
    for (rel, failure) in &marker.tier2_failed_sources {
        if previous
            .as_ref()
            .is_some_and(|old| old.tier2_failed_sources.contains_key(rel))
        {
            continue;
        }
        conn.execute(
            "INSERT INTO fast_img_tier2_failures(task_key,source_rel,source_blake3,reason) VALUES(?1,?2,?3,?4)",
            params![key, rel, failure.src, failure.reason],
        )?;
    }
    append_submissions(conn, &key, previous.as_ref(), marker)?;
    write_receipts(
        conn,
        &key,
        "primary",
        previous
            .as_ref()
            .map_or(&[], |old| old.photos_imported_assets.as_slice()),
        &marker.photos_imported_assets,
    )?;
    write_receipts(
        conn,
        &key,
        "original",
        previous
            .as_ref()
            .map_or(&[], |old| old.tier2_imported_assets.as_slice()),
        &marker.tier2_imported_assets,
    )?;
    ensure!(
        (if validate_retry {
            read_snapshot(conn, &key)?
        } else {
            read_snapshot_base(conn, &key)?
        })
        .as_ref()
            == Some(marker),
        "Fast IMG SQL readback did not reproduce the requested snapshot; transaction must roll back"
    );
    Ok(())
}

/// Explicit, validated one-time migration. Normal reads never fall back to JSON.
///
/// # Errors
/// Preserves old state on unknown data, changed ownership, conflicts or storage failure.
pub fn migrate_legacy_for_source(
    source: &Path,
    requested_output: Option<&Path>,
) -> io::Result<MigrationReport> {
    migration::migrate_legacy_for_source(source, requested_output).map_err(as_io)
}

/// Locate a retired-format marker for migration/diagnostics only.
///
/// # Errors
/// Rejects ambiguous or unavailable formal storage and path identities.
pub fn legacy_marker_path_for_working_copy(output: &Path) -> io::Result<PathBuf> {
    migration::legacy_marker_path_for_working_copy(output).map_err(as_io)
}

fn row_count(conn: &Connection) -> Result<usize> {
    let mut count = 0usize;
    for table in TABLES {
        let rows: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        count = count
            .checked_add(usize::try_from(rows)?)
            .context("Fast IMG state row count overflow")?;
    }
    count = count
        .checked_add(retry::row_count(conn)?)
        .context("Fast IMG retry row count overflow")?;
    count = count
        .checked_add(cleanup::row_count(conn)?)
        .context("Fast IMG cleanup row count overflow")?;
    count = count
        .checked_add(tier2_cleanup::row_count(conn)?)
        .context("Fast IMG original-format cleanup row count overflow")?;
    let version = inspect_store_schema_version(conn)?;
    if version.is_some_and(|version| {
        version >= crate::mfb_sqlite_store::FAST_IMG_ADMISSION_SCHEMA_VERSION
    }) {
        let rows: i64 = conn.query_row("SELECT count(*) FROM fast_img_admission", [], |row| {
            row.get(0)
        })?;
        count = count
            .checked_add(usize::try_from(rows)?)
            .context("Fast IMG admission row count overflow")?;
    }
    for (table, since) in [
        (
            "fast_img_admission_xmp",
            FAST_IMG_ADMISSION_XMP_SCHEMA_VERSION,
        ),
        (
            "fast_img_admission_aae",
            FAST_IMG_ADMISSION_AAE_SCHEMA_VERSION,
        ),
    ] {
        if version.is_some_and(|version| version >= since) {
            let rows: i64 =
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            count = count
                .checked_add(usize::try_from(rows)?)
                .context("Fast IMG sidecar admission row count overflow")?;
        }
    }
    if version.is_some_and(|version| version >= 8) {
        let rows: i64 =
            conn.query_row("SELECT count(*) FROM fast_img_tier2_failures", [], |row| {
                row.get(0)
            })?;
        count = count
            .checked_add(usize::try_from(rows)?)
            .context("Fast IMG original-format failure row count overflow")?;
    }
    if version.is_some_and(|version| version >= FAST_IMG_PHOTOS_UNCERTAINTY_SCHEMA_VERSION) {
        let rows: i64 = conn.query_row(
            "SELECT count(*) FROM fast_img_photos_uncertain",
            [],
            |row| row.get(0),
        )?;
        count = count
            .checked_add(usize::try_from(rows)?)
            .context("Fast IMG Photos uncertainty row count overflow")?;
    }
    for (table, required_since) in EXECUTION_TABLES {
        if version.is_none_or(|version| version < required_since) {
            continue;
        }
        let rows: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        count = count
            .checked_add(usize::try_from(rows)?)
            .context("Fast IMG execution row count overflow")?;
    }
    if version.is_some_and(|version| version >= FAST_IMG_PRIMARY_RESULT_SCHEMA_VERSION) {
        let rows: i64 =
            conn.query_row("SELECT count(*) FROM fast_img_primary_results", [], |row| {
                row.get(0)
            })?;
        count = count
            .checked_add(usize::try_from(rows)?)
            .context("Fast IMG primary result row count overflow")?;
    }
    Ok(count)
}

fn ensure_no_orphans(conn: &Connection) -> Result<()> {
    ensure!(
        conn.prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_none(),
        "SQLite contains orphaned formal state; task discovery and integrity are not confirmed"
    );
    Ok(())
}

/// Count protected Fast IMG rows; this is not an integrity or custody verdict.
///
/// # Errors
/// Rejects unknown or incomplete schema and invalid aggregate counts.
pub fn inspect_row_count(conn: &Connection) -> Result<usize> {
    in_snapshot(conn, |conn| {
        if current_family(conn)? {
            row_count(conn)
        } else {
            Ok(0)
        }
    })
}

/// Verify every persisted field and migration backup in one SQL snapshot.
/// This does not access media or confer Photos custody/source deletion eligibility.
///
/// # Errors
/// Rejects malformed state, hash drift, orphan rows and unsupported schema.
pub fn inspect_integrity(conn: &Connection) -> Result<usize> {
    in_snapshot(conn, |conn| {
        if !current_family(conn)? {
            return Ok(0);
        }
        let keys = conn
            .prepare("SELECT task_key FROM fast_img_tasks ORDER BY task_key")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let version = inspect_store_schema_version(conn)?;
        for key in keys {
            let marker = read_snapshot(conn, &key)?
                .context("Fast IMG task disappeared during integrity inspection")?;
            admission::read_validated(conn, &key, &marker)?;
            if version.is_some_and(|version| version >= FAST_IMG_PHOTOS_UNCERTAINTY_SCHEMA_VERSION)
            {
                photos_uncertainty_evidence(conn, &key, &marker)?;
            }
            if version.is_some_and(|version| version >= JPEG_EXECUTION_SCHEMA_VERSION) {
                jpeg_execution::validate_receipts(conn, &key, &marker, version)?;
            }
            if version.is_some_and(|version| version >= FAST_IMG_PRIMARY_RESULT_SCHEMA_VERSION) {
                primary_result::validate_history(conn, &key, &marker)?;
            }
        }
        ensure_no_orphans(conn)?;
        migration::inspect_backups(conn)?;
        row_count(conn)
    })
}
