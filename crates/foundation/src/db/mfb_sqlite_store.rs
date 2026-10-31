//! Shared local SQLite storage for reconstructable cache and durable run state.
//! Only the path-tree namespace may be discarded on integrity failure.

use anyhow::{Context, Result};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::runtime_config::CachePolicy;

pub const STORE_SCHEMA_VERSION: i32 = 4;
pub const NS_PATH_TREE: &str = "path_tree";
pub const NS_CHECKPOINT: &str = "checkpoint";
pub const NS_PROCESSED: &str = "processed";

const STORE_FILE_NAME: &str = "mfb_store.sqlite";
const STORE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const FAST_IMG_STATE_SCHEMA: &str = "
    CREATE TABLE fast_img_tasks (
        task_key TEXT NOT NULL PRIMARY KEY,
        working_copy TEXT NOT NULL UNIQUE,
        source_root TEXT NOT NULL,
        header_json BLOB NOT NULL,
        header_blake3 BLOB NOT NULL CHECK(length(header_blake3) = 32),
        snapshot_blake3 BLOB NOT NULL CHECK(length(snapshot_blake3) = 32),
        revision INTEGER NOT NULL CHECK(revision >= 0),
        updated_at INTEGER NOT NULL CHECK(updated_at >= 0)
    ) STRICT;
    CREATE INDEX idx_fast_img_tasks_source ON fast_img_tasks(source_root, working_copy);
    CREATE TABLE fast_img_source_outcomes (
        task_key TEXT NOT NULL REFERENCES fast_img_tasks(task_key) ON DELETE CASCADE,
        source_rel TEXT NOT NULL,
        disposition TEXT NOT NULL CHECK(disposition IN ('converted', 'skipped', 'failed')),
        source_blake3 TEXT NOT NULL,
        output_rel TEXT,
        output_blake3 TEXT,
        library_blake3 TEXT,
        reason TEXT,
        PRIMARY KEY(task_key, source_rel)
    ) STRICT;
    CREATE TABLE fast_img_photos_requests (
        task_key TEXT NOT NULL REFERENCES fast_img_tasks(task_key) ON DELETE CASCADE,
        request_key TEXT NOT NULL,
        backend TEXT NOT NULL CHECK(backend IN ('native', 'applescript')),
        native_journal_path TEXT,
        native_request_blake3 TEXT,
        applescript_request_id TEXT,
        library_path TEXT,
        PRIMARY KEY(task_key, request_key),
        CHECK((backend = 'native' AND native_journal_path IS NOT NULL
                   AND native_request_blake3 IS NOT NULL AND applescript_request_id IS NULL)
               OR (backend = 'applescript' AND applescript_request_id IS NOT NULL
                   AND native_journal_path IS NULL AND native_request_blake3 IS NULL))
    ) STRICT;
    CREATE UNIQUE INDEX idx_fast_img_native_journal
        ON fast_img_photos_requests(task_key, native_journal_path);
    CREATE TABLE fast_img_photos_entries (
        task_key TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
        request_key TEXT NOT NULL,
        kind TEXT NOT NULL CHECK(kind IN ('encoded', 'original')),
        source_rel TEXT NOT NULL,
        source_blake3 TEXT NOT NULL,
        output_rel TEXT NOT NULL,
        output_blake3 TEXT NOT NULL,
        album_name TEXT NOT NULL,
        PRIMARY KEY(task_key, request_key, output_rel),
        FOREIGN KEY(task_key, request_key) REFERENCES fast_img_photos_requests(task_key, request_key)
            ON DELETE CASCADE
    ) STRICT;
    CREATE UNIQUE INDEX idx_fast_img_entries_ordinal ON fast_img_photos_entries(task_key, ordinal);
    CREATE TABLE fast_img_photos_receipts (
        task_key TEXT NOT NULL REFERENCES fast_img_tasks(task_key) ON DELETE CASCADE,
        receipt_kind TEXT NOT NULL CHECK(receipt_kind IN ('primary', 'original')),
        rel_path TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
        blake3 TEXT NOT NULL,
        sync_status TEXT NOT NULL,
        quarantined INTEGER NOT NULL CHECK(quarantined IN (0, 1)),
        photos_uuid TEXT,
        library_blake3 TEXT,
        xmp_sidecar_blake3 TEXT,
        PRIMARY KEY(task_key, receipt_kind, rel_path)
    ) STRICT;
    CREATE UNIQUE INDEX idx_fast_img_receipts_ordinal
        ON fast_img_photos_receipts(task_key, receipt_kind, ordinal);
    CREATE TABLE fast_img_migrations (
        origin_path TEXT NOT NULL PRIMARY KEY,
        origin_blake3 BLOB NOT NULL CHECK(length(origin_blake3) = 32),
        source_json BLOB NOT NULL,
        task_key TEXT NOT NULL REFERENCES fast_img_tasks(task_key) ON DELETE RESTRICT,
        retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0, 1))
    ) STRICT;
    CREATE INDEX idx_fast_img_migrations_task ON fast_img_migrations(task_key);
";

const FAST_IMG_DIRECTORY_SNAPSHOT_SCHEMA: &str = "
    ALTER TABLE fast_img_migrations ADD COLUMN directory_snapshot BLOB;
    ALTER TABLE fast_img_migrations ADD COLUMN directory_snapshot_blake3 BLOB
        CONSTRAINT fast_img_directory_snapshot_pair CHECK(
            (directory_snapshot IS NULL AND directory_snapshot_blake3 IS NULL)
            OR (typeof(directory_snapshot) = 'blob'
                AND typeof(directory_snapshot_blake3) = 'blob'
                AND length(directory_snapshot_blake3) = 32)
        );
";

// SQLite exposes CHECK constraints through the stored DDL rather than a pragma.
// Require the exact suffix emitted by our v3-to-v4 ALTER, ignoring ASCII layout
// and keyword case. A pair of unprotected columns is not a recognized v4 store.
const FAST_IMG_DIRECTORY_SNAPSHOT_SCHEMA_SUFFIX: &str = "
    directory_snapshot BLOB,
    directory_snapshot_blake3 BLOB
        CONSTRAINT fast_img_directory_snapshot_pair CHECK(
            (directory_snapshot IS NULL AND directory_snapshot_blake3 IS NULL)
            OR (typeof(directory_snapshot) = 'blob'
                AND typeof(directory_snapshot_blake3) = 'blob'
                AND length(directory_snapshot_blake3) = 32)
        )
    ) STRICT
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathTreeMissReason {
    Absent,
    IdleTtl,
    FutureTimestamp,
    SchemaMismatch,
    IntegrityMismatch,
}

impl PathTreeMissReason {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Absent => "record_absent",
            Self::IdleTtl => "idle_ttl_expired",
            Self::FutureTimestamp => "future_timestamp",
            Self::SchemaMismatch => "stored_schema_mismatch",
            Self::IntegrityMismatch => "payload_blake3_mismatch",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PathTreeLookup<T> {
    Hit(T),
    Miss(PathTreeMissReason),
}

impl<T> PathTreeLookup<T> {
    pub(crate) fn into_option(self) -> Option<T> {
        match self {
            Self::Hit(value) => Some(value),
            Self::Miss(_) => None,
        }
    }
}

struct BlobRecord {
    schema: i32,
    payload: Option<Vec<u8>>,
    stored_blake3: Option<Vec<u8>>,
    updated_at: i64,
}

#[cfg(test)]
thread_local! {
    static TEST_STORE_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    static TEST_STORE_BINDING: std::cell::RefCell<Option<StorePathBinding>> = const { std::cell::RefCell::new(None) };
}

#[derive(Debug, PartialEq, Eq)]
struct StoreFileIdentity {
    canonical: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl StoreFileIdentity {
    fn inspect_metadata(path: &Path, directory: bool) -> Result<Option<fs::Metadata>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect formal store path {}", path.display()));
            }
        };
        anyhow::ensure!(
            !metadata.file_type().is_symlink()
                && if directory {
                    metadata.is_dir()
                } else {
                    metadata.is_file()
                },
            "formal store {} is not a regular non-symlink {}",
            path.display(),
            if directory { "directory" } else { "file" }
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                directory || metadata.nlink() == 1,
                "formal store file has multiple hard links; state retained: {}",
                path.display()
            );
        }
        Ok(Some(metadata))
    }

    fn inspect(path: &Path, directory: bool) -> Result<Option<Self>> {
        let Some(metadata) = Self::inspect_metadata(path, directory)? else {
            return Ok(None);
        };
        let canonical = path
            .canonicalize()
            .with_context(|| format!("resolve formal store path {}", path.display()))?;
        let resolved = fs::symlink_metadata(&canonical)
            .with_context(|| format!("recheck formal store path {}", path.display()))?;
        anyhow::ensure!(
            !resolved.file_type().is_symlink()
                && if directory {
                    resolved.is_dir()
                } else {
                    resolved.is_file()
                },
            "formal store path changed type while resolving: {}",
            path.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                metadata.dev() == resolved.dev()
                    && metadata.ino() == resolved.ino()
                    && (directory || resolved.nlink() == 1),
                "formal store path changed identity while resolving: {}",
                path.display()
            );
        }
        Ok(Some(Self {
            canonical,
            #[cfg(unix)]
            device: {
                use std::os::unix::fs::MetadataExt;
                metadata.dev()
            },
            #[cfg(unix)]
            inode: {
                use std::os::unix::fs::MetadataExt;
                metadata.ino()
            },
        }))
    }
}

