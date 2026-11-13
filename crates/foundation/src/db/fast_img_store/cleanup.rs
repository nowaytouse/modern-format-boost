//! Durable primary unlink intents and observations for one committed Fast IMG task.

use super::*;
use crate::batch::{DiscoveredSource, planned_source::PlannedSource};
use crate::image::fast_img::{SourceUnlinkEvent, SourceUnlinkKind};
use crate::pipeline::verification::{
    FastImgStageName, gate1_complete_or_later, validate_marker_relative_path,
    validate_no_symlink_components,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Entry {
    rel: String,
    kind: &'static str,
    source: String,
    output_rel: String,
    output: String,
    hash: String,
    claim: String,
    removed: bool,
}

type Entries = BTreeMap<(String, String), Entry>;

fn child_admissions(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<Option<BTreeMap<String, FailedRetryAdmission>>> {
    if marker.primary_cleanup_journal && marker.failed_retry_manifest_blake3.is_some() {
        return retry::admissions(conn, key).map(Some);
    }
    Ok(None)
}

fn validate_child_entries(
    source: &str,
    entries: &Entries,
    admissions: Option<&BTreeMap<String, FailedRetryAdmission>>,
) -> Result<()> {
    let Some(admissions) = admissions else {
        return Ok(());
    };
    let admission = admissions
        .get(source)
        .context("Cleanup source is outside the failed-only admission")?;
    let media = entries
        .get(&(source.to_owned(), source.to_owned()))
        .context("Failed-only cleanup has no media intent")?;
    ensure!(
        media.hash == admission.admitted_source_blake3 && media.claim == admission.claim,
        "Failed-only cleanup media differs from the immutable admission"
    );
    let mut companions = entries
        .range((source.to_owned(), String::new())..)
        .take_while(|((saved, _), _)| saved == source)
        .filter_map(|(_, entry)| (entry.kind == "xmp").then_some(entry));
    match (&admission.xmp, companions.next()) {
        (FailedRetrySidecar::Absent, None) => Ok(()),
        (
            FailedRetrySidecar::Present {
                relative_path,
                blake3,
                claim,
            },
            Some(entry),
        ) => {
            ensure!(
                &entry.rel == relative_path
                    && &entry.hash == blake3
                    && &entry.claim == claim
                    && companions.next().is_none(),
                "Failed-only cleanup XMP differs from the immutable admission"
            );
            Ok(())
        }
        _ => bail!("Failed-only cleanup XMP presence differs from the immutable admission"),
    }
}

pub(super) fn hash_is_valid(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_entry(marker: &WorkingCopyMarker, source: &str, entry: &Entry) -> Result<()> {
    for rel in [source, &entry.rel, &entry.output_rel] {
        validate_marker_relative_path("cleanup path", rel).map_err(anyhow::Error::msg)?;
    }
    let expected = marker
        .blake3_log
        .get(source)
        .context("Primary cleanup journal has no converted source outcome")?;
    ensure!(
        expected.src == entry.source
            && expected.out == entry.output
            && expected.out_rel.as_deref() == Some(&entry.output_rel)
            && [&entry.source, &entry.output, &entry.hash]
                .into_iter()
                .all(|hash| hash_is_valid(hash)),
        "Primary cleanup journal does not match the source and output receipt"
    );
    let path = Path::new(&entry.rel);
    match entry.kind {
        "media" => ensure!(
            entry.rel == source && entry.hash == entry.source,
            "Primary cleanup media identity is inconsistent"
        ),
        "xmp" => ensure!(
            entry.rel != source
                && path.parent() == Path::new(source).parent()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("xmp")),
            "Primary cleanup companion is not an adjacent XMP"
        ),
        _ => bail!("Unrecognized primary cleanup entry kind"),
    }
    PlannedSource::validate_identity(&entry.claim, &marker.src_dir.join(path), &entry.hash)
}

fn digest(key: &str, source: &str, entry: &Entry) -> Result<blake3::Hash> {
    Ok(blake3::hash(&canonical_bytes(&(key, source, entry))?))
}

fn read_entries(conn: &Connection, key: &str, marker: &WorkingCopyMarker) -> Result<Entries> {
    let mut entries = BTreeMap::new();
    if !matches!(inspect_store_schema_version(conn)?, Some(6..)) {
        ensure!(
            !marker.primary_cleanup_journal,
            "Primary cleanup journal schema is unavailable"
        );
        return Ok(entries);
    }
    let mut statement = conn.prepare(
        "SELECT source_rel,entry_rel,kind,source_blake3,output_rel,output_blake3,
                entry_blake3,entry_claim,state,evidence_blake3
         FROM fast_img_primary_cleanup WHERE task_key=?1 ORDER BY source_rel,entry_rel",
    )?;
    let mut rows = statement.query([key])?;
    while let Some(row) = rows.next()? {
        ensure!(
            marker.primary_cleanup_journal,
            "Primary cleanup evidence has no journal owner"
        );
        let source: String = row.get(0)?;
        let entry = Entry {
            rel: row.get(1)?,
            kind: match row.get::<_, String>(2)?.as_str() {
                "media" => "media",
                "xmp" => "xmp",
                _ => bail!("Unrecognized primary cleanup entry kind"),
            },
            source: row.get(3)?,
            output_rel: row.get(4)?,
            output: row.get(5)?,
            hash: row.get(6)?,
            claim: row.get(7)?,
            removed: match row.get::<_, String>(8)?.as_str() {
                "planned" => false,
                "removed" => true,
                _ => bail!("Unrecognized primary cleanup entry state"),
            },
        };
        validate_entry(marker, &source, &entry)?;
        ensure!(
            digest(key, &source, &entry)?.as_bytes().as_slice() == row.get::<_, Vec<u8>>(9)?,
            "Primary cleanup evidence failed BLAKE3 verification"
        );
        entries.insert((source, entry.rel.clone()), entry);
    }
    let admissions = if entries.is_empty() {
        None
    } else {
        child_admissions(conn, key, marker)?
    };
    for ((source, _), entry) in &entries {
        if entry.kind == "xmp" {
            ensure!(
                entries.contains_key(&(source.clone(), source.clone())),
                "Primary cleanup XMP has no media intent"
            );
        } else {
            validate_child_entries(source, &entries, admissions.as_ref())?;
        }
    }
    if marker.primary_cleanup_journal && marker.stage == FastImgStageName::CleanupComplete {
        ensure!(
            marker
                .blake3_log
                .keys()
                .all(|rel| entries.contains_key(&(rel.clone(), rel.clone()))),
            "Primary cleanup completion lacks a recorded intent for a converted source"
        );
    }
    Ok(entries)
}

/// Bind all journal rows and their states to missing-root finalization.
pub(super) fn finish_digest(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<Option<(String, bool)>> {
    if !marker.primary_cleanup_journal {
        return Ok(None);
    }
    let entries = read_entries(conn, key, marker)?;
    let complete = marker
        .blake3_log
        .keys()
        .all(|rel| entries.contains_key(&(rel.clone(), rel.clone())));
    let mut hash = blake3::Hasher::new_derive_key("MFB failed-only finish primary cleanup v1");
    for ((source, _), entry) in &entries {
        hash.update(digest(key, source, entry)?.as_bytes());
    }
    Ok(Some((hash.finalize().to_hex().to_string(), complete)))
}

pub(super) fn validate_rows(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<()> {
    read_entries(conn, key, marker).map(|_| ())
}

pub(super) fn reject_missing_task_evidence(conn: &Connection, key: &str) -> Result<()> {
    if matches!(inspect_store_schema_version(conn)?, Some(6..)) {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM fast_img_primary_cleanup WHERE task_key=?1)",
            [key],
            |row| row.get(0),
        )?;
        ensure!(
            !exists,
            "Missing Fast IMG task still has protected primary cleanup evidence; state is not absent"
        );
    }
    Ok(())
}

pub(super) fn row_count(conn: &Connection) -> Result<usize> {
    if !matches!(inspect_store_schema_version(conn)?, Some(6..)) {
        return Ok(0);
    }
    let rows: i64 = conn.query_row("SELECT count(*) FROM fast_img_primary_cleanup", [], |row| {
        row.get(0)
    })?;
    Ok(usize::try_from(rows)?)
}

pub(super) fn exists(root: &Path, rel: &str) -> Result<bool> {
    validate_no_symlink_components(root, Path::new(rel)).map_err(anyhow::Error::msg)?;
    match std::fs::symlink_metadata(root.join(rel)) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "Source cleanup entry is not a regular file: {rel}"
            );
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect source cleanup entry {rel}")),
    }
}

