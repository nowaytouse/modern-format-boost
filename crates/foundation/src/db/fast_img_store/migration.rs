//! Named, one-time import of legacy checkpoints. Runtime readers use SQL only.

use super::{
    MAX_LEGACY_BYTES, MigrationReport, canonical_text, current_family, key_for_canonical,
    open_readonly, read_snapshot, text, write_snapshot,
};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::metadata::SourceDirectoryCleanupSnapshot;
use crate::pipeline::verification::{
    FAST_IMG_MARKER_SCHEMA_VERSION, WorkingCopyMarker, parse_marker_for_working_copy,
    validate_marker_structure,
};

#[derive(Deserialize)]
struct Locator {
    schema: u8,
    src_dir: PathBuf,
    working_copy: PathBuf,
}

fn locator(bytes: &[u8]) -> Result<Locator> {
    crate::common_utils::validate_unique_json_members(bytes)?;
    let value: Locator = serde_json::from_slice(bytes)?;
    ensure!(
        value.schema == FAST_IMG_MARKER_SCHEMA_VERSION,
        "Unknown legacy Fast IMG schema {}; checkpoint retained",
        value.schema
    );
    for path in [&value.src_dir, &value.working_copy] {
        ensure!(
            path.is_absolute() && path.file_name().is_some() && !text(path)?.contains('\0'),
            "Legacy checkpoint has a relative, root, or invalid path; historical CWD was not guessed"
        );
    }
    Ok(value)
}

fn marker(bytes: &[u8]) -> Result<WorkingCopyMarker> {
    let _ = locator(bytes)?;
    let value: WorkingCopyMarker = serde_json::from_slice(bytes)?;
    validate_marker_structure(&value)?;
    Ok(value)
}

/// Match a saved locator without requiring unrelated, vanished media trees to
/// remain mounted. This is discovery only; no historical result is certified.
fn locator_matches(saved: &Path, selected: &Path) -> Result<bool> {
    super::validate_locator(text(saved)?)?;
    match crate::pipeline::verification::canonical_working_copy_path(saved) {
        Ok(resolved) => return Ok(resolved == selected),
        Err(error) if error.kind() == io::ErrorKind::NotFound && saved != selected => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Cannot resolve legacy task locator {}", saved.display())
            });
        }
    }
    // ENOENT can also mean a dangling link. Only an ordinary absent subtree
    // may be excluded; links, inaccessible ancestors and replaced locators
    // still require an explicit error instead of a guessed task identity.
    for ancestor in saved.ancestors().skip(1) {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir()
                        && !metadata.file_type().is_symlink()
                        && ancestor.canonicalize()?.as_os_str() == ancestor.as_os_str(),
                    "Unavailable legacy task locator has a linked or non-directory ancestor: {}",
                    saved.display()
                );
                return Ok(false);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Cannot inspect legacy task ancestor {}", ancestor.display())
                });
            }
        }
    }
    bail!(
        "Legacy task locator has no verifiable ancestor: {}",
        saved.display()
    )
}

fn central_dir() -> Result<PathBuf> {
    Ok(crate::process_lock::inspect_mfb_root_path()?.join("fast_img/markers"))
}

/// The former writer hashed the absolute spelling, not the canonical path.
pub(super) fn legacy_marker_path_for_working_copy(wc: &Path) -> Result<PathBuf> {
    let absolute = if wc.is_absolute() {
        wc.to_path_buf()
    } else {
        std::env::current_dir()?.join(wc)
    };
    let key = blake3::hash(text(&absolute)?.as_bytes())
        .to_hex()
        .to_string();
    Ok(central_dir()?.join(format!("{key}.json")))
}

fn origin_path(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().context("Legacy checkpoint has no parent")?;
    Ok(PathBuf::from(canonical_text(parent)?).join(
        path.file_name()
            .context("Legacy checkpoint has no filename")?,
    ))
}

fn real_directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "Legacy state directory is not a real directory: {}",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("Cannot inspect legacy state directory {}", path.display())),
    }
}

fn central_files() -> Result<Vec<PathBuf>> {
    let directory = central_dir()?;
    if !real_directory(
        directory
            .parent()
            .context("Legacy state directory has no parent")?,
    )? || !real_directory(&directory)?
    {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl Stamp {
    fn of(meta: &fs::Metadata) -> Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            size: meta.len(),
            modified: meta.modified()?,
            #[cfg(unix)]
            device: meta.dev(),
            #[cfg(unix)]
            inode: meta.ino(),
        })
    }
}

struct LegacyFile {
    path: PathBuf,
    bytes: Vec<u8>,
    stamp: Stamp,
}
fn read_legacy(path: &Path) -> Result<Option<LegacyFile>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Cannot inspect legacy checkpoint {}", path.display()));
        }
    };
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_LEGACY_BYTES,
        "Legacy checkpoint must be a bounded, regular, non-symlink file: {}",
        path.display()
    );
    let stamp = Stamp::of(&metadata)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("Cannot read legacy checkpoint {}", path.display()))?;
    ensure!(
        Stamp::of(&file.metadata()?)? == stamp,
        "Legacy checkpoint changed before reading; retained"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_LEGACY_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        u64::try_from(bytes.len())? <= MAX_LEGACY_BYTES
            && Stamp::of(&file.metadata()?)? == stamp
            && Stamp::of(&fs::symlink_metadata(path)?)? == stamp,
        "Legacy checkpoint changed during reading; retained"
    );
    Ok(Some(LegacyFile {
        path: origin_path(path)?,
        bytes,
        stamp,
    }))
}

fn validate_origin(path: &Path, value: &Locator) -> Result<()> {
    if path.file_name().is_some_and(|name| name == ".mfb_wc") {
        ensure!(
            canonical_text(path.parent().context("Local marker has no parent")?)?
                == canonical_text(&value.working_copy)?,
            "Legacy local checkpoint belongs to another output; retained"
        );
    } else {
        ensure!(
            origin_path(&legacy_marker_path_for_working_copy(&value.working_copy)?)? == path,
            "Legacy central filename does not match its saved output identity; retained"
        );
    }
    Ok(())
}

