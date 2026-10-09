//! Shared local SQLite storage for reconstructable cache and durable run state.
//! Only the path-tree namespace may be discarded on integrity failure.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::runtime_config::CachePolicy;

pub const STORE_SCHEMA_VERSION: i32 = 2;
pub const NS_PATH_TREE: &str = "path_tree";
pub const NS_CHECKPOINT: &str = "checkpoint";
pub const NS_PROCESSED: &str = "processed";

const STORE_FILE_NAME: &str = "mfb_store.sqlite";
const STORE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
thread_local! {
    static TEST_STORE_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn now_unix_secs() -> Result<i64> {
    let secs = crate::media_conversion_gate::unix_epoch_secs_optional()
        .context("blob_store updated_at: system clock before UNIX epoch")?;
    i64::try_from(secs).context("blob_store updated_at: epoch seconds exceeded i64")
}

/// Default path: `~/.modern_format_boost/cache/mfb_store.sqlite`.
///
/// # Errors
/// Returns an error if the cache directory cannot be resolved or created.
pub fn default_store_path() -> Result<PathBuf> {
    let mut path = crate::common_utils::get_user_project_cache_dir()?;
    path.push(STORE_FILE_NAME);
    Ok(path)
}

fn migrate_v1_crc32_to_blake3(tx: &Transaction<'_>) -> Result<()> {
    #[derive(Debug)]
    struct LegacyBlob {
        namespace: String,
        cache_key: String,
        schema_version: i32,
        root_path: Option<String>,
        payload: Vec<u8>,
        payload_crc32: i32,
        updated_at: i64,
    }

    tx.execute_batch(
        "ALTER TABLE blob_store RENAME TO blob_store_crc32_v1;
         CREATE TABLE blob_store (
             namespace TEXT NOT NULL,
             cache_key TEXT NOT NULL,
             schema_version INTEGER NOT NULL,
             root_path TEXT,
             payload BLOB NOT NULL,
             payload_blake3 BLOB NOT NULL,
             updated_at INTEGER NOT NULL,
             PRIMARY KEY (namespace, cache_key)
         );",
    )
    .context("Failed to create BLAKE3 SQLite blob table")?;

    let mut statement = tx
        .prepare(
            "SELECT namespace, cache_key, schema_version, root_path, payload, payload_crc32,
         updated_at FROM blob_store_crc32_v1",
        )
        .context("Failed to read legacy SQLite blob rows")?;
    let rows = statement.query_map([], |row| {
        Ok(LegacyBlob {
            namespace: row.get(0)?,
            cache_key: row.get(1)?,
            schema_version: row.get(2)?,
            root_path: row.get(3)?,
            payload: row.get(4)?,
            payload_crc32: row.get(5)?,
            updated_at: row.get(6)?,
        })
    })?;
    let mut migrated_rows = 0usize;
    let mut rejected_rows = 0usize;
    for row in rows {
        let row = row?;
        if crc32fast::hash(&row.payload).cast_signed() != row.payload_crc32 {
            anyhow::ensure!(
                row.namespace == NS_PATH_TREE,
                "SQLite migration refused: {} state failed CRC32 verification; original database retained",
                row.namespace
            );
            rejected_rows += 1;
            continue;
        }
        let digest = blake3::hash(&row.payload);
        tx.execute(
            "INSERT INTO blob_store (namespace, cache_key, schema_version, root_path, payload, \
             payload_blake3, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.namespace,
                row.cache_key,
                row.schema_version,
                row.root_path,
                row.payload,
                digest.as_bytes().as_slice(),
                row.updated_at
            ],
        )?;
        migrated_rows += 1;
    }
    drop(statement);

    tx.execute_batch(
        "DROP TABLE blob_store_crc32_v1;
         CREATE INDEX idx_blob_ns_root ON blob_store(namespace, root_path);
         CREATE INDEX idx_blob_ns_updated ON blob_store(namespace, updated_at);",
    )
    .context("Failed to finish BLAKE3 SQLite blob migration")?;
    tx.execute(
        "UPDATE store_metadata SET value = ?1 WHERE key = 'schema_version'",
        params![STORE_SCHEMA_VERSION],
    )?;
    tracing::debug!(
        migrated_rows,
        rejected_rows,
        "SQLite integrity migration prepared; pending transaction commit"
    );
    Ok(())
}