/// Cross-check saved intents against the current source scope before resuming.
/// Absence after an intent is permitted, but it does not certify a successful unlink.
pub fn validate_primary_cleanup_resume(
    marker: &WorkingCopyMarker,
    current_sources: &BTreeMap<String, String>,
) -> Result<bool> {
    if !marker.primary_cleanup_journal {
        return Ok(false);
    }
    let entries = load_entries(marker)?;
    validate_current_sources(marker, &entries, current_sources)?;
    Ok(!entries.is_empty())
}

fn load_entries(marker: &WorkingCopyMarker) -> Result<Entries> {
    let conn = open_readonly()?.context("Primary cleanup state is unavailable")?;
    let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
    in_snapshot(&conn, |conn| read_entries(conn, &key, marker))
}

fn validate_current_sources(
    marker: &WorkingCopyMarker,
    entries: &Entries,
    current_sources: &BTreeMap<String, String>,
) -> Result<()> {
    for (rel, entry) in &marker.blake3_log {
        if !current_sources.contains_key(rel) {
            ensure!(
                entries.contains_key(&(rel.clone(), rel.clone())) && !exists(&marker.src_dir, rel)?,
                "Primary source is missing without a matching cleanup intent: {rel}; saved output retained"
            );
        } else {
            ensure!(
                current_sources.get(rel) == Some(&entry.src),
                "Primary cleanup source bytes changed: {rel}"
            );
        }
    }
    for ((_, rel), entry) in entries {
        if exists(&marker.src_dir, rel)? {
            ensure!(
                !entry.removed,
                "Previously removed primary cleanup entry reappeared: {rel}"
            );
            PlannedSource::verify(&entry.claim, &marker.src_dir.join(rel))
                .context("Primary cleanup entry changed since its durable intent")?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub enum PrimaryCleanupScope {
    MediaAndCompanion,
    RemainingCompanion,
}

/// A single locked cleanup attempt. SQLite writes only the affected entry rows.
pub struct PrimaryCleanupJournal<'a> {
    conn: Connection,
    marker: &'a WorkingCopyMarker,
    key: String,
    revision: i64,
    entries: Entries,
    admissions: Option<BTreeMap<String, FailedRetryAdmission>>,
}

impl<'a> PrimaryCleanupJournal<'a> {
    pub fn open(marker: &'a WorkingCopyMarker) -> Result<Self> {
        ensure!(
            marker.primary_cleanup_journal && gate1_complete_or_later(&marker.stage),
            "Primary cleanup requires a committed journal and delivery checkpoint"
        );
        let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
        let conn = open_store_connection()?;
        let (revision, entries, admissions) = in_snapshot(&conn, |conn| {
            let saved =
                read_snapshot(conn, &key)?.context("Primary cleanup task was not committed")?;
            ensure!(
                &saved == marker,
                "Primary cleanup checkpoint changed before journal admission"
            );
            let revision = conn.query_row(
                "SELECT revision FROM fast_img_tasks WHERE task_key=?1",
                [&key],
                |row| row.get(0),
            )?;
            Ok((
                revision,
                read_entries(conn, &key, marker)?,
                child_admissions(conn, &key, marker)?,
            ))
        })?;
        Ok(Self {
            conn,
            marker,
            key,
            revision,
            entries,
            admissions,
        })
    }

    /// Check an absent source and distinguish a committed observation from an intent.
    pub fn observed_absent(&self, source: &Path) -> Result<(bool, bool)> {
        let rel = text(source.strip_prefix(&self.marker.src_dir)?)?;
        ensure!(
            !exists(&self.marker.src_dir, rel)?,
            "Primary cleanup source reappeared: {rel}"
        );
        let media = self
            .entries
            .get(&(rel.to_owned(), rel.to_owned()))
            .context(
                "Absent primary source has no matching cleanup intent; saved output retained",
            )?;
        let companion = self
            .entries
            .range((rel.to_owned(), String::new())..)
            .take_while(|((saved, _), _)| saved == rel)
            .find(|(_, entry)| entry.kind == "xmp");
        let needs_companion_cleanup = companion
            .map(|(_, entry)| exists(&self.marker.src_dir, &entry.rel))
            .transpose()?
            .unwrap_or(false);
        Ok((media.removed, needs_companion_cleanup))
    }

    /// Bind exactly the file and optional XMP offered by the live deletion gates.
    pub fn observe(
        &mut self,
        source: &Path,
        scope: PrimaryCleanupScope,
    ) -> Result<
        impl FnMut(SourceUnlinkEvent, SourceUnlinkKind, &Path, &str) -> crate::unified_error::Result<()>
        + '_,
    > {
        let source = text(source.strip_prefix(&self.marker.src_dir)?)?.to_owned();
        let mut pending = BTreeMap::new();
        Ok(move |event, kind, path: &Path, hash: &str| {
            let result = (|| -> Result<()> {
                let rel = text(path.strip_prefix(&self.marker.src_dir)?)?.to_owned();
                let key = (source.clone(), rel.clone());
                match event {
                    SourceUnlinkEvent::Intent => {
                        let entry = self.entry(&source, &rel, kind, path, hash)?;
                        ensure!(
                            pending.insert(key, entry).is_none(),
                            "Primary cleanup repeated an entry intent"
                        );
                    }
                    SourceUnlinkEvent::Ready => {
                        self.begin(&source, scope, &pending)?;
                        for entry in pending.values() {
                            self.verify_entry_location(entry)?;
                        }
                    }
                    SourceUnlinkEvent::Removed => self.record_unlink(key, hash, path)?,
                }
                Ok(())
            })();
            result.map_err(|error| {
                crate::unified_error::ImgQualityError::AnalysisError(format!("{error:#}"))
            })
        })
    }

    fn entry(
        &self,
        source: &str,
        rel: &str,
        kind: SourceUnlinkKind,
        path: &Path,
        hash: &str,
    ) -> Result<Entry> {
        let outcome = self
            .marker
            .blake3_log
            .get(source)
            .context("Primary cleanup has no source outcome")?;
        let kind = match kind {
            SourceUnlinkKind::Media => "media",
            SourceUnlinkKind::Xmp => "xmp",
        };
        validate_no_symlink_components(&self.marker.src_dir, Path::new(rel))
            .map_err(anyhow::Error::msg)?;
        let observed = DiscoveredSource::from_metadata(path, &std::fs::symlink_metadata(path)?)?;
        ensure!(
            observed.content_blake3() == hash,
            "Primary cleanup entry changed before recording intent"
        );
        let entry = Entry {
            rel: rel.to_owned(),
            kind,
            source: outcome.src.clone(),
            output_rel: outcome
                .out_rel
                .clone()
                .context("Primary cleanup needs an explicit output path")?,
            output: outcome.out.clone(),
            hash: hash.to_owned(),
            claim: PlannedSource::encode(&observed)?,
            removed: false,
        };
        validate_entry(self.marker, source, &entry)?;
        self.verify_entry_location(&entry)?;
        Ok(entry)
    }

    fn verify_entry_location(&self, entry: &Entry) -> Result<()> {
        for (root, rel) in [
            (&self.marker.src_dir, entry.rel.as_str()),
            (&self.marker.working_copy, entry.output_rel.as_str()),
        ] {
            validate_no_symlink_components(root, Path::new(rel)).map_err(anyhow::Error::msg)?;
            let path = root.join(rel);
            ensure!(
                path.canonicalize()?.as_os_str() == path.as_os_str(),
                "Primary cleanup entry no longer resolves at its saved path"
            );
        }
        Ok(())
    }

    fn begin(&mut self, source: &str, scope: PrimaryCleanupScope, pending: &Entries) -> Result<()> {
        let mut existing = self
            .entries
            .range((source.to_owned(), String::new())..)
            .take_while(|((saved, _), _)| saved == source);
        match scope {
            PrimaryCleanupScope::RemainingCompanion => {
                self.entries
                    .get(&(source.to_owned(), source.to_owned()))
                    .context("Absent media has no durable cleanup intent")?;
                ensure!(
                    !exists(&self.marker.src_dir, source)?,
                    "Primary cleanup media is no longer absent"
                );
                ensure!(
                    pending.len() == 1
                        && pending.iter().all(|(key, entry)| {
                            entry.kind == "xmp" && self.entries.get(key) == Some(entry)
                        }),
                    "Absent media has no matching original XMP cleanup intent; companion retained"
                );
            }
            PrimaryCleanupScope::MediaAndCompanion => {
                ensure!(
                    pending.contains_key(&(source.to_owned(), source.to_owned())),
                    "Primary cleanup intent omitted its media"
                );
                validate_child_entries(source, pending, self.admissions.as_ref())?;
                if let Some(first) = existing.next() {
                    ensure!(
                        std::iter::once(first).chain(existing).eq(pending.iter()),
                        "Primary cleanup media or XMP differs from its original intent"
                    );
                } else {
                    let tx = self
                        .conn
                        .transaction_with_behavior(TransactionBehavior::Immediate)?;
                    require_revision(&tx, &self.key, self.revision)?;
                    for ((source, _), entry) in pending {
                        tx.execute(
                            "INSERT INTO fast_img_primary_cleanup(task_key,source_rel,entry_rel,kind,
                                source_blake3,output_rel,output_blake3,entry_blake3,entry_claim,state,evidence_blake3)
                             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'planned',?10)",
                            params![self.key, source, entry.rel, entry.kind, entry.source, entry.output_rel,
                                entry.output, entry.hash, entry.claim, digest(&self.key, source, entry)?.as_bytes().as_slice()],
                        )?;
                    }
                    tx.commit()
                        .context("Commit primary cleanup intents before unlink")?;
                    self.entries.extend(pending.clone());
                    return Ok(());
                }
            }
        }
        require_revision(&self.conn, &self.key, self.revision)
    }

    fn record_unlink(&mut self, key: (String, String), hash: &str, path: &Path) -> Result<()> {
        let entry = self
            .entries
            .get(&key)
            .context("Primary unlink has no committed intent")?;
        ensure!(
            !entry.removed && entry.hash == hash,
            "Primary unlink differs from its committed intent"
        );
        crate::io_utils::sync_parent_directory(path)?;
        let mut removed = entry.clone();
        removed.removed = true;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_revision(&tx, &self.key, self.revision)?;
        ensure!(tx.execute(
            "UPDATE fast_img_primary_cleanup SET state='removed', evidence_blake3=?4
             WHERE task_key=?1 AND source_rel=?2 AND entry_rel=?3 AND state='planned' AND evidence_blake3=?5",
            params![self.key, key.0, key.1, digest(&self.key, &key.0, &removed)?.as_bytes().as_slice(),
                digest(&self.key, &key.0, entry)?.as_bytes().as_slice()],
        )? == 1, "Committed primary cleanup intent changed before recording unlink");
        tx.commit().context("Commit observed primary unlink")?;
        self.entries.insert(key, removed);
        Ok(())
    }
}