fn local_candidates(source: &Path, requested: Option<&Path>) -> Result<Vec<PathBuf>> {
    let mut outputs = BTreeSet::new();
    if let Some(requested) = requested {
        outputs.insert(PathBuf::from(canonical_text(requested)?));
    }
    let parent = source.parent().context("Source has no parent")?;
    let name = text(Path::new(
        source
            .file_name()
            .context("Source has no final component")?,
    ))?;
    let prefix = format!("{name}_optimized");
    outputs.insert(parent.join(&prefix));
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix))
        {
            // Do not traverse a user-controlled symlink while searching for state.
            ensure!(
                !entry.file_type()?.is_symlink(),
                "Adjacent output candidate is a symlink: {}",
                entry.path().display()
            );
            if entry.file_type()?.is_dir() {
                outputs.insert(entry.path());
            }
        }
    }
    Ok(outputs
        .into_iter()
        .map(|path| path.join(".mfb_wc"))
        .collect())
}

fn migration_required(path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "MFB_STATE_MIGRATION_REQUIRED: legacy checkpoint {} requires the explicit Fast IMG state migration; no JSON was used as runtime proof",
        path.display()
    )
}

fn reject_pending(source: Option<&str>, wc: Option<&str>) -> Result<()> {
    let Some(conn) = open_readonly()? else {
        return Ok(());
    };
    super::in_snapshot(&conn, |conn| {
        if !current_family(conn)? {
            return Ok(());
        }
        let pending: Option<String> = conn.query_row(
            "SELECT m.origin_path FROM fast_img_migrations m JOIN fast_img_tasks t ON t.task_key=m.task_key
             WHERE m.retired=0 AND ((?1 IS NOT NULL AND t.source_root=?1) OR (?2 IS NOT NULL AND t.working_copy=?2)) LIMIT 1",
            params![source, wc], |row| row.get(0)).optional()?;
        if let Some(path) = pending {
            return Err(migration_required(Path::new(&path)));
        }
        Ok(())
    })
}

