//! Crash recovery between verified encoder output and its published result receipt.
use super::{
    Connection, Context, FAST_IMG_PUBLICATION_SCHEMA_VERSION, Result, TransactionBehavior,
    WorkingCopyMarker, as_io, canonical_bytes, canonical_text, ensure,
    inspect_store_schema_version, io, key_for_canonical, open_readonly, open_store_connection,
    params,
};
use crate::verification::validate_marker_relative_path;
use serde::{Deserialize, Serialize};

/// One verified candidate awaiting its formal output receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PendingPublication {
    pub source_rel: String,
    pub source_blake3: String,
    pub output_rel: String,
    pub output_blake3: String,
    pub stage_name: String,
    pub proof: serde_json::Value,
}

impl PendingPublication {
    fn validate(&self) -> Result<()> {
        for rel in [&self.source_rel, &self.output_rel] {
            validate_marker_relative_path("pending publication", rel)
                .map_err(anyhow::Error::msg)?;
        }
        ensure!(
            self.stage_name
                .strip_prefix(".mfb-fast-img-publish-")
                .is_some_and(|nonce| !nonce.is_empty()
                    && nonce.len() <= 96
                    && nonce
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')),
            "Invalid private publication stage"
        );
        ensure!(
            super::cleanup::hash_is_valid(&self.source_blake3)
                && super::cleanup::hash_is_valid(&self.output_blake3)
                && self.proof.is_object(),
            "Invalid pending publication hashes or proof"
        );
        Ok(())
    }
}

// Include stable task identity and admission/tool contracts, never mutable progress counters.
pub(super) fn owner(
    marker: &WorkingCopyMarker,
    canonical_source: &std::path::Path,
) -> Result<blake3::Hash> {
    Ok(blake3::hash(&canonical_bytes(&(
        canonical_source,
        &marker.started_at,
        &marker.strategy,
        marker.metadata_policy_version,
        &marker.processing_contract,
        &marker.admission_plan_blake3,
        &marker.failed_retry_manifest_blake3,
        &marker.tool_snapshot_manifest,
        &marker.tool_snapshot_identity,
        marker.tool_snapshot_disabled,
    ))?))
}

fn bound_key(conn: &Connection, marker: &WorkingCopyMarker) -> Result<(String, blake3::Hash)> {
    let wc = canonical_text(&marker.working_copy)?;
    let key = key_for_canonical(&wc);
    let (source, header, digest): (String, Vec<u8>, Vec<u8>) = conn.query_row(
        "SELECT source_root,header_json,header_blake3 FROM fast_img_tasks WHERE task_key=?1 AND working_copy=?2",
        params![key, wc], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))
    ).context("Pending publication has no matching persisted task")?;
    ensure!(
        blake3::hash(&header).as_bytes().as_slice() == digest,
        "Invalid persisted publication owner"
    );
    let saved: WorkingCopyMarker = serde_json::from_slice(&header)?;
    let canonical_source = std::path::Path::new(&source);
    let authority = owner(marker, canonical_source)?;
    ensure!(
        source == canonical_text(&marker.src_dir)?
            && saved.src_dir == std::path::Path::new(&source)
            && saved.working_copy == std::path::Path::new(&wc)
            && authority == owner(&saved, canonical_source)?,
        "Pending publication belongs to another task or processing contract"
    );
    Ok((key, authority))
}

fn checksum(key: &str, authority: &blake3::Hash, payload: &[u8]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new_derive_key("MFB fast-img pending publication v1");
    hasher.update(key.as_bytes());
    hasher.update(authority.as_bytes());
    hasher.update(payload);
    hasher.finalize()
}

fn payload(record: &PendingPublication) -> Result<Vec<u8>> {
    record.validate()?;
    let bytes = canonical_bytes(record)?;
    ensure!(
        bytes.len() <= 1_048_576,
        "Fast IMG pending publication exceeds its size limit"
    );
    Ok(bytes)
}

