//! One-way, recoverable relocation of the formal Core SQLite store.

use super::{STORE_FILE_NAME, StoreFileIdentity, StorePathBinding, schema_version};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

const PENDING: &str = ".core-state-relocation-pending.json";
const ACTIVE: &str = ".core-state-relocation-active.json";
const BACKUPS: &str = "relocation-backups";
const BACKUP: &str = "mfb_store.sqlite";
const CANDIDATE: &str = "candidate.sqlite";
const RETIRED_VERSION: i32 = i32::MAX;
const RELOCATION_KEY: &str = "core_state_location_v1";
const DIAGNOSTIC_SNAPSHOT: &str = "core-state-recovery.sqlite";
const DIAGNOSTIC_RECEIPT: &str = ".core-state-diagnostic-recovery.json";
const MAX_RECORD: usize = 4096;

#[derive(Debug, Serialize)]
pub struct FormalStateMigrationReceipt {
    pub schema_version: u8,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub backup: PathBuf,
    pub store_schema_version: i32,
    pub recovered: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct FormalStateRecoveryPreview {
    pub schema_version: u8,
    pub missing_database: PathBuf,
    pub snapshot: PathBuf,
    pub snapshot_blake3: String,
    pub store_schema_version: i32,
}

#[derive(Debug, Serialize)]
pub struct FormalStateRecoveryReceipt {
    pub schema_version: u8,
    pub missing_database: PathBuf,
    pub snapshot: PathBuf,
    pub snapshot_blake3: String,
    pub destination_root: PathBuf,
    pub destination_snapshot: PathBuf,
    pub store_schema_version: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileId {
    device: u64,
    inode: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelocationRecord {
    schema_version: u8,
    id: i64,
    source_identity: FileId,
    destination_identity: FileId,
    backup_blake3: String,
    candidate_blake3: String,
    store_schema_version: i32,
}

struct StatePaths {
    legacy: PathBuf,
    destination: PathBuf,
    state_directory: PathBuf,
}

impl StatePaths {
    fn at(root: &Path) -> Result<Self> {
        ensure!(
            root.is_absolute()
                && !root
                    .components()
                    .any(|component| component == Component::ParentDir),
            "Core state root must be an absolute path without parent traversal"
        );
        let state_directory = root.join("state");
        let legacy_directory = root.join("cache");
        for name in [DIAGNOSTIC_SNAPSHOT, DIAGNOSTIC_RECEIPT] {
            match fs::symlink_metadata(root.join(name)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
                Ok(_) => anyhow::bail!(
                    "A historical Core recovery directory cannot be used as an MFB state root"
                ),
            }
        }
        let _ = StoreFileIdentity::inspect(&state_directory, true)?;
        let _ = StoreFileIdentity::inspect(&legacy_directory, true)?;
        Ok(Self {
            legacy: legacy_directory.join(STORE_FILE_NAME),
            destination: state_directory.join(STORE_FILE_NAME),
            state_directory,
        })
    }

    fn backup_directory(&self, id: i64) -> PathBuf {
        self.state_directory.join(BACKUPS).join(id.to_string())
    }

    fn active_relocation(&self) -> Result<Option<RelocationRecord>> {
        let Some(active) = read_record(&self.state_directory.join(ACTIVE))? else {
            return Ok(None);
        };
        let prepared = read_record(&self.state_directory.join(PENDING))?
            .context("Activated Core relocation lost its prepared authority; state retained")?;
        ensure!(
            prepared == active,
            "Activated and prepared Core relocation records disagree; state retained"
        );
        Ok(Some(active))
    }
}

fn read_record(path: &Path) -> Result<Option<RelocationRecord>> {
    use std::io::Read;
    let Some(observed) = StoreFileIdentity::inspect(path, false)? else {
        return Ok(None);
    };
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?.take(u64::try_from(MAX_RECORD + 1)?);
    ensure!(
        file.get_ref().metadata()?.is_file(),
        "Core relocation record is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.get_ref().metadata()?;
        ensure!(
            metadata.dev() == observed.device
                && metadata.ino() == observed.inode
                && metadata.nlink() == 1,
            "Core relocation record changed before it was opened"
        );
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_RECORD,
        "Core relocation record exceeds its size limit"
    );
    ensure!(
        StoreFileIdentity::inspect(path, false)?.as_ref() == Some(&observed),
        "Core relocation record changed while reading"
    );
    let record: RelocationRecord =
        serde_json::from_slice(&bytes).context("Read Core relocation record")?;
    ensure!(
        record.schema_version == 1
            && record.id > 0
            && (2..=super::STORE_SCHEMA_VERSION).contains(&record.store_schema_version)
            && valid_digest(&record.backup_blake3)
            && valid_digest(&record.candidate_blake3),
        "Unsupported or invalid Core relocation record; state retained"
    );
    Ok(Some(record))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn file_id(path: &Path) -> Result<FileId> {
    #[cfg(unix)]
    {
        let identity =
            StoreFileIdentity::inspect(path, false)?.context("Core state file is missing")?;
        Ok(FileId {
            device: identity.device,
            inode: identity.inode,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        anyhow::bail!("This platform cannot verify relocated SQLite file identity")
    }
}

fn retirement_id(conn: &Connection) -> Result<Option<i64>> {
    let version: Option<i32> = conn
        .query_row(
            "SELECT value FROM store_metadata WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if version != Some(RETIRED_VERSION) {
        return Ok(None);
    }
    let id: i64 = conn
        .query_row(
            "SELECT value FROM store_metadata WHERE key=?1",
            [RELOCATION_KEY],
            |row| row.get(0),
        )
        .context("Retired Core store has no relocation identity")?;
    ensure!(id > 0, "Retired Core store relocation identity is invalid");
    Ok(Some(id))
}

fn database_present(path: &Path) -> Result<bool> {
    let binding = StorePathBinding::new(path)?;
    let present = StoreFileIdentity::inspect(path, false)?.is_some();
    let sidecars = binding.inspect_sidecars()?;
    if !present && sidecars {
        ensure!(
            StoreFileIdentity::inspect(path, false)?.is_some(),
            "Core state database is absent while journal evidence remains"
        );
        return Ok(true);
    }
    Ok(present)
}

fn check_active(paths: &StatePaths, active: &RelocationRecord, legacy_present: bool) -> Result<()> {
    ensure!(
        file_id(&paths.destination)? == active.destination_identity,
        "Relocated Core store is missing or replaced; restore verified state before continuing"
    );
    if legacy_present {
        ensure!(
            file_id(&paths.legacy)? == active.source_identity,
            "Conflicting database appeared at the retired Core location"
        );
    }
    Ok(())
}

/// Locate the sole authoritative formal database without creating directories or data.
///
/// # Errors
/// Rejects unfinished relocations, incompatible locators and ambiguous/unsafe state paths.
pub fn formal_store_path_at(root: &Path) -> Result<PathBuf> {
    Ok(resolve(root)?.0)
}

/// Recheck that maintenance opened the current Core authority.
/// # Errors
/// Rejects path changes, incomplete relocation and a mismatched relocation identity.
pub fn verify_formal_store_connection(
    root: &Path,
    path: &Path,
    connection: &Connection,
) -> Result<()> {
    let (current, id) = resolve(root)?;
    ensure!(
        current == path,
        "Core database location changed during maintenance; retry the operation"
    );
    validate_opened(connection, id)
}

pub(super) fn resolve(root: &Path) -> Result<(PathBuf, Option<i64>)> {
    let paths = StatePaths::at(root)?;
    let active = paths.active_relocation()?;
    let legacy_present = database_present(&paths.legacy)?;
    let destination_present = database_present(&paths.destination)?;
    if let Some(active) = active {
        check_active(&paths, &active, legacy_present)?;
        return Ok((paths.destination, Some(active.id)));
    }
    if let Some(pending) = read_record(&paths.state_directory.join(PENDING))? {
        let legacy = StorePathBinding::new(&paths.legacy)?
            .open(false, false)?
            .context("Prepared relocation lost its legacy Core source")?;
        ensure!(
            legacy_present
                && !destination_present
                && file_id(&paths.legacy)? == pending.source_identity
                && retirement_id(&legacy)?.is_none(),
            "Core state relocation is incomplete; rerun the explicit Core migration before processing"
        );
        return Ok((paths.legacy, None));
    }
    ensure!(
        !(legacy_present && destination_present),
        "Both Core state locations exist without a verified relocation locator"
    );
    if legacy_present {
        return Ok((paths.legacy, None));
    }
    ensure!(
        !paths.state_directory.join(BACKUPS).try_exists()?,
        "Core relocation authority is missing; verify the protected backup before continuing"
    );
    Ok((paths.destination, None))
}

pub(super) fn validate_opened(conn: &Connection, id: Option<i64>) -> Result<()> {
    let has_metadata = id.is_some() || conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='store_metadata')",
        [], |row| row.get::<_, bool>(0),
    )?;
    let stored = if has_metadata {
        conn.query_row(
            "SELECT value FROM store_metadata WHERE key=?1",
            [RELOCATION_KEY],
            |row| row.get(0),
        )
        .optional()?
    } else {
        None
    };
    ensure!(
        stored == id,
        "Core store relocation identity does not match its authority; verify the protected state before use"
    );
    Ok(())
}

/// Relocate the recognized legacy Core SQLite store on an idle installation.
///
/// # Errors
/// Leaves legacy data and verified backups in place on interruption or incompatible state.
/// Recover a consistent interruption by repeating this explicit operation.
pub fn migrate_formal_state_location() -> Result<FormalStateMigrationReceipt> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let _lease = crate::production_lock::acquire_production_lease()?;
        migrate_at(&crate::process_lock::inspect_mfb_root_path()?)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("Core relocation requires durable directory synchronization on this platform")
}

/// Inspect the rollback snapshot of a missing, previously activated Core store.
///
/// # Errors
/// Rejects live or conflicting state, unverified backups and unsupported platforms.
pub fn inspect_formal_state_recovery() -> Result<FormalStateRecoveryPreview> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return migration::recovery_preview_at(&crate::process_lock::inspect_mfb_root_path()?);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("Core recovery requires supported native path identity and atomic publication")
}

/// Export a previously observed historical snapshot for diagnostics in a new private directory.
///
/// # Errors
/// Rejects stale acknowledgments, unsafe output locations, and activated state that is not missing.
pub fn restore_formal_state_snapshot_to(
    destination_root: &Path,
    expected_snapshot_blake3: &str,
) -> Result<FormalStateRecoveryReceipt> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let _lease = crate::production_lock::acquire_production_lease()?;
        migration::restore_snapshot_at(
            &crate::process_lock::inspect_mfb_root_path()?,
            destination_root,
            expected_snapshot_blake3,
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (destination_root, expected_snapshot_blake3);
        anyhow::bail!(
            "Core recovery requires supported native path identity and atomic publication"
        )
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod migration {
    use super::*;
    use rusqlite::TransactionBehavior;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    fn write_record(path: &Path, record: &RelocationRecord) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(
            path.parent()
                .context("Missing relocation record directory")?,
        )?;
        serde_json::to_writer(&mut file, record)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(path).map_err(|err| err.error)?;
        crate::io_utils::sync_parent_directory(path)?;
        Ok(())
    }

    fn digest(path: &Path) -> Result<String> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let before = file_id(path)?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.dev() == before.device
                && metadata.ino() == before.inode
                && metadata.nlink() == 1,
            "Relocation snapshot changed before it was opened"
        );
        let mut hash = blake3::Hasher::new();
        hash.update_reader(&mut file)?;
        let current = file.metadata()?;
        ensure!(
            current.len() == metadata.len()
                && current.mtime() == metadata.mtime()
                && current.mtime_nsec() == metadata.mtime_nsec()
                && current.ctime() == metadata.ctime()
                && current.ctime_nsec() == metadata.ctime_nsec(),
            "Relocation snapshot bytes changed while reading"
        );
        ensure!(
            file_id(path)? == before,
            "Relocation snapshot was replaced while reading"
        );
        Ok(hash.finalize().to_hex().to_string())
    }

