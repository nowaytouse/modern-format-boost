//! Durable processing history, separate from disposable analysis caches.

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod cleanup;
pub use cleanup::{
    HistoryArtifactDecision, HistoryCleanupPreview, HistoryCleanupReceipt, HistoryCleanupRequest,
    HistoryReadOnlyAccessReceipt, HistoryRunSelection, HistorySessionLease,
    acquire_history_session_lease, execute_history_cleanup, prepare_history_read_only_access,
    preview_history_cleanup, register_history_artifacts,
};

pub const HISTORY_DATABASE: &str = "history.sqlite3";
const SCHEMA_VERSION: i64 = 2;
const OUTPUT_BATCH_LINES: usize = 128;
const OUTPUT_BATCH_BYTES: usize = 64 * 1024;
const OUTPUT_READ_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, serde::Serialize)]
pub struct HistoryBackupReceipt {
    pub schema_version: u32,
    pub kind: &'static str,
    pub source: String,
    pub destination: String,
    pub history_schema_version: i64,
    pub bytes: u64,
    pub sessions: u64,
    pub events: u64,
    pub output_rows: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryStorageOverview {
    pub history_schema_version: i64,
    pub sqlite_version: String,
    pub journal_mode: String,
    pub page_size: u64,
    pub page_count: u64,
    pub free_pages: u64,
    pub sessions: u64,
    pub events: u64,
    pub event_bytes: u64,
    pub output_rows: Option<u64>,
    pub output_bytes: Option<u64>,
    pub oldest_session_update: Option<String>,
    pub newest_session_update: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryStorageIssue {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryStorageIntegrity {
    pub status: &'static str,
    pub checks_complete: bool,
    pub sqlite_integrity: &'static str,
    pub foreign_keys: &'static str,
    pub record_types: &'static str,
    pub issue_count: u64,
    pub issues: Vec<HistoryStorageIssue>,
}

impl HistoryStorageIntegrity {
    fn issue(&mut self, code: &'static str, message: &'static str) -> Result<()> {
        self.issue_count = self
            .issue_count
            .checked_add(1)
            .context("history issue count overflow")?;
        if self.issues.len() < 20 {
            self.issues.push(HistoryStorageIssue { code, message });
        }
        self.status = "unhealthy";
        Ok(())
    }
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryStorageInspection {
    pub schema_version: u8,
    pub kind: &'static str,
    pub source: String,
    pub exists: bool,
    pub overview: Option<HistoryStorageOverview>,
    pub integrity: Option<HistoryStorageIntegrity>,
}

/// Inspect one committed snapshot without migration, repair, or payload decoding.
/// Counts and byte lengths describe storage, not media outcomes or completed runs.
pub fn inspect_history_store(
    log_dir: &Path,
    check_integrity: bool,
) -> Result<HistoryStorageInspection> {
    ensure!(
        log_dir.is_absolute(),
        "history inspection directory must be absolute"
    );
    let source = log_dir.join(HISTORY_DATABASE);
    let mut report = HistoryStorageInspection {
        schema_version: 1,
        kind: "history_sqlite_inspection",
        source: source
            .to_str()
            .context("history inspection path is not UTF-8")?
            .to_owned(),
        exists: false,
        overview: None,
        integrity: None,
    };
    match std::fs::symlink_metadata(log_dir) {
        Ok(metadata) => ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "history inspection directory is not a regular directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error).context("inspect history directory"),
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        match std::fs::symlink_metadata(
            source.with_file_name(format!("{HISTORY_DATABASE}{suffix}")),
        ) {
            Ok(metadata) => ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "history inspection sidecar is not a regular file"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect history sidecar"),
        }
    }
    let Some(mut connection) = open_history_read(log_dir)? else {
        return Ok(report);
    };
    let snapshot = connection.transaction()?;
    let version = schema_version(&snapshot)?;
    validate_backup_schema(&snapshot, version)?;
    let mut overview = HistoryStorageOverview {
        history_schema_version: version,
        sqlite_version: snapshot.query_row("SELECT sqlite_version()", [], |row| row.get(0))?,
        journal_mode: snapshot.pragma_query_value(None, "journal_mode", |row| row.get(0))?,
        page_size: backup_row_count(&snapshot, "PRAGMA page_size")?,
        page_count: backup_row_count(&snapshot, "PRAGMA page_count")?,
        free_pages: backup_row_count(&snapshot, "PRAGMA freelist_count")?,
        sessions: backup_row_count(&snapshot, "SELECT COUNT(*) FROM history_sessions")?,
        events: backup_row_count(&snapshot, "SELECT COUNT(*) FROM history_events")?,
        event_bytes: backup_row_count(
            &snapshot,
            "SELECT COALESCE(SUM(length(CAST(event AS BLOB))), 0) FROM history_events",
        )?,
        output_rows: None,
        output_bytes: None,
        oldest_session_update: snapshot.query_row(
            "SELECT MIN(updated_at) FROM history_sessions",
            [],
            |row| row.get(0),
        )?,
        newest_session_update: snapshot.query_row(
            "SELECT MAX(updated_at) FROM history_sessions",
            [],
            |row| row.get(0),
        )?,
    };
    ensure!(
        !overview.sqlite_version.is_empty()
            && overview.page_size.is_power_of_two()
            && (512..=65_536).contains(&overview.page_size)
            && overview.page_count > 0
            && overview.free_pages <= overview.page_count,
        "invalid history storage overview"
    );
    if version == 2 {
        overview.output_rows = Some(backup_row_count(
            &snapshot,
            "SELECT COUNT(*) FROM history_output",
        )?);
        overview.output_bytes = Some(backup_row_count(
            &snapshot,
            "SELECT COALESCE(SUM(length(CAST(line AS BLOB))), 0) FROM history_output",
        )?);
    }
    if check_integrity {
        let mut integrity = HistoryStorageIntegrity {
            status: "healthy",
            checks_complete: false,
            sqlite_integrity: "ok",
            foreign_keys: "ok",
            record_types: "ok",
            issue_count: 0,
            issues: Vec::new(),
        };
        {
            let mut statement = snapshot.prepare("PRAGMA integrity_check")?;
            let mut rows = statement.query([])?;
            let mut returned = false;
            while let Some(row) = rows.next()? {
                returned = true;
                if row.get::<_, String>(0)? != "ok" {
                    integrity.sqlite_integrity = "failed";
                    integrity.issue(
                        "sqlite_integrity",
                        "SQLite reported a structural integrity failure.",
                    )?;
                }
            }
            ensure!(returned, "history integrity check returned no result");
            let mut statement = snapshot.prepare("PRAGMA foreign_key_check")?;
            let mut rows = statement.query([])?;
            while rows.next()?.is_some() {
                integrity.foreign_keys = "failed";
                integrity.issue(
                    "foreign_key",
                    "A history record has an invalid session relationship.",
                )?;
            }
        }
        for query in [
            "SELECT COUNT(*) FROM history_sessions WHERE typeof(session_id) != 'text' OR session_id = '' OR typeof(updated_at) != 'text' OR updated_at = ''",
            "SELECT COUNT(*) FROM history_events WHERE typeof(sequence) != 'integer' OR sequence <= 0 OR typeof(session_id) != 'text' OR session_id = '' OR typeof(ts) != 'text' OR ts = '' OR typeof(event) != 'text'",
        ].into_iter().chain((version == 2).then_some(
            "SELECT COUNT(*) FROM history_output WHERE typeof(sequence) != 'integer' OR sequence <= 0 OR typeof(session_id) != 'text' OR session_id = '' OR typeof(ts) != 'text' OR ts = '' OR typeof(pipeline) != 'text' OR pipeline = '' OR typeof(line) != 'text'")) {
            let invalid = backup_row_count(&snapshot, query)?;
            if invalid > 0 {
                integrity.record_types = "failed";
                integrity.issue("record_types", "History rows have invalid storage types or empty identities.")?;
            }
        }
        integrity.checks_complete = true;
        report.integrity = Some(integrity);
    }
    snapshot.commit()?;
    report.exists = true;
    report.overview = Some(overview);
    Ok(report)
}

/// Copy a single committed SQLite snapshot, without creating or migrating its source.
/// Counts describe database rows in the verified copy, never media file outcomes.
pub fn backup_history_store(log_dir: &Path, destination: &Path) -> Result<HistoryBackupReceipt> {
    use super::sqlite_backup;
    ensure!(
        log_dir.is_absolute(),
        "history backup directory must be absolute; no fallback selected"
    );
    let directory =
        std::fs::symlink_metadata(log_dir).context("inspect history backup directory")?;
    ensure!(
        directory.is_dir() && !directory.file_type().is_symlink(),
        "history backup source must be an existing regular directory"
    );
    let source = log_dir.canonicalize()?.join(HISTORY_DATABASE);
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let path = source.with_file_name(format!("{HISTORY_DATABASE}{suffix}"));
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "history backup source or SQLite sidecar is not a regular file"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() => {}
            Err(error) => {
                return Err(error).context("inspect history backup source; no database created");
            }
        }
    }
    let destination = sqlite_backup::resolve_destination(&source, destination)?;
    let parent = destination
        .parent()
        .context("history backup destination has no parent")?;
    let source_text = source
        .to_str()
        .context("history backup source path is not UTF-8")?
        .to_owned();
    let destination_text = destination
        .to_str()
        .context("history backup destination path is not UTF-8")?
        .to_owned();
    let staged = tempfile::Builder::new()
        .prefix(".mfb-history-backup-")
        .tempdir_in(parent)?;
    let backup = staged.path().join(HISTORY_DATABASE);
    let connection = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    // READ_ONLY protects the main database; query_only would also prohibit the new output database.
    connection
        .execute(
            "VACUUM main INTO ?1",
            [backup
                .to_str()
                .context("history backup staging path is not UTF-8")?],
        )
        .context("copy committed history SQLite snapshot; source retained")?;
    drop(connection);

    let mut verified = Connection::open_with_flags(
        &backup,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    verified.busy_timeout(Duration::from_secs(5))?;
    verified.pragma_update(None, "query_only", true)?;
    let snapshot = verified.transaction()?;
    let version = schema_version(&snapshot)?;
    validate_backup_schema(&snapshot, version)?;
    {
        let mut statement = snapshot.prepare("PRAGMA integrity_check")?;
        let mut rows = statement.query([])?;
        let first = rows
            .next()?
            .context("history SQLite integrity check returned no result")?;
        ensure!(
            first.get::<_, String>(0)? == "ok" && rows.next()?.is_none(),
            "history SQLite integrity verification failed; backup not published"
        );
        drop(rows);
        drop(statement);
        let mut statement = snapshot.prepare("PRAGMA foreign_key_check")?;
        ensure!(
            statement.query([])?.next()?.is_none(),
            "history SQLite foreign key verification failed; backup not published"
        );
    }
    let sessions = backup_row_count(&snapshot, "SELECT COUNT(*) FROM history_sessions")?;
    let events = backup_row_count(&snapshot, "SELECT COUNT(*) FROM history_events")?;
    let output_rows = if version == 1 {
        0
    } else {
        backup_row_count(&snapshot, "SELECT COUNT(*) FROM history_output")?
    };
    snapshot.commit()?;
    drop(verified);
    let bytes = sqlite_backup::synchronize_private_snapshot(&backup)?;
    let receipt = HistoryBackupReceipt {
        schema_version: 1,
        kind: "history_sqlite",
        source: source_text,
        destination: destination_text,
        history_schema_version: version,
        bytes,
        sessions,
        events,
        output_rows,
    };
    sqlite_backup::publish_snapshot(&backup, &destination)?;
    Ok(receipt)
}

fn backup_row_count(snapshot: &Transaction<'_>, query: &str) -> Result<u64> {
    let count: i64 = snapshot.query_row(query, [], |row| row.get(0))?;
    u64::try_from(count).context("history storage row count or byte length is negative")
}

fn validate_backup_schema(snapshot: &Transaction<'_>, version: i64) -> Result<()> {
    for table in ["history_sessions", "history_events"]
        .into_iter()
        .chain((version == 2).then_some("history_output"))
    {
        let kind: String = snapshot.query_row(
            "SELECT type FROM sqlite_schema WHERE name=?1",
            [table],
            |row| row.get(0),
        )?;
        ensure!(
            kind == "table",
            "history backup schema contains a non-table history object"
        );
        let mut columns = snapshot.prepare(&format!("PRAGMA table_info({table})"))?;
        let primary_keys = columns
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|(_, _, ordinal)| *ordinal > 0)
            .collect::<Vec<_>>();
        let (key_name, key_type) = if table == "history_sessions" {
            ("session_id", "TEXT")
        } else {
            ("sequence", "INTEGER")
        };
        ensure!(
            primary_keys.len() == 1
                && primary_keys[0].0 == key_name
                && primary_keys[0].1.eq_ignore_ascii_case(key_type)
                && primary_keys[0].2 == 1,
            "history schema has an unsupported identity or sequence primary key"
        );
        if table != "history_sessions" {
            let mut statement = snapshot.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
            let keys = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(
                keys == [(
                    "history_sessions".to_owned(),
                    "session_id".to_owned(),
                    "session_id".to_owned()
                )],
                "history backup schema has an unsupported session relationship"
            );
        }
    }
    if version == 1 {
        let outputs: i64 = snapshot.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name='history_output'",
            [],
            |row| row.get(0),
        )?;
        ensure!(
            outputs == 0,
            "history schema 1 unexpectedly contains output storage; backup not published"
        );
    }
    Ok(())
}

struct OutputLine {
    ts: String,
    pipeline: String,
    line: String,
}

/// Buffered writer for the combined output captured from one processing session.
pub struct HistoryOutputWriter {
    _lease: std::sync::Arc<HistorySessionLease>,
    connection: Connection,
    session_id: String,
    pending: Vec<OutputLine>,
    pending_bytes: usize,
    committed_records: u64,
    committed_bytes: u64,
}

impl HistoryOutputWriter {
    /// Open the history database once and ensure this session has an identity row.
    pub fn open(log_dir: &Path, session_id: &str) -> Result<Self> {
        ensure!(!session_id.is_empty(), "history session identity is empty");
        let lease = acquire_history_session_lease(log_dir, session_id)?;
        let mut connection = open_history_write(log_dir)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        cleanup::claim_history_session(&transaction, session_id)?;
        let timestamp = now_timestamp();
        transaction.execute(
            "INSERT INTO history_sessions(session_id, updated_at) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO NOTHING",
            params![session_id, timestamp],
        )?;
        transaction
            .commit()
            .context("initialize history output session")?;
        Ok(Self {
            _lease: lease,
            connection,
            session_id: session_id.to_owned(),
            pending: Vec::new(),
            pending_bytes: 0,
            committed_records: 0,
            committed_bytes: 0,
        })
    }

