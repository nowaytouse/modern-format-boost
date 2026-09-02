//! Unified local `SQLite` blob store (`rusqlite` 0.40) — offline cache SSOT
//! (M214).

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

pub const STORE_SCHEMA_VERSION: i32 = 2;
pub const NS_PATH_TREE: &str = "path_tree";
pub const NS_CHECKPOINT: &str = "checkpoint";
pub const NS_PROCESSED: &str = "processed";

const STORE_FILE_NAME: &str = "mfb_store.sqlite";

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

fn migrate_v1_crc32_to_blake3(conn: &mut Connection) -> Result<()> {
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

    let tx = conn
        .transaction()
        .context("Failed to begin SQLite BLAKE3 migration")?;
    let rows = {
        let mut statement = tx
            .prepare(
                "SELECT namespace, cache_key, schema_version, root_path, payload, payload_crc32, \
                 updated_at \
                 FROM blob_store",
            )
            .context("Failed to read legacy SQLite blob rows")?;
        statement
            .query_map([], |row| {
                Ok(LegacyBlob {
                    namespace: row.get(0)?,
                    cache_key: row.get(1)?,
                    schema_version: row.get(2)?,
                    root_path: row.get(3)?,
                    payload: row.get(4)?,
                    payload_crc32: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

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

    let mut migrated_rows = 0usize;
    let mut rejected_rows = 0usize;
    for row in rows {
        if crc32fast::hash(&row.payload).cast_signed() != row.payload_crc32 {
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
    tx.commit()
        .context("Failed to commit BLAKE3 SQLite blob migration")?;
    crate::media_conversion_gate::delivery_runtime_batch_audit(
        "delivery_runtime",
        format!(
            "SQLITE AUDIT: migrated {migrated_rows} valid blob integrity tags from CRC32 to full \
             BLAKE3; rejected {rejected_rows} CRC-mismatched rows"
        ),
    );
    Ok(())
}

fn init_schema(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS store_metadata (
             key TEXT PRIMARY KEY,
             value INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS blob_store (
             namespace TEXT NOT NULL,
             cache_key TEXT NOT NULL,
             schema_version INTEGER NOT NULL,
             root_path TEXT,
             payload BLOB NOT NULL,
             payload_blake3 BLOB NOT NULL,
             updated_at INTEGER NOT NULL,
             PRIMARY KEY (namespace, cache_key)
         );
         CREATE INDEX IF NOT EXISTS idx_blob_ns_root ON blob_store(namespace, root_path);
         CREATE INDEX IF NOT EXISTS idx_blob_ns_updated ON blob_store(namespace, updated_at);",
    )
    .context("Failed to initialize mfb_store.sqlite schema")?;

    let current: Option<i32> = conn
        .query_row(
            "SELECT value FROM store_metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to read mfb_store schema_version")?;
    match current {
        None => {
            conn.execute(
                "INSERT INTO store_metadata (key, value) VALUES ('schema_version', ?1)",
                params![STORE_SCHEMA_VERSION],
            )?;
        }
        Some(1) => migrate_v1_crc32_to_blake3(conn)?,
        Some(v) if v != STORE_SCHEMA_VERSION => {
            anyhow::bail!(
                "mfb_store.sqlite schema version mismatch (db={v}, \
                 expected={STORE_SCHEMA_VERSION}); remove the store file or bump \
                 STORE_SCHEMA_VERSION"
            );
        }
        Some(_) => {}
    }
    Ok(())
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
    conn.busy_timeout(std::time::Duration::from_secs(5))
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
/// Returns an error on store I/O failure. `Ok(None)` means no row (cache miss).
pub fn blob_get(
    namespace: &str,
    cache_key: &str,
    expected_schema_version: i32,
) -> Result<Option<Vec<u8>>> {
    with_conn(|conn| {
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
        if schema != expected_schema_version {
            conn.execute(
                "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key = ?2",
                params![namespace, cache_key],
            )?;
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_runtime",
                format!("SQLITE AUDIT: schema mismatch for {namespace}/{cache_key} (row deleted)"),
            );
            return Ok(None);
        }
        let expected_blake3 = blake3::hash(&payload);
        if stored_blake3.as_slice() != expected_blake3.as_bytes() {
            conn.execute(
                "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key = ?2",
                params![namespace, cache_key],
            )?;
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_runtime",
                format!("SQLITE AUDIT: BLAKE3 mismatch for {namespace}/{cache_key} (row deleted)"),
            );
            return Ok(None);
        }
        Ok(Some(payload))
    })
}

/// Upsert a blob payload with a full BLAKE3 integrity digest.
///
/// # Errors
/// Returns an error if the write fails.
pub fn blob_put(
    namespace: &str,
    cache_key: &str,
    schema_version: i32,
    root_path: Option<&Path>,
    payload: &[u8],
) -> Result<()> {
    let root = root_path.map(|p| p.to_string_lossy().into_owned());
    let digest = blake3::hash(payload);
    let updated_at = now_unix_secs()?;
    with_conn(|conn| {
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
                root.as_deref(),
                payload,
                digest.as_bytes().as_slice(),
                updated_at
            ],
        )?;
        Ok(())
    })
}

/// Delete blobs under a namespace whose `root_path` equals or is prefixed by
/// `target`.
///
/// # Errors
/// Returns an error if the delete fails.
pub fn blob_delete_under_root(namespace: &str, target: &Path) -> Result<u64> {
    let target_abs = target.to_string_lossy().into_owned();
    let pattern = format!("{}/%", target_abs.trim_end_matches('/'));
    with_conn(|conn| {
        let rows = conn.execute(
            "DELETE FROM blob_store WHERE namespace = ?1 AND (root_path = ?2 OR root_path LIKE ?3)",
            params![namespace, target_abs, pattern],
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
    fn blob_put_get_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let store = dir.path().join(STORE_FILE_NAME);
        let _guard = set_test_store_path_for_tests(store);
        blob_put(NS_PATH_TREE, "abc", 1, None, b"payload").expect("put"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let got = blob_get(NS_PATH_TREE, "abc", 1).expect("get"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert_eq!(got.as_deref(), Some(b"payload" as &[u8]));
    }

    #[test]
    fn v1_crc_store_migrates_payloads_to_blake3() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join(STORE_FILE_NAME);
        let legacy = Connection::open(&store).expect("legacy store");
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
                    NS_CHECKPOINT,
                    "corrupt",
                    7,
                    b"corrupt-state",
                    crc32fast::hash(b"corrupt-state").cast_signed() ^ 1
                ],
            )
            .expect("corrupt legacy row");
        drop(legacy);

        let _guard = set_test_store_path_for_tests(store.clone());
        let payload = blob_get(NS_CHECKPOINT, "legacy", 7).expect("migrated read");
        assert_eq!(payload.as_deref(), Some(b"resume-state" as &[u8]));
        assert_eq!(
            blob_get(NS_CHECKPOINT, "corrupt", 7).expect("corrupt row read"),
            None
        );

        let migrated = Connection::open(store).expect("migrated store");
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
}