    fn private_directory(path: &Path) -> Result<()> {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(path) {
            Ok(()) => crate::io_utils::sync_parent_directory(path)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = StoreFileIdentity::inspect_metadata(path, true)?
            .context("Relocation directory disappeared")?;
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid does not access memory.
        let owner = unsafe { libc::geteuid() };
        ensure!(
            metadata.uid() == owner && metadata.mode() & 0o077 == 0,
            "Core relocation directory must be owned by and private to this user"
        );
        Ok(())
    }

    fn existing_private_directory(path: &Path) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let metadata = StoreFileIdentity::inspect_metadata(path, true)?
            .context("Core recovery backup directory does not exist")?;
        // SAFETY: geteuid does not access memory.
        let owner = unsafe { libc::geteuid() };
        ensure!(
            metadata.uid() == owner && metadata.mode() & 0o077 == 0,
            "Core recovery backup directory must be owned by and private to this user"
        );
        Ok(())
    }

    fn inspect_database(conn: &Connection) -> Result<i32> {
        let version =
            schema_version(conn)?.context("Core relocation source has no recognized schema")?;
        ensure!(
            version >= 2,
            "Open the recognized CRC32 Core state in a compatible worker before relocating"
        );
        let mut integrity = conn.prepare("PRAGMA integrity_check")?;
        let mut results = integrity.query([])?;
        ensure!(
            results
                .next()?
                .context("Core integrity check produced no result")?
                .get::<_, String>(0)?
                == "ok"
                && results.next()?.is_none(),
            "Core SQLite integrity check failed; state retained"
        );
        ensure!(
            conn.prepare("PRAGMA foreign_key_check")?
                .query([])?
                .next()?
                .is_none(),
            "Core SQLite foreign-key check failed; state retained"
        );
        let mut statement = conn.prepare(
            "SELECT payload,payload_blake3 FROM blob_store WHERE namespace NOT IN (?1,?2)",
        )?;
        let mut rows =
            statement.query([super::super::NS_PATH_TREE, super::super::NS_GATE1_PROOF])?;
        while let Some(row) = rows.next()? {
            ensure!(
                blake3::hash(row.get_ref(0)?.as_blob()?)
                    .as_bytes()
                    .as_slice()
                    == row.get_ref(1)?.as_blob()?,
                "Protected Core payload failed BLAKE3 validation; state retained"
            );
        }
        crate::fast_img_store::inspect_integrity(conn)?;
        Ok(version)
    }