struct StorePathBinding {
    path: PathBuf,
    parent: PathBuf,
    directory: Option<StoreFileIdentity>,
    database: Option<StoreFileIdentity>,
}

impl StorePathBinding {
    fn new(path: &Path) -> Result<Self> {
        anyhow::ensure!(path.is_absolute(), "formal store path must be absolute");
        let parent = path
            .parent()
            .context("formal store has no parent directory")?
            .to_path_buf();
        Ok(Self {
            path: path.to_path_buf(),
            parent,
            directory: None,
            database: None,
        })
    }

    fn directory_ready(&mut self, write: bool) -> Result<bool> {
        let mut current = StoreFileIdentity::inspect(&self.parent, true)?;
        if let Some(expected) = &self.directory {
            anyhow::ensure!(
                current.as_ref() == Some(expected),
                "bound formal store directory is missing or replaced; refusing another state store: {}",
                self.parent.display()
            );
        } else {
            if current.is_none() && write {
                fs::create_dir_all(&self.parent).with_context(|| {
                    format!("create formal store directory {}", self.parent.display())
                })?;
                current = StoreFileIdentity::inspect(&self.parent, true)?;
            }
            if let Some(identity) = current {
                self.directory = Some(identity);
            } else {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn inspect_database(&self) -> Result<Option<StoreFileIdentity>> {
        let current = StoreFileIdentity::inspect(&self.path, false)?;
        if let Some(expected) = &self.database {
            anyhow::ensure!(
                current.as_ref() == Some(expected),
                "bound formal database is missing or replaced; state was not recreated: {}",
                self.path.display()
            );
        }
        Ok(current)
    }

    fn inspect_sidecars(&self) -> Result<bool> {
        let mut present = false;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut name = self.path.as_os_str().to_os_string();
            name.push(suffix);
            // SQLite may legitimately retire/recreate sidecars between opens.
            // Check their observed type/link count without pinning their inode
            // or requiring a second lookup of an already removed sidecar.
            present |= StoreFileIdentity::inspect_metadata(Path::new(&name), false)?.is_some();
        }
        Ok(present)
    }

    fn open(&mut self, write: bool) -> Result<Option<Connection>> {
        if !self.directory_ready(write)? {
            return Ok(None);
        }
        let mut before = self.inspect_database()?;
        let sidecars = self.inspect_sidecars()?;
        if before.is_none() && sidecars {
            // A concurrent first writer may have created the main file between
            // these observations. Recheck it instead of misclassifying a live
            // initialization as an orphaned journal.
            before = self.inspect_database()?;
        }
        anyhow::ensure!(
            before.is_some() || !sidecars,
            "formal database is absent but SQLite sidecar evidence remains; state was not recreated: {}",
            self.path.display()
        );
        if before.is_none() && !write {
            return Ok(None);
        }
        let mode = if write {
            let mode = OpenFlags::SQLITE_OPEN_READ_WRITE;
            if before.is_none() {
                mode | OpenFlags::SQLITE_OPEN_CREATE
            } else {
                mode
            }
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let connection = Connection::open_with_flags(
            &self.path,
            mode | OpenFlags::SQLITE_OPEN_NOFOLLOW | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("open formal SQLite store {}", self.path.display()))?;
        // Validate the observed path again before init_schema can issue WAL,
        // recovery migration, or DDL. These are before/after path identities,
        // not a claim that SQLite exposed its opened OS descriptor to us.
        anyhow::ensure!(
            self.directory_ready(false)?,
            "formal store directory disappeared while opening"
        );
        let after = self
            .inspect_database()?
            .context("formal database disappeared while opening")?;
        if let Some(before) = before {
            anyhow::ensure!(
                before == after,
                "formal database changed while opening; schema was not initialized: {}",
                self.path.display()
            );
        }
        let _ = self.inspect_sidecars()?;
        self.database = Some(after);
        connection
            .busy_timeout(STORE_BUSY_TIMEOUT)
            .context("Failed to set SQLite busy timeout")?;
        if !write {
            connection.pragma_update(None, "query_only", true)?;
        }
        Ok(Some(connection))
    }
}

static STORE_PATH_BINDING: Mutex<Option<StorePathBinding>> = Mutex::new(None);

fn open_bound_store(path: &Path, write: bool) -> Result<Option<Connection>> {
    let operation = |binding: &mut Option<StorePathBinding>| {
        if binding.is_none() {
            *binding = Some(StorePathBinding::new(path)?);
        }
        let bound = binding
            .as_mut()
            .context("formal store path binding is missing")?;
        anyhow::ensure!(
            bound.path == path,
            "formal store path changed after binding"
        );
        bound.open(write)
    };
    #[cfg(test)]
    if TEST_STORE_PATH.with(|path| path.borrow().is_some()) {
        return TEST_STORE_BINDING.with(|binding| operation(&mut binding.borrow_mut()));
    }
    let mut binding = STORE_PATH_BINDING.lock().map_err(|_| {
        anyhow::anyhow!("formal store path binding lock is poisoned; state retained")
    })?;
    operation(&mut binding)
}

fn now_unix_secs() -> Result<i64> {
    let secs = crate::media_conversion_gate::unix_epoch_secs_optional()
        .context("blob_store updated_at: system clock before UNIX epoch")?;
    i64::try_from(secs).context("blob_store updated_at: epoch seconds exceeded i64")
}

/// Default path: `~/.modern_format_boost/cache/mfb_store.sqlite`.
///
/// # Errors
/// Returns an error if the durable application root cannot be bound for writing.
pub fn default_store_path() -> Result<PathBuf> {
    Ok(crate::process_lock::get_mfb_root()?
        .join("cache")
        .join(STORE_FILE_NAME))
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
        params![2],
    )?;
    tracing::debug!(
        migrated_rows,
        rejected_rows,
        "SQLite integrity migration prepared; pending transaction commit"
    );
    Ok(())
}

fn require_index(
    conn: &Connection,
    table: &str,
    index: &str,
    columns: &[&str],
    unique: bool,
) -> Result<()> {
    let properties: Option<(bool, bool)> = conn
        .query_row(
            "SELECT \"unique\", partial FROM pragma_index_list(?1) WHERE name = ?2",
            params![table, index],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    anyhow::ensure!(
        properties == Some((unique, false)),
        "SQLite store lacks required index {index} on {table}; database retained without repair"
    );
    let actual = conn
        .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")?
        .query_map([index], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(
        actual
            .iter()
            .map(String::as_str)
            .eq(columns.iter().copied()),
        "SQLite index {index} has unexpected columns; database retained without repair"
    );
    Ok(())
}

fn require_foreign_key(
    conn: &Connection,
    table: &str,
    parent: &str,
    columns: &[(&str, &str)],
    on_delete: &str,
) -> Result<()> {
    let actual = conn
        .prepare(
            "SELECT \"table\", \"from\", \"to\", on_delete, seq
             FROM pragma_foreign_key_list(?1) ORDER BY id, seq",
        )?
        .query_map([table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(
        actual.len() == columns.len()
            && actual.iter().zip(columns).enumerate().all(
                |(
                    sequence,
                    (
                        (actual_parent, from, to, action, actual_sequence),
                        (expected_from, expected_to),
                    ),
                )| {
                    actual_parent == parent
                        && from == expected_from
                        && to == expected_to
                        && action == on_delete
                        && usize::try_from(*actual_sequence).ok() == Some(sequence)
                },
            ),
        "SQLite state table {table} has incomplete foreign keys; database retained without repair"
    );
    Ok(())
}

fn validate_fast_img_state_schema(conn: &Connection, version: i32) -> Result<()> {
    let migration_columns = if version == 3 {
        &[
            "origin_path",
            "origin_blake3",
            "source_json",
            "task_key",
            "retired",
        ][..]
    } else {
        &[
            "origin_path",
            "origin_blake3",
            "source_json",
            "task_key",
            "retired",
            "directory_snapshot",
            "directory_snapshot_blake3",
        ][..]
    };
    for (table, expected) in [
        (
            "fast_img_tasks",
            &[
                "task_key",
                "working_copy",
                "source_root",
                "header_json",
                "header_blake3",
                "snapshot_blake3",
                "revision",
                "updated_at",
            ][..],
        ),
        (
            "fast_img_source_outcomes",
            &[
                "task_key",
                "source_rel",
                "disposition",
                "source_blake3",
                "output_rel",
                "output_blake3",
                "library_blake3",
                "reason",
            ][..],
        ),
        (
            "fast_img_photos_requests",
            &[
                "task_key",
                "request_key",
                "backend",
                "native_journal_path",
                "native_request_blake3",
                "applescript_request_id",
                "library_path",
            ][..],
        ),
        (
            "fast_img_photos_entries",
            &[
                "task_key",
                "ordinal",
                "request_key",
                "kind",
                "source_rel",
                "source_blake3",
                "output_rel",
                "output_blake3",
                "album_name",
            ][..],
        ),
        (
            "fast_img_photos_receipts",
            &[
                "task_key",
                "receipt_kind",
                "rel_path",
                "ordinal",
                "blake3",
                "sync_status",
                "quarantined",
                "photos_uuid",
                "library_blake3",
                "xmp_sidecar_blake3",
            ][..],
        ),
        ("fast_img_migrations", migration_columns),
    ] {
        let strict: Option<bool> = conn.query_row(
            "SELECT strict FROM pragma_table_list WHERE schema = 'main' AND name = ?1 AND type = 'table'",
            [table], |row| row.get(0),
        ).optional()?;
        anyhow::ensure!(
            strict == Some(true),
            "SQLite state table {table} is missing or not strict; database retained without repair"
        );
        let statement = conn.prepare(&format!("SELECT * FROM {table} LIMIT 0"))?;
        anyhow::ensure!(
            statement.column_names().eq(expected.iter().copied()),
            "SQLite state table {table} has unexpected columns; database retained without repair"
        );
    }
    for statement in [
        "INSERT INTO fast_img_tasks(task_key) VALUES (?1) ON CONFLICT(task_key) DO NOTHING",
        "INSERT INTO fast_img_tasks(working_copy) VALUES (?1) ON CONFLICT(working_copy) DO NOTHING",
        "INSERT INTO fast_img_source_outcomes(task_key, source_rel) VALUES (?1, ?2) ON CONFLICT(task_key, source_rel) DO NOTHING",
        "INSERT INTO fast_img_photos_requests(task_key, request_key) VALUES (?1, ?2) ON CONFLICT(task_key, request_key) DO NOTHING",
        "INSERT INTO fast_img_photos_entries(task_key, request_key, output_rel) VALUES (?1, ?2, ?3) ON CONFLICT(task_key, request_key, output_rel) DO NOTHING",
        "INSERT INTO fast_img_photos_receipts(task_key, receipt_kind, rel_path) VALUES (?1, ?2, ?3) ON CONFLICT(task_key, receipt_kind, rel_path) DO NOTHING",
        "INSERT INTO fast_img_migrations(origin_path) VALUES (?1) ON CONFLICT(origin_path) DO NOTHING",
    ] {
        conn.prepare(statement).context(
            "SQLite state table lacks required unique keys; database retained without repair",
        )?;
    }
    for (table, index, columns, unique) in [
        (
            "fast_img_tasks",
            "idx_fast_img_tasks_source",
            &["source_root", "working_copy"][..],
            false,
        ),
        (
            "fast_img_photos_requests",
            "idx_fast_img_native_journal",
            &["task_key", "native_journal_path"][..],
            true,
        ),
        (
            "fast_img_photos_entries",
            "idx_fast_img_entries_ordinal",
            &["task_key", "ordinal"][..],
            true,
        ),
        (
            "fast_img_photos_receipts",
            "idx_fast_img_receipts_ordinal",
            &["task_key", "receipt_kind", "ordinal"][..],
            true,
        ),
        (
            "fast_img_migrations",
            "idx_fast_img_migrations_task",
            &["task_key"][..],
            false,
        ),
    ] {
        require_index(conn, table, index, columns, unique)?;
    }
    for table in [
        "fast_img_source_outcomes",
        "fast_img_photos_requests",
        "fast_img_photos_receipts",
    ] {
        require_foreign_key(
            conn,
            table,
            "fast_img_tasks",
            &[("task_key", "task_key")],
            "CASCADE",
        )?;
    }
    require_foreign_key(
        conn,
        "fast_img_photos_entries",
        "fast_img_photos_requests",
        &[("task_key", "task_key"), ("request_key", "request_key")],
        "CASCADE",
    )?;
    require_foreign_key(
        conn,
        "fast_img_migrations",
        "fast_img_tasks",
        &[("task_key", "task_key")],
        "RESTRICT",
    )?;
    if version >= 4 {
        let sql: String = conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'fast_img_migrations'",
            [],
            |row| row.get(0),
        )?;
        let normalized = |sql: &str| {
            sql.chars()
                .filter(|character| !character.is_ascii_whitespace())
                .map(|character| character.to_ascii_lowercase())
                .collect::<String>()
        };
        anyhow::ensure!(
            normalized(&sql).ends_with(&normalized(FAST_IMG_DIRECTORY_SNAPSHOT_SCHEMA_SUFFIX)),
            "SQLite directory snapshot columns lack their paired evidence constraint; database retained without repair"
        );
    }
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
        matches!(version, 1 | 2 | 3 | STORE_SCHEMA_VERSION),
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
    if version >= 2 {
        require_index(
            conn,
            "blob_store",
            "idx_blob_ns_root",
            &["namespace", "root_path"],
            false,
        )?;
        require_index(
            conn,
            "blob_store",
            "idx_blob_ns_updated",
            &["namespace", "updated_at"],
            false,
        )?;
    }
    if version >= 3 {
        validate_fast_img_state_schema(conn, version)?;
    } else {
        let reserved: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema
                WHERE name GLOB 'fast_img_*' OR name GLOB 'idx_fast_img_*')",
            [],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            !reserved,
            "legacy SQLite store contains conflicting Fast IMG state objects; database retained without repair"
        );
    }
    Ok(Some(version))
}

/// Inspect a schema in the caller's read snapshot without creating or migrating storage.
///
/// # Errors
/// Rejects unknown, incomplete or conflicting schema evidence without repair.
pub(crate) fn inspect_store_schema_version(conn: &Connection) -> Result<Option<i32>> {
    schema_version(conn)
}

/// Require the complete current schema without issuing write pragmas or DDL.
///
/// # Errors
/// Rejects a missing, older, unknown or incomplete schema without migration.
pub(crate) fn validate_current_store_schema(conn: &Connection) -> Result<()> {
    anyhow::ensure!(
        schema_version(conn)? == Some(STORE_SCHEMA_VERSION),
        "structured Fast IMG state requires store schema {STORE_SCHEMA_VERSION}; database retained without migration"
    );
    Ok(())
}

/// Enable WAL with bounded lock retries after the caller has validated its schema.
///
/// # Errors
/// Returns an error if SQLite refuses WAL or the lock deadline expires.
pub fn enable_wal(conn: &Connection) -> Result<()> {
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
    let mut version = match schema_version(&tx)? {
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
                params![2],
            )?;
            2
        }
        Some(1) => {
            migrate_v1_crc32_to_blake3(&tx)?;
            2
        }
        Some(version) => version,
    };
    if version == 2 {
        // The v1 integrity conversion and this extension commit as one chain.
        // A collision or invalid legacy row rolls all schema/data changes back.
        anyhow::ensure!(
            schema_version(&tx)? == Some(2),
            "SQLite v2 migration precondition changed"
        );
        tx.execute_batch(FAST_IMG_STATE_SCHEMA)
            .context("Failed to create structured Fast IMG state tables; database retained")?;
        tx.execute(
            "UPDATE store_metadata SET value = ?1 WHERE key = 'schema_version'",
            params![3],
        )?;
        version = 3;
    }
    if version == 3 {
        anyhow::ensure!(
            schema_version(&tx)? == Some(3),
            "SQLite v3 migration precondition changed"
        );
        tx.execute_batch(FAST_IMG_DIRECTORY_SNAPSHOT_SCHEMA)
            .context("Failed to extend Fast IMG directory snapshot evidence; database retained")?;
        tx.execute(
            "UPDATE store_metadata SET value = ?1 WHERE key = 'schema_version'",
            params![STORE_SCHEMA_VERSION],
        )?;
    }
    validate_current_store_schema(&tx)?;
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

/// Resolve the same formal store path without creating its root or storage.
///
/// # Errors
/// Rejects an invalid, replaced or unavailable formal application root.
pub(crate) fn store_path_current_for_read() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_STORE_PATH.with(|path| path.borrow().clone()) {
        return Ok(path);
    }
    Ok(crate::process_lock::inspect_mfb_root_path()?
        .join("cache")
        .join(STORE_FILE_NAME))
}

fn open_connection() -> Result<Connection> {
    let path = store_path()?;
    let mut conn = open_bound_store(&path, true)?
        .context("formal writer did not open its selected database")?;
    init_schema(&mut conn)?;
    Ok(conn)
}

/// Open the fixed formal store for inspection, without creating a database,
/// creating directories, migrating schema, or selecting a fallback store.
/// An existing file with an unknown schema is returned for strict validation;
/// it is never converted into `None` or repaired here.
pub(crate) fn open_store_readonly() -> Result<Option<Connection>> {
    let path = store_path_current_for_read()?;
    open_bound_store(&path, false)
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

pub(crate) fn path_tree_lookup(
    cache_key: &str,
    expected_schema_version: i32,
) -> Result<PathTreeLookup<Vec<u8>>> {
    let mut conn = open_connection()?;
    path_tree_get_with_status(
        &mut conn,
        cache_key,
        expected_schema_version,
        cache_policy(),
        now_unix_secs,
    )
}

fn cache_policy() -> CachePolicy {
    crate::runtime_config::active().map_or_else(CachePolicy::default, |config| config.cache)
}

/// Committed removals from reconstructable directory snapshots only.
#[derive(Debug, Default, serde::Serialize)]
pub struct CachePruneResult {
    pub expired_rows: u64,
    pub evicted_rows: u64,
}

/// Logical payload eligibility in one read snapshot; not committed removals or disk savings.
#[derive(Debug, serde::Serialize)]
pub struct CachePrunePreview {
    pub schema_version: u8,
    pub snapshot_unix_secs: i64,
    pub cutoff_unix_secs: i64,
    pub policy: CachePolicy,
    pub total_rows: u64,
    pub total_payload_bytes: u64,
    /// Includes idle timestamps at/before the cutoff and timestamps after the snapshot clock.
    pub expired_rows: u64,
    pub expired_payload_bytes: u64,
    pub evicted_rows: u64,
    pub evicted_payload_bytes: u64,
    pub retained_rows: u64,
    pub retained_payload_bytes: u64,
}

/// Inspect reconstructable path-tree pruning without creating storage or changing any row.
/// Later pruning takes a fresh snapshot; concurrent writers may change its actual receipt.
///
/// # Errors
/// Returns policy, clock, schema, malformed row, SQLite or aggregate overflow errors.
pub fn preview_path_tree_pruning(
    snapshot: &Transaction<'_>,
    policy: CachePolicy,
) -> Result<CachePrunePreview> {
    preview_path_tree_at(snapshot, policy, now_unix_secs)
}

fn preview_path_tree_at(
    snapshot: &Transaction<'_>,
    policy: CachePolicy,
    clock: impl FnOnce() -> Result<i64>,
) -> Result<CachePrunePreview> {
    policy.validate()?;
    anyhow::ensure!(
        matches!(
            schema_version(snapshot)?,
            Some(2 | 3 | STORE_SCHEMA_VERSION)
        ),
        "cache pruning preview requires a recognized BLAKE3 store schema; database retained without migration"
    );
    let now = clock()?;
    anyhow::ensure!(now >= 0, "cache pruning preview clock is before UNIX epoch");
    let cutoff = now.saturating_sub(i64::try_from(policy.path_tree_ttl_seconds)?);
    let invalid: i64 = snapshot.query_row(
        "SELECT COUNT(*) FROM blob_store WHERE namespace = ?1 AND
         (typeof(cache_key) != 'text' OR typeof(updated_at) != 'integer' OR typeof(payload) != 'blob')",
        [NS_PATH_TREE],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        invalid == 0,
        "cache pruning preview found malformed path-tree rows; database retained"
    );
    let counts = snapshot.query_row(
        "WITH records AS (
             SELECT cache_key, updated_at, length(payload) AS payload_bytes,
                    updated_at <= ?2 OR updated_at > ?3 AS expired
             FROM blob_store WHERE namespace = ?1
         ), ranked AS (
             SELECT cache_key, SUM(payload_bytes) OVER (
                 ORDER BY updated_at DESC, cache_key DESC ROWS UNBOUNDED PRECEDING
             ) AS retained_bytes FROM records WHERE NOT expired
         ), classified AS (
             SELECT records.payload_bytes,
                    CASE WHEN records.expired THEN 0
                         WHEN ranked.retained_bytes > ?4 THEN 1 ELSE 2 END AS category
             FROM records LEFT JOIN ranked USING (cache_key)
         )
         SELECT COUNT(*), COALESCE(SUM(payload_bytes), 0),
                COUNT(CASE WHEN category = 0 THEN 1 END),
                COALESCE(SUM(CASE WHEN category = 0 THEN payload_bytes ELSE 0 END), 0),
                COUNT(CASE WHEN category = 1 THEN 1 END),
                COALESCE(SUM(CASE WHEN category = 1 THEN payload_bytes ELSE 0 END), 0),
                COUNT(CASE WHEN category = 2 THEN 1 END),
                COALESCE(SUM(CASE WHEN category = 2 THEN payload_bytes ELSE 0 END), 0)
         FROM classified",
        params![
            NS_PATH_TREE,
            cutoff,
            now,
            i64::try_from(policy.path_tree_max_bytes)?
        ],
        |row| {
            Ok([
                row.get::<_, i64>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ])
        },
    )?;
    let mut totals = [0_u64; 8];
    for (total, count) in totals.iter_mut().zip(counts) {
        *total = u64::try_from(count).context("negative cache pruning preview aggregate")?;
    }
    let [
        total_rows,
        total_payload_bytes,
        expired_rows,
        expired_payload_bytes,
        evicted_rows,
        evicted_payload_bytes,
        retained_rows,
        retained_payload_bytes,
    ] = totals;
    let sum = |values: [u64; 3]| {
        values
            .into_iter()
            .try_fold(0_u64, u64::checked_add)
            .context("cache pruning preview aggregate overflow")
    };
    anyhow::ensure!(
        total_rows == sum([expired_rows, evicted_rows, retained_rows])?
            && total_payload_bytes
                == sum([
                    expired_payload_bytes,
                    evicted_payload_bytes,
                    retained_payload_bytes
                ])?
            && retained_payload_bytes <= policy.path_tree_max_bytes,
        "cache pruning preview aggregates do not reconcile"
    );
    Ok(CachePrunePreview {
        schema_version: 1,
        snapshot_unix_secs: now,
        cutoff_unix_secs: cutoff,
        policy,
        total_rows,
        total_payload_bytes,
        expired_rows,
        expired_payload_bytes,
        evicted_rows,
        evicted_payload_bytes,
        retained_rows,
        retained_payload_bytes,
    })
}

/// Apply the idle TTL and payload budget to an already validated store.
/// Does not create/migrate storage or touch formal state and unknown namespaces.
///
/// # Errors
/// Returns invalid policy, clock or database errors; deletion is atomic.
pub fn prune_path_tree_cache(
    conn: &mut Connection,
    policy: CachePolicy,
) -> Result<CachePruneResult> {
    prune_path_tree_at(conn, policy, now_unix_secs)
}

fn prune_path_tree_at(
    conn: &mut Connection,
    policy: CachePolicy,
    clock: impl FnOnce() -> Result<i64>,
) -> Result<CachePruneResult> {
    policy.validate()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    anyhow::ensure!(
        schema_version(&tx)? == Some(STORE_SCHEMA_VERSION),
        "cache pruning requires the current store schema; database retained without migration"
    );
    let expired_rows = expire_path_trees(&tx, policy, clock()?)?;
    let evicted_rows = evict_path_trees(&tx, None, i64::try_from(policy.path_tree_max_bytes)?)?;
    tx.commit()?;
    Ok(CachePruneResult {
        expired_rows,
        evicted_rows,
    })
}

fn expire_path_trees(conn: &Connection, policy: CachePolicy, now: i64) -> Result<u64> {
    let cutoff = now.saturating_sub(i64::try_from(policy.path_tree_ttl_seconds)?);
    rows_affected_u64(conn.execute(
        "DELETE FROM blob_store WHERE namespace = ?1 AND (updated_at <= ?2 OR updated_at > ?3)",
        params![NS_PATH_TREE, cutoff, now],
    )?)
}

fn evict_path_trees(conn: &Connection, reserved_key: Option<&str>, budget: i64) -> Result<u64> {
    // Retain the newest prefix using lengths only; an admitted payload can reserve space.
    rows_affected_u64(conn.execute(
        "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key IN (
             SELECT cache_key FROM (
                 SELECT cache_key, SUM(length(payload)) OVER (
                     ORDER BY updated_at DESC, cache_key DESC ROWS UNBOUNDED PRECEDING
                 ) AS retained_bytes
                 FROM blob_store WHERE namespace = ?1 AND (?2 IS NULL OR cache_key != ?2)
             ) WHERE retained_bytes > ?3
         )",
        params![NS_PATH_TREE, reserved_key, budget],
    )?)
}

