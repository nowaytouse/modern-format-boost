use rusqlite::{Connection, params};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cache_cleaner"))
        .args(arguments)
        .env("MFB_HOME_ROOT", root)
        .env("HOME", root)
        .env("MFB_PG_CONNSTR", "host='unterminated")
        .output()
        .unwrap()
}

#[test]
fn cli_inspection_and_confirmed_cleanup_share_exact_state_safe_json_contract() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let store = cache.join("mfb_store.sqlite");
    let conn = Connection::open(&store).unwrap();
    conn.execute_batch(
        "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
         INSERT INTO store_metadata VALUES ('schema_version', 2);
         CREATE TABLE blob_store (namespace TEXT NOT NULL, cache_key TEXT NOT NULL,
             schema_version INTEGER NOT NULL, root_path TEXT, payload BLOB NOT NULL,
             payload_blake3 BLOB NOT NULL, updated_at INTEGER NOT NULL,
             PRIMARY KEY(namespace, cache_key));",
    )
    .unwrap();
    for namespace in ["path_tree", "checkpoint", "processed"] {
        conn.execute(
            "INSERT INTO blob_store VALUES (?1, 'key', 2, '/synthetic', ?2, ?3, 123)",
            params![namespace, b"payload", &[7u8; 32]],
        )
        .unwrap();
    }
    drop(conn);
    let before = fs::read(&store).unwrap();
    let inspect = run(root.path(), &["--stats", "--json"]);
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let statistics: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(statistics["schema_version"], 1);
    assert!(statistics.get("integrity").is_none());
    assert_eq!(statistics["namespaces"].as_array().unwrap().len(), 3);
    assert_eq!(fs::read(&store).unwrap(), before);

    let unattended = run(root.path(), &["--json"]);
    assert!(!unattended.status.success());
    assert!(String::from_utf8_lossy(&unattended.stderr).contains("requires --yes"));
    assert_eq!(fs::read(&store).unwrap(), before);
    assert!(!run(root.path(), &["--postgres", "--yes"]).status.success());
    assert_eq!(fs::read(&store).unwrap(), before);

    let cleanup = run(root.path(), &["--yes", "--json"]);
    assert!(
        cleanup.status.success(),
        "{}",
        String::from_utf8_lossy(&cleanup.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&cleanup.stdout).unwrap();
    assert_eq!(result["removed_rows"], 1);
    assert_eq!(result["removed_files"], 0);
    let remaining = result["namespaces"].as_array().unwrap();
    assert_eq!(remaining.len(), 2);
    assert!(
        remaining
            .iter()
            .all(|row| row["rebuildable"] == false && row["rows"] == 1)
    );
    let conn = Connection::open(&store).unwrap();
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM blob_store WHERE payload = ?1 AND payload_blake3 = ?2 AND updated_at = 123",
        params![b"payload", &[7u8; 32]], |row| row.get(0)).unwrap();
    assert_eq!(count, 2);
    conn.execute("UPDATE store_metadata SET value = 99", [])
        .unwrap();
    drop(conn);
    let unsupported = fs::read(&store).unwrap();
    assert!(!run(root.path(), &["--yes", "--json"]).status.success());
    assert_eq!(fs::read(&store).unwrap(), unsupported);
}

#[test]
fn cache_root_preserves_the_shared_runtime_path_including_trailing_spaces() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("state with spaces ");
    fs::create_dir(&root).unwrap();
    let output = run(&root, &["--stats", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let statistics: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        statistics["cache_directory"],
        root.join("cache").to_str().unwrap()
    );
    assert_eq!(statistics["store_bytes"], 0);
}

fn health_fixture(root: &Path) -> Connection {
    fs::create_dir(root.join("cache")).unwrap();
    let conn = Connection::open(root.join("cache/mfb_store.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
         INSERT INTO store_metadata VALUES ('schema_version', 2);
         CREATE TABLE blob_store (namespace TEXT NOT NULL, cache_key TEXT NOT NULL,
             schema_version INTEGER NOT NULL, root_path TEXT, payload BLOB NOT NULL,
             payload_blake3 BLOB NOT NULL, updated_at INTEGER NOT NULL,
             PRIMARY KEY(namespace, cache_key));",
    )
    .unwrap();
    for namespace in ["path_tree", "checkpoint", "processed", "future_state"] {
        conn.execute(
            "INSERT INTO blob_store VALUES (?1, 'key', 2, '/synthetic', ?2, ?3, 123)",
            params![
                namespace,
                b"private test payload",
                blake3::hash(b"private test payload").as_bytes()
            ],
        )
        .unwrap();
    }
    conn
}

fn health(root: &Path) -> (Output, serde_json::Value) {
    let output = run(root, &["--stats", "--check-integrity", "--json"]);
    let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid health JSON: {error}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output, report)
}

