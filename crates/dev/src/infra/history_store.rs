//! Durable processing history, separate from disposable analysis caches.

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::path::Path;
use std::time::Duration;

pub const HISTORY_DATABASE: &str = "history.sqlite3";
const SCHEMA_VERSION: i64 = 1;

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

    // Resolve directory aliases (for example macOS /var), but never follow a DB link.
    let path = log_dir
        .canonicalize()
        .context("resolve history directory")?
        .join(HISTORY_DATABASE);
    let mut connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .with_context(|| format!("open processing history {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
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
    } else {
        ensure!(
            version == SCHEMA_VERSION,
            "unsupported history database version {version}; original retained"
        );
    }
    let timestamp = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
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

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT: &str = "MFB_HISTORY_FINISHED={\"schema_version\":1,\"outcome\":\"failed\",\"error\":\"synthetic\\nerror\"}";

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
}