    /// Return records and UTF-8 line bytes committed by this writer, excluding row metadata.
    #[must_use]
    pub fn committed_totals(&self) -> (u64, u64) {
        (self.committed_records, self.committed_bytes)
    }

    /// Buffer a captured PTY line, flushing at 128 lines or the 64 KiB threshold.
    pub fn push_line(&mut self, pipeline: &str, line: &str) -> Result<()> {
        let row = OutputLine {
            ts: now_timestamp(),
            pipeline: pipeline.to_owned(),
            line: line.to_owned(),
        };
        self.pending_bytes = self
            .pending_bytes
            .checked_add(row.ts.len())
            .and_then(|bytes| bytes.checked_add(row.pipeline.len()))
            .and_then(|bytes| bytes.checked_add(row.line.len()))
            .context("processing output buffer size overflow")?;
        self.pending.push(row);
        if self.pending.len() >= OUTPUT_BATCH_LINES || self.pending_bytes >= OUTPUT_BATCH_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    /// Commit all buffered lines. Errors leave the buffer available for a retry.
    pub fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let committed_records = self
            .committed_records
            .checked_add(u64::try_from(self.pending.len())?)
            .context("committed processing output record count overflow")?;
        let committed_bytes =
            self.pending
                .iter()
                .try_fold(self.committed_bytes, |bytes, row| {
                    bytes
                        .checked_add(u64::try_from(row.line.len())?)
                        .context("committed processing output byte count overflow")
                })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO history_output(session_id, ts, pipeline, line) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for row in &self.pending {
                insert.execute(params![self.session_id, row.ts, row.pipeline, row.line])?;
            }
        }
        if let Some(last) = self.pending.last() {
            let updated = transaction.execute(
                "UPDATE history_sessions SET updated_at=?2 WHERE session_id=?1",
                params![self.session_id, last.ts],
            )?;
            ensure!(
                updated == 1,
                "processing output session identity is missing"
            );
        }
        transaction
            .commit()
            .context("commit processing output batch")?;
        self.committed_records = committed_records;
        self.committed_bytes = committed_bytes;
        self.pending.clear();
        self.pending_bytes = 0;
        Ok(())
    }
}

