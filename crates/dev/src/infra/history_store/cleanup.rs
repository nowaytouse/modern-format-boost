//! Selection-bound History maintenance; media and formal processing state are never targets.

use super::*;
use rusqlite::OptionalExtension as _;
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const OWNER_TABLE: &str = "history_session_ownership_v1";
const REMOVED_TABLE: &str = "history_removed_sessions_v1";
const MAX_SELECTIONS: usize = 128;
const SNAPSHOT_BYTE_LIMIT: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub struct HistorySessionLease {
    _lock: foundation::process_lock::DirLock,
}

fn identity(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value.bytes().all(|byte| byte.is_ascii_graphic()),
        "History cleanup identity is empty, oversized or invalid"
    );
    Ok(())
}

fn lock_target(directory: &Path, session: &str) -> Result<PathBuf> {
    identity(session)?;
    Ok(directory.canonicalize()?.join(format!(
        ".mfb-history-session-{}",
        blake3::hash(session.as_bytes()).to_hex()
    )))
}

fn owner_table_exists(connection: &Connection) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name=?1",
        [OWNER_TABLE],
        |row| row.get::<_, i64>(0),
    )? == 1)
}

fn validate_owner_table(connection: &Connection) -> Result<()> {
    connection.prepare(
        "SELECT session_id,owner_pid,artifacts_json FROM history_session_ownership_v1 LIMIT 0",
    )?;
    let version: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('history_session_ownership_v1')
         WHERE name='session_id' AND type='TEXT' AND pk=1",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        version == 1,
        "unsupported History ownership identity schema"
    );
    let columns: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('history_session_ownership_v1')",
        [],
        |row| row.get(0),
    )?;
    ensure!(columns == 3, "unsupported History ownership column schema");
    Ok(())
}

fn removed_table_exists(connection: &Connection) -> Result<bool> {
    let kind: Option<String> = connection
        .query_row(
            "SELECT type FROM sqlite_schema WHERE name=?1",
            [REMOVED_TABLE],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(kind) = kind {
        ensure!(
            kind == "table",
            "History removed-session marker is not a table"
        );
        let columns: Vec<(String, String, i64, i64)> = connection.prepare(
            "SELECT name,type,\"notnull\",pk FROM pragma_table_info('history_removed_sessions_v1')",
        )?.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        ensure!(
            columns == [("session_id".to_owned(), "TEXT".to_owned(), 1, 1)],
            "unsupported History removed-session marker schema"
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

fn ensure_session_not_removed(connection: &Connection, session: &str) -> Result<()> {
    if removed_table_exists(connection)? {
        let removed: i64 = connection.query_row(
            "SELECT COUNT(*) FROM history_removed_sessions_v1 WHERE session_id IN (?1,?2)",
            params![session, retired_session_digest(session)],
            |row| row.get(0),
        )?;
        ensure!(
            removed == 0,
            "History session identity was retired by explicit cleanup and cannot be reused"
        );
    }
    Ok(())
}

fn retired_session_digest(session: &str) -> String {
    format!("blake3:{}", blake3::hash(session.as_bytes()).to_hex())
}

/// Claim ownership inside the caller's payload transaction, never in a separate commit.
pub(super) fn claim_history_session(transaction: &Transaction<'_>, session: &str) -> Result<()> {
    ensure_session_not_removed(transaction, session)?;
    if !owner_table_exists(transaction)? {
        transaction.execute_batch(
            "CREATE TABLE history_session_ownership_v1(
             session_id TEXT PRIMARY KEY NOT NULL REFERENCES history_sessions(session_id),
             owner_pid INTEGER NOT NULL,artifacts_json TEXT NOT NULL);",
        )?;
    }
    validate_owner_table(transaction)?;
    transaction.execute(
        "INSERT INTO history_sessions(session_id,updated_at) VALUES(?1,?2)
         ON CONFLICT(session_id) DO NOTHING",
        params![session, now_timestamp()],
    )?;
    transaction.execute(
        "INSERT INTO history_session_ownership_v1 VALUES(?1,?2,'[]')
         ON CONFLICT(session_id) DO NOTHING",
        params![session, i64::from(std::process::id())],
    )?;
    let owner: i64 = transaction.query_row(
        "SELECT owner_pid FROM history_session_ownership_v1 WHERE session_id=?1",
        [session],
        |row| row.get(0),
    )?;
    ensure!(
        owner == i64::from(std::process::id()),
        "History session identity belongs to another writer"
    );
    Ok(())
}

/// Hold this shared guard for the entire launcher session, including archival.
pub fn acquire_history_session_lease(
    directory: &Path,
    session: &str,
) -> Result<Arc<HistorySessionLease>> {
    static LEASES: OnceLock<Mutex<HashMap<PathBuf, Weak<HistorySessionLease>>>> = OnceLock::new();
    let target = lock_target(directory, session)?;
    let mut leases = LEASES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("History writer lease registry is poisoned"))?;
    if let Some(lease) = leases.get(&target).and_then(Weak::upgrade) {
        return Ok(lease);
    }
    let guard = foundation::process_lock::acquire_output_dir_lock(&target)
        .context("History session is active or its writer lease is unavailable")?;
    if let Some(connection) = open_history_read(directory)? {
        ensure_session_not_removed(&connection, session)?;
    }
    let lease = Arc::new(HistorySessionLease { _lock: guard });
    leases.retain(|_, lease| lease.strong_count() > 0);
    leases.insert(target, Arc::downgrade(&lease));
    Ok(lease)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedArtifact {
    relative_path: String,
    device: u64,
    inode: u64,
}

fn authoritative_log_names(session: &str) -> BTreeSet<String> {
    BTreeSet::from([
        format!("MFB_Session_{session}.log"),
        format!("verbose_{session}.log"),
        format!("session_audit_{session}.jsonl"),
    ])
}

fn session_archive_directory<'a>(root: &Path, path: &'a Path, session: &str) -> Option<&'a Path> {
    let parent = path.parent()?;
    if parent.parent() != Some(root) {
        return None;
    }
    let name = parent.file_name()?.to_str()?;
    let stamp = name
        .rsplit_once("_Bundle_")
        .map(|(_, stamp)| stamp)
        .or_else(|| name.strip_prefix("Bundle_"))?;
    let suffix = stamp.strip_prefix(session)?;
    (suffix.is_empty()
        || suffix.strip_prefix('_').is_some_and(|index| {
            !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
        }))
    .then_some(parent)
}

fn legacy_audit_exists(root: &Path, session: &str) -> Result<bool> {
    let name = format!("session_audit_{session}.jsonl");
    let has_audit = |directory: &Path| -> Result<bool> {
        match fs::symlink_metadata(directory.join(&name)) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    };
    if has_audit(root)? {
        return Ok(true);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let filename = entry.file_name();
        if filename.to_str().is_some_and(|name| {
            name.starts_with("Bundle_")
                || (name.contains("_Bundle_") && !name.starts_with("_Bundle_"))
        }) && entry.file_type()?.is_dir()
            && has_audit(&entry.path())?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_registered_manifest(root: &Path, path: &Path, session: &str) -> bool {
    path.file_name() == Some(std::ffi::OsStr::new("manifest.json"))
        && session_archive_directory(root, path, session).is_some()
}

fn relative_artifact(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .context("History artifact is outside its owned directory")?;
    ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "History artifact has an unsafe relative path"
    );
    ensure!(
        path.parent()
            .context("History artifact has no parent")?
            .canonicalize()?
            == path.parent().context("History artifact has no parent")?,
        "History artifact parent contains a symlink"
    );
    Ok(relative
        .to_str()
        .context("History artifact path is not UTF-8")?
        .to_owned())
}

fn artifact_file(path: &Path) -> Result<std::fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.uid() == unsafe { libc::geteuid() } && metadata.nlink() == 1,
        "History artifact is not an exclusively owned regular file"
    );
    Ok(file)
}