fn schema_version(conn: &Connection) -> Result<Option<i32>> {
    let has_metadata: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'store_metadata')",
        [],
        |row| row.get(0),
    )?;
    if !has_metadata {
        let has_objects: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*')",
            [],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            !has_objects,
            "Unrecognized nonempty SQLite store; database retained without initialization"
        );
        return Ok(None);
    }
    let current: Option<i32> = conn
        .query_row(
            "SELECT value FROM store_metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to read mfb_store schema_version")?;
    let version =
        current.context("SQLite store is missing its schema version; database retained")?;
    anyhow::ensure!(
        matches!(version, 1 | STORE_SCHEMA_VERSION),
        "mfb_store.sqlite schema version mismatch (db={version}, expected={STORE_SCHEMA_VERSION}); \
         database retained, use a compatible application or a verified backup"
    );
    let query = if version == 1 {
        "SELECT namespace, cache_key, schema_version, root_path, payload, payload_crc32, updated_at FROM blob_store LIMIT 0"
    } else {
        "SELECT namespace, cache_key, schema_version, root_path, payload, payload_blake3, updated_at FROM blob_store LIMIT 0"
    };
    conn.prepare(query)
        .context("SQLite blob schema is incomplete; database retained without repair")?;
    for query in [
        "INSERT INTO blob_store (namespace, cache_key) VALUES (?1, ?2)
         ON CONFLICT(namespace, cache_key) DO NOTHING",
        "INSERT INTO store_metadata (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO NOTHING",
    ] {
        conn.prepare(query)
            .context("SQLite store lacks required unique keys; database retained without repair")?;
    }
    Ok(Some(version))
}

fn enable_wal(conn: &Connection) -> Result<()> {
    let started = Instant::now();
    loop {
        let remaining = STORE_BUSY_TIMEOUT.saturating_sub(started.elapsed());
        conn.busy_timeout(remaining)?;
        match conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0)) {
            Ok(mode) => {
                anyhow::ensure!(mode == "wal", "SQLite refused WAL mode (reported {mode})");
                conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
                return Ok(());
            }
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::DatabaseBusy
                    && started.elapsed() < STORE_BUSY_TIMEOUT =>
            {
                // WAL mode changes can return BUSY without invoking SQLite's busy handler.
                std::thread::sleep(
                    Duration::from_millis(25)
                        .min(STORE_BUSY_TIMEOUT.saturating_sub(started.elapsed())),
                );
            }
            Err(error) => return Err(error).context("Failed to enable SQLite WAL mode"),
        }
    }
}

fn init_schema(conn: &mut Connection) -> Result<()> {
    // Reject foreign, incomplete and future stores before issuing any write pragma or DDL.
    let version = {
        let snapshot = conn.transaction()?;
        schema_version(&snapshot)?
    };
    enable_wal(conn)?;
    conn.execute_batch(
        "PRAGMA synchronous=FULL;
         PRAGMA foreign_keys=ON;",
    )
    .context("Failed to configure durable SQLite storage")?;
    if version == Some(STORE_SCHEMA_VERSION) {
        return Ok(());
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("Failed to lock SQLite schema initialization")?;
    // Another process may have initialized or migrated while this connection waited.
    match schema_version(&tx)? {
        None => {
            tx.execute_batch(
                "CREATE TABLE store_metadata (
             key TEXT PRIMARY KEY,
             value INTEGER NOT NULL
         );
         CREATE TABLE blob_store (
             namespace TEXT NOT NULL,
             cache_key TEXT NOT NULL,
             schema_version INTEGER NOT NULL,
             root_path TEXT,
             payload BLOB NOT NULL,
             payload_blake3 BLOB NOT NULL,
             updated_at INTEGER NOT NULL,
             PRIMARY KEY (namespace, cache_key)
         );
         CREATE INDEX idx_blob_ns_root ON blob_store(namespace, root_path);
         CREATE INDEX idx_blob_ns_updated ON blob_store(namespace, updated_at);",
            )
            .context("Failed to initialize mfb_store.sqlite schema")?;
            tx.execute(
                "INSERT INTO store_metadata (key, value) VALUES ('schema_version', ?1)",
                params![STORE_SCHEMA_VERSION],
            )?;
        }
        Some(1) => migrate_v1_crc32_to_blake3(&tx)?,
        Some(_) => {}
    }
    tx.commit()
        .context("Failed to commit SQLite schema transaction")
}

fn store_path() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_STORE_PATH.with(|p| p.borrow().clone()) {
        return Ok(path);
    }
    default_store_path()
}

fn open_connection() -> Result<Connection> {
    let path = store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create SQLite store parent directory {}",
                parent.display()
            )
        })?;
    }
    let mut conn = Connection::open(&path)
        .with_context(|| format!("Failed to open SQLite store at {}", path.display()))?;
    conn.busy_timeout(STORE_BUSY_TIMEOUT)
        .context("Failed to set SQLite busy timeout")?;
    init_schema(&mut conn)?;
    Ok(conn)
}

fn with_conn<R>(f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
    let conn = open_connection()?;
    f(&conn)
}