#[test]
fn integrity_cli_checks_cache_and_protected_state_without_changing_database() {
    let root = tempfile::tempdir().unwrap();
    drop(health_fixture(root.path()));
    let store = root.path().join("cache/mfb_store.sqlite");
    let before = fs::read(&store).unwrap();
    let (output, report) = health(root.path());
    assert!(output.status.success());
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["integrity"]["status"], "healthy");
    assert_eq!(report["integrity"]["sqlite_integrity"], "ok");
    assert_eq!(report["integrity"]["foreign_keys"], "ok");
    assert_eq!(report["integrity"]["checked_cache_rows"], 1);
    assert_eq!(report["integrity"]["checked_protected_rows"], 3);
    assert_eq!(report["integrity"]["blob_scan_complete"], true);
    assert_eq!(fs::read(&store).unwrap(), before);
    let conn = Connection::open(&store).unwrap();
    conn.execute("UPDATE blob_store SET payload_blake3 = zeroblob(32) WHERE namespace IN ('path_tree', 'checkpoint')", []).unwrap();
    drop(conn);
    let before = fs::read(&store).unwrap();
    let (output, report) = health(root.path());
    assert!(!output.status.success());
    assert_eq!(report["integrity"]["status"], "unhealthy");
    assert_eq!(report["integrity"]["corrupt_cache_rows"], 1);
    assert_eq!(report["integrity"]["corrupt_protected_rows"], 1);
    assert_eq!(report["integrity"]["issue_count"], 2);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private test payload"));
    let text = run(root.path(), &["--stats", "--check-integrity"]);
    assert!(!text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("Integrity: unhealthy"));
    assert_eq!(fs::read(&store).unwrap(), before);
}

#[test]
fn integrity_cli_distinguishes_absent_unknown_and_unreadable_stores() {
    let root = tempfile::tempdir().unwrap();
    let (output, report) = health(root.path());
    assert!(output.status.success());
    assert_eq!(report["integrity"]["status"], "absent");
    assert_eq!(report["integrity"]["sqlite_integrity"], "not_checked");
    assert_eq!(report["integrity"]["blob_scan_complete"], false);
    assert!(!root.path().join("cache").exists());
    let conn = health_fixture(root.path());
    conn.execute("UPDATE store_metadata SET value = 99", [])
        .unwrap();
    drop(conn);
    let store = root.path().join("cache/mfb_store.sqlite");
    let before = fs::read(&store).unwrap();
    let (output, report) = health(root.path());
    assert!(!output.status.success());
    assert_eq!(report["integrity"]["status"], "error");
    assert_eq!(report["integrity"]["issues"][0]["code"], "schema");
    assert_eq!(report["integrity"]["checked_protected_rows"], 0);
    assert_eq!(fs::read(&store).unwrap(), before);
    fs::write(&store, b"not a SQLite database; retain these bytes").unwrap();
    let before = fs::read(&store).unwrap();
    let (output, report) = health(root.path());
    assert!(!output.status.success());
    assert_eq!(report["integrity"]["status"], "error");
    assert_eq!(fs::read(&store).unwrap(), before);
}

#[test]
fn integrity_cli_includes_committed_wal_rows_and_preserves_main_and_wal_bytes() {
    let root = tempfile::tempdir().unwrap();
    let conn = health_fixture(root.path());
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    conn.execute(
        "UPDATE blob_store SET payload_blake3 = zeroblob(32) WHERE namespace = 'processed'",
        [],
    )
    .unwrap();
    let store = root.path().join("cache/mfb_store.sqlite");
    let wal = root.path().join("cache/mfb_store.sqlite-wal");
    let before = fs::read(&store).unwrap();
    let wal_before = fs::read(&wal).unwrap();
    assert!(!wal_before.is_empty());
    let (output, report) = health(root.path());
    assert!(!output.status.success());
    assert_eq!(report["integrity"]["corrupt_protected_rows"], 1);
    assert_eq!(report["integrity"]["checked_protected_rows"], 3);
    assert_eq!(fs::read(&store).unwrap(), before);
    assert_eq!(fs::read(&wal).unwrap(), wal_before);
    drop(conn);
}

#[test]
fn integrity_cli_reports_foreign_key_violations_without_exposing_values() {
    let root = tempfile::tempdir().unwrap();
    let conn = health_fixture(root.path());
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF;
        CREATE TABLE parent (id INTEGER PRIMARY KEY);
        CREATE TABLE child (parent_id INTEGER REFERENCES parent(id));
        INSERT INTO child VALUES (1);",
    )
    .unwrap();
    drop(conn);
    let (output, report) = health(root.path());
    assert!(!output.status.success());
    assert_eq!(report["integrity"]["status"], "unhealthy");
    assert_eq!(report["integrity"]["foreign_keys"], "failed");
    assert_eq!(report["integrity"]["sqlite_integrity"], "ok");
    assert_eq!(report["integrity"]["issue_count"], 1);
}

#[cfg(unix)]
#[test]
fn integrity_cli_rejects_symlinked_stores_without_following_them() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    drop(health_fixture(outside.path()));
    fs::create_dir(root.path().join("cache")).unwrap();
    let store = outside.path().join("cache/mfb_store.sqlite");
    let before = fs::read(&store).unwrap();
    std::os::unix::fs::symlink(&store, root.path().join("cache/mfb_store.sqlite")).unwrap();
    let (output, report) = health(root.path());
    assert!(!output.status.success());
    assert_eq!(report["integrity"]["status"], "error");
    assert_eq!(report["integrity"]["issues"][0]["code"], "managed_paths");
    assert_eq!(fs::read(&store).unwrap(), before);
}
