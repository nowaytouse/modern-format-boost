//! Reconstructable path-tree scan snapshots in the shared local SQLite store.
//! Advanced analysis databases are not dependencies of directory discovery.

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::Path;

use crate::mfb_sqlite_store::{self, NS_PATH_TREE};

/// Path-tree snapshot schema version (bump when snapshot semantics change).
pub const PATH_TREE_SCHEMA_VERSION: u32 = 3;

fn schema_i32(schema_version: u32) -> Result<i32> {
    i32::try_from(schema_version)
        .with_context(|| format!("path_tree schema_version {schema_version} exceeds i32"))
}

fn decode_snapshot<T: DeserializeOwned>(bytes: &[u8], cache_key: &str) -> Result<T> {
    serde_json::from_slice(bytes)
        .with_context(|| format!("path_tree snapshot JSON decode failed for cache_key={cache_key}"))
}

/// Load a local snapshot within the configured idle TTL, refreshing its last-use time.
/// Callers must still validate filesystem freshness.
///
/// # Errors
/// Propagates database and deserialization errors; `Ok(None)` is a cache miss.
pub fn load_path_tree_snapshot<T: DeserializeOwned>(
    cache_key: &str,
    expected_schema_version: u32,
) -> Result<Option<T>> {
    let schema = schema_i32(expected_schema_version)?;
    let Some(bytes) = mfb_sqlite_store::blob_get(NS_PATH_TREE, cache_key, schema)? else {
        return Ok(None);
    };
    decode_snapshot(&bytes, cache_key).map(Some)
}

/// Persist a snapshot locally, without an advanced database service.
///
/// # Errors
/// Returns serialization, path, capacity or database errors without reporting a successful save.
/// An oversized snapshot retains existing cache entries and reports an explicit not-saved error.
pub fn save_path_tree_snapshot<T: Serialize>(
    cache_key: &str,
    root_path: &Path,
    schema_version: u32,
    snapshot: &T,
) -> Result<()> {
    let schema = schema_i32(schema_version)?;
    let root = crate::media_conversion_gate::canonicalize_for_tool_input(root_path);
    anyhow::ensure!(root.to_str().is_some(), "path_tree root is not valid UTF-8");
    let bytes = serde_json::to_vec(snapshot).context("path_tree snapshot serialize")?;
    mfb_sqlite_store::blob_put(NS_PATH_TREE, cache_key, schema, Some(&root), &bytes)
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
    let mut identity = blake3::Hasher::new_derive_key("MFB path-tree cache identity v3");
    // Fixed-width field digests avoid delimiter ambiguity and preserve native path bytes.
    identity.update(blake3::hash(canonical_dir.as_os_str().as_encoded_bytes()).as_bytes());
    identity.update(blake3::hash(media_kind.as_bytes()).as_bytes());
    identity.update(&[u8::from(recursive)]);
    identity.update(&PATH_TREE_SCHEMA_VERSION.to_le_bytes());
    let mut exts: Vec<String> = extensions.iter().map(|e| e.to_ascii_lowercase()).collect();
    exts.sort_unstable();
    exts.dedup();
    for extension in exts {
        identity.update(blake3::hash(extension.as_bytes()).as_bytes());
    }
    identity.finalize().to_hex().to_string()
}

/// Delete local snapshots whose `root_path` equals or is under `target`.
///
/// # Errors
/// Returns invalid path encoding or database deletion errors.
pub fn purge_path_tree_under(target: &Path) -> Result<u64> {
    let target = crate::media_conversion_gate::canonicalize_for_tool_input(target);
    mfb_sqlite_store::blob_delete_under_root(NS_PATH_TREE, &target)
}