fn path_tree_get(
    conn: &mut Connection,
    cache_key: &str,
    expected_schema_version: i32,
    policy: CachePolicy,
    clock: impl FnOnce() -> Result<i64>,
) -> Result<Option<Vec<u8>>> {
    path_tree_get_with_status(conn, cache_key, expected_schema_version, policy, clock)
        .map(PathTreeLookup::into_option)
}

fn path_tree_get_with_status(
    conn: &mut Connection,
    cache_key: &str,
    expected_schema_version: i32,
    policy: CachePolicy,
    clock: impl FnOnce() -> Result<i64>,
) -> Result<PathTreeLookup<Vec<u8>>> {
    policy.validate()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = clock()?;
    let cutoff = now.saturating_sub(i64::try_from(policy.path_tree_ttl_seconds)?);
    let record = read_blob_record(&tx, NS_PATH_TREE, cache_key, Some((cutoff, now)))?;
    let outcome = if let Some(record) = record {
        let expiry = if record.updated_at > now {
            Some(PathTreeMissReason::FutureTimestamp)
        } else if record.updated_at <= cutoff {
            Some(PathTreeMissReason::IdleTtl)
        } else {
            None
        };
        if let Some(reason) = expiry {
            // Future timestamps cannot establish freshness after a backwards clock adjustment.
            tx.execute(
                "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key = ?2
                 AND (updated_at <= ?3 OR updated_at > ?4)",
                params![NS_PATH_TREE, cache_key, cutoff, now],
            )?;
            PathTreeLookup::Miss(reason)
        } else {
            validate_blob_record(
                &tx,
                NS_PATH_TREE,
                cache_key,
                expected_schema_version,
                record,
            )?
        }
    } else {
        PathTreeLookup::Miss(PathTreeMissReason::Absent)
    };
    if matches!(outcome, PathTreeLookup::Hit(_)) {
        tx.execute(
            "UPDATE blob_store SET updated_at = ?1 WHERE namespace = ?2 AND cache_key = ?3",
            params![now, NS_PATH_TREE, cache_key],
        )?;
    }
    tx.commit()?;
    if matches!(
        outcome,
        PathTreeLookup::Miss(PathTreeMissReason::IdleTtl | PathTreeMissReason::FutureTimestamp)
    ) {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_runtime",
            "SQLITE AUDIT: expired path_tree cache entry removed",
        );
    }
    Ok(outcome)
}