fn rows_affected_u64(rows: usize) -> Result<u64> {
    crate::numeric_cast::usize_to_u64_strict(rows, "sqlite rows_affected")
        .with_context(|| format!("sqlite DELETE affected {rows} rows, exceeds u64::MAX"))
}

/// Read a blob payload.
///
/// # Errors
/// Returns an error on I/O failure or invalid durable state, preserving the row.
/// `Ok(None)` means absent data or invalid reconstructable path-tree cache.
pub fn blob_get(
    namespace: &str,
    cache_key: &str,
    expected_schema_version: i32,
) -> Result<Option<Vec<u8>>> {
    if namespace == NS_PATH_TREE {
        let mut conn = open_connection()?;
        return path_tree_get(
            &mut conn,
            cache_key,
            expected_schema_version,
            cache_policy(),
            now_unix_secs,
        );
    }
    with_conn(|conn| read_blob(conn, namespace, cache_key, expected_schema_version))
}

fn cache_policy() -> CachePolicy {
    crate::runtime_config::active().map_or_else(CachePolicy::default, |config| config.cache)
}

fn path_tree_get(
    conn: &mut Connection,
    cache_key: &str,
    expected_schema_version: i32,
    policy: CachePolicy,
    clock: impl FnOnce() -> Result<i64>,
) -> Result<Option<Vec<u8>>> {
    policy.validate()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = clock()?;
    let cutoff = now.saturating_sub(i64::try_from(policy.path_tree_ttl_seconds)?);
    // Future timestamps cannot establish freshness after a backwards clock adjustment.
    let expired = tx.execute(
        "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key = ?2
         AND (updated_at <= ?3 OR updated_at > ?4)",
        params![NS_PATH_TREE, cache_key, cutoff, now],
    )?;
    let payload = read_blob(&tx, NS_PATH_TREE, cache_key, expected_schema_version)?;
    if payload.is_some() {
        tx.execute(
            "UPDATE blob_store SET updated_at = ?1 WHERE namespace = ?2 AND cache_key = ?3",
            params![now, NS_PATH_TREE, cache_key],
        )?;
    }
    tx.commit()?;
    if expired > 0 {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_runtime",
            "SQLITE AUDIT: expired path_tree cache entry removed",
        );
    }
    Ok(payload)
}

fn read_blob(
    conn: &Connection,
    namespace: &str,
    cache_key: &str,
    expected_schema_version: i32,
) -> Result<Option<Vec<u8>>> {
    let row = match conn.query_row(
        "SELECT schema_version, payload, payload_blake3 FROM blob_store
             WHERE namespace = ?1 AND cache_key = ?2",
        params![namespace, cache_key],
        |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        },
    ) {
        Ok(v) => v,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let (schema, payload, stored_blake3) = row;
    let expected_blake3 = blake3::hash(&payload);
    let invalid = if schema != expected_schema_version {
        Some("schema mismatch")
    } else if stored_blake3.as_slice() != expected_blake3.as_bytes() {
        Some("BLAKE3 mismatch")
    } else {
        None
    };
    if let Some(reason) = invalid {
        anyhow::ensure!(
            namespace == NS_PATH_TREE,
            "SQLite {namespace} state {reason}; saved record retained, recovery cannot continue"
        );
        // Do not delete a newer value committed after this read.
        conn.execute(
            "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key = ?2
                 AND schema_version = ?3 AND payload = ?4 AND payload_blake3 = ?5",
            params![namespace, cache_key, schema, payload, stored_blake3],
        )?;
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_runtime",
            format!(
                "SQLITE AUDIT: {reason} for reconstructable {namespace} cache (invalid snapshot evicted if unchanged)"
            ),
        );
        return Ok(None);
    }
    Ok(Some(payload))
}

/// Upsert a blob payload with a full BLAKE3 integrity digest.
///
/// # Errors
/// Returns an error if the write fails or a path-tree payload exceeds its cache budget.
pub fn blob_put(
    namespace: &str,
    cache_key: &str,
    schema_version: i32,
    root_path: Option<&Path>,
    payload: &[u8],
) -> Result<()> {
    let root = root_path.map(|p| p.to_string_lossy().into_owned());
    if namespace == NS_PATH_TREE {
        let mut conn = open_connection()?;
        return path_tree_put(
            &mut conn,
            cache_key,
            schema_version,
            root.as_deref(),
            payload,
            cache_policy(),
            now_unix_secs,
        );
    }
    let updated_at = now_unix_secs()?;
    with_conn(|conn| {
        upsert_blob(
            conn,
            namespace,
            cache_key,
            schema_version,
            root.as_deref(),
            payload,
            updated_at,
        )
    })
}