/// Commit one structured history event. Never fall back to a second history store.
pub fn append_history_event(log_dir: &Path, session_id: &str, event: &str) -> Result<()> {
    ensure!(!session_id.is_empty(), "history session identity is empty");
    let (name, payload) = event
        .split_once('=')
        .context("history event has no payload")?;
    ensure!(
        matches!(
            name,
            "MFB_HISTORY_CONTEXT"
                | "MFB_HISTORY_INPUT_PLAN"
                | "MFB_HISTORY_CONFIG"
                | "MFB_HISTORY_INVOCATION"
                | "MFB_HISTORY_WORKER_EXECUTION"
                | "MFB_HISTORY_OUTPUT_STORAGE"
                | "MFB_HISTORY_SUMMARY"
                | "MFB_HISTORY_VERIFICATION"
                | "MFB_HISTORY_OUTPUT_CLEANUP"
                | "MFB_HISTORY_FAST_IMG_RESULT"
                | "MFB_HISTORY_FINISHED"
        ),
        "unsupported history event: {name}"
    );
    let payload: serde_json::Value = serde_json::from_str(payload)?;
    ensure!(
        payload["schema_version"] == 1,
        "unsupported history payload version"
    );

    let _lease = acquire_history_session_lease(log_dir, session_id)?;
    let mut connection = open_history_write(log_dir)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    cleanup::claim_history_session(&transaction, session_id)?;
    let timestamp = now_timestamp();
    transaction.execute(
        "INSERT INTO history_sessions(session_id, updated_at) VALUES (?1, ?2)
         ON CONFLICT(session_id) DO UPDATE SET updated_at=excluded.updated_at",
        params![session_id, timestamp],
    )?;
    transaction.execute(
        "INSERT INTO history_events(session_id, ts, event) VALUES (?1, ?2, ?3)",
        params![session_id, timestamp, event],
    )?;
    transaction.commit().context("commit processing history")
}

/// Return the latest output sequence for a session, or zero when no database exists yet.
pub fn session_output_cursor(log_dir: &Path, session_id: &str) -> Result<i64> {
    ensure!(!session_id.is_empty(), "history session identity is empty");
    let Some(connection) = open_history_read(log_dir)? else {
        return Ok(0);
    };
    let transaction = connection.unchecked_transaction()?;
    let version = schema_version(&transaction)?;
    let cursor = if version == 1 {
        0
    } else {
        transaction.query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM history_output WHERE session_id=?1",
            [session_id],
            |row| row.get(0),
        )?
    };
    ensure!(cursor >= 0, "processing output sequence is negative");
    transaction.commit()?;
    Ok(cursor)
}

/// Read one session's captured output after a cursor, bounded to 64 MiB.
pub fn read_session_output_since(
    log_dir: &Path,
    session_id: &str,
    after_sequence: i64,
) -> Result<String> {
    read_selected_session_output(log_dir, session_id, after_sequence, false)
}

/// Read finalization receipts, size facts and retained-file summaries, not verbose diagnostics.
pub fn read_fast_img_finalization_since(
    log_dir: &Path,
    session_id: &str,
    after_sequence: i64,
) -> Result<String> {
    read_selected_session_output(log_dir, session_id, after_sequence, true)
}

fn read_selected_session_output(
    log_dir: &Path,
    session_id: &str,
    after_sequence: i64,
    finalization_only: bool,
) -> Result<String> {
    ensure!(!session_id.is_empty(), "history session identity is empty");
    ensure!(after_sequence >= 0, "history output cursor is negative");
    let Some(connection) = open_history_read(log_dir)? else {
        return Ok(String::new());
    };
    let transaction = connection.unchecked_transaction()?;
    let version = schema_version(&transaction)?;
    if version == 1 {
        transaction.commit()?;
        return Ok(String::new());
    }
    let mut statement = transaction.prepare(
        "SELECT line FROM history_output WHERE session_id=?1 AND sequence>?2
         AND (?3=0 OR instr(line,'MFB_FAST_IMG_RESULT=')>0
              OR instr(line,'MFB_CONVERTED_BYTES=')>0
              OR substr(ltrim(line),1,10)='[SIZE    ]'
              OR substr(ltrim(line),1,13)='[FAIL    ]   '
              OR substr(ltrim(line),1,13)='[SKIP    ]   ')
         ORDER BY sequence",
    )?;
    let mut rows = statement.query(params![session_id, after_sequence, finalization_only])?;
    let mut output = String::new();
    while let Some(row) = rows.next()? {
        let line = row.get_ref(0)?.as_str()?;
        ensure!(
            line.len() < OUTPUT_READ_LIMIT - output.len(),
            "processing output exceeds the 64 MiB read limit"
        );
        output.push_str(line);
        output.push('\n');
    }
    drop(rows);
    drop(statement);
    transaction.commit()?;
    Ok(output)
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn history_path(log_dir: &Path) -> Result<PathBuf> {
    Ok(log_dir
        .canonicalize()
        .context("resolve history directory")?
        .join(HISTORY_DATABASE))
}

fn open_history_write(log_dir: &Path) -> Result<Connection> {
    let path = history_path(log_dir)?;
    let mut connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .with_context(|| format!("open processing history {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    ensure_schema(&mut connection)?;
    foundation::mfb_sqlite_store::enable_wal(&connection)
        .context("processing history schema retained; failed to enable WAL")?;
    persist_history_wal_sidecars(&connection)?;
    Ok(connection)
}

fn persist_history_wal_sidecars(connection: &Connection) -> Result<()> {
    let mut persist = 1_i32;
    // Keep cold history readable by read-only system SQLite without reopening it writable.
    // SAFETY: the live connection owns this handle; SQLite uses the integer only during this call.
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_PERSIST_WAL,
            std::ptr::from_mut(&mut persist).cast(),
        )
    };
    if result != rusqlite::ffi::SQLITE_OK {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(result),
            None,
        ))
        .context(
            "processing history retained; failed to preserve WAL sidecars for read-only readers",
        );
    }
    ensure!(
        persist == 1,
        "history SQLite VFS did not preserve WAL sidecars"
    );
    Ok(())
}

