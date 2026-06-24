//! Path-tree scan snapshots — `PostgreSQL` primary + `SQLite` offline replica
//! (M213/M214).
//!
//! Table DDL must match `crates/dev/src/config/sql/analysis_cache_pg.sql`
//! (`path_tree_snapshots`).

use anyhow::{Context, Result};
use postgres::Client;
use postgres::types::Json;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::Path;

use crate::mfb_sqlite_store::{self, NS_PATH_TREE};

/// Path-tree snapshot schema version (bump to invalidate all tiers).
pub const PATH_TREE_SCHEMA_VERSION: u32 = 2;

const PATH_TREE_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS path_tree_snapshots (
    cache_key TEXT PRIMARY KEY,
    media_kind TEXT NOT NULL,
    root_path TEXT NOT NULL,
    schema_version INT NOT NULL,
    payload JSONB NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_path_tree_root ON path_tree_snapshots(root_path);
CREATE INDEX IF NOT EXISTS idx_path_tree_media_kind ON path_tree_snapshots(media_kind);
";

fn now_unix_secs() -> Result<i64> {
    let secs = crate::media_conversion_gate::unix_epoch_secs_optional()
        .context("path_tree updated_at: system clock before UNIX epoch")?;
    i64::try_from(secs)
        .with_context(|| "path_tree updated_at: epoch seconds exceeded i64".to_string())
}

fn schema_i32(schema_version: u32) -> Result<i32> {
    i32::try_from(schema_version)
        .with_context(|| format!("path_tree schema_version {schema_version} exceeds i32"))
}

fn open_pg_client() -> Result<Client> {
    crate::database::open_pg_client()
}

fn ensure_path_tree_table(client: &mut Client) -> Result<()> {
    client
        .batch_execute(PATH_TREE_TABLE_SQL)
        .context("Failed to ensure path_tree_snapshots table")
}

fn decode_snapshot<T: DeserializeOwned>(bytes: &[u8], cache_key: &str) -> Result<T> {
    serde_json::from_slice(bytes)
        .with_context(|| format!("path_tree snapshot JSON decode failed for cache_key={cache_key}"))
}

fn load_sqlite_snapshot<T: DeserializeOwned>(
    cache_key: &str,
    expected_schema_version: u32,
) -> Result<Option<T>> {
    let schema = schema_i32(expected_schema_version)?;
    let Some(bytes) = mfb_sqlite_store::blob_get(NS_PATH_TREE, cache_key, schema)? else {
        return Ok(None);
    };
    decode_snapshot(&bytes, cache_key).map(Some)
}

fn save_sqlite_snapshot<T: Serialize>(
    cache_key: &str,
    root_path: &Path,
    schema_version: u32,
    snapshot: &T,
) -> Result<()> {
    let bytes = serde_json::to_vec(snapshot).context("path_tree snapshot serialize")?;
    mfb_sqlite_store::blob_put(
        NS_PATH_TREE,
        cache_key,
        schema_i32(schema_version)?,
        Some(root_path),
        &bytes,
    )
}

fn load_pg_snapshot<T: DeserializeOwned>(
    cache_key: &str,
    expected_schema_version: u32,
) -> Result<Option<T>> {
    let mut client = open_pg_client()?;
    ensure_path_tree_table(&mut client)?;
    let Some(row) = client.query_opt(
        "SELECT payload, schema_version FROM path_tree_snapshots WHERE cache_key = $1",
        &[&cache_key],
    )?
    else {
        return Ok(None);
    };
    let schema: i32 = row.get(1);
    if u32::try_from(schema)? != expected_schema_version {
        client.execute(
            "DELETE FROM path_tree_snapshots WHERE cache_key = $1",
            &[&cache_key],
        )?;
        crate::media_conversion_gate::delivery_pipeline_batch_audit(
            "delivery_pipeline_batch",
            format!(
                "CACHE AUDIT: path_tree schema mismatch for key '{cache_key}' (PG row deleted)"
            ),
        );
        return Ok(None);
    }
    let payload: Json<serde_json::Value> = row.get(0);
    let value = serde_json::from_value(payload.0)
        .with_context(|| format!("path_tree JSONB decode failed for cache_key={cache_key}"))?;
    Ok(Some(value))
}

fn save_pg_snapshot<T: Serialize>(
    cache_key: &str,
    media_kind: &str,
    root_path: &Path,
    schema_version: u32,
    snapshot: &T,
) -> Result<()> {
    let payload_value = serde_json::to_value(snapshot).context("path_tree snapshot serialize")?;
    let schema_i32 = schema_i32(schema_version)?;
    let root = root_path.to_string_lossy().into_owned();
    let updated_at = now_unix_secs()?;
    let mut client = open_pg_client()?;
    ensure_path_tree_table(&mut client)?;
    client.execute(
        "INSERT INTO path_tree_snapshots (cache_key, media_kind, root_path, schema_version, \
         payload, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (cache_key) DO UPDATE SET
            media_kind = EXCLUDED.media_kind,
            root_path = EXCLUDED.root_path,
            schema_version = EXCLUDED.schema_version,
            payload = EXCLUDED.payload,
            updated_at = EXCLUDED.updated_at",
        &[
            &cache_key,
            &media_kind,
            &root,
            &schema_i32,
            &Json(payload_value),
            &updated_at,
        ],
    )?;
    Ok(())
}

/// Stable cache key for a path-tree configuration.
#[must_use]
pub fn path_tree_cache_key(
    dir: &Path,
    extensions: &[&str],
    recursive: bool,
    media_kind: &str,
) -> String {
    let canonical_dir = crate::media_conversion_gate::canonicalize_for_tool_input(dir);
    let mut input = canonical_dir.to_string_lossy().into_owned();
    input.push('|');
    input.push_str(media_kind);
    input.push('|');
    input.push_str(if recursive { "recursive" } else { "flat" });
    let mut exts: Vec<String> = extensions.iter().map(|e| e.to_ascii_lowercase()).collect();
    exts.sort_unstable();
    exts.dedup();
    input.push('|');
    input.push_str(&exts.join(","));
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

/// Load a path-tree snapshot (`PostgreSQL`, then `SQLite` replica).
///
/// # Errors
/// Propagates database errors. `Ok(None)` is a cache miss only.
pub fn load_path_tree_snapshot<T: DeserializeOwned>(
    cache_key: &str,
    expected_schema_version: u32,
) -> Result<Option<T>> {
    match load_pg_snapshot(cache_key, expected_schema_version) {
        Ok(Some(snapshot)) => Ok(Some(snapshot)),
        Ok(None) => load_sqlite_snapshot(cache_key, expected_schema_version),
        Err(pg_err) => {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "delivery_pipeline_batch",
                format!("path_tree PG load failed ({pg_err}); trying SQLite replica"),
            );
            load_sqlite_snapshot(cache_key, expected_schema_version).map_err(|sqlite_err| {
                pg_err.context(format!(
                    "path_tree SQLite replica load failed: {sqlite_err}"
                ))
            })
        }
    }
}

/// Persist a path-tree snapshot to `PostgreSQL` (required) and `SQLite`
/// replica.
///
/// # Errors
/// Returns an error when the `PostgreSQL` write fails.
pub fn save_path_tree_snapshot<T: Serialize>(
    cache_key: &str,
    media_kind: &str,
    root_path: &Path,
    schema_version: u32,
    snapshot: &T,
) -> Result<()> {
    save_pg_snapshot(cache_key, media_kind, root_path, schema_version, snapshot)?;
    save_sqlite_snapshot(cache_key, root_path, schema_version, snapshot)
}

/// Delete snapshots whose `root_path` equals or is under `target`.
///
/// # Errors
/// Returns an error if either backend purge fails.
pub fn purge_path_tree_under(target: &Path) -> Result<u64> {
    let mut deleted = 0u64;
    let mut client = open_pg_client()?;
    ensure_path_tree_table(&mut client)?;
    let target_abs = target.to_string_lossy().into_owned();
    let pattern = format!("{}/%", target_abs.trim_end_matches('/'));
    let pg_rows = client.execute(
        "DELETE FROM path_tree_snapshots WHERE root_path = $1 OR root_path LIKE $2",
        &[&target_abs, &pattern],
    )?;
    deleted = deleted.saturating_add(pg_rows);
    deleted = deleted.saturating_add(mfb_sqlite_store::blob_delete_under_root(
        NS_PATH_TREE,
        target,
    )?);
    Ok(deleted)
}

/// Remove all path-tree snapshots.
///
/// # Errors
/// Returns an error if either backend purge fails.
pub fn purge_all_path_tree_snapshots() -> Result<u64> {
    let mut deleted = 0u64;
    let mut client = open_pg_client()?;
    ensure_path_tree_table(&mut client)?;
    let pg_rows = client.execute("DELETE FROM path_tree_snapshots", &[])?;
    deleted = deleted.saturating_add(pg_rows);
    deleted = deleted.saturating_add(mfb_sqlite_store::blob_delete_namespace(NS_PATH_TREE)?);
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_tree_cache_key_is_stable() {
        let a = path_tree_cache_key(Path::new("/tmp/a"), &["png", "jpg"], true, "image");
        let b = path_tree_cache_key(Path::new("/tmp/a"), &["jpg", "png"], true, "image");
        assert_eq!(a, b);
        let c = path_tree_cache_key(Path::new("/tmp/a"), &["png"], false, "image");
        assert_ne!(a, c);
    }
}