fn path_tree_put(
    conn: &mut Connection,
    cache_key: &str,
    schema_version: i32,
    root: Option<&str>,
    payload: &[u8],
    policy: CachePolicy,
    clock: impl FnOnce() -> Result<i64>,
) -> Result<()> {
    policy.validate()?;
    let max_bytes = i64::try_from(policy.path_tree_max_bytes)?;
    let payload_bytes = i64::try_from(payload.len()).context("path_tree payload exceeds i64")?;
    anyhow::ensure!(
        payload_bytes <= max_bytes,
        "path_tree cache not saved: payload {payload_bytes} bytes exceeds cache.path_tree_max_bytes={max_bytes}; existing cache retained"
    );
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = clock()?;
    let cutoff = now.saturating_sub(i64::try_from(policy.path_tree_ttl_seconds)?);
    let expired = tx.execute(
        "DELETE FROM blob_store WHERE namespace = ?1 AND (updated_at <= ?2 OR updated_at > ?3)",
        params![NS_PATH_TREE, cutoff, now],
    )?;
    upsert_blob(
        &tx,
        NS_PATH_TREE,
        cache_key,
        schema_version,
        root,
        payload,
        now,
    )?;
    // Reserve the admitted payload, then retain the newest prefix using lengths only.
    let evicted = tx.execute(
        "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key IN (
             SELECT cache_key FROM (
                 SELECT cache_key, SUM(length(payload)) OVER (
                     ORDER BY updated_at DESC, cache_key DESC ROWS UNBOUNDED PRECEDING
                 ) AS retained_bytes
                 FROM blob_store WHERE namespace = ?1 AND cache_key != ?2
             ) WHERE retained_bytes > ?3
         )",
        params![NS_PATH_TREE, cache_key, max_bytes - payload_bytes],
    )?;
    tx.commit()?;
    if expired > 0 || evicted > 0 {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_runtime",
            format!(
                "SQLITE AUDIT: path_tree lifecycle removed {expired} expired and {evicted} LRU entries"
            ),
        );
    }
    Ok(())
}

fn upsert_blob(
    conn: &Connection,
    namespace: &str,
    cache_key: &str,
    schema_version: i32,
    root: Option<&str>,
    payload: &[u8],
    updated_at: i64,
) -> Result<()> {
    let digest = blake3::hash(payload);
    conn.execute(
        "INSERT INTO blob_store (namespace, cache_key, schema_version, root_path, payload, \
              payload_blake3, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(namespace, cache_key) DO UPDATE SET
                schema_version = excluded.schema_version,
                root_path = excluded.root_path,
                payload = excluded.payload,
                payload_blake3 = excluded.payload_blake3,
                updated_at = excluded.updated_at",
        params![
            namespace,
            cache_key,
            schema_version,
            root,
            payload,
            digest.as_bytes().as_slice(),
            updated_at
        ],
    )?;
    Ok(())
}

/// Delete blobs under a namespace whose `root_path` equals or is prefixed by
/// `target`.
///
/// # Errors
/// Returns an error if the delete fails.
pub fn blob_delete_under_root(namespace: &str, target: &Path) -> Result<u64> {
    let target_abs = target
        .to_str()
        .context("blob deletion target is not valid UTF-8")?;
    let prefix = format!("{}/", target_abs.trim_end_matches('/'));
    with_conn(|conn| {
        let rows = conn.execute(
            "DELETE FROM blob_store WHERE namespace = ?1 AND (root_path = ?2 OR substr(root_path, 1, length(?3)) = ?3)",
            params![namespace, target_abs, prefix],
        )?;
        rows_affected_u64(rows)
    })
}

/// Delete one blob row.
///
/// # Errors
/// Returns an error if the delete fails.
pub fn blob_delete(namespace: &str, cache_key: &str) -> Result<u64> {
    with_conn(|conn| {
        let rows = conn.execute(
            "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key = ?2",
            params![namespace, cache_key],
        )?;
        rows_affected_u64(rows)
    })
}

/// Delete all blobs in a namespace.
///
/// # Errors
/// Returns an error if the delete fails.
pub fn blob_delete_namespace(namespace: &str) -> Result<u64> {
    with_conn(|conn| {
        let rows = conn.execute(
            "DELETE FROM blob_store WHERE namespace = ?1",
            params![namespace],
        )?;
        rows_affected_u64(rows)
    })
}

/// Expose a dedicated connection for structured tables (e.g. dev
/// `media_index`).
///
/// # Errors
/// Returns an error if the store cannot be opened.
pub fn open_store_connection() -> Result<Connection> {
    open_connection()
}

#[cfg(test)]
#[must_use]
pub struct TestStoreGuard;

#[cfg(test)]
impl Drop for TestStoreGuard {
    fn drop(&mut self) {
        TEST_STORE_PATH.with(|p| {
            *p.borrow_mut() = None;
        });
    }
}