fn open_history_read(log_dir: &Path) -> Result<Option<Connection>> {
    let path = history_path(log_dir)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "processing history is not a regular file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect processing history file"),
    }
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .with_context(|| format!("open processing history read-only {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA query_only=ON;")?;
    Ok(Some(connection))
}

fn schema_version(transaction: &Transaction<'_>) -> Result<i64> {
    let version = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        (1..=SCHEMA_VERSION).contains(&version),
        "unsupported history database version {version}; original retained"
    );
    validate_schema(transaction, version)?;
    Ok(version)
}

fn ensure_schema(connection: &mut Connection) -> Result<()> {
    {
        let snapshot = connection.transaction()?;
        let version: i64 = snapshot.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == SCHEMA_VERSION {
            return validate_schema(&snapshot, version);
        }
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => {
            let tables: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            ensure!(
                tables == 0,
                "unversioned history database is not empty; original retained"
            );
            transaction.execute_batch(
                "CREATE TABLE history_sessions (
                    session_id TEXT PRIMARY KEY NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE history_events (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES history_sessions(session_id),
                    ts TEXT NOT NULL,
                    event TEXT NOT NULL
                );
                CREATE INDEX history_sessions_recent ON history_sessions(updated_at DESC);
                CREATE INDEX history_events_session ON history_events(session_id, sequence);
                PRAGMA user_version=1;",
            )?;
            migrate_history_v1(&transaction)?;
        }
        1 => migrate_history_v1(&transaction)?,
        SCHEMA_VERSION => validate_schema(&transaction, SCHEMA_VERSION)?,
        _ => bail!("unsupported history database version {version}; original retained"),
    }
    transaction
        .commit()
        .context("initialize processing history schema")
}

fn migrate_history_v1(transaction: &Transaction<'_>) -> Result<()> {
    validate_schema(transaction, 1)?;
    transaction.execute_batch(
        "CREATE TABLE history_output (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL REFERENCES history_sessions(session_id),
            ts TEXT NOT NULL,
            pipeline TEXT NOT NULL,
            line TEXT NOT NULL
        );
        CREATE INDEX history_output_session_sequence ON history_output(session_id, sequence);
        PRAGMA user_version=2;",
    )?;
    Ok(())
}