pub(crate) fn read_blob(
    conn: &Connection,
    namespace: &str,
    cache_key: &str,
    expected_schema_version: i32,
) -> Result<Option<Vec<u8>>> {
    let Some(record) = read_blob_record(conn, namespace, cache_key, None)? else {
        return Ok(None);
    };
    validate_blob_record(conn, namespace, cache_key, expected_schema_version, record)
        .map(PathTreeLookup::into_option)
}

fn read_blob_record(
    conn: &Connection,
    namespace: &str,
    cache_key: &str,
    freshness_window: Option<(i64, i64)>,
) -> Result<Option<BlobRecord>> {
    let (cutoff, now) = freshness_window.unzip();
    // Reuse this row fetch for expiry diagnostics without materializing stale payloads.
    conn.query_row(
        "SELECT schema_version,
             CASE WHEN ?3 IS NULL OR (updated_at > ?3 AND updated_at <= ?4) THEN payload END,
             CASE WHEN ?3 IS NULL OR (updated_at > ?3 AND updated_at <= ?4) THEN payload_blake3 END,
             updated_at FROM blob_store
             WHERE namespace = ?1 AND cache_key = ?2",
        params![namespace, cache_key, cutoff, now],
        |row| {
            Ok(BlobRecord {
                schema: row.get(0)?,
                payload: row.get(1)?,
                stored_blake3: row.get(2)?,
                updated_at: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn validate_blob_record(
    conn: &Connection,
    namespace: &str,
    cache_key: &str,
    expected_schema_version: i32,
    record: BlobRecord,
) -> Result<PathTreeLookup<Vec<u8>>> {
    let BlobRecord {
        schema,
        payload,
        stored_blake3,
        ..
    } = record;
    let payload = payload.context("SQLite eligible record has no payload")?;
    let stored_blake3 = stored_blake3.context("SQLite eligible record has no payload digest")?;
    let expected_blake3 = blake3::hash(&payload);
    let invalid = if schema != expected_schema_version {
        Some((PathTreeMissReason::SchemaMismatch, "schema mismatch"))
    } else if stored_blake3.as_slice() != expected_blake3.as_bytes() {
        Some((PathTreeMissReason::IntegrityMismatch, "BLAKE3 mismatch"))
    } else {
        None
    };
    if let Some((disposition, reason)) = invalid {
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
        return Ok(PathTreeLookup::Miss(disposition));
    }
    Ok(PathTreeLookup::Hit(payload))
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
    let expired = expire_path_trees(&tx, policy, now)?;
    upsert_blob(
        &tx,
        NS_PATH_TREE,
        cache_key,
        schema_version,
        root,
        payload,
        now,
    )?;
    let evicted = evict_path_trees(&tx, Some(cache_key), max_bytes - payload_bytes)?;
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

pub(crate) fn upsert_blob(
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
        TEST_STORE_BINDING.with(|binding| *binding.borrow_mut() = None);
    }
}

/// Point blob I/O at a temporary store for unit tests.
#[cfg(test)]
pub fn set_test_store_path_for_tests(path: PathBuf) -> TestStoreGuard {
    TEST_STORE_PATH.with(|p| {
        *p.borrow_mut() = Some(path);
    });
    TEST_STORE_BINDING.with(|binding| *binding.borrow_mut() = None);
    TestStoreGuard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formal_store_readonly_absence_and_unknown_schema_never_initialize_state() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("absent/cache/store.sqlite");
        let _guard = set_test_store_path_for_tests(path.clone());
        assert!(open_store_readonly()?.is_none());
        assert!(fs::read_dir(root.path())?.next().is_none());
        fs::create_dir_all(path.parent().unwrap())?;
        let foreign = Connection::open(&path)?;
        foreign.execute_batch(
            "CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('retain');",
        )?;
        drop(foreign);
        let before = fs::read(&path)?;
        let reader = open_store_readonly()?.expect("an unknown existing database is not absent");
        assert!(inspect_store_schema_version(&reader).is_err());
        let query_only: i32 = reader.pragma_query_value(None, "query_only", |row| row.get(0))?;
        assert_eq!(query_only, 1);
        drop(reader);
        assert!(open_connection().is_err());
        assert_eq!(fs::read(&path)?, before);
        assert_eq!(fs::read_dir(path.parent().unwrap())?.count(), 1);
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn formal_store_bound_database_cannot_disappear_or_be_replaced() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("cache/store.sqlite");
        let retained = root.path().join("retained.sqlite");
        let _guard = set_test_store_path_for_tests(path.clone());
        drop(open_connection()?);
        fs::rename(&path, &retained)?;
        assert!(open_store_readonly().is_err());
        assert!(open_connection().is_err());
        assert!(
            !path.exists(),
            "a bound missing database must not be recreated"
        );
        fs::copy(&retained, &path)?;
        let before = fs::read(&path)?;
        assert!(open_store_readonly().is_err());
        assert!(open_connection().is_err());
        assert_eq!(fs::read(&path)?, before);
        assert!(retained.is_file());
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn formal_store_bound_cache_directory_cannot_be_recreated_or_replaced() -> Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("cache");
        let path = directory.join("store.sqlite");
        let retained = root.path().join("retained-cache");
        let _guard = set_test_store_path_for_tests(path.clone());
        drop(open_connection()?);
        fs::rename(&directory, &retained)?;
        assert!(open_store_readonly().is_err());
        assert!(open_connection().is_err());
        assert!(!directory.exists());
        fs::create_dir(&directory)?;
        let unrelated = Connection::open(&path)?;
        unrelated.execute_batch("CREATE TABLE unrelated(value TEXT);")?;
        drop(unrelated);
        let before = fs::read(&path)?;
        assert!(open_store_readonly().is_err());
        assert!(open_connection().is_err());
        assert_eq!(fs::read(&path)?, before);
        assert!(retained.join("store.sqlite").is_file());
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn formal_store_rejects_symlink_and_hardlink_targets_before_schema_writes() -> Result<()> {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir()?;
        let external = root.path().join("unrelated");
        fs::create_dir(&external)?;
        let original = external.join("store.sqlite");
        fs::write(&original, b"not an MFB database; preserve these bytes")?;
        let alias = root.path().join("cache-alias");
        symlink(&external, &alias)?;
        {
            let _guard = set_test_store_path_for_tests(alias.join("store.sqlite"));
            assert!(format!("{:#}", open_connection().unwrap_err()).contains("non-symlink"));
            assert!(open_store_readonly().is_err());
        }
        for hardlink in [false, true] {
            let cache = root.path().join(if hardlink {
                "hardlink-cache"
            } else {
                "symlink-cache"
            });
            fs::create_dir(&cache)?;
            let path = cache.join("store.sqlite");
            if hardlink {
                fs::hard_link(&original, &path)?;
            } else {
                symlink(&original, &path)?;
            }
            let _guard = set_test_store_path_for_tests(path);
            let error = format!("{:#}", open_connection().unwrap_err());
            assert!(
                error.contains(if hardlink {
                    "hard links"
                } else {
                    "non-symlink"
                }),
                "{error}"
            );
            assert!(open_store_readonly().is_err());
        }
        let cache = root.path().join("sidecar-cache");
        fs::create_dir(&cache)?;
        let path = cache.join("store.sqlite");
        symlink(&original, cache.join("store.sqlite-wal"))?;
        let _guard = set_test_store_path_for_tests(path.clone());
        assert!(open_connection().is_err());
        assert!(!path.exists());
        assert_eq!(
            fs::read(original)?,
            b"not an MFB database; preserve these bytes"
        );
        Ok(())
    }

    #[test]
    fn formal_store_binding_allows_normal_in_place_commits() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("cache/store.sqlite");
        let _guard = set_test_store_path_for_tests(path);
        blob_put(NS_CHECKPOINT, "one", 1, None, b"first")?;
        blob_put(NS_CHECKPOINT, "one", 1, None, b"second")?;
        let reader = open_store_readonly()?.expect("committed state must remain present");
        assert_eq!(
            read_blob(&reader, NS_CHECKPOINT, "one", 1)?.as_deref(),
            Some(b"second".as_slice())
        );
        Ok(())
    }

    #[test]
    fn formal_store_orphaned_sqlite_sidecars_are_not_empty_state() -> Result<()> {
        let root = tempfile::tempdir()?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let directory = root.path().join(suffix);
            fs::create_dir(&directory)?;
            let path = directory.join("store.sqlite");
            let sidecar = directory.join(format!("store.sqlite{suffix}"));
            fs::write(&sidecar, b"retained state evidence")?;
            let _guard = set_test_store_path_for_tests(path.clone());
            assert!(open_store_readonly().is_err());
            assert!(open_connection().is_err());
            assert!(!path.exists());
            assert_eq!(fs::read(sidecar)?, b"retained state evidence");
        }
        Ok(())
    }

    #[test]
    fn pruning_preview_is_read_only_and_matches_the_actual_ttl_and_lru_receipt() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(STORE_FILE_NAME);
        let _guard = set_test_store_path_for_tests(path.clone());
        let conn = open_connection()?;
        for (key, timestamp, payload) in [
            ("expired", 90, b"ee".as_slice()),
            ("future", 101, b"fff".as_slice()),
            ("older", 91, b"oooo".as_slice()),
            ("newest", 100, b"nnnnn".as_slice()),
            ("tie-a", 99, b"ta".as_slice()),
            ("tie-z", 99, b"tz".as_slice()),
        ] {
            upsert_blob(&conn, NS_PATH_TREE, key, 3, None, payload, timestamp)?;
        }
        for namespace in [NS_CHECKPOINT, NS_PROCESSED, "unknown"] {
            upsert_blob(&conn, namespace, "protected", 2, None, b"proof", 0)?;
        }
        conn.pragma_update(None, "journal_mode", "delete")?;
        drop(conn);
        let before = std::fs::read(&path)?;
        let reader =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        reader.pragma_update(None, "query_only", true)?;
        let policy = CachePolicy {
            path_tree_max_bytes: 7,
            path_tree_ttl_seconds: 10,
        };
        let snapshot = reader.unchecked_transaction()?;
        let preview = preview_path_tree_at(&snapshot, policy, || Ok(100))?;
        assert_eq!((preview.total_rows, preview.total_payload_bytes), (6, 18));
        assert_eq!(
            (preview.expired_rows, preview.expired_payload_bytes),
            (2, 5)
        );
        assert_eq!(
            (preview.evicted_rows, preview.evicted_payload_bytes),
            (2, 6)
        );
        assert_eq!(
            (preview.retained_rows, preview.retained_payload_bytes),
            (2, 7)
        );
        assert_eq!(
            (preview.snapshot_unix_secs, preview.cutoff_unix_secs),
            (100, 90)
        );
        assert!(snapshot.execute("DELETE FROM blob_store", []).is_err());
        snapshot.commit()?;
        drop(reader);
        assert_eq!(std::fs::read(&path)?, before);
        let mut writer = Connection::open(&path)?;
        let removed = prune_path_tree_at(&mut writer, policy, || Ok(100))?;
        assert_eq!(
            (removed.expired_rows, removed.evicted_rows),
            (preview.expired_rows, preview.evicted_rows)
        );
        assert_eq!(
            read_blob(&writer, NS_PATH_TREE, "tie-z", 3)?,
            Some(b"tz".to_vec())
        );
        assert_eq!(read_blob(&writer, NS_PATH_TREE, "tie-a", 3)?, None);
        for namespace in [NS_CHECKPOINT, NS_PROCESSED, "unknown"] {
            assert_eq!(
                read_blob(&writer, namespace, "protected", 2)?,
                Some(b"proof".to_vec())
            );
        }
        Ok(())
    }

    #[test]
    fn pruning_preview_empty_clock_and_zero_payload_boundaries_do_not_refresh_rows() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut conn = Connection::open(directory.path().join(STORE_FILE_NAME))?;
        init_schema(&mut conn)?;
        let policy = CachePolicy {
            path_tree_max_bytes: 1,
            path_tree_ttl_seconds: 10,
        };
        let snapshot = conn.unchecked_transaction()?;
        let empty = preview_path_tree_at(&snapshot, policy, || Ok(1))?;
        assert_eq!(
            (
                empty.total_rows,
                empty.total_payload_bytes,
                empty.cutoff_unix_secs
            ),
            (0, 0, -9)
        );
        snapshot.commit()?;
        for (key, timestamp, payload) in [
            ("empty", 1, b"".as_slice()),
            ("future", 2, b"f".as_slice()),
            ("oversized", 0, b"xx".as_slice()),
        ] {
            upsert_blob(&conn, NS_PATH_TREE, key, 3, None, payload, timestamp)?;
        }
        conn.pragma_update(None, "query_only", true)?;
        let snapshot = conn.unchecked_transaction()?;
        let preview = preview_path_tree_at(&snapshot, policy, || Ok(1))?;
        assert_eq!((preview.total_rows, preview.total_payload_bytes), (3, 3));
        assert_eq!(
            (preview.expired_rows, preview.expired_payload_bytes),
            (1, 1)
        );
        assert_eq!(
            (preview.evicted_rows, preview.evicted_payload_bytes),
            (1, 2)
        );
        assert_eq!(
            (preview.retained_rows, preview.retained_payload_bytes),
            (1, 0)
        );
        assert_eq!(
            snapshot.query_row(
                "SELECT updated_at FROM blob_store WHERE cache_key='empty'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            1
        );
        assert!(
            preview_path_tree_at(
                &snapshot,
                CachePolicy {
                    path_tree_ttl_seconds: 0,
                    ..policy
                },
                || Ok(1)
            )
            .is_err()
        );
        assert!(preview_path_tree_at(&snapshot, policy, || Ok(-1)).is_err());
        assert!(
            preview_path_tree_at(&snapshot, policy, || anyhow::bail!(
                "injected clock failure"
            ))
            .is_err()
        );
        assert_eq!(
            snapshot.query_row("SELECT COUNT(*) FROM blob_store", [], |row| row
                .get::<_, i64>(0))?,
            3
        );
        snapshot.commit()?;
        Ok(())
    }

    #[test]
    fn pruning_preview_rejects_unknown_schema_and_malformed_rows_without_repair() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut conn = Connection::open(directory.path().join(STORE_FILE_NAME))?;
        init_schema(&mut conn)?;
        upsert_blob(&conn, NS_PATH_TREE, "malformed", 3, None, b"keep", 100)?;
        conn.execute("UPDATE blob_store SET payload='not a blob'", [])?;
        conn.pragma_update(None, "query_only", true)?;
        let snapshot = conn.unchecked_transaction()?;
        assert!(preview_path_tree_at(&snapshot, CachePolicy::default(), || Ok(100)).is_err());
        assert_eq!(
            snapshot.query_row("SELECT payload FROM blob_store", [], |row| row
                .get::<_, String>(0))?,
            "not a blob"
        );
        snapshot.commit()?;
        conn.pragma_update(None, "query_only", false)?;
        conn.execute(
            "UPDATE blob_store SET payload=X'0102', updated_at='not an integer'",
            [],
        )?;
        let snapshot = conn.unchecked_transaction()?;
        assert!(preview_path_tree_at(&snapshot, CachePolicy::default(), || Ok(100)).is_err());
        snapshot.commit()?;
        conn.execute(
            "UPDATE store_metadata SET value=999 WHERE key='schema_version'",
            [],
        )?;
        let snapshot = conn.unchecked_transaction()?;
        assert!(preview_path_tree_at(&snapshot, CachePolicy::default(), || Ok(100)).is_err());
        assert_eq!(
            snapshot.query_row(
                "SELECT value FROM store_metadata WHERE key='schema_version'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            999
        );
        assert_eq!(
            snapshot.query_row("SELECT COUNT(*) FROM blob_store", [], |row| row
                .get::<_, i64>(0))?,
            1
        );
        snapshot.commit()?;
        let empty = Connection::open_in_memory()?;
        let snapshot = empty.unchecked_transaction()?;
        assert!(preview_path_tree_at(&snapshot, CachePolicy::default(), || Ok(100)).is_err());
        assert_eq!(
            snapshot.query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
        snapshot.commit()?;
        Ok(())
    }

    #[test]
    fn path_tree_lookup_reports_exact_miss_reasons_and_failed_mutations() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let _guard = set_test_store_path_for_tests(dir.path().join(STORE_FILE_NAME));
        let mut conn = open_connection()?;
        let policy = CachePolicy {
            path_tree_max_bytes: 100,
            path_tree_ttl_seconds: 10,
        };
        for (key, timestamp, schema) in [
            ("expired", 90, 1),
            ("future", 101, 1),
            ("schema", 100, 2),
            ("digest", 100, 1),
            ("hit", 99, 1),
        ] {
            upsert_blob(&conn, NS_PATH_TREE, key, schema, None, b"cache", timestamp)?;
            upsert_blob(&conn, NS_CHECKPOINT, key, 1, None, b"state", 0)?;
        }
        conn.execute(
            "UPDATE blob_store SET payload_blake3 = X'00' WHERE namespace = ?1 AND cache_key = 'digest'",
            [NS_PATH_TREE],
        )?;
        for (key, expected) in [
            ("absent", PathTreeMissReason::Absent),
            ("expired", PathTreeMissReason::IdleTtl),
            ("future", PathTreeMissReason::FutureTimestamp),
            ("schema", PathTreeMissReason::SchemaMismatch),
            ("digest", PathTreeMissReason::IntegrityMismatch),
        ] {
            assert_eq!(
                path_tree_get_with_status(&mut conn, key, 1, policy, || Ok(100))?,
                PathTreeLookup::Miss(expected),
            );
        }
        assert_eq!(
            path_tree_get_with_status(&mut conn, "hit", 1, policy, || Ok(100))?,
            PathTreeLookup::Hit(b"cache".to_vec()),
        );
        assert_eq!(
            conn.query_row(
                "SELECT updated_at FROM blob_store WHERE namespace = ?1 AND cache_key = 'hit'",
                [NS_PATH_TREE],
                |row| row.get::<_, i64>(0),
            )?,
            100,
        );
        for key in ["expired", "future", "schema", "digest", "hit"] {
            assert_eq!(
                read_blob(&conn, NS_CHECKPOINT, key, 1)?,
                Some(b"state".to_vec())
            );
        }
        upsert_blob(&conn, NS_PATH_TREE, "expired", 1, None, b"keep", 0)?;
        conn.execute_batch(
            "CREATE TRIGGER reject_lookup_delete BEFORE DELETE ON blob_store
             WHEN OLD.namespace = 'path_tree' BEGIN SELECT RAISE(ABORT, 'injected delete failure'); END;",
        )?;
        assert!(path_tree_get_with_status(&mut conn, "expired", 1, policy, || Ok(100)).is_err());
        assert_eq!(
            read_blob(&conn, NS_PATH_TREE, "expired", 1)?,
            Some(b"keep".to_vec())
        );
        conn.execute_batch(
            "DROP TRIGGER reject_lookup_delete;
             CREATE TRIGGER reject_lookup_touch BEFORE UPDATE ON blob_store
             WHEN OLD.namespace = 'path_tree' BEGIN SELECT RAISE(ABORT, 'injected touch failure'); END;",
        )?;
        assert!(path_tree_get_with_status(&mut conn, "hit", 1, policy, || Ok(101)).is_err());
        assert_eq!(
            read_blob(&conn, NS_PATH_TREE, "hit", 1)?,
            Some(b"cache".to_vec())
        );
        Ok(())
    }

    #[test]
    fn explicit_pruning_counts_each_removal_once_and_rolls_back_on_failure() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let _guard = set_test_store_path_for_tests(dir.path().join(STORE_FILE_NAME));
        let mut conn = open_connection()?;
        let policy = CachePolicy {
            path_tree_max_bytes: 4,
            path_tree_ttl_seconds: 10,
        };
        for (key, time) in [
            ("expired", 90),
            ("future", 101),
            ("older", 91),
            ("newest", 100),
        ] {
            upsert_blob(&conn, NS_PATH_TREE, key, 3, None, b"data", time)?;
        }
        for namespace in [NS_CHECKPOINT, NS_PROCESSED, "unknown"] {
            upsert_blob(&conn, namespace, "protected", 2, None, b"proof", 0)?;
        }
        conn.execute_batch("CREATE TRIGGER reject_eviction BEFORE DELETE ON blob_store
            WHEN OLD.cache_key = 'older' BEGIN SELECT RAISE(ABORT, 'injected eviction failure'); END;")?;
        assert!(prune_path_tree_at(&mut conn, policy, || Ok(100)).is_err());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM blob_store", [], |r| r
                .get::<_, i64>(0))?,
            7
        );
        conn.execute_batch("DROP TRIGGER reject_eviction")?;
        let result = prune_path_tree_at(&mut conn, policy, || Ok(100))?;
        assert_eq!((result.expired_rows, result.evicted_rows), (2, 1));
        assert_eq!(
            read_blob(&conn, NS_PATH_TREE, "newest", 3)?,
            Some(b"data".to_vec())
        );
        for namespace in [NS_CHECKPOINT, NS_PROCESSED, "unknown"] {
            assert_eq!(
                read_blob(&conn, namespace, "protected", 2)?,
                Some(b"proof".to_vec())
            );
        }
        let repeated = prune_path_tree_at(&mut conn, policy, || Ok(100))?;
        assert_eq!((repeated.expired_rows, repeated.evicted_rows), (0, 0));
        assert!(
            prune_path_tree_at(&mut conn, policy, || anyhow::bail!("clock unavailable")).is_err()
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM blob_store", [], |r| r
                .get::<_, i64>(0))?,
            4
        );
        conn.execute("UPDATE store_metadata SET value = 99", [])?;
        assert!(prune_path_tree_at(&mut conn, policy, || Ok(200)).is_err());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM blob_store", [], |r| r
                .get::<_, i64>(0))?,
            4
        );
        Ok(())
    }

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

    fn seed_v2_store(store: &Path) {
        seed_v1_store(store, NS_PATH_TREE);
        let mut conn = Connection::open(store).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        migrate_v1_crc32_to_blake3(&tx).unwrap();
        tx.commit().unwrap();
        assert_eq!(schema_version(&conn).unwrap(), Some(2));
    }

    fn seed_v3_store(store: &Path) -> Result<()> {
        seed_v2_store(store);
        let mut conn = Connection::open(store)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(FAST_IMG_STATE_SCHEMA)?;
        tx.execute(
            "UPDATE store_metadata SET value=3 WHERE key='schema_version'",
            [],
        )?;
        tx.commit()?;
        assert_eq!(schema_version(&conn)?, Some(3));
        Ok(())
    }

    fn seed_fast_img_schema_rows(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "INSERT INTO fast_img_tasks VALUES('task', '/synthetic/out', '/synthetic/source',
                X'7B7D', zeroblob(32), zeroblob(32), 1, 1);
             INSERT INTO fast_img_source_outcomes VALUES('task', 'retained.jpg', 'failed', '',
                NULL, NULL, NULL, 'retained source');
             INSERT INTO fast_img_photos_requests VALUES('task', 'request', 'applescript',
                NULL, NULL, 'request-id', NULL);
             INSERT INTO fast_img_photos_entries VALUES('task', 0, 'request', 'encoded',
                'retained.jpg', '', 'retained.JXL', '', 'Synthetic');
             INSERT INTO fast_img_photos_receipts VALUES('task', 'primary', 'verified.JXL', 0,
                '', 'photos_local', 0, 'synthetic-uuid', NULL, NULL);
             INSERT INTO fast_img_migrations(origin_path, origin_blake3, source_json, task_key, retired)
                VALUES('/synthetic/old.json', zeroblob(32), X'7B7D', 'task', 0);",
        )?;
        Ok(())
    }

    #[test]
    fn schema4_v3_readonly_inspection_and_preview_do_not_migrate() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = root.path().join(STORE_FILE_NAME);
        seed_v3_store(&store)?;
        let conn = Connection::open(&store)?;
        seed_fast_img_schema_rows(&conn)?;
        upsert_blob(&conn, NS_PATH_TREE, "cache", 1, None, b"cache", 1)?;
        drop(conn);
        let before = std::fs::read(&store)?;
        let mut reader = Connection::open_with_flags(&store, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        reader.pragma_update(None, "query_only", true)?;
        let snapshot = reader.transaction()?;
        assert_eq!(inspect_store_schema_version(&snapshot)?, Some(3));
        assert!(validate_current_store_schema(&snapshot).is_err());
        let preview = preview_path_tree_at(&snapshot, CachePolicy::default(), || Ok(100))?;
        assert_eq!((preview.total_rows, preview.total_payload_bytes), (1, 5));
        assert!(
            snapshot
                .prepare("SELECT directory_snapshot FROM fast_img_migrations")
                .is_err()
        );
        snapshot.commit()?;
        drop(reader);
        assert_eq!(std::fs::read(&store)?, before);
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(
                !root
                    .path()
                    .join(format!("{STORE_FILE_NAME}{suffix}"))
                    .exists()
            );
        }
        Ok(())
    }

    #[test]
    fn schema4_migrates_v3_without_inventing_directory_snapshots() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = root.path().join(STORE_FILE_NAME);
        seed_v3_store(&store)?;
        let conn = Connection::open(&store)?;
        seed_fast_img_schema_rows(&conn)?;
        drop(conn);
        let _guard = set_test_store_path_for_tests(store);
        let conn = open_connection()?;
        assert_eq!(inspect_store_schema_version(&conn)?, Some(4));
        for table in [
            "fast_img_tasks",
            "fast_img_source_outcomes",
            "fast_img_photos_requests",
            "fast_img_photos_entries",
            "fast_img_photos_receipts",
            "fast_img_migrations",
        ] {
            let rows: i64 =
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            assert_eq!(rows, 1, "schema4 migration changed state rows: {table}");
        }
        let backup = conn.query_row(
            "SELECT origin_blake3,source_json,directory_snapshot,directory_snapshot_blake3
             FROM fast_img_migrations",
            [],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                ))
            },
        )?;
        assert_eq!(backup, (vec![0; 32], b"{}".to_vec(), None, None));
        assert_eq!(
            read_blob(&conn, NS_CHECKPOINT, "legacy", 7)?,
            Some(b"resume-state".to_vec())
        );
        Ok(())
    }

    #[test]
    fn schema4_extension_failure_rolls_back_every_prior_version() -> Result<()> {
        for version in [1, 2, 3] {
            let root = tempfile::tempdir()?;
            let store = root.path().join(STORE_FILE_NAME);
            match version {
                1 => seed_v1_store(&store, NS_PATH_TREE),
                2 => seed_v2_store(&store),
                3 => seed_v3_store(&store)?,
                _ => unreachable!(),
            }
            let conn = Connection::open(&store)?;
            if version == 3 {
                seed_fast_img_schema_rows(&conn)?;
            }
            let original_rows: i64 =
                conn.query_row("SELECT count(*) FROM blob_store", [], |row| row.get(0))?;
            conn.execute_batch(
                "CREATE TRIGGER reject_snapshot_extension BEFORE UPDATE OF value ON store_metadata
                 WHEN NEW.value=4 BEGIN SELECT RAISE(ABORT, 'synthetic snapshot migration failure'); END;",
            )?;
            drop(conn);
            let _guard = set_test_store_path_for_tests(store.clone());
            let error = open_connection().unwrap_err();
            assert!(format!("{error:#}").contains("synthetic snapshot migration failure"));
            let conn = Connection::open(&store)?;
            assert_eq!(inspect_store_schema_version(&conn)?, Some(version));
            let rows: i64 =
                conn.query_row("SELECT count(*) FROM blob_store", [], |row| row.get(0))?;
            assert_eq!(rows, original_rows);
            let added_columns: i64 = conn.query_row(
                "SELECT count(*) FROM pragma_table_info('fast_img_migrations')
                 WHERE name IN ('directory_snapshot','directory_snapshot_blake3')",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(added_columns, 0);
            if version == 3 {
                let raw: Vec<u8> =
                    conn.query_row("SELECT source_json FROM fast_img_migrations", [], |row| {
                        row.get(0)
                    })?;
                assert_eq!(raw, b"{}");
            } else {
                let tables: i64 = conn.query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'fast_img_*'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(tables, 0);
            }
            conn.execute_batch("DROP TRIGGER reject_snapshot_extension")?;
            validate_current_store_schema(&open_connection()?)?;
        }
        Ok(())
    }

    #[test]
    fn schema4_directory_snapshot_pair_requires_both_blobs_and_complete_schema() -> Result<()> {
        let root = tempfile::tempdir()?;
        let _guard = set_test_store_path_for_tests(root.path().join(STORE_FILE_NAME));
        let conn = open_connection()?;
        seed_fast_img_schema_rows(&conn)?;
        for mutation in [
            "directory_snapshot=X'7B7D'",
            "directory_snapshot_blake3=zeroblob(32)",
            "directory_snapshot=X'7B7D', directory_snapshot_blake3=zeroblob(31)",
            "directory_snapshot='{}', directory_snapshot_blake3=zeroblob(32)",
            "directory_snapshot=X'7B7D', directory_snapshot_blake3='text'",
        ] {
            assert!(
                conn.execute(&format!("UPDATE fast_img_migrations SET {mutation}"), [])
                    .is_err(),
                "{mutation}"
            );
            let empty: bool = conn.query_row("SELECT directory_snapshot IS NULL AND directory_snapshot_blake3 IS NULL FROM fast_img_migrations", [], |row| row.get(0))?;
            assert!(
                empty,
                "rejected evidence pair changed its prior state: {mutation}"
            );
        }
        conn.execute(
            "UPDATE fast_img_migrations SET directory_snapshot=?1,directory_snapshot_blake3=?2",
            params![b"{}", blake3::hash(b"{}").as_bytes()],
        )?;
        validate_current_store_schema(&conn)?;
        conn.execute(
            "UPDATE fast_img_migrations SET directory_snapshot=NULL,directory_snapshot_blake3=NULL",
            [],
        )?;
        for complete_unprotected_pair in [false, true] {
            let fixture = tempfile::tempdir()?;
            let store = fixture.path().join(STORE_FILE_NAME);
            seed_v3_store(&store)?;
            let legacy = Connection::open(&store)?;
            legacy.execute_batch(
                "ALTER TABLE fast_img_migrations ADD COLUMN directory_snapshot BLOB",
            )?;
            if complete_unprotected_pair {
                legacy.execute_batch("ALTER TABLE fast_img_migrations ADD COLUMN directory_snapshot_blake3 BLOB; UPDATE store_metadata SET value=4 WHERE key='schema_version'")?;
            }
            assert!(inspect_store_schema_version(&legacy).is_err());
            drop(legacy);
            let before = std::fs::read(&store)?;
            let _fixture_guard = set_test_store_path_for_tests(store.clone());
            assert!(open_connection().is_err());
            assert_eq!(std::fs::read(&store)?, before);
        }
        Ok(())
    }

    #[test]
    fn v2_store_reads_without_migration_and_extends_state_without_losing_payloads() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = root.path().join(STORE_FILE_NAME);
        seed_v2_store(&store);
        let before = std::fs::read(&store)?;
        let readonly =
            Connection::open_with_flags(&store, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        assert_eq!(inspect_store_schema_version(&readonly)?, Some(2));
        assert!(validate_current_store_schema(&readonly).is_err());
        drop(readonly);
        assert_eq!(std::fs::read(&store)?, before);
        let _guard = set_test_store_path_for_tests(store.clone());
        assert_eq!(store_path_current_for_read()?, store);
        let conn = open_connection()?;
        validate_current_store_schema(&conn)?;
        assert_eq!(schema_version(&conn)?, Some(STORE_SCHEMA_VERSION));
        assert_eq!(
            blob_get(NS_CHECKPOINT, "legacy", 7)?,
            Some(b"resume-state".to_vec())
        );
        Ok(())
    }

    #[test]
    fn normalized_fast_img_state_and_migration_backup_are_outside_cache_cleanup() -> Result<()> {
        let root = tempfile::tempdir()?;
        let _guard = set_test_store_path_for_tests(root.path().join(STORE_FILE_NAME));
        let mut conn = open_connection()?;
        seed_fast_img_schema_rows(&conn)?;
        blob_put(NS_PATH_TREE, "cache", 1, None, b"rebuildable")?;
        blob_delete_namespace(NS_PATH_TREE)?;
        prune_path_tree_cache(&mut conn, CachePolicy::default())?;
        for table in [
            "fast_img_tasks",
            "fast_img_source_outcomes",
            "fast_img_photos_requests",
            "fast_img_photos_entries",
            "fast_img_photos_receipts",
            "fast_img_migrations",
        ] {
            let count: i64 =
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            assert_eq!(count, 1, "cache cleanup touched formal state {table}");
        }
        let backup: Vec<u8> =
            conn.query_row("SELECT source_json FROM fast_img_migrations", [], |row| {
                row.get(0)
            })?;
        assert_eq!(backup, b"{}");
        assert!(
            conn.execute("DELETE FROM fast_img_tasks WHERE task_key='task'", [])
                .is_err(),
            "a migration's only legacy backup must not disappear through parent replacement"
        );
        Ok(())
    }

    #[test]
    fn incomplete_current_fast_img_schema_is_rejected_without_repair() -> Result<()> {
        for mutation in [
            "DROP TABLE fast_img_tasks",
            "DROP TABLE fast_img_source_outcomes",
            "DROP TABLE fast_img_photos_requests",
            "DROP TABLE fast_img_photos_entries",
            "DROP TABLE fast_img_photos_receipts",
            "DROP TABLE fast_img_migrations",
            "DROP INDEX idx_fast_img_tasks_source",
            "DROP INDEX idx_fast_img_native_journal",
            "DROP INDEX idx_fast_img_entries_ordinal",
            "DROP INDEX idx_fast_img_receipts_ordinal",
            "DROP INDEX idx_fast_img_migrations_task",
            "DROP INDEX idx_blob_ns_root",
            "DROP INDEX idx_blob_ns_updated",
        ] {
            let root = tempfile::tempdir()?;
            let store = root.path().join(STORE_FILE_NAME);
            let _guard = set_test_store_path_for_tests(store.clone());
            let conn = open_connection()?;
            conn.execute_batch(mutation)?;
            assert!(validate_current_store_schema(&conn).is_err(), "{mutation}");
            drop(conn);
            let before = std::fs::read(&store)?;
            assert!(open_connection().is_err(), "{mutation}");
            assert_eq!(
                std::fs::read(&store)?,
                before,
                "invalid schema was repaired: {mutation}"
            );
        }
        Ok(())
    }

    #[test]
    fn legacy_reserved_state_objects_are_not_treated_as_an_empty_family() -> Result<()> {
        for version in [1, 2] {
            for reserved in [
                "CREATE TABLE fast_img_tasks(value TEXT)",
                "CREATE VIEW fast_img_photos_entries AS SELECT cache_key FROM blob_store",
                "CREATE INDEX idx_fast_img_foreign ON blob_store(cache_key)",
            ] {
                let root = tempfile::tempdir()?;
                let store = root.path().join(STORE_FILE_NAME);
                if version == 1 {
                    seed_v1_store(&store, NS_PATH_TREE);
                } else {
                    seed_v2_store(&store);
                }
                let conn = Connection::open(&store)?;
                conn.execute_batch(reserved)?;
                assert!(inspect_store_schema_version(&conn).is_err());
                drop(conn);
                let before = std::fs::read(&store)?;
                let _guard = set_test_store_path_for_tests(store.clone());
                assert!(open_connection().is_err());
                assert_eq!(std::fs::read(&store)?, before);
            }
        }
        Ok(())
    }

    #[test]
    fn structured_extension_failure_rolls_back_the_entire_version_chain() -> Result<()> {
        for version in [1, 2] {
            let root = tempfile::tempdir()?;
            let store = root.path().join(STORE_FILE_NAME);
            if version == 1 {
                seed_v1_store(&store, NS_PATH_TREE);
            } else {
                seed_v2_store(&store);
            }
            let conn = Connection::open(&store)?;
            conn.execute_batch(
                "CREATE TRIGGER reject_structured_extension BEFORE UPDATE OF value ON store_metadata
                 WHEN NEW.value = 3 BEGIN SELECT RAISE(ABORT, 'synthetic state migration failure'); END;",
            )?;
            let original_rows: i64 =
                conn.query_row("SELECT count(*) FROM blob_store", [], |row| row.get(0))?;
            drop(conn);
            let _guard = set_test_store_path_for_tests(store.clone());
            let error = open_connection().unwrap_err();
            assert!(format!("{error:#}").contains("synthetic state migration failure"));
            let conn = Connection::open(&store)?;
            assert_eq!(schema_version(&conn)?, Some(version));
            let rows: i64 =
                conn.query_row("SELECT count(*) FROM blob_store", [], |row| row.get(0))?;
            assert_eq!(rows, original_rows);
            let state_tables: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'fast_img_*'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(state_tables, 0);
            conn.execute_batch("DROP TRIGGER reject_structured_extension")?;
            let reopened = open_connection()?;
            validate_current_store_schema(&reopened)?;
            assert_eq!(
                blob_get(NS_CHECKPOINT, "legacy", 7)?,
                Some(b"resume-state".to_vec())
            );
        }
        Ok(())
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
        for version in [0, 1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join(STORE_FILE_NAME);
            if version == 1 {
                seed_v1_store(&store, NS_PATH_TREE);
            } else if version == 2 {
                seed_v2_store(&store);
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
            if version != 0 {
                assert_eq!(
                    blob_get(NS_CHECKPOINT, "legacy", 7).unwrap().as_deref(),
                    Some(&b"resume-state"[..])
                );
            }
        }
    }
}