/// Register the session logs and, after archival, their exact archive manifest.
pub fn register_history_artifacts(
    directory: &Path,
    session: &str,
    paths: &[PathBuf],
) -> Result<()> {
    let _lease = acquire_history_session_lease(directory, session)?;
    ensure!(
        (3..=4).contains(&paths.len()),
        "History session requires three authoritative logs and at most one archive manifest"
    );
    let root = cleanup_root(directory)?;
    let expected = authoritative_log_names(session);
    let mut names = BTreeSet::new();
    let mut artifacts = Vec::new();
    let mut manifest_directory = None;
    for path in paths {
        // Normalize only the explicitly selected root alias, never a child link.
        let path = path
            .strip_prefix(directory)
            .map_or_else(|_| path.clone(), |relative| root.join(relative));
        let relative_path = relative_artifact(&root, &path)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("History artifact has no UTF-8 filename")?;
        if is_registered_manifest(&root, &path, session) {
            ensure!(
                manifest_directory.is_none(),
                "History archive has multiple manifests"
            );
            manifest_directory = path.parent().map(Path::to_path_buf);
        } else {
            ensure!(
                expected.contains(name) && names.insert(name.to_owned()),
                "History artifact identity is not a unique session-owned log"
            );
        }
        let metadata = artifact_file(&path)?.metadata()?;
        artifacts.push(OwnedArtifact {
            relative_path,
            device: metadata.dev(),
            inode: metadata.ino(),
        });
    }
    ensure!(
        names == expected,
        "History archive is missing an authoritative session log"
    );
    if let Some(manifest_directory) = manifest_directory {
        ensure!(
            artifacts.iter().all(|artifact| {
                root.join(&artifact.relative_path).parent() == Some(manifest_directory.as_path())
            }),
            "History archive manifest does not share the registered log directory"
        );
    }
    artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut connection = open_history_write(directory)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    claim_history_session(&transaction, session)?;
    let previous: String = transaction.query_row(
        "SELECT artifacts_json FROM history_session_ownership_v1 WHERE session_id=?1",
        [session],
        |row| row.get(0),
    )?;
    let previous: Vec<OwnedArtifact> = serde_json::from_str(&previous)?;
    if !previous.is_empty() {
        for old in &previous {
            let name = Path::new(&old.relative_path).file_name();
            ensure!(
                artifacts
                    .iter()
                    .any(
                        |artifact| Path::new(&artifact.relative_path).file_name() == name
                            && artifact.device == old.device
                            && artifact.inode == old.inode
                    ),
                "History archive replaced or lost an authoritative artifact"
            );
        }
    }
    let changed = transaction.execute(
        "UPDATE history_session_ownership_v1 SET artifacts_json=?2 WHERE session_id=?1 AND owner_pid=?3",
        params![session, serde_json::to_string(&artifacts)?, i64::from(std::process::id())],
    )?;
    ensure!(
        changed == 1,
        "History artifact registration has no matching session owner"
    );
    transaction.commit()?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRunSelection {
    pub session_id: String,
    pub run_id: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryCleanupRequest {
    pub schema_version: u8,
    pub runs: Vec<HistoryRunSelection>,
    #[serde(default)]
    pub snapshot_token: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryForceCleanupRequest {
    pub schema_version: u8,
    pub force_sessions: Vec<String>,
    #[serde(default)]
    pub snapshot_token: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryForceCleanupPreview {
    pub schema_version: u8,
    pub kind: &'static str,
    pub source: String,
    pub snapshot_token: String,
    pub force_sessions: Vec<String>,
    pub event_rows: u64,
    pub output_rows: u64,
    pub retained_artifacts: Vec<HistoryArtifactDecision>,
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryForceCleanupReceipt {
    pub schema_version: u8,
    pub kind: &'static str,
    pub status: &'static str,
    pub database_committed: bool,
    pub force_sessions: Vec<String>,
    pub event_rows: u64,
    pub output_rows: u64,
    pub retained_artifacts: Vec<HistoryArtifactDecision>,
    pub failed_artifacts: Vec<HistoryArtifactDecision>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct HistoryArtifactDecision {
    pub path: String,
    pub scope: &'static str,
    pub status: &'static str,
    pub reason: String,
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryCleanupPreview {
    pub schema_version: u8,
    pub kind: &'static str,
    pub source: String,
    pub snapshot_token: String,
    pub runs: Vec<HistoryRunSelection>,
    pub event_rows: u64,
    pub output_rows: u64,
    pub event_bytes: u64,
    pub output_bytes: u64,
    pub artifacts: Vec<HistoryArtifactDecision>,
    pub can_execute: bool,
    pub external_log_scope: &'static str,
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryCleanupReceipt {
    pub schema_version: u8,
    pub kind: &'static str,
    pub status: &'static str,
    pub database_committed: bool,
    pub removed_runs: u64,
    pub event_rows: u64,
    pub output_rows: u64,
    pub removed_artifacts: u64,
    pub retained_artifacts: Vec<HistoryArtifactDecision>,
    pub failed_artifacts: Vec<HistoryArtifactDecision>,
}

struct RunRange {
    selection: HistoryRunSelection,
    event_start: i64,
    event_end: i64,
    output_start: i64,
    output_end: i64,
}

struct DeletableArtifact {
    path: PathBuf,
    device: u64,
    inode: u64,
    hash: String,
}

struct EmptyArchive {
    path: PathBuf,
    session: String,
    device: u64,
    inode: u64,
}

struct CleanupPlan {
    preview: HistoryCleanupPreview,
    ranges: Vec<RunRange>,
    deletable: Vec<DeletableArtifact>,
    empty_archives: Vec<EmptyArchive>,
}

fn cleanup_root(directory: &Path) -> Result<PathBuf> {
    ensure!(
        directory.is_absolute(),
        "History cleanup directory must be explicit and absolute"
    );
    let metadata = fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "History cleanup directory must be a real directory"
    );
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
        "History cleanup directory is not privately owned"
    );
    let root = directory.canonicalize()?;
    foundation::safety::check_dangerous_directory(&root).map_err(anyhow::Error::msg)?;
    foundation::safety::check_apple_photos_library(&root).map_err(anyhow::Error::msg)?;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let path = root.join(format!("{HISTORY_DATABASE}{suffix}"));
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.nlink() == 1,
                "History database or sidecar is not an exclusively owned regular file"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect History cleanup database or sidecar"),
        }
    }
    ensure!(
        root.parent().is_some(),
        "History cleanup refuses the filesystem root"
    );
    for variable in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(variable) {
            ensure!(
                root.as_path() != Path::new(&home),
                "History cleanup refuses a home directory"
            );
        }
    }
    Ok(root)
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryReadOnlyAccessReceipt {
    pub schema_version: u32,
    pub kind: &'static str,
    pub source: String,
    pub history_schema_version: i64,
}

/// Re-establish SQLite's read-only access files without changing history rows or schema.
pub fn prepare_history_read_only_access(directory: &Path) -> Result<HistoryReadOnlyAccessReceipt> {
    let root = cleanup_root(directory)?;
    let source = root.join(HISTORY_DATABASE);
    let mut connection = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .context("open existing History database to prepare read-only access")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA query_only=ON;")?;
    persist_history_wal_sidecars(&connection)?;
    let snapshot = connection.transaction()?;
    let version = schema_version(&snapshot)?;
    let journal: String = snapshot.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    snapshot.commit()?;
    connection.close().map_err(|(_, error)| error)?;
    if journal.eq_ignore_ascii_case("wal") {
        ensure!(
            root.join(format!("{HISTORY_DATABASE}-wal")).is_file()
                && root.join(format!("{HISTORY_DATABASE}-shm")).is_file(),
            "History read-only access files were not retained"
        );
    }
    let mut reader = open_history_read(&root)?.context("History database disappeared")?;
    let snapshot = reader.transaction()?;
    ensure!(
        schema_version(&snapshot)? == version,
        "History schema changed during preparation"
    );
    snapshot.commit()?;
    Ok(HistoryReadOnlyAccessReceipt {
        schema_version: 1,
        kind: "history_readonly_access",
        source: source
            .to_str()
            .context("History source path is not UTF-8")?
            .to_owned(),
        history_schema_version: version,
    })
}

fn selections(request: &HistoryCleanupRequest) -> Result<BTreeSet<HistoryRunSelection>> {
    ensure!(
        request.schema_version == 1,
        "unsupported History cleanup request version"
    );
    ensure!(
        !request.runs.is_empty() && request.runs.len() <= MAX_SELECTIONS,
        "History cleanup requires 1..=128 selected runs"
    );
    let mut selected = BTreeSet::new();
    for run in &request.runs {
        identity(&run.session_id)?;
        identity(&run.run_id)?;
        ensure!(
            selected.insert(run.clone()),
            "History cleanup contains duplicate run identities"
        );
    }
    Ok(selected)
}

fn cleanup_session_locks<'a>(
    root: &Path,
    sessions: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<foundation::process_lock::DirLock>> {
    sessions.into_iter().collect::<BTreeSet<_>>().into_iter().map(|session| {
        foundation::process_lock::acquire_output_dir_lock(&lock_target(root, session)?)
            .with_context(|| format!("History session {session} is active or its writer lease is unavailable; nothing removed"))
    }).collect()
}

fn cleanup_locks(
    root: &Path,
    selected: &BTreeSet<HistoryRunSelection>,
) -> Result<Vec<foundation::process_lock::DirLock>> {
    cleanup_session_locks(root, selected.iter().map(|run| run.session_id.as_str()))
}

fn validate_cleanup_schema(snapshot: &Transaction<'_>) -> Result<()> {
    ensure!(
        schema_version(snapshot)? >= 2,
        "History cleanup requires schema 2 or newer; original retained"
    );
    validate_cleanup_objects(snapshot, 2)
}

fn validate_cleanup_objects(snapshot: &Transaction<'_>, version: i64) -> Result<()> {
    validate_backup_schema(snapshot, version)?;
    let mut objects = snapshot
        .prepare("SELECT type,name,tbl_name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'")?;
    for row in objects.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (kind, name, table) = row?;
        ensure!(
            matches!(kind.as_str(), "table" | "index"),
            "History cleanup refuses triggers or views"
        );
        ensure!(
            [
                "history_sessions",
                "history_events",
                "history_output",
                OWNER_TABLE,
                REMOVED_TABLE
            ]
            .contains(&table.as_str()),
            "History cleanup refuses unknown database objects: {name}"
        );
    }
    if owner_table_exists(snapshot)? {
        validate_owner_table(snapshot)?;
    }
    removed_table_exists(snapshot)?;
    Ok(())
}

fn legacy_session_is_inactive(session: &str, runs: &[RunRange]) -> Result<()> {
    let mut pieces = session.rsplitn(3, '_');
    let nonce = pieces
        .next()
        .context("legacy History session has no process nonce")?;
    let pid = pieces
        .next()
        .context("legacy History session has no process identity")?
        .parse::<i32>()
        .context("legacy History process identity is invalid")?;
    ensure!(
        pieces.next().is_some()
            && !nonce.is_empty()
            && nonce.bytes().all(|byte| byte.is_ascii_digit())
            && pid > 0,
        "legacy History activity cannot be proven inactive"
    );
    for range in runs {
        ensure!(
            range.selection.run_id.starts_with(&format!("run-{pid}-")),
            "legacy History run and process identity disagree"
        );
    }
    // SAFETY: signal zero only checks existence/permission; it never sends a signal.
    let status = unsafe { libc::kill(pid, 0) };
    let error = std::io::Error::last_os_error();
    ensure!(
        status == -1 && error.raw_os_error() == Some(libc::ESRCH),
        "legacy History writer is alive or its activity is unknown; nothing removed"
    );
    Ok(())
}

fn registered_writer_is_inactive(pid: i64) -> Result<()> {
    let pid = i32::try_from(pid).context("History owner process identity is invalid")?;
    ensure!(pid > 0, "History owner process identity is invalid");
    // This process's shared lock registry already excludes its live writers.
    if pid == i32::try_from(std::process::id())? {
        return Ok(());
    }
    // Locks can reside under different MFB roots; a live owner remains protected.
    let status = unsafe { libc::kill(pid, 0) };
    let error = std::io::Error::last_os_error();
    ensure!(
        status == -1 && error.raw_os_error() == Some(libc::ESRCH),
        "History writer is alive or its activity is unknown; nothing removed"
    );
    Ok(())
}

fn hash_field(hasher: &mut blake3::Hasher, value: &[u8], budget: &mut usize) -> Result<()> {
    *budget = budget
        .checked_add(value.len())
        .context("History snapshot byte count overflow")?;
    ensure!(
        *budget <= SNAPSHOT_BYTE_LIMIT,
        "History cleanup snapshot exceeds its bounded read budget"
    );
    hash_snapshot_field(hasher, value)
}

fn hash_snapshot_field(hasher: &mut blake3::Hasher, value: &[u8]) -> Result<()> {
    hasher.update(&u64::try_from(value.len())?.to_le_bytes());
    hasher.update(value);
    Ok(())
}

fn checked_add(total: &mut u64, count: usize) -> Result<()> {
    *total = total
        .checked_add(u64::try_from(count)?)
        .context("History cleanup count overflow")?;
    Ok(())
}

fn file_hash(path: &Path, device: u64, inode: u64) -> Result<String> {
    let mut file = artifact_file(path)?;
    let before = file.metadata()?;
    ensure!(
        before.dev() == device && before.ino() == inode,
        "registered History artifact identity changed"
    );
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes = file.read(&mut buffer)?;
        if bytes == 0 {
            break;
        }
        hasher.update(&buffer[..bytes]);
    }
    let after = file.metadata()?;
    ensure!(
        before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "History artifact changed while hashing"
    );
    Ok(hasher.finalize().to_hex().to_string())
}

fn build_plan(
    root: &Path,
    snapshot: &Transaction<'_>,
    request: &HistoryCleanupRequest,
) -> Result<CleanupPlan> {
    validate_cleanup_schema(snapshot)?;
    let selected = selections(request)?;
    let output_high_water: Option<i64> = snapshot
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='history_output'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(sequence) = output_high_water {
        ensure!(
            sequence >= 0,
            "History output high-water sequence is invalid"
        );
    } else {
        // AUTOINCREMENT has no sqlite_sequence row before the first output
        // insertion. Keep that state distinct from an allocated sequence of zero;
        // absence is valid only when no output rows require sequence custody.
        let has_output: bool =
            snapshot.query_row("SELECT EXISTS(SELECT 1 FROM history_output)", [], |row| {
                row.get(0)
            })?;
        ensure!(
            !has_output,
            "History output high-water sequence is missing despite persisted output rows"
        );
    }
    let database = fs::symlink_metadata(root.join(HISTORY_DATABASE))?;
    ensure!(
        database.is_file() && !database.file_type().is_symlink(),
        "History database identity is not a regular file"
    );
    let mut hasher = blake3::Hasher::new();
    let mut budget = 0_usize;
    hash_field(
        &mut hasher,
        root.as_os_str().as_encoded_bytes(),
        &mut budget,
    )?;
    hash_field(&mut hasher, &database.dev().to_le_bytes(), &mut budget)?;
    hash_field(&mut hasher, &database.ino().to_le_bytes(), &mut budget)?;
    hash_field(&mut hasher, &serde_json::to_vec(&selected)?, &mut budget)?;
    let mut ranges = Vec::new();
    let mut deletable = Vec::new();
    let mut empty_archives = Vec::new();
    let mut preview = HistoryCleanupPreview {
        schema_version: 1,
        kind: "history_cleanup_preview",
        source: root
            .join(HISTORY_DATABASE)
            .to_str()
            .context("History database path is not UTF-8")?
            .to_owned(),
        snapshot_token: String::new(),
        runs: selected.iter().cloned().collect(),
        event_rows: 0,
        output_rows: 0,
        event_bytes: 0,
        output_bytes: 0,
        artifacts: Vec::new(),
        can_execute: true,
        external_log_scope: "registered_session_logs_only",
    };
    let mut selected_ids = BTreeSet::new();
    for target in &selected {
        ensure!(
            selected_ids.insert(&target.run_id),
            "History run identity is assigned to multiple sessions"
        );
        let owners: i64 = snapshot.query_row(
            "SELECT COUNT(*) FROM history_events WHERE event LIKE 'MFB_HISTORY_CONTEXT=%'
             AND json_extract(substr(event,instr(event,'=')+1),'$.run_id')=?1",
            [&target.run_id],
            |row| row.get(0),
        )?;
        ensure!(
            owners == 1,
            "History run identity is missing, duplicated or ambiguous"
        );
    }
    for session in selected
        .iter()
        .map(|run| &run.session_id)
        .collect::<BTreeSet<_>>()
    {
        let updated: String = snapshot.query_row(
            "SELECT updated_at FROM history_sessions WHERE session_id=?1",
            [session],
            |row| row.get(0),
        )?;
        hash_field(&mut hasher, session.as_bytes(), &mut budget)?;
        hash_field(&mut hasher, updated.as_bytes(), &mut budget)?;
        let oversized: i64 = snapshot.query_row(
            "SELECT COUNT(*) FROM history_events WHERE session_id=?1 AND length(CAST(event AS BLOB))>8388608",
            [session], |row| row.get(0),
        )?;
        ensure!(
            oversized == 0,
            "History event exceeds the cleanup parser bound"
        );
        let mut active: Option<(String, i64, i64)> = None;
        let mut seen = BTreeSet::new();
        let mut session_ranges = Vec::new();
        let mut event_count = 0_u64;
        let mut previous_output = 0_i64;
        let mut query = snapshot.prepare(
            "SELECT sequence,ts,event FROM history_events WHERE session_id=?1 ORDER BY sequence",
        )?;
        let events = query.query_map([session], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for event in events {
            let (sequence, timestamp, text) = event?;
            ensure!(sequence > 0, "History event sequence is invalid");
            event_count = event_count
                .checked_add(1)
                .context("History event count overflow")?;
            hash_field(&mut hasher, &sequence.to_le_bytes(), &mut budget)?;
            hash_field(&mut hasher, timestamp.as_bytes(), &mut budget)?;
            hash_field(&mut hasher, text.as_bytes(), &mut budget)?;
            let (name, payload) = text
                .split_once('=')
                .context("History event payload is unavailable")?;
            let payload: serde_json::Value =
                serde_json::from_str(payload).context("History event JSON is invalid")?;
            ensure!(
                payload["schema_version"] == 1,
                "unsupported History event schema"
            );
            let run_id = payload["run_id"]
                .as_str()
                .context("History event has no authoritative run identity")?;
            identity(run_id)?;
            match name {
                "MFB_HISTORY_CONTEXT" => {
                    ensure!(
                        active.is_none() && seen.insert(run_id.to_owned()),
                        "History run contexts overlap or repeat"
                    );
                    let cursor = payload["output_sequence"]
                        .as_i64()
                        .context("History context has no exact output cursor")?;
                    ensure!(
                        cursor >= previous_output,
                        "History output boundaries overlap or move backwards"
                    );
                    active = Some((run_id.to_owned(), sequence, cursor));
                }
                "MFB_HISTORY_FINISHED" => {
                    let (started, first, begin) = active
                        .take()
                        .context("History finished event has no preceding context")?;
                    ensure!(
                        started == run_id,
                        "History terminal run identity conflicts with context"
                    );
                    let end = payload["output_sequence"]
                        .as_i64()
                        .context("History terminal event has no exact output cursor")?;
                    ensure!(end >= begin, "History output cursor moved backwards");
                    ensure!(
                        matches!(
                            payload["outcome"].as_str(),
                            Some("completed" | "failed" | "cancelled")
                        ),
                        "History run has no supported terminal outcome"
                    );
                    session_ranges.push(RunRange {
                        selection: HistoryRunSelection {
                            session_id: session.clone(),
                            run_id: started,
                        },
                        event_start: first,
                        event_end: sequence,
                        output_start: begin,
                        output_end: end,
                    });
                    previous_output = end;
                }
                _ => {
                    ensure!(
                        active
                            .as_ref()
                            .is_some_and(|(started, _, _)| started == run_id),
                        "History event is outside its uniquely bound run"
                    );
                }
            }
        }
        for target in selected.iter().filter(|run| &run.session_id == session) {
            ensure!(
                session_ranges
                    .iter()
                    .filter(|range| &range.selection == target)
                    .count()
                    == 1,
                "selected History run is unfinished, absent or ambiguous"
            );
        }
        let (owner_pid, registered) = if owner_table_exists(snapshot)? {
            snapshot.query_row(
                "SELECT owner_pid,artifacts_json FROM history_session_ownership_v1 WHERE session_id=?1",
                [session], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            ).optional()?.map_or((None,None), |(pid, data)| (Some(pid),Some(data)))
        } else {
            (None, None)
        };
        if let Some(pid) = owner_pid {
            registered_writer_is_inactive(pid)?;
            hash_field(&mut hasher, &pid.to_le_bytes(), &mut budget)?;
        } else {
            legacy_session_is_inactive(session, &session_ranges)?;
        }
        if let Some(data) = &registered {
            hash_field(&mut hasher, data.as_bytes(), &mut budget)?;
        }
        let wanted: Vec<_> = session_ranges
            .into_iter()
            .filter(|range| selected.contains(&range.selection))
            .collect();
        let mut selected_events = 0_u64;
        for range in &wanted {
            let (count, bytes): (u64,u64) = snapshot.query_row(
                "SELECT COUNT(*),COALESCE(SUM(length(CAST(event AS BLOB))),0) FROM history_events WHERE session_id=?1 AND sequence>=?2 AND sequence<=?3",
                params![session,range.event_start,range.event_end], |row| Ok((row.get(0)?,row.get(1)?)),
            )?;
            selected_events = selected_events
                .checked_add(count)
                .context("History selected event count overflow")?;
            preview.event_rows = preview
                .event_rows
                .checked_add(count)
                .context("History event count overflow")?;
            preview.event_bytes = preview
                .event_bytes
                .checked_add(bytes)
                .context("History event byte count overflow")?;
        }
        let mut output_count = 0_u64;
        let mut selected_outputs = 0_u64;
        let mut query = snapshot.prepare("SELECT sequence,ts,pipeline,line FROM history_output WHERE session_id=?1 ORDER BY sequence")?;
        let mut outputs = query.query([session])?;
        while let Some(row) = outputs.next()? {
            let sequence: i64 = row.get(0)?;
            let timestamp = row.get_ref(1)?.as_str()?;
            let pipeline = row.get_ref(2)?.as_str()?;
            let line = row.get_ref(3)?.as_str()?;
            ensure!(
                sequence > 0 && line.len() <= 8 * 1024 * 1024,
                "History output record is invalid or oversized"
            );
            output_count = output_count
                .checked_add(1)
                .context("History output count overflow")?;
            hash_field(&mut hasher, &sequence.to_le_bytes(), &mut budget)?;
            hash_field(&mut hasher, timestamp.as_bytes(), &mut budget)?;
            hash_field(&mut hasher, pipeline.as_bytes(), &mut budget)?;
            // Output is streamed from one SQLite row and hashed without copying its payload.
            hash_snapshot_field(&mut hasher, line.as_bytes())?;
            if wanted
                .iter()
                .any(|range| sequence > range.output_start && sequence <= range.output_end)
            {
                selected_outputs = selected_outputs
                    .checked_add(1)
                    .context("History selected output count overflow")?;
                checked_add(&mut preview.output_rows, 1)?;
                checked_add(&mut preview.output_bytes, line.len())?;
            }
        }
        ensure!(
            match output_high_water {
                Some(sequence) => previous_output <= sequence,
                None => previous_output == 0,
            },
            "History output boundary exceeds captured output high-water sequence"
        );
        let entire_session =
            active.is_none() && selected_events == event_count && selected_outputs == output_count;
        let artifacts: Vec<OwnedArtifact> = registered
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .context("History artifact registration is invalid")?
            .unwrap_or_default();
        ensure!(
            artifacts.len() <= 4,
            "History artifact registration exceeds its authoritative scope"
        );
        let mut paths = BTreeSet::new();
        let names = authoritative_log_names(session);
        let first_deletable = deletable.len();
        for artifact in artifacts {
            let path = root.join(&artifact.relative_path);
            ensure!(
                paths.insert(artifact.relative_path.clone()),
                "History artifacts repeat a path"
            );
            relative_artifact(root, &path)?;
            ensure!(
                is_registered_manifest(root, &path, session)
                    || path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| names.contains(name)),
                "History ownership registry names a non-log artifact; nothing removed"
            );
            let mut decision = HistoryArtifactDecision {
                path: path
                    .to_str()
                    .context("History artifact path is not UTF-8")?
                    .to_owned(),
                scope: "registered_file",
                status: "retained",
                reason: "shared with unselected or unfinished session records".to_owned(),
            };
            if entire_session {
                match file_hash(&path, artifact.device, artifact.inode) {
                    Ok(hash) => {
                        decision.status = "delete";
                        decision.reason = "registered session file; every database run and output row is selected".to_owned();
                        hash_field(&mut hasher, hash.as_bytes(), &mut budget)?;
                        deletable.push(DeletableArtifact {
                            path,
                            device: artifact.device,
                            inode: artifact.inode,
                            hash,
                        });
                    }
                    Err(error) => {
                        decision.scope = "registered_file_problem";
                        decision.reason = format!("registered artifact retained: {error:#}")
                    }
                }
            }
            hash_field(
                &mut hasher,
                serde_json::to_string(&decision)?.as_bytes(),
                &mut budget,
            )?;
            preview.artifacts.push(decision);
        }
        let mut unverified_archive = None;
        if let Some(directory) = paths.iter().find_map(|relative| {
            let path = root.join(relative);
            session_archive_directory(root, &path, session).map(Path::to_path_buf)
        }) {
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if !path
                    .strip_prefix(root)?
                    .to_str()
                    .is_some_and(|relative| paths.contains(relative))
                {
                    unverified_archive = Some(directory.clone());
                    break;
                }
            }
            if unverified_archive.is_none()
                && deletable.len() - first_deletable == 4
                && deletable[first_deletable..]
                    .iter()
                    .all(|artifact| artifact.path.parent() == Some(directory.as_path()))
            {
                let metadata = fs::symlink_metadata(&directory)?;
                ensure!(
                    metadata.is_dir()
                        && metadata.uid() == unsafe { libc::geteuid() }
                        && metadata.mode() & 0o022 == 0,
                    "History archive is not a privately owned directory"
                );
                hash_field(&mut hasher, &metadata.dev().to_le_bytes(), &mut budget)?;
                hash_field(&mut hasher, &metadata.ino().to_le_bytes(), &mut budget)?;
                empty_archives.push(EmptyArchive {
                    path: directory,
                    session: session.clone(),
                    device: metadata.dev(),
                    inode: metadata.ino(),
                });
            }
        }
        let decision = unverified_archive.map_or_else(
            || HistoryArtifactDecision {
                path: root.display().to_string(),
                scope: "unregistered_external_logs",
                status: "retained",
                reason: "Only logs with verified ownership are eligible for removal.".to_owned(),
            },
            |directory| HistoryArtifactDecision {
                path: directory.display().to_string(),
                scope: "unverified_archive",
                status: "retained",
                reason: "The archive contains files without verified ownership.".to_owned(),
            },
        );
        hash_field(&mut hasher, &serde_json::to_vec(&decision)?, &mut budget)?;
        preview.artifacts.push(decision);
        ranges.extend(wanted);
    }
    preview.snapshot_token = hasher.finalize().to_hex().to_string();
    Ok(CleanupPlan {
        preview,
        ranges,
        deletable,
        empty_archives,
    })
}

/// Read an exact selection-bound snapshot; only central advisory lock files may be created.
pub fn preview_history_cleanup(
    directory: &Path,
    request: &HistoryCleanupRequest,
) -> Result<HistoryCleanupPreview> {
    ensure!(
        request.snapshot_token.is_none(),
        "History cleanup preview must not supply a stale token"
    );
    let selected = selections(request)?;
    let root = cleanup_root(directory)?;
    let _locks = cleanup_locks(&root, &selected)?;
    let mut connection =
        open_history_read(&root)?.context("History database is absent; nothing removed")?;
    let snapshot = connection.transaction()?;
    let plan = build_plan(&root, &snapshot, request)?;
    snapshot.commit()?;
    Ok(plan.preview)
}

fn restore_staged(staged: &[(PathBuf, PathBuf)]) -> Vec<HistoryArtifactDecision> {
    let mut failures = Vec::new();
    for (original, recovery) in staged.iter().rev() {
        match fs::hard_link(recovery, original).and_then(|()| fs::remove_file(recovery)) {
            Ok(()) => {},
            Err(error) => failures.push(HistoryArtifactDecision {
                path: recovery.display().to_string(), scope:"registered_file", status:"failed",
                reason:format!("History database operation failed; owned log retained here because restoration to {} failed: {error}",original.display()),
            }),
        }
    }
    failures
}

fn failure_with_recovery(error: anyhow::Error, staged: &[(PathBuf, PathBuf)]) -> anyhow::Error {
    let failures = restore_staged(staged);
    if failures.is_empty() {
        error
    } else {
        error.context(format!(
            "History log recovery requires attention: {}",
            serde_json::to_string(&failures).unwrap_or_else(|serialization| format!(
                "recovery receipt serialization failed: {serialization}"
            ))
        ))
    }
}

fn truncate_history_wal(connection: &Connection) -> Result<()> {
    let state: (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    ensure!(
        matches!(state, (0, 0, 0) | (0, -1, -1)),
        "History WAL could not be truncated, possibly because another reader is active"
    );
    Ok(())
}

fn compact_legacy_history(connection: &mut Connection) -> Result<()> {
    let version =
        |db: &Connection| db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0));
    if version(connection)? == SCHEMA_VERSION {
        return Ok(());
    }

    let journal: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if journal.eq_ignore_ascii_case("wal") {
        // An exclusive WAL connection rules out an already-open legacy writer before certification.
        connection.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
        connection.busy_timeout(Duration::ZERO)?;
        let acquired = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .and_then(rusqlite::Transaction::commit);
        connection.busy_timeout(Duration::from_secs(5))?;
        match acquired {
            Ok(()) => {
                if version(connection)? < SCHEMA_VERSION {
                    connection.execute_batch("VACUUM")?;
                    truncate_history_wal(connection)?;
                    #[cfg(test)]
                    tests::pause_for_crash_test("after_legacy_checkpoint");
                    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                }
                return Ok(());
            }
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) =>
            {
                connection.pragma_update(None, "locking_mode", "NORMAL")?;
            }
            Err(error) => return Err(error).context("acquire exclusive History privacy migration"),
        }
    }
    // Existing readers keep legacy storage readable. Scrub now, and certify on a later exclusive pass.
    connection.execute_batch("VACUUM")?;
    Ok(())
}

