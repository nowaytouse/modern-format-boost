//! Durable processing history, separate from disposable analysis caches.

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const HISTORY_DATABASE: &str = "history.sqlite3";
const SCHEMA_VERSION: i64 = 2;
const OUTPUT_BATCH_LINES: usize = 128;
const OUTPUT_BATCH_BYTES: usize = 64 * 1024;
const OUTPUT_READ_LIMIT: usize = 64 * 1024 * 1024;

struct OutputLine {
    ts: String,
    pipeline: String,
    line: String,
}

/// Buffered writer for the combined output captured from one processing session.
pub struct HistoryOutputWriter {
    connection: Connection,
    session_id: String,
    pending: Vec<OutputLine>,
    pending_bytes: usize,
}

impl HistoryOutputWriter {
    /// Open the history database once and ensure this session has an identity row.
    pub fn open(log_dir: &Path, session_id: &str) -> Result<Self> {
        ensure!(!session_id.is_empty(), "history session identity is empty");
        let mut connection = open_history_write(log_dir)?;
        ensure_schema(&mut connection)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
            connection,
            session_id: session_id.to_owned(),
            pending: Vec::new(),
            pending_bytes: 0,
        })
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
                | "MFB_HISTORY_CONFIG"
                | "MFB_HISTORY_SUMMARY"
                | "MFB_HISTORY_VERIFICATION"
                | "MFB_HISTORY_FINISHED"
        ),
        "unsupported history event: {name}"
    );
    let payload: serde_json::Value = serde_json::from_str(payload)?;
    ensure!(
        payload["schema_version"] == 1,
        "unsupported history payload version"
    );

    let mut connection = open_history_write(log_dir)?;
    ensure_schema(&mut connection)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
        "SELECT line FROM history_output WHERE session_id=?1 AND sequence>?2 ORDER BY sequence",
    )?;
    let mut rows = statement.query(params![session_id, after_sequence])?;
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
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .with_context(|| format!("open processing history {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    Ok(connection)
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

    const EVENT: &str = "MFB_HISTORY_FINISHED={\"schema_version\":1,\"outcome\":\"failed\",\"error\":\"synthetic\\nerror\"}";

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
    fn output_writer_batches_orders_and_scopes_lines() {
        let directory = tempfile::tempdir().unwrap();
        let mut first = HistoryOutputWriter::open(directory.path(), "first").unwrap();
        let mut second = HistoryOutputWriter::open(directory.path(), "second").unwrap();
        for index in 0..OUTPUT_BATCH_LINES {
            first.push_line("img", &format!("line {index}")).unwrap();
        }
        assert!(first.pending.is_empty(), "line threshold flushes the batch");
        first.push_line("vid", "你好\nsecond line").unwrap();
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
            "line 127\n你好\nsecond line\n"
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
    fn output_flush_rolls_back_and_keeps_buffer_on_failure() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = HistoryOutputWriter::open(directory.path(), "session").unwrap();
        writer.push_line("img", "kept for retry").unwrap();
        let connection = Connection::open(directory.path().join(HISTORY_DATABASE)).unwrap();
        connection.execute_batch(
            "CREATE TRIGGER reject_output BEFORE INSERT ON history_output BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        ).unwrap();
        assert!(writer.flush().is_err());
        assert_eq!(writer.pending.len(), 1);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM history_output", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        connection
            .execute_batch("DROP TRIGGER reject_output;")
            .unwrap();
        writer.flush().unwrap();
        assert_eq!(
            read_session_output_since(directory.path(), "session", 0).unwrap(),
            "kept for retry\n"
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
        connection.execute_batch("PRAGMA user_version=99;").unwrap();
        assert!(session_output_cursor(directory.path(), "legacy").is_err());
    }
}
