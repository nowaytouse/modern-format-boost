//! Durable source unlink evidence for original-format Photos delivery.

use super::*;
use crate::batch::{DiscoveredSource, planned_source::PlannedSource};
use crate::image::fast_img::{SourceUnlinkEvent, SourceUnlinkKind, Tier2CleanupObserver};
use crate::pipeline::verification::{
    FastImgStageName, LibraryAssetRecord, validate_marker_relative_path,
};
use crate::unified_error::ImgQualityError;
use std::fs;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Entry {
    kind: String,
    source: String,
    asset: String,
    library: String,
    hash: String,
    claim: String,
    removed: bool,
}

type SourceEntries = BTreeMap<String, Entry>;
type Entries = BTreeMap<String, SourceEntries>;

fn has_schema(conn: &Connection) -> Result<bool> {
    Ok(inspect_store_schema_version(conn)?.is_some_and(|version| version >= 7))
}

fn asset_digest(marker: &WorkingCopyMarker, asset: &LibraryAssetRecord) -> Result<String> {
    let library = marker
        .photos_library_path
        .as_deref()
        .context("Tier-2 cleanup has no bound Photos library")?;
    let uuid = asset
        .photos_uuid
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("Tier-2 cleanup receipt has no Photos UUID")?;
    let hash = library_hash(asset)?;
    ensure!(
        !asset.quarantined
            && cleanup::hash_is_valid(&asset.blake3)
            && cleanup::hash_is_valid(hash)
            && asset
                .xmp_sidecar_blake3
                .as_deref()
                .is_none_or(cleanup::hash_is_valid),
        "Tier-2 cleanup requires complete, unquarantined Photos evidence"
    );
    Ok(blake3::hash(&canonical_bytes(&(
        library,
        uuid,
        &asset.rel_path,
        &asset.blake3,
        hash,
        &asset.xmp_sidecar_blake3,
    ))?)
    .to_hex()
    .to_string())
}

fn library_hash(asset: &LibraryAssetRecord) -> Result<&str> {
    match (asset.library_blake3.as_deref(), &asset.xmp_sidecar_blake3) {
        (Some(hash), Some(_)) => Ok(hash),
        (Some(hash), None) if hash == asset.blake3 => Ok(hash),
        (None, None) => Ok(&asset.blake3),
        _ => bail!("Tier-2 cleanup receipt does not prove the exact Photos original"),
    }
}

fn entry_digest(key: &str, source: &str, rel: &str, entry: &Entry) -> Result<blake3::Hash> {
    Ok(blake3::hash(&canonical_bytes(&(key, source, rel, entry))?))
}

fn validate_entries(
    marker: &WorkingCopyMarker,
    asset: &LibraryAssetRecord,
    source_entries: &SourceEntries,
    admission: Option<&FailedRetryAdmission>,
) -> Result<()> {
    let source = &asset.rel_path;
    validate_marker_relative_path("Tier-2 cleanup source", source).map_err(anyhow::Error::msg)?;
    ensure!(
        !marker.blake3_log.contains_key(source),
        "Tier-2 cleanup source also claims a primary delivery"
    );
    let media = source_entries
        .get(source)
        .context("Tier-2 cleanup has no media intent")?;
    ensure!(
        media.kind == "media" && media.hash == asset.blake3,
        "Tier-2 media intent differs from Photos receipt"
    );
    let digest = asset_digest(marker, asset)?;
    let library = library_hash(asset)?;
    let mut companions = source_entries.iter().filter(|(rel, _)| *rel != source);
    let companion = companions.next();
    ensure!(
        companions.next().is_none(),
        "Tier-2 cleanup has multiple XMP intents"
    );
    match (asset.xmp_sidecar_blake3.as_deref(), companion) {
        (None, None) => {}
        (Some(hash), Some((rel, entry))) => {
            let path = Path::new(rel);
            ensure!(
                entry.kind == "xmp"
                    && entry.hash == hash
                    && path.parent() == Path::new(source).parent()
                    && path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("xmp")),
                "Tier-2 companion intent differs from Photos receipt"
            );
        }
        _ => bail!("Tier-2 XMP intent coverage differs from Photos receipt"),
    }
    if let Some(admission) = admission {
        ensure!(
            admission.admitted_source_blake3 == media.hash && admission.claim == media.claim,
            "Tier-2 media intent differs from immutable failed-only admission"
        );
        match (&admission.xmp, companion) {
            (FailedRetrySidecar::Absent, None) => {}
            (
                FailedRetrySidecar::Present {
                    relative_path,
                    blake3,
                    claim,
                },
                Some((rel, entry)),
            ) => {
                ensure!(
                    relative_path == rel && blake3 == &entry.hash && claim == &entry.claim,
                    "Tier-2 XMP intent differs from immutable failed-only admission"
                );
            }
            _ => bail!("Tier-2 XMP presence differs from immutable failed-only admission"),
        }
    }
    for (rel, entry) in source_entries {
        validate_marker_relative_path("Tier-2 cleanup entry", rel).map_err(anyhow::Error::msg)?;
        ensure!(
            entry.source == asset.blake3 && entry.asset == digest && entry.library == library,
            "Tier-2 cleanup entry no longer matches its Photos custody receipt"
        );
        PlannedSource::validate_identity(&entry.claim, &marker.src_dir.join(rel), &entry.hash)?;
    }
    Ok(())
}