/// Delete only a revalidated selection and its proven, exclusively owned log files.
pub fn execute_history_cleanup(
    directory: &Path,
    request: &HistoryCleanupRequest,
) -> Result<HistoryCleanupReceipt> {
    let token = request
        .snapshot_token
        .as_deref()
        .context("History cleanup requires the exact preview token")?;
    ensure!(
        token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "History cleanup token is invalid"
    );
    let selected = selections(request)?;
    let root = cleanup_root(directory)?;
    let _locks = cleanup_locks(&root, &selected)?;
    let mut connection = Connection::open_with_flags(
        root.join(HISTORY_DATABASE),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON;",
    )?;
    let secure_delete: i64 =
        connection.pragma_query_value(None, "secure_delete", |row| row.get(0))?;
    ensure!(secure_delete == 1, "History secure deletion is unavailable");
    persist_history_wal_sidecars(&connection)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let plan = build_plan(&root, &transaction, request)?;
    ensure!(
        plan.preview.snapshot_token == token,
        "History selection or its artifacts changed since preview; nothing removed"
    );
    // Take SQLite's write lease on this exact snapshot before staging files.
    let changed = transaction
        .execute("UPDATE history_sessions SET updated_at=updated_at WHERE 0", [])
        .map_err(|error| {
            let context = match error.sqlite_error_code() {
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                    "MFB_HISTORY_CLEANUP_BUSY: another operation is writing History; no rows or files were removed"
                }
                _ => "begin History cleanup write transaction",
            };
            anyhow::Error::from(error).context(context)
        })?;
    ensure!(changed == 0, "History write lease acquisition changed rows");
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    let stage_result = (|| -> Result<()> {
        for artifact in &plan.deletable {
            ensure!(
                file_hash(&artifact.path, artifact.device, artifact.inode)? == artifact.hash,
                "History artifact changed after preview"
            );
            let reserved =
                foundation::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
                    "History cleanup recovery staging",
                    artifact
                        .path
                        .parent()
                        .context("History artifact has no parent")?,
                    ".mfb-history-cleanup-",
                    ".pending",
                )?;
            let (file, recovery) = reserved
                .keep()
                .context("reserve persistent History recovery path")?;
            file.sync_all()?;
            drop(file);
            if let Err(error) = fs::rename(&artifact.path, &recovery) {
                let cleanup = fs::remove_file(&recovery);
                if let Err(cleanup) = cleanup {
                    return Err(error).context(format!(
                        "unused owned recovery file could not be removed at {}: {cleanup}",
                        recovery.display()
                    ));
                }
                return Err(error).context("stage selected History artifact");
            }
            staged.push((artifact.path.clone(), recovery.clone()));
            ensure!(
                file_hash(&recovery, artifact.device, artifact.inode)? == artifact.hash,
                "History artifact changed during staging; database retained"
            );
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        return Err(failure_with_recovery(error, &staged));
    }
    let deletion = (|| -> Result<()> {
        let mut events = 0_u64;
        let mut outputs = 0_u64;
        for range in &plan.ranges {
            checked_add(&mut events,transaction.execute(
                "DELETE FROM history_events WHERE session_id=?1 AND sequence>=?2 AND sequence<=?3",
                params![range.selection.session_id,range.event_start,range.event_end],
            )?)?;
            checked_add(&mut outputs,transaction.execute(
                "DELETE FROM history_output WHERE session_id=?1 AND sequence>?2 AND sequence<=?3",
                params![range.selection.session_id,range.output_start,range.output_end],
            )?)?;
        }
        ensure!(
            events == plan.preview.event_rows && outputs == plan.preview.output_rows,
            "History deletion did not match the approved row counts"
        );
        for session in selected
            .iter()
            .map(|run| &run.session_id)
            .collect::<BTreeSet<_>>()
        {
            let remaining: i64 = transaction.query_row(
                "SELECT (SELECT COUNT(*) FROM history_events WHERE session_id=?1)+(SELECT COUNT(*) FROM history_output WHERE session_id=?1)",
                [session], |row| row.get(0),
            )?;
            if remaining == 0 {
                if !removed_table_exists(&transaction)? {
                    transaction.execute_batch("CREATE TABLE history_removed_sessions_v1(session_id TEXT PRIMARY KEY NOT NULL);")?;
                }
                transaction.execute(
                    "INSERT INTO history_removed_sessions_v1(session_id) VALUES(?1)",
                    [session],
                )?;
            }
        }
        Ok(())
    })();
    if let Err(error) = deletion {
        return Err(failure_with_recovery(error, &staged));
    }
    #[cfg(test)]
    tests::pause_for_crash_test("before_commit");
    if let Err(error) = transaction.commit() {
        return Err(failure_with_recovery(error.into(), &staged));
    }
    #[cfg(test)]
    tests::pause_for_crash_test("after_commit");
    let mut receipt = HistoryCleanupReceipt {
        schema_version: 1,
        kind: "history_cleanup_receipt",
        status: "completed",
        database_committed: true,
        removed_runs: u64::try_from(plan.ranges.len())?,
        event_rows: plan.preview.event_rows,
        output_rows: plan.preview.output_rows,
        removed_artifacts: 0,
        retained_artifacts: plan
            .preview
            .artifacts
            .into_iter()
            .filter(|item| item.status == "retained")
            .collect(),
        failed_artifacts: Vec::new(),
    };
    for (original, recovery) in &staged {
        match fs::remove_file(recovery) {
            Ok(()) => checked_add(&mut receipt.removed_artifacts,1)?,
            Err(error) => receipt.failed_artifacts.push(HistoryArtifactDecision {
                path:recovery.display().to_string(),scope:"registered_file",status:"failed",
                reason:format!("database rows were removed; owned log from {} remains at this recovery path: {error}",original.display()),
            }),
        }
    }
    let mut emptied_sessions = BTreeSet::new();
    for archive in plan.empty_archives {
        if receipt
            .failed_artifacts
            .iter()
            .any(|failure| Path::new(&failure.path).parent() == Some(archive.path.as_path()))
        {
            continue;
        }
        let remove = (|| -> Result<()> {
            let metadata = fs::symlink_metadata(&archive.path)?;
            ensure!(
                metadata.is_dir()
                    && metadata.dev() == archive.device
                    && metadata.ino() == archive.inode,
                "History archive directory identity changed"
            );
            // Never recursively remove a directory, even if it was empty at preview time.
            fs::remove_dir(&archive.path)?;
            Ok(())
        })();
        match remove.and_then(|()| {
            // The legacy reader needs the original marker if an unregistered audit survives.
            ensure!(
                !legacy_audit_exists(&root, &archive.session)?,
                "An unregistered legacy audit still exists for this session"
            );
            Ok(())
        }) {
            Ok(()) => {
                emptied_sessions.insert(archive.session);
            }
            Err(error) => receipt.failed_artifacts.push(HistoryArtifactDecision {
                path: archive.path.display().to_string(),
                scope: "archive_directory",
                status: "failed",
                reason: format!("History archive cleanup could not be fully verified: {error:#}"),
            }),
        }
    }
    let finish = (|| -> Result<()> {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for session in selected
            .iter()
            .map(|run| &run.session_id)
            .collect::<BTreeSet<_>>()
        {
            if owner_table_exists(&transaction)? {
                let encoded: Option<String> = transaction.query_row("SELECT artifacts_json FROM history_session_ownership_v1 WHERE session_id=?1",[session],|row|row.get(0)).optional()?;
                if let Some(encoded) = encoded {
                    let mut artifacts: Vec<OwnedArtifact> = serde_json::from_str(&encoded)?;
                    let mut retained = Vec::new();
                    for mut artifact in artifacts.drain(..) {
                        let original = root.join(&artifact.relative_path);
                        if let Some((_, recovery)) =
                            staged.iter().find(|(path, _)| path == &original)
                        {
                            if receipt
                                .failed_artifacts
                                .iter()
                                .any(|failure| failure.path == recovery.display().to_string())
                            {
                                artifact.relative_path = relative_artifact(&root, recovery)?;
                                retained.push(artifact);
                            }
                        } else {
                            retained.push(artifact);
                        }
                    }
                    if retained.is_empty() {
                        transaction.execute(
                            "DELETE FROM history_session_ownership_v1 WHERE session_id=?1",
                            [session],
                        )?;
                    } else {
                        transaction.execute("UPDATE history_session_ownership_v1 SET artifacts_json=?2 WHERE session_id=?1",params![session,serde_json::to_string(&retained)?])?;
                    }
                }
            }
            let owned = if owner_table_exists(&transaction)? {
                transaction.query_row(
                    "SELECT COUNT(*) FROM history_session_ownership_v1 WHERE session_id=?1",
                    [session],
                    |row| row.get::<_, i64>(0),
                )?
            } else {
                0
            };
            let remaining: i64 = transaction.query_row(
                "SELECT (SELECT COUNT(*) FROM history_events WHERE session_id=?1)+(SELECT COUNT(*) FROM history_output WHERE session_id=?1)",
                [session], |row| row.get(0),
            )?;
            if remaining == 0 && owned == 0 {
                transaction.execute(
                    "DELETE FROM history_sessions WHERE session_id=?1",
                    [session],
                )?;
                if emptied_sessions.contains(session) {
                    transaction.execute(
                        "UPDATE history_removed_sessions_v1 SET session_id=?2 WHERE session_id=?1",
                        params![session, retired_session_digest(session)],
                    )?;
                }
            }
        }
        transaction.commit()?;
        Ok(())
    })();
    if let Err(error) = finish {
        receipt.failed_artifacts.push(HistoryArtifactDecision {
            path: root.join(HISTORY_DATABASE).display().to_string(),
            scope: "ownership_registry",
            status: "failed",
            reason: format!(
                "selected rows were removed; ownership registry finalization failed: {error:#}"
            ),
        });
    }
    let compact = compact_legacy_history(&mut connection);
    let checkpoint = truncate_history_wal(&connection);
    if compact.is_err() || checkpoint.is_err() {
        receipt.failed_artifacts.push(HistoryArtifactDecision {
            path: root.join(HISTORY_DATABASE).display().to_string(),
            scope: "database_privacy",
            status: "failed",
            reason: [compact.err(), checkpoint.err()]
                .into_iter()
                .flatten()
                .map(|error| format!("{error:#}"))
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    if !receipt.failed_artifacts.is_empty()
        || receipt
            .retained_artifacts
            .iter()
            .any(|artifact| artifact.scope != "unregistered_external_logs")
    {
        receipt.status = "partial";
    }
    Ok(receipt)
}

fn force_selections(request: &HistoryForceCleanupRequest) -> Result<BTreeSet<&str>> {
    ensure!(
        request.schema_version == 1,
        "unsupported History force cleanup request version"
    );
    ensure!(
        !request.force_sessions.is_empty() && request.force_sessions.len() <= MAX_SELECTIONS,
        "History force cleanup requires 1..=128 selected sessions"
    );
    let mut sessions = BTreeSet::new();
    for session in &request.force_sessions {
        identity(session)?;
        ensure!(
            !session.contains(['/', '\\']),
            "unsafe History session identity"
        );
        ensure!(
            sessions.insert(session.as_str()),
            "duplicate History session identity"
        );
    }
    Ok(sessions)
}

fn forced_legacy_audits(root: &Path, session: &str) -> Result<Vec<PathBuf>> {
    let name = format!("session_audit_{session}.jsonl");
    let mut audits = Vec::new();
    let mut inspect = |path: PathBuf| -> Result<()> {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => audits.push(path),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect legacy History audit"),
        }
        Ok(())
    };
    inspect(root.join(&name))?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path().join(&name);
        if session_archive_directory(root, &path, session).is_some() && entry.file_type()?.is_dir()
        {
            inspect(path)?;
        }
    }
    audits.sort_unstable();
    Ok(audits)
}

fn completed_legacy_event(event: &str) -> bool {
    event == "SESSION_COMPLETED" || event.starts_with("SESSION_COMPLETED ")
}

fn completed_legacy_audit(path: &Path) -> Result<bool> {
    let mut file = artifact_file(path)?;
    let length = file.metadata()?.len();
    let offset = length.saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(length - offset).read_to_end(&mut bytes)?;
    let Some(last) = bytes
        .rsplit(|byte| *byte == b'\n')
        .find(|line| !line.is_empty())
    else {
        return Ok(false);
    };
    if offset > 0 && last.len() == bytes.len() {
        return Ok(false);
    }
    let value: serde_json::Value = match serde_json::from_slice(last) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    Ok(value["event"].as_str().is_some_and(completed_legacy_event)
        && value["ts"]
            .as_str()
            .is_some_and(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).is_ok()))
}