pub(super) fn reject_unmigrated_for_working_copy(wc: &Path) -> Result<()> {
    for path in [legacy_marker_path_for_working_copy(wc)?, wc.join(".mfb_wc")] {
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(migration_required(&path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Cannot inspect legacy checkpoint presence"),
        }
    }
    let canonical = canonical_text(wc)?;
    for path in central_files()? {
        let file = read_legacy(&path)?.context("Legacy checkpoint disappeared during discovery")?;
        let value = locator(&file.bytes)?;
        validate_origin(&file.path, &value)?;
        if locator_matches(&value.working_copy, Path::new(&canonical))? {
            return Err(migration_required(&file.path));
        }
    }
    reject_pending(None, Some(&canonical))
}

pub(super) fn reject_unmigrated_for_source(source: &Path) -> Result<()> {
    let source = PathBuf::from(canonical_text(source)?);
    for path in central_files()? {
        let file = read_legacy(&path)?.context("Legacy checkpoint disappeared during discovery")?;
        let value = locator(&file.bytes)?;
        validate_origin(&file.path, &value)?;
        if locator_matches(&value.src_dir, &source)? {
            return Err(migration_required(&file.path));
        }
    }
    for path in local_candidates(&source, None)? {
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(migration_required(&path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Cannot inspect adjacent legacy checkpoint"),
        }
    }
    reject_pending(Some(text(&source)?), None)
}

#[derive(Clone)]
struct Backup {
    path: PathBuf,
    bytes: Vec<u8>,
    hash: Vec<u8>,
    key: String,
    retired: bool,
    directory_snapshot: Option<Vec<u8>>,
    directory_snapshot_blake3: Option<Vec<u8>>,
}

fn backups(conn: &Connection) -> Result<Vec<Backup>> {
    read_backups(conn, None)
}

fn read_backups(conn: &Connection, origin: Option<&Path>) -> Result<Vec<Backup>> {
    let version = crate::db::mfb_sqlite_store::inspect_store_schema_version(conn)?;
    let snapshot_columns = match version {
        Some(3) => "NULL,NULL",
        Some(4 | 5) => {
            "CASE WHEN length(directory_snapshot)<=?1 THEN directory_snapshot ELSE NULL END,directory_snapshot_blake3"
        }
        _ => bail!("Legacy migration backup requires a recognized structured state schema"),
    };
    let filter = if origin.is_some() {
        "origin_path=?2"
    } else {
        "?2 IS NULL"
    };
    let mut query = conn.prepare(&format!("SELECT origin_path,origin_blake3,
        CASE WHEN length(source_json)<=?1 THEN source_json ELSE NULL END,task_key,retired,{snapshot_columns}
        FROM fast_img_migrations WHERE {filter} ORDER BY origin_path"))?;
    let rows = query
        .query_map(
            params![
                i64::try_from(MAX_LEGACY_BYTES)?,
                origin.map(text).transpose()?
            ],
            |row| {
                Ok(Backup {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    hash: row.get(1)?,
                    bytes: row
                        .get::<_, Option<Vec<u8>>>(2)?
                        .ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                    key: row.get(3)?,
                    retired: row.get(4)?,
                    directory_snapshot: row.get(5)?,
                    directory_snapshot_blake3: row.get(6)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn backup_for_origin(conn: &Connection, origin: &Path) -> Result<Backup> {
    let mut rows = read_backups(conn, Some(origin))?;
    ensure!(
        rows.len() == 1,
        "Legacy migration receipt disappeared or is ambiguous"
    );
    Ok(rows.remove(0))
}

fn retirement_paths(backup: &Backup) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let parent = backup
        .path
        .parent()
        .context("Legacy origin has no parent")?
        .to_path_buf();
    let holding = parent.join(format!(
        ".mfb-migration-{}",
        blake3::hash(text(&backup.path)?.as_bytes()).to_hex()
    ));
    let moved = holding.join("checkpoint.json");
    Ok((parent, holding, moved))
}

fn decoded_directory_snapshot(backup: &Backup) -> Result<Option<SourceDirectoryCleanupSnapshot>> {
    match (
        &backup.directory_snapshot,
        &backup.directory_snapshot_blake3,
    ) {
        (None, None) => Ok(None),
        (Some(bytes), Some(hash)) => {
            ensure!(
                hash.as_slice() == blake3::hash(bytes).as_bytes(),
                "Legacy directory baseline failed BLAKE3 verification; no timestamps were restored"
            );
            let snapshot = SourceDirectoryCleanupSnapshot::from_durable_json(bytes)?;
            let (parent, _, moved) = retirement_paths(backup)?;
            snapshot.validate_durable_scope(&parent, &[backup.path.clone(), moved])?;
            Ok(Some(snapshot))
        }
        _ => bail!(
            "Legacy directory baseline/hash pair is missing, oversized, or incomplete; state retained"
        ),
    }
}

fn verify_backup(conn: &Connection, backup: &Backup) -> Result<WorkingCopyMarker> {
    decoded_directory_snapshot(backup)?;
    ensure!(
        backup.path.is_absolute()
            && backup.hash.as_slice() == blake3::hash(&backup.bytes).as_bytes(),
        "Legacy migration backup failed origin/BLAKE3 validation; retained"
    );
    let value = marker(&backup.bytes)?;
    let current = read_snapshot(conn, &backup.key)?
        .context("Legacy migration receipt has no committed SQL task")?;
    ensure!(
        key_for_canonical(text(&current.working_copy)?) == backup.key,
        "Legacy migration receipt has an invalid canonical SQL task locator"
    );
    // The raw-spelling -> canonical mapping was validated before this receipt's
    // transaction committed. Cold database integrity must not revisit media FS.
    if backup
        .path
        .file_name()
        .is_some_and(|name| name == ".mfb_wc")
    {
        ensure!(
            backup.path.parent() == Some(current.working_copy.as_path()),
            "Legacy local backup belongs to a different canonical output"
        );
    } else {
        let name = format!(
            "{}.json",
            blake3::hash(text(&value.working_copy)?.as_bytes()).to_hex()
        );
        ensure!(
            backup.path.file_name().and_then(|name| name.to_str()) == Some(name.as_str())
                && backup
                    .path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == "markers")
                && backup
                    .path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == "fast_img"),
            "Legacy central backup filename does not match its original absolute output spelling"
        );
    }
    ensure!(
        current.started_at == value.started_at && current.strategy == value.strategy,
        "Legacy backup belongs to a different SQL task attempt; neither authority was overwritten"
    );
    Ok(value)
}

pub(super) fn inspect_backups(conn: &Connection) -> Result<()> {
    for backup in backups(conn)? {
        verify_backup(conn, &backup)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn same_backup(left: &Backup, right: &Backup) -> bool {
    left.path == right.path
        && left.bytes == right.bytes
        && left.hash == right.hash
        && left.key == right.key
        && left.retired == right.retired
        && left.directory_snapshot == right.directory_snapshot
        && left.directory_snapshot_blake3 == right.directory_snapshot_blake3
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Cannot inspect migration holding path"),
        Ok(_) => bail!(
            "Legacy retirement has no durable directory baseline but a holding path exists; original timestamps are unknown; state retained"
        ),
    }
}

// This transaction is deliberately separate from migration/retirement. No
// rename, holding directory or removal may precede its committed readback.
fn prepare_retirement_baseline(conn: &mut Connection, expected: &Backup) -> Result<Backup> {
    ensure!(
        conn.is_autocommit(),
        "Directory baseline requires an independent committed transaction"
    );
    let current = super::in_snapshot(conn, |conn| {
        let current = backup_for_origin(conn, &expected.path)?;
        verify_backup(conn, &current)?;
        ensure!(
            same_backup(&current, expected),
            "Legacy migration receipt changed before directory capture"
        );
        Ok(current)
    })?;
    ensure!(
        !current.retired,
        "Retired legacy checkpoint reappeared; refusing to reuse an older directory baseline"
    );
    if current.directory_snapshot.is_some() {
        return Ok(current);
    }
    let (parent, holding, moved) = retirement_paths(&current)?;
    ensure!(
        real_directory(&parent)?,
        "Legacy retirement directory disappeared; original timestamps cannot be confirmed"
    );
    require_absent(&holding)?;
    let original = read_legacy(&current.path)?.context(
        "Legacy retirement has no durable directory baseline and its origin is missing; original timestamps are unknown; state retained",
    )?;
    ensure!(
        original.bytes == current.bytes,
        "Legacy checkpoint changed before directory capture; retained"
    );
    // A prior interrupted retirement in this directory must restore its dates
    // before another origin captures a new baseline containing those changes.
    for other in backups(conn)? {
        ensure!(
            other.path == current.path
                || other.retired
                || other.directory_snapshot.is_none()
                || other.path.parent() != Some(parent.as_path()),
            "Another legacy retirement in this directory must finish its saved baseline first"
        );
    }
    let snapshot =
        SourceDirectoryCleanupSnapshot::capture(&parent, std::slice::from_ref(&current.path))?;
    snapshot.validate_durable_scope(&parent, &[current.path.clone(), moved])?;
    let bytes = snapshot.to_durable_json()?;
    let hash = blake3::hash(&bytes);
    let rechecked = read_legacy(&current.path)?
        .context("Legacy checkpoint disappeared during directory capture")?;
    ensure!(
        rechecked.stamp == original.stamp && rechecked.bytes == current.bytes,
        "Legacy checkpoint changed during directory capture; retained"
    );
    require_absent(&holding)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let latest = backup_for_origin(&tx, &current.path)?;
    verify_backup(&tx, &latest)?;
    ensure!(
        same_backup(&latest, &current),
        "Legacy migration receipt changed during directory capture"
    );
    let changed = tx.execute(
        "UPDATE fast_img_migrations SET directory_snapshot=?4,directory_snapshot_blake3=?5
        WHERE origin_path=?1 AND origin_blake3=?2 AND task_key=?3 AND retired=0
        AND directory_snapshot IS NULL AND directory_snapshot_blake3 IS NULL",
        params![
            text(&current.path)?,
            current.hash,
            current.key,
            bytes,
            hash.as_bytes().as_slice()
        ],
    )?;
    ensure!(
        changed == 1,
        "Legacy directory baseline was not durably recorded"
    );
    tx.commit()
        .context("Legacy directory baseline commit failed; legacy files retained")?;
    super::in_snapshot(conn, |conn| {
        let saved = backup_for_origin(conn, &current.path)?;
        verify_backup(conn, &saved)?;
        ensure!(
            !saved.retired
                && saved.bytes == current.bytes
                && saved.hash == current.hash
                && saved.key == current.key
                && saved.directory_snapshot.as_deref() == Some(bytes.as_slice())
                && saved.directory_snapshot_blake3.as_deref() == Some(hash.as_bytes().as_slice()),
            "Legacy directory baseline readback differs from its committed capture; files retained"
        );
        Ok(saved)
    })
}

fn retire(backup: &Backup) -> Result<()> {
    let value = locator(&backup.bytes)?;
    validate_origin(&backup.path, &value)?;
    let (parent, holding, moved) = retirement_paths(backup)?;
    ensure!(
        real_directory(&parent)?,
        "Legacy retirement directory disappeared; metadata restoration remains pending"
    );
    let snapshot = decoded_directory_snapshot(backup)?
        .context("Legacy retirement has no committed directory baseline")?;
    let result = retire_files(backup, &parent, &holding, &moved);
    let restored = snapshot.restore_after_cleanup(&[backup.path.clone(), moved]);
    match (result, restored) {
        (Ok(()), Ok(true)) => {
            ensure!(
                real_directory(&parent)?,
                "Legacy retirement directory disappeared during metadata restoration; confirmation remains pending"
            );
            // Flush the restored dates, not only the earlier directory removals.
            sync_directory(&parent)
                .context("Legacy directory metadata sync failed; retirement remains pending")
        }
        (Err(error), Ok(true)) => Err(error),
        (result, Ok(false)) => bail!(
            "Legacy retirement remains pending: operation={}; directory metadata could not be restored after an external change or unsupported identity; timestamps not overwritten; SQL backup retained",
            result
                .err()
                .map_or_else(|| "completed".into(), |error| format!("{error:#}")),
        ),
        (result, Err(error)) => bail!(
            "Legacy retirement remains pending: operation={}; directory metadata restoration failed: {error}; SQL backup retained",
            result
                .err()
                .map_or_else(|| "completed".into(), |error| format!("{error:#}"))
        ),
    }
}

fn retire_files(backup: &Backup, parent: &Path, holding: &Path, moved: &Path) -> Result<()> {
    let original = read_legacy(&backup.path)?;
    if let Some(original) = &original {
        ensure!(
            original.bytes == backup.bytes,
            "Legacy checkpoint changed after SQL commit; retained at {}",
            backup.path.display()
        );
    }
    if real_directory(holding)? {
        ensure!(
            real_directory(holding)?,
            "Invalid migration holding directory"
        );
        let members = fs::read_dir(holding)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        ensure!(
            members.is_empty() || (members.len() == 1 && members[0] == moved),
            "Unknown files in migration holding directory {}; retained",
            holding.display()
        );
    } else if original.is_some() {
        #[cfg(unix)]
        let mut builder = fs::DirBuilder::new();
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(holding)?;
    }
    let held = read_legacy(moved)?;
    ensure!(
        original.is_none() || held.is_none(),
        "Legacy origin and retirement copy both exist; neither was deleted"
    );
    if let Some(original) = original {
        let rechecked = read_legacy(&backup.path)?
            .context("Legacy checkpoint disappeared before retirement")?;
        ensure!(
            rechecked.stamp == original.stamp && rechecked.bytes == backup.bytes,
            "Legacy checkpoint changed before retirement; retained"
        );
        fs::rename(&backup.path, moved)?;
        let renamed = read_legacy(moved)?;
        if !renamed
            .as_ref()
            .is_some_and(|file| file.stamp == original.stamp && file.bytes == backup.bytes)
        {
            // link() restores only if the original name is still vacant; it
            // cannot overwrite a replacement created by another actor.
            match fs::hard_link(moved, &backup.path) {
                Ok(()) => {
                    fs::remove_file(moved)?;
                    fs::remove_dir(holding)?;
                    sync_directory(parent)?;
                }
                Err(error) => bail!(
                    "Legacy checkpoint changed during retirement; retained at {} (restore refused: {error})",
                    moved.display()
                ),
            }
            bail!("Legacy checkpoint changed during retirement; restored and retained");
        }
    }
    if let Some(held) = read_legacy(moved)? {
        ensure!(
            held.bytes == backup.bytes,
            "Retirement copy differs from the committed backup; retained at {}",
            moved.display()
        );
        fs::remove_file(moved)?;
        sync_directory(holding)?;
    }
    if real_directory(holding)? {
        fs::remove_dir(holding)?;
    }
    sync_directory(parent)
}

pub(super) fn migrate_legacy_for_source(
    source: &Path,
    requested: Option<&Path>,
) -> Result<MigrationReport> {
    let source = PathBuf::from(canonical_text(source)?);
    // This is a logical reservation in the formal state root, not a media
    // directory. The launcher may already hold the input directory's lease.
    let _migration_lock = crate::process_lock::acquire_output_dir_lock(
        &crate::process_lock::get_mfb_root()?.join("fast-img-state-migration"),
    )?;
    let mut paths = BTreeSet::new();
    let mut files = Vec::new();
    for path in central_files()? {
        let file = read_legacy(&path)?.context("Legacy central checkpoint disappeared")?;
        // A malformed central record is not skipped using an untrusted locator.
        let value = marker(&file.bytes)?;
        validate_origin(
            &file.path,
            &Locator {
                schema: value.schema,
                src_dir: value.src_dir.clone(),
                working_copy: value.working_copy.clone(),
            },
        )?;
        if locator_matches(&value.src_dir, &source)? {
            paths.insert(file.path.clone());
            files.push((file, value));
        }
    }
    for path in local_candidates(&source, requested)? {
        let Some(file) = read_legacy(&path)? else {
            continue;
        };
        if !paths.insert(file.path.clone()) {
            continue;
        }
        let value = parse_marker_for_working_copy(
            &file.bytes,
            path.parent().context("Local marker has no output")?,
        )?;
        let _ = locator(&file.bytes)?;
        ensure!(
            canonical_text(&value.src_dir)? == text(&source)?,
            "Requested legacy local checkpoint belongs to another source; retained"
        );
        files.push((file, value));
    }
    let mut outputs = BTreeSet::new();
    for (_, value) in &files {
        let wc = PathBuf::from(canonical_text(&value.working_copy)?);
        ensure!(
            wc.file_name().is_some() && !wc.starts_with(&source) && !source.starts_with(&wc),
            "Legacy migration refuses overlapping or root output ownership; checkpoint retained"
        );
        outputs.insert(wc);
    }
    let mut conn = super::open_store_connection()?;
    super::in_snapshot(&conn, |conn| {
        inspect_backups(conn)?;
        for backup in backups(conn)? {
            if !backup.retired || paths.contains(&backup.path) {
                let current = read_snapshot(conn, &backup.key)?
                    .context("Pending migration task disappeared")?;
                if current.src_dir == source {
                    outputs.insert(current.working_copy);
                }
            }
        }
        Ok(())
    })?;
    let _output_locks = outputs
        .iter()
        .map(|path| crate::process_lock::acquire_output_dir_lock(path))
        .collect::<Result<Vec<_>>>()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    inspect_backups(&tx)?;
    let existing: BTreeMap<_, _> = backups(&tx)?
        .into_iter()
        .map(|backup| (backup.path.clone(), backup))
        .collect();
    let mut new_tasks = BTreeMap::<String, WorkingCopyMarker>::new();
    let mut migrated = 0;
    for (file, value) in &files {
        let checked = parse_marker_for_working_copy(&file.bytes, &value.working_copy)?;
        ensure!(
            checked == *value,
            "Legacy checkpoint changed interpretation during validation"
        );
        let wc = canonical_text(&value.working_copy)?;
        let key = key_for_canonical(&wc);
        if let Some(backup) = existing.get(&file.path) {
            ensure!(
                !backup.retired,
                "Retired legacy checkpoint reappeared; file retained without applying its old directory baseline"
            );
            ensure!(
                backup.bytes == file.bytes && backup.key == key,
                "Legacy origin changed after a committed migration; retained"
            );
        } else {
            let mut normalized = value.clone();
            normalized.working_copy = PathBuf::from(&wc);
            normalized.src_dir.clone_from(&source);
            if let Some(previous) = new_tasks.get(&key) {
                ensure!(
                    previous == &normalized,
                    "Conflicting legacy checkpoints name the same task; none were overwritten"
                );
            } else {
                ensure!(
                    read_snapshot(&tx, &key)?.is_none(),
                    "SQL task already exists without this legacy migration receipt; refusing to overwrite it"
                );
                write_snapshot(&tx, value)?;
                new_tasks.insert(key.clone(), normalized);
            }
            tx.execute("INSERT INTO fast_img_migrations(origin_path,origin_blake3,source_json,task_key,retired) VALUES(?1,?2,?3,?4,0)",
                params![text(&file.path)?, blake3::hash(&file.bytes).as_bytes().as_slice(), file.bytes, key])?;
            migrated += 1;
        }
        let current =
            read_legacy(&file.path)?.context("Legacy checkpoint disappeared before SQL commit")?;
        ensure!(
            current.stamp == file.stamp && current.bytes == file.bytes,
            "Legacy checkpoint changed before SQL commit; source retained"
        );
    }
    tx.commit()
        .context("Fast IMG migration failed to commit; legacy files retained")?;
    // A new read snapshot must verify committed SQL before any legacy removal.
    let mut targets = super::in_snapshot(&conn, |conn| {
        inspect_backups(conn)?;
        let mut targets = Vec::new();
        for backup in backups(conn)? {
            let current =
                read_snapshot(conn, &backup.key)?.context("Migrated SQL task disappeared")?;
            if current.src_dir == source && (!backup.retired || paths.contains(&backup.path)) {
                targets.push(backup);
            }
        }
        Ok(targets)
    })?;
    targets.sort_by(|left, right| {
        (
            left.path.parent(),
            left.directory_snapshot.is_none(),
            &left.path,
        )
            .cmp(&(
                right.path.parent(),
                right.directory_snapshot.is_none(),
                &right.path,
            ))
    });
    let mut retired = 0;
    for backup in targets {
        let backup = prepare_retirement_baseline(&mut conn, &backup)?;
        retire(&backup)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = backup_for_origin(&tx, &backup.path)?;
        verify_backup(&tx, &current)?;
        ensure!(
            same_backup(&current, &backup),
            "Legacy directory baseline changed before retirement confirmation"
        );
        let changed = tx.execute("UPDATE fast_img_migrations SET retired=1 WHERE origin_path=?1 AND origin_blake3=?2 AND task_key=?3 AND retired=0 AND directory_snapshot_blake3=?4",
            params![text(&backup.path)?, backup.hash, backup.key, backup.directory_snapshot_blake3])?;
        ensure!(
            changed == 1,
            "Committed legacy migration receipt changed before retirement confirmation"
        );
        tx.commit()?;
        retired += 1;
    }
    Ok(MigrationReport { migrated, retired })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::pipeline::verification::FastImgStageName;

    fn isolated(name: &str) -> Result<bool> {
        if std::env::var("MFB_LEGACY_MIGRATION_TEST_CHILD").as_deref() == Ok(name) {
            return Ok(true);
        }
        let root = tempfile::tempdir()?;
        let module = module_path!()
            .split_once("::")
            .context("Test module has no crate prefix")?
            .1;
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                &format!("{module}::{name}"),
                "--nocapture",
                "--test-threads=1",
            ])
            .env("MFB_LEGACY_MIGRATION_TEST_CHILD", name)
            .env(
                crate::constants::ENV_MFB_HOME_ROOT,
                root.path().join("formal-state"),
            )
            .output()?;
        ensure!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "Isolated migration regression failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(false)
    }

    fn fixture(root: &Path, name: &str) -> Result<WorkingCopyMarker> {
        let source = root.join(name);
        let wc = root.join(format!("{name}_optimized"));
        fs::create_dir(&source)?;
        fs::create_dir(&wc)?;
        let mut marker = WorkingCopyMarker::new(source.canonicalize()?, wc.canonicalize()?, 0);
        marker.started_at = "2026-10-31T00:00:00Z".into();
        Ok(marker)
    }

    #[test]
    fn unrelated_missing_legacy_ancestors_do_not_block_selected_source() -> Result<()> {
        if !isolated("unrelated_missing_legacy_ancestors_do_not_block_selected_source")? {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let removed = root.path().join("removed-parent");
        fs::create_dir(&removed)?;
        let unrelated = fixture(&removed, "old-source")?;
        let central = central_dir()?;
        fs::create_dir_all(&central)?;
        let origin = legacy_marker_path_for_working_copy(&unrelated.working_copy)?;
        let original = serde_json::to_vec(&unrelated)?;
        fs::write(&origin, &original)?;
        fs::remove_dir(&unrelated.src_dir)?;
        fs::remove_dir(&unrelated.working_copy)?;
        fs::remove_dir(&removed)?;
        let selected = fixture(root.path(), "current-source")?;

        reject_unmigrated_for_source(&selected.src_dir)?;
        reject_unmigrated_for_working_copy(&selected.working_copy)?;
        assert!(
            open_readonly()?.is_none(),
            "discovery must remain read-only"
        );
        let report = migrate_legacy_for_source(&selected.src_dir, None)?;
        assert_eq!((report.migrated, report.retired), (0, 0));
        assert_eq!(fs::read(&origin)?, original);
        assert!(
            !removed.exists(),
            "unrelated history must not recreate media paths"
        );

        // A genuinely selected legacy checkpoint still requires migration.
        let selected_origin = legacy_marker_path_for_working_copy(&selected.working_copy)?;
        fs::write(&selected_origin, serde_json::to_vec(&selected)?)?;
        let error = reject_unmigrated_for_source(&selected.src_dir).unwrap_err();
        assert!(error.to_string().contains("MFB_STATE_MIGRATION_REQUIRED"));
        assert!(
            error
                .to_string()
                .contains(selected_origin.to_str().unwrap())
        );
        let report = migrate_legacy_for_source(&selected.src_dir, None)?;
        assert_eq!((report.migrated, report.retired), (1, 1));
        assert_eq!(fs::read(&origin)?, original);
        assert!(!selected_origin.exists());
        assert!(!removed.exists());
        Ok(())
    }

    #[test]
    fn legacy_discovery_keeps_missing_leaf_aliases_and_rejects_dangling_ancestors() -> Result<()> {
        if !isolated("legacy_discovery_keeps_missing_leaf_aliases_and_rejects_dangling_ancestors")?
        {
            return Ok(());
        }
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir()?;
        let root = root.path().canonicalize()?;
        let selected = fixture(&root, "selected")?;
        let real = root.join("real-parent");
        fs::create_dir(&real)?;
        let alias = root.join("alias");
        symlink(&real, &alias)?;
        let mut legacy = selected.clone();
        legacy.working_copy = alias.join("missing-output");
        let central = central_dir()?;
        fs::create_dir_all(&central)?;
        let origin = legacy_marker_path_for_working_copy(&legacy.working_copy)?;
        fs::write(&origin, serde_json::to_vec(&legacy)?)?;
        let error = reject_unmigrated_for_working_copy(&real.join("missing-output")).unwrap_err();
        assert!(error.to_string().contains("MFB_STATE_MIGRATION_REQUIRED"));
        assert!(error.to_string().contains(origin.to_str().unwrap()));

        fs::remove_file(&origin)?;
        fs::remove_file(&alias)?;
        symlink(root.join("missing-parent"), &alias)?;
        legacy.src_dir = alias.join("old-source");
        legacy.working_copy.clone_from(&selected.working_copy);
        let origin = legacy_marker_path_for_working_copy(&legacy.working_copy)?;
        let original = serde_json::to_vec(&legacy)?;
        fs::write(&origin, &original)?;
        for error in [
            reject_unmigrated_for_source(&selected.src_dir).unwrap_err(),
            migrate_legacy_for_source(&selected.src_dir, None).unwrap_err(),
        ] {
            assert!(
                error
                    .to_string()
                    .contains("linked or non-directory ancestor")
            );
            assert!(error.to_string().contains(legacy.src_dir.to_str().unwrap()));
        }
        assert_eq!(fs::read(&origin)?, original);
        assert!(fs::symlink_metadata(&alias)?.file_type().is_symlink());
        assert!(
            open_readonly()?.is_none(),
            "rejected discovery must not create state"
        );
        Ok(())
    }

    #[test]
    fn embedded_checkpoint_migrates_with_ghost_times_and_preserves_unknown_history() -> Result<()> {
        if !isolated("embedded_checkpoint_migrates_with_ghost_times_and_preserves_unknown_history")?
        {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let original = fixture(root.path(), "source")?;
        let mut value = serde_json::to_value(&original)?;
        value
            .as_object_mut()
            .unwrap()
            .remove("photos_submission_history");
        let raw = serde_json::to_vec_pretty(&value)?;
        let origin = original.working_copy.join(".mfb_wc");
        fs::write(&origin, &raw)?;
        let time = filetime::FileTime::from_unix_time(1_500_000_000, 123_456_789);
        filetime::set_file_times(&original.working_copy, time, time)?;
        assert!(
            reject_unmigrated_for_source(&original.src_dir)
                .unwrap_err()
                .to_string()
                .contains("MFB_STATE_MIGRATION_REQUIRED")
        );
        assert!(
            open_readonly()?.is_none(),
            "normal discovery must not create SQL or migrate JSON"
        );
        let report = migrate_legacy_for_source(&original.src_dir, Some(&original.working_copy))?;
        assert_eq!((report.migrated, report.retired), (1, 1));
        assert!(!origin.exists());
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(&original.working_copy)?),
            time
        );
        let conn = open_readonly()?.context("Migration did not create SQL")?;
        let stored = read_snapshot(
            &conn,
            &key_for_canonical(&canonical_text(&original.working_copy)?),
        )?
        .unwrap();
        assert!(stored.photos_submission_history().is_none());
        let rows = backups(&conn)?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bytes, raw);
        assert!(rows[0].retired);
        reject_unmigrated_for_source(&original.src_dir)?;
        let report = migrate_legacy_for_source(&original.src_dir, None)?;
        assert_eq!((report.migrated, report.retired), (0, 0));
        // Reusing an old receipt must not roll a newly created file/directory
        // generation back to the previous retirement's timestamp.
        fs::write(&origin, &raw)?;
        let new_time = filetime::FileTime::from_unix_time(1_600_000_000, 987_654_321);
        filetime::set_file_times(&original.working_copy, new_time, new_time)?;
        assert!(migrate_legacy_for_source(&original.src_dir, None).is_err());
        assert_eq!(fs::read(&origin)?, raw);
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(&original.working_copy)?),
            new_time
        );
        fs::remove_file(&origin)?;
        // Cold integrity is a database check, even after source/output cleanup.
        fs::remove_dir(&original.working_copy)?;
        fs::remove_dir(&original.src_dir)?;
        inspect_backups(&conn)?;
        Ok(())
    }

    #[test]
    fn legacy_duplicate_future_and_unknown_nested_members_remain_unmigrated() -> Result<()> {
        if !isolated("legacy_duplicate_future_and_unknown_nested_members_remain_unmigrated")? {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let original = fixture(root.path(), "invalid")?;
        let value = serde_json::to_value(&original)?;
        let mut future = value.clone();
        future["schema"] = serde_json::json!(99);
        let mut unknown = value.clone();
        unknown["blake3_log"] = serde_json::json!({"a.jpg":{"src":"legacy", "out":"legacy", "library_asset":null, "future_member":true}});
        let serialized = serde_json::to_string(&value)?;
        let duplicate = format!("{{\"schema\":{},{}", original.schema, &serialized[1..]);
        for raw in [
            serde_json::to_vec(&future)?,
            serde_json::to_vec(&unknown)?,
            duplicate.into_bytes(),
        ] {
            let origin = original.working_copy.join(".mfb_wc");
            fs::write(&origin, &raw)?;
            assert!(
                migrate_legacy_for_source(&original.src_dir, Some(&original.working_copy)).is_err()
            );
            assert_eq!(fs::read(&origin)?, raw);
            assert!(open_readonly()?.is_none());
        }
        Ok(())
    }

    #[test]
    fn migration_refuses_unreceipted_sql_and_recovers_a_committed_retirement() -> Result<()> {
        if !isolated("migration_refuses_unreceipted_sql_and_recovers_a_committed_retirement")? {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let original = fixture(root.path(), "pending")?;
        let origin = origin_path(&original.working_copy.join(".mfb_wc"))?;
        let raw = serde_json::to_vec(&original)?;
        fs::write(&origin, &raw)?;
        let key = key_for_canonical(&canonical_text(&original.working_copy)?);
        let mut conn = super::super::open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_snapshot(&tx, &original)?;
        tx.commit()?;
        assert!(migrate_legacy_for_source(&original.src_dir, None).is_err());
        assert_eq!(fs::read(&origin)?, raw);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO fast_img_migrations(origin_path,origin_blake3,source_json,task_key,retired) VALUES(?1,?2,?3,?4,0)",
            params![text(&origin)?, blake3::hash(&raw).as_bytes().as_slice(), raw, key])?;
        tx.commit()?;
        let mut changed = original.clone();
        changed.stage = FastImgStageName::OutputPrepared;
        let changed_bytes = serde_json::to_vec(&changed)?;
        fs::write(&origin, &changed_bytes)?;
        assert!(migrate_legacy_for_source(&original.src_dir, None).is_err());
        assert_eq!(fs::read(&origin)?, changed_bytes);
        assert_eq!(read_snapshot(&conn, &key)?.unwrap(), original);
        fs::write(&origin, &raw)?;
        let before_interruption = filetime::FileTime::from_unix_time(1_500_000_000, 123_456_789);
        filetime::set_file_times(
            &original.working_copy,
            before_interruption,
            before_interruption,
        )?;
        let pending = backup_for_origin(&conn, &origin)?;
        let saved = prepare_retirement_baseline(&mut conn, &pending)?;
        assert!(saved.directory_snapshot.is_some());
        let holding = origin.parent().unwrap().join(format!(
            ".mfb-migration-{}",
            blake3::hash(text(&origin)?.as_bytes()).to_hex()
        ));
        fs::create_dir(&holding)?;
        fs::rename(&origin, holding.join("checkpoint.json"))?;
        // Simulate the changed directory time left by the interrupted process.
        // It must not become the recovery attempt's replacement baseline.
        let interrupted_time = filetime::FileTime::from_unix_time(1_600_000_000, 987_654_321);
        filetime::set_file_times(&original.working_copy, interrupted_time, interrupted_time)?;
        assert!(reject_unmigrated_for_working_copy(&original.working_copy).is_err());
        let report = migrate_legacy_for_source(&original.src_dir, None)?;
        assert_eq!((report.migrated, report.retired), (0, 1));
        assert!(!origin.exists() && !holding.exists());
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(&original.working_copy)?),
            before_interruption,
            "recovery must restore the pre-interruption timestamp, not recapture the changed one",
        );
        inspect_backups(&conn)?;
        assert!(backups(&conn)?[0].retired);
        Ok(())
    }

    fn pending_fixture(root: &Path, name: &str) -> Result<(WorkingCopyMarker, Connection, Backup)> {
        let original = fixture(root, name)?;
        let origin = origin_path(&original.working_copy.join(".mfb_wc"))?;
        let raw = serde_json::to_vec(&original)?;
        fs::write(&origin, &raw)?;
        let mut conn = super::super::open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_snapshot(&tx, &original)?;
        tx.execute("INSERT INTO fast_img_migrations(origin_path,origin_blake3,source_json,task_key,retired) VALUES(?1,?2,?3,?4,0)",
            params![text(&origin)?, blake3::hash(&raw).as_bytes().as_slice(), raw,
                key_for_canonical(&canonical_text(&original.working_copy)?)])?;
        tx.commit()?;
        let backup = backup_for_origin(&conn, &origin)?;
        Ok((original, conn, backup))
    }

    #[test]
    fn durable_retirement_recovers_after_removal_and_rejects_unknown_old_interruptions()
    -> Result<()> {
        if !isolated(
            "durable_retirement_recovers_after_removal_and_rejects_unknown_old_interruptions",
        )? {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let (original, mut conn, backup) = pending_fixture(root.path(), "removed")?;
        let before = filetime::FileTime::from_unix_time(1_500_000_000, 123_456_789);
        filetime::set_file_times(&original.working_copy, before, before)?;
        let saved = prepare_retirement_baseline(&mut conn, &backup)?;
        let (parent, holding, moved) = retirement_paths(&saved)?;
        fs::create_dir(&holding)?;
        fs::rename(&saved.path, &moved)?;
        fs::remove_file(&moved)?;
        fs::remove_dir(&holding)?;
        let interrupted = filetime::FileTime::from_unix_time(1_600_000_000, 987_654_321);
        filetime::set_file_times(&parent, interrupted, interrupted)?;
        drop(conn);
        let report = migrate_legacy_for_source(&original.src_dir, None)?;
        assert_eq!((report.migrated, report.retired), (0, 1));
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(&parent)?),
            before
        );

        let (unknown, mut conn, backup) = pending_fixture(root.path(), "unknown")?;
        let (_, holding, moved) = retirement_paths(&backup)?;
        fs::create_dir(&holding)?;
        fs::rename(&backup.path, &moved)?;
        let error = prepare_retirement_baseline(&mut conn, &backup)
            .err()
            .context("Missing baseline was accepted")?;
        assert!(error.to_string().contains("timestamps are unknown"));
        assert_eq!(fs::read(&moved)?, backup.bytes);
        assert!(!backup_for_origin(&conn, &backup.path)?.retired);
        fs::remove_file(&moved)?;
        fs::remove_dir(&holding)?;
        assert!(
            prepare_retirement_baseline(&mut conn, &backup).is_err(),
            "absent origin is not evidence of restored metadata"
        );
        assert!(!backup_for_origin(&conn, &backup.path)?.retired);
        fs::remove_dir(&unknown.working_copy)?;
        let (missing, mut conn, backup) = pending_fixture(root.path(), "missing-parent")?;
        let saved = prepare_retirement_baseline(&mut conn, &backup)?;
        fs::remove_file(&saved.path)?;
        fs::remove_dir(&missing.working_copy)?;
        assert!(
            retire(&saved).is_err(),
            "a missing parent cannot confirm timestamp restoration"
        );
        assert!(!backup_for_origin(&conn, &saved.path)?.retired);
        Ok(())
    }

    #[test]
    fn durable_baseline_rejects_tampering_and_external_changes_without_confirming_retirement()
    -> Result<()> {
        if !isolated(
            "durable_baseline_rejects_tampering_and_external_changes_without_confirming_retirement",
        )? {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let (original, mut conn, backup) = pending_fixture(root.path(), "changed")?;
        let retained = original.working_copy.join("unrelated.txt");
        fs::write(&retained, b"original unrelated bytes")?;
        let before = filetime::FileTime::from_unix_time(1_500_000_000, 123_456_789);
        filetime::set_file_times(&original.working_copy, before, before)?;
        let saved = prepare_retirement_baseline(&mut conn, &backup)?;
        conn.execute(
            "UPDATE fast_img_migrations SET directory_snapshot_blake3=?1 WHERE origin_path=?2",
            params![vec![0_u8; 32], text(&saved.path)?],
        )?;
        assert!(inspect_backups(&conn).is_err());
        assert!(migrate_legacy_for_source(&original.src_dir, None).is_err());
        assert_eq!(fs::read(&saved.path)?, saved.bytes);
        conn.execute(
            "UPDATE fast_img_migrations SET directory_snapshot_blake3=?1 WHERE origin_path=?2",
            params![saved.directory_snapshot_blake3, text(&saved.path)?],
        )?;

        let (_, holding, moved) = retirement_paths(&saved)?;
        fs::create_dir(&holding)?;
        fs::rename(&saved.path, &moved)?;
        fs::write(&retained, b"externally changed unrelated bytes")?;
        let external_time = filetime::FileTime::from_unix_time(1_600_000_000, 987_654_321);
        filetime::set_file_times(&original.working_copy, external_time, external_time)?;
        assert!(migrate_legacy_for_source(&original.src_dir, None).is_err());
        assert_eq!(fs::read(&retained)?, b"externally changed unrelated bytes");
        assert!(!backup_for_origin(&conn, &saved.path)?.retired);
        assert_ne!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(&original.working_copy)?),
            filetime::FileTime::from_unix_time(1_500_000_000, 123_456_789)
        );
        inspect_backups(&conn)?;
        Ok(())
    }

    #[test]
    fn shared_directory_finishes_a_pending_baseline_before_capturing_the_next_origin() -> Result<()>
    {
        if !isolated(
            "shared_directory_finishes_a_pending_baseline_before_capturing_the_next_origin",
        )? {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let first = fixture(root.path(), "shared")?;
        let mut second = first.clone();
        second.working_copy = root.path().join("shared_optimized_2");
        fs::create_dir(&second.working_copy)?;
        second.working_copy = second.working_copy.canonicalize()?;
        let directory = central_dir()?;
        fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        let mut conn = super::super::open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for value in [&first, &second] {
            let path = origin_path(&legacy_marker_path_for_working_copy(&value.working_copy)?)?;
            let raw = serde_json::to_vec(value)?;
            fs::write(&path, &raw)?;
            write_snapshot(&tx, value)?;
            tx.execute("INSERT INTO fast_img_migrations(origin_path,origin_blake3,source_json,task_key,retired) VALUES(?1,?2,?3,?4,0)",
                params![text(&path)?, blake3::hash(&raw).as_bytes().as_slice(), raw,
                    key_for_canonical(text(&value.working_copy)?)])?;
        }
        tx.commit()?;
        let before = filetime::FileTime::from_unix_time(1_500_000_000, 123_456_789);
        filetime::set_file_times(&directory, before, before)?;
        // Select the last filename so ordinary pathname order would incorrectly
        // capture its sibling while this baseline's holding directory exists.
        let last = backups(&conn)?
            .pop()
            .context("Missing shared directory fixture")?;
        let saved = prepare_retirement_baseline(&mut conn, &last)?;
        let (_, holding, moved) = retirement_paths(&saved)?;
        fs::create_dir(&holding)?;
        fs::rename(&saved.path, &moved)?;
        drop(conn);
        let report = migrate_legacy_for_source(&first.src_dir, None)?;
        assert_eq!((report.migrated, report.retired), (0, 2));
        assert!(fs::read_dir(&directory)?.next().is_none());
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(&directory)?),
            before
        );
        let conn = open_readonly()?.context("Migration DB disappeared")?;
        let records = backups(&conn)?;
        assert!(
            records
                .iter()
                .all(|backup| backup.retired && backup.directory_snapshot.is_some())
        );
        inspect_backups(&conn)?;
        Ok(())
    }
}