/// Remove all local snapshots, retaining durable and unknown namespaces.
///
/// # Errors
/// Returns an error if deletion fails.
pub fn purge_all_path_tree_snapshots() -> Result<u64> {
    mfb_sqlite_store::blob_delete_namespace(NS_PATH_TREE)
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

    #[test]
    fn path_tree_cache_identity_distinguishes_options_and_ambiguous_fields() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let key = path_tree_cache_key(path, &["PNG", "jpg", "png"], true, "image");
        assert_eq!(
            key,
            path_tree_cache_key(path, &["jpg", "png"], true, "image")
        );
        assert_eq!(
            key,
            path_tree_cache_key(&path.join("."), &["png", "jpg"], true, "image")
        );
        for other in [
            path_tree_cache_key(path, &["png", "jpg"], false, "image"),
            path_tree_cache_key(path, &["png", "jpg"], true, "video"),
            path_tree_cache_key(path, &["png"], true, "image"),
            path_tree_cache_key(&path.join("child"), &["png", "jpg"], true, "image"),
        ] {
            assert_ne!(key, other);
        }
        assert_ne!(
            path_tree_cache_key(path, &["a,b"], true, "image"),
            path_tree_cache_key(path, &["a", "b"], true, "image")
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_identity_does_not_collapse_non_utf8_names() {
        use std::os::unix::ffi::OsStrExt;
        let first = Path::new(std::ffi::OsStr::from_bytes(b"/missing/\xff"));
        let second = Path::new(std::ffi::OsStr::from_bytes(b"/missing/\xfe"));
        assert_eq!(first.to_string_lossy(), second.to_string_lossy());
        assert_ne!(
            path_tree_cache_key(first, &["png"], true, "image"),
            path_tree_cache_key(second, &["png"], true, "image")
        );
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store.sqlite");
        let _guard = mfb_sqlite_store::set_test_store_path_for_tests(store.clone());
        assert!(save_path_tree_snapshot("invalid", first, 2, &vec!["value"]).is_err());
        assert!(
            !store.exists(),
            "invalid root must not create or mutate storage"
        );
    }

    #[test]
    fn local_roundtrip_and_cleanup_retain_formal_state() {
        let root = tempfile::tempdir().unwrap();
        let _guard =
            mfb_sqlite_store::set_test_store_path_for_tests(root.path().join("store.sqlite"));
        let source = root.path().join("source_100%");
        let child = source.join("child");
        let sibling = root.path().join("source_100%other");
        for dir in [&source, &child, &sibling] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for (key, dir) in [("root", &source), ("child", &child), ("sibling", &sibling)] {
            save_path_tree_snapshot(key, dir, PATH_TREE_SCHEMA_VERSION, &vec![key]).unwrap();
            assert_eq!(
                load_path_tree_snapshot::<Vec<String>>(key, PATH_TREE_SCHEMA_VERSION).unwrap(),
                Some(vec![key.to_owned()])
            );
        }
        for namespace in [
            mfb_sqlite_store::NS_CHECKPOINT,
            mfb_sqlite_store::NS_PROCESSED,
            "future_state",
        ] {
            mfb_sqlite_store::blob_put(namespace, "state", 2, Some(&source), b"receipt").unwrap();
        }
        assert_eq!(purge_path_tree_under(&source.join(".")).unwrap(), 2);
        assert!(
            load_path_tree_snapshot::<Vec<String>>("sibling", PATH_TREE_SCHEMA_VERSION)
                .unwrap()
                .is_some()
        );
        assert_eq!(purge_all_path_tree_snapshots().unwrap(), 1);
        assert_eq!(purge_all_path_tree_snapshots().unwrap(), 0);
        for namespace in [
            mfb_sqlite_store::NS_CHECKPOINT,
            mfb_sqlite_store::NS_PROCESSED,
            "future_state",
        ] {
            assert_eq!(
                mfb_sqlite_store::blob_get(namespace, "state", 2).unwrap(),
                Some(b"receipt".to_vec())
            );
        }
    }

    #[test]
    fn invalid_snapshot_and_write_failure_are_explicit_and_retryable() {
        let root = tempfile::tempdir().unwrap();
        let _guard =
            mfb_sqlite_store::set_test_store_path_for_tests(root.path().join("store.sqlite"));
        mfb_sqlite_store::blob_put(NS_PATH_TREE, "invalid", 2, None, b"not-json").unwrap();
        assert!(load_path_tree_snapshot::<Vec<String>>("invalid", 2).is_err());
        assert_eq!(
            load_path_tree_snapshot::<Vec<String>>("missing", 2).unwrap(),
            None
        );
        assert!(load_path_tree_snapshot::<Vec<String>>("invalid", u32::MAX).is_err());
        let conn = mfb_sqlite_store::open_store_connection().unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_cache BEFORE INSERT ON blob_store
            WHEN NEW.namespace = 'path_tree' BEGIN SELECT RAISE(ABORT, 'test write failure'); END;",
        )
        .unwrap();
        assert!(save_path_tree_snapshot("new", root.path(), 2, &vec!["new"]).is_err());
        assert_eq!(
            load_path_tree_snapshot::<Vec<String>>("new", 2).unwrap(),
            None
        );
        conn.execute_batch("DROP TRIGGER reject_cache;").unwrap();
        save_path_tree_snapshot("new", root.path(), 2, &vec!["new"]).unwrap();
        assert_eq!(
            load_path_tree_snapshot::<Vec<String>>("new", 2).unwrap(),
            Some(vec!["new".to_owned()])
        );
        assert!(
            load_path_tree_snapshot::<Vec<String>>("new", 1)
                .unwrap()
                .is_none()
        );
    }
}