fn build_force_plan(
    root: &Path,
    snapshot: Option<&Transaction<'_>>,
    sessions: &BTreeSet<&str>,
) -> Result<HistoryForceCleanupPreview> {
    let version = snapshot.map(schema_version).transpose()?.unwrap_or(0);
    if let Some(snapshot) = snapshot {
        validate_cleanup_objects(snapshot, version)?;
    }
    let source = root.join(HISTORY_DATABASE).display().to_string();
    let mut digest = blake3::Hasher::new();
    hash_snapshot_field(&mut digest, b"MFB History force cleanup v1")?;
    hash_snapshot_field(&mut digest, source.as_bytes())?;
    let mut result = HistoryForceCleanupPreview {
        schema_version: 1,
        kind: "history_force_cleanup_preview",
        source,
        snapshot_token: String::new(),
        force_sessions: sessions.iter().map(|value| (*value).to_owned()).collect(),
        event_rows: 0,
        output_rows: 0,
        retained_artifacts: Vec::new(),
    };
    for session in sessions {
        hash_snapshot_field(&mut digest, b"session")?;
        hash_snapshot_field(&mut digest, session.as_bytes())?;
        let mut exists = false;
        let mut terminal = false;
        let mut owner = None;
        if let Some(db) = snapshot {
            ensure_session_not_removed(db, session)?;
            let updated: Option<Vec<u8>> = db
                .query_row(
                    "SELECT CAST(updated_at AS BLOB) FROM history_sessions WHERE session_id=?1",
                    [session],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(updated) = updated {
                exists = true;
                hash_snapshot_field(&mut digest, b"database row")?;
                hash_snapshot_field(&mut digest, &updated)?;
                if owner_table_exists(db)? {
                    owner = db.query_row(
                        "SELECT owner_pid,artifacts_json FROM history_session_ownership_v1 WHERE session_id=?1",
                        [session], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                    ).optional()?;
                }
                let mut events = db.prepare(
                    "SELECT sequence,CAST(ts AS BLOB),CAST(event AS BLOB) FROM history_events WHERE session_id=?1 ORDER BY sequence",
                )?;
                let mut rows = events.query([session])?;
                while let Some(row) = rows.next()? {
                    hash_snapshot_field(&mut digest, b"event")?;
                    hash_snapshot_field(&mut digest, &row.get::<_, i64>(0)?.to_le_bytes())?;
                    hash_snapshot_field(&mut digest, row.get_ref(1)?.as_blob()?)?;
                    let event = row.get_ref(2)?.as_blob()?;
                    hash_snapshot_field(&mut digest, event)?;
                    terminal = std::str::from_utf8(event).is_ok_and(completed_legacy_event);
                    checked_add(&mut result.event_rows, 1)?;
                }
                if version >= 2 {
                    let mut outputs = db.prepare(
                        "SELECT sequence,CAST(ts AS BLOB),CAST(pipeline AS BLOB),CAST(line AS BLOB) FROM history_output WHERE session_id=?1 ORDER BY sequence",
                    )?;
                    let mut rows = outputs.query([session])?;
                    while let Some(row) = rows.next()? {
                        hash_snapshot_field(&mut digest, b"output")?;
                        hash_snapshot_field(&mut digest, &row.get::<_, i64>(0)?.to_le_bytes())?;
                        for index in 1..=3 {
                            hash_snapshot_field(&mut digest, row.get_ref(index)?.as_blob()?)?;
                        }
                        checked_add(&mut result.output_rows, 1)?;
                    }
                }
            }
        }
        // Keep archive files outside force cleanup, including files not registered as owned.
        let audits = forced_legacy_audits(root, session)?;
        ensure!(
            exists || !audits.is_empty(),
            "selected History session is missing"
        );
        for audit in &audits {
            let metadata = fs::symlink_metadata(audit)?;
            ensure!(
                metadata.is_file(),
                "legacy History audit changed during preview"
            );
            hash_snapshot_field(&mut digest, b"legacy audit")?;
            hash_snapshot_field(&mut digest, audit.as_os_str().as_encoded_bytes())?;
            for value in [metadata.dev(), metadata.ino(), metadata.len()] {
                hash_snapshot_field(&mut digest, &value.to_le_bytes())?;
            }
            for value in [
                metadata.ctime(),
                metadata.ctime_nsec(),
                metadata.mtime(),
                metadata.mtime_nsec(),
            ] {
                hash_snapshot_field(&mut digest, &value.to_le_bytes())?;
            }
        }
        let mut has_registered_artifacts = false;
        if let Some((pid, artifacts)) = owner {
            registered_writer_is_inactive(pid)?;
            hash_snapshot_field(&mut digest, b"owner")?;
            hash_snapshot_field(&mut digest, &pid.to_le_bytes())?;
            hash_snapshot_field(&mut digest, artifacts.as_bytes())?;
            has_registered_artifacts = artifacts != "[]";
        } else {
            let mut parts = session.rsplitn(3, '_');
            let nonce = parts.next().unwrap_or_default();
            let pid = parts.next().and_then(|value| value.parse::<i32>().ok());
            if parts.next().is_some()
                && pid.is_some()
                && !nonce.is_empty()
                && nonce.bytes().all(|byte| byte.is_ascii_digit())
            {
                legacy_session_is_inactive(session, &[])?;
            } else if !terminal {
                ensure!(
                    !exists && !audits.is_empty(),
                    "legacy History activity cannot be proven inactive; nothing removed"
                );
                for audit in &audits {
                    ensure!(
                        completed_legacy_audit(audit)?,
                        "legacy History activity cannot be proven inactive; nothing removed"
                    );
                }
            }
        }
        if let Some(audit) = audits.first() {
            result.retained_artifacts.push(HistoryArtifactDecision {
                path: audit.display().to_string(),
                scope: "unregistered_external_logs",
                status: "retained",
                reason: format!(
                    "Force cleanup retained all {} matching external audit files",
                    audits.len()
                ),
            });
        }
        if has_registered_artifacts {
            result.retained_artifacts.push(HistoryArtifactDecision {
                path: root.display().to_string(),
                scope: "registered_external_logs",
                status: "retained",
                reason: format!(
                    "Force cleanup kept the external artifacts registered to {session}"
                ),
            });
        }
        hash_snapshot_field(&mut digest, b"end session")?;
    }
    result.snapshot_token = digest.finalize().to_hex().to_string();
    Ok(result)
}

/// Preview removal of every stored row belonging to the selected inactive sessions.
pub fn preview_history_force_cleanup(
    directory: &Path,
    request: &HistoryForceCleanupRequest,
) -> Result<HistoryForceCleanupPreview> {
    let root = cleanup_root(directory)?;
    let sessions = force_selections(request)?;
    let _locks = cleanup_session_locks(&root, sessions.iter().copied())?;
    let mut db = open_history_read(&root)?;
    let snapshot = db.as_mut().map(Connection::transaction).transpose()?;
    build_force_plan(&root, snapshot.as_ref(), &sessions)
}

/// Remove selected History sessions without interpreting ambiguous run boundaries.
pub fn execute_history_force_cleanup(
    directory: &Path,
    request: &HistoryForceCleanupRequest,
) -> Result<HistoryForceCleanupReceipt> {
    let token = request
        .snapshot_token
        .as_deref()
        .context("History force cleanup requires the exact preview token")?;
    let root = cleanup_root(directory)?;
    let sessions = force_selections(request)?;
    let _locks = cleanup_session_locks(&root, sessions.iter().copied())?;
    let path = root.join(HISTORY_DATABASE);
    let mut db = if path.try_exists()? {
        Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?
    } else {
        ensure!(
            build_force_plan(&root, None, &sessions)?.snapshot_token == token,
            "History force cleanup selection changed since preview; nothing removed"
        );
        open_history_write(&root)?
    };
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON;")?;
    let secure: i64 = db.pragma_query_value(None, "secure_delete", |row| row.get(0))?;
    ensure!(secure == 1, "History secure deletion is unavailable");
    persist_history_wal_sidecars(&db)?;
    let transaction = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let preview = build_force_plan(&root, Some(&transaction), &sessions)?;
    ensure!(
        preview.snapshot_token == token,
        "History force cleanup selection changed since preview; nothing removed"
    );
    if schema_version(&transaction)? == 1 {
        migrate_history_v1(&transaction)?;
    }
    if !removed_table_exists(&transaction)? {
        transaction.execute_batch(
            "CREATE TABLE history_removed_sessions_v1(session_id TEXT PRIMARY KEY NOT NULL);",
        )?;
    }
    let has_owners = owner_table_exists(&transaction)?;
    let mut event_rows = 0;
    let mut output_rows = 0;
    for session in sessions {
        checked_add(
            &mut event_rows,
            transaction.execute("DELETE FROM history_events WHERE session_id=?1", [session])?,
        )?;
        checked_add(
            &mut output_rows,
            transaction.execute("DELETE FROM history_output WHERE session_id=?1", [session])?,
        )?;
        if has_owners {
            transaction.execute(
                "DELETE FROM history_session_ownership_v1 WHERE session_id=?1",
                [session],
            )?;
        }
        transaction.execute(
            "DELETE FROM history_sessions WHERE session_id=?1",
            [session],
        )?;
        // A raw legacy stamp suppresses an external audit that still exists.
        let retired = if legacy_audit_exists(&root, session)? {
            session.to_owned()
        } else {
            retired_session_digest(session)
        };
        transaction.execute(
            "INSERT INTO history_removed_sessions_v1 VALUES (?1)",
            [retired],
        )?;
    }
    ensure!(
        (event_rows, output_rows) == (preview.event_rows, preview.output_rows),
        "History force cleanup counts changed; nothing removed"
    );
    transaction.commit()?;
    let mut failures = Vec::new();
    for error in [
        compact_legacy_history(&mut db).err(),
        truncate_history_wal(&db).err(),
    ]
    .into_iter()
    .flatten()
    {
        failures.push(HistoryArtifactDecision {
            path: preview.source.clone(),
            scope: "database_privacy",
            status: "failed",
            reason: format!("{error:#}"),
        });
    }
    Ok(HistoryForceCleanupReceipt {
        schema_version: 1,
        kind: "history_force_cleanup_receipt",
        status: if failures.is_empty() && preview.retained_artifacts.is_empty() {
            "completed"
        } else {
            "partial"
        },
        database_committed: true,
        force_sessions: preview.force_sessions,
        event_rows,
        output_rows,
        retained_artifacts: preview.retained_artifacts,
        failed_artifacts: failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixture users share the default serial lock with other state-root tests.
    type HistoryFixture = (
        tempfile::TempDir,
        foundation::common_utils::EnvGuard,
        PathBuf,
    );

    fn fixture() -> Result<Option<HistoryFixture>> {
        // The formal root is bound for the lifetime of the process.
        let thread = std::thread::current();
        let name = thread
            .name()
            .context("history cleanup test name unavailable")?;
        if std::env::var("MFB_HISTORY_CLEANUP_TEST").as_deref() != Ok(name) {
            let output = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", name, "--nocapture", "--test-threads=1"])
                .env("MFB_HISTORY_CLEANUP_TEST", name)
                .output()?;
            ensure!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
                "isolated History cleanup regression failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(None);
        }
        let root = tempfile::tempdir()?;
        let guard = foundation::common_utils::EnvGuard::set(
            "MFB_HOME_ROOT",
            root.path().to_str().context("test root UTF-8")?,
        );
        let logs = root.path().join("logs");
        fs::create_dir(&logs)?;
        Ok(Some((root, guard, logs)))
    }

    fn run(logs: &Path, session: &str, id: &str, line: &str) -> Result<()> {
        let begin = session_output_cursor(logs, session)?;
        append_history_event(
            logs,
            session,
            &format!(
                "MFB_HISTORY_CONTEXT={}",
                serde_json::json!({"schema_version":1,"run_id":id,"output_sequence":begin})
            ),
        )?;
        let mut output = HistoryOutputWriter::open(logs, session)?;
        output.push_line("IMG", line)?;
        output.flush()?;
        drop(output);
        let end = session_output_cursor(logs, session)?;
        append_history_event(
            logs,
            session,
            &format!(
                "MFB_HISTORY_FINISHED={}",
                serde_json::json!({"schema_version":1,"run_id":id,"output_sequence":end,"outcome":"completed"})
            ),
        )?;
        Ok(())
    }

    fn request(session: &str, run_id: &str) -> HistoryCleanupRequest {
        HistoryCleanupRequest {
            schema_version: 1,
            runs: vec![HistoryRunSelection {
                session_id: session.to_owned(),
                run_id: run_id.to_owned(),
            }],
            snapshot_token: None,
        }
    }

    fn owned_logs(logs: &Path, session: &str) -> Result<Vec<PathBuf>> {
        let paths = vec![
            logs.join(format!("MFB_Session_{session}.log")),
            logs.join(format!("verbose_{session}.log")),
            logs.join(format!("session_audit_{session}.jsonl")),
        ];
        for path in &paths {
            fs::write(path, b"private synthetic diagnostic")?;
        }
        register_history_artifacts(logs, session, &paths)?;
        Ok(paths)
    }

    #[test]
    fn forced_cleanup_revalidates_unfinished_session_and_preserves_files() -> Result<()> {
        let Some((root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        let media = root.path().join("user-media.jpg");
        fs::write(&media, b"user file")?;
        let owned = owned_logs(&logs, "unfinished")?;
        append_history_event(
            &logs,
            "unfinished",
            r#"MFB_HISTORY_CONTEXT={"schema_version":1,"run_id":"run-unfinished","output_sequence":0}"#,
        )?;
        let mut writer = HistoryOutputWriter::open(&logs, "unfinished")?;
        writer.push_line("IMG", "private history line")?;
        writer.flush()?;
        drop(writer);
        run(
            &logs,
            "unrelated",
            "run-unrelated",
            "unrelated history line",
        )?;

        let mut request = HistoryForceCleanupRequest {
            schema_version: 1,
            force_sessions: vec!["unfinished".to_owned()],
            snapshot_token: None,
        };
        let lease = acquire_history_session_lease(&logs, "unfinished")?;
        assert!(preview_history_force_cleanup(&logs, &request).is_err());
        drop(lease);

        let preview = preview_history_force_cleanup(&logs, &request)?;
        assert_eq!((preview.event_rows, preview.output_rows), (1, 1));
        assert_eq!(preview.retained_artifacts.len(), 2);
        request.snapshot_token = Some(preview.snapshot_token);
        append_history_event(
            &logs,
            "unfinished",
            r#"MFB_HISTORY_SOURCE_SET={"schema_version":1,"run_id":"run-unfinished","source_root":"/synthetic","file_count":1,"content_blake3":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )?;
        assert!(execute_history_force_cleanup(&logs, &request).is_err());
        request.snapshot_token =
            Some(preview_history_force_cleanup(&logs, &request)?.snapshot_token);
        let receipt = execute_history_force_cleanup(&logs, &request)?;
        assert_eq!((receipt.event_rows, receipt.output_rows), (2, 1));
        assert!(receipt.database_committed);
        assert_eq!(receipt.status, "partial");
        assert!(receipt.failed_artifacts.is_empty());
        let db = open_history_read(&logs)?.context("test History database")?;
        let sessions = db
            .prepare("SELECT session_id FROM history_sessions")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(sessions, ["unrelated"]);
        assert!(ensure_session_not_removed(&db, "unfinished").is_err());
        for path in owned {
            assert_eq!(fs::read(path)?, b"private synthetic diagnostic");
        }
        assert_eq!(fs::read(media)?, b"user file");
        Ok(())
    }

    #[test]
    fn forced_cleanup_retires_a_completed_legacy_audit_without_removing_it() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        let legacy_audit = logs.join("session_audit_old.jsonl");
        fs::write(&legacy_audit, b"{}\n")?;
        let mut request = HistoryForceCleanupRequest {
            schema_version: 1,
            force_sessions: vec!["old".to_owned()],
            snapshot_token: None,
        };
        assert!(preview_history_force_cleanup(&logs, &request).is_err());
        assert!(!logs.join(HISTORY_DATABASE).exists());
        let complete =
            b"{\"ts\":\"2026-10-01T00:00:00Z\",\"event\":\"SESSION_COMPLETED images_ok=1\"}\n";
        fs::write(&legacy_audit, complete)?;
        let preview = preview_history_force_cleanup(&logs, &request)?;
        assert_eq!((preview.event_rows, preview.output_rows), (0, 0));
        assert_eq!(preview.retained_artifacts.len(), 1);
        request.snapshot_token = Some(preview.snapshot_token);
        let receipt = execute_history_force_cleanup(&logs, &request)?;
        assert!(receipt.database_committed);
        assert_eq!(receipt.status, "partial");
        assert!(receipt.failed_artifacts.is_empty());
        assert_eq!(fs::read(legacy_audit)?, complete);
        let db = open_history_read(&logs)?.context("test History database")?;
        let retired = db.query_row(
            "SELECT session_id FROM history_removed_sessions_v1",
            [],
            |row| row.get::<_, String>(0),
        )?;
        assert_eq!(retired, "old");
        Ok(())
    }

    pub(super) fn pause_for_crash_test(stage: &str) {
        if std::env::var("MFB_HISTORY_CRASH_STAGE").as_deref() == Ok(stage) {
            let root = std::env::var_os("MFB_HISTORY_CRASH_ROOT").expect("crash test root");
            fs::write(PathBuf::from(root).join("ready"), b"").expect("signal crash boundary");
            loop {
                std::thread::park();
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_process_exit_preserves_database_transaction_boundaries() -> Result<()> {
        use std::process::{Command, Stdio};
        use std::time::Instant;

        if let Some(root) = std::env::var_os("MFB_HISTORY_CRASH_ROOT") {
            let logs = PathBuf::from(root).join("logs");
            fs::create_dir(&logs)?;
            run(&logs, "selected", "run-selected", "selected output")?;
            run(&logs, "unrelated", "run-unrelated", "unrelated output")?;
            owned_logs(&logs, "selected")?;
            if std::env::var("MFB_HISTORY_CRASH_STAGE").as_deref() == Ok("after_legacy_checkpoint")
            {
                open_history_write(&logs)?.pragma_update(None, "user_version", 2)?;
            }
            let mut selected = request("selected", "run-selected");
            selected.snapshot_token =
                Some(preview_history_cleanup(&logs, &selected)?.snapshot_token);
            let _ = execute_history_cleanup(&logs, &selected)?;
            anyhow::bail!("cleanup completed without reaching the crash test boundary");
        }

        let thread = std::thread::current();
        let test_name = thread
            .name()
            .context("History crash test name unavailable")?;
        for stage in ["before_commit", "after_commit", "after_legacy_checkpoint"] {
            let root = tempfile::tempdir()?;
            let mut child = Command::new(std::env::current_exe()?)
                .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
                .env("MFB_HISTORY_CRASH_ROOT", root.path())
                .env("MFB_HISTORY_CRASH_STAGE", stage)
                .env("MFB_HOME_ROOT", root.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(15);
            let ready = root.path().join("ready");
            while !ready.exists() && Instant::now() < deadline {
                if child.try_wait()?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let reached_boundary = ready.exists();
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            let exit = child.wait_with_output()?;
            ensure!(
                reached_boundary && !exit.status.success(),
                "cleanup did not stop at {stage}: {}\n{}",
                String::from_utf8_lossy(&exit.stdout),
                String::from_utf8_lossy(&exit.stderr)
            );

            let logs = root.path().join("logs");
            let connection = Connection::open(logs.join(HISTORY_DATABASE))?;
            assert_eq!(
                connection
                    .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?,
                "ok",
                "{stage}"
            );
            assert!(
                connection
                    .prepare("PRAGMA foreign_key_check")?
                    .query([])?
                    .next()?
                    .is_none()
            );
            for (table, count) in [("history_events", 2), ("history_output", 1)] {
                let sql = format!("SELECT COUNT(*) FROM {table} WHERE session_id=?1");
                let selected_count = if stage == "before_commit" { count } else { 0 };
                for (session, expected) in [("selected", selected_count), ("unrelated", count)] {
                    let actual: i64 = connection.query_row(&sql, [session], |row| row.get(0))?;
                    assert_eq!(actual, expected, "{stage}: {table}/{session}");
                }
            }
            let version: i64 =
                connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
            let legacy = stage == "after_legacy_checkpoint";
            assert_eq!(version, if legacy { 2 } else { SCHEMA_VERSION }, "{stage}");
            let pending = fs::read_dir(&logs)?
                .collect::<std::io::Result<Vec<_>>>()?
                .into_iter()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".mfb-history-cleanup-")
                })
                .collect::<Vec<_>>();
            assert_eq!(pending.len(), if legacy { 0 } else { 3 }, "{stage}");
            for entry in pending {
                assert_eq!(
                    fs::read(entry.path())?,
                    b"private synthetic diagnostic",
                    "{stage}"
                );
            }
        }
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_write_contention_retains_rows_and_allows_retry() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "finished", "run-finished", "retained while busy")?;
        let paths = owned_logs(&logs, "finished")?;
        let mut selected = request("finished", "run-finished");
        selected.snapshot_token = Some(preview_history_cleanup(&logs, &selected)?.snapshot_token);
        let mut writer = open_history_write(&logs)?;
        let transaction = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Err(error) = execute_history_cleanup(&logs, &selected) else {
            anyhow::bail!("History cleanup unexpectedly acquired a held write transaction");
        };
        assert!(error.to_string().starts_with("MFB_HISTORY_CLEANUP_BUSY:"));
        assert_eq!(
            transaction.query_row("SELECT COUNT(*) FROM history_events", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        assert!(paths.iter().all(|path| path.is_file()));
        transaction.rollback()?;
        drop(writer);
        let receipt = execute_history_cleanup(&logs, &selected)?;
        assert_eq!(receipt.status, "completed");
        assert_eq!(receipt.removed_runs, 1);
        assert_eq!(receipt.removed_artifacts, 3);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_accepts_large_output_and_rechecks_every_payload_byte() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "large", "run-large", "original")?;
        let mut connection = open_history_write(&logs)?;
        let transaction = connection.transaction()?;
        let row_bytes = 8 * 1024 * 1024;
        let rows = SNAPSHOT_BYTE_LIMIT / row_bytes + 1;
        assert_eq!(
            transaction.execute(
                "WITH RECURSIVE chunks(i) AS (
                     VALUES(1) UNION ALL SELECT i + 1 FROM chunks WHERE i < ?1
                 )
                 INSERT INTO history_output(session_id, ts, pipeline, line)
                 SELECT 'large', 'now', 'IMG', CAST(zeroblob(?2) AS TEXT) FROM chunks",
                params![i64::try_from(rows)?, i64::try_from(row_bytes)?],
            )?,
            rows
        );
        assert_eq!(
            transaction.execute(
                "UPDATE history_events
                 SET event='MFB_HISTORY_FINISHED=' || json_set(
                     substr(event, instr(event, '=') + 1), '$.output_sequence',
                     (SELECT MAX(sequence) FROM history_output WHERE session_id='large')
                 )
                 WHERE session_id='large' AND event LIKE 'MFB_HISTORY_FINISHED=%'",
                [],
            )?,
            1
        );
        transaction.commit()?;

        let mut selected = request("large", "run-large");
        let preview = preview_history_cleanup(&logs, &selected)?;
        assert_eq!(preview.output_rows, u64::try_from(rows + 1)?);
        assert_eq!(
            preview.output_bytes,
            u64::try_from(rows * row_bytes + "original".len())?
        );
        selected.snapshot_token = Some(preview.snapshot_token);
        assert_eq!(
            connection.execute(
                "UPDATE history_output SET line='modified'
                 WHERE sequence=(SELECT MIN(sequence) FROM history_output WHERE session_id='large')",
                [],
            )?,
            1
        );
        let Err(error) = execute_history_cleanup(&logs, &selected) else {
            anyhow::bail!("History cleanup accepted a modified payload with its old token");
        };
        assert!(error.to_string().contains("changed since preview"));
        assert_eq!(
            connection.query_row(
                "SELECT COUNT(*) FROM history_output WHERE session_id='large'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            i64::try_from(rows + 1)?
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_scrubs_payloads_and_removes_only_proven_archive_entries() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        let payload = "synthetic-private-payload-for-page-scrubbing";
        let session = "synthetic-private-complete-session";
        let mixed = "synthetic-mixed-session";
        run(&logs, session, "run-private", payload)?;
        run(&logs, mixed, "run-mixed", "mixed run output")?;
        run(&logs, "unselected", "run-other", "untouched run output")?;
        for target in [session, mixed] {
            let mut owned = Vec::new();
            let archive = logs.join(format!("Synthetic_Bundle_{target}"));
            fs::create_dir(&archive)?;
            for path in owned_logs(&logs, target)? {
                let destination = archive.join(path.file_name().context("fixture filename")?);
                fs::rename(path, &destination)?;
                owned.push(destination);
            }
            let manifest = archive.join("manifest.json");
            fs::write(&manifest, b"synthetic owned archive manifest")?;
            owned.push(manifest);
            register_history_artifacts(&logs, target, &owned)?;
            if target == mixed {
                fs::write(archive.join("user.jpeg"), b"untouched user file")?;
            }
        }
        let database = logs.join(HISTORY_DATABASE);
        let wal = logs.join(format!("{HISTORY_DATABASE}-wal"));
        let contains = |path: &Path, needle: &str| -> Result<bool> {
            Ok(fs::read(path)?
                .windows(needle.len())
                .any(|bytes| bytes == needle.as_bytes()))
        };
        assert!(contains(&database, payload)? || contains(&wal, payload)?);
        let mut selected = request(session, "run-private");
        selected.runs.extend(request(mixed, "run-mixed").runs);
        let preview = preview_history_cleanup(&logs, &selected)?;
        assert!(
            preview
                .artifacts
                .iter()
                .any(|item| item.scope == "unverified_archive")
        );
        selected.snapshot_token = Some(preview.snapshot_token);
        let receipt = execute_history_cleanup(&logs, &selected)?;
        assert_eq!(receipt.status, "partial");
        assert_eq!(receipt.removed_artifacts, 8);
        assert!(
            receipt.failed_artifacts.is_empty(),
            "{:?}",
            receipt.failed_artifacts
        );
        assert!(
            !logs
                .join(format!("Synthetic_Bundle_{session}"))
                .try_exists()?
        );
        let mixed_archive = logs.join(format!("Synthetic_Bundle_{mixed}"));
        assert_eq!(
            fs::read(mixed_archive.join("user.jpeg"))?,
            b"untouched user file"
        );
        assert_eq!(fs::read_dir(mixed_archive)?.count(), 1);
        assert_eq!(fs::metadata(&wal)?.len(), 0);
        assert!(!contains(&database, payload)?);
        assert!(!contains(&database, session)?);
        assert!(!contains(&database, "run-private")?);
        assert!(contains(&database, "untouched run output")?);
        assert!(acquire_history_session_lease(&logs, session).is_err());
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn legacy_history_certifies_once_then_secure_deletion_reclaims_pages_without_rewriting()
    -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "legacy", "run-legacy", "legacy output")?;
        run(&logs, "legacy-peer", "run-peer", "other legacy output")?;
        let connection = open_history_write(&logs)?;
        connection.pragma_update(None, "user_version", 2)?;

        let mut peer = request("legacy-peer", "run-peer");
        peer.snapshot_token = Some(preview_history_cleanup(&logs, &peer)?.snapshot_token);
        let receipt = execute_history_cleanup(&logs, &peer)?;
        assert_eq!(
            receipt.status, "completed",
            "{:?}",
            receipt.failed_artifacts
        );
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, 2, "an open peer must prevent migration");
        drop(connection);

        let mut first = request("legacy", "run-legacy");
        first.snapshot_token = Some(preview_history_cleanup(&logs, &first)?.snapshot_token);
        let receipt = execute_history_cleanup(&logs, &first)?;
        assert_eq!(
            receipt.status, "completed",
            "{:?}",
            receipt.failed_artifacts
        );

        let connection = open_history_write(&logs)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, SCHEMA_VERSION);
        let secure: i64 = connection.pragma_query_value(None, "secure_delete", |row| row.get(0))?;
        assert_eq!(secure, 1);
        drop(connection);

        let payload = "private-selected-output-synthetic".repeat(32_768);
        run(&logs, "fast-delete", "run-fast", &payload)?;
        run(&logs, "keep", "run-keep", "untouched output")?;
        let database = logs.join(HISTORY_DATABASE);
        let mut second = request("fast-delete", "run-fast");
        second.snapshot_token = Some(preview_history_cleanup(&logs, &second)?.snapshot_token);
        truncate_history_wal(&open_history_write(&logs)?)?;
        let size_before = fs::metadata(&database)?.len();
        let receipt = execute_history_cleanup(&logs, &second)?;
        assert_eq!(
            receipt.status, "completed",
            "{:?}",
            receipt.failed_artifacts
        );
        assert_eq!(fs::metadata(&database)?.len(), size_before);
        assert_eq!(
            fs::metadata(logs.join(format!("{HISTORY_DATABASE}-wal")))?.len(),
            0
        );
        let bytes = fs::read(database)?;
        for marker in ["private-selected-output-synthetic", "run-fast"] {
            assert!(
                !bytes
                    .windows(marker.len())
                    .any(|chunk| chunk == marker.as_bytes())
            );
        }
        assert!(bytes.windows(16).any(|chunk| chunk == b"untouched output"));
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_retains_cold_read_only_access_after_partial_and_full_removal() -> Result<()>
    {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "first", "one", "first output")?;
        run(&logs, "second", "two", "second output")?;
        for (session, id, remaining) in [("first", "one", 1), ("second", "two", 0)] {
            let mut selected = request(session, id);
            selected.snapshot_token =
                Some(preview_history_cleanup(&logs, &selected)?.snapshot_token);
            let receipt = execute_history_cleanup(&logs, &selected)?;
            assert_eq!(receipt.status, "completed");
            let database = logs.join(HISTORY_DATABASE);
            let wal = logs.join(format!("{HISTORY_DATABASE}-wal"));
            let shm = logs.join(format!("{HISTORY_DATABASE}-shm"));
            assert!(
                wal.is_file() && shm.is_file(),
                "cleanup must preserve cold read-only access"
            );
            let before = fs::read(&database)?;
            let wal_before = fs::read(&wal)?;
            let reader = open_history_read(&logs)?.context("history database disappeared")?;
            assert_eq!(
                reader.query_row("SELECT COUNT(*) FROM history_sessions", [], |row| row
                    .get::<_, i64>(0))?,
                remaining
            );
            assert_eq!(
                reader.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?,
                "ok"
            );
            assert!(reader.execute("DELETE FROM history_sessions", []).is_err());
            drop(reader);
            #[cfg(target_os = "macos")]
            {
                let native = std::process::Command::new("/usr/bin/sqlite3")
                    .arg("-readonly")
                    .arg(database.canonicalize()?)
                    .arg("SELECT COUNT(*) FROM history_sessions; PRAGMA quick_check;")
                    .output()?;
                assert!(
                    native.status.success(),
                    "{}",
                    String::from_utf8_lossy(&native.stderr)
                );
                assert_eq!(
                    String::from_utf8(native.stdout)?,
                    format!("{remaining}\nok\n")
                );
            }
            assert_eq!(fs::read(&database)?, before);
            assert_eq!(fs::read(&wal)?, wal_before);
        }
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_preserves_unselected_runs_shared_logs_and_output_boundaries() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "session", "run-one", "one")?;
        run(&logs, "session", "run-two", "two")?;
        run(&logs, "other", "run-other", "unselected")?;
        let paths = owned_logs(&logs, "session")?;
        let mut selected = request("session", "run-one");
        let preview = preview_history_cleanup(&logs, &selected)?;
        assert_eq!(preview.event_rows, 2);
        assert_eq!(preview.output_rows, 1);
        assert_eq!(preview.output_bytes, 3);
        assert!(
            preview
                .artifacts
                .iter()
                .all(|item| item.status == "retained")
        );
        selected.snapshot_token = Some(preview.snapshot_token);
        let receipt = execute_history_cleanup(&logs, &selected)?;
        assert_eq!(
            (
                receipt.removed_runs,
                receipt.event_rows,
                receipt.output_rows,
                receipt.removed_artifacts
            ),
            (1, 2, 1, 0)
        );
        assert_eq!(receipt.status, "partial");
        for path in &paths {
            assert_eq!(fs::read(path)?, b"private synthetic diagnostic");
        }
        let connection = open_history_read(&logs)?.context("test history absent")?;
        assert_eq!(
            connection.query_row(
                "SELECT COUNT(*) FROM history_events WHERE session_id='session'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            2
        );
        assert_eq!(read_session_output_since(&logs, "session", 0)?, "two\n");
        assert_eq!(
            read_session_output_since(&logs, "other", 0)?,
            "unselected\n"
        );
        let next = preview_history_cleanup(&logs, &request("session", "run-two"))?;
        assert_eq!(next.output_rows, 1);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_accepts_unallocated_output_sequence_only_for_zero_boundaries() -> Result<()>
    {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        append_history_event(
            &logs,
            "session",
            &format!(
                "MFB_HISTORY_CONTEXT={}",
                serde_json::json!({"schema_version":1,"run_id":"run-empty","output_sequence":0})
            ),
        )?;
        let terminal = |cursor| {
            format!(
                "MFB_HISTORY_FINISHED={}",
                serde_json::json!({"schema_version":1,"run_id":"run-empty","output_sequence":cursor,"outcome":"completed"})
            )
        };
        append_history_event(&logs, "session", &terminal(0))?;
        let connection = open_history_write(&logs)?;
        let sequence: Option<i64> = connection
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='history_output'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        assert_eq!(sequence, None, "the output table has never allocated a row");
        let preview = preview_history_cleanup(&logs, &request("session", "run-empty"))?;
        assert_eq!((preview.event_rows, preview.output_rows), (2, 0));

        connection.execute(
            "UPDATE history_events SET event=?1 WHERE event LIKE 'MFB_HISTORY_FINISHED=%'",
            [terminal(1)],
        )?;
        let error = preview_history_cleanup(&logs, &request("session", "run-empty"))
            .expect_err("an absent sequence cannot prove a positive output boundary");
        assert!(error.to_string().contains("high-water"), "{error}");
        assert_eq!(
            connection.query_row("SELECT COUNT(*) FROM history_events", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        assert_eq!(
            connection.query_row("SELECT COUNT(*) FROM history_output", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_rejects_missing_output_sequence_with_persisted_rows() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "session", "run-one", "retained output")?;
        let connection = open_history_write(&logs)?;
        connection.execute(
            "DELETE FROM sqlite_sequence WHERE name='history_output'",
            [],
        )?;
        connection.execute(
            "UPDATE history_events SET event=?1 WHERE event LIKE 'MFB_HISTORY_FINISHED=%'",
            [format!(
                "MFB_HISTORY_FINISHED={}",
                serde_json::json!({"schema_version":1,"run_id":"run-one","output_sequence":0,"outcome":"completed"})
            )],
        )?;

        let error = preview_history_cleanup(&logs, &request("session", "run-one"))
            .expect_err("existing output rows require their allocated high-water sequence");

        assert!(error.to_string().contains("high-water"), "{error}");
        assert_eq!(
            read_session_output_since(&logs, "session", 0)?,
            "retained output\n"
        );
        assert_eq!(
            connection.query_row("SELECT COUNT(*) FROM history_events", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_keeps_allocated_output_boundary_after_rows_are_deleted() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "session", "run-one", "removed output")?;
        let connection = open_history_write(&logs)?;
        connection.execute("DELETE FROM history_output", [])?;
        let sequence: i64 = connection.query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='history_output'",
            [],
            |row| row.get(0),
        )?;
        assert!(sequence > 0);
        let mut selected = request("session", "run-one");
        let preview = preview_history_cleanup(&logs, &selected)?;
        assert_eq!((preview.event_rows, preview.output_rows), (2, 0));
        selected.snapshot_token = Some(preview.snapshot_token);
        let receipt = execute_history_cleanup(&logs, &selected)?;
        assert_eq!(
            (
                receipt.removed_runs,
                receipt.event_rows,
                receipt.output_rows
            ),
            (1, 2, 0)
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_removes_only_registered_files_and_retains_unknown_artifacts() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "session", "run-one", "one")?;
        let paths = owned_logs(&logs, "session")?;
        let private_media = logs.join("not-an-owned-log.jpg");
        fs::write(&private_media, b"private synthetic unrelated bytes")?;
        let mut selected = request("session", "run-one");
        let preview = preview_history_cleanup(&logs, &selected)?;
        assert_eq!(
            preview
                .artifacts
                .iter()
                .filter(|item| item.status == "delete")
                .count(),
            3
        );
        selected.snapshot_token = Some(preview.snapshot_token);
        let receipt = execute_history_cleanup(&logs, &selected)?;
        assert_eq!(receipt.removed_artifacts, 3);
        assert!(
            receipt.failed_artifacts.is_empty(),
            "{:?}",
            receipt.failed_artifacts
        );
        assert!(
            receipt
                .retained_artifacts
                .iter()
                .any(|item| item.scope == "unregistered_external_logs")
        );
        for path in paths {
            assert!(!path.try_exists()?);
        }
        assert_eq!(
            fs::read(&private_media)?,
            b"private synthetic unrelated bytes"
        );
        let connection = open_history_read(&logs)?.context("test history absent")?;
        assert_eq!(
            connection.query_row("SELECT COUNT(*) FROM history_sessions", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
        assert_eq!(
            connection.query_row(
                "SELECT COUNT(*) FROM history_removed_sessions_v1 WHERE session_id='session'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            1
        );
        assert!(acquire_history_session_lease(&logs, "session").is_err());
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_retires_rows_even_when_a_registered_log_requires_attention() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "session", "run-one", "one")?;
        let paths = owned_logs(&logs, "session")?;
        let old = logs.join("unregistered-old.log");
        fs::rename(&paths[0], &old)?;
        fs::write(&paths[0], b"new unrelated file")?;
        let mut selected = request("session", "run-one");
        let preview = preview_history_cleanup(&logs, &selected)?;
        assert_eq!(
            preview
                .artifacts
                .iter()
                .filter(|item| item.scope == "registered_file_problem")
                .count(),
            1
        );
        selected.snapshot_token = Some(preview.snapshot_token);
        let receipt = execute_history_cleanup(&logs, &selected)?;
        assert_eq!(receipt.status, "partial");
        assert_eq!(receipt.removed_artifacts, 2);
        assert_eq!(fs::read(&paths[0])?, b"new unrelated file");
        assert_eq!(fs::read(&old)?, b"private synthetic diagnostic");
        let connection = open_history_read(&logs)?.context("test history absent")?;
        assert_eq!(
            connection.query_row("SELECT COUNT(*) FROM history_events", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
        assert_eq!(
            connection.query_row(
                "SELECT COUNT(*) FROM history_removed_sessions_v1 WHERE session_id='session'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            1
        );
        assert!(acquire_history_session_lease(&logs, "session").is_err());
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_rejects_live_leases_stale_tokens_unknown_ids_and_schema() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "session", "run-one", "one")?;
        let lease = acquire_history_session_lease(&logs, "session")?;
        assert!(preview_history_cleanup(&logs, &request("session", "run-one")).is_err());
        drop(lease);
        let mut selected = request("session", "run-one");
        selected.snapshot_token = Some(preview_history_cleanup(&logs, &selected)?.snapshot_token);
        run(&logs, "session", "run-two", "new")?;
        assert!(execute_history_cleanup(&logs, &selected).is_err());
        assert!(preview_history_cleanup(&logs, &request("session", "unknown")).is_err());
        let mut duplicate = request("session", "run-one");
        duplicate.runs.push(duplicate.runs[0].clone());
        assert!(preview_history_cleanup(&logs, &duplicate).is_err());
        let connection = open_history_write(&logs)?;
        // A different MFB root cannot make a live process safe to remove.
        let parent = unsafe { libc::getppid() };
        connection.execute(
            "UPDATE history_session_ownership_v1 SET owner_pid=?1 WHERE session_id='session'",
            [parent],
        )?;
        assert!(preview_history_cleanup(&logs, &request("session", "run-one")).is_err());
        connection.pragma_update(None, "user_version", 99)?;
        assert!(preview_history_cleanup(&logs, &request("session", "run-one")).is_err());
        assert_eq!(
            connection.query_row("SELECT COUNT(*) FROM history_events", [], |row| row
                .get::<_, i64>(0))?,
            4
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_legacy_activity_and_symlinks_fail_closed() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        let session = format!("fixture_{}_1", std::process::id());
        let id = format!("run-{}-fixture-0", std::process::id());
        run(&logs, &session, &id, "one")?;
        let connection = open_history_write(&logs)?;
        connection.execute(
            "DELETE FROM history_session_ownership_v1 WHERE session_id=?1",
            [&session],
        )?;
        assert!(preview_history_cleanup(&logs, &request(&session, &id)).is_err());
        assert!(preview_history_cleanup(&logs, &request("unprovable", "unknown")).is_err());
        let link = logs.parent().context("logs parent")?.join("symlink-logs");
        std::os::unix::fs::symlink(&logs, &link)?;
        assert!(preview_history_cleanup(&link, &request(&session, &id)).is_err());
        assert_eq!(
            connection.query_row("SELECT COUNT(*) FROM history_events", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        Ok(())
    }

    #[test]
    fn history_cleanup_recovery_never_overwrites_an_existing_path() -> Result<()> {
        let root = tempfile::tempdir()?;
        let original = root.path().join("log");
        let recovery = root.path().join("owned.pending");
        fs::write(&recovery, b"original diagnostic")?;
        fs::write(&original, b"new unrelated file")?;
        let failures = restore_staged(&[(original.clone(), recovery.clone())]);
        assert_eq!(failures.len(), 1);
        assert_eq!(fs::read(&original)?, b"new unrelated file");
        assert_eq!(fs::read(&recovery)?, b"original diagnostic");
        fs::remove_file(&original)?;
        assert_eq!(
            restore_staged(&[(original.clone(), recovery.clone())]).len(),
            0
        );
        assert_eq!(fs::read(&original)?, b"original diagnostic");
        assert!(!recovery.try_exists()?);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_readonly_preparation_retains_rows_schema_and_main_database_bytes() -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "cold-session", "cold-run", "retained output")?;
        let source = logs.join(HISTORY_DATABASE).canonicalize()?;
        let writer = Connection::open_with_flags(
            &source,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        writer.close().map_err(|(_, error)| error)?;
        assert!(!logs.join(format!("{HISTORY_DATABASE}-wal")).exists());
        assert!(!logs.join(format!("{HISTORY_DATABASE}-shm")).exists());
        let before = fs::read(&source)?;
        #[cfg(target_os = "macos")]
        {
            let native = std::process::Command::new("/usr/bin/sqlite3")
                .arg("-readonly")
                .arg(source.canonicalize()?)
                .arg("PRAGMA user_version;")
                .output()?;
            eprintln!(
                "cold system SQLite before preparation: {:?}: {}",
                native.status.code(),
                String::from_utf8_lossy(&native.stderr)
            );
        }
        let receipt = prepare_history_read_only_access(&logs)?;
        assert_eq!(receipt.kind, "history_readonly_access");
        assert_eq!(receipt.history_schema_version, SCHEMA_VERSION);
        assert_eq!(fs::read(&source)?, before);
        assert!(logs.join(format!("{HISTORY_DATABASE}-wal")).is_file());
        assert!(logs.join(format!("{HISTORY_DATABASE}-shm")).is_file());
        assert_eq!(
            read_session_output_since(&logs, "cold-session", 0)?,
            "retained output\n"
        );
        let reader = open_history_read(&logs)?.context("History test database absent")?;
        assert_eq!(
            reader.query_row(
                "SELECT COUNT(*) FROM history_events WHERE session_id='cold-session'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            2
        );
        assert_eq!(
            reader.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
            SCHEMA_VERSION
        );
        assert!(reader.execute("DELETE FROM history_events", []).is_err());
        drop(reader);
        #[cfg(target_os = "macos")]
        {
            let native = std::process::Command::new("/usr/bin/sqlite3")
                .arg("-readonly")
                .arg(&source)
                .arg("PRAGMA user_version; SELECT COUNT(*) FROM history_events;")
                .output()?;
            assert!(
                native.status.success(),
                "{}",
                String::from_utf8_lossy(&native.stderr)
            );
            assert_eq!(native.stdout, format!("{SCHEMA_VERSION}\n2\n").as_bytes());
        }
        assert_eq!(fs::read(&source)?, before);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_readonly_preparation_rejects_missing_corrupt_unknown_and_linked_sources()
    -> Result<()> {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        let source = logs.join(HISTORY_DATABASE);
        assert!(prepare_history_read_only_access(&logs).is_err());
        assert!(!source.exists());
        fs::write(&source, b"not a History database")?;
        assert!(prepare_history_read_only_access(&logs).is_err());
        assert_eq!(fs::read(&source)?, b"not a History database");
        fs::remove_file(&source)?;
        run(&logs, "unknown", "unknown-run", "retained")?;
        let writer = open_history_write(&logs)?;
        writer.pragma_update(None, "user_version", 999)?;
        writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        drop(writer);
        let before = fs::read(&source)?;
        assert!(prepare_history_read_only_access(&logs).is_err());
        assert_eq!(fs::read(&source)?, before);
        let linked = logs.parent().context("test parent")?.join("linked-history");
        std::os::unix::fs::symlink(&logs, &linked)?;
        assert!(prepare_history_read_only_access(&linked).is_err());
        let sidecar = logs.join(format!("{HISTORY_DATABASE}-journal"));
        std::os::unix::fs::symlink(&source, &sidecar)?;
        assert!(prepare_history_read_only_access(&logs).is_err());
        assert_eq!(fs::read(&source)?, before);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn history_cleanup_multiselect_keeps_other_sessions_and_rejects_unsafe_sidecars() -> Result<()>
    {
        let Some((_root, _guard, logs)) = fixture()? else {
            return Ok(());
        };
        run(&logs, "first", "run-first", "first")?;
        run(&logs, "second", "run-second", "second")?;
        run(&logs, "other", "run-other", "other")?;
        let mut selected = request("first", "run-first");
        selected.runs.push(HistoryRunSelection {
            session_id: "second".to_owned(),
            run_id: "run-second".to_owned(),
        });
        let preview = preview_history_cleanup(&logs, &selected)?;
        assert_eq!(
            (preview.runs.len(), preview.event_rows, preview.output_rows),
            (2, 4, 2)
        );
        selected.snapshot_token = Some(preview.snapshot_token);
        let receipt = execute_history_cleanup(&logs, &selected)?;
        assert_eq!(
            (
                receipt.removed_runs,
                receipt.event_rows,
                receipt.output_rows
            ),
            (2, 4, 2)
        );
        assert_eq!(read_session_output_since(&logs, "other", 0)?, "other\n");
        let protected = logs.parent().context("logs parent")?.join("protected");
        fs::write(&protected, b"unrelated private bytes")?;
        let journal = logs.join(format!("{HISTORY_DATABASE}-journal"));
        std::os::unix::fs::symlink(&protected, &journal)?;
        assert!(preview_history_cleanup(&logs, &request("other", "run-other")).is_err());
        assert_eq!(fs::read(&protected)?, b"unrelated private bytes");
        fs::remove_file(journal)?;
        let private_media = logs.join("private.jpg");
        fs::write(&private_media, b"not a History artifact")?;
        let metadata = fs::metadata(&private_media)?;
        let connection = open_history_write(&logs)?;
        connection.execute(
            "UPDATE history_session_ownership_v1 SET artifacts_json=?1 WHERE session_id='other'",
            [serde_json::to_string(&vec![OwnedArtifact {
                relative_path: "private.jpg".to_owned(),
                device: metadata.dev(),
                inode: metadata.ino(),
            }])?],
        )?;
        assert!(preview_history_cleanup(&logs, &request("other", "run-other")).is_err());
        assert_eq!(fs::read(&private_media)?, b"not a History artifact");
        let library = logs
            .parent()
            .context("logs parent")?
            .join("test.photoslibrary");
        fs::create_dir(&library)?;
        assert!(cleanup_root(&library).is_err());
        Ok(())
    }
}