fn validate_schema(transaction: &Transaction<'_>, version: i64) -> Result<()> {
    transaction.prepare("SELECT session_id, updated_at FROM history_sessions LIMIT 0")?;
    transaction.prepare("SELECT sequence, session_id, ts, event FROM history_events LIMIT 0")?;
    if version >= 2 {
        transaction.prepare(
            "SELECT sequence, session_id, ts, pipeline, line FROM history_output LIMIT 0",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_inspection_counts_committed_utf8_rows_without_creating_or_migrating() -> Result<()> {
        let root = tempfile::tempdir()?;
        let missing = root.path().join("missing");
        let absent = inspect_history_store(&missing, true)?;
        assert!(!absent.exists && absent.overview.is_none() && absent.integrity.is_none());
        assert!(!missing.exists());
        append_history_event(root.path(), "one", EVENT)?;
        let mut writer = HistoryOutputWriter::open(root.path(), "one")?;
        writer.push_line("IMG", "\u{56fe}\n\"\u{1f680}")?;
        writer.push_line("VID", "")?;
        writer.flush()?;
        drop(writer);
        let source = root.path().join(HISTORY_DATABASE);
        let before = std::fs::read(&source)?;
        let mut concurrent = open_history_write(root.path())?;
        let uncommitted = concurrent.transaction()?;
        uncommitted.execute(
            "INSERT INTO history_sessions VALUES ('not-committed', 'later')",
            [],
        )?;
        let inspected = inspect_history_store(root.path(), true)?;
        let overview = inspected.overview.context("missing overview")?;
        assert_eq!(
            (overview.sessions, overview.events, overview.output_rows),
            (1, 1, Some(2))
        );
        assert_eq!(overview.event_bytes, u64::try_from(EVENT.len())?);
        assert_eq!(
            overview.output_bytes,
            Some(u64::try_from("\u{56fe}\n\"\u{1f680}".len())?)
        );
        assert!(overview.oldest_session_update.is_some());
        assert_eq!(
            inspected.integrity.context("missing integrity")?.status,
            "healthy"
        );
        assert_eq!(std::fs::read(&source)?, before);
        uncommitted.rollback()?;
        drop(concurrent);
        let legacy_dir = backup_fixture(root.path(), 1);
        let legacy_path = legacy_dir.join(HISTORY_DATABASE);
        let legacy_before = std::fs::read(&legacy_path)?;
        let legacy = inspect_history_store(&legacy_dir, true)?
            .overview
            .context("legacy overview")?;
        assert_eq!(legacy.history_schema_version, 1);
        assert!(legacy.output_rows.is_none() && legacy.output_bytes.is_none());
        assert_eq!(std::fs::read(&legacy_path)?, legacy_before);
        Ok(())
    }

    #[test]
    fn storage_integrity_reports_relationship_and_type_issues_without_payload_claims() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        append_history_event(root.path(), "one", EVENT)?;
        let conn = open_history_write(root.path())?;
        conn.pragma_update(None, "foreign_keys", false)?;
        for _ in 0..25 {
            conn.execute("INSERT INTO history_events(session_id,ts,event) VALUES ('orphan','now','not JSON')", [])?;
        }
        conn.execute(
            "INSERT INTO history_output(session_id,ts,pipeline,line) VALUES ('one','now','',X'00')",
            [],
        )?;
        let inspected = inspect_history_store(root.path(), true)?;
        let integrity = inspected.integrity.context("missing integrity")?;
        assert_eq!(integrity.status, "unhealthy");
        assert!(integrity.checks_complete);
        assert_eq!(integrity.sqlite_integrity, "ok");
        assert_eq!(integrity.foreign_keys, "failed");
        assert_eq!(integrity.record_types, "failed");
        assert_eq!(integrity.issue_count, 26);
        assert_eq!(integrity.issues.len(), 20);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM history_events", [], |row| row
                .get::<_, i64>(0))?,
            26
        );
        // A text event is not JSON-validated by the structural checker.
        conn.execute("DELETE FROM history_events WHERE session_id='orphan'", [])?;
        conn.execute("DELETE FROM history_output", [])?;
        conn.execute("UPDATE history_events SET event='not JSON'", [])?;
        assert_eq!(
            inspect_history_store(root.path(), true)?
                .integrity
                .context("missing integrity")?
                .status,
            "healthy"
        );
        Ok(())
    }

    #[test]
    fn storage_inspection_refuses_unknown_corrupt_and_linked_sources() -> Result<()> {
        let root = tempfile::tempdir()?;
        assert!(inspect_history_store(Path::new("relative"), false).is_err());
        append_history_event(root.path(), "one", EVENT)?;
        let conn = open_history_write(root.path())?;
        conn.pragma_update(None, "user_version", 999)?;
        drop(conn);
        let source = root.path().join(HISTORY_DATABASE);
        let before = std::fs::read(&source)?;
        assert!(inspect_history_store(root.path(), true).is_err());
        assert_eq!(std::fs::read(&source)?, before);
        let bad = root.path().join("bad");
        std::fs::create_dir(&bad)?;
        std::fs::write(bad.join(HISTORY_DATABASE), b"not SQLite")?;
        assert!(inspect_history_store(&bad, true).is_err());
        assert_eq!(std::fs::read(bad.join(HISTORY_DATABASE))?, b"not SQLite");
        #[cfg(unix)]
        {
            let linked = root.path().join("linked");
            std::os::unix::fs::symlink(&bad, &linked)?;
            assert!(inspect_history_store(&linked, false).is_err());
            let sidecar = bad.join(format!("{HISTORY_DATABASE}-wal"));
            std::os::unix::fs::symlink(&source, &sidecar)?;
            assert!(inspect_history_store(&bad, false).is_err());
        }
        Ok(())
    }

    #[test]
    fn storage_inspection_rejects_duplicate_sequence_schema_without_repair() -> Result<()> {
        let root = tempfile::tempdir()?;
        append_history_event(root.path(), "one", EVENT)?;
        let conn = open_history_write(root.path())?;
        conn.pragma_update(None, "foreign_keys", false)?;
        conn.execute_batch("DROP TABLE history_events;
            CREATE TABLE history_events(sequence INTEGER NOT NULL,session_id TEXT NOT NULL REFERENCES history_sessions(session_id),ts TEXT NOT NULL,event TEXT NOT NULL);
            INSERT INTO history_events VALUES(1,'one','now','first'),(1,'one','now','second');")?;
        assert!(inspect_history_store(root.path(), true).is_err());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM history_events", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        let destination_directory = tempfile::tempdir()?;
        let destination = destination_directory
            .path()
            .join("should-not-publish.sqlite");
        assert!(backup_history_store(root.path(), &destination).is_err());
        assert!(!destination.exists());
        Ok(())
    }

    const EVENT: &str = "MFB_HISTORY_FINISHED={\"schema_version\":1,\"outcome\":\"failed\",\"error\":\"synthetic\\nerror\"}";

    #[test]
    fn closed_history_retains_wal_sidecars_for_read_only_readers() {
        let directory = tempfile::tempdir().unwrap();
        append_history_event(directory.path(), "cold-history", EVENT).unwrap();
        let path = directory.path().join(HISTORY_DATABASE);
        let wal = directory.path().join(format!("{HISTORY_DATABASE}-wal"));
        let shm = directory.path().join(format!("{HISTORY_DATABASE}-shm"));
        assert!(wal.is_file() && shm.is_file());
        let before = std::fs::read(&path).unwrap();
        let wal_before = std::fs::read(&wal).unwrap();
        let reader = open_history_read(directory.path()).unwrap().unwrap();
        assert_eq!(
            reader
                .query_row("SELECT event FROM history_events", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            EVENT
        );
        assert!(reader.execute("DELETE FROM history_events", []).is_err());
        drop(reader);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read(&wal).unwrap(), wal_before);
    }

    fn backup_fixture(root: &Path, version: i64) -> PathBuf {
        let directory = root.join("history");
        std::fs::create_dir(&directory).unwrap();
        let connection = Connection::open(directory.join(HISTORY_DATABASE)).unwrap();
        connection.execute_batch(
            "CREATE TABLE history_sessions(session_id TEXT PRIMARY KEY NOT NULL, updated_at TEXT NOT NULL);
             CREATE TABLE history_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES history_sessions(session_id), ts TEXT NOT NULL, event TEXT NOT NULL);
             CREATE INDEX history_sessions_recent ON history_sessions(updated_at DESC);
             CREATE INDEX history_events_session ON history_events(session_id, sequence);
             INSERT INTO history_sessions VALUES ('original', 'synthetic timestamp');
             INSERT INTO history_events(session_id, ts, event) VALUES ('original', 'synthetic timestamp', 'UNRECOGNIZED_RAW_EVENT=not rewritten');
             CREATE TABLE extra_raw_evidence(payload BLOB);
             INSERT INTO extra_raw_evidence VALUES (x'000102ff');
             PRAGMA user_version=1;",
        ).unwrap();
        if version == 2 {
            let mut connection = connection;
            let transaction = connection.transaction().unwrap();
            migrate_history_v1(&transaction).unwrap();
            transaction.execute("INSERT INTO history_output(session_id, ts, pipeline, line) VALUES ('original', 'synthetic timestamp', 'img', 'raw synthetic output')", []).unwrap();
            transaction.commit().unwrap();
        }
        directory
    }

    fn no_staging_remains(root: &Path) {
        assert!(!std::fs::read_dir(root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".mfb-history-backup-")
        }));
    }

    #[test]
    fn history_backup_preserves_schema_one_raw_rows_without_migration() {
        let root = tempfile::tempdir().unwrap();
        let directory = backup_fixture(root.path(), 1);
        let source = directory.join(HISTORY_DATABASE);
        let before = std::fs::read(&source).unwrap();
        let destination = root.path().join("backup.sqlite");
        let receipt = backup_history_store(&directory, &destination).unwrap();
        assert_eq!(
            (
                receipt.schema_version,
                receipt.kind,
                receipt.history_schema_version
            ),
            (1, "history_sqlite", 1)
        );
        assert_eq!(
            (receipt.sessions, receipt.events, receipt.output_rows),
            (1, 1, 0)
        );
        assert_eq!(
            receipt.source,
            source.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            receipt.destination,
            destination.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            receipt.bytes,
            std::fs::metadata(&destination).unwrap().len()
        );
        let backup = Connection::open(&destination).unwrap();
        let raw: String = backup
            .query_row("SELECT event FROM history_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(raw, "UNRECOGNIZED_RAW_EVENT=not rewritten");
        assert_eq!(
            backup
                .query_row("SELECT payload FROM extra_raw_evidence", [], |row| row
                    .get::<_, Vec<u8>>(0))
                .unwrap(),
            [0, 1, 2, 255]
        );
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(backup.prepare("SELECT * FROM history_output").is_err());
        assert_eq!(std::fs::read(&source).unwrap(), before);
        no_staging_remains(root.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&destination)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn history_backup_includes_committed_wal_and_excludes_uncommitted_rows() {
        let root = tempfile::tempdir().unwrap();
        let directory = backup_fixture(root.path(), 2);
        let source = directory.join(HISTORY_DATABASE);
        let writer = Connection::open(&source).unwrap();
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        writer.execute_batch("PRAGMA foreign_keys=ON;
            INSERT INTO history_sessions VALUES ('wal', 'synthetic');
            INSERT INTO history_events(session_id, ts, event) VALUES ('wal', 'synthetic', 'committed');
            INSERT INTO history_output(session_id, ts, pipeline, line) VALUES ('wal', 'synthetic', 'vid', 'committed output');
            BEGIN;
            INSERT INTO history_sessions VALUES ('pending', 'synthetic');
            INSERT INTO history_events(session_id, ts, event) VALUES ('pending', 'synthetic', 'uncommitted');
            INSERT INTO history_output(session_id, ts, pipeline, line) VALUES ('pending', 'synthetic', 'vid', 'uncommitted output');").unwrap();
        let before = std::fs::read(&source).unwrap();
        let wal = directory.join(format!("{HISTORY_DATABASE}-wal"));
        let wal_before = std::fs::read(&wal).unwrap();
        assert!(!wal_before.is_empty());
        let destination = root.path().join("backup.sqlite");
        let receipt = backup_history_store(&directory, &destination).unwrap();
        assert_eq!(
            (
                receipt.history_schema_version,
                receipt.sessions,
                receipt.events,
                receipt.output_rows
            ),
            (2, 2, 2, 2)
        );
        let backup = Connection::open(&destination).unwrap();
        assert_eq!(
            backup
                .query_row(
                    "SELECT COUNT(*) FROM history_sessions WHERE session_id='pending'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            backup
                .query_row(
                    "SELECT line FROM history_output WHERE session_id='wal'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "committed output"
        );
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert_eq!(std::fs::read(&wal).unwrap(), wal_before);
        writer.execute_batch("ROLLBACK").unwrap();
        no_staging_remains(root.path());
    }

    #[test]
    fn history_backup_rejects_missing_relative_existing_and_private_workspace_targets() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("backup.sqlite");
        assert!(backup_history_store(&root.path().join("absent"), &destination).is_err());
        assert!(backup_history_store(Path::new("relative-history"), &destination).is_err());
        assert!(backup_history_store(root.path(), &destination).is_err());
        assert!(!root.path().join(HISTORY_DATABASE).exists() && !destination.exists());
        let directory = backup_fixture(root.path(), 2);
        let source = directory.join(HISTORY_DATABASE);
        let before = std::fs::read(&source).unwrap();
        std::fs::write(&destination, b"existing destination").unwrap();
        assert!(backup_history_store(&directory, &destination).is_err());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"existing destination"
        );
        std::fs::remove_file(&destination).unwrap();
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = root.path().join(format!("backup.sqlite{suffix}"));
            std::fs::write(&sidecar, b"existing sidecar").unwrap();
            assert!(backup_history_store(&directory, &destination).is_err());
            assert_eq!(std::fs::read(&sidecar).unwrap(), b"existing sidecar");
            assert!(!destination.exists());
            std::fs::remove_file(sidecar).unwrap();
        }
        assert!(backup_history_store(&directory, &directory.join("inside.sqlite")).is_err());
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join(".git"), b"synthetic worktree marker").unwrap();
        assert!(backup_history_store(&directory, &workspace.join("backup.sqlite")).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), before);
        no_staging_remains(root.path());
    }

    #[test]
    fn history_backup_refuses_unknown_schema_missing_tables_foreign_keys_and_corruption() {
        for alteration in [
            "PRAGMA user_version=99;",
            "DROP TABLE history_output;",
            "INSERT INTO history_events(session_id, ts, event) VALUES ('missing-session', 'synthetic', 'orphan');",
            "DROP TABLE history_events; CREATE TABLE history_events(sequence INTEGER PRIMARY KEY, session_id TEXT, ts TEXT, event TEXT);",
            "PRAGMA user_version=1;",
        ] {
            let root = tempfile::tempdir().unwrap();
            let directory = backup_fixture(root.path(), 2);
            let source = directory.join(HISTORY_DATABASE);
            let connection = Connection::open(&source).unwrap();
            connection
                .pragma_update(None, "foreign_keys", false)
                .unwrap();
            connection.execute_batch(alteration).unwrap();
            drop(connection);
            let before = std::fs::read(&source).unwrap();
            let destination = root.path().join("backup.sqlite");
            assert!(
                backup_history_store(&directory, &destination).is_err(),
                "{alteration}"
            );
            assert_eq!(std::fs::read(&source).unwrap(), before);
            assert!(!destination.exists());
            no_staging_remains(root.path());
        }
        let root = tempfile::tempdir().unwrap();
        let directory = backup_fixture(root.path(), 2);
        let source = directory.join(HISTORY_DATABASE);
        std::fs::write(&source, b"not a database").unwrap();
        assert!(backup_history_store(&directory, &root.path().join("backup.sqlite")).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"not a database");
        no_staging_remains(root.path());
    }

    #[cfg(unix)]
    #[test]
    fn history_backup_rejects_symlink_directory_source_sidecars_and_destination() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let directory = backup_fixture(root.path(), 2);
        let source = directory.join(HISTORY_DATABASE);
        let outside = root.path().join("outside.sqlite");
        std::fs::copy(&source, &outside).unwrap();
        let original = std::fs::read(&outside).unwrap();
        let destination = root.path().join("backup.sqlite");
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = directory.join(format!("{HISTORY_DATABASE}{suffix}"));
            symlink(&outside, &sidecar).unwrap();
            assert!(backup_history_store(&directory, &destination).is_err());
            std::fs::remove_file(sidecar).unwrap();
        }
        symlink(&outside, &destination).unwrap();
        assert!(backup_history_store(&directory, &destination).is_err());
        std::fs::remove_file(&destination).unwrap();
        let alias = root.path().join("alias");
        symlink(&directory, &alias).unwrap();
        assert!(backup_history_store(&alias, &destination).is_err());
        std::fs::remove_file(&source).unwrap();
        symlink(&outside, &source).unwrap();
        assert!(backup_history_store(&directory, &destination).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), original);
        assert!(!destination.exists());
        no_staging_remains(root.path());
    }

    #[test]
    fn config_history_marker_is_allowed_without_changing_schema() {
        let directory = tempfile::tempdir().unwrap();
        let event = "MFB_HISTORY_CONFIG={\"schema_version\":1,\"pipeline\":\"img\"}";
        append_history_event(directory.path(), "session", event).unwrap();
        let connection = Connection::open(directory.path().join(HISTORY_DATABASE)).unwrap();
        let saved: String = connection
            .query_row("SELECT event FROM history_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(saved, event);
        let input_plan = "MFB_HISTORY_INPUT_PLAN={\"schema_version\":1,\"inputs\":[]}";
        append_history_event(directory.path(), "session", input_plan).unwrap();
        let saved: String = connection
            .query_row(
                "SELECT event FROM history_events ORDER BY sequence DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(saved, input_plan);
        assert!(
            append_history_event(
                directory.path(),
                "session",
                "MFB_HISTORY_INPUT_PLAN={\"schema_version\":2}"
            )
            .is_err()
        );
    }

    #[test]
    fn output_storage_history_marker_uses_current_schema() {
        let directory = tempfile::tempdir().unwrap();
        let event = "MFB_HISTORY_OUTPUT_STORAGE={\"schema_version\":1,\"pipeline\":\"img\"}";
        append_history_event(directory.path(), "session", event).unwrap();
        let connection = Connection::open(directory.path().join(HISTORY_DATABASE)).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT event FROM history_events", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            event
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert!(
            append_history_event(
                directory.path(),
                "session",
                "MFB_HISTORY_OUTPUT_STORAGE={\"schema_version\":2}"
            )
            .is_err()
        );
    }

    #[test]
    fn history_is_ordered_transactional_and_rejects_unknown_databases() {
        let directory = tempfile::tempdir().unwrap();
        for session in ["first", "second", "first"] {
            append_history_event(directory.path(), session, EVENT).unwrap();
        }
        let path = directory.path().join(HISTORY_DATABASE);
        let connection = Connection::open(&path).unwrap();
        let mut query = connection
            .prepare("SELECT session_id, event FROM history_events ORDER BY sequence")
            .unwrap();
        let events = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            events,
            [
                ("first".into(), EVENT.into()),
                ("second".into(), EVENT.into()),
                ("first".into(), EVENT.into())
            ]
        );
        connection.execute_batch("CREATE TRIGGER reject_event BEFORE INSERT ON history_events BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
        assert!(append_history_event(directory.path(), "rejected", EVENT).is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM history_sessions WHERE session_id='rejected'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        connection.pragma_update(None, "user_version", 99).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(
            append_history_event(directory.path(), "future", EVENT)
                .unwrap_err()
                .to_string()
                .contains("original retained")
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        connection.pragma_update(None, "user_version", 0).unwrap();
        assert!(
            append_history_event(directory.path(), "unversioned", EVENT)
                .unwrap_err()
                .to_string()
                .contains("not empty")
        );
    }

    #[test]
    fn schema_one_migrates_without_changing_existing_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(HISTORY_DATABASE);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(
            "CREATE TABLE history_sessions(session_id TEXT PRIMARY KEY NOT NULL, updated_at TEXT NOT NULL);
             CREATE TABLE history_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES history_sessions(session_id), ts TEXT NOT NULL, event TEXT NOT NULL);
             CREATE INDEX history_sessions_recent ON history_sessions(updated_at DESC);
             CREATE INDEX history_events_session ON history_events(session_id, sequence);
             INSERT INTO history_sessions VALUES ('old-session', '2026-01-02T03:04:05.000000Z');
             INSERT INTO history_events(session_id, ts, event) VALUES ('old-session', '2026-01-02T03:04:05.000000Z', 'original event');
             PRAGMA user_version=1;",
        ).unwrap();
        drop(connection);

        let mut writer = HistoryOutputWriter::open(directory.path(), "new-session").unwrap();
        writer.push_line("img", "captured").unwrap();
        writer.flush().unwrap();
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT session_id || ':' || event FROM history_events",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "old-session:original event"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT pipeline || ':' || line FROM history_output",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "img:captured"
        );
    }

    #[test]
    fn failed_schema_migration_retains_original_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(HISTORY_DATABASE);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(
            "CREATE TABLE history_sessions(session_id TEXT PRIMARY KEY NOT NULL, updated_at TEXT NOT NULL);
             CREATE TABLE history_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES history_sessions(session_id), ts TEXT NOT NULL, event TEXT NOT NULL);
             INSERT INTO history_sessions VALUES ('original', '2026-01-02T03:04:05Z');
             INSERT INTO history_events(session_id, ts, event) VALUES ('original', '2026-01-02T03:04:05Z', 'retained');
             CREATE INDEX history_output_session_sequence ON history_events(session_id, sequence);
             PRAGMA user_version=1;",
        ).unwrap();
        drop(connection);
        let original = std::fs::read(&path).unwrap();
        assert!(HistoryOutputWriter::open(directory.path(), "new").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name='history_output'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn fast_img_evidence_is_independent_of_verbose_log_size() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = HistoryOutputWriter::open(directory.path(), "batch").unwrap();
        writer
            .push_line("img", "MFB_FAST_IMG_RESULT=old-attempt")
            .unwrap();
        writer.flush().unwrap();
        let cursor = session_output_cursor(directory.path(), "batch").unwrap();
        let connection = Connection::open(directory.path().join(HISTORY_DATABASE)).unwrap();
        connection.execute(
            "INSERT INTO history_output(session_id, ts, pipeline, line) VALUES ('batch','now','img',CAST(zeroblob(?1) AS TEXT))",
            [OUTPUT_READ_LIMIT as i64],
        ).unwrap();
        let evidence = "MFB_FAST_IMG_RESULT=current-attempt\n[FAIL    ]   bad.jpeg: retained\n[SIZE    ] files_converted: 2\nMFB_CONVERTED_BYTES={\"schema_version\":1,\"scope\":\"converted_this_run\",\"input_bytes\":null,\"output_bytes\":null}";
        for line in evidence.lines() {
            writer.push_line("img", line).unwrap();
        }
        writer.flush().unwrap();
        assert!(read_session_output_since(directory.path(), "batch", cursor).is_err());
        assert_eq!(
            read_fast_img_finalization_since(directory.path(), "batch", cursor).unwrap(),
            format!("{evidence}\n")
        );
        assert!(
            read_fast_img_finalization_since(directory.path(), "other", 0)
                .unwrap()
                .is_empty()
        );
        writer
            .push_line("img", "MFB_FAST_IMG_RESULT=conflicting")
            .unwrap();
        writer.flush().unwrap();
        assert_eq!(
            read_fast_img_finalization_since(directory.path(), "batch", cursor)
                .unwrap()
                .matches("MFB_FAST_IMG_RESULT=")
                .count(),
            2
        );
        let decorated = [
            "\tMFB_FAST_IMG_RESULT=indented-conflict",
            "\x1B[31mMFB_CONVERTED_BYTES=invalid-json\x1B[0m",
        ];
        for line in decorated {
            writer.push_line("img", line).unwrap();
        }
        writer.flush().unwrap();
        let selected = read_fast_img_finalization_since(directory.path(), "batch", cursor).unwrap();
        assert_eq!(selected.matches("MFB_FAST_IMG_RESULT=").count(), 3);
        assert_eq!(selected.matches("MFB_CONVERTED_BYTES=").count(), 2);
        assert!(selected.ends_with(&format!("{}\n{}\n", decorated[0], decorated[1])));
    }

    #[test]
    fn output_writer_batches_orders_and_scopes_lines() {
        let directory = tempfile::tempdir().unwrap();
        let mut first = HistoryOutputWriter::open(directory.path(), "first").unwrap();
        let mut second = HistoryOutputWriter::open(directory.path(), "second").unwrap();
        for index in 0..OUTPUT_BATCH_LINES {
            first.push_line("img", &format!("line {index}")).unwrap();
        }
        assert!(first.pending.is_empty(), "line threshold flushes the batch");
        assert_eq!(first.committed_totals().0, OUTPUT_BATCH_LINES as u64);
        let phase_output = concat!(
            "你好\n",
            "MFB_LOG_PHASE={\"schema_version\":1,\"phase\":\"processing\"}\n",
            "[ERROR] source retained\n",
            "MFB_LOG_PHASE={\"schema_version\":1,\"phase\":\"verification\"}\n",
            "second line"
        );
        first.push_line("vid", phase_output).unwrap();
        second.push_line("img", "other session").unwrap();
        first.flush().unwrap();
        second.flush().unwrap();

        assert_eq!(
            session_output_cursor(directory.path(), "first").unwrap(),
            129
        );
        assert_eq!(
            session_output_cursor(directory.path(), "second").unwrap(),
            130
        );
        assert_eq!(
            read_session_output_since(directory.path(), "first", 127).unwrap(),
            format!("line 127\n{phase_output}\n")
        );
        assert_eq!(
            read_session_output_since(directory.path(), "second", 0).unwrap(),
            "other session\n"
        );
        assert_eq!(
            read_session_output_since(directory.path(), "first", 129).unwrap(),
            ""
        );
    }

    #[test]
    fn output_writer_counts_only_its_committed_records_and_utf8_line_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = HistoryOutputWriter::open(directory.path(), "session").unwrap();
        assert_eq!(writer.committed_totals(), (0, 0));
        writer.flush().unwrap();
        writer.push_line("img", "你好").unwrap();
        writer.push_line("vid", "").unwrap();
        assert_eq!(writer.committed_totals(), (0, 0));
        writer.flush().unwrap();
        assert_eq!(writer.committed_totals(), (2, 6));
        writer.flush().unwrap();
        assert_eq!(writer.committed_totals(), (2, 6));
        writer.push_line("img", "tail\n").unwrap();
        assert_eq!(writer.committed_totals(), (2, 6));
        writer.flush().unwrap();
        assert_eq!(writer.committed_totals(), (3, 11));

        let mut next = HistoryOutputWriter::open(directory.path(), "session").unwrap();
        assert_eq!(next.committed_totals(), (0, 0));
        next.push_line("img", "é").unwrap();
        next.flush().unwrap();
        assert_eq!(next.committed_totals(), (1, 2));
        assert_eq!(writer.committed_totals(), (3, 11));
        assert_eq!(
            next.connection
                .query_row(
                    "SELECT COUNT(*), SUM(length(CAST(line AS BLOB))) FROM history_output",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            (4, 13)
        );
    }

    #[test]
    fn output_writer_rejects_counter_overflow_before_committing_rows() {
        for totals in [(u64::MAX, 0), (0, u64::MAX)] {
            let directory = tempfile::tempdir().unwrap();
            let mut writer = HistoryOutputWriter::open(directory.path(), "session").unwrap();
            (writer.committed_records, writer.committed_bytes) = totals;
            writer.push_line("img", "x").unwrap();
            assert!(writer.flush().is_err());
            assert_eq!(writer.committed_totals(), totals);
            assert_eq!(writer.pending.len(), 1);
            assert_eq!(
                session_output_cursor(directory.path(), "session").unwrap(),
                0
            );
            (writer.committed_records, writer.committed_bytes) = (0, 0);
            writer.flush().unwrap();
            assert_eq!(writer.committed_totals(), (1, 1));
            assert_eq!(
                session_output_cursor(directory.path(), "session").unwrap(),
                1
            );
        }
    }

    #[test]
    fn output_flush_rolls_back_and_keeps_buffer_on_failure() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = HistoryOutputWriter::open(directory.path(), "session").unwrap();
        writer.push_line("img", "already committed").unwrap();
        writer.flush().unwrap();
        let committed = writer.committed_totals();
        writer.push_line("img", "staged before failure").unwrap();
        writer.push_line("img", "kept for retry").unwrap();
        let connection = Connection::open(directory.path().join(HISTORY_DATABASE)).unwrap();
        connection.execute_batch(
            "CREATE TRIGGER reject_output BEFORE INSERT ON history_output WHEN NEW.line = 'kept for retry' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        ).unwrap();
        assert!(writer.flush().is_err());
        assert_eq!(writer.pending.len(), 2);
        assert_eq!(writer.committed_totals(), committed);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM history_output", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        connection
            .execute_batch("DROP TRIGGER reject_output;")
            .unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.committed_totals(), (3, 52));
        let retried = writer.committed_totals();
        writer.flush().unwrap();
        assert_eq!(writer.committed_totals(), retried);
        assert_eq!(
            read_session_output_since(directory.path(), "session", 0).unwrap(),
            "already committed\nstaged before failure\nkept for retry\n"
        );
    }

    #[test]
    fn history_read_snapshot_does_not_block_events_or_captured_output() {
        let directory = tempfile::tempdir().unwrap();
        append_history_event(directory.path(), "original", EVENT).unwrap();
        let mut writer = HistoryOutputWriter::open(directory.path(), "worker").unwrap();
        writer
            .connection
            .busy_timeout(Duration::from_millis(20))
            .unwrap();
        let mut reader = open_history_read(directory.path()).unwrap().unwrap();
        let snapshot = reader.transaction().unwrap();
        assert_eq!(
            snapshot
                .query_row("SELECT COUNT(*) FROM history_events", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );

        writer.push_line("img", "committed while reading").unwrap();
        writer.flush().unwrap();
        append_history_event(directory.path(), "second", EVENT).unwrap();
        assert_eq!(
            snapshot
                .query_row("SELECT COUNT(*) FROM history_events", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1,
            "the reader retains its original consistent snapshot"
        );
        snapshot.commit().unwrap();
        assert_eq!(
            reader
                .query_row("SELECT COUNT(*) FROM history_events", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        assert_eq!(
            read_session_output_since(directory.path(), "worker", 0).unwrap(),
            "committed while reading\n"
        );
    }

    #[test]
    fn concurrent_history_openers_preserve_all_sessions_and_output() {
        let directory = tempfile::tempdir().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..6)
                .map(|index| {
                    let barrier = std::sync::Arc::clone(&barrier);
                    let path = directory.path();
                    scope.spawn(move || {
                        barrier.wait();
                        let session = format!("session-{index}");
                        append_history_event(path, &session, EVENT)?;
                        let mut writer = HistoryOutputWriter::open(path, &session)?;
                        writer.push_line("img", &session)?;
                        writer.flush()
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap().unwrap();
            }
        });
        let reader = open_history_read(directory.path()).unwrap().unwrap();
        for table in ["history_sessions", "history_events", "history_output"] {
            assert_eq!(
                reader
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                6
            );
        }
        assert_eq!(
            reader
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        assert_eq!(
            reader
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(
            reader
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query([])
                .unwrap()
                .next()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn output_reads_support_schema_one_and_reject_malformed_current_schema() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            session_output_cursor(directory.path(), "legacy").unwrap(),
            0
        );
        assert_eq!(
            read_session_output_since(directory.path(), "legacy", 0).unwrap(),
            ""
        );

        let path = directory.path().join(HISTORY_DATABASE);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(
            "CREATE TABLE history_sessions(session_id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
             CREATE TABLE history_events(sequence INTEGER PRIMARY KEY, session_id TEXT, ts TEXT, event TEXT);
             PRAGMA user_version=1;",
        ).unwrap();
        assert_eq!(
            session_output_cursor(directory.path(), "legacy").unwrap(),
            0
        );
        assert_eq!(
            read_session_output_since(directory.path(), "legacy", 0).unwrap(),
            ""
        );
        connection.execute_batch("PRAGMA user_version=2;").unwrap();
        assert!(session_output_cursor(directory.path(), "legacy").is_err());
        let original = std::fs::read(&path).unwrap();
        assert!(HistoryOutputWriter::open(directory.path(), "malformed").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            connection
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
        connection.execute_batch("PRAGMA user_version=99;").unwrap();
        assert!(session_output_cursor(directory.path(), "legacy").is_err());
    }
}