pub(super) fn read_records(
    conn: &Connection,
    key: &str,
    authority: &blake3::Hash,
) -> Result<Vec<PendingPublication>> {
    let mut stmt = conn.prepare(
        "SELECT source_rel,output_rel,payload,payload_blake3,stage_name FROM fast_img_pending_publications WHERE task_key=?1 ORDER BY source_rel"
    )?;
    let mut rows = stmt.query([key])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(2)?;
        let hash: Vec<u8> = row.get(3)?;
        ensure!(
            checksum(key, authority, &bytes).as_bytes().as_slice() == hash,
            "Pending publication failed its task-bound checksum"
        );
        let record: PendingPublication = serde_json::from_slice(&bytes)?;
        record.validate()?;
        ensure!(
            row.get::<_, String>(0)? == record.source_rel
                && row.get::<_, String>(1)? == record.output_rel
                && row.get::<_, String>(4)? == record.stage_name,
            "Pending publication source or destination locator changed"
        );
        records.push(record);
    }
    Ok(records)
}

/// Durably bind a candidate to this task before it becomes visible at its output path.
///
/// # Errors
/// Rejects changed ownership, malformed records or conflicting publications.
pub fn prepare_publication(
    marker: &WorkingCopyMarker,
    record: &PendingPublication,
) -> io::Result<()> {
    (|| -> Result<()> {
        let bytes = payload(record)?;
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (key, authority) = bound_key(&tx, marker)?;
        if marker.has_admission_plan() || marker.failed_retry_manifest_blake3.is_some() {
            let lookup = if marker.has_admission_plan() {
                "SELECT EXISTS(SELECT 1 FROM fast_img_admission WHERE task_key=?1 AND source_rel=?2 AND source_blake3=?3 AND route='primary')"
            } else {
                "SELECT EXISTS(SELECT 1 FROM fast_img_retry_sources WHERE task_key=?1 AND source_rel=?2 AND admitted_source_blake3=?3 AND (route='primary' OR route IS NULL))"
            };
            let admitted: bool = tx.query_row(lookup, params![key, record.source_rel, record.source_blake3], |row| row.get(0))?;
            ensure!(admitted, "Pending publication source differs from its durable admission");
        }
        let digest = checksum(&key, &authority, &bytes);
        tx.execute("INSERT INTO fast_img_pending_publications(task_key,source_rel,output_rel,payload,payload_blake3,stage_name)
            VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(task_key,source_rel) DO NOTHING",
            params![key, record.source_rel, record.output_rel, bytes, digest.as_bytes().as_slice(), record.stage_name])?;
        let same: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM fast_img_pending_publications WHERE
            task_key=?1 AND source_rel=?2 AND output_rel=?3 AND payload=?4 AND payload_blake3=?5 AND stage_name=?6)",
            params![key, record.source_rel, record.output_rel,bytes,digest.as_bytes().as_slice(),record.stage_name], |row| row.get(0))?;
        ensure!(same, "A different publication already occupies this source");
        tx.commit()?;
        Ok(())
    })().map_err(as_io)
}

/// Read verified pending records for this task without updating them.
///
/// # Errors
/// Rejects changed bindings, invalid schema, malformed paths or damaged evidence.
pub fn pending_publications(marker: &WorkingCopyMarker) -> io::Result<Vec<PendingPublication>> {
    (|| -> Result<_> {
        let Some(conn) = open_readonly()? else { return Ok(Vec::new()) };
        super::in_snapshot(&conn, |conn| {
            if inspect_store_schema_version(conn)?.is_none_or(|v| v < FAST_IMG_PUBLICATION_SCHEMA_VERSION) {
                return Ok(Vec::new());
            }
            let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM fast_img_tasks WHERE task_key=?1) OR EXISTS(SELECT 1 FROM fast_img_pending_publications WHERE task_key=?1)",
                [&key], |row| row.get(0))?;
            if !exists { return Ok(Vec::new()) }
            let (_, authority) = bound_key(conn, marker)?;
            read_records(conn, &key, &authority)
        })
    })().map_err(as_io)
}

