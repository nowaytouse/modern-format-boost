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
