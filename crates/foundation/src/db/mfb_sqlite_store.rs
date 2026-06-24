//! Unified local `SQLite` blob store (`rusqlite` 0.40) — offline cache SSOT
//! (M214).

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

pub const STORE_SCHEMA_VERSION: i32 = 1;
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

fn init_schema(conn: &Connection) -> Result<()> {
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
             payload_crc32 INTEGER NOT NULL,
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
    let conn = Connection::open(&path)
        .with_context(|| format!("Failed to open SQLite store at {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .context("Failed to set SQLite busy timeout")?;
    init_schema(&conn)?;
    Ok(conn)
}

fn with_conn<R>(f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
    let conn = open_connection()?;
    f(&conn)
}

fn crc32_bytes(data: &[u8]) -> i32 {
    // Store CRC32 bit pattern in SQLite INTEGER (signed i32 column).
    crc32fast::hash(data).cast_signed()
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
            "SELECT schema_version, payload, payload_crc32 FROM blob_store
             WHERE namespace = ?1 AND cache_key = ?2",
            params![namespace, cache_key],
            |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i32>(2)?,
                ))
            },
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let (schema, payload, stored_crc) = row;
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
        let expected_crc = crc32_bytes(&payload);
        if expected_crc != stored_crc {
            conn.execute(
                "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key = ?2",
                params![namespace, cache_key],
            )?;
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_runtime",
                format!("SQLITE AUDIT: CRC mismatch for {namespace}/{cache_key} (row deleted)"),
            );
            return Ok(None);
        }
        Ok(Some(payload))
    })
}

/// Upsert a blob payload with CRC32 integrity tag.
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
    let crc = crc32_bytes(payload);
    let updated_at = now_unix_secs()?;
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO blob_store (namespace, cache_key, schema_version, root_path, payload, \
             payload_crc32, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(namespace, cache_key) DO UPDATE SET
                schema_version = excluded.schema_version,
                root_path = excluded.root_path,
                payload = excluded.payload,
                payload_crc32 = excluded.payload_crc32,
                updated_at = excluded.updated_at",
            params![
                namespace,
                cache_key,
                schema_version,
                root.as_deref(),
                payload,
                crc,
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
}