/// Retire exactly the publication acknowledged by the caller.
///
/// # Errors
/// Rejects a changed task or conflicting record; its journal remains intact.
pub fn forget_publication(
    marker: &WorkingCopyMarker,
    record: &PendingPublication,
) -> io::Result<()> {
    (|| -> Result<()> {
        let bytes = payload(record)?;
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (key, authority) = bound_key(&tx, marker)?;
        let hash = checksum(&key, &authority, &bytes);
        tx.execute("DELETE FROM fast_img_pending_publications WHERE task_key=?1 AND source_rel=?2 AND output_rel=?3 AND payload=?4 AND payload_blake3=?5 AND stage_name=?6",
            params![key, record.source_rel, record.output_rel, bytes, hash.as_bytes().as_slice(),record.stage_name])?;
        let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM fast_img_pending_publications WHERE task_key=?1 AND source_rel=?2)",
            params![key, record.source_rel], |row| row.get(0))?;
        ensure!(!occupied, "The pending publication changed before retirement");
        tx.commit()?;
        Ok(())
    })().map_err(as_io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v15_pending_publication_upgrade_preserves_checkpoint_and_requires_exact_retirement()
    -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().canonicalize()?;
        let db = root.join("state.sqlite");
        let _store = crate::mfb_sqlite_store::set_test_store_path_for_tests(db.clone());
        let mut marker = WorkingCopyMarker::new(root.join("source"), root.join("output"), 2);
        std::fs::create_dir(&marker.src_dir)?;
        std::fs::create_dir(&marker.working_copy)?;
        marker.skipped_sources.insert(
            "previous.jpg".into(),
            super::super::SkippedSourceEntry {
                src: blake3::hash(b"previous").to_hex().to_string(),
                reason: "synthetic retained source".into(),
            },
        );
        super::super::write_marker(&marker)?;
        let conn = open_store_connection()?;
        conn.execute_batch(
            "DROP TABLE fast_img_pending_publications;
            UPDATE store_metadata SET value=15 WHERE key='schema_version';
            CREATE TRIGGER interrupt_v16 BEFORE UPDATE ON store_metadata
            WHEN NEW.value=16 BEGIN SELECT RAISE(ABORT,'synthetic interruption'); END",
        )?;
        let prior = conn.query_row("SELECT snapshot_blake3 FROM fast_img_tasks", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        drop(conn);
        assert_eq!(pending_publications(&marker)?, []);
        assert!(open_store_connection().is_err());
        let conn = Connection::open(&db)?;
        assert_eq!(inspect_store_schema_version(&conn)?, Some(15));
        conn.execute_batch("DROP TRIGGER interrupt_v16")?;
        drop(conn);

        let pending = PendingPublication {
            source_rel: "source.jpg".into(),
            source_blake3: blake3::hash(b"source").to_hex().to_string(),
            output_rel: "source.JXL".into(),
            output_blake3: blake3::hash(b"output").to_hex().to_string(),
            stage_name: ".mfb-fast-img-publish-synthetic".into(),
            proof: serde_json::json!({"synthetic": true}),
        };
        prepare_publication(&marker, &pending)?;
        prepare_publication(&marker, &pending)?;
        assert_eq!(
            pending_publications(&marker)?,
            std::slice::from_ref(&pending)
        );
        let conn = open_store_connection()?;
        assert_eq!(inspect_store_schema_version(&conn)?, Some(16));
        let preserved: Vec<u8> =
            conn.query_row("SELECT snapshot_blake3 FROM fast_img_tasks", [], |row| {
                row.get(0)
            })?;
        assert_eq!(prior, preserved);
        let previous: String = conn.query_row(
            "SELECT source_blake3 FROM fast_img_source_outcomes WHERE source_rel='previous.jpg'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(previous, marker.skipped_sources["previous.jpg"].src);
        assert_eq!(super::super::inspect_integrity(&conn)?, 3);
        let other = WorkingCopyMarker::new(marker.src_dir.clone(), root.join("second-output"), 1);
        std::fs::create_dir(&other.working_copy)?;
        super::super::write_marker(&other)?;
        assert!(prepare_publication(&other, &pending).is_err());
        assert_eq!(pending_publications(&other)?, []);
        let mut changed = pending.clone();
        changed.output_blake3 = blake3::hash(b"foreign").to_hex().to_string();
        assert!(prepare_publication(&marker, &changed).is_err());
        assert!(forget_publication(&marker, &changed).is_err());
        let mut changed_owner = marker.clone();
        changed_owner.started_at = "different-task".into();
        assert!(pending_publications(&changed_owner).is_err());
        changed.stage_name = "../foreign".into();
        assert!(prepare_publication(&marker, &changed).is_err());
        forget_publication(&marker, &pending)?;
        forget_publication(&marker, &pending)?;
        assert_eq!(pending_publications(&marker)?, []);
        prepare_publication(&marker, &pending)?;
        conn.execute(
            "UPDATE fast_img_pending_publications SET payload_blake3=zeroblob(32)",
            [],
        )?;
        assert!(pending_publications(&marker).is_err());
        assert!(super::super::inspect_integrity(&conn).is_err());
        assert!(forget_publication(&marker, &pending).is_err());
        Ok(())
    }
}