pub(super) fn require_revision(conn: &Connection, key: &str, expected: i64) -> Result<()> {
    let current: i64 = conn.query_row(
        "SELECT revision FROM fast_img_tasks WHERE task_key=?1",
        [key],
        |row| row.get(0),
    )?;
    ensure!(
        current == expected,
        "Source cleanup checkpoint changed during this attempt"
    );
    super::retry::require_parent_unmodified(conn, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::mfb_sqlite_store::set_test_store_path_for_tests;

    #[test]
    fn primary_cleanup_orphan_blocks_task_absence_and_replacement() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let _store = set_test_store_path_for_tests(temp.path().join("state.sqlite"));
        let root = temp.path().canonicalize()?;
        let marker = WorkingCopyMarker::new(root.join("source"), root.join("output"), 0);
        let key = key_for_canonical(text(&marker.working_copy)?);
        let conn = open_store_connection()?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        conn.execute(
            "INSERT INTO fast_img_primary_cleanup VALUES (
                ?1,'source.jpg','source.jpg','media','src','source.JXL','out','src','claim',
                'planned',zeroblob(32))",
            [&key],
        )?;
        assert_eq!(
            read_marker(&marker.working_copy).unwrap_err().kind(),
            io::ErrorKind::Other,
        );
        assert!(super::super::is_reserved(&marker.working_copy).is_err());
        assert!(write_marker(&marker).is_err());
        assert!(super::super::inspect_integrity(&conn).is_err());
        assert_eq!(row_count(&conn)?, 1);
        let tasks: i64 =
            conn.query_row("SELECT count(*) FROM fast_img_tasks", [], |row| row.get(0))?;
        assert_eq!(tasks, 0);
        Ok(())
    }

    #[test]
    fn primary_cleanup_cold_resume_distinguishes_intent_from_observed_unlink() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let _store = set_test_store_path_for_tests(temp.path().join("state.sqlite"));
        let root = temp.path().canonicalize()?;
        let input = root.join("input");
        let output = root.join("output");
        std::fs::create_dir(&input)?;
        std::fs::create_dir(&output)?;
        let a = input.join("a.jpg");
        let b = input.join("b.jpg");
        let xmp = input.join("a.xmp");
        for path in [&a, &b, &xmp] {
            std::fs::write(path, path.file_name().unwrap().as_encoded_bytes())?;
        }
        let hashes = |path: &Path| -> Result<String> {
            Ok(blake3::hash(&std::fs::read(path)?).to_hex().to_string())
        };
        let a_hash = hashes(&a)?;
        let b_hash = hashes(&b)?;
        let xmp_hash = hashes(&xmp)?;
        let out_hash = blake3::hash(b"output").to_hex().to_string();
        let mut marker = WorkingCopyMarker::new(input, output, 2);
        marker.primary_cleanup_journal = true;
        marker.stage = FastImgStageName::Gate1Passed;
        for (rel, hash) in [("a.jpg", &a_hash), ("b.jpg", &b_hash)] {
            let out_rel = Path::new(rel).with_extension("JXL");
            std::fs::write(marker.working_copy.join(&out_rel), b"output")?;
            marker.blake3_log.insert(
                rel.into(),
                Blake3Entry {
                    out_rel: Some(out_rel.to_str().unwrap().to_owned()),
                    src: hash.clone(),
                    out: out_hash.clone(),
                    library_asset: None,
                },
            );
        }
        marker.encoded_count = 2;
        write_marker(&marker)?;
        let current = BTreeMap::from([("b.jpg".into(), b_hash)]);
        assert!(validate_primary_cleanup_resume(&marker, &current).is_err());
        {
            let mut journal = PrimaryCleanupJournal::open(&marker)?;
            let mut observe = journal.observe(&a, PrimaryCleanupScope::MediaAndCompanion)?;
            observe(
                SourceUnlinkEvent::Intent,
                SourceUnlinkKind::Xmp,
                &xmp,
                &xmp_hash,
            )?;
            observe(
                SourceUnlinkEvent::Intent,
                SourceUnlinkKind::Media,
                &a,
                &a_hash,
            )?;
            observe(
                SourceUnlinkEvent::Ready,
                SourceUnlinkKind::Media,
                &a,
                &a_hash,
            )?;
            // Simulate process exit after the unlink and before its observation commits.
            std::fs::remove_file(&a)?;
        }
        let saved = read_marker(&marker.working_copy)?;
        validate_primary_cleanup_resume(&saved, &current)?;
        {
            let mut journal = PrimaryCleanupJournal::open(&saved)?;
            assert_eq!(journal.observed_absent(&a)?, (false, true));
            let mut observe = journal.observe(&a, PrimaryCleanupScope::RemainingCompanion)?;
            observe(
                SourceUnlinkEvent::Intent,
                SourceUnlinkKind::Xmp,
                &xmp,
                &xmp_hash,
            )?;
            observe(
                SourceUnlinkEvent::Ready,
                SourceUnlinkKind::Xmp,
                &xmp,
                &xmp_hash,
            )?;
            std::fs::remove_file(&xmp)?;
            observe(
                SourceUnlinkEvent::Removed,
                SourceUnlinkKind::Xmp,
                &xmp,
                &xmp_hash,
            )?;
        }
        let conn = open_store_connection()?;
        let key = key_for_canonical(&canonical_text(&marker.working_copy)?);
        let evidence = read_entries(&conn, &key, &saved)?;
        assert!(!evidence[&("a.jpg".into(), "a.jpg".into())].removed);
        assert!(evidence[&("a.jpg".into(), "a.xmp".into())].removed);
        std::fs::remove_file(&b)?;
        assert!(validate_primary_cleanup_resume(&saved, &BTreeMap::new()).is_err());
        conn.execute(
            "UPDATE fast_img_primary_cleanup SET state='removed' WHERE entry_rel='a.jpg'",
            [],
        )?;
        assert!(read_marker(&marker.working_copy).is_err());
        Ok(())
    }
}