    fn snapshot(path: &Path) -> Result<Connection> {
        StorePathBinding::new(path)?
            .open(false, false)?
            .context("Required relocation snapshot is missing")
    }

    fn validate_snapshot(
        path: &Path,
        expected_blake3: &str,
        version: i32,
        id: Option<i64>,
    ) -> Result<()> {
        ensure!(
            !StorePathBinding::new(path)?.inspect_sidecars()?,
            "Core relocation snapshot has unexpected SQLite sidecars"
        );
        ensure!(
            digest(path)? == expected_blake3,
            "Core relocation snapshot BLAKE3 changed"
        );
        let connection = snapshot(path)?;
        ensure!(
            inspect_database(&connection)? == version,
            "Core relocation snapshot schema changed"
        );
        let actual: Option<i64> = connection
            .query_row(
                "SELECT value FROM store_metadata WHERE key=?1",
                [RELOCATION_KEY],
                |row| row.get(0),
            )
            .optional()?;
        ensure!(actual == id, "Core relocation snapshot identity changed");
        Ok(())
    }

    pub(super) fn recovery_preview_at(root: &Path) -> Result<FormalStateRecoveryPreview> {
        let paths = StatePaths::at(root)?;
        let active = paths
            .active_relocation()?
            .context("Core has no activated relocation record to recover")?;
        ensure!(
            !database_present(&paths.destination)?,
            "Activated Core database still exists; diagnostic recovery never replaces it"
        );
        if database_present(&paths.legacy)? {
            ensure!(
                file_id(&paths.legacy)? == active.source_identity
                    && retirement_id(&snapshot(&paths.legacy)?)? == Some(active.id),
                "The previous Core location no longer matches its retirement fence"
            );
        }
        let backup_dir = paths.backup_directory(active.id);
        for directory in [
            &paths.state_directory,
            &paths.state_directory.join(BACKUPS),
            &backup_dir,
        ] {
            existing_private_directory(directory)?;
        }
        let snapshot = backup_dir.join(BACKUP);
        validate_snapshot(
            &snapshot,
            &active.backup_blake3,
            active.store_schema_version,
            None,
        )?;
        Ok(FormalStateRecoveryPreview {
            schema_version: 1,
            missing_database: paths.destination,
            snapshot,
            snapshot_blake3: active.backup_blake3,
            store_schema_version: active.store_schema_version,
        })
    }