/// Point blob I/O at a temporary store for unit tests.
#[cfg(test)]
pub fn set_test_store_path_for_tests(path: PathBuf) -> TestStoreGuard {
    TEST_STORE_PATH.with(|p| {
        *p.borrow_mut() = Some(path);
    });
    TestStoreGuard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_tree_idle_expiry_covers_boundary_future_clock_and_read_refresh() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let _guard = set_test_store_path_for_tests(dir.path().join(STORE_FILE_NAME));
        let mut conn = open_connection()?;
        let policy = CachePolicy {
            path_tree_max_bytes: 100,
            path_tree_ttl_seconds: 10,
        };
        for (key, timestamp) in [
            ("old", 89),
            ("boundary", 90),
            ("fresh", 91),
            ("future", 101),
        ] {
            upsert_blob(&conn, NS_PATH_TREE, key, 1, None, b"data", timestamp)?;
        }
        for key in ["old", "boundary", "future"] {
            assert_eq!(path_tree_get(&mut conn, key, 1, policy, || Ok(100))?, None);
        }
        for now in [100, 109, 118] {
            assert_eq!(
                path_tree_get(&mut conn, "fresh", 1, policy, || Ok(now))?,
                Some(b"data".to_vec())
            );
        }
        assert_eq!(
            path_tree_get(&mut conn, "fresh", 1, policy, || Ok(128))?,
            None
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM blob_store", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        let longest_ttl = CachePolicy {
            path_tree_ttl_seconds: i64::MAX.unsigned_abs(),
            ..policy
        };
        upsert_blob(&conn, NS_PATH_TREE, "epoch", 1, None, b"data", 0)?;
        assert!(path_tree_get(&mut conn, "epoch", 1, longest_ttl, || Ok(0))?.is_some());
        Ok(())
    }

    #[test]
    fn path_tree_lru_admission_expiry_and_formal_state_are_isolated() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let _guard = set_test_store_path_for_tests(dir.path().join(STORE_FILE_NAME));
        let mut conn = open_connection()?;
        let policy = CachePolicy {
            path_tree_max_bytes: 8,
            path_tree_ttl_seconds: 10,
        };
        for namespace in [NS_CHECKPOINT, NS_PROCESSED, "future_state"] {
            upsert_blob(
                &conn,
                namespace,
                "state",
                7,
                Some("/synthetic"),
                b"durable-state",
                0,
            )?;
        }
        path_tree_put(&mut conn, "first", 1, None, b"1111", policy, || Ok(100))?;
        path_tree_put(&mut conn, "second", 1, None, b"2222", policy, || Ok(101))?;
        assert!(path_tree_get(&mut conn, "first", 1, policy, || Ok(102))?.is_some());
        path_tree_put(&mut conn, "third", 1, None, b"3333", policy, || Ok(103))?;
        assert_eq!(read_blob(&conn, NS_PATH_TREE, "second", 1)?, None);
        assert!(read_blob(&conn, NS_PATH_TREE, "first", 1)?.is_some());
        let error = path_tree_put(&mut conn, "first", 1, None, b"oversized", policy, || {
            Ok(200)
        })
        .unwrap_err();
        assert!(error.to_string().contains("cache not saved"));
        assert_eq!(
            read_blob(&conn, NS_PATH_TREE, "first", 1)?,
            Some(b"1111".to_vec())
        );
        assert!(read_blob(&conn, NS_PATH_TREE, "third", 1)?.is_some());
        path_tree_put(
            &mut conn,
            "exact-budget",
            1,
            None,
            b"12345678",
            policy,
            || Ok(104),
        )?;
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM blob_store WHERE namespace='path_tree'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        upsert_blob(&conn, NS_PATH_TREE, "future", 1, None, b"future", 200)?;
        path_tree_put(&mut conn, "after-expiry", 1, None, b"new", policy, || {
            Ok(114)
        })?;
        assert_eq!(read_blob(&conn, NS_PATH_TREE, "exact-budget", 1)?, None);
        assert_eq!(read_blob(&conn, NS_PATH_TREE, "future", 1)?, None);
        for namespace in [NS_CHECKPOINT, NS_PROCESSED, "future_state"] {
            assert_eq!(
                blob_get(namespace, "state", 7)?,
                Some(b"durable-state".to_vec())
            );
            let (timestamp, root): (i64, String) = conn.query_row(
                "SELECT updated_at, root_path FROM blob_store WHERE namespace=?1",
                params![namespace],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            assert_eq!(timestamp, 0);
            assert_eq!(root, "/synthetic");
        }
        Ok(())
    }

    #[test]
    fn concurrent_path_tree_admission_keeps_one_budget_and_preserves_state() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(STORE_FILE_NAME);
        let _guard = set_test_store_path_for_tests(path.clone());
        blob_put(NS_CHECKPOINT, "retained", 1, None, b"state")?;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || -> Result<()> {
                    let _guard = set_test_store_path_for_tests(path);
                    barrier.wait();
                    let mut conn = open_connection()?;
                    path_tree_put(
                        &mut conn,
                        &index.to_string(),
                        1,
                        None,
                        b"12345678",
                        CachePolicy {
                            path_tree_max_bytes: 32,
                            path_tree_ttl_seconds: 10,
                        },
                        || Ok(100),
                    )
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap()?;
        }
        let conn = open_connection()?;
        let (rows, bytes): (i64, i64) = conn.query_row(
            "SELECT count(*), sum(length(payload)) FROM blob_store WHERE namespace='path_tree'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((rows, bytes), (4, 32));
        assert_eq!(
            blob_get(NS_CHECKPOINT, "retained", 1)?,
            Some(b"state".to_vec())
        );
        Ok(())
    }

    #[test]
    fn path_tree_failed_eviction_or_touch_rolls_back_entire_transaction() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let _guard = set_test_store_path_for_tests(dir.path().join(STORE_FILE_NAME));
        let mut conn = open_connection()?;
        let policy = CachePolicy {
            path_tree_max_bytes: 4,
            path_tree_ttl_seconds: 10,
        };
        upsert_blob(&conn, NS_PATH_TREE, "expired", 1, None, b"old", 0)?;
        upsert_blob(&conn, NS_PATH_TREE, "recent", 1, None, b"good", 100)?;
        conn.execute_batch(
            "CREATE TRIGGER reject_lru BEFORE DELETE ON blob_store
             WHEN OLD.cache_key = 'recent' BEGIN SELECT RAISE(ABORT, 'test eviction failure'); END;",
        )?;
        assert!(path_tree_put(&mut conn, "new", 1, None, b"new", policy, || Ok(101)).is_err());
        assert!(read_blob(&conn, NS_PATH_TREE, "expired", 1)?.is_some());
        assert_eq!(
            read_blob(&conn, NS_PATH_TREE, "recent", 1)?,
            Some(b"good".to_vec())
        );
        assert_eq!(read_blob(&conn, NS_PATH_TREE, "new", 1)?, None);
        conn.execute_batch(
            "DROP TRIGGER reject_lru;
             CREATE TRIGGER reject_touch BEFORE UPDATE ON blob_store
             BEGIN SELECT RAISE(ABORT, 'test touch failure'); END;",
        )?;
        assert!(path_tree_get(&mut conn, "recent", 1, policy, || Ok(101)).is_err());
        assert_eq!(
            conn.query_row(
                "SELECT updated_at FROM blob_store WHERE cache_key='recent'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            100
        );
        conn.execute_batch("DROP TRIGGER reject_touch;")?;
        path_tree_put(&mut conn, "new", 1, None, b"new", policy, || Ok(101))?;
        assert_eq!(read_blob(&conn, NS_PATH_TREE, "expired", 1)?, None);
        assert_eq!(read_blob(&conn, NS_PATH_TREE, "recent", 1)?, None);
        assert_eq!(
            read_blob(&conn, NS_PATH_TREE, "new", 1)?,
            Some(b"new".to_vec())
        );
        Ok(())
    }

    #[test]
    fn blob_put_get_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let store = dir.path().join(STORE_FILE_NAME);
        let _guard = set_test_store_path_for_tests(store);
        blob_put(NS_PATH_TREE, "abc", 1, None, b"payload").expect("put"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let got = blob_get(NS_PATH_TREE, "abc", 1).expect("get"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert_eq!(got.as_deref(), Some(b"payload" as &[u8]));
    }

    #[test]
    fn deletion_under_root_uses_literal_paths_and_retains_other_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = set_test_store_path_for_tests(dir.path().join(STORE_FILE_NAME));
        let target = "/media/图片_100%";
        for (key, root) in [
            ("exact", target),
            ("child", "/media/图片_100%/child"),
            ("wildcard", "/media/图片X1000/child"),
            ("sibling", "/media/图片_100%other"),
        ] {
            blob_put(NS_PATH_TREE, key, 2, Some(Path::new(root)), b"cache").unwrap();
        }
        blob_put(
            NS_CHECKPOINT,
            "resume",
            2,
            Some(Path::new(target)),
            b"state",
        )
        .unwrap();
        assert_eq!(
            blob_delete_under_root(NS_PATH_TREE, Path::new(target)).unwrap(),
            2
        );
        assert!(blob_get(NS_PATH_TREE, "wildcard", 2).unwrap().is_some());
        assert!(blob_get(NS_PATH_TREE, "sibling", 2).unwrap().is_some());
        assert_eq!(
            blob_get(NS_CHECKPOINT, "resume", 2).unwrap().as_deref(),
            Some(&b"state"[..])
        );
    }

    #[test]
    fn v1_crc_store_migrates_payloads_to_blake3() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join(STORE_FILE_NAME);
        seed_v1_store(&store, NS_PATH_TREE);

        let _guard = set_test_store_path_for_tests(store.clone());
        let payload = blob_get(NS_CHECKPOINT, "legacy", 7).expect("migrated read");
        assert_eq!(payload.as_deref(), Some(b"resume-state" as &[u8]));
        assert_eq!(
            blob_get(NS_PATH_TREE, "corrupt", 7).expect("corrupt cache read"),
            None
        );

        let migrated = Connection::open(store).expect("migrated store");
        assert_eq!(
            schema_version(&migrated).unwrap(),
            Some(STORE_SCHEMA_VERSION)
        );
        let columns = migrated
            .prepare("PRAGMA table_info(blob_store)")
            .expect("table info")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("column rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("columns");
        assert!(columns.iter().any(|column| column == "payload_blake3"));
        assert!(!columns.iter().any(|column| column == "payload_crc32"));
    }

    fn seed_v1_store(store: &Path, corrupt_namespace: &str) {
        let legacy = Connection::open(store).expect("legacy store");
        legacy
            .execute_batch(
                "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
                 INSERT INTO store_metadata (key, value) VALUES ('schema_version', 1);
                 CREATE TABLE blob_store (
                     namespace TEXT NOT NULL,
                     cache_key TEXT NOT NULL,
                     schema_version INTEGER NOT NULL,
                     root_path TEXT,
                     payload BLOB NOT NULL,
                     payload_crc32 INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     PRIMARY KEY (namespace, cache_key)
                 );",
            )
            .expect("legacy schema");
        let payload = b"resume-state";
        let payload_crc32 = crc32fast::hash(payload).cast_signed();
        legacy
            .execute(
                "INSERT INTO blob_store VALUES (?1, ?2, ?3, NULL, ?4, ?5, 1)",
                params![NS_CHECKPOINT, "legacy", 7, payload, payload_crc32],
            )
            .expect("legacy row");
        legacy
            .execute(
                "INSERT INTO blob_store VALUES (?1, ?2, ?3, NULL, ?4, ?5, 1)",
                params![
                    corrupt_namespace,
                    "corrupt",
                    7,
                    b"corrupt-state",
                    crc32fast::hash(b"corrupt-state").cast_signed() ^ 1
                ],
            )
            .expect("corrupt legacy row");
    }

    #[test]
    fn migration_rolls_back_entire_store_when_durable_state_is_corrupt() {
        for namespace in [NS_CHECKPOINT, NS_PROCESSED, "future_state"] {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join(STORE_FILE_NAME);
            seed_v1_store(&store, namespace);
            let _guard = set_test_store_path_for_tests(store.clone());
            let error = open_connection().unwrap_err();
            assert!(
                error.to_string().contains("state failed CRC32"),
                "{error:#}"
            );
            let conn = Connection::open(&store).unwrap();
            assert_eq!(schema_version(&conn).unwrap(), Some(1));
            let count: i64 = conn
                .query_row("SELECT count(*) FROM blob_store", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 2, "both valid and corrupt state must remain");
            let payload: Vec<u8> = conn
                .query_row(
                    "SELECT payload FROM blob_store WHERE cache_key = 'corrupt'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(payload, b"corrupt-state");
            // Repairing the synthetic row allows a later atomic migration.
            conn.execute(
                "UPDATE blob_store SET payload_crc32 = ?1 WHERE cache_key = 'corrupt'",
                [crc32fast::hash(&payload).cast_signed()],
            )
            .unwrap();
            assert_eq!(blob_get(namespace, "corrupt", 7).unwrap(), Some(payload));
        }
    }

    #[test]
    fn invalid_state_is_not_a_cache_miss_or_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = set_test_store_path_for_tests(dir.path().join(STORE_FILE_NAME));
        for namespace in [NS_PATH_TREE, NS_CHECKPOINT, NS_PROCESSED, "future_state"] {
            for (key, mutation, reason) in [
                ("schema", "schema_version = 99", "schema mismatch"),
                ("payload", "payload = X'00'", "BLAKE3 mismatch"),
                ("digest", "payload_blake3 = X'00'", "BLAKE3 mismatch"),
            ] {
                blob_put(namespace, key, 1, None, b"state").unwrap();
                let conn = open_connection().unwrap();
                conn.execute(
                    &format!(
                        "UPDATE blob_store SET {mutation} WHERE namespace = ?1 AND cache_key = ?2"
                    ),
                    params![namespace, key],
                )
                .unwrap();
                let result = blob_get(namespace, key, 1);
                let count: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM blob_store WHERE namespace = ?1 AND cache_key = ?2",
                        params![namespace, key],
                        |r| r.get(0),
                    )
                    .unwrap();
                if namespace == NS_PATH_TREE {
                    assert_eq!(result.unwrap(), None);
                    assert_eq!(count, 0);
                } else {
                    assert!(result.unwrap_err().to_string().contains(reason));
                    assert_eq!(count, 1);
                }
            }
            assert_eq!(blob_get(namespace, "absent", 1).unwrap(), None);
        }
    }

    #[test]
    fn unknown_and_incomplete_databases_are_rejected_without_modification() {
        for schema in [
            "CREATE TABLE foreign_data (value TEXT); INSERT INTO foreign_data VALUES ('keep');",
            "CREATE TABLE sqlitex_foreign_data (value TEXT);",
            "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);",
            "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
             INSERT INTO store_metadata VALUES ('schema_version', 999);",
            "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
             INSERT INTO store_metadata VALUES ('schema_version', 2);",
            "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
             INSERT INTO store_metadata VALUES ('schema_version', 1);
             CREATE TABLE blob_store (payload BLOB);",
            "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
             INSERT INTO store_metadata VALUES ('schema_version', 2);
             CREATE TABLE blob_store (namespace TEXT, cache_key TEXT, schema_version INTEGER,
                root_path TEXT, payload BLOB, payload_blake3 BLOB, updated_at INTEGER);",
            "CREATE TABLE store_metadata (key TEXT, value INTEGER NOT NULL);
             INSERT INTO store_metadata VALUES ('schema_version', 2);
             CREATE TABLE blob_store (namespace TEXT, cache_key TEXT, schema_version INTEGER,
                root_path TEXT, payload BLOB, payload_blake3 BLOB, updated_at INTEGER,
                PRIMARY KEY(namespace, cache_key));",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join(STORE_FILE_NAME);
            Connection::open(&store)
                .unwrap()
                .execute_batch(schema)
                .unwrap();
            let before = std::fs::read(&store).unwrap();
            let _guard = set_test_store_path_for_tests(store.clone());
            assert!(open_connection().is_err());
            assert_eq!(std::fs::read(&store).unwrap(), before);
            assert!(!store.with_file_name("mfb_store.sqlite-wal").exists());
        }
    }

    #[test]
    fn reopening_current_store_does_not_rewrite_schema_and_uses_durable_commits() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = set_test_store_path_for_tests(dir.path().join(STORE_FILE_NAME));
        let first = open_connection().unwrap();
        let cookie: i64 = first
            .pragma_query_value(None, "schema_version", |r| r.get(0))
            .unwrap();
        let second = open_connection().unwrap();
        assert_eq!(
            second
                .pragma_query_value(None, "schema_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            cookie
        );
        assert_eq!(
            second
                .pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            second
                .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        assert_eq!(
            second
                .pragma_query_value(None, "foreign_keys", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn wal_lock_timeout_returns_busy_without_changing_saved_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(STORE_FILE_NAME);
        let holder = Connection::open(&store).unwrap();
        holder
            .execute_batch(
                "CREATE TABLE probe(value TEXT); INSERT INTO probe VALUES ('saved');
             BEGIN IMMEDIATE; UPDATE probe SET value = 'pending';",
            )
            .unwrap();
        let waiting = Connection::open(&store).unwrap();
        let error = enable_wal(&waiting).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::DatabaseBusy
        ));
        holder.execute_batch("ROLLBACK;").unwrap();
        enable_wal(&waiting).unwrap();
        let value: String = waiting
            .query_row("SELECT value FROM probe", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "saved");
        assert_eq!(
            waiting
                .pragma_query_value(None, "busy_timeout", |r| r.get::<_, i64>(0))
                .unwrap(),
            5_000
        );
    }

    #[test]
    fn concurrent_openers_initialize_or_migrate_once_without_losing_rows() {
        for legacy in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join(STORE_FILE_NAME);
            if legacy {
                seed_v1_store(&store, NS_PATH_TREE);
            }
            let barrier = std::sync::Barrier::new(4);
            std::thread::scope(|scope| {
                for index in 0..4 {
                    let store = &store;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let _guard = set_test_store_path_for_tests(store.clone());
                        barrier.wait();
                        blob_put(NS_CHECKPOINT, &format!("worker-{index}"), 1, None, b"saved")
                            .unwrap();
                    });
                }
            });
            let _guard = set_test_store_path_for_tests(store);
            for index in 0..4 {
                assert_eq!(
                    blob_get(NS_CHECKPOINT, &format!("worker-{index}"), 1)
                        .unwrap()
                        .as_deref(),
                    Some(&b"saved"[..])
                );
            }
            if legacy {
                assert_eq!(
                    blob_get(NS_CHECKPOINT, "legacy", 7).unwrap().as_deref(),
                    Some(&b"resume-state"[..])
                );
            }
        }
    }
}