fn complete(marker: &WorkingCopyMarker, entries: &Entries) -> bool {
    marker
        .tier2_imported_assets
        .iter()
        .all(|asset| entries.contains_key(&asset.rel_path))
}

fn admissions(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<Option<BTreeMap<String, FailedRetryAdmission>>> {
    marker
        .failed_retry_manifest_blake3
        .is_some()
        .then(|| retry::admissions(conn, key))
        .transpose()
}

fn admission<'a>(
    admissions: Option<&'a BTreeMap<String, FailedRetryAdmission>>,
    source: &str,
) -> Result<Option<&'a FailedRetryAdmission>> {
    admissions
        .map(|values| {
            values
                .get(source)
                .context("Tier-2 cleanup source is outside its failed-only admission")
        })
        .transpose()
}

fn read_entries(conn: &Connection, key: &str, marker: &WorkingCopyMarker) -> Result<Entries> {
    let mut entries = Entries::new();
    if !has_schema(conn)? {
        ensure!(
            !marker.tier2_cleanup_journal,
            "Tier-2 cleanup journal schema is unavailable"
        );
        return Ok(entries);
    }
    let mut statement = conn.prepare(
        "SELECT source_rel,entry_rel,kind,source_blake3,asset_blake3,library_blake3,
                entry_blake3,entry_claim,state,evidence_blake3
         FROM fast_img_tier2_cleanup WHERE task_key=?1 ORDER BY source_rel,entry_rel",
    )?;
    let mut rows = statement.query([key])?;
    while let Some(row) = rows.next()? {
        ensure!(
            marker.tier2_cleanup_journal,
            "Tier-2 cleanup evidence has no journal owner"
        );
        let source: String = row.get(0)?;
        let rel: String = row.get(1)?;
        let entry = Entry {
            kind: row.get(2)?,
            source: row.get(3)?,
            asset: row.get(4)?,
            library: row.get(5)?,
            hash: row.get(6)?,
            claim: row.get(7)?,
            removed: match row.get::<_, String>(8)?.as_str() {
                "planned" => false,
                "removed" => true,
                _ => bail!("Unrecognized Tier-2 cleanup state"),
            },
        };
        ensure!(
            entry_digest(key, &source, &rel, &entry)?
                .as_bytes()
                .as_slice()
                == row.get::<_, Vec<u8>>(9)?,
            "Tier-2 cleanup evidence failed BLAKE3 verification"
        );
        entries.entry(source).or_default().insert(rel, entry);
    }
    if marker.tier2_cleanup_journal {
        let assets: BTreeMap<_, _> = marker
            .tier2_imported_assets
            .iter()
            .map(|asset| (asset.rel_path.as_str(), asset))
            .collect();
        ensure!(
            assets.len() == marker.tier2_imported_assets.len(),
            "Tier-2 cleanup receipt path is ambiguous"
        );
        for source in assets.keys() {
            validate_marker_relative_path("Tier-2 cleanup source", source)
                .map_err(anyhow::Error::msg)?;
            ensure!(
                !marker.blake3_log.contains_key(*source),
                "Tier-2 cleanup source also claims a primary delivery"
            );
        }
        if entries.is_empty() && !marker.is_terminal() {
            return Ok(entries);
        }
        let admitted = admissions(conn, key, marker)?;
        for (source, source_entries) in &entries {
            let asset = assets
                .get(source.as_str())
                .context("Tier-2 cleanup has no matching Photos receipt")?;
            validate_entries(
                marker,
                asset,
                source_entries,
                admission(admitted.as_ref(), source)?,
            )?;
        }
    }
    if marker.tier2_cleanup_journal && marker.is_terminal() {
        ensure!(
            complete(marker, &entries),
            "Tier-2 cleanup completion lacks source intents"
        );
    }
    Ok(entries)
}