    fn recovery_target(original_root: &Path, destination: &Path) -> Result<PathBuf> {
        ensure!(
            destination.is_absolute()
                && !destination
                    .components()
                    .any(|part| matches!(part, Component::ParentDir | Component::CurDir)),
            "Core recovery destination must use an absolute path without traversal"
        );
        let parent = destination
            .parent()
            .context("Core recovery destination has no parent directory")?;
        let name = destination
            .file_name()
            .context("Core recovery destination has no directory name")?;
        let canonical = parent.canonicalize()?;
        ensure!(
            parent == canonical,
            "Core recovery destination parent uses an alias; select its canonical path"
        );
        let parent_info = fs::symlink_metadata(&canonical)?;
        ensure!(
            parent_info.is_dir() && !parent_info.is_symlink(),
            "Core recovery parent is not a real directory"
        );
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid does not access memory.
        let owner = unsafe { libc::geteuid() };
        ensure!(
            parent_info.uid() == owner && parent_info.mode() & 0o022 == 0,
            "Core recovery destination parent must be owned by and writable only by this user"
        );
        for ancestor in canonical.ancestors() {
            match fs::symlink_metadata(ancestor.join(".git")) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
                Ok(_) => anyhow::bail!("Core recovery destination is inside a Git worktree"),
            }
        }
        let destination = canonical.join(name);
        let original = original_root.canonicalize()?;
        ensure!(
            !destination.starts_with(&original) && !original.starts_with(&destination),
            "Core recovery destination must be outside the authoritative MFB root"
        );
        match fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(destination),
            Err(error) => Err(error.into()),
            Ok(_) => anyhow::bail!("Core recovery destination already exists; no files replaced"),
        }
    }

    pub(super) fn restore_snapshot_at(
        original_root: &Path,
        destination_root: &Path,
        expected_snapshot_blake3: &str,
    ) -> Result<FormalStateRecoveryReceipt> {
        use std::os::unix::fs::OpenOptionsExt;

        ensure!(
            valid_digest(expected_snapshot_blake3),
            "Core recovery requires a valid acknowledged snapshot hash"
        );
        let preview = recovery_preview_at(original_root)?;
        ensure!(
            preview.snapshot_blake3 == expected_snapshot_blake3,
            "Acknowledged snapshot does not match the verified Core recovery backup"
        );
        let destination_root = recovery_target(original_root, destination_root)?;
        let parent = destination_root
            .parent()
            .context("Missing Core recovery destination parent")?;
        let staging = tempfile::Builder::new()
            .prefix(".mfb-core-recovery-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)?;
        existing_private_directory(staging.path())?;
        let staged_snapshot = staging.path().join(DIAGNOSTIC_SNAPSHOT);
        let mut source = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&preview.snapshot)?;
        ensure!(
            source.metadata()?.is_file(),
            "Core recovery snapshot is no longer a regular file"
        );
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged_snapshot)?;
        std::io::copy(&mut source, &mut output)?;
        output.sync_all()?;
        drop(output);
        validate_snapshot(
            &staged_snapshot,
            &preview.snapshot_blake3,
            preview.store_schema_version,
            None,
        )?;
        fs::set_permissions(&staged_snapshot, fs::Permissions::from_mode(0o400))?;
        crate::io_utils::sync_committed_file_and_parent(&staged_snapshot)?;
        let result = FormalStateRecoveryReceipt {
            schema_version: 1,
            missing_database: preview.missing_database.clone(),
            snapshot: preview.snapshot.clone(),
            snapshot_blake3: preview.snapshot_blake3.clone(),
            destination_snapshot: destination_root.join(DIAGNOSTIC_SNAPSHOT),
            destination_root: destination_root.clone(),
            store_schema_version: preview.store_schema_version,
        };
        let marker = staging.path().join(DIAGNOSTIC_RECEIPT);
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&marker)?;
        serde_json::to_writer(&file, &result)?;
        file.set_permissions(fs::Permissions::from_mode(0o400))?;
        file.sync_all()?;
        crate::io_utils::sync_parent_directory(&marker)?;
        ensure!(
            recovery_target(original_root, &destination_root)? == destination_root,
            "Core recovery destination changed before publication"
        );
        ensure!(
            recovery_preview_at(original_root)? == preview,
            "Core recovery evidence changed before publication"
        );
        rename_unoccupied(staging.path(), &destination_root)?;
        crate::io_utils::sync_parent_directory(&destination_root).context(
            "Core recovery directory was published but its parent could not be synchronized",
        )?;
        Ok(result)
    }

    fn synchronize_snapshot(path: &Path) -> Result<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        crate::io_utils::sync_committed_file_and_parent(path)?;
        Ok(())
    }

    fn copy_snapshot(source: &Connection, destination: &Path) -> Result<()> {
        let mut output = StorePathBinding::new(destination)?
            .open(true, true)?
            .context("Core rollback snapshot destination could not be opened")?;
        output.pragma_update(None, "synchronous", "FULL")?;
        super::super::copy_sqlite_snapshot(source, &mut output)?;
        drop(output);
        synchronize_snapshot(destination)
    }

    fn prepare(
        paths: &StatePaths,
        legacy: &Connection,
        source_identity: FileId,
        version: i32,
    ) -> Result<RelocationRecord> {
        let id: i64 = legacy.query_row("SELECT random() & 9223372036854775807", [], |row| {
            row.get(0)
        })?;
        ensure!(id > 0, "Core relocation identity generation failed");
        let backups = paths.state_directory.join(BACKUPS);
        private_directory(&backups)?;
        let dir = paths.backup_directory(id);
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        crate::io_utils::sync_parent_directory(&dir)?;
        let backup = dir.join(BACKUP);
        let reader = snapshot(&paths.legacy)?;
        copy_snapshot(&reader, &backup)?;
        drop(reader);
        let backup_blake3 = digest(&backup)?;
        validate_snapshot(&backup, &backup_blake3, version, None)?;

        let candidate = dir.join(CANDIDATE);
        fs::copy(&backup, &candidate)?;
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o600))?;
        let copy = StorePathBinding::new(&candidate)?
            .open(true, false)?
            .context("Core candidate is missing")?;
        copy.pragma_update(None, "synchronous", "FULL")?;
        copy.execute(
            "INSERT INTO store_metadata VALUES (?1,?2)",
            rusqlite::params![RELOCATION_KEY, id],
        )?;
        drop(copy);
        synchronize_snapshot(&candidate)?;
        let candidate_blake3 = digest(&candidate)?;
        validate_snapshot(&candidate, &candidate_blake3, version, Some(id))?;
        let record = RelocationRecord {
            schema_version: 1,
            id,
            source_identity,
            destination_identity: file_id(&candidate)?,
            backup_blake3,
            candidate_blake3,
            store_schema_version: version,
        };
        write_record(&paths.state_directory.join(PENDING), &record)?;
        Ok(record)
    }

    fn fence_legacy(conn: &Connection, record: &RelocationRecord) -> Result<()> {
        let mut statement = conn.prepare(
            "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name",
        )?;
        let tables = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        conn.execute(
            "UPDATE store_metadata SET value=?1 WHERE key='schema_version'",
            [RETIRED_VERSION],
        )?;
        conn.execute(
            "INSERT INTO store_metadata VALUES (?1,?2)",
            rusqlite::params![RELOCATION_KEY, record.id],
        )?;
        for (index, name) in tables.iter().enumerate() {
            let name = name.replace('"', "\"\"");
            for action in ["INSERT", "UPDATE", "DELETE"] {
                conn.execute_batch(&format!(
                    "CREATE TRIGGER mfb_retired_{index}_{action} BEFORE {action} ON \"{name}\"
                     BEGIN SELECT RAISE(ABORT,'Core store has been relocated'); END;"
                ))?;
            }
        }
        ensure!(
            retirement_id(conn)? == Some(record.id),
            "Core retirement fence readback failed"
        );
        Ok(())
    }

    fn receipt(
        paths: &StatePaths,
        record: &RelocationRecord,
        recovered: bool,
    ) -> FormalStateMigrationReceipt {
        FormalStateMigrationReceipt {
            schema_version: 1,
            source: paths.legacy.clone(),
            destination: paths.destination.clone(),
            backup: paths.backup_directory(record.id).join(BACKUP),
            store_schema_version: record.store_schema_version,
            recovered,
        }
    }

    fn publish(paths: &StatePaths, record: &RelocationRecord) -> Result<()> {
        let dir = paths.backup_directory(record.id);
        private_directory(&dir)?;
        let backup = dir.join(BACKUP);
        validate_snapshot(
            &backup,
            &record.backup_blake3,
            record.store_schema_version,
            None,
        )?;
        if !database_present(&paths.destination)? {
            let candidate = dir.join(CANDIDATE);
            validate_snapshot(
                &candidate,
                &record.candidate_blake3,
                record.store_schema_version,
                Some(record.id),
            )?;
            ensure!(
                file_id(&candidate)? == record.destination_identity,
                "Prepared Core store was replaced"
            );
            rename_unoccupied(&candidate, &paths.destination)?;
            crate::io_utils::sync_parent_directory(&candidate)?;
        }
        ensure!(
            file_id(&paths.destination)? == record.destination_identity,
            "Relocated Core store does not match its reserved identity"
        );
        validate_snapshot(
            &paths.destination,
            &record.candidate_blake3,
            record.store_schema_version,
            Some(record.id),
        )?;
        synchronize_snapshot(&paths.destination)?;
        write_record(&paths.state_directory.join(ACTIVE), record)?;
        Ok(())
    }

    fn rename_unoccupied(source: &Path, destination: &Path) -> Result<()> {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let source = CString::new(source.as_os_str().as_bytes())?;
        let destination = CString::new(destination.as_os_str().as_bytes())?;
        #[cfg(target_os = "macos")]
        // SAFETY: The paths are live NUL-terminated strings.
        let status =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        // SAFETY: The paths are live NUL-terminated strings.
        let status = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if status != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Publish Core state without overwriting a destination");
        }
        Ok(())
    }

    pub(super) fn migrate_at(root: &Path) -> Result<FormalStateMigrationReceipt> {
        let paths = StatePaths::at(root)?;
        if let Some(active) = paths.active_relocation()? {
            let legacy_present = database_present(&paths.legacy)?;
            check_active(&paths, &active, legacy_present)?;
            if legacy_present {
                ensure!(
                    retirement_id(&snapshot(&paths.legacy)?)? == Some(active.id),
                    "Legacy database no longer has the matching retirement fence"
                );
            }
            validate_snapshot(
                &paths.backup_directory(active.id).join(BACKUP),
                &active.backup_blake3,
                active.store_schema_version,
                None,
            )?;
            validate_opened(&snapshot(&paths.destination)?, Some(active.id))?;
            return Ok(receipt(&paths, &active, true));
        }
        let pending_path = paths.state_directory.join(PENDING);
        let pending = read_record(&pending_path)?;
        ensure!(
            pending.is_some() || !database_present(&paths.destination)?,
            "Existing Core destination has no migration authority"
        );
        let mut legacy = StorePathBinding::new(&paths.legacy)?
            .open(true, false)?
            .context("No legacy Core state to migrate")?;
        let source_identity = file_id(&paths.legacy)?;
        legacy.pragma_update(None, "synchronous", "FULL")?;
        let tx = legacy
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("Lock the legacy Core database without racing current writers")?;
        let recovered = pending.is_some();
        if let Some(retired) = retirement_id(&tx)? {
            let record =
                pending.context("Retired Core database has no pending relocation record")?;
            ensure!(
                retired == record.id && source_identity == record.source_identity,
                "Retired Core database does not match its prepared relocation record"
            );
            tx.commit()?;
            publish(&paths, &record)?;
            return Ok(receipt(&paths, &record, true));
        }
        let version = inspect_database(&tx)?;
        if let Some(previous) = pending {
            ensure!(
                !database_present(&paths.destination)?
                    && source_identity == previous.source_identity,
                "Unfenced Core relocation has a conflicting source or destination"
            );
            let previous_backup = paths.backup_directory(previous.id).join(BACKUP);
            validate_snapshot(
                &previous_backup,
                &previous.backup_blake3,
                previous.store_schema_version,
                None,
            )?;
            rename_unoccupied(
                &pending_path,
                &paths.backup_directory(previous.id).join(PENDING),
            )?;
            crate::io_utils::sync_parent_directory(&pending_path)?;
            crate::io_utils::sync_parent_directory(&previous_backup)?;
        }
        create_private_state_directory(&paths.state_directory)?;
        let record = prepare(&paths, &tx, source_identity, version)?;
        ensure!(
            file_id(&paths.legacy)? == source_identity,
            "Legacy Core store changed during snapshot"
        );
        fence_legacy(&tx, &record)?;
        tx.commit()
            .context("Commit Core relocation fence while holding the SQLite writer lock")?;
        crate::io_utils::sync_parent_directory(&paths.legacy)?;
        publish(&paths, &record)?;
        Ok(receipt(&paths, &record, recovered))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn seed(root: &Path) -> Result<StatePaths> {
            let paths = StatePaths::at(root)?;
            fs::create_dir(paths.legacy.parent().unwrap())?;
            let mut connection = StorePathBinding::new(&paths.legacy)?
                .open(true, true)?
                .unwrap();
            crate::mfb_sqlite_store::init_schema(&mut connection)?;
            crate::mfb_sqlite_store::upsert_blob(
                &connection,
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                "first",
                1,
                None,
                b"checkpoint data",
                1,
            )?;
            crate::mfb_sqlite_store::upsert_blob(
                &connection,
                "unknown-state",
                "first",
                1,
                None,
                b"unknown retained data",
                2,
            )?;
            connection.execute_batch(
                "CREATE TABLE media_entries(id INTEGER PRIMARY KEY, private BLOB);
                 INSERT INTO media_entries VALUES(42,x'7265636F7264');
                 CREATE TABLE hidden_rowid(private TEXT);
                 INSERT INTO hidden_rowid(rowid,private) VALUES(5,'one'),(101,'two');
                 PRAGMA user_version=9;",
            )?;
            Ok(paths)
        }

        #[test]
        fn core_snapshot_recovery_exports_exact_historical_bytes_without_reactivating_authority()
        -> Result<()> {
            let parent = tempfile::tempdir()?;
            let parent = parent.path().canonicalize()?;
            let root = parent.join("original");
            fs::create_dir(&root)?;
            let paths = seed(&root)?;
            let migration = migrate_at(&root)?;
            assert!(recovery_preview_at(&root).is_err());
            let current = StorePathBinding::new(&paths.destination)?
                .open(true, false)?
                .unwrap();
            crate::mfb_sqlite_store::upsert_blob(
                &current,
                "unknown-state",
                "latest",
                1,
                None,
                b"post-cutover",
                3,
            )?;
            drop(current);
            let held = paths.state_directory.join("held-current.sqlite");
            fs::rename(&paths.destination, &held)?;

            let preview = recovery_preview_at(&root)?;
            assert_eq!(preview.missing_database, paths.destination);
            assert_eq!(preview.snapshot, migration.backup);
            let before = [
                fs::read(&preview.snapshot)?,
                fs::read(&held)?,
                fs::read(paths.state_directory.join(ACTIVE))?,
                fs::read(paths.state_directory.join(PENDING))?,
                fs::read(&paths.legacy)?,
            ];
            let destination = parent.join("diagnostic");
            assert!(restore_snapshot_at(&root, &destination, &"0".repeat(64)).is_err());
            assert!(!destination.exists());
            let exported = restore_snapshot_at(&root, &destination, &preview.snapshot_blake3)?;
            assert_eq!(
                exported.destination_snapshot,
                destination.join(DIAGNOSTIC_SNAPSHOT)
            );
            assert_eq!(fs::read(&exported.destination_snapshot)?, before[0]);
            assert_eq!(
                fs::metadata(&exported.destination_snapshot)?
                    .permissions()
                    .mode()
                    & 0o777,
                0o400
            );
            assert_eq!(
                fs::metadata(&destination)?.permissions().mode() & 0o777,
                0o700
            );
            let receipt: serde_json::Value =
                serde_json::from_slice(&fs::read(destination.join(DIAGNOSTIC_RECEIPT))?)?;
            assert_eq!(receipt["snapshot_blake3"], preview.snapshot_blake3);
            let diagnostic = snapshot(&exported.destination_snapshot)?;
            assert!(
                crate::mfb_sqlite_store::read_blob(&diagnostic, "unknown-state", "latest", 1)?
                    .is_none()
            );
            assert_eq!(
                crate::mfb_sqlite_store::read_blob(&diagnostic, "unknown-state", "first", 1)?,
                Some(b"unknown retained data".to_vec())
            );
            assert!(restore_snapshot_at(&root, &destination, &preview.snapshot_blake3).is_err());
            let alias = parent.join("alias");
            std::os::unix::fs::symlink(&destination, &alias)?;
            assert!(formal_store_path_at(&alias).is_err());
            assert!(formal_store_path_at(&destination).is_err());
            assert!(formal_store_path_at(&root).is_err());
            assert!(!paths.destination.exists() && !destination.join("state").exists());
            assert_eq!(
                [
                    fs::read(&preview.snapshot)?,
                    fs::read(&held)?,
                    fs::read(paths.state_directory.join(ACTIVE))?,
                    fs::read(paths.state_directory.join(PENDING))?,
                    fs::read(&paths.legacy)?,
                ],
                before
            );
            Ok(())
        }

        #[test]
        fn core_snapshot_recovery_refuses_changed_backups_or_destinations_and_orphaned_sidecars()
        -> Result<()> {
            let parent = tempfile::tempdir()?;
            let parent = parent.path().canonicalize()?;
            let root = parent.join("original");
            fs::create_dir(&root)?;
            let paths = seed(&root)?;
            let migration = migrate_at(&root)?;
            fs::rename(
                &paths.destination,
                paths.state_directory.join("held-current.sqlite"),
            )?;
            let orphan = paths.destination.with_extension("sqlite-wal");
            fs::write(&orphan, b"possibly newer transactions")?;
            assert!(recovery_preview_at(&root).is_err());
            assert_eq!(fs::read(&orphan)?, b"possibly newer transactions");
            fs::remove_file(orphan)?;
            let preview = recovery_preview_at(&root)?;

            let git = parent.join("git");
            fs::create_dir(&git)?;
            fs::write(git.join(".git"), b"synthetic worktree marker")?;
            let alias = parent.join("alias");
            std::os::unix::fs::symlink(&root, &alias)?;
            for path in [
                &root.join("overlap"),
                &git.join("private"),
                &alias.join("alias-overlap"),
            ] {
                assert!(restore_snapshot_at(&root, path, &preview.snapshot_blake3).is_err());
                assert!(!path.exists());
            }
            let destination = parent.join("diagnostic");
            fs::create_dir(&destination)?;
            fs::write(destination.join("existing"), b"retained")?;
            assert!(restore_snapshot_at(&root, &destination, &preview.snapshot_blake3).is_err());
            assert_eq!(fs::read(destination.join("existing"))?, b"retained");
            fs::write(&migration.backup, b"changed backup")?;
            assert!(
                restore_snapshot_at(&root, &parent.join("not-created"), &preview.snapshot_blake3)
                    .is_err()
            );
            assert!(!parent.join("not-created").exists());
            assert_eq!(fs::read(&migration.backup)?, b"changed backup");
            Ok(())
        }

        #[test]
        fn core_location_migration_preserves_wal_unknown_rows_and_fences_old_writes() -> Result<()>
        {
            let root = tempfile::tempdir()?;
            let expected = root.path().join("state").join(STORE_FILE_NAME);
            assert_eq!(formal_store_path_at(root.path())?, expected);
            assert_eq!(fs::read_dir(root.path())?.count(), 0);
            let paths = seed(root.path())?;
            assert_eq!(formal_store_path_at(root.path())?, paths.legacy);

            // Holding a connection preserves committed WAL contents and supplies a pre-cutover writer.
            let old = StorePathBinding::new(&paths.legacy)?
                .open(true, false)?
                .unwrap();
            old.pragma_update(None, "wal_autocheckpoint", 0)?;
            crate::mfb_sqlite_store::upsert_blob(
                &old,
                crate::mfb_sqlite_store::NS_PROCESSED,
                "second",
                1,
                None,
                b"committed WAL",
                3,
            )?;
            assert!(paths.legacy.with_extension("sqlite-wal").metadata()?.len() > 0);
            let result = migrate_at(root.path())?;
            assert!(!result.recovered);
            assert_eq!(formal_store_path_at(root.path())?, expected);
            assert!(schema_version(&old).is_err());
            assert!(
                old.execute("UPDATE media_entries SET private=x'00'", [])
                    .is_err()
            );
            assert!(old.execute("DELETE FROM blob_store", []).is_err());
            let output = snapshot(&result.destination)?;
            let backup = snapshot(&result.backup)?;
            for database in [&output, &backup] {
                assert_eq!(
                    crate::mfb_sqlite_store::read_blob(
                        database,
                        crate::mfb_sqlite_store::NS_PROCESSED,
                        "second",
                        1
                    )?,
                    Some(b"committed WAL".to_vec())
                );
                assert_eq!(
                    crate::mfb_sqlite_store::read_blob(database, "unknown-state", "first", 1)?,
                    Some(b"unknown retained data".to_vec())
                );
                assert_eq!(
                    database.query_row(
                        "SELECT private FROM media_entries WHERE id=42",
                        [],
                        |row| row.get::<_, Vec<u8>>(0)
                    )?,
                    b"record"
                );
                assert_eq!(
                    database.query_row(
                        "SELECT private FROM hidden_rowid WHERE rowid=101",
                        [],
                        |row| row.get::<_, String>(0)
                    )?,
                    "two"
                );
                assert_eq!(
                    database
                        .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))?,
                    9
                );
            }
            assert!(migrate_at(root.path())?.recovered);
            drop((output, backup));
            fs::rename(
                &paths.destination,
                paths.state_directory.join("saved.sqlite"),
            )?;
            assert!(formal_store_path_at(root.path()).is_err());
            assert!(!paths.destination.exists());
            Ok(())
        }

        #[test]
        fn core_location_migration_recovers_after_rollback_or_committed_fence() -> Result<()> {
            for commit_fence in [false, true] {
                let root = tempfile::tempdir()?;
                let paths = seed(root.path())?;
                private_directory(&paths.state_directory)?;
                let mut legacy = StorePathBinding::new(&paths.legacy)?
                    .open(true, false)?
                    .unwrap();
                let tx = legacy.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let record = prepare(&paths, &tx, file_id(&paths.legacy)?, inspect_database(&tx)?)?;
                fence_legacy(&tx, &record)?;
                if commit_fence {
                    tx.commit()?;
                    assert!(formal_store_path_at(root.path()).is_err());
                } else {
                    tx.rollback()?;
                    assert_eq!(formal_store_path_at(root.path())?, paths.legacy);
                    crate::mfb_sqlite_store::upsert_blob(
                        &legacy,
                        "unknown-state",
                        "second",
                        1,
                        None,
                        b"after rollback",
                        2,
                    )?;
                }
                let backup = paths.backup_directory(record.id).join(BACKUP);
                let before = fs::read(&backup)?;
                let result = migrate_at(root.path())?;
                assert!(result.recovered);
                assert_eq!(formal_store_path_at(root.path())?, paths.destination);
                let output = snapshot(&result.destination)?;
                assert_eq!(
                    crate::mfb_sqlite_store::read_blob(&output, "unknown-state", "second", 1)?,
                    (!commit_fence).then(|| b"after rollback".to_vec())
                );
                assert_eq!(fs::read(&backup)?, before);
                assert_eq!(result.backup == backup, commit_fence);
            }
            Ok(())
        }

        #[test]
        fn core_location_migration_refuses_unknown_split_brain_without_mutation() -> Result<()> {
            let root = tempfile::tempdir()?;
            let paths = seed(root.path())?;
            private_directory(&paths.state_directory)?;
            fs::write(&paths.destination, b"unowned destination")?;
            let before = fs::read(&paths.legacy)?;
            assert!(formal_store_path_at(root.path()).is_err());
            assert!(migrate_at(root.path()).is_err());
            assert_eq!(fs::read(&paths.destination)?, b"unowned destination");
            assert_eq!(fs::read(&paths.legacy)?, before);
            Ok(())
        }

        #[test]
        fn activated_core_requires_matching_prepared_record_before_maintenance() -> Result<()> {
            let root = tempfile::tempdir()?;
            let paths = seed(root.path())?;
            migrate_at(root.path())?;
            let conn = snapshot(&paths.destination)?;
            let before = fs::read(&paths.destination)?;
            verify_formal_store_connection(root.path(), &paths.destination, &conn)?;

            let prepared = paths.state_directory.join(PENDING);
            let hold = paths.state_directory.join("held-prepared.json");
            fs::rename(&prepared, &hold)?;
            assert!(formal_store_path_at(root.path()).is_err());
            assert!(
                verify_formal_store_connection(root.path(), &paths.destination, &conn).is_err()
            );
            assert!(migrate_at(root.path()).is_err());
            let mut conflicting = read_record(&hold)?.unwrap();
            conflicting.id ^= 1;
            if conflicting.id == 0 {
                conflicting.id = 2;
            }
            write_record(&prepared, &conflicting)?;
            assert!(formal_store_path_at(root.path()).is_err());
            assert!(
                verify_formal_store_connection(root.path(), &paths.destination, &conn).is_err()
            );
            assert!(migrate_at(root.path()).is_err());
            assert_eq!(fs::read(&paths.destination)?, before);
            Ok(())
        }

        #[test]
        fn detached_relocated_core_cannot_become_fresh_authority() -> Result<()> {
            let old = tempfile::tempdir()?;
            let paths = seed(old.path())?;
            let legacy = snapshot(&paths.legacy)?;
            validate_opened(&legacy, None)?;
            assert!(validate_opened(&legacy, Some(1)).is_err());
            drop(legacy);
            migrate_at(old.path())?;

            let fresh = tempfile::tempdir()?;
            let fresh_path = formal_store_path_at(fresh.path())?;
            create_private_state_directory(fresh_path.parent().unwrap())?;
            let uninitialized = Connection::open(&fresh_path)?;
            validate_opened(&uninitialized, None)?;
            drop(uninitialized);
            fs::copy(&paths.destination, &fresh_path)?;
            let before = fs::read(&fresh_path)?;
            let detached = snapshot(&fresh_path)?;
            assert!(validate_opened(&detached, None).is_err());
            assert!(verify_formal_store_connection(fresh.path(), &fresh_path, &detached).is_err());
            assert_eq!(fs::read(&fresh_path)?, before);
            Ok(())
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
use migration::migrate_at;

pub(super) fn create_private_state_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => crate::io_utils::sync_parent_directory(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = StoreFileIdentity::inspect_metadata(path, true)?
        .context("Core state directory disappeared")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid does not access memory.
        let owner = unsafe { libc::geteuid() };
        ensure!(
            metadata.uid() == owner && metadata.mode() & 0o077 == 0,
            "Core state directory must be owned by and private to this user"
        );
    }
    #[cfg(not(unix))]
    let _ = metadata;
    Ok(())
}