pub(super) fn validate_rows(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<()> {
    read_entries(conn, key, marker).map(|_| ())
}

pub(super) fn reject_missing_task_evidence(conn: &Connection, key: &str) -> Result<()> {
    if has_schema(conn)? {
        let found: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM fast_img_tier2_cleanup WHERE task_key=?1)",
            [key],
            |row| row.get(0),
        )?;
        ensure!(
            !found,
            "Missing Fast IMG task still has protected Tier-2 cleanup evidence"
        );
    }
    Ok(())
}

pub(super) fn row_count(conn: &Connection) -> Result<usize> {
    if !has_schema(conn)? {
        return Ok(0);
    }
    let count: i64 = conn.query_row("SELECT count(*) FROM fast_img_tier2_cleanup", [], |row| {
        row.get(0)
    })?;
    Ok(usize::try_from(count)?)
}

pub(super) fn finish_digest(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<Option<(String, bool)>> {
    if !marker.tier2_cleanup_journal {
        return Ok(None);
    }
    let entries = read_entries(conn, key, marker)?;
    let mut digest = blake3::Hasher::new_derive_key("MFB failed-only finish Tier-2 cleanup v1");
    for (source, source_entries) in &entries {
        for (rel, entry) in source_entries {
            digest.update(entry_digest(key, source, rel, entry)?.as_bytes());
        }
    }
    Ok(Some((
        digest.finalize().to_hex().to_string(),
        complete(marker, &entries),
    )))
}

fn inspect_absence(root: &Path, source: &str, entries: &SourceEntries) -> Result<(bool, bool)> {
    ensure!(
        !cleanup::exists(root, source)?,
        "Tier-2 source reappeared: {source}"
    );
    let media = entries
        .get(source)
        .context("Absent Tier-2 source has no durable media intent")?;
    let expected_xmp = entries.iter().find(|(_, entry)| entry.kind == "xmp");
    let path = root.join(source);
    let parent = path.parent().context("Tier-2 source has no parent")?;
    let current_xmp = match fs::symlink_metadata(parent) {
        Ok(meta) if meta.is_dir() => crate::metadata::find_xmp_sidecar(&path)?,
        Ok(_) => bail!("Tier-2 source parent changed or is not a directory"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let remaining = if let Some(current) = current_xmp {
        let (rel, entry) =
            expected_xmp.context("Unrecorded XMP appeared beside absent Tier-2 source")?;
        ensure!(
            current.as_os_str() == root.join(rel).as_os_str() && !entry.removed,
            "Tier-2 XMP changed or reappeared after removal; retained"
        );
        PlannedSource::verify(&entry.claim, &current)
            .context("Tier-2 XMP identity changed; retained")?;
        true
    } else {
        if let Some((rel, _)) = expected_xmp {
            ensure!(
                !cleanup::exists(root, rel)?,
                "Recorded Tier-2 XMP no longer matches the current companion selection"
            );
        }
        false
    };
    ensure!(
        !cleanup::exists(root, source)?,
        "Tier-2 source reappeared during companion inspection"
    );
    Ok((media.removed, remaining))
}

/// Return absent, journal-verified original-format sources for retry admission.
pub fn validate_tier2_cleanup_resume(marker: &WorkingCopyMarker) -> Result<BTreeSet<String>> {
    let mut absent = BTreeSet::new();
    if !marker.tier2_cleanup_journal || marker.tier2_imported_assets.is_empty() {
        return Ok(absent);
    }
    let conn = open_readonly()?.context("Tier-2 cleanup store is unavailable")?;
    let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
    let entries = in_snapshot(&conn, |conn| read_entries(conn, &key, marker))?;
    for asset in &marker.tier2_imported_assets {
        let rel = &asset.rel_path;
        if !cleanup::exists(&marker.src_dir, rel)? {
            let saved = entries
                .get(rel)
                .context("Absent Tier-2 source has no durable cleanup intent")?;
            inspect_absence(&marker.src_dir, rel, saved)?;
            absent.insert(rel.clone());
        }
    }
    Ok(absent)
}

/// Recheck source and companion absence for completed journaled original delivery.
///
/// # Errors
/// Rejects changed checkpoints, missing intents, remaining originals or XMP, and inspection errors.
pub fn verify_tier2_cleanup_complete(marker: &WorkingCopyMarker) -> Result<()> {
    if !marker.tier2_cleanup_journal || marker.tier2_imported_assets.is_empty() {
        return Ok(());
    }
    ensure!(marker.is_terminal(), "Tier-2 original cleanup is still pending");
    let conn = open_readonly()?.context("Tier-2 cleanup store is unavailable")?;
    let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
    in_snapshot(&conn, |conn| {
        let saved =
            read_snapshot(conn, &key)?.context("Tier-2 cleanup checkpoint is missing")?;
        ensure!(
            &saved == marker,
            "Tier-2 cleanup checkpoint changed during verification"
        );
        let entries = read_entries(conn, &key, marker)?;
        for asset in &marker.tier2_imported_assets {
            let rel = &asset.rel_path;
            let intent = entries
                .get(rel)
                .context("Tier-2 source has no durable cleanup intent")?;
            let (_, xmp_remaining) = inspect_absence(&marker.src_dir, rel, intent)?;
            ensure!(!xmp_remaining, "Tier-2 XMP still awaits cleanup: {rel}");
        }
        Ok(())
    })
}

/// A Tier-2 cleanup attempt bound to a committed marker and its Photos receipts.
pub struct Tier2CleanupJournal<'a> {
    conn: Connection,
    marker: &'a WorkingCopyMarker,
    key: String,
    revision: i64,
    assets: BTreeMap<&'a str, &'a LibraryAssetRecord>,
    admissions: Option<BTreeMap<String, FailedRetryAdmission>>,
    entries: Entries,
    pending: SourceEntries,
    pending_source: Option<String>,
    ready: Option<(String, BTreeSet<String>)>,
}

impl<'a> Tier2CleanupJournal<'a> {
    pub fn open(marker: &'a WorkingCopyMarker) -> Result<Self> {
        ensure!(
            marker.tier2_cleanup_journal
                && marker.tier2_in_progress
                && marker.stage == FastImgStageName::CleanupComplete,
            "Tier-2 cleanup requires a committed original-format delivery checkpoint"
        );
        let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
        let conn = open_store_connection()?;
        let (revision, entries, admissions) = in_snapshot(&conn, |conn| {
            let saved =
                read_snapshot(conn, &key)?.context("Tier-2 cleanup task was not committed")?;
            ensure!(
                &saved == marker,
                "Tier-2 cleanup checkpoint changed before journal admission"
            );
            let revision = conn.query_row(
                "SELECT revision FROM fast_img_tasks WHERE task_key=?1",
                [&key],
                |row| row.get(0),
            )?;
            Ok((
                revision,
                read_entries(conn, &key, marker)?,
                admissions(conn, &key, marker)?,
            ))
        })?;
        let assets: BTreeMap<_, _> = marker
            .tier2_imported_assets
            .iter()
            .map(|asset| (asset.rel_path.as_str(), asset))
            .collect();
        ensure!(
            assets.len() == marker.tier2_imported_assets.len(),
            "Tier-2 cleanup receipt path is ambiguous"
        );
        Ok(Self {
            conn,
            marker,
            key,
            revision,
            assets,
            admissions,
            entries,
            pending: SourceEntries::new(),
            pending_source: None,
            ready: None,
        })
    }

    fn validate_asset(&self, asset: &LibraryAssetRecord) -> Result<()> {
        ensure!(
            self.assets
                .get(asset.rel_path.as_str())
                .is_some_and(|saved| *saved == asset),
            "Tier-2 cleanup does not match the committed asset"
        );
        Ok(())
    }

    fn begin(&mut self, asset: &LibraryAssetRecord, kind: SourceUnlinkKind) -> Result<()> {
        let source = &asset.rel_path;
        let saved = self.entries.get(source);
        let pending = &self.pending;
        for (rel, entry) in pending {
            ensure!(
                cleanup::exists(&self.marker.src_dir, rel)?,
                "Tier-2 cleanup entry disappeared before unlink"
            );
            PlannedSource::verify(&entry.claim, &self.marker.src_dir.join(rel))?;
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        cleanup::require_revision(&tx, &self.key, self.revision)?;
        if kind == SourceUnlinkKind::Xmp {
            let saved = saved.context("Tier-2 XMP has no durable source intent")?;
            ensure!(
                inspect_absence(&self.marker.src_dir, source, saved)?.1
                    && pending.len() == 1
                    && pending
                        .iter()
                        .all(|(rel, entry)| saved.get(rel) == Some(entry)),
                "Tier-2 XMP no longer matches its original intent"
            );
        } else {
            validate_entries(
                self.marker,
                asset,
                pending,
                admission(self.admissions.as_ref(), source)?,
            )?;
            if let Some(saved) = saved {
                ensure!(
                    saved == pending,
                    "Tier-2 source or XMP differs from its original cleanup intent"
                );
            } else {
                for (rel, entry) in pending {
                    tx.execute(
                        "INSERT INTO fast_img_tier2_cleanup(task_key,source_rel,entry_rel,kind,source_blake3,
                            asset_blake3,library_blake3,entry_blake3,entry_claim,state,evidence_blake3)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'planned',?10)",
                        params![self.key, source, rel, entry.kind, entry.source, entry.asset, entry.library,
                            entry.hash, entry.claim, entry_digest(&self.key, source, rel, entry)?.as_bytes().as_slice()],
                    )?;
                }
            }
        }
        for (rel, entry) in pending {
            let persisted: Vec<u8> = tx.query_row(
                "SELECT evidence_blake3 FROM fast_img_tier2_cleanup WHERE task_key=?1 AND source_rel=?2 AND entry_rel=?3",
                params![self.key, source, rel], |row| row.get(0),
            )?;
            ensure!(
                persisted == entry_digest(&self.key, source, rel, entry)?.as_bytes(),
                "Tier-2 cleanup intent changed before unlink"
            );
        }
        tx.commit()
            .context("Commit Tier-2 cleanup intents before unlink")?;
        if saved.is_none() {
            self.entries.insert(source.clone(), pending.clone());
        }
        self.ready = Some((source.clone(), self.pending.keys().cloned().collect()));
        self.pending.clear();
        self.pending_source = None;
        Ok(())
    }

    fn record_unlink(&mut self, source: &str, rel: &str, kind: &str, hash: &str) -> Result<()> {
        let entry = self
            .entries
            .get_mut(source)
            .and_then(|entries| entries.get_mut(rel))
            .context("Tier-2 unlink has no committed intent")?;
        let (ready_source, ready_entries) =
            self.ready.as_mut().context("Tier-2 unlink was not ready")?;
        ensure!(
            ready_source == source
                && ready_entries.contains(rel)
                && entry.kind == kind
                && !entry.removed
                && entry.hash == hash,
            "Tier-2 unlink differs from its committed intent"
        );
        ensure!(
            !cleanup::exists(&self.marker.src_dir, rel)?,
            "Tier-2 unlink path reappeared before recording its outcome"
        );
        crate::io_utils::sync_parent_directory(&self.marker.src_dir.join(rel))?;
        let prior = entry_digest(&self.key, source, rel, entry)?;
        let mut removed = entry.clone();
        removed.removed = true;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        cleanup::require_revision(&tx, &self.key, self.revision)?;
        ensure!(tx.execute(
            "UPDATE fast_img_tier2_cleanup SET state='removed', evidence_blake3=?4
             WHERE task_key=?1 AND source_rel=?2 AND entry_rel=?3 AND state='planned' AND evidence_blake3=?5",
            params![self.key, source, rel, entry_digest(&self.key, source, rel, &removed)?.as_bytes().as_slice(),
                prior.as_bytes().as_slice()],
        )? == 1, "Committed Tier-2 cleanup intent changed before recording unlink");
        tx.commit().context("Commit observed Tier-2 unlink")?;
        *entry = removed;
        ready_entries.remove(rel);
        Ok(())
    }
}

fn to_img_error(error: anyhow::Error) -> ImgQualityError {
    ImgQualityError::AnalysisError(format!("{error:#}"))
}

impl Tier2CleanupObserver for Tier2CleanupJournal<'_> {
    fn observe(
        &mut self,
        asset: &LibraryAssetRecord,
        event: SourceUnlinkEvent,
        kind: SourceUnlinkKind,
        path: &Path,
        hash: &str,
    ) -> crate::unified_error::Result<()> {
        (|| -> Result<()> {
            self.validate_asset(asset)?;
            let rel = crate::pipeline::verification::fast_img_relative_key(path, &self.marker.src_dir, "Tier-2 cleanup entry")
                .map_err(anyhow::Error::msg)?;
            let source = &asset.rel_path;
            let entry_kind = match kind { SourceUnlinkKind::Media => "media", SourceUnlinkKind::Xmp => "xmp" };
            ensure!((kind == SourceUnlinkKind::Media) == (&rel == source),
                "Tier-2 cleanup event role does not match its source path");
            match event {
                SourceUnlinkEvent::Intent => {
                    ensure!(self.pending_source.as_ref().is_none_or(|pending| pending == source)
                        && self.ready.as_ref().is_none_or(|(_, ready)| ready.is_empty()),
                        "Tier-2 cleanup intents overlap another source operation");
                    ensure!(cleanup::exists(&self.marker.src_dir, &rel)?, "Tier-2 intent requires a current regular file");
                    let observed = DiscoveredSource::from_metadata(path, &fs::symlink_metadata(path)?)?;
                    ensure!(observed.content_blake3() == hash, "Tier-2 entry changed before recording intent");
                    let entry = Entry { kind: entry_kind.into(), source: asset.blake3.clone(), asset: asset_digest(self.marker, asset)?,
                        library: library_hash(asset)?.to_owned(), hash: hash.to_owned(), claim: PlannedSource::encode(&observed)?, removed: false };
                    ensure!(self.pending.insert(rel, entry).is_none(), "Tier-2 cleanup repeated an entry intent");
                    self.pending_source = Some(source.clone());
                }
                SourceUnlinkEvent::Ready => {
                    ensure!(self.pending_source.as_ref() == Some(source)
                        && self.pending.get(&rel).is_some_and(|entry| entry.hash == hash && entry.kind == entry_kind),
                        "Tier-2 cleanup readiness does not match the current intent");
                    self.begin(asset, kind)?;
                }
                SourceUnlinkEvent::Removed => self.record_unlink(source, &rel, entry_kind, hash)?,
            }
            Ok(())
        })().map_err(to_img_error)
    }

    fn observed_absent(
        &mut self,
        asset: &LibraryAssetRecord,
        source: &Path,
    ) -> crate::unified_error::Result<(bool, bool)> {
        (|| -> Result<(bool, bool)> {
            self.validate_asset(asset)?;
            ensure!(
                source.as_os_str() == self.marker.src_dir.join(&asset.rel_path).as_os_str(),
                "Absent Tier-2 source path differs from its Photos receipt"
            );
            let saved = self.entries.get(&asset.rel_path).context(
                "Absent Tier-2 source has no durable cleanup intent; Photos proof retained",
            )?;
            inspect_absence(&self.marker.src_dir, &asset.rel_path, saved)
        })()
        .map_err(to_img_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::mfb_sqlite_store::set_test_store_path_for_tests;
    use SourceUnlinkEvent::{Intent, Ready, Removed};
    use SourceUnlinkKind::{Media, Xmp};

    #[test]
    fn tier2_journal_cold_resume_retains_replacements_and_distinguishes_unknown_unlinks()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let _store = set_test_store_path_for_tests(temp.path().join("state.sqlite"));
        let root = temp.path().canonicalize()?;
        let mut marker = WorkingCopyMarker::new(root.join("input"), root.join("output"), 0);
        let rel = Path::new("nested").join("photo.webp");
        let rel = text(&rel)?;
        let media = marker.src_dir.join(rel);
        let sidecar = media.with_extension("xmp");
        fs::create_dir_all(media.parent().unwrap())?;
        fs::create_dir(&marker.working_copy)?;
        fs::write(&media, b"synthetic original")?;
        fs::write(&sidecar, b"<xmp/>")?;
        let source_hash = blake3::hash(&fs::read(&media)?).to_hex().to_string();
        let xmp_hash = blake3::hash(&fs::read(&sidecar)?).to_hex().to_string();
        let asset = LibraryAssetRecord {
            rel_path: rel.into(),
            blake3: source_hash.clone(),
            sync_status: "local".into(),
            photos_uuid: Some("synthetic-original-uuid".into()),
            library_blake3: Some(
                blake3::hash(b"synthetic enriched original")
                    .to_hex()
                    .to_string(),
            ),
            xmp_sidecar_blake3: Some(xmp_hash.clone()),
            ..Default::default()
        };
        marker.tier2_cleanup_journal = true;
        marker.tier2_in_progress = true;
        marker.stage = FastImgStageName::CleanupComplete;
        marker.photos_library_path = Some(root.join("Synthetic.photoslibrary"));
        marker.tier2_imported_assets = vec![asset.clone()];
        write_marker(&marker)?;
        let key = key_for_canonical(text(&marker.working_copy)?);
        let conn = open_store_connection()?;
        assert!(!finish_digest(&conn, &key, &marker)?.unwrap().1);
        fs::rename(&media, media.with_extension("held"))?;
        assert!(validate_tier2_cleanup_resume(&marker).is_err());
        fs::rename(media.with_extension("held"), &media)?;
        let mut terminal = marker.clone();
        terminal.tier2_in_progress = false;
        assert!(write_marker(&terminal).is_err());

        {
            let mut journal = Tier2CleanupJournal::open(&marker)?;
            journal.observe(&asset, Intent, Xmp, &sidecar, &xmp_hash)?;
            journal.observe(&asset, Intent, Media, &media, &source_hash)?;
            journal.observe(&asset, Ready, Media, &media, &source_hash)?;
            fs::remove_file(&media)?;
            // Simulate losing power before recording the successful media unlink.
        }
        let saved = read_marker(&marker.working_copy)?;
        assert_eq!(
            validate_tier2_cleanup_resume(&saved)?,
            BTreeSet::from([rel.to_owned()])
        );
        {
            let mut journal = Tier2CleanupJournal::open(&saved)?;
            assert_eq!(journal.observed_absent(&asset, &media)?, (false, true));
            journal.observe(&asset, Intent, Xmp, &sidecar, &xmp_hash)?;
            journal.observe(&asset, Ready, Xmp, &sidecar, &xmp_hash)?;
            fs::remove_file(&sidecar)?;
            journal.observe(&asset, Removed, Xmp, &sidecar, &xmp_hash)?;
        }
        let entries = read_entries(&conn, &key, &saved)?;
        let rows = &entries[rel];
        assert!(!rows[rel].removed);
        assert!(
            rows.iter()
                .any(|(_, entry)| entry.kind == "xmp" && entry.removed)
        );
        assert!(finish_digest(&conn, &key, &saved)?.unwrap().1);
        write_marker(&terminal)?;
        verify_tier2_cleanup_complete(&terminal)?;
        fs::write(&sidecar, b"<xmp/>")?;
        assert!(validate_tier2_cleanup_resume(&terminal).is_err());
        assert!(verify_tier2_cleanup_complete(&terminal).is_err());
        assert_eq!(fs::read(&sidecar)?, b"<xmp/>");
        fs::remove_file(&sidecar)?;
        fs::remove_dir(media.parent().unwrap())?;
        assert_eq!(
            validate_tier2_cleanup_resume(&terminal)?,
            BTreeSet::from([rel.to_owned()])
        );
        verify_tier2_cleanup_complete(&terminal)?;
        let mut changed = terminal;
        changed.tier2_imported_assets[0].photos_uuid = Some("different-uuid".into());
        assert!(write_marker(&changed).is_err());
        Ok(())
    }
}
